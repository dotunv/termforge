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
pub const PROTOCOL_VERSION: u32 = 1;

/// Stable identifier for a PTY session owned by `forged`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub uuid::Uuid);

impl SessionId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateSession {
    /// Profile name from config; `None` selects the default shell.
    pub profile: Option<String>,
    pub cwd: Option<PathBuf>,
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: SessionId,
    pub title: String,
    pub shell: String,
    pub cwd: Option<PathBuf>,
    pub size: TermSize,
    pub running: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Event {
    /// Raw PTY bytes. Kept as bytes so UTF-8 sequences split across reads
    /// are never corrupted in transit.
    Output {
        session: SessionId,
        data: Vec<u8>,
    },
    Exited {
        session: SessionId,
        exit_code: Option<u32>,
    },
    CwdChanged {
        session: SessionId,
        cwd: String,
    },
    Notify {
        session: SessionId,
        title: Option<String>,
        body: String,
    },
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
            ServerMsg::Event(Event::Output {
                session: s,
                data: vec![0xe2, 0x94],
            }),
            ServerMsg::Event(Event::Exited {
                session: s,
                exit_code: Some(1),
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
