//! The terminal view: owns one session and turns GPUI input into terminal
//! input, selections and searches.
//!
//! Everything terminal-specific (emulation, key and mouse encoding, text
//! selection, search, block tracking, colours) lives in framework-free
//! crates; this file only wires those onto GPUI events. Painting is in
//! `paint.rs`.
//!
//! `TerminalView` and its state live here along with the render entry point;
//! behaviour is split by concern into `tabs` (sessions, tabs, persistence),
//! `input` (keyboard, mouse, selection), `search`, `palette`, `chrome`
//! (header, status bar), `sidebar` and `settings`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use futures::StreamExt;
use gpui::{
    canvas, div, prelude::*, px, relative, App, Bounds, ClickEvent, ClipboardItem, Context,
    CursorStyle, DispatchPhase, ElementId, ElementInputHandler, Entity, FocusHandle, Hitbox,
    HitboxBehavior, KeyDownEvent, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, ScrollWheelEvent, SharedString, Subscription, Task, Window,
    WindowBackgroundAppearance,
};
use tf_config::{ContextConfig, CursorStyleConfig};
use tf_engine::text::{find_all, is_openable, link_at, Link, Match};
use tf_engine::{
    GridSize, MouseTracking, Scroll, Selection, SelectionKind, Snapshot, TerminalEngine,
};
use tf_input::{
    encode_focus, encode_key, encode_mouse, encode_paste, InputModes, Key, Mods, MouseAction,
    MouseButton as TMouse, Tracking,
};
use tf_session::{SessionEvent, SpawnOptions};
use tf_ui::{tokens, Rgb, Theme};

use crate::appearance::Appearance;
use crate::fonts::{ui_family, MonoFont};
use crate::paint::{hsla, paint_grid, with_alpha, BlockMark, Metrics, Overlay, PaintArgs};
use crate::panes::{Axis, Dir, PaneId, PaneTree, Rect as PaneRect};
use crate::remote_session::{self, Attach, SessionHandle};
use crate::surface::{self, DockTab, GitChange, Inspector};
use crate::updates::UpdateNotice;
use crate::workspace::{self, Layout, SavedChrome, SavedPanes, SavedTab};

mod accounts;
mod chrome;
mod customize;
mod ime;
mod input;
mod palette;
#[cfg(test)]
mod pane_tests;
mod search;
mod settings;
mod sidebar;
mod split;
mod tabs;
mod tasks;
mod workbench;

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

/// What the palette overlay is choosing between.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum PickerMode {
    #[default]
    Commands,
    Themes,
    MonoFonts,
    UiFonts,
    Contexts,
}

impl PickerMode {
    fn title(self) -> &'static str {
        match self {
            Self::Commands => "COMMAND PALETTE",
            Self::Themes => "THEME",
            Self::MonoFonts => "TERMINAL FONT",
            Self::UiFonts => "INTERFACE FONT",
            Self::Contexts => "CLIENT CONTEXT",
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Self::Commands => "Type a command…",
            Self::Themes => "Search themes…",
            Self::MonoFonts | Self::UiFonts => "Search fonts…",
            Self::Contexts => "Search clients and accounts…",
        }
    }

    fn is_font(self) -> bool {
        matches!(self, Self::MonoFonts | Self::UiFonts)
    }
}

/// One choice in a picker: the value to apply, its label and a hint.
#[derive(Debug, Clone)]
struct PickerItem {
    value: String,
    label: String,
    hint: String,
}

#[derive(Debug, Default)]
struct PaletteState {
    query: String,
    selected: usize,
    mode: PickerMode,
    items: Vec<PickerItem>,
}

