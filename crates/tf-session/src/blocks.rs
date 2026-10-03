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

/// Builds blocks from the OSC 133 mark stream.
///
/// The detector is tolerant of shells that skip marks: a new `A` closes any
/// open block, and a `D` without a preceding `C` (for example an empty
/// Enter) does not create a block.
#[derive(Debug, Default)]
pub struct BlockIndex {
    blocks: Vec<Block>,
    /// Lifetime count of commands that have finished. Unlike `blocks.len()`
    /// this never decreases when the scrollback trims old blocks, so it is
    /// what a long-running consumer should compare against to know how much
    /// work has gone past.
    finished: usize,
}

impl BlockIndex {
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
