//! `forged` is TermForge's authenticated local session host. It owns PTYs,
//! bounded replay, project tasks and the SQLite store so work survives UI and
//! daemon restarts (`docs/adr/0005-process-model.md`).

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use clap::{Parser, Subcommand};
use tf_engine::{GridSize, TerminalEngine};
use tf_ipc::{ConnectionHandler, IpcServer, ServerConnection};
use tf_proto::{
    BlockedKind as ProtoBlockedKind, ClientMsg, CreateSession, Event, ProgramState as ProtoState,
    ProgramStatus, ProtoError, Request, Response, ServerMsg, SessionId, SessionInfo, TaskId,
    TaskInfo, TaskState as ProtoTaskState, TermSize, WorkingDirectory, PROTOCOL_VERSION,
};
use tf_session::{LiveSession, SessionEvent, SpawnOptions, Waker};
use tf_store::{RestorableSession, Store, TaskRecord, TaskState as StoreTaskState};

const MAX_REPLAY_BYTES: usize = 8 * 1024 * 1024;
const MAX_REPLAY_EVENTS: usize = 8_192;
const AUTH_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Parser)]
#[command(name = "forged", version, about = "TermForge session host")]
struct Cli {
    /// Override the data directory.
    #[arg(long, env = "TERMFORGE_DATA_DIR")]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the session host (default).
    Run,
    /// Print environment diagnostics.
    Doctor,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("TERMFORGE_LOG")
                .unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let data_dir = match cli.data_dir {
        Some(d) => d,
        None => directories::ProjectDirs::from("dev", "TermForge", "TermForge")
            .context("could not resolve a data directory")?
            .data_local_dir()
            .to_path_buf(),
    };
    std::fs::create_dir_all(&data_dir).with_context(|| format!("create {}", data_dir.display()))?;

    match cli.command.unwrap_or(Command::Run) {
        Command::Run => run(&data_dir),
        Command::Doctor => doctor(&data_dir),
    }
}

fn run(data_dir: &std::path::Path) -> Result<()> {
    let store = tf_store::Store::open(&data_dir.join("termforge.db"))?;
    let integration_dir = data_dir.join("shell");
    tf_shell::install(&integration_dir)?;
    let token = tf_ipc::token::ensure_token().context("create IPC authentication token")?;
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        protocol = tf_proto::PROTOCOL_VERSION,
        schema = store.schema_version()?,
        data = %data_dir.display(),
        "forged started"
    );
    let store = Arc::new(Mutex::new(store));
    let host = Arc::new(SessionHost::new(integration_dir, store));
    host.restore_sessions();
    let handler = Arc::new(ForgedHandler { host, token });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("create forged runtime")?;
    runtime.block_on(async move {
        let mut server = tf_ipc::server().context("create IPC server")?;
        let address = server.address().to_owned();
        server.start(handler).await.context("start IPC server")?;
        tracing::info!(%address, "forged IPC server listening");
        std::future::pending::<()>().await;
        #[allow(unreachable_code)]
        Ok(())
    })
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Debug)]
struct HostedSession {
    session: Arc<LiveSession>,
    cwd: Arc<Mutex<Option<WorkingDirectory>>>,
    ssh_host: Option<String>,
    project_root: Option<PathBuf>,
    task: Option<TaskId>,
    size: TermSize,
    log: Arc<Mutex<EventLog>>,
}

#[derive(Debug, Default)]
struct EventLog {
    entries: VecDeque<(u64, Event)>,
    next: u64,
    output_bytes: usize,
}

impl EventLog {
    fn push(&mut self, event: Event) {
        let bytes = event_output_len(&event);
        self.entries.push_back((self.next, event));
        self.next = self.next.wrapping_add(1);
        self.output_bytes += bytes;
        while self.output_bytes > MAX_REPLAY_BYTES || self.entries.len() > MAX_REPLAY_EVENTS {
            let Some((_, removed)) = self.entries.pop_front() else {
                break;
            };
            self.output_bytes = self.output_bytes.saturating_sub(event_output_len(&removed));
        }
    }

