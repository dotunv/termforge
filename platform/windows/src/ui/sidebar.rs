// Phase 3: collapsible sidebar with workspace/session tree.
pub struct Sidebar { pub visible: bool }
impl Sidebar { pub fn new() -> Self { Self { visible: true } } }
impl Default for Sidebar { fn default() -> Self { Self::new() } }
