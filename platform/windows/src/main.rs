//! TermForge — Phase 3 entry point.
//!
//! Manages multiple ConPTY sessions, routes keyboard input to the active
//! session, renders chrome (tab bar, sidebar, status bar) + terminal panes
//! via the DX12 compositor, and handles pane split drag-to-resize.

mod input;
mod ui;
mod window;

use std::sync::mpsc;

use anyhow::Result;
use libterm::{
    block::detector::BlockDetector,
    mux::{layout::PaneLayout, session::{Session, SessionKind}},
    pty::{conpty::ConPty, Pty},
    vt::VtParser,
};
use renderer_dx12::{compositor::Compositor, context::Dx12Context};
use tokio::sync::mpsc::UnboundedReceiver;
use ui::chrome::{self, ChromeLayout, ChromeState, SPLIT_HANDLE_W};
use window::{Window, WindowEvent};

use input::{char_to_pty_bytes, vk_to_pty_bytes};

const INIT_W: u32 = 1280;
const INIT_H: u32 = 768;

// ── Per-session bundle ────────────────────────────────────────────────────────

struct Entry {
    session:   Session,
    vt_parser: VtParser,
    pty:       ConPty,
    pty_rx:    UnboundedReceiver<Vec<u8>>,
}

impl Entry {
    fn spawn(kind: SessionKind, shell: &str) -> Result<Self> {
        let cols: u16 = 220;
        let rows: u16 = 50;
        let session_id = uuid::Uuid::new_v4();
        let session    = Session::new(kind, cols, rows);
        let detector   = BlockDetector::new(session_id);
        let vt_parser  = VtParser::new(cols, rows, detector);

        let (pty_tx, pty_rx) = tokio::sync::mpsc::unbounded_channel();
        let pty = ConPty::spawn(shell, cols, rows, pty_tx)?;

        Ok(Self { session, vt_parser, pty, pty_rx })
    }

    /// Drain all pending PTY output into the VT parser.  Returns true if
    /// anything was processed (session needs re-render).
    fn drain_pty(&mut self) -> bool {
        let mut dirty = false;
        while let Ok(bytes) = self.pty_rx.try_recv() {
            self.vt_parser.process(&bytes);
            dirty = true;
        }
        if dirty {
            self.session.grid = self.vt_parser.grid().clone_grid();
            self.session.mark_dirty();
        }
        dirty
    }

    /// Resize PTY to match the current pane pixel dimensions + cell size.
    fn resize_pty(&mut self, pane_w: f32, pane_h: f32, cell_w: u32, cell_h: u32) {
        let new_cols = ((pane_w / cell_w as f32) as u16).max(10);
        let new_rows = ((pane_h / cell_h as f32) as u16).max(3);
        let _ = self.pty.resize(new_cols, new_rows);
    }
}

// ── Split-drag state ──────────────────────────────────────────────────────────

struct DragState {
    /// Pixel x of the handle when the drag began.
    origin_x:    f32,
    /// Ratio at drag start.
    start_ratio: f32,
    /// Content area x + w captured at drag start (to normalise mouse delta).
    content_x:   f32,
    content_w:   f32,
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    tracing::info!("TermForge starting — Phase 3 (chrome + multi-session)");

    // Tokio runtime for UnboundedReceiver::try_recv on the main thread.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let _guard = rt.enter();

    // ── Window ───────────────────────────────────────────────────────────────
    let (event_tx, event_rx) = mpsc::sync_channel::<WindowEvent>(512);
    let window = Window::new("TermForge", INIT_W, INIT_H, event_tx)?;

    // ── Compositor (DX12 + glyph atlas) ─────────────────────────────────────
    let ctx = Dx12Context::new(window.hwnd, INIT_W, INIT_H)?;
    let mut compositor = Compositor::build(ctx, "Cascadia Code", 13.0)?;
    let (cell_w, cell_h) = compositor.cell_size();

