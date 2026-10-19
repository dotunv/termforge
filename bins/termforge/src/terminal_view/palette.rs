//! Command palette.

use super::*;

/// A command as presented by the palette. Keeping the search metadata beside
/// the rendered text means filters do not have to reverse-engineer action
/// labels at every call site.
#[derive(Debug, Clone)]
struct PaletteCommand {
    action: PaletteAction,
    label: String,
    shortcut: String,
    category: PaletteCategory,
}

impl PaletteCommand {
    fn command(
        action: PaletteAction,
        label: impl Into<String>,
        shortcut: impl Into<String>,
    ) -> Self {
        Self {
            action,
            category: PaletteCategory::Other,
            label: label.into(),
            shortcut: shortcut.into(),
        }
    }

    fn ssh(action: PaletteAction, label: impl Into<String>) -> Self {
        Self {
            action,
            label: label.into(),
            shortcut: String::new(),
            category: PaletteCategory::Ssh,
        }
    }

    fn picker(action: PaletteAction, item: &PickerItem) -> Self {
        Self {
            action,
            label: item.label.clone(),
            shortcut: item.hint.clone(),
            category: PaletteCategory::Other,
        }
    }

    fn matches(&self, filter: &PaletteFilter) -> bool {
        filter
            .category
            .is_none_or(|category| category == self.category)
            && (filter.needle.is_empty()
                || self.label.to_ascii_lowercase().contains(&filter.needle)
                || self.shortcut.to_ascii_lowercase().contains(&filter.needle))
    }

    fn into_tuple(self) -> (PaletteAction, String, String) {
        (self.action, self.label, self.shortcut)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaletteCategory {
    Ssh,
    Other,
}

/// A leading `ssh:` is a deliberate scope selector, rather than an accidental
/// consequence of every SSH command label beginning with the same word.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PaletteFilter {
    category: Option<PaletteCategory>,
    needle: String,
}

impl PaletteFilter {
    fn parse(query: &str) -> Self {
        let query = query.trim().to_ascii_lowercase();
        let (category, needle) = match query.strip_prefix("ssh:") {
            Some(needle) => (Some(PaletteCategory::Ssh), needle.trim().to_owned()),
            None => (None, query),
        };
        Self { category, needle }
    }
}

impl TerminalView {
    pub(super) fn filtered_palette_commands(&self) -> Vec<(PaletteAction, String, String)> {
        let filter = PaletteFilter::parse(
            self.palette
                .as_ref()
                .map(|palette| palette.query.as_str())
                .unwrap_or_default(),
        );
        if let Some(palette) = self
            .palette
            .as_ref()
            .filter(|palette| palette.mode != PickerMode::Commands)
        {
            return palette
                .items
                .iter()
                .map(|item| {
                    let action = match palette.mode {
                        PickerMode::Themes => PaletteAction::SetTheme(item.value.clone()),
                        PickerMode::MonoFonts => PaletteAction::SetMonoFont(item.value.clone()),
                        PickerMode::UiFonts => PaletteAction::SetUiFont(item.value.clone()),
                        PickerMode::Contexts => PaletteAction::SetContext(item.value.clone()),
                        PickerMode::Commands => unreachable!("command mode handled above"),
                    };
                    PaletteCommand::picker(action, item)
                })
                .filter(|command| command.matches(&filter))
                .map(PaletteCommand::into_tuple)
                .collect();
        }
        self.palette_commands()
            .into_iter()
            .filter(|command| command.matches(&filter))
            .map(PaletteCommand::into_tuple)
            .collect()
    }

