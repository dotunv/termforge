use super::cell::{Attrs, Cell, Color};
use super::scrollback::ScrollbackBuffer;

pub struct TerminalGrid {
    cols: u16,
    rows: u16,
    cells: Vec<Vec<Cell>>,
    cursor_col: u16,
    cursor_row: u16,
    current_fg: Color,
    current_bg: Color,
    current_attrs: Attrs,
    pub scrollback: ScrollbackBuffer,

    // ── Terminal state extensions ──────────────────────────────────────────
    /// Whether the cursor is rendered.  Hidden by ?25l, shown by ?25h.
    cursor_visible: bool,
    /// Scroll region: top and bottom rows (0-based, inclusive).
    /// Initialised to the full screen.  Set by DECSTBM (CSI Ps;Ps r).
    scroll_top: u16,
    scroll_bot: u16,
    /// Alternate screen buffer (None = primary screen is active).
    /// Entered by ?1049h, exited by ?1049l.
    alt_cells: Option<Vec<Vec<Cell>>>,
    /// Saved cursor position when entering the alternate screen.
    alt_saved_cursor: (u16, u16),
}

impl TerminalGrid {
    pub fn new(cols: u16, rows: u16) -> Self {
        let scroll_bot = rows.saturating_sub(1);
        Self {
            cols,
            rows,
            cells: vec![Self::blank_row(cols as usize); rows as usize],
            cursor_col: 0,
            cursor_row: 0,
            current_fg: Color::Default,
            current_bg: Color::Default,
            current_attrs: Attrs::default(),
            scrollback: ScrollbackBuffer::new(10_000),
            cursor_visible: true,
            scroll_top: 0,
            scroll_bot,
            alt_cells: None,
            alt_saved_cursor: (0, 0),
        }
    }

    fn blank_row(cols: usize) -> Vec<Cell> {
        vec![Cell::default(); cols]
    }

    pub fn cols(&self) -> u16 {
        self.cols
    }
    pub fn rows(&self) -> u16 {
        self.rows
    }
    pub fn cursor(&self) -> (u16, u16) {
        (self.cursor_col, self.cursor_row)
    }

    pub fn cell(&self, col: u16, row: u16) -> Option<&Cell> {
        self.cells.get(row as usize)?.get(col as usize)
    }

    pub fn write_char(&mut self, c: char) {
        if self.cursor_col >= self.cols {
            self.carriage_return();
            self.line_feed();
        }
        let row = self.cursor_row as usize;
        let col = self.cursor_col as usize;
        if row < self.cells.len() && col < self.cells[row].len() {
            self.cells[row][col] = Cell {
                ch: c,
                fg: self.current_fg,
                bg: self.current_bg,
                attrs: self.current_attrs,
            };
        }
        self.cursor_col += 1;
    }

    pub fn carriage_return(&mut self) {
        self.cursor_col = 0;
    }

    pub fn line_feed(&mut self) {
        if self.cursor_row == self.scroll_bot {
            // At the bottom of the scroll region — scroll the region up.
            self.cells.remove(self.scroll_top as usize);
            self.cells.insert(self.scroll_bot as usize, Self::blank_row(self.cols as usize));
            // On the primary screen (full-screen scroll region), keep scrollback.
            if self.scroll_top == 0 && self.scroll_bot == self.rows.saturating_sub(1) {
                // The removed row was already inserted into the scroll region;
                // push the top-most evicted row to scrollback.
                // Note: removal + insert above already maintains cell count,
                // so we push the conceptually evicted row separately.
                // For simplicity, push a blank placeholder — the visible output is correct.
                // (True scrollback history is a Phase 4 concern.)
            }
        } else if self.cursor_row + 1 < self.rows {
            self.cursor_row += 1;
        }
    }

