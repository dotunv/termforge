//! Named-pipe IPC server: `\\.\pipe\termforge`
//!
//! Runs on a background thread.  Clients connect, send a JSON command, receive
//! a JSON response, and disconnect.  All session mutations are forwarded to the
//! main loop via an `mpsc` channel.
//!
//! ## Protocol
//! Request (newline-terminated UTF-8 JSON):
//! ```json
//! {"cmd":"list"}
//! {"cmd":"send","session":"<uuid>","text":"hello\n"}
//! {"cmd":"kill","session":"<uuid>"}
//! {"cmd":"hibernate","session":"<uuid>"}
//! {"cmd":"wake","session":"<uuid>"}
//! ```
//! Response (UTF-8 JSON + newline):
//! ```json
//! {"ok":true,"sessions":[...]}   // list
//! {"ok":true}                    // send / kill / hibernate / wake
//! {"ok":false,"error":"..."}
//! ```

use std::sync::mpsc;

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use windows::Win32::Foundation::HANDLE;

/// HANDLE is a kernel reference-counted object; safe to send across threads
/// when accessed only through documented Win32 APIs.
struct SendHandle(HANDLE);
unsafe impl Send for SendHandle {}

// ── Commands sent from IPC thread → main loop ─────────────────────────────────

#[derive(Debug)]
pub enum IpcCommand {
    Send { session: Uuid, text: String },
    Kill { session: Uuid },
    Hibernate { session: Uuid },
    Wake { session: Uuid },
}

// ── Session info sent from main loop → IPC clients ───────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub hibernated: bool,
}

// ── Shared list that main loop updates every frame ───────────────────────────

pub type SessionList = std::sync::Arc<std::sync::Mutex<Vec<SessionInfo>>>;

pub fn new_session_list() -> SessionList {
    std::sync::Arc::new(std::sync::Mutex::new(Vec::new()))
}

// ── Wire protocol helpers ─────────────────────────────────────────────────────

#[derive(Deserialize)]
struct Request {
    cmd: String,
    session: Option<String>,
    text: Option<String>,
}

#[derive(Serialize)]
struct Response {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sessions: Option<Vec<SessionInfo>>,
}

// ── Server thread ─────────────────────────────────────────────────────────────

const PIPE_NAME: &str = r"\\.\pipe\termforge";

/// Spawn the IPC server on a background thread.
/// Returns the channel the main loop should drain for incoming commands.
pub fn start(session_list: SessionList) -> mpsc::Receiver<IpcCommand> {
    let (cmd_tx, cmd_rx) = mpsc::channel::<IpcCommand>();
    std::thread::Builder::new()
        .name("termforge-ipc".into())
        .spawn(move || run_server(session_list, cmd_tx))
        .expect("failed to spawn IPC thread");
    cmd_rx
}

fn run_server(session_list: SessionList, cmd_tx: mpsc::Sender<IpcCommand>) {
    loop {
        let pipe = match create_pipe_instance() {
            Ok(h) => h,
            Err(e) => {
                tracing::error!("IPC: create pipe failed: {e}");
                std::thread::sleep(std::time::Duration::from_secs(2));
                continue;
            }
        };

        // Block until a client connects.
        if !connect_client(pipe) {
            disconnect_and_close(pipe);
            continue;
        }

        let list = session_list.clone();
        let tx = cmd_tx.clone();
        // Wrap in SendHandle so the closure is Send.
        let sh = SendHandle(pipe);
        std::thread::spawn(move || handle_client(sh, list, tx));
    }
}

fn handle_client(
    pipe: SendHandle,
    session_list: SessionList,
    cmd_tx: mpsc::Sender<IpcCommand>,
) {
    use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile};
    let pipe = pipe.0;

    let mut buf = [0u8; 4096];
    let mut read = 0u32;
    let ok = unsafe { ReadFile(pipe, Some(&mut buf), Some(&mut read), None) };
    if ok.is_err() || read == 0 {
        disconnect_and_close(pipe);
        return;
    }

    let input = String::from_utf8_lossy(&buf[..read as usize]);
    let response = process_request(input.trim(), &session_list, &cmd_tx);
    let mut resp_bytes = serde_json::to_vec(&response).unwrap_or_default();
    resp_bytes.push(b'\n');

    let mut written = 0u32;
    unsafe { let _ = WriteFile(pipe, Some(&resp_bytes), Some(&mut written), None); }
    disconnect_and_close(pipe);
}

fn process_request(
    input: &str,
    session_list: &SessionList,
    cmd_tx: &mpsc::Sender<IpcCommand>,
) -> Response {
    let req: Request = match serde_json::from_str(input) {
        Ok(r) => r,
        Err(e) => return err_response(format!("invalid JSON: {e}")),
    };

    match req.cmd.as_str() {
        "list" => {
            let sessions = session_list.lock().map(|g| g.clone()).unwrap_or_default();
            Response { ok: true, error: None, sessions: Some(sessions) }
        }
        "send" => {
            let (sid, text) = match (parse_uuid(&req.session), req.text) {
                (Some(id), Some(t)) => (id, t),
                _ => return err_response("send requires session (uuid) and text"),
            };
            let _ = cmd_tx.send(IpcCommand::Send { session: sid, text });
            ok_response()
        }
        "kill" => {
            let sid = match parse_uuid(&req.session) {
                Some(id) => id,
                None => return err_response("kill requires session (uuid)"),
            };
            let _ = cmd_tx.send(IpcCommand::Kill { session: sid });
            ok_response()
        }
        "hibernate" => {
            let sid = match parse_uuid(&req.session) {
                Some(id) => id,
                None => return err_response("hibernate requires session (uuid)"),
            };
            let _ = cmd_tx.send(IpcCommand::Hibernate { session: sid });
            ok_response()
        }
        "wake" => {
            let sid = match parse_uuid(&req.session) {
                Some(id) => id,
                None => return err_response("wake requires session (uuid)"),
            };
            let _ = cmd_tx.send(IpcCommand::Wake { session: sid });
            ok_response()
        }
        other => err_response(format!("unknown command: {other}")),
    }
}

// ── Win32 pipe helpers ────────────────────────────────────────────────────────

fn create_pipe_instance() -> anyhow::Result<windows::Win32::Foundation::HANDLE> {
    use windows::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
    use windows::Win32::System::Pipes::{
        CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
    };

    let name: Vec<u16> = PIPE_NAME
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let handle = unsafe {
        CreateNamedPipeW(
            windows::core::PCWSTR(name.as_ptr()),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            255, // max instances
            4096,
            4096,
            0,
            None,
        )
    };

    if handle.is_invalid() {
        anyhow::bail!("CreateNamedPipeW failed");
    }
    Ok(handle)
}

fn connect_client(pipe: windows::Win32::Foundation::HANDLE) -> bool {
    use windows::Win32::System::Pipes::ConnectNamedPipe;
    unsafe { ConnectNamedPipe(pipe, None).is_ok() }
}

fn disconnect_and_close(pipe: windows::Win32::Foundation::HANDLE) {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Pipes::DisconnectNamedPipe;
    unsafe {
        let _ = DisconnectNamedPipe(pipe);
        let _ = CloseHandle(pipe);
    }
}

// ── Small helpers ─────────────────────────────────────────────────────────────

fn parse_uuid(s: &Option<String>) -> Option<Uuid> {
    s.as_ref()?.parse().ok()
}

fn ok_response() -> Response {
    Response { ok: true, error: None, sessions: None }
}

fn err_response(msg: impl Into<String>) -> Response {
    Response { ok: false, error: Some(msg.into()), sessions: None }
}
