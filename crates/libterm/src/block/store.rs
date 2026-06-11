use std::cell::Cell;
use std::time::Instant;
use uuid::Uuid;

use crate::grid::cell::Color;
use crate::vt::sequences::OscNotification;

pub type BlockId = Uuid;
pub type SessionId = Uuid;

/// A run of same-coloured characters within a styled output line.
#[derive(Debug, Clone)]
pub struct StyledRun {
    pub text: String,
    pub fg: Color,
    pub bg: Color,
}

/// One line of styled output (a sequence of coloured runs).
pub type StyledLine = Vec<StyledRun>;

/// A single styled cell in the in-progress line (column-addressable so that
/// carriage returns overwrite rather than erase — needed for CRLF line endings
/// and progress bars).
#[derive(Debug, Clone, Copy)]
struct StyledCell {
    ch: char,
    fg: Color,
    bg: Color,
}

/// Collapse a row of cells into merged colour runs, trimming trailing blanks.
fn cells_to_runs(cells: &[StyledCell]) -> StyledLine {
    let mut end = cells.len();
    while end > 0 {
        let c = &cells[end - 1];
        if c.ch == ' ' && c.fg == Color::Default && c.bg == Color::Default {
            end -= 1;
        } else {
            break;
        }
    }
    let mut runs: StyledLine = Vec::new();
    for cell in &cells[..end] {
        if let Some(last) = runs.last_mut() {
            if last.fg == cell.fg && last.bg == cell.bg {
                last.text.push(cell.ch);
                continue;
            }
        }
        runs.push(StyledRun { text: cell.ch.to_string(), fg: cell.fg, bg: cell.bg });
    }
    runs
}

#[derive(Debug, Clone)]
pub struct CommandBlock {
    pub id: BlockId,
    pub session_id: SessionId,
    pub command: String,
    pub output: Vec<u8>,
    /// Coloured output, captured line-by-line from the grid pen state.  This is
    /// what the block view renders; `output` is kept as the plain-text form for
    /// heuristics / copy.
    pub styled: Vec<StyledLine>,
    /// In-progress (not yet newline-terminated) line, addressed by column so a
    /// carriage return overwrites instead of erasing.
    cur_cells: Vec<StyledCell>,
    cur_col: usize,
    /// Working directory (OSC 7) captured when the command started.
    pub cwd: Option<String>,
    pub exit_code: Option<i32>,
    pub started_at: Instant,
    pub finished_at: Option<Instant>,
    pub status: BlockStatus,
    pub kind: BlockKind,
    pub agent_ctx: Option<AgentContext>,
    /// Agent-pushed notification state (from OSC 9001).
    pub notification: Option<OscNotification>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BlockStatus {
    Running,
    Success,
    Error,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BlockKind {
    Command,
    Agent,
    System,
}

#[derive(Debug, Clone)]
pub struct AgentContext {
    /// Human-readable model tag, e.g. "claude-opus-4-5" or "gpt-4o".
    pub model: String,
    /// High-level task description (first prompt / system label).
    pub task: String,
    /// Ordered list of tool names called so far.
    pub tool_calls: Vec<String>,
    /// Total input tokens consumed.
    pub tokens_in: u32,
    /// Total output tokens produced.
    pub tokens_out: u32,
    /// Estimated cost in USD (0.0 when unknown).
    pub cost_usd: f32,
}

impl AgentContext {
    pub fn new(model: impl Into<String>, task: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            task: task.into(),
            tool_calls: Vec::new(),
            tokens_in: 0,
            tokens_out: 0,
            cost_usd: 0.0,
        }
    }

    pub fn add_tool_call(&mut self, name: impl Into<String>) {
        self.tool_calls.push(name.into());
    }

    /// Returns a compact cost string, or empty string if cost is unknown.
    pub fn cost_display(&self) -> String {
        if self.cost_usd > 0.0 {
            format!("${:.4}", self.cost_usd)
        } else {
            String::new()
        }
    }
}

impl CommandBlock {
    pub fn new(session_id: SessionId, command: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            session_id,
            command: command.into(),
            output: Vec::new(),
            styled: Vec::new(),
            cur_cells: Vec::new(),
            cur_col: 0,
            cwd: None,
            exit_code: None,
            started_at: Instant::now(),
            finished_at: None,
            status: BlockStatus::Running,
            kind: BlockKind::Command,
            agent_ctx: None,
            notification: None,
        }
    }

