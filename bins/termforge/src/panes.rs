//! Pane tree for split terminals (ADR 0012).
//!
//! A workspace tab owns one `PaneTree`. Leaves are pane ids, interior nodes
//! split their rectangle between two children. The tree is pure data: it
//! knows nothing about sessions or GPUI, so geometry, focus movement and
//! persistence can be tested without a display.

use std::fmt::Write as _;

pub type PaneId = u64;

/// Splits shallower than this are rejected, and decoding refuses deeper input.
const MAX_DEPTH: usize = 16;
/// Smallest share either side of a split may be given.
const MIN_RATIO: f32 = 0.1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// Children sit side by side (a vertical divider).
    Horizontal,
    /// Children are stacked (a horizontal divider).
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Leaf(PaneId),
    Split {
        axis: Axis,
        /// Share of the rectangle given to `a`.
        ratio: f32,
        a: Box<Node>,
        b: Box<Node>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct PaneTree {
    root: Node,
    focus: PaneId,
}

impl PaneTree {
    pub fn new(id: PaneId) -> Self {
        Self {
            root: Node::Leaf(id),
            focus: id,
        }
    }

    pub fn focused(&self) -> PaneId {
        self.focus
    }

    pub fn len(&self) -> usize {
        self.ids().len()
    }

    pub fn contains(&self, id: PaneId) -> bool {
        self.ids().contains(&id)
    }

    /// Pane ids in reading order (left/top first).
    pub fn ids(&self) -> Vec<PaneId> {
        fn walk(n: &Node, out: &mut Vec<PaneId>) {
            match n {
                Node::Leaf(id) => out.push(*id),
                Node::Split { a, b, .. } => {
                    walk(a, out);
                    walk(b, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.root, &mut out);
        out
    }

    pub fn set_focus(&mut self, id: PaneId) -> bool {
        if self.contains(id) {
            self.focus = id;
            true
        } else {
            false
        }
    }

    /// Split `target`, placing `new` after it, and focus the new pane.
    /// Returns false if `target` is unknown, `new` already exists, or the
    /// tree is already at its maximum depth there.
    pub fn split(&mut self, target: PaneId, axis: Axis, new: PaneId) -> bool {
        if self.contains(new) {
            return false;
        }
        fn go(n: &mut Node, target: PaneId, axis: Axis, new: PaneId, depth: usize) -> bool {
            match n {
                Node::Leaf(id) if *id == target => {
                    if depth >= MAX_DEPTH {
                        return false;
                    }
                    *n = Node::Split {
                        axis,
                        ratio: 0.5,
                        a: Box::new(Node::Leaf(target)),
                        b: Box::new(Node::Leaf(new)),
                    };
                    true
                }
                Node::Leaf(_) => false,
                Node::Split { a, b, .. } => {
                    go(a, target, axis, new, depth + 1) || go(b, target, axis, new, depth + 1)
                }
            }
        }
        let done = go(&mut self.root, target, axis, new, 0);
        if done {
            self.focus = new;
        }
        done
    }

    /// Remove a pane; its sibling takes the space. Returns the pane that
    /// should receive focus, or `None` if `id` is the last pane (close the
    /// tab instead) or unknown.
    pub fn close(&mut self, id: PaneId) -> Option<PaneId> {
        if self.len() <= 1 || !self.contains(id) {
            return None;
        }
        fn go(n: Node, id: PaneId) -> Node {
            match n {
                Node::Split { axis, ratio, a, b } => match (*a, *b) {
                    (Node::Leaf(x), sibling) if x == id => sibling,
                    (sibling, Node::Leaf(x)) if x == id => sibling,
                    (a, b) => Node::Split {
                        axis,
                        ratio,
                        a: Box::new(go(a, id)),
                        b: Box::new(go(b, id)),
                    },
                },
                leaf => leaf,
            }
        }
        let before = self.ids();
        let at = before.iter().position(|p| *p == id).unwrap_or(0);
        self.root = go(std::mem::replace(&mut self.root, Node::Leaf(0)), id);
        if self.focus == id {
            // Prefer the pane that was just before, else just after.
            let after = self.ids();
            self.focus = after[at.saturating_sub(1).min(after.len() - 1)];
        }
        Some(self.focus)
    }

    /// Rectangle of every pane inside `area`, in reading order.
    pub fn layout(&self, area: Rect) -> Vec<(PaneId, Rect)> {
        fn go(n: &Node, r: Rect, out: &mut Vec<(PaneId, Rect)>) {
            match n {
                Node::Leaf(id) => out.push((*id, r)),
                Node::Split { axis, ratio, a, b } => match axis {
                    Axis::Horizontal => {
                        let wa = r.w * ratio;
                        go(a, Rect::new(r.x, r.y, wa, r.h), out);
                        go(b, Rect::new(r.x + wa, r.y, r.w - wa, r.h), out);
                    }
                    Axis::Vertical => {
                        let ha = r.h * ratio;
                        go(a, Rect::new(r.x, r.y, r.w, ha), out);
                        go(b, Rect::new(r.x, r.y + ha, r.w, r.h - ha), out);
                    }
                },
            }
        }
        let mut out = Vec::new();
        go(&self.root, area, &mut out);
        out
    }

    /// The pane nearest `from` in direction `dir`, by geometry rather than
    /// tree order, so focus moves the way the panes look.
    pub fn neighbor(&self, from: PaneId, dir: Dir, area: Rect) -> Option<PaneId> {
        const EPS: f32 = 0.001;
        let rects = self.layout(area);
        let (_, src) = *rects.iter().find(|(id, _)| *id == from)?;
        let (sx, sy) = src.center();
        rects
            .iter()
            .filter(|(id, r)| {
                *id != from
                    && match dir {
                        Dir::Left => r.x + r.w <= src.x + EPS,
                        Dir::Right => r.x >= src.x + src.w - EPS,
                        Dir::Up => r.y + r.h <= src.y + EPS,
                        Dir::Down => r.y >= src.y + src.h - EPS,
                    }
                    && match dir {
                        // Must overlap on the cross axis.
                        Dir::Left | Dir::Right => {
                            r.y < src.y + src.h - EPS && r.y + r.h > src.y + EPS
                        }
                        Dir::Up | Dir::Down => r.x < src.x + src.w - EPS && r.x + r.w > src.x + EPS,
                    }
            })
            .min_by(|(_, a), (_, b)| {
                let d = |r: &Rect| {
                    let (cx, cy) = r.center();
                    (cx - sx).abs() + (cy - sy).abs()
                };
                d(a).total_cmp(&d(b))
            })
            .map(|(id, _)| *id)
    }

    /// Move the divider nearest to `id` on the given side by `delta` (a
    /// fraction of that split's size). Returns false if there is no divider
    /// on that side.
    pub fn resize(&mut self, id: PaneId, dir: Dir, delta: f32) -> bool {
        let axis = match dir {
            Dir::Left | Dir::Right => Axis::Horizontal,
            Dir::Up | Dir::Down => Axis::Vertical,
        };
        // Growing towards Right/Down means the pane is in `a`; towards
        // Left/Up it must be in `b`, and the divider moves the other way.
        let in_a = matches!(dir, Dir::Right | Dir::Down);
        fn leaves(n: &Node, id: PaneId) -> bool {
            match n {
                Node::Leaf(x) => *x == id,
                Node::Split { a, b, .. } => leaves(a, id) || leaves(b, id),
            }
        }
        fn go(n: &mut Node, id: PaneId, axis: Axis, in_a: bool, delta: f32) -> bool {
            let Node::Split {
                axis: ax,
                ratio,
                a,
                b,
            } = n
            else {
                return false;
            };
            let (near, other) = if in_a { (a, b) } else { (b, a) };
            if leaves(near, id) {
                // Prefer a deeper divider that is closer to the pane.
                if go(near, id, axis, in_a, delta) {
                    return true;
                }
                if *ax == axis {
                    let signed = if in_a { delta } else { -delta };
                    *ratio = (*ratio + signed).clamp(MIN_RATIO, 1.0 - MIN_RATIO);
                    return true;
                }
                return false;
            }
            // Pane is on the other side: the divider is not on its `dir` edge
            // at this level, but may be deeper.
            go(other, id, axis, in_a, delta)
        }
        go(&mut self.root, id, axis, in_a, delta)
    }

    /// The same shape with every pane id passed through `f`. Used to turn
    /// runtime ids into stable positions for the layout file and back.
    pub fn remap(&self, f: impl Fn(PaneId) -> PaneId) -> Self {
        fn go(n: &Node, f: &impl Fn(PaneId) -> PaneId) -> Node {
            match n {
                Node::Leaf(id) => Node::Leaf(f(*id)),
                Node::Split { axis, ratio, a, b } => Node::Split {
                    axis: *axis,
                    ratio: *ratio,
                    a: Box::new(go(a, f)),
                    b: Box::new(go(b, f)),
                },
            }
        }
        Self {
            root: go(&self.root, &f),
            focus: f(self.focus),
        }
    }

    /// Compact text form for the layout file: `1`, `h0.500(1,2)`, `v0.300(1,h0.500(2,3))`.
    pub fn encode(&self) -> String {
        fn go(n: &Node, out: &mut String) {
            match n {
                Node::Leaf(id) => {
                    let _ = write!(out, "{id}");
                }
                Node::Split { axis, ratio, a, b } => {
                    let c = match axis {
                        Axis::Horizontal => 'h',
                        Axis::Vertical => 'v',
                    };
                    let _ = write!(out, "{c}{ratio:.3}(");
                    go(a, out);
                    out.push(',');
                    go(b, out);
                    out.push(')');
                }
            }
        }
        let mut out = String::new();
        go(&self.root, &mut out);
        out
    }

    /// Inverse of [`encode`](Self::encode). Rejects malformed input,
    /// duplicate ids, out-of-range ratios and excessive depth; focus lands
    /// on the first pane.
    pub fn decode(text: &str) -> Option<Self> {
        fn number<T: std::str::FromStr>(s: &mut &str, accept: impl Fn(char) -> bool) -> Option<T> {
            let end = s.find(|c: char| !accept(c)).unwrap_or(s.len());
            let (head, tail) = s.split_at(end);
            *s = tail;
            head.parse().ok()
        }
        fn node(s: &mut &str, depth: usize) -> Option<Node> {
            if depth > MAX_DEPTH {
                return None;
            }
            let axis = match s.chars().next()? {
                'h' => Some(Axis::Horizontal),
                'v' => Some(Axis::Vertical),
                _ => None,
            };
            let Some(axis) = axis else {
                return Some(Node::Leaf(number(s, |c| c.is_ascii_digit())?));
            };
            *s = &s[1..];
            let ratio: f32 = number(s, |c| c.is_ascii_digit() || c == '.')?;
            if !(MIN_RATIO..=1.0 - MIN_RATIO).contains(&ratio) {
                return None;
            }
            *s = s.strip_prefix('(')?;
            let a = node(s, depth + 1)?;
            *s = s.strip_prefix(',')?;
            let b = node(s, depth + 1)?;
            *s = s.strip_prefix(')')?;
            Some(Node::Split {
                axis,
                ratio,
                a: Box::new(a),
                b: Box::new(b),
            })
        }
        let mut rest = text;
        let root = node(&mut rest, 0)?;
        if !rest.is_empty() {
            return None;
        }
        let tree = Self { focus: 0, root };
        let mut ids = tree.ids();
        let first = *ids.first()?;
        ids.sort_unstable();
        if ids.windows(2).any(|w| w[0] == w[1]) {
            return None;
        }
        Some(Self {
            focus: first,
            ..tree
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect::new(0.0, 0.0, 1000.0, 600.0);

    fn area_sum(t: &PaneTree) -> f32 {
        t.layout(AREA).iter().map(|(_, r)| r.w * r.h).sum()
    }

    /// 1 | (2 over 3)
    fn sample() -> PaneTree {
        let mut t = PaneTree::new(1);
        assert!(t.split(1, Axis::Horizontal, 2));
        assert!(t.split(2, Axis::Vertical, 3));
        t
    }

    #[test]
    fn split_focuses_new_pane_and_tiles_the_area() {
        let t = sample();
        assert_eq!(t.focused(), 3);
        assert_eq!(t.ids(), vec![1, 2, 3]);
        assert!((area_sum(&t) - AREA.w * AREA.h).abs() < 1.0);
        let rects = t.layout(AREA);
        assert_eq!(rects[0].1, Rect::new(0.0, 0.0, 500.0, 600.0));
        assert_eq!(rects[1].1, Rect::new(500.0, 0.0, 500.0, 300.0));
        assert_eq!(rects[2].1, Rect::new(500.0, 300.0, 500.0, 300.0));
    }

    #[test]
    fn split_rejects_unknown_target_and_duplicate_id() {
        let mut t = PaneTree::new(1);
        assert!(!t.split(9, Axis::Horizontal, 2));
        assert!(!t.split(1, Axis::Horizontal, 1));
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn close_gives_space_to_sibling_and_moves_focus() {
        let mut t = sample();
        assert_eq!(t.close(3), Some(2));
        assert_eq!(t.layout(AREA)[1].1, Rect::new(500.0, 0.0, 500.0, 600.0));
        assert_eq!(t.close(1), Some(2));
        assert_eq!(t.close(2), None, "last pane is closed by closing the tab");
        assert_eq!(t.ids(), vec![2]);
    }

    #[test]
    fn neighbor_follows_geometry() {
        let t = sample();
        assert_eq!(t.neighbor(1, Dir::Right, AREA), Some(2));
        assert_eq!(t.neighbor(3, Dir::Up, AREA), Some(2));
        assert_eq!(t.neighbor(2, Dir::Down, AREA), Some(3));
        assert_eq!(t.neighbor(3, Dir::Left, AREA), Some(1));
        assert_eq!(t.neighbor(1, Dir::Left, AREA), None);
        assert_eq!(t.neighbor(1, Dir::Up, AREA), None);
    }

    #[test]
    fn resize_moves_the_divider_and_clamps() {
        let mut t = sample();
        assert!(t.resize(1, Dir::Right, 0.2));
        assert_eq!(t.layout(AREA)[0].1.w, 700.0);
        assert!(t.resize(1, Dir::Right, 5.0));
        assert!((t.layout(AREA)[0].1.w - 900.0).abs() < 0.01);
        assert!(!t.resize(1, Dir::Left, 0.1), "no divider on the left edge");
        assert!(
            !t.resize(1, Dir::Down, 0.1),
            "no vertical divider beside pane 1"
        );
        assert!(t.resize(3, Dir::Up, 0.1));
    }

    #[test]
    fn encode_round_trips() {
        let t = sample();
        let text = t.encode();
        assert_eq!(text, "h0.500(1,v0.500(2,3))");
        let back = PaneTree::decode(&text).unwrap();
        assert_eq!(back.ids(), t.ids());
        assert_eq!(back.layout(AREA), t.layout(AREA));
        assert_eq!(back.focused(), 1);
    }

    #[test]
    fn remap_rewrites_ids_and_focus() {
        let t = sample();
        let m = t.remap(|id| id + 10);
        assert_eq!(m.ids(), vec![11, 12, 13]);
        assert_eq!(m.focused(), 13);
        assert_eq!(m.layout(AREA).len(), 3);
    }

    #[test]
    fn decode_rejects_bad_input() {
        for bad in [
            "",
            "h0.5(1,2",
            "h0.5(1,1)",
            "x",
            "h0.05(1,2)",
            "h0.5(1,2)x",
            "h0.5(1)",
            "(",
            "h(1,2)",
            "99999999999999999999999",
        ] {
            assert!(PaneTree::decode(bad).is_none(), "{bad:?}");
        }
        let deep = "h0.5(1,".repeat(40) + "2" + &")".repeat(40);
        assert!(PaneTree::decode(&deep).is_none());
    }

    #[test]
    fn depth_is_bounded() {
        let mut t = PaneTree::new(0);
        let mut ok = 0;
        for i in 1..40 {
            if t.split(i - 1, Axis::Horizontal, i) {
                ok += 1;
            }
        }
        assert_eq!(ok, MAX_DEPTH);
        assert!(PaneTree::decode(&t.encode()).is_some());
    }
}
