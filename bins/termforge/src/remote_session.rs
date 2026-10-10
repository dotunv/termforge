//! Session facade used by the terminal view.
//!
//! The preferred path keeps the PTY in `forged` and maintains only a local
//! terminal model for rendering. An embedded fallback keeps development builds
//! usable when the daemon has not been started yet.

use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;
use std::{path::PathBuf, process::Stdio};

use anyhow::{Context, Result};
use tf_engine::{AlacrittyEngine, GridSize, TerminalEngine};
use tf_ipc::IpcClient;
use tf_proto::{
    BlockedKind as ProtoBlockedKind, CreateSession, Event, ProgramState as ProtoState,
    ProgramStatus, Request, Response, SessionId, SessionInfo, TermSize,
};
use tf_session::{LiveSession, Processor, SessionEvent, SpawnOptions, Waker};
use tokio::sync::mpsc::{self as async_mpsc, UnboundedReceiver, UnboundedSender};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Debug)]
enum Command {
    Write(Vec<u8>),
    Resize(GridSize),
    Close,
    Disconnect,
}

/// Which daemon session a new handle should talk to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attach {
    /// Always create a fresh session.
    New,
    /// Reuse a running session of the same project and host, else create one.
    Matching,
    /// Reattach to this exact session if it is still running, else create one.
    Session(SessionId),
}

#[derive(Debug)]
struct RemoteShared {
    session_id: Mutex<Option<SessionId>>,
    processor: Mutex<Processor<AlacrittyEngine>>,
    events: Mutex<Vec<SessionEvent>>,
    exit: Mutex<Option<Option<u32>>>,
    commands: UnboundedSender<Command>,
}

#[derive(Debug)]
pub struct RemoteSession {
    shared: Arc<RemoteShared>,
}

impl RemoteSession {
    fn connect(
        spawn: SpawnOptions,
        project_root: Option<PathBuf>,
        attach: Attach,
        wake: Waker,
    ) -> Result<Self> {
        let (commands, command_rx) = async_mpsc::unbounded_channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let shared = Arc::new(RemoteShared {
            session_id: Mutex::new(None),
            processor: Mutex::new(Processor::new(AlacrittyEngine::new(spawn.size))),
            events: Mutex::new(Vec::new()),
            exit: Mutex::new(None),
            commands,
        });
        let thread_shared = Arc::clone(&shared);
        thread::Builder::new()
            .name("tf-forged-client".into())
            .spawn(move || {
                remote_loop(
                    thread_shared,
                    spawn,
                    project_root,
                    attach,
                    wake,
                    command_rx,
                    ready_tx,
                )
            })
            .context("spawn forged client thread")?;
        ready_rx
            .recv_timeout(Duration::from_secs(2))
            .context("forged connection timed out")??;
        Ok(Self { shared })
    }

    fn write(&self, bytes: &[u8]) -> Result<()> {
        self.shared
            .commands
            .send(Command::Write(bytes.to_vec()))
            .context("forged client stopped")
    }

    fn resize(&self, size: GridSize) -> Result<()> {
        if lock(&self.shared.processor).engine().size() == size {
            return Ok(());
        }
        lock(&self.shared.processor).engine_mut().resize(size);
        self.shared
            .commands
            .send(Command::Resize(size))
            .context("forged client stopped")
    }

    fn with<R>(&self, f: impl FnOnce(&Processor<AlacrittyEngine>) -> R) -> R {
        f(&lock(&self.shared.processor))
    }

    fn with_mut<R>(&self, f: impl FnOnce(&mut Processor<AlacrittyEngine>) -> R) -> R {
        f(&mut lock(&self.shared.processor))
    }

    fn take_events(&self) -> Vec<SessionEvent> {
        std::mem::take(&mut *lock(&self.shared.events))
    }

    fn exit_status(&self) -> Option<Option<u32>> {
        *lock(&self.shared.exit)
    }

    fn close(&self) -> Result<()> {
        self.shared
            .commands
            .send(Command::Close)
            .context("forged client stopped")
    }
}

