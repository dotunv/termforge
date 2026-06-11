//! Command palette (Ctrl+K) — Linear-style fuzzy launcher.
//!
//! Type to filter, ↑/↓ to select, Enter to run, Esc to dismiss.  Besides the
//! fixed actions, the palette is a universal search: it also matches open
//! tabs, workspaces, and saved SSH hosts.  The app rebuilds the item list
//! each time the palette opens, so dynamic entries are always current.

use renderer_windows::icons;
use renderer_windows::tokens::*;
use renderer_windows::ui_renderer::{UiCommand, UiTextStyle};

use super::chrome;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PaletteAction {
    NewTab,
    CloseTab,
    NextTab,
    ToggleSidebar,
    ToggleSplit,
    NewWorkspace,
    SshManager,
    AgentLauncher,
    Settings,
    Quit,
    /// Download + apply a curated font by catalog index (no system install).
    DownloadFont(usize),
    /// Jump to an open tab by index.
    SwitchTab(usize),
    /// Switch to a workspace by index.
    SwitchWorkspace(usize),
    /// Connect to a saved SSH host by index into the host store.
    ConnectSsh(usize),
}

#[derive(Debug, Clone)]
pub struct PaletteEntry {
    pub label: String,
    /// Right-gutter hint: a keyboard shortcut or a category tag.
    pub hint: String,
    pub action: PaletteAction,
}

/// The fixed action entries, always present.
pub fn base_entries() -> Vec<PaletteEntry> {
    fn e(label: &str, hint: &str, action: PaletteAction) -> PaletteEntry {
        PaletteEntry { label: label.into(), hint: hint.into(), action }
    }
    vec![
        e("New tab", "Ctrl+T", PaletteAction::NewTab),
        e("Close tab", "Ctrl+W", PaletteAction::CloseTab),
        e("Next tab", "Ctrl+Tab", PaletteAction::NextTab),
        e("Toggle sidebar", "Ctrl+B", PaletteAction::ToggleSidebar),
        e("Toggle split pane", "Ctrl+P", PaletteAction::ToggleSplit),
        e("New workspace", "", PaletteAction::NewWorkspace),
        e("SSH manager", "Ctrl+H", PaletteAction::SshManager),
        e("Launch agent", "Ctrl+A", PaletteAction::AgentLauncher),
        e("Settings", "Ctrl+,", PaletteAction::Settings),
        e("Quit TermForge", "", PaletteAction::Quit),
    ]
}

/// Font-download entries from the curated catalog.
pub fn font_entries(catalog: &[(&str, &str, &str, &str)]) -> Vec<PaletteEntry> {
    catalog
        .iter()
        .enumerate()
        .map(|(i, (name, ..))| PaletteEntry {
            label: format!("Download font: {name}"),
            hint: "font".into(),
            action: PaletteAction::DownloadFont(i),
        })
        .collect()
}

/// Case-insensitive subsequence match ("st" hits "Settings", "nt" hits
/// "New tab" and "Next tab").
fn fuzzy_match(query: &str, label: &str) -> bool {
    let mut chars = label.chars().flat_map(char::to_lowercase);
    'outer: for q in query.chars().flat_map(char::to_lowercase) {
        for c in chars.by_ref() {
            if c == q {
                continue 'outer;
            }
        }
        return false;
    }
    true
}

const MAX_VISIBLE: usize = 10;

pub struct CommandPaletteState {
    pub query: String,
    pub selected: usize,
    items: Vec<PaletteEntry>,
}

impl CommandPaletteState {
    pub fn new() -> Self {
        Self { query: String::new(), selected: 0, items: base_entries() }
    }

    /// Replace the item list (called by the app each time the palette opens,
    /// with dynamic tab / workspace / host entries appended).
    pub fn set_items(&mut self, items: Vec<PaletteEntry>) {
        self.items = items;
        self.query.clear();
        self.selected = 0;
    }

    pub fn reset(&mut self) {
        self.query.clear();
        self.selected = 0;
    }

    fn filtered(&self) -> Vec<&PaletteEntry> {
        self.items
            .iter()
            .filter(|i| fuzzy_match(&self.query, &i.label))
            .take(MAX_VISIBLE)
            .collect()
    }

    pub fn move_selection(&mut self, delta: i32) {
        let n = self.filtered().len();
        if n == 0 {
            self.selected = 0;
            return;
        }
        self.selected =
            (self.selected as i32 + delta).rem_euclid(n as i32) as usize;
    }

    /// Feed a WM_CHAR.  Returns the chosen action on Enter.
    pub fn handle_char(&mut self, c: char) -> Option<PaletteAction> {
        match c {
            '\r' | '\n' => self.filtered().get(self.selected).map(|i| i.action),
            '\x08' | '\x7f' => {
                self.query.pop();
                self.selected = 0;
                None
            }
            _ if !c.is_control() => {
                self.query.push(c);
                self.selected = 0;
                None
            }
            _ => None,
        }
    }
}

