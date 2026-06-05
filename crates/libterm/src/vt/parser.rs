use vte::{Params, Parser, Perform};

use crate::block::detector::BlockDetector;
use crate::grid::TerminalGrid;
use crate::vt::sequences::Osc133;

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
}

struct VtPerformer {
    grid: TerminalGrid,
    detector: BlockDetector,
}

impl Perform for VtPerformer {
    fn print(&mut self, c: char) {
        self.grid.write_char(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\r' => self.grid.carriage_return(),
            b'\n' => self.grid.line_feed(),
            b'\x08' => self.grid.backspace(),
            b'\x07' => {} // bell — ignore
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, _intermediates: &[u8], _ignore: bool, action: char) {
        let p: Vec<u16> = params.iter().map(|s| s.first().copied().unwrap_or(0)).collect();
        match action {
            // Cursor position: CUP / HVP
            'H' | 'f' => {
                let row = p.first().copied().unwrap_or(1).saturating_sub(1);
                let col = p.get(1).copied().unwrap_or(1).saturating_sub(1);
                self.grid.set_cursor(col, row);
            }
            // Cursor up
            'A' => self.grid.move_cursor_rel(0, -(p.first().copied().unwrap_or(1) as i32)),
            // Cursor down
            'B' => self.grid.move_cursor_rel(0, p.first().copied().unwrap_or(1) as i32),
            // Cursor forward
            'C' => self.grid.move_cursor_rel(p.first().copied().unwrap_or(1) as i32, 0),
            // Cursor back
            'D' => self.grid.move_cursor_rel(-(p.first().copied().unwrap_or(1) as i32), 0),
            // Erase in display
            'J' => self.grid.erase_in_display(p.first().copied().unwrap_or(0)),
            // Erase in line
            'K' => self.grid.erase_in_line(p.first().copied().unwrap_or(0)),
            // Select graphic rendition (color/attrs)
            'm' => self.grid.sgr(&p),
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if let Some(raw) = params.first() {
            if let Ok(s) = std::str::from_utf8(raw) {
                if let Some(marker) = Osc133::parse(s) {
                    self.detector.handle(marker);
                }
            }
        }
    }

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _action: char) {}
    fn put(&mut self, _byte: u8) {}
    fn unhook(&mut self) {}
    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, _byte: u8) {}
}