    fn palette_commands(&self) -> Vec<PaletteCommand> {
        let mut commands = PALETTE_COMMANDS
            .iter()
            .cloned()
            .map(|(action, label, shortcut)| PaletteCommand::command(action, label, shortcut))
            .collect::<Vec<_>>();
        if self.spawn.ssh_host.is_some() {
            commands.push(PaletteCommand::command(
                PaletteAction::SwitchLocal,
                "Workspace: switch to local",
                "",
            ));
            if let Some(host) = self.spawn.ssh_host.clone() {
                commands.push(PaletteCommand::ssh(
                    PaletteAction::CopyRemoteIntegration(host),
                    "SSH: copy Bash integration bootstrap",
                ));
            }
        }
        for (index, shell) in self.shells.iter().enumerate() {
            commands.push(PaletteCommand::command(
                PaletteAction::NewShell(index),
                format!("Shell: new {} terminal", shell.name),
                "",
            ));
            commands.push(PaletteCommand::command(
                PaletteAction::SetDefaultShell(index),
                format!("Shell: use {} for new terminals", shell.name),
                "",
            ));
        }
        let mut ssh_hosts = self.ssh_hosts.clone();
        ssh_hosts.sort_unstable_by_key(|host| host.to_ascii_lowercase());
        ssh_hosts.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
        commands.extend(ssh_hosts.into_iter().map(|host| {
            PaletteCommand::ssh(
                PaletteAction::ConnectSsh(host.clone()),
                format!("SSH: connect to {host}"),
            )
        }));
        commands
    }

    fn normalize_palette_selection(palette: &mut PaletteState, command_count: usize) {
        palette.selected = if command_count == 0 {
            0
        } else {
            palette.selected.min(command_count - 1)
        };
    }

    pub(super) fn palette_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<'_, Self>) -> bool {
        let key = ev.keystroke.key.as_str();
        let modifiers = ev.keystroke.modifiers;
        if modifiers.control && modifiers.shift && key.eq_ignore_ascii_case("p") {
            self.palette = None;
            cx.notify();
            return true;
        }
        let commands = self.filtered_palette_commands();
        let count = commands.len();
        let Some(palette) = &mut self.palette else {
            return false;
        };
        Self::normalize_palette_selection(palette, count);
        let picker = palette.mode != PickerMode::Commands;
        match key {
            "escape" => {
                self.palette = None;
                if picker {
                    // Discard the live preview.
                    self.apply_appearance(cx);
                }
            }
            "up" if count > 0 => palette.selected = (palette.selected + count - 1) % count,
            "down" if count > 0 => palette.selected = (palette.selected + 1) % count,
            "enter" if count > 0 => {
                let action = commands[palette.selected].0.clone();
                self.palette = None;
                self.run_palette_action(action, cx);
            }
            "backspace" => {
                palette.query.pop();
                palette.selected = 0;
            }
            _ if !modifiers.control && !modifiers.alt => {
                if let Some(text) = ev.keystroke.key_char.as_deref() {
                    if !text.chars().any(char::is_control) {
                        palette.query.push_str(text);
                        palette.selected = 0;
                    }
                }
            }
            _ => return false,
        }
        if picker {
            self.preview_picker(cx);
        }
        cx.notify();
        true
    }

