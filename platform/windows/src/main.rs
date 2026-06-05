//! TermForge — Phase 4 entry point.
//!
//! Supports multiple local (ConPTY) and remote (SSH) sessions, a full DX12
//! chrome render pass, collapsible sidebar, split-pane drag, and the SSH
//! manager modal (Ctrl+H).

mod input;
mod ui;
mod window;

use std::sync::mpsc;

use anyhow::Result;
use libterm::{
    block::detector::BlockDetector,
    mux::{layout::PaneLayout, session::{Session, SessionKind}},
    pty::{conpty::ConPty, Pty},
    ssh::{client::{SshAuth, SshClient}, host_store::HostStore},
    vt::VtParser,
};
use renderer_dx12::{compositor::Compositor, context::Dx12Context};
use tokio::sync::mpsc::UnboundedReceiver;
use ui::{
    chrome::{self, ChromeLayout, ChromeState, SPLIT_HANDLE_W},
    ssh_manager::{generate_ssh_manager_commands, SshManagerState},
};
use window::{Window, WindowEvent};
use input::{char_to_pty_bytes, vk_to_pty_bytes};

const INIT_W: u32 = 1280;
const INIT_H: u32 = 768;

// ── Per-session bundle ────────────────────────────────────────────────────────

struct Entry {
    session:   Session,
    vt_parser: VtParser,
    pty:       Box<dyn Pty + Send>,
    pty_rx:    UnboundedReceiver<Vec<u8>>,
}

impl Entry {
    /// Spawn a local ConPTY session.
    fn spawn_local(shell: &str) -> Result<Self> {
        let (cols, rows) = (220u16, 50u16);
        let sid          = uuid::Uuid::new_v4();
        let session      = Session::new(SessionKind::Local, cols, rows);
        let detector     = BlockDetector::new(sid);
        let vt_parser    = VtParser::new(cols, rows, detector);
        let (tx, rx)     = tokio::sync::mpsc::unbounded_channel();
        let pty          = ConPty::spawn(shell, cols, rows, tx)?;
        Ok(Self { session, vt_parser, pty: Box::new(pty), pty_rx: rx })
    }

    /// Connect an SSH session.
    async fn spawn_ssh(
        hostname: String,
        port: u16,
        username: String,
        auth: SshAuth,
    ) -> Result<Self> {
        let (cols, rows) = (220u16, 50u16);
        let title        = format!("{username}@{hostname}");
        let sid          = uuid::Uuid::new_v4();
        let kind         = SessionKind::Ssh {
            host: hostname.clone(),
            user: username.clone(),
        };
        let mut session  = Session::new(kind, cols, rows);
        session.title    = title;
        let detector     = BlockDetector::new(sid);
        let vt_parser    = VtParser::new(cols, rows, detector);
        let (tx, rx)     = tokio::sync::mpsc::unbounded_channel();
        let pty          = SshClient::connect(
            &hostname, port, &username, auth, cols, rows, tx,
        ).await?;
        Ok(Self { session, vt_parser, pty: Box::new(pty), pty_rx: rx })
    }

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

    fn resize_pty(&mut self, pane_w: f32, pane_h: f32, cw: u32, ch: u32) {
        let new_cols = ((pane_w / cw as f32) as u16).max(10);
        let new_rows = ((pane_h / ch as f32) as u16).max(3);
        let _ = self.pty.resize(new_cols, new_rows);
    }
}

// ── Split drag state ──────────────────────────────────────────────────────────

struct DragState {
    origin_x:    f32,
    start_ratio: f32,
    content_w:   f32,
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    tracing::info!("TermForge starting — Phase 4 (SSH + multi-session)");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let _guard = rt.enter();

    // ── Window & compositor ────────────────────────────────────────────────
    let (event_tx, event_rx) = mpsc::sync_channel::<WindowEvent>(512);
    let window      = Window::new("TermForge", INIT_W, INIT_H, event_tx)?;
    let ctx         = Dx12Context::new(window.hwnd, INIT_W, INIT_H)?;
    let mut compositor = Compositor::build(ctx, "Cascadia Code", 13.0)?;
    let (cw, ch)    = compositor.cell_size();

