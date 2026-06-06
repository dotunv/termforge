use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use uuid::Uuid;

use crate::block::store::{BlockStore, SessionId};
use crate::grid::TerminalGrid;

pub type TabId = Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionKind {
    /// Local PTY session (green).
    Local,
    /// SSH session (blue).
    Ssh { host: String, user: String },
    /// AI agent session (purple) — runs any CLI tool as a structured session.
    Agent {
        /// Short display name for the session (usually the command basename).
        name: String,
        /// Model / engine label, e.g. "claude" or "gpt-4o".  May be empty.
        model: String,
    },
}

pub struct Session {
    pub id: SessionId,
    pub kind: SessionKind,
    pub title: String,
    pub grid: TerminalGrid,
    pub blocks: BlockStore,
    /// Set to true whenever the grid changes; compositor clears it after drawing.
    pub dirty: Arc<AtomicBool>,
}

impl Session {
    pub fn new(kind: SessionKind, cols: u16, rows: u16) -> Self {
        let id = Uuid::new_v4();
        let title = match &kind {
            SessionKind::Local => "local".to_string(),
            SessionKind::Ssh { host, user } => format!("{user}@{host}"),
            SessionKind::Agent { name, .. } => format!("agent:{name}"),
        };
        Self {
            id,
            kind,
            title,
            grid: TerminalGrid::new(cols, rows),
            blocks: BlockStore::default(),
            dirty: Arc::new(AtomicBool::new(true)),
        }
    }

    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Relaxed);
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Relaxed)
    }

    pub fn clear_dirty(&self) {
        self.dirty.store(false, Ordering::Relaxed);
    }

    /// Shallow render copy: clones the grid and shares the dirty flag.
    /// Used to hand a snapshot to the compositor without cloning block history.
    pub fn clone_for_render(&self) -> Session {
        Session {
            id: self.id,
            kind: self.kind.clone(),
            title: self.title.clone(),
            grid: self.grid.clone_grid(),
            blocks: BlockStore::default(),
            dirty: self.dirty.clone(),
        }
    }
}
