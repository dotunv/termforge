//! The terminal view: owns one session and turns GPUI input into terminal
//! input, selections and searches.
//!
//! Everything terminal-specific (emulation, key and mouse encoding, text
//! selection, search, block tracking, colours) lives in framework-free
//! crates; this file only wires those onto GPUI events. Painting is in
//! `paint.rs`.

use std::path::PathBuf;
use std::sync::Arc;

use futures::StreamExt;
use gpui::{
    canvas, div, prelude::*, px, App, Bounds, ClickEvent, ClipboardItem, Context, CursorStyle,
    DispatchPhase, ElementId, Entity, FocusHandle, Hitbox, HitboxBehavior, KeyDownEvent, Modifiers,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, ScrollWheelEvent,
    SharedString, Subscription, Task, Window,
};
use tf_engine::text::{find_all, is_openable, link_at, Link, Match};
use tf_engine::{
    GridSize, MouseTracking, Scroll, Selection, SelectionKind, Snapshot, TerminalEngine,
};
use tf_input::{
    encode_focus, encode_key, encode_mouse, encode_paste, InputModes, Key, Mods, MouseAction,
    MouseButton as TMouse, Tracking,
};
use tf_session::{SessionEvent, SpawnOptions};
use tf_ui::{tokens, Rgb, Theme, ThemeInput};

use crate::fonts::{detect_ui_family, MonoFont};
use crate::paint::{hsla, paint_grid, BlockMark, Metrics, Overlay, PaintArgs};
use crate::remote_session::{self, Attach, SessionHandle};
use crate::workspace::{self, Layout, SavedTab};

/// Search results beyond this are not collected, so a one-letter query in
/// a full scrollback cannot stall the UI.
const MAX_MATCHES: usize = 5_000;

#[derive(Debug, Default)]
struct SearchState {
    query: String,
    case_sensitive: bool,
    matches: Vec<Match>,
    current: Option<usize>,
    /// New output arrived since `matches` was computed.
    stale: bool,
}

#[derive(Debug, Default)]
struct PaletteState {
    query: String,
    selected: usize,
}

#[derive(Debug, Clone)]
enum PaletteAction {
    NewTab,
    CloseTab,
    NextTab,
    PreviousTab,
    RestartSession,
    Quit,
    Find,
    Copy,
    Paste,
    FontIncrease,
    FontDecrease,
    FontReset,
    ScrollTop,
    ScrollBottom,
    CopyDiagnostics,
    ToggleSidebar,
    OpenSettings,
    NewShell(usize),
    SetDefaultShell(usize),
    NextAttention,
    RenameWorkspace,
    TogglePin,
    MoveUp,
    MoveDown,
    ListTasks,
    StatusWorking,
    StatusDone,
    StatusClear,
    DaemonDoctor,
    ConnectSsh(String),
    SwitchLocal,
}

const PALETTE_COMMANDS: &[(PaletteAction, &str, &str)] = &[
    (
        PaletteAction::NewTab,
        "Tabs: new local terminal",
        "Ctrl Shift T",
    ),
    (
        PaletteAction::CloseTab,
        "Tabs: close current tab",
        "Ctrl Shift W",
    ),
    (PaletteAction::NextTab, "Tabs: select next tab", "Ctrl Tab"),
    (
        PaletteAction::PreviousTab,
        "Tabs: select previous tab",
        "Ctrl Shift Tab",
    ),
    (PaletteAction::RestartSession, "Session: restart", ""),
    (PaletteAction::Find, "Find in terminal", "Ctrl Shift F"),
    (PaletteAction::Copy, "Copy selection", "Ctrl Shift C"),
    (PaletteAction::Paste, "Paste", "Ctrl Shift V"),
    (PaletteAction::FontIncrease, "Increase font size", "Ctrl +"),
    (PaletteAction::FontDecrease, "Decrease font size", "Ctrl -"),
    (PaletteAction::FontReset, "Reset font size", "Ctrl 0"),
    (PaletteAction::ScrollTop, "Scroll to top", "Ctrl Shift Home"),
    (
        PaletteAction::ScrollBottom,
        "Scroll to bottom",
        "Ctrl Shift End",
    ),
    (PaletteAction::CopyDiagnostics, "Copy diagnostics", ""),
    (
        PaletteAction::NextAttention,
        "Workspace: jump to one needing attention",
        "Ctrl Shift U",
    ),
    (
        PaletteAction::RenameWorkspace,
        "Workspace: rename",
        "Ctrl Shift R",
    ),
    (
        PaletteAction::TogglePin,
        "Workspace: pin or unpin",
        "Ctrl Shift K",
    ),
    (PaletteAction::MoveUp, "Workspace: move up", "Ctrl Shift Up"),
    (
        PaletteAction::MoveDown,
        "Workspace: move down",
        "Ctrl Shift Down",
    ),
    (
        PaletteAction::ToggleSidebar,
        "View: toggle sidebar",
        "Ctrl Shift B",
    ),
    (PaletteAction::OpenSettings, "Settings: open", "Ctrl ,"),
    (PaletteAction::ListTasks, "Tasks: list project tasks", ""),
    (PaletteAction::StatusWorking, "Status: mark working", ""),
    (PaletteAction::StatusDone, "Status: mark done", ""),
    (PaletteAction::StatusClear, "Status: clear", ""),
    (PaletteAction::DaemonDoctor, "Tools: daemon diagnostics", ""),
    (
        PaletteAction::Quit,
        "Application: quit TermForge",
        "Ctrl Shift Q",
    ),
];

#[derive(Debug, Clone)]
struct WorkspaceTab {
    id: u64,
    session: Option<Arc<SessionHandle>>,
    spawn: SpawnOptions,
    project_root: Option<PathBuf>,
    cwd: Option<String>,
    title: Option<String>,
    last_exit: Option<i32>,
    commands: usize,
    error: Option<String>,
    history: usize,
    font_size: f32,
    scroll_offset: usize,
    /// User-chosen name; overrides the terminal title.
    name: Option<String>,
    pinned: bool,
    branch: Option<String>,
    attention: Option<Attention>,
    attention_note: Option<String>,
}

/// A mouse press forwarded to the program (mouse tracking on).
#[derive(Debug, Clone, Copy)]
struct Reported {
    button: TMouse,
    last: (u16, u16),
}

/// Drag state for tab reordering.
#[derive(Debug, Default)]
struct DragState {
    dragged_index: Option<usize>,
    drag_start: Option<gpui::Point<Pixels>>,
    is_dragging: bool,
}

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// A section-header action: element id, glyph, handler.
type SectionAction = (&'static str, &'static str, ClickHandler);

/// Why a background workspace wants the user's attention, most urgent last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Attention {
    Notice,
    Done,
    Error,
    Blocked,
}

impl Attention {
    fn color(self, t: &Theme) -> Rgb {
        match self {
            Self::Notice => t.accent,
            Self::Done => t.success,
            Self::Error => t.danger,
            Self::Blocked => t.warning,
        }
    }
}

/// Inline rename of a workspace row.
#[derive(Debug)]
struct RenameState {
    tab_id: u64,
    text: String,
}

/// Payload while a workspace row is dragged to reorder it.
#[derive(Clone)]
struct DraggedWorkspace {
    index: usize,
    title: SharedString,
    bg: Rgb,
    fg: Rgb,
    border: Rgb,
}

struct DragPreview(DraggedWorkspace);

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        div()
            .px(px(tokens::space::MD))
            .py(px(tokens::space::SM))
            .rounded(px(tokens::radius::MD))
            .bg(hsla(self.0.bg))
            .border_1()
            .border_color(hsla(self.0.border))
            .text_color(hsla(self.0.fg))
            .child(self.0.title.clone())
    }
}

/// Collapsible sidebar sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SidebarSection {
    Workspaces,
    SshHosts,
    Project,
}

/// Pages of the settings panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum SettingsPage {
    #[default]
    General,
    Appearance,
    Keybindings,
    Advanced,
}

const SETTINGS_PAGES: &[(SettingsPage, &str)] = &[
    (SettingsPage::General, "General"),
    (SettingsPage::Appearance, "Appearance"),
    (SettingsPage::Keybindings, "Keybindings"),
    (SettingsPage::Advanced, "Advanced"),
];

/// Sidebar widths offered in settings.
const SIDEBAR_WIDTHS: &[(f32, &str)] = &[(200.0, "Narrow"), (248.0, "Normal"), (300.0, "Wide")];

pub struct TerminalView {
    session: Option<Arc<SessionHandle>>,
    spawn: SpawnOptions,
    local_spawn: SpawnOptions,
    project_root: Option<PathBuf>,
    local_project_root: Option<PathBuf>,
    ssh_hosts: Vec<String>,
    focus: FocusHandle,
    theme: Arc<Theme>,
    font: MonoFont,
    /// Proportional family for interface chrome.
    ui_font: SharedString,
    font_size: f32,
    scroll_accum: f32,
    cwd: Option<String>,
    title: Option<String>,
    last_exit: Option<i32>,
    commands: usize,
    error: Option<String>,

    selection: Option<Selection>,
    selecting: bool,
    reported: Option<Reported>,
    last_motion: Option<(u16, u16)>,
    hover_link: Option<Link>,
    search: Option<SearchState>,
    palette: Option<PaletteState>,
    tabs: Vec<WorkspaceTab>,
    active_tab: usize,
    next_tab_id: u64,
    wake: tf_session::Waker,
    /// Scrollback length at the last output, to notice clears.
    history: usize,
    /// Canvas bounds from the last paint, for mouse hit-testing.
    grid_bounds: Bounds<Pixels>,
    /// Actual grid size computed from canvas bounds.
    computed_grid_size: Option<GridSize>,
    /// Drag state for tab reordering.
    drag_state: DragState,

    // Sidebar and settings state
    collapsed_sections: std::collections::HashSet<SidebarSection>,
    show_sidebar: bool,
    /// Reveal the hidden sidebar as an overlay when the pointer hits the left edge.
    sidebar_hover: bool,
    sidebar_peek: bool,
    sidebar_width: f32,
    show_tab_strip: bool,
    show_status_bar: bool,
    settings: Option<SettingsPage>,
    renaming: Option<RenameState>,
    /// Shells found on this machine, for the picker and settings.
    shells: Vec<tf_pty::ShellProfile>,
    shell_menu: bool,
    /// Last layout handed to the daemon, to skip redundant saves.
    saved_layout: String,

    _pump: Option<Task<()>>,
    _focus_subs: Vec<Subscription>,
}

impl TerminalView {
    pub fn new(
        spawn: SpawnOptions,
        project_root: Option<PathBuf>,
        theme: Arc<Theme>,
        font: MonoFont,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        let subs = vec![
            cx.on_focus_in(&focus, window, |this, _, cx| this.focus_changed(true, cx)),
            cx.on_focus_out(&focus, window, |this, _, _, cx| {
                this.focus_changed(false, cx);
            }),
        ];
        let local_spawn = if spawn.ssh_host.is_none() {
            spawn.clone()
        } else {
            let mut local = spawn.clone();
            local.ssh_host = None;
            if let Some(profile) = tf_pty::discover_shells().into_iter().next() {
                local.profile = profile;
            }
            local.cwd = directories::UserDirs::new().map(|dirs| dirs.home_dir().to_path_buf());
            local
        };
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();
        let wake: tf_session::Waker = Arc::new(move || {
            let _ = tx.unbounded_send(());
        });
        let mut view = Self {
            session: None,
            local_spawn,
            spawn,
            local_project_root: project_root.clone(),
            project_root,
            ssh_hosts: tf_pty::discover_ssh_hosts(),
            focus,
            theme,
            ui_font: detect_ui_family(cx, &font.family),
            font,
            font_size: 14.0,
            scroll_accum: 0.0,
            cwd: None,
            title: None,
            last_exit: None,
            commands: 0,
            error: None,
            selection: None,
            selecting: false,
            reported: None,
            last_motion: None,
            hover_link: None,
            search: None,
            palette: None,
            tabs: Vec::new(),
            active_tab: 0,
            next_tab_id: 1,
            wake,
            history: 0,
            grid_bounds: Bounds::default(),
            computed_grid_size: None,
            drag_state: DragState::default(),
            collapsed_sections: std::collections::HashSet::new(),
            show_sidebar: true,
            sidebar_hover: true,
            sidebar_peek: false,
            sidebar_width: 248.0,
            show_tab_strip: false,
            show_status_bar: true,
            settings: None,
            renaming: None,
            shells: tf_pty::discover_shells(),
            shell_menu: false,
            saved_layout: String::new(),
            _pump: None,
            _focus_subs: subs,
        };
        view.restore_workspace(cx);
        view._pump = Some(cx.spawn(async move |this, cx| {
            while rx.next().await.is_some() {
                while rx.try_recv().is_ok() {}
                if this.update(cx, |view, cx| view.on_output(cx)).is_err() {
                    break;
                }
            }
        }));
        view
    }

    fn start_session(&mut self, reattach: bool) {
        self.start_session_as(if reattach {
            Attach::Matching
        } else {
            Attach::New
        });
    }

    fn start_session_as(&mut self, attach: Attach) {
        match SessionHandle::spawn(
            self.spawn.clone(),
            self.project_root.clone(),
            attach,
            Arc::clone(&self.wake),
        ) {
            Ok(session) => {
                self.session = Some(Arc::new(session));
                self.error = None;
                self.last_exit = None;
                self.commands = 0;
            }
            Err(e) => {
                tracing::error!("failed to start shell: {e:#}");
                self.error = Some(format!("{e:#}"));
                self.session = None;
            }
        }
        self.selection = None;
        self.cwd = None;
        self.title = None;
        self.search = None;
        self.palette = None;
        self.history = 0;
    }

    fn start(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(session) = &self.session {
            if let Err(error) = session.close() {
                tracing::warn!(%error, "failed to close session before restart");
            }
        }
        self.session = None;
        self.start_session(true);
        self.sync_active_tab();
        self.on_output(cx);
        self.persist_layout();
    }

    fn current_tab(&mut self) -> WorkspaceTab {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let branch = self.discover_branch();
        WorkspaceTab {
            id,
            session: self.session.clone(),
            spawn: self.spawn.clone(),
            project_root: self.project_root.clone(),
            cwd: self.cwd.clone(),
            title: self.title.clone(),
            last_exit: self.last_exit,
            commands: self.commands,
            error: self.error.clone(),
            history: self.history,
            font_size: self.font_size,
            scroll_offset: 0,
            name: None,
            pinned: false,
            branch,
            attention: None,
            attention_note: None,
        }
    }

    /// Branch of the repository the current shell is in, from its reported
    /// directory or, before one is reported, its launch directory.
    fn discover_branch(&self) -> Option<String> {
        if self.spawn.ssh_host.is_some() {
            return None;
        }
        let dir = match &self.cwd {
            Some(cwd) => workspace::local_path(cwd),
            None => self
                .project_root
                .clone()
                .or_else(|| self.spawn.cwd.clone())?,
        };
        workspace::git_branch(&dir)
    }