    pub(super) fn run_palette_action(&mut self, action: PaletteAction, cx: &mut Context<'_, Self>) {
        match action {
            PaletteAction::NewTab => self.new_local_tab(cx),
            PaletteAction::CloseTab => self.close_active_tab(cx),
            PaletteAction::SplitRight => self.split_pane(Axis::Horizontal, cx),
            PaletteAction::SplitDown => self.split_pane(Axis::Vertical, cx),
            PaletteAction::ClosePane => self.close_pane(cx),
            PaletteAction::NextPane => self.cycle_pane(cx),
            PaletteAction::NextTab => self.cycle_tab(false, cx),
            PaletteAction::PreviousTab => self.cycle_tab(true, cx),
            PaletteAction::RestartSession => self.start(cx),
            PaletteAction::Quit => cx.quit(),
            PaletteAction::Find => self.open_search(cx),
            PaletteAction::Copy => self.copy_selection(cx),
            PaletteAction::Paste => self.paste(cx),
            PaletteAction::FontIncrease => self.font_size = (self.font_size + 1.0).min(32.0),
            PaletteAction::FontDecrease => self.font_size = (self.font_size - 1.0).max(8.0),
            PaletteAction::FontReset => {
                self.font_size = self.appearance.config.terminal.font_size;
            }
            PaletteAction::ScrollTop => self.scroll(Scroll::Top, cx),
            PaletteAction::ScrollBottom => self.scroll(Scroll::Bottom, cx),
            PaletteAction::CopyDiagnostics => {
                cx.write_to_clipboard(ClipboardItem::new_string(self.diagnostics_text()));
            }
            PaletteAction::NextAttention => self.jump_to_attention(cx),
            PaletteAction::RenameWorkspace => self.begin_rename(self.active_tab, cx),
            PaletteAction::TogglePin => self.toggle_pin(self.active_tab, cx),
            PaletteAction::MoveUp => self.move_active_tab(false, cx),
            PaletteAction::MoveDown => self.move_active_tab(true, cx),
            PaletteAction::NewShell(index) => self.new_shell_tab(index, cx),
            PaletteAction::SetDefaultShell(index) => self.set_default_shell(index, cx),
            PaletteAction::ToggleSidebar => self.toggle_sidebar(cx),
            PaletteAction::ToggleContextPanel => self.toggle_context_panel(cx),
            PaletteAction::ToggleUtilityDock => self.toggle_utility_dock(cx),
            PaletteAction::OpenSettings => self.open_settings(cx),
            PaletteAction::NewTask => self.open_task_form("", "", cx),
            PaletteAction::ListTasks => self.write(b"tf task list\r"),
            PaletteAction::StatusWorking => self.write(b"tf status working --app termforge\r"),
            PaletteAction::StatusDone => self.write(b"tf status done --app termforge\r"),
            PaletteAction::StatusClear => self.write(b"tf status clear\r"),
            PaletteAction::DaemonDoctor => self.write(b"forged doctor\r"),
            PaletteAction::ConnectSsh(host) => self.new_ssh_tab(host, cx),
            PaletteAction::CopyRemoteIntegration(host) => {
                let command = format!(
                    "tf shell-integration bash | ssh -- {host} 'umask 077; mkdir -p ~/.config/termforge && cat > ~/.config/termforge/termforge.bash'\n# Then, on {host}: source ~/.config/termforge/termforge.bash"
                );
                cx.write_to_clipboard(ClipboardItem::new_string(command));
            }
            PaletteAction::PickTheme => self.open_picker(PickerMode::Themes, cx),
            PaletteAction::PickMonoFont => self.open_picker(PickerMode::MonoFonts, cx),
            PaletteAction::PickUiFont => self.open_picker(PickerMode::UiFonts, cx),
            PaletteAction::SetTheme(name) => self.set_theme(&name, cx),
            PaletteAction::SetMonoFont(family) => self.set_mono_font(&family, cx),
            PaletteAction::SetUiFont(family) => self.set_ui_font(&family, cx),
            PaletteAction::ReloadAppearance => self.reload_appearance(cx),
            PaletteAction::RevealThemes => self.reveal_dir(true, cx),
            PaletteAction::RevealFonts => self.reveal_dir(false, cx),
            PaletteAction::ExportTheme => self.export_theme(cx),
            PaletteAction::PickContext => self.open_picker(PickerMode::Contexts, cx),
            PaletteAction::SetContext(id) => self.set_context(&id, cx),
            PaletteAction::SwitchLocal => {
                if let Some(index) = self
                    .tabs
                    .iter()
                    .position(|tab| tab.spawn.ssh_host.is_none())
                {
                    self.select_tab(index, cx);
                } else {
                    self.new_local_tab(cx);
                }
            }
        }
        cx.notify();
    }

