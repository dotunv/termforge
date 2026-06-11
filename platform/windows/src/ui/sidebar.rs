//! Sidebar: the workspace + tools navigator.  Sessions live in the top tab
//! strip now (single home), so the sidebar is dedicated to workspaces, tools,
//! and recent agent/command blocks — rendered as cmux/Warp-style cards with a
//! real type hierarchy (Title / Body / Caption) and system icons.

use libterm::block::store::BlockStatus;
use renderer_windows::icons;
use renderer_windows::ui_renderer::{UiCommand, UiTextStyle};
use renderer_windows::tokens::*;

use super::layout::{ChromeState, Rect};

// ── Shared vertical metrics (render + hit_test MUST agree) ─────────────────────

const PAD_X: f32 = SPACE_3;

#[inline]
fn item_h(ch: f32) -> f32 {
    ch + 14.0
}
#[inline]
fn header_h(ch: f32) -> f32 {
    ch + SPACE_2
}

/// Build all `UiCommand`s for the sidebar area.
pub fn render(s: &ChromeState<'_>) -> Vec<UiCommand> {
    let sb = match s.layout.sidebar {
        Some(r) => r,
        None => return Vec::new(),
    };

    let mut cmds = Vec::with_capacity(256);
    let ch = s.cell_h as f32;
    let item = item_h(ch);
    let hdr = header_h(ch);
    let sx = sb.x;
    let card_x = sx + SPACE_2;
    let card_w = sb.w - SPACE_2 * 2.0 - 1.0;

    // Background + right divider.
    cmds.push(fill(sb, BG_SURFACE));
    cmds.push(UiCommand::FillRect {
        x: sb.x + sb.w - 1.0,
        y: sb.y,
        w: 1.0,
        h: sb.h,
        color: BORDER_SUBTLE,
    });

    let mut sy = sb.y + SPACE_3;

    // ── Brand row ─────────────────────────────────────────────────────────────
    cmds.push(icon(icons::TERMINAL, sx + PAD_X, sy + (item - ch) * 0.5, true, ACCENT_GREEN, BG_SURFACE));
    cmds.push(styled("TermForge", sx + PAD_X + 24.0, sy + (item - ch) * 0.5, UiTextStyle::Title, TEXT_PRIMARY, BG_SURFACE));
    sy += item + SPACE_2;

    // ── WORKSPACES ────────────────────────────────────────────────────────────
    cmds.push(section_header("WORKSPACES", sx + PAD_X, sy + 2.0));
    sy += hdr;

    for (i, name) in s.workspace_names.iter().enumerate() {
        let is_active = i == s.active_workspace;
        let hovered = mouse_in(s, sx, sy, sb.w - 1.0, item);
        card_background(&mut cmds, card_x, sy, card_w, item, is_active, hovered, BLUE);

        let row_bg = card_bg(is_active, hovered);
        let fg = if is_active || hovered { TEXT_PRIMARY } else { TEXT_MUTED };
        let icon_fg = if is_active { BLUE } else { TEXT_MUTED };
        cmds.push(icon(icons::FOLDER, card_x + 12.0, sy + (item - ch) * 0.5, false, icon_fg, row_bg));
        let label_style = if is_active { UiTextStyle::Bold } else { UiTextStyle::Body };
        cmds.push(styled(name, card_x + 36.0, sy + (item - ch) * 0.5, label_style, fg, row_bg));

        // Session-count chip, right-aligned.
        if let Some(&n) = s.workspace_counts.get(i) {
            if n > 0 {
                let txt = n.to_string();
                let cw = txt.chars().count() as f32 * s.ui_char_w + 12.0;
                let cx = card_x + card_w - cw - 10.0;
                let cyy = sy + (item - ch - 2.0) * 0.5;
                cmds.push(UiCommand::FillRoundRect {
                    x: cx, y: cyy, w: cw, h: ch + 2.0, radius: RADIUS_SM,
                    color: BG_ACTIVE, bg: row_bg,
                });
                cmds.push(styled(&txt, cx + 6.0, cyy + 1.0, UiTextStyle::Caption, TEXT_MUTED, BG_ACTIVE));
            }
        }
        sy += item;
    }

    // "New workspace" row.
    let nw_hover = mouse_in(s, sx, sy, sb.w - 1.0, item);
    card_background(&mut cmds, card_x, sy, card_w, item, false, nw_hover, BLUE);
    let nw_bg = card_bg(false, nw_hover);
    let nw_fg = if nw_hover { TEXT_MUTED } else { TEXT_FAINT };
    cmds.push(icon(icons::ADD, card_x + 12.0, sy + (item - ch) * 0.5, false, nw_fg, nw_bg));
    cmds.push(styled("New workspace", card_x + 36.0, sy + (item - ch) * 0.5, UiTextStyle::Body, nw_fg, nw_bg));
    sy += item + SPACE_2;

    // ── TOOLS ─────────────────────────────────────────────────────────────────
    cmds.push(section_header("TOOLS", sx + PAD_X, sy + 2.0));
    sy += hdr;

    let tools: [(&str, &str, &str); 3] = [
        (icons::GLOBE, "SSH manager", "Ctrl+H"),
        (icons::LOCK, "Key vault", ""),
        (icons::SYNC, "Agent runs", "Ctrl+A"),
    ];
    for (gly, label, shortcut) in &tools {
        let hovered = mouse_in(s, sx, sy, sb.w - 1.0, item);
        card_background(&mut cmds, card_x, sy, card_w, item, false, hovered, BLUE);
        let row_bg = card_bg(false, hovered);
        let fg = if hovered { TEXT_PRIMARY } else { TEXT_MUTED };
        cmds.push(icon(gly, card_x + 12.0, sy + (item - ch) * 0.5, false, fg, row_bg));
        cmds.push(styled(label, card_x + 36.0, sy + (item - ch) * 0.5, UiTextStyle::Body, fg, row_bg));
        if hovered && !shortcut.is_empty() {
            let hint_w = shortcut.chars().count() as f32 * s.ui_char_w;
            cmds.push(styled(shortcut, card_x + card_w - hint_w - 12.0, sy + (item - ch) * 0.5, UiTextStyle::Caption, TEXT_FAINT, row_bg));
        } else {
            cmds.push(icon(icons::CHEVRON_RIGHT, card_x + card_w - 22.0, sy + (item - ch) * 0.5, false, TEXT_FAINT, row_bg));
        }
        sy += item;
    }
    sy += SPACE_2;

    // ── RECENT BLOCKS ─────────────────────────────────────────────────────────
    if !s.agent_blocks.is_empty() {
        cmds.push(section_header("RECENT BLOCKS", sx + PAD_X, sy + 2.0));
        sy += hdr;

        for block in s.agent_blocks.iter().rev().take(8) {
            if sy + item > sb.y + sb.h - SPACE_1 {
                break;
            }
            let hovered = mouse_in(s, sx, sy, sb.w - 1.0, item);
            card_background(&mut cmds, card_x, sy, card_w, item, false, hovered, PURPLE);
            let row_bg = card_bg(false, hovered);

            let (dot_col, dur_fallback) = match block.status {
                BlockStatus::Running => (PURPLE, "run"),
                BlockStatus::Success => (GREEN, "ok"),
                BlockStatus::Error => (RED, "err"),
                BlockStatus::Cancelled => (TEXT_MUTED, "---"),
            };
            cmds.push(circle(card_x + 14.0, sy + item * 0.5, 3.0, dot_col, row_bg));

            let max_chars = 15usize;
            let label: String = if block.command.chars().count() > max_chars {
                let cut = block.command.char_indices().nth(max_chars - 1).map(|(i, _)| i).unwrap_or(block.command.len());
                format!("{}\u{2026}", &block.command[..cut])
            } else {
                block.command.clone()
            };
            cmds.push(styled(&label, card_x + 30.0, sy + (item - ch) * 0.5, UiTextStyle::Body, TEXT_PRIMARY, row_bg));

            let dur = block
                .duration_ms()
                .map(|ms| if ms >= 1000 { format!("{:.1}s", ms as f32 / 1000.0) } else { format!("{ms}ms") })
                .unwrap_or_else(|| dur_fallback.to_string());
            let dur_w = dur.chars().count() as f32 * s.ui_char_w;
            cmds.push(styled(&dur, card_x + card_w - dur_w - 10.0, sy + (item - ch) * 0.5, UiTextStyle::Caption, dot_col, row_bg));
            sy += item;
        }
    }

    cmds
}

