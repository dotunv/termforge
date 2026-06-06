//! Chrome UI rendering via the same CellVertex / glyph-atlas pipeline.
//!
//! `build_quads` converts a flat `Vec<UiCommand>` into `CellVertex` quads in
//! one pass.  The shader does `lerp(bg, fg, alpha)`, so:
//!
//!   FillRect      → space glyph (alpha≈0), output ≈ bg  → solid fill
//!   DrawCircle    → atlas circle glyph (alpha=1 inside), fg=circle color
//!   FillRoundRect → 7 quads: 4 antialised corners + 3 solid fill strips
//!   DrawText      → terminal monospace glyphs, fg+bg per quad
//!   DrawUiText    → proportional Segoe UI glyphs, variable advance widths

use crate::glyph_atlas::{GlyphAtlas, CORNER_R};
use crate::pipeline::CellVertex;

// ── Colour helpers ────────────────────────────────────────────────────────────

pub fn hex(s: &str) -> [f32; 4] {
    let s = s.trim_start_matches('#');
    let r = u8::from_str_radix(&s[0..2], 16).unwrap_or(0);
    let g = u8::from_str_radix(&s[2..4], 16).unwrap_or(0);
    let b = u8::from_str_radix(&s[4..6], 16).unwrap_or(0);
    [srgb(r), srgb(g), srgb(b), 1.0]
}

fn srgb(c: u8) -> f32 {
    let f = c as f32 / 255.0;
    if f <= 0.04045 { f / 12.92 } else { ((f + 0.055) / 1.055).powf(2.4) }
}

// ── Palette ───────────────────────────────────────────────────────────────────

// Background / surface
// COL_BG    = terminal pane background — very dark, near-black
// COL_PANEL = titlebar / tabbar / sidebar — neutral grey so chrome reads
//             as a clearly different surface from the terminal content
pub const COL_BG: &str = "#0d1117";
pub const COL_PANEL: &str = "#1e1e1e";
pub const COL_HOVER: &str = "#282828";
pub const COL_RAISED: &str = "#333333";
pub const COL_BORDER: &str = "#444444";

// Text
pub const COL_TEXT: &str = "#e6edf3";
pub const COL_MUTED: &str = "#8b949e";
pub const COL_FAINT: &str = "#6e7681";

// Accent
pub const COL_GREEN: &str = "#3fb950";
pub const COL_BLUE: &str = "#58a6ff";
pub const COL_PURPLE: &str = "#d2a8ff";
pub const COL_RED: &str = "#f85149";
pub const COL_AMBER: &str = "#e3b341";

// Traffic lights
pub const COL_TL_RED: &str = "#ff5f56";
pub const COL_TL_YELLOW: &str = "#ffbd2e";
pub const COL_TL_GREEN: &str = "#27c93f";

// Block state backgrounds and borders
pub const COL_BLOCK_SUCCESS_BG: &str = "#0d2f1a";
pub const COL_BLOCK_SUCCESS_BD: &str = "#1a4a2a";
pub const COL_BLOCK_ERROR_BG: &str = "#2d1117";
pub const COL_BLOCK_ERROR_BD: &str = "#4a1a1a";
pub const COL_BLOCK_RUNNING_BG: &str = "#0c1f3d";
pub const COL_BLOCK_RUNNING_BD: &str = "#1a2a4a";
pub const COL_BLOCK_AGENT_BG: &str = "#1e1040";
pub const COL_BLOCK_AGENT_BD: &str = "#2d1a5a";
pub const COL_BLOCK_AWAIT_BG: &str = "#2d2000";
pub const COL_BLOCK_AWAIT_BD: &str = "#4a3800";

// Badge tinted backgrounds (matching mockup)
pub const COL_BADGE_GREEN_BG: &str = "#0d2f1a";
pub const COL_BADGE_BLUE_BG: &str = "#0c1f3d";
pub const COL_BADGE_PURPLE_BG: &str = "#1e1040";

// ── Command list ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum UiCommand {
    /// Solid-colour rectangle.
    FillRect { x: f32, y: f32, w: f32, h: f32, color: [f32; 4] },

    /// 1px horizontal line at bottom of rect.
    BottomBorder { x: f32, y: f32, w: f32, h: f32, color: [f32; 4] },

    /// Antialiased filled circle.  `bg` is the surface color behind it (needed
    /// for antialiased edge blending).
    DrawCircle { cx: f32, cy: f32, r: f32, fg: [f32; 4], bg: [f32; 4] },

    /// Rounded rectangle filled with `color` over background `bg`.
    /// Uses pre-rasterised CORNER_R=4px corner pieces from the atlas.
    FillRoundRect { x: f32, y: f32, w: f32, h: f32, color: [f32; 4], bg: [f32; 4] },

    /// Terminal-font (monospace) text.
    DrawText { x: f32, y: f32, text: String, fg: [f32; 4], bg: [f32; 4] },

    /// Proportional UI-font (Segoe UI) text.
    DrawUiText { x: f32, y: f32, text: String, fg: [f32; 4], bg: [f32; 4] },
}

