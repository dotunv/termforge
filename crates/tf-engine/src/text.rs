//! Text over the grid: selection, search and link detection.
//!
//! Everything here works on absolute line indices (`0` is the oldest
//! scrollback line), so selections and matches stay attached to content
//! while the viewport scrolls. Soft-wrapped rows are joined into logical
//! lines, so a URL or match that wraps is still found.

use crate::{Cell, CellFlags, TerminalEngine};

/// A cell position. Ordered top-to-bottom, then left-to-right.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Point {
    pub line: usize,
    pub col: u16,
}

impl Point {
    pub fn new(line: usize, col: u16) -> Self {
        Self { line, col }
    }
}

/// How a selection grows, set by the click count that started it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectionKind {
    #[default]
    Cells,
    Words,
    Lines,
}

/// A selection between two points, inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub anchor: Point,
    pub head: Point,
    pub kind: SelectionKind,
}

impl Selection {
    pub fn new(at: Point, kind: SelectionKind) -> Self {
        Self {
            anchor: at,
            head: at,
            kind,
        }
    }

    pub fn extend(&mut self, to: Point) {
        self.head = to;
    }

    /// A plain click that has not moved selects nothing.
    pub fn is_empty(&self) -> bool {
        self.kind == SelectionKind::Cells && self.anchor == self.head
    }

    /// Ordered, inclusive range after word/line expansion.
    pub fn range<E: TerminalEngine + ?Sized>(&self, e: &E) -> (Point, Point) {
        let (a, b) = if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        };
        let last_col = e.size().cols.saturating_sub(1);
        match self.kind {
            SelectionKind::Cells => (a, b),
            SelectionKind::Words => (word_bounds(e, a).0, word_bounds(e, b).1),
            SelectionKind::Lines => {
                let (start, _) = logical_bounds(e, a.line);
                let (_, end) = logical_bounds(e, b.line);
                (Point::new(start, 0), Point::new(end, last_col))
            }
        }
    }

    /// Selected text. Soft-wrapped rows are joined without a newline and
    /// trailing blanks on hard-wrapped rows are dropped.
    pub fn text<E: TerminalEngine + ?Sized>(&self, e: &E) -> String {
        if self.is_empty() {
            return String::new();
        }
        let (start, end) = self.range(e);
        let mut out = String::new();
        for line in start.line..=end.line {
            let Some(cells) = e.line(line) else { break };
            let from = if line == start.line {
                start.col as usize
            } else {
                0
            };
            let to = if line == end.line {
                (end.col as usize).min(cells.len().saturating_sub(1))
            } else {
                cells.len().saturating_sub(1)
            };
            let wraps = cells
                .last()
                .is_some_and(|c| c.flags.contains(CellFlags::WRAP));
            let mut row = String::new();
            for cell in cells.get(from..=to).into_iter().flatten() {
                push_cell(&mut row, cell);
            }
            if wraps && line != end.line {
                out.push_str(&row);
            } else {
                out.push_str(row.trim_end());
                if line != end.line {
                    out.push('\n');
                }
            }
        }
        out
    }
}

/// Whether `p` lies inside an ordered inclusive range.
pub fn in_range((start, end): (Point, Point), p: Point) -> bool {
    start <= p && p <= end
}

fn push_cell(s: &mut String, cell: &Cell) {
    if cell.flags.contains(CellFlags::WIDE_SPACER) {
        return;
    }
    s.push(cell.ch);
    if let Some(zw) = &cell.zerowidth {
        s.push_str(zw);
    }
}

/// Characters that end a word for double-click selection. Paths, flags and
/// `file.rs:12:5` locations stay whole, as developers expect.
fn is_word_break(c: char) -> bool {
    c.is_whitespace() || ",│`|\"'()[]{}<>".contains(c)
}

fn wraps(e: &(impl TerminalEngine + ?Sized), line: usize) -> bool {
    e.line(line)
        .and_then(|c| c.last().map(|c| c.flags.contains(CellFlags::WRAP)))
        .unwrap_or(false)
}

