//! Session lifecycle, workspace tabs, attention tracking and layout persistence.

use super::*;

impl TerminalView {
    pub(super) fn start_session(&mut self, reattach: bool) {
        self.start_session_as(if reattach {
            Attach::Matching
        } else {
            Attach::New
        });
    }

    pub(super) fn start_session_as(&mut self, attach: Attach) {
        match SessionHandle::spawn(
            self.spawn.clone(),
            self.project_root.clone(),
            attach,
            Arc::clone(&self.wake),
        ) {
            Ok(session) => {
                self.session = Some(Arc::new(session));
                self.error = None;
                self.last_exit = None;
                self.commands = 0;
            }
            Err(e) => {
                tracing::error!("failed to start shell: {e:#}");
                self.error = Some(format!("{e:#}"));
                self.session = None;
            }
        }
        self.selection = None;
        self.cwd = None;
        self.title = None;
        self.search = None;
        self.palette = None;
        self.history = 0;
    }

    pub(super) fn start(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(session) = &self.session {
            if let Err(error) = session.close() {
                tracing::warn!(%error, "failed to close session before restart");
            }
        }
        self.session = None;
        self.start_session(true);
        self.sync_active_tab();
        self.on_output(cx);
        self.persist_layout();
    }

    pub(super) fn current_tab(&mut self) -> WorkspaceTab {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let pane = self.next_pane_id;
        self.next_pane_id += 1;
        let branch = self.discover_branch();
        WorkspaceTab {
            id,
            session: self.session.clone(),
            spawn: self.spawn.clone(),
            project_root: self.project_root.clone(),
            cwd: self.cwd.clone(),
            title: self.title.clone(),
            last_exit: self.last_exit,
            commands: self.commands,
            error: self.error.clone(),
            history: self.history,
            font_size: self.font_size,
            scroll_offset: 0,
            name: None,
            pinned: false,
            branch,
            attention: None,
            attention_note: None,
            layout: PaneTree::new(pane),
            panes: HashMap::new(),
        }
    }

    /// Branch of the repository the current shell is in, from its reported
    /// directory or, before one is reported, its launch directory.
    pub(super) fn discover_branch(&self) -> Option<String> {
        if self.spawn.ssh_host.is_some() {
            return None;
        }
        let dir = match &self.cwd {
            Some(cwd) => workspace::local_path(cwd),
            None => self
                .project_root
                .clone()
                .or_else(|| self.spawn.cwd.clone())?,
        };
        workspace::git_branch(&dir)
    }

    pub(super) fn sync_active_tab(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        tab.session = self.session.clone();
        tab.spawn = self.spawn.clone();
        tab.project_root = self.project_root.clone();
        tab.cwd = self.cwd.clone();
        tab.title = self.title.clone();
        tab.last_exit = self.last_exit;
        tab.commands = self.commands;
        tab.error = self.error.clone();
        tab.history = self.history;
        tab.font_size = self.font_size;
        if let Some(session) = &self.session {
            tab.scroll_offset = session.with(|p| p.engine().display_offset());
        }
    }

    pub(super) fn select_tab(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if index >= self.tabs.len() || index == self.active_tab {
            return;
        }
        self.sync_active_tab();
        self.renaming = None;
        self.tabs[index].attention = None;
        self.tabs[index].attention_note = None;
        let tab = self.tabs[index].clone();
        self.active_tab = index;
        self.load_tab_state(tab);
        self.on_output(cx);
        self.persist_layout();
    }

    /// Make the view show the focused pane described by `tab`'s flat fields.
    pub(super) fn load_tab_state(&mut self, tab: WorkspaceTab) {
        self.session = tab.session;
        self.spawn = tab.spawn;
        self.project_root = tab.project_root;
        self.cwd = tab.cwd;
        self.title = tab.title;
        self.last_exit = tab.last_exit;
        self.commands = tab.commands;
        self.error = tab.error;
        self.history = tab.history;
        self.font_size = tab.font_size;
        self.selection = None;
        self.search = None;
        self.palette = None;
        if let Some(session) = &self.session {
            session.with_mut(|p| {
                p.engine_mut()
                    .scroll(Scroll::Lines(tab.scroll_offset as i32))
            });
        }
    }