#[derive(Debug, Clone)]
enum PaletteAction {
    NewTab,
    CloseTab,
    SplitRight,
    SplitDown,
    ClosePane,
    NextPane,
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
    ToggleContextPanel,
    ToggleUtilityDock,
    OpenSettings,
    NewShell(usize),
    SetDefaultShell(usize),
    NextAttention,
    RenameWorkspace,
    TogglePin,
    MoveUp,
    MoveDown,
    NewTask,
    ListTasks,
    StatusWorking,
    StatusDone,
    StatusClear,
    DaemonDoctor,
    ConnectSsh(String),
    CopyRemoteIntegration(String),
    SwitchLocal,
    PickTheme,
    PickMonoFont,
    PickUiFont,
    SetTheme(String),
    SetMonoFont(String),
    SetUiFont(String),
    ReloadAppearance,
    RevealThemes,
    RevealFonts,
    ExportTheme,
    PickContext,
    SetContext(String),
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
    (
        PaletteAction::SplitRight,
        "Panes: split right",
        "Ctrl Shift D",
    ),
    (
        PaletteAction::SplitDown,
        "Panes: split down",
        "Ctrl Shift E",
    ),
    (
        PaletteAction::ClosePane,
        "Panes: close focused pane",
        "Ctrl Shift X",
    ),
    (
        PaletteAction::NextPane,
        "Panes: focus next pane",
        "Ctrl Alt Arrows",
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
    (
        PaletteAction::ToggleContextPanel,
        "View: toggle context panel",
        "Ctrl Shift I",
    ),
    (
        PaletteAction::ToggleUtilityDock,
        "View: toggle utility dock",
        "Ctrl Shift J",
    ),
    (PaletteAction::OpenSettings, "Settings: open", "Ctrl ,"),
    (PaletteAction::PickTheme, "Appearance: choose theme…", ""),
    (
        PaletteAction::PickMonoFont,
        "Appearance: choose terminal font…",
        "",
    ),
    (
        PaletteAction::PickUiFont,
        "Appearance: choose interface font…",
        "",
    ),
    (
        PaletteAction::ReloadAppearance,
        "Appearance: reload config, themes and fonts",
        "",
    ),
    (
        PaletteAction::RevealThemes,
        "Appearance: open themes folder",
        "",
    ),
    (
        PaletteAction::RevealFonts,
        "Appearance: open fonts folder",
        "",
    ),
    (
        PaletteAction::ExportTheme,
        "Appearance: save current theme to themes folder",
        "",
    ),
    (
        PaletteAction::PickContext,
        "Project: choose client or account context…",
        "",
    ),
    (PaletteAction::NewTask, "Tasks: new task", ""),
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
    /// Set when TermForge launched this workspace to run a command.
    managed: Option<surface::Managed>,
    branch: Option<String>,
    attention: Option<Attention>,
    attention_note: Option<String>,
    /// Split layout. The fields above describe the focused pane; the other
    /// panes' state is parked in `panes` until focus moves to them.
    layout: PaneTree,
    panes: HashMap<PaneId, PaneSlot>,
}

/// Everything about one pane that `TerminalView` swaps in when it takes
/// focus, mirroring the per-session fields of [`WorkspaceTab`].
#[derive(Debug, Clone)]
struct PaneSlot {
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
    /// Why this pane wants a look, shown on its border until focused.
    attention: Option<Attention>,
}

/// A divider being dragged with the mouse.
/// Which edge of a docked region is being dragged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edge {
    Dock,
    Panel,
}

/// A resize in progress: the pointer position and region size at press time.
#[derive(Debug, Clone, Copy)]
struct EdgeDrag {
    edge: Edge,
    pointer: f32,
    size: f32,
}

#[derive(Debug, Clone)]
struct DividerDrag {
    path: Vec<bool>,
    axis: Axis,
    /// The split's rectangle as fractions of the pane area.
    parent: PaneRect,
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
    Worktrees,
    Projects,
    Tasks,
}

/// Project artifacts shown in the central workspace. These are deliberately
/// separate from terminal tabs: a shell session is a tool, not the workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum WorkspaceArtifact {
    #[default]
    Overview,
    Tasks,
    Sessions,
}

/// Tools hosted by the dock beneath the active workspace artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum DockTool {
    #[default]
    Terminal,
    Problems,
    Output,
}

