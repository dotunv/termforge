//! Applying and persisting appearance choices: themes, fonts, cursor.

use super::*;

impl TerminalView {
    pub(super) fn active_context(&self) -> Option<&ContextConfig> {
        let id = self.context_id.as_deref()?;
        self.appearance
            .config
            .contexts
            .iter()
            .find(|context| context.id == id)
    }

    /// Select a client/account identity for this project. Running processes
    /// retain their environment; every new or restarted local shell gets the
    /// selected identity.
    pub(super) fn set_context(&mut self, id: &str, cx: &mut Context<'_, Self>) {
        if !self
            .appearance
            .config
            .contexts
            .iter()
            .any(|context| context.id == id)
        {
            return;
        }
        let Some(root) = self.local_project_root.clone() else {
            self.error = Some("Client contexts require a local project".into());
            cx.notify();
            return;
        };
        if let Err(error) = remote_session::set_project_context(root, Some(id.to_owned())) {
            self.error = Some(format!("Could not save project context: {error:#}"));
            cx.notify();
            return;
        }
        self.context_id = Some(id.to_owned());
        self.apply_context_to_spawns();
        self.identity_check =
            crate::contexts::inspect(self.local_project_root.as_deref(), self.active_context());
        cx.notify();
    }

    /// Repair selection after config reload (for example a renamed profile).
    pub(super) fn reconcile_context(&mut self) {
        let exists = self.context_id.as_deref().is_some_and(|id| {
            self.appearance
                .config
                .contexts
                .iter()
                .any(|context| context.id == id)
        });
        if !exists {
            self.context_id = self
                .appearance
                .config
                .contexts
                .first()
                .map(|context| context.id.clone());
        }
        self.apply_context_to_spawns();
        self.identity_check =
            crate::contexts::inspect(self.local_project_root.as_deref(), self.active_context());
    }

    fn apply_context_to_spawns(&mut self) {
        let context = self.active_context().cloned();
        crate::contexts::apply_to_env(&mut self.local_spawn.env, context.as_ref());
        if self.spawn.ssh_host.is_none() {
            crate::contexts::apply_to_env(&mut self.spawn.env, context.as_ref());
        }
        for tab in &mut self.tabs {
            if tab.spawn.ssh_host.is_none() {
                crate::contexts::apply_to_env(&mut tab.spawn.env, context.as_ref());
            }
            for pane in tab.panes.values_mut() {
                if pane.spawn.ssh_host.is_none() {
                    crate::contexts::apply_to_env(&mut pane.spawn.env, context.as_ref());
                }
            }
        }
    }

    /// Rebuild the live theme and fonts from the committed config. Also the
    /// way a picker's preview is thrown away.
    pub(super) fn apply_appearance(&mut self, cx: &mut Context<'_, Self>) {
        let terminal = &self.appearance.config.terminal;
        self.theme = Arc::new(self.appearance.theme());
        self.font = MonoFont::new(cx, &terminal.font_family, terminal.ligatures);
        self.line_height = terminal.line_height;
        self.cursor_style = terminal.cursor_style;
        self.ui_font = ui_family(
            cx,
            &self.appearance.config.ui.font_family,
            &self.font.family,
        );
        cx.notify();
    }

    /// Change the config, save it and apply the result.
    fn update_config(
        &mut self,
        cx: &mut Context<'_, Self>,
        change: impl FnOnce(&mut tf_config::Config),
    ) {
        change(&mut self.appearance.config);
        self.appearance.save();
        self.apply_appearance(cx);
    }

    pub(super) fn set_theme(&mut self, name: &str, cx: &mut Context<'_, Self>) {
        let dark = self.appearance.theme_named(name).is_dark;
        self.update_config(cx, |config| {
            config.ui.theme_name = name.to_owned();
            // Keeps the fallback sensible if the theme file is later removed.
            config.ui.theme = if dark {
                tf_config::ThemeMode::Dark
            } else {
                tf_config::ThemeMode::Light
            };
        });
    }

    pub(super) fn set_accent(&mut self, hex: &str, cx: &mut Context<'_, Self>) {
        self.update_config(cx, |config| config.ui.accent = hex.to_owned());
    }

    pub(super) fn set_contrast(&mut self, contrast: Option<u8>, cx: &mut Context<'_, Self>) {
        self.update_config(cx, |config| config.ui.contrast = contrast);
    }

    pub(super) fn set_mono_font(&mut self, family: &str, cx: &mut Context<'_, Self>) {
        self.update_config(cx, |config| config.terminal.font_family = family.to_owned());
    }

    pub(super) fn set_ui_font(&mut self, family: &str, cx: &mut Context<'_, Self>) {
        self.update_config(cx, |config| config.ui.font_family = family.to_owned());
    }

    pub(super) fn set_line_height(&mut self, line_height: f32, cx: &mut Context<'_, Self>) {
        self.update_config(cx, |config| config.terminal.line_height = line_height);
    }

    pub(super) fn set_ligatures(&mut self, on: bool, cx: &mut Context<'_, Self>) {
        self.update_config(cx, |config| config.terminal.ligatures = on);
    }

    pub(super) fn set_accent_bold(&mut self, on: bool, cx: &mut Context<'_, Self>) {
        self.update_config(cx, |config| config.terminal.accent_bold = on);
    }

    pub(super) fn set_window_opacity(&mut self, opacity: f32, cx: &mut Context<'_, Self>) {
        let opacity = opacity.clamp(0.5, 1.0);
        self.update_config(cx, |config| config.ui.opacity = opacity);
    }

