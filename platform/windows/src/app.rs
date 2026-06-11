//! Application state and main run loop.
//!
//! `AppState` owns every piece of mutable state that persists across frames.
//! `AppState::run` is the entire event-render loop; its sub-methods each own
//! one clearly-named responsibility.

use std::sync::mpsc;

use anyhow::Result;
use libterm::{
    block::store::CommandBlock,
    config::Config,
    mux::{layout::PaneLayout, session::{Session, SessionKind}},
    ssh::host_store::HostStore,
};
use renderer_windows::compositor::Compositor;

use crate::{
    entry::{ssh_auth_for, Entry},
    input::{char_to_pty_bytes, vk_to_pty_bytes},
    ipc::{self, IpcCommand, SessionInfo},
    snapshot,
    ui::{
        agent_launcher::{generate_agent_launcher_commands, AgentLauncherState},
        animation::{AnimationEngine, AnimationTarget, Easing},
        block_overlay::{build_block_rtree, BlockHitTarget},
        block_view,
        chrome,
        command_palette::{generate_command_palette_commands, CommandPaletteState, PaletteAction},
        input_editor::{self, EditorEffect, InputEditor},
        layout::{ChromeLayout, ChromeState, Rect, PANE_HEADER_H, SESSION_BAR_H, SIDEBAR_W, SPLIT_HANDLE_W},
        settings::{generate_settings_commands, SettingsState},
        sidebar::{hit_test as sidebar_hit_test, SidebarHit},
        ssh_manager::{generate_ssh_manager_commands, SshManagerState},
    },
    window::{Window, WindowEvent},
    workspace::{WorkspaceSlot, DEFAULT_WORKSPACE_NAME},
};

const HIBERNATE_SECS: u64 = 300;

// ── Modal stack ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
enum Modal {
    Settings,
    SshManager,
    AgentLauncher,
    /// Ctrl+K fuzzy action launcher.
    CommandPalette,
    /// Quit confirmation — shown when closing with busy sessions.
    ConfirmClose,
}

// ── Render cache ──────────────────────────────────────────────────────────────

/// Caches the most recent agent blocks so we don't clone them every frame.
/// Keyed by the session's UUID — tab indices shift when tabs close, so an
/// index would let a new tab inherit a stale cache.
struct AgentBlockCache {
    session_id: Option<libterm::block::store::SessionId>,
    change_counter: u64,
    blocks: Vec<CommandBlock>,
}

impl AgentBlockCache {
    fn new() -> Self {
        Self { session_id: None, change_counter: 0, blocks: Vec::new() }
    }
    fn get_or_update(
        &mut self,
        session_id: libterm::block::store::SessionId,
        store: &libterm::block::store::BlockStore,
    ) -> &[CommandBlock] {
        if self.session_id != Some(session_id) || self.change_counter != store.change_counter() {
            self.session_id = Some(session_id);
            self.change_counter = store.change_counter();
            self.blocks = store.clone_recent(20);
        }
        &self.blocks
    }
}

// ── Drag state ────────────────────────────────────────────────────────────────

struct DragState {
    origin_x: f32,
    start_ratio: f32,
    content_w: f32,
}

// ── AppState ──────────────────────────────────────────────────────────────────

pub struct AppState {
    // Platform
    window: Window,
    compositor: Compositor,
    event_rx: mpsc::Receiver<WindowEvent>,
    pty_wake: windows::Win32::Foundation::HANDLE,
    rt: tokio::runtime::Runtime,

    // Session management
    /// Working copies of the active workspace's sessions.
    entries: Vec<Entry>,
    active_tab: usize,
    /// All workspace slots (sessions stored here when not the active workspace).
    workspace_slots: Vec<WorkspaceSlot>,
    active_workspace: usize,

    // Window state
    window_w: u32,
    window_h: u32,
    sidebar_vis: bool,
    split_ratio: Option<f32>,
    drag: Option<DragState>,
    running: bool,
    ui_dirty: bool,
    snapshot_dirty: bool,
    window_shown: bool,

    // Mouse state for hover tracking
    mouse_x: i32,
    mouse_y: i32,
    /// Block-view scroll-back offset in pixels for the active session (0 =
    /// pinned to the newest block). Reset on tab switch / new command.
    block_scroll: f32,

    /// Set when handle_key consumes a Ctrl shortcut: the shortcut's WM_CHAR
    /// control character still follows and must not reach the PTY (otherwise
    /// e.g. Ctrl+T echoes ^T into the shell).
    swallow_char: bool,

    /// Worker → run-loop channel for completed font downloads
    /// (Ok(family) to apply, Err logged).
    font_tx: mpsc::Sender<anyhow::Result<String>>,
    font_rx: mpsc::Receiver<anyhow::Result<String>>,

    // UI overlays
    modals: Vec<Modal>,
    palette: CommandPaletteState,
    /// Per-session anchored input editors (Warp-style), keyed by session id.
    editors: std::collections::HashMap<libterm::block::store::SessionId, InputEditor>,
    ssh_mgr: SshManagerState,
    agent_launcher: AgentLauncherState,
    settings: SettingsState,
    host_store: HostStore,

    // Render cache: avoids cloning block history every frame when unchanged
    agent_block_cache: AgentBlockCache,

    // IPC
    ipc_sessions: ipc::SessionList,
    ipc_rx: mpsc::Receiver<IpcCommand>,

    // Animation engine
    anim: AnimationEngine,

    // Hit testing
    block_rtree: rstar::RTree<BlockHitTarget>,

    // Persisted config
    shell: String,
    init_cols: u16,
    init_rows: u16,
}

impl AppState {
    pub fn new(
        window: Window,
        compositor: Compositor,
        event_rx: mpsc::Receiver<WindowEvent>,
        pty_wake: windows::Win32::Foundation::HANDLE,
        rt: tokio::runtime::Runtime,
        shell: String,
        init_cols: u16,
        init_rows: u16,
        initial_entries: Vec<Entry>,
        initial_active: usize,
        window_w: u32,
        window_h: u32,
    ) -> Self {
        // Start with a single workspace; more are created on demand. The
        // restored sessions live in the working copy (`entries`), so slot 0's
        // `entries` stays empty until the user switches away from it.
        let mut workspace_slots: Vec<WorkspaceSlot> =
            vec![WorkspaceSlot::new(DEFAULT_WORKSPACE_NAME)];
        workspace_slots[0].active_tab = initial_active;

        let ipc_sessions = ipc::new_session_list();
        let ipc_rx = ipc::start(ipc_sessions.clone());
        let host_store = HostStore::load().unwrap_or_default();
        let ssh_mgr = SshManagerState::new(host_store.hosts.clone());
        let agent_launcher = AgentLauncherState::new();
        let app_config = Config::load_or_default(&Config::default_path());
        let settings = SettingsState::new(app_config);

        let (font_tx, font_rx) = mpsc::channel();

        // Sidebar starts visible: seed its animated offset at full width so
        // the first frame doesn't render it collapsed (value() defaults to 0).
        let mut anim = AnimationEngine::new();
        anim.set_value(AnimationTarget::SidebarOffset, SIDEBAR_W);

        Self {
            window,
            compositor,
            event_rx,
            pty_wake,
            rt,
            entries: initial_entries,
            active_tab: initial_active,
            workspace_slots,
            active_workspace: 0,
            window_w,
            window_h,
            sidebar_vis: true,
            split_ratio: None,
            drag: None,
            running: true,
            ui_dirty: true,
            snapshot_dirty: true,
            window_shown: false,
            mouse_x: -1,
            mouse_y: -1,
            block_scroll: 0.0,
            swallow_char: false,
            font_tx,
            font_rx,
            modals: Vec::new(),
            palette: CommandPaletteState::new(),
            editors: std::collections::HashMap::new(),
            ssh_mgr,
            agent_launcher,
            settings,
            host_store,
            agent_block_cache: AgentBlockCache::new(),
            ipc_sessions,
            ipc_rx,
            anim,
            block_rtree: build_block_rtree(Vec::new()),
            shell,
            init_cols,
            init_rows,
        }
    }

