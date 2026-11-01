//! Polymorphic area split tree for Blender-style spatial workspaces (ADR 0012, ADR 0021).
//!
//! A workspace tab owns one `PaneTree`. Each leaf is an [`AreaNode`] with a
//! unique [`AreaId`] and an active [`EditorKind`], while interior nodes split
//! their rectangle between two children. The tree is pure data: it knows
//! nothing about live PTY handles or GPUI, so geometry, focus movement, editor
//! swapping, and persistence can be tested without a display.

use std::fmt::Write as _;

pub type PaneId = u64;
pub type AreaId = PaneId;
pub type PtySessionId = PaneId;

/// Splits shallower than this are rejected, and decoding refuses deeper input.
const MAX_DEPTH: usize = 16;
/// Smallest share either side of a split may be given.
const MIN_RATIO: f32 = 0.1;

/// Active semantic lens on the 2D `GraphCanvas` viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CanvasLens {
    /// Crates, packages, modules, and architectural dependency DAG.
    #[default]
    Architecture,
    /// Symbols, structs, functions, and intra-file call flow DAG.
    FunctionFlow,
    /// Live `forged` daemon, AI coding agents, SSH/cloud hosts, worktrees, and PTY sessions.
    SystemAgents,
}

impl CanvasLens {
    pub const ALL: [Self; 3] = [Self::Architecture, Self::FunctionFlow, Self::SystemAgents];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Architecture => "Architecture",
            Self::FunctionFlow => "Function Flow",
            Self::SystemAgents => "Agents & Runtime",
        }
    }

    pub const fn id(self) -> &'static str {
        match self {
            Self::Architecture => "arch",
            Self::FunctionFlow => "flow",
            Self::SystemAgents => "runtime",
        }
    }

    pub const fn icon(self) -> &'static str {
        match self {
            Self::Architecture => "◈",
            Self::FunctionFlow => "ƒ",
            Self::SystemAgents => "✦",
        }
    }
}

/// State for the interactive 2D pan/zoom topology viewport (`EditorKind::GraphCanvas`).
#[derive(Debug, Clone, PartialEq)]
pub struct CanvasState {
    pub pan_x: f32,
    pub pan_y: f32,
    pub zoom: f32,
    pub selected_node: Option<String>,
    pub lens: CanvasLens,
}

impl Default for CanvasState {
    fn default() -> Self {
        Self {
            pan_x: 0.0,
            pan_y: 0.0,
            zoom: 1.0,
            selected_node: None,
            lens: CanvasLens::default(),
        }
    }
}

impl CanvasState {
    pub fn world_to_screen(&self, wx: f32, wy: f32) -> (f32, f32) {
        (wx * self.zoom + self.pan_x, wy * self.zoom + self.pan_y)
    }

    #[cfg(test)]
    pub fn screen_to_world(&self, sx: f32, sy: f32) -> (f32, f32) {
        let z = self.zoom.max(0.25);
        ((sx - self.pan_x) / z, (sy - self.pan_y) / z)
    }

    pub fn zoom_by(&mut self, delta: f32) {
        self.zoom = (self.zoom + delta).clamp(0.4, 2.5);
    }

    pub fn pan_by(&mut self, dx: f32, dy: f32) {
        self.pan_x += dx;
        self.pan_y += dy;
    }

    pub fn reset_view(&mut self) {
        self.pan_x = 0.0;
        self.pan_y = 0.0;
        self.zoom = 1.0;
    }
}

/// State for the contextual property/state inspector (`EditorKind::Inspector`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InspectorState {
    pub target: Option<String>,
    pub section: String,
}

/// State for the project/worktree outliner (`EditorKind::Outliner`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProjectTreeState {
    pub filter: String,
    pub show_hidden: bool,
}

/// State for the log waterfall timeline (`EditorKind::Timeline`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogStreamState {
    pub follow: bool,
    pub filter: String,
}

impl Default for LogStreamState {
    fn default() -> Self {
        Self {
            follow: true,
            filter: String::new(),
        }
    }
}

/// Discriminant for selecting an [`EditorKind`] from an area header dropdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorKindTag {
    Terminal,
    GraphCanvas,
    Inspector,
    Outliner,
    Timeline,
    CodeEditor,
    GitDiff,
    TaskBoard,
    AgentFleet,
}

