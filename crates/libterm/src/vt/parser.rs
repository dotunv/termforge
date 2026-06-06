use vte::{Params, Parser, Perform};

use crate::block::detector::BlockDetector;
use crate::block::store::BlockStore;
use crate::grid::TerminalGrid;
use crate::vt::sequences::{Osc133, OscNotification};

/// Wraps the `vte` parser and feeds events into a `TerminalGrid`.
pub struct VtParser {
    parser: Parser,
    performer: VtPerformer,
}

impl VtParser {
    pub fn new(cols: u16, rows: u16, detector: BlockDetector) -> Self {
        Self {
            parser: Parser::new(),
            performer: VtPerformer {
                grid: TerminalGrid::new(cols, rows),
                detector,
                blocks: BlockStore::default(),
            },
        }
    }

    pub fn process(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.parser.advance(&mut self.performer, b);
        }
    }

    pub fn grid(&self) -> &TerminalGrid {
        &self.performer.grid
    }

    pub fn grid_mut(&mut self) -> &mut TerminalGrid {
        &mut self.performer.grid
    }

    pub fn detector(&self) -> &BlockDetector {
        &self.performer.detector
    }

    pub fn detector_mut(&mut self) -> &mut BlockDetector {
        &mut self.performer.detector
    }

    pub fn blocks(&self) -> &BlockStore {
        &self.performer.blocks
    }

    pub fn blocks_mut(&mut self) -> &mut BlockStore {
        &mut self.performer.blocks
    }
}

struct VtPerformer {
    grid: TerminalGrid,
    detector: BlockDetector,
    blocks: BlockStore,
}

