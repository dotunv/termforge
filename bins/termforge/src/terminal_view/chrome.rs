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

    pub(super) fn header(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let t = &self.theme;
        let tab_count = self.tabs.len();
        div()
            .h(px(if self.show_tab_strip { 72.0 } else { 30.0 }))
            .flex()
            .flex_col()
            .bg(hsla(t.bg_app))
            .border_b_1()
            .border_color(hsla(t.border))
            .child(
                div()
                    .h(px(30.0))
                    .px(px(tokens::space::MD))
                    .flex()
                    .items_center()
                    .justify_between()
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
                                    .text_color(hsla(t.text))
                                    .child(SharedString::from(self.project_name())),
                            )
                            .child(
                                div()
                                    .text_size(px(tokens::text::XS))
                                    .text_color(hsla(t.text_faint))
                                    .child(SharedString::from(
                                        self.cwd
                                            .clone()
                                            .unwrap_or_else(|| "Project workspace".into()),
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(tokens::space::SM))
                            .child(
                                div()
                                    .text_size(px(tokens::text::XS))
                                    .text_color(hsla(t.text_faint))
                                    .child("Ctrl Shift P  Commands"),
                            )
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
                    ),
            )
            .when(self.show_tab_strip, |header| {
                header.child(
                    div()
                        .h(px(42.0))
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
                                .h(px(36.0))
                                .max_w(px(240.0))
                                .px(px(tokens::space::MD))
                                .flex()
                                .items_center()
                                .gap(px(tokens::space::SM))
                                .rounded(px(tokens::radius::MD))
                                .when(active, |d| d.bg(hsla(t.bg_panel)).text_color(hsla(t.text)))
                                .when(!active, |d| {
                                    d.text_color(hsla(t.text_muted))
                                        .hover(|style| style.bg(hsla(t.bg_hover)))
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
                                .h(px(30.0))
                                .mb(px(3.0))
                                .px(px(tokens::space::SM))
                                .flex()
                                .items_center()
                                .rounded(px(tokens::radius::MD))
                                .text_color(hsla(t.text_muted))
                                .hover(|style| style.bg(hsla(t.bg_hover)).text_color(hsla(t.text)))
                                .on_click(cx.listener(|this, _, _, cx| this.new_local_tab(cx)))
                                .child("+  New terminal"),
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
        if self.hover_link.is_some() {
            right.push_str("Ctrl+click to open  ·  ");
        }
        if let Some(g) = grid {
            right.push_str(&format!("{}×{}", g.cols, g.rows));
        }
        div()
            .h(px(tokens::layout::STATUSBAR_H))
            .px(px(tokens::space::MD))
            .flex()
            .items_center()
            .justify_between()
            .text_size(px(tokens::text::XS))
            .text_color(hsla(t.text_faint))
            .bg(hsla(t.bg_app))
            .border_t_1()
            .border_color(hsla(t.border))
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

    pub(super) fn close_tab_at(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if index < self.tabs.len() {
            self.select_tab(index, cx);
            self.close_active_tab(cx);
        }
    }

    pub(super) fn set_dark_theme(&mut self, dark: bool, cx: &mut Context<'_, Self>) {
        let input = if dark {
            ThemeInput::DARK
        } else {
            ThemeInput::LIGHT
        };
        self.theme = Arc::new(Theme::generate(input));
        cx.notify();
    }
}
