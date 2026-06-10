//! Design tokens — the single source of truth for all colours, spacing, and
//! layout constants used by the GDI pipeline.
//!
//! Palette direction: **neutral cmux/Ghostty dark**.  Surfaces are flat neutral
//! greys stepped into a clear elevation ladder; saturated accent colour is
//! reserved for *status and notification rings*, never for plain chrome fills.
//! Accent semantics stay constant: green = local PTY, blue = SSH, purple =
//! agent/AI, amber = awaiting input, red = error.

// ── Background / surface palette (neutral elevation ladder) ────────────────────

pub const BG_BASE: [f32; 4] = [0.055, 0.055, 0.067, 1.0];    // #0E0E11 terminal
pub const BG_SURFACE: [f32; 4] = [0.082, 0.082, 0.102, 1.0]; // #15151A chrome
pub const BG_HOVER: [f32; 4] = [0.110, 0.110, 0.133, 1.0];   // #1C1C22
pub const BG_ACTIVE: [f32; 4] = [0.141, 0.141, 0.169, 1.0];  // #24242B
pub const BG_RAISED: [f32; 4] = [0.173, 0.173, 0.204, 1.0];  // #2C2C34 modals

// ── Border palette ─────────────────────────────────────────────────────────────

pub const BORDER_SUBTLE: [f32; 4] = [0.118, 0.118, 0.141, 1.0];  // #1E1E24
pub const BORDER_DEFAULT: [f32; 4] = [0.165, 0.165, 0.196, 1.0]; // #2A2A32
pub const BORDER_STRONG: [f32; 4] = [0.227, 0.227, 0.267, 1.0];  // #3A3A44

// ── Text palette ───────────────────────────────────────────────────────────────

pub const TEXT_PRIMARY: [f32; 4] = [0.902, 0.902, 0.918, 1.0]; // #E6E6EA
pub const TEXT_MUTED: [f32; 4] = [0.604, 0.604, 0.651, 1.0];   // #9A9AA6
pub const TEXT_FAINT: [f32; 4] = [0.369, 0.369, 0.416, 1.0];   // #5E5E6A

// ── Accent / status colours (reserved for status + rings) ──────────────────────

pub const COLOR_LOCAL: [f32; 4] = [0.247, 0.725, 0.498, 1.0]; // #3FB97F green
pub const COLOR_SSH: [f32; 4] = [0.357, 0.616, 0.961, 1.0];   // #5B9DF5 blue
pub const COLOR_AGENT: [f32; 4] = [0.725, 0.545, 0.910, 1.0]; // #B98BE8 purple
pub const COLOR_ERROR: [f32; 4] = [0.941, 0.384, 0.353, 1.0]; // #F0625A red

pub const ACCENT_GREEN: [f32; 4] = COLOR_LOCAL;
pub const ACCENT_BLUE: [f32; 4] = COLOR_SSH;

pub const GREEN: [f32; 4] = COLOR_LOCAL;
pub const BLUE: [f32; 4] = COLOR_SSH;
pub const PURPLE: [f32; 4] = COLOR_AGENT;
pub const RED: [f32; 4] = COLOR_ERROR;
pub const AMBER: [f32; 4] = [0.910, 0.694, 0.298, 1.0];   // #E8B14C
pub const CYAN: [f32; 4] = [0.310, 0.816, 0.847, 1.0];    // #4FD0D8

// ── Window caption buttons ─────────────────────────────────────────────────────

pub const CAPTION_CLOSE: [f32; 4] = COLOR_ERROR;
pub const CAPTION_MIN: [f32; 4] = TEXT_MUTED;
pub const CAPTION_MAX: [f32; 4] = TEXT_MUTED;

// ── Block overlay (subtle accent-tinted darks over BG_BASE) ─────────────────────

pub const BLOCK_SUCCESS_BG: [f32; 4] = [0.067, 0.125, 0.102, 1.0]; // #11201A
pub const BLOCK_SUCCESS_BD: [f32; 4] = GREEN;

pub const BLOCK_ERROR_BG: [f32; 4] = [0.141, 0.078, 0.086, 1.0]; // #241416
pub const BLOCK_ERROR_BD: [f32; 4] = RED;

