use std::time::Instant;
use uuid::Uuid;

use crate::vt::sequences::OscNotification;

pub type BlockId = Uuid;
pub type SessionId = Uuid;

#[derive(Debug, Clone)]
pub struct CommandBlock {
    pub id: BlockId,
    pub session_id: SessionId,
    pub command: String,
    pub output: Vec<u8>,
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

    pub fn output_as_str(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.output)
    }

    pub fn duration_ms(&self) -> Option<u128> {
        self.finished_at
            .map(|f| f.duration_since(self.started_at).as_millis())
    }
}

/// Queryable store of all command blocks for a session.
#[derive(Default, Clone)]
pub struct BlockStore {
    blocks: Vec<CommandBlock>,
}

impl BlockStore {
    pub fn push(&mut self, block: CommandBlock) {
        self.blocks.push(block);
    }

    pub fn get_mut(&mut self, id: BlockId) -> Option<&mut CommandBlock> {
        self.blocks.iter_mut().find(|b| b.id == id)
    }

    pub fn last_mut(&mut self) -> Option<&mut CommandBlock> {
        self.blocks.last_mut()
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