    fn sync_active_tab(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        tab.session = self.session.clone();
        tab.spawn = self.spawn.clone();
        tab.project_root = self.project_root.clone();
        tab.cwd = self.cwd.clone();
        tab.title = self.title.clone();
        tab.last_exit = self.last_exit;
        tab.commands = self.commands;
        tab.error = self.error.clone();
        tab.history = self.history;
        tab.font_size = self.font_size;
        if let Some(session) = &self.session {
            tab.scroll_offset = session.with(|p| p.engine().display_offset());
        }
    }

    fn select_tab(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if index >= self.tabs.len() || index == self.active_tab {
            return;
        }
        self.sync_active_tab();
        self.renaming = None;
        self.tabs[index].attention = None;
        self.tabs[index].attention_note = None;
        let tab = self.tabs[index].clone();
        self.active_tab = index;
        self.session = tab.session;
        self.spawn = tab.spawn;
        self.project_root = tab.project_root;
        self.cwd = tab.cwd;
        self.title = tab.title;
        self.last_exit = tab.last_exit;
        self.commands = tab.commands;
        self.error = tab.error;
        self.history = tab.history;
        self.font_size = tab.font_size;
        self.selection = None;
        self.search = None;
        self.palette = None;
        if let Some(session) = &self.session {
            session.with_mut(|p| {
                p.engine_mut()
                    .scroll(Scroll::Lines(tab.scroll_offset as i32))
            });
        }
        self.on_output(cx);
        self.persist_layout();
    }

    fn new_local_tab(&mut self, cx: &mut Context<'_, Self>) {
        self.sync_active_tab();
        self.spawn = self.local_spawn.clone();
        self.project_root = self.local_project_root.clone();
        self.start_session(false);
        let tab = self.current_tab();
        self.tabs.push(tab);
        self.active_tab = self.tabs.len() - 1;
        self.persist_layout();
        cx.notify();
    }

    /// Open a local terminal running a specific discovered shell.
    fn new_shell_tab(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(profile) = self.shells.get(index).cloned() else {
            return;
        };
        self.shell_menu = false;
        self.sync_active_tab();
        self.spawn = self.local_spawn.clone();
        self.spawn.profile = profile;
        self.project_root = self.local_project_root.clone();
        self.start_session(false);
        let tab = self.current_tab();
        self.tabs.push(tab);
        self.active_tab = self.tabs.len() - 1;
        self.persist_layout();
        cx.notify();
    }

    /// Choose the shell used by "New terminal" and the keyboard shortcut.
    fn set_default_shell(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if let Some(profile) = self.shells.get(index) {
            self.local_spawn.profile = profile.clone();
            cx.notify();
        }
    }

    fn new_ssh_tab(&mut self, host: String, cx: &mut Context<'_, Self>) {
        let Some(profile) = tf_pty::ssh_profile(&host) else {
            self.error = Some("OpenSSH client is not available".into());
            cx.notify();
            return;
        };
        self.sync_active_tab();
        self.spawn = self.local_spawn.clone();
        self.spawn.profile = profile;
        self.spawn.ssh_host = Some(host);
        self.spawn.cwd = None;
        self.project_root = None;
        self.start_session(false);
        let tab = self.current_tab();
        self.tabs.push(tab);
        self.active_tab = self.tabs.len() - 1;
        self.persist_layout();
        cx.notify();
    }

    fn close_active_tab(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(session) = &self.session {
            if let Err(error) = session.close() {
                tracing::warn!(%error, "failed to close tab session");
            }
        }
        self.session = None;
        if self.tabs.len() == 1 {
            self.spawn = self.local_spawn.clone();
            self.project_root = self.local_project_root.clone();
            self.start(cx);
            return;
        }
        self.tabs.remove(self.active_tab);
        let next = self.active_tab.min(self.tabs.len() - 1);
        self.active_tab = usize::MAX;
        self.select_tab(next, cx);
        self.persist_layout();
    }

    fn cycle_tab(&mut self, backwards: bool, cx: &mut Context<'_, Self>) {
        if self.tabs.len() < 2 {
            return;
        }
        let next = if backwards {
            (self.active_tab + self.tabs.len() - 1) % self.tabs.len()
        } else {
            (self.active_tab + 1) % self.tabs.len()
        };
        self.select_tab(next, cx);
    }

