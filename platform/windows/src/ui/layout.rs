//! Chrome layout computation and the state bundle passed to each sub-renderer.

use libterm::block::store::CommandBlock;
use libterm::mux::session::Session;

// ── Fixed chrome dimensions ───────────────────────────────────────────────────

/// Single unified surface replacing the old separate titlebar (36px) + tabbar (32px).
/// Contains traffic lights, session tabs, and gear icon all in one 40px row.
pub const SESSION_BAR_H: f32 = 40.0;
/// Pixel width reserved for the traffic-light zone on the left of the session bar.
/// Tabs begin at this x-offset.
pub const TL_ZONE_W: f32 = 52.0;
pub const SIDEBAR_W: f32 = 190.0;
pub const STATUSBAR_H: f32 = 22.0;
pub const PANE_HEADER_H: f32 = 24.0;
pub const SPLIT_HANDLE_W: f32 = 4.0;

// Traffic light geometry
pub const TL_RADIUS: f32 = 5.0;
pub const TL_X0: f32 = 17.0;
pub const TL_GAP: f32 = 16.0;
pub const TL_Y: f32 = SESSION_BAR_H / 2.0;

// ── Rect ──────────────────────────────────────────────────────────────────────

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

// ── ChromeLayout ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ChromeLayout {
    /// Unified session bar: traffic lights + tabs + gear.
    pub session_bar: Rect,
    pub sidebar: Option<Rect>,
    pub content: Rect,
    pub statusbar: Rect,
    pub window_w: f32,
    pub window_h: f32,
}

impl ChromeLayout {
    pub fn compute(window_w: f32, window_h: f32, sidebar_visible: bool) -> Self {
        let session_bar = Rect { x: 0.0, y: 0.0, w: window_w, h: SESSION_BAR_H };
        let statusbar = Rect { x: 0.0, y: window_h - STATUSBAR_H, w: window_w, h: STATUSBAR_H };
        let body_top = SESSION_BAR_H;
        let body_h = window_h - body_top - STATUSBAR_H;
        let (sidebar, content) = if sidebar_visible {
            let sb = Rect { x: 0.0, y: body_top, w: SIDEBAR_W, h: body_h };
            let ct = Rect { x: SIDEBAR_W, y: body_top, w: window_w - SIDEBAR_W, h: body_h };
            (Some(sb), ct)
        } else {
            (None, Rect { x: 0.0, y: body_top, w: window_w, h: body_h })
        };
        Self { session_bar, sidebar, content, statusbar, window_w, window_h }
    }
}

// ── ChromeState ───────────────────────────────────────────────────────────────

/// All state the chrome sub-renderers need to produce their command lists.
pub struct ChromeState<'a> {
    pub layout: &'a ChromeLayout,
    pub sessions: &'a [Session],
    pub active_tab: usize,
    /// Which session_idx is the "active pane" (the one receiving keyboard input).
    /// In single-pane mode this equals `active_tab`.
    pub active_pane_session_idx: usize,
    /// Last command exit code per tab (index == tab index). `None` = no commands yet.
    pub tab_exit_codes: &'a [Option<i32>],
    /// Shell name for the active session (e.g. "powershell", "user@host", "claude").
    pub active_shell_name: &'a str,
    /// Working directory for the active session, if known via OSC 7.
    pub active_cwd: Option<&'a str>,
    pub workspace_names: &'a [String],
    pub active_workspace: usize,
    pub sidebar_visible: bool,
    pub split_handles: &'a [f32],
    pub error_count: usize,
    pub awaiting_count: usize,
    pub cell_w: u32,
    pub cell_h: u32,
    pub agent_blocks: &'a [CommandBlock],
    pub pane_rects: &'a [(f32, f32, f32, f32, usize)],
    /// Average Segoe UI character width (px).  Use atlas.measure_ui_text() for
    /// precise measurements; this is for quick proportional layout estimates.
    pub ui_char_w: f32,
}
