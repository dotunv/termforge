//! Chrome layout: titlebar, tab bar, sidebar, content area, status bar.
//!
//! All dimensions are in window pixels.  `generate_commands` returns a flat
//! list of [`UiCommand`]s describing every chrome element for the current
//! frame.  The compositor draws them in one pass before the terminal panes.

use libterm::block::store::{BlockStatus, CommandBlock};
use libterm::mux::session::{Session, SessionKind};
use renderer_dx12::ui_renderer::{
    hex, UiCommand, COL_AMBER, COL_BG, COL_BLUE, COL_BORDER, COL_FAINT, COL_GREEN, COL_HOVER,
    COL_MUTED, COL_PANEL, COL_PURPLE, COL_RED, COL_TEXT,
};

// ── Fixed chrome heights / widths ────────────────────────────────────────────
pub const TITLEBAR_H: f32 = 36.0;
pub const TABBAR_H: f32 = 32.0;
pub const SIDEBAR_W: f32 = 200.0; // per design doc Section 14.4
pub const STATUSBAR_H: f32 = 22.0;

pub const PANE_HEADER_H: f32 = 24.0; // per-pane CWD/host header
pub const SPLIT_HANDLE_W: f32 = 4.0; // draggable split handle width

/// Axis-aligned pixel rect.
#[derive(Debug, Clone, Copy)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

/// Computed rects for one frame.
#[derive(Debug, Clone)]
pub struct ChromeLayout {
    pub titlebar: Rect,
    pub tabbar: Rect,
    pub sidebar: Option<Rect>, // None when collapsed
    pub content: Rect,         // terminal pane area
    pub statusbar: Rect,
    pub window_w: f32,
    pub window_h: f32,
}

