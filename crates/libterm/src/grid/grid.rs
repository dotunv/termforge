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
}

impl TerminalGrid {
    pub fn new(cols: u16, rows: u16) -> Self {
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
        if self.cursor_row + 1 >= self.rows {
            // scroll up: push top row into scrollback
            let evicted = self.cells.remove(0);
            self.scrollback.push(evicted);
            self.cells.push(Self::blank_row(self.cols as usize));
        } else {
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
