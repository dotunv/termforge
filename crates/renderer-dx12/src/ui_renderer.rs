//! Chrome UI rendering via the same CellVertex / glyph-atlas pipeline.
//!
//! Every chrome element (tab bar, sidebar, status bar) is expressed as a list
//! of [`UiCommand`]s.  [`build_quads`] converts them to `CellVertex` quads
//! that can be submitted in a single draw call before the terminal panes.
//!
//! **FillRect** exploits the pixel shader identity:
//!     `output = lerp(bg, fg, alpha)`
//! For the space glyph, alpha ≈ 0 everywhere, so output ≈ bg — a solid fill.

use crate::glyph_atlas::GlyphAtlas;
use crate::pipeline::CellVertex;

// ────────────────────────────────────────────────────────────────────────────
// Colour helpers
// ────────────────────────────────────────────────────────────────────────────

pub fn hex(s: &str) -> [f32; 4] {
    let s = s.trim_start_matches('#');
    let r = u8::from_str_radix(&s[0..2], 16).unwrap_or(0);
    let g = u8::from_str_radix(&s[2..4], 16).unwrap_or(0);
    let b = u8::from_str_radix(&s[4..6], 16).unwrap_or(0);
    [srgb(r), srgb(g), srgb(b), 1.0]
}

fn srgb(c: u8) -> f32 {
    let f = c as f32 / 255.0;
    if f <= 0.04045 {
        f / 12.92
    } else {
        ((f + 0.055) / 1.055).powf(2.4)
    }
}

pub const COL_BG: &str = "#0d1117";
pub const COL_PANEL: &str = "#161b22";
pub const COL_BORDER: &str = "#30363d";
pub const COL_TEXT: &str = "#e6edf3";
pub const COL_MUTED: &str = "#8b949e";
pub const COL_GREEN: &str = "#3fb950";
pub const COL_BLUE: &str = "#58a6ff";
pub const COL_PURPLE: &str = "#d2a8ff";
pub const COL_RED: &str = "#f85149";
pub const COL_AMBER: &str = "#e3b341";
pub const COL_ACTIVE_TAB: &str = "#0d1117";
pub const COL_FAINT: &str = "#6e7681"; // text-faint — section labels, timestamps
pub const COL_HOVER: &str = "#1c2128"; // bg-hover — sidebar active/hover
pub const COL_RAISED: &str = "#21262d"; // bg-raised — badges, tooltips

// Block header backgrounds (Section 16.2)
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

// ────────────────────────────────────────────────────────────────────────────
// Command list
// ────────────────────────────────────────────────────────────────────────────

/// An instruction to the UI renderer.
#[derive(Debug, Clone)]
pub enum UiCommand {
    /// Solid-colour rectangle (pixel coordinates).
    FillRect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [f32; 4],
    },

    /// Horizontal 1px border at the bottom of a rect.
    BottomBorder {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [f32; 4],
    },

    /// Axis-aligned text rendered with the glyph atlas.
    DrawText {
        x: f32,
        y: f32,
        text: String,
        fg: [f32; 4],
        bg: [f32; 4],
    },

    /// Small filled square that represents a coloured dot.
    DrawDot {
        cx: f32,
        cy: f32,
        r: f32,
        color: [f32; 4],
    },
}

// ────────────────────────────────────────────────────────────────────────────
// Vertex builder
// ────────────────────────────────────────────────────────────────────────────

/// Convert a list of [`UiCommand`]s into `CellVertex` quads.
pub fn build_quads(cmds: &[UiCommand], atlas: &GlyphAtlas) -> Vec<CellVertex> {
    let mut out = Vec::with_capacity(cmds.len() * 12);
    let space_uv = atlas.uv_for_char(' ');

    for cmd in cmds {
        match cmd {
            UiCommand::FillRect { x, y, w, h, color } => {
                push_quad(
                    &mut out,
                    *x,
                    *y,
                    *w,
                    *h,
                    space_uv.u0,
                    space_uv.v0,
                    space_uv.u1,
                    space_uv.v1,
                    [0.0; 4],
                    *color,
                );
            }

            UiCommand::BottomBorder { x, y, w, h, color } => {
                // 1px strip at bottom.
                push_quad(
                    &mut out,
                    *x,
                    y + h - 1.0,
                    *w,
                    1.0,
                    space_uv.u0,
                    space_uv.v0,
                    space_uv.u1,
                    space_uv.v1,
                    [0.0; 4],
                    *color,
                );
            }

            UiCommand::DrawText { x, y, text, fg, bg } => {
                let cw = atlas.cell_w as f32;
                let ch = atlas.cell_h as f32;
                for (i, c) in text.chars().enumerate() {
                    let cx = x + i as f32 * cw;
                    let uv = atlas.uv_for_char(c);
                    push_quad(
                        &mut out, cx, *y, cw, ch, uv.u0, uv.v0, uv.u1, uv.v1, *fg, *bg,
                    );
                }
            }

            UiCommand::DrawDot { cx, cy, r, color } => {
                let x = cx - r;
                let y = cy - r;
                let side = r * 2.0;
                push_quad(
                    &mut out,
                    x,
                    y,
                    side,
                    side,
                    space_uv.u0,
                    space_uv.v0,
                    space_uv.u1,
                    space_uv.v1,
                    [0.0; 4],
                    *color,
                );
            }
        }
    }
    out
}

// ────────────────────────────────────────────────────────────────────────────
// Internal helpers
// ────────────────────────────────────────────────────────────────────────────

fn push_quad(
    out: &mut Vec<CellVertex>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    u0: f32,
    v0: f32,
    u1: f32,
    v1: f32,
    fg: [f32; 4],
    bg: [f32; 4],
) {
    let (x1, y1) = (x + w, y + h);
    let tl = vert(x, y, u0, v0, fg, bg);
    let tr = vert(x1, y, u1, v0, fg, bg);
    let bl = vert(x, y1, u0, v1, fg, bg);
    let br = vert(x1, y1, u1, v1, fg, bg);
    out.extend_from_slice(&[tl, tr, bl, tr, br, bl]);
}

fn vert(px: f32, py: f32, u: f32, v: f32, fg: [f32; 4], bg: [f32; 4]) -> CellVertex {
    CellVertex {
        pos: [px, py],
        uv: [u, v],
        fg,
        bg,
    }
}