    fn on_output(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(session) = &self.session {
            for event in session.take_events() {
                match event {
                    SessionEvent::Output(_) => {}
                    SessionEvent::Cwd(cwd) => {
                        self.cwd = Some(match cwd.host {
                            Some(host) => format!("{host}:{}", cwd.path),
                            None => cwd.path,
                        });
                        let branch = self.discover_branch();
                        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                            tab.branch = branch;
                        }
                    }
                    SessionEvent::BlockFinished { exit_code, .. } => {
                        self.last_exit = exit_code;
                        self.commands += 1;
                    }
                    SessionEvent::Notify { title, body } => {
                        tracing::info!(?title, %body, "terminal notification");
                    }
                    SessionEvent::ProgramStatusChanged => {}
                }
            }
            let (title, history) =
                session.with(|p| (p.engine().title(), p.engine().history_size()));
            self.title = title;
            // Scrollback shrank (`clear`): absolute positions now point at
            // different content, so drop anything anchored to them.
            if history < self.history {
                self.selection = None;
                if let Some(s) = &mut self.search {
                    s.matches.clear();
                    s.current = None;
                }
            }
            self.history = history;
            if let Some(s) = &mut self.search {
                s.stale = true;
            }
        }
        self.drain_background_tabs();
        self.sync_active_tab();
        cx.notify();
    }

    /// Background workspaces keep producing events; consume them so their
    /// sidebar rows stay current and can ask for attention.
    fn drain_background_tabs(&mut self) {
        let active = self.active_tab;
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            if index == active {
                continue;
            }
            let Some(session) = tab.session.clone() else {
                continue;
            };
            for event in session.take_events() {
                match event {
                    SessionEvent::Cwd(cwd) => {
                        tab.cwd = Some(match cwd.host {
                            Some(host) => format!("{host}:{}", cwd.path),
                            None => cwd.path,
                        });
                        if tab.spawn.ssh_host.is_none() {
                            tab.branch = tab
                                .cwd
                                .as_deref()
                                .map(workspace::local_path)
                                .and_then(|dir| workspace::git_branch(&dir));
                        }
                    }
                    SessionEvent::BlockFinished { exit_code, .. } => {
                        tab.last_exit = exit_code;
                        tab.commands += 1;
                        match exit_code {
                            Some(code) if code != 0 => Self::raise_attention(
                                tab,
                                Attention::Error,
                                Some(format!("command failed (exit {code})")),
                            ),
                            _ => Self::raise_attention(
                                tab,
                                Attention::Done,
                                Some("command finished".into()),
                            ),
                        }
                    }
                    SessionEvent::Notify { title, body } => {
                        let note =
                            title.map_or_else(|| body.clone(), |title| format!("{title}: {body}"));
                        Self::raise_attention(tab, Attention::Notice, Some(note));
                    }
                    SessionEvent::ProgramStatusChanged => {
                        if let Some((attention, note)) = Self::status_attention(&session) {
                            Self::raise_attention(tab, attention, note);
                        }
                    }
                    SessionEvent::Output(_) => {}
                }
            }
        }
    }

    /// The attention level implied by a session's reported program status.
    fn status_attention(session: &SessionHandle) -> Option<(Attention, Option<String>)> {
        session.with(|p| {
            p.program_status()
                .iter()
                .filter_map(|record| {
                    let attention = match record.report.state {
                        tf_session::ProgramState::Blocked => Attention::Blocked,
                        tf_session::ProgramState::Error => Attention::Error,
                        tf_session::ProgramState::Done => Attention::Done,
                        _ => return None,
                    };
                    let note = record
                        .report
                        .message
                        .clone()
                        .or_else(|| record.report.title.clone())
                        .or_else(|| record.report.app.clone());
                    Some((attention, note))
                })
                .max_by_key(|(attention, _)| *attention)
        })
    }

    /// Raise, never lower: a blocked workspace must not be hidden by a later
    /// "command finished".
    fn raise_attention(tab: &mut WorkspaceTab, level: Attention, note: Option<String>) {
        if tab.attention.is_none_or(|current| level >= current) {
            tab.attention = Some(level);
            tab.attention_note = note;
        }
    }

    /// Jump to the workspace that most needs the user, preferring blocked
    /// over errors over finished, and the nearest one after the current tab.
    fn jump_to_attention(&mut self, cx: &mut Context<'_, Self>) {
        let count = self.tabs.len();
        let mut best: Option<(Attention, usize)> = None;
        for offset in 1..=count {
            let index = (self.active_tab + offset) % count;
            if let Some(level) = self.tabs[index].attention {
                if best.is_none_or(|(current, _)| level > current) {
                    best = Some((level, index));
                }
            }
        }
        if let Some((_, index)) = best {
            self.select_tab(index, cx);
        }
    }

    // ---- workspace metadata: rename, pin, reorder, persistence -----------

    fn begin_rename(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        let text = tab
            .name
            .clone()
            .or_else(|| tab.title.clone())
            .unwrap_or_else(|| Self::tab_label(tab));
        self.renaming = Some(RenameState {
            tab_id: tab.id,
            text,
        });
        cx.notify();
    }

    fn rename_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<'_, Self>) {
        let key = ev.keystroke.key.as_str();
        let modifiers = ev.keystroke.modifiers;
        let Some(rename) = &mut self.renaming else {
            return;
        };
        match key {
            "escape" => self.renaming = None,
            "enter" => {
                let name = rename.text.trim().to_owned();
                let tab_id = rename.tab_id;
                self.renaming = None;
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                    tab.name = (!name.is_empty()).then_some(name);
                }
                self.persist_layout();
            }
            "backspace" => {
                rename.text.pop();
            }
            _ if !modifiers.control && !modifiers.alt => {
                if let Some(text) = ev.keystroke.key_char.as_deref() {
                    if !text.chars().any(char::is_control) {
                        rename.text.push_str(text);
                    }
                }
            }
            _ => {}
        }
        cx.notify();
    }

    /// Keep pinned workspaces above unpinned ones, preserving relative order,
    /// and keep `active_tab` pointing at the same workspace.
    fn normalize_tab_order(&mut self) {
        let active_id = self.tabs.get(self.active_tab).map(|tab| tab.id);
        self.tabs.sort_by_key(|tab| !tab.pinned);
        if let Some(id) = active_id {
            if let Some(index) = self.tabs.iter().position(|tab| tab.id == id) {
                self.active_tab = index;
            }
        }
    }

    fn toggle_pin(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        tab.pinned = !tab.pinned;
        self.normalize_tab_order();
        self.persist_layout();
        cx.notify();
    }

    fn move_tab(&mut self, from: usize, to: usize, cx: &mut Context<'_, Self>) {
        if from >= self.tabs.len() || to >= self.tabs.len() || from == to {
            return;
        }
        let active_id = self.tabs.get(self.active_tab).map(|tab| tab.id);
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        if let Some(id) = active_id {
            if let Some(index) = self.tabs.iter().position(|tab| tab.id == id) {
                self.active_tab = index;
            }
        }
        self.normalize_tab_order();
        self.persist_layout();
        cx.notify();
    }

    fn move_active_tab(&mut self, down: bool, cx: &mut Context<'_, Self>) {
        let from = self.active_tab;
        let to = if down {
            from + 1
        } else {
            from.saturating_sub(1)
        };
        self.move_tab(from, to, cx);
    }

    /// Hand the layout to the daemon so the next launch restores it.
    fn persist_layout(&mut self) {
        let Some(root) = self.local_project_root.clone() else {
            return;
        };
        let tabs: Vec<SavedTab> = self
            .tabs
            .iter()
            .filter_map(|tab| {
                let id = tab.session.as_ref()?.id()?;
                Some(SavedTab {
                    session: id.to_string(),
                    pinned: tab.pinned,
                    name: tab.name.clone(),
                })
            })
            .collect();
        let active = self
            .tabs
            .get(self.active_tab)
            .and_then(|tab| tab.session.as_ref()?.id())
            .map(|id| id.to_string());
        let encoded = Layout { tabs, active }.encode();
        if encoded != self.saved_layout {
            self.saved_layout.clone_from(&encoded);
            remote_session::save_workspace_state(root, encoded);
        }
    }

    /// Reattach to the workspaces the daemon kept alive, in the saved order;
    /// fall back to a single fresh (or matching) session.
    fn restore_workspace(&mut self, cx: &mut Context<'_, Self>) {
        let restored = self.restore_from_daemon(cx);
        if !restored {
            self.start_session(true);
            let tab = self.current_tab();
            self.tabs.push(tab);
        }
    }

    fn restore_from_daemon(&mut self, cx: &mut Context<'_, Self>) -> bool {
        let Some(root) = self.local_project_root.clone() else {
            return false;
        };
        let Some(stored) = remote_session::load_workspace(Some(root.clone())) else {
            return false;
        };
        let layout = stored
            .layout
            .as_deref()
            .map(Layout::decode)
            .unwrap_or_default();
        let running = |id: &str| {
            stored
                .sessions
                .iter()
                .find(|session| session.running && session.id.to_string() == id)
        };
        let mut plan: Vec<(tf_proto::SessionInfo, Option<String>, bool)> = Vec::new();
        for saved in &layout.tabs {
            if let Some(info) = running(&saved.session) {
                plan.push((info.clone(), saved.name.clone(), saved.pinned));
            }
        }
        for info in &stored.sessions {
            let known = plan.iter().any(|(planned, _, _)| planned.id == info.id);
            if info.running && !known && info.project_root.as_ref() == Some(&root) {
                plan.push((info.clone(), None, false));
            }
        }
        if plan.is_empty() {
            return false;
        }
        let mut active_index = 0;
        for (info, name, pinned) in plan {
            self.spawn = self.local_spawn.clone();
            self.project_root = info.project_root.clone();
            if let Some(shell) = self.shells.iter().find(|shell| shell.name == info.shell) {
                self.spawn.profile = shell.clone();
            }
            if let Some(host) = info.ssh_host.clone() {
                let Some(profile) = tf_pty::ssh_profile(&host) else {
                    continue;
                };
                self.spawn.profile = profile;
                self.spawn.ssh_host = Some(host);
                self.spawn.cwd = None;
            }
            self.start_session_as(Attach::Session(info.id));
            let mut tab = self.current_tab();
            tab.name = name;
            tab.pinned = pinned;
            if layout.active.as_deref() == Some(&info.id.to_string()) {
                active_index = self.tabs.len();
            }
            self.tabs.push(tab);
        }
        if self.tabs.is_empty() {
            return false;
        }
        self.normalize_tab_order();
        let target = active_index.min(self.tabs.len() - 1);
        self.active_tab = usize::MAX;
        self.select_tab(target, cx);
        true
    }

    fn metrics(&self, window: &Window) -> Metrics {
        let ts = window.text_system();
        let font_size = px(self.font_size);
        let id = ts.resolve_font(&self.font.regular());
        let cell_w = ts
            .advance(id, font_size, 'm')
            .map(|s| s.width)
            .unwrap_or(px(self.font_size * 0.6));
        Metrics {
            cell_w,
            line_h: px((self.font_size * 1.35).round()),
        }
    }

    fn write(&self, bytes: &[u8]) {
        if let Some(s) = &self.session {
            if let Err(e) = s.write(bytes) {
                tracing::warn!("write to pty failed: {e:#}");
            }
        }
    }

    fn modes(&self) -> tf_engine::Modes {
        self.session
            .as_ref()
            .map(|s| s.with(|p| p.engine().modes()))
            .unwrap_or_default()
    }

    fn input_modes(&self) -> InputModes {
        let m = self.modes();
        InputModes {
            app_cursor: m.app_cursor,
            bracketed_paste: m.bracketed_paste,
        }
    }

    fn exited(&self) -> Option<Option<u32>> {
        self.session.as_ref().and_then(|s| s.exit_status())
    }

    fn focus_changed(&mut self, focused: bool, cx: &mut Context<'_, Self>) {
        if self.modes().focus_events {
            self.write(encode_focus(focused));
        }
        cx.notify();
    }

    // ---- keyboard -------------------------------------------------------

    fn key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<'_, Self>) {
        let ks = &ev.keystroke;
        let m = ks.modifiers;
        let mods = Mods {
            ctrl: m.control,
            alt: m.alt,
            shift: m.shift,
        };
        let key = ks.key.as_str();
        let ctrl_shift = m.control && m.shift && !m.alt;

        if self.shell_menu && key == "escape" {
            self.shell_menu = false;
            cx.notify();
            cx.stop_propagation();
            return;
        }
        if self.renaming.is_some() {
            self.rename_key(ev, cx);
            cx.stop_propagation();
            return;
        }
        if self.settings.is_some() {
            if key == "escape" {
                self.settings = None;
                cx.notify();
            }
            cx.stop_propagation();
            return;
        }
        if self.search.is_some() && self.search_key(ev, cx) {
            cx.stop_propagation();
            return;
        }
        if self.palette.is_some() && self.palette_key(ev, cx) {
            cx.stop_propagation();
            return;
        }

        // App shortcuts first. Ctrl+Shift chords never collide with shells.
        if ctrl_shift && key.eq_ignore_ascii_case("t") {
            self.new_local_tab(cx);
        } else if ctrl_shift && key.eq_ignore_ascii_case("w") {
            self.close_active_tab(cx);
        } else if ctrl_shift && key.eq_ignore_ascii_case("q") {
            cx.quit();
        } else if ctrl_shift && key.eq_ignore_ascii_case("b") {
            self.toggle_sidebar(cx);
        } else if ctrl_shift && key.eq_ignore_ascii_case("u") {
            self.jump_to_attention(cx);
        } else if ctrl_shift && key.eq_ignore_ascii_case("r") {
            self.begin_rename(self.active_tab, cx);
        } else if ctrl_shift && key.eq_ignore_ascii_case("k") {
            self.toggle_pin(self.active_tab, cx);
        } else if ctrl_shift && matches!(key, "up" | "down") {
            self.move_active_tab(key == "down", cx);
        } else if m.control && !m.shift && !m.alt && key == "," {
            self.open_settings(cx);
        } else if m.control && key == "tab" {
            self.cycle_tab(m.shift, cx);
        } else if ctrl_shift && key.eq_ignore_ascii_case("p") {
            self.ssh_hosts = tf_pty::discover_ssh_hosts();
            self.palette = Some(PaletteState::default());
            cx.notify();
        } else if ctrl_shift && key.eq_ignore_ascii_case("f") {
            self.open_search(cx);
        } else if (ctrl_shift && key.eq_ignore_ascii_case("v")) || (m.shift && key == "insert") {
            self.paste(cx);
        } else if ctrl_shift && key.eq_ignore_ascii_case("c") {
            self.copy_selection(cx);
        } else if ctrl_shift && key.eq_ignore_ascii_case("a") {
            self.select_all(cx);
        } else if m.control && !m.shift && !m.alt && key == "c" && self.has_selection() {
            // Windows Terminal convention: Ctrl+C copies when something is
            // selected, and is ^C otherwise.
            self.copy_selection(cx);
            self.clear_selection(cx);
        } else if key == "escape" && self.has_selection() {
            self.clear_selection(cx);
        } else if m.control && !m.alt && !m.shift && matches!(key, "=" | "+" | "-" | "0") {
            self.font_size = match key {
                "-" => (self.font_size - 1.0).max(8.0),
                "0" => 14.0,
                _ => (self.font_size + 1.0).min(32.0),
            };
            cx.notify();
        } else if m.shift && !m.control && matches!(key, "pageup" | "pagedown") {
            let s = if key == "pageup" {
                Scroll::PageUp
            } else {
                Scroll::PageDown
            };
            self.scroll(s, cx);
        } else if m.shift && m.control && matches!(key, "home" | "end") {
            let s = if key == "home" {
                Scroll::Top
            } else {
                Scroll::Bottom
            };
            self.scroll(s, cx);
        } else if m.control
            && !m.shift
            && !m.alt
            && key.len() == 1
            && key.chars().next().is_some_and(|c| c.is_ascii_digit())
        {
            // Ctrl+1..Ctrl+9 for direct tab access
            if let Some(digit) = key.chars().next().and_then(|c| c.to_digit(10)) {
                let index = (digit as usize).saturating_sub(1);
                if index < self.tabs.len() {
                    self.select_tab(index, cx);
                }
            }
        } else if m.control && m.shift && !m.alt && matches!(key, "left" | "right") {
            // Ctrl+Shift+Left/Right for tab navigation
            self.cycle_tab(key == "left", cx);
        } else {
            self.send_key(ev, mods, window, cx);
            return;
        }
        cx.stop_propagation();
    }

    fn send_key(
        &mut self,
        ev: &KeyDownEvent,
        mods: Mods,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let ks = &ev.keystroke;
        let key = ks.key.as_str();
        if self.exited().is_some() {
            if key == "enter" {
                self.start(cx);
                window.focus(&self.focus);
                cx.notify();
            }
            cx.stop_propagation();
            return;
        }
        let logical = Key::from_name(key).or_else(|| {
            // Prefer the layout-resolved character; fall back to the key
            // itself for Ctrl chords, where no character is produced.
            ks.key_char
                .clone()
                .or_else(|| (mods.ctrl || mods.alt).then(|| key.to_owned()))
                .map(Key::Text)
        });
        let Some(logical) = logical else { return };
        if let Some(bytes) = encode_key(&logical, mods, self.input_modes()) {
            self.selection = None;
            self.scroll(Scroll::Bottom, cx);
            self.write(&bytes);
            cx.stop_propagation();
        }
    }

    fn paste(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
            let bytes = encode_paste(&text, self.input_modes());
            self.selection = None;
            self.scroll(Scroll::Bottom, cx);
            self.write(&bytes);
        }
    }

    fn scroll(&mut self, scroll: Scroll, cx: &mut Context<'_, Self>) {
        if let Some(s) = &self.session {
            s.with_mut(|p| p.engine_mut().scroll(scroll));
            cx.notify();
        }
    }

    // ---- selection ------------------------------------------------------

    fn has_selection(&self) -> bool {
        self.selection.is_some_and(|s| !s.is_empty())
    }

    fn selection_text(&self) -> Option<String> {
        let sel = self.selection.filter(|s| !s.is_empty())?;
        let text = self.session.as_ref()?.with(|p| sel.text(p.engine()));
        (!text.is_empty()).then_some(text)
    }

    fn copy_selection(&self, cx: &mut Context<'_, Self>) {
        if let Some(text) = self.selection_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn clear_selection(&mut self, cx: &mut Context<'_, Self>) {
        self.selection = None;
        cx.notify();
    }

    fn select_all(&mut self, cx: &mut Context<'_, Self>) {
        let Some(s) = &self.session else { return };
        let (total, cols) = s.with(|p| (p.engine().total_lines(), p.engine().size().cols));
        let mut sel = Selection::new(tf_engine::Point::new(0, 0), SelectionKind::Cells);
        sel.extend(tf_engine::Point::new(
            total.saturating_sub(1),
            cols.saturating_sub(1),
        ));
        self.selection = Some(sel);
        cx.notify();
    }

    // ---- mouse ----------------------------------------------------------

    /// Viewport cell and absolute point under a window position.
    fn hit(
        &self,
        pos: gpui::Point<Pixels>,
        window: &Window,
    ) -> Option<((u16, u16), tf_engine::Point)> {
        let s = self.session.as_ref()?;
        let (size, top) = s.with(|p| {
            let e = p.engine();
            (e.size(), e.history_size() - e.display_offset())
        });
        let cell = self
            .metrics(window)
            .cell_at(self.grid_bounds, pos, size.cols, size.rows);
        Some((cell, tf_engine::Point::new(top + cell.1 as usize, cell.0)))
    }

    /// Mouse tracking applies unless Shift is held (Shift always selects).
    fn tracking(&self, mods: Modifiers) -> Tracking {
        if mods.shift {
            return Tracking::Off;
        }
        match self.modes().mouse {
            MouseTracking::Off => Tracking::Off,
            MouseTracking::Click => Tracking::Click,
            MouseTracking::Drag => Tracking::Drag,
            MouseTracking::Motion => Tracking::Motion,
        }
    }

    fn report(&mut self, action: MouseAction, cell: (u16, u16), mods: Modifiers) -> bool {
        let tracking = self.tracking(mods);
        let m = Mods {
            ctrl: mods.control,
            alt: mods.alt,
            shift: mods.shift,
        };
        match encode_mouse(action, cell.0, cell.1, m, tracking, self.modes().sgr_mouse) {
            Some(bytes) => {
                self.write(&bytes);
                true
            }
            None => false,
        }
    }

    fn mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<'_, Self>) {
        window.focus(&self.focus);
        let Some((cell, at)) = self.hit(ev.position, window) else {
            return;
        };
        if ev.button == MouseButton::Left && ev.modifiers.control {
            if let Some(link) = self.link_at(at) {
                if is_openable(&link.url) {
                    cx.open_url(&link.url);
                }
                return;
            }
        }
        let button = match ev.button {
            MouseButton::Left => Some(TMouse::Left),
            MouseButton::Middle => Some(TMouse::Middle),
            MouseButton::Right => Some(TMouse::Right),
            _ => None,
        };
        if let Some(b) = button {
            if self.report(MouseAction::Press(b), cell, ev.modifiers) {
                self.reported = Some(Reported {
                    button: b,
                    last: cell,
                });
                self.selection = None;
                cx.notify();
                return;
            }
        }
        match ev.button {
            MouseButton::Left => {
                let kind = match ev.click_count {
                    0 | 1 => SelectionKind::Cells,
                    2 => SelectionKind::Words,
                    _ => SelectionKind::Lines,
                };
                match &mut self.selection {
                    Some(sel) if ev.modifiers.shift && ev.click_count <= 1 => sel.extend(at),
                    _ => self.selection = Some(Selection::new(at, kind)),
                }
                self.selecting = true;
            }
            // Windows Terminal convention: right-click copies a selection,
            // or pastes when there is none.
            MouseButton::Right => {
                if self.has_selection() {
                    self.copy_selection(cx);
                    self.selection = None;
                } else {
                    self.paste(cx);
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn mouse_move(&mut self, ev: &MouseMoveEvent, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some((cell, at)) = self.hit(ev.position, window) else {
            return;
        };
        self.update_hover(ev.modifiers, Some(at), cx);

        if let Some(r) = self.reported {
            if cell != r.last {
                self.report(MouseAction::Motion(Some(r.button)), cell, ev.modifiers);
                self.reported = Some(Reported { last: cell, ..r });
            }
            return;
        }
        if self.selecting && ev.pressed_button == Some(MouseButton::Left) {
            // Dragging past the edge scrolls, one line per move event.
            let b = self.grid_bounds;
            if ev.position.y < b.origin.y {
                self.scroll(Scroll::Lines(1), cx);
            } else if ev.position.y > b.origin.y + b.size.height {
                self.scroll(Scroll::Lines(-1), cx);
            }
            if let Some((_, at)) = self.hit(ev.position, window) {
                if let Some(sel) = &mut self.selection {
                    sel.extend(at);
                }
            }
            cx.notify();
            return;
        }
        if ev.pressed_button.is_none()
            && self.tracking(ev.modifiers) == Tracking::Motion
            && self.last_motion != Some(cell)
            && self.grid_bounds.contains(&ev.position)
        {
            self.last_motion = Some(cell);
            self.report(MouseAction::Motion(None), cell, ev.modifiers);
        }
    }

    fn mouse_up(&mut self, ev: &MouseUpEvent, window: &mut Window, cx: &mut Context<'_, Self>) {
        if let Some(r) = self.reported.take() {
            let cell = self.hit(ev.position, window).map_or(r.last, |(c, _)| c);
            self.report(MouseAction::Release(r.button), cell, ev.modifiers);
            return;
        }
        if self.selecting {
            self.selecting = false;
            if self.selection.is_some_and(|s| s.is_empty()) {
                self.selection = None;
            }
            cx.notify();
        }
    }

    fn link_at(&self, at: tf_engine::Point) -> Option<Link> {
        self.session.as_ref()?.with(|p| link_at(p.engine(), at))
    }

    /// Ctrl+hover underlines links, as in VS Code and Windows Terminal.
    fn update_hover(
        &mut self,
        mods: Modifiers,
        at: Option<tf_engine::Point>,
        cx: &mut Context<'_, Self>,
    ) {
        let link = if mods.control && !mods.shift {
            at.and_then(|p| self.link_at(p))
                .filter(|l| is_openable(&l.url))
        } else {
            None
        };
        if link != self.hover_link {
            self.hover_link = link;
            cx.notify();
        }
    }

    fn scroll_wheel(
        &mut self,
        ev: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let line_h = self.metrics(window).line_h;
        self.scroll_accum += ev.delta.pixel_delta(line_h).y / line_h;
        let lines = self.scroll_accum.trunc() as i32;
        if lines == 0 {
            return;
        }
        self.scroll_accum -= lines as f32;
        let steps = lines.unsigned_abs().min(10);

        if self.tracking(ev.modifiers) != Tracking::Off {
            if let Some((cell, _)) = self.hit(ev.position, window) {
                let b = if lines > 0 {
                    TMouse::WheelUp
                } else {
                    TMouse::WheelDown
                };
                for _ in 0..steps {
                    self.report(MouseAction::Press(b), cell, ev.modifiers);
                }
            }
            return;
        }
        let modes = self.modes();
        if modes.alt_screen && modes.alternate_scroll {
            // Alternate scroll mode: full-screen apps get arrow keys.
            let key = if lines > 0 { Key::Up } else { Key::Down };
            if let Some(b) = encode_key(&key, Mods::NONE, self.input_modes()) {
                for _ in 0..steps {
                    self.write(&b);
                }
            }
        } else {
            self.scroll(Scroll::Lines(lines * 3), cx);
        }
    }

    // ---- search ---------------------------------------------------------

    fn open_search(&mut self, cx: &mut Context<'_, Self>) {
        let prefill = self
            .selection_text()
            .filter(|t| !t.contains('\n') && t.chars().count() <= 200);
        let search = self.search.get_or_insert_with(SearchState::default);
        if let Some(text) = prefill {
            search.query = text;
            self.selection = None;
        }
        self.run_search(cx);
    }

    fn close_search(&mut self, cx: &mut Context<'_, Self>) {
        self.search = None;
        cx.notify();
    }

    /// Recompute matches and jump to the one nearest the bottom of the
    /// viewport, searching upwards (terminal output reads bottom-up).
    fn run_search(&mut self, cx: &mut Context<'_, Self>) {
        let Some(session) = &self.session else { return };
        let Some(search) = &mut self.search else {
            return;
        };
        let (matches, bottom) = session.with(|p| {
            let e = p.engine();
            let m = find_all(e, &search.query, search.case_sensitive, MAX_MATCHES);
            (
                m,
                e.history_size() - e.display_offset() + e.size().rows as usize,
            )
        });
        search.current = matches
            .iter()
            .rposition(|m| m.start.line < bottom)
            .or_else(|| matches.len().checked_sub(1));
        search.matches = matches;
        search.stale = false;
        self.reveal_current(cx);
    }

    fn step_search(&mut self, up: bool, cx: &mut Context<'_, Self>) {
        if self.search.as_ref().is_some_and(|s| s.stale) {
            // Keep the position: re-find, then continue from the old match.
            let old = self
                .search
                .as_ref()
                .and_then(|s| s.current.and_then(|i| s.matches.get(i)).copied());
            self.run_search(cx);
            if let (Some(old), Some(s)) = (old, &mut self.search) {
                if let Some(i) = s.matches.iter().position(|m| *m == old) {
                    s.current = Some(i);
                }
            }
        }
        let Some(s) = &mut self.search else { return };
        let n = s.matches.len();
        if n == 0 {
            return;
        }
        s.current = Some(match (s.current, up) {
            (Some(i), true) => (i + n - 1) % n,
            (Some(i), false) => (i + 1) % n,
            (None, _) => n - 1,
        });
        self.reveal_current(cx);
    }

    /// Scroll so the current match is visible, centred when it was not.
    fn reveal_current(&mut self, cx: &mut Context<'_, Self>) {
        let Some(line) = self
            .search
            .as_ref()
            .and_then(|s| s.current.and_then(|i| s.matches.get(i)))
            .map(|m| m.start.line)
        else {
            cx.notify();
            return;
        };
        if let Some(session) = &self.session {
            session.with_mut(|p| {
                let e = p.engine_mut();
                let history = e.history_size();
                let rows = e.size().rows as usize;
                let offset = e.display_offset();
                let top = history - offset;
                if line < top || line >= top + rows {
                    let want_top = line.saturating_sub(rows / 2).min(history);
                    let want_offset = history - want_top;
                    e.scroll(Scroll::Lines(want_offset as i32 - offset as i32));
                }
            });
        }
        cx.notify();
    }

    /// Keys while the search bar is open. Returns whether it was consumed.
    fn search_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<'_, Self>) -> bool {
        let ks = &ev.keystroke;
        let m = ks.modifiers;
        let key = ks.key.as_str();
        let Some(search) = &mut self.search else {
            return false;
        };
        match key {
            "escape" => self.close_search(cx),
            "enter" => self.step_search(!m.shift, cx),
            "up" => self.step_search(true, cx),
            "down" => self.step_search(false, cx),
            "backspace" => {
                if m.control {
                    search.query.clear();
                } else {
                    search.query.pop();
                }
                self.run_search(cx);
            }
            "c" if m.alt && !m.control => {
                search.case_sensitive = !search.case_sensitive;
                self.run_search(cx);
            }
            "f" if m.control && m.shift => {}
            "v" if m.control => {
                if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    search
                        .query
                        .push_str(text.lines().next().unwrap_or_default());
                    self.run_search(cx);
                }
            }
            _ if !m.control && !m.alt => match ks.key_char.as_deref() {
                Some(ch) if !ch.chars().any(char::is_control) => {
                    search.query.push_str(ch);
                    self.run_search(cx);
                }
                _ => return false,
            },
            _ => return false,
        }
        true
    }

    fn search_bar(&self) -> Option<impl IntoElement> {
        let s = self.search.as_ref()?;
        let t = &self.theme;
        let count = match (s.current, s.matches.len()) {
            (_, 0) if s.query.is_empty() => String::new(),
            (_, 0) => "No results".to_owned(),
            (Some(i), n) if n >= MAX_MATCHES => format!("{} of {n}+", i + 1),
            (Some(i), n) => format!("{} of {n}", i + 1),
            (None, n) => format!("{n}"),
        };
        let pill = |label: &'static str, on: bool| {
            div()
                .px(px(tokens::space::XS + 2.0))
                .rounded(px(tokens::radius::SM))
                .text_size(px(tokens::text::XS))
                .when(on, |d| d.bg(hsla(t.accent)).text_color(hsla(t.accent_text)))
                .when(!on, |d| d.text_color(hsla(t.text_faint)))
                .child(label)
        };
        Some(
            div()
                .absolute()
                .top(px(72.0 + tokens::space::SM as f32))
                .right(px(tokens::space::LG))
                .h(px(30.0))
                .min_w(px(300.0))
                .px(px(tokens::space::MD))
                .flex()
                .items_center()
                .gap(px(tokens::space::SM))
                .rounded(px(tokens::radius::LG))
                .bg(hsla(t.bg_elevated))
                .border_1()
                .border_color(hsla(t.border_strong))
                .shadow_md()
                .child(
                    div()
                        .text_color(hsla(t.text_faint))
                        .text_size(px(tokens::text::XS))
                        .child("Find"),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .text_color(hsla(t.text))
                        .child(SharedString::from(s.query.clone()))
                        .child(div().w(px(1.5)).h(px(15.0)).bg(hsla(t.accent))),
                )
                .child(
                    div()
                        .text_size(px(tokens::text::XS))
                        .text_color(hsla(if count == "No results" {
                            t.danger
                        } else {
                            t.text_muted
                        }))
                        .child(SharedString::from(count)),
                )
                .child(pill("Aa", s.case_sensitive))
                .child(
                    div()
                        .text_size(px(tokens::text::XS))
                        .text_color(hsla(t.text_faint))
                        .child("↑↓  Esc"),
                ),
        )
    }

    fn filtered_palette_commands(&self) -> Vec<(PaletteAction, String, String)> {
        let query = self
            .palette
            .as_ref()
            .map(|palette| palette.query.to_ascii_lowercase())
            .unwrap_or_default();
        let mut commands: Vec<_> = PALETTE_COMMANDS
            .iter()
            .cloned()
            .map(|(action, label, shortcut)| (action, label.to_owned(), shortcut.to_owned()))
            .collect();
        if self.spawn.ssh_host.is_some() {
            commands.push((
                PaletteAction::SwitchLocal,
                "Workspace: switch to local".into(),
                String::new(),
            ));
        }
        for (index, shell) in self.shells.iter().enumerate() {
            commands.push((
                PaletteAction::NewShell(index),
                format!("Shell: new {} terminal", shell.name),
                String::new(),
            ));
            commands.push((
                PaletteAction::SetDefaultShell(index),
                format!("Shell: use {} for new terminals", shell.name),
                String::new(),
            ));
        }
        commands.extend(self.ssh_hosts.iter().cloned().map(|host| {
            (
                PaletteAction::ConnectSsh(host.clone()),
                format!("SSH: connect to {host}"),
                String::new(),
            )
        }));
        commands
            .into_iter()
            .filter(|(_, label, _)| query.is_empty() || label.to_ascii_lowercase().contains(&query))
            .collect()
    }

    fn palette_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<'_, Self>) -> bool {
        let key = ev.keystroke.key.as_str();
        let modifiers = ev.keystroke.modifiers;
        if modifiers.control && modifiers.shift && key.eq_ignore_ascii_case("p") {
            self.palette = None;
            cx.notify();
            return true;
        }
        let commands = self.filtered_palette_commands();
        let count = commands.len();
        let Some(palette) = &mut self.palette else {
            return false;
        };
        match key {
            "escape" => self.palette = None,
            "up" if count > 0 => palette.selected = (palette.selected + count - 1) % count,
            "down" if count > 0 => palette.selected = (palette.selected + 1) % count,
            "enter" if count > 0 => {
                let action = commands[palette.selected.min(count - 1)].0.clone();
                self.palette = None;
                self.run_palette_action(action, cx);
            }
            "backspace" => {
                palette.query.pop();
                palette.selected = 0;
            }
            _ if !modifiers.control && !modifiers.alt => {
                if let Some(text) = ev.keystroke.key_char.as_deref() {
                    if !text.chars().any(char::is_control) {
                        palette.query.push_str(text);
                        palette.selected = 0;
                    }
                }
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    fn run_palette_action(&mut self, action: PaletteAction, cx: &mut Context<'_, Self>) {
        match action {
            PaletteAction::NewTab => self.new_local_tab(cx),
            PaletteAction::CloseTab => self.close_active_tab(cx),
            PaletteAction::NextTab => self.cycle_tab(false, cx),
            PaletteAction::PreviousTab => self.cycle_tab(true, cx),
            PaletteAction::RestartSession => self.start(cx),
            PaletteAction::Quit => cx.quit(),
            PaletteAction::Find => self.open_search(cx),
            PaletteAction::Copy => self.copy_selection(cx),
            PaletteAction::Paste => self.paste(cx),
            PaletteAction::FontIncrease => self.font_size = (self.font_size + 1.0).min(32.0),
            PaletteAction::FontDecrease => self.font_size = (self.font_size - 1.0).max(8.0),
            PaletteAction::FontReset => self.font_size = 14.0,
            PaletteAction::ScrollTop => self.scroll(Scroll::Top, cx),
            PaletteAction::ScrollBottom => self.scroll(Scroll::Bottom, cx),
            PaletteAction::CopyDiagnostics => {
                let text = format!(
                    "TermForge {} · protocol v{} · shell={} · cwd={}",
                    env!("CARGO_PKG_VERSION"),
                    tf_proto::PROTOCOL_VERSION,
                    self.spawn.profile.name,
                    self.cwd.as_deref().unwrap_or("unknown")
                );
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            PaletteAction::NextAttention => self.jump_to_attention(cx),
            PaletteAction::RenameWorkspace => self.begin_rename(self.active_tab, cx),
            PaletteAction::TogglePin => self.toggle_pin(self.active_tab, cx),
            PaletteAction::MoveUp => self.move_active_tab(false, cx),
            PaletteAction::MoveDown => self.move_active_tab(true, cx),
            PaletteAction::NewShell(index) => self.new_shell_tab(index, cx),
            PaletteAction::SetDefaultShell(index) => self.set_default_shell(index, cx),
            PaletteAction::ToggleSidebar => self.toggle_sidebar(cx),
            PaletteAction::OpenSettings => self.open_settings(cx),
            PaletteAction::ListTasks => self.write(b"tf task list\r"),
            PaletteAction::StatusWorking => self.write(b"tf status working --app termforge\r"),
            PaletteAction::StatusDone => self.write(b"tf status done --app termforge\r"),
            PaletteAction::StatusClear => self.write(b"tf status clear\r"),
            PaletteAction::DaemonDoctor => self.write(b"forged doctor\r"),
            PaletteAction::ConnectSsh(host) => self.new_ssh_tab(host, cx),
            PaletteAction::SwitchLocal => {
                if let Some(index) = self
                    .tabs
                    .iter()
                    .position(|tab| tab.spawn.ssh_host.is_none())
                {
                    self.select_tab(index, cx);
                } else {
                    self.new_local_tab(cx);
                }
            }
        }
        cx.notify();
    }

    fn command_palette(&self) -> Option<impl IntoElement> {
        let palette = self.palette.as_ref()?;
        let commands = self.filtered_palette_commands();
        let command_count = commands.len();
        let visible_start = palette.selected.saturating_sub(7);
        let t = &self.theme;
        let query = if palette.query.is_empty() {
            "Type a command…".to_owned()
        } else {
            palette.query.clone()
        };
        Some(
            div()
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .right(px(0.0))
                .flex()
                .justify_center()
                .child(
                    div()
                        .mt(px(48.0))
                        .w(px(620.0))
                        .p(px(tokens::space::SM))
                        .rounded(px(tokens::radius::XL))
                        .bg(hsla(t.bg_elevated))
                        .border_1()
                        .border_color(hsla(t.border_strong))
                        .shadow_lg()
                        .child(
                            div()
                                .h(px(30.0))
                                .px(px(tokens::space::SM))
                                .flex()
                                .items_center()
                                .justify_between()
                                .text_size(px(tokens::text::XS))
                                .text_color(hsla(t.text_faint))
                                .child("COMMAND PALETTE")
                                .child(SharedString::from(format!("{command_count} actions"))),
                        )
                        .child(
                            div()
                                .h(px(42.0))
                                .px(px(tokens::space::MD))
                                .flex()
                                .items_center()
                                .rounded(px(tokens::radius::MD))
                                .bg(hsla(t.bg_app))
                                .text_color(hsla(if palette.query.is_empty() {
                                    t.text_faint
                                } else {
                                    t.text
                                }))
                                .child(">  ")
                                .child(SharedString::from(query)),
                        )
                        .child(div().mt(px(tokens::space::XS)).flex().flex_col().children(
                            commands.iter().enumerate().skip(visible_start).take(9).map(
                                |(index, (_, label, shortcut))| {
                                    div()
                                        .h(px(36.0))
                                        .px(px(tokens::space::MD))
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .rounded(px(tokens::radius::MD))
                                        .when(index == palette.selected, |d| {
                                            d.bg(hsla(t.accent)).text_color(hsla(t.accent_text))
                                        })
                                        .when(index != palette.selected, |d| {
                                            d.text_color(hsla(t.text_muted))
                                        })
                                        .child(SharedString::from(label.clone()))
                                        .child(
                                            div()
                                                .text_size(px(tokens::text::XS))
                                                .child(SharedString::from(shortcut.clone())),
                                        )
                                },
                            ),
                        ))
                        .when(command_count == 0, |d| {
                            d.child(
                                div()
                                    .p(px(tokens::space::MD))
                                    .text_color(hsla(t.text_faint))
                                    .child("No matching commands"),
                            )
                        })
                        .child(
                            div()
                                .h(px(30.0))
                                .px(px(tokens::space::SM))
                                .flex()
                                .items_center()
                                .justify_between()
                                .text_size(px(tokens::text::XS))
                                .text_color(hsla(t.text_faint))
                                .child("↑↓ Navigate   ↵ Run")
                                .child("Esc Close"),
                        ),
                ),
        )
    }

    // ---- chrome ---------------------------------------------------------

    fn project_name(&self) -> String {
        self.project_root
            .as_deref()
            .and_then(std::path::Path::file_name)
            .and_then(|name| name.to_str())
            .unwrap_or("Workspace")
            .to_owned()
    }

    fn tab_label(tab: &WorkspaceTab) -> String {
        tab.spawn
            .ssh_host
            .as_ref()
            .map(|host| format!("SSH · {host}"))
            .unwrap_or_else(|| tab.spawn.profile.name.clone())
    }

    fn tab_status_color(&self, tab: &WorkspaceTab, active: bool, t: &Theme) -> Rgb {
        if active {
            return self.status_color();
        }
        if let Some(session) = &tab.session {
            let color = session.with(|p| {
                let status = p.program_status();
                status
                    .iter()
                    .max_by_key(|r| match r.report.state {
                        tf_session::ProgramState::Blocked => 5,
                        tf_session::ProgramState::Error => 4,
                        tf_session::ProgramState::Working => 3,
                        tf_session::ProgramState::Done => 2,
                        tf_session::ProgramState::Idle => 1,
                        tf_session::ProgramState::Clear => 0,
                    })
                    .map(|record| match record.report.state {
                        tf_session::ProgramState::Working => t.accent,
                        tf_session::ProgramState::Blocked => t.warning,
                        tf_session::ProgramState::Error => t.danger,
                        tf_session::ProgramState::Done => t.success,
                        tf_session::ProgramState::Idle | tf_session::ProgramState::Clear => {
                            t.text_faint
                        }
                    })
            });
            if let Some(color) = color {
                return color;
            }
        }
        match (
            tab.session.as_ref().and_then(|s| s.exit_status()),
            tab.last_exit,
        ) {
            (Some(_), _) => t.text_faint,
            (None, Some(c)) if c != 0 => t.danger,
            _ => t.success,
        }
    }

    fn header(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let t = &self.theme;
        let tab_count = self.tabs.len();
        div()
            .h(px(if self.show_tab_strip { 72.0 } else { 30.0 }))
            .flex()
            .flex_col()
            .bg(hsla(t.bg_app))
            .border_b_1()
            .border_color(hsla(t.border))
            .child(
                div()
                    .h(px(30.0))
                    .px(px(tokens::space::MD))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(tokens::space::SM))
                            .when(!self.show_sidebar, |d| {
                                d.child(self.icon_button(
                                    "header-show-sidebar",
                                    "☰",
                                    cx.listener(|this, _, _, cx| this.toggle_sidebar(cx)),
                                ))
                            })
                            .child(
                                div()
                                    .text_color(hsla(t.text))
                                    .child(SharedString::from(self.project_name())),
                            )
                            .child(
                                div()
                                    .text_size(px(tokens::text::XS))
                                    .text_color(hsla(t.text_faint))
                                    .child(SharedString::from(
                                        self.cwd
                                            .clone()
                                            .unwrap_or_else(|| "Project workspace".into()),
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(tokens::space::SM))
                            .child(
                                div()
                                    .text_size(px(tokens::text::XS))
                                    .text_color(hsla(t.text_faint))
                                    .child("Ctrl Shift P  Commands"),
                            )
                            .child(
                                div()
                                    .px(px(6.0))
                                    .py(px(1.0))
                                    .text_size(px(tokens::text::XS))
                                    .text_color(hsla(t.text_muted))
                                    .bg(hsla(t.bg_elevated))
                                    .rounded(px(tokens::radius::SM))
                                    .child(SharedString::from(format!(
                                        "{tab_count} tab{}",
                                        if tab_count == 1 { "" } else { "s" }
                                    ))),
                            )
                            .child(self.icon_button(
                                "header-settings",
                                "⚙",
                                cx.listener(|this, _, _, cx| this.open_settings(cx)),
                            )),
                    ),
            )
            .when(self.show_tab_strip, |header| {
                header.child(
                    div()
                        .h(px(42.0))
                        .px(px(tokens::space::SM))
                        .flex()
                        .items_end()
                        .gap(px(2.0))
                        .children(self.tabs.iter().enumerate().map(|(index, tab)| {
                            let active = index == self.active_tab;
                            let is_ssh = tab.spawn.ssh_host.is_some();
                            let label = Self::tab_label(tab);
                            let indicator_color = self.tab_status_color(tab, active, t);
                            let tab_id = tab.id;
                            let is_dragging = self.drag_state.is_dragging
                                && self.drag_state.dragged_index == Some(index);
                            div()
                                .id(("workspace-tab", tab.id as usize))
                                .h(px(36.0))
                                .max_w(px(240.0))
                                .px(px(tokens::space::MD))
                                .flex()
                                .items_center()
                                .gap(px(tokens::space::SM))
                                .rounded(px(tokens::radius::MD))
                                .when(active, |d| d.bg(hsla(t.bg_panel)))
                                .when(!active, |d| {
                                    d.text_color(hsla(t.text_muted))
                                        .hover(|style| style.bg(hsla(t.bg_hover)))
                                })
                                .when(is_dragging, |d| d.opacity(0.5).bg(hsla(t.accent)))
                                .child(
                                    div()
                                        .id(("select-workspace-tab", tab.id as usize))
                                        .flex_1()
                                        .flex()
                                        .items_center()
                                        .gap(px(tokens::space::SM))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.select_tab(index, cx);
                                        }))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                                this.drag_state.dragged_index = Some(index);
                                                this.drag_state.drag_start = Some(ev.position);
                                                this.drag_state.is_dragging = false;
                                                cx.notify();
                                            }),
                                        )
                                        .on_mouse_move(cx.listener(
                                            move |this, ev: &MouseMoveEvent, _, cx| {
                                                if this.drag_state.dragged_index == Some(index) {
                                                    if let Some(start) = this.drag_state.drag_start
                                                    {
                                                        let dx = f32::from(ev.position.x)
                                                            - f32::from(start.x);
                                                        let dy = f32::from(ev.position.y)
                                                            - f32::from(start.y);
                                                        let delta = (dx * dx + dy * dy).sqrt();
                                                        if delta > 5.0 {
                                                            this.drag_state.is_dragging = true;
                                                        }
                                                    }
                                                    if this.drag_state.is_dragging {
                                                        this.drag_state.drag_start =
                                                            Some(ev.position);
                                                        // Find target tab index under cursor
                                                        // For simplicity, we'll handle reorder on mouse_up
                                                        cx.notify();
                                                    }
                                                }
                                            },
                                        ))
                                        .on_mouse_up(
                                            MouseButton::Left,
                                            cx.listener(move |this, _ev: &MouseUpEvent, _, cx| {
                                                if this.drag_state.dragged_index == Some(index)
                                                    && this.drag_state.is_dragging
                                                {
                                                    // Find the tab under the cursor by checking all tab positions
                                                    // For now, we'll use a simple approach: find nearest tab
                                                    let dragged =
                                                        this.drag_state.dragged_index.unwrap();
                                                    if dragged != index {
                                                        let tab = this.tabs.remove(dragged);
                                                        let insert_at = if dragged < index {
                                                            index - 1
                                                        } else {
                                                            index
                                                        };
                                                        this.tabs.insert(insert_at, tab);
                                                        // Update active_tab if it was affected
                                                        if this.active_tab == dragged {
                                                            this.active_tab = insert_at;
                                                        } else if dragged < this.active_tab
                                                            && insert_at >= this.active_tab
                                                        {
                                                            this.active_tab -= 1;
                                                        } else if dragged > this.active_tab
                                                            && insert_at <= this.active_tab
                                                        {
                                                            this.active_tab += 1;
                                                        }
                                                    }
                                                    this.drag_state = DragState::default();
                                                    cx.notify();
                                                } else if this.drag_state.dragged_index
                                                    == Some(index)
                                                {
                                                    // Click without drag - reset drag state
                                                    this.drag_state = DragState::default();
                                                    cx.notify();
                                                }
                                            }),
                                        )
                                        .child(
                                            div()
                                                .size(px(6.0))
                                                .rounded_full()
                                                .bg(hsla(indicator_color)),
                                        )
                                        .when(is_ssh, |d| {
                                            d.child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .text_size(px(10.0))
                                                    .text_color(hsla(t.accent))
                                                    .child("⇄"),
                                            )
                                        })
                                        .child(
                                            div()
                                                .overflow_hidden()
                                                .child(SharedString::from(label)),
                                        ),
                                )
                                // Close button
                                .child(
                                    div()
                                        .id(("close-workspace-tab", tab_id as usize))
                                        .size(px(22.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded(px(tokens::radius::SM))
                                        .text_color(hsla(t.text_faint))
                                        .hover(|style| {
                                            style.bg(hsla(t.bg_hover)).text_color(hsla(t.text))
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if this.active_tab < this.tabs.len()
                                                && this.tabs[this.active_tab].id == tab_id
                                            {
                                                this.close_active_tab(cx);
                                            }
                                        }))
                                        .child("×"),
                                )
                        }))
                        .child(
                            div()
                                .id("new-workspace-tab")
                                .h(px(30.0))
                                .mb(px(3.0))
                                .px(px(tokens::space::SM))
                                .flex()
                                .items_center()
                                .rounded(px(tokens::radius::MD))
                                .text_color(hsla(t.text_muted))
                                .hover(|style| style.bg(hsla(t.bg_hover)).text_color(hsla(t.text)))
                                .on_click(cx.listener(|this, _, _, cx| this.new_local_tab(cx)))
                                .child("+  New terminal"),
                        ),
                )
            })
    }

    fn status_color(&self) -> Rgb {
        let t = &self.theme;
        if let Some((state, _)) = self.program_status_summary() {
            return match state {
                tf_session::ProgramState::Blocked => t.warning,
                tf_session::ProgramState::Error => t.danger,
                tf_session::ProgramState::Working => t.accent,
                tf_session::ProgramState::Done => t.success,
                tf_session::ProgramState::Idle | tf_session::ProgramState::Clear => t.text_faint,
            };
        }
        match (self.exited(), self.last_exit) {
            (Some(_), _) => t.text_faint,
            (None, Some(c)) if c != 0 => t.danger,
            _ => t.success,
        }
    }

    fn status_bar(&self, grid: Option<GridSize>) -> impl IntoElement {
        let t = &self.theme;
        let mut left = format!(
            "{} command{}",
            self.commands,
            if self.commands == 1 { "" } else { "s" }
        );
        if let Some(code) = self.last_exit {
            left.push_str(&format!("  ·  last exit {code}"));
        }
        if let Some((state, detail)) = self.program_status_summary() {
            let label = match state {
                tf_session::ProgramState::Idle => "idle",
                tf_session::ProgramState::Working => "working",
                tf_session::ProgramState::Done => "done",
                tf_session::ProgramState::Blocked => "blocked",
                tf_session::ProgramState::Error => "error",
                tf_session::ProgramState::Clear => "",
            };
            if !label.is_empty() {
                left.push_str("  ·  ");
                left.push_str(label);
                if let Some(detail) = detail {
                    left.push_str(": ");
                    left.push_str(&detail);
                }
            }
        }
        let mut right = String::new();
        if self.hover_link.is_some() {
            right.push_str("Ctrl+click to open  ·  ");
        }
        if let Some(g) = grid {
            right.push_str(&format!("{}×{}", g.cols, g.rows));
        }
        div()
            .h(px(tokens::layout::STATUSBAR_H))
            .px(px(tokens::space::MD))
            .flex()
            .items_center()
            .justify_between()
            .text_size(px(tokens::text::XS))
            .text_color(hsla(t.text_faint))
            .bg(hsla(t.bg_app))
            .border_t_1()
            .border_color(hsla(t.border))
            .child(SharedString::from(left))
            .child(SharedString::from(right))
    }

    fn program_status_summary(&self) -> Option<(tf_session::ProgramState, Option<String>)> {
        self.session.as_ref().and_then(|session| {
            session.with(|processor| {
                let priority = |state| match state {
                    tf_session::ProgramState::Blocked => 5,
                    tf_session::ProgramState::Error => 4,
                    tf_session::ProgramState::Working => 3,
                    tf_session::ProgramState::Done => 2,
                    tf_session::ProgramState::Idle => 1,
                    tf_session::ProgramState::Clear => 0,
                };
                processor
                    .program_status()
                    .iter()
                    .max_by_key(|record| priority(record.report.state))
                    .map(|record| {
                        let detail = record
                            .report
                            .message
                            .clone()
                            .or_else(|| record.report.title.clone())
                            .or_else(|| record.report.app.clone());
                        (record.report.state, detail)
                    })
            })
        })
    }

    // ---- sidebar and settings ----------------------------------------------

    fn toggle_sidebar(&mut self, cx: &mut Context<'_, Self>) {
        self.show_sidebar = !self.show_sidebar;
        self.sidebar_peek = false;
        cx.notify();
    }

    fn open_settings(&mut self, cx: &mut Context<'_, Self>) {
        self.settings = Some(self.settings.unwrap_or_default());
        self.palette = None;
        self.search = None;
        cx.notify();
    }

    fn toggle_section(&mut self, section: SidebarSection, cx: &mut Context<'_, Self>) {
        if !self.collapsed_sections.remove(&section) {
            self.collapsed_sections.insert(section);
        }
        cx.notify();
    }

    fn open_palette(&mut self, cx: &mut Context<'_, Self>) {
        self.ssh_hosts = tf_pty::discover_ssh_hosts();
        self.palette = Some(PaletteState::default());
        cx.notify();
    }

    fn close_tab_at(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if index < self.tabs.len() {
            self.select_tab(index, cx);
            self.close_active_tab(cx);
        }
    }

    fn set_dark_theme(&mut self, dark: bool, cx: &mut Context<'_, Self>) {
        let input = if dark {
            ThemeInput::DARK
        } else {
            ThemeInput::LIGHT
        };
        self.theme = Arc::new(Theme::generate(input));
        cx.notify();
    }

    fn icon_button(
        &self,
        id: &'static str,
        glyph: &'static str,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let t = &self.theme;
        div()
            .id(id)
            .size(px(24.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(tokens::radius::SM))
            .text_color(hsla(t.text_faint))
            .hover(|style| style.bg(hsla(t.bg_hover)).text_color(hsla(t.text)))
            .on_click(on_click)
            .child(glyph)
    }

    /// A full-width clickable sidebar row.
    fn sidebar_row(
        &self,
        id: &'static str,
        label: &'static str,
        hint: &'static str,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let t = &self.theme;
        div()
            .id(id)
            .h(px(32.0))
            .px(px(tokens::space::SM))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(tokens::space::MD))
            .rounded(px(tokens::radius::MD))
            .text_size(px(tokens::text::MD))
            .text_color(hsla(t.text_muted))
            .hover(|style| style.bg(hsla(t.bg_hover)).text_color(hsla(t.text)))
            .on_click(on_click)
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(label),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(tokens::text::XS))
                    .text_color(hsla(t.text_faint))
                    .child(hint),
            )
    }

    fn section_header(
        &self,
        section: SidebarSection,
        id: &'static str,
        title: &'static str,
        action: Option<SectionAction>,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        let open = !self.collapsed_sections.contains(&section);
        div()
            .h(px(26.0))
            .mt(px(tokens::space::SM))
            .px(px(tokens::space::SM))
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .id(id)
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap(px(tokens::space::XS))
                    .text_size(px(tokens::text::XS))
                    .text_color(hsla(t.text_faint))
                    .hover(|style| style.text_color(hsla(t.text)))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_section(section, cx)))
                    .child(if open { "▾" } else { "▸" })
                    .child(title),
            )
            .children(action.map(|(action_id, glyph, handler)| {
                self.icon_button(action_id, glyph, move |ev, window, cx| {
                    handler(ev, window, cx)
                })
            }))
    }

    /// Last two path components, so rows stay readable at any sidebar width.
    fn short_path(path: &str) -> String {
        let trimmed = path.trim_end_matches(['/', '\\']);
        let parts: Vec<&str> = trimmed.rsplit(['/', '\\']).take(2).collect();
        match parts.as_slice() {
            [leaf, parent] if !parent.is_empty() => format!("{parent}/{leaf}"),
            [leaf, ..] if !leaf.is_empty() => (*leaf).to_owned(),
            _ => trimmed.to_owned(),
        }
    }

    /// Highest-priority program status line for a tab, if any program reports one.
    fn tab_status_text(tab: &WorkspaceTab) -> Option<String> {
        let session = tab.session.as_ref()?;
        session.with(|p| {
            let rank = |state| match state {
                tf_session::ProgramState::Blocked => 5,
                tf_session::ProgramState::Error => 4,
                tf_session::ProgramState::Working => 3,
                tf_session::ProgramState::Done => 2,
                tf_session::ProgramState::Idle => 1,
                tf_session::ProgramState::Clear => 0,
            };
            let status = p.program_status();
            let record = status.iter().max_by_key(|r| rank(r.report.state))?;
            let label = match record.report.state {
                tf_session::ProgramState::Blocked => "blocked",
                tf_session::ProgramState::Error => "error",
                tf_session::ProgramState::Working => "working",
                tf_session::ProgramState::Done => "done",
                tf_session::ProgramState::Idle | tf_session::ProgramState::Clear => return None,
            };
            let detail = record
                .report
                .message
                .clone()
                .or_else(|| record.report.title.clone())
                .or_else(|| record.report.app.clone());
            Some(match detail {
                Some(detail) => format!("{label} · {detail}"),
                None => label.to_owned(),
            })
        })
    }

    fn workspace_row(
        &self,
        index: usize,
        tab: &WorkspaceTab,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        let active = index == self.active_tab;
        let tab_id = tab.id as usize;
        let is_ssh = tab.spawn.ssh_host.is_some();
        let renaming = self
            .renaming
            .as_ref()
            .filter(|rename| rename.tab_id == tab.id)
            .map(|rename| rename.text.clone());
        let title = tab
            .name
            .clone()
            .or_else(|| tab.title.clone().filter(|title| !title.trim().is_empty()))
            .unwrap_or_else(|| Self::tab_label(tab));
        let mut subtitle = tab
            .cwd
            .as_deref()
            .map(Self::short_path)
            .unwrap_or_else(|| Self::tab_label(tab));
        if let Some(branch) = &tab.branch {
            subtitle = format!("⎇ {branch}  ·  {subtitle}");
        }
        if self.shells.len() > 1 && !is_ssh {
            subtitle = format!("{}  ·  {subtitle}", tab.spawn.profile.name);
        }
        let status = Self::tab_status_text(tab);
        let dot = self.tab_status_color(tab, active, t);
        let ring = tab.attention.map(|attention| attention.color(t));
        let note = tab
            .attention_note
            .clone()
            .filter(|_| tab.attention.is_some())
            .or(status);
        let note_color = ring.unwrap_or(dot);
        let pinned = tab.pinned;
        let dragged = DraggedWorkspace {
            index,
            title: SharedString::from(title.clone()),
            bg: t.bg_elevated,
            fg: t.text,
            border: t.border_strong,
        };
        let hover_bg = t.bg_hover;
        div()
            .id(("workspace-row", tab_id))
            .group(SharedString::from(format!("ws-row-{tab_id}")))
            .relative()
            .px(px(tokens::space::SM))
            .py(px(6.0))
            .flex()
            .items_start()
            .gap(px(tokens::space::SM))
            .rounded(px(tokens::radius::MD))
            .when(active, |d| d.bg(hsla(t.bg_selected)))
            .when(!active, |d| d.hover(|style| style.bg(hsla(t.bg_hover))))
            .drag_over::<DraggedWorkspace>(move |style, _, _, _| style.bg(hsla(hover_bg)))
            .on_drag(dragged, |dragged, _, _, cx| {
                cx.new(|_| DragPreview(dragged.clone()))
            })
            .on_drop(cx.listener(move |this, dragged: &DraggedWorkspace, _, cx| {
                this.move_tab(dragged.index, index, cx);
            }))
            .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                if ev.click_count() >= 2 {
                    this.begin_rename(index, cx);
                } else {
                    this.select_tab(index, cx);
                }
            }))
            .children(ring.map(|color| {
                div()
                    .absolute()
                    .left(px(0.0))
                    .top(px(6.0))
                    .bottom(px(6.0))
                    .w(px(2.0))
                    .rounded_full()
                    .bg(hsla(color))
            }))
            .child(
                div()
                    .mt(px(4.0))
                    .size(px(9.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .when_some(ring, |d, color| d.border_1().border_color(hsla(color)))
                    .child(div().size(px(5.0)).rounded_full().bg(hsla(dot))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(tokens::space::XS))
                            .text_color(hsla(if active { t.text } else { t.text_muted }))
                            .child(match renaming {
                                Some(text) => div()
                                    .px(px(tokens::space::XS))
                                    .rounded(px(tokens::radius::SM))
                                    .bg(hsla(t.bg_elevated))
                                    .border_1()
                                    .border_color(hsla(t.accent))
                                    .text_color(hsla(t.text))
                                    .child(SharedString::from(format!("{text}▏"))),
                                None => div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(SharedString::from(title)),
                            })
                            .when(is_ssh, |d| {
                                d.child(
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(hsla(t.accent))
                                        .child("⇄"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(tokens::text::XS))
                            .text_color(hsla(t.text_faint))
                            .child(SharedString::from(subtitle)),
                    )
                    .children(note.map(|note| {
                        div()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(tokens::text::XS))
                            .text_color(hsla(note_color))
                            .child(SharedString::from(note))
                    })),
            )
            .child(
                div()
                    .id(("workspace-row-pin", tab_id))
                    .flex_none()
                    .when(!pinned, |d| {
                        d.invisible()
                            .group_hover(SharedString::from(format!("ws-row-{tab_id}")), |style| {
                                style.visible()
                            })
                    })
                    .size(px(20.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(tokens::radius::SM))
                    .text_color(hsla(if pinned { t.accent } else { t.text_faint }))
                    .hover(|style| style.bg(hsla(t.bg_hover)).text_color(hsla(t.text)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_pin(index, cx);
                    }))
                    .child(if pinned { "★" } else { "☆" }),
            )
            .child(
                div()
                    .id(("workspace-row-close", tab_id))
                    .flex_none()
                    .invisible()
                    .group_hover(SharedString::from(format!("ws-row-{tab_id}")), |style| {
                        style.visible()
                    })
                    .size(px(20.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(tokens::radius::SM))
                    .text_color(hsla(t.text_faint))
                    .hover(|style| style.bg(hsla(t.bg_hover)).text_color(hsla(t.text)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.close_tab_at(index, cx);
                    }))
                    .child("×"),
            )
    }

    fn workspace_sidebar(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let t = &self.theme;
        let project = self.project_name();
        let branch_hint = match self
            .tabs
            .get(self.active_tab)
            .and_then(|tab| tab.branch.clone())
        {
            Some(branch) => format!("⎇ {branch}"),
            None => self
                .local_project_root
                .as_deref()
                .map(|root| Self::short_path(&root.to_string_lossy()))
                .unwrap_or_else(|| "no project".into()),
        };
        let attention_count = self
            .tabs
            .iter()
            .filter(|tab| tab.attention.is_some())
            .count();
        let attention_color = self
            .tabs
            .iter()
            .filter_map(|tab| tab.attention)
            .max()
            .map_or(t.accent, |level| level.color(t));
        let workspaces_open = !self
            .collapsed_sections
            .contains(&SidebarSection::Workspaces);
        let hosts_open = !self.collapsed_sections.contains(&SidebarSection::SshHosts);
        let project_open = !self.collapsed_sections.contains(&SidebarSection::Project);
        let rows: Vec<_> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| self.workspace_row(index, tab, cx))
            .collect();
        let hosts: Vec<_> = self
            .ssh_hosts
            .iter()
            .map(|host| {
                let target = host.clone();
                let label = SharedString::from(host.clone());
                div()
                    .id(SharedString::from(format!("sidebar-host-{host}")))
                    .h(px(28.0))
                    .px(px(tokens::space::SM))
                    .flex()
                    .items_center()
                    .gap(px(tokens::space::SM))
                    .rounded(px(tokens::radius::MD))
                    .text_color(hsla(t.text_muted))
                    .hover(|style| style.bg(hsla(t.bg_hover)).text_color(hsla(t.text)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.new_ssh_tab(target.clone(), cx);
                    }))
                    .child(div().text_color(hsla(t.accent)).child("⇄"))
                    .child(div().overflow_hidden().child(label))
            })
            .collect();
        let new_tab_action: ClickHandler = {
            let view = cx.entity();
            Box::new(move |_, _, cx| view.update(cx, |this, cx| this.new_local_tab(cx)))
        };
        let refresh_hosts: ClickHandler = {
            let view = cx.entity();
            Box::new(move |_, _, cx| {
                view.update(cx, |this, cx| {
                    this.ssh_hosts = tf_pty::discover_ssh_hosts();
                    cx.notify();
                });
            })
        };

        div()
            .w(px(self.sidebar_width))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(hsla(t.bg_app))
            .border_r_1()
            .border_color(hsla(t.border))
            .child(
                div()
                    .p(px(tokens::space::SM))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .px(px(tokens::space::SM))
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_color(hsla(t.text))
                                    .child(SharedString::from(project)),
                            )
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(px(tokens::text::XS))
                                    .text_color(hsla(t.text_faint))
                                    .child(SharedString::from(branch_hint)),
                            ),
                    )
                    .children((attention_count > 0).then(|| {
                        div()
                            .id("sidebar-attention")
                            .mr(px(tokens::space::XS))
                            .px(px(6.0))
                            .py(px(1.0))
                            .rounded_full()
                            .bg(hsla(attention_color))
                            .text_size(px(tokens::text::XS))
                            .text_color(hsla(t.accent_text))
                            .hover(|style| style.opacity(0.85))
                            .on_click(cx.listener(|this, _, _, cx| this.jump_to_attention(cx)))
                            .child(SharedString::from(format!("● {attention_count}")))
                    }))
                    .child(self.icon_button(
                        "sidebar-hide",
                        if self.show_sidebar { "‹" } else { "◧" },
                        cx.listener(|this, _, _, cx| this.toggle_sidebar(cx)),
                    )),
            )
            .child(
                div()
                    .px(px(tokens::space::SM))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(div().flex_1().child(self.sidebar_row(
                                "sidebar-new-session",
                                "+  New terminal",
                                "Ctrl Shift T",
                                cx.listener(|this, _, _, cx| this.new_local_tab(cx)),
                            )))
                            .child(self.icon_button(
                                "sidebar-shell-menu",
                                "▾",
                                cx.listener(|this, _, _, cx| {
                                    this.shell_menu = !this.shell_menu;
                                    cx.notify();
                                }),
                            )),
                    )
                    .child(self.sidebar_row(
                        "sidebar-palette",
                        "⌘  Command palette",
                        "Ctrl Shift P",
                        cx.listener(|this, _, _, cx| this.open_palette(cx)),
                    )),
            )
            .child(
                div()
                    .id("sidebar-scroll")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .px(px(tokens::space::SM))
                    .flex()
                    .flex_col()
                    .child(self.section_header(
                        SidebarSection::Workspaces,
                        "section-workspaces",
                        "WORKSPACES",
                        Some(("section-workspaces-add", "+", new_tab_action)),
                        cx,
                    ))
                    .when(workspaces_open, |d| {
                        d.child(div().flex().flex_col().gap(px(2.0)).children(rows))
                    })
                    .child(self.section_header(
                        SidebarSection::SshHosts,
                        "section-hosts",
                        "SSH HOSTS",
                        Some(("section-hosts-refresh", "↻", refresh_hosts)),
                        cx,
                    ))
                    .when(hosts_open, |d| {
                        if hosts.is_empty() {
                            d.child(
                                div()
                                    .px(px(tokens::space::SM))
                                    .py(px(4.0))
                                    .text_size(px(tokens::text::XS))
                                    .text_color(hsla(t.text_faint))
                                    .child("No hosts in ~/.ssh/config"),
                            )
                        } else {
                            d.child(div().flex().flex_col().children(hosts))
                        }
                    })
                    .child(self.section_header(
                        SidebarSection::Project,
                        "section-project",
                        "PROJECT",
                        None,
                        cx,
                    ))
                    .when(project_open, |d| {
                        d.child(self.sidebar_row(
                            "sidebar-tasks",
                            "Tasks",
                            "↗",
                            cx.listener(|this, _, _, _| this.write(b"tf task list\r")),
                        ))
                        .child(self.sidebar_row(
                            "sidebar-find",
                            "Find in terminal",
                            "Ctrl Shift F",
                            cx.listener(|this, _, _, cx| this.open_search(cx)),
                        ))
                        .child(self.sidebar_row(
                            "sidebar-doctor",
                            "Daemon diagnostics",
                            "↗",
                            cx.listener(|this, _, _, _| this.write(b"forged doctor\r")),
                        ))
                    }),
            )
            .child(
                div()
                    .p(px(tokens::space::SM))
                    .border_t_1()
                    .border_color(hsla(t.border))
                    .child(self.sidebar_row(
                        "sidebar-settings",
                        "⚙  Settings",
                        "Ctrl ,",
                        cx.listener(|this, _, _, cx| this.open_settings(cx)),
                    )),
            )
    }

    /// Dropdown listing the shells available on this machine.
    fn shell_menu_overlay(&self, cx: &mut Context<'_, Self>) -> Option<impl IntoElement> {
        if !self.shell_menu {
            return None;
        }
        let t = &self.theme;
        let default = self.local_spawn.profile.name.clone();
        let items: Vec<_> = self
            .shells
            .iter()
            .enumerate()
            .map(|(index, shell)| {
                let is_default = shell.name == default;
                div()
                    .id(SharedString::from(format!("shell-menu-{index}")))
                    .h(px(28.0))
                    .px(px(tokens::space::SM))
                    .flex()
                    .items_center()
                    .justify_between()
                    .rounded(px(tokens::radius::MD))
                    .text_color(hsla(t.text_muted))
                    .hover(|style| style.bg(hsla(t.bg_hover)).text_color(hsla(t.text)))
                    .on_click(cx.listener(move |this, _, _, cx| this.new_shell_tab(index, cx)))
                    .child(SharedString::from(shell.name.clone()))
                    .child(
                        div()
                            .text_size(px(tokens::text::XS))
                            .text_color(hsla(t.text_faint))
                            .child(if is_default { "default" } else { "" }),
                    )
            })
            .collect();
        Some(
            div()
                .id("shell-menu-scrim")
                .absolute()
                .top(px(0.0))
                .bottom(px(0.0))
                .left(px(0.0))
                .right(px(0.0))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.shell_menu = false;
                    cx.notify();
                }))
                .child(
                    div()
                        .id("shell-menu")
                        .absolute()
                        .top(px(74.0))
                        .left(px(tokens::space::SM))
                        .w(px((self.sidebar_width - 2.0 * tokens::space::SM).max(180.0)))
                        .p(px(tokens::space::XS))
                        .flex()
                        .flex_col()
                        .rounded(px(tokens::radius::LG))
                        .bg(hsla(t.bg_elevated))
                        .border_1()
                        .border_color(hsla(t.border_strong))
                        .shadow_lg()
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .px(px(tokens::space::SM))
                                .py(px(tokens::space::XS))
                                .text_size(px(tokens::text::XS))
                                .text_color(hsla(t.text_faint))
                                .child("NEW TERMINAL WITH…"),
                        )
                        .children(items),
                ),
        )
    }

    // ---- settings panel ----------------------------------------------------

    fn settings_section_title(&self, title: &'static str) -> impl IntoElement {
        div()
            .mt(px(tokens::space::LG))
            .mb(px(tokens::space::SM))
            .px(px(2.0))
            .text_size(px(tokens::text::XS))
            .text_color(hsla(self.theme.text_faint))
            .child(title)
    }

    /// A rounded group of rows with hairline dividers between them.
    fn settings_card(&self, rows: Vec<gpui::AnyElement>) -> gpui::AnyElement {
        let t = &self.theme;
        let mut children = Vec::with_capacity(rows.len() * 2);
        for (index, row) in rows.into_iter().enumerate() {
            if index > 0 {
                children.push(
                    div()
                        .h(px(1.0))
                        .mx(px(tokens::space::MD))
                        .bg(hsla(t.border))
                        .into_any_element(),
                );
            }
            children.push(row);
        }
        div()
            .flex()
            .flex_col()
            .rounded(px(tokens::radius::LG))
            .bg(hsla(t.bg_panel))
            .border_1()
            .border_color(hsla(t.border))
            .children(children)
            .into_any_element()
    }

    fn setting_row(
        &self,
        label: &'static str,
        description: &'static str,
        control: impl IntoElement,
    ) -> gpui::AnyElement {
        let t = &self.theme;
        div()
            .px(px(tokens::space::MD))
            .py(px(tokens::space::MD))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(tokens::space::XL))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(px(tokens::text::MD))
                            .text_color(hsla(t.text))
                            .child(label),
                    )
                    .when(!description.is_empty(), |d| {
                        d.child(
                            div()
                                .text_size(px(tokens::text::SM))
                                .line_height(px(17.0))
                                .text_color(hsla(t.text_faint))
                                .child(description),
                        )
                    }),
            )
            .child(div().flex_none().child(control))
            .into_any_element()
    }

    fn toggle_switch(
        &self,
        id: &'static str,
        on: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let t = &self.theme;
        div()
            .id(id)
            .w(px(38.0))
            .h(px(22.0))
            .p(px(2.0))
            .flex()
            .items_center()
            .when(on, |d| d.justify_end())
            .rounded_full()
            .cursor_pointer()
            .bg(hsla(if on { t.accent } else { t.bg_hover }))
            .border_1()
            .border_color(hsla(if on { t.accent } else { t.border_strong }))
            .on_click(on_click)
            .child(div().size(px(16.0)).rounded_full().bg(hsla(if on {
                t.accent_text
            } else {
                t.text_muted
            })))
    }

    fn button(
        &self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let t = &self.theme;
        div()
            .id(id)
            .h(px(28.0))
            .min_w(px(28.0))
            .px(px(tokens::space::MD))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(tokens::radius::MD))
            .cursor_pointer()
            .bg(hsla(t.bg_elevated))
            .border_1()
            .border_color(hsla(t.border_strong))
            .text_color(hsla(t.text))
            .hover(|style| style.bg(hsla(t.bg_hover)))
            .on_click(on_click)
            .child(label.into())
    }

    /// Joined options of which exactly one is selected.
    fn segmented(
        &self,
        id_prefix: &'static str,
        options: Vec<(SharedString, bool, ClickHandler)>,
    ) -> impl IntoElement {
        let t = &self.theme;
        div()
            .flex()
            .rounded(px(tokens::radius::MD))
            .border_1()
            .border_color(hsla(t.border_strong))
            .bg(hsla(t.bg_elevated))
            .overflow_hidden()
            .children(
                options
                    .into_iter()
                    .enumerate()
                    .map(|(index, (label, selected, handler))| {
                        div()
                            .id(SharedString::from(format!("{id_prefix}-{index}")))
                            .h(px(28.0))
                            .px(px(tokens::space::MD))
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .when(index > 0, |d| d.border_l_1().border_color(hsla(t.border)))
                            .when(selected, |d| {
                                d.bg(hsla(t.accent)).text_color(hsla(t.accent_text))
                            })
                            .when(!selected, |d| {
                                d.text_color(hsla(t.text_muted)).hover(|style| {
                                    style.bg(hsla(t.bg_hover)).text_color(hsla(t.text))
                                })
                            })
                            .on_click(move |ev, window, cx| handler(ev, window, cx))
                            .child(label)
                    }),
            )
    }

    fn theme_card(
        &self,
        id: &'static str,
        label: &'static str,
        input: ThemeInput,
        selected: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let t = &self.theme;
        let preview = Theme::generate(input);
        div()
            .id(id)
            .flex()
            .flex_col()
            .gap(px(tokens::space::SM))
            .cursor_pointer()
            .on_click(on_click)
            .child(
                div()
                    .w(px(168.0))
                    .h(px(92.0))
                    .p(px(tokens::space::MD))
                    .flex()
                    .flex_col()
                    .gap(px(7.0))
                    .rounded(px(tokens::radius::LG))
                    .bg(hsla(preview.bg_app))
                    .border_2()
                    .border_color(hsla(if selected { t.accent } else { t.border_strong }))
                    .child(
                        div()
                            .h(px(8.0))
                            .w(px(64.0))
                            .rounded_full()
                            .bg(hsla(preview.text)),
                    )
                    .child(
                        div()
                            .h(px(6.0))
                            .w(px(112.0))
                            .rounded_full()
                            .bg(hsla(preview.text_muted)),
                    )
                    .child(
                        div()
                            .h(px(6.0))
                            .w(px(84.0))
                            .rounded_full()
                            .bg(hsla(preview.accent)),
                    ),
            )
            .child(
                div()
                    .text_color(hsla(if selected { t.text } else { t.text_muted }))
                    .child(label),
            )
    }

    /// Row of key caps for a shortcut written as space-separated keys.
    fn key_caps(&self, shortcut: &'static str) -> impl IntoElement {
        let t = &self.theme;
        div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .children(shortcut.split(' ').map(|key| {
                div()
                    .min_w(px(22.0))
                    .h(px(22.0))
                    .px(px(6.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(tokens::radius::SM))
                    .bg(hsla(t.bg_elevated))
                    .border_1()
                    .border_color(hsla(t.border_strong))
                    .text_size(px(tokens::text::XS))
                    .text_color(hsla(t.text))
                    .child(SharedString::from(key))
            }))
    }

    fn settings_general(&self, cx: &mut Context<'_, Self>) -> Vec<gpui::AnyElement> {
        let default_shell = self.local_spawn.profile.name.clone();
        let shells: Vec<(SharedString, bool, ClickHandler)> = self
            .shells
            .iter()
            .enumerate()
            .map(|(index, shell)| {
                let view = cx.entity();
                let handler: ClickHandler = Box::new(move |_, _, cx| {
                    view.update(cx, |this, cx| this.set_default_shell(index, cx));
                });
                (
                    SharedString::from(shell.name.clone()),
                    shell.name == default_shell,
                    handler,
                )
            })
            .collect();
        vec![
            self.settings_section_title("LAYOUT").into_any_element(),
            self.settings_card(vec![
                self.setting_row(
                    "Sidebar",
                    "Workspaces, SSH hosts and project actions.",
                    self.toggle_switch(
                        "set-sidebar",
                        self.show_sidebar,
                        cx.listener(|this, _, _, cx| this.toggle_sidebar(cx)),
                    ),
                ),
                self.setting_row(
                    "Open sidebar on hover",
                    "While the sidebar is hidden, move the pointer to the left edge to peek at it.",
                    self.toggle_switch(
                        "set-sidebar-hover",
                        self.sidebar_hover,
                        cx.listener(|this, _, _, cx| {
                            this.sidebar_hover = !this.sidebar_hover;
                            this.sidebar_peek = false;
                            cx.notify();
                        }),
                    ),
                ),
                self.setting_row(
                    "Tab strip",
                    "Also show workspaces as horizontal tabs above the terminal.",
                    self.toggle_switch(
                        "set-tabstrip",
                        self.show_tab_strip,
                        cx.listener(|this, _, _, cx| {
                            this.show_tab_strip = !this.show_tab_strip;
                            cx.notify();
                        }),
                    ),
                ),
                self.setting_row(
                    "Status bar",
                    "Command count, last exit code, program status and grid size.",
                    self.toggle_switch(
                        "set-statusbar",
                        self.show_status_bar,
                        cx.listener(|this, _, _, cx| {
                            this.show_status_bar = !this.show_status_bar;
                            cx.notify();
                        }),
                    ),
                ),
            ]),
            self.settings_section_title("SHELL").into_any_element(),
            self.settings_card(vec![self.setting_row(
                "Default shell",
                "Used by New terminal and Ctrl Shift T. Pick another from the arrow beside New terminal.",
                self.segmented("set-shell", shells),
            )]),
        ]
    }

    fn settings_appearance(&self, cx: &mut Context<'_, Self>) -> Vec<gpui::AnyElement> {
        let t = &self.theme;
        let dark = self.theme.is_dark;
        let size = self.font_size;
        let width = self.sidebar_width;
        let widths: Vec<(SharedString, bool, ClickHandler)> = SIDEBAR_WIDTHS
            .iter()
            .map(|(w, label)| {
                let w = *w;
                let view = cx.entity();
                let handler: ClickHandler = Box::new(move |_, _, cx| {
                    view.update(cx, |this, cx| {
                        this.sidebar_width = w;
                        cx.notify();
                    });
                });
                (SharedString::from(*label), (width - w).abs() < 1.0, handler)
            })
            .collect();
        vec![
            self.settings_section_title("THEME").into_any_element(),
            div()
                .flex()
                .gap(px(tokens::space::LG))
                .child(self.theme_card(
                    "set-theme-dark",
                    "Dark",
                    ThemeInput::DARK,
                    dark,
                    cx.listener(|this, _, _, cx| this.set_dark_theme(true, cx)),
                ))
                .child(self.theme_card(
                    "set-theme-light",
                    "Light",
                    ThemeInput::LIGHT,
                    !dark,
                    cx.listener(|this, _, _, cx| this.set_dark_theme(false, cx)),
                ))
                .into_any_element(),
            self.settings_section_title("TERMINAL TEXT")
                .into_any_element(),
            self.settings_card(vec![
                self.setting_row(
                    "Font size",
                    "Ctrl +, Ctrl - and Ctrl 0 also work inside the terminal.",
                    div()
                        .flex()
                        .items_center()
                        .gap(px(tokens::space::SM))
                        .child(self.button(
                            "set-font-dec",
                            "−",
                            cx.listener(|this, _, _, cx| {
                                this.font_size = (this.font_size - 1.0).max(8.0);
                                cx.notify();
                            }),
                        ))
                        .child(
                            div()
                                .w(px(44.0))
                                .flex()
                                .justify_center()
                                .text_color(hsla(t.text))
                                .child(SharedString::from(format!("{size:.0} pt"))),
                        )
                        .child(self.button(
                            "set-font-inc",
                            "+",
                            cx.listener(|this, _, _, cx| {
                                this.font_size = (this.font_size + 1.0).min(32.0);
                                cx.notify();
                            }),
                        ))
                        .child(self.button(
                            "set-font-reset",
                            "Reset",
                            cx.listener(|this, _, _, cx| {
                                this.font_size = 14.0;
                                cx.notify();
                            }),
                        )),
                ),
                div()
                    .m(px(tokens::space::MD))
                    .px(px(tokens::space::MD))
                    .py(px(tokens::space::SM))
                    .rounded(px(tokens::radius::MD))
                    .bg(hsla(t.term_bg))
                    .border_1()
                    .border_color(hsla(t.border))
                    .font_family(self.font.family.clone())
                    .text_size(px(size))
                    .text_color(hsla(t.text))
                    .child(SharedString::from("❯ cargo test --workspace  # 0123456789"))
                    .into_any_element(),
            ]),
            self.settings_section_title("SIDEBAR").into_any_element(),
            self.settings_card(vec![self.setting_row(
                "Sidebar width",
                "",
                self.segmented("set-width", widths),
            )]),
        ]
    }

    fn settings_keybindings(&self) -> Vec<gpui::AnyElement> {
        let groups: &[(&'static str, &[(&'static str, &'static str)])] = &[
            (
                "TABS",
                &[
                    ("New terminal", "Ctrl Shift T"),
                    ("Close tab", "Ctrl Shift W"),
                    ("Next tab", "Ctrl Tab"),
                    ("Previous tab", "Ctrl Shift Tab"),
                    ("Jump to tab 1–9", "Ctrl 1…9"),
                ],
            ),
            (
                "WORKSPACES",
                &[
                    ("Toggle sidebar", "Ctrl Shift B"),
                    ("Rename workspace", "Ctrl Shift R"),
                    ("Pin or unpin workspace", "Ctrl Shift K"),
                    ("Move workspace up or down", "Ctrl Shift ↑ ↓"),
                    ("Jump to workspace needing attention", "Ctrl Shift U"),
                ],
            ),
            (
                "TERMINAL",
                &[
                    ("Command palette", "Ctrl Shift P"),
                    ("Find", "Ctrl Shift F"),
                    ("Copy", "Ctrl Shift C"),
                    ("Paste", "Ctrl Shift V"),
                    ("Increase, decrease, reset font", "Ctrl + − 0"),
                    ("Scroll to top or bottom", "Ctrl Shift Home End"),
                ],
            ),
            (
                "APPLICATION",
                &[("Open settings", "Ctrl ,"), ("Quit", "Ctrl Shift Q")],
            ),
        ];
        let t = &self.theme;
        let mut out = Vec::new();
        for (title, items) in groups {
            out.push(self.settings_section_title(title).into_any_element());
            out.push(
                self.settings_card(
                    items
                        .iter()
                        .map(|(label, shortcut)| {
                            div()
                                .px(px(tokens::space::MD))
                                .h(px(40.0))
                                .flex()
                                .items_center()
                                .justify_between()
                                .text_size(px(tokens::text::MD))
                                .text_color(hsla(t.text))
                                .child(*label)
                                .child(self.key_caps(shortcut))
                                .into_any_element()
                        })
                        .collect(),
                ),
            );
        }
        out
    }

    fn settings_advanced(&self, cx: &mut Context<'_, Self>) -> Vec<gpui::AnyElement> {
        let t = &self.theme;
        let info = format!(
            "TermForge {} · protocol v{} · {}",
            env!("CARGO_PKG_VERSION"),
            tf_proto::PROTOCOL_VERSION,
            self.spawn.profile.name
        );
        vec![
            self.settings_section_title("DIAGNOSTICS")
                .into_any_element(),
            self.settings_card(vec![
                self.setting_row(
                    "Build",
                    "",
                    div()
                        .text_size(px(tokens::text::SM))
                        .text_color(hsla(t.text_muted))
                        .child(SharedString::from(info)),
                ),
                self.setting_row(
                    "Copy diagnostics",
                    "Version, protocol, shell and working directory to the clipboard.",
                    self.button(
                        "set-copy-diag",
                        "Copy",
                        cx.listener(|this, _, _, cx| {
                            this.run_palette_action(PaletteAction::CopyDiagnostics, cx);
                        }),
                    ),
                ),
                self.setting_row(
                    "Daemon doctor",
                    "Runs `forged doctor` in the active terminal.",
                    self.button(
                        "set-doctor",
                        "Run",
                        cx.listener(|this, _, _, cx| {
                            this.settings = None;
                            this.run_palette_action(PaletteAction::DaemonDoctor, cx);
                        }),
                    ),
                ),
                self.setting_row(
                    "Restart session",
                    "Restarts the shell in the active workspace.",
                    self.button(
                        "set-restart",
                        "Restart",
                        cx.listener(|this, _, _, cx| {
                            this.settings = None;
                            this.start(cx);
                        }),
                    ),
                ),
            ]),
            div()
                .mt(px(tokens::space::LG))
                .px(px(2.0))
                .text_size(px(tokens::text::SM))
                .text_color(hsla(t.text_faint))
                .child("Settings apply to this window only and are not saved yet.")
                .into_any_element(),
        ]
    }

    fn settings_panel(&self, cx: &mut Context<'_, Self>) -> Option<impl IntoElement> {
        let page = self.settings?;
        let t = &self.theme;
        let (title, subtitle) = match page {
            SettingsPage::General => ("General", "Layout and the shell new terminals start with."),
            SettingsPage::Appearance => ("Appearance", "Theme, terminal text and sidebar size."),
            SettingsPage::Keybindings => {
                ("Keybindings", "Keyboard shortcuts available in TermForge.")
            }
            SettingsPage::Advanced => ("Advanced", "Diagnostics and session controls."),
        };
        let body = match page {
            SettingsPage::General => self.settings_general(cx),
            SettingsPage::Appearance => self.settings_appearance(cx),
            SettingsPage::Keybindings => self.settings_keybindings(),
            SettingsPage::Advanced => self.settings_advanced(cx),
        };
        let nav = SETTINGS_PAGES
            .iter()
            .enumerate()
            .map(|(i, (target, label))| {
                let target = *target;
                let id: &'static str = ["set-nav-0", "set-nav-1", "set-nav-2", "set-nav-3"][i];
                div()
                    .id(id)
                    .h(px(34.0))
                    .px(px(tokens::space::MD))
                    .flex()
                    .items_center()
                    .rounded(px(tokens::radius::MD))
                    .text_size(px(tokens::text::MD))
                    .cursor_pointer()
                    .when(target == page, |d| {
                        d.bg(hsla(t.bg_selected)).text_color(hsla(t.text))
                    })
                    .when(target != page, |d| {
                        d.text_color(hsla(t.text_muted))
                            .hover(|style| style.bg(hsla(t.bg_hover)).text_color(hsla(t.text)))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings = Some(target);
                        cx.notify();
                    }))
                    .child(*label)
            });
        Some(
            div()
                .id("settings-scrim")
                .absolute()
                .top(px(0.0))
                .bottom(px(0.0))
                .left(px(0.0))
                .right(px(0.0))
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::black().opacity(0.5))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.settings = None;
                    cx.notify();
                }))
                .child(
                    div()
                        .id("settings-panel")
                        .w(px(860.0))
                        .max_w_full()
                        .h(px(580.0))
                        .max_h_full()
                        .flex()
                        .rounded(px(tokens::radius::XL))
                        .bg(hsla(t.bg_app))
                        .border_1()
                        .border_color(hsla(t.border_strong))
                        .shadow_lg()
                        .overflow_hidden()
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .w(px(200.0))
                                .flex_none()
                                .p(px(tokens::space::MD))
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .bg(hsla(t.bg_panel))
                                .border_r_1()
                                .border_color(hsla(t.border))
                                .child(
                                    div()
                                        .px(px(tokens::space::MD))
                                        .pt(px(tokens::space::SM))
                                        .pb(px(tokens::space::MD))
                                        .text_size(px(tokens::text::LG))
                                        .text_color(hsla(t.text))
                                        .child("Settings"),
                                )
                                .children(nav),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .px(px(tokens::space::XL))
                                        .pt(px(tokens::space::LG))
                                        .pb(px(tokens::space::MD))
                                        .flex()
                                        .items_start()
                                        .justify_between()
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap(px(2.0))
                                                .child(
                                                    div()
                                                        .text_size(px(tokens::text::XL))
                                                        .text_color(hsla(t.text))
                                                        .child(title),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(tokens::text::SM))
                                                        .text_color(hsla(t.text_faint))
                                                        .child(subtitle),
                                                ),
                                        )
                                        .child(self.icon_button(
                                            "settings-close",
                                            "×",
                                            cx.listener(|this, _, _, cx| {
                                                this.settings = None;
                                                cx.notify();
                                            }),
                                        )),
                                )
                                .child(
                                    div()
                                        .id("settings-body")
                                        .flex_1()
                                        .overflow_y_scroll()
                                        .px(px(tokens::space::XL))
                                        .pb(px(tokens::space::XL))
                                        .flex()
                                        .flex_col()
                                        .text_size(px(tokens::text::MD))
                                        .text_color(hsla(t.text_muted))
                                        .children(body),
                                ),
                        ),
                ),
        )
    }

    /// Window-level mouse listeners. GPUI only accepts these during paint and
    /// drops them after each frame, so they are registered from the canvas
    /// paint callback every frame.
    fn register_mouse_listeners(view: &Entity<Self>, window: &mut Window) {
        let v = view.clone();
        window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble {
                v.update(cx, |this, cx| {
                    if this.settings.is_none() && this.grid_bounds.contains(&ev.position) {
                        this.mouse_down(ev, window, cx);
                    }
                });
            }
        });
        let v = view.clone();
        window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble {
                v.update(cx, |this, cx| {
                    if this.grid_bounds.contains(&ev.position)
                        || this.reported.is_some()
                        || this.selecting
                    {
                        this.mouse_move(ev, window, cx);
                    }
                });
            }
        });
        let v = view.clone();
        window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble {
                v.update(cx, |this, cx| {
                    if this.reported.is_some() || this.selecting {
                        this.mouse_up(ev, window, cx);
                    }
                });
            }
        });
    }

    /// What the canvas needs for one frame, gathered under one session lock.
    fn frame(&self, session: &SessionHandle, size: GridSize) -> Frame {
        if let Err(e) = session.resize(size) {
            tracing::warn!("resize failed: {e:#}");
        }
        session.with(|p| {
            let e = p.engine();
            let snap = e.snapshot();
            let top = snap.top_line_abs();
            let bottom = top + snap.lines.len();
            let visible = |m: &Match| m.end.line >= top && m.start.line < bottom;
            let mut overlay = Overlay {
                selection: self.selection.filter(|s| !s.is_empty()).map(|s| s.range(e)),
                link: self.hover_link.as_ref().map(|l| (l.start, l.end)),
                ..Overlay::default()
            };
            if let Some(s) = &self.search {
                overlay.matches = s
                    .matches
                    .iter()
                    .filter(|m| visible(m))
                    .map(|m| (m.start, m.end))
                    .collect();
                overlay.current_match = s
                    .current
                    .and_then(|i| s.matches.get(i))
                    .map(|m| (m.start, m.end));
            }
            let blocks = p
                .blocks()
                .iter()
                .map(|b| BlockMark {
                    start: b.prompt_line,
                    end: b.end_line,
                    state: b.state,
                    exit_code: b.exit_code,
                })
                .collect();
            Frame {
                snap,
                blocks,
                overlay,
                hitbox: None,
            }
        })
    }
}