    // ── Main loop ─────────────────────────────────────────────────────────────

    pub fn run(&mut self) -> Result<()> {
        let _guard = self.rt.handle().enter();

        while self.running {
            self.drain_pty();
            self.drain_ipc();
            self.drain_font_downloads();
            self.tick_hibernation();
            self.anim.tick();
            drain_win32_messages();
            self.handle_events()?;

            if self.snapshot_dirty {
                snapshot::persist(&self.entries, self.active_tab, &self.shell.clone());
                self.snapshot_dirty = false;
            }

            self.render()?;
            self.wait();
        }

        // Persist final state, then exit without unwinding: teardown
        // deadlocks otherwise — ClosePseudoConsole blocks while the output
        // pipe has unread data, and tokio's Runtime::drop waits forever on
        // the spawn_blocking PTY readers stuck in ReadFile.
        snapshot::persist(&self.entries, self.active_tab, &self.shell.clone());
        std::process::exit(0);
    }

    // ── PTY drain ─────────────────────────────────────────────────────────────

    fn drain_pty(&mut self) {
        for entry in &mut self.entries {
            entry.drain_pty();
        }
    }

    // ── IPC drain ─────────────────────────────────────────────────────────────

    fn drain_ipc(&mut self) {
        while let Ok(cmd) = self.ipc_rx.try_recv() {
            self.ui_dirty = true;
            match cmd {
                IpcCommand::Send { session, text } => {
                    if let Some(e) = self.entries.iter_mut().find(|e| e.session.id == session) {
                        e.record_input();
                        let _ = e.pty.write(text.as_bytes());
                    }
                }
                IpcCommand::Kill { session } => {
                    if let Some(pos) = self.entries.iter().position(|e| e.session.id == session) {
                        self.entries.remove(pos);
                        self.active_tab = self.active_tab.min(self.entries.len().saturating_sub(1));
                        self.snapshot_dirty = true;
                    }
                }
                IpcCommand::Hibernate { session } => {
                    if let Some(e) = self.entries.iter_mut().find(|e| e.session.id == session) {
                        e.hibernate();
                    }
                }
                IpcCommand::Wake { session } => {
                    if let Some(e) = self.entries.iter_mut().find(|e| e.session.id == session) {
                        e.wake();
                    }
                }
            }
        }
    }

    // ── Hibernation ───────────────────────────────────────────────────────────

    fn tick_hibernation(&mut self) {
        for (i, entry) in self.entries.iter_mut().enumerate() {
            if i == self.active_tab { continue; }
            if entry.hibernated { continue; }
            if !matches!(entry.session.kind, SessionKind::Agent { .. }) { continue; }
            if entry.last_activity.elapsed().as_secs() >= HIBERNATE_SECS {
                entry.hibernate();
                self.ui_dirty = true;
            }
        }
    }

    // ── Window events ─────────────────────────────────────────────────────────

    fn handle_events(&mut self) -> Result<()> {
        let (pre_tab_count, pre_active) = (self.entries.len(), self.active_tab);

        while let Ok(event) = self.event_rx.try_recv() {
            self.ui_dirty = true;
            match event {
                WindowEvent::Close => self.request_close(),
                WindowEvent::Paint => {
                    // Cheap path: re-blit the last frame; fall back to a full
                    // redraw if no frame has been rendered yet.
                    if !self.compositor.present() {
                        self.ui_dirty = true;
                    }
                }
                WindowEvent::Char(cu) => self.handle_char(cu),
                WindowEvent::KeyDown { vk, ctrl } => self.handle_key(vk, ctrl)?,
                WindowEvent::Resize { width, height } => {
                    self.window_w = width;
                    self.window_h = height;
                    self.compositor.resize(width, height)?;
                }
                WindowEvent::DpiChanged { dpi, w, h } => {
                    let fam = self.settings.config.font.family.clone();
                    let size = self.settings.config.font.size;
                    self.compositor.rebuild_atlas(&fam, size, dpi as f32)?;
                    self.window_w = w as u32;
                    self.window_h = h as u32;
                    self.compositor.resize(self.window_w, self.window_h)?;
                    let (cw, ch) = self.compositor.cell_size();
                    let ct = ChromeLayout::compute(
                        self.window_w as f32,
                        self.window_h as f32,
                        self.sidebar_vis,
                    )
                    .content;
                    for e in &mut self.entries {
                        e.resize_pty(ct.w, ct.h - PANE_HEADER_H, cw, ch);
                    }
                }
                WindowEvent::LButtonDown { x, y } => self.handle_lbutton_down(x, y),
                WindowEvent::MouseMove { x, y } => {
                    // Hover highlights live in the chrome; repaint only when
                    // the cursor is over chrome (session bar / sidebar /
                    // statusbar), not on every move across the terminal.
                    let over_chrome = |px: i32, py: i32| {
                        (py as f32) < SESSION_BAR_H
                            || (py as f32) > self.window_h as f32 - crate::ui::layout::STATUSBAR_H
                            || (self.sidebar_vis && (px as f32) < SIDEBAR_W)
                    };
                    let moved = self.mouse_x != x || self.mouse_y != y;
                    // Also repaint on the move that *leaves* chrome so stale
                    // hover highlights are cleared.
                    if moved && (over_chrome(x, y) || over_chrome(self.mouse_x, self.mouse_y)) {
                        self.ui_dirty = true;
                    }
                    self.mouse_x = x;
                    self.mouse_y = y;
                    if let Some(ref ds) = self.drag {
                        let delta = x as f32 - ds.origin_x;
                        self.split_ratio = Some(
                            (ds.start_ratio + delta / ds.content_w).clamp(0.1, 0.9),
                        );
                    }
                }
                WindowEvent::LButtonUp => {
                    self.drag = None;
                }
                WindowEvent::MouseWheel { delta } => {
                    // Scroll back through the block-view history of the active
                    // session (positive delta = wheel up = older blocks).
                    let (_, ch) = self.compositor.cell_size();
                    self.block_scroll = (self.block_scroll + delta as f32 * ch as f32 * 3.0).max(0.0);
                    self.ui_dirty = true;
                }
            }
        }

        if self.entries.len() != pre_tab_count || self.active_tab != pre_active {
            self.snapshot_dirty = true;
        }
        Ok(())
    }