    pub fn backspace(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        }
    }

    pub fn set_cursor(&mut self, col: u16, row: u16) {
        self.cursor_col = col.min(self.cols.saturating_sub(1));
        self.cursor_row = row.min(self.rows.saturating_sub(1));
    }

    pub fn move_cursor_rel(&mut self, dcol: i32, drow: i32) {
        let new_col = (self.cursor_col as i32 + dcol).clamp(0, self.cols as i32 - 1) as u16;
        let new_row = (self.cursor_row as i32 + drow).clamp(0, self.rows as i32 - 1) as u16;
        self.cursor_col = new_col;
        self.cursor_row = new_row;
    }

    pub fn erase_in_display(&mut self, mode: u16) {
        match mode {
            // erase from cursor to end of screen
            0 => {
                let (col, row) = (self.cursor_col as usize, self.cursor_row as usize);
                for c in col..self.cols as usize {
                    self.cells[row][c] = Cell::default();
                }
                for r in (row + 1)..self.rows as usize {
                    self.cells[r] = Self::blank_row(self.cols as usize);
                }
            }
            // erase from beginning to cursor
            1 => {
                let (col, row) = (self.cursor_col as usize, self.cursor_row as usize);
                for r in 0..row {
                    self.cells[r] = Self::blank_row(self.cols as usize);
                }
                for c in 0..=col {
                    self.cells[row][c] = Cell::default();
                }
            }
            // erase entire screen
            2 | 3 => {
                for row in &mut self.cells {
                    *row = Self::blank_row(self.cols as usize);
                }
            }
            _ => {}
        }
    }

    pub fn erase_in_line(&mut self, mode: u16) {
        let row = self.cursor_row as usize;
        match mode {
            // erase from cursor to end of line
            0 => {
                for c in self.cursor_col as usize..self.cols as usize {
                    self.cells[row][c] = Cell::default();
                }
            }
            // erase from beginning to cursor
            1 => {
                for c in 0..=self.cursor_col as usize {
                    self.cells[row][c] = Cell::default();
                }
            }
            // erase entire line
            2 => {
                self.cells[row] = Self::blank_row(self.cols as usize);
            }
            _ => {}
        }
    }

    /// Process SGR (Select Graphic Rendition) parameters.
    pub fn sgr(&mut self, params: &[u16]) {
        let mut i = 0;
        while i < params.len() {
            match params[i] {
                0 => {
                    self.current_fg = Color::Default;
                    self.current_bg = Color::Default;
                    self.current_attrs = Attrs::default();
                }
                1 => self.current_attrs.bold = true,
                2 => self.current_attrs.dim = true,
                3 => self.current_attrs.italic = true,
                4 => self.current_attrs.underline = true,
                7 => self.current_attrs.inverse = true,
                9 => self.current_attrs.strikethrough = true,
                22 => {
                    self.current_attrs.bold = false;
                    self.current_attrs.dim = false;
                }
                23 => self.current_attrs.italic = false,
                24 => self.current_attrs.underline = false,
                27 => self.current_attrs.inverse = false,
                29 => self.current_attrs.strikethrough = false,
                // standard foreground colors 30-37
                n @ 30..=37 => self.current_fg = Color::Indexed((n - 30) as u8),
                39 => self.current_fg = Color::Default,
                // standard background colors 40-47
                n @ 40..=47 => self.current_bg = Color::Indexed((n - 40) as u8),
                49 => self.current_bg = Color::Default,
                // bright foreground 90-97
                n @ 90..=97 => self.current_fg = Color::Indexed((n - 90 + 8) as u8),
                // bright background 100-107
                n @ 100..=107 => self.current_bg = Color::Indexed((n - 100 + 8) as u8),
                // 256-color and RGB foreground
                38 => {
                    if params.get(i + 1).copied() == Some(5) && i + 2 < params.len() {
                        self.current_fg = Color::Indexed(params[i + 2] as u8);
                        i += 2;
                    } else if params.get(i + 1).copied() == Some(2) && i + 4 < params.len() {
                        self.current_fg = Color::Rgb(
                            params[i + 2] as u8,
                            params[i + 3] as u8,
                            params[i + 4] as u8,
                        );
                        i += 4;
                    }
                }
                // 256-color and RGB background
                48 => {
                    if params.get(i + 1).copied() == Some(5) && i + 2 < params.len() {
                        self.current_bg = Color::Indexed(params[i + 2] as u8);
                        i += 2;
                    } else if params.get(i + 1).copied() == Some(2) && i + 4 < params.len() {
                        self.current_bg = Color::Rgb(
                            params[i + 2] as u8,
                            params[i + 3] as u8,
                            params[i + 4] as u8,
                        );
                        i += 4;
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    /// Cheap snapshot of the cell grid for the render thread.
    pub fn clone_grid(&self) -> TerminalGrid {
        TerminalGrid {
            cols: self.cols,
            rows: self.rows,
            cells: self.cells.clone(),
            cursor_col: self.cursor_col,
            cursor_row: self.cursor_row,
            current_fg: self.current_fg,
            current_bg: self.current_bg,
            current_attrs: self.current_attrs,
            scrollback: ScrollbackBuffer::new(0), // not needed in render copy
            cursor_visible: self.cursor_visible,
            scroll_top: self.scroll_top,
            scroll_bot: self.scroll_bot,
            // alt_cells is not needed on the render side; None is safe here.
            alt_cells: None,
            alt_saved_cursor: self.alt_saved_cursor,
        }
    }

    // ── Additional cursor / line operations ──────────────────────────────

    /// CHA / G — Move cursor to column `col` (0-based) on the current row.
    pub fn cursor_col_abs(&mut self, col: u16) {
        self.cursor_col = col.min(self.cols.saturating_sub(1));
    }

    /// VPA / d — Move cursor to row `row` (0-based), current column.
    pub fn cursor_row_abs(&mut self, row: u16) {
        self.cursor_row = row.min(self.rows.saturating_sub(1));
    }

    /// CNL / E — Cursor Next Line: move down N rows, column 0.
    pub fn cursor_next_line(&mut self, n: u16) {
        let new_row = (self.cursor_row as i32 + n as i32)
            .clamp(0, self.rows as i32 - 1) as u16;
        self.cursor_row = new_row;
        self.cursor_col = 0;
    }

    /// CPL / F — Cursor Previous Line: move up N rows, column 0.
    pub fn cursor_prev_line(&mut self, n: u16) {
        let new_row = (self.cursor_row as i32 - n as i32)
            .clamp(0, self.rows as i32 - 1) as u16;
        self.cursor_row = new_row;
        self.cursor_col = 0;
    }

    /// IL / L — Insert N blank lines at cursor row; scroll the scroll region
    /// down (lines at the bottom of the region are lost).
    pub fn insert_lines(&mut self, n: u16) {
        let row = self.cursor_row as usize;
        let bot = self.scroll_bot as usize;
        if row > bot { return; }
        for _ in 0..n {
            if bot < self.cells.len() {
                self.cells.remove(bot);
            }
            self.cells.insert(row, Self::blank_row(self.cols as usize));
        }
        self.cursor_col = 0;
    }

    /// DL / M — Delete N lines at cursor row; scroll the scroll region up
    /// (blank lines appear at the bottom of the region).
    pub fn delete_lines(&mut self, n: u16) {
        let row = self.cursor_row as usize;
        let bot = self.scroll_bot as usize;
        if row > bot { return; }
        for _ in 0..n {
            if row < self.cells.len() {
                self.cells.remove(row);
            }
            if bot < self.cells.len() + 1 {
                self.cells.insert(bot, Self::blank_row(self.cols as usize));
            }
        }
        self.cursor_col = 0;
    }

    /// DCH / P — Delete N characters at cursor position; shift rest of line left.
    pub fn delete_chars(&mut self, n: u16) {
        let row = self.cursor_row as usize;
        let col = self.cursor_col as usize;
        if row >= self.cells.len() { return; }
        let line = &mut self.cells[row];
        let n = n as usize;
        for _ in 0..n {
            if col < line.len() {
                line.remove(col);
                line.push(Cell::default());
            }
        }
    }

    /// ICH / @ — Insert N blank characters at cursor position; shift rest of
    /// line right (characters pushed past the right margin are lost).
    pub fn insert_chars(&mut self, n: u16) {
        let row = self.cursor_row as usize;
        let col = self.cursor_col as usize;
        if row >= self.cells.len() { return; }
        let line = &mut self.cells[row];
        let n = n as usize;
        for i in 0..n {
            if col + i < line.len() {
                line.insert(col + i, Cell::default());
                line.pop();
            }
        }
    }

    /// ECH / X — Erase N characters starting at cursor (replace with spaces),
    /// without moving the cursor.
    pub fn erase_chars(&mut self, n: u16) {
        let row = self.cursor_row as usize;
        let col = self.cursor_col as usize;
        if row >= self.cells.len() { return; }
        let end = (col + n as usize).min(self.cols as usize);
        for c in col..end {
            if c < self.cells[row].len() {
                self.cells[row][c] = Cell::default();
            }
        }
    }

    /// SU / S — Scroll the scroll region up N lines (blank lines appear at bottom).
    pub fn scroll_up(&mut self, n: u16) {
        let top = self.scroll_top as usize;
        let bot = self.scroll_bot as usize;
        for _ in 0..n {
            if top < self.cells.len() {
                self.cells.remove(top);
            }
            if bot < self.cells.len() + 1 {
                self.cells.insert(bot, Self::blank_row(self.cols as usize));
            }
        }
    }

    /// SD / T — Scroll the scroll region down N lines (blank lines appear at top).
    pub fn scroll_down(&mut self, n: u16) {
        let top = self.scroll_top as usize;
        let bot = self.scroll_bot as usize;
        for _ in 0..n {
            if bot < self.cells.len() {
                self.cells.remove(bot);
            }
            if top <= self.cells.len() {
                self.cells.insert(top, Self::blank_row(self.cols as usize));
            }
        }
    }

    // ── Terminal state accessors / mutators ───────────────────────────────

    /// Whether the cursor should be rendered (toggled by ?25h / ?25l).
    pub fn is_cursor_visible(&self) -> bool {
        self.cursor_visible
    }

    /// Show (`true`) or hide (`false`) the cursor.  Driven by DEC private
    /// mode ?25 (CSI ?25h to show, CSI ?25l to hide).
    pub fn set_cursor_visible(&mut self, v: bool) {
        self.cursor_visible = v;
    }

    /// Set the scroll region (DECSTBM).  `top` and `bot` are 0-based inclusive
    /// row indices.  Invalid ranges are clamped and ignored.
    pub fn set_scroll_region(&mut self, top: u16, bot: u16) {
        let top = top.min(self.rows.saturating_sub(1));
        let bot = bot.min(self.rows.saturating_sub(1));
        if top < bot {
            self.scroll_top = top;
            self.scroll_bot = bot;
            // DECSTBM also homes the cursor to the top-left.
            self.cursor_col = 0;
            self.cursor_row = 0;
        }
    }

    /// ESC M — Reverse Index.  Move cursor up one line.  If already at the
    /// top of the scroll region, scroll the region down (insert a blank line
    /// at top, push the bottom line out).
    pub fn reverse_index(&mut self) {
        if self.cursor_row == self.scroll_top {
            // Insert blank line at top of scroll region, remove bottom.
            self.cells.remove(self.scroll_bot as usize);
            self.cells.insert(self.scroll_top as usize, Self::blank_row(self.cols as usize));
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
        }
    }

    /// Enter the alternate screen buffer (?1049h).
    /// Saves the primary screen and cursor, then starts with a blank screen.
    pub fn enter_alt_screen(&mut self) {
        if self.alt_cells.is_some() {
            return; // already on alt screen
        }
        self.alt_saved_cursor = (self.cursor_col, self.cursor_row);
        self.alt_cells = Some(std::mem::replace(
            &mut self.cells,
            vec![Self::blank_row(self.cols as usize); self.rows as usize],
        ));
        self.cursor_col = 0;
        self.cursor_row = 0;
        self.scroll_top = 0;
        self.scroll_bot = self.rows.saturating_sub(1);
    }

    /// Exit the alternate screen buffer (?1049l).
    /// Restores the primary screen and saved cursor position.
    pub fn exit_alt_screen(&mut self) {
        if let Some(primary) = self.alt_cells.take() {
            self.cells = primary;
            let (col, row) = self.alt_saved_cursor;
            self.cursor_col = col;
            self.cursor_row = row;
            self.scroll_top = 0;
            self.scroll_bot = self.rows.saturating_sub(1);
        }
    }

    /// Returns all text content of the visible grid as a plain string (rows joined by newlines).
    pub fn visible_text(&self) -> String {
        self.cells
            .iter()
            .map(|row| {
                let s: String = row.iter().map(|c| c.ch).collect();
                s.trim_end().to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
