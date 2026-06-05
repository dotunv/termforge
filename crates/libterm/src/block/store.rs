use std::time::Instant;
use uuid::Uuid;

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
    pub agent_id: String,
    pub task: String,
    pub tool_calls: Vec<String>,
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
        }
    }

    pub fn finish(&mut self, exit_code: i32) {
        self.exit_code = Some(exit_code);
        self.finished_at = Some(Instant::now());
        self.status = if exit_code == 0 { BlockStatus::Success } else { BlockStatus::Error };
    }

    pub fn append_output(&mut self, bytes: &[u8]) {
        self.output.extend_from_slice(bytes);
    }

    pub fn output_as_str(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.output)
    }

    pub fn duration_ms(&self) -> Option<u128> {
        self.finished_at.map(|f| f.duration_since(self.started_at).as_millis())
    }
}

/// Queryable store of all command blocks for a session.
#[derive(Default)]
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
        self.blocks.iter().filter(|b| b.status == BlockStatus::Running)
    }
}
