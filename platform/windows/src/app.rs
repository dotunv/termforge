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
use renderer_dx12::compositor::Compositor;

use crate::{
    entry::{ssh_auth_for, Entry},
    input::{char_to_pty_bytes, vk_to_pty_bytes},
    ipc::{self, IpcCommand, SessionInfo},
    snapshot,
    ui::{
        agent_launcher::{generate_agent_launcher_commands, AgentLauncherState},
        block_overlay::{build_block_rtree, generate_block_overlays, BlockHitTarget},
        chrome,
        layout::{ChromeLayout, ChromeState, PANE_HEADER_H, SPLIT_HANDLE_W},
        settings::{generate_settings_commands, SettingsState},
        sidebar::{hit_test as sidebar_hit_test, SidebarHit},
        ssh_manager::{generate_ssh_manager_commands, SshManagerState},
    },
    window::{Window, WindowEvent},
    workspace::{WorkspaceSlot, WORKSPACE_NAMES},
};

const HIBERNATE_SECS: u64 = 300;

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

    // UI overlays
    ssh_mgr: SshManagerState,
    agent_launcher: AgentLauncherState,
    settings: SettingsState,
    host_store: HostStore,

    // IPC
    ipc_sessions: ipc::SessionList,
    ipc_rx: mpsc::Receiver<IpcCommand>,

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
        // Build four workspace slots; put the restored sessions in slot 0.
        let mut workspace_slots: Vec<WorkspaceSlot> = WORKSPACE_NAMES
            .iter()
            .map(|_| WorkspaceSlot::empty())
            .collect();
        workspace_slots[0].active_tab = initial_active;
        // The working copy starts as slot 0's sessions.
        // workspace_slots[0].entries stays empty; we hold them in `entries`.

        let ipc_sessions = ipc::new_session_list();
        let ipc_rx = ipc::start(ipc_sessions.clone());
        let host_store = HostStore::load().unwrap_or_default();
        let ssh_mgr = SshManagerState::new(host_store.hosts.clone());
        let agent_launcher = AgentLauncherState::new();
        let app_config = Config::load_or_default(&Config::default_path());
        let settings = SettingsState::new(app_config);

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
            ssh_mgr,
            agent_launcher,
            settings,
            host_store,
            ipc_sessions,
            ipc_rx,
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
            self.tick_hibernation();
            drain_win32_messages();
            self.handle_events()?;

            if self.snapshot_dirty {
                snapshot::persist(&self.entries, self.active_tab, &self.shell.clone());
                self.snapshot_dirty = false;
            }

            self.render()?;
            self.wait();
        }
        Ok(())
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
                WindowEvent::Close => self.running = false,
                WindowEvent::Char(cu) => self.handle_char(cu),
                WindowEvent::KeyDown { vk, ctrl } => self.handle_key(vk, ctrl)?,
                WindowEvent::Resize { width, height } => {
                    self.window_w = width;
                    self.window_h = height;
                    self.compositor.resize(width, height)?;
                }
                WindowEvent::DpiChanged { dpi, w, h } => {
                    self.compositor.rebuild_atlas("Cascadia Code", 13.0, dpi as f32)?;
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
                WindowEvent::MouseMove { x, .. } => {
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

        if self.settings.open {
            self.settings.handle_char(c);
            return;
        }
        if self.ssh_mgr.open {
            if let Some(host) = self.ssh_mgr.handle_char(c) {
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
                        self.ssh_mgr.open = false;
                    }
                    Err(e) => tracing::error!("SSH connect: {e}"),
                }
            }
            return;
        }
        if self.agent_launcher.open {
            if let Some((cmd, model)) = self.agent_launcher.handle_char(c) {
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
        if let Some(bytes) = char_to_pty_bytes(cu) {
            if let Some(e) = self.entries.get_mut(self.active_tab) {
                e.record_input();
                let _ = e.pty.write(&bytes);
            }
        }
    }

    // ── Keyboard shortcuts ────────────────────────────────────────────────────

    fn handle_key(&mut self, vk: u32, ctrl: bool) -> Result<()> {
        if self.settings.open {
            self.settings.handle_key(vk);
            if !self.settings.open {
                self.settings.save_if_dirty();
            }
            return Ok(());
        }
        if self.agent_launcher.open {
            if vk == 0x1B { self.agent_launcher.handle_escape(); }
            return Ok(());
        }
        if self.ssh_mgr.open {
            match vk {
                0x26 => self.ssh_mgr.handle_key_up(),
                0x28 => self.ssh_mgr.handle_key_down(),
                0x1B => self.ssh_mgr.handle_escape(),
                0x44 => {
                    if self.ssh_mgr.handle_delete() {
                        self.host_store.hosts = self.ssh_mgr.hosts.clone();
                        let _ = self.host_store.save();
                    }
                }
                _ => {}
            }
            return Ok(());
        }

        if ctrl {
            let shell = self.shell.clone();
            match vk {
                0xDC => { self.sidebar_vis = !self.sidebar_vis; return Ok(()); }
                0x48 => { self.ssh_mgr.open = !self.ssh_mgr.open; return Ok(()); }
                0x41 => { self.agent_launcher.open = !self.agent_launcher.open; return Ok(()); }
                0xBC => { self.settings.open = !self.settings.open; return Ok(()); }
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
        }

        if let Some(bytes) = vk_to_pty_bytes(vk, ctrl) {
            if let Some(e) = self.entries.get_mut(self.active_tab) {
                e.record_input();
                let _ = e.pty.write(&bytes);
            }
        }
        Ok(())
    }

    // ── Mouse clicks ──────────────────────────────────────────────────────────

    fn handle_lbutton_down(&mut self, x: i32, y: i32) {
        // Traffic lights (left zone of session bar)
        if y < 40 && x < 52 {
            if x < 25 {
                self.running = false;
            } else if x < 41 {
                unsafe {
                    use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_MINIMIZE};
                    let _ = ShowWindow(self.window.hwnd, SW_MINIMIZE);
                }
            } else {
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
                match sidebar_hit_test(sb, x as f32, y as f32, self.entries.len(), ch as f32, WORKSPACE_NAMES.len()) {
                    Some(SidebarHit::Session(i)) => {
                        self.active_tab = i;
                        if let Some(e) = self.entries.get_mut(i) { e.wake(); }
                    }
                    Some(SidebarHit::Workspace(ws_idx)) => {
                        self.switch_workspace(ws_idx);
                    }
                    Some(SidebarHit::SshManager) => { self.ssh_mgr.open = true; }
                    Some(SidebarHit::AgentRuns) => { self.agent_launcher.open = true; }
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
                copy_to_clipboard(&block.output_as_str().into_owned());
            }
        }

        // Session bar click (tab area — right of traffic-light zone)
        if layout.session_bar.contains(x as f32, y as f32) && x >= 52 {
            let ucw = self.compositor.ui_char_w();
            let mut tx = layout.session_bar.x + crate::ui::layout::TL_ZONE_W + 4.0;
            for (i, e) in self.entries.iter().enumerate() {
                let tw = chrome::tab_width(&e.session.title, ucw);
                if (x as f32) >= tx && (x as f32) < tx + tw {
                    self.active_tab = i;
                    break;
                }
                tx += tw + 1.0;
            }
            if let Some(e) = self.entries.get_mut(self.active_tab) { e.wake(); }
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
        let layout = ChromeLayout::compute(
            self.window_w as f32,
            self.window_h as f32,
            self.sidebar_vis,
        );
        let ct = layout.content;
        let term_h = ct.h - PANE_HEADER_H;
        let term_y = ct.y + PANE_HEADER_H;
        let (cw, ch) = self.compositor.cell_size();

        // Compute pane layout and resize PTYs as needed.
        let (pane_layout, render_idx, pane_rects) =
            self.compute_pane_layout(&layout, term_h, cw, ch);

        let handle_xs = pane_layout.split_handle_xs(ct.x, ct.y, ct.w, ct.h);
        let chrome_sessions: Vec<Session> =
            self.entries.iter().map(|e| e.session.clone_for_render()).collect();
        let agent_blocks = self.agent_blocks();
        let error_count = self.count_errors();
        let awaiting_count = self.count_awaiting();
        let ui_char_w = self.compositor.ui_char_w();

        // Per-tab last command exit codes (read from live entries, not render clones).
        let tab_exit_codes: Vec<Option<i32>> = self.entries.iter()
            .map(|e| e.session.blocks.all().last().and_then(|b| b.exit_code))
            .collect();

        // Shell/session name shown in the command bar.
        let active_shell_name: String = if let Some(e) = self.entries.get(self.active_tab) {
            match &e.session.kind {
                SessionKind::Local      => self.shell.clone(),
                SessionKind::Ssh { .. } => e.session.title.clone(),
                SessionKind::Agent { name, .. } => name.clone(),
            }
        } else {
            String::new()
        };

        // Build chrome commands.
        let mut chrome_cmds = chrome::generate_commands(&ChromeState {
            layout: &layout,
            sessions: &chrome_sessions,
            active_tab: self.active_tab,
            active_pane_session_idx: self.active_tab,
            tab_exit_codes: &tab_exit_codes,
            active_shell_name: &active_shell_name,
            active_cwd: None, // CWD via OSC 7 not yet wired
            workspace_names: &WORKSPACE_NAMES,
            active_workspace: self.active_workspace,
            sidebar_visible: self.sidebar_vis,
            split_handles: &handle_xs,
            error_count,
            awaiting_count,
            cell_w: cw,
            cell_h: ch,
            agent_blocks: &agent_blocks,
            pane_rects: &pane_rects,
            ui_char_w,
        });

        // Overlay: SSH manager
        chrome_cmds.extend(generate_ssh_manager_commands(
            &self.ssh_mgr,
            self.window_w as f32,
            self.window_h as f32,
            cw,
            ch,
        ));
        // Overlay: agent launcher
        chrome_cmds.extend(generate_agent_launcher_commands(
            &self.agent_launcher,
            self.window_w as f32,
            self.window_h as f32,
            cw,
            ch,
        ));
        // Overlay: settings
        chrome_cmds.extend(generate_settings_commands(
            &self.settings,
            self.window_w as f32,
            self.window_h as f32,
            cw,
            ch,
        ));

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

        // Block overlays + RTree
        let mut all_hits: Vec<BlockHitTarget> = Vec::new();
        for &(px, py, pw, _ph, session_idx) in &pane_rects {
            if let Some(entry) = self.entries.get(session_idx) {
                let blocks = entry.session.blocks.all();
                if !blocks.is_empty() {
                    let grid_rows = entry.session.grid.rows();
                    let (overlay_cmds, hits) = generate_block_overlays(
                        blocks,
                        px,
                        py + PANE_HEADER_H,
                        pw,
                        cw as f32,
                        ch as f32,
                        grid_rows,
                    );
                    chrome_cmds.extend(overlay_cmds);
                    all_hits.extend(hits);
                }
            }
        }
        self.block_rtree = build_block_rtree(all_hits);

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

        // Only render when dirty.
        self.ui_dirty |= self.compositor.cursor_blink_due();
        let any_dirty = render_idx.iter().any(|&i| self.entries[i].session.is_dirty())
            || self.ui_dirty
            || self.ssh_mgr.open
            || self.agent_launcher.open
            || self.settings.open;

        if any_dirty {
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
        }
        Ok(())
    }

    // ── Pane layout helper ────────────────────────────────────────────────────

    fn compute_pane_layout(
        &mut self,
        layout: &ChromeLayout,
        term_h: f32,
        cw: u32,
        ch: u32,
    ) -> (PaneLayout, Vec<usize>, Vec<(f32, f32, f32, f32, usize)>) {
        let ct = layout.content;

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
        if self.entries.len() > 1 {
            self.entries.remove(self.active_tab);
            self.active_tab = self.active_tab.min(self.entries.len() - 1);
        }
    }

    fn agent_blocks(&self) -> Vec<CommandBlock> {
        if let Some(e) = self.entries.get(self.active_tab) {
            if matches!(e.session.kind, SessionKind::Agent { .. }) {
                return e.session.blocks.clone_recent(20);
            }
        }
        Vec::new()
    }

    fn count_errors(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| {
                e.session.blocks.all().last()
                    .and_then(|b| b.exit_code)
                    .map(|code| code != 0)
                    .unwrap_or(false)
            })
            .count()
    }

    fn count_awaiting(&self) -> usize {
        use libterm::vt::sequences::OscNotification;
        self.entries
            .iter()
            .flat_map(|e| e.session.blocks.all())
            .filter(|b| {
                matches!(b.notification, Some(OscNotification::StatusAwaiting))
            })
            .count()
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