impl Drop for RemoteSession {
    fn drop(&mut self) {
        let _ = self.shared.commands.send(Command::Disconnect);
    }
}

async fn connect_and_subscribe(
    spawn: &SpawnOptions,
    project_root: Option<&PathBuf>,
    attach: Attach,
) -> Result<(IpcClient, SessionId)> {
    let token = tf_ipc::token::ensure_token().context("load IPC token")?;
    let mut client = IpcClient::connect("", &token, "termforge-ui")
        .await
        .context("connect to forged")?;
    let sessions = match client.request(Request::ListSessions).await? {
        Response::Sessions(sessions) => sessions,
        response => anyhow::bail!("unexpected list-sessions response: {response:?}"),
    };
    let existing = match attach {
        Attach::New => None,
        Attach::Matching => {
            matching_running_session(&sessions, project_root, spawn.ssh_host.as_deref())
        }
        Attach::Session(id) => sessions
            .iter()
            .find(|session| session.running && session.id == id)
            .map(|session| session.id),
    };
    let session = if let Some(existing) = existing {
        existing
    } else {
        match client
            .request(Request::CreateSession(CreateSession {
                profile: Some(spawn.profile.name.clone()),
                ssh_host: spawn.ssh_host.clone(),
                cwd: spawn.cwd.clone(),
                project_root: project_root.cloned(),
                task: None,
                size: TermSize {
                    cols: spawn.size.cols,
                    rows: spawn.size.rows,
                },
            }))
            .await?
        {
            Response::Created(session) => session,
            response => anyhow::bail!("unexpected create-session response: {response:?}"),
        }
    };
    client.subscribe(session).await?;
    Ok((client, session))
}

fn matching_running_session(
    sessions: &[SessionInfo],
    project_root: Option<&PathBuf>,
    ssh_host: Option<&str>,
) -> Option<SessionId> {
    sessions
        .iter()
        .find(|session| {
            session.running
                && session.project_root.as_ref() == project_root
                && session.ssh_host.as_deref() == ssh_host
        })
        .map(|session| session.id)
}