    // ── Initial local session ──────────────────────────────────────────────
    let shell = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into());
    let mut entries: Vec<Entry> = vec![Entry::spawn_local(&shell)?];

    // ── SSH host store ─────────────────────────────────────────────────────
    let host_store   = HostStore::load().unwrap_or_default();
    let mut ssh_mgr  = SshManagerState::new(host_store.hosts.clone());

    // ── App state ──────────────────────────────────────────────────────────
    let mut window_w    = INIT_W;
    let mut window_h    = INIT_H;
    let mut active_tab  = 0usize;
    let mut sidebar_vis = true;
    let mut split_ratio: Option<f32> = None;
    let mut drag: Option<DragState>  = None;
    let mut running = true;

    while running {
        // ── PTY drain ────────────────────────────────────────────────────
        for entry in &mut entries { entry.drain_pty(); }

        // ── Win32 messages ────────────────────────────────────────────────
        drain_win32_messages();

        // ── Window events ─────────────────────────────────────────────────
        while let Ok(event) = event_rx.try_recv() {
            match event {
                WindowEvent::Close => running = false,

                WindowEvent::Char(cu) => {
                    let c = char::from_u32(cu as u32).unwrap_or('\0');

                    if ssh_mgr.open {
                        // Route to SSH manager
                        if let Some(host) = ssh_mgr.handle_char(c) {
                            let auth = ssh_auth_for(&host);
                            match rt.block_on(Entry::spawn_ssh(
                                host.hostname.clone(),
                                host.port,
                                host.username.clone(),
                                auth,
                            )) {
                                Ok(e)  => {
                                    entries.push(e);
                                    active_tab = entries.len() - 1;
                                    ssh_mgr.open = false;
                                }
                                Err(e) => tracing::error!("SSH connect: {e}"),
                            }
                        }
                    } else if let Some(bytes) = char_to_pty_bytes(cu) {
                        if let Some(e) = entries.get_mut(active_tab) {
                            let _ = e.pty.write(&bytes);
                        }
                    }
                }

                WindowEvent::KeyDown { vk, ctrl } => {
                    // ── SSH manager navigation ────────────────────────────
                    if ssh_mgr.open {
                        match vk {
                            0x26 => ssh_mgr.handle_key_up(),    // VK_UP
                            0x28 => ssh_mgr.handle_key_down(),  // VK_DOWN
                            0x1B => ssh_mgr.handle_escape(),    // VK_ESCAPE
                            _ => {}
                        }
                        continue;
                    }

                    // ── Global shortcuts ──────────────────────────────────
                    if ctrl {
                        match vk {
                            0xDC => { sidebar_vis = !sidebar_vis; continue; }          // Ctrl+\
                            0x48 => { ssh_mgr.open = !ssh_mgr.open; continue; }        // Ctrl+H
                            0x54 => { spawn_tab(&mut entries, &shell); active_tab = entries.len() - 1; continue; } // Ctrl+T
                            0x57 => { close_tab(&mut entries, &mut active_tab); continue; } // Ctrl+W
                            0x09 => { active_tab = (active_tab + 1) % entries.len().max(1); continue; } // Ctrl+Tab
                            0x50 => { split_ratio = match split_ratio { None => Some(0.5), Some(_) => None }; continue; } // Ctrl+P
                            n @ 0x31..=0x39 => {
                                let idx = (n - 0x31) as usize;
                                if idx < entries.len() { active_tab = idx; }
                                continue;
                            }
                            _ => {}
                        }
                    }

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
                }

                WindowEvent::LButtonDown { x, y } => {
                    let layout = ChromeLayout::compute(
                        window_w as f32, window_h as f32, sidebar_vis);

                    // Tab click
                    if layout.tabbar.contains(x as f32, y as f32) {
                        let mut tx = layout.tabbar.x + 8.0;
                        for (i, e) in entries.iter().enumerate() {
                            let tw = tab_width(&e.session.title, cw);
                            if (x as f32) >= tx && (x as f32) < tx + tw {
                                active_tab = i; break;
                            }
                            tx += tw + 2.0;
                        }
                    }

                    // Split drag
                    if let Some(ratio) = split_ratio {
                        let ct = layout.content;
                        let hx = ct.x + ct.w * ratio;
                        if (x as f32 - hx).abs() <= SPLIT_HANDLE_W + 4.0 {
                            drag = Some(DragState {
                                origin_x:    x as f32,
                                start_ratio: ratio,
                                content_w:   ct.w,
                            });
                        }
                    }
                }

                WindowEvent::MouseMove { x, .. } => {
                    if let Some(ref ds) = drag {
                        let delta     = x as f32 - ds.origin_x;
                        split_ratio   = Some((ds.start_ratio + delta / ds.content_w).clamp(0.1, 0.9));
                    }
                }

                WindowEvent::LButtonUp => { drag = None; }
            }
        }

        // ── Chrome + layout ───────────────────────────────────────────────
        let chrome_layout = ChromeLayout::compute(
            window_w as f32, window_h as f32, sidebar_vis);
        let ct = chrome_layout.content;

        let (layout, render_idx): (PaneLayout, Vec<usize>) = match split_ratio {
            None => {
                if let Some(e) = entries.get_mut(active_tab) {
                    e.resize_pty(ct.w, ct.h, cw, ch);
                }
                let id = entries[active_tab].session.id;
                (PaneLayout::leaf(id), vec![active_tab])
            }
            Some(ratio) => {
                let l = active_tab;
                let r = if entries.len() > 1 { (active_tab + 1) % entries.len() } else { active_tab };
                if let Some(e) = entries.get_mut(l) { e.resize_pty(ct.w * ratio, ct.h, cw, ch); }
                if l != r { if let Some(e) = entries.get_mut(r) { e.resize_pty(ct.w * (1.0-ratio), ct.h, cw, ch); } }
                let layout = PaneLayout::hsplit(
                    PaneLayout::leaf(entries[l].session.id),
                    PaneLayout::leaf(entries[r].session.id),
                    ratio,
                );
                (layout, vec![l, r])
            }
        };

        let handle_xs = layout.split_handle_xs(ct.x, ct.y, ct.w, ct.h);

        let chrome_sessions: Vec<Session> = entries.iter().map(|e| e.session.clone_for_render()).collect();
        let error_count = entries.iter().filter(|e|
            e.session.blocks.all().last().map(|b| b.exit_code == Some(1)).unwrap_or(false)
        ).count();

        let mut chrome_cmds = chrome::generate_commands(&ChromeState {
            layout:          &chrome_layout,
            sessions:        &chrome_sessions,
            active_tab,
            workspace_name:  "work / backend",
            sidebar_visible: sidebar_vis,
            split_handles:   &handle_xs,
            error_count,
            cell_w: cw, cell_h: ch,
        });

        // SSH manager overlay on top
        chrome_cmds.extend(generate_ssh_manager_commands(
            &ssh_mgr, window_w as f32, window_h as f32, cw, ch,
        ));

        // ── Render ────────────────────────────────────────────────────────
        let render_sessions: Vec<Session> = render_idx.iter()
            .map(|&i| entries[i].session.clone_for_render())
            .collect();

        let any_dirty = render_idx.iter().any(|&i| entries[i].session.is_dirty())
            || ssh_mgr.open;

        if any_dirty {
            compositor.render_frame_with_chrome(
                &render_sessions, &layout,
                &chrome_cmds,
                window_w, window_h,
                Some((ct.x, ct.y, ct.w, ct.h)),
            )?;
        }

        std::thread::sleep(std::time::Duration::from_millis(1));
    }

    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

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

fn spawn_tab(entries: &mut Vec<Entry>, shell: &str) {
    match Entry::spawn_local(shell) {
        Ok(e)  => entries.push(e),
        Err(e) => tracing::error!("spawn tab: {e}"),
    }
}

fn close_tab(entries: &mut Vec<Entry>, active_tab: &mut usize) {
    if entries.len() > 1 {
        entries.remove(*active_tab);
        *active_tab = (*active_tab).min(entries.len() - 1);
    }
}

fn tab_width(title: &str, cw: u32) -> f32 {
    // pad + dot_gap + text + pad
    10.0 + 6.0 + 4.0 + title.chars().count() as f32 * cw as f32 + 10.0
}

fn ssh_auth_for(host: &libterm::ssh::host_store::HostConfig) -> SshAuth {
    if let Some(ref kp) = host.key_path {
        SshAuth::PrivateKey {
            key_path:   std::path::PathBuf::from(kp),
            passphrase: None,
        }
    } else {
        SshAuth::Agent
    }
}