    pub(super) fn new_local_tab(&mut self, cx: &mut Context<'_, Self>) {
        self.sync_active_tab();
        self.spawn = self.local_spawn.clone();
        self.project_root = self.local_project_root.clone();
        self.start_session(false);
        let tab = self.current_tab();
        self.tabs.push(tab);
        self.active_tab = self.tabs.len() - 1;
        self.persist_layout();
        cx.notify();
    }

    /// Open a local terminal running a specific discovered shell.
    pub(super) fn new_shell_tab(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(profile) = self.shells.get(index).cloned() else {
            return;
        };
        self.shell_menu = false;
        self.sync_active_tab();
        self.spawn = self.local_spawn.clone();
        self.spawn.profile = profile;
        self.project_root = self.local_project_root.clone();
        self.start_session(false);
        let tab = self.current_tab();
        self.tabs.push(tab);
        self.active_tab = self.tabs.len() - 1;
        self.persist_layout();
        cx.notify();
    }

    /// Choose the shell used by "New terminal" and the keyboard shortcut.
    pub(super) fn set_default_shell(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if let Some(profile) = self.shells.get(index) {
            self.local_spawn.profile = profile.clone();
            cx.notify();
        }
    }

    pub(super) fn new_ssh_tab(&mut self, host: String, cx: &mut Context<'_, Self>) {
        let Some(profile) = tf_pty::ssh_profile(&host) else {
            self.error = Some("OpenSSH client is not available".into());
            cx.notify();
            return;
        };
        self.sync_active_tab();
        self.spawn = self.local_spawn.clone();
        self.spawn.profile = profile;
        self.spawn.ssh_host = Some(host);
        self.spawn.cwd = None;
        self.project_root = None;
        self.start_session(false);
        let tab = self.current_tab();
        self.tabs.push(tab);
        self.active_tab = self.tabs.len() - 1;
        self.persist_layout();
        cx.notify();
    }