    // ── Sessions ─────────────────────────────────────────────────────────────
    let shell = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into());
    let mut entries: Vec<Entry> = vec![Entry::spawn(SessionKind::Local, &shell)?];

    // ── App state ─────────────────────────────────────────────────────────────
    let mut window_w    = INIT_W;
    let mut window_h    = INIT_H;
    let mut active_tab  = 0usize;
    let mut sidebar_vis = true;
    // None = single pane; Some(r) = two-pane horizontal split at ratio r.
    let mut split_ratio: Option<f32> = None;
    let mut drag: Option<DragState> = None;
    let mut running = true;

    while running {
        // ── PTY drain ────────────────────────────────────────────────────────
        for entry in &mut entries {
            entry.drain_pty();
        }

        // ── Pump Win32 messages ───────────────────────────────────────────────
        drain_win32_messages();

        // ── Handle window events ──────────────────────────────────────────────
        while let Ok(event) = event_rx.try_recv() {
            match event {
                WindowEvent::Close => running = false,

                WindowEvent::Char(cu) => {
                    if let Some(bytes) = char_to_pty_bytes(cu) {
                        if let Some(e) = entries.get_mut(active_tab) {
                            let _ = e.pty.write(&bytes);
                        }
                    }
                }

                WindowEvent::KeyDown { vk, ctrl } => {
                    // ── Global shortcuts ──────────────────────────────────────
                    if ctrl {
                        match vk {
                            // Ctrl+\ — toggle sidebar
                            0xDC => { sidebar_vis = !sidebar_vis; continue; }
                            // Ctrl+T — new local tab
                            0x54 => {
                                if let Ok(e) = Entry::spawn(SessionKind::Local, &shell) {
                                    entries.push(e);
                                    active_tab = entries.len() - 1;
                                }
                                continue;
                            }
                            // Ctrl+W — close active tab
                            0x57 => {
                                if entries.len() > 1 {
                                    entries.remove(active_tab);
                                    active_tab = active_tab.min(entries.len() - 1);
                                }
                                continue;
                            }
                            // Ctrl+Tab — next tab
                            0x09 => {
                                active_tab = (active_tab + 1) % entries.len();
                                continue;
                            }
                            // Ctrl+\ split toggle  (Ctrl+P = split)
                            0x50 => {
                                split_ratio = match split_ratio {
                                    None    => Some(0.5),
                                    Some(_) => None,
                                };
                                continue;
                            }
                            // Ctrl+1..9 — switch tab
                            n @ 0x31..=0x39 => {
                                let idx = (n - 0x31) as usize;
                                if idx < entries.len() { active_tab = idx; }
                                continue;
                            }
                            _ => {}
                        }
                    }
                    // Forward escape-sequence keys to active PTY.
                    if let Some(bytes) = vk_to_pty_bytes(vk, ctrl) {
                        if let Some(e) = entries.get_mut(active_tab) {
                            let _ = e.pty.write(&bytes);
                        }
                    }
                }

                WindowEvent::Resize { width, height } => {
                    window_w = width;
                    window_h = height;
                    compositor.resize(width, height)?;
                    // PTY resize handled below after chrome layout is recalculated.
                }

                WindowEvent::LButtonDown { x, y } => {
                    // ── Tab click ─────────────────────────────────────────────
                    let layout = ChromeLayout::compute(
                        window_w as f32, window_h as f32, sidebar_vis);
                    if layout.tabbar.contains(x as f32, y as f32) {
                        let cw = cell_w as f32;
                        let mut tx = layout.tabbar.x + 8.0;
                        for (i, entry) in entries.iter().enumerate() {
                            let label = entry.session.title.as_str();
                            let tw = 10.0 + 6.0 + 4.0 + label.chars().count() as f32 * cw + 10.0;
                            if (x as f32) >= tx && (x as f32) < tx + tw {
                                active_tab = i;
                                break;
                            }
                            tx += tw + 2.0;
                        }
                    }

                    // ── Split handle drag start ────────────────────────────────
                    if let Some(ratio) = split_ratio {
                        let layout = ChromeLayout::compute(
                            window_w as f32, window_h as f32, sidebar_vis);
                        let ct = layout.content;
                        let handle_x = ct.x + ct.w * ratio;
                        if (x as f32 - handle_x).abs() <= SPLIT_HANDLE_W + 4.0 {
                            drag = Some(DragState {
                                origin_x:    x as f32,
                                start_ratio: ratio,
                                content_x:   ct.x,
                                content_w:   ct.w,
                            });
                        }
                    }
                }

                WindowEvent::MouseMove { x, .. } => {
                    if let Some(ref ds) = drag {
                        let delta = x as f32 - ds.origin_x;
                        let new_ratio = (ds.start_ratio + delta / ds.content_w)
                            .clamp(0.1, 0.9);
                        split_ratio = Some(new_ratio);
                    }
                }

                WindowEvent::LButtonUp => {
                    drag = None;
                }
            }
        }

        // ── Build chrome layout ───────────────────────────────────────────────
        let chrome_layout = ChromeLayout::compute(
            window_w as f32, window_h as f32, sidebar_vis);
        let ct = chrome_layout.content;

        // ── Build pane layout + resize PTYs ──────────────────────────────────
        let (layout, render_entries): (PaneLayout, Vec<usize>) = match split_ratio {
            None => {
                let id = entries[active_tab].session.id;
                let pane_w = ct.w;
                let pane_h = ct.h;
                entries[active_tab].resize_pty(pane_w, pane_h, cell_w, cell_h);
                (PaneLayout::leaf(id), vec![active_tab])
            }
            Some(ratio) => {
                let left_idx  = active_tab;
                let right_idx = if entries.len() < 2 {
                    // Only one session: show it on both sides (read-only right).
                    active_tab
                } else {
                    (active_tab + 1) % entries.len()
                };
                let left_id  = entries[left_idx].session.id;
                let right_id = entries[right_idx].session.id;
                entries[left_idx].resize_pty(ct.w * ratio, ct.h, cell_w, cell_h);
                entries[right_idx].resize_pty(ct.w * (1.0 - ratio), ct.h, cell_w, cell_h);
                let layout = PaneLayout::hsplit(
                    PaneLayout::leaf(left_id),
                    PaneLayout::leaf(right_id),
                    ratio,
                );
                (layout, vec![left_idx, right_idx])
            }
        };

        // Collect split handle x positions for chrome rendering.
        let handle_xs: Vec<f32> = layout.split_handle_xs(ct.x, ct.y, ct.w, ct.h);

        // ── Chrome commands ───────────────────────────────────────────────────
        let error_count = entries.iter()
            .filter(|e| e.session.blocks.all()
                .last()
                .map(|b| b.exit_code == Some(1))
                .unwrap_or(false))
            .count();

        // Collect shallow session copies for chrome generation (title + kind only).
        let chrome_sessions: Vec<Session> = entries.iter()
            .map(|e| e.session.clone_for_render())
            .collect();

        let chrome_cmds = chrome::generate_commands(&ChromeState {
            layout:          &chrome_layout,
            sessions:        &chrome_sessions,
            active_tab,
            workspace_name:  "work / backend",
            sidebar_visible: sidebar_vis,
            split_handles:   &handle_xs,
            error_count,
            cell_w,
            cell_h,
        });

        // ── Render ────────────────────────────────────────────────────────────
        // Collect render snapshots (only sessions in current layout).
        let render_sessions: Vec<Session> = render_entries.iter()
            .map(|&i| entries[i].session.clone_for_render())
            .collect();

        // Only render if any visible session changed or chrome needs refresh.
        let any_dirty = render_entries.iter().any(|&i| entries[i].session.is_dirty());
        if any_dirty || !chrome_cmds.is_empty() {
            compositor.render_frame_with_chrome(
                &render_sessions,
                &layout,
                &chrome_cmds,
                window_w,
                window_h,
                Some((ct.x, ct.y, ct.w, ct.h)),
            )?;
        }

        std::thread::sleep(std::time::Duration::from_millis(1));
    }

    Ok(())
}

fn drain_win32_messages() {
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::*;
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message == WM_QUIT { return; }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
