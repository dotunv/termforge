use std::collections::HashMap;
use std::time::Instant;

pub type AnimationId = u64;

// Easing curves and targets beyond the ones currently wired up are kept as
// the animation vocabulary for upcoming chrome transitions.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Easing {
    Linear,
    EaseOutQuad,
    EaseInOutCubic,
    EaseOutBack,
    Bounce,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnimationTarget {
    SidebarOffset,
    OverlayOpacity,
    SettingsScale,
    BlockSlideY(u64),
    PulseScale(u64),
    TabIndicatorX,
    HoverGlow(u64),
}

struct ActiveAnimation {
    id: AnimationId,
    target: AnimationTarget,
    start_val: f32,
    end_val: f32,
    start_time: Instant,
    duration_ms: u64,
    easing: Easing,
    on_complete: Option<Box<dyn FnOnce()>>,
}

pub struct AnimationEngine {
    next_id: u64,
    active: Vec<ActiveAnimation>,
    values: HashMap<AnimationTarget, f32>,
}

impl AnimationEngine {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            active: Vec::new(),
            values: HashMap::new(),
        }
    }

    pub fn animate(
        &mut self,
        target: AnimationTarget,
        from: f32,
        to: f32,
        duration_ms: u64,
        easing: Easing,
    ) -> AnimationId {
        self.animate_with_callback(target, from, to, duration_ms, easing, None)
    }

    pub fn animate_with_callback(
        &mut self,
        target: AnimationTarget,
        from: f32,
        to: f32,
        duration_ms: u64,
        easing: Easing,
        on_complete: Option<Box<dyn FnOnce()>>,
    ) -> AnimationId {
        self.cancel_target(&target);
        let id = self.next_id;
        self.next_id += 1;
        self.values.insert(target, from);
        self.active.push(ActiveAnimation {
            id,
            target,
            start_val: from,
            end_val: to,
            start_time: Instant::now(),
            duration_ms,
            easing,
            on_complete,
        });
        id
    }

    pub fn tick(&mut self) {
        let now = Instant::now();
        let mut completed = Vec::new();
        for anim in &mut self.active {
            let elapsed = now.duration_since(anim.start_time);
            let t = (elapsed.as_millis() as f64 / anim.duration_ms as f64).min(1.0) as f32;
            let eased = Self::ease(anim.easing, t);
            let value = anim.start_val + (anim.end_val - anim.start_val) * eased;
            self.values.insert(anim.target, value);
            if t >= 1.0 {
                self.values.insert(anim.target, anim.end_val);
                completed.push(anim.id);
            }
        }
        for id in &completed {
            if let Some(pos) = self.active.iter().position(|a| a.id == *id) {
                let mut anim = self.active.remove(pos);
                if let Some(cb) = anim.on_complete.take() {
                    cb();
                }
            }
        }
    }

    /// Set a target's resting value directly, without animating.
    pub fn set_value(&mut self, target: AnimationTarget, value: f32) {
        self.cancel_target(&target);
        self.values.insert(target, value);
    }

    pub fn value(&self, target: &AnimationTarget) -> f32 {
        self.values.get(target).copied().unwrap_or(0.0)
    }

    #[allow(dead_code)]
    pub fn cancel(&mut self, id: AnimationId) {
        self.active.retain(|a| a.id != id);
    }

    pub fn cancel_target(&mut self, target: &AnimationTarget) {
        self.active.retain(|a| a.target != *target);
    }

    /// True while any animation is in flight (render loop should stay dirty).
    pub fn is_busy(&self) -> bool {
        !self.active.is_empty()
    }

    #[allow(dead_code)]
    pub fn is_animating(&self, target: &AnimationTarget) -> bool {
        self.active.iter().any(|a| a.target == *target)
    }

    fn ease(easing: Easing, t: f32) -> f32 {
        match easing {
            Easing::Linear => t,
            Easing::EaseOutQuad => t * (2.0 - t),
            Easing::EaseInOutCubic => {
                if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 }
            }
            Easing::EaseOutBack => {
                let c1 = 1.70158;
                let c3 = c1 + 1.0;
                1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
            }
            Easing::Bounce => {
                let n1 = 7.5625;
                let d1 = 2.75;
                if t < 1.0 / d1 { n1 * t * t }
                else if t < 2.0 / d1 { let t = t - 1.5 / d1; n1 * t * t + 0.75 }
                else if t < 2.5 / d1 { let t = t - 2.25 / d1; n1 * t * t + 0.9375 }
                else { let t = t - 2.625 / d1; n1 * t * t + 0.984375 }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_linear_easing() {
        assert!((AnimationEngine::ease(Easing::Linear, 0.0) - 0.0).abs() < 0.001);
        assert!((AnimationEngine::ease(Easing::Linear, 0.5) - 0.5).abs() < 0.001);
        assert!((AnimationEngine::ease(Easing::Linear, 1.0) - 1.0).abs() < 0.001);
    }
    #[test]
    fn test_ease_out_quad() {
        assert!((AnimationEngine::ease(Easing::EaseOutQuad, 0.0) - 0.0).abs() < 0.001);
        assert!((AnimationEngine::ease(Easing::EaseOutQuad, 1.0) - 1.0).abs() < 0.001);
        assert!(AnimationEngine::ease(Easing::EaseOutQuad, 0.5) > 0.5);
    }
}