    fn snapshot(&self) -> (u64, Vec<Event>) {
        (
            self.next,
            self.entries
                .iter()
                .map(|(_, event)| event.clone())
                .collect(),
        )
    }

    fn since(&self, cursor: u64) -> (u64, Vec<Event>) {
        let offset = self
            .entries
            .front()
            .map(|(first, _)| cursor.saturating_sub(*first) as usize)
            .unwrap_or(0)
            .min(self.entries.len());
        (
            self.next,
            self.entries
                .iter()
                .skip(offset)
                .map(|(_, event)| event.clone())
                .collect(),
        )
    }
}

fn event_output_len(event: &Event) -> usize {
    match event {
        Event::Output { data, .. } => data.len(),
        _ => 0,
    }
}

#[derive(Debug)]
struct SessionHost {
    sessions: Mutex<HashMap<SessionId, HostedSession>>,
    integration_dir: PathBuf,
    store: Arc<Mutex<Store>>,
}

impl SessionHost {
    fn new(integration_dir: PathBuf, store: Arc<Mutex<Store>>) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            integration_dir,
            store,
        }
    }

    fn restore_sessions(&self) {
        let records = match lock(&self.store).restorable_sessions() {
            Ok(records) => records,
            Err(error) => {
                tracing::warn!(%error, "could not load restorable sessions");
                return;
            }
        };
        for record in records {
            if let Err(error) = lock(&self.store).remove_restorable_session(&record.id) {
                tracing::warn!(%error, session = %record.id, "could not retire old session record");
                continue;
            }
            let create = CreateSession {
                profile: Some(record.profile),
                ssh_host: record.ssh_host,
                cwd: record.cwd.filter(|path| path.is_dir()),
                project_root: record.project_root.filter(|path| path.is_dir()),
                task: record.task_id.and_then(|id| id.parse().ok()),
                size: TermSize {
                    cols: record.cols,
                    rows: record.rows,
                },
            };
            match self.create(create) {
                Ok(id) => tracing::info!(%id, "restored project shell after daemon restart"),
                Err(error) => tracing::warn!(%error, "could not restore project shell"),
            }
        }
    }

    fn handle(&self, request: Request) -> Result<Response, ProtoError> {
        match request {
            Request::Ping => Ok(Response::Pong),
            Request::ListSessions => Ok(Response::Sessions(self.list())),
            Request::ListTasks { project_root } => self.list_tasks(&project_root),
            Request::CreateTask(create) => self.create_task(create),
            Request::UpdateTask(update) => self.update_task(update),
            Request::CreateSession(create) => self.create(create).map(Response::Created),
            Request::Write { session, data } => {
                let live = self.session(session)?;
                live.write(&data).map_err(internal)?;
                Ok(Response::Ok)
            }
            Request::Resize { session, size } => {
                let live = self.session(session)?;
                live.resize(GridSize::new(size.cols.max(2), size.rows.max(1)))
                    .map_err(internal)?;
                if let Some(entry) = lock(&self.sessions).get_mut(&session) {
                    entry.size = size;
                }
                self.persist(session)?;
                Ok(Response::Ok)
            }
            Request::Close { session } => {
                lock(&self.sessions)
                    .remove(&session)
                    .ok_or(ProtoError::SessionNotFound(session))?;
                lock(&self.store)
                    .remove_restorable_session(&session.to_string())
                    .map_err(internal)?;
                Ok(Response::Ok)
            }
            Request::SaveWorkspaceState {
                project_root,
                state,
            } => {
                let store = lock(&self.store);
                let project = store.open_project(&project_root).map_err(internal)?;
                store
                    .save_workspace_state(project.id, &state)
                    .map_err(internal)?;
                Ok(Response::Ok)
            }
            Request::LoadWorkspaceState { project_root } => {
                let store = lock(&self.store);
                let project = store.open_project(&project_root).map_err(internal)?;
                store
                    .load_workspace_state(project.id)
                    .map(Response::WorkspaceState)
                    .map_err(internal)
            }
            Request::Subscribe { .. } | Request::Unsubscribe { .. } => Err(ProtoError::Internal(
                "subscriptions are handled by the connection".into(),
            )),
        }
    }

    fn create(&self, create: CreateSession) -> Result<SessionId, ProtoError> {
        let profile = match create.ssh_host.as_deref() {
            Some(host) => tf_pty::ssh_profile(host).ok_or_else(|| {
                ProtoError::Internal(format!("invalid SSH host or ssh not found: {host}"))
            })?,
            None => resolve_profile(create.profile.as_deref())?,
        };
        let launch_cwd = create
            .ssh_host
            .is_none()
            .then_some(create.cwd.as_deref())
            .flatten();
        if let Some(cwd) = launch_cwd {
            if !cwd.is_dir() {
                return Err(ProtoError::Internal(format!(
                    "working directory does not exist: {}",
                    cwd.display()
                )));
            }
        }
        let project_root = create
            .project_root
            .as_deref()
            .map(dunce::canonicalize)
            .transpose()
            .map_err(internal)?;
        if let Some(task) = create.task {
            let root = project_root.as_deref().ok_or_else(|| {
                ProtoError::Internal("a task-bound session requires a project root".into())
            })?;
            let belongs_to_project = lock(&self.store)
                .tasks_for_project(root)
                .map_err(internal)?
                .iter()
                .any(|candidate| candidate.id == task.to_string());
            if !belongs_to_project {
                return Err(ProtoError::Internal(format!(
                    "task {task} does not belong to this project"
                )));
            }
        }
        let size = TermSize {
            cols: create.size.cols.max(2),
            rows: create.size.rows.max(1),
        };
        let (wake_tx, wake_rx) = std::sync::mpsc::channel();
        let wake: Waker = Arc::new(move || {
            let _ = wake_tx.send(());
        });
        let session = LiveSession::spawn(
            SpawnOptions {
                profile,
                ssh_host: create.ssh_host.clone(),
                cwd: launch_cwd.map(PathBuf::from),
                size: GridSize::new(size.cols, size.rows),
                env: tool_env(),
                integration_dir: Some(self.integration_dir.clone()),
            },
            wake,
        )
        .map_err(internal)?;
        let id = SessionId::new();
        let session = Arc::new(session);
        let cwd = Arc::new(Mutex::new(launch_cwd.map(|path| WorkingDirectory {
            host: None,
            path: path.to_string_lossy().into_owned(),
        })));
        let log = Arc::new(Mutex::new(EventLog::default()));
        lock(&self.sessions).insert(
            id,
            HostedSession {
                session: Arc::clone(&session),
                cwd: Arc::clone(&cwd),
                ssh_host: create.ssh_host.clone(),
                project_root,
                task: create.task,
                size,
                log: Arc::clone(&log),
            },
        );
        if let Err(error) = self.persist(id) {
            lock(&self.sessions).remove(&id);
            return Err(error);
        }
        start_session_monitor(
            id,
            Arc::downgrade(&session),
            Arc::clone(&cwd),
            create.ssh_host.is_some(),
            Arc::clone(&log),
            Arc::clone(&self.store),
            wake_rx,
        );
        Ok(id)
    }

    fn persist(&self, id: SessionId) -> Result<(), ProtoError> {
        let sessions = lock(&self.sessions);
        let entry = sessions.get(&id).ok_or(ProtoError::SessionNotFound(id))?;
        let record = RestorableSession {
            id: id.to_string(),
            project_root: entry.project_root.clone(),
            profile: entry.session.profile().name.clone(),
            ssh_host: entry.ssh_host.clone(),
            cwd: lock(&entry.cwd)
                .as_ref()
                .and_then(|cwd| local_cwd(entry.ssh_host.is_some(), cwd)),
            cols: entry.size.cols,
            rows: entry.size.rows,
            task_id: entry.task.map(|task| task.to_string()),
        };
        drop(sessions);
        lock(&self.store)
            .save_restorable_session(&record)
            .map_err(internal)
    }

    fn session(&self, id: SessionId) -> Result<Arc<LiveSession>, ProtoError> {
        lock(&self.sessions)
            .get(&id)
            .map(|entry| Arc::clone(&entry.session))
            .ok_or(ProtoError::SessionNotFound(id))
    }

    fn list(&self) -> Vec<SessionInfo> {
        lock(&self.sessions)
            .iter()
            .map(|(id, entry)| {
                let title = entry
                    .session
                    .with(|processor| processor.engine().title().unwrap_or_default());
                SessionInfo {
                    id: *id,
                    title,
                    shell: entry.session.profile().name.clone(),
                    ssh_host: entry.ssh_host.clone(),
                    cwd: lock(&entry.cwd).clone(),
                    project_root: entry.project_root.clone(),
                    task: entry.task,
                    size: entry.size,
                    running: entry.session.exit_status().is_none(),
                }
            })
            .collect()
    }

    fn subscription_snapshot(&self, id: SessionId) -> Result<(u64, Vec<Event>), ProtoError> {
        let sessions = lock(&self.sessions);
        let log = Arc::clone(
            &sessions
                .get(&id)
                .ok_or(ProtoError::SessionNotFound(id))?
                .log,
        );
        drop(sessions);
        let snapshot = lock(&log).snapshot();
        Ok(snapshot)
    }

    fn events_since(&self, id: SessionId, cursor: u64) -> Result<(u64, Vec<Event>), ProtoError> {
        let sessions = lock(&self.sessions);
        let log = Arc::clone(
            &sessions
                .get(&id)
                .ok_or(ProtoError::SessionNotFound(id))?
                .log,
        );
        drop(sessions);
        let events = lock(&log).since(cursor);
        Ok(events)
    }

    fn program_status_snapshot(&self, id: SessionId) -> Result<Event, ProtoError> {
        let session = self.session(id)?;
        let records = session.with(|processor| {
            processor
                .program_status()
                .iter()
                .filter_map(|record| program_status_to_proto(&record.report))
                .collect()
        });
        Ok(Event::ProgramStatusSnapshot {
            session: id,
            records,
        })
    }

    fn list_tasks(&self, project_root: &std::path::Path) -> Result<Response, ProtoError> {
        let tasks = lock(&self.store)
            .tasks_for_project(project_root)
            .map_err(internal)?
            .into_iter()
            .map(task_to_proto)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Response::Tasks(tasks))
    }

    fn create_task(&self, create: tf_proto::CreateTask) -> Result<Response, ProtoError> {
        validate_task_text(&create.title, 200, "task title")?;
        validate_task_text(&create.context, 16 * 1024, "task context")?;
        let project_root = dunce::canonicalize(&create.project_root).map_err(internal)?;
        let id = TaskId::new();
        lock(&self.store)
            .create_task(&TaskRecord {
                id: id.to_string(),
                project_root,
                title: create.title,
                context: create.context,
                state: StoreTaskState::Planned,
                updated_at: 0,
            })
            .map_err(internal)?;
        Ok(Response::TaskCreated(id))
    }

    fn update_task(&self, update: tf_proto::UpdateTask) -> Result<Response, ProtoError> {
        if let Some(title) = update.title.as_deref() {
            validate_task_text(title, 200, "task title")?;
        }
        if let Some(context) = update.context.as_deref() {
            validate_task_text(context, 16 * 1024, "task context")?;
        }
        let changed = lock(&self.store)
            .update_task(
                &update.id.to_string(),
                update.state.map(task_state_to_store),
                update.title.as_deref(),
                update.context.as_deref(),
            )
            .map_err(internal)?;
        if !changed {
            return Err(ProtoError::Internal(format!(
                "task not found: {}",
                update.id
            )));
        }
        Ok(Response::Ok)
    }
}

