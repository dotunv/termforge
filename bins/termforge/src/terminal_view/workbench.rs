//! Project-first workbench surfaces around the terminal dock.

use super::*;

const MAX_PROJECT_FILES: usize = 360;
const MAX_PROJECT_DEPTH: usize = 5;

#[derive(Debug, Clone)]
struct ProjectFileEntry {
    path: PathBuf,
    depth: usize,
    is_dir: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ProjectFileGroup {
    Source,
    Libraries,
    Packages,
    Config,
    Ai,
    Docs,
    Other,
}

impl ProjectFileGroup {
    const ALL: [Self; 7] = [
        Self::Source,
        Self::Libraries,
        Self::Packages,
        Self::Config,
        Self::Ai,
        Self::Docs,
        Self::Other,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Source => "Source code",
            Self::Libraries => "Libraries",
            Self::Packages => "Packages",
            Self::Config => "Config & tooling",
            Self::Ai => "AI skills & instructions",
            Self::Docs => "Documentation",
            Self::Other => "Other files",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Source => "⌘",
            Self::Libraries => "◫",
            Self::Packages => "□",
            Self::Config => "⚙",
            Self::Ai => "✦",
            Self::Docs => "▤",
            Self::Other => "·",
        }
    }
}

impl TerminalView {
    pub(super) fn toggle_terminal_maximized(&mut self, cx: &mut Context<'_, Self>) {
        self.terminal_maximized = !self.terminal_maximized;
        self.dock_collapsed = false;
        self.dock_tool = DockTool::Terminal;
        cx.notify();
    }

    fn select_workspace_artifact(
        &mut self,
        artifact: WorkspaceArtifact,
        cx: &mut Context<'_, Self>,
    ) {
        self.workspace_artifact = artifact;
        cx.notify();
    }

    fn select_file_explorer_mode(&mut self, mode: FileExplorerMode, cx: &mut Context<'_, Self>) {
        self.file_explorer_mode = mode;
        cx.notify();
    }

    fn select_dock_tool(&mut self, tool: DockTool, cx: &mut Context<'_, Self>) {
        self.dock_tool = tool;
        self.dock_collapsed = false;
        cx.notify();
    }

    pub(super) fn toggle_dock_collapsed(&mut self, cx: &mut Context<'_, Self>) {
        self.dock_collapsed = !self.dock_collapsed;
        self.terminal_maximized = false;
        cx.notify();
    }

