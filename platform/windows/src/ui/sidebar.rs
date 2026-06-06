//! Sidebar rendering and hit-testing.
//!
//! The sidebar contains:
//!   • Workspace label (active workspace name + blue dot)
//!   • Session list (one row per open session, active highlighted)
//!   • WORKSPACES section (one row per workspace slot)
//!   • TOOLS section (SSH manager, key vault, agent runs)
//!   • Agent block history (only when an agent session is active)

use libterm::block::store::BlockStatus;
use libterm::mux::session::{Session, SessionKind};
use renderer_dx12::ui_renderer::{
    hex, UiCommand,
    COL_BADGE_BLUE_BG, COL_BADGE_GREEN_BG, COL_BADGE_PURPLE_BG,
    COL_BLUE, COL_BORDER, COL_FAINT, COL_GREEN, COL_HOVER, COL_MUTED,
    COL_PANEL, COL_PURPLE, COL_RED, COL_TEXT,
};

use super::layout::{ChromeState, Rect};

// ── Public entry point ────────────────────────────────────────────────────────

/// Build all `UiCommand`s for the sidebar area.
/// Returns an empty vec when the sidebar is hidden.
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
    cmds.push(fill(sb, COL_PANEL));
    cmds.push(UiCommand::FillRect {
        x: sb.x + sb.w - 1.0,
        y: sb.y,
        w: 1.0,
        h: sb.h,
        color: hex(COL_BORDER),
    });

    let mut sy = sb.y + 6.0;

    // ── Workspace label row ───────────────────────────────────────────────────
    let ws_name = s.workspace_names.get(s.active_workspace).copied().unwrap_or("TermForge");
    cmds.push(circle(sx + 14.0, sy + item_h * 0.5, 4.0, COL_BLUE, COL_PANEL));
    cmds.push(ui_text(ws_name, sx + text_pad, sy + (item_h - ch) * 0.5, COL_BLUE, COL_PANEL));
    sy += item_h;

    // ── Session list ──────────────────────────────────────────────────────────
    for (i, session) in s.sessions.iter().enumerate() {
        let is_active = i == s.active_tab;
        let item_bg = if is_active { COL_HOVER } else { COL_PANEL };
        let (dot_color, label) = session_dot_info(session);
        let (badge_text, badge_fg, badge_bg) = session_badge(session);

        cmds.push(UiCommand::FillRect {
            x: sx,
            y: sy,
            w: sb.w - 1.0,
            h: item_h,
            color: hex(item_bg),
        });

        if is_active {
            cmds.push(UiCommand::FillRect {
                x: sx,
                y: sy,
                w: 3.0,
                h: item_h,
                color: hex(dot_color),
            });
        }

        cmds.push(circle(sx + 20.0, sy + item_h * 0.5, 3.0, dot_color, item_bg));

        let text_col = if is_active { COL_TEXT } else { COL_MUTED };
        cmds.push(ui_text(label, sx + text_pad, sy + (item_h - ch) * 0.5, text_col, item_bg));

        let badge_w = badge_text.chars().count() as f32 * ucw + 10.0;
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

    for (i, &name) in s.workspace_names.iter().enumerate() {
        let is_active_ws = i == s.active_workspace;
        let item_bg = if is_active_ws { COL_HOVER } else { COL_PANEL };
        let dot_col = if is_active_ws { COL_BLUE } else { COL_MUTED };
        let text_col = if is_active_ws { COL_TEXT } else { COL_MUTED };
        let dot_r = if is_active_ws { 3.0 } else { 2.5 };

        if is_active_ws {
            cmds.push(UiCommand::FillRect {
                x: sx,
                y: sy,
                w: sb.w - 1.0,
                h: item_h,
                color: hex(COL_HOVER),
            });
            cmds.push(UiCommand::FillRect {
                x: sx,
                y: sy,
                w: 3.0,
                h: item_h,
                color: hex(COL_BLUE),
            });
        }

        cmds.push(circle(sx + 14.0, sy + item_h * 0.5, dot_r, dot_col, item_bg));
        cmds.push(ui_text(name, sx + text_pad, sy + (item_h - ch) * 0.5, text_col, item_bg));
        sy += item_h;
    }

    // ── TOOLS section ─────────────────────────────────────────────────────────
    sy += 4.0;
    cmds.push(div_line(sx + 12.0, sy, sb.w - 24.0));
    sy += 6.0;
    cmds.push(section_header("TOOLS", sx + 12.0, sy));
    sy += item_h;

    let tools = [
        "\u{2263} SSH manager",
        "\u{25C6} key vault",
        "\u{25CE} agent runs",
    ];
    for label in &tools {
        cmds.push(circle(sx + 14.0, sy + item_h * 0.5, 2.5, COL_MUTED, COL_PANEL));
        cmds.push(ui_text(label, sx + text_pad, sy + (item_h - ch) * 0.5, COL_MUTED, COL_PANEL));
        sy += item_h;
    }

    // ── Agent block history (active agent tab only) ───────────────────────────
    if !s.agent_blocks.is_empty() {
        sy += 4.0;
        cmds.push(div_line(sx + 12.0, sy, sb.w - 24.0));
        sy += 6.0;
        cmds.push(UiCommand::DrawUiText {
            x: sx + 12.0,
            y: sy,
            text: "RECENT BLOCKS".to_string(),
            fg: hex(COL_PURPLE),
            bg: hex(COL_PANEL),
        });
        sy += item_h;

        for block in s.agent_blocks.iter().rev().take(8) {
            if sy + item_h > sb.y + sb.h - 4.0 {
                break;
            }
            let (dot_col, badge_str) = match block.status {
                BlockStatus::Running => (COL_PURPLE, "run"),
                BlockStatus::Success => (COL_GREEN, "ok"),
                BlockStatus::Error => (COL_RED, "err"),
                BlockStatus::Cancelled => (COL_MUTED, "---"),
            };
            cmds.push(circle(sx + 14.0, sy + item_h * 0.5, 3.0, dot_col, COL_PANEL));

            let max_chars = 16usize;
            let label: String = if block.command.chars().count() > max_chars {
                let cut = block.command.char_indices()
                    .nth(max_chars - 1)
                    .map(|(i, _)| i)
                    .unwrap_or(block.command.len());
                format!("{}\u{2026}", &block.command[..cut])
            } else {
                block.command.clone()
            };
            cmds.push(ui_text(&label, sx + 24.0, sy + (item_h - ch) * 0.5, COL_TEXT, COL_PANEL));

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
            cmds.push(ui_text(&dur_str, dur_x, sy + (item_h - ch) * 0.5, dot_col, COL_PANEL));

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

    // Workspace label row — informational only
    sy += item_h;

    // Session rows
    for i in 0..session_count {
        if y >= sy && y < sy + item_h {
            return Some(SidebarHit::Session(i));
        }
        sy += item_h;
    }

    // Divider + WORKSPACES header + workspace rows
    sy += 4.0 + 6.0 + item_h;
    for i in 0..workspace_count {
        if y >= sy && y < sy + item_h {
            return Some(SidebarHit::Workspace(i));
        }
        sy += item_h;
    }

    // Divider + TOOLS header + tool rows
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

fn session_dot_info(s: &Session) -> (&'static str, &str) {
    match &s.kind {
        SessionKind::Local => (COL_GREEN, s.title.as_str()),
        SessionKind::Ssh { .. } => (COL_BLUE, s.title.as_str()),
        SessionKind::Agent { .. } => (COL_PURPLE, s.title.as_str()),
    }
}

fn session_badge(s: &Session) -> (&'static str, &'static str, &'static str) {
    match &s.kind {
        SessionKind::Local => ("active", COL_GREEN, COL_BADGE_GREEN_BG),
        SessionKind::Ssh { .. } => ("SSH", COL_BLUE, COL_BADGE_BLUE_BG),
        SessionKind::Agent { .. } => ("agent", COL_PURPLE, COL_BADGE_PURPLE_BG),
    }
}

fn fill(r: Rect, color: &str) -> UiCommand {
    UiCommand::FillRect { x: r.x, y: r.y, w: r.w, h: r.h, color: hex(color) }
}

fn circle(cx: f32, cy: f32, r: f32, fg: &str, bg: &str) -> UiCommand {
    UiCommand::DrawCircle { cx, cy, r, fg: hex(fg), bg: hex(bg) }
}

fn round_rect(x: f32, y: f32, w: f32, h: f32, color: &str, bg: &str) -> UiCommand {
    UiCommand::FillRoundRect { x, y, w, h, color: hex(color), bg: hex(bg) }
}

fn ui_text(t: &str, x: f32, y: f32, fg: &str, bg: &str) -> UiCommand {
    UiCommand::DrawUiText { x, y, text: t.to_string(), fg: hex(fg), bg: hex(bg) }
}

fn div_line(x: f32, y: f32, w: f32) -> UiCommand {
    UiCommand::FillRect { x, y, w, h: 0.5, color: hex(COL_BORDER) }
}

fn section_header(label: &str, x: f32, y: f32) -> UiCommand {
    UiCommand::DrawUiText { x, y, text: label.to_string(), fg: hex(COL_FAINT), bg: hex(COL_PANEL) }
}