/// Pages of the settings panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum SettingsPage {
    #[default]
    General,
    Appearance,
    Accounts,
    Keybindings,
    Advanced,
}

const SETTINGS_PAGES: &[(SettingsPage, &str)] = &[
    (SettingsPage::General, "General"),
    (SettingsPage::Appearance, "Appearance"),
    (SettingsPage::Accounts, "Accounts"),
    (SettingsPage::Keybindings, "Keybindings"),
    (SettingsPage::Advanced, "Advanced"),
];

/// Accent swatches offered in settings. The empty entry keeps the theme's own.
const ACCENTS: &[&str] = &[
    "", "#5e6ad2", "#3b82f6", "#14b8a6", "#22c55e", "#f59e0b", "#f97316", "#ef4444", "#ec4899",
    "#8b5cf6",
];

/// Sidebar widths offered in settings.
const SIDEBAR_WIDTHS: &[(f32, &str)] = &[(190.0, "Narrow"), (216.0, "Normal"), (260.0, "Wide")];

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
    /// Row height as a multiple of the font size.
    line_height: f32,
    cursor_style: CursorStyleConfig,
    /// Persisted appearance settings plus user themes and fonts.
    appearance: Appearance,
    scroll_accum: f32,
    cwd: Option<String>,
    title: Option<String>,
    last_exit: Option<i32>,
    commands: usize,
    error: Option<String>,
    /// Native IME preedit text. It is deliberately not sent to the PTY until
    /// the platform commits it.
    ime_preedit: String,

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
    next_pane_id: PaneId,
    divider_drag: Option<DividerDrag>,
    /// Bounds of the area the panes tile, from the last paint.
    pane_area: Bounds<Pixels>,
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
    /// The window draws its own titlebar controls (set each render).
    client_decorated: bool,
    show_status_bar: bool,
    /// Give the terminal dock the whole center area without changing sessions.
    terminal_maximized: bool,
    /// Hide dock content while leaving its tool strip available.
    dock_collapsed: bool,
    /// Artifact currently occupying the project workspace.
    workspace_artifact: WorkspaceArtifact,
    /// Tool currently occupying the bottom dock.
    dock_tool: DockTool,
    /// Optional inspector; a project opens directly on its working surface.
    show_context_panel: bool,
    dock_tab: DockTab,
    changes: Option<Vec<GitChange>>,
    changes_dir: Option<PathBuf>,
    /// Projects the daemon knows about, current first.
    projects: Vec<PathBuf>,
    dock_height: f32,
    /// Agent CLIs found on PATH, read when the Processes tab opens.
    agent_clis: Vec<(&'static str, &'static str)>,
    /// Workspace whose output the dock's Output tab is watching.
    dock_target: Option<u64>,
    /// The dock's session was moved there (interactive), not just watched.
    dock_moved: bool,
    /// Keystrokes go to the docked session instead of the canvas.
    dock_focus: bool,
    panel_width: f32,
    edge_drag: Option<EdgeDrag>,
    worktrees: Vec<surface::Worktree>,
    inspector: Inspector,
    diff_lines: Vec<String>,
    /// Non-owning session switcher beneath the canvas.
    show_utility_dock: bool,
    settings: Option<SettingsPage>,
    renaming: Option<RenameState>,
    account_editor: Option<accounts::AccountEditor>,
    pending_context_delete: Option<String>,
    /// Project tasks, the new-task form and command suggestions.
    tasks: tasks::TasksState,
    /// Shells found on this machine, for the picker and settings.
    shells: Vec<tf_pty::ShellProfile>,
    shell_menu: bool,
    /// Last layout handed to the daemon, to skip redundant saves.
    saved_layout: String,
    /// Honest continuity message when `forged` recreated shells rather than
    /// preserving their original processes.
    recovery_notice: Option<String>,
    /// When the notice appeared; it clears itself so it never lingers.
    recovery_at: Option<std::time::Instant>,
    update_notice: Option<UpdateNotice>,
    /// Client/account identity assigned to the local project.
    context_id: Option<String>,
    identity_check: crate::contexts::IdentityCheck,

    _pump: Option<Task<()>>,
    _update_check: Option<Task<()>>,
    /// Polls the config, themes and fonts for outside edits.
    _appearance_watch: Option<Task<()>>,
    _focus_subs: Vec<Subscription>,
}