// ── Hit testing ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarHit {
    Workspace(usize),
    NewWorkspace,
    SshManager,
    KeyVault,
    AgentRuns,
}

/// Mirror of the vertical walk in [`render`].  `workspace_count` must match the
/// number of workspace rows drawn.
pub fn hit_test(sb: Rect, _x: f32, y: f32, cell_h: f32, workspace_count: usize) -> Option<SidebarHit> {
    if y < sb.y || y >= sb.y + sb.h {
        return None;
    }
    let ch = cell_h;
    let item = item_h(ch);
    let hdr = header_h(ch);

    let mut sy = sb.y + SPACE_3;
    // Brand row.
    sy += item + SPACE_2;
    // WORKSPACES header.
    sy += hdr;
    for i in 0..workspace_count {
        if y >= sy && y < sy + item {
            return Some(SidebarHit::Workspace(i));
        }
        sy += item;
    }
    // New workspace.
    if y >= sy && y < sy + item {
        return Some(SidebarHit::NewWorkspace);
    }
    sy += item + SPACE_2;
    // TOOLS header.
    sy += hdr;
    for &hit in &[SidebarHit::SshManager, SidebarHit::KeyVault, SidebarHit::AgentRuns] {
        if y >= sy && y < sy + item {
            return Some(hit);
        }
        sy += item;
    }
    None
}

