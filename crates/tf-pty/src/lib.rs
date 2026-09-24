//! PTY process management.
//!
//! On Windows, `portable-pty` prefers a sideloaded `conpty.dll` (with
//! `OpenConsole.exe`) placed next to the executable and falls back to the
//! in-box ConPTY in `kernel32.dll`. `cargo xtask conpty` fetches the pinned
//! Microsoft package into `assets/conpty/`; release bundles ship both files.
//! See `docs/adr/0003-pty-and-conpty.md`.

mod shells;

pub use shells::{discover_shells, ShellKind, ShellProfile};

use std::io::{Read, Write};
use std::path::Path;

use anyhow::{Context, Result};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

/// Terminal size in cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtyDims {
    pub cols: u16,
    pub rows: u16,
}

impl From<PtyDims> for PtySize {
    fn from(d: PtyDims) -> Self {
        PtySize {
            rows: d.rows.max(1),
            cols: d.cols.max(2),
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

/// A running shell attached to a pseudoterminal.
pub struct Pty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

impl std::fmt::Debug for Pty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pty")
            .field("pid", &self.child.process_id())
            .finish_non_exhaustive()
    }
}

/// The read half of a PTY. Read it on a dedicated thread; reads block.
pub type PtyReader = Box<dyn Read + Send>;

impl Pty {
    /// Spawn `profile` in `cwd`. Returns the PTY and its output reader.
    pub fn spawn(
        profile: &ShellProfile,
        cwd: Option<&Path>,
        dims: PtyDims,
        env: &[(String, String)],
    ) -> Result<(Self, PtyReader)> {
        let pair = native_pty_system()
            .openpty(dims.into())
            .context("open pty")?;

        let mut cmd = CommandBuilder::new(&profile.program);
        cmd.args(&profile.args);
        if let Some(dir) = cwd {
            cmd.cwd(dir);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "TermForge");
        cmd.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));
        for (k, v) in env {
            cmd.env(k, v);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("spawn {} ({})", profile.name, profile.program.display()))?;
        // The slave handle must be dropped in this process, otherwise reads
        // never observe EOF after the child exits.
        drop(pair.slave);

        let reader = pair.master.try_clone_reader().context("clone pty reader")?;
        let writer = pair.master.take_writer().context("take pty writer")?;
        tracing::debug!(shell = %profile.name, pid = ?child.process_id(), "spawned pty");
        Ok((
            Self {
                master: pair.master,
                writer,
                child,
            },
            reader,
        ))
    }

    pub fn write_all(&mut self, bytes: &[u8]) -> Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn resize(&self, dims: PtyDims) -> Result<()> {
        self.master.resize(dims.into()).context("resize pty")
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.process_id()
    }

    /// Non-blocking exit check. `Some(code)` once the child has exited.
    pub fn try_wait(&mut self) -> Result<Option<u32>> {
        Ok(self.child.try_wait()?.map(|s| s.exit_code()))
    }

    /// Terminate the child and reap it so it never lingers as a zombie.
    pub fn kill(&mut self) -> Result<()> {
        if self.child.try_wait()?.is_none() {
            self.child.kill().context("kill child")?;
        }
        let _ = self.child.wait();
        Ok(())
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        if let Err(e) = self.kill() {
            tracing::warn!("failed to kill pty child: {e:#}");
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    #[test]
    fn spawn_echo_and_exit() {
        let profile = ShellProfile {
            name: "sh".into(),
            kind: ShellKind::Other,
            program: PathBuf::from("/bin/sh"),
            args: vec!["-c".into(), "printf tf-ok; exit 3".into()],
        };
        let (mut pty, mut reader) =
            Pty::spawn(&profile, None, PtyDims { cols: 80, rows: 24 }, &[]).unwrap();
        let handle = std::thread::spawn(move || {
            let mut out = Vec::new();
            let _ = reader.read_to_end(&mut out);
            out
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        let code = loop {
            if let Some(code) = pty.try_wait().unwrap() {
                break code;
            }
            assert!(Instant::now() < deadline, "child did not exit");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(code, 3);
        drop(pty);
        let out = handle.join().unwrap();
        assert!(String::from_utf8_lossy(&out).contains("tf-ok"));
    }
}