impl EditorKindTag {
    pub const ALL: [Self; 9] = [
        Self::Terminal,
        Self::GraphCanvas,
        Self::CodeEditor,
        Self::AgentFleet,
        Self::GitDiff,
        Self::Inspector,
        Self::Outliner,
        Self::Timeline,
        Self::TaskBoard,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Terminal => "Terminal",
            Self::GraphCanvas => "Graph Canvas",
            Self::Inspector => "Inspector",
            Self::Outliner => "Outliner",
            Self::Timeline => "Timeline",
            Self::CodeEditor => "Code Editor",
            Self::GitDiff => "Git Diff",
            Self::TaskBoard => "Task Board",
            Self::AgentFleet => "Agent Fleet",
        }
    }

    pub const fn icon(self) -> &'static str {
        match self {
            Self::Terminal => ">_",
            Self::GraphCanvas => "◈",
            Self::Inspector => "⊡",
            Self::Outliner => "▤",
            Self::Timeline => "≣",
            Self::CodeEditor => "✎",
            Self::GitDiff => "±",
            Self::TaskBoard => "☑",
            Self::AgentFleet => "✦",
        }
    }

    pub const fn category(self) -> &'static str {
        match self {
            Self::Terminal => "EXEC",
            Self::GraphCanvas => "VIEW",
            Self::Inspector => "DATA",
            Self::Outliner => "TREE",
            Self::Timeline => "LOGS",
            Self::CodeEditor => "CODE",
            Self::GitDiff => "DIFF",
            Self::TaskBoard => "TASK",
            Self::AgentFleet => "CMUX",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::Terminal => "Interactive PTY shell session",
            Self::GraphCanvas => "Architecture, function flow & runtime",
            Self::Inspector => "Selected node & project properties",
            Self::Outliner => "Workspace file & directory tree",
            Self::Timeline => "Live IPC, session & LSP waterfall",
            Self::CodeEditor => "Syntax buffer & source file editor",
            Self::GitDiff => "Working tree & staged changes",
            Self::TaskBoard => "Project task board & workflow",
            Self::AgentFleet => "AI agents, cloud SSH & cmux sessions",
        }
    }

    pub const fn code(self) -> char {
        match self {
            Self::Terminal => 't',
            Self::GraphCanvas => 'c',
            Self::Inspector => 'i',
            Self::Outliner => 'o',
            Self::Timeline => 'l',
            Self::CodeEditor => 'e',
            Self::GitDiff => 'g',
            Self::TaskBoard => 'k',
            Self::AgentFleet => 'a',
        }
    }

    pub const fn from_code(code: char) -> Option<Self> {
        match code {
            't' => Some(Self::Terminal),
            'c' => Some(Self::GraphCanvas),
            'i' => Some(Self::Inspector),
            'o' => Some(Self::Outliner),
            'l' => Some(Self::Timeline),
            'e' => Some(Self::CodeEditor),
            'g' => Some(Self::GitDiff),
            'k' => Some(Self::TaskBoard),
            'a' => Some(Self::AgentFleet),
            _ => None,
        }
    }
}

/// Polymorphic editor mounted inside a spatial workspace area.
#[derive(Debug, Clone, PartialEq)]
pub enum EditorKind {
    Terminal(PtySessionId),
    GraphCanvas(CanvasState),
    Inspector(InspectorState),
    Outliner(ProjectTreeState),
    Timeline(LogStreamState),
    CodeEditor,
    GitDiff,
    TaskBoard,
    AgentFleet,
}

impl EditorKind {
    pub const fn tag(&self) -> EditorKindTag {
        match self {
            Self::Terminal(_) => EditorKindTag::Terminal,
            Self::GraphCanvas(_) => EditorKindTag::GraphCanvas,
            Self::Inspector(_) => EditorKindTag::Inspector,
            Self::Outliner(_) => EditorKindTag::Outliner,
            Self::Timeline(_) => EditorKindTag::Timeline,
            Self::CodeEditor => EditorKindTag::CodeEditor,
            Self::GitDiff => EditorKindTag::GitDiff,
            Self::TaskBoard => EditorKindTag::TaskBoard,
            Self::AgentFleet => EditorKindTag::AgentFleet,
        }
    }

    pub fn from_tag(tag: EditorKindTag, session_id: PtySessionId) -> Self {
        match tag {
            EditorKindTag::Terminal => Self::Terminal(session_id),
            EditorKindTag::GraphCanvas => Self::GraphCanvas(CanvasState::default()),
            EditorKindTag::Inspector => Self::Inspector(InspectorState::default()),
            EditorKindTag::Outliner => Self::Outliner(ProjectTreeState::default()),
            EditorKindTag::Timeline => Self::Timeline(LogStreamState::default()),
            EditorKindTag::CodeEditor => Self::CodeEditor,
            EditorKindTag::GitDiff => Self::GitDiff,
            EditorKindTag::TaskBoard => Self::TaskBoard,
            EditorKindTag::AgentFleet => Self::AgentFleet,
        }
    }
}

