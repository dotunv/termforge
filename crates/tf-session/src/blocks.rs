use tf_tap::Mark;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockState {
    /// Prompt drawn, user is typing.
    Prompt,
    /// Command submitted, output streaming.
    Running,
    Finished,
}

/// One command, expressed as absolute line anchors in the session's grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub prompt_line: usize,
    pub input_line: Option<usize>,
    pub output_line: Option<usize>,
    pub end_line: Option<usize>,
    pub exit_code: Option<i32>,
    pub cwd: Option<String>,
    pub state: BlockState,
}

/// Most blocks retained at once.
///
/// Bounding this is not a memory optimisation, it is a correctness one.
/// [`BlockIndex::apply`] inspects every retained block when a prompt arrives,
/// so cost per prompt is linear in this number. Nothing else trims the index
/// once the scrollback reaches its cap, because `history_size()` stops growing
/// and the caller can no longer tell that lines were dropped: without a bound
/// a session left open for a day degrades quadratically and leaks.
const DEFAULT_MAX_BLOCKS: usize = 5_000;

/// Builds blocks from the OSC 133 mark stream.
///
/// The detector is tolerant of shells that skip marks: a new `A` closes any
/// open block, and a `D` without a preceding `C` (for example an empty
/// Enter) does not create a block.
#[derive(Debug)]
pub struct BlockIndex {
    blocks: Vec<Block>,
    /// Lifetime count of commands that have finished. Unlike `blocks.len()`
    /// this never decreases when the scrollback trims old blocks, so it is
    /// what a long-running consumer should compare against to know how much
    /// work has gone past.
    finished: usize,
    max_blocks: usize,
}

impl Default for BlockIndex {
    fn default() -> Self {
        Self {
            blocks: Vec::new(),
            finished: 0,
            max_blocks: DEFAULT_MAX_BLOCKS,
        }
    }
}

