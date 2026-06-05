mod input;
mod ui;
mod window;

use std::sync::mpsc;

use anyhow::Result;
use libterm::{
    block::detector::BlockDetector,
    mux::{
        layout::PaneLayout,
        session::{Session, SessionKind},
    },
    pty::conpty::ConPty,
    pty::Pty,
    vt::VtParser,
};
use renderer_dx12::{compositor::Compositor, context::Dx12Context};

use input::{char_to_pty_bytes, vk_to_pty_bytes};
use window::{Window, WindowEvent};

/// Initial window dimensions.
const INIT_W: u32 = 1200;
const INIT_H: u32 = 720;
/// Terminal grid size.
const TERM_COLS: u16 = 220;
const TERM_ROWS: u16 = 50;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    tracing::info!("TermForge starting — Phase 2 (Win32 + DX12)");

    // ── Window event channel ───────────────────────────────────────────────
    let (event_tx, event_rx) = mpsc::sync_channel::<WindowEvent>(256);

    // ── Win32 window ───────────────────────────────────────────────────────
    let window = Window::new("TermForge", INIT_W, INIT_H, event_tx)?;

    // ── DX12 context + compositor ──────────────────────────────────────────
    let ctx = Dx12Context::new(window.hwnd, INIT_W, INIT_H)?;
    let mut compositor = Compositor::build(ctx, "Cascadia Code", 13.0)?;

    // ── ConPTY ─────────────────────────────────────────────────────────────
    let (pty_tx, pty_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();

    let shell = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into());
    let mut pty = ConPty::spawn(&shell, TERM_COLS, TERM_ROWS, pty_tx)?;

    // ── Session + VT parser ────────────────────────────────────────────────
    let session_id = uuid::Uuid::new_v4();
    let detector = BlockDetector::new(session_id);
    let mut vt_parser = VtParser::new(TERM_COLS, TERM_ROWS, detector);
    let mut session = Session::new(SessionKind::Local, TERM_COLS, TERM_ROWS);

    // Single-pane layout fills the whole window.
    let layout = PaneLayout::leaf(session.id);

    // ── Tokio runtime for async channel recv ──────────────────────────────
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut pty_rx = rt.block_on(async { pty_rx });

    let mut window_w = INIT_W;
    let mut window_h = INIT_H;
    let mut running = true;

    // ── Main loop ──────────────────────────────────────────────────────────
    while running {
        // Drain PTY output into the VT parser.
        while let Ok(bytes) = pty_rx.try_recv() {
            vt_parser.process(&bytes);
            session.mark_dirty();
        }

        // Sync the parsed grid into the session.
        session.grid = vt_parser.grid().clone_grid();

        // Pump Win32 messages without blocking.
        drain_win32_messages();

        // Handle window events.
        while let Ok(event) = event_rx.try_recv() {
            match event {
                WindowEvent::Close => running = false,

                WindowEvent::Char(code_unit) => {
                    if let Some(bytes) = char_to_pty_bytes(code_unit) {
                        let _ = pty.write(&bytes);
                    }
                }

                WindowEvent::KeyDown { vk, ctrl } => {
                    if let Some(bytes) = vk_to_pty_bytes(vk, ctrl) {
                        let _ = pty.write(&bytes);
                    }
                }

                WindowEvent::Resize { width, height } => {
                    window_w = width;
                    window_h = height;
                    compositor.resize(width, height)?;
                    // Approximate PTY resize — Phase 3 uses exact cell metrics.
                    let new_cols = (width / 8).max(20) as u16;
                    let new_rows = (height / 16).max(5) as u16;
                    let _ = pty.resize(new_cols, new_rows);
                }
            }
        }

        // Render if dirty.
        if session.is_dirty() {
            let snap = session.clone_for_render();
            compositor.render_frame(&[snap], &layout, window_w, window_h)?;
        }

        // Yield 1ms — keeps CPU low when idle without adding noticeable lag.
        std::thread::sleep(std::time::Duration::from_millis(1));
    }

    Ok(())
}

/// Drain all pending Win32 messages without blocking.
fn drain_win32_messages() {
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::*;
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message == WM_QUIT {
                return;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