    pub fn set_notification(&mut self, n: OscNotification) {
        self.notification = Some(n);
    }

    pub fn finish(&mut self, exit_code: i32) {
        self.exit_code = Some(exit_code);
        self.finished_at = Some(Instant::now());
        self.status = if exit_code == 0 {
            BlockStatus::Success
        } else {
            BlockStatus::Error
        };
    }

    pub fn append_output(&mut self, bytes: &[u8]) {
        self.output.extend_from_slice(bytes);
    }

    /// Write one printable character at the current column with its pen colour
    /// (overwriting any cell already there), then advance the column.
    pub fn push_styled(&mut self, c: char, fg: Color, bg: Color) {
        let cell = StyledCell { ch: c, fg, bg };
        if self.cur_col < self.cur_cells.len() {
            self.cur_cells[self.cur_col] = cell;
        } else {
            while self.cur_cells.len() < self.cur_col {
                self.cur_cells.push(StyledCell { ch: ' ', fg: Color::Default, bg: Color::Default });
            }
            self.cur_cells.push(cell);
        }
        self.cur_col += 1;
    }

    /// Newline — commit the in-progress line as merged colour runs.
    pub fn styled_newline(&mut self) {
        let line = cells_to_runs(&self.cur_cells);
        self.styled.push(line);
        self.cur_cells.clear();
        self.cur_col = 0;
    }

    /// Carriage return — move the write column to 0 *without* erasing, so the
    /// existing text stays unless subsequent output overwrites it (correct for
    /// CRLF endings and progress bars).
    pub fn styled_carriage_return(&mut self) {
        self.cur_col = 0;
    }

    /// The in-progress line as runs (empty when nothing has been written since
    /// the last newline).
    pub fn current_runs(&self) -> StyledLine {
        cells_to_runs(&self.cur_cells)
    }

    pub fn output_as_str(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.output)
    }

    pub fn duration_ms(&self) -> Option<u128> {
        self.finished_at
            .map(|f| f.duration_since(self.started_at).as_millis())
    }
}

/// Queryable store of all command blocks for a session.
#[derive(Clone)]
pub struct BlockStore {
    blocks: Vec<CommandBlock>,
    /// Monotonically increasing counter bumped on every push and on every
    /// handout of a `&mut CommandBlock` (the caller *may* mutate through it,
    /// so we invalidate conservatively). Render caches snapshot this to skip
    /// re-cloning unchanged data.
    change_counter: Cell<u64>,
}

impl Default for BlockStore {
    fn default() -> Self {
        Self {
            blocks: Vec::new(),
            change_counter: Cell::new(0),
        }
    }
}

impl BlockStore {
    /// Returns the current change counter. Render caches can snapshot this
    /// and compare on subsequent frames to avoid redundant work.
    pub fn change_counter(&self) -> u64 {
        self.change_counter.get()
    }

    pub fn push(&mut self, block: CommandBlock) {
        self.blocks.push(block);
        self.change_counter.set(self.change_counter.get() + 1);
    }

    pub fn get_mut(&mut self, id: BlockId) -> Option<&mut CommandBlock> {
        let r = self.blocks.iter_mut().find(|b| b.id == id);
        if r.is_some() {
            self.change_counter.set(self.change_counter.get() + 1);
        }
        r
    }

    pub fn last_mut(&mut self) -> Option<&mut CommandBlock> {
        let r = self.blocks.last_mut();
        if r.is_some() {
            self.change_counter.set(self.change_counter.get() + 1);
        }
        r
    }

    pub fn all(&self) -> &[CommandBlock] {
        &self.blocks
    }

    pub fn running(&self) -> impl Iterator<Item = &CommandBlock> {
        self.blocks
            .iter()
            .filter(|b| b.status == BlockStatus::Running)
    }

    /// Clone the most recent `n` blocks (O(n), safe to call per-frame for small n).
    pub fn clone_recent(&self, n: usize) -> Vec<CommandBlock> {
        let start = self.blocks.len().saturating_sub(n);
        self.blocks[start..].to_vec()
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}
