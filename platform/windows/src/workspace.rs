//! Workspace management.
//!
//! Each workspace owns an independent list of sessions and remembers which
//! tab was active when the user last switched away.  Workspaces are switched
//! by swapping the working copies of `entries` and `active_tab` in `AppState`.
//!
//! Workspaces are created on demand — the app starts with a single "main"
//! workspace and the user adds more via the sidebar.  There is no pre-populated
//! demo data.

use crate::entry::Entry;

/// Name of the workspace the app starts with.
pub const DEFAULT_WORKSPACE_NAME: &str = "main";

/// Persistent slot for one workspace: its name, sessions, and last-active tab.
pub struct WorkspaceSlot {
    pub name: String,
    pub entries: Vec<Entry>,
    pub active_tab: usize,
}

impl WorkspaceSlot {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), entries: Vec::new(), active_tab: 0 }
    }
}
