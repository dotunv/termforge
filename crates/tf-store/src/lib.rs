//! Local persistence for `forged`.
//!
//! One SQLite database per user, in WAL mode, owned by the session host.
//! Schema changes are append-only entries in [`MIGRATIONS`], tracked with
//! `PRAGMA user_version`, and every migration runs inside a transaction.
//!
//! Projects are keyed by their canonical root path, stored as plain text so
//! every component agrees on identity without re-deriving hashes.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("path {0} could not be canonicalised: {1}")]
    Path(PathBuf, std::io::Error),
    #[error("database schema version {found} is newer than this build supports ({supported})")]
    TooNew { found: i64, supported: i64 },
    #[error("stored session environment: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid task dependency: {0}")]
    InvalidTaskDependency(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// Ordered schema migrations. Never edit an entry once released; append.
const MIGRATIONS: &[&str] = &[
    // v1
    r"
    CREATE TABLE projects (
        id             INTEGER PRIMARY KEY,
        root           TEXT NOT NULL UNIQUE,
        name           TEXT NOT NULL,
        created_at     INTEGER NOT NULL,
        last_opened_at INTEGER NOT NULL
    );
    CREATE TABLE command_history (
        id          INTEGER PRIMARY KEY,
        project_id  INTEGER REFERENCES projects(id) ON DELETE CASCADE,
        command     TEXT NOT NULL,
        cwd         TEXT,
        exit_code   INTEGER,
        started_at  INTEGER NOT NULL,
        duration_ms INTEGER
    );
    CREATE INDEX idx_history_project_started ON command_history(project_id, started_at DESC);
    CREATE TABLE workspace_state (
        project_id INTEGER PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
        state      TEXT NOT NULL,
        saved_at   INTEGER NOT NULL
    );
    ",
    // v2: enough metadata to recreate a project shell after forged restarts.
    r"
    CREATE TABLE restorable_sessions (
        id           TEXT PRIMARY KEY,
        project_root TEXT,
        profile      TEXT NOT NULL,
        cwd          TEXT,
        cols         INTEGER NOT NULL,
        rows         INTEGER NOT NULL,
        updated_at   INTEGER NOT NULL
    );
    CREATE INDEX idx_restorable_sessions_project
        ON restorable_sessions(project_root, updated_at DESC);
    ",
    // v3: durable project tasks and optional session association.
    r"
    CREATE TABLE tasks (
        id           TEXT PRIMARY KEY,
        project_root TEXT NOT NULL,
        title        TEXT NOT NULL,
        context      TEXT NOT NULL,
        state        TEXT NOT NULL,
        created_at   INTEGER NOT NULL,
        updated_at   INTEGER NOT NULL
    );
    CREATE INDEX idx_tasks_project_updated
        ON tasks(project_root, updated_at DESC);
    ALTER TABLE restorable_sessions ADD COLUMN task_id TEXT REFERENCES tasks(id) ON DELETE SET NULL;
    ",
    // v4: identify sessions recreated through the user's system SSH client.
    r"
    ALTER TABLE restorable_sessions ADD COLUMN ssh_host TEXT;
    ",
    // v5: an optional runnable command attached to a task.
    r"
    ALTER TABLE tasks ADD COLUMN command TEXT;
    ",
    // v6: client/account context selected for a project. Profiles themselves
    // live in config.toml so they are portable; the store owns assignment.
    r"
    ALTER TABLE projects ADD COLUMN context_id TEXT;
    ",
    // v7: preserve identity selection when forged recreates a shell.
    r"
    ALTER TABLE restorable_sessions ADD COLUMN context_env TEXT NOT NULL DEFAULT '[]';
    ",
    // v8: task planning metadata and an integrity-checked dependency graph.
    r"
    ALTER TABLE tasks ADD COLUMN priority TEXT NOT NULL DEFAULT 'normal'
        CHECK(priority IN ('low', 'normal', 'high', 'urgent'));
    CREATE TABLE task_dependencies (
        task_id       TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
        depends_on_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
        PRIMARY KEY(task_id, depends_on_id),
        CHECK(task_id <> depends_on_id)
    );
    CREATE INDEX idx_task_dependencies_reverse
        ON task_dependencies(depends_on_id, task_id);
    ",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub id: i64,
    pub root: PathBuf,
    pub name: String,
    pub last_opened_at: i64,
    pub context_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRecord {
    pub command: String,
    pub cwd: Option<String>,
    pub exit_code: Option<i32>,
    pub started_at: i64,
    pub duration_ms: Option<i64>,
}

/// Minimal shell metadata that can safely survive a daemon restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestorableSession {
    pub id: String,
    pub project_root: Option<PathBuf>,
    pub profile: String,
    pub ssh_host: Option<String>,
    pub cwd: Option<PathBuf>,
    pub cols: u16,
    pub rows: u16,
    pub task_id: Option<String>,
    pub context_env: Vec<(String, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Planned,
    Active,
    Blocked,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskPriority {
    Low,
    Normal,
    High,
    Urgent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRecord {
    pub id: String,
    pub project_root: PathBuf,
    pub title: String,
    pub context: String,
    /// A single-line shell command the UI can run for this task.
    pub command: Option<String>,
    pub state: TaskState,
    pub priority: TaskPriority,
    pub dependencies: Vec<String>,
    pub updated_at: i64,
}

#[derive(Debug, Default)]
pub struct TaskUpdate<'a> {
    pub state: Option<TaskState>,
    pub title: Option<&'a str>,
    pub context: Option<&'a str>,
    pub command: Option<&'a str>,
    pub priority: Option<TaskPriority>,
    pub dependencies: Option<&'a [String]>,
}

#[derive(Debug)]
pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let mut store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn schema_version(&self) -> Result<i64> {
        Ok(self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?)
    }

    fn migrate(&mut self) -> Result<()> {
        let current = self.schema_version()?;
        let supported = MIGRATIONS.len() as i64;
        if current > supported {
            return Err(StoreError::TooNew {
                found: current,
                supported,
            });
        }
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", (i + 1) as i64)?;
            tx.commit()?;
        }
        Ok(())
    }

    /// Register (or touch) a project by its root directory.
    pub fn open_project(&self, root: &Path) -> Result<Project> {
        let root = dunce::canonicalize(root).map_err(|e| StoreError::Path(root.into(), e))?;
        let root_str = root.to_string_lossy().into_owned();
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root_str.clone());
        let now = now_ms();
        self.conn.execute(
            "INSERT INTO projects (root, name, created_at, last_opened_at) VALUES (?1, ?2, ?3, ?3)
             ON CONFLICT(root) DO UPDATE SET last_opened_at = excluded.last_opened_at",
            params![root_str, name, now],
        )?;
        Ok(self.conn.query_row(
            "SELECT id, root, name, last_opened_at, context_id FROM projects WHERE root = ?1",
            params![root_str],
            row_to_project,
        )?)
    }

    pub fn recent_projects(&self, limit: usize) -> Result<Vec<Project>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, root, name, last_opened_at, context_id FROM projects
             ORDER BY last_opened_at DESC, id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], row_to_project)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Assign a portable context profile id to a canonical project.
    pub fn set_project_context(&self, root: &Path, context_id: Option<&str>) -> Result<Project> {
        let project = self.open_project(root)?;
        self.conn.execute(
            "UPDATE projects SET context_id = ?1 WHERE id = ?2",
            params![context_id, project.id],
        )?;
        Ok(self.conn.query_row(
            "SELECT id, root, name, last_opened_at, context_id FROM projects WHERE id = ?1",
            params![project.id],
            row_to_project,
        )?)
    }

    pub fn record_command(&self, project_id: Option<i64>, rec: &CommandRecord) -> Result<()> {
        self.conn.execute(
            "INSERT INTO command_history (project_id, command, cwd, exit_code, started_at, duration_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![project_id, rec.command, rec.cwd, rec.exit_code, rec.started_at, rec.duration_ms],
        )?;
        Ok(())
    }

    pub fn recent_commands(&self, project_id: i64, limit: usize) -> Result<Vec<CommandRecord>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT command, cwd, exit_code, started_at, duration_ms FROM command_history
             WHERE project_id = ?1 ORDER BY started_at DESC, id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![project_id, limit as i64], |r| {
            Ok(CommandRecord {
                command: r.get(0)?,
                cwd: r.get(1)?,
                exit_code: r.get(2)?,
                started_at: r.get(3)?,
                duration_ms: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Persist the opaque workspace layout document for a project.
    pub fn save_workspace_state(&self, project_id: i64, state: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO workspace_state (project_id, state, saved_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(project_id) DO UPDATE SET state = excluded.state, saved_at = excluded.saved_at",
            params![project_id, state, now_ms()],
        )?;
        Ok(())
    }

    pub fn load_workspace_state(&self, project_id: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT state FROM workspace_state WHERE project_id = ?1",
                params![project_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn save_restorable_session(&self, session: &RestorableSession) -> Result<()> {
        self.conn.execute(
            "INSERT INTO restorable_sessions
                (id, project_root, profile, cwd, cols, rows, updated_at, task_id, ssh_host, context_env)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(id) DO UPDATE SET
                project_root = excluded.project_root,
                profile = excluded.profile,
                cwd = excluded.cwd,
                cols = excluded.cols,
                rows = excluded.rows,
                updated_at = excluded.updated_at,
                task_id = excluded.task_id,
                ssh_host = excluded.ssh_host,
                context_env = excluded.context_env",
            params![
                session.id,
                session
                    .project_root
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned()),
                session.profile,
                session
                    .cwd
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned()),
                session.cols,
                session.rows,
                now_ms(),
                session.task_id,
                session.ssh_host,
                serde_json::to_string(&session.context_env)?,
            ],
        )?;
        Ok(())
    }

    pub fn restorable_sessions(&self) -> Result<Vec<RestorableSession>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, project_root, profile, cwd, cols, rows, task_id, ssh_host, context_env
             FROM restorable_sessions ORDER BY updated_at, id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(RestorableSession {
                id: row.get(0)?,
                project_root: row.get::<_, Option<String>>(1)?.map(PathBuf::from),
                profile: row.get(2)?,
                cwd: row.get::<_, Option<String>>(3)?.map(PathBuf::from),
                cols: row.get(4)?,
                rows: row.get(5)?,
                task_id: row.get(6)?,
                ssh_host: row.get(7)?,
                context_env: serde_json::from_str(&row.get::<_, String>(8)?).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        8,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn remove_restorable_session(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM restorable_sessions WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn update_restorable_session_cwd(&self, id: &str, cwd: &Path) -> Result<()> {
        self.conn.execute(
            "UPDATE restorable_sessions SET cwd = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, cwd.to_string_lossy(), now_ms()],
        )?;
        Ok(())
    }

    pub fn create_task(&self, task: &TaskRecord) -> Result<()> {
        let project_root = dunce::canonicalize(&task.project_root)
            .map_err(|error| StoreError::Path(task.project_root.clone(), error))?;
        let project_root = project_root.to_string_lossy().into_owned();
        let transaction = self.conn.unchecked_transaction()?;
        transaction.execute(
            "INSERT INTO tasks (id, project_root, title, context, state, created_at, updated_at, command, priority)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7, ?8)",
            params![
                task.id,
                project_root,
                task.title,
                task.context,
                task_state_name(task.state),
                now_ms(),
                task.command,
                task_priority_name(task.priority),
            ],
        )?;
        replace_task_dependencies(&transaction, &task.id, &project_root, &task.dependencies)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn tasks_for_project(&self, project_root: &Path) -> Result<Vec<TaskRecord>> {
        let root = dunce::canonicalize(project_root)
            .map_err(|error| StoreError::Path(project_root.into(), error))?;
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, project_root, title, context, state, updated_at, command, priority FROM tasks
             WHERE project_root = ?1 ORDER BY updated_at DESC, id",
        )?;
        let rows = stmt.query_map(params![root.to_string_lossy()], row_to_task)?;
        let mut tasks: Vec<TaskRecord> = rows.collect::<rusqlite::Result<_>>()?;
        drop(stmt);
        for task in &mut tasks {
            let mut dependencies = self.conn.prepare_cached(
                "SELECT depends_on_id FROM task_dependencies
                 WHERE task_id = ?1 ORDER BY depends_on_id",
            )?;
            task.dependencies = dependencies
                .query_map(params![task.id], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
        }
        Ok(tasks)
    }

    pub fn update_task(&self, id: &str, update: &TaskUpdate<'_>) -> Result<bool> {
        // `command`: `None` leaves it alone, `Some("")` clears it.
        let transaction = self.conn.unchecked_transaction()?;
        let project_root: Option<String> = transaction
            .query_row(
                "SELECT project_root FROM tasks WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(project_root) = project_root else {
            return Ok(false);
        };
        let changed = transaction.execute(
            "UPDATE tasks SET
                state = COALESCE(?2, state),
                title = COALESCE(?3, title),
                context = COALESCE(?4, context),
                command = CASE WHEN ?5 IS NULL THEN command
                               WHEN ?5 = '' THEN NULL
                               ELSE ?5 END,
                priority = COALESCE(?6, priority),
                updated_at = ?7
             WHERE id = ?1",
            params![
                id,
                update.state.map(task_state_name),
                update.title,
                update.context,
                update.command,
                update.priority.map(task_priority_name),
                now_ms()
            ],
        )?;
        if let Some(dependencies) = update.dependencies {
            replace_task_dependencies(&transaction, id, &project_root, dependencies)?;
        }
        transaction.commit()?;
        Ok(changed != 0)
    }

    /// Remove a task. Sessions that referenced it keep running with no task.
    pub fn delete_task(&self, id: &str) -> Result<bool> {
        let removed = self
            .conn
            .execute("DELETE FROM tasks WHERE id = ?1", params![id])?;
        Ok(removed != 0)
    }
}

fn task_state_name(state: TaskState) -> &'static str {
    match state {
        TaskState::Planned => "planned",
        TaskState::Active => "active",
        TaskState::Blocked => "blocked",
        TaskState::Done => "done",
    }
}

fn task_priority_name(priority: TaskPriority) -> &'static str {
    match priority {
        TaskPriority::Low => "low",
        TaskPriority::Normal => "normal",
        TaskPriority::High => "high",
        TaskPriority::Urgent => "urgent",
    }
}

fn replace_task_dependencies(
    connection: &Connection,
    task_id: &str,
    project_root: &str,
    dependencies: &[String],
) -> Result<()> {
    connection.execute(
        "DELETE FROM task_dependencies WHERE task_id = ?1",
        params![task_id],
    )?;
    let mut unique = std::collections::HashSet::new();
    for dependency in dependencies {
        if dependency == task_id || !unique.insert(dependency) {
            return Err(StoreError::InvalidTaskDependency(
                "dependencies must be unique and cannot reference the task itself".into(),
            ));
        }
        let dependency_project: Option<String> = connection
            .query_row(
                "SELECT project_root FROM tasks WHERE id = ?1",
                params![dependency],
                |row| row.get(0),
            )
            .optional()?;
        if dependency_project.as_deref() != Some(project_root) {
            return Err(StoreError::InvalidTaskDependency(format!(
                "task {dependency} does not exist in this project"
            )));
        }
        let creates_cycle: bool = connection.query_row(
            "WITH RECURSIVE prerequisites(id) AS (
                 SELECT depends_on_id FROM task_dependencies WHERE task_id = ?1
                 UNION
                 SELECT edge.depends_on_id
                 FROM task_dependencies edge
                 JOIN prerequisites prior ON edge.task_id = prior.id
             )
             SELECT EXISTS(SELECT 1 FROM prerequisites WHERE id = ?2)",
            params![dependency, task_id],
            |row| row.get(0),
        )?;
        if creates_cycle {
            return Err(StoreError::InvalidTaskDependency(format!(
                "dependency on {dependency} would create a cycle"
            )));
        }
        connection.execute(
            "INSERT INTO task_dependencies (task_id, depends_on_id) VALUES (?1, ?2)",
            params![task_id, dependency],
        )?;
    }
    Ok(())
}

fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskRecord> {
    let state = match row.get::<_, String>(4)?.as_str() {
        "planned" => TaskState::Planned,
        "active" => TaskState::Active,
        "blocked" => TaskState::Blocked,
        "done" => TaskState::Done,
        value => return Err(rusqlite::Error::InvalidParameterName(value.into())),
    };
    Ok(TaskRecord {
        id: row.get(0)?,
        project_root: PathBuf::from(row.get::<_, String>(1)?),
        title: row.get(2)?,
        context: row.get(3)?,
        command: row.get(6)?,
        state,
        priority: match row.get::<_, String>(7)?.as_str() {
            "low" => TaskPriority::Low,
            "normal" => TaskPriority::Normal,
            "high" => TaskPriority::High,
            "urgent" => TaskPriority::Urgent,
            value => return Err(rusqlite::Error::InvalidParameterName(value.into())),
        },
        dependencies: Vec::new(),
        updated_at: row.get(5)?,
    })
}

fn row_to_project(r: &rusqlite::Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        id: r.get(0)?,
        root: PathBuf::from(r.get::<_, String>(1)?),
        name: r.get(2)?,
        last_opened_at: r.get(3)?,
        context_id: r.get(4)?,
    })
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_apply_and_are_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("tf.db");
        let s = Store::open(&db).unwrap();
        assert_eq!(s.schema_version().unwrap(), MIGRATIONS.len() as i64);
        drop(s);
        let s = Store::open(&db).unwrap();
        assert_eq!(s.schema_version().unwrap(), MIGRATIONS.len() as i64);
    }

    #[test]
    fn refuses_newer_schema() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("tf.db");
        {
            let c = Connection::open(&db).unwrap();
            c.pragma_update(None, "user_version", 999).unwrap();
        }
        assert!(matches!(
            Store::open(&db),
            Err(StoreError::TooNew { found: 999, .. })
        ));
    }

    #[test]
    fn project_identity_is_canonical_path() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("My App.v2");
        std::fs::create_dir(&sub).unwrap();
        let s = Store::open_in_memory().unwrap();
        let a = s.open_project(&sub).unwrap();
        let b = s.open_project(&sub.join(".")).unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(a.name, "My App.v2");
        assert_eq!(s.recent_projects(10).unwrap().len(), 1);
    }

    #[test]
    fn project_context_assignment_is_durable_and_clearable() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.open_project(dir.path()).unwrap().context_id, None);
        assert_eq!(
            s.set_project_context(dir.path(), Some("client-a"))
                .unwrap()
                .context_id
                .as_deref(),
            Some("client-a")
        );
        assert_eq!(
            s.open_project(dir.path()).unwrap().context_id.as_deref(),
            Some("client-a")
        );
        assert_eq!(
            s.set_project_context(dir.path(), None).unwrap().context_id,
            None
        );
    }

    #[test]
    fn history_and_workspace_state() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open_in_memory().unwrap();
        let p = s.open_project(dir.path()).unwrap();
        for i in 0..3 {
            s.record_command(
                Some(p.id),
                &CommandRecord {
                    command: format!("cmd{i}"),
                    cwd: None,
                    exit_code: Some(0),
                    started_at: i,
                    duration_ms: Some(5),
                },
            )
            .unwrap();
        }
        let recent = s.recent_commands(p.id, 2).unwrap();
        assert_eq!(
            recent
                .iter()
                .map(|r| r.command.as_str())
                .collect::<Vec<_>>(),
            ["cmd2", "cmd1"]
        );

        assert_eq!(s.load_workspace_state(p.id).unwrap(), None);
        s.save_workspace_state(p.id, r#"{"tabs":1}"#).unwrap();
        s.save_workspace_state(p.id, r#"{"tabs":2}"#).unwrap();
        assert_eq!(
            s.load_workspace_state(p.id).unwrap().as_deref(),
            Some(r#"{"tabs":2}"#)
        );
    }

    #[test]
    fn restorable_sessions_roundtrip_and_update() {
        let store = Store::open_in_memory().unwrap();
        let mut session = RestorableSession {
            id: "session-1".into(),
            project_root: Some(PathBuf::from("/project")),
            profile: "bash".into(),
            ssh_host: None,
            cwd: Some(PathBuf::from("/project/src")),
            cols: 100,
            rows: 30,
            task_id: None,
            context_env: vec![("TERMFORGE_CONTEXT".into(), "client-a".into())],
        };
        store.save_restorable_session(&session).unwrap();
        session.cwd = Some(PathBuf::from("/project/tests"));
        session.cols = 140;
        store.save_restorable_session(&session).unwrap();

        assert_eq!(store.restorable_sessions().unwrap(), vec![session]);
        store
            .update_restorable_session_cwd("session-1", Path::new("/project/live"))
            .unwrap();
        assert_eq!(
            store.restorable_sessions().unwrap()[0].cwd.as_deref(),
            Some(Path::new("/project/live"))
        );
        store.remove_restorable_session("session-1").unwrap();
        assert!(store.restorable_sessions().unwrap().is_empty());
    }

    #[test]
    fn ssh_session_identity_roundtrips_without_a_local_cwd() {
        let store = Store::open_in_memory().unwrap();
        let session = RestorableSession {
            id: "ssh-session".into(),
            project_root: None,
            profile: "SSH devbox".into(),
            ssh_host: Some("devbox".into()),
            cwd: None,
            cols: 120,
            rows: 30,
            task_id: None,
            context_env: Vec::new(),
        };
        store.save_restorable_session(&session).unwrap();
        assert_eq!(store.restorable_sessions().unwrap(), vec![session]);
    }

    #[test]
    fn tasks_are_project_scoped_and_mutable() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory().unwrap();
        let task = TaskRecord {
            id: "task-1".into(),
            project_root: root.path().canonicalize().unwrap(),
            title: "Ship beta".into(),
            context: "Keep the daemon local-first".into(),
            command: Some("cargo test".into()),
            state: TaskState::Planned,
            priority: TaskPriority::Normal,
            dependencies: Vec::new(),
            updated_at: 0,
        };
        store.create_task(&task).unwrap();
        assert!(store.tasks_for_project(other.path()).unwrap().is_empty());
        assert_eq!(store.tasks_for_project(root.path()).unwrap().len(), 1);

        assert!(store
            .update_task(
                "task-1",
                &TaskUpdate {
                    state: Some(TaskState::Active),
                    context: Some("Protocol and storage are stable"),
                    priority: Some(TaskPriority::High),
                    ..TaskUpdate::default()
                },
            )
            .unwrap());
        let updated = store.tasks_for_project(root.path()).unwrap().pop().unwrap();
        assert_eq!(updated.state, TaskState::Active);
        assert_eq!(updated.context, "Protocol and storage are stable");
        assert_eq!(updated.priority, TaskPriority::High);
        // Updating other fields leaves the command alone; "" clears it.
        assert_eq!(updated.command.as_deref(), Some("cargo test"));
        assert!(store
            .update_task(
                "task-1",
                &TaskUpdate {
                    command: Some("cargo clippy"),
                    ..TaskUpdate::default()
                },
            )
            .unwrap());
        let changed = store.tasks_for_project(root.path()).unwrap().pop().unwrap();
        assert_eq!(changed.command.as_deref(), Some("cargo clippy"));
        assert!(store
            .update_task(
                "task-1",
                &TaskUpdate {
                    command: Some(""),
                    ..TaskUpdate::default()
                },
            )
            .unwrap());
        let cleared = store.tasks_for_project(root.path()).unwrap().pop().unwrap();
        assert_eq!(cleared.command, None);

        let dependent = TaskRecord {
            id: "task-2".into(),
            project_root: root.path().canonicalize().unwrap(),
            title: "Publish beta".into(),
            context: String::new(),
            command: None,
            state: TaskState::Planned,
            priority: TaskPriority::Urgent,
            dependencies: vec!["task-1".into()],
            updated_at: 0,
        };
        store.create_task(&dependent).unwrap();
        let tasks = store.tasks_for_project(root.path()).unwrap();
        assert_eq!(
            tasks
                .iter()
                .find(|task| task.id == "task-2")
                .unwrap()
                .dependencies,
            vec!["task-1"]
        );
        assert!(matches!(
            store.update_task(
                "task-1",
                &TaskUpdate {
                    dependencies: Some(&["task-2".into()]),
                    ..TaskUpdate::default()
                },
            ),
            Err(StoreError::InvalidTaskDependency(_))
        ));

        assert!(!store
            .update_task(
                "missing",
                &TaskUpdate {
                    state: Some(TaskState::Done),
                    ..TaskUpdate::default()
                },
            )
            .unwrap());
        assert!(store.delete_task("task-1").unwrap());
        assert!(!store.delete_task("task-1").unwrap());
        let task_two = store.tasks_for_project(root.path()).unwrap().pop().unwrap();
        assert_eq!(task_two.id, "task-2");
        assert!(task_two.dependencies.is_empty());
        assert!(store.delete_task("task-2").unwrap());
        assert!(store.tasks_for_project(root.path()).unwrap().is_empty());
    }
}
