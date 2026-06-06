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
                cwd: None,
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

    /// Current working directory reported by the shell via OSC 7, if any.
    pub fn cwd(&self) -> Option<&str> {
        self.performer.cwd.as_deref()
    }
}

struct VtPerformer {
    grid: TerminalGrid,
    detector: BlockDetector,
    blocks: BlockStore,
    /// Working directory reported via OSC 7 (`\e]7;file://host/path\e\\`).
    cwd: Option<String>,
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
            if let Some(path) = parse_osc7_cwd(s).or_else(|| parse_osc9_9_cwd(s)) {
                self.cwd = Some(path);
            } else if let Some(marker) = Osc133::parse(s) {
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

/// Parse an OSC 7 working-directory report.
///
/// The payload looks like `7;file://hostname/C:/Users/me/code` (Windows) or
/// `7;file://hostname/home/me` (POSIX).  Returns the decoded filesystem path,
/// or `None` if `s` is not an OSC 7 sequence.
fn parse_osc7_cwd(s: &str) -> Option<String> {
    let rest = s.strip_prefix("7;")?;
    let after_scheme = rest.strip_prefix("file://")?;
    // Skip the authority (hostname) up to the first '/'.  The slash is the
    // start of the path and must be kept for POSIX absolute paths.
    let path = match after_scheme.find('/') {
        Some(i) => &after_scheme[i..],
        None => after_scheme,
    };
    let decoded = percent_decode(path);
    // Windows paths arrive as "/C:/Users/..." — drop the leading slash and
    // normalise to backslashes so they read like native paths in the chrome.
    let normalised = if decoded.len() >= 3
        && decoded.as_bytes()[0] == b'/'
        && decoded.as_bytes()[2] == b':'
    {
        decoded[1..].replace('/', "\\")
    } else {
        decoded
    };
    if normalised.is_empty() { None } else { Some(normalised) }
}

/// Parse an OSC 9;9 working-directory report (the Windows/ConEmu convention
/// emitted by ConPTY and PowerShell shell integration):
/// `9;9;C:\Users\me\code` or `9;9;"C:\Users\me\code"`.
fn parse_osc9_9_cwd(s: &str) -> Option<String> {
    let rest = s.strip_prefix("9;9;")?;
    let trimmed = rest.trim().trim_matches('"');
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Minimal `%XX` percent-decoding for OSC 7 paths (e.g. spaces as `%20`).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod osc7_tests {
    use super::parse_osc7_cwd;

    #[test]
    fn windows_path() {
        assert_eq!(
            parse_osc7_cwd("7;file://host/C:/Users/me/code"),
            Some("C:\\Users\\me\\code".to_string())
        );
    }

    #[test]
    fn posix_path() {
        assert_eq!(
            parse_osc7_cwd("7;file://host/home/me"),
            Some("/home/me".to_string())
        );
    }

    #[test]
    fn percent_decoded_space() {
        assert_eq!(
            parse_osc7_cwd("7;file://host/C:/My%20Docs"),
            Some("C:\\My Docs".to_string())
        );
    }

    #[test]
    fn not_osc7() {
        assert_eq!(parse_osc7_cwd("133;A"), None);
    }

    #[test]
    fn osc9_9_quoted() {
        assert_eq!(
            super::parse_osc9_9_cwd("9;9;\"C:\\Users\\me\\code\""),
            Some("C:\\Users\\me\\code".to_string())
        );
    }

    #[test]
    fn osc9_9_unquoted() {
        assert_eq!(
            super::parse_osc9_9_cwd("9;9;C:\\Users\\me"),
            Some("C:\\Users\\me".to_string())
        );
    }

    #[test]
    fn process_stores_cwd_from_osc9_9() {
        use crate::block::detector::BlockDetector;
        use crate::vt::VtParser;
        let mut p = VtParser::new(80, 24, BlockDetector::new(uuid::Uuid::new_v4()));
        // ESC ] 9 ; 9 ; <path> BEL
        p.process(b"\x1b]9;9;C:\\Users\\me\\code\x07");
        assert_eq!(p.cwd(), Some("C:\\Users\\me\\code"));
    }
}