// ── Vertex builder ────────────────────────────────────────────────────────────

pub fn build_quads(cmds: &[UiCommand], atlas: &GlyphAtlas) -> Vec<CellVertex> {
    let mut out = Vec::with_capacity(cmds.len() * 12);
    let space_uv = atlas.uv_for_char(' ');

    for cmd in cmds {
        match cmd {
            UiCommand::FillRect { x, y, w, h, color } => {
                push_quad(&mut out, *x, *y, *w, *h, space_uv, [0.0; 4], *color);
            }

            UiCommand::BottomBorder { x, y, w, h, color } => {
                push_quad(&mut out, *x, y + h - 1.0, *w, 1.0, space_uv, [0.0; 4], *color);
            }

            UiCommand::DrawCircle { cx, cy, r, fg, bg } => {
                let uv = atlas.circle_uv;
                let d = r * 2.0;
                push_quad(&mut out, cx - r, cy - r, d, d, uv, *fg, *bg);
            }

            UiCommand::FillRoundRect { x, y, w, h, color, bg } => {
                let r = CORNER_R as f32;
                let [tl, tr, bl, br] = atlas.corner_uvs;

                // 4 corner quads
                push_quad(&mut out, *x,           *y,           r, r, tl, *color, *bg);
                push_quad(&mut out, x + w - r,    *y,           r, r, tr, *color, *bg);
                push_quad(&mut out, *x,           y + h - r,    r, r, bl, *color, *bg);
                push_quad(&mut out, x + w - r,    y + h - r,    r, r, br, *color, *bg);

                // 3 solid fill strips: top edge, bottom edge, center body
                push_quad(&mut out, x + r, *y,        w - r * 2.0, r,        space_uv, [0.0; 4], *color);
                push_quad(&mut out, x + r, y + h - r, w - r * 2.0, r,        space_uv, [0.0; 4], *color);
                push_quad(&mut out, *x,    y + r,     *w,          h - r * 2.0, space_uv, [0.0; 4], *color);
            }

            UiCommand::DrawText { x, y, text, fg, bg } => {
                let cw = atlas.cell_w as f32;
                let ch = atlas.cell_h as f32;
                for (i, c) in text.chars().enumerate() {
                    let uv = atlas.uv_for_char(c);
                    push_quad(&mut out, x + i as f32 * cw, *y, cw, ch, uv, *fg, *bg);
                }
            }

            UiCommand::DrawUiText { x, y, text, fg, bg } => {
                let cell_h = atlas.cell_h as f32;
                // The UI font is smaller than the terminal cell.  Without an
                // offset the glyph sits at the top of the quad; shift it down
                // by half the height difference so it is visually centred in
                // whatever row the caller computed y for.
                let y_off = ((atlas.cell_h as i32 - atlas.ui_cell_h as i32).max(0) as f32
                    * 0.5)
                    .floor();
                let mut cx = *x;
                for c in text.chars() {
                    if let Some((uv, adv)) = atlas.uv_for_ui_char(c) {
                        push_quad(&mut out, cx, y + y_off, adv, cell_h, uv, *fg, *bg);
                        cx += adv;
                    } else {
                        cx += atlas.cell_w as f32 * 0.5;
                    }
                }
            }
        }
    }
    out
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn push_quad(
    out: &mut Vec<CellVertex>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    uv: crate::glyph_atlas::GlyphUv,
    fg: [f32; 4],
    bg: [f32; 4],
) {
    let (x1, y1) = (x + w, y + h);
    let tl = vert(x,  y,  uv.u0, uv.v0, fg, bg);
    let tr = vert(x1, y,  uv.u1, uv.v0, fg, bg);
    let bl = vert(x,  y1, uv.u0, uv.v1, fg, bg);
    let br = vert(x1, y1, uv.u1, uv.v1, fg, bg);
    out.extend_from_slice(&[tl, tr, bl, tr, br, bl]);
}

fn vert(px: f32, py: f32, u: f32, v: f32, fg: [f32; 4], bg: [f32; 4]) -> CellVertex {
    CellVertex { pos: [px, py], uv: [u, v], fg, bg }
}
