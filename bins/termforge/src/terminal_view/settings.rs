//! Settings panel.

use super::*;

impl TerminalView {
    pub(super) fn diagnostics_text(&self) -> String {
        let hosted = self
            .session
            .as_ref()
            .and_then(|session| session.id())
            .map(|id| format!("hosted ({id})"))
            .unwrap_or_else(|| "embedded/unavailable".into());
        let fallbacks = if self.font.fallback_names().is_empty() {
            "none installed".to_owned()
        } else {
            self.font.fallback_names().join(", ")
        };
        format!(
            "TermForge {}\nprotocol: v{}\nplatform: {}/{}\nsession: {}\nshell: {}\nshell integration: {}\nssh host: {}\ncontext: {}\ngit email: {}\norigin host: {}\nidentity warnings: {}\nterminal font: {}\nfont fallbacks: {}\nupdate verification key: {}\ncwd: {}",
            env!("CARGO_PKG_VERSION"),
            tf_proto::PROTOCOL_VERSION,
            std::env::consts::OS,
            std::env::consts::ARCH,
            hosted,
            self.spawn.profile.name,
            if self.spawn.ssh_host.is_some() {
                "managed by remote profile"
            } else if self.spawn.profile.supports_integration() {
                "automatic"
            } else {
                "not available for this profile"
            },
            self.spawn.ssh_host.as_deref().unwrap_or("local"),
            self.active_context().map_or("none", |context| context.name.as_str()),
            self.identity_check.actual_email.as_deref().unwrap_or("unknown"),
            self.identity_check.origin_host.as_deref().unwrap_or("unknown"),
            if self.identity_check.warnings.is_empty() {
                "none".into()
            } else {
                self.identity_check.warnings.join("; ")
            },
            self.font.family,
            fallbacks,
            if option_env!("TERMFORGE_UPDATE_PUBLIC_KEY").is_some() {
                "embedded"
            } else {
                "missing (checks disabled)"
            },
            self.cwd.as_deref().unwrap_or("unknown"),
        )
    }

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
        description: impl Into<SharedString>,
        control: impl IntoElement,
    ) -> gpui::AnyElement {
        let t = &self.theme;
        let description: SharedString = description.into();
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
        id: SharedString,
        label: SharedString,
        preview: &Theme,
        selected: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let t = &self.theme;
        div()
            .id(id)
            .flex()
            .flex_col()
            .gap(px(tokens::space::SM))
            .cursor_pointer()
            .on_click(on_click)
            .child(
                div()
                    .w(px(138.0))
                    .h(px(84.0))
                    .p(px(tokens::space::MD))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .rounded(px(tokens::radius::LG))
                    .bg(hsla(preview.term_bg))
                    .border_2()
                    .border_color(hsla(if selected { t.accent } else { t.border_strong }))
                    .child(
                        div()
                            .h(px(7.0))
                            .w(px(56.0))
                            .rounded_full()
                            .bg(hsla(preview.term_fg)),
                    )
                    .child(
                        div()
                            .h(px(5.0))
                            .w(px(92.0))
                            .rounded_full()
                            .bg(hsla(preview.text_muted)),
                    )
                    .child(
                        div()
                            .h(px(5.0))
                            .w(px(70.0))
                            .rounded_full()
                            .bg(hsla(preview.accent)),
                    )
                    .child(
                        div().flex().gap(px(3.0)).children(
                            preview.ansi[1..7]
                                .iter()
                                .map(|c| div().size(px(10.0)).rounded(px(2.0)).bg(hsla(*c))),
                        ),
                    ),
            )
            .child(
                div()
                    .text_size(px(tokens::text::SM))
                    .text_color(hsla(if selected { t.text } else { t.text_muted }))
                    .child(label),
            )
    }

    /// An option for [`Self::segmented`] that runs `act` on the view.
    fn choice(
        cx: &mut Context<'_, Self>,
        label: impl Into<SharedString>,
        selected: bool,
        act: impl Fn(&mut Self, &mut Context<'_, Self>) + 'static,
    ) -> (SharedString, bool, ClickHandler) {
        let view = cx.entity();
        let handler: ClickHandler = Box::new(move |_, _, cx| {
            view.update(cx, |this, cx| act(this, cx));
        });
        (label.into(), selected, handler)
    }

    fn accent_swatch(
        &self,
        index: usize,
        hex: &'static str,
        selected: bool,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        let colour = tf_ui::Rgb::from_hex(hex);
        div()
            .id(SharedString::from(format!("set-accent-{index}")))
            .size(px(24.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .cursor_pointer()
            .border_2()
            .border_color(hsla(if selected { t.text } else { t.border_strong }))
            .when_some(colour, |d, c| d.bg(hsla(c)))
            .when(colour.is_none(), |d| {
                // "Theme" entry: no colour of its own, so show a slash.
                d.bg(hsla(t.bg_elevated))
                    .text_size(px(tokens::text::XS))
                    .text_color(hsla(t.text_muted))
                    .child("∅")
            })
            .on_click(cx.listener(move |this, _, _, cx| this.set_accent(hex, cx)))
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
        let active_context = self.active_context();
        let context_name = active_context.map_or("No context", |context| context.name.as_str());
        let identity = active_context.map_or_else(
            || "No client/account identity is configured.".to_owned(),
            |context| {
                let email = if context.git_email.is_empty() {
                    "Git email not constrained"
                } else {
                    context.git_email.as_str()
                };
                let host = if context.github_host.is_empty() {
                    "any host"
                } else {
                    context.github_host.as_str()
                };
                format!("{email} · {host}. Used for new terminals.")
            },
        );
        let mut context_rows = vec![self.setting_row(
            "Account",
            identity,
            div()
                .flex()
                .gap(px(tokens::space::SM))
                .child(self.button(
                    "set-context-pick",
                    context_name.to_owned(),
                    cx.listener(|this, _, _, cx| this.open_picker(PickerMode::Contexts, cx)),
                ))
                .child(self.button(
                    "set-context-edit",
                    "Manage accounts",
                    cx.listener(|this, _, _, cx| {
                        this.settings = Some(SettingsPage::Accounts);
                        cx.notify();
                    }),
                )),
        )];
        context_rows.extend(self.identity_check.warnings.iter().map(|warning| {
            div()
                .px(px(tokens::space::MD))
                .py(px(tokens::space::SM))
                .text_size(px(tokens::text::SM))
                .text_color(hsla(self.theme.warning))
                .child(SharedString::from(format!("Identity mismatch: {warning}")))
                .into_any_element()
        }));
        vec![
            self.settings_section_title("CLIENT / ACCOUNT").into_any_element(),
            self.settings_card(context_rows),
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
        let size = self.font_size;
        let width = self.sidebar_width;
        let config = &self.appearance.config;
        let widths: Vec<(SharedString, bool, ClickHandler)> = SIDEBAR_WIDTHS
            .iter()
            .map(|(w, label)| {
                let w = *w;
                Self::choice(cx, *label, (width - w).abs() < 1.0, move |this, cx| {
                    this.sidebar_width = w;
                    cx.notify();
                })
            })
            .collect();
        let mut out: Vec<gpui::AnyElement> = Vec::new();

        let notice = |text: String, colour: tf_ui::Rgb| {
            div()
                .mt(px(tokens::space::MD))
                .px(px(2.0))
                .text_size(px(tokens::text::SM))
                .text_color(hsla(colour))
                .child(SharedString::from(text))
                .into_any_element()
        };
        if let Some(error) = &self.appearance.config_error {
            out.push(notice(
                format!(
                    "config.toml could not be read ({error}). Changes are not saved until it is fixed; edit it and use Reload."
                ),
                t.danger,
            ));
        }
        if let Some(error) = &self.appearance.save_error {
            out.push(notice(
                format!("Could not save settings: {error}"),
                t.danger,
            ));
        }

        // THEME
        let current = self.appearance.theme_name();
        let cards: Vec<gpui::AnyElement> = self
            .appearance
            .themes
            .themes
            .iter()
            .enumerate()
            .map(|(index, spec)| {
                let name = spec.name.clone();
                let preview = self.appearance.theme_named(&name);
                self.theme_card(
                    SharedString::from(format!("set-theme-{index}")),
                    SharedString::from(name.clone()),
                    &preview,
                    name.eq_ignore_ascii_case(&current),
                    cx.listener(move |this, _, _, cx| this.set_theme(&name, cx)),
                )
                .into_any_element()
            })
            .collect();
        out.push(self.settings_section_title("THEME").into_any_element());
        out.push(
            div()
                .flex()
                .flex_wrap()
                .gap(px(tokens::space::LG))
                .children(cards)
                .into_any_element(),
        );
        let issues = self.theme.contrast_issues();
        if !issues.is_empty() {
            let shown: Vec<String> = issues
                .iter()
                .take(3)
                .map(|i| format!("{} {:.1}:1 (needs {:.1})", i.name, i.ratio, i.required))
                .collect();
            out.push(notice(
                format!(
                    "This theme is below contrast targets: {}{}.",
                    shown.join(", "),
                    if issues.len() > 3 { ", …" } else { "" }
                ),
                t.warning,
            ));
        }
        let themes_dir = self
            .appearance
            .themes_dir
            .as_ref()
            .map(|d| d.display().to_string())
            .unwrap_or_else(|| "unavailable".into());
        let mut theme_rows = vec![
            self.setting_row(
                "Custom themes",
                format!("Drop .toml theme files (TermForge or Alacritty format) in {themes_dir}; changes apply automatically."),
                div()
                    .flex()
                    .gap(px(tokens::space::SM))
                    .child(self.button(
                        "set-themes-open",
                        "Open folder",
                        cx.listener(|this, _, _, cx| this.reveal_dir(true, cx)),
                    ))
                    .child(self.button(
                        "set-themes-reload",
                        "Reload",
                        cx.listener(|this, _, _, cx| this.reload_appearance(cx)),
                    )),
            ),
            self.setting_row(
                "Save current theme",
                "Writes the selected theme, with your accent and contrast, to the themes folder to edit or share.",
                self.button(
                    "set-theme-export",
                    "Save",
                    cx.listener(|this, _, _, cx| this.export_theme(cx)),
                ),
            ),
            self.setting_row(
                "Pick from list",
                "Search all themes with a live preview.",
                self.button(
                    "set-theme-pick",
                    "Choose…",
                    cx.listener(|this, _, _, cx| this.open_picker(PickerMode::Themes, cx)),
                ),
            ),
        ];
        for error in &self.appearance.themes.errors {
            theme_rows.push(
                div()
                    .px(px(tokens::space::MD))
                    .py(px(tokens::space::SM))
                    .text_size(px(tokens::text::SM))
                    .text_color(hsla(t.warning))
                    .child(SharedString::from(error.clone()))
                    .into_any_element(),
            );
        }
        out.push(div().h(px(tokens::space::MD)).into_any_element());
        out.push(self.settings_card(theme_rows));

        // ACCENT AND CONTRAST
        let accent = config.ui.accent.to_ascii_lowercase();
        let contrast = config.ui.contrast;
        let contrasts: Vec<(SharedString, bool, ClickHandler)> = [
            ("Theme", None),
            ("Soft", Some(15)),
            ("Balanced", Some(30)),
            ("High", Some(60)),
            ("Maximum", Some(100)),
        ]
        .into_iter()
        .map(|(label, value)| {
            Self::choice(cx, label, contrast == value, move |this, cx| {
                this.set_contrast(value, cx);
            })
        })
        .collect();
        out.push(self.settings_section_title("COLOUR").into_any_element());
        out.push(self.settings_card(vec![
            self.setting_row(
                "Accent",
                "Overrides the theme's accent. The first swatch uses the theme's own.",
                div()
                    .flex()
                    .gap(px(6.0))
                    .children(ACCENTS.iter().enumerate().map(|(index, hex)| {
                        self.accent_swatch(index, hex, accent == *hex, cx)
                    })),
            ),
            self.setting_row(
                "Contrast",
                "Separation between surfaces and text. Colours a theme sets itself are kept.",
                self.segmented("set-contrast", contrasts),
            ),
        ]));

        // FONTS
        let font_name = |configured: &str, shown: &str| -> String {
            if configured.is_empty() {
                format!("Automatic ({shown})")
            } else {
                configured.to_owned()
            }
        };
        let mono_label = if self.font.requested_missing {
            format!("{} (not installed)", config.terminal.font_family)
        } else {
            font_name(&config.terminal.font_family, &self.font.family)
        };
        let ui_label = font_name(&config.ui.font_family, &self.ui_font);
        let fonts_dir = self
            .appearance
            .fonts_dir
            .as_ref()
            .map(|d| d.display().to_string())
            .unwrap_or_else(|| "unavailable".into());
        let loaded = self.appearance.fonts.loaded.len();
        let mut font_rows = vec![
            self.setting_row(
                "Terminal font",
                "Monospaced families only. Fonts in your fonts folder appear here too.",
                self.button(
                    "set-font-mono",
                    mono_label,
                    cx.listener(|this, _, _, cx| this.open_picker(PickerMode::MonoFonts, cx)),
                ),
            ),
            self.setting_row(
                "Interface font",
                "Sidebar, tabs, settings and the status bar.",
                self.button(
                    "set-font-ui",
                    ui_label,
                    cx.listener(|this, _, _, cx| this.open_picker(PickerMode::UiFonts, cx)),
                ),
            ),
            self.setting_row(
                "Custom fonts",
                format!(
                    "{loaded} loaded from {fonts_dir}. New .ttf, .otf or .ttc files are picked up automatically. Replacing a loaded file needs a restart."
                ),
                div()
                    .flex()
                    .gap(px(tokens::space::SM))
                    .child(self.button(
                        "set-fonts-open",
                        "Open folder",
                        cx.listener(|this, _, _, cx| this.reveal_dir(false, cx)),
                    ))
                    .child(self.button(
                        "set-fonts-reload",
                        "Reload",
                        cx.listener(|this, _, _, cx| this.reload_appearance(cx)),
                    )),
            ),
        ];
        for error in &self.appearance.fonts.errors {
            font_rows.push(
                div()
                    .px(px(tokens::space::MD))
                    .py(px(tokens::space::SM))
                    .text_size(px(tokens::text::SM))
                    .text_color(hsla(t.warning))
                    .child(SharedString::from(error.clone()))
                    .into_any_element(),
            );
        }
        out.push(self.settings_section_title("FONTS").into_any_element());
        out.push(self.settings_card(font_rows));

        // TERMINAL TEXT
        let line_height = config.terminal.line_height;
        let heights: Vec<(SharedString, bool, ClickHandler)> =
            [("Compact", 1.15f32), ("Normal", 1.35), ("Relaxed", 1.55)]
                .into_iter()
                .map(|(label, value)| {
                    Self::choice(
                        cx,
                        label,
                        (line_height - value).abs() < 0.01,
                        move |this, cx| this.set_line_height(value, cx),
                    )
                })
                .collect();
        let cursor = config.terminal.cursor_style;
        let cursors: Vec<(SharedString, bool, ClickHandler)> = [
            ("Block", CursorStyleConfig::Block),
            ("Bar", CursorStyleConfig::Bar),
            ("Underline", CursorStyleConfig::Underline),
        ]
        .into_iter()
        .map(|(label, value)| {
            Self::choice(cx, label, cursor == value, move |this, cx| {
                this.set_cursor_style(value, cx);
            })
        })
        .collect();
        let opacity = config.ui.opacity;
        let opacities: Vec<(SharedString, bool, ClickHandler)> = [
            ("100%", 1.0f32),
            ("95%", 0.95),
            ("90%", 0.90),
            ("80%", 0.80),
        ]
        .into_iter()
        .map(|(label, value)| {
            Self::choice(
                cx,
                label,
                (opacity - value).abs() < 0.01,
                move |this, cx| this.set_window_opacity(value, cx),
            )
        })
        .collect();
        out.push(
            self.settings_section_title("TERMINAL TEXT")
                .into_any_element(),
        );
        out.push(self.settings_card(vec![
            self.setting_row(
                "Font size",
                "Ctrl +, Ctrl - and Ctrl 0 also work inside the terminal; 0 returns to this size.",
                div()
                    .flex()
                    .items_center()
                    .gap(px(tokens::space::SM))
                    .child(self.button(
                        "set-font-dec",
                        "−",
                        cx.listener(|this, _, _, cx| {
                            let size = this.font_size - 1.0;
                            this.set_default_font_size(size, cx);
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
                            let size = this.font_size + 1.0;
                            this.set_default_font_size(size, cx);
                        }),
                    ))
                    .child(self.button(
                        "set-font-reset",
                        "Reset",
                        cx.listener(|this, _, _, cx| this.set_default_font_size(14.0, cx)),
                    )),
            ),
            self.setting_row(
                "Line height",
                "",
                self.segmented("set-line-height", heights),
            ),
            self.setting_row(
                "Ligatures",
                "Join sequences such as -> and != when the font has ligatures.",
                self.toggle_switch(
                    "set-ligatures",
                    config.terminal.ligatures,
                    cx.listener(|this, _, _, cx| {
                        let on = !this.appearance.config.terminal.ligatures;
                        this.set_ligatures(on, cx);
                    }),
                ),
            ),
            self.setting_row(
                "Agent emphasis",
                "Tint bold text with the theme accent when the program did not choose an ANSI colour.",
                self.toggle_switch(
                    "set-accent-bold",
                    config.terminal.accent_bold,
                    cx.listener(|this, _, _, cx| {
                        let on = !this.appearance.config.terminal.accent_bold;
                        this.set_accent_bold(on, cx);
                    }),
                ),
            ),
            self.setting_row("Cursor", "", self.segmented("set-cursor", cursors)),
            self.setting_row(
                "Window opacity",
                "Makes the whole window translucent. Support depends on your desktop compositor.",
                self.segmented("set-opacity", opacities),
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
                .line_height(px((size * self.line_height).round()))
                .text_color(hsla(t.term_fg))
                .child({
                    // Shaped with the terminal's own font so the ligature
                    // setting is visible here exactly as in the grid.
                    let sample = "❯ cargo test --workspace  # 0123456789 -> != =>";
                    gpui::StyledText::new(sample).with_runs(vec![gpui::TextRun {
                        len: sample.len(),
                        font: self.font.regular(),
                        color: hsla(t.term_fg),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }])
                })
                .into_any_element(),
        ]));

        out.push(self.settings_section_title("SIDEBAR").into_any_element());
        out.push(self.settings_card(vec![self.setting_row(
            "Sidebar width",
            "",
            self.segmented("set-width", widths),
        )]));
        out
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
        let session_health = if self
            .session
            .as_ref()
            .and_then(|session| session.id())
            .is_some()
        {
            "Connected to forged; UI restart recovery is active"
        } else {
            "Embedded or unavailable; live-process recovery is inactive"
        };
        let integration = if self.spawn.ssh_host.is_some() {
            "Remote profile must source TermForge integration"
        } else if self.spawn.profile.supports_integration() {
            "Injected automatically"
        } else {
            "Unsupported by this shell profile"
        };
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
                    "Session host",
                    session_health,
                    div().text_color(hsla(t.success)).child(
                        if self
                            .session
                            .as_ref()
                            .and_then(|session| session.id())
                            .is_some()
                        {
                            "Connected"
                        } else {
                            "Limited"
                        },
                    ),
                ),
                self.setting_row(
                    "Shell integration",
                    integration,
                    div()
                        .text_size(px(tokens::text::SM))
                        .text_color(hsla(t.text_muted))
                        .child(self.spawn.profile.name.clone()),
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
                .child("Appearance settings are saved to config.toml. Sidebar, tab strip and status bar choices apply to this window only.")
                .into_any_element(),
        ]
    }

    pub(super) fn settings_panel(&self, cx: &mut Context<'_, Self>) -> Option<impl IntoElement> {
        let page = self.settings?;
        let t = &self.theme;
        let (title, subtitle) = match page {
            SettingsPage::General => ("General", "Layout and the shell new terminals start with."),
            SettingsPage::Appearance => (
                "Appearance",
                "Themes, colours, fonts, cursor and sidebar size.",
            ),
            SettingsPage::Accounts => (
                "Accounts",
                "Choose which Git and GitHub account each project uses.",
            ),
            SettingsPage::Keybindings => {
                ("Keybindings", "Keyboard shortcuts available in TermForge.")
            }
            SettingsPage::Advanced => ("Advanced", "Diagnostics and session controls."),
        };
        let body = match page {
            SettingsPage::General => self.settings_general(cx),
            SettingsPage::Appearance => self.settings_appearance(cx),
            SettingsPage::Accounts => self.settings_accounts(cx),
            SettingsPage::Keybindings => self.settings_keybindings(),
            SettingsPage::Advanced => self.settings_advanced(cx),
        };
        let nav = SETTINGS_PAGES
            .iter()
            .enumerate()
            .map(|(i, (target, label))| {
                let target = *target;
                let id: &'static str = [
                    "set-nav-0",
                    "set-nav-1",
                    "set-nav-2",
                    "set-nav-3",
                    "set-nav-4",
                ][i];
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
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
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