fn remote_loop(
    shared: Arc<RemoteShared>,
    spawn: SpawnOptions,
    project_root: Option<PathBuf>,
    attach: Attach,
    wake: Waker,
    mut commands: UnboundedReceiver<Command>,
    ready: mpsc::SyncSender<Result<()>>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = ready.send(Err(error).context("create IPC runtime"));
            return;
        }
    };
    runtime.block_on(async move {
        let (mut client, session) =
            match connect_and_subscribe(&spawn, project_root.as_ref(), attach).await {
                Ok(connected) => {
                    *lock(&shared.session_id) = Some(connected.1);
                    let _ = ready.send(Ok(()));
                    connected
                }
                Err(error) => {
                    let _ = ready.send(Err(error));
                    return;
                }
            };

        loop {
            tokio::select! {
                command = commands.recv() => {
                    match command {
                    Some(Command::Write(data)) => {
                        if client
                            .request(Request::Write { session, data })
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    Some(Command::Resize(size)) => {
                        if client
                            .request(Request::Resize {
                                session,
                                size: TermSize {
                                    cols: size.cols,
                                    rows: size.rows,
                                },
                            })
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    Some(Command::Close) => {
                        let _ = client.request(Request::Close { session }).await;
                        return;
                    }
                    Some(Command::Disconnect) | None => return,
                    }
                }
                event = client.next_event() => match event {
                    Ok(Some(event)) => apply_event(&shared, event, &wake),
                    Ok(None) | Err(_) => return,
                }
            }
        }
    });
}

fn apply_event(shared: &RemoteShared, event: Event, wake: &Waker) {
    match event {
        Event::Output { data, .. } => {
            let mut events = Vec::new();
            // Replies were already generated by forged's authoritative
            // processor and written to the PTY there.
            let _ = lock(&shared.processor).process(&data, &mut events);
            lock(&shared.events).append(&mut events);
        }
        Event::Exited { exit_code, .. } => *lock(&shared.exit) = Some(exit_code),
        // These semantic events are also recovered from the authoritative raw
        // stream by Processor. Keep the protocol variants for non-rendering
        // clients, but do not duplicate them in TerminalView.
        Event::CwdChanged { .. } | Event::Notify { .. } => {}
        Event::ProgramStatusSnapshot { records, .. } => {
            lock(&shared.processor)
                .replace_program_status(records.into_iter().map(program_status_from_proto));
            lock(&shared.events).push(SessionEvent::ProgramStatusChanged);
        }
    }
    wake();
}

fn program_status_from_proto(status: ProgramStatus) -> tf_session::ProgramStatusReport {
    let state = match status.state {
        ProtoState::Idle => tf_session::ProgramState::Idle,
        ProtoState::Working => tf_session::ProgramState::Working,
        ProtoState::Done => tf_session::ProgramState::Done,
        ProtoState::Blocked => tf_session::ProgramState::Blocked,
        ProtoState::Error => tf_session::ProgramState::Error,
    };
    let kind = status.kind.map(|kind| match kind {
        ProtoBlockedKind::Permission => tf_session::BlockedKind::Permission,
        ProtoBlockedKind::Question => tf_session::BlockedKind::Question,
        ProtoBlockedKind::Auth => tf_session::BlockedKind::Auth,
    });
    tf_session::ProgramStatusReport {
        state,
        id: status.id,
        kind,
        progress: status.progress,
        app: status.app,
        title: status.title,
        message: status.message,
    }
}

#[derive(Debug)]
pub enum SessionHandle {
    Remote(RemoteSession),
    Embedded(LiveSession),
}

impl SessionHandle {
    pub fn spawn(
        spawn: SpawnOptions,
        project_root: Option<PathBuf>,
        attach: Attach,
        wake: Waker,
    ) -> Result<Self> {
        if std::env::var_os("TERMFORGE_EMBED_SESSION").is_none() {
            match RemoteSession::connect(
                spawn.clone(),
                project_root.clone(),
                attach,
                Arc::clone(&wake),
            ) {
                Ok(session) => return Ok(Self::Remote(session)),
                Err(initial_error) => {
                    if let Err(start_error) = start_forged() {
                        tracing::warn!(
                            "forged unavailable ({initial_error:#}) and could not be started: {start_error:#}"
                        );
                    } else {
                        let mut last_error = initial_error;
                        for _ in 0..20 {
                            thread::sleep(Duration::from_millis(50));
                            match RemoteSession::connect(
                                spawn.clone(),
                                project_root.clone(),
                                attach,
                                Arc::clone(&wake),
                            ) {
                                Ok(session) => return Ok(Self::Remote(session)),
                                Err(error) => last_error = error,
                            }
                        }
                        tracing::warn!(
                            "forged was started but did not become ready ({last_error:#}); using embedded session"
                        );
                    }
                }
            }
        }
        Ok(Self::Embedded(LiveSession::spawn(spawn, wake)?))
    }

    /// The daemon's id for this session; `None` for embedded sessions, which
    /// cannot be reattached after a restart.
    pub fn id(&self) -> Option<SessionId> {
        match self {
            Self::Remote(session) => *lock(&session.shared.session_id),
            Self::Embedded(_) => None,
        }
    }

    pub fn write(&self, bytes: &[u8]) -> Result<()> {
        match self {
            Self::Remote(session) => session.write(bytes),
            Self::Embedded(session) => session.write(bytes),
        }
    }

    pub fn resize(&self, size: GridSize) -> Result<()> {
        match self {
            Self::Remote(session) => session.resize(size),
            Self::Embedded(session) => session.resize(size),
        }
    }

    pub fn with<R>(&self, f: impl FnOnce(&Processor<AlacrittyEngine>) -> R) -> R {
        match self {
            Self::Remote(session) => session.with(f),
            Self::Embedded(session) => session.with(f),
        }
    }

    pub fn with_mut<R>(&self, f: impl FnOnce(&mut Processor<AlacrittyEngine>) -> R) -> R {
        match self {
            Self::Remote(session) => session.with_mut(f),
            Self::Embedded(session) => session.with_mut(f),
        }
    }

    pub fn take_events(&self) -> Vec<SessionEvent> {
        match self {
            Self::Remote(session) => session.take_events(),
            Self::Embedded(session) => session.take_events(),
        }
    }

    pub fn exit_status(&self) -> Option<Option<u32>> {
        match self {
            Self::Remote(session) => session.exit_status(),
            Self::Embedded(session) => session.exit_status(),
        }
    }

    pub fn close(&self) -> Result<()> {
        match self {
            Self::Remote(session) => session.close(),
            Self::Embedded(_) => Ok(()),
        }
    }
}

fn start_forged() -> Result<()> {
    let executable = forged_executable()?;
    std::process::Command::new(&executable)
        .arg("run")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("start {}", executable.display()))?;
    Ok(())
}

