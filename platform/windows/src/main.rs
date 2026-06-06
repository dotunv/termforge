//! TermForge — Phase 5 entry point.
//!
//! Adds agent sessions (purple, Ctrl+A), block history overlay in the sidebar,
//! agent launcher modal, SSH host persistence, and delete-host (D key).

// GUI app: no console of our own.  A console-subsystem host causes the ConPTY
// child (cmd.exe) to leak a visible console window instead of running headless.
#![windows_subsystem = "windows"]

mod input;
mod ipc;
mod snapshot;
mod ui;
mod window;

use std::sync::mpsc;

use anyhow::Result;
use input::{char_to_pty_bytes, vk_to_pty_bytes};
use libterm::{
    block::{detector::BlockDetector, store::CommandBlock},
    mux::{
        layout::PaneLayout,
        session::{Session, SessionKind},
    },
    pty::{conpty::ConPty, Pty},
    ssh::{
        client::{SshAuth, SshClient},
        host_store::HostStore,
    },
    vt::VtParser,
};
use renderer_dx12::{compositor::Compositor, context::Dx12Context};
use rstar::RTreeObject as _;
use tokio::sync::mpsc::UnboundedReceiver;
use ui::{
    agent_launcher::{generate_agent_launcher_commands, AgentLauncherState},
    chrome::{
        self, build_block_rtree, generate_block_overlays, BlockHitTarget, ChromeLayout,
        ChromeState, PANE_HEADER_H, SPLIT_HANDLE_W,
    },
    settings::{generate_settings_commands, SettingsState},
    ssh_manager::{generate_ssh_manager_commands, SshManagerState},
};
use ipc::{IpcCommand, SessionInfo};
use snapshot::{SessionRecord, SessionRecordKind, Snapshot};
use window::{Window, WindowEvent};

const INIT_W: u32 = 1280;
const INIT_H: u32 = 768;

// ── Per-session bundle ────────────────────────────────────────────────────────

struct Entry {
    session: Session,
    vt_parser: VtParser,
    pty: Box<dyn Pty + Send>,
    pty_rx: UnboundedReceiver<Vec<u8>>,
    /// Tracks when this entry last received PTY output or user input.
    last_activity: std::time::Instant,
    /// True when the child process has been suspended to save CPU.
    hibernated: bool,
}

impl Entry {
    /// Spawn a local ConPTY session sized to `cols`×`rows` cells.
    fn spawn_local(shell: &str, cols: u16, rows: u16) -> Result<Self> {
        let sid = uuid::Uuid::new_v4();
        let session = Session::new(SessionKind::Local, cols, rows);
        let detector = BlockDetector::new(sid);
        let vt_parser = VtParser::new(cols, rows, detector);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let pty = ConPty::spawn(shell, cols, rows, tx)?;
        Ok(Self {
            session,
            vt_parser,
            pty: Box::new(pty),
            pty_rx: rx,
            last_activity: std::time::Instant::now(),
            hibernated: false,
        })
    }

    /// Spawn a local ConPTY session tagged as Agent (purple).
    fn spawn_agent(command: &str, model: &str) -> Result<Self> {
        let (cols, rows) = (220u16, 50u16);
        let sid = uuid::Uuid::new_v4();
        // Use the basename of the command as the short name.
        let name = std::path::Path::new(command)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(command)
            .to_string();
        let kind = SessionKind::Agent {
            name,
            model: model.to_string(),
        };
        let mut session = Session::new(kind, cols, rows);
        // Override title with the raw command for clarity.
        session.title = command.to_string();
        let detector = BlockDetector::new(sid);
        let vt_parser = VtParser::new(cols, rows, detector);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let pty = ConPty::spawn(command, cols, rows, tx)?;
        Ok(Self {
            session,
            vt_parser,
            pty: Box::new(pty),
            pty_rx: rx,
            last_activity: std::time::Instant::now(),
            hibernated: false,
        })
    }