fn validate_task_text(value: &str, max: usize, name: &str) -> Result<(), ProtoError> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(|ch| ch == '\0') {
        return Err(ProtoError::Internal(format!("invalid {name}")));
    }
    Ok(())
}

fn task_state_to_store(state: ProtoTaskState) -> StoreTaskState {
    match state {
        ProtoTaskState::Planned => StoreTaskState::Planned,
        ProtoTaskState::Active => StoreTaskState::Active,
        ProtoTaskState::Blocked => StoreTaskState::Blocked,
        ProtoTaskState::Done => StoreTaskState::Done,
    }
}

fn task_to_proto(task: TaskRecord) -> Result<TaskInfo, ProtoError> {
    Ok(TaskInfo {
        id: task.id.parse().map_err(internal)?,
        project_root: task.project_root,
        title: task.title,
        context: task.context,
        state: match task.state {
            StoreTaskState::Planned => ProtoTaskState::Planned,
            StoreTaskState::Active => ProtoTaskState::Active,
            StoreTaskState::Blocked => ProtoTaskState::Blocked,
            StoreTaskState::Done => ProtoTaskState::Done,
        },
        updated_at: task.updated_at,
    })
}

fn program_status_to_proto(report: &tf_session::ProgramStatusReport) -> Option<ProgramStatus> {
    let state = match report.state {
        tf_session::ProgramState::Idle => ProtoState::Idle,
        tf_session::ProgramState::Working => ProtoState::Working,
        tf_session::ProgramState::Done => ProtoState::Done,
        tf_session::ProgramState::Blocked => ProtoState::Blocked,
        tf_session::ProgramState::Error => ProtoState::Error,
        tf_session::ProgramState::Clear => return None,
    };
    let kind = report.kind.map(|kind| match kind {
        tf_session::BlockedKind::Permission => ProtoBlockedKind::Permission,
        tf_session::BlockedKind::Question => ProtoBlockedKind::Question,
        tf_session::BlockedKind::Auth => ProtoBlockedKind::Auth,
    });
    Some(ProgramStatus {
        state,
        id: report.id.clone(),
        kind,
        progress: report.progress,
        app: report.app.clone(),
        title: report.title.clone(),
        message: report.message.clone(),
    })
}

