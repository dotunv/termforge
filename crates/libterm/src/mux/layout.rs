use uuid::Uuid;

pub type PaneId = Uuid;

/// A binary split tree for free-form pane layout.
#[derive(Debug, Clone)]
pub enum PaneLayout {
    /// A leaf: a single pane containing a session.
    Leaf { id: PaneId, session_id: Uuid },
    /// A horizontal split (left | right), with ratio in 0.0..1.0.
    HSplit { left: Box<PaneLayout>, right: Box<PaneLayout>, ratio: f32 },
    /// A vertical split (top / bottom), with ratio in 0.0..1.0.
    VSplit { top: Box<PaneLayout>, bottom: Box<PaneLayout>, ratio: f32 },
}

impl PaneLayout {
    pub fn leaf(session_id: Uuid) -> Self {
        Self::Leaf { id: Uuid::new_v4(), session_id }
    }

    pub fn hsplit(left: PaneLayout, right: PaneLayout, ratio: f32) -> Self {
        Self::HSplit { left: Box::new(left), right: Box::new(right), ratio }
    }

    pub fn vsplit(top: PaneLayout, bottom: PaneLayout, ratio: f32) -> Self {
        Self::VSplit { top: Box::new(top), bottom: Box::new(bottom), ratio }
    }

    /// Returns all session IDs referenced in the layout.
    pub fn session_ids(&self) -> Vec<Uuid> {
        match self {
            Self::Leaf { session_id, .. } => vec![*session_id],
            Self::HSplit { left, right, .. } => {
                let mut ids = left.session_ids();
                ids.extend(right.session_ids());
                ids
            }
            Self::VSplit { top, bottom, .. } => {
                let mut ids = top.session_ids();
                ids.extend(bottom.session_ids());
                ids
            }
        }
    }

    /// Compute pixel-space rects for every leaf given a root rect.
    pub fn rects(&self, x: f32, y: f32, w: f32, h: f32) -> Vec<(Uuid, f32, f32, f32, f32)> {
        match self {
            Self::Leaf { session_id, .. } => vec![(*session_id, x, y, w, h)],
            Self::HSplit { left, right, ratio, .. } => {
                let lw = w * ratio;
                let mut r = left.rects(x, y, lw, h);
                r.extend(right.rects(x + lw, y, w - lw, h));
                r
            }
            Self::VSplit { top, bottom, ratio, .. } => {
                let th = h * ratio;
                let mut r = top.rects(x, y, w, th);
                r.extend(bottom.rects(x, y + th, w, h - th));
                r
            }
        }
    }
}