    pub(super) fn command_palette(&self) -> Option<impl IntoElement> {
        let palette = self.palette.as_ref()?;
        let commands = self.filtered_palette_commands();
        let command_count = commands.len();
        let selected = if command_count == 0 {
            0
        } else {
            palette.selected.min(command_count - 1)
        };
        let visible_start = selected.saturating_sub(7);
        let t = &self.theme;
        let mode = palette.mode;
        let query = if palette.query.is_empty() {
            mode.placeholder().to_owned()
        } else {
            palette.query.clone()
        };
        Some(
            div()
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .right(px(0.0))
                .flex()
                .justify_center()
                .child(
                    div()
                        .mt(px(48.0))
                        .w(px(620.0))
                        .p(px(tokens::space::SM))
                        .rounded(px(tokens::radius::XL))
                        .bg(hsla(t.bg_elevated))
                        .border_1()
                        .border_color(hsla(t.border_strong))
                        .shadow_lg()
                        .child(
                            div()
                                .h(px(30.0))
                                .px(px(tokens::space::SM))
                                .flex()
                                .items_center()
                                .justify_between()
                                .text_size(px(tokens::text::XS))
                                .text_color(hsla(t.text_faint))
                                .child(mode.title())
                                .child(SharedString::from(format!(
                                    "{command_count} {}",
                                    if mode == PickerMode::Commands {
                                        "actions"
                                    } else {
                                        "choices"
                                    }
                                ))),
                        )
                        .child(
                            div()
                                .h(px(42.0))
                                .px(px(tokens::space::MD))
                                .flex()
                                .items_center()
                                .rounded(px(tokens::radius::MD))
                                .bg(hsla(t.bg_app))
                                .text_color(hsla(if palette.query.is_empty() {
                                    t.text_faint
                                } else {
                                    t.text
                                }))
                                .child(">  ")
                                .child(SharedString::from(query)),
                        )
                        .child(div().mt(px(tokens::space::XS)).flex().flex_col().children(
                            commands.iter().enumerate().skip(visible_start).take(9).map(
                                |(index, (_, label, shortcut))| {
                                    div()
                                        .h(px(36.0))
                                        .px(px(tokens::space::MD))
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .rounded(px(tokens::radius::MD))
                                        .when(index == selected, |d| {
                                            d.bg(hsla(t.accent)).text_color(hsla(t.accent_text))
                                        })
                                        .when(index != selected, |d| {
                                            d.text_color(hsla(t.text_muted))
                                        })
                                        .when(
                                            mode.is_font() && !label.starts_with("Automatic"),
                                            |d| d.font_family(SharedString::from(label.clone())),
                                        )
                                        .child(SharedString::from(label.clone()))
                                        .child(
                                            div()
                                                .text_size(px(tokens::text::XS))
                                                .child(SharedString::from(shortcut.clone())),
                                        )
                                },
                            ),
                        ))
                        .when(command_count == 0, |d| {
                            d.child(
                                div()
                                    .p(px(tokens::space::MD))
                                    .text_color(hsla(t.text_faint))
                                    .child("No matching commands"),
                            )
                        })
                        .child(
                            div()
                                .h(px(30.0))
                                .px(px(tokens::space::SM))
                                .flex()
                                .items_center()
                                .justify_between()
                                .text_size(px(tokens::text::XS))
                                .text_color(hsla(t.text_faint))
                                .child("↑↓ Navigate   ↵ Run")
                                .child("Esc Close"),
                        ),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_scope_filters_without_relying_on_label_text() {
        let filter = PaletteFilter::parse("ssh: staging");
        let ssh = PaletteCommand::ssh(
            PaletteAction::ConnectSsh("staging".into()),
            "Connect to staging",
        );
        let workspace =
            PaletteCommand::command(PaletteAction::ToggleSidebar, "View: toggle sidebar", "");

        assert!(ssh.matches(&filter));
        assert!(!workspace.matches(&filter));
    }

    #[test]
    fn empty_ssh_scope_keeps_all_ssh_commands() {
        let filter = PaletteFilter::parse(" ssh: ");
        let connect = PaletteCommand::ssh(
            PaletteAction::ConnectSsh("buildbox".into()),
            "SSH: connect to buildbox",
        );
        let bootstrap = PaletteCommand::ssh(
            PaletteAction::CopyRemoteIntegration("buildbox".into()),
            "SSH: copy Bash integration bootstrap",
        );

        assert!(connect.matches(&filter));
        assert!(bootstrap.matches(&filter));
    }

    #[test]
    fn selection_is_clamped_after_results_shrink() {
        let mut palette = PaletteState {
            selected: 8,
            ..PaletteState::default()
        };

        TerminalView::normalize_palette_selection(&mut palette, 2);
        assert_eq!(palette.selected, 1);

        TerminalView::normalize_palette_selection(&mut palette, 0);
        assert_eq!(palette.selected, 0);
    }
}