    pub(super) fn close_active_tab(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(session) = &self.session {
            if let Err(error) = session.close() {
                tracing::warn!(%error, "failed to close tab session");
            }
        }
        self.session = None;
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            for slot in tab.panes.drain().map(|(_, slot)| slot) {
                if let Some(session) = slot.session {
                    if let Err(error) = session.close() {
                        tracing::warn!(%error, "failed to close pane session");
                    }
                }
            }
            tab.layout = PaneTree::new(tab.layout.focused());
        }
        if self.tabs.len() == 1 {
            self.spawn = self.local_spawn.clone();
            self.project_root = self.local_project_root.clone();
            self.start(cx);
            return;
        }
        self.tabs.remove(self.active_tab);
        let next = self.active_tab.min(self.tabs.len() - 1);
        self.active_tab = usize::MAX;
        self.select_tab(next, cx);
        self.persist_layout();
    }

    pub(super) fn cycle_tab(&mut self, backwards: bool, cx: &mut Context<'_, Self>) {
        if self.tabs.len() < 2 {
            return;
        }
        let next = if backwards {
            (self.active_tab + self.tabs.len() - 1) % self.tabs.len()
        } else {
            (self.active_tab + 1) % self.tabs.len()
        };
        self.select_tab(next, cx);
    }

    pub(super) fn on_output(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(session) = &self.session {
            for event in session.take_events() {
                match event {
                    SessionEvent::Output(_) => {}
                    SessionEvent::Cwd(cwd) => {
                        self.cwd = Some(match cwd.host {
                            Some(host) => format!("{host}:{}", cwd.path),
                            None => cwd.path,
                        });
                        let branch = self.discover_branch();
                        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                            tab.branch = branch;
                        }
                    }
                    SessionEvent::BlockFinished { exit_code, .. } => {
                        self.last_exit = exit_code;
                        self.commands += 1;
                    }
                    SessionEvent::Notify { title, body } => {
                        tracing::info!(?title, %body, "terminal notification");
                    }
                    SessionEvent::ProgramStatusChanged => {}
                }
            }
            let (title, history) =
                session.with(|p| (p.engine().title(), p.engine().history_size()));
            self.title = title;
            // Scrollback shrank (`clear`): absolute positions now point at
            // different content, so drop anything anchored to them.
            if history < self.history {
                self.selection = None;
                if let Some(s) = &mut self.search {
                    s.matches.clear();
                    s.current = None;
                }
            }
            self.history = history;
            if let Some(s) = &mut self.search {
                s.stale = true;
            }
        }
        self.drain_background_tabs();
        self.drain_parked_panes();
        self.sync_active_tab();
        cx.notify();
    }

    /// Background workspaces keep producing events; consume them so their
    /// sidebar rows stay current and can ask for attention.
    pub(super) fn drain_background_tabs(&mut self) {
        let active = self.active_tab;
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            if index == active {
                continue;
            }
            let Some(session) = tab.session.clone() else {
                continue;
            };
            for event in session.take_events() {
                match event {
                    SessionEvent::Cwd(cwd) => {
                        tab.cwd = Some(match cwd.host {
                            Some(host) => format!("{host}:{}", cwd.path),
                            None => cwd.path,
                        });
                        if tab.spawn.ssh_host.is_none() {
                            tab.branch = tab
                                .cwd
                                .as_deref()
                                .map(workspace::local_path)
                                .and_then(|dir| workspace::git_branch(&dir));
                        }
                    }
                    SessionEvent::BlockFinished { exit_code, .. } => {
                        tab.last_exit = exit_code;
                        tab.commands += 1;
                        match exit_code {
                            Some(code) if code != 0 => Self::raise_attention(
                                tab,
                                Attention::Error,
                                Some(format!("command failed (exit {code})")),
                            ),
                            _ => Self::raise_attention(
                                tab,
                                Attention::Done,
                                Some("command finished".into()),
                            ),
                        }
                    }
                    SessionEvent::Notify { title, body } => {
                        let note =
                            title.map_or_else(|| body.clone(), |title| format!("{title}: {body}"));
                        Self::raise_attention(tab, Attention::Notice, Some(note));
                    }
                    SessionEvent::ProgramStatusChanged => {
                        if let Some((attention, note)) = Self::status_attention(&session) {
                            Self::raise_attention(tab, attention, note);
                        }
                    }
                    SessionEvent::Output(_) => {}
                }
            }
        }
    }

    /// Parked panes (every unfocused pane of every workspace) keep producing
    /// events too. Their state is kept current, and a pane in a workspace the
    /// user is not looking at raises that workspace's attention.
    pub(super) fn drain_parked_panes(&mut self) {
        let active = self.active_tab;
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            let ids: Vec<PaneId> = tab.panes.keys().copied().collect();
            for id in ids {
                let Some(slot) = tab.panes.get_mut(&id) else {
                    continue;
                };
                let Some(session) = slot.session.clone() else {
                    continue;
                };
                let mut raised: Vec<(Attention, Option<String>)> = Vec::new();
                for event in session.take_events() {
                    match event {
                        SessionEvent::Cwd(cwd) => {
                            slot.cwd = Some(match cwd.host {
                                Some(host) => format!("{host}:{}", cwd.path),
                                None => cwd.path,
                            });
                        }
                        SessionEvent::BlockFinished { exit_code, .. } => {
                            slot.last_exit = exit_code;
                            slot.commands += 1;
                            raised.push(match exit_code {
                                Some(code) if code != 0 => (
                                    Attention::Error,
                                    Some(format!("command failed (exit {code})")),
                                ),
                                _ => (Attention::Done, Some("command finished".into())),
                            });
                        }
                        SessionEvent::Notify { title, body } => {
                            let note = title
                                .map_or_else(|| body.clone(), |title| format!("{title}: {body}"));
                            raised.push((Attention::Notice, Some(note)));
                        }
                        SessionEvent::ProgramStatusChanged => {
                            if let Some(found) = Self::status_attention(&session) {
                                raised.push(found);
                            }
                        }
                        SessionEvent::Output(_) => {}
                    }
                }
                if index != active {
                    for (level, note) in raised {
                        Self::raise_attention(tab, level, note);
                    }
                }
            }
        }
    }

    /// The attention level implied by a session's reported program status.
    pub(super) fn status_attention(session: &SessionHandle) -> Option<(Attention, Option<String>)> {
        session.with(|p| {
            p.program_status()
                .iter()
                .filter_map(|record| {
                    let attention = match record.report.state {
                        tf_session::ProgramState::Blocked => Attention::Blocked,
                        tf_session::ProgramState::Error => Attention::Error,
                        tf_session::ProgramState::Done => Attention::Done,
                        _ => return None,
                    };
                    let note = record
                        .report
                        .message
                        .clone()
                        .or_else(|| record.report.title.clone())
                        .or_else(|| record.report.app.clone());
                    Some((attention, note))
                })
                .max_by_key(|(attention, _)| *attention)
        })
    }

    /// Raise, never lower: a blocked workspace must not be hidden by a later
    /// "command finished".
    pub(super) fn raise_attention(tab: &mut WorkspaceTab, level: Attention, note: Option<String>) {
        if tab.attention.is_none_or(|current| level >= current) {
            tab.attention = Some(level);
            tab.attention_note = note;
        }
    }

    /// Jump to the workspace that most needs the user, preferring blocked
    /// over errors over finished, and the nearest one after the current tab.
    pub(super) fn jump_to_attention(&mut self, cx: &mut Context<'_, Self>) {
        let count = self.tabs.len();
        let mut best: Option<(Attention, usize)> = None;
        for offset in 1..=count {
            let index = (self.active_tab + offset) % count;
            if let Some(level) = self.tabs[index].attention {
                if best.is_none_or(|(current, _)| level > current) {
                    best = Some((level, index));
                }
            }
        }
        if let Some((_, index)) = best {
            self.select_tab(index, cx);
        }
    }

    // ---- workspace metadata: rename, pin, reorder, persistence -----------

    pub(super) fn begin_rename(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        let text = tab
            .name
            .clone()
            .or_else(|| tab.title.clone())
            .unwrap_or_else(|| Self::tab_label(tab));
        self.renaming = Some(RenameState {
            tab_id: tab.id,
            text,
        });
        cx.notify();
    }

    pub(super) fn rename_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<'_, Self>) {
        let key = ev.keystroke.key.as_str();
        let modifiers = ev.keystroke.modifiers;
        let Some(rename) = &mut self.renaming else {
            return;
        };
        match key {
            "escape" => self.renaming = None,
            "enter" => {
                let name = rename.text.trim().to_owned();
                let tab_id = rename.tab_id;
                self.renaming = None;
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                    tab.name = (!name.is_empty()).then_some(name);
                }
                self.persist_layout();
            }
            "backspace" => {
                rename.text.pop();
            }
            _ if !modifiers.control && !modifiers.alt => {
                if let Some(text) = ev.keystroke.key_char.as_deref() {
                    if !text.chars().any(char::is_control) {
                        rename.text.push_str(text);
                    }
                }
            }
            _ => {}
        }
        cx.notify();
    }

    /// Keep pinned workspaces above unpinned ones, preserving relative order,
    /// and keep `active_tab` pointing at the same workspace.
    pub(super) fn normalize_tab_order(&mut self) {
        let active_id = self.tabs.get(self.active_tab).map(|tab| tab.id);
        self.tabs.sort_by_key(|tab| !tab.pinned);
        if let Some(id) = active_id {
            if let Some(index) = self.tabs.iter().position(|tab| tab.id == id) {
                self.active_tab = index;
            }
        }
    }

    pub(super) fn toggle_pin(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        tab.pinned = !tab.pinned;
        self.normalize_tab_order();
        self.persist_layout();
        cx.notify();
    }

    pub(super) fn move_tab(&mut self, from: usize, to: usize, cx: &mut Context<'_, Self>) {
        if from >= self.tabs.len() || to >= self.tabs.len() || from == to {
            return;
        }
        let active_id = self.tabs.get(self.active_tab).map(|tab| tab.id);
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        if let Some(id) = active_id {
            if let Some(index) = self.tabs.iter().position(|tab| tab.id == id) {
                self.active_tab = index;
            }
        }
        self.normalize_tab_order();
        self.persist_layout();
        cx.notify();
    }

    pub(super) fn move_active_tab(&mut self, down: bool, cx: &mut Context<'_, Self>) {
        let from = self.active_tab;
        let to = if down {
            from + 1
        } else {
            from.saturating_sub(1)
        };
        self.move_tab(from, to, cx);
    }

    /// Hand the layout to the daemon so the next launch restores it.
    pub(super) fn persist_layout(&mut self) {
        let Some(root) = self.local_project_root.clone() else {
            return;
        };
        let tabs: Vec<SavedTab> = self
            .tabs
            .iter()
            .filter_map(|tab| {
                let id = tab.session.as_ref()?.id()?;
                Some(SavedTab {
                    session: id.to_string(),
                    pinned: tab.pinned,
                    name: tab.name.clone(),
                    panes: Self::saved_panes(tab),
                })
            })
            .collect();
        let active = self
            .tabs
            .get(self.active_tab)
            .and_then(|tab| tab.session.as_ref()?.id())
            .map(|id| id.to_string());
        let encoded = Layout { tabs, active }.encode();
        if encoded != self.saved_layout {
            self.saved_layout.clone_from(&encoded);
            remote_session::save_workspace_state(root, encoded);
        }
    }

    /// The split layout of a workspace in a form that survives restarts:
    /// leaves are positions in the session list, not runtime pane ids.
    /// `None` for a single pane, or if any pane has no daemon session.
    fn saved_panes(tab: &WorkspaceTab) -> Option<SavedPanes> {
        if tab.layout.len() < 2 {
            return None;
        }
        let ids = tab.layout.ids();
        let focused = tab.layout.focused();
        let mut sessions = Vec::with_capacity(ids.len());
        for id in &ids {
            let session = if *id == focused {
                tab.session.as_ref()
            } else {
                tab.panes.get(id)?.session.as_ref()
            };
            sessions.push(session?.id()?.to_string());
        }
        let position = |id: PaneId| ids.iter().position(|p| *p == id).unwrap_or(0) as PaneId;
        Some(SavedPanes {
            tree: tab.layout.remap(position).encode(),
            focus: position(focused) as usize,
            sessions,
        })
    }

    /// Reattach to the workspaces the daemon kept alive, in the saved order;
    /// fall back to a single fresh (or matching) session.
    pub(super) fn restore_workspace(&mut self, cx: &mut Context<'_, Self>) {
        let restored = self.restore_from_daemon(cx);
        if !restored {
            self.start_session(true);
            let tab = self.current_tab();
            self.tabs.push(tab);
        }
    }

    /// Point the view's spawn settings at a daemon session and attach to it.
    /// Returns false when the session cannot be reached from here (an SSH
    /// host without a client), leaving the view's session untouched.
    fn attach_saved_session(&mut self, info: &tf_proto::SessionInfo) -> bool {
        self.spawn = self.local_spawn.clone();
        self.project_root = info.project_root.clone();
        if let Some(shell) = self.shells.iter().find(|shell| shell.name == info.shell) {
            self.spawn.profile = shell.clone();
        }
        if let Some(host) = info.ssh_host.clone() {
            let Some(profile) = tf_pty::ssh_profile(&host) else {
                return false;
            };
            self.spawn.profile = profile;
            self.spawn.ssh_host = Some(host);
            self.spawn.cwd = None;
        }
        self.start_session_as(Attach::Session(info.id));
        true
    }

    pub(super) fn restore_from_daemon(&mut self, cx: &mut Context<'_, Self>) -> bool {
        struct Planned {
            info: tf_proto::SessionInfo,
            name: Option<String>,
            pinned: bool,
            /// Pane tree (leaves are indices into the list) and its sessions.
            split: Option<(PaneTree, Vec<tf_proto::SessionInfo>)>,
        }

        let Some(root) = self.local_project_root.clone() else {
            return false;
        };
        let Some(stored) = remote_session::load_workspace(Some(root.clone())) else {
            return false;
        };
        let layout = stored
            .layout
            .as_deref()
            .map(Layout::decode)
            .unwrap_or_default();
        let running = |id: &str| {
            stored
                .sessions
                .iter()
                .find(|session| session.running && session.id.to_string() == id)
        };
        // A saved split is only restored whole: if the tree is malformed or
        // any pane's session is gone, the workspace comes back as a single
        // pane and the surviving sessions become their own workspaces.
        let restore_split = |saved: &SavedTab| -> Option<(PaneTree, Vec<tf_proto::SessionInfo>)> {
            let panes = saved.panes.as_ref()?;
            let tree = PaneTree::decode(&panes.tree)?;
            let mut ids = tree.ids();
            ids.sort_unstable();
            if ids != (0..panes.sessions.len() as PaneId).collect::<Vec<_>>()
                || panes.sessions.get(panes.focus) != Some(&saved.session)
            {
                return None;
            }
            let infos = panes
                .sessions
                .iter()
                .map(|id| running(id).cloned())
                .collect::<Option<Vec<_>>>()?;
            let mut tree = tree;
            tree.set_focus(panes.focus as PaneId);
            Some((tree, infos))
        };

        let mut plan: Vec<Planned> = Vec::new();
        for saved in &layout.tabs {
            if let Some(info) = running(&saved.session) {
                plan.push(Planned {
                    info: info.clone(),
                    name: saved.name.clone(),
                    pinned: saved.pinned,
                    split: restore_split(saved),
                });
            }
        }
        for info in &stored.sessions {
            let known = plan.iter().any(|planned| {
                planned.info.id == info.id
                    || planned
                        .split
                        .as_ref()
                        .is_some_and(|(_, infos)| infos.iter().any(|i| i.id == info.id))
            });
            if info.running && !known && info.project_root.as_ref() == Some(&root) {
                plan.push(Planned {
                    info: info.clone(),
                    name: None,
                    pinned: false,
                    split: None,
                });
            }
        }
        if plan.is_empty() {
            return false;
        }
        let mut active_index = 0;
        for planned in plan {
            let Planned {
                info,
                name,
                pinned,
                split,
            } = planned;
            let mut parked: HashMap<PaneId, PaneSlot> = HashMap::new();
            let mut layout_tree = None;
            let mut complete = true;
            if let Some((tree, infos)) = split {
                // Attach every pane but the focused one first and park it;
                // the focused pane is attached last so it is what the view
                // holds when the tab is captured below.
                let base = self.next_pane_id;
                self.next_pane_id += infos.len() as PaneId;
                let focused = tree.focused() as usize;
                for (index, pane) in infos.iter().enumerate() {
                    if index == focused {
                        continue;
                    }
                    if !self.attach_saved_session(pane) {
                        complete = false;
                        break;
                    }
                    parked.insert(base + index as PaneId, PaneSlot::from_view(self));
                }
                layout_tree = complete.then(|| tree.remap(|index| base + index));
            }
            if !self.attach_saved_session(&info) {
                // Dropping the handles detaches; the daemon keeps the shells.
                continue;
            }
            let mut tab = self.current_tab();
            tab.name = name;
            tab.pinned = pinned;
            if let Some(tree) = layout_tree {
                tab.layout = tree;
                tab.panes = parked;
            }
            if layout.active.as_deref() == Some(&info.id.to_string()) {
                active_index = self.tabs.len();
            }
            self.tabs.push(tab);
        }
        if self.tabs.is_empty() {
            return false;
        }
        self.normalize_tab_order();
        let target = active_index.min(self.tabs.len() - 1);
        self.active_tab = usize::MAX;
        self.select_tab(target, cx);
        true
    }
}
