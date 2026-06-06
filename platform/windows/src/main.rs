//! TermForge — entry point.
//!
//! Initialises the platform layer (logging, tokio, PTY wake event, window,
//! DX12 compositor) then hands off to `AppState::run`.

// GUI subsystem: no console window of our own.
#![windows_subsystem = "windows"]

mod app;
mod entry;
mod input;
mod ipc;
mod snapshot;
mod ui;
mod window;
mod workspace;

use std::sync::mpsc;

use anyhow::Result;
use renderer_dx12::{compositor::Compositor, context::Dx12Context};

use app::AppState;
use entry::PTY_WAKE_EVENT;
use snapshot::restore_or_default;
use ui::layout::{ChromeLayout, PANE_HEADER_H};
use window::Window;

const INIT_W: u32 = 1280;
const INIT_H: u32 = 768;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    tracing::info!("TermForge starting");

    // ── Tokio runtime ──────────────────────────────────────────────────────
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    // ── PTY wake event ─────────────────────────────────────────────────────
    let pty_wake = unsafe {
        windows::Win32::System::Threading::CreateEventW(None, false, false, None)
            .expect("CreateEventW")
    };
    PTY_WAKE_EVENT.set(pty_wake.0 as isize).ok();

    // ── Window + compositor ────────────────────────────────────────────────
    let (event_tx, event_rx) = mpsc::sync_channel::<window::WindowEvent>(512);
    let window = Window::new("TermForge", INIT_W, INIT_H, event_tx)?;
    let ctx = Dx12Context::new(window.hwnd, INIT_W, INIT_H)?;
    let compositor = Compositor::build(ctx, "Cascadia Code", 13.0, window.dpi as f32)?;
    let (cw, ch) = compositor.cell_size();

    // ── Initial session size ───────────────────────────────────────────────
    let init_content = ChromeLayout::compute(INIT_W as f32, INIT_H as f32, true).content;
    let init_cols = ((init_content.w / cw as f32) as u16).max(20);
    let init_rows = ((init_content.h - PANE_HEADER_H) / ch as f32) as u16;
    let init_rows = init_rows.max(5);

    // ── Shell detection ────────────────────────────────────────────────────
    let shell = detect_shell();

    // ── Restore or create initial sessions ────────────────────────────────
    // spawn_blocking inside ConPty::spawn requires an active Tokio runtime
    // context on the current thread.  Enter it here, drop the guard after
    // restore_or_default returns (before the rt is moved into AppState).
    let (initial_entries, initial_active) = {
        let _guard = rt.enter();
        restore_or_default(&shell, init_cols, init_rows, &rt)
    };

    // ── Launch app loop ────────────────────────────────────────────────────
    AppState::new(
        window,
        compositor,
        event_rx,
        pty_wake,
        rt,
        shell,
        init_cols,
        init_rows,
        initial_entries,
        initial_active,
        INIT_W,
        INIT_H,
    )
    .run()
}

fn detect_shell() -> String {
    use std::os::windows::process::CommandExt as _;
    // CREATE_NO_WINDOW prevents where.exe from briefly opening a console window.
    // Our process has no console (windows_subsystem = "windows"), so without this
    // flag the child would allocate its own visible console for ~100 ms.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    for candidate in ["pwsh.exe", "powershell.exe"] {
        if std::process::Command::new("where")
            .arg(candidate)
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return candidate.to_string();
        }
    }
    std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into())
}

