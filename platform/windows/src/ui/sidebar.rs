// Phase 3 sidebar — rendering handled via chrome.rs + ui_renderer.
#![allow(dead_code)]
pub struct Sidebar {
    pub visible: bool,
}
impl Sidebar {
    pub fn new() -> Self {
        Self { visible: true }
    }
}
impl Default for Sidebar {
    fn default() -> Self {
        Self::new()
    }
}
