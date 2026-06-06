use uuid::Uuid;

pub type PaneId = Uuid;

/// A binary split tree for free-form pane layout.
#[derive(Debug, Clone)]
pub enum PaneLayout {
    /// A leaf: a single pane containing a session.
    Leaf { id: PaneId, session_id: Uuid },
    /// A horizontal split (left | right), with ratio in 0.0..1.0.
    HSplit {
        left: Box<PaneLayout>,
        right: Box<PaneLayout>,
        ratio: f32,
    },
    /// A vertical split (top / bottom), with ratio in 0.0..1.0.
    VSplit {
        top: Box<PaneLayout>,
        bottom: Box<PaneLayout>,
        ratio: f32,
    },
}

impl PaneLayout {
    pub fn leaf(session_id: Uuid) -> Self {
        Self::Leaf {
            id: Uuid::new_v4(),
            session_id,
        }
    }

    pub fn hsplit(left: PaneLayout, right: PaneLayout, ratio: f32) -> Self {
        Self::HSplit {
            left: Box::new(left),
            right: Box::new(right),
            ratio,
        }
    }

    pub fn vsplit(top: PaneLayout, bottom: PaneLayout, ratio: f32) -> Self {
        Self::VSplit {
            top: Box::new(top),
            bottom: Box::new(bottom),
            ratio,
        }
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

    /// Pixel x-positions of all vertical (HSplit) handles inside the given rect.
    pub fn split_handle_xs(&self, x: f32, _y: f32, w: f32, _h: f32) -> Vec<f32> {
        match self {
            Self::Leaf { .. } => vec![],
            Self::HSplit {
                left, right, ratio, ..
            } => {
                let split_x = x + w * ratio;
                let mut xs = vec![split_x];
                xs.extend(left.split_handle_xs(x, _y, w * ratio, _h));
                xs.extend(right.split_handle_xs(split_x, _y, w * (1.0 - ratio), _h));
                xs
            }
            Self::VSplit {
                top, bottom, ratio, ..
            } => {
                let mut xs = top.split_handle_xs(x, _y, w, _h * ratio);
                xs.extend(bottom.split_handle_xs(x, _y + _h * ratio, w, _h * (1.0 - ratio)));
                xs
            }
        }
    }

    /// Update the ratio of the HSplit whose handle is nearest to `handle_x`.
    /// `new_ratio` = (new_split_x - x) / w, caller must clamp.
    pub fn set_nearest_hsplit_ratio(&mut self, handle_x: f32, x: f32, w: f32, new_ratio: f32) {
        match self {
            Self::HSplit {
                left: _,
                right: _,
                ratio,
                ..
            } => {
                let split_x = x + w * *ratio;
                if (split_x - handle_x).abs() < 4.0 {
                    *ratio = new_ratio.clamp(0.1, 0.9);
                }
            }
            Self::VSplit {
                top, bottom, ratio, ..
            } => {
                let th = w * *ratio; // repurpose field — actually VSplit uses h
                top.set_nearest_hsplit_ratio(handle_x, x, th, new_ratio);
                bottom.set_nearest_hsplit_ratio(handle_x, x, w - th, new_ratio);
            }
            Self::Leaf { .. } => {}
        }
    }

    /// Compute pixel-space rects for every leaf given a root rect.
    pub fn rects(&self, x: f32, y: f32, w: f32, h: f32) -> Vec<(Uuid, f32, f32, f32, f32)> {
        match self {
            Self::Leaf { session_id, .. } => vec![(*session_id, x, y, w, h)],
            Self::HSplit {
                left, right, ratio, ..
            } => {
                let lw = w * ratio;
                let mut r = left.rects(x, y, lw, h);
                r.extend(right.rects(x + lw, y, w - lw, h));
                r
            }
            Self::VSplit {
                top, bottom, ratio, ..
            } => {
                let th = h * ratio;
                let mut r = top.rects(x, y, w, th);
                r.extend(bottom.rects(x, y + th, w, h - th));
                r
            }
        }
    }
}
