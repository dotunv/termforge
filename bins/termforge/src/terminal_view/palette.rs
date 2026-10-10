//! Command palette.

use super::*;

impl TerminalView {
    pub(super) fn filtered_palette_commands(&self) -> Vec<(PaletteAction, String, String)> {
        let query = self
            .palette
            .as_ref()
            .map(|palette| palette.query.to_ascii_lowercase())
            .unwrap_or_default();
        let mut commands: Vec<_> = PALETTE_COMMANDS
            .iter()
            .cloned()
            .map(|(action, label, shortcut)| (action, label.to_owned(), shortcut.to_owned()))
            .collect();
        if self.spawn.ssh_host.is_some() {
            commands.push((
                PaletteAction::SwitchLocal,
                "Workspace: switch to local".into(),
                String::new(),
            ));
        }
        for (index, shell) in self.shells.iter().enumerate() {
            commands.push((
                PaletteAction::NewShell(index),
                format!("Shell: new {} terminal", shell.name),
                String::new(),
            ));
            commands.push((
                PaletteAction::SetDefaultShell(index),
                format!("Shell: use {} for new terminals", shell.name),
                String::new(),
            ));
        }
        commands.extend(self.ssh_hosts.iter().map(|host| {
            (
                PaletteAction::ConnectSsh(host.clone()),
                format!("SSH: connect to {host}"),
                String::new(),
            )
        }));
        commands
            .into_iter()
            .filter(|(_, label, _)| query.is_empty() || label.to_ascii_lowercase().contains(&query))
            .collect()
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
        match key {
            "escape" => self.palette = None,
            "up" if count > 0 => palette.selected = (palette.selected + count - 1) % count,
            "down" if count > 0 => palette.selected = (palette.selected + 1) % count,
            "enter" if count > 0 => {
                let action = commands[palette.selected.min(count - 1)].0.clone();
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
            PaletteAction::FontReset => self.font_size = 14.0,
            PaletteAction::ScrollTop => self.scroll(Scroll::Top, cx),
            PaletteAction::ScrollBottom => self.scroll(Scroll::Bottom, cx),
            PaletteAction::CopyDiagnostics => {
                let text = format!(
                    "TermForge {} · protocol v{} · shell={} · cwd={}",
                    env!("CARGO_PKG_VERSION"),
                    tf_proto::PROTOCOL_VERSION,
                    self.spawn.profile.name,
                    self.cwd.as_deref().unwrap_or("unknown")
                );
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            PaletteAction::NextAttention => self.jump_to_attention(cx),
            PaletteAction::RenameWorkspace => self.begin_rename(self.active_tab, cx),
            PaletteAction::TogglePin => self.toggle_pin(self.active_tab, cx),
            PaletteAction::MoveUp => self.move_active_tab(false, cx),
            PaletteAction::MoveDown => self.move_active_tab(true, cx),
            PaletteAction::NewShell(index) => self.new_shell_tab(index, cx),
            PaletteAction::SetDefaultShell(index) => self.set_default_shell(index, cx),
            PaletteAction::ToggleSidebar => self.toggle_sidebar(cx),
            PaletteAction::OpenSettings => self.open_settings(cx),
            PaletteAction::NewTask => self.open_task_form("", "", cx),
            PaletteAction::ListTasks => self.write(b"tf task list\r"),
            PaletteAction::StatusWorking => self.write(b"tf status working --app termforge\r"),
            PaletteAction::StatusDone => self.write(b"tf status done --app termforge\r"),
            PaletteAction::StatusClear => self.write(b"tf status clear\r"),
            PaletteAction::DaemonDoctor => self.write(b"forged doctor\r"),
            PaletteAction::ConnectSsh(host) => self.new_ssh_tab(host, cx),
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
        let visible_start = palette.selected.saturating_sub(7);
        let t = &self.theme;
        let query = if palette.query.is_empty() {
            "Type a command…".to_owned()
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
                                .child("COMMAND PALETTE")
                                .child(SharedString::from(format!("{command_count} actions"))),
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
                                        .when(index == palette.selected, |d| {
                                            d.bg(hsla(t.accent)).text_color(hsla(t.accent_text))
                                        })
                                        .when(index != palette.selected, |d| {
                                            d.text_color(hsla(t.text_muted))
                                        })
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
