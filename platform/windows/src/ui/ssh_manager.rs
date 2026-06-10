//! SSH Manager — full-screen modal overlay (Ctrl+H to open/close).
//!
//! Two modes:
//!   `List`    — browse saved hosts, press Enter to connect, N for new
//!   `NewHost` — form with four text fields (hostname, port, user, key path)
//!
//! All rendering is via [`UiCommand`]s fed into the existing DX12 pipeline.

use libterm::ssh::host_store::HostConfig;
use renderer_windows::ui_renderer::UiCommand;
use renderer_windows::tokens::*;

use super::chrome;

// ── Text input ────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone)]
pub struct TextInput {
    pub label: &'static str,
    pub value: String,
    pub active: bool,
}

impl TextInput {
    pub fn new(label: &'static str) -> Self {
        Self {
            label,
            value: String::new(),
            active: false,
        }
    }

    pub fn push(&mut self, c: char) {
        self.value.push(c);
    }

    pub fn pop(&mut self) {
        self.value.pop();
    }

    pub fn clear(&mut self) {
        self.value.clear();
    }
}

// ── Manager state ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum ManagerMode {
    List,
    NewHost,
}

pub struct SshManagerState {
    pub mode: ManagerMode,
    pub hosts: Vec<HostConfig>,
    pub selected: usize,
    /// NewHost form fields: [hostname, port, username, key_path]
    pub fields: [TextInput; 4],
    pub focus: usize, // active field index in NewHost mode
}

impl SshManagerState {
    pub fn new(hosts: Vec<HostConfig>) -> Self {
        Self {
            mode: ManagerMode::List,
            hosts,
            selected: 0,
            fields: [
                TextInput::new("Hostname"),
                TextInput::new("Port"),
                TextInput::new("Username"),
                TextInput::new("Key path (blank = auto)"),
            ],
            focus: 0,
        }
    }

    // ── Keyboard routing ──────────────────────────────────────────────────────

    /// Returns `Some(HostConfig)` when the user confirms a connection.
    pub fn handle_char(&mut self, c: char) -> Option<HostConfig> {
        match self.mode {
            ManagerMode::List => match c {
                'n' | 'N' => {
                    self.mode = ManagerMode::NewHost;
                    self.focus = 0;
                    for f in &mut self.fields {
                        f.clear();
                        f.active = false;
                    }
                    self.fields[0].active = true;
                }
                '\r' | '\n' => return self.connect_selected(),
                _ => {}
            },
            ManagerMode::NewHost => {
                match c {
                    '\r' | '\n' => return self.submit_new_host(),
                    '\x08' | '\x7f' => {
                        self.fields[self.focus].pop();
                    } // backspace
                    '\x09' => self.next_field(), // Tab
                    _ if !c.is_control() => {
                        self.fields[self.focus].push(c);
                    }
                    _ => {}
                }
            }
        }
        None
    }

    pub fn scroll_by(&mut self, delta: i32) {
        let max = self.hosts.len().saturating_sub(1);
        let new = (self.selected as i32 + delta).clamp(0, max as i32) as usize;
        self.selected = new;
    }

    pub fn handle_key_up(&mut self) {
        self.scroll_by(-1);
    }
    pub fn handle_key_down(&mut self) {
        self.scroll_by(1);
    }

    /// Delete the currently selected host from the list.
    /// Returns `true` if a host was removed (caller should persist to disk).
    pub fn handle_delete(&mut self) -> bool {
        if self.mode == ManagerMode::List && !self.hosts.is_empty() {
            self.hosts.remove(self.selected);
            self.selected = self.selected.min(self.hosts.len().saturating_sub(1));
            true
        } else {
            false
        }
    }

    /// Returns true if the modal should close (List mode with Escape).
    pub fn handle_escape(&mut self) -> bool {
        match self.mode {
            ManagerMode::NewHost => {
                self.mode = ManagerMode::List;
                self.focus = 0;
                for f in &mut self.fields {
                    f.clear();
                }
                false
            }
            ManagerMode::List => true,
        }
    }

    fn next_field(&mut self) {
        self.fields[self.focus].active = false;
        self.focus = (self.focus + 1) % self.fields.len();
        self.fields[self.focus].active = true;
    }

    fn connect_selected(&self) -> Option<HostConfig> {
        self.hosts.get(self.selected).cloned()
    }