pub const BLOCK_RUNNING_BG: [f32; 4] = [0.075, 0.102, 0.141, 1.0]; // #131A24
pub const BLOCK_RUNNING_BD: [f32; 4] = BLUE;

pub const BLOCK_AGENT_BG: [f32; 4] = [0.110, 0.086, 0.149, 1.0]; // #1C1626
pub const BLOCK_AGENT_BD: [f32; 4] = PURPLE;

pub const BLOCK_AWAIT_BG: [f32; 4] = [0.141, 0.122, 0.071, 1.0]; // #241F12
pub const BLOCK_AWAIT_BD: [f32; 4] = AMBER;

// ── Badge backgrounds ──────────────────────────────────────────────────────────

pub const BADGE_GREEN_BG: [f32; 4] = BLOCK_SUCCESS_BG;
pub const BADGE_BLUE_BG: [f32; 4] = BLOCK_RUNNING_BG;
pub const BADGE_PURPLE_BG: [f32; 4] = BLOCK_AGENT_BG;

// ── Radius scale ────────────────────────────────────────────────────────────────

pub const RADIUS_SM: f32 = 6.0;
pub const RADIUS_MD: f32 = 10.0;
pub const RADIUS_LG: f32 = 14.0;

// ── Spacing scale ───────────────────────────────────────────────────────────────

pub const SPACE_1: f32 = 4.0;
pub const SPACE_2: f32 = 8.0;
pub const SPACE_3: f32 = 12.0;
pub const SPACE_4: f32 = 16.0;
pub const SPACE_5: f32 = 20.0;

pub const SP_PADDING_VERT: f32 = SPACE_2;
pub const SP_PADDING_HORZ: f32 = SPACE_3;

// ── Typography (point sizes + weights for the UI font set) ─────────────────────

pub const UI_PT_BODY: f32 = 11.0;
pub const UI_PT_CAPTION: f32 = 9.5;
pub const UI_PT_TITLE: f32 = 13.5;
pub const UI_WEIGHT_REGULAR: u32 = 400;
pub const UI_WEIGHT_SEMIBOLD: u32 = 600;
/// Icon font point sizes (Segoe Fluent Icons / MDL2 Assets).
pub const ICON_PT_BODY: f32 = 12.0;
pub const ICON_PT_TITLE: f32 = 15.0;

// ── Notification ring (cmux-style accent halo around a pane) ───────────────────

/// Stroke thickness of the active-pane border ring.
pub const RING_W: f32 = 1.5;
/// Per-layer alpha for the awaiting-input glow (drawn as expanding strokes).
pub const RING_GLOW_ALPHA: f32 = 0.18;
/// Number of glow layers around the awaiting ring.
pub const RING_GLOW_LAYERS: u32 = 4;

// ── Layout sizing units ────────────────────────────────────────────────────────

pub const TITLEBAR_HEIGHT: f32 = 44.0;
// Windows 11 titlebar spec: caption buttons are full-bleed backplates,
// 46px wide each, anchored to the top-right and always fully visible.
pub const CAPTION_BTN_W: f32 = 46.0;
pub const CAPTION_ZONE_W: f32 = CAPTION_BTN_W * 3.0; // min · max · close
pub const CAPTION_BTN_H: f32 = TITLEBAR_HEIGHT;
/// System close-hover red (#C42B1C) with white glyph, per the Windows spec.
pub const CAPTION_CLOSE_HOVER_BG: [f32; 4] = [0.769, 0.169, 0.110, 1.0];
pub const CAPTION_CLOSE_HOVER_FG: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

pub const TABBAR_HEIGHT: f32 = 32.0;
pub const SIDEBAR_W: f32 = 232.0;
pub const STATUSBAR_HEIGHT: f32 = 22.0;
pub const PANE_HEADER_H: f32 = 30.0;
pub const SPLIT_HANDLE_W: f32 = 4.0;
/// Gutter inset around each terminal pane so a rounded card border can sit
/// around it without overlapping the live grid.
pub const PANE_GUTTER: f32 = 6.0;

pub const OVERLAY_DIM: [f32; 4] = [0.030, 0.030, 0.040, 0.62]; // dim backdrop
