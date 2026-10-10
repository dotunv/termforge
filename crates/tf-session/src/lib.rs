//! A terminal session ties together a PTY, the OSC [`tf_tap::Tap`], a
//! [`tf_engine::TerminalEngine`] and the [`BlockIndex`].
//!
//! Blocks are *virtual*: the session keeps one real grid and records the
//! absolute line at which each OSC 133 mark arrived. The UI draws block
//! chrome over those ranges. See `docs/adr/0004-virtual-blocks.md`.

mod blocks;
mod live;
mod program_status;

pub use blocks::{Block, BlockIndex, BlockState};
pub use live::{LiveSession, SpawnOptions, Waker};
pub use program_status::{ProgramStatusIndex, ProgramStatusRecord};
pub use tf_tap::{BlockedKind, ProgramState, ProgramStatusReport, WorkingDirectory};

use tf_engine::TerminalEngine;
use tf_tap::{Located, Tap, TapEvent};

/// Side effects produced by [`Processor::process`] for the owner to act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    /// Raw PTY bytes, preserved exactly so a remote renderer can feed the
    /// same stream into its terminal engine without corrupting split UTF-8.
    Output(Vec<u8>),
    Cwd(WorkingDirectory),
    Notify {
        title: Option<String>,
        body: String,
    },
    BlockFinished {
        /// Position in [`BlockIndex::iter`] *at the moment the block finished*.
        /// Positions shift as the scrollback trims old blocks and as the index
        /// reaches its retention bound, so treat this as a hint for "the block
        /// that just completed" rather than a durable key.
        index: usize,
        exit_code: Option<i32>,
    },
    ProgramStatusChanged,
}

/// Pure, PTY-free processing pipeline. Owners feed it PTY output; it keeps
/// the emulator, the tap and the block index consistent with each other.
pub struct Processor<E: TerminalEngine> {
    engine: E,
    tap: Tap,
    blocks: BlockIndex,
    cwd: Option<WorkingDirectory>,
    scratch: Vec<Located>,
    history: usize,
    program_status: ProgramStatusIndex,
}

impl<E: TerminalEngine> std::fmt::Debug for Processor<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Processor")
            .field("blocks", &self.blocks.len())
            .field("cwd", &self.cwd)
            .finish_non_exhaustive()
    }
}

impl<E: TerminalEngine> Processor<E> {
    pub fn new(engine: E) -> Self {
        Self {
            engine,
            tap: Tap::new(),
            blocks: BlockIndex::default(),
            cwd: None,
            scratch: Vec::new(),
            history: 0,
            program_status: ProgramStatusIndex::default(),
        }
    }

    /// Process one chunk of PTY output.
    ///
    /// The chunk is fed to the engine in segments split at each recognised
    /// sequence, so every mark is anchored to the cursor line at the exact
    /// moment it arrived. Returns bytes that must be written back to the PTY
    /// (terminal replies) and pushes session events into `events`.
    pub fn process(&mut self, chunk: &[u8], events: &mut Vec<SessionEvent>) -> Vec<u8> {
        self.scratch.clear();
        self.tap.feed(chunk, &mut self.scratch);
        let mut start = 0;
        let mut replies = Vec::new();
        for located in std::mem::take(&mut self.scratch) {
            self.feed_engine(&chunk[start..located.offset]);
            start = located.offset;
            if matches!(located.event, TapEvent::ProgramStatusQuery) {
                replies.extend_from_slice(b"\x1b]7501;?\x1b\\");
            }
            self.apply(located.event, events);
        }
        self.feed_engine(&chunk[start..]);
        replies.extend(self.engine.take_replies());
        replies
    }

    fn feed_engine(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.engine.feed(bytes);
        // The alternate screen has its own (empty) history; anchors refer to
        // the primary screen, so ignore history changes while it is active.
        if self.engine.modes().alt_screen {
            return;
        }
        // Keep block anchors valid when scrollback is cleared (`clear`,
        // `ESC [3J`). Lines trimmed at the scrollback cap are not yet
        // detected; see docs/adr/0004-virtual-blocks.md.
        let history = self.engine.history_size();
        if history < self.history {
            self.blocks.remove_top_lines(self.history - history);
        }
        self.history = history;
    }

    fn apply(&mut self, event: TapEvent, events: &mut Vec<SessionEvent>) {
        match event {
            TapEvent::FullReset => {
                if self.program_status.clear() {
                    events.push(SessionEvent::ProgramStatusChanged);
                }
            }
            TapEvent::Mark(mark) => {
                if matches!(mark, tf_tap::Mark::PromptStart)
                    && self.program_status.clear_transient()
                {
                    events.push(SessionEvent::ProgramStatusChanged);
                }
                let line = self.engine.cursor_line_abs();
                let cwd = self.cwd.as_ref().map(|cwd| cwd.path.clone());
                if let Some((index, exit_code)) = self.blocks.apply(mark, line, cwd) {
                    events.push(SessionEvent::BlockFinished { index, exit_code });
                }
            }
            TapEvent::Cwd(cwd) => {
                self.cwd = Some(cwd.clone());
                events.push(SessionEvent::Cwd(cwd));
            }
            TapEvent::Notify { title, body } => events.push(SessionEvent::Notify { title, body }),
            TapEvent::Progress { .. } => {}
            TapEvent::ProgramStatus(report) => {
                self.program_status.apply(report);
                events.push(SessionEvent::ProgramStatusChanged);
            }
            TapEvent::ProgramStatusQuery => {}
        }
    }

    pub fn engine(&self) -> &E {
        &self.engine
    }

    pub fn engine_mut(&mut self) -> &mut E {
        &mut self.engine
    }

    pub fn blocks(&self) -> &BlockIndex {
        &self.blocks
    }

    pub fn cwd(&self) -> Option<&WorkingDirectory> {
        self.cwd.as_ref()
    }

    pub fn program_status(&self) -> &ProgramStatusIndex {
        &self.program_status
    }

    pub fn replace_program_status(
        &mut self,
        reports: impl IntoIterator<Item = ProgramStatusReport>,
    ) {
        self.program_status.replace(reports);
    }

    pub fn process_exited(&mut self) -> bool {
        self.program_status.clear_transient()
    }
}
