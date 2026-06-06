use crate::grid::TerminalGrid;

#[test]
fn write_chars_appear_in_grid() {
    let mut grid = TerminalGrid::new(80, 24);
    for c in "hello".chars() {
        grid.write_char(c);
    }
    let text = grid.visible_text();
    assert!(
        text.contains("hello"),
        "expected 'hello' in grid output, got: {text:?}"
    );
}

#[test]
fn line_feed_advances_row() {
    let mut grid = TerminalGrid::new(80, 24);
    grid.write_char('A');
    grid.carriage_return();
    grid.line_feed();
    grid.write_char('B');
    assert_eq!(grid.cursor(), (1, 1));
    let text = grid.visible_text();
    assert!(text.contains('A'));
    assert!(text.contains('B'));
}

#[test]
fn cursor_positioning() {
    let mut grid = TerminalGrid::new(80, 24);
    grid.set_cursor(10, 5);
    assert_eq!(grid.cursor(), (10, 5));
}

#[test]
fn erase_in_line_clears_content() {
    let mut grid = TerminalGrid::new(80, 24);
    for c in "hello world".chars() {
        grid.write_char(c);
    }
    grid.set_cursor(5, 0);
    grid.erase_in_line(0); // erase from cursor to end
    let text = grid.visible_text();
    assert!(
        text.starts_with("hello"),
        "expected 'hello' prefix, got: {text:?}"
    );
    assert!(
        !text.contains("world"),
        "expected 'world' erased, got: {text:?}"
    );
}

#[test]
fn scrollback_accumulates_on_overflow() {
    let mut grid = TerminalGrid::new(80, 5); // only 5 rows
    for i in 0..10 {
        for c in format!("line {i}").chars() {
            grid.write_char(c);
        }
        grid.carriage_return();
        grid.line_feed();
    }
    assert!(
        grid.scrollback.len() > 0,
        "expected scrollback to have rows"
    );
}