fn start_session_monitor(
    id: SessionId,
    session: std::sync::Weak<LiveSession>,
    cwd: Arc<Mutex<Option<WorkingDirectory>>>,
    remote: bool,
    log: Arc<Mutex<EventLog>>,
    store: Arc<Mutex<Store>>,
    wake_rx: std::sync::mpsc::Receiver<()>,
) {
    let _ = thread::Builder::new()
        .name("forged-session-events".into())
        .spawn(move || {
            let mut exit_sent = false;
            loop {
                let _ = wake_rx.recv_timeout(Duration::from_millis(100));
                let Some(session) = session.upgrade() else {
                    break;
                };
                for event in session.take_events() {
                    let event = match event {
                        SessionEvent::Output(data) => Some(Event::Output { session: id, data }),
                        SessionEvent::Cwd(new_cwd) => {
                            let protocol_cwd = WorkingDirectory {
                                host: new_cwd.host,
                                path: new_cwd.path,
                            };
                            *lock(&cwd) = Some(protocol_cwd.clone());
                            if let Some(path) = local_cwd(remote, &protocol_cwd) {
                                if let Err(error) = lock(&store)
                                    .update_restorable_session_cwd(&id.to_string(), &path)
                                {
                                    tracing::warn!(%error, %id, "could not persist session directory");
                                }
                            }
                            Some(Event::CwdChanged {
                                session: id,
                                cwd: protocol_cwd,
                            })
                        }
                        SessionEvent::Notify { title, body } => Some(Event::Notify {
                            session: id,
                            title,
                            body,
                        }),
                        SessionEvent::BlockFinished { .. } | SessionEvent::ProgramStatusChanged => {
                            None
                        }
                    };
                    if let Some(event) = event {
                        lock(&log).push(event);
                    }
                }
                if let Some(exit_code) = session.exit_status() {
                    if !exit_sent {
                        lock(&log).push(Event::Exited {
                            session: id,
                            exit_code,
                        });
                        exit_sent = true;
                        if let Err(error) = lock(&store).remove_restorable_session(&id.to_string())
                        {
                            tracing::warn!(%error, %id, "could not retire exited session record");
                        }
                    }
                }
            }
        });
}