    pub(super) fn workspace_tab_strip(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let t = &self.theme;
        let tabs = [
            (WorkspaceArtifact::Overview, "Overview"),
            (WorkspaceArtifact::Files, "Files"),
            (WorkspaceArtifact::Tasks, "Tasks"),
            (WorkspaceArtifact::Sessions, "Sessions"),
        ];
        div()
            .h(px(40.0))
            .flex_none()
            .px(px(tokens::space::SM))
            .flex()
            .items_end()
            .gap(px(2.0))
            .border_b_1()
            .border_color(hsla(t.border))
            .children(tabs.into_iter().map(|(artifact, label)| {
                let active = artifact == self.workspace_artifact;
                div()
                    .id(SharedString::from(format!("artifact-{label}")))
                    .h(px(34.0))
                    .px(px(tokens::space::MD))
                    .flex()
                    .items_center()
                    .rounded_t(px(tokens::radius::MD))
                    .text_size(px(tokens::text::SM))
                    .when(active, |d| {
                        d.bg(hsla(t.bg_panel))
                            .text_color(hsla(t.text))
                            .border_b_1()
                            .border_color(hsla(t.accent))
                    })
                    .when(!active, |d| {
                        d.text_color(hsla(t.text_muted))
                            .hover(|style| style.bg(hsla(t.bg_hover)).text_color(hsla(t.text)))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_workspace_artifact(artifact, cx);
                    }))
                    .child(label)
            }))
    }

    pub(super) fn workspace_artifact_content(
        &self,
        cx: &mut Context<'_, Self>,
    ) -> gpui::AnyElement {
        match self.workspace_artifact {
            WorkspaceArtifact::Overview => self.workbench_overview(cx).into_any_element(),
            WorkspaceArtifact::Files => self.workbench_files(cx).into_any_element(),
            WorkspaceArtifact::Tasks => self.workbench_tasks(cx).into_any_element(),
            WorkspaceArtifact::Sessions => self.workbench_sessions(cx).into_any_element(),
        }
    }

    pub(super) fn workbench_files(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let t = &self.theme;
        let Some(root) = self.local_project_root.as_deref() else {
            return div()
                .id("workbench-files")
                .flex_1()
                .min_h(px(0.0))
                .p(px(tokens::space::LG))
                .bg(hsla(t.bg_app))
                .child(self.artifact_heading(
                    "Project files",
                    "Open a local project to organize its files.",
                    cx,
                ));
        };
        let entries = Self::project_files(root);
        let truncated = entries.len() == MAX_PROJECT_FILES;
        let root_label = root.to_string_lossy().into_owned();
        let content = match self.file_explorer_mode {
            FileExplorerMode::Smart => self.smart_file_groups(&entries).into_any_element(),
            FileExplorerMode::Tree => self.file_tree(&entries).into_any_element(),
        };
        div()
            .id("workbench-files")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .p(px(tokens::space::LG))
            .bg(hsla(t.bg_app))
            .child(
                div()
                    .mb(px(tokens::space::LG))
                    .flex()
                    .items_start()
                    .justify_between()
                    .child(
                        div()
                            .child(
                                div()
                                    .text_size(px(tokens::text::XL))
                                    .text_color(hsla(t.text))
                                    .child("Project files"),
                            )
                            .child(
                                div()
                                    .mt(px(2.0))
                                    .max_w(px(460.0))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(px(tokens::text::SM))
                                    .text_color(hsla(t.text_faint))
                                    .child(SharedString::from(root_label)),
                            ),
                    )
                    .child(self.file_mode_switch(cx)),
            )
            .when(truncated, |d| {
                d.child(
                    div()
                        .mb(px(tokens::space::MD))
                        .px(px(tokens::space::MD))
                        .py(px(tokens::space::SM))
                        .rounded(px(tokens::radius::MD))
                        .bg(with_alpha(hsla(t.warning), 0.10))
                        .text_size(px(tokens::text::SM))
                        .text_color(hsla(t.text_muted))
                        .child("Showing the first 360 project entries."),
                )
            })
            .child(content)
    }

    fn file_mode_switch(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let t = &self.theme;
        let modes = [
            (FileExplorerMode::Smart, "Smart groups"),
            (FileExplorerMode::Tree, "Folders & files"),
        ];
        div()
            .p(px(2.0))
            .flex()
            .items_center()
            .rounded(px(tokens::radius::MD))
            .bg(with_alpha(hsla(t.text), 0.05))
            .children(modes.into_iter().map(|(mode, label)| {
                let active = mode == self.file_explorer_mode;
                div()
                    .id(SharedString::from(format!("file-mode-{label}")))
                    .px(px(tokens::space::SM))
                    .py(px(4.0))
                    .rounded(px(tokens::radius::SM))
                    .text_size(px(tokens::text::XS))
                    .when(active, |d| d.bg(hsla(t.bg_panel)).text_color(hsla(t.text)))
                    .when(!active, |d| {
                        d.text_color(hsla(t.text_faint))
                            .hover(|style| style.text_color(hsla(t.text)))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_file_explorer_mode(mode, cx);
                    }))
                    .child(label)
            }))
    }

    fn smart_file_groups(&self, entries: &[ProjectFileEntry]) -> impl IntoElement {
        let t = &self.theme;
        let mut groups: std::collections::BTreeMap<ProjectFileGroup, Vec<&ProjectFileEntry>> =
            std::collections::BTreeMap::new();
        for entry in entries {
            groups
                .entry(Self::project_file_group(&entry.path))
                .or_default()
                .push(entry);
        }
        div().flex().flex_col().gap(px(tokens::space::MD)).children(
            ProjectFileGroup::ALL.into_iter().filter_map(|group| {
                let entries = groups.get(&group)?;
                Some(
                    div()
                        .rounded(px(tokens::radius::LG))
                        .border_1()
                        .border_color(hsla(t.border))
                        .bg(hsla(t.bg_panel))
                        .overflow_hidden()
                        .child(
                            self.workbench_card_header(group.label(), &entries.len().to_string()),
                        )
                        .child(div().p(px(tokens::space::XS)).children(entries.iter().map(
                            |entry| self.file_row(entry, group.icon(), false).into_any_element(),
                        ))),
                )
            }),
        )
    }

    fn file_tree(&self, entries: &[ProjectFileEntry]) -> impl IntoElement {
        let t = &self.theme;
        div()
            .rounded(px(tokens::radius::LG))
            .border_1()
            .border_color(hsla(t.border))
            .bg(hsla(t.bg_panel))
            .overflow_hidden()
            .child(self.workbench_card_header("FOLDERS & FILES", &entries.len().to_string()))
            .child(
                div().p(px(tokens::space::XS)).children(
                    entries.iter().map(|entry| {
                        self.file_row(entry, if entry.is_dir { "▸" } else { "·" }, true)
                    }),
                ),
            )
    }

    fn file_row(
        &self,
        entry: &ProjectFileEntry,
        icon: &'static str,
        show_depth: bool,
    ) -> impl IntoElement {
        let t = &self.theme;
        let label = entry.path.to_string_lossy().into_owned();
        let indent = if show_depth {
            entry.depth as f32 * 14.0
        } else {
            0.0
        };
        div()
            .h(px(26.0))
            .pl(px(tokens::space::SM + indent))
            .pr(px(tokens::space::SM))
            .flex()
            .items_center()
            .gap(px(tokens::space::SM))
            .rounded(px(tokens::radius::SM))
            .text_size(px(tokens::text::SM))
            .text_color(hsla(if entry.is_dir {
                t.text_muted
            } else {
                t.text_faint
            }))
            .hover(|style| {
                style
                    .bg(with_alpha(hsla(t.text), 0.05))
                    .text_color(hsla(t.text))
            })
            .child(div().w(px(12.0)).text_color(hsla(t.text_faint)).child(icon))
            .child(
                div()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(SharedString::from(label)),
            )
    }

    fn project_files(root: &Path) -> Vec<ProjectFileEntry> {
        fn visit(root: &Path, dir: &Path, depth: usize, output: &mut Vec<ProjectFileEntry>) {
            if depth > MAX_PROJECT_DEPTH || output.len() >= MAX_PROJECT_FILES {
                return;
            }
            let Ok(read_dir) = std::fs::read_dir(dir) else {
                return;
            };
            let mut entries: Vec<_> = read_dir.flatten().collect();
            entries.sort_by_key(|entry| {
                let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
                (!is_dir, entry.file_name())
            });
            for entry in entries {
                if output.len() >= MAX_PROJECT_FILES {
                    return;
                }
                let path = entry.path();
                let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
                let name = entry.file_name().to_string_lossy().into_owned();
                if is_dir && matches!(name.as_str(), ".git" | "target" | "node_modules" | ".next") {
                    continue;
                }
                if is_dir
                    && name.starts_with('.')
                    && !matches!(
                        name.as_str(),
                        ".claude" | ".cursor" | ".agents" | ".codex" | ".github"
                    )
                {
                    continue;
                }
                let relative = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
                output.push(ProjectFileEntry {
                    path: relative,
                    depth,
                    is_dir,
                });
                if is_dir {
                    visit(root, &path, depth + 1, output);
                }
            }
        }

        let mut output = Vec::new();
        visit(root, root, 0, &mut output);
        output
    }

    fn project_file_group(path: &Path) -> ProjectFileGroup {
        let first = path
            .components()
            .next()
            .map(|component| component.as_os_str().to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if matches!(first.as_str(), ".claude" | ".cursor" | ".agents" | ".codex")
            || matches!(name.as_str(), "agents.md" | "claude.md" | "skill.md")
        {
            ProjectFileGroup::Ai
        } else if matches!(first.as_str(), "docs" | "doc")
            || matches!(
                name.as_str(),
                "readme.md" | "contributing.md" | "changelog.md"
            )
        {
            ProjectFileGroup::Docs
        } else if matches!(first.as_str(), "crates" | "libs" | "lib") {
            ProjectFileGroup::Libraries
        } else if matches!(first.as_str(), "packages" | "vendor")
            || matches!(
                name.as_str(),
                "cargo.toml" | "package.json" | "pnpm-lock.yaml" | "yarn.lock"
            )
        {
            ProjectFileGroup::Packages
        } else if matches!(first.as_str(), "src" | "app" | "apps" | "bin" | "bins")
            || matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some(
                    "rs" | "ts"
                        | "tsx"
                        | "js"
                        | "jsx"
                        | "py"
                        | "go"
                        | "java"
                        | "swift"
                        | "c"
                        | "h"
                        | "cpp"
                )
            )
        {
            ProjectFileGroup::Source
        } else if first.starts_with('.')
            || matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("toml" | "yaml" | "yml" | "json" | "jsonc" | "ini" | "env")
            )
            || matches!(name.as_str(), "makefile" | "dockerfile" | "justfile")
        {
            ProjectFileGroup::Config
        } else {
            ProjectFileGroup::Other
        }
    }

    pub(super) fn workbench_overview(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let t = &self.theme;
        let (open_tasks, active_tasks, blocked_tasks) = self.task_summary();
        let branch = self.discover_branch().unwrap_or_else(|| "No branch".into());
        let context = self
            .active_context()
            .map_or("No context", |context| context.name.as_str())
            .to_owned();
        let attention = self
            .program_status_summary()
            .and_then(|(_, detail)| detail)
            .unwrap_or_else(|| "All sessions are quiet".into());
        div()
            .id("workbench-overview")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .p(px(tokens::space::LG))
            .bg(hsla(t.bg_app))
            .child(self.artifact_heading(
                "Project overview",
                &format!("{} · {branch} · {context}", self.project_name()),
                cx,
            ))
            .child(
                div()
                    .flex()
                    .gap(px(tokens::space::MD))
                    .child(self.overview_metric(
                        "TASKS",
                        &open_tasks.to_string(),
                        &format!("{active_tasks} active · {blocked_tasks} blocked"),
                        t.accent,
                    ))
                    .child(self.overview_metric(
                        "SESSIONS",
                        &self.tabs.len().to_string(),
                        &attention,
                        t.success,
                    ))
                    .child(self.overview_metric(
                        "REPOSITORY",
                        &branch,
                        "Working tree context",
                        t.text_faint,
                    )),
            )
            .child(
                div()
                    .mt(px(tokens::space::MD))
                    .p(px(tokens::space::LG))
                    .rounded(px(tokens::radius::LG))
                    .border_1()
                    .border_color(hsla(t.border))
                    .bg(hsla(t.bg_panel))
                    .child(
                        div()
                            .text_color(hsla(t.text))
                            .child("Continue working"),
                    )
                    .child(
                        div()
                            .mt(px(tokens::space::XS))
                            .text_size(px(tokens::text::SM))
                            .text_color(hsla(t.text_faint))
                            .child("Choose Tasks or Sessions above. The terminal remains available in the dock without becoming the workspace."),
                    ),
            )
    }

    pub(super) fn workbench_sessions(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let t = &self.theme;

        let session_rows: Vec<gpui::AnyElement> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let active = index == self.active_tab;
                let label = tab
                    .name
                    .clone()
                    .or_else(|| tab.title.clone())
                    .unwrap_or_else(|| Self::tab_label(tab));
                let detail = if let Some(host) = tab.spawn.ssh_host.as_deref() {
                    format!("SSH · {host}")
                } else {
                    tab.cwd
                        .clone()
                        .unwrap_or_else(|| tab.spawn.profile.name.clone())
                };
                let status = self.tab_status_color(tab, active, t);
                div()
                    .id(("workbench-session", index))
                    .h(px(42.0))
                    .px(px(tokens::space::SM))
                    .flex()
                    .items_center()
                    .gap(px(tokens::space::SM))
                    .rounded(px(tokens::radius::SM))
                    .hover(|style| style.bg(hsla(t.bg_hover)))
                    .on_click(cx.listener(move |this, _, _, cx| this.select_tab(index, cx)))
                    .child(div().size(px(7.0)).rounded_full().bg(hsla(status)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_color(hsla(t.text))
                                    .child(SharedString::from(label)),
                            )
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(px(tokens::text::XS))
                                    .text_color(hsla(t.text_faint))
                                    .child(SharedString::from(detail)),
                            ),
                    )
                    .child(
                        div()
                            .px(px(6.0))
                            .py(px(2.0))
                            .rounded(px(tokens::radius::SM))
                            .border_1()
                            .border_color(hsla(t.border))
                            .text_size(px(tokens::text::XS))
                            .text_color(hsla(if active { t.accent } else { t.text_faint }))
                            .child(if active { "Focused" } else { "Open" }),
                    )
                    .into_any_element()
            })
            .collect();

        div()
            .id("workbench-sessions")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .p(px(tokens::space::LG))
            .bg(hsla(t.bg_app))
            .child(self.artifact_heading(
                "Sessions",
                "Local and SSH shells associated with this project",
                cx,
            ))
            .child(
                div()
                    .rounded(px(tokens::radius::LG))
                    .border_1()
                    .border_color(hsla(t.border))
                    .bg(hsla(t.bg_panel))
                    .overflow_hidden()
                    .child(
                        self.workbench_card_header("OPEN SESSIONS", &self.tabs.len().to_string()),
                    )
                    .child(div().p(px(tokens::space::XS)).children(session_rows)),
            )
    }

    pub(super) fn workbench_tasks(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let t = &self.theme;
        let rows: Vec<_> = self
            .tasks
            .items
            .iter()
            .enumerate()
            .map(|(index, task)| self.task_row(index, task, cx).into_any_element())
            .collect();
        div()
            .id("workbench-tasks")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .p(px(tokens::space::LG))
            .bg(hsla(t.bg_app))
            .child(self.artifact_heading(
                "Project tasks",
                "Run, advance, and track commands without turning them into terminal tabs",
                cx,
            ))
            .child(
                div()
                    .rounded(px(tokens::radius::LG))
                    .border_1()
                    .border_color(hsla(t.border))
                    .bg(hsla(t.bg_panel))
                    .overflow_hidden()
                    .child(self.workbench_card_header(
                        "TASKS",
                        &self.tasks.items.len().to_string(),
                    ))
                    .when(rows.is_empty(), |d| {
                        d.child(
                            div()
                                .p(px(tokens::space::LG))
                                .text_size(px(tokens::text::SM))
                                .text_color(hsla(t.text_faint))
                                .child("No tasks yet. Create one to keep project intent next to its command and output."),
                        )
                    })
                    .child(div().p(px(tokens::space::XS)).children(rows)),
            )
    }

    fn artifact_heading(
        &self,
        title: &str,
        subtitle: &str,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        let tasks = self.workspace_artifact == WorkspaceArtifact::Tasks;
        div()
            .mb(px(tokens::space::LG))
            .flex()
            .items_start()
            .justify_between()
            .child(
                div()
                    .child(
                        div()
                            .text_size(px(tokens::text::XL))
                            .text_color(hsla(t.text))
                            .child(SharedString::from(title.to_owned())),
                    )
                    .child(
                        div()
                            .mt(px(2.0))
                            .text_size(px(tokens::text::SM))
                            .text_color(hsla(t.text_faint))
                            .child(SharedString::from(subtitle.to_owned())),
                    ),
            )
            .child(
                div()
                    .id("artifact-primary-action")
                    .px(px(tokens::space::MD))
                    .py(px(6.0))
                    .rounded(px(tokens::radius::MD))
                    .border_1()
                    .border_color(hsla(t.border))
                    .text_size(px(tokens::text::SM))
                    .text_color(hsla(t.text_muted))
                    .hover(|style| style.bg(hsla(t.bg_hover)).text_color(hsla(t.text)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if tasks {
                            this.open_task_form("", "", cx);
                        } else {
                            this.new_local_tab(cx);
                        }
                    }))
                    .child(if tasks {
                        "+ New task"
                    } else {
                        "+ New terminal"
                    }),
            )
    }

    fn overview_metric(
        &self,
        label: &'static str,
        value: &str,
        detail: &str,
        color: Rgb,
    ) -> impl IntoElement {
        let t = &self.theme;
        div()
            .flex_1()
            .min_w(px(150.0))
            .p(px(tokens::space::MD))
            .rounded(px(tokens::radius::LG))
            .border_1()
            .border_color(hsla(t.border))
            .bg(hsla(t.bg_panel))
            .child(
                div()
                    .text_size(px(tokens::text::XS))
                    .text_color(hsla(t.text_faint))
                    .child(label),
            )
            .child(
                div()
                    .mt(px(tokens::space::SM))
                    .flex()
                    .items_center()
                    .gap(px(tokens::space::SM))
                    .child(div().size(px(7.0)).rounded_full().bg(hsla(color)))
                    .child(
                        div()
                            .text_size(px(tokens::text::XL))
                            .text_color(hsla(t.text))
                            .child(SharedString::from(value.to_owned())),
                    ),
            )
            .child(
                div()
                    .mt(px(2.0))
                    .text_size(px(tokens::text::XS))
                    .text_color(hsla(t.text_faint))
                    .child(SharedString::from(detail.to_owned())),
            )
    }

    fn workbench_card_header(&self, title: &'static str, trailing: &str) -> impl IntoElement {
        let t = &self.theme;
        div()
            .h(px(34.0))
            .px(px(tokens::space::MD))
            .flex()
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(hsla(t.border))
            .text_size(px(tokens::text::XS))
            .text_color(hsla(t.text_faint))
            .child(title)
            .child(SharedString::from(trailing.to_owned()))
    }

    pub(super) fn terminal_panel_header(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let t = &self.theme;
        let tools = [
            (DockTool::Terminal, "Terminal"),
            (DockTool::Problems, "Problems"),
            (DockTool::Output, "Output"),
        ];
        div()
            .id("terminal-panel-header")
            .h(px(32.0))
            .flex_none()
            .px(px(tokens::space::SM))
            .flex()
            .items_center()
            .border_t_1()
            .border_b_1()
            .border_color(hsla(t.border))
            .bg(hsla(t.bg_app))
            .children(tools.into_iter().map(|(tool, label)| {
                let active = self.dock_tool == tool;
                div()
                    .id(SharedString::from(format!("dock-{label}")))
                    .h_full()
                    .px(px(tokens::space::SM))
                    .flex()
                    .items_center()
                    .text_size(px(tokens::text::SM))
                    .when(active, |d| {
                        d.border_b_1()
                            .border_color(hsla(t.accent))
                            .text_color(hsla(t.text))
                    })
                    .when(!active, |d| {
                        d.text_color(hsla(t.text_faint))
                            .hover(|style| style.text_color(hsla(t.text)))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_dock_tool(tool, cx);
                    }))
                    .child(label)
            }))
            .child(
                div()
                    .ml(px(tokens::space::SM))
                    .text_size(px(tokens::text::XS))
                    .text_color(hsla(t.text_faint))
                    .child(SharedString::from(
                        self.spawn
                            .ssh_host
                            .as_ref()
                            .map(|host| format!("SSH · {host}"))
                            .unwrap_or_else(|| self.spawn.profile.name.clone()),
                    )),
            )
            .child(div().flex_1())
            .child(
                div()
                    .id("dock-collapse")
                    .size(px(24.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(tokens::radius::SM))
                    .text_color(hsla(t.text_faint))
                    .hover(|style| style.bg(hsla(t.bg_hover)).text_color(hsla(t.text)))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_dock_collapsed(cx)))
                    .child(if self.dock_collapsed { "⌃" } else { "⌄" }),
            )
            .child(
                div()
                    .id("terminal-panel-maximize")
                    .size(px(24.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(tokens::radius::SM))
                    .text_color(hsla(t.text_faint))
                    .hover(|style| style.bg(hsla(t.bg_hover)).text_color(hsla(t.text)))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_terminal_maximized(cx)))
                    .child(if self.terminal_maximized {
                        "↙"
                    } else {
                        "↗"
                    }),
            )
    }

    pub(super) fn dock_placeholder(&self) -> impl IntoElement {
        let t = &self.theme;
        let (title, detail) = match self.dock_tool {
            DockTool::Terminal => ("Terminal", "Interactive project shells"),
            DockTool::Problems => (
                "No problems detected",
                "Process failures and project diagnostics will appear here.",
            ),
            DockTool::Output => (
                "No captured output",
                "Task and background-process output will appear here.",
            ),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .text_color(hsla(t.text_muted))
            .child(title)
            .child(
                div()
                    .mt(px(tokens::space::XS))
                    .text_size(px(tokens::text::XS))
                    .text_color(hsla(t.text_faint))
                    .child(detail),
            )
    }

    pub(super) fn workbench_inspector(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let t = &self.theme;
        let branch = self.discover_branch().unwrap_or_else(|| "No branch".into());
        let context = self
            .active_context()
            .map_or("No context", |context| context.name.as_str())
            .to_owned();
        let attention = self
            .program_status_summary()
            .and_then(|(_, detail)| detail)
            .unwrap_or_else(|| "No session needs attention".into());
        let (heading, facts): (&str, Vec<(&str, String)>) = match self.workspace_artifact {
            WorkspaceArtifact::Overview => (
                "PROJECT INSPECTOR",
                vec![
                    ("PROJECT", self.project_name()),
                    ("BRANCH", branch),
                    ("ACCOUNT", context),
                    ("ATTENTION", attention),
                ],
            ),
            WorkspaceArtifact::Files => {
                let entries = self
                    .local_project_root
                    .as_deref()
                    .map(Self::project_files)
                    .unwrap_or_default();
                (
                    "FILE EXPLORER",
                    vec![
                        (
                            "VIEW",
                            match self.file_explorer_mode {
                                FileExplorerMode::Smart => "Smart groups",
                                FileExplorerMode::Tree => "Folders & files",
                            }
                            .into(),
                        ),
                        ("ITEMS", entries.len().to_string()),
                        ("PROJECT", self.project_name()),
                    ],
                )
            }
            WorkspaceArtifact::Tasks => {
                let (open, active, blocked) = self.task_summary();
                (
                    "TASK INSPECTOR",
                    vec![
                        ("OPEN", open.to_string()),
                        ("ACTIVE", active.to_string()),
                        ("BLOCKED", blocked.to_string()),
                        ("PROJECT", self.project_name()),
                    ],
                )
            }
            WorkspaceArtifact::Sessions => {
                let tab = self.tabs.get(self.active_tab);
                let name = tab
                    .and_then(|tab| tab.name.clone().or_else(|| tab.title.clone()))
                    .unwrap_or_else(|| "Current session".into());
                let location = tab
                    .and_then(|tab| tab.cwd.clone())
                    .unwrap_or_else(|| "Unknown directory".into());
                let connection = tab
                    .and_then(|tab| tab.spawn.ssh_host.clone())
                    .map(|host| format!("SSH · {host}"))
                    .unwrap_or_else(|| "Local".into());
                (
                    "SESSION INSPECTOR",
                    vec![
                        ("SESSION", name),
                        ("LOCATION", location),
                        ("CONNECTION", connection),
                        ("STATUS", attention),
                    ],
                )
            }
        };
        div()
            .id("workbench-inspector")
            .w(px(240.0))
            .min_w(px(220.0))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(hsla(t.border))
            .bg(hsla(t.bg_panel))
            .child(
                div()
                    .h(px(38.0))
                    .px(px(tokens::space::MD))
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(hsla(t.border))
                    .text_size(px(tokens::text::XS))
                    .text_color(hsla(t.text_faint))
                    .child(SharedString::from(heading.to_owned())),
            )
            .child(
                div()
                    .p(px(tokens::space::LG))
                    .flex()
                    .flex_col()
                    .gap(px(tokens::space::LG))
                    .children(
                        facts
                            .into_iter()
                            .map(|(label, value)| self.inspector_fact(label, &value)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(tokens::space::XS))
                            .child(self.inspector_action(
                                "inspector-new-terminal",
                                "+  New terminal",
                                cx.listener(|this, _, _, cx| this.new_local_tab(cx)),
                            ))
                            .child(self.inspector_action(
                                "inspector-new-task",
                                "○  New task",
                                cx.listener(|this, _, _, cx| this.open_task_form("", "", cx)),
                            ))
                            .child(self.inspector_action(
                                "inspector-command-palette",
                                "⌘  Command palette",
                                cx.listener(|this, _, _, cx| this.open_palette(cx)),
                            )),
                    ),
            )
    }

    fn inspector_fact(&self, label: &'static str, value: &str) -> impl IntoElement {
        let t = &self.theme;
        div()
            .pb(px(tokens::space::MD))
            .border_b_1()
            .border_color(hsla(t.border))
            .child(
                div()
                    .mb(px(tokens::space::XS))
                    .text_size(px(tokens::text::XS))
                    .text_color(hsla(t.text_faint))
                    .child(label),
            )
            .child(
                div()
                    .text_size(px(tokens::text::SM))
                    .text_color(hsla(t.text_muted))
                    .child(SharedString::from(value.to_owned())),
            )
    }

    fn inspector_action(
        &self,
        id: &'static str,
        label: &'static str,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let t = &self.theme;
        div()
            .id(id)
            .h(px(32.0))
            .px(px(tokens::space::SM))
            .flex()
            .items_center()
            .rounded(px(tokens::radius::MD))
            .border_1()
            .border_color(hsla(t.border))
            .text_size(px(tokens::text::SM))
            .text_color(hsla(t.text_muted))
            .hover(|style| style.bg(hsla(t.bg_hover)).text_color(hsla(t.text)))
            .on_click(on_click)
            .child(label)
    }
}

#[cfg(test)]
mod tests {
    use super::{ProjectFileGroup, TerminalView};
    use std::path::Path;

    #[test]
    fn smart_file_groups_prioritize_ai_docs_and_project_structure() {
        assert_eq!(
            TerminalView::project_file_group(Path::new(".claude/skills/release/SKILL.md")),
            ProjectFileGroup::Ai
        );
        assert_eq!(
            TerminalView::project_file_group(Path::new("docs/adr/0001.md")),
            ProjectFileGroup::Docs
        );
        assert_eq!(
            TerminalView::project_file_group(Path::new("crates/tf-ui/src/lib.rs")),
            ProjectFileGroup::Libraries
        );
        assert_eq!(
            TerminalView::project_file_group(Path::new("Cargo.toml")),
            ProjectFileGroup::Packages
        );
        assert_eq!(
            TerminalView::project_file_group(Path::new("bins/termforge/src/main.rs")),
            ProjectFileGroup::Source
        );
    }
}
