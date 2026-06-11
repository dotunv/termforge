//! Settings page — full-screen modal (Ctrl+,) with categorized settings.
//!
//! Inspired by Warp / Windows Terminal settings: a left nav with categories,
//! a right panel showing editable fields with live preview.  All changes are
//! persisted to termforge.toml immediately.

use libterm::config::Config;
use renderer_windows::ui_renderer::UiCommand;
use renderer_windows::tokens::*;

use super::chrome;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SettingsCategory {
    Appearance,
    Shell,
    Terminal,
    Keybindings,
    About,
}

impl SettingsCategory {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::Shell => "Shell",
            Self::Terminal => "Terminal",
            Self::Keybindings => "Keybindings",
            Self::About => "About",
        }
    }

    pub fn all() -> &'static [SettingsCategory] {
        &[
            Self::Appearance,
            Self::Shell,
            Self::Terminal,
            Self::Keybindings,
            Self::About,
        ]
    }
}

// ── Editable field ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SettingsField {
    pub label: String,
    pub description: String,
    pub value: FieldValue,
    pub key: String, // config key for persistence
}

#[derive(Debug, Clone)]
pub enum FieldValue {
    Text(String),
    Number(f32, f32, f32), // value, min, max
    Bool(bool),
    Choice(Vec<String>, usize), // options, selected index
}

impl FieldValue {
    pub fn display(&self) -> String {
        match self {
            FieldValue::Text(s) => s.clone(),
            FieldValue::Number(v, _, _) => format!("{v}"),
            FieldValue::Bool(b) => {
                if *b {
                    "On".to_string()
                } else {
                    "Off".to_string()
                }
            }
            FieldValue::Choice(opts, idx) => opts.get(*idx).cloned().unwrap_or_default(),
        }
    }
}

// ── State ────────────────────────────────────────────────────────────────────

pub struct SettingsState {
    pub category: SettingsCategory,
    pub selected_field: usize,
    pub editing: bool,
    pub edit_buffer: String,
    pub config: Config,
    pub dirty: bool,
    pub scroll_offset: usize,
    pub search_query: String,
    pub searching: bool,
    /// Rows that fit in the fields list at the current window size.  Updated
    /// by the app each frame via [`SettingsState::compute_visible_rows`] so
    /// keyboard scrolling and the renderer agree.
    pub visible_rows: usize,
}

impl SettingsState {
    pub fn new(config: Config) -> Self {
        Self {
            category: SettingsCategory::Appearance,
            selected_field: 0,
            editing: false,
            edit_buffer: String::new(),
            config,
            dirty: false,
            scroll_offset: 0,
            search_query: String::new(),
            searching: false,
            visible_rows: 6,
        }
    }

    /// Rows that fit in the settings fields list — mirrors the panel layout
    /// in [`generate_settings_commands`] (panel height, header, footer, and
    /// optional search bar).
    pub fn compute_visible_rows(window_h: f32, cell_h: f32, searching: bool) -> usize {
        let ch = cell_h;
        let panel_h = (window_h * 0.80).clamp(500.0, 680.0);
        let header_h = ch + 20.0;
        let nav_h = panel_h - header_h - 2.0;
        let search_h = if searching { (ch + 8.0) + 4.0 } else { 0.0 };
        let list_avail = nav_h - 40.0 - ch - 16.0 - search_h;
        let field_h = ch * 2.0 + 16.0;
        ((list_avail / field_h) as usize).max(1)
    }

    /// Return fields matching the search query (case-insensitive).
    /// If search_query is empty, returns all fields.
    pub fn filtered_fields(&self) -> Vec<SettingsField> {
        let all = self.fields_for_category();
        if self.search_query.is_empty() {
            return all;
        }
        let q = self.search_query.to_ascii_lowercase();
        all.into_iter()
            .filter(|f| f.label.to_ascii_lowercase().contains(&q)
                || f.description.to_ascii_lowercase().contains(&q)
                || f.value.display().to_ascii_lowercase().contains(&q))
            .collect()
    }