    /// Connect an SSH session.
    async fn spawn_ssh(
        hostname: String,
        port: u16,
        username: String,
        auth: SshAuth,
    ) -> Result<Self> {
        let (cols, rows) = (220u16, 50u16);
        let title = format!("{username}@{hostname}");
        let sid = uuid::Uuid::new_v4();
        let kind = SessionKind::Ssh {
            host: hostname.clone(),
            user: username.clone(),
        };
        let mut session = Session::new(kind, cols, rows);
        session.title = title;
        let detector = BlockDetector::new(sid);
        let vt_parser = VtParser::new(cols, rows, detector);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let pty = SshClient::connect(&hostname, port, &username, auth, cols, rows, tx).await?;
        Ok(Self {
            session,
            vt_parser,
            pty: Box::new(pty),
            pty_rx: rx,
            last_activity: std::time::Instant::now(),
            hibernated: false,
        })
    }

    fn drain_pty(&mut self) -> bool {
        if self.hibernated {
            return false;
        }
        let mut dirty = false;
        while let Ok(bytes) = self.pty_rx.try_recv() {
            self.vt_parser.process(&bytes);
            dirty = true;
        }
        if dirty {
            self.last_activity = std::time::Instant::now();
            self.session.grid = self.vt_parser.grid().clone_grid();
            self.session.blocks = self.vt_parser.blocks().clone();
            self.session.mark_dirty();
        }
        dirty
    }

    /// Suspend the child process and mark this entry as hibernated.
    fn hibernate(&mut self) {
        if !self.hibernated {
            self.pty.suspend();
            self.hibernated = true;
            self.session.mark_dirty();
        }
    }

    /// Wake a hibernated entry: resume the child process and re-enable draining.
    fn wake(&mut self) {
        if self.hibernated {
            self.pty.resume();
            self.hibernated = false;
            self.last_activity = std::time::Instant::now();
            self.session.mark_dirty();
        }
    }

    /// Record user input activity and wake the entry if it was hibernated.
    fn record_input(&mut self) {
        self.last_activity = std::time::Instant::now();
        self.wake();
    }

    fn resize_pty(&mut self, pane_w: f32, pane_h: f32, cw: u32, ch: u32) {
        let new_cols = ((pane_w / cw as f32) as u16).max(10);
        let new_rows = ((pane_h / ch as f32) as u16).max(3);
        let _ = self.pty.resize(new_cols, new_rows);
    }
}

// ── Split drag state ──────────────────────────────────────────────────────────

struct DragState {
    origin_x: f32,
    start_ratio: f32,
    content_w: f32,
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    tracing::info!("TermForge starting — Phase 5 (agent sessions)");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let _guard = rt.enter();

    // ── Window & compositor ────────────────────────────────────────────────
    let (event_tx, event_rx) = mpsc::sync_channel::<WindowEvent>(512);
    let window = Window::new("TermForge", INIT_W, INIT_H, event_tx)?;
    let ctx = Dx12Context::new(window.hwnd, INIT_W, INIT_H)?;
    let mut compositor = Compositor::build(ctx, "Cascadia Code", 13.0)?;
    let (cw, ch) = compositor.cell_size();

    // ── Initial sessions ───────────────────────────────────────────────────
    let init_content = ChromeLayout::compute(INIT_W as f32, INIT_H as f32, true).content;
    let init_cols = ((init_content.w / cw as f32) as u16).max(20);
    let init_rows = (((init_content.h - PANE_HEADER_H) / ch as f32) as u16).max(5);

