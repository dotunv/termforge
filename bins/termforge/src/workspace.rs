//! Workspace metadata that is not terminal state: git branch discovery,
//! the workbench workspaces and their default layouts, and the layout document persisted by
//! `forged` between UI launches.

use std::path::{Path, PathBuf};

use crate::panes::{AreaId, Axis, EditorKind, LogStreamState, PaneTree};

/// The top-level activities of the workbench.
///
/// Each workspace owns a layout of areas. Only [`Workspace::Build`] holds
/// terminal sessions (its layout is the active tab's pane tree); every other
/// workspace is a pure view layout whose areas never own a process, so
/// switching workspaces never starts, moves or closes a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Workspace {
    /// Work items and what needs attention.
    Work,
    /// Understand the project: files, source and structure.
    #[default]
    Explore,
    /// Change and run things: terminal sessions.
    Build,
    /// Understand and stage changes.
    Review,
    /// Running systems and agents.
    Operate,
}

impl Workspace {
    pub const ALL: [Self; 5] = [
        Self::Work,
        Self::Explore,
        Self::Build,
        Self::Review,
        Self::Operate,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Work => "Work",
            Self::Explore => "Explore",
            Self::Build => "Build",
            Self::Review => "Review",
            Self::Operate => "Operate",
        }
    }

    pub const fn id(self) -> &'static str {
        match self {
            Self::Work => "work",
            Self::Explore => "explore",
            Self::Build => "build",
            Self::Review => "review",
            Self::Operate => "operate",
        }
    }

    /// Parses a saved id. Ids written before the workspaces were named
    /// (`tasks`, `files`, `git`, `terminal`, `project`) still load.
    pub fn from_id(id: &str) -> Option<Self> {
        Some(match id {
            "work" | "tasks" => Self::Work,
            "explore" | "files" | "project" => Self::Explore,
            "build" | "terminal" => Self::Build,
            "review" | "git" => Self::Review,
            "operate" => Self::Operate,
            _ => return None,
        })
    }

    /// Whether this workspace's layout is the active tab's session tree.
    pub const fn hosts_sessions(self) -> bool {
        matches!(self, Self::Build)
    }

    /// Default session-free layout, or `None` for [`Workspace::Build`].
    /// `next_id` hands out area ids that are unique across the window.
    pub fn default_layout(self, mut next_id: impl FnMut() -> AreaId) -> Option<PaneTree> {
        let tree = match self {
            Self::Build => return None,
            Self::Work => PaneTree::new_with_editor(next_id(), EditorKind::TaskBoard),
            Self::Explore => PaneTree::new_with_editor(next_id(), EditorKind::CodeEditor),
            Self::Review => PaneTree::new_with_editor(next_id(), EditorKind::GitDiff),
            Self::Operate => {
                let environment = next_id();
                let services = next_id();
                let timeline = next_id();
                let mut tree = PaneTree::new_with_editor(environment, EditorKind::Environment);
                let _ = tree.split_with_editor(
                    environment,
                    Axis::Horizontal,
                    0.34,
                    services,
                    EditorKind::Services,
                );
                let _ = tree.split_with_editor(
                    services,
                    Axis::Vertical,
                    0.66,
                    timeline,
                    EditorKind::Timeline(LogStreamState::default()),
                );
                let _ = tree.set_focus(services);
                tree
            }
        };
        Some(tree)
    }
}

/// Branch name (or short commit id when detached) for the repository
/// containing `dir`, read straight from `.git` without spawning `git`.
pub fn git_branch(dir: &Path) -> Option<String> {
    for ancestor in dir.ancestors() {
        let dot_git = ancestor.join(".git");
        let git_dir = if dot_git.is_dir() {
            dot_git
        } else if dot_git.is_file() {
            // Worktrees and submodules: `.git` is a file pointing at the real dir.
            let text = std::fs::read_to_string(&dot_git).ok()?;
            let target = text.trim().strip_prefix("gitdir:")?.trim();
            let target = PathBuf::from(target);
            if target.is_absolute() {
                target
            } else {
                ancestor.join(target)
            }
        } else {
            continue;
        };
        let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
        let head = head.trim();
        return match head.strip_prefix("ref:") {
            Some(reference) => {
                let reference = reference.trim();
                Some(
                    reference
                        .strip_prefix("refs/heads/")
                        .unwrap_or(reference)
                        .to_owned(),
                )
            }
            None => Some(head.chars().take(7).collect()),
        };
    }
    None
}

