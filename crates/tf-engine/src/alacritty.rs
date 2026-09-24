use std::sync::{Arc, Mutex, PoisonError};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::{Config, TermMode};
use alacritty_terminal::vte::ansi::Processor;
use alacritty_terminal::Term;

use crate::{Cursor, GridSize, Snapshot, TerminalEngine};

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
        let rows = (0..grid.screen_lines())
            .map(|l| {
                let row = &grid[Line(l as i32)];
                let mut s = String::with_capacity(cols);
                for c in 0..cols {
                    let cell = &row[Column(c)];
                    // Skip the spacer that follows a wide character.
                    if cell
                        .flags
                        .contains(alacritty_terminal::term::cell::Flags::WIDE_CHAR_SPACER)
                    {
                        continue;
                    }
                    s.push(cell.c);
                    if let Some(zw) = cell.zerowidth() {
                        s.extend(zw);
                    }
                }
                s
            })
            .collect();
        let point = grid.cursor.point;
        Snapshot {
            size: self.size,
            rows,
            cursor: Cursor {
                row: point.line.0.max(0) as u16,
                col: point.column.0 as u16,
            },
            alt_screen: self.term.mode().contains(TermMode::ALT_SCREEN),
            history: grid.history_size(),
        }
    }

    fn cursor_line_abs(&self) -> usize {
        let grid = self.term.grid();
        grid.history_size() + grid.cursor.point.line.0.max(0) as usize
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
        assert!(e.snapshot().alt_screen);
        e.feed(b"\x1b[?1049l");
        assert!(!e.snapshot().alt_screen);
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
    fn resize_reflows_without_panicking() {
        let mut e = engine();
        e.feed(b"abcdefghijklmnopqrstuvwxyz");
        e.resize(GridSize::new(10, 5));
        e.resize(GridSize::new(40, 10));
        assert!(e.snapshot().text().contains("abcdefghij"));
    }
}
