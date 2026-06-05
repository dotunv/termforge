//! Chrome layout: titlebar, tab bar, sidebar, content area, status bar.
//!
//! All dimensions are in window pixels.  `generate_commands` returns a flat
//! list of [`UiCommand`]s describing every chrome element for the current
//! frame.  The compositor draws them in one pass before the terminal panes.

use libterm::block::store::{BlockStatus, CommandBlock};
use libterm::mux::session::{Session, SessionKind};
use renderer_dx12::ui_renderer::{hex, UiCommand,
    COL_BG, COL_PANEL, COL_BORDER, COL_TEXT, COL_MUTED,
    COL_GREEN, COL_BLUE, COL_PURPLE, COL_AMBER, COL_RED};

// ── Fixed chrome heights / widths ────────────────────────────────────────────
pub const TITLEBAR_H:  f32 = 36.0;
pub const TABBAR_H:    f32 = 32.0;
pub const SIDEBAR_W:   f32 = 190.0;
pub const STATUSBAR_H: f32 = 22.0;

pub const SPLIT_HANDLE_W: f32 = 4.0; // draggable split handle width

/// Axis-aligned pixel rect.
#[derive(Debug, Clone, Copy)]
pub struct Rect { pub x: f32, pub y: f32, pub w: f32, pub h: f32 }

