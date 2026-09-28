//! Grid painting. Pure function of a snapshot plus overlays, so the view
//! decides *what* is highlighted and this file decides *how* it looks.

use gpui::{
    fill, point, px, size, App, Bounds, Font, FontStyle, FontWeight, Hsla, Pixels, SharedString,
    StrikethroughStyle, TextRun, UnderlineStyle, Window,
};
use tf_engine::{text::in_range, Cell, CellFlags, Color, Point, Snapshot};
use tf_session::BlockState;
use tf_ui::{Rgb, Theme};

use crate::fonts::MonoFont;

pub const PAD_X: f32 = 14.0;
pub const PAD_Y: f32 = 8.0;
const GUTTER_X: f32 = 5.0;
const GUTTER_W: f32 = 2.0;

pub fn hsla(c: Rgb) -> Hsla {
    gpui::rgb(u32::from(c.r) << 16 | u32::from(c.g) << 8 | u32::from(c.b)).into()
}

pub fn with_alpha(mut c: Hsla, a: f32) -> Hsla {
    c.a = a;
    c
}

/// Cell metrics for the current font, measured by the platform text system.
#[derive(Debug, Clone, Copy)]
pub struct Metrics {
    pub cell_w: Pixels,
    pub line_h: Pixels,
}

impl Metrics {
    /// Top-left of the cell grid inside the canvas bounds.
    pub fn origin(bounds: Bounds<Pixels>) -> gpui::Point<Pixels> {
        point(bounds.origin.x + px(PAD_X), bounds.origin.y + px(PAD_Y))
    }

    /// Viewport cell under a window position, clamped to the grid. The
    /// column rounds at the cell midpoint so selections feel natural.
    pub fn cell_at(
        &self,
        bounds: Bounds<Pixels>,
        pos: gpui::Point<Pixels>,
        cols: u16,
        rows: u16,
    ) -> (u16, u16) {
        let o = Self::origin(bounds);
        let x = ((pos.x - o.x) / self.cell_w).max(0.0);
        let y = ((pos.y - o.y) / self.line_h).max(0.0);
        let col = (x.floor() as u16).min(cols.saturating_sub(1));
        let row = (y.floor() as u16).min(rows.saturating_sub(1));
        (col, row)
    }
}

/// Per-block summary copied out of the session for painting.
#[derive(Debug, Clone, Copy)]
pub struct BlockMark {
    pub start: usize,
    pub end: Option<usize>,
    pub state: BlockState,
    pub exit_code: Option<i32>,
}

/// Everything drawn on top of the cells, in absolute coordinates.
#[derive(Debug, Clone, Default)]
pub struct Overlay {
    pub selection: Option<(Point, Point)>,
    pub matches: Vec<(Point, Point)>,
    pub current_match: Option<(Point, Point)>,
    pub link: Option<(Point, Point)>,
}

struct Painter<'a> {
    theme: &'a Theme,
    font: &'a MonoFont,
}

impl Painter<'_> {
    fn resolve(&self, c: Color) -> Rgb {
        match c {
            Color::DefaultFg => self.theme.term_fg,
            Color::DefaultBg => self.theme.term_bg,
            Color::Indexed(i) => self.theme.indexed(i),
            Color::Rgb(r, g, b) => Rgb::new(r, g, b),
        }
    }

    /// Effective (fg, bg) after inverse/dim/hidden.
    fn colors(&self, cell: &Cell) -> (Hsla, Option<Hsla>) {
        let mut fg = self.resolve(cell.fg);
        let mut bg = self.resolve(cell.bg);
        let mut bg_set = cell.bg != Color::DefaultBg;
        if cell.flags.contains(CellFlags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
            bg_set = true;
        }
        let mut fg = hsla(fg);
        if cell.flags.contains(CellFlags::DIM) {
            fg = with_alpha(fg, 0.6);
        }
        if cell.flags.contains(CellFlags::HIDDEN) {
            fg = with_alpha(fg, 0.0);
        }
        (fg, bg_set.then(|| hsla(bg)))
    }

    fn run(&self, cell: &Cell, len: usize, fg: Hsla, underline: bool) -> TextRun {
        let f = cell.flags;
        let mut font: Font = self.font.regular();
        if f.contains(CellFlags::BOLD) {
            font.weight = FontWeight::BOLD;
        }
        if f.contains(CellFlags::ITALIC) {
            font.style = FontStyle::Italic;
        }
        TextRun {
            len,
            font,
            color: fg,
            background_color: None,
            underline: (underline || f.contains(CellFlags::UNDERLINE)).then_some(UnderlineStyle {
                thickness: px(1.0),
                color: Some(fg),
                wavy: false,
            }),
            strikethrough: f.contains(CellFlags::STRIKE).then_some(StrikethroughStyle {
                thickness: px(1.0),
                color: Some(fg),
            }),
        }
    }
}

