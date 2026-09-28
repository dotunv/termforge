use std::sync::{Arc, Mutex, PoisonError};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll as AScroll};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell as ACell, Flags};
use alacritty_terminal::term::{Config, TermMode};
use alacritty_terminal::vte::ansi::{Color as AColor, NamedColor, Processor};
use alacritty_terminal::Term;

use crate::{Cell, CellFlags, Color, Cursor, GridSize, Modes, Scroll, Snapshot, TerminalEngine};

const DEFAULT_SCROLLBACK: usize = 10_000;

#[derive(Debug, Default)]
struct Shared {
    replies: Vec<u8>,
    title: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct Listener(Arc<Mutex<Shared>>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let mut s = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        match event {
            Event::PtyWrite(text) => s.replies.extend_from_slice(text.as_bytes()),
            Event::Title(t) => s.title = Some(t),
            Event::ResetTitle => s.title = None,
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Dims(GridSize);

impl Dimensions for Dims {
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }
    fn screen_lines(&self) -> usize {
        self.0.rows as usize
    }
    fn columns(&self) -> usize {
        self.0.cols as usize
    }
}

/// [`TerminalEngine`] backed by `alacritty_terminal`.
pub struct AlacrittyEngine {
    term: Term<Listener>,
    parser: Processor,
    shared: Listener,
    size: GridSize,
}

impl std::fmt::Debug for AlacrittyEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AlacrittyEngine")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

impl AlacrittyEngine {
    pub fn new(size: GridSize) -> Self {
        Self::with_scrollback(size, DEFAULT_SCROLLBACK)
    }

    pub fn with_scrollback(size: GridSize, scrollback: usize) -> Self {
        let shared = Listener::default();
        let config = Config {
            scrolling_history: scrollback,
            ..Config::default()
        };
        let term = Term::new(config, &Dims(size), shared.clone());
        Self {
            term,
            parser: Processor::new(),
            shared,
            size,
        }
    }
}

impl TerminalEngine for AlacrittyEngine {
    fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    fn resize(&mut self, size: GridSize) {
        self.size = size;
        self.term.resize(Dims(size));
    }

    fn size(&self) -> GridSize {
        self.size
    }

    fn snapshot(&self) -> Snapshot {
        let grid = self.term.grid();
        let cols = grid.columns();
        let offset = grid.display_offset();
        let lines = (0..grid.screen_lines())
            .map(|l| {
                let row = &grid[Line(l as i32 - offset as i32)];
                (0..cols).map(|c| convert_cell(&row[Column(c)])).collect()
            })
            .collect();
        let point = grid.cursor.point;
        // Cursor position is relative to the bottom-anchored screen; shift it
        // into the viewport and hide it when scrolled out of view.
        let cursor_row = point.line.0 + offset as i32;
        let modes = self.modes();
        Snapshot {
            size: self.size,
            lines,
            cursor: Cursor {
                row: cursor_row.max(0) as u16,
                col: point.column.0 as u16,
            },
            modes: Modes {
                cursor_visible: modes.cursor_visible && cursor_row < grid.screen_lines() as i32,
                ..modes
            },
            history: grid.history_size(),
            display_offset: offset,
        }
    }

    fn cursor_line_abs(&self) -> usize {
        let grid = self.term.grid();
        grid.history_size() + grid.cursor.point.line.0.max(0) as usize
    }

    fn history_size(&self) -> usize {
        self.term.grid().history_size()
    }

    fn take_replies(&mut self) -> Vec<u8> {
        let mut s = self.shared.0.lock().unwrap_or_else(PoisonError::into_inner);
        std::mem::take(&mut s.replies)
    }

    fn title(&self) -> Option<String> {
        self.shared
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .title
            .clone()
    }

    fn modes(&self) -> Modes {
        let m = self.term.mode();
        Modes {
            app_cursor: m.contains(TermMode::APP_CURSOR),
            bracketed_paste: m.contains(TermMode::BRACKETED_PASTE),
            alt_screen: m.contains(TermMode::ALT_SCREEN),
            cursor_visible: m.contains(TermMode::SHOW_CURSOR),
        }
    }

    fn scroll(&mut self, scroll: Scroll) {
        self.term.scroll_display(match scroll {
            Scroll::Lines(n) => AScroll::Delta(n),
            Scroll::PageUp => AScroll::PageUp,
            Scroll::PageDown => AScroll::PageDown,
            Scroll::Top => AScroll::Top,
            Scroll::Bottom => AScroll::Bottom,
        });
    }
}

fn convert_color(c: AColor, flags: &mut CellFlags) -> Color {
    match c {
        AColor::Spec(rgb) => Color::Rgb(rgb.r, rgb.g, rgb.b),
        AColor::Indexed(i) => Color::Indexed(i),
        AColor::Named(n) => {
            let idx = n as usize;
            if idx < 16 {
                return Color::Indexed(idx as u8);
            }
            match n {
                NamedColor::Background => Color::DefaultBg,
                NamedColor::DimForeground => {
                    flags.insert(CellFlags::DIM);
                    Color::DefaultFg
                }
                NamedColor::DimBlack
                | NamedColor::DimRed
                | NamedColor::DimGreen
                | NamedColor::DimYellow
                | NamedColor::DimBlue
                | NamedColor::DimMagenta
                | NamedColor::DimCyan
                | NamedColor::DimWhite => {
                    flags.insert(CellFlags::DIM);
                    Color::Indexed((idx - NamedColor::DimBlack as usize) as u8)
                }
                _ => Color::DefaultFg,
            }
        }
    }
}

fn convert_cell(cell: &ACell) -> Cell {
    let f = cell.flags;
    let mut flags = CellFlags::empty();
    for (from, to) in [
        (Flags::BOLD, CellFlags::BOLD),
        (Flags::ITALIC, CellFlags::ITALIC),
        (Flags::INVERSE, CellFlags::INVERSE),
        (Flags::DIM, CellFlags::DIM),
        (Flags::STRIKEOUT, CellFlags::STRIKE),
        (Flags::HIDDEN, CellFlags::HIDDEN),
        (Flags::WIDE_CHAR, CellFlags::WIDE),
        (Flags::WIDE_CHAR_SPACER, CellFlags::WIDE_SPACER),
    ] {
        if f.contains(from) {
            flags.insert(to);
        }
    }
    if f.intersects(Flags::ALL_UNDERLINES) {
        flags.insert(CellFlags::UNDERLINE);
    }
    let fg = convert_color(cell.fg, &mut flags);
    let bg = convert_color(cell.bg, &mut flags);
    Cell {
        ch: if f.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
            ' '
        } else {
            cell.c
        },
        zerowidth: cell
            .zerowidth()
            .map(|zw| zw.iter().collect::<String>().into_boxed_str()),
        fg,
        bg,
        flags,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> AlacrittyEngine {
        AlacrittyEngine::new(GridSize::new(20, 5))
    }

    #[test]
    fn plain_text_and_newlines() {
        let mut e = engine();
        e.feed(b"hello\r\nworld");
        assert_eq!(e.snapshot().text(), "hello\nworld");
        assert_eq!(e.snapshot().cursor, Cursor { row: 1, col: 5 });
    }

    #[test]
    fn utf8_split_across_feeds_is_not_corrupted() {
        let mut e = engine();
        let s = "┌─é日本┐".as_bytes();
        for b in s {
            e.feed(std::slice::from_ref(b));
        }
        assert_eq!(e.snapshot().text(), "┌─é日本┐");
    }

    #[test]
    fn scrolling_moves_lines_to_history_and_abs_line_grows() {
        let mut e = engine();
        for i in 0..8 {
            e.feed(format!("line{i}\r\n").as_bytes());
        }
        let snap = e.snapshot();
        assert_eq!(snap.history, 4);
        assert_eq!(e.cursor_line_abs(), 8);
        insta::assert_snapshot!(snap.text(), @r"
        line4
        line5
        line6
        line7
        ");
    }

    #[test]
    fn alt_screen_flag() {
        let mut e = engine();
        e.feed(b"\x1b[?1049h");
        assert!(e.snapshot().modes.alt_screen);
        e.feed(b"\x1b[?1049l");
        assert!(!e.snapshot().modes.alt_screen);
    }

    #[test]
    fn device_attribute_query_produces_reply() {
        let mut e = engine();
        e.feed(b"\x1b[c");
        let reply = e.take_replies();
        assert!(reply.starts_with(b"\x1b[?"), "unexpected reply {reply:?}");
        assert!(e.take_replies().is_empty());
    }

    #[test]
    fn title_is_tracked() {
        let mut e = engine();
        e.feed(b"\x1b]0;nvim\x07");
        assert_eq!(e.title().as_deref(), Some("nvim"));
    }

    #[test]
    fn colors_and_attributes() {
        let mut e = engine();
        e.feed(b"\x1b[1;31mR\x1b[0m\x1b[38;5;200mX\x1b[48;2;1;2;3mY\x1b[0m\x1b[7mI");
        let s = e.snapshot();
        let row = &s.lines[0];
        assert_eq!(row[0].fg, Color::Indexed(1));
        assert!(row[0].flags.contains(CellFlags::BOLD));
        assert_eq!(row[1].fg, Color::Indexed(200));
        assert_eq!(row[2].bg, Color::Rgb(1, 2, 3));
        assert!(row[3].flags.contains(CellFlags::INVERSE));
        assert_eq!(row[4], Cell::default());
    }

    #[test]
    fn wide_chars_keep_column_alignment() {
        let mut e = engine();
        e.feed("日x".as_bytes());
        let s = e.snapshot();
        assert_eq!(s.lines[0].len(), 20);
        assert!(s.lines[0][0].flags.contains(CellFlags::WIDE));
        assert!(s.lines[0][1].flags.contains(CellFlags::WIDE_SPACER));
        assert_eq!(s.lines[0][2].ch, 'x');
    }

    #[test]
    fn modes_are_reported() {
        let mut e = engine();
        assert!(e.modes().cursor_visible);
        e.feed(b"\x1b[?1h\x1b[?2004h\x1b[?25l");
        let m = e.modes();
        assert!(m.app_cursor && m.bracketed_paste && !m.cursor_visible);
    }

    #[test]
    fn scrolling_the_viewport() {
        let mut e = engine();
        for i in 0..10 {
            e.feed(format!("line{i}\r\n").as_bytes());
        }
        e.scroll(Scroll::Lines(3));
        let s = e.snapshot();
        assert_eq!(s.display_offset, 3);
        assert_eq!(s.row_text(0).trim_end(), "line3");
        assert_eq!(s.top_line_abs(), 3);
        assert!(!s.modes.cursor_visible, "cursor row is below the viewport");
        e.scroll(Scroll::Bottom);
        assert_eq!(e.snapshot().display_offset, 0);
    }

    #[test]
    fn resize_reflows_without_panicking() {
        let mut e = engine();
        e.feed(b"abcdefghijklmnopqrstuvwxyz");
        e.resize(GridSize::new(10, 5));
        e.resize(GridSize::new(40, 10));
        assert!(e.snapshot().text().contains("abcdefghij"));
    }
}