/// First and last row of the logical line containing `line`.
fn logical_bounds(e: &(impl TerminalEngine + ?Sized), line: usize) -> (usize, usize) {
    let mut start = line;
    while start > 0 && wraps(e, start - 1) {
        start -= 1;
    }
    let mut end = line;
    let total = e.total_lines();
    while end + 1 < total && wraps(e, end) {
        end += 1;
    }
    (start, end)
}

/// A logical line flattened into characters with their cell positions.
struct Logical {
    chars: Vec<char>,
    points: Vec<Point>,
    /// Row after this logical line.
    next: usize,
}

fn logical_line(e: &(impl TerminalEngine + ?Sized), start: usize) -> Option<Logical> {
    let mut chars = Vec::new();
    let mut points = Vec::new();
    let mut line = start;
    loop {
        let cells = e.line(line)?;
        for (col, cell) in cells.iter().enumerate() {
            if cell.flags.contains(CellFlags::WIDE_SPACER) {
                continue;
            }
            chars.push(cell.ch);
            points.push(Point::new(line, col as u16));
        }
        let wrapped = cells
            .last()
            .is_some_and(|c| c.flags.contains(CellFlags::WRAP));
        line += 1;
        if !wrapped || line >= e.total_lines() {
            break;
        }
    }
    Some(Logical {
        chars,
        points,
        next: line,
    })
}

/// Last cell covered by the character at `p` (wide characters span two).
fn char_end(e: &(impl TerminalEngine + ?Sized), p: Point) -> Point {
    let wide = e
        .line(p.line)
        .and_then(|c| {
            c.get(p.col as usize)
                .map(|c| c.flags.contains(CellFlags::WIDE))
        })
        .unwrap_or(false);
    if wide {
        Point::new(p.line, p.col + 1)
    } else {
        p
    }
}

/// Word around `p`, following soft wraps. On a break character the word is
/// just that cell.
pub fn word_bounds(e: &(impl TerminalEngine + ?Sized), p: Point) -> (Point, Point) {
    let (first, _) = logical_bounds(e, p.line);
    let Some(l) = logical_line(e, first) else {
        return (p, p);
    };
    let Some(i) = l.points.iter().rposition(|q| *q <= p) else {
        return (p, p);
    };
    if is_word_break(l.chars[i]) {
        return (l.points[i], char_end(e, l.points[i]));
    }
    let mut s = i;
    while s > 0 && !is_word_break(l.chars[s - 1]) {
        s -= 1;
    }
    let mut t = i;
    while t + 1 < l.chars.len() && !is_word_break(l.chars[t + 1]) {
        t += 1;
    }
    (l.points[s], char_end(e, l.points[t]))
}

/// A found range, inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    pub start: Point,
    pub end: Point,
}

/// Every occurrence of `query` in the whole buffer, oldest first. Matching
/// is literal; `case_sensitive = false` folds simple case. Stops after
/// `limit` matches so a one-letter query cannot stall the UI.
pub fn find_all(
    e: &(impl TerminalEngine + ?Sized),
    query: &str,
    case_sensitive: bool,
    limit: usize,
) -> Vec<Match> {
    let fold = |c: char| {
        if case_sensitive {
            c
        } else {
            c.to_lowercase().next().unwrap_or(c)
        }
    };
    let needle: Vec<char> = query.chars().map(fold).collect();
    let mut out = Vec::new();
    if needle.is_empty() {
        return out;
    }
    let mut line = 0;
    while let Some(l) = logical_line(e, line) {
        let hay: Vec<char> = l.chars.iter().copied().map(fold).collect();
        let mut i = 0;
        while i + needle.len() <= hay.len() {
            if hay[i..i + needle.len()] == needle[..] {
                out.push(Match {
                    start: l.points[i],
                    end: char_end(e, l.points[i + needle.len() - 1]),
                });
                if out.len() >= limit {
                    return out;
                }
                i += needle.len();
            } else {
                i += 1;
            }
        }
        line = l.next;
    }
    out
}

/// A plain-text URL found in the grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub start: Point,
    pub end: Point,
    pub url: String,
}

/// Only web links are detected in plain text. `file://` is left out on
/// purpose: on Windows, opening one can launch an executable.
const SCHEMES: [&str; 2] = ["https://", "http://"];

/// Whether a link target is safe to hand to the OS opener.
pub fn is_openable(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    SCHEMES.iter().any(|s| lower.starts_with(s))
}