struct Frame {
    snap: Snapshot,
    blocks: Vec<BlockMark>,
    overlay: Overlay,
    hitbox: Option<Hitbox>,
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = Arc::clone(&self.theme);
        let metrics = self.metrics(window);
        let font = self.font.clone();
        let font_size = px(self.font_size);
        let focused = self.focus.is_focused(window);
        let session = self.session.clone();
        let _grid = session.as_ref().map(|s| s.with(|p| p.engine().size()));
        let view: Entity<Self> = cx.entity();
        let mouse_mode = self.modes().mouse != MouseTracking::Off;
        let over_link = self.hover_link.is_some();

        let exit_overlay =
            self.exited().map(|code| {
                let exit_label = code.map_or_else(|| "unknown".to_owned(), |code| code.to_string());
                div()
                    .absolute()
                    .top(px(0.0))
                    .bottom(px(0.0))
                    .left(px(0.0))
                    .right(px(0.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .px(px(tokens::space::LG))
                            .py(px(tokens::space::MD))
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(tokens::space::MD))
                            .rounded(px(tokens::radius::LG))
                            .bg(hsla(theme.bg_elevated))
                            .border_1()
                            .border_color(hsla(theme.border_strong))
                            .text_color(hsla(theme.text_muted))
                            .shadow_lg()
                            .child(SharedString::from(format!(
                                "Session exited · code {exit_label}"
                            )))
                            .child(
                                div()
                                    .flex()
                                    .gap(px(tokens::space::SM))
                                    .child(
                                        div()
                                            .id("restart-exited-session")
                                            .px(px(tokens::space::MD))
                                            .py(px(tokens::space::SM))
                                            .rounded(px(tokens::radius::MD))
                                            .bg(hsla(theme.bg_selected))
                                            .hover(|style| style.bg(hsla(theme.bg_hover)))
                                            .on_click(cx.listener(|this, _, _, cx| this.start(cx)))
                                            .child("Restart"),
                                    )
                                    .child(
                                        div()
                                            .id("close-exited-tab")
                                            .px(px(tokens::space::MD))
                                            .py(px(tokens::space::SM))
                                            .rounded(px(tokens::radius::MD))
                                            .hover(|style| style.bg(hsla(theme.bg_hover)))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.close_active_tab(cx)
                                            }))
                                            .child("Close tab"),
                                    )
                                    .when(self.spawn.ssh_host.is_some(), |d| {
                                        d.child(
                                            div()
                                                .id("open-local-after-ssh")
                                                .px(px(tokens::space::MD))
                                                .py(px(tokens::space::SM))
                                                .rounded(px(tokens::radius::MD))
                                                .text_color(hsla(theme.accent))
                                                .hover(|style| style.bg(hsla(theme.bg_hover)))
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.new_local_tab(cx)
                                                }))
                                                .child("Open local"),
                                        )
                                    }),
                            ),
                    )
            });
        let error = self.error.clone().map(|e| {
            div()
                .p(px(tokens::space::XL))
                .text_color(hsla(theme.danger))
                .child(SharedString::from(format!(
                    "Could not start the shell: {e}"
                )))
        });

        let grid_canvas = canvas(
            {
                let view = view.clone();
                move |bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App| {
                    let cols = ((bounds.size.width - px(crate::paint::PAD_X * 2.0))
                        / metrics.cell_w)
                        .floor();
                    let rows = ((bounds.size.height - px(crate::paint::PAD_Y * 2.0))
                        / metrics.line_h)
                        .floor();
                    let size = GridSize::new(cols.max(2.0) as u16, rows.max(1.0) as u16);
                    let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
                    let frame = session.as_ref().map(|s| view.read(cx).frame(s, size));
                    frame.map(|f| Frame {
                        hitbox: Some(hitbox),
                        ..f
                    })
                }
            },
            {
                let view = view.clone();
                move |bounds, frame, window, cx| {
                    Self::register_mouse_listeners(&view, window);
                    let Some(frame) = frame else { return };
                    view.update(cx, |v, _| {
                        v.grid_bounds = bounds;
                        // Compute and store the actual grid size from canvas bounds
                        let cols = ((bounds.size.width - px(crate::paint::PAD_X * 2.0))
                            / metrics.cell_w)
                            .floor();
                        let rows = ((bounds.size.height - px(crate::paint::PAD_Y * 2.0))
                            / metrics.line_h)
                            .floor();
                        v.computed_grid_size =
                            Some(GridSize::new(cols.max(2.0) as u16, rows.max(1.0) as u16));
                    });
                    if let Some(hitbox) = &frame.hitbox {
                        let style = if over_link {
                            CursorStyle::PointingHand
                        } else if mouse_mode {
                            CursorStyle::Arrow
                        } else {
                            CursorStyle::IBeam
                        };
                        window.set_cursor_style(style, hitbox);
                    }
                    paint_grid(
                        PaintArgs {
                            snap: &frame.snap,
                            blocks: &frame.blocks,
                            overlay: &frame.overlay,
                            bounds,
                            theme: &theme,
                            font: &font,
                            font_size,
                            metrics,
                            focused,
                        },
                        window,
                        cx,
                    );
                }
            },
        )
        .size_full();

        let search_overlay = self.search_bar();
        let palette_overlay = self.command_palette();
        let settings_overlay = self.settings_panel(cx);
        let shell_menu_overlay = self.shell_menu_overlay(cx);

        div()
            .size_full()
            .flex()
            .relative()
            .bg(hsla(self.theme.bg_app))
            .font_family(self.ui_font.clone())
            .text_size(px(tokens::text::MD))
            .when(self.show_sidebar, |d| d.child(self.workspace_sidebar(cx)))
            .when(!self.show_sidebar && self.sidebar_hover, |d| {
                if self.sidebar_peek {
                    d.child(
                        div()
                            .id("sidebar-peek")
                            .absolute()
                            .top(px(0.0))
                            .bottom(px(0.0))
                            .left(px(0.0))
                            .shadow_lg()
                            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                                if !*hovered {
                                    this.sidebar_peek = false;
                                    cx.notify();
                                }
                            }))
                            .child(self.workspace_sidebar(cx)),
                    )
                } else {
                    d.child(
                        div()
                            .id("sidebar-hover-zone")
                            .absolute()
                            .top(px(0.0))
                            .bottom(px(0.0))
                            .left(px(0.0))
                            .w(px(6.0))
                            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                                if *hovered {
                                    this.sidebar_peek = true;
                                    cx.notify();
                                }
                            })),
                    )
                }
            })
            .child(
                div()
                    .id("workspace-main")
                    .flex_1()
                    .flex()
                    .flex_col()
                    .bg(hsla(self.theme.term_bg))
                    .child(self.header(cx))
                    .child(
                        div()
                            .id("terminal")
                            .relative()
                            .flex_1()
                            .overflow_hidden()
                            .track_focus(&self.focus)
                            .on_key_down(cx.listener(Self::key_down))
                            .on_scroll_wheel(cx.listener(Self::scroll_wheel))
                            .on_modifiers_changed(cx.listener(
                                |this, ev: &gpui::ModifiersChangedEvent, window, cx| {
                                    let at =
                                        this.hit(window.mouse_position(), window).map(|(_, p)| p);
                                    this.update_hover(ev.modifiers, at, cx);
                                },
                            ))
                            .child(grid_canvas)
                            .children(error)
                            .children(exit_overlay),
                    )
                    .when(self.show_status_bar, |d| {
                        d.child(self.status_bar(self.computed_grid_size))
                    }),
            )
            .children(search_overlay)
            .children(palette_overlay)
            .children(shell_menu_overlay)
            .children(settings_overlay)
    }
}