    pub fn fields_for_category(&self) -> Vec<SettingsField> {
        match self.category {
            SettingsCategory::Appearance => vec![
                SettingsField {
                    label: "Font Family".into(),
                    description: "Terminal font face".into(),
                    value: FieldValue::Text(self.config.font.family.clone()),
                    key: "font.family".into(),
                },
                SettingsField {
                    label: "Font Size".into(),
                    description: "Font size in points".into(),
                    value: FieldValue::Number(self.config.font.size, 8.0, 32.0),
                    key: "font.size".into(),
                },
                SettingsField {
                    label: "Line Height".into(),
                    description: "Line height multiplier".into(),
                    value: FieldValue::Number(self.config.font.line_height, 1.0, 2.5),
                    key: "font.line_height".into(),
                },
                SettingsField {
                    label: "Theme".into(),
                    description: "Color theme".into(),
                    value: FieldValue::Choice(
                        vec![
                            "dark".into(),
                            "light".into(),
                            "solarized".into(),
                            "monokai".into(),
                        ],
                        match self.config.colors.theme.as_str() {
                            "light" => 1,
                            "solarized" => 2,
                            "monokai" => 3,
                            _ => 0,
                        },
                    ),
                    key: "colors.theme".into(),
                },
                SettingsField {
                    label: "Background".into(),
                    description: "Terminal background color (hex)".into(),
                    value: FieldValue::Text(self.config.colors.background.clone()),
                    key: "colors.background".into(),
                },
                SettingsField {
                    label: "Foreground".into(),
                    description: "Terminal text color (hex)".into(),
                    value: FieldValue::Text(self.config.colors.foreground.clone()),
                    key: "colors.foreground".into(),
                },
                SettingsField {
                    label: "Opacity".into(),
                    description: "Window opacity (0.0–1.0)".into(),
                    value: FieldValue::Number(self.config.opacity, 0.3, 1.0),
                    key: "opacity".into(),
                },
            ],
            SettingsCategory::Shell => vec![
                SettingsField {
                    label: "Shell Path".into(),
                    description: "Shell executable (empty = auto-detect)".into(),
                    value: FieldValue::Text(self.config.shell.path.clone()),
                    key: "shell.path".into(),
                },
                SettingsField {
                    label: "Shell Integration".into(),
                    description: "Auto-source OSC 133 hooks on startup".into(),
                    value: FieldValue::Bool(self.config.shell.auto_integration),
                    key: "shell.auto_integration".into(),
                },
                SettingsField {
                    label: "Scrollback Lines".into(),
                    description: "Max lines kept in scrollback buffer".into(),
                    value: FieldValue::Number(
                        self.config.scrollback_lines as f32,
                        1000.0,
                        100_000.0,
                    ),
                    key: "scrollback_lines".into(),
                },
            ],
            SettingsCategory::Terminal => vec![
                SettingsField {
                    label: "Cursor Style".into(),
                    description: "Cursor appearance".into(),
                    value: FieldValue::Choice(
                        vec!["block".into(), "bar".into(), "underline".into()],
                        match self.config.cursor.style.as_str() {
                            "bar" => 1,
                            "underline" => 2,
                            _ => 0,
                        },
                    ),
                    key: "cursor.style".into(),
                },
                SettingsField {
                    label: "Cursor Blink".into(),
                    description: "Enable cursor blinking".into(),
                    value: FieldValue::Bool(self.config.cursor.blink),
                    key: "cursor.blink".into(),
                },
                SettingsField {
                    label: "Padding Top".into(),
                    description: "Terminal top padding in pixels".into(),
                    value: FieldValue::Number(self.config.padding.top as f32, 0.0, 32.0),
                    key: "padding.top".into(),
                },
                SettingsField {
                    label: "Padding Left".into(),
                    description: "Terminal left padding in pixels".into(),
                    value: FieldValue::Number(self.config.padding.left as f32, 0.0, 32.0),
                    key: "padding.left".into(),
                },
                SettingsField {
                    label: "Sidebar Visible".into(),
                    description: "Show sidebar on startup".into(),
                    value: FieldValue::Bool(self.config.window.sidebar_visible),
                    key: "window.sidebar_visible".into(),
                },
                SettingsField {
                    label: "Default Workspace".into(),
                    description: "Workspace name shown at startup".into(),
                    value: FieldValue::Text(self.config.window.default_workspace.clone()),
                    key: "window.default_workspace".into(),
                },
            ],
            SettingsCategory::Keybindings => vec![
                // Placeholder — keybindings are shown read-only for now.
                SettingsField {
                    label: "New Tab".into(),
                    description: "Open a new tab".into(),
                    value: FieldValue::Text("Ctrl+T".into()),
                    key: "keys.new_tab".into(),
                },
                SettingsField {
                    label: "Close Tab".into(),
                    description: "Close current tab".into(),
                    value: FieldValue::Text("Ctrl+W".into()),
                    key: "keys.close_tab".into(),
                },
                SettingsField {
                    label: "Toggle Sidebar".into(),
                    description: "Show/hide sidebar".into(),
                    value: FieldValue::Text("Ctrl+\\".into()),
                    key: "keys.toggle_sidebar".into(),
                },
                SettingsField {
                    label: "Split Pane".into(),
                    description: "Split current pane".into(),
                    value: FieldValue::Text("Ctrl+P".into()),
                    key: "keys.split_pane".into(),
                },
                SettingsField {
                    label: "SSH Manager".into(),
                    description: "Open SSH host manager".into(),
                    value: FieldValue::Text("Ctrl+H".into()),
                    key: "keys.ssh_manager".into(),
                },
                SettingsField {
                    label: "Settings".into(),
                    description: "Open settings".into(),
                    value: FieldValue::Text("Ctrl+,".into()),
                    key: "keys.settings".into(),
                },
            ],
            SettingsCategory::About => vec![],
        }
    }

