//! Header, tab strip, status bar and window chrome.

use super::*;

impl TerminalView {
    pub(super) fn project_name(&self) -> String {
        self.project_root
            .as_deref()
            .and_then(std::path::Path::file_name)
            .and_then(|name| name.to_str())
            .unwrap_or("Workspace")
            .to_owned()
    }

    pub(super) fn tab_label(tab: &WorkspaceTab) -> String {
        tab.spawn
            .ssh_host
            .as_ref()
            .map(|host| format!("SSH · {host}"))
            .unwrap_or_else(|| tab.spawn.profile.name.clone())
    }

    pub(super) fn tab_status_color(&self, tab: &WorkspaceTab, active: bool, t: &Theme) -> Rgb {
        if active {
            return self.status_color();
        }
        if let Some(session) = &tab.session {
            let color = session.with(|p| {
                let status = p.program_status();
                status
                    .iter()
                    .max_by_key(|r| match r.report.state {
                        tf_session::ProgramState::Blocked => 5,
                        tf_session::ProgramState::Error => 4,
                        tf_session::ProgramState::Working => 3,
                        tf_session::ProgramState::Done => 2,
                        tf_session::ProgramState::Idle => 1,
                        tf_session::ProgramState::Clear => 0,
                    })
                    .map(|record| match record.report.state {
                        tf_session::ProgramState::Working => t.accent,
                        tf_session::ProgramState::Blocked => t.warning,
                        tf_session::ProgramState::Error => t.danger,
                        tf_session::ProgramState::Done => t.success,
                        tf_session::ProgramState::Idle | tf_session::ProgramState::Clear => {
                            t.text_faint
                        }
                    })
            });
            if let Some(color) = color {
                return color;
            }
        }
        match (
            tab.session.as_ref().and_then(|s| s.exit_status()),
            tab.last_exit,
        ) {
            (Some(_), _) => t.text_faint,
            (None, Some(c)) if c != 0 => t.danger,
            _ => t.success,
        }
    }

    /// Empty strip that moves the window when dragged (client decorations
    /// only; with server decorations the compositor owns the titlebar).
    pub(super) fn drag_region(&self, id: &'static str) -> gpui::Stateful<gpui::Div> {
        let region = div().id(id);
        if !self.client_decorated {
            return region;
        }
        region.on_mouse_down(MouseButton::Left, |ev: &MouseDownEvent, window, _| {
            if ev.click_count >= 2 {
                window.zoom_window();
            } else {
                window.start_window_move();
            }
        })
    }

    /// Minimise, maximise and close, drawn in the toolbar.
    pub(super) fn window_controls(&self) -> impl IntoElement {
        let t = &self.theme;
        let button = |id: &'static str, glyph: &'static str, danger: bool| {
            let hover = if danger {
                with_alpha(hsla(t.danger), 0.85)
            } else {
                with_alpha(hsla(t.text), 0.08)
            };
            div()
                .id(id)
                .size(px(26.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(tokens::radius::SM))
                .text_color(hsla(t.text_muted))
                .hover(move |style| style.bg(hover).text_color(hsla(t.text)))
                .child(glyph)
        };
        div()
            .ml(px(tokens::space::SM))
            .flex()
            .items_center()
            .child(
                button("window-minimize", "–", false)
                    .on_click(|_, window, _| window.minimize_window()),
            )
            .child(
                button("window-maximize", "▢", false).on_click(|_, window, _| window.zoom_window()),
            )
            .child(
                button("window-close", "×", true).on_click(|_, window, _| window.remove_window()),
            )
    }

    /// Thin hit strips on the window edges so a borderless window can still
    /// be resized.
    pub(super) fn resize_edges(&self) -> Vec<gpui::AnyElement> {
        use gpui::ResizeEdge as E;
        const T: f32 = 5.0;
        const C: f32 = 10.0;
        let strip = |id: &'static str, edge: E| {
            let d = div().id(id).absolute();
            let d = match edge {
                E::Top => d
                    .top(px(0.0))
                    .left(px(C))
                    .right(px(C))
                    .h(px(T))
                    .cursor_row_resize(),
                E::Bottom => d
                    .bottom(px(0.0))
                    .left(px(C))
                    .right(px(C))
                    .h(px(T))
                    .cursor_row_resize(),
                E::Left => d
                    .left(px(0.0))
                    .top(px(C))
                    .bottom(px(C))
                    .w(px(T))
                    .cursor_col_resize(),
                E::Right => d
                    .right(px(0.0))
                    .top(px(C))
                    .bottom(px(C))
                    .w(px(T))
                    .cursor_col_resize(),
                E::TopLeft => d
                    .top(px(0.0))
                    .left(px(0.0))
                    .size(px(C))
                    .cursor_nwse_resize(),
                E::TopRight => d
                    .top(px(0.0))
                    .right(px(0.0))
                    .size(px(C))
                    .cursor_nesw_resize(),
                E::BottomLeft => d
                    .bottom(px(0.0))
                    .left(px(0.0))
                    .size(px(C))
                    .cursor_nesw_resize(),
                E::BottomRight => d
                    .bottom(px(0.0))
                    .right(px(0.0))
                    .size(px(C))
                    .cursor_nwse_resize(),
            };
            d.on_mouse_down(MouseButton::Left, move |_: &MouseDownEvent, window, _| {
                window.start_window_resize(edge);
            })
            .into_any_element()
        };
        vec![
            strip("resize-top", E::Top),
            strip("resize-bottom", E::Bottom),
            strip("resize-left", E::Left),
            strip("resize-right", E::Right),
            strip("resize-top-left", E::TopLeft),
            strip("resize-top-right", E::TopRight),
            strip("resize-bottom-left", E::BottomLeft),
            strip("resize-bottom-right", E::BottomRight),
        ]
    }