fn is_url_char(c: char) -> bool {
    !c.is_whitespace() && !c.is_control() && !"<>\"'`{}|\\^│".contains(c)
}

/// URLs on the logical line containing `line`.
pub fn links_at(e: &(impl TerminalEngine + ?Sized), line: usize) -> Vec<Link> {
    let (first, _) = logical_bounds(e, line);
    let Some(l) = logical_line(e, first) else {
        return Vec::new();
    };
    let text: String = l.chars.iter().collect();
    // Byte offset -> char index, for mapping `find` results back.
    let byte_to_char: Vec<usize> = {
        let mut v = vec![0; text.len() + 1];
        for (ci, (bi, _)) in text.char_indices().enumerate() {
            v[bi] = ci;
        }
        v[text.len()] = l.chars.len();
        v
    };
    let mut out = Vec::new();
    let mut from = 0;
    while from < text.len() {
        let Some((pos, scheme)) = SCHEMES
            .iter()
            .filter_map(|s| text[from..].find(s).map(|p| (from + p, *s)))
            .min_by_key(|(p, _)| *p)
        else {
            break;
        };
        let start = byte_to_char[pos];
        let mut end = start + scheme.chars().count();
        while end < l.chars.len() && is_url_char(l.chars[end]) {
            end += 1;
        }
        // Trim trailing punctuation and unbalanced closing brackets.
        while let Some(&last) = l.chars[start..end].last() {
            let unbalanced = |open: char, close: char| {
                last == close && {
                    let s = &l.chars[start..end];
                    s.iter().filter(|&&c| c == close).count()
                        > s.iter().filter(|&&c| c == open).count()
                }
            };
            if ".,:;!?".contains(last) || unbalanced('(', ')') || unbalanced('[', ']') {
                end -= 1;
            } else {
                break;
            }
        }
        if end > start + scheme.chars().count() {
            out.push(Link {
                start: l.points[start],
                end: char_end(e, l.points[end - 1]),
                url: l.chars[start..end].iter().collect(),
            });
        }
        from = text
            .char_indices()
            .nth(end)
            .map_or(text.len(), |(b, _)| b)
            .max(pos + 1);
    }
    out
}