    /// Apply the edit buffer to the current field and config.
    pub fn apply_edit(&mut self) {
        let fields = self.filtered_fields();
        if let Some(field) = fields.get(self.selected_field) {
            match &field.value {
                FieldValue::Text(_) => {
                    self.set_config_value(&field.key, &self.edit_buffer.clone());
                }
                FieldValue::Number(_, min, max) => {
                    if let Ok(v) = self.edit_buffer.parse::<f32>() {
                        let clamped = v.clamp(*min, *max);
                        self.set_config_value(&field.key, &clamped.to_string());
                    }
                }
                _ => {}
            }
            self.dirty = true;
        }
        self.editing = false;
        self.edit_buffer.clear();
    }

    /// Toggle a bool field or cycle a choice field.
    pub fn toggle_current(&mut self) {
        let fields = self.filtered_fields();
        if let Some(field) = fields.get(self.selected_field) {
            match &field.value {
                FieldValue::Bool(b) => {
                    self.set_config_value(&field.key, &(!b).to_string());
                    self.dirty = true;
                }
                FieldValue::Choice(opts, idx) => {
                    let next = (idx + 1) % opts.len();
                    self.set_config_value(&field.key, &opts[next]);
                    self.dirty = true;
                }
                FieldValue::Text(_) | FieldValue::Number(..) => {
                    // Enter edit mode
                    self.editing = true;
                    self.edit_buffer = field.value.display();
                }
            }
        }
    }

    fn set_config_value(&mut self, key: &str, val: &str) {
        match key {
            "font.family" => self.config.font.family = val.to_string(),
            "font.size" => {
                if let Ok(v) = val.parse() {
                    self.config.font.size = v;
                }
            }
            "font.line_height" => {
                if let Ok(v) = val.parse() {
                    self.config.font.line_height = v;
                }
            }
            "colors.theme" => self.config.colors.theme = val.to_string(),
            "colors.background" => self.config.colors.background = val.to_string(),
            "colors.foreground" => self.config.colors.foreground = val.to_string(),
            "opacity" => {
                if let Ok(v) = val.parse() {
                    self.config.opacity = v;
                }
            }
            "shell.path" => self.config.shell.path = val.to_string(),
            "shell.auto_integration" => {
                if let Ok(v) = val.parse() {
                    self.config.shell.auto_integration = v;
                }
            }
            "scrollback_lines" => {
                if let Ok(v) = val.parse::<f32>() {
                    self.config.scrollback_lines = v as usize;
                }
            }
            "cursor.style" => self.config.cursor.style = val.to_string(),
            "cursor.blink" => {
                if let Ok(v) = val.parse() {
                    self.config.cursor.blink = v;
                }
            }
            "padding.top" => {
                if let Ok(v) = val.parse::<f32>() {
                    self.config.padding.top = v as u32;
                }
            }
            "padding.left" => {
                if let Ok(v) = val.parse::<f32>() {
                    self.config.padding.left = v as u32;
                }
            }
            "window.sidebar_visible" => {
                if let Ok(v) = val.parse() {
                    self.config.window.sidebar_visible = v;
                }
            }
            "window.default_workspace" => self.config.window.default_workspace = val.to_string(),
            _ => {}
        }
    }

