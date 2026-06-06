//! Session persistence: write a snapshot of open sessions to disk after every
//! layout change and restore them on the next startup.
//!
//! Snapshot file: `%APPDATA%\TermForge\session_snapshot.json`
//!
//! The snapshot records just enough to re-open each session — no grid state or
//! scrollback, since those are ephemeral.  SSH sessions are recorded but not
//! auto-reconnected (the user must confirm); agents and locals are re-spawned.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// One session's resurrection data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    /// Session kind tag.
    pub kind: SessionRecordKind,
    /// Display title.
    pub title: String,
    /// Grid columns at time of snapshot.
    pub cols: u16,
    /// Grid rows at time of snapshot.
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SessionRecordKind {
    Local { command: String },
    Ssh { host: String, user: String, port: u16 },
    Agent { command: String, model: String },
}

/// Top-level snapshot file.
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Snapshot {
    /// Format version — bump if the schema changes incompatibly.
    pub version: u32,
    /// Index of the session that was active when the snapshot was taken.
    pub active: usize,
    pub sessions: Vec<SessionRecord>,
}

impl Snapshot {
    pub const VERSION: u32 = 1;

    /// Absolute path to `%APPDATA%\TermForge\session_snapshot.json`.
    pub fn path() -> PathBuf {
        let base = std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));
        base.join("TermForge").join("session_snapshot.json")
    }

    /// Persist to disk.  Silently ignores errors so a failed write never crashes the app.
    pub fn save(&self) {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, json);
        }
    }

    /// Load from disk.  Returns `None` if the file is absent, unreadable, or
    /// has an incompatible version.
    pub fn load() -> Option<Self> {
        let path = Self::path();
        let data = std::fs::read_to_string(&path).ok()?;
        let snap: Snapshot = serde_json::from_str(&data).ok()?;
        if snap.version != Self::VERSION {
            return None;
        }
        if snap.sessions.is_empty() {
            return None;
        }
        Some(snap)
    }
}
