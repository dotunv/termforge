//! Agent Launcher — minimal modal (Ctrl+A) to spawn any command as a purple
//! agent session.
//!
//! The user types a shell command (e.g. `claude`, `python run_agent.py`) and
//! presses Enter.  The caller receives the command string, spawns a ConPTY
//! entry tagged as `SessionKind::Agent`, and closes the modal.

use renderer_dx12::ui_renderer::{hex, UiCommand,
    COL_BG, COL_PANEL, COL_BORDER, COL_TEXT, COL_MUTED, COL_PURPLE};

// ── State ─────────────────────────────────────────────────────────────────────

pub struct AgentLauncherState {
    pub open:    bool,
    /// The command the user is typing.
    pub command: String,
    /// Model label shown in UI (user can type it after a space, or leave blank).
    pub model:   String,
}

impl AgentLauncherState {
    pub fn new() -> Self {
        Self {
            open:    false,
            command: String::new(),
            model:   String::new(),
        }
    }

    /// Feed a character from WM_CHAR.
    /// Returns `Some((command, model))` when the user commits with Enter.
    pub fn handle_char(&mut self, c: char) -> Option<(String, String)> {
        match c {
            '\r' | '\n' => {
                let raw = self.command.trim().to_string();
                if raw.is_empty() { return None; }
                // First word = command, remainder = model hint.
                let mut parts = raw.splitn(2, ' ');
                let cmd   = parts.next().unwrap_or("").to_string();
                let model = parts.next().unwrap_or("").trim().to_string();
                self.command.clear();
                self.model.clear();
                self.open = false;
                Some((cmd, model))
            }
            '\x08' | '\x7f' => { self.command.pop(); None }
            _ if !c.is_control() => { self.command.push(c); None }
            _ => None,
        }
    }

    pub fn handle_escape(&mut self) {
        self.command.clear();
        self.open = false;
    }
}

// ── Rendering ─────────────────────────────────────────────────────────────────

pub fn generate_agent_launcher_commands(
    state: &AgentLauncherState,
    window_w: f32,
    window_h: f32,
    cell_w:   u32,
    cell_h:   u32,
) -> Vec<UiCommand> {
    if !state.open { return vec![]; }

    let cw = cell_w as f32;
    let ch = cell_h as f32;
    let mut cmds = Vec::with_capacity(32);

    // Dark overlay
    cmds.push(UiCommand::FillRect {
        x: 0.0, y: 0.0, w: window_w, h: window_h,
        color: [0.04, 0.07, 0.09, 0.88],
    });

    // Panel — narrow and centred, command-palette style.
    let panel_w = (window_w * 0.55).max(460.0).min(680.0);
    let panel_h = ch * 3.0 + 40.0; // header + input row + hint
    let px = (window_w - panel_w) * 0.5;
    let py = window_h * 0.30;

    cmds.push(UiCommand::FillRect { x: px, y: py, w: panel_w, h: panel_h, color: hex(COL_PANEL) });
    // Purple top accent line
    cmds.push(UiCommand::FillRect { x: px, y: py, w: panel_w, h: 2.0, color: hex(COL_PURPLE) });
    // Border
    for (ox, oy, bw, bh) in [
        (px,              py,              panel_w, 1.0),
        (px,              py + panel_h - 1.0, panel_w, 1.0),
        (px,              py,              1.0, panel_h),
        (px + panel_w - 1.0, py,           1.0, panel_h),
    ] {
        cmds.push(UiCommand::FillRect { x: ox, y: oy, w: bw, h: bh, color: hex(COL_BORDER) });
    }

    // Header row
    let header_y = py + 10.0;
    cmds.push(UiCommand::DrawText {
        x: px + 14.0, y: header_y,
        text: "▸ Launch Agent".to_string(),
        fg: hex(COL_PURPLE), bg: hex(COL_PANEL),
    });
    cmds.push(UiCommand::DrawText {
        x: px + panel_w - 9.0 * cw - 14.0, y: header_y,
        text: "[Esc] cancel".to_string(),
        fg: hex(COL_MUTED), bg: hex(COL_PANEL),
    });

    // Input row
    let input_y = py + ch + 20.0;
    let prompt = "$ ";
    let prompt_w = prompt.len() as f32 * cw;

    cmds.push(UiCommand::FillRect {
        x: px + 1.0, y: input_y - 4.0,
        w: panel_w - 2.0, h: ch + 8.0,
        color: hex(COL_BG),
    });
    cmds.push(UiCommand::DrawText {
        x: px + 14.0, y: input_y,
        text: prompt.to_string(),
        fg: hex(COL_PURPLE), bg: hex(COL_BG),
    });
    let display = format!("{}_", state.command);
    cmds.push(UiCommand::DrawText {
        x: px + 14.0 + prompt_w, y: input_y,
        text: display,
        fg: hex(COL_TEXT), bg: hex(COL_BG),
    });

    // Hint row
    let hint_y = py + ch * 2.0 + 30.0;
    cmds.push(UiCommand::DrawText {
        x: px + 14.0, y: hint_y,
        text: "Enter command (e.g. claude, python agent.py).  [Enter] launch".to_string(),
        fg: hex(COL_MUTED), bg: hex(COL_PANEL),
    });

    cmds
}