fn local_cwd(remote: bool, cwd: &WorkingDirectory) -> Option<PathBuf> {
    (!remote).then(|| PathBuf::from(&cwd.path))
}

fn resolve_profile(wanted: Option<&str>) -> Result<tf_pty::ShellProfile, ProtoError> {
    let shells = tf_pty::discover_shells();
    if let Some(wanted) = wanted {
        return shells
            .into_iter()
            .find(|profile| profile.name.eq_ignore_ascii_case(wanted))
            .ok_or_else(|| ProtoError::Internal(format!("shell profile not found: {wanted}")));
    }
    shells
        .into_iter()
        .next()
        .ok_or_else(|| ProtoError::Internal("no shell found".into()))
}

/// Environment for spawned shells: put the directory holding `forged` (and
/// the `tf` CLI shipped beside it) on `PATH`, so `tf` works inside every
/// session however the daemon was started.
fn tool_env() -> Vec<(String, String)> {
    let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf))
    else {
        return Vec::new();
    };
    let mut paths = vec![dir];
    if let Some(existing) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(paths)
        .map(|path| vec![("PATH".to_owned(), path.to_string_lossy().into_owned())])
        .unwrap_or_default()
}

fn internal(error: impl std::fmt::Display) -> ProtoError {
    ProtoError::Internal(error.to_string())
}

