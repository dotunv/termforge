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
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub id: i64,
    pub root: PathBuf,
    pub name: String,
    pub last_opened_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRecord {
    pub command: String,
    pub cwd: Option<String>,
    pub exit_code: Option<i32>,
    pub started_at: i64,
    pub duration_ms: Option<i64>,
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
            "SELECT id, root, name, last_opened_at FROM projects WHERE root = ?1",
            params![root_str],
            row_to_project,
        )?)
    }

    pub fn recent_projects(&self, limit: usize) -> Result<Vec<Project>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, root, name, last_opened_at FROM projects
             ORDER BY last_opened_at DESC, id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], row_to_project)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
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
}

fn row_to_project(r: &rusqlite::Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        id: r.get(0)?,
        root: PathBuf::from(r.get::<_, String>(1)?),
        name: r.get(2)?,
        last_opened_at: r.get(3)?,
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
}
