//! A terminal session ties together a PTY, the OSC [`tf_tap::Tap`], a
//! [`tf_engine::TerminalEngine`] and the [`BlockIndex`].
//!
//! Blocks are *virtual*: the session keeps one real grid and records the
//! absolute line at which each OSC 133 mark arrived. The UI draws block
//! chrome over those ranges. See `docs/adr/0004-virtual-blocks.md`.

mod blocks;
mod live;

pub use blocks::{Block, BlockIndex, BlockState};
pub use live::{LiveSession, SpawnOptions, Waker};

use tf_engine::TerminalEngine;
use tf_tap::{Located, Tap, TapEvent};

/// Side effects produced by [`Processor::process`] for the owner to act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    Cwd(String),
    Notify {
        title: Option<String>,
        body: String,
    },
    BlockFinished {
        index: usize,
        exit_code: Option<i32>,
    },
}

/// Pure, PTY-free processing pipeline. Owners feed it PTY output; it keeps
/// the emulator, the tap and the block index consistent with each other.
pub struct Processor<E: TerminalEngine> {
    engine: E,
    tap: Tap,
    blocks: BlockIndex,
    cwd: Option<String>,
    scratch: Vec<Located>,
    history: usize,
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
        for located in std::mem::take(&mut self.scratch) {
            self.feed_engine(&chunk[start..located.offset]);
            start = located.offset;
            self.apply(located.event, events);
        }
        self.feed_engine(&chunk[start..]);
        self.engine.take_replies()
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
            TapEvent::Mark(mark) => {
                let line = self.engine.cursor_line_abs();
                if let Some((index, exit_code)) = self.blocks.apply(mark, line, self.cwd.clone()) {
                    events.push(SessionEvent::BlockFinished { index, exit_code });
                }
            }
            TapEvent::Cwd(cwd) => {
                self.cwd = Some(cwd.clone());
                events.push(SessionEvent::Cwd(cwd));
            }
            TapEvent::Notify { title, body } => events.push(SessionEvent::Notify { title, body }),
            TapEvent::Progress { .. } => {}
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

    pub fn cwd(&self) -> Option<&str> {
        self.cwd.as_deref()
    }
}
