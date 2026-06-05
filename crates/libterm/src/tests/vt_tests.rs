use uuid::Uuid;

use crate::block::detector::BlockDetector;
use crate::vt::VtParser;

fn make_parser(cols: u16, rows: u16) -> VtParser {
    VtParser::new(cols, rows, BlockDetector::new(Uuid::new_v4()))
}

#[test]
fn plain_text_renders_to_grid() {
    let mut p = make_parser(80, 24);
    p.process(b"hello world");
    let text = p.grid().visible_text();
    assert!(text.contains("hello world"), "got: {text:?}");
}

#[test]
fn crlf_moves_to_next_line() {
    let mut p = make_parser(80, 24);
    p.process(b"line1\r\nline2");
    let text = p.grid().visible_text();
    assert!(text.contains("line1"), "missing line1 in: {text:?}");
    assert!(text.contains("line2"), "missing line2 in: {text:?}");
}

#[test]
fn cursor_positioning_csi() {
    let mut p = make_parser(80, 24);
    // CSI 5;10H — move to row 5, col 10
    p.process(b"\x1b[5;10H");
    assert_eq!(p.grid().cursor(), (9, 4)); // 0-indexed
}

#[test]
fn erase_in_line_csi() {
    let mut p = make_parser(80, 24);
    p.process(b"hello");
    // move to col 2, erase to end of line
    p.process(b"\x1b[1;3H\x1b[0K");
    let text = p.grid().visible_text();
    // "he" should remain, "llo" erased
    let first_line = text.lines().next().unwrap_or("");
    assert!(first_line.starts_with("he"), "expected 'he' prefix, got: {first_line:?}");
}

#[test]
fn osc133_prompt_boundary_detected() {
    let mut p = make_parser(80, 24);
    // OSC 133;A — prompt start
    p.process(b"\x1b]133;A\x1b\\");
    // The detector should have received a PromptStart without panicking.
    // Smoke test — detailed block lifecycle tested in block_tests.
}
