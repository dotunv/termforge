use renderer_windows::ui_renderer::UiCommand;
use renderer_windows::tokens::*;

use super::chrome;

pub struct AgentLauncherState {
    pub command: String,
    pub model: String,
}

impl AgentLauncherState {
    pub fn new() -> Self {
        Self { command: String::new(), model: String::new() }
    }

    pub fn handle_char(&mut self, c: char) -> Option<(String, String)> {
        match c {
            '\r' | '\n' => {
                let raw = self.command.trim().to_string();
                if raw.is_empty() { return None; }
                let mut parts = raw.splitn(2, ' ');
                let cmd = parts.next().unwrap_or("").to_string();
                let model = parts.next().unwrap_or("").trim().to_string();
                self.command.clear();
                self.model.clear();
                Some((cmd, model))
            }
            '\x08' | '\x7f' => { self.command.pop(); None }
            _ if !c.is_control() => { self.command.push(c); None }
            _ => None,
        }
    }

    pub fn handle_escape(&mut self) -> bool {
        self.command.clear();
        true
    }
}

pub fn generate_agent_launcher_commands(
    state: &AgentLauncherState,
    window_w: f32,
    window_h: f32,
    cell_w: u32,
    cell_h: u32,
) -> Vec<UiCommand> {
    let cw = cell_w as f32;
    let ch = cell_h as f32;
    let mut cmds = Vec::with_capacity(32);

    cmds.push(UiCommand::FillRect {
        x: 0.0, y: 0.0, w: window_w, h: window_h,
        color: OVERLAY_DIM,
    });

    let panel_w = (window_w * 0.55).clamp(460.0, 680.0);
    let panel_h = ch * 3.0 + 40.0;
    let px = (window_w - panel_w) * 0.5;
    let py = window_h * 0.30;

    cmds.push(chrome::shadow(px, py, panel_w, panel_h, 8.0));

    cmds.push(UiCommand::FillRoundRect {
        x: px, y: py, w: panel_w, h: panel_h, radius: RADIUS_LG,
        color: BG_SURFACE,
        bg: BG_BASE,
    });
    cmds.push(UiCommand::FillRect {
        x: px + 8.0, y: py, w: panel_w - 16.0, h: 2.0,
        color: PURPLE,
    });
    cmds.push(UiCommand::FillRect {
        x: px, y: py + panel_h - 1.0, w: panel_w, h: 1.0,
        color: BORDER_DEFAULT,
    });

    let header_y = py + 10.0;
    cmds.push(UiCommand::DrawUiText {
        x: px + 14.0, y: header_y,
        text: "\u{25B8} Launch Agent".to_string(),
        fg: PURPLE,
        bg: BG_SURFACE,
    });
    cmds.push(UiCommand::DrawUiText {
        x: px + panel_w - 9.0 * cw - 14.0, y: header_y,
        text: "[Esc] cancel".to_string(),
        fg: TEXT_MUTED,
        bg: BG_SURFACE,
    });

    let input_y = py + ch + 20.0;
    let prompt = "$ ";
    let prompt_w = prompt.len() as f32 * cw;

    cmds.push(UiCommand::FillRect {
        x: px + 1.0, y: input_y - 4.0,
        w: panel_w - 2.0, h: ch + 8.0,
        color: BG_BASE,
    });
    cmds.push(UiCommand::DrawText {
        x: px + 14.0, y: input_y,
        text: prompt.to_string(),
        fg: PURPLE,
        bg: BG_BASE,
    });
    let display = format!("{}_", state.command);
    cmds.push(UiCommand::DrawText {
        x: px + 14.0 + prompt_w, y: input_y,
        text: display,
        fg: TEXT_PRIMARY,
        bg: BG_BASE,
    });

    // The "$ command" input rows above stay in the terminal monospace font on
    // purpose — they echo what will run in the shell.  Chrome text is UI font.
    let hint_y = py + ch * 2.0 + 30.0;
    cmds.push(UiCommand::DrawUiText {
        x: px + 14.0, y: hint_y,
        text: "Enter command (e.g. claude, python agent.py).  [Enter] launch".to_string(),
        fg: TEXT_MUTED,
        bg: BG_SURFACE,
    });

    cmds
}
