//! A running session: PTY + reader thread + [`Processor`].

use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use tf_engine::{AlacrittyEngine, GridSize, Snapshot, TerminalEngine};
use tf_pty::{Pty, PtyDims, ShellProfile};

use crate::{Processor, SessionEvent};

/// Called from background threads whenever there is new output or the
/// session exits. Must be cheap and must not block.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

#[derive(Debug, Clone)]
pub struct SpawnOptions {
    pub profile: ShellProfile,
    pub cwd: Option<PathBuf>,
    pub size: GridSize,
    pub env: Vec<(String, String)>,
    /// Directory holding the shell-integration scripts. When set, supported
    /// shells are launched with integration injected.
    pub integration_dir: Option<PathBuf>,
}

struct Shared {
    processor: Mutex<Processor<AlacrittyEngine>>,
    /// `None` once the child has exited and the PTY has been closed.
    pty: Mutex<Option<Pty>>,
    events: Mutex<Vec<SessionEvent>>,
    exit: Mutex<Option<Option<u32>>>,
    closing: AtomicBool,
    wake: Waker,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A live terminal session. Cheap to query from the UI thread: the
/// processor lock is only held for the duration of one PTY read.
pub struct LiveSession {
    shared: Arc<Shared>,
    profile: ShellProfile,
}

impl std::fmt::Debug for LiveSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveSession")
            .field("shell", &self.profile.name)
            .finish_non_exhaustive()
    }
}

impl LiveSession {
    pub fn spawn(opts: SpawnOptions, wake: Waker) -> Result<Self> {
        let profile = match &opts.integration_dir {
            Some(dir) => {
                tf_shell::install(dir).context("install shell integration")?;
                tf_shell::inject(&opts.profile, dir).unwrap_or_else(|| opts.profile.clone())
            }
            None => opts.profile.clone(),
        };
        let dims = PtyDims {
            cols: opts.size.cols,
            rows: opts.size.rows,
        };
        let (pty, reader) = Pty::spawn(&profile, opts.cwd.as_deref(), dims, &opts.env)?;

        let shared = Arc::new(Shared {
            processor: Mutex::new(Processor::new(AlacrittyEngine::new(opts.size))),
            pty: Mutex::new(Some(pty)),
            events: Mutex::new(Vec::new()),
            exit: Mutex::new(None),
            closing: AtomicBool::new(false),
            wake,
        });

        let s = Arc::clone(&shared);
        thread::Builder::new()
            .name("tf-pty-reader".into())
            .spawn(move || read_loop(&s, reader))
            .context("spawn reader thread")?;

        // ConPTY does not signal EOF when the child exits; the pseudoconsole
        // must be closed first. Poll for exit and close it ourselves.
        let s = Arc::clone(&shared);
        thread::Builder::new()
            .name("tf-pty-waiter".into())
            .spawn(move || wait_loop(&s))
            .context("spawn waiter thread")?;

        Ok(Self {
            shared,
            profile: opts.profile,
        })
    }

    pub fn profile(&self) -> &ShellProfile {
        &self.profile
    }

    /// Write input to the shell. A no-op after exit.
    pub fn write(&self, bytes: &[u8]) -> Result<()> {
        if let Some(pty) = lock(&self.shared.pty).as_mut() {
            pty.write_all(bytes)?;
        }
        Ok(())
    }

    pub fn resize(&self, size: GridSize) -> Result<()> {
        {
            let mut p = lock(&self.shared.processor);
            if p.engine().size() == size {
                return Ok(());
            }
            p.engine_mut().resize(size);
        }
        if let Some(pty) = lock(&self.shared.pty).as_ref() {
            pty.resize(PtyDims {
                cols: size.cols,
                rows: size.rows,
            })?;
        }
        Ok(())
    }

    pub fn snapshot(&self) -> Snapshot {
        lock(&self.shared.processor).engine().snapshot()
    }

    /// Run `f` with the processor locked. Keep `f` short.
    pub fn with<R>(&self, f: impl FnOnce(&Processor<AlacrittyEngine>) -> R) -> R {
        f(&lock(&self.shared.processor))
    }

    pub fn with_mut<R>(&self, f: impl FnOnce(&mut Processor<AlacrittyEngine>) -> R) -> R {
        f(&mut lock(&self.shared.processor))
    }

    /// Drain events produced since the last call.
    pub fn take_events(&self) -> Vec<SessionEvent> {
        std::mem::take(&mut *lock(&self.shared.events))
    }

    /// `Some(exit_code)` once the shell has exited.
    pub fn exit_status(&self) -> Option<Option<u32>> {
        *lock(&self.shared.exit)
    }
}

impl Drop for LiveSession {
    fn drop(&mut self) {
        self.shared.closing.store(true, Ordering::SeqCst);
        // Dropping the PTY kills the child and closes the pseudoconsole,
        // which unblocks the reader thread.
        drop(lock(&self.shared.pty).take());
    }
}

fn read_loop(shared: &Shared, mut reader: Box<dyn Read + Send>) {
    let mut buf = vec![0u8; 64 * 1024];
    let mut events = Vec::new();
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let reply = lock(&shared.processor).process(&buf[..n], &mut events);
        if !reply.is_empty() {
            if let Some(pty) = lock(&shared.pty).as_mut() {
                let _ = pty.write_all(&reply);
            }
        }
        if !events.is_empty() {
            lock(&shared.events).append(&mut events);
        }
        (shared.wake)();
    }
    tracing::debug!("pty reader finished");
    (shared.wake)();
}

fn wait_loop(shared: &Shared) {
    loop {
        if shared.closing.load(Ordering::SeqCst) {
            return;
        }
        let status = match lock(&shared.pty).as_mut() {
            Some(pty) => pty.try_wait().ok().flatten(),
            None => return,
        };
        if let Some(code) = status {
            // Give the reader a moment to drain the final output.
            thread::sleep(Duration::from_millis(50));
            *lock(&shared.exit) = Some(Some(code));
            drop(lock(&shared.pty).take());
            (shared.wake)();
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
}