impl TerminalView {
    pub fn new(
        mut spawn: SpawnOptions,
        project_root: Option<PathBuf>,
        appearance: Appearance,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let terminal = &appearance.config.terminal;
        let theme = Arc::new(appearance.theme());
        let font = MonoFont::new(cx, &terminal.font_family, terminal.ligatures);
        let ui_font = ui_family(cx, &appearance.config.ui.font_family, &font.family);
        let (font_size, line_height, cursor_style) = (
            terminal.font_size,
            terminal.line_height,
            terminal.cursor_style,
        );
        let focus = cx.focus_handle();
        window.focus(&focus);
        let subs = vec![
            cx.on_focus_in(&focus, window, |this, _, cx| this.focus_changed(true, cx)),
            cx.on_focus_out(&focus, window, |this, _, _, cx| {
                this.focus_changed(false, cx);
            }),
        ];
        let stored_context = project_root
            .clone()
            .and_then(remote_session::project_context);
        let context_id = stored_context.or_else(|| {
            appearance
                .config
                .contexts
                .first()
                .map(|context| context.id.clone())
        });
        let context = context_id.as_deref().and_then(|id| {
            appearance
                .config
                .contexts
                .iter()
                .find(|context| context.id == id)
        });
        crate::contexts::apply_to_env(&mut spawn.env, context);
        let identity_check = crate::contexts::inspect(project_root.as_deref(), context);
        if let (Some(root), Some(id)) = (&project_root, &context_id) {
            if let Err(error) = remote_session::set_project_context(root.clone(), Some(id.clone()))
            {
                tracing::debug!(%error, "could not persist default project context");
            }
        }
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
            ui_font,
            font,
            font_size,
            line_height,
            cursor_style,
            appearance,
            scroll_accum: 0.0,
            cwd: None,
            title: None,
            last_exit: None,
            commands: 0,
            error: None,
            ime_preedit: String::new(),
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
            next_pane_id: 1,
            divider_drag: None,
            pane_area: Bounds::default(),
            wake,
            history: 0,
            grid_bounds: Bounds::default(),
            computed_grid_size: None,
            drag_state: DragState::default(),
            collapsed_sections: std::collections::HashSet::from([SidebarSection::Tasks]),
            show_sidebar: true,
            sidebar_hover: true,
            sidebar_peek: false,
            sidebar_width: 216.0,
            show_tab_strip: true,
            client_decorated: false,
            show_status_bar: true,
            terminal_maximized: false,
            dock_collapsed: false,
            workspace_artifact: WorkspaceArtifact::Overview,
            dock_tool: DockTool::Terminal,
            show_context_panel: false,
            dock_tab: DockTab::default(),
            changes: None,
            changes_dir: None,
            projects: Vec::new(),
            dock_height: 168.0,
            agent_clis: Vec::new(),
            dock_target: None,
            dock_moved: false,
            dock_focus: false,
            panel_width: 320.0,
            edge_drag: None,
            worktrees: Vec::new(),
            inspector: Inspector::default(),
            diff_lines: Vec::new(),
            show_utility_dock: false,
            settings: None,
            renaming: None,
            account_editor: None,
            pending_context_delete: None,
            tasks: tasks::TasksState::default(),
            shells: tf_pty::discover_shells(),
            shell_menu: false,
            saved_layout: String::new(),
            recovery_notice: None,
            recovery_at: None,
            update_notice: None,
            context_id,
            identity_check,
            _pump: None,
            _update_check: None,
            _appearance_watch: None,
            _focus_subs: subs,
        };
        view.restore_workspace(cx);
        view.tasks = tasks::TasksState::for_project(view.local_project_root.as_deref());
        view.refresh_tasks(cx);
        view._pump = Some(cx.spawn(async move |this, cx| {
            while rx.next().await.is_some() {
                while rx.try_recv().is_ok() {}
                if this.update(cx, |view, cx| view.on_output(cx)).is_err() {
                    break;
                }
            }
        }));
        view._appearance_watch = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(2))
                .await;
            let alive = this.update(cx, |view, cx| {
                // A picker's live preview must not be thrown away.
                let picking = view
                    .palette
                    .as_ref()
                    .is_some_and(|p| p.mode != PickerMode::Commands);
                if !picking && view.appearance.changed_on_disk() {
                    view.reload_appearance(cx);
                }
                if view
                    .recovery_at
                    .is_some_and(|at| at.elapsed() > std::time::Duration::from_secs(12))
                {
                    view.recovery_notice = None;
                    view.recovery_at = None;
                    cx.notify();
                }
                // Background shells do not notify on output, so poll the
                // observed process state while it is on screen.
                if view.show_utility_dock && view.dock_tab == DockTab::Processes {
                    cx.notify();
                }
            });
            if alive.is_err() {
                break;
            }
        }));
        view.refresh_worktrees();
        view.refresh_projects();
        // Debug builds only: open chrome states for screenshot checks, e.g.
        // TERMFORGE_DEBUG_UI=dock:changes,diff or dock:processes,panel.
        #[cfg(debug_assertions)]
        if let Ok(spec) = std::env::var("TERMFORGE_DEBUG_UI") {
            view.debug_open(&spec, cx);
        }
        let update = cx.background_spawn(async { crate::updates::check() });
        view._update_check = Some(cx.spawn(async move |this, cx| match update.await {
            Ok(Some(notice)) => {
                let _ = this.update(cx, |view, cx| {
                    view.update_notice = Some(notice);
                    cx.notify();
                });
            }
            Ok(None) => {}
            Err(error) => tracing::warn!(%error, "update check failed"),
        }));
        view
    }
}