pub fn generate_command_palette_commands(
    state: &CommandPaletteState,
    window_w: f32,
    window_h: f32,
    ch: f32,
    ucw: f32,
) -> Vec<UiCommand> {
    let mut cmds = Vec::with_capacity(64);

    cmds.push(UiCommand::FillRect {
        x: 0.0, y: 0.0, w: window_w, h: window_h,
        color: OVERLAY_DIM,
    });

    let items = state.filtered();
    let row_h = ch + 14.0;
    let input_h = ch + 22.0;
    let panel_w = (window_w * 0.45).clamp(440.0, 600.0);
    let panel_h = input_h + items.len().max(1) as f32 * row_h + SPACE_3;
    let px = (window_w - panel_w) * 0.5;
    let py = window_h * 0.20;

    cmds.push(chrome::shadow(px, py, panel_w, panel_h, 16.0));
    cmds.push(UiCommand::FillRoundRect {
        x: px, y: py, w: panel_w, h: panel_h, radius: RADIUS_LG,
        color: BG_RAISED, bg: BG_BASE,
    });
    cmds.push(UiCommand::StrokeRoundRect {
        x: px, y: py, w: panel_w, h: panel_h, radius: RADIUS_LG,
        thickness: 1.0, color: BORDER_STRONG,
    });

    // Query input row: search icon + text.
    cmds.push(UiCommand::DrawIcon {
        x: px + SPACE_4, y: py + (input_h - ch) * 0.5,
        text: icons::SEARCH.to_string(), fg: TEXT_MUTED, bg: BG_RAISED, large: false,
    });
    let (q_text, q_fg) = if state.query.is_empty() {
        ("Search commands, tabs, workspaces, hosts…".to_string(), TEXT_FAINT)
    } else {
        (format!("{}_", state.query), TEXT_PRIMARY)
    };
    cmds.push(UiCommand::DrawStyledText {
        x: px + SPACE_4 + 26.0, y: py + (input_h - ch) * 0.5,
        text: q_text, fg: q_fg, bg: BG_RAISED, style: UiTextStyle::Body,
    });
    cmds.push(UiCommand::FillRect {
        x: px + SPACE_3, y: py + input_h, w: panel_w - SPACE_3 * 2.0, h: 1.0,
        color: BORDER_DEFAULT,
    });

    if items.is_empty() {
        cmds.push(UiCommand::DrawStyledText {
            x: px + SPACE_4, y: py + input_h + SPACE_2,
            text: "No matches".to_string(), fg: TEXT_FAINT, bg: BG_RAISED, style: UiTextStyle::Body,
        });
        return cmds;
    }

    for (i, item) in items.iter().enumerate() {
        let ry = py + input_h + SPACE_1 + i as f32 * row_h;
        let is_sel = i == state.selected;
        let row_bg = if is_sel { BG_HOVER } else { BG_RAISED };
        if is_sel {
            cmds.push(UiCommand::FillRoundRect {
                x: px + SPACE_1, y: ry, w: panel_w - SPACE_1 * 2.0, h: row_h - 2.0,
                radius: RADIUS_SM, color: BG_HOVER, bg: BG_RAISED,
            });
            cmds.push(UiCommand::FillRoundRect {
                x: px + SPACE_1, y: ry + 4.0, w: 3.0, h: row_h - 10.0,
                radius: 1.5, color: ACCENT_BLUE, bg: BG_HOVER,
            });
        }
        let label_style = if is_sel { UiTextStyle::Bold } else { UiTextStyle::Body };
        cmds.push(UiCommand::DrawStyledText {
            x: px + SPACE_4, y: ry + (row_h - ch) * 0.5,
            text: item.label.clone(),
            fg: if is_sel { TEXT_PRIMARY } else { TEXT_MUTED },
            bg: row_bg, style: label_style,
        });
        if !item.hint.is_empty() {
            let hint_w = item.hint.chars().count() as f32 * ucw * 0.92;
            cmds.push(UiCommand::DrawStyledText {
                x: px + panel_w - hint_w - SPACE_4,
                y: ry + (row_h - ch) * 0.5 + 1.0,
                text: item.hint.clone(), fg: TEXT_FAINT, bg: row_bg, style: UiTextStyle::Caption,
            });
        }
    }

    cmds
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_subsequence() {
        assert!(fuzzy_match("st", "Settings"));
        assert!(fuzzy_match("nwt", "New tab"));
        assert!(fuzzy_match("", "anything"));
        assert!(!fuzzy_match("xyz", "Settings"));
    }

    #[test]
    fn enter_returns_selected_action() {
        let mut s = CommandPaletteState::new();
        for c in "ssh m".chars() {
            s.handle_char(c);
        }
        assert_eq!(s.handle_char('\r'), Some(PaletteAction::SshManager));
    }

    #[test]
    fn selection_wraps() {
        let mut s = CommandPaletteState::new();
        s.move_selection(-1);
        assert_eq!(s.selected, base_entries().len() - 1);
        s.move_selection(1);
        assert_eq!(s.selected, 0);
    }

    #[test]
    fn dynamic_entries_searchable() {
        let mut s = CommandPaletteState::new();
        let mut items = base_entries();
        items.push(PaletteEntry {
            label: "Go to tab: local 2".into(),
            hint: "tab".into(),
            action: PaletteAction::SwitchTab(1),
        });
        items.push(PaletteEntry {
            label: "Connect: dev@prod-server".into(),
            hint: "ssh host".into(),
            action: PaletteAction::ConnectSsh(0),
        });
        s.set_items(items);
        for c in "local 2".chars() {
            s.handle_char(c);
        }
        assert_eq!(s.handle_char('\r'), Some(PaletteAction::SwitchTab(1)));
        s.reset();
        for c in "prod".chars() {
            s.handle_char(c);
        }
        assert_eq!(s.handle_char('\r'), Some(PaletteAction::ConnectSsh(0)));
    }
}
