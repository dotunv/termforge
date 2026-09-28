//! The terminal view: paints a session's grid with GPUI and forwards input.
//!
//! Everything terminal-specific (emulation, key encoding, block tracking,
//! colours) lives in framework-free crates; this file only maps those
//! results onto GPUI primitives.

use std::sync::Arc;

use futures::StreamExt;
use gpui::{
    canvas, div, fill, point, prelude::*, px, size, App, Bounds, ClipboardItem, Context,
    FocusHandle, Font, FontStyle, FontWeight, Hsla, KeyDownEvent, MouseButton, Pixels,
    ScrollWheelEvent, SharedString, StrikethroughStyle, Task, TextRun, UnderlineStyle, Window,
};
use tf_engine::{Cell, CellFlags, Color, GridSize, Scroll, Snapshot, TerminalEngine};
use tf_input::{encode_key, encode_paste, InputModes, Key, Mods};
use tf_session::{BlockState, LiveSession, SessionEvent, SpawnOptions};
use tf_ui::{tokens, Rgb, Theme};

use crate::fonts::MonoFont;

const PAD_X: f32 = 14.0;
const PAD_Y: f32 = 8.0;
const GUTTER_X: f32 = 5.0;
const GUTTER_W: f32 = 2.0;

pub fn hsla(c: Rgb) -> Hsla {
    gpui::rgb(u32::from(c.r) << 16 | u32::from(c.g) << 8 | u32::from(c.b)).into()
}

fn with_alpha(mut c: Hsla, a: f32) -> Hsla {
    c.a = a;
    c
}

/// Cell metrics for the current font, measured by the platform text system.
#[derive(Debug, Clone, Copy)]
struct Metrics {
    cell_w: Pixels,
    line_h: Pixels,
}

/// Per-block summary copied out of the session for painting.
#[derive(Debug, Clone, Copy)]
struct BlockMark {
    start: usize,
    end: Option<usize>,
    state: BlockState,
    exit_code: Option<i32>,
}

pub struct TerminalView {
    session: Option<Arc<LiveSession>>,
    spawn: SpawnOptions,
    focus: FocusHandle,
    theme: Arc<Theme>,
    font: MonoFont,
    font_size: f32,
    scroll_accum: f32,
    cwd: Option<String>,
    title: Option<String>,
    last_exit: Option<i32>,
    commands: usize,
    error: Option<String>,
    _pump: Option<Task<()>>,
}

impl TerminalView {
    pub fn new(
        spawn: SpawnOptions,
        theme: Arc<Theme>,
        font: MonoFont,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        let mut view = Self {
            session: None,
            spawn,
            focus,
            theme,
            font,
            font_size: 14.0,
            scroll_accum: 0.0,
            cwd: None,
            title: None,
            last_exit: None,
            commands: 0,
            error: None,
            _pump: None,
        };
        view.start(cx);
        view
    }

    fn start(&mut self, cx: &mut Context<'_, Self>) {
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();
        let waker: tf_session::Waker = Arc::new(move || {
            let _ = tx.unbounded_send(());
        });
        match LiveSession::spawn(self.spawn.clone(), waker) {
            Ok(session) => {
                self.session = Some(Arc::new(session));
                self.error = None;
                self.last_exit = None;
                self.commands = 0;
            }
            Err(e) => {
                tracing::error!("failed to start shell: {e:#}");
                self.error = Some(format!("{e:#}"));
                self.session = None;
            }
        }
        self._pump = Some(cx.spawn(async move |this, cx| {
            while rx.next().await.is_some() {
                // Coalesce bursts: one repaint per batch of wakes.
                while rx.try_recv().is_ok() {}
                if this.update(cx, |view, cx| view.on_output(cx)).is_err() {
                    break;
                }
            }
        }));
    }

    fn on_output(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(session) = &self.session {
            for event in session.take_events() {
                match event {
                    SessionEvent::Cwd(cwd) => self.cwd = Some(cwd),
                    SessionEvent::BlockFinished { exit_code, .. } => {
                        self.last_exit = exit_code;
                        self.commands += 1;
                    }
                    SessionEvent::Notify { title, body } => {
                        tracing::info!(?title, %body, "terminal notification");
                    }
                }
            }
            self.title = session.with(|p| p.engine().title());
        }
        cx.notify();
    }