/// The link under `p`: an OSC 8 hyperlink if the program set one, else a
/// detected URL.
pub fn link_at(e: &(impl TerminalEngine + ?Sized), p: Point) -> Option<Link> {
    let cells = e.line(p.line)?;
    if let Some(url) = cells.get(p.col as usize).and_then(|c| c.link.clone()) {
        let same = |c: &Cell| c.link.as_deref() == Some(&*url);
        let mut s = p.col as usize;
        while s > 0 && same(&cells[s - 1]) {
            s -= 1;
        }
        let mut t = p.col as usize;
        while t + 1 < cells.len() && same(&cells[t + 1]) {
            t += 1;
        }
        return Some(Link {
            start: Point::new(p.line, s as u16),
            end: Point::new(p.line, t as u16),
            url: url.to_string(),
        });
    }
    links_at(e, p.line)
        .into_iter()
        .find(|l| in_range((l.start, l.end), p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AlacrittyEngine, GridSize};

    fn engine(cols: u16, text: &str) -> AlacrittyEngine {
        let mut e = AlacrittyEngine::new(GridSize::new(cols, 6));
        e.feed(text.as_bytes());
        e
    }

    fn sel(a: (usize, u16), b: (usize, u16), kind: SelectionKind) -> Selection {
        let mut s = Selection::new(Point::new(a.0, a.1), kind);
        s.extend(Point::new(b.0, b.1));
        s
    }

    #[test]
    fn cell_selection_across_lines_trims_trailing_blanks() {
        let e = engine(20, "hello world\r\nsecond line");
        let s = sel((0, 6), (1, 5), SelectionKind::Cells);
        assert_eq!(s.text(&e), "world\nsecond");
        // Backwards drag gives the same text.
        let s = sel((1, 5), (0, 6), SelectionKind::Cells);
        assert_eq!(s.text(&e), "world\nsecond");
    }

    #[test]
    fn click_without_drag_selects_nothing() {
        let e = engine(20, "abc");
        assert_eq!(sel((0, 1), (0, 1), SelectionKind::Cells).text(&e), "");
    }

    #[test]
    fn soft_wrapped_lines_join_without_newline() {
        let e = engine(10, "0123456789abcdef");
        let s = sel((0, 0), (1, 5), SelectionKind::Cells);
        assert_eq!(s.text(&e), "0123456789abcdef");
    }

    #[test]
    fn word_selection_keeps_paths_and_locations_whole() {
        let e = engine(40, "error at src/main.rs:12:5 (here)");
        let s = sel((0, 12), (0, 12), SelectionKind::Words);
        assert_eq!(s.text(&e), "src/main.rs:12:5");
        let s = sel((0, 28), (0, 28), SelectionKind::Words);
        assert_eq!(s.text(&e), "here");
    }

    #[test]
    fn word_selection_follows_soft_wraps() {
        let e = engine(10, "aa verylongword");
        let s = sel((1, 1), (1, 1), SelectionKind::Words);
        assert_eq!(s.text(&e), "verylongword");
    }

    #[test]
    fn line_selection_takes_whole_logical_lines() {
        let e = engine(10, "0123456789ab\r\nnext");
        let s = sel((1, 0), (1, 0), SelectionKind::Lines);
        assert_eq!(s.text(&e), "0123456789ab");
    }

    #[test]
    fn wide_characters_select_as_one() {
        let e = engine(20, "a日本b");
        let s = sel((0, 1), (0, 3), SelectionKind::Cells);
        assert_eq!(s.text(&e), "日本");
    }

    #[test]
    fn search_is_case_insensitive_and_spans_wraps() {
        let e = engine(10, "Error one\r\nxxxxxxxxerror\r\nERR");
        let m = find_all(&e, "error", false, 100);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].start, Point::new(0, 0));
        // Second match starts on row 1 col 8 and wraps onto row 2.
        assert_eq!(m[1].start, Point::new(1, 8));
        assert_eq!(m[1].end, Point::new(2, 2));
        assert_eq!(find_all(&e, "error", true, 100).len(), 1);
        assert_eq!(find_all(&e, "", false, 100).len(), 0);
        assert_eq!(find_all(&e, "r", false, 3).len(), 3, "limit applies");
    }

    #[test]
    fn urls_are_detected_and_trimmed() {
        let e = engine(
            80,
            "see https://example.com/a?b=1, or (http://x.dev/p) now.",
        );
        let links = links_at(&e, 0);
        let urls: Vec<_> = links.iter().map(|l| l.url.as_str()).collect();
        assert_eq!(urls, ["https://example.com/a?b=1", "http://x.dev/p"]);
        assert_eq!(links[0].start, Point::new(0, 4));
        let wiki = engine(
            80,
            "https://en.wikipedia.org/wiki/Rust_(programming_language).",
        );
        assert_eq!(
            links_at(&wiki, 0)[0].url,
            "https://en.wikipedia.org/wiki/Rust_(programming_language)"
        );
    }

    #[test]
    fn wrapped_url_is_found_from_either_row() {
        let e = engine(12, "https://example.com/path");
        let a = link_at(&e, Point::new(0, 3)).map(|l| l.url);
        let b = link_at(&e, Point::new(1, 3)).map(|l| l.url);
        assert_eq!(a.as_deref(), Some("https://example.com/path"));
        assert_eq!(a, b);
        assert!(link_at(&e, Point::new(3, 0)).is_none());
    }

    #[test]
    fn only_web_links_are_openable() {
        assert!(is_openable("https://example.com"));
        assert!(is_openable("HTTP://example.com"));
        assert!(!is_openable("file:///C:/Windows/System32/calc.exe"));
        assert!(!is_openable("javascript:alert(1)"));
    }

    #[test]
    fn osc8_link_wins_over_text() {
        let e = engine(40, "\x1b]8;;https://docs.rs\x1b\\docs\x1b]8;;\x1b\\ rest");
        let l = link_at(&e, Point::new(0, 2)).expect("link");
        assert_eq!(l.url, "https://docs.rs");
        assert_eq!((l.start.col, l.end.col), (0, 3));
        assert!(link_at(&e, Point::new(0, 6)).is_none());
    }
}
