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

    /// Replace a workspace's focused-pane mirror with this parked pane.
    ///
    /// A `WorkspaceTab` keeps its focused pane in its flat fields and every
    /// other pane in `panes`. Keeping this write in one place makes a focus
    /// transition an explicit swap instead of a sequence of partial updates.
    fn make_focused(self, tab: &mut WorkspaceTab) {
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

impl WorkspaceTab {
    /// Store the state currently mirrored by this tab as a non-focused pane.
    fn park_focused(&mut self) {
        let focused = self.layout.focused();
        let replaced = self.panes.insert(focused, PaneSlot::from_tab(self));
        debug_assert!(replaced.is_none(), "focused pane was already parked");
    }

    /// Promote a parked pane to the tab's focused-pane mirror. The former
    /// focused pane is parked first, so this operation cannot lose a live
    /// session while changing focus.
    fn focus_parked(&mut self, id: PaneId) -> bool {
        if self.layout.focused() == id {
            return false;
        }
        let Some(next) = self.panes.remove(&id) else {
            return false;
        };
        self.park_focused();
        debug_assert!(self.layout.set_focus(id));
        next.make_focused(self);
        true
    }

    /// Remove the focused leaf and promote the sibling chosen by the tree.
    /// The caller is responsible for closing the current session first.
    fn close_focused(&mut self) -> Option<PaneSlot> {
        let closing = self.layout.focused();
        let next = self.layout.close(closing)?;
        let slot = self.panes.remove(&next);
        debug_assert!(slot.is_some(), "a non-focused pane must be parked");
        slot
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
        let new_id = self.next_pane_id;
        // Validate the tree mutation before starting a new shell or parking
        // the current one. A depth-limit rejection must be a true no-op.
        let mut next_layout = self.tabs[self.active_tab].layout.clone();
        if !next_layout.split(focused, axis, new_id) {
            return;
        }
        if self.spawn.ssh_host.is_none() {
            if let Some(cwd) = &self.cwd {
                self.spawn.cwd = Some(workspace::local_path(cwd));
            }
        }
        self.start_session(false);
        self.next_pane_id += 1;
        let tab = &mut self.tabs[self.active_tab];
        tab.park_focused();
        tab.layout = next_layout;
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
        if !tab.focus_parked(id) {
            return;
        }
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
            Self::close_session_handle(session, "focused pane");
        }
        let tab = &mut self.tabs[self.active_tab];
        let Some(slot) = tab.close_focused() else {
            return;
        };
        slot.make_focused(tab);
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
        if self.settings.is_some() {
            return;
        }
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

    /// Toggle the top-left Editor Type Selector dropdown for `id`.
    pub(super) fn toggle_area_selector(&mut self, id: AreaId, cx: &mut Context<'_, Self>) {
        if self.open_area_selector == Some(id) {
            self.open_area_selector = None;
        } else {
            self.open_area_selector = Some(id);
        }
        cx.notify();
    }

    /// Swap the editor mounted in area `id` in place without killing any PTY session.
    pub(super) fn set_area_editor(
        &mut self,
        id: AreaId,
        tag: EditorKindTag,
        cx: &mut Context<'_, Self>,
    ) {
        self.open_area_selector = None;
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        if tab.layout.set_editor_tag(id, tag) {
            self.persist_layout();
            cx.notify();
        }
    }

    /// Split a specific area `id` along `axis`.
    pub(super) fn split_area(&mut self, id: AreaId, axis: Axis, cx: &mut Context<'_, Self>) {
        self.focus_pane(id, cx);
        self.split_pane(axis, cx);
    }

    /// Recover the terminal sessions from older spatial layouts without
    /// starting new shells or displaying the retired graph and inspector areas.
    pub fn prepare_workbench(&mut self, cx: &mut Context<'_, Self>) {
        if self.workspace_artifact == WorkspaceArtifact::Overview {
            self.workspace_artifact = WorkspaceArtifact::Files;
            self.terminal_maximized = false;
        }
        for tab in &mut self.tabs {
            for id in tab.layout.ids() {
                tab.layout.set_editor_tag(id, EditorKindTag::Terminal);
            }
        }
        self.show_tab_strip = self.workspace_artifact == WorkspaceArtifact::Sessions;
        cx.notify();
    }

    /// Maximize or restore area `id` within the split tree.
    pub(super) fn toggle_area_maximized(&mut self, id: AreaId, cx: &mut Context<'_, Self>) {
        if self.maximized_area == Some(id) {
            self.maximized_area = None;
        } else {
            self.maximized_area = Some(id);
        }
        cx.notify();
    }

    /// Close a specific area `id`.
    pub(super) fn close_area(&mut self, id: AreaId, cx: &mut Context<'_, Self>) {
        if self.maximized_area == Some(id) {
            self.maximized_area = None;
        }
        self.focus_pane(id, cx);
        self.close_pane(cx);
    }

    /// Apply a Blender-style workspace layout preset (`TopologyFlow`, `ConsoleMatrix`,
    /// `StateDebug`) by swapping the active tab's split tree and remounting existing
    /// live session handles without ever killing active PTY sessions.
    #[allow(dead_code)] // Legacy preset migration remains covered by tests.
    pub(super) fn apply_workspace_preset(
        &mut self,
        preset: WorkspacePreset,
        cx: &mut Context<'_, Self>,
    ) {
        if self.tabs.is_empty() {
            return;
        }
        self.sync_active_tab();
        let needed = preset.area_count();

        let mut pool: Vec<(PaneId, PaneSlot)> = {
            let tab = &mut self.tabs[self.active_tab];
            if preset != self.active_preset {
                tab.preset_layouts
                    .insert(self.active_preset, tab.layout.clone());
            }
            let focused_id = tab.layout.focused();
            let mut items = vec![(focused_id, PaneSlot::from_tab(tab))];
            items.extend(tab.panes.drain());
            items.extend(tab.retained_panes.drain());
            items.sort_by_key(|(id, _)| *id);
            items.dedup_by_key(|(id, _)| *id);
            items
        };

        while pool.len() < needed {
            let new_id = self.next_pane_id;
            self.next_pane_id += 1;
            if self.spawn.ssh_host.is_none() {
                if let Some(cwd) = &self.cwd {
                    self.spawn.cwd = Some(workspace::local_path(cwd));
                }
            }
            self.start_session(false);
            pool.push((new_id, PaneSlot::from_view(self)));
        }

        let saved = self.tabs[self.active_tab]
            .preset_layouts
            .get(&preset)
            .filter(|tree| {
                tree.ids()
                    .iter()
                    .all(|id| pool.iter().any(|(key, _)| key == id))
            })
            .cloned();
        let mut tree = saved.unwrap_or_else(|| {
            let ids: Vec<PaneId> = pool.iter().take(needed).map(|(id, _)| *id).collect();
            preset.build_tree(&ids)
        });
        // Sync shared canvas/inspector states into the preset's newly constructed leaves.
        for area in tree.areas_mut() {
            match &mut area.editor {
                EditorKind::GraphCanvas(state) => {
                    let preset_lens = state.lens;
                    *state = self.canvas_state.clone();
                    if preset == WorkspacePreset::CodeStudio {
                        state.lens = preset_lens;
                    }
                }
                EditorKind::Inspector(state) => *state = self.inspector_state.clone(),
                EditorKind::Outliner(state) => *state = self.outliner_state.clone(),
                EditorKind::Timeline(state) => *state = self.timeline_state.clone(),
                EditorKind::Terminal(_)
                | EditorKind::CodeEditor
                | EditorKind::GitDiff
                | EditorKind::TaskBoard
                | EditorKind::AgentFleet => {}
            }
        }
        let next_focus = tree.focused();
        let active_ids = tree.ids();

        let tab = &mut self.tabs[self.active_tab];
        tab.layout = tree;
        for (id, slot) in pool {
            if active_ids.contains(&id) {
                if id == next_focus {
                    slot.make_focused(tab);
                } else {
                    tab.panes.insert(id, slot);
                }
            } else {
                tab.retained_panes.insert(id, slot);
            }
        }
        let updated_tab = tab.clone();
        self.active_preset = preset;
        self.maximized_area = None;
        self.open_area_selector = None;
        self.load_tab_state(updated_tab);
        self.on_output(cx);
        self.persist_layout();
        cx.notify();
    }

    /// Standardized Blender-style area header strip containing:
    /// - Top-left Editor Type Selector dropdown (`Terminal <-> Graph Canvas <-> Inspector <-> Outliner <-> Timeline`)
    /// - Context actions for the mounted editor
    /// - Split Horizontal, Split Vertical, Maximize, and Close buttons
    pub(super) fn area_header_strip(
        &self,
        id: AreaId,
        editor_tag: EditorKindTag,
        focused: bool,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        let selector_open = self.open_area_selector == Some(id);
        let maximized = self.maximized_area == Some(id);
        div()
            .id(("area-header", id))
            .h(px(34.0))
            .px(px(tokens::space::SM))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(tokens::space::SM))
            .border_b_1()
            .border_color(hsla(t.border))
            .bg(hsla(if focused { t.bg_elevated } else { t.bg_toolbar }))
            .text_size(px(tokens::text::XS))
            .when(editor_tag != EditorKindTag::Terminal, |d| {
                d.child(
                    div()
                        .id(("area-selector", id))
                        .h(px(20.0))
                        .pl(px(5.0))
                        .pr(px(7.0))
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .rounded(px(tokens::radius::SM))
                        .border_1()
                        .border_color(hsla(if selector_open || focused {
                            t.accent
                        } else {
                            t.border_strong
                        }))
                        .bg(hsla(if selector_open {
                            t.bg_selected
                        } else {
                            t.bg_app
                        }))
                        .text_color(hsla(t.text))
                        .cursor_pointer()
                        .hover(|style| style.bg(hsla(t.bg_hover)).border_color(hsla(t.accent)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_area_selector(id, cx);
                        }))
                        .child(
                            div()
                                .px(px(4.0))
                                .py(px(1.0))
                                .rounded(px(3.0))
                                .bg(with_alpha(hsla(t.accent), 0.16))
                                .text_color(hsla(t.accent))
                                .child(editor_tag.icon()),
                        )
                        .child(editor_tag.label())
                        .child(
                            div()
                                .text_color(hsla(t.text_faint))
                                .child(if selector_open { "▴" } else { "▾" }),
                        ),
                )
            })
            .child(self.area_context_actions(id, editor_tag, cx))
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .child(
                        div()
                            .id(("area-split-h", id))
                            .size(px(24.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.0))
                            .text_color(hsla(t.text_faint))
                            .cursor_pointer()
                            .hover(|style| style.bg(hsla(t.bg_hover)).text_color(hsla(t.text)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.split_area(id, Axis::Horizontal, cx);
                            }))
                            .child("◫"),
                    )
                    .child(
                        div()
                            .id(("area-split-v", id))
                            .size(px(24.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.0))
                            .text_color(hsla(t.text_faint))
                            .cursor_pointer()
                            .hover(|style| style.bg(hsla(t.bg_hover)).text_color(hsla(t.text)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.split_area(id, Axis::Vertical, cx);
                            }))
                            .child("⊟"),
                    )
                    .child(
                        div()
                            .id(("area-maximize", id))
                            .size(px(24.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.0))
                            .text_color(hsla(if maximized { t.accent } else { t.text_faint }))
                            .cursor_pointer()
                            .hover(|style| style.bg(hsla(t.bg_hover)).text_color(hsla(t.text)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_area_maximized(id, cx);
                            }))
                            .child(if maximized { "↙" } else { "↗" }),
                    )
                    .child(
                        div()
                            .id(("area-close", id))
                            .size(px(24.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.0))
                            .text_color(hsla(t.text_faint))
                            .cursor_pointer()
                            .hover(|style| style.bg(hsla(t.bg_hover)).text_color(hsla(t.danger)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.close_area(id, cx);
                            }))
                            .child("×"),
                    ),
            )
    }

    /// Floating Blender-style Editor Type popover menu rendered over the area
    /// when its top-left selector pill is open.
    pub(super) fn render_area_selector_popover(
        &self,
        id: AreaId,
        editor_tag: EditorKindTag,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        div()
            .id(("area-selector-popover", id))
            .absolute()
            .top(px(30.0))
            .left(px(8.0))
            .w(px(264.0))
            .p(px(tokens::space::XS))
            .rounded(px(tokens::radius::MD))
            .border_1()
            .border_color(hsla(t.border_strong))
            .bg(hsla(t.bg_elevated))
            .shadow_lg()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .px(px(tokens::space::SM))
                    .py(px(4.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(hsla(t.border))
                    .text_size(px(10.0))
                    .text_color(hsla(t.text_faint))
                    .child("EDITOR TYPE")
                    .child(SharedString::from(format!("AREA #{id}"))),
            )
            .children(EditorKindTag::ALL.into_iter().map(|candidate| {
                let active = candidate == editor_tag;
                div()
                    .id(SharedString::from(format!(
                        "area-kind-{}-{id}",
                        candidate.code()
                    )))
                    .px(px(tokens::space::SM))
                    .py(px(5.0))
                    .flex()
                    .items_center()
                    .gap(px(tokens::space::SM))
                    .rounded(px(tokens::radius::SM))
                    .bg(hsla(if active { t.bg_selected } else { t.bg_elevated }))
                    .cursor_pointer()
                    .hover(|style| style.bg(hsla(t.bg_hover)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_area_editor(id, candidate, cx);
                    }))
                    .child(
                        div()
                            .size(px(22.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(tokens::radius::SM))
                            .bg(with_alpha(
                                hsla(if active { t.accent } else { t.border_strong }),
                                0.18,
                            ))
                            .text_size(px(tokens::text::XS))
                            .text_color(hsla(if active { t.accent } else { t.text_muted }))
                            .child(candidate.icon()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_size(px(tokens::text::XS))
                                            .text_color(hsla(if active {
                                                t.text
                                            } else {
                                                t.text_muted
                                            }))
                                            .child(candidate.label()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(9.0))
                                            .text_color(hsla(if active {
                                                t.accent
                                            } else {
                                                t.text_faint
                                            }))
                                            .child(candidate.category()),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(hsla(t.text_faint))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(candidate.description()),
                            ),
                    )
            }))
    }

    fn area_context_actions(
        &self,
        id: AreaId,
        editor_tag: EditorKindTag,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        match editor_tag {
            EditorKindTag::Terminal => {
                let parked = self
                    .tabs
                    .get(self.active_tab)
                    .and_then(|tab| tab.panes.get(&id));
                let spawn = parked.map_or(&self.spawn, |pane| &pane.spawn);
                let cwd = parked.map_or(&self.cwd, |pane| &pane.cwd);
                let root = parked.map_or(&self.project_root, |pane| &pane.project_root);
                let shell = spawn.profile.name.clone();
                let cwd = cwd
                    .clone()
                    .or_else(|| {
                        root.as_ref()
                            .and_then(|p| p.file_name())
                            .and_then(|n| n.to_str())
                            .map(str::to_owned)
                    })
                    .unwrap_or_else(|| "shell".into());
                let short_cwd = cwd
                    .trim_end_matches(['/', '\\'])
                    .rsplit(['/', '\\'])
                    .next()
                    .filter(|part| !part.is_empty())
                    .unwrap_or(&cwd)
                    .to_owned();
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .text_color(hsla(t.text_faint))
                    .child(
                        div()
                            .text_color(hsla(t.text_muted))
                            .child(SharedString::from(shell)),
                    )
                    .child(
                        div()
                            .max_w(px(180.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(SharedString::from(short_cwd)),
                    )
                    .into_any_element()
            }
            EditorKindTag::GraphCanvas => div()
                .flex()
                .items_center()
                .gap(px(4.0))
                .child(
                    div()
                        .px(px(5.0))
                        .py(px(1.0))
                        .rounded(px(3.0))
                        .bg(hsla(t.bg_app))
                        .text_color(hsla(t.text_muted))
                        .child("2D Scene"),
                )
                .child(
                    div()
                        .id(("area-canvas-zoom-out", id))
                        .px(px(4.0))
                        .rounded(px(tokens::radius::SM))
                        .text_color(hsla(t.text_muted))
                        .cursor_pointer()
                        .hover(|style| style.bg(hsla(t.bg_hover)).text_color(hsla(t.text)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.zoom_canvas(Some(id), -0.15, cx);
                        }))
                        .child("−"),
                )
                .child(
                    div()
                        .id(("area-canvas-zoom-reset", id))
                        .px(px(5.0))
                        .py(px(1.0))
                        .rounded(px(tokens::radius::SM))
                        .bg(hsla(t.bg_app))
                        .text_color(hsla(t.text_faint))
                        .cursor_pointer()
                        .hover(|style| style.bg(hsla(t.bg_hover)).text_color(hsla(t.text)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.reset_canvas_view(Some(id), cx);
                        }))
                        .child(SharedString::from(format!(
                            "{:.0}%",
                            self.canvas_state.zoom * 100.0
                        ))),
                )
                .child(
                    div()
                        .id(("area-canvas-zoom-in", id))
                        .px(px(4.0))
                        .rounded(px(tokens::radius::SM))
                        .text_color(hsla(t.text_muted))
                        .cursor_pointer()
                        .hover(|style| style.bg(hsla(t.bg_hover)).text_color(hsla(t.text)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.zoom_canvas(Some(id), 0.15, cx);
                        }))
                        .child("+"),
                )
                .into_any_element(),
            EditorKindTag::Inspector => div()
                .flex()
                .items_center()
                .gap(px(tokens::space::XS))
                .text_color(hsla(t.text_faint))
                .child(
                    div()
                        .px(px(5.0))
                        .py(px(1.0))
                        .rounded(px(3.0))
                        .bg(hsla(t.bg_app))
                        .text_color(hsla(t.text_muted))
                        .child(SharedString::from(
                            self.inspector_state
                                .target
                                .clone()
                                .unwrap_or_else(|| "Workspace".into()),
                        )),
                )
                .into_any_element(),
            EditorKindTag::Outliner => div()
                .flex()
                .items_center()
                .gap(px(tokens::space::XS))
                .text_color(hsla(t.text_faint))
                .child(SharedString::from(format!(
                    "{} items",
                    self.project_file_entries.len()
                )))
                .into_any_element(),
            EditorKindTag::Timeline => div()
                .id(("area-timeline-follow", id))
                .px(px(6.0))
                .py(px(1.0))
                .rounded(px(tokens::radius::SM))
                .bg(hsla(t.bg_app))
                .text_color(hsla(if self.timeline_state.follow {
                    t.success
                } else {
                    t.text_faint
                }))
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.timeline_state.follow = !this.timeline_state.follow;
                    cx.notify();
                }))
                .child(if self.timeline_state.follow {
                    "● Live Stream"
                } else {
                    "○ Paused"
                })
                .into_any_element(),
            EditorKindTag::CodeEditor => div()
                .flex()
                .items_center()
                .gap(px(tokens::space::XS))
                .text_color(hsla(t.text_faint))
                .child(SharedString::from(
                    self.selected_project_file
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "Buffer".into()),
                ))
                .into_any_element(),
            EditorKindTag::GitDiff => div()
                .flex()
                .items_center()
                .gap(px(tokens::space::XS))
                .text_color(hsla(t.text_faint))
                .child(SharedString::from(format!(
                    "{} changed",
                    self.changes.as_ref().map_or(0, Vec::len)
                )))
                .into_any_element(),
            EditorKindTag::TaskBoard => div()
                .flex()
                .items_center()
                .gap(px(tokens::space::XS))
                .text_color(hsla(t.text_faint))
                .child(SharedString::from(format!(
                    "{} tasks",
                    self.tasks.items.len()
                )))
                .into_any_element(),
            EditorKindTag::AgentFleet => div()
                .flex()
                .items_center()
                .gap(px(tokens::space::XS))
                .text_color(hsla(t.text_faint))
                .child(SharedString::from(format!(
                    "{} sessions · {} SSH",
                    self.tabs.len(),
                    self.ssh_hosts.len()
                )))
                .into_any_element(),
        }
    }
}
