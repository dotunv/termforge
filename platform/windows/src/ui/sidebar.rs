use libterm::block::store::BlockStatus;
use libterm::mux::session::{Session, SessionKind};
use renderer_windows::ui_renderer::UiCommand;
use renderer_windows::tokens::*;

use super::layout::{ChromeState, Rect};

/// Build all `UiCommand`s for the sidebar area.
pub fn render(s: &ChromeState<'_>) -> Vec<UiCommand> {
    let sb = match s.layout.sidebar {
        Some(r) => r,
        None => return Vec::new(),
    };

    let mut cmds = Vec::with_capacity(256);
    let ch = s.cell_h as f32;
    let ucw = s.ui_char_w;
    let item_h = ch + 10.0;
    let text_pad = 32.0;
    let sx = sb.x;

    // Background + right border
    cmds.push(fill(sb, BG_SURFACE));
    cmds.push(UiCommand::FillRect {
        x: sb.x + sb.w - 1.0,
        y: sb.y,
        w: 1.0,
        h: sb.h,
        color: BORDER_DEFAULT,
    });

    let mut sy = sb.y + 6.0;

    // ── SESSIONS section ──────────────────────────────────────────────────────
    sy += 6.0;
    cmds.push(section_header("SESSIONS", sx + 12.0, sy));
    sy += item_h;

        let labels =
            super::chrome::display_labels(s.sessions.iter().map(|s| s.title.as_str()));
        for (i, session) in s.sessions.iter().enumerate() {
            let is_active = i == s.active_tab;
            let is_hovered = mouse_in(s, sx, sy, sb.w - 1.0, item_h);
            let item_bg = if is_active { BG_ACTIVE } else if is_hovered { BG_HOVER } else { BG_SURFACE };
            let (dot_color, _) = session_dot_info(session);
            let label = labels[i].as_str();
            let (badge_text, badge_fg, badge_bg) = session_badge(session);

            cmds.push(UiCommand::FillRect {
                x: sx,
                y: sy,
                w: sb.w - 1.0,
                h: item_h,
                color: item_bg,
            });

            if is_active {
                cmds.push(UiCommand::FillRect {
                    x: sx,
                    y: sy,
                    w: 3.0,
                    h: item_h,
                    color: dot_color,
                });
            }

            cmds.push(circle(sx + 20.0, sy + item_h * 0.5, 3.0, dot_color, item_bg));

            let text_col = if is_active || is_hovered { TEXT_PRIMARY } else { TEXT_MUTED };
            cmds.push(ui_text(label, sx + text_pad, sy + (item_h - ch) * 0.5, text_col, item_bg));

            let badge_w = s.badge_widths.get(i).copied().unwrap_or(60.0);
            let badge_x = sx + sb.w - 1.0 - badge_w - 8.0;
            let badge_y = sy + (item_h - ch - 4.0) * 0.5;
            cmds.push(round_rect(badge_x, badge_y, badge_w, ch + 4.0, badge_bg, item_bg));
            cmds.push(ui_text(badge_text, badge_x + 5.0, badge_y + 2.0, badge_fg, badge_bg));

            sy += item_h;
        }

    // ── WORKSPACES section ────────────────────────────────────────────────────
    sy += 4.0;
    cmds.push(div_line(sx + 12.0, sy, sb.w - 24.0));
    sy += 6.0;
    cmds.push(section_header("WORKSPACES", sx + 12.0, sy));
    sy += item_h;

    for (i, name) in s.workspace_names.iter().enumerate() {
        let is_active_ws = i == s.active_workspace;
        let is_hovered = mouse_in(s, sx, sy, sb.w - 1.0, item_h);
        let item_bg = if is_active_ws { BG_ACTIVE } else if is_hovered { BG_HOVER } else { BG_SURFACE };
        let dot_col = if is_active_ws || is_hovered { BLUE } else { TEXT_MUTED };
        let text_col = if is_active_ws || is_hovered { TEXT_PRIMARY } else { TEXT_MUTED };
        let dot_r = if is_active_ws { 3.0 } else { 2.5 };

        if is_active_ws {
            cmds.push(UiCommand::FillRect {
                x: sx,
                y: sy,
                w: sb.w - 1.0,
                h: item_h,
                color: BG_ACTIVE,
            });
            cmds.push(UiCommand::FillRect {
                x: sx,
                y: sy,
                w: 3.0,
                h: item_h,
                color: BLUE,
            });
        } else if is_hovered {
            cmds.push(UiCommand::FillRect {
                x: sx,
                y: sy,
                w: sb.w - 1.0,
                h: item_h,
                color: BG_HOVER,
            });
        }

        cmds.push(circle(sx + 14.0, sy + item_h * 0.5, dot_r, dot_col, item_bg));
        cmds.push(ui_text(name, sx + text_pad, sy + (item_h - ch) * 0.5, text_col, item_bg));
        sy += item_h;
    }

    // "New workspace" row
    let plus_hover = mouse_in(s, sx, sy, sb.w - 1.0, item_h);
    if plus_hover {
        cmds.push(UiCommand::FillRect {
            x: sx, y: sy, w: sb.w - 1.0, h: item_h,
            color: BG_HOVER,
        });
    }
    let plus_bg = if plus_hover { BG_HOVER } else { BG_SURFACE };
    let plus_y = sy + (item_h - ch) * 0.5;
    cmds.push(ui_text("+", sx + 13.0, plus_y, TEXT_FAINT, plus_bg));
    cmds.push(ui_text("New workspace", sx + text_pad, plus_y, TEXT_FAINT, plus_bg));
    sy += item_h;

    // ── TOOLS section ─────────────────────────────────────────────────────────
    sy += 4.0;
    cmds.push(div_line(sx + 12.0, sy, sb.w - 24.0));
    sy += 6.0;
    cmds.push(section_header("TOOLS", sx + 12.0, sy));
    sy += item_h;

    // Label + its keybinding, shown right-aligned on hover so shortcuts are
    // discoverable in place (Linear-style).
    let tools: [(&str, &str); 3] = [
        ("SSH manager", "Ctrl+H"),
        ("key vault", ""),
        ("agent runs", "Ctrl+A"),
    ];
    for (label, shortcut) in &tools {
        let tool_hover = mouse_in(s, sx, sy, sb.w - 1.0, item_h);
        let tool_bg = if tool_hover { BG_HOVER } else { BG_SURFACE };
        if tool_hover {
            cmds.push(UiCommand::FillRect {
                x: sx, y: sy, w: sb.w - 1.0, h: item_h,
                color: BG_HOVER,
            });
        }
        let fg = if tool_hover { TEXT_PRIMARY } else { TEXT_MUTED };
        cmds.push(circle(sx + 14.0, sy + item_h * 0.5, 2.5, fg, tool_bg));
        cmds.push(ui_text(label, sx + text_pad, sy + (item_h - ch) * 0.5, fg, tool_bg));
        if tool_hover && !shortcut.is_empty() {
            let hint_w = shortcut.chars().count() as f32 * ucw;
            cmds.push(ui_text(
                shortcut,
                sx + sb.w - 1.0 - hint_w - 10.0,
                sy + (item_h - ch) * 0.5,
                TEXT_FAINT,
                tool_bg,
            ));
        } else {
            cmds.push(ui_text(
                "\u{203A}",
                sx + sb.w - 1.0 - ucw - 10.0,
                sy + (item_h - ch) * 0.5,
                TEXT_FAINT,
                tool_bg,
            ));
        }
        sy += item_h;
    }

    // ── Agent block history ──────────────────────────────────────────────────
    if !s.agent_blocks.is_empty() {
        sy += 4.0;
        cmds.push(div_line(sx + 12.0, sy, sb.w - 24.0));
        sy += 6.0;
        cmds.push(UiCommand::DrawUiText {
            x: sx + 12.0,
            y: sy,
            text: "RECENT BLOCKS".to_string(),
            fg: PURPLE,
            bg: BG_SURFACE,
        });
        sy += item_h;

        for block in s.agent_blocks.iter().rev().take(8) {
            if sy + item_h > sb.y + sb.h - 4.0 {
                break;
            }
            let block_hover = mouse_in(s, sx, sy, sb.w - 1.0, item_h);
            let block_bg = if block_hover { BG_HOVER } else { BG_SURFACE };
            if block_hover {
                cmds.push(UiCommand::FillRect {
                    x: sx, y: sy, w: sb.w - 1.0, h: item_h,
                    color: BG_HOVER,
                });
            }
            let (dot_col, badge_str) = match block.status {
                BlockStatus::Running => (PURPLE, "run"),
                BlockStatus::Success => (GREEN, "ok"),
                BlockStatus::Error => (RED, "err"),
                BlockStatus::Cancelled => (TEXT_MUTED, "---"),
            };
            cmds.push(circle(sx + 14.0, sy + item_h * 0.5, 3.0, dot_col, block_bg));

            let max_chars = 15usize;
            let label: String = if block.command.chars().count() > max_chars {
                let cut = block.command.char_indices()
                    .nth(max_chars - 1)
                    .map(|(i, _)| i)
                    .unwrap_or(block.command.len());
                format!("{}\u{2026}", &block.command[..cut])
            } else {
                block.command.clone()
            };
            cmds.push(ui_text(&label, sx + text_pad, sy + (item_h - ch) * 0.5, TEXT_PRIMARY, block_bg));

            let dur_str = block
                .duration_ms()
                .map(|ms| {
                    if ms >= 1000 {
                        format!("{:.1}s", ms as f32 / 1000.0)
                    } else {
                        format!("{ms}ms")
                    }
                })
                .unwrap_or_else(|| badge_str.to_string());
            let dur_w = dur_str.chars().count() as f32 * ucw;
            let dur_x = sx + sb.w - 1.0 - dur_w - 8.0;
            cmds.push(ui_text(&dur_str, dur_x, sy + (item_h - ch) * 0.5, dot_col, block_bg));

            sy += item_h;
        }
    }

    cmds
}

