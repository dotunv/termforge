//! IPC protocol shared by `termforge` (UI), `tf` (CLI) and `forged`
//! (session host).
//!
//! All message types live here so every side is compiled against the same
//! definitions. Framing is a 4-byte little-endian length followed by a
//! `postcard` payload, written with a single `write_all` so frames can never
//! interleave when callers serialise writes per connection.
//!
//! Compatibility rule: [`PROTOCOL_VERSION`] is bumped on any breaking change.
//! The server rejects clients whose version does not match during
//! [`ClientMsg::Hello`].

mod frame;

pub use frame::{read_frame, write_frame, FrameError, MAX_FRAME_LEN};

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Bumped on any incompatible change to the types in this crate.
pub const PROTOCOL_VERSION: u32 = 7;

/// Stable identifier for a PTY session owned by `forged`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub uuid::Uuid);

impl SessionId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(pub uuid::Uuid);

impl TaskId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::str::FromStr for TaskId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Correlates a [`Request`] with its [`ServerMsg::Response`].
pub type RequestId = u64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMsg {
    /// Must be the first message on every connection.
    Hello {
        version: u32,
        token: String,
        client: String,
    },
    Request {
        id: RequestId,
        request: Request,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Request {
    Ping,
    ListSessions,
    ListTasks {
        project_root: PathBuf,
    },
    CreateTask(CreateTask),
    UpdateTask(UpdateTask),
    DeleteTask {
        id: TaskId,
    },
    CreateSession(CreateSession),
    Write {
        session: SessionId,
        data: Vec<u8>,
    },
    Resize {
        session: SessionId,
        size: TermSize,
    },
    Close {
        session: SessionId,
    },
    /// Start streaming [`Event`]s for a session. The server first replays
    /// the scrollback so late subscribers see the full history.
    Subscribe {
        session: SessionId,
    },
    Unsubscribe {
        session: SessionId,
    },
    /// Persist the UI's opaque workspace layout document for a project.
    SaveWorkspaceState {
        project_root: PathBuf,
        state: String,
    },
    LoadWorkspaceState {
        project_root: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateSession {
    /// Profile name from config; `None` selects the default shell.
    pub profile: Option<String>,
    /// OpenSSH host alias. When set, `profile` is ignored and the system SSH
    /// client is launched without TermForge handling credentials.
    pub ssh_host: Option<String>,
    pub cwd: Option<PathBuf>,
    /// Canonical root of the project this session belongs to.
    pub project_root: Option<PathBuf>,
    pub task: Option<TaskId>,
    pub size: TermSize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TermSize {
    pub cols: u16,
    pub rows: u16,
}

impl Default for TermSize {
    fn default() -> Self {
        Self {
            cols: 120,
            rows: 30,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerMsg {
    Welcome {
        version: u32,
        server: String,
    },
    Response {
        id: RequestId,
        result: Result<Response, ProtoError>,
    },
    Event(Event),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Response {
    Pong,
    Ok,
    Sessions(Vec<SessionInfo>),
    Created(SessionId),
    Tasks(Vec<TaskInfo>),
    TaskCreated(TaskId),
    /// The stored layout document, if one was saved for the project.
    WorkspaceState(Option<String>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateTask {
    pub project_root: PathBuf,
    pub title: String,
    /// A concise durable summary of intent, constraints and decisions.
    pub context: String,
    /// A single-line shell command the UI can run for this task.
    pub command: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateTask {
    pub id: TaskId,
    pub state: Option<TaskState>,
    pub title: Option<String>,
    pub context: Option<String>,
    /// `None` leaves the command unchanged; `Some("")` clears it.
    pub command: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskState {
    Planned,
    Active,
    Blocked,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskInfo {
    pub id: TaskId,
    pub project_root: PathBuf,
    pub title: String,
    pub context: String,
    pub command: Option<String>,
    pub state: TaskState,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: SessionId,
    pub title: String,
    pub shell: String,
    pub ssh_host: Option<String>,
    pub cwd: Option<WorkingDirectory>,
    /// Canonical project identity, shared by the UI, CLI and session host.
    pub project_root: Option<PathBuf>,
    pub task: Option<TaskId>,
    pub size: TermSize,
    pub running: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Event {
    /// Raw PTY bytes. Kept as bytes so UTF-8 sequences split across reads
    /// are never corrupted in transit.
    Output { session: SessionId, data: Vec<u8> },
    Exited {
        session: SessionId,
        exit_code: Option<u32>,
    },
    CwdChanged {
        session: SessionId,
        cwd: WorkingDirectory,
    },
    Notify {
        session: SessionId,
        title: Option<String>,
        body: String,
    },
    /// Authoritative current OSC 7501 state, sent after scrollback replay
    /// when a client subscribes.
    ProgramStatusSnapshot {
        session: SessionId,
        records: Vec<ProgramStatus>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkingDirectory {
    pub host: Option<String>,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgramStatus {
    pub state: ProgramState,
    pub id: Option<String>,
    pub kind: Option<BlockedKind>,
    pub progress: Option<u8>,
    pub app: Option<String>,
    pub title: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProgramState {
    Idle,
    Working,
    Done,
    Blocked,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockedKind {
    Permission,
    Question,
    Auth,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum ProtoError {
    #[error("protocol version mismatch: server {server}, client {client}")]
    VersionMismatch { server: u32, client: u32 },
    #[error("authentication failed")]
    Unauthorized,
    #[error("session not found: {0}")]
    SessionNotFound(SessionId),
    #[error("{0}")]
    Internal(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_every_message_kind() {
        let s = SessionId::new();
        let msgs = vec![
            ServerMsg::Welcome {
                version: PROTOCOL_VERSION,
                server: "forged".into(),
            },
            ServerMsg::Response {
                id: 7,
                result: Ok(Response::Created(s)),
            },
            ServerMsg::Response {
                id: 8,
                result: Err(ProtoError::SessionNotFound(s)),
            },
            ServerMsg::Response {
                id: 9,
                result: Ok(Response::WorkspaceState(Some("layout".into()))),
            },
            ServerMsg::Event(Event::Output {
                session: s,
                data: vec![0xe2, 0x94],
            }),
            ServerMsg::Event(Event::Exited {
                session: s,
                exit_code: Some(1),
            }),
            ServerMsg::Event(Event::CwdChanged {
                session: s,
                cwd: WorkingDirectory {
                    host: Some("devbox".into()),
                    path: "/srv/project".into(),
                },
            }),
            ServerMsg::Event(Event::ProgramStatusSnapshot {
                session: s,
                records: vec![ProgramStatus {
                    state: ProgramState::Blocked,
                    id: Some("deploy/eu".into()),
                    kind: Some(BlockedKind::Permission),
                    progress: Some(50),
                    app: Some("deploy".into()),
                    title: None,
                    message: Some("Approve?".into()),
                }],
            }),
        ];
        for m in msgs {
            let mut buf = Vec::new();
            write_frame(&mut buf, &m).unwrap();
            let back: ServerMsg = read_frame(&mut buf.as_slice()).unwrap().unwrap();
            assert_eq!(back, m);
        }
    }
}