    // ── Character input ───────────────────────────────────────────────────────

    fn handle_char(&mut self, cu: u16) {
        let c = char::from_u32(cu as u32).unwrap_or('\0');

        // Drop the control char generated by a Ctrl shortcut handle_key just
        // consumed (e.g. 0x14 after Ctrl+T).  Guarded on is_control so a
        // shortcut that produces no WM_CHAR can't swallow a real keystroke.
        if self.swallow_char {
            self.swallow_char = false;
            if c.is_control() {
                return;
            }
        }

        match self.modals.last() {
            Some(Modal::Settings) => {
                self.settings.handle_char(c);
                return;
            }
            Some(Modal::SshManager) => {
                if let Some(host) = self.ssh_mgr.handle_char(c) {
                    self.modals.pop();
                    self.host_store.hosts = self.ssh_mgr.hosts.clone();
                    let _ = self.host_store.save();
                    let auth = ssh_auth_for(&host);
                    match self.rt.block_on(Entry::spawn_ssh(
                        host.hostname.clone(),
                        host.port,
                        host.username.clone(),
                        auth,
                    )) {
                        Ok(e) => {
                            self.entries.push(e);
                            self.active_tab = self.entries.len() - 1;
                        }
                        Err(e) => tracing::error!("SSH connect: {e}"),
                    }
                }
                return;
            }
            Some(Modal::AgentLauncher) => {
                if let Some((cmd, model)) = self.agent_launcher.handle_char(c) {
                    self.modals.pop();
                    match Entry::spawn_agent(&cmd, &model) {
                        Ok(e) => {
                            self.entries.push(e);
                            self.active_tab = self.entries.len() - 1;
                        }
                        Err(e) => tracing::error!("agent spawn: {e}"),
                    }
                }
                return;
            }
            Some(Modal::CommandPalette) => {
                if let Some(action) = self.palette.handle_char(c) {
                    self.modals.pop();
                    self.palette.reset();
                    self.run_palette_action(action);
                }
                return;
            }
            Some(Modal::ConfirmClose) => {
                match c.to_ascii_lowercase() {
                    'y' => self.running = false,
                    'n' => { self.modals.pop(); }
                    _ => {}
                }
                return;
            }
            None => {}
        }

        // Anchored input editor: at an idle integration-detected prompt the
        // app owns the keystroke instead of the PTY.
        if self.editor_active() {
            let (id, block_count) = {
                let e = &self.entries[self.active_tab];
                (e.session.id, e.session.blocks.all().len())
            };
            let ed = self.editors.get_mut(&id).expect("editor_active inserted it");
            let searching = matches!(ed.mode, crate::ui::input_editor::EditorMode::Search { .. });
            let effect = if searching {
                match c {
                    '\r' | '\n' => {
                        // Accept the match and run it (bash behaviour).
                        ed.end_search(true);
                        ed.submit()
                    }
                    '\t' => {
                        // Accept into the buffer for further editing.
                        ed.end_search(true);
                        EditorEffect::None
                    }
                    '\x1b' => {
                        ed.end_search(false);
                        EditorEffect::None
                    }
                    '\x08' | '\x7f' => {
                        ed.search_backspace();
                        EditorEffect::None
                    }
                    _ if !c.is_control() => {
                        ed.search_push(c);
                        EditorEffect::None
                    }
                    _ => EditorEffect::None,
                }
            } else {
                match c {
                    '\r' | '\n' => ed.submit(),
                    '\t' => ed.flush_for_completion(block_count),
                    '\x03' => ed.interrupt(),
                    '\x08' | '\x7f' => {
                        ed.backspace();
                        EditorEffect::None
                    }
                    _ if !c.is_control() => {
                        ed.insert(c);
                        EditorEffect::None
                    }
                    // Other control chars are swallowed at the prompt — the
                    // shell hasn't seen the draft, so forwarding would desync.
                    _ => EditorEffect::None,
                }
            };
            self.ui_dirty = true;
            if let EditorEffect::Send(bytes) = effect {
                if let Some(e) = self.entries.get_mut(self.active_tab) {
                    e.record_input();
                    // A submitted command ends in CR — hand the exact text to the
                    // detector so the block shows what the user typed, not the
                    // PSReadLine-repainted echo.
                    if bytes.last() == Some(&b'\r') {
                        if let Ok(s) = std::str::from_utf8(&bytes[..bytes.len() - 1]) {
                            let cmd = s.trim();
                            if !cmd.is_empty() {
                                e.vt_parser.detector_mut().set_next_command(cmd);
                            }
                        }
                    }
                    let _ = e.pty.write(&bytes);
                }
            }
            return;
        }

        if let Some(bytes) = char_to_pty_bytes(cu) {
            if let Some(e) = self.entries.get_mut(self.active_tab) {
                e.record_input();
                let _ = e.pty.write(&bytes);
            }
        }
    }

    // ── Keyboard shortcuts ────────────────────────────────────────────────────