/// A leaf in the workspace binary split tree.
#[derive(Debug, Clone, PartialEq)]
pub struct AreaNode {
    pub id: AreaId,
    pub editor: EditorKind,
}

impl AreaNode {
    pub fn new(id: AreaId, editor: EditorKind) -> Self {
        Self { id, editor }
    }

    pub fn terminal(id: AreaId) -> Self {
        Self {
            id,
            editor: EditorKind::Terminal(id),
        }
    }
}

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

/// A draggable boundary between two panes.
#[derive(Debug, Clone, PartialEq)]
pub struct Divider {
    /// Route from the root to the split (`false` = first child).
    pub path: Vec<bool>,
    pub axis: Axis,
    /// The split's own rectangle, in the same units as the layout area.
    pub parent: Rect,
    pub ratio: f32,
}

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Leaf(AreaNode),
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
        Self::new_with_editor(id, EditorKind::Terminal(id))
    }

    pub fn new_with_editor(id: AreaId, editor: EditorKind) -> Self {
        Self {
            root: Node::Leaf(AreaNode::new(id, editor)),
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
                Node::Leaf(area) => out.push(area.id),
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

    /// All [`AreaNode`] leaves in reading order.
    pub fn areas(&self) -> Vec<&AreaNode> {
        fn walk<'a>(n: &'a Node, out: &mut Vec<&'a AreaNode>) {
            match n {
                Node::Leaf(area) => out.push(area),
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

    /// Mutable access to all [`AreaNode`] leaves in reading order.
    pub fn areas_mut(&mut self) -> Vec<&mut AreaNode> {
        fn walk<'a>(n: &'a mut Node, out: &mut Vec<&'a mut AreaNode>) {
            match n {
                Node::Leaf(area) => out.push(area),
                Node::Split { a, b, .. } => {
                    walk(a, out);
                    walk(b, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&mut self.root, &mut out);
        out
    }

    #[allow(dead_code)] // Retained for saved spatial layouts while the workbench is replaced.
    pub fn area(&self, id: AreaId) -> Option<&AreaNode> {
        fn find(n: &Node, id: AreaId) -> Option<&AreaNode> {
            match n {
                Node::Leaf(area) if area.id == id => Some(area),
                Node::Leaf(_) => None,
                Node::Split { a, b, .. } => find(a, id).or_else(|| find(b, id)),
            }
        }
        find(&self.root, id)
    }

    pub fn area_mut(&mut self, id: AreaId) -> Option<&mut AreaNode> {
        fn find(n: &mut Node, id: AreaId) -> Option<&mut AreaNode> {
            match n {
                Node::Leaf(area) if area.id == id => Some(area),
                Node::Leaf(_) => None,
                Node::Split { a, b, .. } => find(a, id).or_else(|| find(b, id)),
            }
        }
        find(&mut self.root, id)
    }

    #[allow(dead_code)] // Retained for saved spatial layouts while the workbench is replaced.
    pub fn editor(&self, id: AreaId) -> Option<&EditorKind> {
        self.area(id).map(|area| &area.editor)
    }

    /// Swap the editor mounted in area `id` in place without altering split geometry.
    pub fn set_editor(&mut self, id: AreaId, editor: EditorKind) -> bool {
        if let Some(area) = self.area_mut(id) {
            area.editor = editor;
            true
        } else {
            false
        }
    }

    /// Swap the editor mounted in area `id` by tag, preserving its session id when
    /// switching back to [`EditorKindTag::Terminal`].
    pub fn set_editor_tag(&mut self, id: AreaId, tag: EditorKindTag) -> bool {
        if let Some(area) = self.area_mut(id) {
            if area.editor.tag() != tag {
                area.editor = EditorKind::from_tag(tag, id);
            }
            true
        } else {
            false
        }
    }

    pub fn set_focus(&mut self, id: PaneId) -> bool {
        if self.contains(id) {
            self.focus = id;
            true
        } else {
            false
        }
    }

    /// Split `target`, placing `new` after it with a default `Terminal` editor,
    /// and focus the new pane.
    /// Returns false if `target` is unknown, `new` already exists, or the
    /// tree is already at its maximum depth there.
    pub fn split(&mut self, target: PaneId, axis: Axis, new: PaneId) -> bool {
        self.split_with_editor(target, axis, 0.5, new, EditorKind::Terminal(new))
    }

    /// Split `target` with a custom ratio and explicit [`EditorKind`] for `new`.
    pub fn split_with_editor(
        &mut self,
        target: AreaId,
        axis: Axis,
        ratio: f32,
        new: AreaId,
        editor: EditorKind,
    ) -> bool {
        if self.contains(new) || !ratio.is_finite() {
            return false;
        }
        let clamped_ratio = ratio.clamp(MIN_RATIO, 1.0 - MIN_RATIO);
        fn go(
            n: &mut Node,
            target: AreaId,
            axis: Axis,
            ratio: f32,
            new_area: &mut Option<AreaNode>,
            depth: usize,
        ) -> bool {
            match n {
                Node::Leaf(existing) if existing.id == target => {
                    if depth >= MAX_DEPTH {
                        return false;
                    }
                    let Some(next_area) = new_area.take() else {
                        return false;
                    };
                    let current = existing.clone();
                    *n = Node::Split {
                        axis,
                        ratio,
                        a: Box::new(Node::Leaf(current)),
                        b: Box::new(Node::Leaf(next_area)),
                    };
                    true
                }
                Node::Leaf(_) => false,
                Node::Split { a, b, .. } => {
                    go(a, target, axis, ratio, new_area, depth + 1)
                        || go(b, target, axis, ratio, new_area, depth + 1)
                }
            }
        }
        let mut new_area = Some(AreaNode::new(new, editor));
        let done = go(
            &mut self.root,
            target,
            axis,
            clamped_ratio,
            &mut new_area,
            0,
        );
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
                    (Node::Leaf(x), sibling) if x.id == id => sibling,
                    (sibling, Node::Leaf(x)) if x.id == id => sibling,
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
        self.root = go(
            std::mem::replace(&mut self.root, Node::Leaf(AreaNode::terminal(0))),
            id,
        );
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
                Node::Leaf(leaf) => out.push((leaf.id, r)),
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
                Node::Leaf(x) => x.id == id,
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

    /// Every divider with the rectangle of the split it belongs to, so a
    /// drag can turn a pointer position back into a ratio.
    pub fn dividers(&self, area: Rect) -> Vec<Divider> {
        fn go(n: &Node, r: Rect, path: &mut Vec<bool>, out: &mut Vec<Divider>) {
            let Node::Split { axis, ratio, a, b } = n else {
                return;
            };
            out.push(Divider {
                path: path.clone(),
                axis: *axis,
                parent: r,
                ratio: *ratio,
            });
            let (ra, rb) = match axis {
                Axis::Horizontal => {
                    let wa = r.w * ratio;
                    (
                        Rect::new(r.x, r.y, wa, r.h),
                        Rect::new(r.x + wa, r.y, r.w - wa, r.h),
                    )
                }
                Axis::Vertical => {
                    let ha = r.h * ratio;
                    (
                        Rect::new(r.x, r.y, r.w, ha),
                        Rect::new(r.x, r.y + ha, r.w, r.h - ha),
                    )
                }
            };
            path.push(false);
            go(a, ra, path, out);
            path.pop();
            path.push(true);
            go(b, rb, path, out);
            path.pop();
        }
        let mut out = Vec::new();
        go(&self.root, area, &mut Vec::new(), &mut out);
        out
    }

    /// Set the ratio of the split at `path` (as reported by
    /// [`dividers`](Self::dividers)), clamped so neither side vanishes.
    pub fn set_ratio(&mut self, path: &[bool], ratio: f32) -> bool {
        let mut node = &mut self.root;
        for step in path {
            let Node::Split { a, b, .. } = node else {
                return false;
            };
            node = if *step { b } else { a };
        }
        match node {
            Node::Split { ratio: r, .. } if ratio.is_finite() => {
                *r = ratio.clamp(MIN_RATIO, 1.0 - MIN_RATIO);
                true
            }
            _ => false,
        }
    }

    /// The same shape with every pane id passed through `f`. Used to turn
    /// runtime ids into stable positions for the layout file and back.
    pub fn remap(&self, f: impl Fn(PaneId) -> PaneId) -> Self {
        fn go(n: &Node, f: &impl Fn(PaneId) -> PaneId) -> Node {
            match n {
                Node::Leaf(area) => {
                    let new_id = f(area.id);
                    let editor = match &area.editor {
                        EditorKind::Terminal(pty_id) if *pty_id == area.id => {
                            EditorKind::Terminal(new_id)
                        }
                        EditorKind::Terminal(pty_id) => EditorKind::Terminal(f(*pty_id)),
                        other => other.clone(),
                    };
                    Node::Leaf(AreaNode::new(new_id, editor))
                }
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

    /// Compact text form for the layout file: `1`, `h0.500(1,2)`, `v0.300(1:c,h0.500(2:i,3))`.
    pub fn encode(&self) -> String {
        fn go(n: &Node, out: &mut String) {
            match n {
                Node::Leaf(area) => {
                    let id = area.id;
                    match area.editor.tag() {
                        EditorKindTag::Terminal => {
                            let _ = write!(out, "{id}");
                        }
                        tag => {
                            let code = tag.code();
                            let _ = write!(out, "{id}:{code}");
                        }
                    }
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
                let id: AreaId = number(s, |c| c.is_ascii_digit())?;
                let editor = if let Some(after_colon) = s.strip_prefix(':') {
                    let code = after_colon.chars().next()?;
                    let tag = EditorKindTag::from_code(code)?;
                    *s = &after_colon[code.len_utf8()..];
                    EditorKind::from_tag(tag, id)
                } else {
                    EditorKind::Terminal(id)
                };
                return Some(Node::Leaf(AreaNode::new(id, editor)));
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
    fn dividers_report_their_split_rectangle() {
        let t = sample();
        let d = t.dividers(AREA);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].path, Vec::<bool>::new());
        assert_eq!(d[0].parent, AREA);
        assert_eq!(d[1].path, vec![true]);
        assert_eq!(d[1].axis, Axis::Vertical);
        assert_eq!(d[1].parent, Rect::new(500.0, 0.0, 500.0, 600.0));
    }

    #[test]
    fn set_ratio_follows_paths_and_clamps() {
        let mut t = sample();
        assert!(t.set_ratio(&[true], 0.25));
        assert_eq!(t.layout(AREA)[1].1.h, 150.0);
        assert!(t.set_ratio(&[], 0.0));
        assert!((t.layout(AREA)[0].1.w - 100.0).abs() < 0.01);
        assert!(!t.set_ratio(&[false], 0.5), "a leaf has no ratio");
        assert!(!t.set_ratio(&[true, true, true], 0.5));
        assert!(!t.set_ratio(&[], f32::NAN));
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

    #[test]
    fn polymorphic_editor_kind_swaps_in_place_and_round_trips() {
        let mut t = PaneTree::new_with_editor(1, EditorKind::GraphCanvas(CanvasState::default()));
        assert!(t.split_with_editor(
            1,
            Axis::Horizontal,
            0.7,
            2,
            EditorKind::Inspector(InspectorState::default()),
        ));
        assert!(t.split_with_editor(2, Axis::Vertical, 0.5, 3, EditorKind::Terminal(3)));
        assert_eq!(
            t.editor(1).map(EditorKind::tag),
            Some(EditorKindTag::GraphCanvas)
        );
        assert_eq!(
            t.editor(2).map(EditorKind::tag),
            Some(EditorKindTag::Inspector)
        );
        assert_eq!(
            t.editor(3).map(EditorKind::tag),
            Some(EditorKindTag::Terminal)
        );

        let encoded = t.encode();
        assert_eq!(encoded, "h0.700(1:c,v0.500(2:i,3))");
        let decoded = PaneTree::decode(&encoded).expect("decode polymorphic tree");
        assert_eq!(
            decoded.editor(1).map(EditorKind::tag),
            Some(EditorKindTag::GraphCanvas)
        );
        assert_eq!(
            decoded.editor(2).map(EditorKind::tag),
            Some(EditorKindTag::Inspector)
        );
        assert_eq!(
            decoded.editor(3).map(EditorKind::tag),
            Some(EditorKindTag::Terminal)
        );

        assert!(t.set_editor_tag(2, EditorKindTag::Timeline));
        assert_eq!(
            t.editor(2).map(EditorKind::tag),
            Some(EditorKindTag::Timeline)
        );
        assert!(t.set_editor_tag(2, EditorKindTag::Terminal));
        assert_eq!(t.editor(2), Some(&EditorKind::Terminal(2)));
    }

    #[test]
    fn canvas_state_transforms_world_and_screen_coordinates() {
        let mut canvas = CanvasState::default();
        canvas.pan_by(40.0, -20.0);
        canvas.zoom_by(0.5);
        assert!((canvas.zoom - 1.5).abs() < 0.001);
        let (sx, sy) = canvas.world_to_screen(100.0, 80.0);
        let (wx, wy) = canvas.screen_to_world(sx, sy);
        assert!((wx - 100.0).abs() < 0.01);
        assert!((wy - 80.0).abs() < 0.01);
        canvas.reset_view();
        assert_eq!(canvas.zoom, 1.0);
        assert_eq!((canvas.pan_x, canvas.pan_y), (0.0, 0.0));
    }
}