fn forged_executable() -> Result<PathBuf> {
    let mut path = std::env::current_exe().context("resolve TermForge executable")?;
    path.set_file_name(if cfg!(windows) {
        "forged.exe"
    } else {
        "forged"
    });
    anyhow::ensure!(
        path.is_file(),
        "forged is not installed beside TermForge ({})",
        path.display()
    );
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared() -> RemoteShared {
        let (commands, _) = async_mpsc::unbounded_channel();
        RemoteShared {
            session_id: Mutex::new(None),
            processor: Mutex::new(Processor::new(AlacrittyEngine::new(GridSize::new(40, 5)))),
            events: Mutex::new(Vec::new()),
            exit: Mutex::new(None),
            commands,
        }
    }

    #[test]
    fn output_events_feed_the_local_render_model() {
        let shared = shared();
        let wake: Waker = Arc::new(|| {});
        apply_event(
            &shared,
            Event::Output {
                session: SessionId::new(),
                data: b"hello".to_vec(),
            },
            &wake,
        );
        let text = lock(&shared.processor).engine().snapshot().text();
        assert!(text.contains("hello"));
    }

    #[test]
    fn exit_events_update_remote_state() {
        let shared = shared();
        let wake: Waker = Arc::new(|| {});
        apply_event(
            &shared,
            Event::Exited {
                session: SessionId::new(),
                exit_code: Some(7),
            },
            &wake,
        );
        assert_eq!(*lock(&shared.exit), Some(Some(7)));
    }

    #[test]
    fn status_snapshot_replaces_state_reconstructed_from_scrollback() {
        let shared = shared();
        let wake: Waker = Arc::new(|| {});
        let mut old_events = Vec::new();
        lock(&shared.processor).process(b"\x1b]7501;state=working:id=old\x1b\\", &mut old_events);

        apply_event(
            &shared,
            Event::ProgramStatusSnapshot {
                session: SessionId::new(),
                records: vec![ProgramStatus {
                    state: ProtoState::Blocked,
                    id: Some("current".into()),
                    kind: Some(ProtoBlockedKind::Question),
                    progress: None,
                    app: Some("agent".into()),
                    title: None,
                    message: Some("Choose a target".into()),
                }],
            },
            &wake,
        );

        let records: Vec<_> = lock(&shared.processor)
            .program_status()
            .iter()
            .map(|record| record.report.clone())
            .collect();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id.as_deref(), Some("current"));
        assert_eq!(records[0].state, tf_session::ProgramState::Blocked);
        assert_eq!(
            lock(&shared.events).as_slice(),
            [SessionEvent::ProgramStatusChanged]
        );
    }

    #[test]
    fn reattaches_only_to_a_running_session_in_the_same_project() {
        let wanted = PathBuf::from("/projects/wanted");
        let other = PathBuf::from("/projects/other");
        let wrong_id = SessionId::new();
        let stopped_id = SessionId::new();
        let expected_id = SessionId::new();
        let session = |id, project_root, running| SessionInfo {
            id,
            title: String::new(),
            shell: "shell".into(),
            ssh_host: None,
            cwd: None,
            project_root: Some(project_root),
            task: None,
            size: TermSize::default(),
            running,
        };
        let sessions = vec![
            session(wrong_id, other, true),
            session(stopped_id, wanted.clone(), false),
            session(expected_id, wanted.clone(), true),
        ];

        assert_eq!(
            matching_running_session(&sessions, Some(&wanted), None),
            Some(expected_id)
        );
    }

    #[test]
    fn reattaches_only_to_the_same_ssh_host() {
        let project = PathBuf::from("/projects/wanted");
        let wrong = SessionId::new();
        let expected = SessionId::new();
        let session = |id, ssh_host: &str| SessionInfo {
            id,
            title: String::new(),
            shell: "ssh".into(),
            ssh_host: Some(ssh_host.into()),
            cwd: None,
            project_root: Some(project.clone()),
            task: None,
            size: TermSize::default(),
            running: true,
        };
        let sessions = vec![session(wrong, "other"), session(expected, "devbox")];
        assert_eq!(
            matching_running_session(&sessions, Some(&project), Some("devbox")),
            Some(expected)
        );
    }
}