    pub fn scroll_by(&mut self, delta: i32) {
        let fields = self.filtered_fields();
        let max = fields.len().saturating_sub(1);
        let new = (self.selected_field as i32 + delta).clamp(0, max as i32) as usize;
        self.selected_field = new;
        // Keep selected field in view
        let visible = self.visible_rows.max(1);
        if self.selected_field < self.scroll_offset {
            self.scroll_offset = self.selected_field;
        } else if self.selected_field >= self.scroll_offset + visible {
            self.scroll_offset = self.selected_field.saturating_sub(visible - 1);
        }
    }

    /// Handle keyboard input while settings are open.  Returns `true` if the
    /// modal should close.
    pub fn handle_key(&mut self, vk: u32, ctrl: bool) -> bool {
        if self.editing {
            match vk {
                0x1B => {
                    self.editing = false;
                    self.edit_buffer.clear();
                }
                0x0D => self.apply_edit(),
                _ => {}
            }
            return false;
        }

        if self.searching {
            match vk {
                0x1B => {
                    self.searching = false;
                    self.search_query.clear();
                    self.scroll_offset = 0;
                    self.selected_field = 0;
                }
                0x26 => self.scroll_by(-1),
                0x28 => self.scroll_by(1),
                0x0D | 0x20 => self.toggle_current(),
                _ => {}
            }
            return false;
        }

        // Ctrl+F toggles search
        if ctrl && vk == 0x46 {
            self.searching = true;
            self.search_query.clear();
            self.selected_field = 0;
            self.scroll_offset = 0;
            return false;
        }

        match vk {
            0x1B => return true, // Escape — close
            0x26 => self.scroll_by(-1), // Up
            0x28 => self.scroll_by(1),  // Down
            0x25 => {
                // Left — prev category
                let cats = SettingsCategory::all();
                let idx = cats.iter().position(|c| *c == self.category).unwrap_or(0);
                if idx > 0 {
                    self.category = cats[idx - 1];
                    self.selected_field = 0;
                }
            }
            0x27 => {
                // Right — next category
                let cats = SettingsCategory::all();
                let idx = cats.iter().position(|c| *c == self.category).unwrap_or(0);
                if idx + 1 < cats.len() {
                    self.category = cats[idx + 1];
                    self.selected_field = 0;
                }
            }
            0x0D | 0x20 => self.toggle_current(), // Enter/Space — toggle or edit
            _ => {}
        }
        false
    }

    /// Handle a character input while editing a text/number field, or when searching.
    pub fn handle_char(&mut self, c: char) {
        if self.searching {
            match c {
                '\x08' | '\x7f' => {
                    self.search_query.pop();
                }
                _ if !c.is_control() => {
                    self.search_query.push(c);
                }
                _ => {}
            }
            self.selected_field = 0;
            self.scroll_offset = 0;
            return;
        }
        if !self.editing {
            return;
        }
        match c {
            '\x08' | '\x7f' => {
                self.edit_buffer.pop();
            }
            _ if !c.is_control() => self.edit_buffer.push(c),
            _ => {}
        }
    }

    /// Save config to disk if dirty.
    pub fn save_if_dirty(&mut self) {
        if self.dirty {
            let path = Config::default_path();
            if let Err(e) = self.config.save(&path) {
                tracing::error!("Failed to save config: {e}");
            } else {
                tracing::info!("Config saved to {}", path.display());
            }
            self.dirty = false;
        }
    }
}

/// Friendly names for colour hex values shown in settings.
fn color_name(hex: &str) -> Option<&'static str> {
    match hex.to_ascii_lowercase().as_str() {
        "#0d1117" => Some("GitHub Dark"),
        "#16161e" => Some("Warp Night"),
        "#1e1f2b" => Some("Slate Violet"),
        "#e6edf3" | "#e5e9f0" => Some("Snow"),
        "#000000" => Some("Black"),
        "#ffffff" => Some("White"),
        "#282a36" => Some("Dracula"),
        "#1a1b26" => Some("Tokyo Night"),
        "#002b36" => Some("Solarized Dark"),
        "#272822" => Some("Monokai"),
        _ => None,
    }
}

// ── Mouse hit-testing ────────────────────────────────────────────────────────

