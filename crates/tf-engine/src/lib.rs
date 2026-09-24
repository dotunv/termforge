//! Terminal emulation behind a small trait.
//!
//! TermForge does not implement its own VT emulator (see
//! `docs/adr/0002-terminal-engine.md`). [`AlacrittyEngine`] wraps
//! `alacritty_terminal`; a `libghostty-vt` implementation can be added later
//! without touching callers.

mod alacritty;

pub use alacritty::AlacrittyEngine;

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

/// A copy of the visible screen, safe to hand to another thread or process.
///
/// Phase 0 carries text only. Cell attributes (colour, weight, underline,
/// hyperlinks) are added together with the renderer in Phase 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub size: GridSize,
    pub rows: Vec<String>,
    pub cursor: Cursor,
    pub alt_screen: bool,
    /// Lines currently held in scrollback above the screen.
    pub history: usize,
}

impl Snapshot {
    /// Visible text with trailing blank lines and trailing spaces removed.
    pub fn text(&self) -> String {
        let mut lines: Vec<&str> = self.rows.iter().map(|r| r.trim_end()).collect();
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        lines.join("\n")
    }
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
    /// Bytes the emulator must send back to the PTY (for example replies to
    /// device-attribute queries). Callers must drain this after every feed.
    fn take_replies(&mut self) -> Vec<u8>;
    /// Window title set by the running program, if any.
    fn title(&self) -> Option<String>;
}