// ── Hit testing ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarHit {
    Session(usize),
    Workspace(usize),
    NewWorkspace,
    SshManager,
    KeyVault,
    AgentRuns,
}

pub fn hit_test(
    sb: Rect,
    _x: f32,
    y: f32,
    session_count: usize,
    cell_h: f32,
    workspace_count: usize,
) -> Option<SidebarHit> {
    if y < sb.y || y >= sb.y + sb.h {
        return None;
    }
    let item_h = cell_h + 10.0;
    let mut sy = sb.y + 6.0;

    sy += 6.0 + item_h;

    for i in 0..session_count {
        if y >= sy && y < sy + item_h {
            return Some(SidebarHit::Session(i));
        }
        sy += item_h;
    }

    sy += 4.0 + 6.0 + item_h;
    for i in 0..workspace_count {
        if y >= sy && y < sy + item_h {
            return Some(SidebarHit::Workspace(i));
        }
        sy += item_h;
    }

    if y >= sy && y < sy + item_h {
        return Some(SidebarHit::NewWorkspace);
    }
    sy += item_h;

    sy += 4.0 + 6.0 + item_h;
    for &hit in &[SidebarHit::SshManager, SidebarHit::KeyVault, SidebarHit::AgentRuns] {
        if y >= sy && y < sy + item_h {
            return Some(hit);
        }
        sy += item_h;
    }

    None
}