    pub(super) fn set_cursor_style(
        &mut self,
        style: CursorStyleConfig,
        cx: &mut Context<'_, Self>,
    ) {
        self.update_config(cx, |config| config.terminal.cursor_style = style);
    }

    /// Set the size new and reset terminals use, and apply it here.
    pub(super) fn set_default_font_size(&mut self, size: f32, cx: &mut Context<'_, Self>) {
        let size = size.clamp(8.0, 32.0);
        self.font_size = size;
        self.update_config(cx, |config| config.terminal.font_size = size);
    }

    /// Re-read config, themes and fonts from disk.
    pub(super) fn reload_appearance(&mut self, cx: &mut Context<'_, Self>) {
        self.appearance.reload(cx);
        self.reconcile_context();
        self.apply_appearance(cx);
    }

    pub(super) fn open_picker(&mut self, mode: PickerMode, cx: &mut Context<'_, Self>) {
        let (items, current) = match mode {
            PickerMode::Commands => return,
            PickerMode::Themes => {
                let builtin: Vec<String> = tf_config::theme::builtin_themes()
                    .into_iter()
                    .map(|spec| spec.name.to_ascii_lowercase())
                    .collect();
                let items = self
                    .appearance
                    .themes
                    .themes
                    .iter()
                    .map(|spec| {
                        let kind = if spec.is_dark() { "Dark" } else { "Light" };
                        let yours = !builtin.contains(&spec.name.to_ascii_lowercase());
                        PickerItem {
                            value: spec.name.clone(),
                            label: spec.name.clone(),
                            hint: if yours {
                                format!("{kind} · yours")
                            } else {
                                kind.to_owned()
                            },
                        }
                    })
                    .collect();
                (items, self.appearance.theme_name())
            }
            PickerMode::MonoFonts | PickerMode::UiFonts => {
                let mono = mode == PickerMode::MonoFonts;
                let families = if mono {
                    self.appearance.mono_families(cx)
                } else {
                    crate::fonts::all_families(cx)
                };
                let mut items = vec![PickerItem {
                    value: String::new(),
                    label: "Automatic".into(),
                    hint: "best installed face".into(),
                }];
                items.extend(families.into_iter().map(|family| PickerItem {
                    value: family.clone(),
                    label: family,
                    hint: String::new(),
                }));
                let current = if mono {
                    self.appearance.config.terminal.font_family.clone()
                } else {
                    self.appearance.config.ui.font_family.clone()
                };
                (items, current)
            }
            PickerMode::Contexts => {
                let items = self
                    .appearance
                    .config
                    .contexts
                    .iter()
                    .map(|context| PickerItem {
                        value: context.id.clone(),
                        label: context.name.clone(),
                        hint: match (&context.git_email, &context.github_host) {
                            (email, host) if !email.is_empty() && !host.is_empty() => {
                                format!("{email} · {host}")
                            }
                            (email, _) if !email.is_empty() => email.clone(),
                            (_, host) => host.clone(),
                        },
                    })
                    .collect();
                (items, self.context_id.clone().unwrap_or_default())
            }
        };
        let selected = items
            .iter()
            .position(|item| item.value.eq_ignore_ascii_case(&current))
            .unwrap_or(0);
        self.palette = Some(PaletteState {
            query: String::new(),
            selected,
            mode,
            items,
        });
        cx.notify();
    }

    /// Show the highlighted picker entry without saving it.
    pub(super) fn preview_picker(&mut self, cx: &mut Context<'_, Self>) {
        let Some(palette) = &self.palette else { return };
        if palette.mode == PickerMode::Commands {
            return;
        }
        let entries = self.filtered_palette_commands();
        let Some((action, _, _)) =
            entries.get(palette.selected.min(entries.len().saturating_sub(1)))
        else {
            return;
        };
        match action {
            PaletteAction::SetTheme(name) => {
                self.theme = Arc::new(self.appearance.theme_named(name));
            }
            PaletteAction::SetMonoFont(family) => {
                let ligatures = self.appearance.config.terminal.ligatures;
                self.font = MonoFont::new(cx, family, ligatures);
            }
            PaletteAction::SetUiFont(family) => {
                self.ui_font = ui_family(cx, family, &self.font.family);
            }
            _ => {}
        }
        cx.notify();
    }
}

impl TerminalView {
    /// Save the current theme as a file the user can edit and share. The
    /// reload then lists it as a custom theme.
    pub(super) fn export_theme(&mut self, cx: &mut Context<'_, Self>) {
        match self.appearance.export_theme() {
            Ok(path) => {
                tracing::info!(path = %path.display(), "theme exported");
                self.appearance.save_error = None;
                self.reload_appearance(cx);
            }
            Err(e) => {
                self.appearance.save_error = Some(format!("Could not export theme: {e}"));
                cx.notify();
            }
        }
    }

    /// Show the themes or fonts folder in the system file manager, creating
    /// it first so there is something to open.
    pub(super) fn reveal_dir(&mut self, themes: bool, cx: &mut Context<'_, Self>) {
        let dir = if themes {
            self.appearance.themes_dir.clone()
        } else {
            self.appearance.fonts_dir.clone()
        };
        let Some(dir) = dir else { return };
        match std::fs::create_dir_all(&dir) {
            Ok(()) => cx.reveal_path(&dir),
            Err(e) => {
                self.appearance.save_error = Some(format!("{}: {e}", dir.display()));
                cx.notify();
            }
        }
    }
}