/// What a click at (x, y) lands on inside the settings modal.
/// Geometry must stay in sync with [`generate_settings_commands`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SettingsHit {
    Category(usize),
    /// Index into `filtered_fields()` (absolute, scroll already applied).
    Field(usize),
    /// Inside the panel but not on an interactive element.
    Panel,
    /// On the dim backdrop — close the modal.
    Outside,
}

pub fn hit_test(
    state: &SettingsState,
    window_w: f32,
    window_h: f32,
    cell_h: f32,
    x: f32,
    y: f32,
) -> SettingsHit {
    let ch = cell_h;
    let panel_w = (window_w * 0.75).clamp(700.0, 960.0);
    let panel_h = (window_h * 0.80).clamp(500.0, 680.0);
    let px = (window_w - panel_w) * 0.5;
    let py = (window_h - panel_h) * 0.5;

    if x < px || x >= px + panel_w || y < py || y >= py + panel_h {
        return SettingsHit::Outside;
    }

    let header_h = ch + 20.0;
    let nav_y = py + header_h + 1.0;
    let nav_w: f32 = 160.0;
    let nav_x = px + 1.0;

    // Left nav categories
    if x >= nav_x && x < nav_x + nav_w && y >= nav_y + 8.0 {
        let item_h = ch + 12.0;
        let idx = ((y - nav_y - 8.0) / item_h) as usize;
        if idx < SettingsCategory::all().len() {
            return SettingsHit::Category(idx);
        }
    }

    // Field rows in the right panel
    let content_x = nav_x + nav_w + 1.0;
    let content_w = panel_w - nav_w - 3.0;
    let content_y = nav_y;
    let nav_h = panel_h - header_h - 2.0;
    let search_h = if state.searching { (ch + 8.0) + 4.0 } else { 0.0 };
    let list_top = content_y + 40.0 + search_h;
    let list_avail = nav_h - 40.0 - ch - 16.0 - search_h;
    let field_h = ch * 2.0 + 16.0;
    if x >= content_x + 8.0
        && x < content_x + content_w - 8.0
        && y >= list_top
        && y < list_top + list_avail
    {
        let idx = state.scroll_offset + ((y - list_top) / field_h) as usize;
        if idx < state.filtered_fields().len() {
            return SettingsHit::Field(idx);
        }
    }

    SettingsHit::Panel
}

// ── Rendering ────────────────────────────────────────────────────────────────

