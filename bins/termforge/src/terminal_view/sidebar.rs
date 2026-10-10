//! Workspace sidebar and shell menu.

use super::*;

impl TerminalView {
    pub(super) fn icon_button(
        &self,
        id: &'static str,
        glyph: &'static str,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let t = &self.theme;
        div()
            .id(id)
            .size(px(24.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(tokens::radius::SM))
            .text_color(hsla(t.text_faint))
            .hover(|style| style.bg(hsla(t.bg_hover)).text_color(hsla(t.text)))
            .on_click(on_click)
            .child(glyph)
    }

    /// A full-width clickable sidebar row.
    pub(super) fn sidebar_row(
        &self,
        id: &'static str,
        label: &'static str,
        hint: &'static str,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let t = &self.theme;
        div()
            .id(id)
            .h(px(32.0))
            .px(px(tokens::space::SM))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(tokens::space::MD))
            .rounded(px(tokens::radius::MD))
            .text_size(px(tokens::text::MD))
            .text_color(hsla(t.text_muted))
            .hover(|style| style.bg(hsla(t.bg_hover)).text_color(hsla(t.text)))
            .on_click(on_click)
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(label),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(tokens::text::XS))
                    .text_color(hsla(t.text_faint))
                    .child(hint),
            )
    }

    pub(super) fn section_header(
        &self,
        section: SidebarSection,
        id: &'static str,
        title: &'static str,
        action: Option<SectionAction>,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        let open = !self.collapsed_sections.contains(&section);
        div()
            .h(px(26.0))
            .mt(px(tokens::space::SM))
            .px(px(tokens::space::SM))
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .id(id)
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap(px(tokens::space::XS))
                    .text_size(px(tokens::text::XS))
                    .text_color(hsla(t.text_faint))
                    .hover(|style| style.text_color(hsla(t.text)))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_section(section, cx)))
                    .child(if open { "▾" } else { "▸" })
                    .child(title),
            )
            .children(action.map(|(action_id, glyph, handler)| {
                self.icon_button(action_id, glyph, move |ev, window, cx| {
                    handler(ev, window, cx)
                })
            }))
    }

    /// Last two path components, so rows stay readable at any sidebar width.
    pub(super) fn short_path(path: &str) -> String {
        let trimmed = path.trim_end_matches(['/', '\\']);
        let parts: Vec<&str> = trimmed.rsplit(['/', '\\']).take(2).collect();
        match parts.as_slice() {
            [leaf, parent] if !parent.is_empty() => format!("{parent}/{leaf}"),
            [leaf, ..] if !leaf.is_empty() => (*leaf).to_owned(),
            _ => trimmed.to_owned(),
        }
    }

    /// Highest-priority program status line for a tab, if any program reports one.
    pub(super) fn tab_status_text(tab: &WorkspaceTab) -> Option<String> {
        let session = tab.session.as_ref()?;
        session.with(|p| {
            let rank = |state| match state {
                tf_session::ProgramState::Blocked => 5,
                tf_session::ProgramState::Error => 4,
                tf_session::ProgramState::Working => 3,
                tf_session::ProgramState::Done => 2,
                tf_session::ProgramState::Idle => 1,
                tf_session::ProgramState::Clear => 0,
            };
            let status = p.program_status();
            let record = status.iter().max_by_key(|r| rank(r.report.state))?;
            let label = match record.report.state {
                tf_session::ProgramState::Blocked => "blocked",
                tf_session::ProgramState::Error => "error",
                tf_session::ProgramState::Working => "working",
                tf_session::ProgramState::Done => "done",
                tf_session::ProgramState::Idle | tf_session::ProgramState::Clear => return None,
            };
            let detail = record
                .report
                .message
                .clone()
                .or_else(|| record.report.title.clone())
                .or_else(|| record.report.app.clone());
            Some(match detail {
                Some(detail) => format!("{label} · {detail}"),
                None => label.to_owned(),
            })
        })
    }

    pub(super) fn workspace_row(
        &self,
        index: usize,
        tab: &WorkspaceTab,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        let active = index == self.active_tab;
        let tab_id = tab.id as usize;
        let is_ssh = tab.spawn.ssh_host.is_some();
        let renaming = self
            .renaming
            .as_ref()
            .filter(|rename| rename.tab_id == tab.id)
            .map(|rename| rename.text.clone());
        let title = tab
            .name
            .clone()
            .or_else(|| tab.title.clone().filter(|title| !title.trim().is_empty()))
            .unwrap_or_else(|| Self::tab_label(tab));
        let mut subtitle = tab
            .cwd
            .as_deref()
            .map(Self::short_path)
            .unwrap_or_else(|| Self::tab_label(tab));
        if let Some(branch) = &tab.branch {
            subtitle = format!("⎇ {branch}  ·  {subtitle}");
        }
        if self.shells.len() > 1 && !is_ssh {
            subtitle = format!("{}  ·  {subtitle}", tab.spawn.profile.name);
        }
        let status = Self::tab_status_text(tab);
        let dot = self.tab_status_color(tab, active, t);
        let ring = tab.attention.map(|attention| attention.color(t));
        let note = tab
            .attention_note
            .clone()
            .filter(|_| tab.attention.is_some())
            .or(status);
        let note_color = ring.unwrap_or(dot);
        let pinned = tab.pinned;
        let dragged = DraggedWorkspace {
            index,
            title: SharedString::from(title.clone()),
            bg: t.bg_elevated,
            fg: t.text,
            border: t.border_strong,
        };
        let hover_bg = t.bg_hover;
        div()
            .id(("workspace-row", tab_id))
            .group(SharedString::from(format!("ws-row-{tab_id}")))
            .relative()
            .px(px(tokens::space::SM))
            .py(px(6.0))
            .flex()
            .items_start()
            .gap(px(tokens::space::SM))
            .rounded(px(tokens::radius::MD))
            .when(active, |d| d.bg(hsla(t.bg_selected)))
            .when(!active, |d| d.hover(|style| style.bg(hsla(t.bg_hover))))
            .drag_over::<DraggedWorkspace>(move |style, _, _, _| style.bg(hsla(hover_bg)))
            .on_drag(dragged, |dragged, _, _, cx| {
                cx.new(|_| DragPreview(dragged.clone()))
            })
            .on_drop(cx.listener(move |this, dragged: &DraggedWorkspace, _, cx| {
                this.move_tab(dragged.index, index, cx);
            }))
            .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                if ev.click_count() >= 2 {
                    this.begin_rename(index, cx);
                } else {
                    this.select_tab(index, cx);
                }
            }))
            .children(ring.map(|color| {
                div()
                    .absolute()
                    .left(px(0.0))
                    .top(px(6.0))
                    .bottom(px(6.0))
                    .w(px(2.0))
                    .rounded_full()
                    .bg(hsla(color))
            }))
            .child(
                div()
                    .mt(px(4.0))
                    .size(px(9.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .when_some(ring, |d, color| d.border_1().border_color(hsla(color)))
                    .child(div().size(px(5.0)).rounded_full().bg(hsla(dot))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(tokens::space::XS))
                            .text_color(hsla(if active { t.text } else { t.text_muted }))
                            .child(match renaming {
                                Some(text) => div()
                                    .px(px(tokens::space::XS))
                                    .rounded(px(tokens::radius::SM))
                                    .bg(hsla(t.bg_elevated))
                                    .border_1()
                                    .border_color(hsla(t.accent))
                                    .text_color(hsla(t.text))
                                    .child(SharedString::from(format!("{text}▏"))),
                                None => div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(SharedString::from(title)),
                            })
                            .when(is_ssh, |d| {
                                d.child(
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(hsla(t.accent))
                                        .child("⇄"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(tokens::text::XS))
                            .text_color(hsla(t.text_faint))
                            .child(SharedString::from(subtitle)),
                    )
                    .children(note.map(|note| {
                        div()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(tokens::text::XS))
                            .text_color(hsla(note_color))
                            .child(SharedString::from(note))
                    })),
            )
            .child(
                div()
                    .id(("workspace-row-pin", tab_id))
                    .flex_none()
                    .when(!pinned, |d| {
                        d.invisible()
                            .group_hover(SharedString::from(format!("ws-row-{tab_id}")), |style| {
                                style.visible()
                            })
                    })
                    .size(px(20.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(tokens::radius::SM))
                    .text_color(hsla(if pinned { t.accent } else { t.text_faint }))
                    .hover(|style| style.bg(hsla(t.bg_hover)).text_color(hsla(t.text)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_pin(index, cx);
                    }))
                    .child(if pinned { "★" } else { "☆" }),
            )
            .child(
                div()
                    .id(("workspace-row-close", tab_id))
                    .flex_none()
                    .invisible()
                    .group_hover(SharedString::from(format!("ws-row-{tab_id}")), |style| {
                        style.visible()
                    })
                    .size(px(20.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(tokens::radius::SM))
                    .text_color(hsla(t.text_faint))
                    .hover(|style| style.bg(hsla(t.bg_hover)).text_color(hsla(t.text)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.close_tab_at(index, cx);
                    }))
                    .child("×"),
            )
    }

    pub(super) fn workspace_sidebar(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let t = &self.theme;
        let project = self.project_name();
        let branch_hint = match self
            .tabs
            .get(self.active_tab)
            .and_then(|tab| tab.branch.clone())
        {
            Some(branch) => format!("⎇ {branch}"),
            None => self
                .local_project_root
                .as_deref()
                .map(|root| Self::short_path(&root.to_string_lossy()))
                .unwrap_or_else(|| "no project".into()),
        };
        let attention_count = self
            .tabs
            .iter()
            .filter(|tab| tab.attention.is_some())
            .count();
        let attention_color = self
            .tabs
            .iter()
            .filter_map(|tab| tab.attention)
            .max()
            .map_or(t.accent, |level| level.color(t));
        let workspaces_open = !self
            .collapsed_sections
            .contains(&SidebarSection::Workspaces);
        let hosts_open = !self.collapsed_sections.contains(&SidebarSection::SshHosts);
        let project_open = !self.collapsed_sections.contains(&SidebarSection::Project);
        let rows: Vec<_> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| self.workspace_row(index, tab, cx))
            .collect();
        let hosts: Vec<_> = self
            .ssh_hosts
            .iter()
            .map(|host| {
                let target = host.clone();
                let label = SharedString::from(host.clone());
                div()
                    .id(SharedString::from(format!("sidebar-host-{host}")))
                    .h(px(28.0))
                    .px(px(tokens::space::SM))
                    .flex()
                    .items_center()
                    .gap(px(tokens::space::SM))
                    .rounded(px(tokens::radius::MD))
                    .text_color(hsla(t.text_muted))
                    .hover(|style| style.bg(hsla(t.bg_hover)).text_color(hsla(t.text)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.new_ssh_tab(target.clone(), cx);
                    }))
                    .child(div().text_color(hsla(t.accent)).child("⇄"))
                    .child(div().overflow_hidden().child(label))
            })
            .collect();
        let new_tab_action: ClickHandler = {
            let view = cx.entity();
            Box::new(move |_, _, cx| view.update(cx, |this, cx| this.new_local_tab(cx)))
        };
        let refresh_hosts: ClickHandler = {
            let view = cx.entity();
            Box::new(move |_, _, cx| {
                view.update(cx, |this, cx| {
                    this.ssh_hosts = tf_pty::discover_ssh_hosts();
                    cx.notify();
                });
            })
        };

        div()
            .w(px(self.sidebar_width))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(hsla(t.bg_app))
            .border_r_1()
            .border_color(hsla(t.border))
            .child(
                div()
                    .p(px(tokens::space::SM))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .px(px(tokens::space::SM))
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_color(hsla(t.text))
                                    .child(SharedString::from(project)),
                            )
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(px(tokens::text::XS))
                                    .text_color(hsla(t.text_faint))
                                    .child(SharedString::from(branch_hint)),
                            ),
                    )
                    .children((attention_count > 0).then(|| {
                        div()
                            .id("sidebar-attention")
                            .mr(px(tokens::space::XS))
                            .px(px(6.0))
                            .py(px(1.0))
                            .rounded_full()
                            .bg(hsla(attention_color))
                            .text_size(px(tokens::text::XS))
                            .text_color(hsla(t.accent_text))
                            .hover(|style| style.opacity(0.85))
                            .on_click(cx.listener(|this, _, _, cx| this.jump_to_attention(cx)))
                            .child(SharedString::from(format!("● {attention_count}")))
                    }))
                    .child(self.icon_button(
                        "sidebar-hide",
                        if self.show_sidebar { "‹" } else { "◧" },
                        cx.listener(|this, _, _, cx| this.toggle_sidebar(cx)),
                    )),
            )
            .child(
                div()
                    .px(px(tokens::space::SM))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(div().flex_1().child(self.sidebar_row(
                                "sidebar-new-session",
                                "+  New terminal",
                                "Ctrl Shift T",
                                cx.listener(|this, _, _, cx| this.new_local_tab(cx)),
                            )))
                            .child(self.icon_button(
                                "sidebar-shell-menu",
                                "▾",
                                cx.listener(|this, _, _, cx| {
                                    this.shell_menu = !this.shell_menu;
                                    cx.notify();
                                }),
                            )),
                    )
                    .child(self.sidebar_row(
                        "sidebar-palette",
                        "⌘  Command palette",
                        "Ctrl Shift P",
                        cx.listener(|this, _, _, cx| this.open_palette(cx)),
                    )),
            )
            .child(
                div()
                    .id("sidebar-scroll")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .px(px(tokens::space::SM))
                    .flex()
                    .flex_col()
                    .child(self.section_header(
                        SidebarSection::Workspaces,
                        "section-workspaces",
                        "WORKSPACES",
                        Some(("section-workspaces-add", "+", new_tab_action)),
                        cx,
                    ))
                    .when(workspaces_open, |d| {
                        d.child(div().flex().flex_col().gap(px(2.0)).children(rows))
                    })
                    .child(self.section_header(
                        SidebarSection::SshHosts,
                        "section-hosts",
                        "SSH HOSTS",
                        Some(("section-hosts-refresh", "↻", refresh_hosts)),
                        cx,
                    ))
                    .when(hosts_open, |d| {
                        if hosts.is_empty() {
                            d.child(
                                div()
                                    .px(px(tokens::space::SM))
                                    .py(px(4.0))
                                    .text_size(px(tokens::text::XS))
                                    .text_color(hsla(t.text_faint))
                                    .child("No hosts in ~/.ssh/config"),
                            )
                        } else {
                            d.child(div().flex().flex_col().children(hosts))
                        }
                    })
                    .child(self.tasks_section(cx))
                    .child(self.section_header(
                        SidebarSection::Project,
                        "section-project",
                        "PROJECT",
                        None,
                        cx,
                    ))
                    .when(project_open, |d| {
                        d.child(self.sidebar_row(
                            "sidebar-find",
                            "Find in terminal",
                            "Ctrl Shift F",
                            cx.listener(|this, _, _, cx| this.open_search(cx)),
                        ))
                        .child(self.sidebar_row(
                            "sidebar-doctor",
                            "Daemon diagnostics",
                            "↗",
                            cx.listener(|this, _, _, _| this.write(b"forged doctor\r")),
                        ))
                    }),
            )
            .child(
                div()
                    .p(px(tokens::space::SM))
                    .border_t_1()
                    .border_color(hsla(t.border))
                    .child(self.sidebar_row(
                        "sidebar-settings",
                        "⚙  Settings",
                        "Ctrl ,",
                        cx.listener(|this, _, _, cx| this.open_settings(cx)),
                    )),
            )
    }

    /// Dropdown listing the shells available on this machine.
    pub(super) fn shell_menu_overlay(
        &self,
        cx: &mut Context<'_, Self>,
    ) -> Option<impl IntoElement> {
        if !self.shell_menu {
            return None;
        }
        let t = &self.theme;
        let default = self.local_spawn.profile.name.clone();
        let items: Vec<_> = self
            .shells
            .iter()
            .enumerate()
            .map(|(index, shell)| {
                let is_default = shell.name == default;
                div()
                    .id(SharedString::from(format!("shell-menu-{index}")))
                    .h(px(28.0))
                    .px(px(tokens::space::SM))
                    .flex()
                    .items_center()
                    .justify_between()
                    .rounded(px(tokens::radius::MD))
                    .text_color(hsla(t.text_muted))
                    .hover(|style| style.bg(hsla(t.bg_hover)).text_color(hsla(t.text)))
                    .on_click(cx.listener(move |this, _, _, cx| this.new_shell_tab(index, cx)))
                    .child(SharedString::from(shell.name.clone()))
                    .child(
                        div()
                            .text_size(px(tokens::text::XS))
                            .text_color(hsla(t.text_faint))
                            .child(if is_default { "default" } else { "" }),
                    )
            })
            .collect();
        Some(
            div()
                .id("shell-menu-scrim")
                .absolute()
                .top(px(0.0))
                .bottom(px(0.0))
                .left(px(0.0))
                .right(px(0.0))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.shell_menu = false;
                    cx.notify();
                }))
                .child(
                    div()
                        .id("shell-menu")
                        .absolute()
                        .top(px(74.0))
                        .left(px(tokens::space::SM))
                        .w(px((self.sidebar_width - 2.0 * tokens::space::SM).max(180.0)))
                        .p(px(tokens::space::XS))
                        .flex()
                        .flex_col()
                        .rounded(px(tokens::radius::LG))
                        .bg(hsla(t.bg_elevated))
                        .border_1()
                        .border_color(hsla(t.border_strong))
                        .shadow_lg()
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .px(px(tokens::space::SM))
                                .py(px(tokens::space::XS))
                                .text_size(px(tokens::text::XS))
                                .text_color(hsla(t.text_faint))
                                .child("NEW TERMINAL WITH…"),
                        )
                        .children(items),
                ),
        )
    }
}