    fn submit_new_host(&mut self) -> Option<HostConfig> {
        let hostname = self.fields[0].value.trim().to_string();
        if hostname.is_empty() {
            return None;
        }

        let port: u16 = self.fields[1].value.trim().parse().unwrap_or(22);
        let username = self.fields[2].value.trim().to_string();
        let key_path = {
            let s = self.fields[3].value.trim().to_string();
            if s.is_empty() {
                None
            } else {
                Some(s)
            }
        };

        let host = HostConfig {
            name: hostname.clone(),
            hostname,
            port,
            username: if username.is_empty() {
                "root".to_string()
            } else {
                username
            },
            key_path,
            group: None,
            jump_host: None,
        };

        self.hosts.push(host.clone());
        self.mode = ManagerMode::List;
        self.selected = self.hosts.len() - 1;
        Some(host)
    }
}

// ── Rendering ─────────────────────────────────────────────────────────────────

pub fn generate_ssh_manager_commands(
    state: &SshManagerState,
    window_w: f32,
    window_h: f32,
    cell_w: u32,
    cell_h: u32,
) -> Vec<UiCommand> {
    let cw = cell_w as f32;
    let ch = cell_h as f32;
    let mut cmds = Vec::with_capacity(128);

    // Semi-transparent overlay — dark bg covers everything.
    cmds.push(UiCommand::FillRect {
        x: 0.0,
        y: 0.0,
        w: window_w,
        h: window_h,
        color: OVERLAY_DIM,
    });

    // Modal panel
    let modal_w = (window_w * 0.65).clamp(500.0, 800.0);
    let modal_h = (window_h * 0.72).clamp(300.0, 560.0);
    let mx = (window_w - modal_w) * 0.5;
    let my = (window_h - modal_h) * 0.5;

    // Drop shadow behind panel
    cmds.push(chrome::shadow(mx, my, modal_w, modal_h, 8.0));

    // Panel background (rounded)
    cmds.push(UiCommand::FillRoundRect {
        x: mx,
        y: my,
        w: modal_w,
        h: modal_h,
        color: BG_SURFACE,
        bg: BG_BASE,
    });
    // Bottom border only (other edges blend into rounded corners)
    cmds.push(UiCommand::FillRect {
        x: mx, y: my + modal_h - 1.0, w: modal_w, h: 1.0,
        color: BORDER_DEFAULT,
    });

    // Title
    let title = match state.mode {
        ManagerMode::List => "SSH Manager",
        ManagerMode::NewHost => "New SSH Host",
    };
    cmds.push(UiCommand::DrawUiText {
        x: mx + 20.0,
        y: my + 14.0,
        text: title.to_string(),
        fg: TEXT_PRIMARY,
        bg: BG_SURFACE,
    });
    cmds.push(UiCommand::DrawUiText {
        x: mx + modal_w - 8.0 * cw - 20.0,
        y: my + 14.0,
        text: "[Esc] close".to_string(),
        fg: TEXT_MUTED,
        bg: BG_SURFACE,
    });
    // Title underline
    cmds.push(UiCommand::FillRect {
        x: mx,
        y: my + ch + 20.0,
        w: modal_w,
        h: 1.0,
        color: BORDER_DEFAULT,
    });

    let body_y = my + ch + 26.0;

    match state.mode {
        ManagerMode::List => render_list(&mut cmds, state, mx, body_y, modal_w, modal_h, cw, ch),
        ManagerMode::NewHost => render_new_host(&mut cmds, state, mx, body_y, modal_w, cw, ch),
    }

    cmds
}