    pub(super) fn header(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let t = &self.theme;
        let tab_count = self.tabs.len();
        let branch = self
            .tabs
            .get(self.active_tab)
            .and_then(|tab| tab.branch.as_deref());
        div()
            .h(px(if self.show_tab_strip { 68.0 } else { 38.0 }))
            .flex()
            .flex_col()
            .bg(hsla(t.bg_toolbar))
            .border_b_1()
            .border_color(hsla(t.border))
            .child(
                div()
                    .h(px(38.0))
                    .px(px(tokens::space::MD))
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(tokens::space::SM))
                            .when(!self.show_sidebar, |d| {
                                d.child(self.icon_button(
                                    "header-show-sidebar",
                                    "☰",
                                    cx.listener(|this, _, _, cx| this.toggle_sidebar(cx)),
                                ))
                            })
                            .child(
                                div()
                                    .text_size(px(tokens::text::SM))
                                    .text_color(hsla(t.text))
                                    .child(SharedString::from(self.project_name())),
                            )
                            .when_some(branch, |d, branch| {
                                d.child(div().text_color(hsla(t.text_faint)).child("·"))
                                    .child(
                                        div()
                                            .max_w(px(180.0))
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_size(px(tokens::text::XS))
                                            .text_color(hsla(t.text_faint))
                                            .child(SharedString::from(format!("⎇ {branch}"))),
                                    )
                            })
                            .child(
                                div()
                                    .w(px(1.0))
                                    .h(px(14.0))
                                    .bg(with_alpha(hsla(t.border), 0.75)),
                            )
                            .child(
                                div()
                                    .text_size(px(tokens::text::XS))
                                    .text_color(hsla(t.text_faint))
                                    .child("Workspace"),
                            )
                            .child(self.context_badge("header-context-badge", cx)),
                    )
                    .child(self.drag_region("header-drag").flex_1().h_full())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(tokens::space::SM))
                            .child(
                                div()
                                    .id("header-command-palette")
                                    .px(px(7.0))
                                    .py(px(3.0))
                                    .rounded(px(tokens::radius::SM))
                                    .border_1()
                                    .border_color(hsla(t.border))
                                    .text_size(px(tokens::text::XS))
                                    .text_color(hsla(t.text_faint))
                                    .hover(|style| {
                                        style.bg(hsla(t.bg_hover)).text_color(hsla(t.text))
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| this.open_palette(cx)))
                                    .child("⌕  Search"),
                            )
                            .child(self.icon_button(
                                "header-toggle-context-panel",
                                "◫",
                                cx.listener(|this, _, _, cx| this.toggle_context_panel(cx)),
                            ))
                            .child(self.icon_button(
                                "header-toggle-utility-dock",
                                "▱",
                                cx.listener(|this, _, _, cx| this.toggle_utility_dock(cx)),
                            ))
                            .child(
                                div()
                                    .px(px(6.0))
                                    .py(px(1.0))
                                    .text_size(px(tokens::text::XS))
                                    .text_color(hsla(t.text_muted))
                                    .bg(hsla(t.bg_elevated))
                                    .rounded(px(tokens::radius::SM))
                                    .child(SharedString::from(format!(
                                        "{tab_count} tab{}",
                                        if tab_count == 1 { "" } else { "s" }
                                    ))),
                            )
                            .child(self.icon_button(
                                "header-settings",
                                "⚙",
                                cx.listener(|this, _, _, cx| this.open_settings(cx)),
                            )),
                    )
                    .when(self.client_decorated, |d| d.child(self.window_controls())),
            )
            .when(self.show_tab_strip, |header| {
                header.child(
                    div()
                        .h(px(30.0))
                        .px(px(tokens::space::SM))
                        .flex()
                        .items_end()
                        .gap(px(2.0))
                        .children(self.tabs.iter().enumerate().map(|(index, tab)| {
                            let active = index == self.active_tab;
                            let is_ssh = tab.spawn.ssh_host.is_some();
                            let label = Self::tab_label(tab);
                            let indicator_color = self.tab_status_color(tab, active, t);
                            let tab_id = tab.id;
                            let is_dragging = self.drag_state.is_dragging
                                && self.drag_state.dragged_index == Some(index);
                            div()
                                .id(("workspace-tab", tab.id as usize))
                                .h(px(30.0))
                                .max_w(px(220.0))
                                .px(px(tokens::space::MD))
                                .flex()
                                .items_center()
                                .gap(px(tokens::space::SM))
                                .text_size(px(tokens::text::SM))
                                .border_b_1()
                                .border_color(hsla(if active { t.accent } else { t.bg_toolbar }))
                                .when(active, |d| d.text_color(hsla(t.text)))
                                .when(!active, |d| {
                                    d.text_color(hsla(t.text_muted))
                                        .hover(|style| style.text_color(hsla(t.text)))
                                })
                                .when(is_dragging, |d| d.opacity(0.5).bg(hsla(t.accent)))
                                .child(
                                    div()
                                        .id(("select-workspace-tab", tab.id as usize))
                                        .flex_1()
                                        .flex()
                                        .items_center()
                                        .gap(px(tokens::space::SM))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.select_tab(index, cx);
                                        }))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                                this.drag_state.dragged_index = Some(index);
                                                this.drag_state.drag_start = Some(ev.position);
                                                this.drag_state.is_dragging = false;
                                                cx.notify();
                                            }),
                                        )
                                        .on_mouse_move(cx.listener(
                                            move |this, ev: &MouseMoveEvent, _, cx| {
                                                if this.drag_state.dragged_index == Some(index) {
                                                    if let Some(start) = this.drag_state.drag_start
                                                    {
                                                        let dx = f32::from(ev.position.x)
                                                            - f32::from(start.x);
                                                        let dy = f32::from(ev.position.y)
                                                            - f32::from(start.y);
                                                        let delta = (dx * dx + dy * dy).sqrt();
                                                        if delta > 5.0 {
                                                            this.drag_state.is_dragging = true;
                                                        }
                                                    }
                                                    if this.drag_state.is_dragging {
                                                        this.drag_state.drag_start =
                                                            Some(ev.position);
                                                        // Find target tab index under cursor
                                                        // For simplicity, we'll handle reorder on mouse_up
                                                        cx.notify();
                                                    }
                                                }
                                            },
                                        ))
                                        .on_mouse_up(
                                            MouseButton::Left,
                                            cx.listener(move |this, _ev: &MouseUpEvent, _, cx| {
                                                if this.drag_state.dragged_index == Some(index)
                                                    && this.drag_state.is_dragging
                                                {
                                                    // Find the tab under the cursor by checking all tab positions
                                                    // For now, we'll use a simple approach: find nearest tab
                                                    let Some(dragged) =
                                                        this.drag_state.dragged_index
                                                    else {
                                                        return;
                                                    };
                                                    if dragged != index {
                                                        let tab = this.tabs.remove(dragged);
                                                        let insert_at = if dragged < index {
                                                            index - 1
                                                        } else {
                                                            index
                                                        };
                                                        this.tabs.insert(insert_at, tab);
                                                        // Update active_tab if it was affected
                                                        if this.active_tab == dragged {
                                                            this.active_tab = insert_at;
                                                        } else if dragged < this.active_tab
                                                            && insert_at >= this.active_tab
                                                        {
                                                            this.active_tab -= 1;
                                                        } else if dragged > this.active_tab
                                                            && insert_at <= this.active_tab
                                                        {
                                                            this.active_tab += 1;
                                                        }
                                                    }
                                                    this.drag_state = DragState::default();
                                                    cx.notify();
                                                } else if this.drag_state.dragged_index
                                                    == Some(index)
                                                {
                                                    // Click without drag - reset drag state
                                                    this.drag_state = DragState::default();
                                                    cx.notify();
                                                }
                                            }),
                                        )
                                        .child(
                                            div()
                                                .size(px(6.0))
                                                .rounded_full()
                                                .bg(hsla(indicator_color)),
                                        )
                                        .when(is_ssh, |d| {
                                            d.child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .text_size(px(10.0))
                                                    .text_color(hsla(t.accent))
                                                    .child("⇄"),
                                            )
                                        })
                                        .child(
                                            div()
                                                .overflow_hidden()
                                                .child(SharedString::from(label)),
                                        ),
                                )
                                // Close button
                                .child(
                                    div()
                                        .id(("close-workspace-tab", tab_id as usize))
                                        .size(px(22.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded(px(tokens::radius::SM))
                                        .text_color(hsla(t.text_faint))
                                        .hover(|style| {
                                            style.bg(hsla(t.bg_hover)).text_color(hsla(t.text))
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if this.active_tab < this.tabs.len()
                                                && this.tabs[this.active_tab].id == tab_id
                                            {
                                                this.close_active_tab(cx);
                                            }
                                        }))
                                        .child("×"),
                                )
                        }))
                        .child(
                            div()
                                .id("new-workspace-tab")
                                .h(px(24.0))
                                .mb(px(3.0))
                                .px(px(tokens::space::SM))
                                .flex()
                                .items_center()
                                .text_size(px(tokens::text::SM))
                                .rounded(px(tokens::radius::MD))
                                .text_color(hsla(t.text_muted))
                                .hover(|style| style.bg(hsla(t.bg_hover)).text_color(hsla(t.text)))
                                .on_click(cx.listener(|this, _, _, cx| this.new_local_tab(cx)))
                                .child("+"),
                        ),
                )
            })
    }

    pub(super) fn status_color(&self) -> Rgb {
        let t = &self.theme;
        if let Some((state, _)) = self.program_status_summary() {
            return match state {
                tf_session::ProgramState::Blocked => t.warning,
                tf_session::ProgramState::Error => t.danger,
                tf_session::ProgramState::Working => t.accent,
                tf_session::ProgramState::Done => t.success,
                tf_session::ProgramState::Idle | tf_session::ProgramState::Clear => t.text_faint,
            };
        }
        match (self.exited(), self.last_exit) {
            (Some(_), _) => t.text_faint,
            (None, Some(c)) if c != 0 => t.danger,
            _ => t.success,
        }
    }

    pub(super) fn status_bar(&self, grid: Option<GridSize>) -> impl IntoElement {
        let t = &self.theme;
        let mut left = format!(
            "{} command{}",
            self.commands,
            if self.commands == 1 { "" } else { "s" }
        );
        if let Some(code) = self.last_exit {
            left.push_str(&format!("  ·  last exit {code}"));
        }
        if let Some((state, detail)) = self.program_status_summary() {
            let label = match state {
                tf_session::ProgramState::Idle => "idle",
                tf_session::ProgramState::Working => "working",
                tf_session::ProgramState::Done => "done",
                tf_session::ProgramState::Blocked => "blocked",
                tf_session::ProgramState::Error => "error",
                tf_session::ProgramState::Clear => "",
            };
            if !label.is_empty() {
                left.push_str("  ·  ");
                left.push_str(label);
                if let Some(detail) = detail {
                    left.push_str(": ");
                    left.push_str(&detail);
                }
            }
        }
        let mut right = String::new();
        if let Some(host) = self.spawn.ssh_host.as_deref() {
            right.push_str("SSH ");
            right.push_str(host);
            right.push_str("  ·  remote integration is profile-managed  ·  ");
        }
        if let Some(warning) = self.identity_check.warnings.first() {
            right.push_str("Identity warning: ");
            right.push_str(warning);
            right.push_str("  ·  ");
        }
        if self.hover_link.is_some() {
            right.push_str("Ctrl+click to open  ·  ");
        }
        if let Some(g) = grid {
            right.push_str(&format!("{}×{}", g.cols, g.rows));
        }
        div()
            .h(px(tokens::layout::STATUSBAR_H))
            .px(px(tokens::space::SM))
            .flex()
            .items_center()
            .justify_between()
            .text_size(px(tokens::text::XS))
            .text_color(hsla(t.text_faint))
            .bg(hsla(t.bg_app))
            .child(SharedString::from(left))
            .child(SharedString::from(right))
    }

    pub(super) fn program_status_summary(
        &self,
    ) -> Option<(tf_session::ProgramState, Option<String>)> {
        self.session.as_ref().and_then(|session| {
            session.with(|processor| {
                let priority = |state| match state {
                    tf_session::ProgramState::Blocked => 5,
                    tf_session::ProgramState::Error => 4,
                    tf_session::ProgramState::Working => 3,
                    tf_session::ProgramState::Done => 2,
                    tf_session::ProgramState::Idle => 1,
                    tf_session::ProgramState::Clear => 0,
                };
                processor
                    .program_status()
                    .iter()
                    .max_by_key(|record| priority(record.report.state))
                    .map(|record| {
                        let detail = record
                            .report
                            .message
                            .clone()
                            .or_else(|| record.report.title.clone())
                            .or_else(|| record.report.app.clone());
                        (record.report.state, detail)
                    })
            })
        })
    }

    // ---- sidebar and settings ----------------------------------------------

    pub(super) fn toggle_sidebar(&mut self, cx: &mut Context<'_, Self>) {
        self.show_sidebar = !self.show_sidebar;
        self.sidebar_peek = false;
        cx.notify();
    }

    pub(super) fn toggle_context_panel(&mut self, cx: &mut Context<'_, Self>) {
        self.show_context_panel = !self.show_context_panel;
        self.persist_layout();
        cx.notify();
    }

    pub(super) fn toggle_utility_dock(&mut self, cx: &mut Context<'_, Self>) {
        self.show_utility_dock = !self.show_utility_dock;
        self.persist_layout();
        cx.notify();
    }

    pub(super) fn repo_dir(&self) -> Option<PathBuf> {
        self.tabs
            .get(self.active_tab)
            .and_then(|tab| tab.cwd.as_deref())
            .map(crate::workspace::local_path)
            .or_else(|| self.local_project_root.clone())
    }

    /// Resize the dock or right panel while its edge is dragged.
    pub(super) fn drag_edge(&mut self, position: gpui::Point<Pixels>, cx: &mut Context<'_, Self>) {
        let Some(drag) = self.edge_drag else {
            return;
        };
        match drag.edge {
            Edge::Dock => {
                let delta = drag.pointer - f32::from(position.y);
                self.dock_height = (drag.size + delta).clamp(96.0, 520.0);
            }
            Edge::Panel => {
                let delta = drag.pointer - f32::from(position.x);
                self.panel_width = (drag.size + delta).clamp(240.0, 640.0);
            }
        }
        cx.notify();
    }

    /// A thin grab strip that starts an edge drag.
    fn edge_handle(
        &self,
        id: &'static str,
        edge: Edge,
        cx: &mut Context<'_, Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let handle = div().id(id).absolute();
        let handle = match edge {
            Edge::Dock => handle
                .top(px(-2.0))
                .left(px(0.0))
                .right(px(0.0))
                .h(px(5.0))
                .cursor_row_resize(),
            Edge::Panel => handle
                .left(px(-2.0))
                .top(px(0.0))
                .bottom(px(0.0))
                .w(px(5.0))
                .cursor_col_resize(),
        };
        handle
            .hover(|style| style.bg(with_alpha(hsla(self.theme.accent), 0.5)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    let (pointer, size) = match edge {
                        Edge::Dock => (f32::from(ev.position.y), this.dock_height),
                        Edge::Panel => (f32::from(ev.position.x), this.panel_width),
                    };
                    this.edge_drag = Some(EdgeDrag {
                        edge,
                        pointer,
                        size,
                    });
                    cx.stop_propagation();
                }),
            )
    }

    /// Managed processes and agents with observed state, plus launchers.
    fn dock_processes(&self, cx: &mut Context<'_, Self>) -> gpui::AnyElement {
        let t = &self.theme;
        let action = |id: SharedString, label: &'static str, color: Rgb| {
            div()
                .id(id)
                .px(px(tokens::space::XS))
                .rounded(px(tokens::radius::SM))
                .text_size(px(tokens::text::XS))
                .text_color(hsla(color))
                .hover(|style| style.bg(with_alpha(hsla(t.text), 0.08)))
                .child(label)
        };
        let running: Vec<_> = self
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(index, tab)| Self::managed_state(tab).map(|state| (index, tab, state)))
            .collect();
        let mut launchers: Vec<(String, String, bool)> = self
            .agent_clis
            .iter()
            .map(|(label, exe)| (format!("Agent: {label}"), (*exe).to_owned(), true))
            .collect();
        launchers.extend(
            self.tasks
                .items
                .iter()
                .filter(|task| task.state != tf_proto::TaskState::Done)
                .filter_map(|task| Some((task.title.clone(), task.command.clone()?, false))),
        );
        div()
            .id("utility-processes")
            .flex_1()
            .overflow_y_scroll()
            .px(px(tokens::space::SM))
            .py(px(tokens::space::XS))
            .flex()
            .flex_col()
            .gap(px(1.0))
            .children(running.into_iter().map(|(index, tab, state)| {
                let managed = tab.managed.clone().expect("filtered on managed");
                let color = match state {
                    surface::ProcessState::Running => t.accent,
                    surface::ProcessState::Succeeded => t.success,
                    surface::ProcessState::Failed(_) => t.danger,
                    _ => t.text_faint,
                };
                let can_restart = !matches!(
                    state,
                    surface::ProcessState::Running | surface::ProcessState::Pending
                ) && !matches!(state, surface::ProcessState::SessionEnded(_));
                let tab_id = tab.id;
                div()
                    .id(("process-row", tab.id as usize))
                    .h(px(24.0))
                    .px(px(tokens::space::SM))
                    .flex()
                    .items_center()
                    .gap(px(tokens::space::SM))
                    .rounded(px(tokens::radius::SM))
                    .text_size(px(tokens::text::SM))
                    .text_color(hsla(t.text_muted))
                    .hover(|style| style.bg(with_alpha(hsla(t.text), 0.05)))
                    .on_click(cx.listener(move |this, _, _, cx| this.select_tab(index, cx)))
                    .child(div().size(px(6.0)).rounded_full().bg(hsla(color)))
                    .child(div().flex_1().overflow_hidden().whitespace_nowrap().child(
                        SharedString::from(format!(
                            "{}{}",
                            if managed.agent { "agent · " } else { "" },
                            tab.name.clone().unwrap_or(managed.command)
                        )),
                    ))
                    .child(
                        div()
                            .text_size(px(tokens::text::XS))
                            .text_color(hsla(color))
                            .child(SharedString::from(state.label())),
                    )
                    .when(state == surface::ProcessState::Running, |d| {
                        d.child(
                            action(format!("process-stop-{tab_id}").into(), "stop", t.warning)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.stop_managed(index);
                                    cx.notify();
                                })),
                        )
                    })
                    .when(can_restart, |d| {
                        d.child(
                            action(
                                format!("process-restart-{tab_id}").into(),
                                "restart",
                                t.accent,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.restart_managed(index);
                                    cx.notify();
                                },
                            )),
                        )
                    })
            }))
            .child(
                div()
                    .mt(px(tokens::space::XS))
                    .px(px(tokens::space::SM))
                    .text_size(px(tokens::text::XS))
                    .text_color(hsla(t.text_faint))
                    .child("Start"),
            )
            .children(
                launchers
                    .into_iter()
                    .enumerate()
                    .map(|(i, (title, command, agent))| {
                        let label = SharedString::from(format!("▶  {title}"));
                        div()
                            .id(("process-start", i))
                            .h(px(22.0))
                            .px(px(tokens::space::SM))
                            .flex()
                            .items_center()
                            .rounded(px(tokens::radius::SM))
                            .text_size(px(tokens::text::SM))
                            .text_color(hsla(t.text_muted))
                            .hover(|style| style.bg(with_alpha(hsla(t.text), 0.05)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.start_managed(title.clone(), command.clone(), agent, cx)
                            }))
                            .child(label)
                    }),
            )
            .into_any_element()
    }

    /// The workspace shown in the dock, other than the one on the canvas.
    pub(super) fn docked(&self) -> Option<(usize, Arc<SessionHandle>)> {
        let id = self.dock_target?;
        let (index, tab) = self.tabs.iter().enumerate().find(|(_, tab)| tab.id == id)?;
        (index != self.active_tab)
            .then(|| tab.session.clone().map(|session| (index, session)))
            .flatten()
    }

    /// Where keystrokes go: the docked session while the dock has focus.
    pub(super) fn input_session(&self) -> Option<Arc<SessionHandle>> {
        if self.dock_focus
            && self.dock_moved
            && self.show_utility_dock
            && self.dock_tab == DockTab::Output
        {
            if let Some((_, session)) = self.docked() {
                return Some(session);
            }
        }
        self.session.clone()
    }

    /// Show a workspace in the dock instead of on the canvas. The session is
    /// the same one: nothing is spawned, and selecting it moves it back.
    pub(super) fn move_to_dock(&mut self, tab_id: u64, cx: &mut Context<'_, Self>) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            return;
        };
        if self.tabs.len() < 2 {
            return;
        }
        if index == self.active_tab {
            // The canvas needs something else to show.
            let other = if index == 0 { 1 } else { index - 1 };
            self.select_tab(other, cx);
        }
        self.dock_target = Some(tab_id);
        self.dock_moved = true;
        self.dock_focus = true;
        self.dock_tab = DockTab::Output;
        self.show_utility_dock = true;
        self.persist_layout();
        cx.notify();
    }

    pub(super) fn watch_in_dock(&mut self, tab_id: u64, cx: &mut Context<'_, Self>) {
        self.dock_target = Some(tab_id);
        self.dock_moved = false;
        self.dock_focus = false;
        self.dock_tab = DockTab::Output;
        self.show_utility_dock = true;
        cx.notify();
    }

    /// Live, read-only view of another workspace's session. It shows the same
    /// session the sidebar lists: no process is created or restarted.
    fn dock_output(&self, metrics: Metrics, cx: &mut Context<'_, Self>) -> gpui::AnyElement {
        let t = &self.theme;
        let target = self.docked();
        let Some((index, session)) = target else {
            return div()
                .p(px(tokens::space::MD))
                .text_size(px(tokens::text::SM))
                .text_color(hsla(t.text_faint))
                .child("Use \"watch\" on a session in Sessions to follow its output here.")
                .into_any_element();
        };
        let (view, theme, font) = (cx.entity(), Arc::clone(&self.theme), self.font.clone());
        let (font_size, cursor_style) = (px(self.font_size), self.cursor_style);
        let accent_bold = self.appearance.config.terminal.accent_bold;
        let plain = canvas(
            move |bounds: Bounds<Pixels>, _: &mut Window, cx: &mut App| {
                let cols =
                    ((bounds.size.width - px(crate::paint::PAD_X * 2.0)) / metrics.cell_w).floor();
                let rows =
                    ((bounds.size.height - px(crate::paint::PAD_Y * 2.0)) / metrics.line_h).floor();
                let size = GridSize::new(cols.max(2.0) as u16, rows.max(1.0) as u16);
                Some(view.read(cx).frame(&session, size, false))
            },
            move |bounds, frame, window, cx| {
                let Some(frame) = frame else { return };
                paint_grid(
                    PaintArgs {
                        snap: &frame.snap,
                        blocks: &frame.blocks,
                        overlay: &frame.overlay,
                        bounds,
                        theme: &theme,
                        font: &font,
                        font_size,
                        metrics,
                        focused: false,
                        cursor_style,
                        accent_bold,
                    },
                    window,
                    cx,
                );
            },
        )
        .size_full();
        let moved = self.dock_moved;
        let focused = self.dock_focus;
        let name = self
            .tabs
            .get(index)
            .map(|tab| tab.name.clone().unwrap_or_else(|| Self::tab_label(tab)))
            .unwrap_or_default();
        div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .when(moved, |d| {
                d.child(
                    div()
                        .h(px(20.0))
                        .px(px(tokens::space::MD))
                        .flex()
                        .items_center()
                        .justify_between()
                        .text_size(px(tokens::text::XS))
                        .text_color(hsla(if focused { t.accent } else { t.text_faint }))
                        .child(SharedString::from(format!(
                            "{name} · docked{}",
                            if focused { " · typing here" } else { "" }
                        )))
                        .child(
                            div()
                                .id("dock-restore")
                                .px(px(tokens::space::XS))
                                .rounded(px(tokens::radius::SM))
                                .hover(|style| style.bg(with_alpha(hsla(t.text), 0.08)))
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.select_tab(index, cx)),
                                )
                                .child("move to canvas"),
                        ),
                )
            })
            .child(
                div()
                    .id("utility-output")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_hidden()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.dock_moved {
                            this.dock_focus = true;
                            cx.notify();
                        } else {
                            this.select_tab(index, cx);
                        }
                    }))
                    .child(plain),
            )
            .into_any_element()
    }

    /// Re-read the worktree list. Called on tab changes, never from render.
    pub(super) fn refresh_worktrees(&mut self) {
        self.worktrees = self
            .repo_dir()
            .map(|dir| surface::git_worktrees(&dir))
            .unwrap_or_default();
    }

    pub(super) fn refresh_changes(&mut self, cx: &mut Context<'_, Self>) {
        self.changes_dir = self.repo_dir();
        self.changes = self.changes_dir.as_deref().and_then(surface::git_changes);
        cx.notify();
    }

    pub(super) fn set_dock_tab(&mut self, tab: DockTab, cx: &mut Context<'_, Self>) {
        self.dock_tab = tab;
        if tab == DockTab::Changes {
            self.refresh_changes(cx);
        }
        if tab == DockTab::Processes {
            self.agent_clis = surface::AGENT_CLIS
                .iter()
                .copied()
                .filter(|(_, exe)| surface::on_path(exe))
                .collect();
        }
        self.persist_layout();
        cx.notify();
    }

    pub(super) fn open_diff(&mut self, change: &GitChange, cx: &mut Context<'_, Self>) {
        let untracked = change.kind == surface::ChangeKind::Untracked;
        // Diff against the directory the list was read from, not wherever the
        // shell has since moved to.
        self.diff_lines = self
            .changes_dir
            .as_deref()
            .and_then(|dir| surface::git_diff(dir, &change.path, untracked))
            .map(|text| text.lines().map(str::to_owned).collect())
            .unwrap_or_default();
        self.inspector = Inspector::Diff(change.path.clone());
        self.show_context_panel = true;
        cx.notify();
    }

    /// Right panel. It owns no session state, so opening it cannot duplicate
    /// or restart a PTY: it only inspects the active session or one diff.
    pub(super) fn context_panel(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let t = &self.theme;
        let (title, body) = match &self.inspector {
            Inspector::Session => ("Session".to_owned(), self.session_inspector()),
            Inspector::Diff(path) => (path.clone(), self.diff_inspector()),
        };
        let back = matches!(self.inspector, Inspector::Diff(_));
        div()
            .id("context-panel")
            .relative()
            .w(px(self.panel_width))
            .flex_none()
            .flex()
            .flex_col()
            .bg(hsla(t.bg_panel))
            .border_l_1()
            .border_color(hsla(t.border))
            .child(self.edge_handle("context-panel-edge", Edge::Panel, cx))
            .child(
                div()
                    .h(px(34.0))
                    .px(px(tokens::space::MD))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(tokens::space::SM))
                    .border_b_1()
                    .border_color(hsla(t.border))
                    .text_size(px(tokens::text::SM))
                    .text_color(hsla(t.text))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(SharedString::from(title)),
                    )
                    .when(back, |d| {
                        d.child(self.icon_button(
                            "context-panel-back",
                            "‹",
                            cx.listener(|this, _, _, cx| {
                                this.inspector = Inspector::Session;
                                cx.notify();
                            }),
                        ))
                    })
                    .child(self.icon_button(
                        "context-panel-close",
                        "×",
                        cx.listener(|this, _, _, cx| this.toggle_context_panel(cx)),
                    )),
            )
            .child(
                div()
                    .id("context-panel-body")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_scroll()
                    .child(body),
            )
    }

    fn session_inspector(&self) -> gpui::AnyElement {
        let t = &self.theme;
        let location = self.cwd.clone().unwrap_or_else(|| self.project_name());
        let branch = self
            .tabs
            .get(self.active_tab)
            .and_then(|tab| tab.branch.clone())
            .unwrap_or_else(|| "No branch detected".into());
        let activity = self
            .program_status_summary()
            .and_then(|(_, detail)| detail)
            .unwrap_or_else(|| "No program status reported".into());
        let row = |label: &'static str, value: String| {
            div()
                .px(px(tokens::space::MD))
                .py(px(6.0))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(tokens::text::XS))
                        .text_color(hsla(t.text_faint))
                        .child(label),
                )
                .child(
                    div()
                        .text_size(px(tokens::text::SM))
                        .text_color(hsla(t.text_muted))
                        .child(SharedString::from(value)),
                )
        };
        div()
            .flex()
            .flex_col()
            .child(row("Directory", location))
            .child(row("Branch", branch))
            .child(row("Activity", activity))
            .into_any_element()
    }

    fn diff_inspector(&self) -> gpui::AnyElement {
        let t = &self.theme;
        if self.diff_lines.is_empty() {
            return div()
                .p(px(tokens::space::MD))
                .text_size(px(tokens::text::SM))
                .text_color(hsla(t.text_faint))
                .child("No textual diff available for this file.")
                .into_any_element();
        }
        div()
            .py(px(tokens::space::XS))
            .flex()
            .flex_col()
            .font_family(self.font.family.clone())
            .text_size(px(11.5))
            .children(self.diff_lines.iter().map(|line| {
                let (fg, bg) = match surface::classify_diff_line(line) {
                    surface::DiffLine::Added => {
                        (t.success, Some(with_alpha(hsla(t.success), 0.10)))
                    }
                    surface::DiffLine::Removed => {
                        (t.danger, Some(with_alpha(hsla(t.danger), 0.10)))
                    }
                    surface::DiffLine::Hunk => (t.accent, None),
                    surface::DiffLine::Header => (t.text_faint, None),
                    surface::DiffLine::Context => (t.text_muted, None),
                };
                div()
                    .px(px(tokens::space::MD))
                    .whitespace_nowrap()
                    .text_color(hsla(fg))
                    .when_some(bg, |d, bg| d.bg(bg))
                    .child(SharedString::from(line.clone()))
            }))
            .into_any_element()
    }

    /// Bottom dock: Sessions selects existing workspaces (never a second PTY
    /// for a different presentation); Changes lists the working-tree diff.
    pub(super) fn utility_dock(
        &self,
        metrics: Metrics,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        let edge = self.edge_handle("utility-dock-edge", Edge::Dock, cx);
        let processes = (self.dock_tab == DockTab::Processes).then(|| self.dock_processes(cx));
        let output = (self.dock_tab == DockTab::Output).then(|| self.dock_output(metrics, cx));
        let change_count = self.changes.as_ref().map_or(0, Vec::len);
        let tab = |id: &'static str, label: String, which: DockTab| {
            let active = self.dock_tab == which;
            div()
                .id(id)
                .h(px(22.0))
                .px(px(tokens::space::SM))
                .flex()
                .items_center()
                .rounded(px(tokens::radius::SM))
                .text_size(px(tokens::text::SM))
                .text_color(hsla(if active { t.text } else { t.text_muted }))
                .when(active, |d| d.bg(with_alpha(hsla(t.text), 0.08)))
                .hover(|style| style.text_color(hsla(t.text)))
                .on_click(cx.listener(move |this, _, _, cx| this.set_dock_tab(which, cx)))
                .child(SharedString::from(label))
        };
        let body: gpui::AnyElement = match self.dock_tab {
            DockTab::Sessions => div()
                .flex_1()
                .px(px(tokens::space::SM))
                .py(px(tokens::space::XS))
                .flex()
                .flex_col()
                .gap(px(1.0))
                .children(self.tabs.iter().enumerate().map(|(index, tab)| {
                    let active = index == self.active_tab;
                    let label = tab.name.clone().unwrap_or_else(|| Self::tab_label(tab));
                    let branch = tab.branch.clone().unwrap_or_default();
                    div()
                        .id(("utility-session", tab.id as usize))
                        .h(px(24.0))
                        .px(px(tokens::space::SM))
                        .flex()
                        .items_center()
                        .justify_between()
                        .rounded(px(tokens::radius::SM))
                        .text_size(px(tokens::text::SM))
                        .text_color(hsla(if active { t.text } else { t.text_muted }))
                        .when(active, |d| d.bg(with_alpha(hsla(t.text), 0.07)))
                        .hover(|style| style.bg(with_alpha(hsla(t.text), 0.05)))
                        .on_click(cx.listener(move |this, _, _, cx| this.select_tab(index, cx)))
                        .child(SharedString::from(label))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(tokens::space::SM))
                                .child(
                                    div()
                                        .text_size(px(tokens::text::XS))
                                        .text_color(hsla(t.text_faint))
                                        .child(SharedString::from(branch)),
                                )
                                .child({
                                    let tab_id = tab.id;
                                    div()
                                        .id(("utility-dock-move", tab.id as usize))
                                        .px(px(tokens::space::XS))
                                        .rounded(px(tokens::radius::SM))
                                        .text_size(px(tokens::text::XS))
                                        .text_color(hsla(t.accent))
                                        .hover(|style| style.bg(with_alpha(hsla(t.text), 0.08)))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.move_to_dock(tab_id, cx);
                                        }))
                                        .child("dock")
                                })
                                .when(!active, |d| {
                                    let tab_id = tab.id;
                                    d.child(
                                        div()
                                            .id(("utility-watch", tab.id as usize))
                                            .px(px(tokens::space::XS))
                                            .rounded(px(tokens::radius::SM))
                                            .text_size(px(tokens::text::XS))
                                            .text_color(hsla(t.accent))
                                            .hover(|style| style.bg(with_alpha(hsla(t.text), 0.08)))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.watch_in_dock(tab_id, cx);
                                            }))
                                            .child("watch"),
                                    )
                                }),
                        )
                }))
                .into_any_element(),
            DockTab::Output => output.unwrap_or_else(|| div().into_any_element()),
            DockTab::Processes => processes.unwrap_or_else(|| div().into_any_element()),
            DockTab::Changes => match &self.changes {
                None => div()
                    .p(px(tokens::space::MD))
                    .text_size(px(tokens::text::SM))
                    .text_color(hsla(t.text_faint))
                    .child("Not inside a Git repository.")
                    .into_any_element(),
                Some(changes) if changes.is_empty() => div()
                    .p(px(tokens::space::MD))
                    .text_size(px(tokens::text::SM))
                    .text_color(hsla(t.text_faint))
                    .child("Working tree clean.")
                    .into_any_element(),
                Some(changes) => div()
                    .id("utility-changes")
                    .flex_1()
                    .overflow_y_scroll()
                    .px(px(tokens::space::SM))
                    .py(px(tokens::space::XS))
                    .flex()
                    .flex_col()
                    .children(changes.iter().enumerate().map(|(index, change)| {
                        let color = match change.kind {
                            surface::ChangeKind::Added | surface::ChangeKind::Untracked => {
                                t.success
                            }
                            surface::ChangeKind::Deleted | surface::ChangeKind::Conflicted => {
                                t.danger
                            }
                            _ => t.warning,
                        };
                        let picked = change.clone();
                        let selected = self.inspector == Inspector::Diff(change.path.clone());
                        div()
                            .id(("utility-change", index))
                            .h(px(22.0))
                            .px(px(tokens::space::SM))
                            .flex()
                            .items_center()
                            .gap(px(tokens::space::SM))
                            .rounded(px(tokens::radius::SM))
                            .text_size(px(tokens::text::SM))
                            .text_color(hsla(t.text_muted))
                            .when(selected, |d| d.bg(with_alpha(hsla(t.text), 0.07)))
                            .hover(|style| style.bg(with_alpha(hsla(t.text), 0.05)))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.open_diff(&picked, cx)),
                            )
                            .child(
                                div()
                                    .w(px(12.0))
                                    .text_color(hsla(color))
                                    .child(change.kind.letter()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(SharedString::from(change.path.clone())),
                            )
                            .when(change.staged, |d| {
                                d.child(
                                    div()
                                        .text_size(px(tokens::text::XS))
                                        .text_color(hsla(t.text_faint))
                                        .child("staged"),
                                )
                            })
                    }))
                    .into_any_element(),
            },
        };
        div()
            .id("utility-dock")
            .relative()
            .h(px(self.dock_height))
            .flex_none()
            .flex()
            .flex_col()
            .bg(hsla(t.bg_panel))
            .border_t_1()
            .border_color(hsla(if self.dock_focus && self.dock_moved {
                t.accent
            } else {
                t.border
            }))
            .child(edge)
            .child(
                div()
                    .h(px(30.0))
                    .px(px(tokens::space::SM))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .gap(px(2.0))
                            .child(tab(
                                "dock-tab-sessions",
                                "Sessions".into(),
                                DockTab::Sessions,
                            ))
                            .child(tab(
                                "dock-tab-changes",
                                if change_count > 0 {
                                    format!("Changes {change_count}")
                                } else {
                                    "Changes".into()
                                },
                                DockTab::Changes,
                            ))
                            .child(tab(
                                "dock-tab-processes",
                                "Processes".into(),
                                DockTab::Processes,
                            ))
                            .child(tab("dock-tab-output", "Terminal".into(), DockTab::Output)),
                    )
                    .child(self.icon_button(
                        "utility-dock-close",
                        "×",
                        cx.listener(|this, _, _, cx| this.toggle_utility_dock(cx)),
                    )),
            )
            .child(body)
    }

    pub(super) fn open_settings(&mut self, cx: &mut Context<'_, Self>) {
        self.settings = Some(self.settings.unwrap_or_default());
        self.palette = None;
        self.search = None;
        cx.notify();
    }

    pub(super) fn toggle_section(&mut self, section: SidebarSection, cx: &mut Context<'_, Self>) {
        if !self.collapsed_sections.remove(&section) {
            self.collapsed_sections.insert(section);
        }
        cx.notify();
    }

    pub(super) fn open_palette(&mut self, cx: &mut Context<'_, Self>) {
        self.ssh_hosts = tf_pty::discover_ssh_hosts();
        self.palette = Some(PaletteState::default());
        cx.notify();
    }

    /// Configured SSH aliases are useful when connecting, but should not
    /// permanently occupy navigation space. Open the command palette already
    /// narrowed to remote-connection actions instead.
    pub(super) fn open_ssh_picker(&mut self, cx: &mut Context<'_, Self>) {
        self.ssh_hosts = tf_pty::discover_ssh_hosts();
        self.palette = Some(PaletteState {
            query: "ssh:".into(),
            ..PaletteState::default()
        });
        self.search = None;
        cx.notify();
    }

    pub(super) fn close_tab_at(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if index < self.tabs.len() {
            self.select_tab(index, cx);
            self.close_active_tab(cx);
        }
    }
}
