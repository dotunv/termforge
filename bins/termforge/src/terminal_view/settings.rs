//! Settings panel.

use super::*;

impl TerminalView {
    pub(super) fn settings_section_title(&self, title: &'static str) -> impl IntoElement {
        div()
            .mt(px(tokens::space::LG))
            .mb(px(tokens::space::SM))
            .px(px(2.0))
            .text_size(px(tokens::text::XS))
            .text_color(hsla(self.theme.text_faint))
            .child(title)
    }

    /// A rounded group of rows with hairline dividers between them.
    pub(super) fn settings_card(&self, rows: Vec<gpui::AnyElement>) -> gpui::AnyElement {
        let t = &self.theme;
        let mut children = Vec::with_capacity(rows.len() * 2);
        for (index, row) in rows.into_iter().enumerate() {
            if index > 0 {
                children.push(
                    div()
                        .h(px(1.0))
                        .mx(px(tokens::space::MD))
                        .bg(hsla(t.border))
                        .into_any_element(),
                );
            }
            children.push(row);
        }
        div()
            .flex()
            .flex_col()
            .rounded(px(tokens::radius::LG))
            .bg(hsla(t.bg_panel))
            .border_1()
            .border_color(hsla(t.border))
            .children(children)
            .into_any_element()
    }

    pub(super) fn setting_row(
        &self,
        label: &'static str,
        description: &'static str,
        control: impl IntoElement,
    ) -> gpui::AnyElement {
        let t = &self.theme;
        div()
            .px(px(tokens::space::MD))
            .py(px(tokens::space::MD))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(tokens::space::XL))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(px(tokens::text::MD))
                            .text_color(hsla(t.text))
                            .child(label),
                    )
                    .when(!description.is_empty(), |d| {
                        d.child(
                            div()
                                .text_size(px(tokens::text::SM))
                                .line_height(px(17.0))
                                .text_color(hsla(t.text_faint))
                                .child(description),
                        )
                    }),
            )
            .child(div().flex_none().child(control))
            .into_any_element()
    }

    pub(super) fn toggle_switch(
        &self,
        id: &'static str,
        on: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let t = &self.theme;
        div()
            .id(id)
            .w(px(38.0))
            .h(px(22.0))
            .p(px(2.0))
            .flex()
            .items_center()
            .when(on, |d| d.justify_end())
            .rounded_full()
            .cursor_pointer()
            .bg(hsla(if on { t.accent } else { t.bg_hover }))
            .border_1()
            .border_color(hsla(if on { t.accent } else { t.border_strong }))
            .on_click(on_click)
            .child(div().size(px(16.0)).rounded_full().bg(hsla(if on {
                t.accent_text
            } else {
                t.text_muted
            })))
    }

    pub(super) fn button(
        &self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let t = &self.theme;
        div()
            .id(id)
            .h(px(28.0))
            .min_w(px(28.0))
            .px(px(tokens::space::MD))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(tokens::radius::MD))
            .cursor_pointer()
            .bg(hsla(t.bg_elevated))
            .border_1()
            .border_color(hsla(t.border_strong))
            .text_color(hsla(t.text))
            .hover(|style| style.bg(hsla(t.bg_hover)))
            .on_click(on_click)
            .child(label.into())
    }

    /// Joined options of which exactly one is selected.
    pub(super) fn segmented(
        &self,
        id_prefix: &'static str,
        options: Vec<(SharedString, bool, ClickHandler)>,
    ) -> impl IntoElement {
        let t = &self.theme;
        div()
            .flex()
            .rounded(px(tokens::radius::MD))
            .border_1()
            .border_color(hsla(t.border_strong))
            .bg(hsla(t.bg_elevated))
            .overflow_hidden()
            .children(
                options
                    .into_iter()
                    .enumerate()
                    .map(|(index, (label, selected, handler))| {
                        div()
                            .id(SharedString::from(format!("{id_prefix}-{index}")))
                            .h(px(28.0))
                            .px(px(tokens::space::MD))
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .when(index > 0, |d| d.border_l_1().border_color(hsla(t.border)))
                            .when(selected, |d| {
                                d.bg(hsla(t.accent)).text_color(hsla(t.accent_text))
                            })
                            .when(!selected, |d| {
                                d.text_color(hsla(t.text_muted)).hover(|style| {
                                    style.bg(hsla(t.bg_hover)).text_color(hsla(t.text))
                                })
                            })
                            .on_click(move |ev, window, cx| handler(ev, window, cx))
                            .child(label)
                    }),
            )
    }

    pub(super) fn theme_card(
        &self,
        id: &'static str,
        label: &'static str,
        input: ThemeInput,
        selected: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let t = &self.theme;
        let preview = Theme::generate(input);
        div()
            .id(id)
            .flex()
            .flex_col()
            .gap(px(tokens::space::SM))
            .cursor_pointer()
            .on_click(on_click)
            .child(
                div()
                    .w(px(168.0))
                    .h(px(92.0))
                    .p(px(tokens::space::MD))
                    .flex()
                    .flex_col()
                    .gap(px(7.0))
                    .rounded(px(tokens::radius::LG))
                    .bg(hsla(preview.bg_app))
                    .border_2()
                    .border_color(hsla(if selected { t.accent } else { t.border_strong }))
                    .child(
                        div()
                            .h(px(8.0))
                            .w(px(64.0))
                            .rounded_full()
                            .bg(hsla(preview.text)),
                    )
                    .child(
                        div()
                            .h(px(6.0))
                            .w(px(112.0))
                            .rounded_full()
                            .bg(hsla(preview.text_muted)),
                    )
                    .child(
                        div()
                            .h(px(6.0))
                            .w(px(84.0))
                            .rounded_full()
                            .bg(hsla(preview.accent)),
                    ),
            )
            .child(
                div()
                    .text_color(hsla(if selected { t.text } else { t.text_muted }))
                    .child(label),
            )
    }

    /// Row of key caps for a shortcut written as space-separated keys.
    pub(super) fn key_caps(&self, shortcut: &'static str) -> impl IntoElement {
        let t = &self.theme;
        div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .children(shortcut.split(' ').map(|key| {
                div()
                    .min_w(px(22.0))
                    .h(px(22.0))
                    .px(px(6.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(tokens::radius::SM))
                    .bg(hsla(t.bg_elevated))
                    .border_1()
                    .border_color(hsla(t.border_strong))
                    .text_size(px(tokens::text::XS))
                    .text_color(hsla(t.text))
                    .child(SharedString::from(key))
            }))
    }

    pub(super) fn settings_general(&self, cx: &mut Context<'_, Self>) -> Vec<gpui::AnyElement> {
        let default_shell = self.local_spawn.profile.name.clone();
        let shells: Vec<(SharedString, bool, ClickHandler)> = self
            .shells
            .iter()
            .enumerate()
            .map(|(index, shell)| {
                let view = cx.entity();
                let handler: ClickHandler = Box::new(move |_, _, cx| {
                    view.update(cx, |this, cx| this.set_default_shell(index, cx));
                });
                (
                    SharedString::from(shell.name.clone()),
                    shell.name == default_shell,
                    handler,
                )
            })
            .collect();
        vec![
            self.settings_section_title("LAYOUT").into_any_element(),
            self.settings_card(vec![
                self.setting_row(
                    "Sidebar",
                    "Workspaces, SSH hosts and project actions.",
                    self.toggle_switch(
                        "set-sidebar",
                        self.show_sidebar,
                        cx.listener(|this, _, _, cx| this.toggle_sidebar(cx)),
                    ),
                ),
                self.setting_row(
                    "Open sidebar on hover",
                    "While the sidebar is hidden, move the pointer to the left edge to peek at it.",
                    self.toggle_switch(
                        "set-sidebar-hover",
                        self.sidebar_hover,
                        cx.listener(|this, _, _, cx| {
                            this.sidebar_hover = !this.sidebar_hover;
                            this.sidebar_peek = false;
                            cx.notify();
                        }),
                    ),
                ),
                self.setting_row(
                    "Tab strip",
                    "Also show workspaces as horizontal tabs above the terminal.",
                    self.toggle_switch(
                        "set-tabstrip",
                        self.show_tab_strip,
                        cx.listener(|this, _, _, cx| {
                            this.show_tab_strip = !this.show_tab_strip;
                            cx.notify();
                        }),
                    ),
                ),
                self.setting_row(
                    "Status bar",
                    "Command count, last exit code, program status and grid size.",
                    self.toggle_switch(
                        "set-statusbar",
                        self.show_status_bar,
                        cx.listener(|this, _, _, cx| {
                            this.show_status_bar = !this.show_status_bar;
                            cx.notify();
                        }),
                    ),
                ),
            ]),
            self.settings_section_title("SHELL").into_any_element(),
            self.settings_card(vec![self.setting_row(
                "Default shell",
                "Used by New terminal and Ctrl Shift T. Pick another from the arrow beside New terminal.",
                self.segmented("set-shell", shells),
            )]),
        ]
    }

    pub(super) fn settings_appearance(&self, cx: &mut Context<'_, Self>) -> Vec<gpui::AnyElement> {
        let t = &self.theme;
        let dark = self.theme.is_dark;
        let size = self.font_size;
        let width = self.sidebar_width;
        let widths: Vec<(SharedString, bool, ClickHandler)> = SIDEBAR_WIDTHS
            .iter()
            .map(|(w, label)| {
                let w = *w;
                let view = cx.entity();
                let handler: ClickHandler = Box::new(move |_, _, cx| {
                    view.update(cx, |this, cx| {
                        this.sidebar_width = w;
                        cx.notify();
                    });
                });
                (SharedString::from(*label), (width - w).abs() < 1.0, handler)
            })
            .collect();
        vec![
            self.settings_section_title("THEME").into_any_element(),
            div()
                .flex()
                .gap(px(tokens::space::LG))
                .child(self.theme_card(
                    "set-theme-dark",
                    "Dark",
                    ThemeInput::DARK,
                    dark,
                    cx.listener(|this, _, _, cx| this.set_dark_theme(true, cx)),
                ))
                .child(self.theme_card(
                    "set-theme-light",
                    "Light",
                    ThemeInput::LIGHT,
                    !dark,
                    cx.listener(|this, _, _, cx| this.set_dark_theme(false, cx)),
                ))
                .into_any_element(),
            self.settings_section_title("TERMINAL TEXT")
                .into_any_element(),
            self.settings_card(vec![
                self.setting_row(
                    "Font size",
                    "Ctrl +, Ctrl - and Ctrl 0 also work inside the terminal.",
                    div()
                        .flex()
                        .items_center()
                        .gap(px(tokens::space::SM))
                        .child(self.button(
                            "set-font-dec",
                            "−",
                            cx.listener(|this, _, _, cx| {
                                this.font_size = (this.font_size - 1.0).max(8.0);
                                cx.notify();
                            }),
                        ))
                        .child(
                            div()
                                .w(px(44.0))
                                .flex()
                                .justify_center()
                                .text_color(hsla(t.text))
                                .child(SharedString::from(format!("{size:.0} pt"))),
                        )
                        .child(self.button(
                            "set-font-inc",
                            "+",
                            cx.listener(|this, _, _, cx| {
                                this.font_size = (this.font_size + 1.0).min(32.0);
                                cx.notify();
                            }),
                        ))
                        .child(self.button(
                            "set-font-reset",
                            "Reset",
                            cx.listener(|this, _, _, cx| {
                                this.font_size = 14.0;
                                cx.notify();
                            }),
                        )),
                ),
                div()
                    .m(px(tokens::space::MD))
                    .px(px(tokens::space::MD))
                    .py(px(tokens::space::SM))
                    .rounded(px(tokens::radius::MD))
                    .bg(hsla(t.term_bg))
                    .border_1()
                    .border_color(hsla(t.border))
                    .font_family(self.font.family.clone())
                    .text_size(px(size))
                    .text_color(hsla(t.text))
                    .child(SharedString::from("❯ cargo test --workspace  # 0123456789"))
                    .into_any_element(),
            ]),
            self.settings_section_title("SIDEBAR").into_any_element(),
            self.settings_card(vec![self.setting_row(
                "Sidebar width",
                "",
                self.segmented("set-width", widths),
            )]),
        ]
    }

    pub(super) fn settings_keybindings(&self) -> Vec<gpui::AnyElement> {
        let groups: &[(&'static str, &[(&'static str, &'static str)])] = &[
            (
                "TABS",
                &[
                    ("New terminal", "Ctrl Shift T"),
                    ("Close tab", "Ctrl Shift W"),
                    ("Next tab", "Ctrl Tab"),
                    ("Previous tab", "Ctrl Shift Tab"),
                    ("Jump to tab 1–9", "Ctrl 1…9"),
                ],
            ),
            (
                "PANES",
                &[
                    ("Split right", "Ctrl Shift D"),
                    ("Split down", "Ctrl Shift E"),
                    ("Close pane", "Ctrl Shift X"),
                    ("Move focus between panes", "Ctrl Alt ← → ↑ ↓"),
                    ("Resize focused pane", "Ctrl Alt Shift ← → ↑ ↓"),
                ],
            ),
            (
                "WORKSPACES",
                &[
                    ("Toggle sidebar", "Ctrl Shift B"),
                    ("Rename workspace", "Ctrl Shift R"),
                    ("Pin or unpin workspace", "Ctrl Shift K"),
                    ("Move workspace up or down", "Ctrl Shift ↑ ↓"),
                    ("Jump to workspace needing attention", "Ctrl Shift U"),
                ],
            ),
            (
                "TERMINAL",
                &[
                    ("Command palette", "Ctrl Shift P"),
                    ("Find", "Ctrl Shift F"),
                    ("Copy", "Ctrl Shift C"),
                    ("Paste", "Ctrl Shift V"),
                    ("Increase, decrease, reset font", "Ctrl + − 0"),
                    ("Scroll to top or bottom", "Ctrl Shift Home End"),
                ],
            ),
            (
                "APPLICATION",
                &[("Open settings", "Ctrl ,"), ("Quit", "Ctrl Shift Q")],
            ),
        ];
        let t = &self.theme;
        let mut out = Vec::new();
        for (title, items) in groups {
            out.push(self.settings_section_title(title).into_any_element());
            out.push(
                self.settings_card(
                    items
                        .iter()
                        .map(|(label, shortcut)| {
                            div()
                                .px(px(tokens::space::MD))
                                .h(px(40.0))
                                .flex()
                                .items_center()
                                .justify_between()
                                .text_size(px(tokens::text::MD))
                                .text_color(hsla(t.text))
                                .child(*label)
                                .child(self.key_caps(shortcut))
                                .into_any_element()
                        })
                        .collect(),
                ),
            );
        }
        out
    }

    pub(super) fn settings_advanced(&self, cx: &mut Context<'_, Self>) -> Vec<gpui::AnyElement> {
        let t = &self.theme;
        let info = format!(
            "TermForge {} · protocol v{} · {}",
            env!("CARGO_PKG_VERSION"),
            tf_proto::PROTOCOL_VERSION,
            self.spawn.profile.name
        );
        vec![
            self.settings_section_title("DIAGNOSTICS")
                .into_any_element(),
            self.settings_card(vec![
                self.setting_row(
                    "Build",
                    "",
                    div()
                        .text_size(px(tokens::text::SM))
                        .text_color(hsla(t.text_muted))
                        .child(SharedString::from(info)),
                ),
                self.setting_row(
                    "Copy diagnostics",
                    "Version, protocol, shell and working directory to the clipboard.",
                    self.button(
                        "set-copy-diag",
                        "Copy",
                        cx.listener(|this, _, _, cx| {
                            this.run_palette_action(PaletteAction::CopyDiagnostics, cx);
                        }),
                    ),
                ),
                self.setting_row(
                    "Daemon doctor",
                    "Runs `forged doctor` in the active terminal.",
                    self.button(
                        "set-doctor",
                        "Run",
                        cx.listener(|this, _, _, cx| {
                            this.settings = None;
                            this.run_palette_action(PaletteAction::DaemonDoctor, cx);
                        }),
                    ),
                ),
                self.setting_row(
                    "Restart session",
                    "Restarts the shell in the active workspace.",
                    self.button(
                        "set-restart",
                        "Restart",
                        cx.listener(|this, _, _, cx| {
                            this.settings = None;
                            this.start(cx);
                        }),
                    ),
                ),
            ]),
            div()
                .mt(px(tokens::space::LG))
                .px(px(2.0))
                .text_size(px(tokens::text::SM))
                .text_color(hsla(t.text_faint))
                .child("Settings apply to this window only and are not saved yet.")
                .into_any_element(),
        ]
    }

    pub(super) fn settings_panel(&self, cx: &mut Context<'_, Self>) -> Option<impl IntoElement> {
        let page = self.settings?;
        let t = &self.theme;
        let (title, subtitle) = match page {
            SettingsPage::General => ("General", "Layout and the shell new terminals start with."),
            SettingsPage::Appearance => ("Appearance", "Theme, terminal text and sidebar size."),
            SettingsPage::Keybindings => {
                ("Keybindings", "Keyboard shortcuts available in TermForge.")
            }
            SettingsPage::Advanced => ("Advanced", "Diagnostics and session controls."),
        };
        let body = match page {
            SettingsPage::General => self.settings_general(cx),
            SettingsPage::Appearance => self.settings_appearance(cx),
            SettingsPage::Keybindings => self.settings_keybindings(),
            SettingsPage::Advanced => self.settings_advanced(cx),
        };
        let nav = SETTINGS_PAGES
            .iter()
            .enumerate()
            .map(|(i, (target, label))| {
                let target = *target;
                let id: &'static str = ["set-nav-0", "set-nav-1", "set-nav-2", "set-nav-3"][i];
                div()
                    .id(id)
                    .h(px(34.0))
                    .px(px(tokens::space::MD))
                    .flex()
                    .items_center()
                    .rounded(px(tokens::radius::MD))
                    .text_size(px(tokens::text::MD))
                    .cursor_pointer()
                    .when(target == page, |d| {
                        d.bg(hsla(t.bg_selected)).text_color(hsla(t.text))
                    })
                    .when(target != page, |d| {
                        d.text_color(hsla(t.text_muted))
                            .hover(|style| style.bg(hsla(t.bg_hover)).text_color(hsla(t.text)))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings = Some(target);
                        cx.notify();
                    }))
                    .child(*label)
            });
        Some(
            div()
                .id("settings-scrim")
                .absolute()
                .top(px(0.0))
                .bottom(px(0.0))
                .left(px(0.0))
                .right(px(0.0))
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::black().opacity(0.5))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.settings = None;
                    cx.notify();
                }))
                .child(
                    div()
                        .id("settings-panel")
                        .w(px(860.0))
                        .max_w_full()
                        .h(px(580.0))
                        .max_h_full()
                        .flex()
                        .rounded(px(tokens::radius::XL))
                        .bg(hsla(t.bg_app))
                        .border_1()
                        .border_color(hsla(t.border_strong))
                        .shadow_lg()
                        .overflow_hidden()
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .w(px(200.0))
                                .flex_none()
                                .p(px(tokens::space::MD))
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .bg(hsla(t.bg_panel))
                                .border_r_1()
                                .border_color(hsla(t.border))
                                .child(
                                    div()
                                        .px(px(tokens::space::MD))
                                        .pt(px(tokens::space::SM))
                                        .pb(px(tokens::space::MD))
                                        .text_size(px(tokens::text::LG))
                                        .text_color(hsla(t.text))
                                        .child("Settings"),
                                )
                                .children(nav),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .px(px(tokens::space::XL))
                                        .pt(px(tokens::space::LG))
                                        .pb(px(tokens::space::MD))
                                        .flex()
                                        .items_start()
                                        .justify_between()
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap(px(2.0))
                                                .child(
                                                    div()
                                                        .text_size(px(tokens::text::XL))
                                                        .text_color(hsla(t.text))
                                                        .child(title),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(tokens::text::SM))
                                                        .text_color(hsla(t.text_faint))
                                                        .child(subtitle),
                                                ),
                                        )
                                        .child(self.icon_button(
                                            "settings-close",
                                            "×",
                                            cx.listener(|this, _, _, cx| {
                                                this.settings = None;
                                                cx.notify();
                                            }),
                                        )),
                                )
                                .child(
                                    div()
                                        .id("settings-body")
                                        .flex_1()
                                        .overflow_y_scroll()
                                        .px(px(tokens::space::XL))
                                        .pb(px(tokens::space::XL))
                                        .flex()
                                        .flex_col()
                                        .text_size(px(tokens::text::MD))
                                        .text_color(hsla(t.text_muted))
                                        .children(body),
                                ),
                        ),
                ),
        )
    }
}
