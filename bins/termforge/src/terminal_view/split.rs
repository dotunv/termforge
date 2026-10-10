//! Split panes within a workspace (ADR 0012).
//!
//! The view always shows one *focused* pane through its flat fields (the
//! same fields a workspace tab mirrors). Every other pane of the workspace is
//! parked in `WorkspaceTab::panes`; moving focus swaps the two, exactly like
//! switching workspaces does, so per-pane state is never duplicated.

use super::*;

/// Keep a runaway shortcut from creating an unusable grid of tiny panes.
const MAX_PANES: usize = 8;

impl PaneSlot {
    /// The focused pane's state as the tab currently mirrors it.
    fn from_tab(tab: &WorkspaceTab) -> Self {
        Self {
            session: tab.session.clone(),
            spawn: tab.spawn.clone(),
            project_root: tab.project_root.clone(),
            cwd: tab.cwd.clone(),
            title: tab.title.clone(),
            last_exit: tab.last_exit,
            commands: tab.commands,
            error: tab.error.clone(),
            history: tab.history,
            font_size: tab.font_size,
            scroll_offset: tab.scroll_offset,
            attention: None,
        }
    }

    /// Whatever session the view currently holds.
    pub(super) fn from_view(view: &TerminalView) -> Self {
        Self {
            session: view.session.clone(),
            spawn: view.spawn.clone(),
            project_root: view.project_root.clone(),
            cwd: view.cwd.clone(),
            title: view.title.clone(),
            last_exit: view.last_exit,
            commands: view.commands,
            error: view.error.clone(),
            history: view.history,
            font_size: view.font_size,
            scroll_offset: view
                .session
                .as_ref()
                .map_or(0, |s| s.with(|p| p.engine().display_offset())),
            attention: None,
        }
    }

    fn apply_to(self, tab: &mut WorkspaceTab) {
        tab.session = self.session;
        tab.spawn = self.spawn;
        tab.project_root = self.project_root;
        tab.cwd = self.cwd;
        tab.title = self.title;
        tab.last_exit = self.last_exit;
        tab.commands = self.commands;
        tab.error = self.error;
        tab.history = self.history;
        tab.font_size = self.font_size;
        tab.scroll_offset = self.scroll_offset;
    }
}

