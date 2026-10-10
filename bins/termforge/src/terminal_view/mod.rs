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
    CursorStyle, DispatchPhase, ElementId, Entity, FocusHandle, Hitbox, HitboxBehavior,
    KeyDownEvent, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    ScrollWheelEvent, SharedString, Subscription, Task, Window,
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
use crate::panes::{Axis, Dir, PaneId, PaneTree, Rect as PaneRect};
use crate::remote_session::{self, Attach, SessionHandle};
use crate::workspace::{self, Layout, SavedPanes, SavedTab};

mod chrome;
mod input;
mod palette;
mod search;
mod settings;
mod sidebar;
mod split;
mod tabs;
mod tasks;

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
    (PaletteAction::OpenSettings, "Settings: open", "Ctrl ,"),
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
    Tasks,
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
    next_pane_id: PaneId,
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
    /// Project tasks, the new-task form and command suggestions.
    tasks: tasks::TasksState,
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
            next_pane_id: 1,
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
            tasks: tasks::TasksState::default(),
            shells: tf_pty::discover_shells(),
            shell_menu: false,
            saved_layout: String::new(),
            _pump: None,
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
        view
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

        // Split panes: the focused pane keeps the interactive canvas above;
        // parked panes get a plain read-only canvas each.
        let split_layout = self
            .tabs
            .get(self.active_tab)
            .filter(|tab| tab.layout.len() > 1)
            .map(|tab| {
                let rects = tab.layout.layout(PaneRect::new(0.0, 0.0, 1.0, 1.0));
                let parked: Vec<(PaneId, Option<Arc<SessionHandle>>)> = tab
                    .panes
                    .iter()
                    .map(|(id, slot)| (*id, slot.session.clone()))
                    .collect();
                (rects, tab.layout.focused(), parked)
            });
        let (single_content, split_content) = match split_layout {
            None => (Some((grid_canvas, error, exit_overlay)), None),
            Some((rects, focused_id, parked)) => {
                let mut focused_content = Some((grid_canvas, error, exit_overlay));
                let mut panes = Vec::new();
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
                        let parked_session = parked
                            .iter()
                            .find(|(pane_id, _)| *pane_id == id)
                            .and_then(|(_, session)| session.clone());
                        let border = hsla(theme.border);
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
                            .child(plain);
                    }
                    panes.push(pane);
                }
                (None, Some(panes))
            }
        };

        let search_overlay = self.search_bar();
        let palette_overlay = self.command_palette();
        let settings_overlay = self.settings_panel(cx);
        let task_form_overlay = self.task_form_overlay(cx);
        let shell_menu_overlay = self.shell_menu_overlay(cx);

        div()
            .size_full()
            .flex()
            .relative()
            .bg(hsla(self.theme.bg_app))
            .font_family(self.ui_font.clone())
            .text_size(px(tokens::text::MD))
            .text_color(hsla(self.theme.text))
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
                            .when_some(single_content, |d, (grid, error, exit)| {
                                d.child(grid).children(error).children(exit)
                            })
                            .children(split_content.into_iter().flatten()),
                    )
                    .when(self.show_status_bar, |d| {
                        d.child(self.status_bar(self.computed_grid_size))
                    }),
            )
            .children(search_overlay)
            .children(palette_overlay)
            .children(shell_menu_overlay)
            .children(task_form_overlay)
            .children(settings_overlay)
    }
}
