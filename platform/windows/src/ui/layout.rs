//! Chrome layout computation and the state bundle passed to each sub-renderer.

use libterm::block::store::CommandBlock;
use libterm::mux::session::Session;
use renderer_windows::tokens;

// ── Fixed chrome dimensions (from design tokens) ──────────────────────────────

pub const SESSION_BAR_H: f32 = tokens::TITLEBAR_HEIGHT;
pub const CAPTION_ZONE_W: f32 = tokens::CAPTION_ZONE_W;
pub const SIDEBAR_W: f32 = tokens::SIDEBAR_W;
pub const STATUSBAR_H: f32 = tokens::STATUSBAR_HEIGHT;
pub const PANE_HEADER_H: f32 = tokens::PANE_HEADER_H;
pub const SPLIT_HANDLE_W: f32 = tokens::SPLIT_HANDLE_W;

/// Left pad before the first session tab (caption buttons are on the right).
pub const TAB_PAD_LEFT: f32 = 8.0;

// Caption button geometry (full-bleed 46px backplates, Windows 11 spec)
pub const CAPTION_BTN_W: f32 = tokens::CAPTION_BTN_W;

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
    pub session_bar: Rect,
    pub sidebar: Option<Rect>,
    pub content: Rect,
    pub statusbar: Rect,
}

impl ChromeLayout {
    pub fn compute(window_w: f32, window_h: f32, sidebar_visible: bool) -> Self {
        let offset = if sidebar_visible { SIDEBAR_W } else { 0.0 };
        Self::compute_animated(window_w, window_h, sidebar_visible, offset)
    }

    pub fn compute_animated(
        window_w: f32,
        window_h: f32,
        _sidebar_visible: bool,
        anim_offset: f32,
    ) -> Self {
        let session_bar = Rect { x: 0.0, y: 0.0, w: window_w, h: SESSION_BAR_H };
        let statusbar = Rect { x: 0.0, y: window_h - STATUSBAR_H, w: window_w, h: STATUSBAR_H };
        let body_top = SESSION_BAR_H;
        let body_h = window_h - body_top - STATUSBAR_H;
        // The offset alone decides the rendered width, so hide animations
        // (visibility already false, offset shrinking to 0) still draw.
        let sw = anim_offset.clamp(0.0, SIDEBAR_W);
        let sb_w = if sw > 1.0 { sw } else { 0.0 };
        let (sidebar, content) = if sb_w > 0.0 {
            let sb = Rect { x: 0.0, y: body_top, w: sb_w, h: body_h };
            let ct = Rect { x: sb_w, y: body_top, w: window_w - sb_w, h: body_h };
            (Some(sb), ct)
        } else {
            (None, Rect { x: 0.0, y: body_top, w: window_w, h: body_h })
        };
        Self { session_bar, sidebar, content, statusbar }
    }
}

// ── ChromeState ───────────────────────────────────────────────────────────────

pub struct ChromeState<'a> {
    pub layout: &'a ChromeLayout,
    pub sessions: &'a [Session],
    pub active_tab: usize,
    pub active_pane_session_idx: usize,
    pub tab_exit_codes: &'a [Option<i32>],
    pub active_shell_name: &'a str,
    pub active_cwd: Option<&'a str>,
    pub workspace_names: &'a [String],
    pub active_workspace: usize,
    pub split_handles: &'a [f32],
    pub cell_h: u32,
    pub agent_blocks: &'a [CommandBlock],
    pub pane_rects: &'a [(f32, f32, f32, f32, usize)],
    pub ui_char_w: f32,
    pub badge_widths: &'a [f32],
    pub mouse_pos: (f32, f32),
}