    fn metrics(&self, window: &Window) -> Metrics {
        let ts = window.text_system();
        let font_size = px(self.font_size);
        let id = ts.resolve_font(&self.font.regular());
        let cell_w = ts
            .advance(id, font_size, 'm')
            .map(|s| s.width)
            .unwrap_or(px(self.font_size * 0.6));
        Metrics {
            cell_w,
            line_h: px((self.font_size * 1.35).round()),
        }
    }

    fn write(&self, bytes: &[u8]) {
        if let Some(s) = &self.session {
            if let Err(e) = s.write(bytes) {
                tracing::warn!("write to pty failed: {e:#}");
            }
        }
    }

    fn input_modes(&self) -> InputModes {
        self.session
            .as_ref()
            .map(|s| {
                let m = s.with(|p| p.engine().modes());
                InputModes {
                    app_cursor: m.app_cursor,
                    bracketed_paste: m.bracketed_paste,
                }
            })
            .unwrap_or_default()
    }

    fn exited(&self) -> Option<Option<u32>> {
        self.session.as_ref().and_then(|s| s.exit_status())
    }

    fn key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<'_, Self>) {
        let ks = &ev.keystroke;
        let m = ks.modifiers;
        let mods = Mods {
            ctrl: m.control,
            alt: m.alt,
            shift: m.shift,
        };
        let key = ks.key.as_str();

        // App shortcuts first (Ctrl+Shift+... never collides with shells).
        if m.control && m.shift && key.eq_ignore_ascii_case("v") || (m.shift && key == "insert") {
            self.paste(cx);
            cx.stop_propagation();
            return;
        }
        if m.control && m.shift && key.eq_ignore_ascii_case("c") {
            self.copy_screen(cx);
            cx.stop_propagation();
            return;
        }
        if m.control && !m.alt && !m.shift && matches!(key, "=" | "+" | "-" | "0") {
            self.font_size = match key {
                "-" => (self.font_size - 1.0).max(8.0),
                "0" => 14.0,
                _ => (self.font_size + 1.0).min(32.0),
            };
            cx.notify();
            cx.stop_propagation();
            return;
        }
        if m.shift && !m.control && matches!(key, "pageup" | "pagedown") {
            self.scroll(
                if key == "pageup" {
                    Scroll::PageUp
                } else {
                    Scroll::PageDown
                },
                cx,
            );
            cx.stop_propagation();
            return;
        }

        if let Some(Some(_)) = self.exited() {
            if key == "enter" {
                self.start(cx);
                window.focus(&self.focus);
                cx.notify();
            }
            cx.stop_propagation();
            return;
        }