#[derive(Debug)]
struct ForgedHandler {
    host: Arc<SessionHost>,
    token: String,
}

#[async_trait]
impl ConnectionHandler for ForgedHandler {
    async fn handle(&self, mut connection: Box<dyn ServerConnection>) -> tf_ipc::Result<()> {
        let Some(first) = tokio::time::timeout(AUTH_TIMEOUT, connection.recv())
            .await
            .map_err(|_| {
                tf_ipc::IpcError::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "IPC authentication timed out",
                ))
            })??
        else {
            return Ok(());
        };
        let ClientMsg::Hello {
            version,
            token,
            client,
        } = first
        else {
            connection
                .send(error_response(0, ProtoError::Unauthorized))
                .await?;
            return connection.close().await;
        };
        if version != PROTOCOL_VERSION {
            connection
                .send(error_response(
                    0,
                    ProtoError::VersionMismatch {
                        server: PROTOCOL_VERSION,
                        client: version,
                    },
                ))
                .await?;
            return connection.close().await;
        }
        if !tokens_equal(&token, &self.token) {
            connection
                .send(error_response(0, ProtoError::Unauthorized))
                .await?;
            return connection.close().await;
        }
        tracing::debug!(%client, "IPC client authenticated");
        connection
            .send(ServerMsg::Welcome {
                version: PROTOCOL_VERSION,
                server: format!("forged/{}", env!("CARGO_PKG_VERSION")),
            })
            .await?;

        let mut subscriptions = HashMap::<SessionId, u64>::new();
        let mut tick = tokio::time::interval(Duration::from_millis(16));
        loop {
            tokio::select! {
                message = connection.recv() => {
                    let Some(message) = message? else { break };
                    let ClientMsg::Request { id, request } = message else {
                        connection.send(error_response(
                            0,
                            ProtoError::Internal("hello may only be sent once".into()),
                        )).await?;
                        continue;
                    };
                    match request {
                        Request::Subscribe { session } => {
                            match self.host.subscription_snapshot(session) {
                                Ok((cursor, replay)) => {
                                    subscriptions.insert(session, cursor);
                                    connection.send(ServerMsg::Response { id, result: Ok(Response::Ok) }).await?;
                                    for event in replay {
                                        connection.send(ServerMsg::Event(event)).await?;
                                    }
                                    if let Ok(snapshot) = self.host.program_status_snapshot(session) {
                                        connection.send(ServerMsg::Event(snapshot)).await?;
                                    }
                                }
                                Err(error) => connection.send(error_response(id, error)).await?,
                            }
                        }
                        Request::Unsubscribe { session } => {
                            subscriptions.remove(&session);
                            connection.send(ServerMsg::Response { id, result: Ok(Response::Ok) }).await?;
                        }
                        request => {
                            connection.send(ServerMsg::Response {
                                id,
                                result: self.host.handle(request),
                            }).await?;
                        }
                    }
                }
                _ = tick.tick(), if !subscriptions.is_empty() => {
                    let ids: Vec<_> = subscriptions.keys().copied().collect();
                    for session in ids {
                        match self.host.events_since(session, subscriptions[&session]) {
                            Ok((cursor, events)) => {
                                subscriptions.insert(session, cursor);
                                for event in events {
                                    connection.send(ServerMsg::Event(event)).await?;
                                }
                            }
                            Err(_) => { subscriptions.remove(&session); }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

fn error_response(id: u64, error: ProtoError) -> ServerMsg {
    ServerMsg::Response {
        id,
        result: Err(error),
    }
}

fn tokens_equal(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

#[allow(clippy::print_stdout)]
fn doctor(data_dir: &std::path::Path) -> Result<()> {
    let store = tf_store::Store::open(&data_dir.join("termforge.db"))?;
    println!("forged {}", env!("CARGO_PKG_VERSION"));
    println!("protocol     v{}", tf_proto::PROTOCOL_VERSION);
    println!("data dir     {}", data_dir.display());
    println!("schema       v{}", store.schema_version()?);
    let exe_dir = std::env::current_exe()?.parent().map(|p| p.to_path_buf());
    let sideloaded = exe_dir
        .map(|d| d.join("conpty.dll").is_file() && d.join("OpenConsole.exe").is_file())
        .unwrap_or(false);
    if cfg!(windows) {
        println!(
            "conpty       {}",
            if sideloaded {
                "bundled"
            } else {
                "in-box (run `cargo xtask conpty`)"
            }
        );
    }
    println!("shells:");
    for s in tf_pty::discover_shells() {
        println!("  {:<20} {}", s.name, s.program.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn test_host(integration_dir: PathBuf) -> SessionHost {
        SessionHost::new(
            integration_dir,
            Arc::new(Mutex::new(Store::open_in_memory().expect("store"))),
        )
    }

    struct FakeConnection {
        incoming: VecDeque<ClientMsg>,
        outgoing: Arc<Mutex<Vec<ServerMsg>>>,
    }

    impl std::fmt::Debug for FakeConnection {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("FakeConnection").finish_non_exhaustive()
        }
    }

    #[async_trait]
    impl ServerConnection for FakeConnection {
        async fn send(&mut self, message: ServerMsg) -> tf_ipc::Result<()> {
            lock(&self.outgoing).push(message);
            Ok(())
        }

        async fn recv(&mut self) -> tf_ipc::Result<Option<ClientMsg>> {
            Ok(self.incoming.pop_front())
        }

        async fn close(&mut self) -> tf_ipc::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn token_comparison_rejects_length_and_content_changes() {
        assert!(tokens_equal("abc123", "abc123"));
        assert!(!tokens_equal("abc123", "abc124"));
        assert!(!tokens_equal("abc123", "abc1234"));
    }

    #[test]
    fn empty_host_answers_ping_and_lists_no_sessions() {
        let dir = tempfile::tempdir().expect("temp dir");
        let host = test_host(dir.path().to_path_buf());
        assert_eq!(host.handle(Request::Ping), Ok(Response::Pong));
        assert_eq!(
            host.handle(Request::ListSessions),
            Ok(Response::Sessions(Vec::new()))
        );
    }

    #[test]
    fn tasks_are_created_listed_and_updated_per_project() {
        let dir = tempfile::tempdir().expect("temp dir");
        let host = test_host(dir.path().to_path_buf());
        let Response::TaskCreated(id) = host
            .handle(Request::CreateTask(tf_proto::CreateTask {
                project_root: dir.path().to_path_buf(),
                title: "Ship beta".into(),
                context: "Preserve project state".into(),
            }))
            .expect("create task")
        else {
            panic!("unexpected response");
        };
        host.handle(Request::UpdateTask(tf_proto::UpdateTask {
            id,
            state: Some(ProtoTaskState::Active),
            title: None,
            context: Some("Daemon and UI share context".into()),
        }))
        .expect("update task");
        let Response::Tasks(tasks) = host
            .handle(Request::ListTasks {
                project_root: dir.path().to_path_buf(),
            })
            .expect("list tasks")
        else {
            panic!("unexpected response");
        };
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, id);
        assert_eq!(tasks[0].state, ProtoTaskState::Active);
        assert_eq!(tasks[0].context, "Daemon and UI share context");
    }

    #[test]
    fn authenticated_connection_receives_welcome_and_response() {
        let dir = tempfile::tempdir().expect("temp dir");
        let handler = ForgedHandler {
            host: Arc::new(test_host(dir.path().to_path_buf())),
            token: "secret".into(),
        };
        let outgoing = Arc::new(Mutex::new(Vec::new()));
        let connection = FakeConnection {
            incoming: VecDeque::from([
                ClientMsg::Hello {
                    version: PROTOCOL_VERSION,
                    token: "secret".into(),
                    client: "test".into(),
                },
                ClientMsg::Request {
                    id: 42,
                    request: Request::Ping,
                },
            ]),
            outgoing: Arc::clone(&outgoing),
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("runtime");
        runtime
            .block_on(handler.handle(Box::new(connection)))
            .expect("handler");
        let messages = lock(&outgoing);
        assert!(matches!(messages.first(), Some(ServerMsg::Welcome { .. })));
        assert_eq!(
            messages.get(1),
            Some(&ServerMsg::Response {
                id: 42,
                result: Ok(Response::Pong),
            })
        );
    }

    #[test]
    fn event_log_replays_independently_from_a_cursor() {
        let session = SessionId::new();
        let mut log = EventLog::default();
        log.push(Event::Output {
            session,
            data: b"one".to_vec(),
        });
        let first_cursor = log.next;
        log.push(Event::Output {
            session,
            data: b"two".to_vec(),
        });

        let (_, replay) = log.snapshot();
        assert_eq!(replay.len(), 2);
        let (cursor, later) = log.since(first_cursor);
        assert_eq!(cursor, 2);
        assert_eq!(later.len(), 1);
        assert!(matches!(&later[0], Event::Output { data, .. } if data == b"two"));
    }

    #[test]
    fn event_log_bounds_raw_output() {
        let session = SessionId::new();
        let mut log = EventLog::default();
        for _ in 0..9 {
            log.push(Event::Output {
                session,
                data: vec![0; 1024 * 1024],
            });
        }
        assert!(log.output_bytes <= MAX_REPLAY_BYTES);
        assert_eq!(log.entries.len(), 8);
    }

    #[test]
    fn event_log_bounds_non_output_events() {
        let session = SessionId::new();
        let mut log = EventLog::default();
        for index in 0..MAX_REPLAY_EVENTS + 100 {
            log.push(Event::Notify {
                session,
                title: None,
                body: index.to_string(),
            });
        }
        assert_eq!(log.entries.len(), MAX_REPLAY_EVENTS);
        let first_sequence = log.entries.front().expect("bounded log has entries").0;
        let (_, replay) = log.since(first_sequence);
        assert_eq!(replay.len(), MAX_REPLAY_EVENTS);
        let (_, tail) = log.since(log.next);
        assert!(tail.is_empty());
    }

    #[test]
    fn remote_working_directories_never_become_local_paths() {
        let cwd = WorkingDirectory {
            host: Some("devbox".into()),
            path: "/srv/project".into(),
        };
        assert_eq!(local_cwd(false, &cwd), Some(PathBuf::from("/srv/project")));
        assert_eq!(local_cwd(true, &cwd), None);
    }
}
