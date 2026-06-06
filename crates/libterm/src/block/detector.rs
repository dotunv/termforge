use super::store::{BlockStatus, BlockStore, CommandBlock, SessionId};
use crate::vt::sequences::{Osc133, OscNotification};

/// Actions returned by the detector for the caller to apply.
#[derive(Debug)]
pub enum BlockAction {
    /// A new command block should be started (OSC 133;C seen).
    StartBlock,
    /// The running block should be finished with the given exit code.
    FinishBlock(i32),
    /// A prompt appeared — if a running block exists with no exit code,
    /// treat it as exit 0 (fallback when 133;D is not emitted).
    PromptSeen,
    /// Update the running block's notification state (OSC 9001).
    Notification(OscNotification),
    /// No action needed.
    None,
}

/// Detects OSC 133 prompt boundaries and drives CommandBlock lifecycle.
pub struct BlockDetector {
    session_id: SessionId,
    /// Text captured between PromptEnd (B) and CommandStart (C) = the typed command.
    capture_command: bool,
    captured_chars: String,
}

impl BlockDetector {
    pub fn new(session_id: SessionId) -> Self {
        Self {
            session_id,
            capture_command: false,
            captured_chars: String::new(),
        }
    }

    /// Call from the VT performer whenever an OSC 133 marker is parsed.
    /// Returns an action the caller should apply to the BlockStore.
    pub fn handle(&mut self, marker: Osc133) -> BlockAction {
        match marker {
            Osc133::PromptStart => {
                // A new prompt appeared — the previous command has ended.
                self.capture_command = false;
                self.captured_chars.clear();
                BlockAction::PromptSeen
            }
            Osc133::PromptEnd => {
                // Prompt drawing finished — start capturing typed text.
                self.capture_command = true;
                self.captured_chars.clear();
                BlockAction::None
            }
            Osc133::CommandStart => {
                // User pressed Enter — the captured text is the command.
                self.capture_command = false;
                BlockAction::StartBlock
            }
            Osc133::CommandFinished(code) => BlockAction::FinishBlock(code),
        }
    }

    /// Feed printable characters so we can capture the typed command line.
    pub fn feed_char(&mut self, c: char) {
        if self.capture_command {
            self.captured_chars.push(c);
        }
    }

    /// Return the captured command text and clear it.
    pub fn take_command(&mut self) -> String {
        std::mem::take(&mut self.captured_chars)
    }

    /// Apply a BlockAction to a BlockStore. Convenience method.
    pub fn apply(&mut self, action: BlockAction, store: &mut BlockStore) {
        match action {
            BlockAction::StartBlock => {
                let cmd = self.take_command();
                self.start_block(cmd, store);
            }
            BlockAction::FinishBlock(code) => {
                self.finish_block(code, Vec::new(), store);
            }
            BlockAction::PromptSeen => {
                // Close any running block as success (fallback).
                if let Some(block) = store.last_mut() {
                    if block.status == BlockStatus::Running {
                        block.finish(0);
                    }
                }
            }
            BlockAction::Notification(n) => {
                if let Some(block) = store.last_mut() {
                    block.set_notification(n);
                }
            }
            BlockAction::None => {}
        }
    }

    /// Start a block directly. Kept as a small public helper for tests and for
    /// callers that synthesize command boundaries without OSC 133 markers.
    pub fn start_block(&mut self, command: impl Into<String>, store: &mut BlockStore) {
        store.push(CommandBlock::new(self.session_id, command));
    }

    /// Finish the running block directly, optionally appending final output.
    pub fn finish_block(&mut self, exit_code: i32, output: Vec<u8>, store: &mut BlockStore) {
        if let Some(block) = store.last_mut() {
            if block.status == BlockStatus::Running {
                if !output.is_empty() {
                    block.append_output(&output);
                }
                block.finish(exit_code);
            }
        }
    }

    pub fn session_id(&self) -> SessionId {
        self.session_id
    }

    pub fn is_capturing(&self) -> bool {
        self.capture_command
    }
}