impl BlockIndex {
    /// Retain at most `max_blocks` blocks. Lower values bound the per-prompt
    /// cost in [`BlockIndex::apply`]; blocks older than the bound are dropped,
    /// so their output stays in the scrollback but their metadata does not.
    pub fn with_max_blocks(max_blocks: usize) -> Self {
        Self {
            max_blocks: max_blocks.max(1),
            ..Self::default()
        }
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    /// Total commands that have finished since the session started, including
    /// those whose blocks have since been trimmed from the scrollback.
    pub fn finished_commands(&self) -> usize {
        self.finished
    }

    /// The most recent command block that has finished, if any. Cheap: the
    /// newest blocks are at the end. Returns `None` if the block has since
    /// been trimmed, so callers that must not miss a block should compare
    /// against [`BlockIndex::finished`] instead.
    pub fn last_command(&self) -> Option<&Block> {
        self.blocks
            .iter()
            .rev()
            .find(|b| b.output_line.is_some() && b.state == BlockState::Finished)
    }

    /// A finished command block by its position in [`BlockIndex::iter`].
    ///
    /// Positions shift when the scrollback trims, so this is only meaningful
    /// for the recent tail of the session.
    pub fn command_at(&self, i: usize) -> Option<&Block> {
        self.blocks
            .iter()
            .filter(|b| b.output_line.is_some() && b.state == BlockState::Finished)
            .nth(i)
    }

    /// Number of command blocks currently retained that have finished.
    pub fn finished(&self) -> usize {
        self.blocks
            .iter()
            .filter(|b| b.output_line.is_some() && b.state == BlockState::Finished)
            .count()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn get(&self, i: usize) -> Option<&Block> {
        self.blocks.get(i)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Block> {
        self.blocks.iter()
    }

    /// Blocks that finished running (commands with output).
    pub fn commands(&self) -> impl Iterator<Item = &Block> {
        self.blocks.iter().filter(|b| b.output_line.is_some())
    }

    /// Apply a mark observed at absolute `line`. Returns
    /// `(index, exit_code)` when a command block finishes.
    pub fn apply(
        &mut self,
        mark: Mark,
        line: usize,
        cwd: Option<String>,
    ) -> Option<(usize, Option<i32>)> {
        match mark {
            Mark::PromptStart => {
                self.close_open(line);
                // Blocks must be ordered by line. A prompt at or above an
                // existing block means the screen was cleared or rewritten
                // in place (`clear`, Ctrl+L, a TUI that exited), so those
                // blocks no longer describe what is on screen.
                self.blocks.retain(|b| b.prompt_line < line);
                for b in &mut self.blocks {
                    if b.end_line.is_some_and(|e| e > line) {
                        b.end_line = Some(line);
                    }
                }
                self.blocks.push(Block {
                    prompt_line: line,
                    input_line: None,
                    output_line: None,
                    end_line: None,
                    exit_code: None,
                    cwd,
                    state: BlockState::Prompt,
                });
                // Keep the index bounded: `apply` is linear in `blocks`, and
                // nothing else trims it once the scrollback caps.
                if self.blocks.len() > self.max_blocks {
                    let excess = self.blocks.len() - self.max_blocks;
                    self.blocks.drain(..excess);
                }
                None
            }
            Mark::InputStart => {
                if let Some(b) = self.current_mut(BlockState::Prompt) {
                    b.input_line = Some(line);
                }
                None
            }
            Mark::CommandExecuted => {
                if let Some(b) = self.current_mut(BlockState::Prompt) {
                    b.output_line = Some(line);
                    b.state = BlockState::Running;
                }
                None
            }
            Mark::CommandFinished { exit_code } => {
                let idx = self.blocks.len().checked_sub(1)?;
                let b = &mut self.blocks[idx];
                if b.state != BlockState::Running {
                    return None;
                }
                if line < b.prompt_line {
                    // The command cleared the screen; nothing to anchor to.
                    self.blocks.pop();
                    return None;
                }
                b.exit_code = exit_code;
                b.end_line = Some(line);
                b.state = BlockState::Finished;
                self.finished += 1;
                Some((idx, exit_code))
            }
        }
    }

    /// `lines` lines were removed from the top of the scrollback. Shift all
    /// anchors up and drop blocks that no longer exist.
    pub fn remove_top_lines(&mut self, lines: usize) {
        if lines == 0 {
            return;
        }
        self.blocks.retain_mut(|b| {
            let last = b.end_line.unwrap_or(usize::MAX);
            if last < lines {
                return false;
            }
            let shift = |l: &mut usize| *l = l.saturating_sub(lines);
            shift(&mut b.prompt_line);
            b.input_line.as_mut().map(shift);
            b.output_line.as_mut().map(shift);
            b.end_line.as_mut().map(shift);
            true
        });
    }

    fn current_mut(&mut self, state: BlockState) -> Option<&mut Block> {
        self.blocks.last_mut().filter(|b| b.state == state)
    }

    fn close_open(&mut self, line: usize) {
        if let Some(b) = self.blocks.last_mut() {
            if b.state == BlockState::Running {
                b.end_line = Some(line);
                b.state = BlockState::Finished;
            } else if b.state == BlockState::Prompt && b.output_line.is_none() {
                // Prompt abandoned (Ctrl+C, empty Enter): drop it.
                self.blocks.pop();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_lifecycle() {
        let mut idx = BlockIndex::default();
        assert_eq!(idx.apply(Mark::PromptStart, 0, None), None);
        idx.apply(Mark::InputStart, 0, None);
        idx.apply(Mark::CommandExecuted, 1, None);
        assert_eq!(
            idx.apply(Mark::CommandFinished { exit_code: Some(0) }, 4, None),
            Some((0, Some(0)))
        );
        let b = idx.get(0).unwrap();
        assert_eq!(
            (b.prompt_line, b.output_line, b.end_line),
            (0, Some(1), Some(4))
        );
        assert_eq!(b.state, BlockState::Finished);
    }

    #[test]
    fn empty_enter_does_not_create_blocks() {
        let mut idx = BlockIndex::default();
        idx.apply(Mark::PromptStart, 0, None);
        idx.apply(Mark::InputStart, 0, None);
        assert_eq!(
            idx.apply(Mark::CommandFinished { exit_code: Some(0) }, 1, None),
            None
        );
        idx.apply(Mark::PromptStart, 1, None);
        assert_eq!(idx.len(), 1);
        assert_eq!(idx.commands().count(), 0);
    }

    #[test]
    fn removing_history_shifts_and_prunes() {
        let mut idx = BlockIndex::default();
        for (start, end) in [(0, 3), (4, 8), (9, 12)] {
            idx.apply(Mark::PromptStart, start, None);
            idx.apply(Mark::CommandExecuted, start + 1, None);
            idx.apply(Mark::CommandFinished { exit_code: Some(0) }, end, None);
        }
        idx.apply(Mark::PromptStart, 13, None);
        idx.remove_top_lines(6);
        assert_eq!(idx.len(), 3, "first block fully removed");
        let b = idx.get(0).unwrap();
        assert_eq!(
            (b.prompt_line, b.end_line),
            (0, Some(2)),
            "partially removed block is clipped"
        );
        assert_eq!(idx.get(2).unwrap().prompt_line, 7);
    }

    #[test]
    fn prompt_above_existing_blocks_drops_them() {
        let mut idx = BlockIndex::default();
        idx.apply(Mark::PromptStart, 10, None);
        idx.apply(Mark::CommandExecuted, 11, None);
        idx.apply(Mark::CommandFinished { exit_code: Some(0) }, 14, None);
        idx.apply(Mark::PromptStart, 14, None);
        idx.apply(Mark::CommandExecuted, 15, None);
        // `clear` moves the cursor home before D.
        assert_eq!(
            idx.apply(Mark::CommandFinished { exit_code: Some(0) }, 0, None),
            None
        );
        idx.apply(Mark::PromptStart, 0, None);
        assert_eq!(idx.len(), 1);
        assert_eq!(idx.commands().count(), 0);
    }

    #[test]
    fn block_count_stays_bounded() {
        // Regression: unbounded growth made `apply` quadratic, because a
        // prompt scans every retained block and nothing trims the index once
        // the scrollback caps.
        let mut idx = BlockIndex::with_max_blocks(64);
        for i in 0..5_000usize {
            let p = i * 4;
            idx.apply(Mark::PromptStart, p, None);
            idx.apply(Mark::InputStart, p, None);
            idx.apply(Mark::CommandExecuted, p + 1, None);
            idx.apply(Mark::CommandFinished { exit_code: Some(0) }, p + 2, None);
            assert!(idx.len() <= 64, "grew past the bound at {i}: {}", idx.len());
        }
        assert_eq!(idx.len(), 64);
        // The lifetime count is unaffected by the bound.
        assert_eq!(idx.finished_commands(), 5_000);
        // The newest block is always the one kept.
        let last = idx.last_command().expect("newest retained");
        assert_eq!(last.prompt_line, 4 * 4_999);
    }

    #[test]
    fn default_bound_is_finite() {
        let mut idx = BlockIndex::default();
        for i in 0..(DEFAULT_MAX_BLOCKS + 500) {
            let p = i * 4;
            idx.apply(Mark::PromptStart, p, None);
            idx.apply(Mark::InputStart, p, None);
            idx.apply(Mark::CommandExecuted, p + 1, None);
            idx.apply(Mark::CommandFinished { exit_code: Some(0) }, p + 2, None);
        }
        assert_eq!(idx.len(), DEFAULT_MAX_BLOCKS);
        assert_eq!(idx.finished_commands(), DEFAULT_MAX_BLOCKS + 500);
    }

    #[test]
    fn finished_count_survives_pruning() {
        let mut idx = BlockIndex::default();
        for start in [0, 4, 8] {
            idx.apply(Mark::PromptStart, start, None);
            idx.apply(Mark::CommandExecuted, start + 1, None);
            idx.apply(
                Mark::CommandFinished { exit_code: Some(0) },
                start + 3,
                None,
            );
        }
        assert_eq!(idx.finished_commands(), 3);
        assert_eq!(idx.last_command().unwrap().prompt_line, 8);
        // Scrollback trims the first two blocks; the counter must not go back.
        idx.remove_top_lines(8);
        assert_eq!(idx.len(), 1);
        assert_eq!(
            idx.finished_commands(),
            3,
            "pruning must not rewind the count"
        );
        assert_eq!(idx.last_command().unwrap().prompt_line, 0);
    }

    #[test]
    fn last_command_ignores_empty_prompts() {
        let mut idx = BlockIndex::default();
        idx.apply(Mark::PromptStart, 0, None);
        idx.apply(Mark::InputStart, 0, None);
        idx.apply(Mark::CommandExecuted, 1, None);
        idx.apply(Mark::CommandFinished { exit_code: Some(0) }, 2, None);
        // A fresh prompt with nothing typed is not a command.
        idx.apply(Mark::PromptStart, 3, None);
        idx.apply(Mark::InputStart, 3, None);
        assert_eq!(idx.last_command().unwrap().prompt_line, 0);
    }

    #[test]
    fn missing_d_is_closed_by_next_prompt() {
        let mut idx = BlockIndex::default();
        idx.apply(Mark::PromptStart, 0, None);
        idx.apply(Mark::CommandExecuted, 1, None);
        idx.apply(Mark::PromptStart, 9, None);
        assert_eq!(idx.get(0).unwrap().end_line, Some(9));
        assert_eq!(idx.get(0).unwrap().exit_code, None);
    }
}
