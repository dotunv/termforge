//! Shared helpers for the real-shell integration tests.
//!
//! These spawn an actual PTY running an actual shell with the actual
//! integration script, so they are slower and more fragile than unit tests.
//! Two rules keep them honest:
//!
//! * a shell that isn't installed skips, so the suite runs anywhere; and
//! * a shell that *is* installed but has no integration script fails, because
//!   that silently yields a terminal with no command blocks.
//!
//! The ordering assertion these exist for lives in [`drive`], which keeps
//! exactly one command in flight. That is slower than pipelining, but it
//! makes "command N's marks all arrived before command N+1 started"
//! observable, which is the property ConPTY is suspected of breaking.

#![allow(dead_code)] // each test binary uses a different subset

use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tf_engine::GridSize;
use tf_pty::{ShellKind, ShellProfile};
use tf_session::{Block, BlockState, LiveSession, SpawnOptions};

/// A live session plus the temp dirs it needs to stay alive for the test's
/// duration: dropping them mid-run would break the shell's cwd and `$HOME`.
pub struct RealShell {
    pub session: LiveSession,
    pub profile: ShellProfile,
    pub kind: ShellKind,
    /// Fires when the reader thread saw output, so loops can block on
    /// progress instead of spinning.
    pub wake: Receiver<()>,
    _dir: tempfile::TempDir,
    _home: tempfile::TempDir,
}

/// Why a shell could not be exercised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    /// The shell isn't installed on this machine.
    NotInstalled,
    /// Installed, but `tf-shell` cannot inject integration into it.
    NoIntegration,
}

impl std::fmt::Display for Skip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Skip::NotInstalled => "not installed",
            Skip::NoIntegration => "no integration script",
        })
    }
}

impl RealShell {
    /// Spawn `kind` with the integration script injected, or say why not.
    pub fn spawn(kind: ShellKind) -> Result<Self, Skip> {
        Self::spawn_with(kind, GridSize::new(100, 30))
    }

    pub fn spawn_with(kind: ShellKind, size: GridSize) -> Result<Self, Skip> {
        let Some(profile) = tf_pty::discover_shells()
            .into_iter()
            .find(|s| s.kind == kind)
        else {
            return Err(Skip::NotInstalled);
        };

        let dir = tempfile::tempdir().expect("tempdir");
        let home = tempfile::tempdir().expect("tempdir");
        let integration = dir.path().join("shell");

        // Checked before spawning: a shell we can launch but not instrument
        // would produce no blocks and a baffling timeout later.
        if tf_shell::inject(&profile, &integration).is_none() {
            return Err(Skip::NoIntegration);
        }

        let (tx, rx) = mpsc::channel::<()>();
        let tx = Mutex::new(tx);
        let session = LiveSession::spawn(
            SpawnOptions {
                profile: profile.clone(),
                cwd: Some(dir.path().to_path_buf()),
                size,
                env: vec![
                    ("HOME".into(), home.path().to_string_lossy().into_owned()),
                    ("PS1".into(), "$ ".into()),
                ],
                integration_dir: Some(integration),
            },
            Arc::new(move || {
                let _ = tx.lock().expect("waker lock").send(());
            }),
        )
        .expect("spawn session");

        Ok(Self {
            session,
            profile,
            kind,
            wake: rx,
            _dir: dir,
            _home: home,
        })
    }

    /// How many commands have finished, including any since trimmed from the
    /// scrollback. Monotonic, so it is the reliable progress signal.
    pub fn finished(&self) -> usize {
        self.session.with(|p| p.blocks().finished())
    }

    /// The most recent finished command block, if it is still retained.
    pub fn last_command(&self) -> Option<Block> {
        self.session.with(|p| p.blocks().last_command().cloned())
    }

    /// Command blocks that finished and are still retained, oldest first.
    pub fn finished_commands(&self) -> Vec<Block> {
        self.session.with(|p| {
            p.blocks()
                .commands()
                .filter(|b| b.state == BlockState::Finished)
                .cloned()
                .collect()
        })
    }