impl ChromeLayout {
    pub fn compute(window_w: f32, window_h: f32, sidebar_visible: bool) -> Self {
        let titlebar = Rect {
            x: 0.0,
            y: 0.0,
            w: window_w,
            h: TITLEBAR_H,
        };
        let tabbar = Rect {
            x: 0.0,
            y: TITLEBAR_H,
            w: window_w,
            h: TABBAR_H,
        };
        let statusbar = Rect {
            x: 0.0,
            y: window_h - STATUSBAR_H,
            w: window_w,
            h: STATUSBAR_H,
        };

        let body_top = TITLEBAR_H + TABBAR_H;
        let body_h = window_h - body_top - STATUSBAR_H;

        let (sidebar, content) = if sidebar_visible {
            let sb = Rect {
                x: 0.0,
                y: body_top,
                w: SIDEBAR_W,
                h: body_h,
            };
            let ct = Rect {
                x: SIDEBAR_W,
                y: body_top,
                w: window_w - SIDEBAR_W,
                h: body_h,
            };
            (Some(sb), ct)
        } else {
            (
                None,
                Rect {
                    x: 0.0,
                    y: body_top,
                    w: window_w,
                    h: body_h,
                },
            )
        };

        Self {
            titlebar,
            tabbar,
            sidebar,
            content,
            statusbar,
            window_w,
            window_h,
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Command generation
// ────────────────────────────────────────────────────────────────────────────

pub struct ChromeState<'a> {
    pub layout: &'a ChromeLayout,
    pub sessions: &'a [Session],
    pub active_tab: usize,
    pub workspace_name: &'a str,
    pub sidebar_visible: bool,
    /// Pixel x of each split handle (for drag detection).
    pub split_handles: &'a [f32],
    /// Active session error count (for status bar amber warning).
    pub error_count: usize,
    /// Number of blocks across all sessions currently awaiting user input (OSC 9001).
    pub awaiting_count: usize,
    /// Cell dimensions from atlas (used for text vertical centring).
    pub cell_w: u32,
    pub cell_h: u32,
    /// Recent blocks of the active agent session (empty for non-agent sessions).
    pub agent_blocks: &'a [CommandBlock],
    /// Per-pane rects (x, y, w, h) and their session indices — for pane headers.
    pub pane_rects: &'a [(f32, f32, f32, f32, usize)],
}

pub fn generate_commands(s: &ChromeState<'_>) -> Vec<UiCommand> {
    let mut cmds = Vec::with_capacity(256);
    let cw = s.cell_w as f32;
    let ch = s.cell_h as f32;

    // ── Title bar ─────────────────────────────────────────────────────────
    let tb = s.layout.titlebar;
    cmds.push(fill(tb, COL_PANEL));
    cmds.push(bottom_border(tb, COL_BORDER));

    // Windows titlebar buttons: ─ □ × (right-aligned, 46px each)
    let btn_w: f32 = 46.0;
    let btn_count: f32 = 3.0;
    let btn_x_start = tb.x + tb.w - btn_w * btn_count;
    let btn_text_y = tb.y + (tb.h - ch) * 0.5;
    for (i, (label, hover_color)) in [("─", COL_MUTED), ("□", COL_MUTED), ("×", "#f85149")]
        .iter()
        .enumerate()
    {
        let bx = btn_x_start + i as f32 * btn_w;
        // Button text centered in its 46px region
        let label_w = label.chars().count() as f32 * cw;
        cmds.push(text(
            label,
            bx + (btn_w - label_w) * 0.5,
            btn_text_y,
            hover_color,
            COL_PANEL,
        ));
    }

    // Workspace name centred in titlebar
    let title = format!("TermForge — {}", s.workspace_name);
    let title_x = tb.x + (tb.w - title.len() as f32 * cw) * 0.5;
    let title_y = tb.y + (tb.h - ch) * 0.5;
    cmds.push(text(&title, title_x, title_y, COL_MUTED, COL_PANEL));

    // ── Tab bar ───────────────────────────────────────────────────────────
    let tabbar = s.layout.tabbar;
    cmds.push(fill(tabbar, COL_PANEL));
    cmds.push(bottom_border(tabbar, COL_BORDER));

    let mut tab_x = tabbar.x + 8.0;
    let tab_text_y = tabbar.y + (tabbar.h - ch) * 0.5;
    let tab_dot_y = tabbar.y + tabbar.h * 0.5;

    for (i, session) in s.sessions.iter().enumerate() {
        let is_active = i == s.active_tab;
        let tab_bg = if is_active { COL_BG } else { COL_PANEL };
        let (dot_color, label) = session_tab_info(session);

        let label_chars = label.chars().count() as f32;
        let tab_w = 10.0 + 6.0 + 4.0 + label_chars * cw + 10.0; // pad + dot + gap + text + pad

        // Tab background
        cmds.push(UiCommand::FillRect {
            x: tab_x,
            y: tabbar.y + 4.0,
            w: tab_w,
            h: tabbar.h - 8.0,
            color: hex(tab_bg),
        });

        // Coloured dot
        cmds.push(dot(tab_x + 10.0 + 3.0, tab_dot_y, 3.0, dot_color));

        // Label
        cmds.push(text(
            label,
            tab_x + 10.0 + 6.0 + 4.0,
            tab_text_y,
            if is_active { COL_TEXT } else { COL_MUTED },
            tab_bg,
        ));

        tab_x += tab_w + 2.0;
    }

    // "+" new-tab button
    cmds.push(text("+", tab_x + 4.0, tab_text_y, COL_MUTED, COL_PANEL));

    // ── Sidebar ───────────────────────────────────────────────────────────
    if let Some(sb) = s.layout.sidebar {
        cmds.push(fill(sb, COL_PANEL));
        // Right border
        cmds.push(UiCommand::FillRect {
            x: sb.x + sb.w - 1.0,
            y: sb.y,
            w: 1.0,
            h: sb.h,
            color: hex(COL_BORDER),
        });

        let mut sy = sb.y + 6.0;
        let sx = sb.x;
        let item_h = ch + 10.0;

        // Workspace label
        cmds.push(dot(sx + 12.0, sy + ch * 0.5, 6.0, COL_BLUE));
        cmds.push(text(s.workspace_name, sx + 24.0, sy, COL_BLUE, COL_PANEL));
        sy += item_h;

        // Session list items
        for (i, session) in s.sessions.iter().enumerate() {
            let is_active = i == s.active_tab;
            let item_bg = if is_active { COL_HOVER } else { COL_PANEL };
            let (dot_color, label) = session_tab_info(session);

            cmds.push(UiCommand::FillRect {
                x: sx,
                y: sy,
                w: sb.w - 1.0,
                h: item_h,
                color: hex(item_bg),
            });

            // Active item: 3px left accent bar in session color.
            if is_active {
                cmds.push(UiCommand::FillRect {
                    x: sx,
                    y: sy,
                    w: 3.0,
                    h: item_h,
                    color: hex(dot_color),
                });
            }

            // Status dot (offset past accent bar).
            cmds.push(dot(sx + 22.0, sy + item_h * 0.5, 3.5, dot_color));

            let text_color = if is_active { COL_TEXT } else { COL_MUTED };
            cmds.push(text(
                label,
                sx + 32.0,
                sy + (item_h - ch) * 0.5,
                text_color,
                item_bg,
            ));

            sy += item_h;
        }

        // Divider + WORKSPACES section header
        sy += 4.0;
        cmds.push(UiCommand::FillRect {
            x: sx + 12.0,
            y: sy,
            w: sb.w - 24.0,
            h: 1.0,
            color: hex(COL_BORDER),
        });
        sy += 8.0;
        cmds.push(text("WORKSPACES", sx + 12.0, sy, COL_FAINT, COL_PANEL));
        sy += item_h;

        // Static workspace items (Phase 3 — real workspaces in Phase 4)
        for label in &["work / backend", "infra / servers", "personal"] {
            cmds.push(dot(sx + 12.0, sy + item_h * 0.5, 4.0, COL_MUTED));
            cmds.push(text(
                label,
                sx + 22.0,
                sy + (item_h - ch) * 0.5,
                COL_MUTED,
                COL_PANEL,
            ));
            sy += item_h;
        }

        // Divider + TOOLS section header
        sy += 4.0;
        cmds.push(UiCommand::FillRect {
            x: sx + 12.0,
            y: sy,
            w: sb.w - 24.0,
            h: 1.0,
            color: hex(COL_BORDER),
        });
        sy += 8.0;
        cmds.push(text("TOOLS", sx + 12.0, sy, COL_FAINT, COL_PANEL));
        sy += item_h;

        for label in &["SSH manager", "key vault", "agent runs"] {
            cmds.push(dot(sx + 12.0, sy + item_h * 0.5, 4.0, COL_MUTED));
            cmds.push(text(
                label,
                sx + 22.0,
                sy + (item_h - ch) * 0.5,
                COL_MUTED,
                COL_PANEL,
            ));
            sy += item_h;
        }

        // ── Agent block history (shown only when active tab is an Agent) ──────
        if !s.agent_blocks.is_empty() {
            // Divider
            cmds.push(UiCommand::FillRect {
                x: sx + 12.0,
                y: sy + 2.0,
                w: sb.w - 24.0,
                h: 1.0,
                color: hex(COL_BORDER),
            });
            sy += 6.0;

            cmds.push(text("RECENT BLOCKS", sx + 12.0, sy, COL_PURPLE, COL_PANEL));
            sy += item_h;

            for block in s.agent_blocks.iter().rev().take(8) {
                let (dot_col, badge) = match block.status {
                    BlockStatus::Running => (COL_PURPLE, "run"),
                    BlockStatus::Success => (COL_GREEN, "ok "),
                    BlockStatus::Error => (COL_RED, "err"),
                    BlockStatus::Cancelled => (COL_MUTED, "---"),
                };
                let item_bg = COL_PANEL;

                cmds.push(dot(sx + 14.0, sy + item_h * 0.5, 3.5, dot_col));

                // Truncate command to fit sidebar width (~18 chars)
                let cmd = &block.command;
                let max_chars = 18usize;
                let label: String = if cmd.chars().count() > max_chars {
                    format!(
                        "{}…",
                        &cmd[..cmd
                            .char_indices()
                            .nth(max_chars - 1)
                            .map(|(i, _)| i)
                            .unwrap_or(cmd.len())]
                    )
                } else {
                    cmd.clone()
                };
                cmds.push(text(
                    &label,
                    sx + 24.0,
                    sy + (item_h - ch) * 0.5,
                    COL_TEXT,
                    item_bg,
                ));

                // Right-align badge + optional duration
                let dur_str = if let Some(ms) = block.duration_ms() {
                    if ms >= 1000 {
                        format!("{:.1}s", ms as f32 / 1000.0)
                    } else {
                        format!("{ms}ms")
                    }
                } else {
                    badge.to_string()
                };
                let bx = sx + sb.w - 1.0 - dur_str.len() as f32 * cw - 6.0;
                cmds.push(text(
                    &dur_str,
                    bx,
                    sy + (item_h - ch) * 0.5,
                    dot_col,
                    item_bg,
                ));

                sy += item_h;
                if sy + item_h > sb.y + sb.h - 4.0 {
                    break;
                }
            }
        }
    }

    // ── Pane headers (CWD / host label per pane) ──────────────────────────
    for &(px, py, pw, _ph, session_idx) in s.pane_rects {
        if let Some(session) = s.sessions.get(session_idx) {
            // Header background (bg-surface)
            let hdr = Rect {
                x: px,
                y: py,
                w: pw,
                h: PANE_HEADER_H,
            };
            cmds.push(fill(hdr, COL_PANEL));
            cmds.push(bottom_border(hdr, COL_BORDER));

            // Session-kind colored dot (5px, per spec)
            let (dot_col, _) = session_tab_info(session);
            let dot_cy = hdr.y + hdr.h * 0.5;
            cmds.push(dot(px + 10.0 + 2.5, dot_cy, 2.5, dot_col));

            // Label: CWD for local, user@host for SSH, agent name for agent
            let label = &session.title;
            let text_y = hdr.y + (hdr.h - ch) * 0.5;
            cmds.push(text(
                label,
                px + 10.0 + 5.0 + 6.0,
                text_y,
                COL_MUTED,
                COL_PANEL,
            ));
        }
    }

    // ── Pane split handles (vertical bars between panes) ──────────────────
    for &hx in s.split_handles {
        cmds.push(UiCommand::FillRect {
            x: hx,
            y: s.layout.content.y,
            w: SPLIT_HANDLE_W,
            h: s.layout.content.h,
            color: hex(COL_BORDER),
        });
    }

    // ── Status bar ────────────────────────────────────────────────────────
    let stb = s.layout.statusbar;
    cmds.push(fill(stb, COL_BG));
    cmds.push(UiCommand::FillRect {
        x: stb.x,
        y: stb.y,
        w: stb.w,
        h: 1.0,
        color: hex(COL_BORDER),
    });

    let ssh_count = s
        .sessions
        .iter()
        .filter(|s| matches!(s.kind, SessionKind::Ssh { .. }))
        .count();
    let agent_count = s
        .sessions
        .iter()
        .filter(|s| matches!(s.kind, SessionKind::Agent { .. }))
        .count();
    let active_count = s.sessions.len();

    let st_y = stb.y + (stb.h - ch) * 0.5;
    let mut st_x = stb.x + 12.0;

    // ● N active (green)
    cmds.push(dot(st_x + 3.0, stb.y + stb.h * 0.5, 3.0, COL_GREEN));
    st_x += 10.0;
    cmds.push(text(
        &format!("{active_count} active"),
        st_x,
        st_y,
        COL_GREEN,
        COL_BG,
    ));
    st_x += (active_count.to_string().len() as f32 + 7.0) * cw + 14.0;

    // 🔒 N SSH (blue) — always shown per spec (5 items)
    cmds.push(text(
        &format!("{ssh_count} SSH"),
        st_x,
        st_y,
        COL_BLUE,
        COL_BG,
    ));
    st_x += (ssh_count.to_string().len() as f32 + 4.0) * cw + 14.0;

    // ⬡ N agent (purple) — always shown per spec (5 items)
    cmds.push(text(
        &format!("{agent_count} agent"),
        st_x,
        st_y,
        COL_PURPLE,
        COL_BG,
    ));
    let _ = st_x; // suppress unused warning

    // Right group
    let mut right_x = stb.x + stb.w - 12.0;

    // ⏳ N waiting — agents awaiting user input (OSC 9001 status:awaiting_user)
    if s.awaiting_count > 0 {
        let wait_str = format!("{} waiting", s.awaiting_count);
        right_x -= wait_str.len() as f32 * cw + 14.0;
        cmds.push(dot(right_x - 6.0, stb.y + stb.h * 0.5, 3.0, COL_AMBER));
        cmds.push(text(&wait_str, right_x, st_y, COL_AMBER, COL_BG));
    }

    // ⚠ N error (amber) — always shown per spec (5 items)
    let err_str = format!("{} error", s.error_count);
    right_x -= err_str.len() as f32 * cw + 14.0;
    let err_color = if s.error_count > 0 {
        COL_AMBER
    } else {
        COL_MUTED
    };
    cmds.push(text(&err_str, right_x, st_y, err_color, COL_BG));

    cmds
}

// ────────────────────────────────────────────────────────────────────────────
// Convenience builders
// ────────────────────────────────────────────────────────────────────────────

fn fill(r: Rect, color: &str) -> UiCommand {
    UiCommand::FillRect {
        x: r.x,
        y: r.y,
        w: r.w,
        h: r.h,
        color: hex(color),
    }
}

fn bottom_border(r: Rect, color: &str) -> UiCommand {
    UiCommand::BottomBorder {
        x: r.x,
        y: r.y,
        w: r.w,
        h: r.h,
        color: hex(color),
    }
}

fn dot(cx: f32, cy: f32, r: f32, color: &str) -> UiCommand {
    UiCommand::DrawDot {
        cx,
        cy,
        r,
        color: hex(color),
    }
}

fn text<'a>(t: &'a str, x: f32, y: f32, fg: &str, bg: &str) -> UiCommand {
    UiCommand::DrawText {
        x,
        y,
        text: t.to_string(),
        fg: hex(fg),
        bg: hex(bg),
    }
}

fn session_tab_info<'a>(s: &'a Session) -> (&'static str, &'a str) {
    match &s.kind {
        SessionKind::Local => (COL_GREEN, s.title.as_str()),
        SessionKind::Ssh { .. } => (COL_BLUE, s.title.as_str()),
        SessionKind::Agent { .. } => (COL_PURPLE, s.title.as_str()),
    }
}