    fn handle_key(&mut self, vk: u32, ctrl: bool) -> Result<()> {
        match self.modals.last() {
            Some(Modal::Settings) => {
                // Ctrl+, toggles settings, so it also closes them.
                if ctrl && vk == 0xBC {
                    self.modals.pop();
                    self.settings.save_if_dirty();
                    return Ok(());
                }
                if self.settings.handle_key(vk, ctrl) {
                    self.modals.pop();
                    self.settings.save_if_dirty();
                }
                return Ok(());
            }
            Some(Modal::AgentLauncher) => {
                if ((ctrl && vk == 0x41) || vk == 0x1B)
                    && self.agent_launcher.handle_escape() {
                        self.modals.pop();
                    }
                return Ok(());
            }
            Some(Modal::SshManager) => {
                match vk {
                    // Ctrl+H toggles the manager, so it also closes it —
                    // keeps the open/close key symmetric.
                    0x48 if ctrl => {
                        self.modals.pop();
                        return Ok(());
                    }
                    0x26 => self.ssh_mgr.handle_key_up(),
                    0x28 => self.ssh_mgr.handle_key_down(),
                    0x1B => {
                        if self.ssh_mgr.handle_escape() {
                            self.modals.pop();
                        }
                    }
                    0x44
                        if self.ssh_mgr.handle_delete() => {
                            self.host_store.hosts = self.ssh_mgr.hosts.clone();
                            let _ = self.host_store.save();
                        }
                    _ => {}
                }
                return Ok(());
            }
            Some(Modal::CommandPalette) => {
                match vk {
                    0x26 => self.palette.move_selection(-1), // Up
                    0x28 => self.palette.move_selection(1),  // Down
                    0x1B => {
                        self.modals.pop();
                        self.palette.reset();
                    }
                    _ => {}
                }
                return Ok(());
            }
            Some(Modal::ConfirmClose) => {
                match vk {
                    0x0D => self.running = false, // Enter — quit
                    0x1B => { self.modals.pop(); } // Esc — cancel
                    _ => {}
                }
                return Ok(());
            }
            None => {}
        }

        // Ctrl+R at an idle prompt opens reverse history search in the input
        // editor (and cycles to older matches when already searching).
        if ctrl && vk == 0x52 && self.editor_active() {
            let id = self.entries[self.active_tab].session.id;
            if let Some(ed) = self.editors.get_mut(&id) {
                ed.start_search();
            }
            self.swallow_char = true;
            self.ui_dirty = true;
            return Ok(());
        }

        if ctrl {
            let shell = self.shell.clone();
            // Assume an arm below consumes this as a shortcut, so its WM_CHAR
            // control char gets swallowed; cleared after the match for keys
            // that fall through to the PTY (Ctrl+C etc.).
            self.swallow_char = true;
            match vk {
                 0x42 => {
                      self.sidebar_vis = !self.sidebar_vis;
                      // Animate from the current offset so toggling mid-flight
                      // reverses smoothly instead of jumping.
                      let target = AnimationTarget::SidebarOffset;
                      let from = self.anim.value(&target);
                      let to = if self.sidebar_vis { SIDEBAR_W } else { 0.0 };
                      self.anim.animate(target, from, to, 200, Easing::EaseOutQuad);
                      return Ok(());
                  }
                 0x4B => {
                      // Ctrl+K — command palette (universal search: actions
                      // + open tabs + workspaces + saved SSH hosts)
                      let items = self.build_palette_items();
                      self.palette.set_items(items);
                      self.push_modal(Modal::CommandPalette);
                      return Ok(());
                  }
                 0x48 => {
                      if self.modals.last() == Some(&Modal::SshManager) {
                          self.modals.pop();
                      } else {
                          self.modals.push(Modal::SshManager);
                      }
                      return Ok(());
                  }
                 0x41 => {
                      if self.modals.last() == Some(&Modal::AgentLauncher) {
                          self.modals.pop();
                      } else {
                          self.modals.push(Modal::AgentLauncher);
                      }
                      return Ok(());
                  }
                 0xBC => {
                      if self.modals.last() == Some(&Modal::Settings) {
                          self.modals.pop();
                          self.settings.save_if_dirty();
                      } else {
                          self.modals.push(Modal::Settings);
                      }
                      return Ok(());
                  }
                 0x54 => {
                     let (ic, ir) = (self.init_cols, self.init_rows);
                     self.spawn_tab(&shell, ic, ir);
                     self.active_tab = self.entries.len() - 1;
                     return Ok(());
                 }
                 0x57 => { self.close_tab(); return Ok(()); }
                 0x09 => {
                     self.active_tab = (self.active_tab + 1) % self.entries.len().max(1);
                     if let Some(e) = self.entries.get_mut(self.active_tab) { e.wake(); }
                     return Ok(());
                 }
                 0x50 => {
                     self.split_ratio = match self.split_ratio { None => Some(0.5), Some(_) => None };
                     return Ok(());
                 }
                 n @ 0x31..=0x39 => {
                     let idx = (n - 0x31) as usize;
                     if idx < self.entries.len() {
                         self.active_tab = idx;
                     }
                     return Ok(());
                 }
                 _ => {}
            }
            // Fell through: not an app shortcut, so the key (and its WM_CHAR)
            // belongs to the terminal.
            self.swallow_char = false;
        }

        // Anchored input editor: navigation keys edit the app-owned buffer
        // instead of emitting escape sequences at the shell.
        if !ctrl && self.editor_active() {
            let id = self.entries[self.active_tab].session.id;
            if let Some(ed) = self.editors.get_mut(&id) {
                // Reverse-search consumes navigation keys itself; Esc is
                // handled via WM_CHAR. Just swallow arrows here so they don't
                // emit escape sequences mid-search.
                if matches!(ed.mode, crate::ui::input_editor::EditorMode::Search { .. })
                    && matches!(vk, 0x25 | 0x26 | 0x27 | 0x28 | 0x23 | 0x24 | 0x2E) {
                        return Ok(());
                    }
                let handled = match vk {
                    0x25 => { ed.move_left(); true }    // ←
                    0x27 => { ed.move_right(); true }   // →
                    0x26 => { ed.history_prev(); true } // ↑
                    0x28 => { ed.history_next(); true } // ↓
                    0x24 => { ed.home(); true }         // Home
                    0x23 => { ed.end(); true }          // End
                    0x2E => { ed.delete(); true }       // Delete
                    _ => false,
                };
                if handled {
                    self.ui_dirty = true;
                    return Ok(());
                }
            }
        }

        if let Some(bytes) = vk_to_pty_bytes(vk, ctrl) {
            if let Some(e) = self.entries.get_mut(self.active_tab) {
                e.record_input();
                let _ = e.pty.write(&bytes);
            }
        }
        Ok(())
    }

    /// Assemble the palette item list: fixed actions plus the current tabs,
    /// workspaces, and saved SSH hosts.
    fn build_palette_items(&self) -> Vec<crate::ui::command_palette::PaletteEntry> {
        use crate::ui::command_palette::{base_entries, PaletteEntry};
        let mut items = base_entries();
        let labels = chrome::display_labels(self.entries.iter().map(|e| e.session.title.as_str()));
        for (i, label) in labels.iter().enumerate() {
            items.push(PaletteEntry {
                label: format!("Go to tab: {label}"),
                hint: "tab".into(),
                action: PaletteAction::SwitchTab(i),
            });
        }
        for (i, ws) in self.workspace_slots.iter().enumerate() {
            items.push(PaletteEntry {
                label: format!("Workspace: {}", ws.name),
                hint: "workspace".into(),
                action: PaletteAction::SwitchWorkspace(i),
            });
        }
        for (i, host) in self.host_store.hosts.iter().enumerate() {
            items.push(PaletteEntry {
                label: format!("Connect: {}", host.display_label()),
                hint: "ssh host".into(),
                action: PaletteAction::ConnectSsh(i),
            });
        }
        items.extend(crate::ui::command_palette::font_entries(crate::fonts::FONT_CATALOG));
        items
    }

