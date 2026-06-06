//! Session persistence: write a snapshot of open sessions to disk after every
//! layout change and restore them on the next startup.
//!
//! Snapshot file: `%APPDATA%\TermForge\session_snapshot.json`
//!
//! The snapshot records just enough to re-open each session — no grid state or
//! scrollback, since those are ephemeral.  SSH sessions are recorded but not
//! auto-reconnected (the user must confirm); agents and locals are re-spawned.

use std::path::PathBuf;

use anyhow::Result;
use libterm::mux::session::SessionKind;
use serde::{Deserialize, Serialize};

use crate::entry::Entry;

// ── Data types ────────────────────────────────────────────────────────────────

/// One session's resurrection data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub kind: SessionRecordKind,
    pub title: String,
    pub cols: u16,
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

// ── Helpers called by AppState ────────────────────────────────────────────────

/// Restore sessions from a snapshot, or fall back to spawning one default shell.
/// Returns `(entries, active_tab)`.
pub fn restore_or_default(
    shell: &str,
    fallback_cols: u16,
    fallback_rows: u16,
    _rt: &tokio::runtime::Runtime,
) -> (Vec<Entry>, usize) {
    if let Some(snap) = Snapshot::load() {
        let mut entries = Vec::new();
        for rec in &snap.sessions {
            let result: Result<Entry> = match &rec.kind {
                SessionRecordKind::Local { command } => {
                    Entry::spawn_local(command, rec.cols, rec.rows)
                }
                SessionRecordKind::Agent { command, model } => {
                    Entry::spawn_agent(command, model)
                }
                SessionRecordKind::Ssh { .. } => {
                    // SSH sessions are not auto-reconnected on startup.
                    Entry::spawn_local(shell, rec.cols, rec.rows)
                }
            };
            match result {
                Ok(e) => entries.push(e),
                Err(err) => tracing::warn!("restore session failed: {err}"),
            }
        }
        if !entries.is_empty() {
            let active = snap.active.min(entries.len() - 1);
            tracing::info!("restored {} session(s) from snapshot", entries.len());
            return (entries, active);
        }
    }
    match Entry::spawn_local(shell, fallback_cols, fallback_rows) {
        Ok(e) => (vec![e], 0),
        Err(err) => {
            tracing::error!("failed to spawn default shell: {err}");
            (Vec::new(), 0)
        }
    }
}

/// Serialize current entries into a Snapshot and persist to disk.
pub fn persist(entries: &[Entry], active_tab: usize, default_shell: &str) {
    let sessions: Vec<SessionRecord> = entries
        .iter()
        .map(|e| {
            let (cols, rows) = (e.session.grid.cols(), e.session.grid.rows());
            let kind = match &e.session.kind {
                SessionKind::Local => SessionRecordKind::Local {
                    command: default_shell.to_string(),
                },
                SessionKind::Ssh { host, user } => SessionRecordKind::Ssh {
                    host: host.clone(),
                    user: user.clone(),
                    port: 22,
                },
                SessionKind::Agent { model, .. } => SessionRecordKind::Agent {
                    command: e.session.title.clone(),
                    model: model.clone(),
                },
            };
            SessionRecord { kind, title: e.session.title.clone(), cols, rows }
        })
        .collect();

    Snapshot {
        version: Snapshot::VERSION,
        active: active_tab.min(sessions.len().saturating_sub(1)),
        sessions,
    }
    .save();
}
