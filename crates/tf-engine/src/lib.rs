//! Terminal emulation behind a small trait.
//!
//! TermForge does not implement its own VT emulator (see
//! `docs/adr/0002-terminal-engine.md`). [`AlacrittyEngine`] wraps
//! `alacritty_terminal`; a `libghostty-vt` implementation can be added later
//! without touching callers.

mod alacritty;
pub mod text;

pub use alacritty::AlacrittyEngine;
pub use text::{Match, Point, Selection, SelectionKind};

/// Grid dimensions in cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridSize {
    pub cols: u16,
    pub rows: u16,
}

impl GridSize {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols: cols.max(2),
            rows: rows.max(1),
        }
    }
}

/// Cursor position within the visible screen (0-based).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub row: u16,
    pub col: u16,
}

/// A cell colour as the program requested it. Resolution to RGB happens in
/// the UI with the active theme, so theme switches never need a re-parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Color {
    DefaultFg,
    DefaultBg,
    /// 0..=15 are the ANSI colours, 16..=255 the xterm cube and greys.
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// Cell attribute bits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct CellFlags(u16);

impl CellFlags {
    pub const BOLD: Self = Self(1);
    pub const ITALIC: Self = Self(1 << 1);
    pub const UNDERLINE: Self = Self(1 << 2);
    pub const INVERSE: Self = Self(1 << 3);
    pub const DIM: Self = Self(1 << 4);
    pub const STRIKE: Self = Self(1 << 5);
    pub const HIDDEN: Self = Self(1 << 6);
    /// First half of a double-width character.
    pub const WIDE: Self = Self(1 << 7);
    /// Second half of a double-width character; render nothing.
    pub const WIDE_SPACER: Self = Self(1 << 8);
    /// Set on the last cell of a row that soft-wraps into the next row.
    pub const WRAP: Self = Self(1 << 9);

    pub const fn empty() -> Self {
        Self(0)
    }
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    /// Combining characters attached to `ch`.
    pub zerowidth: Option<Box<str>>,
    pub fg: Color,
    pub bg: Color,
    pub flags: CellFlags,
    /// OSC 8 hyperlink target, if the program attached one.
    pub link: Option<std::sync::Arc<str>>,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            zerowidth: None,
            fg: Color::DefaultFg,
            bg: Color::DefaultBg,
            flags: CellFlags::empty(),
            link: None,
        }
    }
}

/// Which mouse events the running program asked to receive.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MouseTracking {
    #[default]
    Off,
    /// Presses and releases (DECSET 1000).
    Click,
    /// Plus motion while a button is held (1002).
    Drag,
    /// Plus all motion (1003).
    Motion,
}

/// Terminal modes the UI needs to know about.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modes {
    pub app_cursor: bool,
    pub bracketed_paste: bool,
    pub alt_screen: bool,
    pub cursor_visible: bool,
    pub mouse: MouseTracking,
    /// SGR mouse encoding (1006).
    pub sgr_mouse: bool,
    /// Focus in/out reports (1004).
    pub focus_events: bool,
    /// Wheel sends arrow keys on the alternate screen (1007).
    pub alternate_scroll: bool,
}

/// A copy of the visible viewport, safe to hand to another thread or
/// process. Every row has exactly `size.cols` cells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub size: GridSize,
    pub lines: Vec<Vec<Cell>>,
    pub cursor: Cursor,
    pub modes: Modes,
    /// Lines currently held in scrollback above the screen.
    pub history: usize,
    /// How many lines the viewport is scrolled up from the bottom.
    pub display_offset: usize,
}

impl Snapshot {
    /// Absolute line number (as used by [`TerminalEngine::cursor_line_abs`])
    /// of viewport row 0.
    pub fn top_line_abs(&self) -> usize {
        self.history - self.display_offset
    }

    /// Text of one viewport row, skipping wide-character spacers.
    pub fn row_text(&self, row: usize) -> String {
        let mut s = String::new();
        for cell in self.lines.get(row).into_iter().flatten() {
            if cell.flags.contains(CellFlags::WIDE_SPACER) {
                continue;
            }
            s.push(cell.ch);
            if let Some(zw) = &cell.zerowidth {
                s.push_str(zw);
            }
        }
        s
    }

    /// Visible text with trailing blank lines and trailing spaces removed.
    pub fn text(&self) -> String {
        let rows: Vec<String> = (0..self.lines.len()).map(|r| self.row_text(r)).collect();
        let mut lines: Vec<&str> = rows.iter().map(|r| r.trim_end()).collect();
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        lines.join("\n")
    }
}

/// Viewport scrolling requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scroll {
    /// Positive scrolls up into history.
    Lines(i32),
    PageUp,
    PageDown,
    Top,
    Bottom,
}

/// The operations TermForge needs from a VT emulator.
pub trait TerminalEngine: Send {
    /// Feed raw PTY output.
    fn feed(&mut self, bytes: &[u8]);
    fn resize(&mut self, size: GridSize);
    fn size(&self) -> GridSize;
    fn snapshot(&self) -> Snapshot;
    /// Absolute line of the cursor, counting scrollback. Used to anchor
    /// semantic marks (blocks) to content.
    fn cursor_line_abs(&self) -> usize;
    /// Lines currently held in scrollback. Callers compare successive
    /// values to notice when history is cleared (`ESC [3J`) or trimmed.
    fn history_size(&self) -> usize;
    /// Bytes the emulator must send back to the PTY (for example replies to
    /// device-attribute queries). Callers must drain this after every feed.
    fn take_replies(&mut self) -> Vec<u8>;
    /// Window title set by the running program, if any.
    fn title(&self) -> Option<String>;
    fn modes(&self) -> Modes;
    fn scroll(&mut self, scroll: Scroll);
    /// Total addressable lines: scrollback plus screen.
    fn total_lines(&self) -> usize {
        self.history_size() + self.size().rows as usize
    }
    /// One line by absolute index (`0` is the oldest scrollback line), or
    /// `None` when out of range. Used for selection, search and links.
    fn line(&self, abs: usize) -> Option<Vec<Cell>>;
    /// Current viewport offset from the bottom, in lines.
    fn display_offset(&self) -> usize;
}