fn render_list(
    cmds: &mut Vec<UiCommand>,
    state: &SshManagerState,
    mx: f32,
    body_y: f32,
    modal_w: f32,
    modal_h: f32,
    _cw: f32,
    ch: f32,
) {
    let item_h = ch + 10.0;
    let pad_x = 20.0;

    if state.hosts.is_empty() {
        cmds.push(UiCommand::DrawUiText {
            x: mx + pad_x,
            y: body_y + 10.0,
            text: "No saved hosts.  Press N to add one.".to_string(),
            fg: TEXT_MUTED,
            bg: BG_SURFACE,
        });
        return;
    }

    // Max visible rows: modal height minus title area (body_y - my = ch + 26)
    // minus 40px footer for hints, divided by row height.
    let title_area_h = ch + 26.0;
    let footer_h = 40.0;
    let total_h = state.hosts.len() as f32 * item_h;
    let visible_area = modal_h - title_area_h - footer_h;
    let visible = (visible_area / item_h) as usize;
    let start = state.selected.saturating_sub(visible / 2);

    for (i, host) in state.hosts.iter().enumerate().skip(start).take(visible) {
        let iy = body_y + (i - start) as f32 * item_h;
        let selected = i == state.selected;
        let item_bg = if selected { BG_HOVER } else { BG_SURFACE };

        cmds.push(UiCommand::FillRect {
            x: mx + 1.0,
            y: iy,
            w: modal_w - 2.0,
            h: item_h,
            color: item_bg,
        });
        if selected {
            cmds.push(UiCommand::FillRect {
                x: mx + 1.0,
                y: iy,
                w: 3.0,
                h: item_h,
                color: ACCENT_BLUE,
            });
        }

        // Name + address
        let label = format!("{}  —  {}", host.name, host.display_label());
        cmds.push(UiCommand::DrawUiText {
            x: mx + pad_x + 6.0,
            y: iy + (item_h - ch) * 0.5,
            text: label,
            fg: if selected { TEXT_PRIMARY } else { TEXT_MUTED },
            bg: item_bg,
        });
    }

    // Scrollbar
    if total_h > visible_area {
        let sb_w = 6.0;
        let sb_x = mx + modal_w - sb_w - 1.0;
        let sb_h = visible_area;
        // Track
        cmds.push(UiCommand::FillRect {
            x: sb_x, y: body_y, w: sb_w, h: sb_h,
            color: BG_SURFACE,
        });
        // Thumb
        let thumb_h = (visible_area / total_h * visible_area).max(item_h);
        let thumb_y = body_y + (start as f32 / state.hosts.len() as f32) * (sb_h - thumb_h);
        cmds.push(UiCommand::FillRect {
            x: sb_x, y: thumb_y, w: sb_w, h: thumb_h,
            color: TEXT_MUTED,
        });
    }

    // Footer hints
    let footer_y = body_y + visible as f32 * item_h + 8.0;
    cmds.push(UiCommand::DrawUiText {
        x: mx + pad_x,
        y: footer_y,
        text: "[↑↓] navigate   [Enter] connect   [N] new   [D] delete   [Esc] back".to_string(),
        fg: TEXT_MUTED,
        bg: BG_SURFACE,
    });
}

fn render_new_host(
    cmds: &mut Vec<UiCommand>,
    state: &SshManagerState,
    mx: f32,
    body_y: f32,
    modal_w: f32,
    cw: f32,
    ch: f32,
) {
    let field_h = ch + 16.0;
    let pad_x = 20.0;
    let label_w = 22.0 * cw; // fixed label column width

    for (i, field) in state.fields.iter().enumerate() {
        let fy = body_y + i as f32 * (field_h + 8.0);
        let active = i == state.focus;
        let border_color = if active { ACCENT_BLUE } else { BORDER_DEFAULT };

        // Label
        cmds.push(UiCommand::DrawUiText {
            x: mx + pad_x,
            y: fy + (field_h - ch) * 0.5,
            text: format!("{:<22}", field.label),
            fg: TEXT_MUTED,
            bg: BG_SURFACE,
        });

        // Input box
        let box_x = mx + pad_x + label_w;
        let box_w = modal_w - pad_x * 2.0 - label_w;

        cmds.push(UiCommand::FillRect {
            x: box_x,
            y: fy,
            w: box_w,
            h: field_h,
            color: BG_BASE,
        });
        // Border
        for (ox, oy, bw, bh) in [
            (box_x, fy, box_w, 1.0),
            (box_x, fy + field_h - 1.0, box_w, 1.0),
            (box_x, fy, 1.0, field_h),
            (box_x + box_w - 1.0, fy, 1.0, field_h),
        ] {
            cmds.push(UiCommand::FillRect {
                x: ox,
                y: oy,
                w: bw,
                h: bh,
                color: border_color,
            });
        }

        // Value text (+ blinking cursor placeholder)
        let display = if active {
            format!("{}_", field.value)
        } else {
            field.value.clone()
        };
        cmds.push(UiCommand::DrawUiText {
            x: box_x + 8.0,
            y: fy + (field_h - ch) * 0.5,
            text: display,
            fg: TEXT_PRIMARY,
            bg: BG_BASE,
        });
    }

    let hint_y = body_y + state.fields.len() as f32 * (field_h + 8.0) + 12.0;
    cmds.push(UiCommand::DrawUiText {
        x: mx + pad_x,
        y: hint_y,
        text: "[Tab] next field   [Enter] connect   [Esc] back".to_string(),
        fg: TEXT_MUTED,
        bg: BG_SURFACE,
    });
}
