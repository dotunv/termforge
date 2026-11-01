//! Session-free workspace layouts.
//!
//! Every workspace except Build is a tree of areas that only *view* project
//! state (tasks, files, changes, agents, logs). Such a tree never owns a
//! process, so splitting, retyping, closing or switching away from one cannot
//! start, move or kill a terminal session. Sessions live in the active tab's
//! pane tree, which is Build's layout.

use super::*;

impl TerminalView {
    /// Fill in the default layout of every session-free workspace that has
    /// none yet, keeping layouts that were restored from the layout file.
    pub(super) fn ensure_workspace_layouts(&mut self) {
        for workspace in Workspace::ALL {
            if workspace.hosts_sessions() || self.workspace_layouts.contains_key(&workspace) {
                continue;
            }
            let mut next = self.next_pane_id;
            let tree = workspace.default_layout(|| {
                let id = next;
                next += 1;
                id
            });
            self.next_pane_id = next;
            if let Some(tree) = tree {
                self.workspace_layouts.insert(workspace, tree);
            }
        }
    }

    /// Adopt saved session-free layouts. Anything malformed, or that would
    /// mount a terminal, is ignored in favour of the default.
    pub(super) fn apply_saved_areas(&mut self, saved: &[(String, String)]) {
        for (id, encoded) in saved {
            let Some(workspace) = Workspace::from_id(id).filter(|ws| !ws.hosts_sessions()) else {
                continue;
            };
            let Some(tree) = PaneTree::decode(encoded) else {
                continue;
            };
            if tree
                .areas()
                .iter()
                .any(|area| matches!(area.editor, EditorKind::Terminal(_)))
            {
                continue;
            }
            // Ids only mean something within one saved file; renumber them so
            // they cannot collide with the ids of this window's panes.
            let mut ids = tree.ids();
            ids.sort_unstable();
            let base = self.next_pane_id;
            let tree = tree
                .remap(|old| base + ids.iter().position(|id| *id == old).unwrap_or(0) as PaneId);
            self.next_pane_id = base + ids.len() as PaneId;
            self.workspace_layouts.insert(workspace, tree);
        }
    }

    /// Encoded session-free layouts for the layout file.
    pub(super) fn saved_areas(&self) -> Vec<(String, String)> {
        Workspace::ALL
            .into_iter()
            .filter_map(|workspace| {
                let tree = self.workspace_layouts.get(&workspace)?;
                Some((workspace.id().to_owned(), tree.encode()))
            })
            .collect()
    }

    /// The pane tree of the visible workspace.
    pub(super) fn current_tree_mut(&mut self) -> Option<&mut PaneTree> {
        if self.workspace.hosts_sessions() {
            self.tabs
                .get_mut(self.active_tab)
                .map(|tab| &mut tab.layout)
        } else {
            self.workspace_layouts.get_mut(&self.workspace)
        }
    }

    /// Show `workspace`. Sessions are untouched: only the visible layout and
    /// the data its areas show change.
    pub(super) fn switch_workspace(&mut self, workspace: Workspace, cx: &mut Context<'_, Self>) {
        if self.workspace == workspace && !self.terminal_maximized {
            return;
        }
        self.show_workspace(workspace, cx);
        if workspace == Workspace::Review {
            self.refresh_changes(cx);
        }
    }

    /// Make `workspace` visible without reloading the data it shows, for
    /// callers that are about to load something specific into it.
    pub(super) fn show_workspace(&mut self, workspace: Workspace, cx: &mut Context<'_, Self>) {
        self.ensure_workspace_layouts();
        self.workspace = workspace;
        self.terminal_maximized = false;
        self.maximized_area = None;
        self.open_area_selector = None;
        self.show_tab_strip = workspace.hosts_sessions();
        self.refresh_ports_if_stale(cx);
        self.persist_layout();
        cx.notify();
    }

    /// Focus an area of the visible workspace.
    pub(super) fn focus_area(&mut self, id: AreaId, cx: &mut Context<'_, Self>) {
        if self.workspace.hosts_sessions() {
            self.focus_pane(id, cx);
        } else if let Some(tree) = self.current_tree_mut() {
            if tree.focused() != id && tree.set_focus(id) {
                cx.notify();
            }
        }
    }

    /// Split a session-free area; the new area starts as an inspector.
    pub(super) fn split_tree_area(&mut self, id: AreaId, axis: Axis, cx: &mut Context<'_, Self>) {
        let new_id = self.next_pane_id;
        let editor = EditorKind::Inspector(self.inspector_state.clone());
        let Some(tree) = self.current_tree_mut() else {
            return;
        };
        if tree.len() >= super::split::MAX_PANES
            || !tree.split_with_editor(id, axis, 0.5, new_id, editor)
        {
            return;
        }
        self.next_pane_id += 1;
        self.persist_layout();
        cx.notify();
    }