// ── Private helpers ───────────────────────────────────────────────────────────

fn session_dot_info(s: &Session) -> ([f32; 4], &str) {
    match &s.kind {
        SessionKind::Local => (COLOR_LOCAL, s.title.as_str()),
        SessionKind::Ssh { .. } => (COLOR_SSH, s.title.as_str()),
        SessionKind::Agent { .. } => (COLOR_AGENT, s.title.as_str()),
    }
}

fn session_badge(s: &Session) -> (&str, [f32; 4], [f32; 4]) {
    match &s.kind {
        SessionKind::Local => ("active", GREEN, BADGE_GREEN_BG),
        SessionKind::Ssh { .. } => ("SSH", BLUE, BADGE_BLUE_BG),
        SessionKind::Agent { .. } => ("agent", PURPLE, BADGE_PURPLE_BG),
    }
}

fn fill(r: Rect, color: [f32; 4]) -> UiCommand {
    UiCommand::FillRect { x: r.x, y: r.y, w: r.w, h: r.h, color }
}

fn circle(cx: f32, cy: f32, r: f32, fg: [f32; 4], bg: [f32; 4]) -> UiCommand {
    UiCommand::DrawCircle { cx, cy, r, fg, bg }
}

fn round_rect(x: f32, y: f32, w: f32, h: f32, color: [f32; 4], bg: [f32; 4]) -> UiCommand {
    UiCommand::FillRoundRect { x, y, w, h, radius: RADIUS_SM, color, bg }
}

fn ui_text(t: &str, x: f32, y: f32, fg: [f32; 4], bg: [f32; 4]) -> UiCommand {
    UiCommand::DrawUiText { x, y, text: t.to_string(), fg, bg }
}

fn div_line(x: f32, y: f32, w: f32) -> UiCommand {
    UiCommand::FillRect { x, y, w, h: 1.0, color: BORDER_SUBTLE }
}

fn section_header(label: &str, x: f32, y: f32) -> UiCommand {
    UiCommand::DrawUiText { x, y, text: label.to_string(), fg: TEXT_FAINT, bg: BG_SURFACE }
}

fn mouse_in(s: &ChromeState<'_>, x: f32, y: f32, w: f32, h: f32) -> bool {
    let (mx, my) = s.mouse_pos;
    mx >= 0.0 && my >= 0.0 && mx >= x && mx < x + w && my >= y && my < y + h
}