/// The directory part of a reported working directory, which is
/// `host:path` when the shell integration included a host.
pub fn local_path(reported: &str) -> PathBuf {
    if let Some(index) = reported.find(':') {
        let rest = &reported[index + 1..];
        // A single letter before the colon is a Windows drive, not a host.
        if index > 1 && (rest.starts_with('/') || rest.starts_with('\\')) {
            return PathBuf::from(rest);
        }
    }
    PathBuf::from(reported)
}

/// One workspace as remembered across launches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedTab {
    pub session: String,
    pub pinned: bool,
    pub name: Option<String>,
    /// Split panes, when the workspace has more than one.
    pub panes: Option<SavedPanes>,
}

/// A pane tree whose leaves are positions in `sessions`, so the layout file
/// does not depend on runtime pane ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedPanes {
    pub tree: String,
    pub sessions: Vec<String>,
    /// Index into `sessions` of the focused pane.
    pub focus: usize,
}

/// Visibility and size of the dock and right panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedChrome {
    pub dock_open: bool,
    pub dock_height: u32,
    /// `sessions`, `changes` or `output`; unknown values fall back to the default tab.
    pub dock_tab: String,
    pub panel_open: bool,
    pub panel_width: u32,
    /// Daemon session shown interactively in the dock, if any. Runtime tab
    /// ids are ephemeral, so the durable session id is the only safe link.
    pub docked_session: Option<String>,
}

/// Project workbench environment and its per-environment presentation choices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedWorkbench {
    pub environment: String,
    pub file_mode: String,
    pub selected_file: Option<String>,
    pub git_diff_view: String,
    pub task_filter: String,
    pub task_sort: String,
    pub dock_tool: String,
    pub dock_collapsed: bool,
    pub terminal_maximized: bool,
    pub sidebar_width: u32,
}

/// Pan, zoom and lens of the graph canvas, in whole units so the layout stays
/// comparable (`Eq`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedCanvas {
    pub lens: String,
    /// Zoom times one thousand.
    pub zoom_milli: u32,
    pub pan_x: i32,
    pub pan_y: i32,
}

/// Order, names and pins of the workspaces in a window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Layout {
    pub tabs: Vec<SavedTab>,
    pub active: Option<String>,
    /// Dock and panel state; absent in files written by older builds.
    pub chrome: Option<SavedChrome>,
    /// Absent in layouts written before workbench environments were durable.
    pub workbench: Option<SavedWorkbench>,
    /// Encoded session-free area layout per workspace id. Older builds skip
    /// these lines.
    pub areas: Vec<(String, String)>,
    /// Absent in layouts written before the canvas state was saved.
    pub canvas: Option<SavedCanvas>,
}

const HEADER: &str = "termforge-layout 1";

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

impl Layout {
    pub fn encode(&self) -> String {
        let mut out = String::from(HEADER);
        out.push('\n');
        if let Some(active) = &self.active {
            out.push_str(&format!("A\t{active}\n"));
        }
        if let Some(c) = &self.chrome {
            out.push_str(&format!(
                "C\t{}\t{}\t{}\t{}\t{}\t{}\n",
                u8::from(c.dock_open),
                c.dock_height,
                c.dock_tab,
                u8::from(c.panel_open),
                c.panel_width,
                c.docked_session.as_deref().unwrap_or_default()
            ));
        }
        if let Some(workbench) = &self.workbench {
            out.push_str(&format!(
                "W\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                workbench.environment,
                workbench.file_mode,
                escape(workbench.selected_file.as_deref().unwrap_or_default()),
                workbench.git_diff_view,
                workbench.task_filter,
                workbench.task_sort,
                workbench.dock_tool,
                u8::from(workbench.dock_collapsed),
                u8::from(workbench.terminal_maximized),
                workbench.sidebar_width,
            ));
        }
        if let Some(c) = &self.canvas {
            out.push_str(&format!(
                "V\t{}\t{}\t{}\t{}\n",
                c.lens, c.zoom_milli, c.pan_x, c.pan_y
            ));
        }
        for (workspace, tree) in &self.areas {
            out.push_str(&format!("L\t{workspace}\t{tree}\n"));
        }
        for tab in &self.tabs {
            out.push_str(&format!(
                "T\t{}\t{}\t{}\n",
                tab.session,
                u8::from(tab.pinned),
                escape(tab.name.as_deref().unwrap_or_default())
            ));
            // Applies to the preceding `T` line; older builds skip it.
            if let Some(panes) = &tab.panes {
                out.push_str(&format!(
                    "S\t{}\t{}\t{}\n",
                    panes.tree,
                    panes.focus,
                    panes.sessions.join(",")
                ));
            }
        }
        out
    }

