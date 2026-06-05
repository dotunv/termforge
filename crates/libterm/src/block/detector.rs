use crate::vt::sequences::Osc133;
use super::store::{BlockStore, CommandBlock, SessionId};

/// Detects OSC 133 prompt boundaries and drives CommandBlock lifecycle.
pub struct BlockDetector {
    session_id: SessionId,
    #[allow(dead_code)]
    current_command: Option<String>,
    pending_command_start: bool,
}

impl BlockDetector {
    pub fn new(session_id: SessionId) -> Self {
        Self {
            session_id,
            current_command: None,
            pending_command_start: false,
        }
    }

    /// Call this from the VT performer whenever an OSC 133 marker is parsed.
    pub fn handle(&mut self, marker: Osc133) {
        match marker {
            Osc133::PromptStart => {
                // new prompt shown — previous command (if any) has ended
            }
            Osc133::PromptEnd => {
                // prompt drawing finished; the next bytes before CommandStart are the typed command
                self.pending_command_start = false;
            }
            Osc133::CommandStart => {
                self.pending_command_start = true;
            }
            Osc133::CommandFinished(_) => {
                // handled by finish_block
            }
        }
    }

    /// Called by the session I/O loop with the typed command line (from shell integration).
    pub fn start_block(&mut self, command: impl Into<String>, store: &mut BlockStore) {
        let block = CommandBlock::new(self.session_id, command);
        store.push(block);
    }

    /// Called when OSC 133;D;exit_code arrives.
    pub fn finish_block(&mut self, exit_code: i32, output_since_start: Vec<u8>, store: &mut BlockStore) {
        if let Some(block) = store.last_mut() {
            if block.status == crate::block::store::BlockStatus::Running {
                block.append_output(&output_since_start);
                block.finish(exit_code);
            }
        }
    }

    pub fn session_id(&self) -> SessionId {
        self.session_id
    }
}