    /// Execute a command-palette action (palette modal already popped).
    fn run_palette_action(&mut self, action: PaletteAction) {
        match action {
            PaletteAction::NewTab => {
                let shell = self.shell.clone();
                let (ic, ir) = (self.init_cols, self.init_rows);
                self.spawn_tab(&shell, ic, ir);
                self.active_tab = self.entries.len() - 1;
            }
            PaletteAction::CloseTab => self.close_tab(),
            PaletteAction::NextTab => {
                self.active_tab = (self.active_tab + 1) % self.entries.len().max(1);
                if let Some(e) = self.entries.get_mut(self.active_tab) { e.wake(); }
            }
            PaletteAction::ToggleSidebar => {
                self.sidebar_vis = !self.sidebar_vis;
                let target = AnimationTarget::SidebarOffset;
                let from = self.anim.value(&target);
                let to = if self.sidebar_vis { SIDEBAR_W } else { 0.0 };
                self.anim.animate(target, from, to, 200, Easing::EaseOutQuad);
            }
            PaletteAction::ToggleSplit => {
                self.split_ratio =
                    match self.split_ratio { None => Some(0.5), Some(_) => None };
            }
            PaletteAction::NewWorkspace => self.add_workspace(),
            PaletteAction::SshManager => self.push_modal(Modal::SshManager),
            PaletteAction::AgentLauncher => self.push_modal(Modal::AgentLauncher),
            PaletteAction::Settings => self.push_modal(Modal::Settings),
            PaletteAction::Quit => self.request_close(),
            PaletteAction::SwitchTab(i) => {
                if i < self.entries.len() {
                    self.active_tab = i;
                    if let Some(e) = self.entries.get_mut(i) { e.wake(); }
                }
            }
            PaletteAction::SwitchWorkspace(i) => {
                if i < self.workspace_slots.len() {
                    self.switch_workspace(i);
                }
            }
            PaletteAction::DownloadFont(i) => {
                // Download on a worker thread; the run loop applies the
                // family when the result arrives on the font channel.
                let tx = self.font_tx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(crate::fonts::download_and_register(i));
                });
            }
            PaletteAction::ConnectSsh(i) => {
                let Some(host) = self.host_store.hosts.get(i).cloned() else { return };
                let auth = ssh_auth_for(&host);
                match self.rt.block_on(Entry::spawn_ssh(
                    host.hostname.clone(),
                    host.port,
                    host.username.clone(),
                    auth,
                )) {
                    Ok(e) => {
                        self.entries.push(e);
                        self.active_tab = self.entries.len() - 1;
                    }
                    Err(e) => tracing::error!("SSH connect (palette): {e}"),
                }
            }
        }
    }

    /// Apply fonts downloaded by palette-triggered worker threads: persist
    /// the family to config, rebuild the GDI fonts, and resize PTYs to the
    /// new cell metrics.
    fn drain_font_downloads(&mut self) {
        while let Ok(res) = self.font_rx.try_recv() {
            match res {
                Ok(family) => {
                    tracing::info!("font ready: {family}");
                    self.settings.config.font.family = family.clone();
                    self.settings.dirty = true;
                    self.settings.save_if_dirty();
                    let size = self.settings.config.font.size;
                    let dpi = self.window.dpi as f32;
                    if let Err(e) = self.compositor.rebuild_atlas(&family, size, dpi) {
                        tracing::error!("font apply: {e}");
                        continue;
                    }
                    let (cw, ch) = self.compositor.cell_size();
                    let ct = ChromeLayout::compute(
                        self.window_w as f32,
                        self.window_h as f32,
                        self.sidebar_vis,
                    )
                    .content;
                    for e in &mut self.entries {
                        e.resize_pty(ct.w, ct.h - PANE_HEADER_H, cw, ch);
                    }
                    self.ui_dirty = true;
                }
                Err(e) => tracing::error!("font download: {e}"),
            }
        }
    }

    /// True when the active session's shell is idle at an integration-
    /// detected prompt, so the anchored input editor owns keystrokes.
    /// Sessions without OSC 133 shell integration never produce blocks and
    /// therefore keep classic raw-PTY behaviour.
    fn editor_active(&mut self) -> bool {
        let Some(e) = self.entries.get(self.active_tab) else { return false };
        let blocks = e.session.blocks.all();
        let Some(last) = blocks.last() else { return false };
        if matches!(last.status, libterm::block::store::BlockStatus::Running) {
            return false;
        }
        let count = blocks.len();
        let id = e.session.id;
        let ed = self.editors.entry(id).or_insert_with(InputEditor::new);
        ed.maybe_rearm(count);
        ed.passthrough_since.is_none()
    }

    /// Push a modal unless it is already on the stack.
    fn push_modal(&mut self, m: Modal) {
        if !self.modals.contains(&m) {
            self.modals.push(m);
        }
    }

    /// Sessions that would lose work if the app quit now: anything with a
    /// running command, plus all SSH and agent sessions (across workspaces).
    fn busy_session_count(&self) -> usize {
        let busy = |e: &Entry| {
            !matches!(e.session.kind, SessionKind::Local)
                || e.session
                    .blocks
                    .all()
                    .last()
                    .is_some_and(|b| matches!(b.status, libterm::block::store::BlockStatus::Running))
        };
        self.entries.iter().filter(|e| busy(e)).count()
            + self
                .workspace_slots
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != self.active_workspace)
                .flat_map(|(_, ws)| ws.entries.iter())
                .filter(|e| busy(e))
                .count()
    }

    /// Close the app, or ask first when sessions are busy or several tabs
    /// are open (Windows Terminal convention).
    fn request_close(&mut self) {
        if self.busy_session_count() > 0 || self.entries.len() > 1 {
            self.push_modal(Modal::ConfirmClose);
        } else {
            self.running = false;
        }
    }

    // ── Mouse clicks ──────────────────────────────────────────────────────────

    fn handle_lbutton_down(&mut self, x: i32, y: i32) {
        // While a modal is open it owns all input — don't let clicks reach
        // the chrome / tabs / sidebar underneath the dim overlay.
        if !self.modals.is_empty() {
            // Settings is mouse-interactive: nav categories, field rows,
            // and backdrop-to-close.
            if self.modals.last() == Some(&Modal::Settings) {
                let (_, ch) = self.compositor.cell_size();
                use crate::ui::settings::{hit_test as settings_hit, SettingsHit};
                match settings_hit(
                    &self.settings,
                    self.window_w as f32,
                    self.window_h as f32,
                    ch as f32,
                    x as f32,
                    y as f32,
                ) {
                    SettingsHit::Category(i) => {
                        self.settings.category = crate::ui::settings::SettingsCategory::all()[i];
                        self.settings.selected_field = 0;
                        self.settings.scroll_offset = 0;
                    }
                    SettingsHit::Field(i) => {
                        if self.settings.selected_field == i {
                            // Second click on the selected row toggles/edits.
                            self.settings.toggle_current();
                        } else {
                            self.settings.selected_field = i;
                        }
                    }
                    SettingsHit::Outside => {
                        self.modals.pop();
                        self.settings.save_if_dirty();
                    }
                    SettingsHit::Panel => {}
                }
                self.ui_dirty = true;
            }
            return;
        }

        // Caption buttons (right zone of session bar, Windows convention:
        // minimize · maximize · close, close rightmost). The zone is three
        // equal full-bleed 46px backplates, so the index is a clean division.
        let zone_x = self.window_w as f32 - crate::ui::layout::CAPTION_ZONE_W;
        if (y as f32) < SESSION_BAR_H && (x as f32) >= zone_x {
            let rel = x as f32 - zone_x;
            let btn = ((rel / crate::ui::layout::CAPTION_BTN_W) as i32).clamp(0, 2);
            match btn {
                0 => {
                    unsafe {
                        use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_MINIMIZE};
                        let _ = ShowWindow(self.window.hwnd, SW_MINIMIZE);
                    }
                }
                1 => {
                    unsafe {
                        use windows::Win32::UI::WindowsAndMessaging::{
                            IsZoomed, ShowWindow, SW_MAXIMIZE, SW_RESTORE,
                        };
                        let sw = if IsZoomed(self.window.hwnd).as_bool() {
                            SW_RESTORE
                        } else {
                            SW_MAXIMIZE
                        };
                        let _ = ShowWindow(self.window.hwnd, sw);
                    }
                }
                _ => self.request_close(),
            }
            return;
        }

        let layout = ChromeLayout::compute(
            self.window_w as f32,
            self.window_h as f32,
            self.sidebar_vis,
        );
        let (cw, ch) = self.compositor.cell_size();

        // Sidebar
        if let Some(sb) = layout.sidebar {
            if sb.contains(x as f32, y as f32) {
                match sidebar_hit_test(sb, x as f32, y as f32, ch as f32, self.workspace_slots.len()) {
                    Some(SidebarHit::Workspace(ws_idx)) => {
                        self.switch_workspace(ws_idx);
                    }
                    Some(SidebarHit::NewWorkspace) => {
                        self.add_workspace();
                    }
                    Some(SidebarHit::SshManager) => { self.push_modal(Modal::SshManager); }
                    Some(SidebarHit::AgentRuns) => { self.push_modal(Modal::AgentLauncher); }
                    Some(SidebarHit::KeyVault) | None => {}
                }
            }
        }

        // Block overlay click → copy output to clipboard
        let pt = [x as f32, y as f32];
        let env = rstar::AABB::from_point(pt);
        if let Some(target) = self.block_rtree.locate_in_envelope_intersecting(&env).next() {
            if let Some(block) = self.entries
                .iter()
                .flat_map(|e| e.session.blocks.all())
                .find(|b| b.id == target.block_id)
            {
                copy_to_clipboard(&block.output_as_str());
            }
        }

        // Session bar click (tabs from the left edge; gear left of the
        // caption zone; empty space starts an OS window drag)
        if layout.session_bar.contains(x as f32, y as f32) && (x as f32) < zone_x {
            let xf = x as f32;
            let ucw = self.compositor.ui_char_w();
            let mut hit = false;

            // Same disambiguated labels the renderer uses, so widths match.
            let labels =
                chrome::display_labels(self.entries.iter().map(|e| e.session.title.as_str()));
            let mut tx = layout.session_bar.x + crate::ui::layout::TAB_PAD_LEFT;
            for (i, label) in labels.iter().enumerate() {
                let tw = chrome::tab_width(label, ucw);
                if xf >= tx && xf < tx + tw {
                    // The close × sits at the tab's right edge; clicks there
                    // close the tab instead of selecting it.
                    if xf >= tx + tw - ucw - 14.0 {
                        self.close_tab_at(i);
                    } else {
                        self.active_tab = i;
                        if let Some(e) = self.entries.get_mut(i) { e.wake(); }
                    }
                    hit = true;
                    break;
                }
                tx += tw + 1.0;
            }
            if hit {
                return;
            }

            // "+" new-tab button (right after the last tab)
            if xf >= tx + 4.0 && xf < tx + 28.0 {
                let shell = self.shell.clone();
                let (ic, ir) = (self.init_cols, self.init_rows);
                self.spawn_tab(&shell, ic, ir);
                self.active_tab = self.entries.len() - 1;
                return;
            }

            // Gear ⚙ — opens settings
            let gear_x = zone_x - 32.0;
            if xf >= gear_x && xf < gear_x + 28.0 {
                self.push_modal(Modal::Settings);
                return;
            }

            // Empty bar space: hand off to the OS caption drag loop so the
            // frameless window can still be moved.
            unsafe {
                use windows::Win32::Foundation::{LPARAM, WPARAM};
                use windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;
                use windows::Win32::UI::WindowsAndMessaging::{
                    SendMessageW, HTCAPTION, WM_NCLBUTTONDOWN,
                };
                let _ = ReleaseCapture();
                SendMessageW(
                    self.window.hwnd,
                    WM_NCLBUTTONDOWN,
                    WPARAM(HTCAPTION as usize),
                    LPARAM(0),
                );
            }
            return;
        }

        // Split drag
        if let Some(ratio) = self.split_ratio {
            let ct = layout.content;
            let hx = ct.x + ct.w * ratio;
            if (x as f32 - hx).abs() <= SPLIT_HANDLE_W + 4.0 {
                self.drag = Some(DragState {
                    origin_x: x as f32,
                    start_ratio: ratio,
                    content_w: ct.w,
                });
            }
        }
        let _ = (cw, ch); // suppress unused warning if cell_size not used below
    }

    // ── Workspace switching ───────────────────────────────────────────────────

    /// Create a new, empty workspace and switch to it. The new workspace gets a
    /// fresh shell on first activation (handled by `switch_workspace`).
    fn add_workspace(&mut self) {
        let name = format!("workspace {}", self.workspace_slots.len() + 1);
        self.workspace_slots.push(WorkspaceSlot::new(name));
        let new_idx = self.workspace_slots.len() - 1;
        self.switch_workspace(new_idx);
    }

    fn switch_workspace(&mut self, ws_idx: usize) {
        if ws_idx == self.active_workspace { return; }

        // Save current workspace.
        self.workspace_slots[self.active_workspace].entries =
            std::mem::take(&mut self.entries);
        self.workspace_slots[self.active_workspace].active_tab = self.active_tab;

        // Load target workspace.
        self.active_workspace = ws_idx;
        self.entries = std::mem::take(&mut self.workspace_slots[ws_idx].entries);
        self.active_tab = self.workspace_slots[ws_idx].active_tab;

        // Auto-spawn a shell if this workspace has no sessions yet.
        if self.entries.is_empty() {
            let shell = self.shell.clone();
            let (ic, ir) = (self.init_cols, self.init_rows);
            self.spawn_tab(&shell, ic, ir);
            self.active_tab = 0;
        }

        self.snapshot_dirty = true;
    }

    // ── Render ────────────────────────────────────────────────────────────────

    fn render(&mut self) -> Result<()> {
        // The animated offset is the single source of truth for sidebar width:
        // seeded to SIDEBAR_W at startup, animated to 0 / SIDEBAR_W on toggle.
        let anim_sb_offset = self.anim.value(&AnimationTarget::SidebarOffset);
        let layout = ChromeLayout::compute_animated(
            self.window_w as f32,
            self.window_h as f32,
            self.sidebar_vis,
            anim_sb_offset,
        );
        // Inset the content area so each pane reads as a floating rounded card
        // (cmux/Warp style) with a gutter of terminal-background around it.
        let ct = layout.content.inset(renderer_windows::tokens::PANE_GUTTER);
        let term_h = ct.h - PANE_HEADER_H;
        let term_y = ct.y + PANE_HEADER_H;
        let (cw, ch) = self.compositor.cell_size();

        // Compute pane layout and resize PTYs as needed.
        let (pane_layout, render_idx, pane_rects) =
            self.compute_pane_layout(ct, term_h, cw, ch);

        // Early-out when nothing changed: everything below (session clones,
        // GDI text measurement, chrome command building, RTree rebuild) is
        // per-frame work that would otherwise run even on idle frames.
        self.ui_dirty |= self.compositor.cursor_blink_due();
        let any_dirty = render_idx.iter().any(|&i| self.entries[i].session.is_dirty())
            || self.ui_dirty
            || self.anim.is_busy()
            || !self.modals.is_empty();
        if !any_dirty {
            return Ok(());
        }

        let handle_xs = pane_layout.split_handle_xs(ct.x, ct.y, ct.w, ct.h);
        let chrome_sessions: Vec<Session> =
            self.entries.iter().map(|e| e.session.clone_for_render()).collect();
        let agent_blocks = self.agent_blocks();
        let ui_char_w = self.compositor.ui_char_w();

        // Per-tab last command exit codes (read from live entries, not render clones).
        let tab_exit_codes: Vec<Option<i32>> = self.entries.iter()
            .map(|e| e.session.blocks.all().last().and_then(|b| b.exit_code))
            .collect();

        // Per-session "agent awaiting input" flag — latest block carries an
        // OSC awaiting-status notification.  Drives the pane notification ring.
        let awaiting: Vec<bool> = self.entries.iter()
            .map(|e| e.session.blocks.all().last().is_some_and(|b| {
                matches!(b.notification, Some(libterm::vt::sequences::OscNotification::StatusAwaiting))
            }))
            .collect();

        // Shell/session name shown in the command bar.
        let active_shell_name: String = if let Some(e) = self.entries.get(self.active_tab) {
            match &e.session.kind {
                // self.shell may be a full command line (integration args);
                // show just the executable name.
                SessionKind::Local => self
                    .shell
                    .split_whitespace()
                    .next()
                    .unwrap_or(&self.shell)
                    .to_string(),
                SessionKind::Ssh { .. } => e.session.title.clone(),
                SessionKind::Agent { name, .. } => name.clone(),
            }
        } else {
            String::new()
        };

        // Workspace names for the sidebar (dynamic; created on demand).
        let workspace_names: Vec<String> =
            self.workspace_slots.iter().map(|w| w.name.clone()).collect();
        // Session count per workspace: the active workspace's sessions live in
        // `self.entries`; inactive ones are parked in their slot.
        let workspace_counts: Vec<usize> = self
            .workspace_slots
            .iter()
            .enumerate()
            .map(|(i, w)| if i == self.active_workspace { self.entries.len() } else { w.entries.len() })
            .collect();

        // Working directory of the active session (OSC 7), shown in command bar.
        let active_cwd: Option<String> = self
            .entries
            .get(self.active_tab)
            .and_then(|e| e.session.cwd.clone());

        // Build chrome commands.
        let mut chrome_cmds = chrome::generate_commands(&ChromeState {
            layout: &layout,
            sessions: &chrome_sessions,
            active_tab: self.active_tab,
            active_pane_session_idx: self.active_tab,
            tab_exit_codes: &tab_exit_codes,
            awaiting: &awaiting,
            active_shell_name: &active_shell_name,
            active_cwd: active_cwd.as_deref(),
            workspace_names: &workspace_names,
            workspace_counts: &workspace_counts,
            active_workspace: self.active_workspace,
            split_handles: &handle_xs,
            cell_h: ch,
            agent_blocks: &agent_blocks,
            pane_rects: &pane_rects,
            ui_char_w,
            mouse_pos: (self.mouse_x as f32, self.mouse_y as f32),
        });

        // Keep keyboard scrolling in sync with what the renderer can show.
        self.settings.visible_rows = SettingsState::compute_visible_rows(
            self.window_h as f32,
            ch as f32,
            self.settings.searching,
        );

        // Overlay: modals (only visible when on the stack)
        let busy_count = if self.modals.contains(&Modal::ConfirmClose) {
            self.busy_session_count()
        } else {
            0
        };
        for m in &self.modals {
            match m {
                Modal::SshManager => {
                    chrome_cmds.extend(generate_ssh_manager_commands(
                        &self.ssh_mgr,
                        self.window_w as f32,
                        self.window_h as f32,
                        cw,
                        ch,
                    ));
                }
                Modal::AgentLauncher => {
                    chrome_cmds.extend(generate_agent_launcher_commands(
                        &self.agent_launcher,
                        self.window_w as f32,
                        self.window_h as f32,
                        cw,
                        ch,
                    ));
                }
                Modal::Settings => {
                    chrome_cmds.extend(generate_settings_commands(
                        &self.settings,
                        self.window_w as f32,
                        self.window_h as f32,
                        cw,
                        ch,
                        ui_char_w,
                    ));
                }
                Modal::CommandPalette => {
                    chrome_cmds.extend(generate_command_palette_commands(
                        &self.palette,
                        self.window_w as f32,
                        self.window_h as f32,
                        ch as f32,
                        ui_char_w,
                    ));
                }
                Modal::ConfirmClose => {
                    chrome_cmds.extend(chrome::generate_confirm_close_commands(
                        self.window_w as f32,
                        self.window_h as f32,
                        busy_count,
                        ch as f32,
                    ));
                }
            }
        }

        // Welcome overlay: shown until the shell sends its first output byte.
        if let Some(entry) = self.entries.get(self.active_tab) {
            if entry.fresh {
                chrome_cmds.extend(chrome::generate_init_overlay(
                    ct.x,
                    term_y,
                    ct.w,
                    term_h,
                    cw as f32,
                    ch as f32,
                    ui_char_w,
                ));
            }
        }

        // Command blocks are first-class *data* (they drive the sidebar's
        // RECENT BLOCKS list and the tab/pane status), but we deliberately do
        // NOT draw them as floating cards over the pane: the live terminal grid
        // already renders every command and its output, so an overlay just
        // duplicated that content inside a heavy border.  No pane overlay → no
        // block hit targets.
        self.block_rtree = build_block_rtree(Vec::new());

        // ── Block view ────────────────────────────────────────────────────────
        // Render the captured command blocks as a scrollable history *instead*
        // of the raw grid, whenever a session has integration-produced blocks
        // and is not on the alternate screen.  Interactive full-screen apps
        // (vim, htop, less, ssh TUIs) switch to the alt screen, so they fall
        // back to the raw terminal automatically.
        let editor_on = self.modals.is_empty() && self.editor_active();
        let bar_h = input_editor::bar_height(ch as f32);
        for &(px, py, pw, ph, session_idx) in &pane_rects {
            let Some(entry) = self.entries.get(session_idx) else { continue };
            if entry.session.grid.is_alt_screen() || entry.session.blocks.is_empty() {
                continue; // raw grid shows through
            }
            let is_active = session_idx == self.active_tab;
            let reserve = if is_active && editor_on { bar_h } else { 0.0 };
            let bx = px + 1.0;
            let by = py + PANE_HEADER_H;
            let bw = pw - 2.0;
            let bh = (ph - PANE_HEADER_H - 1.0 - reserve).max(0.0);
            let blocks = entry.session.blocks.all();
            if is_active {
                let ms = block_view::max_scroll(blocks, bh, ch as f32);
                self.block_scroll = self.block_scroll.clamp(0.0, ms);
            }
            let scroll = if is_active { self.block_scroll } else { 0.0 };
            chrome_cmds.extend(block_view::generate_block_view(
                blocks, bx, by, bw, bh, cw as f32, ch as f32, ui_char_w, scroll,
            ));
        }

        // Anchored input editor bar — bottom of the active pane, only at an
        // idle integration-detected prompt and with no modal on top.
        if self.modals.is_empty() && self.editor_active() {
            if let Some(&(px, py, pw, ph, _)) = pane_rects
                .iter()
                .find(|&&(_, _, _, _, si)| si == self.active_tab)
            {
                let e = &self.entries[self.active_tab];
                let accent = match &e.session.kind {
                    SessionKind::Local => renderer_windows::tokens::COLOR_LOCAL,
                    SessionKind::Ssh { .. } => renderer_windows::tokens::COLOR_SSH,
                    SessionKind::Agent { .. } => renderer_windows::tokens::COLOR_AGENT,
                };
                if let Some(ed) = self.editors.get(&e.session.id) {
                    let bar_h = input_editor::bar_height(ch as f32);
                    chrome_cmds.extend(input_editor::generate_input_bar(
                        ed,
                        px,
                        py + ph - bar_h,
                        pw,
                        cw as f32,
                        ch as f32,
                        accent,
                    ));
                }
            }
        }

        // Update IPC session list.
        if let Ok(mut list) = self.ipc_sessions.lock() {
            *list = self.entries.iter().map(|e| SessionInfo {
                id: e.session.id.to_string(),
                kind: match &e.session.kind {
                    SessionKind::Local => "local".into(),
                    SessionKind::Ssh { .. } => "ssh".into(),
                    SessionKind::Agent { .. } => "agent".into(),
                },
                title: e.session.title.clone(),
                hibernated: e.hibernated,
            }).collect();
        }

        let render_sessions: Vec<Session> =
            render_idx.iter().map(|&i| self.entries[i].session.clone_for_render()).collect();
        self.compositor.render_frame_with_chrome(
            &render_sessions,
            &pane_layout,
            &chrome_cmds,
            self.window_w,
            self.window_h,
            Some((ct.x, term_y, ct.w, term_h)),
        )?;
        self.ui_dirty = false;
        if !self.window_shown {
            self.window.show();
            self.window_shown = true;
        }
        Ok(())
    }

    // ── Pane layout helper ────────────────────────────────────────────────────

    fn compute_pane_layout(
        &mut self,
        ct: Rect,
        term_h: f32,
        cw: u32,
        ch: u32,
    ) -> (PaneLayout, Vec<usize>, Vec<(f32, f32, f32, f32, usize)>) {
        match self.split_ratio {
            None => {
                if let Some(e) = self.entries.get_mut(self.active_tab) {
                    e.resize_pty(ct.w, term_h, cw, ch);
                }
                if self.entries.is_empty() {
                    return (PaneLayout::leaf(uuid::Uuid::nil()), Vec::new(), Vec::new());
                }
                let id = self.entries[self.active_tab].session.id;
                let pane_rects = vec![(ct.x, ct.y, ct.w, ct.h, self.active_tab)];
                (PaneLayout::leaf(id), vec![self.active_tab], pane_rects)
            }
            Some(ratio) => {
                let l = self.active_tab;
                let r = if self.entries.len() > 1 {
                    (self.active_tab + 1) % self.entries.len()
                } else {
                    self.active_tab
                };
                if let Some(e) = self.entries.get_mut(l) {
                    e.resize_pty(ct.w * ratio, term_h, cw, ch);
                }
                if l != r {
                    if let Some(e) = self.entries.get_mut(r) {
                        e.resize_pty(ct.w * (1.0 - ratio), term_h, cw, ch);
                    }
                }
                let pl = PaneLayout::hsplit(
                    PaneLayout::leaf(self.entries[l].session.id),
                    PaneLayout::leaf(self.entries[r].session.id),
                    ratio,
                );
                let lw = ct.w * ratio - SPLIT_HANDLE_W * 0.5;
                let rw = ct.w * (1.0 - ratio) - SPLIT_HANDLE_W * 0.5;
                let pane_rects = vec![
                    (ct.x, ct.y, lw, ct.h, l),
                    (ct.x + ct.w * ratio + SPLIT_HANDLE_W * 0.5, ct.y, rw, ct.h, r),
                ];
                (pl, vec![l, r], pane_rects)
            }
        }
    }

    // ── Wait for next event ───────────────────────────────────────────────────

    fn wait(&self) {
        unsafe {
            use windows::Win32::UI::WindowsAndMessaging::{
                MsgWaitForMultipleObjectsEx, MWMO_INPUTAVAILABLE, QS_ALLINPUT,
            };
            let handles = [self.pty_wake];
            MsgWaitForMultipleObjectsEx(Some(&handles), 16, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
        }
    }

    // ── Small helpers ─────────────────────────────────────────────────────────

    fn spawn_tab(&mut self, shell: &str, cols: u16, rows: u16) {
        match Entry::spawn_local(shell, cols, rows) {
            Ok(e) => self.entries.push(e),
            Err(e) => tracing::error!("spawn tab: {e}"),
        }
    }

    fn close_tab(&mut self) {
        self.close_tab_at(self.active_tab);
    }

    fn close_tab_at(&mut self, idx: usize) {
        if self.entries.len() > 1 && idx < self.entries.len() {
            self.entries.remove(idx);
            if self.active_tab >= idx {
                self.active_tab = self.active_tab.saturating_sub(1).min(self.entries.len() - 1);
            }
            self.snapshot_dirty = true;
            self.ui_dirty = true;
        }
    }

    fn agent_blocks(&mut self) -> Vec<CommandBlock> {
        if let Some(e) = self.entries.get(self.active_tab) {
            if matches!(e.session.kind, SessionKind::Agent { .. }) {
                return self.agent_block_cache
                    .get_or_update(e.session.id, &e.session.blocks)
                    .to_vec();
            }
        }
        Vec::new()
    }

}

// ── Process-level helpers ─────────────────────────────────────────────────────

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
            let _ = SetClipboardData(13, HANDLE(hmem.0)); // CF_UNICODETEXT = 13
            let _ = CloseClipboard();
        }
    }
}