#[cfg(debug_assertions)]
impl TerminalView {
    fn debug_open(&mut self, spec: &str, cx: &mut Context<'_, Self>) {
        for part in spec.split(',') {
            match part {
                "dock:sessions" => self.set_dock_tab(DockTab::Sessions, cx),
                "dock:changes" => self.set_dock_tab(DockTab::Changes, cx),
                "dock:processes" => self.set_dock_tab(DockTab::Processes, cx),
                "diff" => {
                    if let Some(change) = self.changes.as_ref().and_then(|c| c.first().cloned()) {
                        self.open_diff(&change, cx);
                    }
                }
                "panel" => self.show_context_panel = true,
                _ => {}
            }
        }
        self.show_utility_dock = spec.contains("dock:");
        cx.notify();
    }
}

impl TerminalView {
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
        window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble {
                v.update(cx, |this, cx| {
                    if this.divider_drag.is_some() {
                        this.drag_divider(ev.position, cx);
                    }
                    if this.edge_drag.is_some() {
                        this.drag_edge(ev.position, cx);
                    }
                });
            }
        });
        let v = view.clone();
        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble {
                v.update(cx, |this, cx| {
                    if this.edge_drag.take().is_some() {
                        this.persist_layout();
                    }
                    this.end_divider_drag(cx);
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
    fn frame(&self, session: &SessionHandle, size: GridSize, interactive: bool) -> Frame {
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
                // Selection, links and search belong to the focused pane.
                overlay: if interactive {
                    overlay
                } else {
                    Overlay::default()
                },
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
        self.client_decorated = matches!(
            window.window_decorations(),
            gpui::Decorations::Client { .. }
        );
        let theme = Arc::clone(&self.theme);
        let metrics = self.metrics(window);
        let font = self.font.clone();
        let font_size = px(self.font_size);
        let cursor_style = self.cursor_style;
        let accent_bold = self.appearance.config.terminal.accent_bold;
        let window_opacity = self.appearance.config.ui.opacity.clamp(0.5, 1.0);
        window.set_background_appearance(if window_opacity < 0.999 {
            WindowBackgroundAppearance::Transparent
        } else {
            WindowBackgroundAppearance::Opaque
        });
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
                    let frame = session.as_ref().map(|s| view.read(cx).frame(s, size, true));
                    frame.map(|f| Frame {
                        hitbox: Some(hitbox),
                        ..f
                    })
                }
            },
            {
                let view = view.clone();
                let theme = Arc::clone(&theme);
                let font = font.clone();
                move |bounds, frame, window, cx| {
                    let paint_started = std::time::Instant::now();
                    Self::register_mouse_listeners(&view, window);
                    let focus = view.read(cx).focus.clone();
                    window.handle_input(&focus, ElementInputHandler::new(bounds, view.clone()), cx);
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
                            cursor_style,
                            accent_bold,
                        },
                        window,
                        cx,
                    );
                    crate::perf::record_grid_paint(paint_started.elapsed());
                }
            },
        )
        .size_full();

        // Split panes: the focused pane keeps the interactive canvas above;
        // parked panes get a plain read-only canvas each.
        let split_layout = self
            .tabs
            .get(self.active_tab)
            .filter(|tab| tab.layout.len() > 1)
            .map(|tab| {
                let rects = tab.layout.layout(PaneRect::new(0.0, 0.0, 1.0, 1.0));
                let parked: Vec<(PaneId, Option<Arc<SessionHandle>>, Option<Attention>)> = tab
                    .panes
                    .iter()
                    .map(|(id, slot)| (*id, slot.session.clone(), slot.attention))
                    .collect();
                let dividers = tab.layout.dividers(PaneRect::new(0.0, 0.0, 1.0, 1.0));
                (rects, tab.layout.focused(), parked, dividers)
            });
        let (single_content, split_content) = match split_layout {
            None => (Some((grid_canvas, error, exit_overlay)), None),
            Some((rects, focused_id, parked, dividers)) => {
                let mut focused_content = Some((grid_canvas, error, exit_overlay));
                let mut panes: Vec<gpui::AnyElement> = Vec::new();
                // Records where the panes tile so a divider drag can turn the
                // pointer position into a ratio.
                let area_probe = canvas(|_, _, _| (), {
                    let view = view.clone();
                    move |bounds, (), _, cx| {
                        view.update(cx, |v, _| v.pane_area = bounds);
                    }
                })
                .absolute()
                .size_full();
                panes.push(
                    div()
                        .absolute()
                        .size_full()
                        .child(area_probe)
                        .into_any_element(),
                );
                for (id, rect) in rects {
                    let mut pane = div()
                        .absolute()
                        .left(relative(rect.x))
                        .top(relative(rect.y))
                        .w(relative(rect.w))
                        .h(relative(rect.h))
                        .border_1()
                        .overflow_hidden();
                    if id == focused_id {
                        pane = pane.border_color(hsla(theme.accent));
                        if let Some((grid, error, exit)) = focused_content.take() {
                            pane = pane.child(grid).children(error).children(exit);
                        }
                    } else {
                        let (parked_session, attention) = parked
                            .iter()
                            .find(|(pane_id, ..)| *pane_id == id)
                            .map_or((None, None), |(_, session, attention)| {
                                (session.clone(), *attention)
                            });
                        let border = hsla(attention.map_or(theme.border, |a| a.color(&theme)));
                        let (view, theme, font) = (view.clone(), Arc::clone(&theme), font.clone());
                        let plain = canvas(
                            move |bounds: Bounds<Pixels>, _: &mut Window, cx: &mut App| {
                                let session = parked_session?;
                                let cols = ((bounds.size.width - px(crate::paint::PAD_X * 2.0))
                                    / metrics.cell_w)
                                    .floor();
                                let rows = ((bounds.size.height - px(crate::paint::PAD_Y * 2.0))
                                    / metrics.line_h)
                                    .floor();
                                let size =
                                    GridSize::new(cols.max(2.0) as u16, rows.max(1.0) as u16);
                                Some(view.read(cx).frame(&session, size, false))
                            },
                            move |bounds, frame, window, cx| {
                                let Some(frame) = frame else { return };
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
                                        focused: false,
                                        cursor_style,
                                        accent_bold,
                                    },
                                    window,
                                    cx,
                                );
                            },
                        )
                        .size_full();
                        pane = pane
                            .border_color(border)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                    this.focus_pane(id, cx);
                                }),
                            )
                            .on_scroll_wheel(cx.listener(
                                move |this, ev: &ScrollWheelEvent, window, cx| {
                                    this.scroll_parked(id, ev, window, cx);
                                    // Do not also scroll the focused pane.
                                    cx.stop_propagation();
                                },
                            ))
                            .child(plain);
                    }
                    panes.push(pane.into_any_element());
                }
                // Grab handles sit on top of the panes, centred on each split.
                const GRAB: f32 = 6.0;
                for (index, divider) in dividers.into_iter().enumerate() {
                    let p = divider.parent;
                    let handle = div().id(("pane-divider", index)).absolute();
                    let handle = match divider.axis {
                        Axis::Horizontal => handle
                            .left(relative(p.x + p.w * divider.ratio))
                            .ml(px(-GRAB / 2.0))
                            .w(px(GRAB))
                            .top(relative(p.y))
                            .h(relative(p.h))
                            .cursor_col_resize(),
                        Axis::Vertical => handle
                            .top(relative(p.y + p.h * divider.ratio))
                            .mt(px(-GRAB / 2.0))
                            .h(px(GRAB))
                            .left(relative(p.x))
                            .w(relative(p.w))
                            .cursor_row_resize(),
                    };
                    panes.push(
                        handle
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                    this.begin_divider_drag(&divider);
                                    cx.stop_propagation();
                                }),
                            )
                            .into_any_element(),
                    );
                }
                (None, Some(panes))
            }
        };

        let search_overlay = self.search_bar();
        let palette_overlay = self.command_palette();
        let settings_overlay = self.settings_panel(cx);
        let account_editor_overlay = self.account_editor_overlay(cx);
        let task_form_overlay = self.task_form_overlay(cx);
        let shell_menu_overlay = self.shell_menu_overlay(cx);
        let recovery_notice = self.recovery_notice.clone();
        let update_notice = self.update_notice.clone();

        div()
            .size_full()
            .opacity(window_opacity)
            .flex()
            .relative()
            .bg(hsla(self.theme.bg_app))
            .font_family(self.ui_font.clone())
            .text_size(px(tokens::text::MD))
            .text_color(hsla(self.theme.text))
            .when(self.show_sidebar, |d| d.child(self.workspace_sidebar(cx)))
            .child(
                div()
                    .id("workspace-main")
                    .flex_1()
                    .flex()
                    .flex_col()
                    .bg(hsla(self.theme.bg_app))
                    .min_w(px(0.0))
                    .bg(hsla(self.theme.bg_app))
                    .child(self.header(cx))
                    .children(recovery_notice.map(|notice| {
                        div()
                            .id("recovery-notice")
                            .h(px(26.0))
                            .px(px(tokens::space::MD))
                            .flex()
                            .items_center()
                            .justify_between()
                            .bg(hsla(self.theme.bg_toolbar))
                            .text_size(px(tokens::text::XS))
                            .text_color(hsla(self.theme.text_faint))
                            .child(SharedString::from(notice))
                            .child(
                                div()
                                    .id("dismiss-recovery-notice")
                                    .px(px(tokens::space::SM))
                                    .cursor_pointer()
                                    .text_color(hsla(self.theme.text_faint))
                                    .hover(|style| style.text_color(hsla(self.theme.text)))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.recovery_notice = None;
                                        cx.notify();
                                    }))
                                    .child("Dismiss"),
                            )
                    }))
                    .children(update_notice.map(|notice| {
                        let url = notice.url.clone();
                        div()
                            .id("update-notice")
                            .h(px(36.0))
                            .px(px(tokens::space::MD))
                            .flex()
                            .items_center()
                            .justify_between()
                            .bg(hsla(self.theme.bg_panel))
                            .border_b_1()
                            .border_color(hsla(self.theme.border))
                            .text_size(px(tokens::text::SM))
                            .text_color(hsla(self.theme.text_muted))
                            .child(SharedString::from(format!(
                                "TermForge {} is available · {}",
                                notice.version,
                                notice.notes.lines().next().unwrap_or("Verified release")
                            )))
                            .child(
                                div()
                                    .flex()
                                    .gap(px(tokens::space::MD))
                                    .child(
                                        div()
                                            .id("download-update")
                                            .cursor_pointer()
                                            .text_color(hsla(self.theme.accent))
                                            .on_click(move |_, _, cx| cx.open_url(&url))
                                            .child("Download"),
                                    )
                                    .child(
                                        div()
                                            .id("dismiss-update")
                                            .cursor_pointer()
                                            .text_color(hsla(self.theme.text_faint))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.update_notice = None;
                                                cx.notify();
                                            }))
                                            .child("Dismiss"),
                                    ),
                            )
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_h(px(0.0))
                            .flex()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .flex()
                                    .flex_col()
                                    .when(!self.terminal_maximized, |d| {
                                        d.child(self.workspace_tab_strip(cx))
                                    })
                                    .when(!self.terminal_maximized, |d| {
                                        d.child(self.workspace_artifact_content(cx))
                                    })
                                    .child(self.terminal_panel_header(cx))
                                    .when(!self.dock_collapsed, |d| {
                                        d.child(
                                        div()
                                            .id("terminal")
                                            .relative()
                                            .overflow_hidden()
                                            .bg(hsla(self.theme.term_bg))
                                            .track_focus(&self.focus)
                                            .on_key_down(cx.listener(Self::key_down))
                                            .on_scroll_wheel(cx.listener(Self::scroll_wheel))
                                            .on_modifiers_changed(cx.listener(
                                                |this,
                                                 ev: &gpui::ModifiersChangedEvent,
                                                 window,
                                                 cx| {
                                                    let at = this
                                                        .hit(window.mouse_position(), window)
                                                        .map(|(_, p)| p);
                                                    this.update_hover(ev.modifiers, at, cx);
                                                },
                                            ))
                                            .when(self.terminal_maximized, |d| d.flex_1())
                                            .when(!self.terminal_maximized, |d| {
                                                d.h(px(236.0)).flex_none()
                                            })
                                            .when(self.dock_tool != DockTool::Terminal, |d| {
                                                d.h(px(168.0)).flex_none()
                                            })
                                            .when_some(single_content, |d, (grid, error, exit)| {
                                                if self.dock_tool == DockTool::Terminal {
                                                    d.child(grid).children(error).children(exit)
                                                } else {
                                                    d
                                                }
                                            })
                                            .when(self.dock_tool == DockTool::Terminal, |d| {
                                                d.children(split_content.into_iter().flatten())
                                            })
                                            .when(self.dock_tool != DockTool::Terminal, |d| {
                                                d.child(self.dock_placeholder())
                                            }),
                                    )
                                    }),
                            )
                            .when(!self.terminal_maximized, |d| {
                                d.child(self.workbench_inspector(cx))
                            }),
                            .when(self.show_utility_dock, |d| d.child(self.utility_dock(metrics, cx))),
                    )
                    .when(self.show_status_bar, |d| {
                        d.child(self.status_bar(self.computed_grid_size))
                    }),
            )
            .when(self.client_decorated, |d| d.children(self.resize_edges()))
            // Absolute overlays must be painted after the workspace so their
            // hitboxes stay above its full-window terminal canvas.
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
            .children(search_overlay)
            .children(shell_menu_overlay)
            .children(task_form_overlay)
            .children(settings_overlay)
            .children(account_editor_overlay)
            // Painted last so a picker opened from settings is on top.
            .children(palette_overlay)
    }
}