    /// `true` when the shell sits at a prompt, ready for input.
    pub fn at_prompt(&self) -> bool {
        self.session.with(|p| {
            p.blocks()
                .iter()
                .last()
                .is_some_and(|b| b.state == BlockState::Prompt && b.input_line.is_some())
        })
    }

    pub fn send(&self, line: &str) {
        self.session
            .write(format!("{line}\r").as_bytes())
            .expect("write to pty");
    }

    /// Wait up to `dur` for session activity. `false` means nothing arrived.
    pub fn wait(&self, dur: Duration) -> bool {
        self.wake.recv_timeout(dur).is_ok()
    }

    /// Ask the shell to exit, so the child ends before the test returns.
    pub fn quit(&self) {
        let _ = self.session.write(b"exit\r");
    }

    pub fn screen(&self) -> String {
        self.session.snapshot().text()
    }

    pub fn blocks_debug(&self) -> String {
        self.session.with(|p| format!("{:?}", p.blocks()))
    }
}

/// Progress on a driven run, for a readable timeout message.
pub fn diagnostics(s: &RealShell, sent: usize, total: usize) -> String {
    format!(
        "{name}: {sent}/{total} commands, {finished} finished\n\
         blocks: {blocks}\n--- screen ---\n{screen}",
        name = s.profile.name,
        finished = s.finished(),
        blocks = s.blocks_debug(),
        screen = s.screen(),
    )
}

/// Run `count` commands through the shell, keeping one in flight, and call
/// `check` with each finished block and its 0-based index.
///
/// `next` supplies the command text for step `n`. The contract is that the
/// shell's exit status for it must equal [`expected_code`].
///
/// Returns how long the run took.
pub fn drive(
    shell: &RealShell,
    count: usize,
    budget: Duration,
    next: impl Fn(usize) -> String,
    check: impl Fn(&Block, usize) -> Result<(), String>,
) -> Result<Duration, String> {
    let started = Instant::now();
    let mut sent = 0usize;
    while sent < count {
        if Instant::now() >= started + budget {
            return Err(diagnostics(shell, sent, count));
        }
        // `finished()` is monotonic, so this is exact even after trimming.
        if shell.finished() > sent {
            let b = shell.last_command().ok_or_else(|| {
                format!(
                    "{}: a command finished but no block was retained",
                    shell.profile.name
                )
            })?;
            check(&b, sent).map_err(|e| format!("{e}\n--- screen ---\n{}", shell.screen()))?;
            sent += 1;
            continue;
        }
        // Only type once the previous command has been accounted for and the
        // shell is back at a prompt, otherwise keystrokes land in the output
        // of a command still running.
        if shell.finished() == sent && shell.at_prompt() {
            shell.send(&next(sent));
            let until = Instant::now() + Duration::from_secs(30);
            while shell.finished() == sent && Instant::now() < until {
                if !shell.wait(Duration::from_millis(50)) {
                    continue;
                }
            }
        } else {
            let _ = shell.wait(Duration::from_millis(50));
        }
    }
    Ok(started.elapsed())
}

/// Shells this platform can meaningfully test.
///
/// `Cmd`, `zsh` and `fish` are excluded because `tf-shell` cannot inject into
/// them, so there is nothing to assert. WSL is discovered and launchable by
/// `tf-pty`, but injecting into a Linux shell from a Windows host still needs
/// a path translation step that does not exist, so it is excluded here too
/// rather than quietly reporting success. See
/// `tf_pty::ShellProfile::supports_integration`.
pub fn testable_shells() -> Vec<ShellKind> {
    let mut kinds = vec![
        ShellKind::Bash,
        ShellKind::Pwsh,
        ShellKind::WindowsPowerShell,
    ];
    if cfg!(windows) {
        kinds.push(ShellKind::GitBash);
    }
    kinds
}