    let shell = if which_exists("pwsh.exe") {
        "pwsh.exe".to_string()
    } else if which_exists("powershell.exe") {
        "powershell.exe".to_string()
    } else {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into())
    };

    let (mut entries, mut active_tab_init) = restore_sessions_or_default(
        &shell, init_cols, init_rows, &rt,
    );
    let mut entries: Vec<Entry> = entries;

    // ── IPC server ────────────────────────────────────────────────────────
    let ipc_sessions = ipc::new_session_list();
    let ipc_rx = ipc::start(ipc_sessions.clone());

    // ── SSH host store ─────────────────────────────────────────────────────
    let mut host_store = HostStore::load().unwrap_or_default();
    let mut ssh_mgr = SshManagerState::new(host_store.hosts.clone());
    let mut agent_launcher = AgentLauncherState::new();
    let app_config =
        libterm::config::Config::load_or_default(&libterm::config::Config::default_path());
    let mut settings = SettingsState::new(app_config);

    // ── App state ──────────────────────────────────────────────────────────
    let mut window_w = INIT_W;
    let mut window_h = INIT_H;
    let mut active_tab = active_tab_init;
    let mut sidebar_vis = true;
    let mut split_ratio: Option<f32> = None;
    let mut drag: Option<DragState> = None;
    let mut running = true;
    let mut ui_dirty = true;
    let mut snapshot_dirty = true; // write snapshot on first frame
    // Rebuilt each render pass; used in the *next* event loop iteration for click routing.
    let mut block_rtree = build_block_rtree(Vec::new());

    while running {
        // ── PTY drain ────────────────────────────────────────────────────
        for entry in &mut entries {
            entry.drain_pty();
        }

        // ── IPC command drain ─────────────────────────────────────────────
        while let Ok(cmd) = ipc_rx.try_recv() {
            ui_dirty = true; // all IPC mutations must repaint
            match cmd {
                IpcCommand::Send { session, text } => {
                    if let Some(e) = entries.iter_mut().find(|e| e.session.id == session) {
                        e.record_input();
                        let _ = e.pty.write(text.as_bytes());
                    }
                }
                IpcCommand::Kill { session } => {
                    if let Some(pos) = entries.iter().position(|e| e.session.id == session) {
                        entries.remove(pos);
                        active_tab = active_tab.min(entries.len().saturating_sub(1));
                        snapshot_dirty = true;
                    }
                }
                IpcCommand::Hibernate { session } => {
                    if let Some(e) = entries.iter_mut().find(|e| e.session.id == session) {
                        e.hibernate();
                    }
                }
                IpcCommand::Wake { session } => {
                    if let Some(e) = entries.iter_mut().find(|e| e.session.id == session) {
                        e.wake();
                    }
                }
            }
        }

        // ── Agent hibernation: suspend idle agent sessions ────────────────
        const HIBERNATE_SECS: u64 = 300; // 5 minutes
        for (i, entry) in entries.iter_mut().enumerate() {
            if i == active_tab { continue; } // never hibernate the active tab
            if entry.hibernated { continue; }
            if !matches!(entry.session.kind, SessionKind::Agent { .. }) { continue; }
            if entry.last_activity.elapsed().as_secs() >= HIBERNATE_SECS {
                entry.hibernate();
                ui_dirty = true; // hibernation changes the block overlay color
            }
        }

        // ── Win32 messages ────────────────────────────────────────────────
        drain_win32_messages();

        // ── Window events ─────────────────────────────────────────────────
        let (pre_tab_count, pre_active) = (entries.len(), active_tab);
        while let Ok(event) = event_rx.try_recv() {
            ui_dirty = true;
            match event {
                WindowEvent::Close => running = false,

                WindowEvent::Char(cu) => {
                    let c = char::from_u32(cu as u32).unwrap_or('\0');

                    if settings.open {
                        settings.handle_char(c);
                    } else if ssh_mgr.open {
                        // Route to SSH manager
                        if let Some(host) = ssh_mgr.handle_char(c) {
                            // Persist immediately when a new host is added.
                            host_store.hosts = ssh_mgr.hosts.clone();
                            let _ = host_store.save();

                            let auth = ssh_auth_for(&host);
                            match rt.block_on(Entry::spawn_ssh(
                                host.hostname.clone(),
                                host.port,
                                host.username.clone(),
                                auth,
                            )) {
                                Ok(e) => {
                                    entries.push(e);
                                    active_tab = entries.len() - 1;
                                    ssh_mgr.open = false;
                                }
                                Err(e) => tracing::error!("SSH connect: {e}"),
                            }
                        }
                    } else if agent_launcher.open {
                        if let Some((cmd, model)) = agent_launcher.handle_char(c) {
                            match Entry::spawn_agent(&cmd, &model) {
                                Ok(e) => {
                                    entries.push(e);
                                    active_tab = entries.len() - 1;
                                }
                                Err(e) => tracing::error!("agent spawn: {e}"),
                            }
                        }
                    } else if let Some(bytes) = char_to_pty_bytes(cu) {
                        if let Some(e) = entries.get_mut(active_tab) {
                            e.record_input();
                            let _ = e.pty.write(&bytes);
                        }
                    }
                }

                WindowEvent::KeyDown { vk, ctrl } => {
                    // ── Settings modal keyboard ──────────────────────────
                    if settings.open {
                        settings.handle_key(vk);
                        if !settings.open {
                            settings.save_if_dirty();
                        }
                        continue;
                    }

                    // ── Agent launcher keyboard ───────────────────────────
                    if agent_launcher.open {
                        match vk {
                            0x1B => agent_launcher.handle_escape(), // VK_ESCAPE
                            _ => {}
                        }
                        continue;
                    }

                    // ── SSH manager navigation ────────────────────────────
                    if ssh_mgr.open {
                        match vk {
                            0x26 => ssh_mgr.handle_key_up(),   // VK_UP
                            0x28 => ssh_mgr.handle_key_down(), // VK_DOWN
                            0x1B => ssh_mgr.handle_escape(),   // VK_ESCAPE
                            0x44 => {
                                // D — delete host
                                if ssh_mgr.handle_delete() {
                                    host_store.hosts = ssh_mgr.hosts.clone();
                                    let _ = host_store.save();
                                }
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // ── Global shortcuts ──────────────────────────────────
                    if ctrl {
                        match vk {
                            0xDC => {
                                sidebar_vis = !sidebar_vis;
                                continue;
                            } // Ctrl+\
                            0x48 => {
                                ssh_mgr.open = !ssh_mgr.open;
                                continue;
                            } // Ctrl+H
                            0x41 => {
                                agent_launcher.open = !agent_launcher.open;
                                continue;
                            } // Ctrl+A
                            0xBC => {
                                settings.open = !settings.open;
                                continue;
                            } // Ctrl+, — Settings
                            0x54 => {
                                spawn_tab(&mut entries, &shell, init_cols, init_rows);
                                active_tab = entries.len() - 1;
                                continue;
                            } // Ctrl+T
                            0x57 => {
                                close_tab(&mut entries, &mut active_tab);
                                continue;
                            } // Ctrl+W
                            0x09 => {
                                active_tab = (active_tab + 1) % entries.len().max(1);
                                if let Some(e) = entries.get_mut(active_tab) { e.wake(); }
                                continue;
                            } // Ctrl+Tab
                            0x50 => {
                                split_ratio = match split_ratio {
                                    None => Some(0.5),
                                    Some(_) => None,
                                };
                                continue;
                            } // Ctrl+P
                            n @ 0x31..=0x39 => {
                                let idx = (n - 0x31) as usize;
                                if idx < entries.len() {
                                    active_tab = idx;
                                }
                                continue;
                            }
                            _ => {}
                        }
                    }

                    if let Some(bytes) = vk_to_pty_bytes(vk, ctrl) {
                        if let Some(e) = entries.get_mut(active_tab) {
                            e.record_input();
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
                    let layout =
                        ChromeLayout::compute(window_w as f32, window_h as f32, sidebar_vis);

                    // Block click — check RTree before tabs/splits so overlays
                    // take precedence over the underlying pane area.
                    let pt = [x as f32, y as f32];
                    let point_env = rstar::AABB::from_point(pt);
                    if let Some(target) =
                        block_rtree.locate_in_envelope_intersecting(&point_env).next()
                    {
                        // Copy block output to clipboard.
                        if let Some(block) = entries
                            .iter()
                            .flat_map(|e| e.session.blocks.all())
                            .find(|b| b.id == target.block_id)
                        {
                            let text = block.output_as_str().into_owned();
                            copy_to_clipboard(&text);
                        }
                    }

                    // Tab click
                    if layout.tabbar.contains(x as f32, y as f32) {
                        let mut tx = layout.tabbar.x + 8.0;
                        for (i, e) in entries.iter().enumerate() {
                            let tw = tab_width(&e.session.title, cw);
                            if (x as f32) >= tx && (x as f32) < tx + tw {
                                active_tab = i;
                                break;
                            }
                            tx += tw + 2.0;
                        }
                        // Wake any hibernated session when switching to it.
                        if let Some(e) = entries.get_mut(active_tab) {
                            e.wake();
                        }
                    }

                    // Split drag
                    if let Some(ratio) = split_ratio {
                        let ct = layout.content;
                        let hx = ct.x + ct.w * ratio;
                        if (x as f32 - hx).abs() <= SPLIT_HANDLE_W + 4.0 {
                            drag = Some(DragState {
                                origin_x: x as f32,
                                start_ratio: ratio,
                                content_w: ct.w,
                            });
                        }
                    }
                }

                WindowEvent::MouseMove { x, .. } => {
                    if let Some(ref ds) = drag {
                        let delta = x as f32 - ds.origin_x;
                        split_ratio = Some((ds.start_ratio + delta / ds.content_w).clamp(0.1, 0.9));
                    }
                }

                WindowEvent::LButtonUp => {
                    drag = None;
                }
            }
        }
        // Mark snapshot dirty if the session list or active tab changed.
        if entries.len() != pre_tab_count || active_tab != pre_active {
            snapshot_dirty = true;
        }

        // ── Chrome + layout ───────────────────────────────────────────────
        let chrome_layout = ChromeLayout::compute(window_w as f32, window_h as f32, sidebar_vis);
        let ct = chrome_layout.content;

        // Terminal area = content area minus pane header height.
        let term_h = ct.h - PANE_HEADER_H;
        let term_y = ct.y + PANE_HEADER_H;

        let (layout, render_idx): (PaneLayout, Vec<usize>) = match split_ratio {
            None => {
                if let Some(e) = entries.get_mut(active_tab) {
                    e.resize_pty(ct.w, term_h, cw, ch);
                }
                let id = entries[active_tab].session.id;
                (PaneLayout::leaf(id), vec![active_tab])
            }
            Some(ratio) => {
                let l = active_tab;
                let r = if entries.len() > 1 {
                    (active_tab + 1) % entries.len()
                } else {
                    active_tab
                };
                if let Some(e) = entries.get_mut(l) {
                    e.resize_pty(ct.w * ratio, term_h, cw, ch);
                }
                if l != r {
                    if let Some(e) = entries.get_mut(r) {
                        e.resize_pty(ct.w * (1.0 - ratio), term_h, cw, ch);
                    }
                }
                let layout = PaneLayout::hsplit(
                    PaneLayout::leaf(entries[l].session.id),
                    PaneLayout::leaf(entries[r].session.id),
                    ratio,
                );
                (layout, vec![l, r])
            }
        };

        let handle_xs = layout.split_handle_xs(ct.x, ct.y, ct.w, ct.h);

        let chrome_sessions: Vec<Session> = entries
            .iter()
            .map(|e| e.session.clone_for_render())
            .collect();
        let error_count = entries
            .iter()
            .filter(|e| {
                e.session
                    .blocks
                    .all()
                    .last()
                    .and_then(|b| b.exit_code)
                    .map(|code| code != 0)
                    .unwrap_or(false)
            })
            .count();
        let awaiting_count = entries
            .iter()
            .flat_map(|e| e.session.blocks.all())
            .filter(|b| {
                matches!(
                    b.notification,
                    Some(libterm::vt::sequences::OscNotification::StatusAwaiting)
                )
            })
            .count();

        // Collect recent agent blocks for the sidebar overlay (only for agent sessions).
        let agent_blocks: Vec<CommandBlock> = if let Some(e) = entries.get(active_tab) {
            if matches!(e.session.kind, SessionKind::Agent { .. }) {
                e.session.blocks.clone_recent(20)
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        // Build pane rects for pane header rendering (x, y, w, h, session_idx).
        let pane_rects_for_chrome: Vec<(f32, f32, f32, f32, usize)> = match split_ratio {
            None => vec![(ct.x, ct.y, ct.w, ct.h, active_tab)],
            Some(ratio) => {
                let l = active_tab;
                let r = if entries.len() > 1 {
                    (active_tab + 1) % entries.len()
                } else {
                    active_tab
                };
                let lw = ct.w * ratio - SPLIT_HANDLE_W * 0.5;
                let rw = ct.w * (1.0 - ratio) - SPLIT_HANDLE_W * 0.5;
                vec![
                    (ct.x, ct.y, lw, ct.h, l),
                    (
                        ct.x + ct.w * ratio + SPLIT_HANDLE_W * 0.5,
                        ct.y,
                        rw,
                        ct.h,
                        r,
                    ),
                ]
            }
        };

        let mut chrome_cmds = chrome::generate_commands(&ChromeState {
            layout: &chrome_layout,
            sessions: &chrome_sessions,
            active_tab,
            workspace_name: "work / backend",
            sidebar_visible: sidebar_vis,
            split_handles: &handle_xs,
            error_count,
            awaiting_count,
            cell_w: cw,
            cell_h: ch,
            agent_blocks: &agent_blocks,
            pane_rects: &pane_rects_for_chrome,
        });

        // SSH manager overlay on top
        chrome_cmds.extend(generate_ssh_manager_commands(
            &ssh_mgr,
            window_w as f32,
            window_h as f32,
            cw,
            ch,
        ));

        // Agent launcher overlay
        chrome_cmds.extend(generate_agent_launcher_commands(
            &agent_launcher,
            window_w as f32,
            window_h as f32,
            cw,
            ch,
        ));

        // Settings overlay (topmost)
        chrome_cmds.extend(generate_settings_commands(
            &settings,
            window_w as f32,
            window_h as f32,
            cw,
            ch,
        ));

        // ── Block overlays per visible pane ───────────────────────────────
        let mut all_hit_targets: Vec<BlockHitTarget> = Vec::new();
        for &(px, py, pw, _ph, session_idx) in &pane_rects_for_chrome {
            if let Some(entry) = entries.get(session_idx) {
                let blocks = entry.session.blocks.all();
                if !blocks.is_empty() {
                    let grid_rows = entry.session.grid.rows();
                    let (overlay_cmds, hit_targets) = generate_block_overlays(
                        blocks,
                        px,
                        py + PANE_HEADER_H,
                        pw,
                        cw as f32,
                        ch as f32,
                        grid_rows,
                    );
                    chrome_cmds.extend(overlay_cmds);
                    all_hit_targets.extend(hit_targets);
                }
            }
        }
        block_rtree = build_block_rtree(all_hit_targets);

        // ── Update IPC session list ───────────────────────────────────────
        if let Ok(mut list) = ipc_sessions.lock() {
            *list = entries
                .iter()
                .map(|e| SessionInfo {
                    id: e.session.id.to_string(),
                    kind: match &e.session.kind {
                        SessionKind::Local => "local".into(),
                        SessionKind::Ssh { .. } => "ssh".into(),
                        SessionKind::Agent { .. } => "agent".into(),
                    },
                    title: e.session.title.clone(),
                    hibernated: e.hibernated,
                })
                .collect();
        }

        // ── Session snapshot ──────────────────────────────────────────────
        if snapshot_dirty {
            save_snapshot(&entries, active_tab, &shell);
            snapshot_dirty = false;
        }

        // ── Render ────────────────────────────────────────────────────────
        let render_sessions: Vec<Session> = render_idx
            .iter()
            .map(|&i| entries[i].session.clone_for_render())
            .collect();

        let any_dirty = render_idx.iter().any(|&i| entries[i].session.is_dirty())
            || ui_dirty
            || ssh_mgr.open
            || agent_launcher.open
            || settings.open;

        if any_dirty {
            compositor.render_frame_with_chrome(
                &render_sessions,
                &layout,
                &chrome_cmds,
                window_w,
                window_h,
                Some((ct.x, term_y, ct.w, term_h)),
            )?;
            ui_dirty = false;
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
            if msg.message == WM_QUIT {
                return;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn spawn_tab(entries: &mut Vec<Entry>, shell: &str, cols: u16, rows: u16) {
    match Entry::spawn_local(shell, cols, rows) {
        Ok(e) => entries.push(e),
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

fn which_exists(name: &str) -> bool {
    std::process::Command::new("where")
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Copy `text` to the Windows clipboard as CF_UNICODETEXT.
fn copy_to_clipboard(text: &str) {
    unsafe {
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::DataExchange::{
            CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
        };
        use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0u16)).collect();
        let byte_len = wide.len() * 2;

        let Ok(hmem) = GlobalAlloc(GMEM_MOVEABLE, byte_len) else { return };
        let ptr = GlobalLock(hmem);
        if ptr.is_null() { return; }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr.cast::<u16>(), wide.len());
        let _ = GlobalUnlock(hmem);

        if OpenClipboard(None).is_ok() {
            let _ = EmptyClipboard();
            // CF_UNICODETEXT = 13
            let _ = SetClipboardData(13, HANDLE(hmem.0));
            let _ = CloseClipboard();
        }
    }
}

/// Serialize current entries into a Snapshot and persist to disk.
fn save_snapshot(entries: &[Entry], active_tab: usize, default_shell: &str) {
    let sessions: Vec<SessionRecord> = entries
        .iter()
        .map(|e| {
            let (cols, rows) = (e.session.grid.cols(), e.session.grid.rows());
            let kind = match &e.session.kind {
                SessionKind::Local => SessionRecordKind::Local {
                    command: default_shell.to_string(),
                },
                SessionKind::Ssh { host, user } => SessionRecordKind::Ssh {
                    host: host.clone(),
                    user: user.clone(),
                    port: 22,
                },
                SessionKind::Agent { name, model } => SessionRecordKind::Agent {
                    command: e.session.title.clone(),
                    model: model.clone(),
                },
            };
            SessionRecord {
                kind,
                title: e.session.title.clone(),
                cols,
                rows,
            }
        })
        .collect();

    Snapshot {
        version: Snapshot::VERSION,
        active: active_tab.min(sessions.len().saturating_sub(1)),
        sessions,
    }
    .save();
}

/// Restore sessions from a snapshot, or fall back to spawning one default shell.
/// Returns (entries, active_tab).
fn restore_sessions_or_default(
    shell: &str,
    fallback_cols: u16,
    fallback_rows: u16,
    rt: &tokio::runtime::Runtime,
) -> (Vec<Entry>, usize) {
    if let Some(snap) = Snapshot::load() {
        let mut entries = Vec::new();
        for rec in &snap.sessions {
            let result = match &rec.kind {
                SessionRecordKind::Local { command } => {
                    Entry::spawn_local(command, rec.cols, rec.rows)
                }
                SessionRecordKind::Agent { command, model } => {
                    Entry::spawn_agent(command, model)
                }
                SessionRecordKind::Ssh { .. } => {
                    // SSH sessions are not auto-reconnected; fall back to a local shell.
                    Entry::spawn_local(shell, rec.cols, rec.rows)
                }
            };
            match result {
                Ok(e) => entries.push(e),
                Err(err) => tracing::warn!("restore session failed: {err}"),
            }
        }
        if !entries.is_empty() {
            let active = snap.active.min(entries.len() - 1);
            tracing::info!("restored {} session(s) from snapshot", entries.len());
            return (entries, active);
        }
    }
    // No snapshot or all restores failed — open a fresh local session.
    match Entry::spawn_local(shell, fallback_cols, fallback_rows) {
        Ok(e) => (vec![e], 0),
        Err(err) => {
            tracing::error!("failed to spawn default shell: {err}");
            (Vec::new(), 0)
        }
    }
}

fn ssh_auth_for(host: &libterm::ssh::host_store::HostConfig) -> SshAuth {
    if let Some(ref kp) = host.key_path {
        SshAuth::PrivateKey {
            key_path: std::path::PathBuf::from(kp),
            passphrase: None,
        }
    } else {
        SshAuth::Agent
    }
}
