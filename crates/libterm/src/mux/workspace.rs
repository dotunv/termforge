use uuid::Uuid;
use super::session::{Session, TabId};

pub type WorkspaceId = Uuid;

/// A workspace groups a set of tabs. Switching workspaces swaps the entire tab bar.
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    /// Ordered list of session IDs representing open tabs.
    pub tabs: Vec<TabId>,
    pub active_tab: usize,
}

impl Workspace {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            tabs: Vec::new(),
            active_tab: 0,
        }
    }

    pub fn add_tab(&mut self, session_id: TabId) {
        self.tabs.push(session_id);
        self.active_tab = self.tabs.len() - 1;
    }

    pub fn remove_tab(&mut self, session_id: TabId) {
        self.tabs.retain(|id| *id != session_id);
        if self.active_tab >= self.tabs.len() && !self.tabs.is_empty() {
            self.active_tab = self.tabs.len() - 1;
        }
    }

    pub fn active_session_id(&self) -> Option<TabId> {
        self.tabs.get(self.active_tab).copied()
    }
}

/// Top-level multiplexer: holds all workspaces and sessions.
pub struct Multiplexer {
    pub workspaces: Vec<Workspace>,
    pub sessions: Vec<Session>,
    pub active_workspace: usize,
}

impl Multiplexer {
    pub fn new() -> Self {
        let default_ws = Workspace::new("default");
        Self {
            workspaces: vec![default_ws],
            sessions: Vec::new(),
            active_workspace: 0,
        }
    }

    pub fn add_workspace(&mut self, name: impl Into<String>) -> WorkspaceId {
        let ws = Workspace::new(name);
        let id = ws.id;
        self.workspaces.push(ws);
        id
    }

    pub fn add_session(&mut self, workspace_id: WorkspaceId, session: Session) {
        let session_id = session.id;
        self.sessions.push(session);
        if let Some(ws) = self.workspaces.iter_mut().find(|w| w.id == workspace_id) {
            ws.add_tab(session_id);
        }
    }

    pub fn session(&self, id: uuid::Uuid) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == id)
    }

    pub fn session_mut(&mut self, id: uuid::Uuid) -> Option<&mut Session> {
        self.sessions.iter_mut().find(|s| s.id == id)
    }

    pub fn active_workspace(&self) -> &Workspace {
        &self.workspaces[self.active_workspace]
    }
}

impl Default for Multiplexer {
    fn default() -> Self {
        Self::new()
    }
}