impl Perform for VtPerformer {
    fn print(&mut self, c: char) {
        // Feed the character to the detector for command capture.
        self.detector.feed_char(c);
        self.grid.write_char(c);
        let mut buf = [0; 4];
        self.append_to_running_block(c.encode_utf8(&mut buf).as_bytes());
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\r' => {
                self.grid.carriage_return();
                self.append_to_running_block(b"\r");
            }
            b'\n' => {
                self.grid.line_feed();
                self.append_to_running_block(b"\n");
            }
            b'\x08' => {
                self.grid.backspace();
                self.append_to_running_block(&[byte]);
            }
            b'\x07' => {} // bell — ignore
            _ => {}
        }
    }

    fn csi_dispatch(
        &mut self,
        params: &Params,
        intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        // Flatten all sub-parameters into a single vec so that colon-separated
        // params (e.g. ESC[38:2:255:0:0m for RGB color) and the more common
        // semicolon-separated form (ESC[38;2;255;0;0m) are handled uniformly.
        // Without this, `params[i+2..i+4]` slices for RGB SGR were reading
        // the first sub-parameter of each param group, producing garbage colors.
        let p: Vec<u16> = params
            .iter()
            .flat_map(|s| s.iter().copied())
            .collect();

        // DEC private modes: CSI ? Ps h/l  (intermediate byte = b'?')
        if intermediates.first() == Some(&b'?') {
            match action {
                'h' => {
                    for &mode in &p {
                        match mode {
                            25   => self.grid.set_cursor_visible(true),
                            1049 => self.grid.enter_alt_screen(),
                            _ => {}
                        }
                    }
                }
                'l' => {
                    for &mode in &p {
                        match mode {
                            25   => self.grid.set_cursor_visible(false),
                            1049 => self.grid.exit_alt_screen(),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
            return;
        }

        match action {
            // Cursor position: CUP / HVP
            'H' | 'f' => {
                let row = p.first().copied().unwrap_or(1).saturating_sub(1);
                let col = p.get(1).copied().unwrap_or(1).saturating_sub(1);
                self.grid.set_cursor(col, row);
            }
            // Cursor up
            'A' => self
                .grid
                .move_cursor_rel(0, -(p.first().copied().unwrap_or(1) as i32)),
            // Cursor down
            'B' => self
                .grid
                .move_cursor_rel(0, p.first().copied().unwrap_or(1) as i32),
            // Cursor forward
            'C' => self
                .grid
                .move_cursor_rel(p.first().copied().unwrap_or(1) as i32, 0),
            // Cursor back
            'D' => self
                .grid
                .move_cursor_rel(-(p.first().copied().unwrap_or(1) as i32), 0),
            // Erase in display
            'J' => self.grid.erase_in_display(p.first().copied().unwrap_or(0)),
            // Erase in line
            'K' => self.grid.erase_in_line(p.first().copied().unwrap_or(0)),
            // Select graphic rendition (color/attrs)
            'm' => self.grid.sgr(&p),
            // DECSTBM — set scroll region: CSI Pt ; Pb r
            // Parameters are 1-based; default top=1, default bottom=rows.
            'r' => {
                let top = p.first().copied().unwrap_or(1).saturating_sub(1);
                let bot = p.get(1).copied()
                    .map(|v| v.saturating_sub(1))
                    .unwrap_or_else(|| self.grid.rows().saturating_sub(1));
                self.grid.set_scroll_region(top, bot);
            }
            // CHA — Cursor Character Absolute (column, 1-based)
            'G' => {
                let col = p.first().copied().unwrap_or(1).saturating_sub(1);
                self.grid.cursor_col_abs(col);
            }
            // VPA — Line Position Absolute (row, 1-based)
            'd' => {
                let row = p.first().copied().unwrap_or(1).saturating_sub(1);
                self.grid.cursor_row_abs(row);
            }
            // CNL — Cursor Next Line
            'E' => self.grid.cursor_next_line(p.first().copied().unwrap_or(1)),
            // CPL — Cursor Previous Line
            'F' => self.grid.cursor_prev_line(p.first().copied().unwrap_or(1)),
            // IL — Insert Lines
            'L' => self.grid.insert_lines(p.first().copied().unwrap_or(1)),
            // DL — Delete Lines
            'M' => self.grid.delete_lines(p.first().copied().unwrap_or(1)),
            // DCH — Delete Characters
            'P' => self.grid.delete_chars(p.first().copied().unwrap_or(1)),
            // ICH — Insert Characters
            '@' => self.grid.insert_chars(p.first().copied().unwrap_or(1)),
            // ECH — Erase Characters
            'X' => self.grid.erase_chars(p.first().copied().unwrap_or(1)),
            // SU — Scroll Up
            'S' => self.grid.scroll_up(p.first().copied().unwrap_or(1)),
            // SD — Scroll Down
            'T' => self.grid.scroll_down(p.first().copied().unwrap_or(1)),
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        // `vte` splits OSC payload by ';', so `\e]133;A\e\\` arrives as
        // params = [b"133", b"A"].  Rejoin for our parser which expects "133;A".
        let joined: Vec<u8> = params
            .iter()
            .enumerate()
            .flat_map(|(i, p)| {
                if i > 0 {
                    std::iter::once(b';')
                        .chain(p.iter().copied())
                        .collect::<Vec<_>>()
                } else {
                    p.to_vec()
                }
            })
            .collect();
        if let Ok(s) = std::str::from_utf8(&joined) {
            if let Some(marker) = Osc133::parse(s) {
                let action = self.detector.handle(marker);
                self.detector.apply(action, &mut self.blocks);
            } else if let Some(notif) = OscNotification::parse(s) {
                self.detector.apply(
                    crate::block::detector::BlockAction::Notification(notif),
                    &mut self.blocks,
                );
            }
        }
    }

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _action: char) {}
    fn put(&mut self, _byte: u8) {}
    fn unhook(&mut self) {}
    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, byte: u8) {
        match byte {
            // ESC M — Reverse Index: move cursor up one line, scrolling down
            // at the top of the scroll region.  Needed by vim, less, htop.
            b'M' => self.grid.reverse_index(),
            _ => {}
        }
    }
}

impl VtPerformer {
    fn append_to_running_block(&mut self, bytes: &[u8]) {
        if let Some(block) = self.blocks.last_mut() {
            if block.status == crate::block::store::BlockStatus::Running {
                block.append_output(bytes);
            }
        }
    }
}