    /// Close a session-free area. The last area of a workspace stays.
    pub(super) fn close_tree_area(&mut self, id: AreaId, cx: &mut Context<'_, Self>) {
        let Some(tree) = self.current_tree_mut() else {
            return;
        };
        if tree.close(id).is_none() {
            return;
        }
        if self.maximized_area == Some(id) {
            self.maximized_area = None;
        }
        self.open_area_selector = None;
        self.persist_layout();
        cx.notify();
    }

    /// The visible session-free layout: header strip, editor surface and
    /// draggable dividers for every area.
    pub(super) fn workspace_areas(&self, cx: &mut Context<'_, Self>) -> gpui::AnyElement {
        let Some(tree) = self.workspace_layouts.get(&self.workspace) else {
            return div().flex_1().into_any_element();
        };
        let theme = &self.theme;
        let mut rects = tree.layout(PaneRect::new(0.0, 0.0, 1.0, 1.0));
        if let Some(max_id) = self.maximized_area {
            if rects.iter().any(|(id, _)| *id == max_id) {
                rects = vec![(max_id, PaneRect::new(0.0, 0.0, 1.0, 1.0))];
            }
        }
        let dividers = if self.maximized_area.is_some() {
            Vec::new()
        } else {
            tree.dividers(PaneRect::new(0.0, 0.0, 1.0, 1.0))
        };
        let focused_id = tree.focused();
        let view = cx.entity();
        let probe = canvas(|_, _, _| (), {
            move |bounds, (), _, cx| {
                view.update(cx, |v, _| v.pane_area = bounds);
            }
        })
        .absolute()
        .size_full();

        let mut children: Vec<gpui::AnyElement> =
            vec![div().absolute().size_full().child(probe).into_any_element()];
        for (id, rect) in rects {
            let Some(editor) = tree.editor(id).cloned() else {
                continue;
            };
            let border = if id == focused_id {
                hsla(theme.accent)
            } else {
                hsla(theme.border_strong)
            };
            let mut tile = div()
                .relative()
                .size_full()
                .flex()
                .flex_col()
                .border_1()
                .border_color(border)
                .bg(hsla(theme.bg_app))
                .overflow_hidden()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.focus_area(id, cx);
                    }),
                )
                .child(self.area_header_strip(id, editor.tag(), id == focused_id, cx))
                .child(
                    div()
                        .flex_1()
                        .min_h(px(0.0))
                        .relative()
                        .overflow_hidden()
                        .child(
                            div()
                                .size_full()
                                .flex()
                                .flex_col()
                                .child(self.render_non_terminal_area(id, &editor, cx)),
                        ),
                );
            if self.open_area_selector == Some(id) {
                tile = tile.child(self.render_area_selector_popover(id, editor.tag(), cx));
            }
            children.push(
                div()
                    .id(("workspace-area", id))
                    .absolute()
                    .left(relative(rect.x))
                    .top(relative(rect.y))
                    .w(relative(rect.w))
                    .h(relative(rect.h))
                    .p(px(1.0))
                    .flex()
                    .flex_col()
                    .child(tile)
                    .into_any_element(),
            );
        }
        const GRAB: f32 = 8.0;
        for (index, divider) in dividers.into_iter().enumerate() {
            let p = divider.parent;
            let handle = div()
                .id(("workspace-divider", index))
                .absolute()
                .flex()
                .items_center()
                .justify_center()
                .hover(|style| style.bg(with_alpha(hsla(theme.accent), 0.22)));
            let grip = |horizontal: bool| {
                div()
                    .when(horizontal, |d| d.w(px(2.0)).h(px(28.0)))
                    .when(!horizontal, |d| d.h(px(2.0)).w(px(28.0)))
                    .rounded_full()
                    .bg(with_alpha(hsla(theme.border_strong), 0.7))
            };
            let handle = match divider.axis {
                Axis::Horizontal => handle
                    .left(relative(p.x + p.w * divider.ratio))
                    .ml(px(-GRAB / 2.0))
                    .w(px(GRAB))
                    .top(relative(p.y))
                    .h(relative(p.h))
                    .cursor_col_resize()
                    .child(grip(true)),
                Axis::Vertical => handle
                    .top(relative(p.y + p.h * divider.ratio))
                    .mt(px(-GRAB / 2.0))
                    .h(px(GRAB))
                    .left(relative(p.x))
                    .w(relative(p.w))
                    .cursor_row_resize()
                    .child(grip(false)),
            };
            children.push(
                handle
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.begin_divider_drag(&divider);
                            cx.stop_propagation();
                        }),
                    )
                    .into_any_element(),
            );
        }
        div()
            .id("workspace-areas")
            .flex_1()
            .min_h(px(0.0))
            .relative()
            .overflow_hidden()
            .children(children)
            .into_any_element()
    }
}