// ── Private helpers ───────────────────────────────────────────────────────────

#[inline]
fn card_bg(active: bool, hovered: bool) -> [f32; 4] {
    if active {
        BG_ACTIVE
    } else if hovered {
        BG_HOVER
    } else {
        BG_SURFACE
    }
}

/// Paint a row's rounded card background + active accent edge.
fn card_background(cmds: &mut Vec<UiCommand>, x: f32, y: f32, w: f32, h: f32, active: bool, hovered: bool, accent: [f32; 4]) {
    if active || hovered {
        cmds.push(UiCommand::FillRoundRect {
            x, y: y + 2.0, w, h: h - 4.0, radius: RADIUS_SM,
            color: card_bg(active, hovered), bg: BG_SURFACE,
        });
    }
    if active {
        cmds.push(UiCommand::FillRoundRect {
            x, y: y + 6.0, w: 3.0, h: h - 12.0, radius: 1.5,
            color: accent, bg: BG_SURFACE,
        });
    }
}

fn fill(r: Rect, color: [f32; 4]) -> UiCommand {
    UiCommand::FillRect { x: r.x, y: r.y, w: r.w, h: r.h, color }
}

fn circle(cx: f32, cy: f32, r: f32, fg: [f32; 4], bg: [f32; 4]) -> UiCommand {
    UiCommand::DrawCircle { cx, cy, r, fg, bg }
}

fn styled(t: &str, x: f32, y: f32, style: UiTextStyle, fg: [f32; 4], bg: [f32; 4]) -> UiCommand {
    UiCommand::DrawStyledText { x, y, text: t.to_string(), fg, bg, style }
}

fn icon(glyph: &str, x: f32, y: f32, large: bool, fg: [f32; 4], bg: [f32; 4]) -> UiCommand {
    UiCommand::DrawIcon { x, y, text: glyph.to_string(), fg, bg, large }
}

fn section_header(label: &str, x: f32, y: f32) -> UiCommand {
    UiCommand::DrawStyledText { x, y, text: label.to_string(), fg: TEXT_FAINT, bg: BG_SURFACE, style: UiTextStyle::Caption }
}

fn mouse_in(s: &ChromeState<'_>, x: f32, y: f32, w: f32, h: f32) -> bool {
    let (mx, my) = s.mouse_pos;
    mx >= 0.0 && my >= 0.0 && mx >= x && mx < x + w && my >= y && my < y + h
}