/// Column span of an inclusive absolute range on one row, if it touches it.
fn span_on_row((start, end): (Point, Point), line: usize, cols: usize) -> Option<(usize, usize)> {
    if line < start.line || line > end.line {
        return None;
    }
    let from = if line == start.line {
        start.col as usize
    } else {
        0
    };
    let to = if line == end.line {
        end.col as usize + 1
    } else {
        cols
    };
    (to > from).then_some((from, to.min(cols)))
}

pub struct PaintArgs<'a> {
    pub snap: &'a Snapshot,
    pub blocks: &'a [BlockMark],
    pub overlay: &'a Overlay,
    pub bounds: Bounds<Pixels>,
    pub theme: &'a Theme,
    pub font: &'a MonoFont,
    pub font_size: Pixels,
    pub metrics: Metrics,
    pub focused: bool,
}

pub fn paint_grid(a: PaintArgs<'_>, window: &mut Window, cx: &mut App) {
    let PaintArgs {
        snap,
        blocks,
        overlay,
        bounds,
        theme,
        font,
        font_size,
        metrics: m,
        focused,
    } = a;
    let p = Painter { theme, font };
    let origin = Metrics::origin(bounds);
    let cell_origin = |row: usize, col: usize| {
        point(
            origin.x + m.cell_w * col as f32,
            origin.y + m.line_h * row as f32,
        )
    };
    let cell_rect = |row: usize, from: usize, to: usize| {
        Bounds::new(
            cell_origin(row, from),
            size(m.cell_w * (to - from) as f32, m.line_h),
        )
    };

    let top = snap.top_line_abs();
    let rows = snap.lines.len();
    let cols = snap.size.cols as usize;

    // Block chrome: a separator above each prompt and a status bar in the
    // gutter. Blocks are virtual (docs/adr/0004-virtual-blocks.md). Full-
    // screen apps own the whole grid, so no chrome there.
    let blocks = if snap.modes.alt_screen {
        &[][..]
    } else {
        blocks
    };
    for (i, b) in blocks.iter().enumerate() {
        let end = b.end.unwrap_or(top + rows);
        if end < top || b.start >= top + rows {
            continue;
        }
        let start_row = b.start.saturating_sub(top);
        let end_row = (end.saturating_sub(top)).min(rows);
        if i > 0 && b.start >= top {
            let y = origin.y + m.line_h * start_row as f32;
            window.paint_quad(fill(
                Bounds::new(point(bounds.origin.x, y), size(bounds.size.width, px(1.0))),
                hsla(theme.border),
            ));
        }
        let color = match (b.state, b.exit_code) {
            (BlockState::Running, _) => Some(theme.accent),
            (BlockState::Finished, Some(c)) if c != 0 => Some(theme.danger),
            (BlockState::Finished, _) => Some(theme.border_strong),
            (BlockState::Prompt, _) => None,
        };
        if let Some(color) = color {
            let y0 = origin.y + m.line_h * start_row as f32;
            let h = m.line_h * (end_row.max(start_row + 1) - start_row) as f32;
            window.paint_quad(
                fill(
                    Bounds::new(
                        point(bounds.origin.x + px(GUTTER_X), y0),
                        size(px(GUTTER_W), h),
                    ),
                    hsla(color),
                )
                .corner_radii(px(1.0)),
            );
        }
    }

    let selection_bg = hsla(theme.term_selection);
    let match_bg = with_alpha(hsla(theme.warning), 0.28);
    let current_bg = with_alpha(hsla(theme.warning), 0.65);

    for (row, cells) in snap.lines.iter().enumerate() {
        let line = top + row;

        // Backgrounds, merged into runs.
        let mut col = 0;
        while col < cells.len() {
            let (_, bg) = p.colors(&cells[col]);
            let Some(bg) = bg else {
                col += 1;
                continue;
            };
            let start = col;
            while col < cells.len() && p.colors(&cells[col]).1 == Some(bg) {
                col += 1;
            }
            window.paint_quad(fill(cell_rect(row, start, col), bg));
        }

        // Highlights: search matches under the selection.
        for r in &overlay.matches {
            if let Some((f, t)) = span_on_row(*r, line, cols) {
                window.paint_quad(fill(cell_rect(row, f, t), match_bg));
            }
        }
        if let Some((f, t)) = overlay
            .current_match
            .and_then(|r| span_on_row(r, line, cols))
        {
            window.paint_quad(fill(cell_rect(row, f, t), current_bg).corner_radii(px(2.0)));
        }
        if let Some((f, t)) = overlay.selection.and_then(|r| span_on_row(r, line, cols)) {
            window.paint_quad(fill(cell_rect(row, f, t), selection_bg));
        }

        // Text, in segments broken at wide characters so every glyph stays
        // on the cell grid.
        let link = overlay.link;
        let mut seg = String::new();
        let mut runs: Vec<TextRun> = Vec::new();
        let mut seg_start = 0;
        let flush = |seg: &mut String,
                     runs: &mut Vec<TextRun>,
                     at: usize,
                     force: bool,
                     window: &mut Window,
                     cx: &mut App| {
            if seg.trim_end().is_empty()
                && runs
                    .iter()
                    .all(|r| r.underline.is_none() && r.strikethrough.is_none())
            {
                seg.clear();
                runs.clear();
                return;
            }
            let shaped = window.text_system().shape_line(
                SharedString::from(std::mem::take(seg)),
                font_size,
                runs,
                force.then_some(m.cell_w),
            );
            runs.clear();
            let _ = shaped.paint(cell_origin(row, at), m.line_h, window, cx);
        };
        for (col, cell) in cells.iter().enumerate() {
            if cell.flags.contains(CellFlags::WIDE_SPACER) {
                continue;
            }
            let (fg, _) = p.colors(cell);
            let hovered = link.is_some_and(|r| in_range(r, Point::new(line, col as u16)));
            let mut text = String::new();
            text.push(cell.ch);
            if let Some(zw) = &cell.zerowidth {
                text.push_str(zw);
            }
            if cell.flags.contains(CellFlags::WIDE) {
                flush(&mut seg, &mut runs, seg_start, true, window, cx);
                let run = p.run(cell, text.len(), fg, hovered);
                let mut single = text;
                flush(&mut single, &mut vec![run], col, false, window, cx);
                seg_start = col + 2;
                continue;
            }
            if seg.is_empty() {
                seg_start = col;
            }
            let run = p.run(cell, text.len(), fg, hovered);
            match runs.last_mut() {
                Some(last) if same_style(last, &run) => last.len += run.len,
                _ => runs.push(run),
            }
            seg.push_str(&text);
        }
        flush(&mut seg, &mut runs, seg_start, true, window, cx);
    }

    // Cursor.
    if snap.modes.cursor_visible {
        let c = snap.cursor;
        let (row, col) = (c.row as usize, c.col as usize);
        if row < rows {
            let cursor = hsla(theme.term_cursor);
            let wide = snap.lines[row]
                .get(col)
                .is_some_and(|c| c.flags.contains(CellFlags::WIDE));
            let w = if wide { m.cell_w * 2.0 } else { m.cell_w };
            let rect = Bounds::new(cell_origin(row, col), size(w, m.line_h));
            if focused {
                window.paint_quad(fill(rect, cursor));
                if let Some(cell) = snap.lines[row].get(col).filter(|c| c.ch != ' ') {
                    let text = SharedString::from(cell.ch.to_string());
                    let run = p.run(cell, text.len(), hsla(theme.term_bg), false);
                    let shaped = window
                        .text_system()
                        .shape_line(text, font_size, &[run], None);
                    let _ = shaped.paint(rect.origin, m.line_h, window, cx);
                }
            } else {
                window.paint_quad(
                    fill(rect, with_alpha(cursor, 0.0))
                        .border_widths(px(1.0))
                        .border_color(cursor),
                );
            }
        }
    }
}

fn same_style(a: &TextRun, b: &TextRun) -> bool {
    a.font == b.font
        && a.color == b.color
        && a.underline == b.underline
        && a.strikethrough == b.strikethrough
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_cover_multi_row_ranges() {
        let r = (Point::new(5, 3), Point::new(7, 1));
        assert_eq!(span_on_row(r, 4, 10), None);
        assert_eq!(span_on_row(r, 5, 10), Some((3, 10)));
        assert_eq!(span_on_row(r, 6, 10), Some((0, 10)));
        assert_eq!(span_on_row(r, 7, 10), Some((0, 2)));
        assert_eq!(span_on_row(r, 8, 10), None);
    }
}