        let logical = Key::from_name(key).or_else(|| {
            // Prefer the layout-resolved character; fall back to the key
            // itself for Ctrl chords, where no character is produced.
            ks.key_char
                .clone()
                .or_else(|| (m.control || m.alt).then(|| key.to_owned()))
                .map(Key::Text)
        });
        let Some(logical) = logical else { return };
        if let Some(bytes) = encode_key(&logical, mods, self.input_modes()) {
            self.scroll(Scroll::Bottom, cx);
            self.write(&bytes);
            cx.stop_propagation();
        }
    }

    fn paste(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
            let bytes = encode_paste(&text, self.input_modes());
            self.scroll(Scroll::Bottom, cx);
            self.write(&bytes);
        }
    }

    fn scroll(&mut self, scroll: Scroll, cx: &mut Context<'_, Self>) {
        if let Some(s) = &self.session {
            s.with_mut(|p| p.engine_mut().scroll(scroll));
            cx.notify();
        }
    }

    fn scroll_wheel(
        &mut self,
        ev: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let line_h = self.metrics(window).line_h;
        self.scroll_accum += ev.delta.pixel_delta(line_h).y / line_h;
        let lines = self.scroll_accum.trunc() as i32;
        if lines == 0 {
            return;
        }
        self.scroll_accum -= lines as f32;
        let alt = self
            .session
            .as_ref()
            .is_some_and(|s| s.with(|p| p.engine().modes().alt_screen));
        if alt {
            // Alternate scroll mode: full-screen apps get arrow keys.
            let key = if lines > 0 { Key::Up } else { Key::Down };
            if let Some(b) = encode_key(&key, Mods::NONE, self.input_modes()) {
                for _ in 0..lines.unsigned_abs().min(10) {
                    self.write(&b);
                }
            }
        } else {
            self.scroll(Scroll::Lines(lines * 3), cx);
        }
    }

    /// Copy the visible screen as plain text (selection lands in Phase 2).
    fn copy_screen(&self, cx: &mut Context<'_, Self>) {
        if let Some(s) = &self.session {
            cx.write_to_clipboard(ClipboardItem::new_string(s.snapshot().text()));
        }
    }

    fn header(&self) -> impl IntoElement {
        let t = &self.theme;
        let shell = self.spawn.profile.name.clone();
        let location = self
            .title
            .clone()
            .or_else(|| self.cwd.clone())
            .unwrap_or_default();
        div()
            .h(px(tokens::layout::TAB_H + 6.0))
            .px(px(tokens::space::MD))
            .flex()
            .items_center()
            .gap(px(tokens::space::SM))
            .bg(hsla(t.bg_app))
            .border_b_1()
            .border_color(hsla(t.border))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(tokens::space::SM))
                    .h(px(tokens::layout::TAB_H - 4.0))
                    .px(px(tokens::space::MD))
                    .rounded(px(tokens::radius::MD))
                    .bg(hsla(t.bg_elevated))
                    .border_1()
                    .border_color(hsla(t.border))
                    .child(
                        div()
                            .size(px(6.0))
                            .rounded_full()
                            .bg(hsla(self.status_color())),
                    )
                    .child(
                        div()
                            .text_color(hsla(t.text))
                            .child(SharedString::from(shell)),
                    )
                    .child(
                        div()
                            .text_color(hsla(t.text_faint))
                            .max_w(px(420.0))
                            .overflow_hidden()
                            .child(SharedString::from(location)),
                    ),
            )
    }

    fn status_color(&self) -> Rgb {
        let t = &self.theme;
        match (self.exited(), self.last_exit) {
            (Some(_), _) => t.text_faint,
            (None, Some(c)) if c != 0 => t.danger,
            _ => t.success,
        }
    }

    fn status_bar(&self, grid: Option<GridSize>) -> impl IntoElement {
        let t = &self.theme;
        let mut left = format!(
            "{} command{}",
            self.commands,
            if self.commands == 1 { "" } else { "s" }
        );
        if let Some(code) = self.last_exit {
            left.push_str(&format!("  ·  last exit {code}"));
        }
        let right = grid
            .map(|g| format!("{}×{}", g.cols, g.rows))
            .unwrap_or_default();
        div()
            .h(px(tokens::layout::STATUSBAR_H))
            .px(px(tokens::space::MD))
            .flex()
            .items_center()
            .justify_between()
            .text_size(px(tokens::text::XS))
            .text_color(hsla(t.text_faint))
            .bg(hsla(t.bg_app))
            .border_t_1()
            .border_color(hsla(t.border))
            .child(SharedString::from(left))
            .child(SharedString::from(right))
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = Arc::clone(&self.theme);
        let metrics = self.metrics(window);
        let font = self.font.clone();
        let font_size = px(self.font_size);
        let focused = self.focus.is_focused(window);
        let session = self.session.clone();
        let grid = session.as_ref().map(|s| s.with(|p| p.engine().size()));

        let exit_overlay = self.exited().flatten().map(|code| {
            div()
                .absolute()
                .bottom(px(tokens::space::LG))
                .left(px(tokens::space::LG))
                .px(px(tokens::space::MD))
                .py(px(tokens::space::SM))
                .rounded(px(tokens::radius::LG))
                .bg(hsla(theme.bg_elevated))
                .border_1()
                .border_color(hsla(theme.border_strong))
                .text_color(hsla(theme.text_muted))
                .child(SharedString::from(format!(
                    "Process exited with code {code}. Press Enter to restart."
                )))
        });
        let error = self.error.clone().map(|e| {
            div()
                .p(px(tokens::space::XL))
                .text_color(hsla(theme.danger))
                .child(SharedString::from(format!(
                    "Could not start the shell: {e}"
                )))
        });

        let grid_canvas = canvas(
            {
                let session = session.clone();
                move |bounds: Bounds<Pixels>, _window: &mut Window, _cx: &mut App| {
                    let cols = ((bounds.size.width - px(PAD_X * 2.0)) / metrics.cell_w).floor();
                    let rows = ((bounds.size.height - px(PAD_Y * 2.0)) / metrics.line_h).floor();
                    let size = GridSize::new(cols.max(2.0) as u16, rows.max(1.0) as u16);
                    let snapshot = session.as_ref().map(|s| {
                        if let Err(e) = s.resize(size) {
                            tracing::warn!("resize failed: {e:#}");
                        }
                        let blocks = s.with(|p| {
                            p.blocks()
                                .iter()
                                .map(|b| BlockMark {
                                    start: b.prompt_line,
                                    end: b.end_line,
                                    state: b.state,
                                    exit_code: b.exit_code,
                                })
                                .collect::<Vec<_>>()
                        });
                        (s.snapshot(), blocks)
                    });
                    snapshot
                }
            },
            move |bounds, snapshot, window, cx| {
                if let Some((snap, blocks)) = snapshot {
                    paint_grid(
                        &snap, &blocks, bounds, &theme, &font, font_size, metrics, focused, window,
                        cx,
                    );
                }
            },
        )
        .size_full();

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(hsla(self.theme.term_bg))
            .font_family(self.font.family.clone())
            .text_size(px(tokens::text::SM))
            .child(self.header())
            .child(
                div()
                    .id("terminal")
                    .relative()
                    .flex_1()
                    .overflow_hidden()
                    .track_focus(&self.focus)
                    .cursor_text()
                    .on_key_down(cx.listener(Self::key_down))
                    .on_scroll_wheel(cx.listener(Self::scroll_wheel))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            window.focus(&this.focus);
                            cx.notify();
                        }),
                    )
                    // Right-click pastes, as in Windows Terminal and conhost.
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, _, _, cx| this.paste(cx)),
                    )
                    .child(grid_canvas)
                    .children(error)
                    .children(exit_overlay),
            )
            .child(self.status_bar(grid))
    }
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

    fn run(&self, cell: &Cell, len: usize, fg: Hsla) -> TextRun {
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
            underline: f.contains(CellFlags::UNDERLINE).then_some(UnderlineStyle {
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

#[allow(clippy::too_many_arguments)]
fn paint_grid(
    snap: &Snapshot,
    blocks: &[BlockMark],
    bounds: Bounds<Pixels>,
    theme: &Theme,
    font: &MonoFont,
    font_size: Pixels,
    m: Metrics,
    focused: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let p = Painter { theme, font };
    let origin = point(bounds.origin.x + px(PAD_X), bounds.origin.y + px(PAD_Y));
    let cell_origin = |row: usize, col: usize| {
        point(
            origin.x + m.cell_w * col as f32,
            origin.y + m.line_h * row as f32,
        )
    };

    // Block chrome: a separator above each prompt and a status bar in the
    // gutter. Blocks are virtual (docs/adr/0004-virtual-blocks.md).
    let top = snap.top_line_abs();
    let rows = snap.lines.len();
    // Full-screen apps own the whole grid; no block chrome there.
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

    for (row, cells) in snap.lines.iter().enumerate() {
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
            window.paint_quad(fill(
                Bounds::new(
                    cell_origin(row, start),
                    size(m.cell_w * (col - start) as f32, m.line_h),
                ),
                bg,
            ));
        }

        // Text, in segments broken at wide characters so every glyph stays
        // on the cell grid.
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
            let line = window.text_system().shape_line(
                SharedString::from(std::mem::take(seg)),
                font_size,
                runs,
                force.then_some(m.cell_w),
            );
            runs.clear();
            let _ = line.paint(cell_origin(row, at), m.line_h, window, cx);
        };
        for (col, cell) in cells.iter().enumerate() {
            if cell.flags.contains(CellFlags::WIDE_SPACER) {
                continue;
            }
            let (fg, _) = p.colors(cell);
            let mut text = String::new();
            text.push(cell.ch);
            if let Some(zw) = &cell.zerowidth {
                text.push_str(zw);
            }
            if cell.flags.contains(CellFlags::WIDE) {
                flush(&mut seg, &mut runs, seg_start, true, window, cx);
                let run = p.run(cell, text.len(), fg);
                let mut single = text;
                flush(&mut single, &mut vec![run], col, false, window, cx);
                seg_start = col + 2;
                continue;
            }
            if seg.is_empty() {
                seg_start = col;
            }
            let run = p.run(cell, text.len(), fg);
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
                    let run = p.run(cell, text.len(), hsla(theme.term_bg));
                    let line = window
                        .text_system()
                        .shape_line(text, font_size, &[run], None);
                    let _ = line.paint(rect.origin, m.line_h, window, cx);
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