/// What the daemon remembers about a project's workspace.
#[derive(Debug, Default)]
pub struct StoredWorkspace {
    /// Every session the daemon knows about.
    pub sessions: Vec<SessionInfo>,
    /// The opaque layout document last saved with [`save_workspace_state`].
    pub layout: Option<String>,
}

/// Run an IPC exchange on its own short-lived runtime so it can be called
/// from the UI thread without an ambient runtime.
fn blocking_ipc<T, Fut>(exchange: impl FnOnce(IpcClient) -> Fut + Send + 'static) -> Result<T>
where
    T: Send + 'static,
    Fut: std::future::Future<Output = Result<T>>,
{
    thread::Builder::new()
        .name("tf-forged-oneshot".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("create IPC runtime")?;
            runtime.block_on(async move {
                let token = tf_ipc::token::ensure_token().context("load IPC token")?;
                let client = tokio::time::timeout(
                    Duration::from_secs(2),
                    IpcClient::connect("", &token, "termforge-ui"),
                )
                .await
                .context("forged connection timed out")?
                .context("connect to forged")?;
                tokio::time::timeout(Duration::from_secs(2), exchange(client))
                    .await
                    .context("forged request timed out")?
            })
        })
        .context("spawn forged one-shot thread")?
        .join()
        .map_err(|_| anyhow::anyhow!("forged one-shot thread panicked"))?
}

/// Fetch the daemon's sessions and the saved layout for a project. Returns
/// `None` when the daemon is not running, in which case there is nothing to
/// restore.
pub fn load_workspace(project_root: Option<PathBuf>) -> Option<StoredWorkspace> {
    blocking_ipc(move |mut client| async move {
        let sessions = match client.request(Request::ListSessions).await? {
            Response::Sessions(sessions) => sessions,
            response => anyhow::bail!("unexpected list-sessions response: {response:?}"),
        };
        let layout = match project_root {
            Some(project_root) => match client
                .request(Request::LoadWorkspaceState { project_root })
                .await?
            {
                Response::WorkspaceState(layout) => layout,
                response => anyhow::bail!("unexpected workspace response: {response:?}"),
            },
            None => None,
        };
        Ok(StoredWorkspace { sessions, layout })
    })
    .map_err(|error| tracing::debug!("no workspace to restore: {error:#}"))
    .ok()
}

/// Save the layout document in the background; failures only cost restore
/// fidelity, so they are logged rather than surfaced.
pub fn save_workspace_state(project_root: PathBuf, state: String) {
    let spawned = thread::Builder::new()
        .name("tf-save-layout".into())
        .spawn(move || {
            let result = blocking_ipc(move |mut client| async move {
                client
                    .request(Request::SaveWorkspaceState {
                        project_root,
                        state,
                    })
                    .await?;
                Ok(())
            });
            if let Err(error) = result {
                tracing::debug!("could not save workspace layout: {error:#}");
            }
        });
    if let Err(error) = spawned {
        tracing::debug!("could not start layout save thread: {error}");
    }
}