impl Rect {
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

/// Computed rects for one frame.
#[derive(Debug, Clone)]
pub struct ChromeLayout {
    pub titlebar:  Rect,
    pub tabbar:    Rect,
    pub sidebar:   Option<Rect>,   // None when collapsed
    pub content:   Rect,           // terminal pane area
    pub statusbar: Rect,
    pub window_w:  f32,
    pub window_h:  f32,
}

impl ChromeLayout {
    pub fn compute(window_w: f32, window_h: f32, sidebar_visible: bool) -> Self {
        let titlebar  = Rect { x: 0.0, y: 0.0,          w: window_w, h: TITLEBAR_H };
        let tabbar    = Rect { x: 0.0, y: TITLEBAR_H,   w: window_w, h: TABBAR_H };
        let statusbar = Rect { x: 0.0, y: window_h - STATUSBAR_H, w: window_w, h: STATUSBAR_H };

        let body_top = TITLEBAR_H + TABBAR_H;
        let body_h   = window_h - body_top - STATUSBAR_H;

        let (sidebar, content) = if sidebar_visible {
            let sb = Rect { x: 0.0,      y: body_top, w: SIDEBAR_W, h: body_h };
            let ct = Rect { x: SIDEBAR_W, y: body_top, w: window_w - SIDEBAR_W, h: body_h };
            (Some(sb), ct)
        } else {
            (None, Rect { x: 0.0, y: body_top, w: window_w, h: body_h })
        };

        Self { titlebar, tabbar, sidebar, content, statusbar, window_w, window_h }
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
    /// Cell dimensions from atlas (used for text vertical centring).
    pub cell_w: u32,
    pub cell_h: u32,
    /// Recent blocks of the active agent session (empty for non-agent sessions).
    pub agent_blocks: &'a [CommandBlock],
}

pub fn generate_commands(s: &ChromeState<'_>) -> Vec<UiCommand> {
    let mut cmds = Vec::with_capacity(256);
    let cw = s.cell_w as f32;
    let ch = s.cell_h as f32;

    // ── Title bar ─────────────────────────────────────────────────────────
    let tb = s.layout.titlebar;
    cmds.push(fill(tb, COL_PANEL));
    cmds.push(bottom_border(tb, COL_BORDER));

    // Traffic-light dots
    let dot_y = tb.y + tb.h * 0.5;
    cmds.push(dot(tb.x + 12.0, dot_y, 5.0, "#ff5f56"));
    cmds.push(dot(tb.x + 24.0, dot_y, 5.0, "#ffbd2e"));
    cmds.push(dot(tb.x + 36.0, dot_y, 5.0, "#27c93f"));

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
    let tab_dot_y  = tabbar.y + tabbar.h * 0.5;

    for (i, session) in s.sessions.iter().enumerate() {
        let is_active = i == s.active_tab;
        let tab_bg = if is_active { COL_BG } else { COL_PANEL };
        let (dot_color, label) = session_tab_info(session);

        let label_chars = label.chars().count() as f32;
        let tab_w = 10.0 + 6.0 + 4.0 + label_chars * cw + 10.0; // pad + dot + gap + text + pad

        // Tab background
        cmds.push(UiCommand::FillRect {
            x: tab_x, y: tabbar.y + 4.0,
            w: tab_w, h: tabbar.h - 8.0,
            color: hex(tab_bg),
        });

        // Coloured dot
        cmds.push(dot(tab_x + 10.0 + 3.0, tab_dot_y, 3.0, dot_color));

        // Label
        cmds.push(text(label, tab_x + 10.0 + 6.0 + 4.0, tab_text_y,
            if is_active { COL_TEXT } else { COL_MUTED },
            tab_bg));

        tab_x += tab_w + 2.0;
    }

    // "+" new-tab button
    cmds.push(text("+", tab_x + 4.0, tab_text_y, COL_MUTED, COL_PANEL));

    // ── Sidebar ───────────────────────────────────────────────────────────
    if let Some(sb) = s.layout.sidebar {
        cmds.push(fill(sb, COL_PANEL));
        // Right border
        cmds.push(UiCommand::FillRect {
            x: sb.x + sb.w - 1.0, y: sb.y, w: 1.0, h: sb.h,
            color: hex(COL_BORDER),
        });

        let mut sy = sb.y + 6.0;
        let sx = sb.x;
        let item_h = ch + 6.0;

        // Workspace label
        cmds.push(dot(sx + 12.0, sy + ch * 0.5, 6.0, COL_BLUE));
        cmds.push(text(s.workspace_name, sx + 24.0, sy, COL_BLUE, COL_PANEL));
        sy += item_h;

        // Session list items
        for (i, session) in s.sessions.iter().enumerate() {
            let is_active = i == s.active_tab;
            let item_bg = if is_active { "#1c2128" } else { COL_PANEL };
            let (dot_color, label) = session_tab_info(session);
            let badge = session_badge(session);

            cmds.push(UiCommand::FillRect {
                x: sx, y: sy, w: sb.w - 1.0, h: item_h,
                color: hex(item_bg),
            });
            cmds.push(dot(sx + 26.0, sy + item_h * 0.5, 4.0, dot_color));
            let text_color = if is_active { COL_TEXT } else { COL_MUTED };
            cmds.push(text(label, sx + 34.0, sy + (item_h - ch) * 0.5, text_color, item_bg));
            // Badge
            if !badge.is_empty() {
                let bx = sx + sb.w - 1.0 - badge.len() as f32 * cw - 8.0;
                cmds.push(text(badge, bx, sy + (item_h - ch) * 0.5, COL_MUTED, item_bg));
            }
            sy += item_h;
        }

        // Divider
        cmds.push(UiCommand::FillRect {
            x: sx + 12.0, y: sy + 2.0, w: sb.w - 24.0, h: 1.0,
            color: hex(COL_BORDER),
        });
        sy += 6.0;

        // Section header: WORKSPACES
        cmds.push(text("WORKSPACES", sx + 12.0, sy, COL_MUTED, COL_PANEL));
        sy += item_h;

        // Static workspace items (Phase 3 — real workspaces in Phase 4)
        for label in &["work / backend", "infra / servers", "personal"] {
            cmds.push(dot(sx + 12.0, sy + item_h * 0.5, 4.0, COL_MUTED));
            cmds.push(text(label, sx + 22.0, sy + (item_h - ch) * 0.5, COL_MUTED, COL_PANEL));
            sy += item_h;
        }

        // Divider
        cmds.push(UiCommand::FillRect {
            x: sx + 12.0, y: sy + 2.0, w: sb.w - 24.0, h: 1.0,
            color: hex(COL_BORDER),
        });
        sy += 6.0;

        // Section header: TOOLS
        cmds.push(text("TOOLS", sx + 12.0, sy, COL_MUTED, COL_PANEL));
        sy += item_h;

        for label in &["SSH manager", "key vault", "agent runs"] {
            cmds.push(dot(sx + 12.0, sy + item_h * 0.5, 4.0, COL_MUTED));
            cmds.push(text(label, sx + 22.0, sy + (item_h - ch) * 0.5, COL_MUTED, COL_PANEL));
            sy += item_h;
        }

        // ── Agent block history (shown only when active tab is an Agent) ──────
        if !s.agent_blocks.is_empty() {
            // Divider
            cmds.push(UiCommand::FillRect {
                x: sx + 12.0, y: sy + 2.0, w: sb.w - 24.0, h: 1.0,
                color: hex(COL_BORDER),
            });
            sy += 6.0;

            cmds.push(text("RECENT BLOCKS", sx + 12.0, sy, COL_PURPLE, COL_PANEL));
            sy += item_h;

            for block in s.agent_blocks.iter().rev().take(8) {
                let (dot_col, badge) = match block.status {
                    BlockStatus::Running   => (COL_PURPLE, "run"),
                    BlockStatus::Success   => (COL_GREEN,  "ok "),
                    BlockStatus::Error     => (COL_RED,    "err"),
                    BlockStatus::Cancelled => (COL_MUTED,  "---"),
                };
                let item_bg = COL_PANEL;

                cmds.push(dot(sx + 14.0, sy + item_h * 0.5, 3.5, dot_col));

                // Truncate command to fit sidebar width (~18 chars)
                let cmd = &block.command;
                let max_chars = 18usize;
                let label: String = if cmd.chars().count() > max_chars {
                    format!("{}…", &cmd[..cmd.char_indices().nth(max_chars - 1).map(|(i,_)| i).unwrap_or(cmd.len())])
                } else {
                    cmd.clone()
                };
                cmds.push(text(&label, sx + 24.0, sy + (item_h - ch) * 0.5, COL_TEXT, item_bg));

                // Right-align badge + optional duration
                let dur_str = if let Some(ms) = block.duration_ms() {
                    if ms >= 1000 { format!("{:.1}s", ms as f32 / 1000.0) } else { format!("{ms}ms") }
                } else {
                    badge.to_string()
                };
                let bx = sx + sb.w - 1.0 - dur_str.len() as f32 * cw - 6.0;
                cmds.push(text(&dur_str, bx, sy + (item_h - ch) * 0.5, dot_col, item_bg));

                sy += item_h;
                if sy + item_h > sb.y + sb.h - 4.0 { break; }
            }
        }
    }

    // ── Pane split handles (vertical bars between panes) ──────────────────
    for &hx in s.split_handles {
        cmds.push(UiCommand::FillRect {
            x: hx, y: s.layout.content.y,
            w: SPLIT_HANDLE_W, h: s.layout.content.h,
            color: hex(COL_BORDER),
        });
    }

    // ── Status bar ────────────────────────────────────────────────────────
    let stb = s.layout.statusbar;
    cmds.push(fill(stb, COL_BG));
    cmds.push(UiCommand::FillRect {
        x: stb.x, y: stb.y, w: stb.w, h: 1.0,
        color: hex(COL_BORDER),
    });

    let ssh_count  = s.sessions.iter().filter(|s| matches!(s.kind, SessionKind::Ssh { .. })).count();
    let agent_count = s.sessions.iter().filter(|s| matches!(s.kind, SessionKind::Agent { .. })).count();
    let active_count = s.sessions.len();

    let st_y = stb.y + (stb.h - ch) * 0.5;
    let mut st_x = stb.x + 12.0;

    // ● N active (green)
    cmds.push(dot(st_x + 3.0, stb.y + stb.h * 0.5, 3.0, COL_GREEN));
    st_x += 10.0;
    cmds.push(text(&format!("{active_count} active"), st_x, st_y, COL_GREEN, COL_BG));
    st_x += (active_count.to_string().len() as f32 + 7.0) * cw + 14.0;

    // 🔒 N SSH (blue)
    if ssh_count > 0 {
        cmds.push(text(&format!("{ssh_count} SSH"), st_x, st_y, COL_BLUE, COL_BG));
        st_x += (ssh_count.to_string().len() as f32 + 4.0) * cw + 14.0;
    }

    // ⬡ N agent (purple)
    if agent_count > 0 {
        cmds.push(text(&format!("{agent_count} agent"), st_x, st_y, COL_PURPLE, COL_BG));
        st_x += (agent_count.to_string().len() as f32 + 6.0) * cw + 14.0;
    }

    // Right group
    let mut right_x = stb.x + stb.w - 12.0;

    // ⬡ 0% GPU — placeholder
    let gpu_str = "0%";
    right_x -= gpu_str.len() as f32 * cw + 14.0;
    cmds.push(text(gpu_str, right_x, st_y, COL_MUTED, COL_BG));

    // ⚠ N error (amber)
    if s.error_count > 0 {
        let err_str = format!("{} error", s.error_count);
        right_x -= err_str.len() as f32 * cw + 14.0;
        cmds.push(dot(right_x + 3.0, stb.y + stb.h * 0.5, 3.0, COL_AMBER));
        cmds.push(text(&err_str, right_x + 10.0, st_y, COL_AMBER, COL_BG));
    }

    cmds
}

// ────────────────────────────────────────────────────────────────────────────
// Convenience builders
// ────────────────────────────────────────────────────────────────────────────

fn fill(r: Rect, color: &str) -> UiCommand {
    UiCommand::FillRect { x: r.x, y: r.y, w: r.w, h: r.h, color: hex(color) }
}

fn bottom_border(r: Rect, color: &str) -> UiCommand {
    UiCommand::BottomBorder { x: r.x, y: r.y, w: r.w, h: r.h, color: hex(color) }
}

fn dot(cx: f32, cy: f32, r: f32, color: &str) -> UiCommand {
    UiCommand::DrawDot { cx, cy, r, color: hex(color) }
}

fn text<'a>(t: &'a str, x: f32, y: f32, fg: &str, bg: &str) -> UiCommand {
    UiCommand::DrawText {
        x, y,
        text: t.to_string(),
        fg: hex(fg),
        bg: hex(bg),
    }
}

fn session_tab_info<'a>(s: &'a Session) -> (&'static str, &'a str) {
    match &s.kind {
        SessionKind::Local         => (COL_GREEN,  s.title.as_str()),
        SessionKind::Ssh { .. }   => (COL_BLUE,   s.title.as_str()),
        SessionKind::Agent { .. } => (COL_PURPLE,  s.title.as_str()),
    }
}

fn session_badge(s: &Session) -> &'static str {
    match &s.kind {
        SessionKind::Local         => "active",
        SessionKind::Ssh { .. }   => "SSH",
        SessionKind::Agent { .. } => "agent",
    }
}