impl TerminalView {
    /// Split the focused pane. The new pane opens a fresh shell, in the same
    /// directory when that is known and local, and takes focus.
    pub(super) fn split_pane(&mut self, axis: Axis, cx: &mut Context<'_, Self>) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        if tab.layout.len() >= MAX_PANES {
            return;
        }
        self.sync_active_tab();
        let focused = self.tabs[self.active_tab].layout.focused();
        let parked = PaneSlot::from_tab(&self.tabs[self.active_tab]);
        if self.spawn.ssh_host.is_none() {
            if let Some(cwd) = &self.cwd {
                self.spawn.cwd = Some(workspace::local_path(cwd));
            }
        }
        self.start_session(false);
        let new_id = self.next_pane_id;
        self.next_pane_id += 1;
        let tab = &mut self.tabs[self.active_tab];
        tab.panes.insert(focused, parked);
        tab.layout.split(focused, axis, new_id);
        self.sync_active_tab();
        self.persist_layout();
        cx.notify();
    }

    /// Give focus to a parked pane of the active workspace.
    pub(super) fn focus_pane(&mut self, id: PaneId, cx: &mut Context<'_, Self>) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        if tab.layout.focused() == id || !tab.panes.contains_key(&id) {
            return;
        }
        self.sync_active_tab();
        let tab = &mut self.tabs[self.active_tab];
        let Some(slot) = tab.panes.remove(&id) else {
            return;
        };
        let previous = tab.layout.focused();
        tab.panes.insert(previous, PaneSlot::from_tab(tab));
        tab.layout.set_focus(id);
        slot.apply_to(tab);
        // A pane the user returns to has been seen.
        tab.attention = None;
        tab.attention_note = None;
        let tab = tab.clone();
        self.load_tab_state(tab);
        self.on_output(cx);
        self.persist_layout();
    }

    /// Move focus to the pane that is visibly in `dir`.
    pub(super) fn focus_direction(&mut self, dir: Dir, cx: &mut Context<'_, Self>) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        // Adjacency depends only on the split ratios, so any area works.
        let area = PaneRect::new(0.0, 0.0, 1000.0, 600.0);
        if let Some(target) = tab.layout.neighbor(tab.layout.focused(), dir, area) {
            self.focus_pane(target, cx);
        }
    }

    /// Cycle focus through the panes in reading order.
    pub(super) fn cycle_pane(&mut self, cx: &mut Context<'_, Self>) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        let ids = tab.layout.ids();
        if ids.len() < 2 {
            return;
        }
        let at = ids
            .iter()
            .position(|id| *id == tab.layout.focused())
            .unwrap_or(0);
        self.focus_pane(ids[(at + 1) % ids.len()], cx);
    }

    /// Close the focused pane and its session. The last pane of a workspace
    /// closes the workspace instead.
    pub(super) fn close_pane(&mut self, cx: &mut Context<'_, Self>) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        if tab.layout.len() < 2 {
            self.close_active_tab(cx);
            return;
        }
        if let Some(session) = &self.session {
            if let Err(error) = session.close() {
                tracing::warn!(%error, "failed to close pane session");
            }
        }
        let tab = &mut self.tabs[self.active_tab];
        let closing = tab.layout.focused();
        let Some(next) = tab.layout.close(closing) else {
            return;
        };
        let Some(slot) = tab.panes.remove(&next) else {
            return;
        };
        slot.apply_to(tab);
        let tab = tab.clone();
        self.load_tab_state(tab);
        self.on_output(cx);
        self.persist_layout();
    }

    /// Wheel over a pane that is not focused scrolls that pane's scrollback.
    /// Full-screen programs in it are not sent keys; focus the pane for that.
    pub(super) fn scroll_parked(
        &mut self,
        id: PaneId,
        ev: &ScrollWheelEvent,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(session) = self
            .tabs
            .get(self.active_tab)
            .and_then(|tab| tab.panes.get(&id))
            .and_then(|slot| slot.session.clone())
        else {
            return;
        };
        let line_h = self.metrics(window).line_h;
        self.scroll_accum += ev.delta.pixel_delta(line_h).y / line_h;
        let lines = self.scroll_accum.trunc() as i32;
        if lines == 0 {
            return;
        }
        self.scroll_accum -= lines as f32;
        session.with_mut(|p| p.engine_mut().scroll(Scroll::Lines(lines * 3)));
        cx.notify();
    }

    /// Start dragging a divider (from `PaneTree::dividers`).
    pub(super) fn begin_divider_drag(&mut self, divider: &crate::panes::Divider) {
        self.divider_drag = Some(DividerDrag {
            path: divider.path.clone(),
            axis: divider.axis,
            parent: divider.parent,
        });
    }

    /// Move the dragged divider under the pointer.
    pub(super) fn drag_divider(
        &mut self,
        position: gpui::Point<Pixels>,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(drag) = &self.divider_drag else {
            return;
        };
        let area = self.pane_area;
        if area.size.width <= px(1.0) || area.size.height <= px(1.0) {
            return;
        }
        // Pointer as a fraction of the whole pane area, then of the split.
        let fx = (position.x - area.origin.x) / area.size.width;
        let fy = (position.y - area.origin.y) / area.size.height;
        let ratio = match drag.axis {
            Axis::Horizontal => (fx - drag.parent.x) / drag.parent.w,
            Axis::Vertical => (fy - drag.parent.y) / drag.parent.h,
        };
        let path = drag.path.clone();
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            if tab.layout.set_ratio(&path, ratio) {
                cx.notify();
            }
        }
    }

    /// Finish a divider drag and remember the result.
    pub(super) fn end_divider_drag(&mut self, cx: &mut Context<'_, Self>) {
        if self.divider_drag.take().is_some() {
            self.persist_layout();
            cx.notify();
        }
    }

    /// Grow the focused pane towards `dir` by a small step.
    pub(super) fn resize_pane(&mut self, dir: Dir, cx: &mut Context<'_, Self>) {
        const STEP: f32 = 0.05;
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        let focused = tab.layout.focused();
        if tab.layout.resize(focused, dir, STEP) {
            self.persist_layout();
            cx.notify();
        }
    }
}
