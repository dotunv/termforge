//! Small golden-grid conformance suite for sequences real TUIs depend on.
//!
//! This is not a replacement for vttest/esctest, but it makes common cursor,
//! erase, insert and wide-cell behavior reviewable in ordinary CI.

use tf_engine::{AlacrittyEngine, CellFlags, GridSize, TerminalEngine};

fn engine(input: &[u8]) -> AlacrittyEngine {
    let mut engine = AlacrittyEngine::with_scrollback(GridSize::new(10, 4), 8);
    engine.feed(input);
    engine
}

#[test]
fn cursor_addressing_and_erase_line() {
    let e = engine(b"first\r\nsecond\r\nthird\x1b[2;3H\x1b[K!");
    insta::assert_snapshot!(e.snapshot().text(), @r"
    first
    se!
    third
    ");
}

#[test]
fn insert_and_delete_characters() {
    let e = engine(b"abcdef\x1b[3D\x1b[2@XY\x1b[P");
    insta::assert_snapshot!(e.snapshot().text(), @r"
    abcXYef
    ");
}

#[test]
fn wide_character_owns_exactly_two_cells() {
    let e = engine("A界B".as_bytes());
    let line = e.line(0).unwrap();
    assert_eq!(line[0].ch, 'A');
    assert_eq!(line[1].ch, '界');
    assert!(line[1].flags.contains(CellFlags::WIDE));
    assert!(line[2].flags.contains(CellFlags::WIDE_SPACER));
    assert_eq!(line[3].ch, 'B');
}

#[test]
fn alternate_screen_does_not_destroy_primary_grid() {
    let mut e = engine(b"primary");
    e.feed(b"\x1b[?1049halt\x1b[?1049l");
    insta::assert_snapshot!(e.snapshot().text(), @r"
    primary
    ");
}