    /// Unknown or malformed input yields an empty layout rather than an error:
    /// the layout is a convenience and must never block startup.
    pub fn decode(text: &str) -> Self {
        let mut lines = text.lines();
        if lines.next() != Some(HEADER) {
            return Self::default();
        }
        let mut layout = Self::default();
        for line in lines {
            let mut fields = line.split('\t');
            match (fields.next(), fields.next()) {
                (Some("A"), Some(id)) => layout.active = Some(id.to_owned()),
                (Some("C"), Some(dock_open)) => {
                    let dock_height = fields.next().and_then(|v| v.parse().ok());
                    let dock_tab = fields.next().map(str::to_owned);
                    let panel_open = fields.next().map(|v| v == "1");
                    let panel_width = fields.next().and_then(|v| v.parse().ok());
                    // The sixth field was added after the initial chrome
                    // format. Older layouts intentionally decode as no
                    // docked surface.
                    let docked_session =
                        fields.next().map(str::to_owned).filter(|id| !id.is_empty());
                    if let (
                        Some(dock_height),
                        Some(dock_tab),
                        Some(panel_open),
                        Some(panel_width),
                    ) = (dock_height, dock_tab, panel_open, panel_width)
                    {
                        layout.chrome = Some(SavedChrome {
                            dock_open: dock_open == "1",
                            dock_height,
                            dock_tab,
                            panel_open,
                            panel_width,
                            docked_session,
                        });
                    }
                }
                (Some("W"), Some(environment)) => {
                    let file_mode = fields.next();
                    let selected_file = fields.next().map(unescape).filter(|path| !path.is_empty());
                    let git_diff_view = fields.next();
                    let task_filter = fields.next();
                    let task_sort = fields.next();
                    let dock_tool = fields.next();
                    let dock_collapsed = fields.next().map(|value| value == "1");
                    let terminal_maximized = fields.next().map(|value| value == "1");
                    let sidebar_width = fields.next().and_then(|value| value.parse().ok());
                    if let (
                        Some(file_mode),
                        Some(git_diff_view),
                        Some(task_filter),
                        Some(task_sort),
                        Some(dock_tool),
                        Some(dock_collapsed),
                        Some(terminal_maximized),
                        Some(sidebar_width),
                    ) = (
                        file_mode,
                        git_diff_view,
                        task_filter,
                        task_sort,
                        dock_tool,
                        dock_collapsed,
                        terminal_maximized,
                        sidebar_width,
                    ) {
                        layout.workbench = Some(SavedWorkbench {
                            environment: environment.to_owned(),
                            file_mode: file_mode.to_owned(),
                            selected_file,
                            git_diff_view: git_diff_view.to_owned(),
                            task_filter: task_filter.to_owned(),
                            task_sort: task_sort.to_owned(),
                            dock_tool: dock_tool.to_owned(),
                            dock_collapsed,
                            terminal_maximized,
                            sidebar_width,
                        });
                    }
                }
                (Some("V"), Some(lens)) => {
                    let zoom_milli = fields.next().and_then(|v| v.parse().ok());
                    let pan_x = fields.next().and_then(|v| v.parse().ok());
                    let pan_y = fields.next().and_then(|v| v.parse().ok());
                    if let (Some(zoom_milli), Some(pan_x), Some(pan_y)) = (zoom_milli, pan_x, pan_y)
                    {
                        layout.canvas = Some(SavedCanvas {
                            lens: lens.to_owned(),
                            zoom_milli,
                            pan_x,
                            pan_y,
                        });
                    }
                }
                (Some("L"), Some(workspace)) => {
                    if let Some(tree) = fields.next().filter(|tree| !tree.is_empty()) {
                        layout.areas.push((workspace.to_owned(), tree.to_owned()));
                    }
                }
                (Some("T"), Some(id)) => {
                    let pinned = fields.next() == Some("1");
                    let name = fields
                        .next()
                        .map(unescape)
                        .filter(|name| !name.trim().is_empty());
                    layout.tabs.push(SavedTab {
                        session: id.to_owned(),
                        pinned,
                        name,
                        panes: None,
                    });
                }
                (Some("S"), Some(tree)) => {
                    let focus = fields.next().and_then(|f| f.parse().ok());
                    let sessions: Vec<String> = fields
                        .next()
                        .map(|list| list.split(',').map(str::to_owned).collect())
                        .unwrap_or_default();
                    if let (Some(tab), Some(focus)) = (layout.tabs.last_mut(), focus) {
                        if focus < sessions.len() {
                            tab.panes = Some(SavedPanes {
                                tree: tree.to_owned(),
                                sessions,
                                focus,
                            });
                        }
                    }
                }
                _ => {}
            }
        }
        layout
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_round_trips_awkward_names() {
        let layout = Layout {
            tabs: vec![
                SavedTab {
                    session: "a".into(),
                    pinned: true,
                    name: Some("tab\tone\\ \nnext".into()),
                    panes: Some(SavedPanes {
                        tree: "h0.500(0,1)".into(),
                        sessions: vec!["a".into(), "c".into()],
                        focus: 1,
                    }),
                },
                SavedTab {
                    session: "b".into(),
                    pinned: false,
                    name: None,
                    panes: None,
                },
            ],
            active: Some("b".into()),
            chrome: Some(SavedChrome {
                dock_open: true,
                dock_height: 240,
                dock_tab: "changes".into(),
                panel_open: false,
                panel_width: 360,
                docked_session: Some("a".into()),
            }),
            workbench: Some(SavedWorkbench {
                environment: "files".into(),
                file_mode: "tree".into(),
                selected_file: Some("src/main.rs".into()),
                git_diff_view: "split".into(),
                task_filter: "blocked".into(),
                task_sort: "title".into(),
                dock_tool: "problems".into(),
                dock_collapsed: true,
                terminal_maximized: false,
                sidebar_width: 280,
            }),
            areas: vec![("work".into(), "h0.680(1:k,2:i)".into())],
            canvas: Some(SavedCanvas {
                lens: "flow".into(),
                zoom_milli: 1250,
                pan_x: -40,
                pan_y: 12,
            }),
        };
        assert_eq!(Layout::decode(&layout.encode()), layout);
    }

    #[test]
    fn chrome_line_is_optional_and_validated() {
        let header = "termforge-layout 1\n";
        assert!(Layout::decode(&format!("{header}T\ta\t0\t\n"))
            .chrome
            .is_none());
        assert!(
            Layout::decode(&format!("{header}C\t1\tx\tchanges\t0\t300\n"))
                .chrome
                .is_none()
        );
        assert!(Layout::decode(&format!("{header}C\t1\t200\n"))
            .chrome
            .is_none());
    }

    #[test]
    fn pane_lines_are_validated_and_optional() {
        let header = "termforge-layout 1\n";
        // A version-1 file (no S lines) still loads, as single-pane tabs.
        let v1 = Layout::decode(&format!("{header}T\ta\t0\t\n"));
        assert_eq!(v1.tabs.len(), 1);
        assert!(v1.tabs[0].panes.is_none());
        // S before any T, a focus past the end, and a missing focus are ignored.
        for bad in [
            "S\th0.500(0,1)\t0\ta,b\n",
            "T\ta\t0\t\nS\th0.500(0,1)\t5\ta,b\n",
            "T\ta\t0\t\nS\th0.500(0,1)\tx\ta,b\n",
        ] {
            let layout = Layout::decode(&format!("{header}{bad}"));
            assert!(layout.tabs.iter().all(|t| t.panes.is_none()), "{bad:?}");
        }
    }

    #[test]
    fn garbage_layout_is_empty() {
        assert_eq!(Layout::decode("nonsense"), Layout::default());
        assert_eq!(Layout::decode(""), Layout::default());
    }

    #[test]
    fn local_path_strips_host_but_keeps_drive() {
        assert_eq!(local_path("box:/home/me"), PathBuf::from("/home/me"));
        assert_eq!(local_path("C:\\Users\\me"), PathBuf::from("C:\\Users\\me"));
        assert_eq!(local_path("/plain"), PathBuf::from("/plain"));
    }

    #[test]
    fn branch_from_head_file() {
        let dir = tempfile_dir();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join(".git/HEAD"), "ref: refs/heads/feature/x\n").unwrap();
        assert_eq!(git_branch(&dir.join("src")).as_deref(), Some("feature/x"));
        std::fs::write(dir.join(".git/HEAD"), "0123456789abcdef\n").unwrap();
        assert_eq!(git_branch(&dir).as_deref(), Some("0123456"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn tempfile_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tf-ws-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn workspaces_have_session_free_default_layouts() {
        let mut next = 10;
        let mut ids = || {
            next += 1;
            next
        };
        for ws in Workspace::ALL {
            assert_eq!(Workspace::from_id(ws.id()), Some(ws));
            let tree = ws.default_layout(&mut ids);
            assert_eq!(tree.is_some(), !ws.hosts_sessions(), "{ws:?}");
            if let Some(tree) = tree {
                assert!(
                    tree.areas()
                        .iter()
                        .all(|a| !matches!(a.editor, EditorKind::Terminal(_))),
                    "{ws:?} must not own sessions"
                );
            }
        }
    }

    #[test]
    fn legacy_workspace_ids_still_load() {
        assert_eq!(Workspace::from_id("tasks"), Some(Workspace::Work));
        assert_eq!(Workspace::from_id("files"), Some(Workspace::Explore));
        assert_eq!(Workspace::from_id("git"), Some(Workspace::Review));
        assert_eq!(Workspace::from_id("terminal"), Some(Workspace::Build));
        assert_eq!(Workspace::from_id("project"), Some(Workspace::Explore));
        assert_eq!(Workspace::from_id("nonsense"), None);
    }

    #[test]
    fn area_layouts_round_trip_and_are_optional() {
        let layout = Layout {
            areas: vec![
                ("work".into(), "h0.680(1:k,2:i)".into()),
                ("review".into(), "3:g".into()),
            ],
            ..Layout::default()
        };
        let decoded = Layout::decode(&layout.encode());
        assert_eq!(decoded.areas, layout.areas);
        assert!(Layout::decode("termforge-layout 1\n").areas.is_empty());
    }

    #[test]
    fn operate_defaults_to_environment_services_and_activity() {
        let mut next = 0;
        let tree = Workspace::Operate
            .default_layout(|| {
                next += 1;
                next
            })
            .unwrap();
        let mut tags: Vec<_> = tree
            .areas()
            .iter()
            .map(|a| a.editor.tag().label())
            .collect();
        tags.sort_unstable();
        assert_eq!(tags, ["Environment", "Services", "Timeline"]);
        assert_eq!(
            tree.editor(tree.focused()).unwrap().tag().label(),
            "Services"
        );
    }

    #[test]
    fn canvas_line_is_optional_and_validated() {
        let header = "termforge-layout 1\n";
        assert!(Layout::decode(header).canvas.is_none());
        assert!(Layout::decode(&format!("{header}V\tflow\t1000\n"))
            .canvas
            .is_none());
        assert!(Layout::decode(&format!("{header}V\tflow\tx\t0\t0\n"))
            .canvas
            .is_none());
        let ok = Layout::decode(&format!("{header}V\tflow\t1500\t-3\t9\n"));
        assert_eq!(ok.canvas.unwrap().pan_x, -3);
    }
}
