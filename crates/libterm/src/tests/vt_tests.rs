use uuid::Uuid;

use crate::block::detector::BlockDetector;
use crate::vt::{sequences::OscNotification, VtParser};

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
    assert!(
        first_line.starts_with("he"),
        "expected 'he' prefix, got: {first_line:?}"
    );
}

#[test]
fn osc133_prompt_boundary_detected() {
    let mut p = make_parser(80, 24);
    // OSC 133;A — prompt start
    p.process(b"\x1b]133;A\x1b\\");
    // The detector should have received a PromptStart without panicking.
    // Smoke test — detailed block lifecycle tested in block_tests.
}

// ── OSC 9001 notification tests ──────────────────────────────────────────────

fn start_block(p: &mut VtParser) {
    // Simulate a full OSC 133 prompt→command sequence so there is a running block.
    p.process(b"\x1b]133;A\x1b\\"); // PromptStart
    p.process(b"\x1b]133;B\x1b\\"); // PromptEnd — start capturing
    p.process(b"mycommand");         // typed command
    p.process(b"\x1b]133;C\x1b\\"); // CommandStart — block created
}

#[test]
fn osc9001_status_awaiting_sets_notification() {
    let mut p = make_parser(80, 24);
    start_block(&mut p);
    p.process(b"\x1b]9001;status:awaiting_user\x1b\\");
    let block = p.blocks().all().last().expect("block should exist");
    assert_eq!(block.notification, Some(OscNotification::StatusAwaiting));
}

#[test]
fn osc9001_status_success_sets_notification() {
    let mut p = make_parser(80, 24);
    start_block(&mut p);
    p.process(b"\x1b]9001;status:success\x1b\\");
    let block = p.blocks().all().last().expect("block should exist");
    assert_eq!(block.notification, Some(OscNotification::StatusSuccess));
}

#[test]
fn osc9001_status_error_sets_notification() {
    let mut p = make_parser(80, 24);
    start_block(&mut p);
    p.process(b"\x1b]9001;status:error\x1b\\");
    let block = p.blocks().all().last().expect("block should exist");
    assert_eq!(block.notification, Some(OscNotification::StatusError));
}

#[test]
fn osc9001_notify_sets_message() {
    let mut p = make_parser(80, 24);
    start_block(&mut p);
    p.process(b"\x1b]9001;notify:build complete\x1b\\");
    let block = p.blocks().all().last().expect("block should exist");
    assert_eq!(
        block.notification,
        Some(OscNotification::Notify("build complete".to_owned()))
    );
}

#[test]
fn osc9001_without_block_is_ignored() {
    // If no block exists, the notification should not panic and store stays empty.
    let mut p = make_parser(80, 24);
    p.process(b"\x1b]9001;status:awaiting_user\x1b\\");
    assert!(p.blocks().is_empty());
}

#[test]
fn osc9001_unknown_payload_is_ignored() {
    let mut p = make_parser(80, 24);
    start_block(&mut p);
    p.process(b"\x1b]9001;unknown:payload\x1b\\");
    let block = p.blocks().all().last().expect("block should exist");
    assert_eq!(block.notification, None);
}

// ── Block output appended tests ───────────────────────────────────────────────

#[test]
fn block_output_appended_from_printable_chars() {
    let mut p = make_parser(80, 24);
    start_block(&mut p);
    p.process(b"hello world");
    let block = p.blocks().all().last().expect("block should exist");
    assert!(
        block.output_as_str().contains("hello world"),
        "printable output not in block: {:?}",
        block.output_as_str()
    );
}

#[test]
fn block_output_appended_from_control_chars() {
    let mut p = make_parser(80, 24);
    start_block(&mut p);
    p.process(b"line1\r\nline2");
    let block = p.blocks().all().last().expect("block should exist");
    let out = block.output_as_str();
    assert!(out.contains("line1"), "missing line1 in block output: {out:?}");
    assert!(out.contains("line2"), "missing line2 in block output: {out:?}");
    assert!(out.contains('\n'), "CRLF not in block output: {out:?}");
}

#[test]
fn block_output_not_appended_when_no_block() {
    let mut p = make_parser(80, 24);
    p.process(b"stray output before any block");
    assert!(p.blocks().is_empty(), "no block should exist yet");
}

#[test]
fn finished_block_does_not_accumulate_output() {
    let mut p = make_parser(80, 24);
    start_block(&mut p);
    // Finish block via OSC 133;D;0
    p.process(b"\x1b]133;D;0\x1b\\");
    let len_before = p.blocks().all().last().unwrap().output.len();
    // Output after finish should NOT go into the finished block
    p.process(b"post-exit output");
    let len_after = p.blocks().all().last().unwrap().output.len();
    assert_eq!(len_before, len_after, "finished block should not accumulate output");
}

#[test]
fn osc9001_notification_can_be_overwritten() {
    let mut p = make_parser(80, 24);
    start_block(&mut p);
    p.process(b"\x1b]9001;status:awaiting_user\x1b\\");
    p.process(b"\x1b]9001;status:success\x1b\\");
    let block = p.blocks().all().last().expect("block should exist");
    assert_eq!(block.notification, Some(OscNotification::StatusSuccess));
}
