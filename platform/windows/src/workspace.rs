//! Workspace management.
//!
//! Each workspace owns an independent list of sessions and remembers which
//! tab was active when the user last switched away.  Workspaces are switched
//! by swapping the working copies of `entries` and `active_tab` in `AppState`.

use crate::entry::Entry;

/// Fixed workspace names shown in the sidebar.
pub const WORKSPACE_NAMES: [&str; 4] = [
    "work / backend",
    "infra / servers",
    "worktrees",
    "personal",
];

/// Persistent slot for one workspace: its sessions and last-active tab index.
pub struct WorkspaceSlot {
    pub entries: Vec<Entry>,
    pub active_tab: usize,
}

impl WorkspaceSlot {
    pub fn empty() -> Self {
        Self { entries: Vec::new(), active_tab: 0 }
    }
}