pub fn generate_settings_commands(
    state: &SettingsState,
    window_w: f32,
    window_h: f32,
    cell_w: u32,
    cell_h: u32,
    ui_char_w: f32,
) -> Vec<UiCommand> {
    let _ = cell_w;
    let ucw = ui_char_w;
    let ch = cell_h as f32;
    let mut cmds = Vec::with_capacity(128);

    // Dark overlay
    cmds.push(UiCommand::FillRect {
        x: 0.0,
        y: 0.0,
        w: window_w,
        h: window_h,
        color: OVERLAY_DIM,
    });

    // Main panel — centered, large
    let panel_w = (window_w * 0.75).clamp(700.0, 960.0);
    let panel_h = (window_h * 0.80).clamp(500.0, 680.0);
    let px = (window_w - panel_w) * 0.5;
    let py = (window_h - panel_h) * 0.5;

    // Drop shadow behind panel
    cmds.push(chrome::shadow(px, py, panel_w, panel_h, 8.0));

    // Panel background (rounded)
    cmds.push(UiCommand::FillRoundRect {
        x: px,
        y: py,
        w: panel_w,
        h: panel_h,
        radius: RADIUS_LG,
        color: BG_SURFACE,
        bg: BG_BASE,
    });
    // Border
    {
        let (ox, oy, bw, bh) = (px, py + panel_h - 1.0, panel_w, 1.0);
        cmds.push(UiCommand::FillRect {
            x: ox,
            y: oy,
            w: bw,
            h: bh,
            color: BORDER_DEFAULT,
        });
    }

    // ── Header ───────────────────────────────────────────────────────────
    let header_h = ch + 20.0;
    cmds.push(UiCommand::FillRect {
        x: px + 1.0,
        y: py + 1.0,
        w: panel_w - 2.0,
        h: header_h,
        color: BG_BASE,
    });
    cmds.push(UiCommand::DrawUiText {
        x: px + 16.0,
        y: py + 10.0,
        text: "Settings".to_string(),
        fg: TEXT_PRIMARY,
        bg: BG_BASE,
    });
    cmds.push(UiCommand::DrawUiText {
        x: px + panel_w - 16.0 - 11.0 * ucw,
        y: py + 10.0,
        text: "[Esc] close".to_string(),
        fg: TEXT_MUTED,
        bg: BG_BASE,
    });

    // ── Left nav (categories) ────────────────────────────────────────────
    let nav_w: f32 = 160.0;
    let nav_x = px + 1.0;
    let nav_y = py + header_h + 1.0;
    let nav_h = panel_h - header_h - 2.0;

    // Nav background
    cmds.push(UiCommand::FillRect {
        x: nav_x,
        y: nav_y,
        w: nav_w,
        h: nav_h,
        color: BG_SURFACE,
    });
    // Right border on nav
    cmds.push(UiCommand::FillRect {
        x: nav_x + nav_w,
        y: nav_y,
        w: 1.0,
        h: nav_h,
        color: BORDER_DEFAULT,
    });

    let item_h = ch + 12.0;
    for (i, cat) in SettingsCategory::all().iter().enumerate() {
        let iy = nav_y + 8.0 + i as f32 * item_h;
        let is_active = *cat == state.category;
        let bg = if is_active { BG_HOVER } else { BG_SURFACE };
        let fg = if is_active { TEXT_PRIMARY } else { TEXT_MUTED };

        cmds.push(UiCommand::FillRect {
            x: nav_x,
            y: iy,
            w: nav_w,
            h: item_h,
            color: bg,
        });
        // Active indicator bar
        if is_active {
            cmds.push(UiCommand::FillRect {
                x: nav_x,
                y: iy,
                w: 3.0,
                h: item_h,
                color: ACCENT_GREEN,
            });
        }
        cmds.push(UiCommand::DrawUiText {
            x: nav_x + 16.0,
            y: iy + (item_h - ch) * 0.5,
            text: cat.label().to_string(),
            fg,
            bg,
        });
    }

    // Navigation hint at bottom of nav
    let hint_y = nav_y + nav_h - ch - 8.0;
    cmds.push(UiCommand::DrawUiText {
        x: nav_x + 8.0,
        y: hint_y,
        text: "<-/-> nav".to_string(),
        fg: TEXT_FAINT,
        bg: BG_SURFACE,
    });

    // ── Right panel (fields) ─────────────────────────────────────────────
    let content_x = nav_x + nav_w + 1.0;
    let content_w = panel_w - nav_w - 3.0;
    let content_y = nav_y;

    // Content background
    cmds.push(UiCommand::FillRect {
        x: content_x,
        y: content_y,
        w: content_w,
        h: nav_h,
        color: BG_BASE,
    });

    // Category title
    cmds.push(UiCommand::DrawUiText {
        x: content_x + 16.0,
        y: content_y + 12.0,
        text: state.category.label().to_string(),
        fg: TEXT_PRIMARY,
        bg: BG_BASE,
    });

    // About page — special
    if state.category == SettingsCategory::About {
        let about_lines = [
            "TermForge v0.2",
            "",
            "A high-performance, native terminal emulator",
            "with first-class Windows support.",
            "",
            "Built with Rust + DirectX 12",
            "by Victor Oluwasomi Dotun",
            "",
            "Config: %APPDATA%/TermForge/termforge.toml",
        ];
        for (i, line) in about_lines.iter().enumerate() {
            cmds.push(UiCommand::DrawUiText {
                x: content_x + 16.0,
                y: content_y + 40.0 + i as f32 * (ch + 4.0),
                text: line.to_string(),
                fg: if line.is_empty() { BG_BASE } else { TEXT_MUTED },
                bg: BG_BASE,
            });
        }
        return cmds;
    }

    // Search bar
    let search_bar_h = ch + 8.0;
    if state.searching {
        cmds.push(UiCommand::FillRect {
            x: content_x + 8.0,
            y: content_y + 40.0,
            w: content_w - 16.0,
            h: search_bar_h,
            color: BG_BASE,
        });
        // Search icon + query
        let search_display = if state.search_query.is_empty() {
            "Type to filter...".to_string()
        } else {
            format!("{}_", state.search_query)
        };
        let search_fg = if state.search_query.is_empty() { TEXT_FAINT } else { TEXT_PRIMARY };
        cmds.push(UiCommand::DrawUiText {
            x: content_x + 16.0,
            y: content_y + 42.0 + (search_bar_h - ch) * 0.5,
            text: format!("\u{2315} {search_display}"),
            fg: search_fg,
            bg: BG_BASE,
        });
    }

    // Fields list
    let fields = state.filtered_fields();
    let field_h = ch * 2.0 + 16.0; // label + value + padding
    let list_top = content_y + 40.0 + if state.searching { search_bar_h + 4.0 } else { 0.0 };
    let list_avail = nav_h - 40.0 - ch - 16.0 - if state.searching { search_bar_h + 4.0 } else { 0.0 };
    let total_field_h = fields.len() as f32 * field_h;

    for (field_idx, field) in fields.iter().enumerate().skip(state.scroll_offset) {
        let fy = list_top + (field_idx - state.scroll_offset) as f32 * field_h;
        if fy + field_h > list_top + list_avail {
            break;
        }

        let is_selected = field_idx == state.selected_field;
        let row_bg = if is_selected { BG_HOVER } else { BG_BASE };

        // Row background
        cmds.push(UiCommand::FillRect {
            x: content_x + 8.0,
            y: fy,
            w: content_w - 16.0,
            h: field_h - 4.0,
            color: row_bg,
        });

        // Selection indicator
        if is_selected {
            cmds.push(UiCommand::FillRect {
                x: content_x + 8.0,
                y: fy,
                w: 3.0,
                h: field_h - 4.0,
                color: ACCENT_GREEN,
            });
        }

        // Label
        cmds.push(UiCommand::DrawUiText {
            x: content_x + 20.0,
            y: fy + 4.0,
            text: field.label.clone(),
            fg: TEXT_PRIMARY,
            bg: row_bg,
        });

        // Description
        cmds.push(UiCommand::DrawUiText {
            x: content_x + 20.0,
            y: fy + 4.0 + ch + 2.0,
            text: field.description.clone(),
            fg: TEXT_FAINT,
            bg: row_bg,
        });

        // Value (right-aligned). Colour fields show a friendly name next to
        // the raw hex so values read at a glance.
        let val_display = if is_selected && state.editing {
            format!("{}_", state.edit_buffer)
        } else {
            let raw = field.value.display();
            if field.key.starts_with("colors.") {
                match color_name(&raw) {
                    Some(name) => format!("{name}  {raw}"),
                    None => raw,
                }
            } else {
                raw
            }
        };
        let val_w = val_display.chars().count() as f32 * ucw;
        let val_x = content_x + content_w - 24.0 - val_w;
        let val_color = match &field.value {
            FieldValue::Bool(true) => ACCENT_GREEN,
            FieldValue::Bool(false) => TEXT_MUTED,
            FieldValue::Choice(..) => ACCENT_BLUE,
            _ => TEXT_PRIMARY,
        };
        cmds.push(UiCommand::DrawUiText {
            x: val_x,
            y: fy + 4.0,
            text: val_display,
            fg: val_color,
            bg: row_bg,
        });
    }

    // Scrollbar for fields
    if total_field_h > list_avail {
        let sb_w = 6.0;
        let sb_x = content_x + content_w - sb_w - 1.0;
        cmds.push(UiCommand::FillRect {
            x: sb_x, y: list_top, w: sb_w, h: list_avail,
            color: BG_SURFACE,
        });
        let thumb_h = (list_avail / total_field_h * list_avail).max(field_h);
        let max_thumb_y = list_avail - thumb_h;
        let scroll_frac = state.scroll_offset as f32 / fields.len().saturating_sub(1).max(1) as f32;
        let thumb_y = list_top + (scroll_frac * max_thumb_y).clamp(0.0, max_thumb_y);
        cmds.push(UiCommand::FillRect {
            x: sb_x, y: thumb_y, w: sb_w, h: thumb_h,
            color: TEXT_MUTED,
        });
    }

    // Footer hint
    let footer_y = content_y + nav_h - ch - 8.0;
    let hint = if state.searching {
        "Type to filter  |  Esc to clear search"
    } else {
        match state.category {
            SettingsCategory::Keybindings => "Keybindings are read-only in this version",
            _ => "Ctrl+F search  |  Up/Down select  |  Enter/Space edit  |  Changes save automatically",
        }
    };
    cmds.push(UiCommand::DrawUiText {
        x: content_x + 16.0,
        y: footer_y,
        text: hint.to_string(),
        fg: TEXT_FAINT,
        bg: BG_BASE,
    });

    cmds
}