fn session_badge(s: &Session) -> &'static str {
    match &s.kind {
        SessionKind::Local => "active",
        SessionKind::Ssh { .. } => "SSH",
        SessionKind::Agent { .. } => "agent",
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Command block overlays (Section 16)
// ────────────────────────────────────────────────────────────────────────────

use libterm::block::store::{BlockId, BlockKind, BlockStatus as BS};
use libterm::vt::sequences::OscNotification;
use renderer_dx12::ui_renderer::{
    COL_BLOCK_AGENT_BD, COL_BLOCK_AGENT_BG, COL_BLOCK_AWAIT_BD, COL_BLOCK_AWAIT_BG,
    COL_BLOCK_ERROR_BD, COL_BLOCK_ERROR_BG, COL_BLOCK_RUNNING_BD, COL_BLOCK_RUNNING_BG,
    COL_BLOCK_SUCCESS_BD, COL_BLOCK_SUCCESS_BG,
};
use rstar::{RTree, RTreeObject, AABB};

/// Pixel-space bounding box of a rendered block, used for O(log n) click routing.
#[derive(Debug, Clone)]
pub struct BlockHitTarget {
    pub block_id: BlockId,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl RTreeObject for BlockHitTarget {
    type Envelope = AABB<[f32; 2]>;

    fn envelope(&self) -> Self::Envelope {
        AABB::from_corners([self.x, self.y], [self.x + self.w, self.y + self.h])
    }
}

/// Build an RTree from a slice of hit targets for O(log n) point queries.
pub fn build_block_rtree(targets: Vec<BlockHitTarget>) -> RTree<BlockHitTarget> {
    RTree::bulk_load(targets)
}

/// Generate block header overlays for a pane. Drawn on top of the terminal
/// grid cells.  Each block gets a colored header bar at the row where the
/// command was typed.
///
/// `pane_x/y/w` = pixel rect of the terminal area (below pane header).
/// `cell_h` = height of one terminal row in pixels.
/// `blocks` = the session's recent blocks.
/// `grid_rows` = total rows in the grid (for row→pixel mapping).
pub fn generate_block_overlays(
    blocks: &[CommandBlock],
    pane_x: f32,
    pane_y: f32,
    pane_w: f32,
    cell_w: f32,
    cell_h: f32,
    grid_rows: u16,
) -> (Vec<UiCommand>, Vec<BlockHitTarget>) {
    let mut cmds = Vec::new();
    let mut hits = Vec::new();
    if blocks.is_empty() {
        return (cmds, hits);
    }

    let block_header_h = cell_h + 4.0; // slightly taller than one cell row
    let margin_v = 3.0; // per spec: 3px top/bottom margin

    // We render the most recent blocks from the bottom of the pane upward.
    // Start from the row just above the cursor prompt (last 2 rows reserved).
    let mut y = pane_y + (grid_rows as f32 - 2.0) * cell_h;

    for block in blocks.iter().rev() {
        if y < pane_y {
            break; // off-screen
        }

        // Notification state overrides the default status-based color when the
        // agent pushes an OSC 9001 event while still running.
        let (header_bg, border_col, prompt_col, badge_text, badge_fg) =
            if let Some(notif) = &block.notification {
                match notif {
                    OscNotification::StatusAwaiting => (
                        COL_BLOCK_AWAIT_BG,
                        COL_BLOCK_AWAIT_BD,
                        COL_AMBER,
                        "wait".to_string(),
                        COL_AMBER,
                    ),
                    OscNotification::StatusSuccess => (
                        COL_BLOCK_SUCCESS_BG,
                        COL_BLOCK_SUCCESS_BD,
                        COL_GREEN,
                        "ok".to_string(),
                        COL_GREEN,
                    ),
                    OscNotification::StatusError => (
                        COL_BLOCK_ERROR_BG,
                        COL_BLOCK_ERROR_BD,
                        COL_RED,
                        "err".to_string(),
                        COL_RED,
                    ),
                    OscNotification::Notify(_) => (
                        COL_BLOCK_AWAIT_BG,
                        COL_BLOCK_AWAIT_BD,
                        COL_AMBER,
                        "msg".to_string(),
                        COL_AMBER,
                    ),
                }
            } else {
                match (&block.status, &block.kind) {
                    (BS::Success, BlockKind::Agent) => (
                        COL_BLOCK_AGENT_BG,
                        COL_BLOCK_AGENT_BD,
                        COL_PURPLE,
                        "ok".to_string(),
                        COL_GREEN,
                    ),
                    (BS::Error, BlockKind::Agent) => (
                        COL_BLOCK_AGENT_BG,
                        COL_BLOCK_AGENT_BD,
                        COL_PURPLE,
                        format!("err {}", block.exit_code.unwrap_or(1)),
                        COL_RED,
                    ),
                    (BS::Running, BlockKind::Agent) => (
                        COL_BLOCK_AGENT_BG,
                        COL_BLOCK_AGENT_BD,
                        COL_PURPLE,
                        "run".to_string(),
                        COL_PURPLE,
                    ),
                    (BS::Success, _) => (
                        COL_BLOCK_SUCCESS_BG,
                        COL_BLOCK_SUCCESS_BD,
                        COL_GREEN,
                        format!("ok {}", block.exit_code.unwrap_or(0)),
                        COL_GREEN,
                    ),
                    (BS::Error, _) => (
                        COL_BLOCK_ERROR_BG,
                        COL_BLOCK_ERROR_BD,
                        COL_RED,
                        format!("err {}", block.exit_code.unwrap_or(1)),
                        COL_RED,
                    ),
                    (BS::Running, _) => (
                        COL_BLOCK_RUNNING_BG,
                        COL_BLOCK_RUNNING_BD,
                        COL_BLUE,
                        "run".to_string(),
                        COL_BLUE,
                    ),
                    (BS::Cancelled, _) => (
                        COL_PANEL,
                        COL_BORDER,
                        COL_MUTED,
                        "---".to_string(),
                        COL_MUTED,
                    ),
                }
            };

        // Output lines: count newlines in output (capped for display)
        let output_lines = if block.output.is_empty() {
            0usize
        } else {
            block.output.iter().filter(|&&b| b == b'\n').count().min(8)
        };
        let block_total_h = block_header_h + margin_v * 2.0 + output_lines as f32 * cell_h;

        // Position this block: bottom of the available space, moving up.
        let block_y = y - block_total_h;
        if block_y < pane_y {
            break;
        }

        // Record bounding box for RTree hit detection.
        hits.push(BlockHitTarget {
            block_id: block.id,
            x: pane_x + 8.0,
            y: block_y,
            w: pane_w - 16.0,
            h: block_total_h,
        });

        // Whole-block background (matches terminal bg so it blends cleanly).
        cmds.push(UiCommand::FillRect {
            x: pane_x + 8.0,
            y: block_y,
            w: pane_w - 16.0,
            h: block_total_h,
            color: hex(COL_BG),
        });

        // Left accent bar — the primary visual signature of the block.
        // 3px wide, full block height, colored by status.
        cmds.push(UiCommand::FillRect {
            x: pane_x + 8.0,
            y: block_y,
            w: 3.0,
            h: block_total_h,
            color: hex(border_col),
        });

        // Header background (starts after accent bar).
        cmds.push(UiCommand::FillRect {
            x: pane_x + 11.0,
            y: block_y,
            w: pane_w - 19.0,
            h: block_header_h,
            color: hex(header_bg),
        });

        // Separator under header when output is present.
        if output_lines > 0 {
            cmds.push(UiCommand::FillRect {
                x: pane_x + 11.0,
                y: block_y + block_header_h,
                w: pane_w - 19.0,
                h: 1.0,
                color: hex(COL_BORDER),
            });
        }

        // Prompt symbol + command text in header.
        let text_y = block_y + (block_header_h - cell_h) * 0.5;
        let prompt = ">";
        cmds.push(text(prompt, pane_x + 18.0, text_y, prompt_col, header_bg));

        // Command name (truncate to fit; leave room for badge on right).
        let max_cmd_chars = ((pane_w - 120.0) / cell_w) as usize;
        let cmd_display: String =
            if block.command.chars().count() > max_cmd_chars && max_cmd_chars > 3 {
                format!(
                    "{}...",
                    &block.command[..block
                        .command
                        .char_indices()
                        .nth(max_cmd_chars - 3)
                        .map(|(i, _)| i)
                        .unwrap_or(block.command.len())]
                )
            } else {
                block.command.clone()
            };
        cmds.push(text(
            &cmd_display,
            pane_x + 18.0 + cell_w * 2.0,
            text_y,
            COL_TEXT,
            header_bg,
        ));

        // Exit badge (right-aligned in header)
        let badge_x = pane_x + pane_w - 16.0 - badge_text.len() as f32 * cell_w - 12.0;
        cmds.push(text(&badge_text, badge_x, text_y, badge_fg, header_bg));

        // Notify message (shown in header when agent pushes a free-form message)
        if let Some(OscNotification::Notify(msg)) = &block.notification {
            let max_msg = ((pane_w - 200.0) / cell_w) as usize;
            let display: String = msg.chars().take(max_msg.max(1)).collect();
            cmds.push(text(
                &display,
                pane_x + 16.0 + cell_w * 4.0,
                text_y,
                COL_AMBER,
                header_bg,
            ));
        }

        // Duration (if finished)
        if let Some(ms) = block.duration_ms() {
            let dur = if ms >= 1000 {
                format!("{:.1}s", ms as f32 / 1000.0)
            } else {
                format!("{ms}ms")
            };
            let dur_x = badge_x - dur.len() as f32 * cell_w - 8.0;
            cmds.push(text(&dur, dur_x, text_y, COL_FAINT, header_bg));
        }

        // Output body (below header).
        if output_lines > 0 {
            // Slightly raised body background for visual separation.
            cmds.push(UiCommand::FillRect {
                x: pane_x + 11.0,
                y: block_y + block_header_h + 1.0,
                w: pane_w - 19.0,
                h: block_total_h - block_header_h - 1.0,
                color: hex(COL_PANEL),
            });

            let output_str = String::from_utf8_lossy(&block.output);
            let mut line_y = block_y + block_header_h + 1.0 + (cell_h * 0.2);
            for line in output_str.lines().take(output_lines) {
                let display: String = line.chars().take(max_cmd_chars + 10).collect();
                let line_color = if line.contains("FAIL")
                    || line.contains("error")
                    || line.contains("Error")
                {
                    COL_RED
                } else {
                    COL_MUTED
                };
                cmds.push(text(&display, pane_x + 20.0, line_y, line_color, COL_PANEL));
                line_y += cell_h;
            }
        }

        y = block_y - margin_v;
    }

    (cmds, hits)
}
