use libterm::mux::session::Session;

/// Single-GPU compositor — all sessions share one DX12 device.
/// Full implementation in Phase 2.
pub struct Compositor;

impl Compositor {
    pub fn new() -> Self { Self }

    /// Called once per frame. Draws only dirty sessions, then presents once.
    pub fn render_frame(&mut self, _sessions: &mut [Session]) {
        // Phase 2: iterate sessions, check dirty flag, issue draw calls, present.
    }
}

impl Default for Compositor {
    fn default() -> Self { Self::new() }
}
