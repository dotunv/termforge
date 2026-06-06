//! Per-session bundle: PTY handle, VT parser, and libterm session state.
//!
//! `Entry` is the single unit the app loop works with.  It owns one child
//! process (ConPTY, SSH, or agent), the VT parser that processes its output,
//! and the [`Session`] snapshot that the renderer reads.

use anyhow::Result;
use libterm::{
    block::detector::BlockDetector,
    mux::session::{Session, SessionKind},
    pty::{conpty::ConPty, Pty},
    ssh::{
        client::{SshAuth, SshClient},
        host_store::HostConfig,
    },
    vt::VtParser,
};
use tokio::sync::mpsc::UnboundedReceiver;
use uuid::Uuid;

/// Raw Win32 HANDLE value (as isize) for the PTY-data wake event.
/// Set once at startup; read by all ConPty reader tasks to signal the main loop.
pub static PTY_WAKE_EVENT: std::sync::OnceLock<isize> = std::sync::OnceLock::new();

// ── Entry ─────────────────────────────────────────────────────────────────────

pub struct Entry {
    pub session: Session,
    pub vt_parser: VtParser,
    pub pty: Box<dyn Pty + Send>,
    pub pty_rx: UnboundedReceiver<Vec<u8>>,
    /// Tracks when this entry last received PTY output or user input.
    pub last_activity: std::time::Instant,
    /// True when the child process has been suspended to save CPU.
    pub hibernated: bool,
    /// Last PTY dimensions sent to the child; avoids redundant ResizePseudoConsole
    /// calls that cause the shell to reprint the prompt on every frame.
    pub last_cols: u16,
    pub last_rows: u16,
    /// True until the first PTY byte is received.  While true the UI shows
    /// the "new session" welcome overlay with keyboard shortcut hints.
    pub fresh: bool,
}

impl Entry {
    // ── Constructors ──────────────────────────────────────────────────────────

    /// Spawn a local ConPTY shell session.
    pub fn spawn_local(shell: &str, cols: u16, rows: u16) -> Result<Self> {
        let wake = *PTY_WAKE_EVENT.get().unwrap_or(&0);
        let sid = Uuid::new_v4();
        let session = Session::new(SessionKind::Local, cols, rows);
        let detector = BlockDetector::new(sid);
        let vt_parser = VtParser::new(cols, rows, detector);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let pty = ConPty::spawn(shell, cols, rows, tx, wake)?;
        Ok(Self::new(session, vt_parser, Box::new(pty), rx, cols, rows))
    }

    /// Spawn a local ConPTY session tagged as an Agent (purple in UI).
    pub fn spawn_agent(command: &str, model: &str) -> Result<Self> {
        let wake = *PTY_WAKE_EVENT.get().unwrap_or(&0);
        let (cols, rows) = (220u16, 50u16);
        let sid = Uuid::new_v4();
        let name = std::path::Path::new(command)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(command)
            .to_string();
        let kind = SessionKind::Agent { name, model: model.to_string() };
        let mut session = Session::new(kind, cols, rows);
        session.title = command.to_string();
        let detector = BlockDetector::new(sid);
        let vt_parser = VtParser::new(cols, rows, detector);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let pty = ConPty::spawn(command, cols, rows, tx, wake)?;
        Ok(Self::new(session, vt_parser, Box::new(pty), rx, cols, rows))
    }

    /// Connect an SSH session (async — awaited inside the tokio runtime).
    pub async fn spawn_ssh(
        hostname: String,
        port: u16,
        username: String,
        auth: SshAuth,
    ) -> Result<Self> {
        let (cols, rows) = (220u16, 50u16);
        let sid = Uuid::new_v4();
        let kind = SessionKind::Ssh { host: hostname.clone(), user: username.clone() };
        let mut session = Session::new(kind, cols, rows);
        session.title = format!("{username}@{hostname}");
        let detector = BlockDetector::new(sid);
        let vt_parser = VtParser::new(cols, rows, detector);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let pty = SshClient::connect(&hostname, port, &username, auth, cols, rows, tx).await?;
        Ok(Self::new(session, vt_parser, Box::new(pty), rx, cols, rows))
    }

    fn new(
        session: Session,
        vt_parser: VtParser,
        pty: Box<dyn Pty + Send>,
        pty_rx: UnboundedReceiver<Vec<u8>>,
        cols: u16,
        rows: u16,
    ) -> Self {
        Self {
            session,
            vt_parser,
            pty,
            pty_rx,
            last_activity: std::time::Instant::now(),
            hibernated: false,
            last_cols: cols,
            last_rows: rows,
            fresh: true,
        }
    }

    // ── Lifecycle ─────────────────────────────────────────────────────────────

    /// Drain all pending PTY bytes into the VT parser.  Returns true if any
    /// data was processed (i.e. the session is now dirty for rendering).
    pub fn drain_pty(&mut self) -> bool {
        if self.hibernated {
            return false;
        }
        let mut dirty = false;
        while let Ok(bytes) = self.pty_rx.try_recv() {
            self.vt_parser.process(&bytes);
            dirty = true;
        }
        if dirty {
            self.fresh = false; // first PTY output — dismiss welcome overlay
            self.last_activity = std::time::Instant::now();
            self.session.grid = self.vt_parser.grid().clone_grid();
            self.session.blocks = self.vt_parser.blocks().clone();
            self.session.mark_dirty();
        }
        dirty
    }

    /// Suspend the child process to save CPU when not visible.
    pub fn hibernate(&mut self) {
        if !self.hibernated {
            self.pty.suspend();
            self.hibernated = true;
            self.session.mark_dirty();
        }
    }

    /// Resume a hibernated session.
    pub fn wake(&mut self) {
        if self.hibernated {
            self.pty.resume();
            self.hibernated = false;
            self.last_activity = std::time::Instant::now();
            self.session.mark_dirty();
        }
    }

    /// Record user input and wake from hibernation if needed.
    /// Also clears the `fresh` flag so the welcome overlay is dismissed.
    pub fn record_input(&mut self) {
        self.fresh = false; // user has typed — dismiss the new-session welcome card
        self.last_activity = std::time::Instant::now();
        self.wake();
    }

    /// Resize the PTY only when the computed cell dimensions actually changed,
    /// preventing the shell from reprinting its prompt on every frame.
    pub fn resize_pty(&mut self, pane_w: f32, pane_h: f32, cw: u32, ch: u32) {
        let new_cols = ((pane_w / cw as f32) as u16).max(10);
        let new_rows = ((pane_h / ch as f32) as u16).max(3);
        if new_cols != self.last_cols || new_rows != self.last_rows {
            let _ = self.pty.resize(new_cols, new_rows);
            self.last_cols = new_cols;
            self.last_rows = new_rows;
        }
    }
}

// ── Auth helper ───────────────────────────────────────────────────────────────

/// Determine the SSH authentication method from a stored host config.
pub fn ssh_auth_for(host: &HostConfig) -> SshAuth {
    if let Some(ref kp) = host.key_path {
        SshAuth::PrivateKey {
            key_path: std::path::PathBuf::from(kp),
            passphrase: None,
        }
    } else {
        SshAuth::Agent
    }
}
