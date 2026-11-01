//! Project-first workbench surfaces around the terminal dock.

use super::*;

const MAX_PROJECT_DEPTH: usize = 5;

/// Generated and dependency directories never make a useful default project
/// view. Users can still inspect them in their filesystem, without allowing
/// one large build tree to hide the actual project.
const DEFAULT_IGNORED_DIRECTORIES: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".next",
    ".nuxt",
    ".output",
    ".parcel-cache",
    ".pytest_cache",
    ".svelte-kit",
    ".terraform",
    ".turbo",
    ".venv",
    ".vite",
    "__pycache__",
    "build",
    "coverage",
    "dist",
    "node_modules",
    "out",
    "target",
    "tmp",
    "venv",
];

const VISIBLE_DOT_DIRECTORIES: &[&str] = &[
    ".agents",
    ".changeset",
    ".claude",
    ".codex",
    ".cursor",
    ".devcontainer",
    ".github",
    ".husky",
    ".vscode",
];

#[derive(Debug, Clone)]
pub(super) struct ProjectFileEntry {
    pub(super) path: PathBuf,
    depth: usize,
    is_dir: bool,
}

type EditorLine = Vec<(String, Option<tf_editor::HighlightKind>, bool)>;

fn highlighted_editor_lines(
    text: &str,
    spans: &[tf_editor::HighlightSpan],
    selection: std::ops::Range<usize>,
) -> Vec<EditorLine> {
    fn push_segment(
        segments: &mut EditorLine,
        text: &str,
        start: usize,
        end: usize,
        kind: Option<tf_editor::HighlightKind>,
        selection: &std::ops::Range<usize>,
    ) {
        let selected_start = selection.start.clamp(start, end);
        let selected_end = selection.end.clamp(start, end);
        if start < selected_start {
            segments.push((text[start..selected_start].to_owned(), kind, false));
        }
        if selected_start < selected_end {
            segments.push((text[selected_start..selected_end].to_owned(), kind, true));
        }
        if selected_end < end {
            segments.push((text[selected_end..end].to_owned(), kind, false));
        }
    }
    let mut line_start = 0;
    text.split('\n')
        .map(|line| {
            let line_end = line_start + line.len();
            let mut position = line_start;
            let mut segments = Vec::new();
            for span in spans
                .iter()
                .filter(|span| span.range.start < line_end && span.range.end > line_start)
            {
                let start = span.range.start.max(line_start);
                let end = span.range.end.min(line_end);
                if position < start {
                    push_segment(&mut segments, text, position, start, None, &selection);
                }
                if start < end {
                    push_segment(&mut segments, text, start, end, Some(span.kind), &selection);
                    position = end;
                }
            }
            if position < line_end {
                push_segment(&mut segments, text, position, line_end, None, &selection);
            }
            if segments.is_empty() {
                segments.push((String::new(), None, false));
            }
            line_start = line_end.saturating_add(1);
            segments
        })
        .collect()
}

#[derive(Debug, Clone)]
struct ProjectIgnoreRule {
    pattern: String,
    directory_only: bool,
    anchored: bool,
    negated: bool,
}

#[derive(Debug, Default)]
struct ProjectIgnore {
    rules: Vec<ProjectIgnoreRule>,
}

impl ProjectIgnore {
    fn from_root(root: &Path) -> Self {
        let rules = std::fs::read_to_string(root.join(".gitignore"))
            .ok()
            .into_iter()
            .flat_map(|contents| contents.lines().map(str::to_owned).collect::<Vec<_>>())
            .filter_map(|line| Self::parse_rule(&line))
            .collect();
        Self { rules }
    }

    fn parse_rule(line: &str) -> Option<ProjectIgnoreRule> {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return None;
        }
        let (negated, line) = line
            .strip_prefix('!')
            .map_or((false, line), |pattern| (true, pattern));
        let (anchored, line) = line
            .strip_prefix('/')
            .map_or((false, line), |pattern| (true, pattern));
        let (directory_only, pattern) = line
            .strip_suffix('/')
            .map_or((false, line), |pattern| (true, pattern));
        (!pattern.is_empty()).then(|| ProjectIgnoreRule {
            pattern: pattern.to_owned(),
            directory_only,
            anchored,
            negated,
        })
    }

    fn ignores(&self, relative: &Path, is_dir: bool) -> bool {
        let name = relative
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if is_dir && DEFAULT_IGNORED_DIRECTORIES.contains(&name) {
            return true;
        }
        if is_dir && name.starts_with('.') && !VISIBLE_DOT_DIRECTORIES.contains(&name) {
            return true;
        }

        let relative = relative.to_string_lossy().replace('\\', "/");
        let mut ignored = false;
        for rule in &self.rules {
            if rule.directory_only && !is_dir {
                continue;
            }
            let candidate = if rule.anchored || rule.pattern.contains('/') {
                relative.as_str()
            } else {
                name
            };
            if Self::glob_matches(&rule.pattern, candidate) {
                ignored = !rule.negated;
            }
        }
        ignored
    }

    fn glob_matches(pattern: &str, value: &str) -> bool {
        fn matches(pattern: &[u8], value: &[u8]) -> bool {
            match pattern {
                [] => value.is_empty(),
                [b'*', b'*', b'/', rest @ ..] => {
                    matches(rest, value) || (!value.is_empty() && matches(pattern, &value[1..]))
                }
                [b'*', rest @ ..] => {
                    matches(rest, value)
                        || (!value.is_empty() && value[0] != b'/' && matches(pattern, &value[1..]))
                }
                [b'?', rest @ ..] => {
                    !value.is_empty() && value[0] != b'/' && matches(rest, &value[1..])
                }
                [head, rest @ ..] => {
                    !value.is_empty() && *head == value[0] && matches(rest, &value[1..])
                }
            }
        }
        matches(pattern.as_bytes(), value.as_bytes())
    }
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
    /// Return true only when closing is safe or the user has confirmed it.
    pub(super) fn confirm_window_close(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let count = usize::from(
            self.editor_buffer
                .as_ref()
                .is_some_and(tf_editor::Buffer::is_dirty),
        ) + self
            .editor_buffers
            .values()
            .filter(|buffer| buffer.is_dirty())
            .count();
        if count == 0 && self.editor_saves.is_empty() {
            return true;
        }
        if self.close_confirmation_pending {
            return false;
        }
        self.close_confirmation_pending = true;
        if !self.editor_saves.is_empty() {
            let answer = window.prompt(
                gpui::PromptLevel::Info,
                "Save in progress",
                Some("Wait for the current save to finish before closing."),
                &["Keep editing"],
                cx,
            );
            cx.spawn(async move |this, cx| {
                let _ = answer.await;
                let _ = this.update(cx, |view, _| view.close_confirmation_pending = false);
            })
            .detach();
            return false;
        }
        let detail = format!(
            "{count} file{} contain{} unsaved changes. Closing will discard them.",
            if count == 1 { "" } else { "s" },
            if count == 1 { "s" } else { "" },
        );
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            "Discard unsaved changes?",
            Some(&detail),
            &["Keep editing", "Discard and close"],
            cx,
        );
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let discard = matches!(answer.await, Ok(1));
            let _ = this.update(cx, |view, _| view.close_confirmation_pending = false);
            if discard {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            }
        })
        .detach();
        false
    }

    pub(super) fn request_window_close(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.confirm_window_close(window, cx) {
            window.remove_window();
        }
    }

    #[cfg(test)]
    pub(super) fn toggle_terminal_maximized(&mut self, cx: &mut Context<'_, Self>) {
        self.terminal_maximized = !self.terminal_maximized;
        self.dock_collapsed = false;
        self.dock_tool = DockTool::Terminal;
        self.persist_layout();
        cx.notify();
    }

    pub(super) fn reset_project_files(&mut self) {
        if let Some(cancel) = self.project_files_cancel.take() {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.project_files_generation = self.project_files_generation.wrapping_add(1);
        self.project_file_entries.clear();
        self.project_files_loading = false;
        self.project_files_error = None;
        self.project_files_checked_at = None;
    }

    pub(super) fn refresh_project_files(&mut self, cx: &mut Context<'_, Self>) {
        let Some(root) = self.local_project_root.clone() else {
            self.reset_project_files();
            return;
        };
        if let Some(cancel) = self.project_files_cancel.take() {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.project_files_generation = self.project_files_generation.wrapping_add(1);
        let generation = self.project_files_generation;
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.project_files_cancel = Some(cancel.clone());
        self.project_files_loading = true;
        self.project_files_error = None;
        cx.spawn(async move |this, cx| {
            let requested_root = root.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    std::fs::read_dir(&root)
                        .map_err(|error| error.to_string())
                        .map(|_| Self::project_files(&root, &cancel))
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.project_files_generation != generation
                    || view.local_project_root.as_ref() != Some(&requested_root)
                {
                    return;
                }
                view.project_files_loading = false;
                view.project_files_cancel = None;
                view.project_files_checked_at = Some(std::time::Instant::now());
                match result {
                    Ok(entries) => {
                        view.project_file_entries = entries;
                        view.project_files_error = None;
                    }
                    Err(error) => view.project_files_error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn open_project_file(&mut self, relative: PathBuf, cx: &mut Context<'_, Self>) {
        if self.selected_project_file.as_ref() == Some(&relative)
            && (self.editor_buffer.is_some() || self.editor_loading)
        {
            return;
        }
        self.park_editor_buffer();
        self.editor_scroll = gpui::ScrollHandle::new();
        self.editor_open_generation = self.editor_open_generation.wrapping_add(1);
        let generation = self.editor_open_generation;
        self.lsp.stop();
        self.lsp_server = None;
        self.lsp_error = None;
        self.lsp_restart_at = None;
        self.editor_pending_position = None;
        self.selected_project_file = Some(relative.clone());
        self.editor_search = None;
        self.editor_error = None;
        let Some(root) = self.local_project_root.clone() else {
            self.editor_loading = false;
            self.editor_error = Some("No local project is open".into());
            return;
        };
        let path = root.join(&relative);
        if self.editor_saves.contains(&path)
            || self
                .editor_buffers
                .get(&path)
                .is_some_and(tf_editor::Buffer::is_dirty)
        {
            self.editor_buffer = self.editor_buffers.remove(&path);
            self.editor_loading = false;
            self.editor_error = self.editor_save_errors.get(&path).cloned();
            self.reveal_editor_selection();
            self.start_editor_lsp();
            return;
        }
        let cached = self.editor_buffers.get(&path).cloned();
        self.editor_loading = true;
        cx.spawn(async move |this, cx| {
            let requested_path = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    match cached {
                        Some(buffer)
                            if buffer.external_change() == tf_editor::ExternalChange::Unchanged =>
                        {
                            Ok(buffer)
                        }
                        _ => tf_editor::Buffer::open(path),
                    }
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.editor_open_generation != generation
                    || view.local_project_root.as_ref() != Some(&root)
                    || view.selected_project_file.as_ref() != Some(&relative)
                {
                    return;
                }
                view.editor_loading = false;
                view.editor_buffers.remove(&requested_path);
                match result {
                    Ok(buffer) => {
                        view.editor_buffer = Some(buffer);
                        view.editor_error = view.editor_save_errors.get(&requested_path).cloned();
                        view.apply_pending_editor_position();
                        view.reveal_editor_selection();
                        view.start_editor_lsp();
                    }
                    Err(error) => {
                        view.editor_buffer = None;
                        view.editor_error = Some(match error {
                            tf_editor::OpenError::Io(message) => message,
                            tf_editor::OpenError::TooLarge(bytes) => {
                                format!("File is too large to edit ({bytes} bytes; limit is 2 MiB)")
                            }
                            tf_editor::OpenError::NotUtf8 => "File is not valid UTF-8".into(),
                        });
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn park_editor_buffer(&mut self) {
        if let Some(buffer) = self.editor_buffer.take() {
            self.editor_buffers
                .insert(buffer.path().to_path_buf(), buffer);
        }
    }

    pub(super) fn save_editor_buffer(&mut self, cx: &mut Context<'_, Self>) {
        let Some(snapshot) = self.editor_buffer.clone() else {
            return;
        };
        self.save_editor_snapshot(snapshot, cx);
    }

    fn save_editor_snapshot(
        &mut self,
        mut snapshot: tf_editor::Buffer,
        cx: &mut Context<'_, Self>,
    ) {
        let path = snapshot.path().to_path_buf();
        if !self.editor_saves.insert(path.clone()) {
            self.editor_queued_saves.insert(path, snapshot);
            return;
        }
        self.editor_save_errors.remove(&path);
        self.editor_error = None;
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move {
                match snapshot.external_change() {
                    tf_editor::ExternalChange::Unchanged => snapshot.save_atomic().map_err(|error| format!("Could not save: {error}"))?,
                    tf_editor::ExternalChange::Changed => return Err("File changed on disk; reopen it before saving to avoid overwriting changes".to_owned()),
                    tf_editor::ExternalChange::Deleted => return Err("File was deleted on disk; reopen the project before saving".to_owned()),
                }
                Ok(snapshot)
            }).await;
            let _ = this.update(cx, |view, cx| {
                view.editor_saves.remove(&path);
                let active = view.editor_buffer.as_ref().is_some_and(|buffer| buffer.path() == path);
                match result {
                    Ok(saved) => {
                        let buffer = if active { view.editor_buffer.as_mut() } else { view.editor_buffers.get_mut(&path) };
                        if let Some(buffer) = buffer {
                            buffer.apply_saved_snapshot(&saved);
                        }
                        view.editor_save_errors.remove(&path);
                        if active {
                            view.editor_error = None;
                            view.lsp.save();
                        }
                        if let Some(mut queued) = view.editor_queued_saves.remove(&path) {
                            if queued.apply_saved_snapshot(&saved) {
                                view.save_editor_snapshot(queued, cx);
                            }
                        }
                    }
                    Err(error) => {
                        view.editor_queued_saves.remove(&path);
                        view.editor_save_errors.insert(path.clone(), error.clone());
                        if active {
                            view.editor_error = Some(error);
                        }
                    }
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    fn start_editor_lsp(&mut self) {
        self.lsp_restart_attempts = 0;
        self.lsp_restart_at = None;
        self.lsp_error = None;
        self.launch_editor_lsp();
    }

    fn launch_editor_lsp(&mut self) {
        self.lsp_server = None;
        let Some(root) = self.local_project_root.as_deref() else {
            self.lsp.stop();
            return;
        };
        let Some(buffer) = self.editor_buffer.as_ref() else {
            self.lsp.stop();
            return;
        };
        self.lsp
            .open(root, buffer.path(), buffer.text(), buffer.revision());
    }

    pub(super) fn sync_editor_lsp(&mut self) {
        if let Some(buffer) = self.editor_buffer.as_ref() {
            self.lsp_diagnostics
                .retain(|diagnostic| diagnostic.path != buffer.path());
            self.lsp.change(buffer.text(), buffer.revision());
        }
    }

    pub(super) fn poll_lsp(&mut self) -> bool {
        let generation = self.lsp.generation();
        let events = self.lsp.drain();
        let mut changed = !events.is_empty();
        for event in events {
            match event {
                crate::lsp::Event::Ready {
                    generation: event_generation,
                    server,
                } if event_generation == generation => {
                    self.lsp_server = Some(server);
                    self.lsp_error = None;
                    self.lsp_restart_at = None;
                }
                crate::lsp::Event::Diagnostics {
                    generation: event_generation,
                    path,
                    version,
                    items,
                } if event_generation == generation => {
                    self.update_file_diagnostics(path, version, items)
                }
                crate::lsp::Event::Failed {
                    generation: event_generation,
                    message,
                } if event_generation == generation => {
                    self.lsp_server = None;
                    self.lsp_error = Some(message);
                }
                crate::lsp::Event::Stopped {
                    generation: event_generation,
                } if event_generation == generation => {
                    self.lsp_server = None;
                    self.lsp.acknowledge_stopped(event_generation);
                    if self.lsp_restart_attempts < 4 {
                        let delay = 1_u64 << self.lsp_restart_attempts;
                        self.lsp_restart_attempts += 1;
                        self.lsp_restart_at =
                            Some(std::time::Instant::now() + std::time::Duration::from_secs(delay));
                    }
                }
                _ => {}
            }
        }
        if self
            .lsp_restart_at
            .is_some_and(|restart| std::time::Instant::now() >= restart)
        {
            self.lsp_restart_at = None;
            self.launch_editor_lsp();
            changed = true;
        }
        changed
    }

    pub(super) fn update_file_diagnostics(
        &mut self,
        path: PathBuf,
        version: Option<i64>,
        items: Vec<crate::lsp::Diagnostic>,
    ) {
        let buffer = self
            .editor_buffer
            .as_ref()
            .filter(|buffer| buffer.path() == path)
            .or_else(|| self.editor_buffers.get(&path));
        if let (Some(version), Some(buffer)) = (version, buffer) {
            if i64::try_from(buffer.revision()).unwrap_or(i64::MAX) != version {
                return;
            }
        }
        self.lsp_diagnostics
            .retain(|diagnostic| diagnostic.path != path);
        self.lsp_diagnostics.extend(
            items
                .into_iter()
                .filter(|diagnostic| diagnostic.path == path),
        );
    }

    pub(super) fn open_diagnostic(
        &mut self,
        diagnostic: &crate::lsp::Diagnostic,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(root) = self.local_project_root.as_ref() else {
            return;
        };
        let Ok(relative) = diagnostic.path.strip_prefix(root) else {
            return;
        };
        if relative.as_os_str().is_empty()
            || !relative
                .components()
                .all(|component| matches!(component, std::path::Component::Normal(_)))
        {
            return;
        }
        let relative = relative.to_path_buf();
        let path = diagnostic.path.clone();
        self.show_workspace(Workspace::Explore, cx);
        self.open_project_file(relative, cx);
        self.editor_pending_position = Some((path, diagnostic.start));
        self.apply_pending_editor_position();
        self.persist_layout();
        cx.notify();
    }

    fn apply_pending_editor_position(&mut self) {
        let Some((path, position)) = self.editor_pending_position.as_ref() else {
            return;
        };
        let Some(buffer) = self
            .editor_buffer
            .as_mut()
            .filter(|buffer| buffer.path() == path)
        else {
            return;
        };
        if let Ok(offset) = tf_editor::position_to_byte(buffer.text(), *position) {
            buffer.set_selection(offset..offset);
        }
        self.editor_pending_position = None;
        self.reveal_editor_selection();
    }

    pub(super) fn reveal_editor_selection(&self) {
        let Some(buffer) = self.editor_buffer.as_ref() else {
            return;
        };
        let cursor = buffer.cursor();
        let line = buffer.text()[..cursor]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count();
        self.editor_scroll.scroll_to_item(line);
    }

    fn select_project_file(&mut self, path: PathBuf, cx: &mut Context<'_, Self>) {
        self.open_project_file(path, cx);
        self.persist_layout();
        cx.notify();
    }

    fn open_selected_file_externally(&self) {
        let Some(path) = self.selected_project_file.as_ref().and_then(|relative| {
            self.local_project_root
                .as_ref()
                .map(|root| root.join(relative))
        }) else {
            return;
        };
        if let Err(error) = launch_external_editor(&path) {
            tracing::warn!(path = %path.display(), "could not open external editor: {error}");
        }
    }

    pub(super) fn change_selection_is_current(&self) -> bool {
        let Inspector::Diff(selected) = &self.inspector else {
            return false;
        };
        self.changes.as_ref().is_some_and(|changes| {
            changes.iter().any(|change| {
                change.path == *selected
                    && match self.selected_git_scope {
                        surface::DiffScope::Staged => change.staged,
                        surface::DiffScope::Unstaged => change.unstaged,
                    }
            })
        })
    }

    pub(super) fn open_first_workbench_diff(&mut self, cx: &mut Context<'_, Self>) {
        let change = self.changes.as_ref().and_then(|changes| {
            changes
                .iter()
                .find(|change| change.unstaged)
                .map(|change| (change.clone(), surface::DiffScope::Unstaged))
                .or_else(|| {
                    changes
                        .iter()
                        .find(|change| change.staged)
                        .map(|change| (change.clone(), surface::DiffScope::Staged))
                })
        });
        if let Some((change, scope)) = change {
            self.select_workbench_change(change, scope, cx);
        } else {
            self.git_diff_generation = self.git_diff_generation.wrapping_add(1);
            self.git_diff_loading = false;
            self.git_diff_error = None;
            self.diff_lines.clear();
            self.inspector = Inspector::Session;
        }
    }

    pub(super) fn open_workbench_diff(
        &mut self,
        change: GitChange,
        scope: surface::DiffScope,
        cx: &mut Context<'_, Self>,
    ) {
        let untracked = change.kind == surface::ChangeKind::Untracked;
        let Some(root) = self.changes_dir.clone() else {
            return;
        };
        self.git_diff_generation = self.git_diff_generation.wrapping_add(1);
        let generation = self.git_diff_generation;
        let project_generation = self.git_project_generation;
        let requested_root = root.clone();
        self.diff_lines.clear();
        let selected_path = change.path.clone();
        self.inspector = Inspector::Diff(selected_path.clone());
        self.selected_git_scope = scope;
        self.git_diff_loading = true;
        self.git_diff_error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let path = change.path;
            let diff = cx
                .background_executor()
                .spawn(async move {
                    surface::git_diff(
                        &root,
                        &path,
                        change.original_path.as_deref(),
                        scope,
                        untracked,
                    )
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.git_diff_generation != generation
                    || view.git_project_generation != project_generation
                    || view.changes_dir.as_ref() != Some(&requested_root)
                    || view.inspector != Inspector::Diff(selected_path)
                    || view.selected_git_scope != scope
                {
                    return;
                }
                view.git_diff_loading = false;
                match diff {
                    Some(text) => view.diff_lines = text.lines().map(str::to_owned).collect(),
                    None => {
                        view.git_diff_error =
                            Some("Git could not read this diff. Refresh and try again.".into())
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn select_workbench_change(
        &mut self,
        change: GitChange,
        scope: surface::DiffScope,
        cx: &mut Context<'_, Self>,
    ) {
        self.open_workbench_diff(change, scope, cx);
    }

    fn mutate_workbench_change(
        &mut self,
        change: GitChange,
        stage: bool,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(root) = self.changes_dir.clone() else {
            return;
        };
        let project_generation = self.git_project_generation;
        let requested_root = root.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    if stage {
                        surface::git_stage(&root, &change.path)
                    } else {
                        surface::git_unstage_change(&root, &change)
                    }
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.git_project_generation != project_generation
                    || view.changes_dir.as_ref() != Some(&requested_root)
                {
                    return;
                }
                if let Err(error) = result {
                    view.git_error = Some(error);
                    cx.notify();
                    return;
                }
                view.refresh_changes(cx);
                if !view.change_selection_is_current() {
                    view.open_first_workbench_diff(cx);
                }
            });
        })
        .detach();
    }

    fn set_git_diff_view(&mut self, view: GitDiffView, cx: &mut Context<'_, Self>) {
        self.git_diff_view = view;
        self.persist_layout();
        cx.notify();
    }

    fn toggle_reviewed_change(&mut self, cx: &mut Context<'_, Self>) {
        let Inspector::Diff(path) = &self.inspector else {
            return;
        };
        let key = (path.clone(), self.selected_git_scope);
        if !self.reviewed_changes.insert(key.clone()) {
            self.reviewed_changes.remove(&key);
        }
        cx.notify();
    }

    fn begin_git_discard(&mut self, change: GitChange, cx: &mut Context<'_, Self>) {
        self.git_dialog = Some(GitDialog::Discard(change));
        cx.notify();
    }

    fn begin_git_commit(&mut self, cx: &mut Context<'_, Self>) {
        self.git_dialog = Some(GitDialog::Commit(String::new()));
        cx.notify();
    }

    fn submit_git_dialog(&mut self, cx: &mut Context<'_, Self>) {
        let Some(dialog) = self.git_dialog.take() else {
            return;
        };
        let Some(root) = self.changes_dir.clone() else {
            return;
        };
        let project_generation = self.git_project_generation;
        let requested_root = root.clone();
        self.git_loading = true;
        self.git_error = None;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    match dialog {
                        GitDialog::Discard(change) => surface::git_discard(
                            &root,
                            &change.path,
                            change.kind == surface::ChangeKind::Untracked,
                        ),
                        GitDialog::Commit(message) => surface::git_commit(&root, &message),
                    }
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.git_project_generation != project_generation
                    || view.changes_dir.as_ref() != Some(&requested_root)
                {
                    return;
                }
                view.git_loading = false;
                match result {
                    Ok(()) => view.refresh_changes(cx),
                    Err(error) => {
                        view.git_error = Some(error);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    pub(super) fn git_dialog_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let Some(dialog) = &mut self.git_dialog else {
            return false;
        };
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        match key {
            "escape" => self.git_dialog = None,
            "enter" => {
                let can_submit = match dialog {
                    GitDialog::Discard(_) => true,
                    GitDialog::Commit(message) => surface::validate_commit_message(message).is_ok(),
                };
                if can_submit {
                    self.submit_git_dialog(cx);
                }
                return true;
            }
            "backspace" => {
                if let GitDialog::Commit(message) = dialog {
                    message.pop();
                }
            }
            "v" if modifiers.control => {
                if let (GitDialog::Commit(message), Some(text)) = (
                    dialog,
                    cx.read_from_clipboard().and_then(|item| item.text()),
                ) {
                    message.extend(
                        text.lines()
                            .next()
                            .unwrap_or_default()
                            .chars()
                            .filter(|character| !character.is_control()),
                    );
                    message.truncate(200);
                }
            }
            _ if !modifiers.control && !modifiers.alt => {
                if let (GitDialog::Commit(message), Some(text)) =
                    (dialog, event.keystroke.key_char.as_deref())
                {
                    if !text.chars().any(char::is_control) && message.chars().count() < 200 {
                        message.push_str(text);
                    }
                }
            }
            _ => {}
        }
        cx.notify();
        true
    }

    fn select_file_explorer_mode(&mut self, mode: FileExplorerMode, cx: &mut Context<'_, Self>) {
        self.file_explorer_mode = mode;
        self.persist_layout();
        cx.notify();
    }

    fn toggle_file_path(&mut self, path: PathBuf, cx: &mut Context<'_, Self>) {
        if !self.collapsed_file_paths.insert(path.clone()) {
            self.collapsed_file_paths.remove(&path);
        }
        cx.notify();
    }

    fn set_file_tree_collapsed(
        &mut self,
        paths: Vec<PathBuf>,
        collapsed: bool,
        cx: &mut Context<'_, Self>,
    ) {
        if collapsed {
            self.collapsed_file_paths.extend(paths);
        } else {
            self.collapsed_file_paths.clear();
        }
        cx.notify();
    }

    fn toggle_file_group(&mut self, group: &'static str, cx: &mut Context<'_, Self>) {
        if !self.collapsed_file_groups.insert(group) {
            self.collapsed_file_groups.remove(group);
        }
        cx.notify();
    }

    pub(super) fn workbench_changes(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let t = &self.theme;
        let changes = self.changes.as_deref().unwrap_or_default();
        let selected = match &self.inspector {
            Inspector::Diff(path) => Some(path.as_str()),
            Inspector::Session => None,
        };
        let (added, removed) = surface::parse_diff(&self.diff_lines.join("\n"))
            .iter()
            .fold((0usize, 0usize), |counts, line| match line.kind {
                surface::DiffLine::Added => (counts.0 + 1, counts.1),
                surface::DiffLine::Removed => (counts.0, counts.1 + 1),
                _ => counts,
            });
        let selected_label = selected.unwrap_or("Select a changed file").to_owned();
        let staged_count = changes.iter().filter(|change| change.staged).count();
        let reviewed = selected.is_some_and(|path| {
            self.reviewed_changes
                .contains(&(path.to_owned(), self.selected_git_scope))
        });
        let mut grouped_changes = Vec::new();
        if changes.iter().any(|change| change.unstaged) {
            grouped_changes.push((None, surface::DiffScope::Unstaged));
            grouped_changes.extend(
                changes
                    .iter()
                    .filter(|change| change.unstaged)
                    .map(|change| (Some(change), surface::DiffScope::Unstaged)),
            );
        }
        if changes.iter().any(|change| change.staged) {
            grouped_changes.push((None, surface::DiffScope::Staged));
            grouped_changes.extend(
                changes
                    .iter()
                    .filter(|change| change.staged)
                    .map(|change| (Some(change), surface::DiffScope::Staged)),
            );
        }

        div()
            .id("workbench-changes")
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .bg(hsla(t.bg_app))
            .child(
                div()
                    .w(px(292.0))
                    .min_w(px(240.0))
                    .h_full()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .border_r_1()
                    .border_color(hsla(t.border))
                    .bg(hsla(t.bg_panel))
                    .child(
                        div()
                            .h(px(42.0))
                            .px(px(tokens::space::MD))
                            .flex()
                            .items_center()
                            .justify_between()
                            .border_b_1()
                            .border_color(hsla(t.border))
                            .text_size(px(tokens::text::SM))
                            .text_color(hsla(t.text))
                            .child("Changes")
                            .when(staged_count > 0, |d| {
                                d.child(
                                    div()
                                        .id("git-commit")
                                        .px(px(tokens::space::SM))
                                        .py(px(4.0))
                                        .rounded(px(tokens::radius::MD))
                                        .bg(hsla(t.accent))
                                        .text_size(px(tokens::text::XS))
                                        .text_color(hsla(t.accent_text))
                                        .hover(|style| style.opacity(0.88))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.begin_git_commit(cx);
                                        }))
                                        .child("Commit"),
                                )
                            })
                            .child(
                                div()
                                    .px(px(tokens::space::SM))
                                    .py(px(2.0))
                                    .rounded_full()
                                    .bg(with_alpha(hsla(t.text), 0.06))
                                    .text_size(px(tokens::text::XS))
                                    .text_color(hsla(t.text_faint))
                                    .child(SharedString::from(changes.len().to_string())),
                            ),
                    )
                    .child(
                        div()
                            .id("workbench-changes-list")
                            .flex_1()
                            .min_h(px(0.0))
                            .overflow_y_scroll()
                            .p(px(tokens::space::XS))
                            .when(
                                changes.is_empty() && !self.git_loading && self.git_error.is_none(),
                                |d| {
                                    d.child(
                                        div()
                                            .p(px(tokens::space::MD))
                                            .text_size(px(tokens::text::SM))
                                            .text_color(hsla(t.text_faint))
                                            .child("Working tree clean"),
                                    )
                                },
                            )
                            .when(self.git_loading && changes.is_empty(), |d| {
                                d.child(
                                    div()
                                        .p(px(tokens::space::MD))
                                        .text_size(px(tokens::text::SM))
                                        .text_color(hsla(t.text_faint))
                                        .child("Reading repository…"),
                                )
                            })
                            .when_some(self.git_error.as_ref(), |d, error| {
                                d.child(
                                    div()
                                        .m(px(tokens::space::SM))
                                        .p(px(tokens::space::SM))
                                        .rounded(px(tokens::radius::MD))
                                        .bg(with_alpha(hsla(t.danger), 0.08))
                                        .text_size(px(tokens::text::XS))
                                        .text_color(hsla(t.danger))
                                        .child(SharedString::from(error.clone())),
                                )
                            })
                            .children(grouped_changes.into_iter().map(|(change, scope)| {
                                let Some(change) = change else {
                                    return div()
                                        .h(px(28.0))
                                        .px(px(tokens::space::SM))
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .text_size(px(tokens::text::XS))
                                        .text_color(hsla(t.text_faint))
                                        .child(match scope {
                                            surface::DiffScope::Unstaged => "CHANGES",
                                            surface::DiffScope::Staged => "STAGED CHANGES",
                                        })
                                        .into_any_element();
                                };
                                let active = selected == Some(change.path.as_str())
                                    && self.selected_git_scope == scope;
                                let path = change.path.clone();
                                let selected_change = change.clone();
                                let stage_change = change.clone();
                                let unstage_change = change.clone();
                                let discard_change = change.clone();
                                let row_reviewed =
                                    self.reviewed_changes.contains(&(path.clone(), scope));
                                let parent = Path::new(&path)
                                    .parent()
                                    .filter(|parent| !parent.as_os_str().is_empty())
                                    .map(|parent| parent.to_string_lossy().into_owned());
                                div()
                                    .id(SharedString::from(format!("workbench-change-{path}")))
                                    .h(px(32.0))
                                    .px(px(tokens::space::SM))
                                    .flex()
                                    .items_center()
                                    .gap(px(tokens::space::SM))
                                    .rounded(px(tokens::radius::MD))
                                    .when(active, |d| d.bg(hsla(t.bg_selected)))
                                    .when(!active, |d| d.hover(|style| style.bg(hsla(t.bg_hover))))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.select_workbench_change(
                                            selected_change.clone(),
                                            scope,
                                            cx,
                                        );
                                    }))
                                    .child(
                                        div()
                                            .w(px(16.0))
                                            .text_size(px(tokens::text::XS))
                                            .text_color(hsla(t.warning))
                                            .child(change.kind.letter()),
                                    )
                                    .when(row_reviewed, |d| {
                                        d.child(
                                            div()
                                                .text_size(px(tokens::text::XS))
                                                .text_color(hsla(t.success))
                                                .child("✓"),
                                        )
                                    })
                                    .child(
                                        div()
                                            .min_w(px(0.0))
                                            .flex_1()
                                            .flex()
                                            .items_baseline()
                                            .gap(px(tokens::space::SM))
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_size(px(tokens::text::SM))
                                            .text_color(hsla(if active {
                                                t.text
                                            } else {
                                                t.text_muted
                                            }))
                                            .child(SharedString::from(
                                                Path::new(&path)
                                                    .file_name()
                                                    .map(|name| name.to_string_lossy().into_owned())
                                                    .unwrap_or(path.clone()),
                                            ))
                                            .children(parent.map(|parent| {
                                                div()
                                                    .text_size(px(tokens::text::XS))
                                                    .text_color(hsla(t.text_faint))
                                                    .child(SharedString::from(parent))
                                            })),
                                    )
                                    .when(scope == surface::DiffScope::Unstaged, |d| {
                                        d.child(
                                            div()
                                                .id(SharedString::from(format!("discard-{path}")))
                                                .px(px(tokens::space::XS))
                                                .rounded(px(tokens::radius::SM))
                                                .text_size(px(tokens::text::XS))
                                                .text_color(hsla(t.danger))
                                                .hover(|style| {
                                                    style.bg(with_alpha(hsla(t.danger), 0.10))
                                                })
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.begin_git_discard(
                                                        discard_change.clone(),
                                                        cx,
                                                    );
                                                }))
                                                .child("Discard"),
                                        )
                                    })
                                    .when(scope == surface::DiffScope::Unstaged, |d| {
                                        d.child(
                                            div()
                                                .id(SharedString::from(format!("stage-{path}")))
                                                .px(px(tokens::space::XS))
                                                .rounded(px(tokens::radius::SM))
                                                .text_size(px(tokens::text::XS))
                                                .text_color(hsla(t.text_faint))
                                                .hover(|style| {
                                                    style
                                                        .bg(hsla(t.bg_hover))
                                                        .text_color(hsla(t.text))
                                                })
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.mutate_workbench_change(
                                                        stage_change.clone(),
                                                        true,
                                                        cx,
                                                    );
                                                }))
                                                .child("Stage"),
                                        )
                                    })
                                    .when(scope == surface::DiffScope::Staged, |d| {
                                        d.child(
                                            div()
                                                .id(SharedString::from(format!("unstage-{path}")))
                                                .px(px(tokens::space::XS))
                                                .rounded(px(tokens::radius::SM))
                                                .text_size(px(tokens::text::XS))
                                                .text_color(hsla(t.text_faint))
                                                .hover(|style| {
                                                    style
                                                        .bg(hsla(t.bg_hover))
                                                        .text_color(hsla(t.text))
                                                })
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.mutate_workbench_change(
                                                        unstage_change.clone(),
                                                        false,
                                                        cx,
                                                    );
                                                }))
                                                .child("Unstage"),
                                        )
                                    })
                                    .into_any_element()
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .h(px(42.0))
                            .px(px(tokens::space::MD))
                            .flex()
                            .items_center()
                            .gap(px(tokens::space::MD))
                            .border_b_1()
                            .border_color(hsla(t.border))
                            .bg(hsla(t.bg_panel))
                            .child(
                                div()
                                    .min_w(px(0.0))
                                    .flex_1()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(px(tokens::text::SM))
                                    .text_color(hsla(t.text))
                                    .child(SharedString::from(selected_label)),
                            )
                            .when(selected.is_some(), |d| {
                                d.child(
                                    div()
                                        .px(px(tokens::space::SM))
                                        .py(px(2.0))
                                        .rounded(px(tokens::radius::SM))
                                        .bg(with_alpha(hsla(t.accent), 0.08))
                                        .text_size(px(tokens::text::XS))
                                        .text_color(hsla(t.accent))
                                        .child(match self.selected_git_scope {
                                            surface::DiffScope::Staged => "Staged",
                                            surface::DiffScope::Unstaged => "Working tree",
                                        }),
                                )
                            })
                            .when(selected.is_some(), |d| {
                                d.child(
                                    div()
                                        .p(px(2.0))
                                        .flex()
                                        .rounded(px(tokens::radius::MD))
                                        .bg(with_alpha(hsla(t.text), 0.05))
                                        .children(
                                            [
                                                (GitDiffView::Unified, "Unified"),
                                                (GitDiffView::Split, "Split"),
                                            ]
                                            .into_iter()
                                            .map(
                                                |(view, label)| {
                                                    let active = self.git_diff_view == view;
                                                    div()
                                                        .id(SharedString::from(format!(
                                                            "git-view-{label}"
                                                        )))
                                                        .px(px(tokens::space::SM))
                                                        .py(px(3.0))
                                                        .rounded(px(tokens::radius::SM))
                                                        .text_size(px(tokens::text::XS))
                                                        .when(active, |d| {
                                                            d.bg(hsla(t.bg_selected))
                                                                .text_color(hsla(t.text))
                                                        })
                                                        .when(!active, |d| {
                                                            d.text_color(hsla(t.text_faint)).hover(
                                                                |style| {
                                                                    style.text_color(hsla(t.text))
                                                                },
                                                            )
                                                        })
                                                        .on_click(cx.listener(
                                                            move |this, _, _, cx| {
                                                                this.set_git_diff_view(view, cx)
                                                            },
                                                        ))
                                                        .child(label)
                                                },
                                            ),
                                        ),
                                )
                            })
                            .when(selected.is_some(), |d| {
                                d.child(
                                    div()
                                        .id("git-reviewed-toggle")
                                        .px(px(tokens::space::SM))
                                        .py(px(4.0))
                                        .rounded(px(tokens::radius::MD))
                                        .border_1()
                                        .border_color(hsla(if reviewed {
                                            t.success
                                        } else {
                                            t.border
                                        }))
                                        .text_size(px(tokens::text::XS))
                                        .text_color(hsla(if reviewed {
                                            t.success
                                        } else {
                                            t.text_muted
                                        }))
                                        .hover(|style| style.bg(hsla(t.bg_hover)))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.toggle_reviewed_change(cx)
                                        }))
                                        .child(if reviewed {
                                            "✓ Reviewed"
                                        } else {
                                            "Mark reviewed"
                                        }),
                                )
                            })
                            .when(selected.is_some(), |d| {
                                d.child(
                                    div().text_size(px(tokens::text::XS)).child(
                                        div()
                                            .flex()
                                            .gap(px(tokens::space::SM))
                                            .child(
                                                div()
                                                    .text_color(hsla(t.success))
                                                    .child(SharedString::from(format!("+{added}"))),
                                            )
                                            .child(
                                                div().text_color(hsla(t.danger)).child(
                                                    SharedString::from(format!("−{removed}")),
                                                ),
                                            ),
                                    ),
                                )
                            }),
                    )
                    .child(self.workbench_diff_body()),
            )
    }

    fn workbench_diff_body(&self) -> impl IntoElement {
        let t = &self.theme;
        let has_selection = matches!(self.inspector, Inspector::Diff(_));
        let rows = surface::parse_diff(&self.diff_lines.join("\n"));
        div()
            .id("workbench-diff-body")
            .flex_1()
            .min_h(px(0.0))
            .overflow_scroll()
            .py(px(tokens::space::SM))
            .when(!has_selection, |d| {
                d.flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(tokens::text::SM))
                    .text_color(hsla(t.text_faint))
                    .child("Choose a file to inspect its changes")
            })
            .when(has_selection && self.git_diff_loading, |d| {
                d.flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(tokens::text::SM))
                    .text_color(hsla(t.text_faint))
                    .child("Loading diff…")
            })
            .when_some(self.git_diff_error.clone(), |d, error| {
                d.p(px(tokens::space::MD))
                    .text_size(px(tokens::text::SM))
                    .text_color(hsla(t.danger))
                    .child(SharedString::from(error))
            })
            .when(
                has_selection
                    && self.diff_lines.is_empty()
                    && !self.git_diff_loading
                    && self.git_diff_error.is_none(),
                |d| {
                    d.flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(tokens::text::SM))
                        .text_color(hsla(t.text_faint))
                        .child("No textual diff available")
                },
            )
            .when(self.git_diff_view == GitDiffView::Unified, |d| {
                d.children(rows.iter().map(|row| {
                    let addition = row.kind == surface::DiffLine::Added;
                    let deletion = row.kind == surface::DiffLine::Removed;
                    let hunk = row.kind == surface::DiffLine::Hunk;
                    div()
                        .min_w_full()
                        .h(px(22.0))
                        .flex()
                        .items_center()
                        .when(addition, |d| d.bg(with_alpha(hsla(t.success), 0.10)))
                        .when(deletion, |d| d.bg(with_alpha(hsla(t.danger), 0.10)))
                        .when(hunk, |d| d.bg(with_alpha(hsla(t.accent), 0.08)))
                        .child(self.diff_line_number(row.old_line))
                        .child(self.diff_line_number(row.new_line))
                        .child(
                            div()
                                .px(px(tokens::space::MD))
                                .whitespace_nowrap()
                                .font_family(self.font.family.clone())
                                .text_size(px(self.font_size - 1.0))
                                .text_color(hsla(if addition {
                                    t.success
                                } else if deletion {
                                    t.danger
                                } else if hunk {
                                    t.accent
                                } else {
                                    t.text_muted
                                }))
                                .child(SharedString::from(row.text.clone())),
                        )
                }))
            })
            .when(self.git_diff_view == GitDiffView::Split, |d| {
                d.children(surface::split_diff_rows(&rows).into_iter().map(|row| {
                    let left_text = row.left.map_or_else(String::new, |line| line.text.clone());
                    let right_text = row.right.map_or_else(String::new, |line| line.text.clone());
                    let removed = row
                        .left
                        .is_some_and(|line| line.kind == surface::DiffLine::Removed);
                    let added = row
                        .right
                        .is_some_and(|line| line.kind == surface::DiffLine::Added);
                    div()
                        .min_w_full()
                        .h(px(22.0))
                        .flex()
                        .child(self.split_diff_cell(
                            row.left.and_then(|line| line.old_line),
                            left_text,
                            removed,
                            false,
                        ))
                        .child(self.split_diff_cell(
                            row.right.and_then(|line| line.new_line),
                            right_text,
                            false,
                            added,
                        ))
                }))
            })
    }

    fn diff_line_number(&self, line: Option<u32>) -> impl IntoElement {
        let t = &self.theme;
        div()
            .w(px(44.0))
            .h_full()
            .flex_none()
            .flex()
            .items_center()
            .justify_end()
            .pr(px(tokens::space::SM))
            .bg(with_alpha(hsla(t.text), 0.025))
            .text_size(px(tokens::text::XS))
            .text_color(hsla(t.text_faint))
            .children(line.map(|line| SharedString::from(line.to_string())))
    }

    fn split_diff_cell(
        &self,
        line: Option<u32>,
        text: String,
        removed: bool,
        added: bool,
    ) -> impl IntoElement {
        let t = &self.theme;
        div()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .flex()
            .border_r_1()
            .border_color(hsla(t.border))
            .when(removed, |d| d.bg(with_alpha(hsla(t.danger), 0.10)))
            .when(added, |d| d.bg(with_alpha(hsla(t.success), 0.10)))
            .child(self.diff_line_number(line))
            .child(
                div()
                    .px(px(tokens::space::MD))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .font_family(self.font.family.clone())
                    .text_size(px(self.font_size - 1.0))
                    .text_color(hsla(if removed {
                        t.danger
                    } else if added {
                        t.success
                    } else {
                        t.text_muted
                    }))
                    .child(SharedString::from(text)),
            )
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
        let entries = &self.project_file_entries;
        let root_label = root.to_string_lossy().into_owned();
        let content = match self.file_explorer_mode {
            FileExplorerMode::Smart => self.smart_file_groups(entries, cx).into_any_element(),
            FileExplorerMode::Tree => self.file_tree(entries, cx).into_any_element(),
        };
        div()
            .id("workbench-files")
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .bg(hsla(t.bg_app))
            .child(
                div()
                    .px(px(tokens::space::LG))
                    .py(px(tokens::space::MD))
                    .flex()
                    .items_start()
                    .justify_between()
                    .border_b_1()
                    .border_color(hsla(t.border))
                    .bg(hsla(t.bg_panel))
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
                    .child(
                        div()
                            .flex()
                            .gap(px(tokens::space::SM))
                            .child(
                                div()
                                    .id("refresh-project-files")
                                    .cursor_pointer()
                                    .text_size(px(tokens::text::XS))
                                    .text_color(hsla(t.text_muted))
                                    .on_click(
                                        cx.listener(|this, _, _, cx| {
                                            this.refresh_project_files(cx)
                                        }),
                                    )
                                    .child(if self.project_files_loading {
                                        "Refreshing…"
                                    } else {
                                        "Refresh"
                                    }),
                            )
                            .child(self.file_mode_switch(cx)),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .child(
                        div()
                            .id("project-file-explorer")
                            .w(px(310.0))
                            .min_w(px(260.0))
                            .h_full()
                            .flex_none()
                            .overflow_y_scroll()
                            .p(px(tokens::space::SM))
                            .border_r_1()
                            .border_color(hsla(t.border))
                            .bg(hsla(t.bg_panel))
                            .when(self.project_files_loading && entries.is_empty(), |d| {
                                d.child(
                                    div()
                                        .p(px(tokens::space::SM))
                                        .text_color(hsla(t.text_faint))
                                        .child("Loading project files…"),
                                )
                            })
                            .when_some(self.project_files_error.clone(), |d, error| {
                                d.child(
                                    div()
                                        .p(px(tokens::space::SM))
                                        .text_color(hsla(t.danger))
                                        .child(SharedString::from(format!(
                                            "Could not read project files: {error}"
                                        ))),
                                )
                            })
                            .child(content),
                    )
                    .child(self.project_file_editor(cx)),
            )
    }

    fn project_file_editor(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let t = &self.theme;
        let path = self
            .selected_project_file
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned());
        let dirty = self
            .editor_buffer
            .as_ref()
            .is_some_and(tf_editor::Buffer::is_dirty);
        let saving = self
            .editor_buffer
            .as_ref()
            .is_some_and(|buffer| self.editor_saves.contains(buffer.path()));
        let diagnostics_by_line: HashMap<u32, crate::lsp::Severity> = self
            .editor_buffer
            .as_ref()
            .map(|buffer| {
                self.lsp_diagnostics
                    .iter()
                    .filter(|diagnostic| diagnostic.path == buffer.path())
                    .map(|diagnostic| (diagnostic.start.line, diagnostic.severity))
                    .collect()
            })
            .unwrap_or_default();
        let lsp_label = self.lsp_server.as_ref().map(|server| {
            let count = diagnostics_by_line.len();
            if count == 0 {
                format!("{server} · ready")
            } else {
                format!(
                    "{server} · {count} problem{}",
                    if count == 1 { "" } else { "s" }
                )
            }
        });
        let editor_search = self.editor_search.as_ref().map(|search| {
            (
                search.query.clone(),
                search.matches.len(),
                search.current.saturating_add(1),
            )
        });
        let editor_lines: Vec<EditorLine> = self
            .editor_buffer
            .as_ref()
            .map(|buffer| {
                let cursor = buffer.cursor();
                let mut rendered = buffer.text().to_owned();
                rendered.insert(cursor, '▏');
                let mut selection = buffer.selection();
                if !selection.is_empty() {
                    if cursor <= selection.start {
                        selection.start += '▏'.len_utf8();
                    }
                    if cursor < selection.end {
                        selection.end += '▏'.len_utf8();
                    }
                }
                let language = tf_editor::Language::from_path(buffer.path());
                let spans = tf_editor::highlight(&rendered, language);
                highlighted_editor_lines(&rendered, &spans, selection)
            })
            .unwrap_or_default();
        div()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(38.0))
                    .px(px(tokens::space::MD))
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(hsla(t.border))
                    .bg(hsla(t.bg_panel))
                    .text_size(px(tokens::text::SM))
                    .text_color(hsla(t.text_muted))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(SharedString::from(
                                path.clone()
                                    .map(|path| if dirty { format!("{path}  •") } else { path })
                                    .unwrap_or_else(|| "No file open".into()),
                            )),
                    )
                    .when(self.editor_buffer.is_some(), |d| {
                        d.child(
                            div()
                                .mr(px(tokens::space::SM))
                                .text_size(px(tokens::text::XS))
                                .text_color(hsla(t.text_faint))
                                .child("Ctrl+S save · Ctrl+Z undo"),
                        )
                    })
                    .when(saving, |d| {
                        d.child(
                            div()
                                .mr(px(tokens::space::SM))
                                .text_size(px(tokens::text::XS))
                                .text_color(hsla(t.text_faint))
                                .child("Saving…"),
                        )
                    })
                    .when_some(lsp_label, |d, label| {
                        d.child(
                            div()
                                .mr(px(tokens::space::SM))
                                .px(px(tokens::space::SM))
                                .h(px(22.0))
                                .flex()
                                .items_center()
                                .rounded(px(tokens::radius::SM))
                                .bg(with_alpha(hsla(t.success), 0.08))
                                .text_size(px(tokens::text::XS))
                                .text_color(hsla(t.success))
                                .child(SharedString::from(label)),
                        )
                    })
                    .when(path.is_some(), |d| {
                        d.child(
                            div()
                                .id("open-selected-file-externally")
                                .h(px(26.0))
                                .px(px(tokens::space::SM))
                                .flex()
                                .items_center()
                                .gap(px(tokens::space::XS))
                                .rounded(px(tokens::radius::MD))
                                .border_1()
                                .border_color(hsla(t.border))
                                .text_size(px(tokens::text::XS))
                                .text_color(hsla(t.text_muted))
                                .hover(|style| style.bg(hsla(t.bg_hover)).text_color(hsla(t.text)))
                                .on_click(cx.listener(|this, _, _, _| {
                                    this.open_selected_file_externally();
                                }))
                                .child("↗")
                                .child("Open in editor"),
                        )
                    }),
            )
            .when_some(editor_search, |d, (query, count, current)| {
                let empty = query.is_empty();
                d.child(
                    div()
                        .h(px(38.0))
                        .px(px(tokens::space::MD))
                        .flex()
                        .items_center()
                        .gap(px(tokens::space::SM))
                        .border_b_1()
                        .border_color(hsla(t.border))
                        .bg(hsla(t.bg_elevated))
                        .child("⌕")
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .text_size(px(tokens::text::SM))
                                .text_color(hsla(if empty { t.text_faint } else { t.text }))
                                .child(SharedString::from(if empty {
                                    "Find in file…".to_owned()
                                } else {
                                    format!("{query}▏")
                                })),
                        )
                        .child(
                            div()
                                .text_size(px(tokens::text::XS))
                                .text_color(hsla(t.text_faint))
                                .child(SharedString::from(if count == 0 {
                                    "No results".to_owned()
                                } else {
                                    format!("{current} of {count}")
                                })),
                        )
                        .child(
                            div()
                                .id("editor-search-previous")
                                .size(px(24.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(tokens::radius::SM))
                                .hover(|style| style.bg(hsla(t.bg_hover)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.move_editor_search(true, cx);
                                }))
                                .child("↑"),
                        )
                        .child(
                            div()
                                .id("editor-search-next")
                                .size(px(24.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(tokens::radius::SM))
                                .hover(|style| style.bg(hsla(t.bg_hover)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.move_editor_search(false, cx);
                                }))
                                .child("↓"),
                        )
                        .child(
                            div()
                                .id("editor-search-close")
                                .size(px(24.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(tokens::radius::SM))
                                .hover(|style| style.bg(hsla(t.bg_hover)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.editor_search = None;
                                    cx.notify();
                                }))
                                .child("×"),
                        ),
                )
            })
            .when_some(self.editor_error.clone(), |d, error| {
                d.child(
                    div()
                        .px(px(tokens::space::MD))
                        .py(px(tokens::space::SM))
                        .border_b_1()
                        .border_color(hsla(t.border))
                        .bg(with_alpha(hsla(t.danger), 0.07))
                        .text_size(px(tokens::text::XS))
                        .text_color(hsla(t.danger))
                        .child(SharedString::from(error)),
                )
            })
            .when_some(self.lsp_error.clone(), |d, error| {
                d.child(
                    div()
                        .px(px(tokens::space::MD))
                        .py(px(tokens::space::SM))
                        .border_b_1()
                        .border_color(hsla(t.border))
                        .bg(with_alpha(hsla(t.warning), 0.07))
                        .text_size(px(tokens::text::XS))
                        .text_color(hsla(t.warning))
                        .child(SharedString::from(format!(
                            "Language server stopped: {error}"
                        ))),
                )
            })
            .child(
                div()
                    .id("project-file-editor")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_scroll()
                    .track_scroll(&self.editor_scroll)
                    .py(px(tokens::space::SM))
                    .when(path.is_none(), |d| {
                        d.flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(tokens::text::SM))
                            .text_color(hsla(t.text_faint))
                            .child("Choose a file from the project explorer")
                    })
                    .when(path.is_some() && self.editor_buffer.is_none(), |d| {
                        d.flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(tokens::text::SM))
                            .text_color(hsla(t.text_faint))
                            .child(SharedString::from(
                                self.editor_error.clone().unwrap_or_else(|| {
                                    if self.editor_loading {
                                        "Opening file…".into()
                                    } else {
                                        "This file cannot be edited".into()
                                    }
                                }),
                            ))
                    })
                    .children(
                        editor_lines
                            .into_iter()
                            .enumerate()
                            .map(|(index, segments)| {
                                let diagnostic = diagnostics_by_line.get(&(index as u32)).copied();
                                div()
                                    .min_w_full()
                                    .h(px(22.0))
                                    .px(px(tokens::space::MD))
                                    .flex()
                                    .items_center()
                                    .hover(|style| style.bg(hsla(t.bg_hover)))
                                    .child(
                                        div()
                                            .w(px(52.0))
                                            .flex_none()
                                            .text_right()
                                            .pr(px(tokens::space::MD))
                                            .text_size(px(tokens::text::XS))
                                            .text_color(hsla(t.text_faint))
                                            .child(SharedString::from((index + 1).to_string())),
                                    )
                                    .child(
                                        div()
                                            .w(px(12.0))
                                            .flex_none()
                                            .text_size(px(tokens::text::XS))
                                            .text_color(hsla(match diagnostic {
                                                Some(crate::lsp::Severity::Error) => t.danger,
                                                Some(crate::lsp::Severity::Warning) => t.warning,
                                                Some(crate::lsp::Severity::Information) => t.accent,
                                                Some(crate::lsp::Severity::Hint) => t.text_faint,
                                                None => t.text_faint,
                                            }))
                                            .child(if diagnostic.is_some() { "●" } else { "" }),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .whitespace_nowrap()
                                            .font_family(self.font.family.clone())
                                            .text_size(px(self.font_size - 1.0))
                                            .text_color(hsla(t.text_muted))
                                            .children(segments.into_iter().map(
                                                |(text, kind, selected)| {
                                                    let color = match kind {
                                                        Some(tf_editor::HighlightKind::Comment) => {
                                                            t.text_faint
                                                        }
                                                        Some(tf_editor::HighlightKind::String) => {
                                                            t.success
                                                        }
                                                        Some(tf_editor::HighlightKind::Number) => {
                                                            t.warning
                                                        }
                                                        Some(tf_editor::HighlightKind::Keyword) => {
                                                            t.accent
                                                        }
                                                        Some(tf_editor::HighlightKind::Type) => {
                                                            t.ansi[6]
                                                        }
                                                        Some(tf_editor::HighlightKind::Heading) => {
                                                            t.accent
                                                        }
                                                        None => t.text_muted,
                                                    };
                                                    div()
                                                        .when(selected, |d| {
                                                            d.bg(with_alpha(hsla(t.accent), 0.25))
                                                        })
                                                        .text_color(hsla(color))
                                                        .child(SharedString::from(text))
                                                },
                                            )),
                                    )
                            }),
                    ),
            )
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

    fn smart_file_groups(
        &self,
        entries: &[ProjectFileEntry],
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
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
                let landmarks: Vec<_> = entries
                    .iter()
                    .copied()
                    .filter(|entry| entry.depth == 0)
                    .collect();
                if landmarks.is_empty() {
                    return None;
                }
                let label = group.label();
                let collapsed = self.collapsed_file_groups.contains(label);
                Some(
                    div()
                        .rounded(px(tokens::radius::LG))
                        .border_1()
                        .border_color(hsla(t.border))
                        .bg(hsla(t.bg_panel))
                        .overflow_hidden()
                        .child(
                            div()
                                .id(SharedString::from(format!("file-group-{label}")))
                                .h(px(34.0))
                                .px(px(tokens::space::MD))
                                .flex()
                                .items_center()
                                .justify_between()
                                .cursor_pointer()
                                .border_b_1()
                                .border_color(hsla(t.border))
                                .text_size(px(tokens::text::XS))
                                .text_color(hsla(t.text_faint))
                                .hover(|style| style.bg(hsla(t.bg_hover)).text_color(hsla(t.text)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_file_group(label, cx);
                                }))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(tokens::space::SM))
                                        .child(if collapsed { "▸" } else { "▾" })
                                        .child(group.icon())
                                        .child(label),
                                )
                                .child(SharedString::from(entries.len().to_string())),
                        )
                        .when(!collapsed, |d| {
                            d.child(
                                div()
                                    .p(px(tokens::space::XS))
                                    .children(landmarks.iter().map(|entry| {
                                        let descendants = entries
                                            .iter()
                                            .filter(|candidate| {
                                                candidate.path.starts_with(&entry.path)
                                            })
                                            .count()
                                            .saturating_sub(1);
                                        self.smart_file_row(entry, group.icon(), descendants)
                                            .into_any_element()
                                    })),
                            )
                        }),
                )
            }),
        )
    }

    fn file_tree(
        &self,
        entries: &[ProjectFileEntry],
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        let folders: Vec<_> = entries
            .iter()
            .filter(|entry| entry.is_dir)
            .map(|entry| entry.path.clone())
            .collect();
        let all_collapsed = !folders.is_empty()
            && folders
                .iter()
                .all(|path| self.collapsed_file_paths.contains(path));
        let visible: Vec<_> = entries
            .iter()
            .filter(|entry| !self.file_entry_hidden(entry))
            .collect();
        let folder_action = folders.clone();
        div()
            .rounded(px(tokens::radius::LG))
            .border_1()
            .border_color(hsla(t.border))
            .bg(hsla(t.bg_panel))
            .overflow_hidden()
            .child(
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
                    .child("FOLDERS & FILES")
                    .child(
                        div()
                            .id("file-tree-collapse-all")
                            .px(px(tokens::space::XS))
                            .py(px(2.0))
                            .rounded(px(tokens::radius::SM))
                            .hover(|style| style.bg(hsla(t.bg_hover)).text_color(hsla(t.text)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_file_tree_collapsed(
                                    folder_action.clone(),
                                    !all_collapsed,
                                    cx,
                                );
                            }))
                            .child(if all_collapsed {
                                "Expand all"
                            } else {
                                "Collapse all"
                            }),
                    ),
            )
            .child(
                div().p(px(tokens::space::XS)).children(
                    visible
                        .into_iter()
                        .map(|entry| self.file_tree_row(entry, cx)),
                ),
            )
    }

    fn file_entry_hidden(&self, entry: &ProjectFileEntry) -> bool {
        entry
            .path
            .ancestors()
            .skip(1)
            .any(|parent| self.collapsed_file_paths.contains(parent))
    }

    fn file_tree_row(
        &self,
        entry: &ProjectFileEntry,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        let collapsed = self.collapsed_file_paths.contains(&entry.path);
        let path = entry.path.clone();
        let folder_path = path.clone();
        let file_path = path.clone();
        let dirty = !entry.is_dir
            && (self.selected_project_file.as_ref() == Some(&entry.path)
                && self
                    .editor_buffer
                    .as_ref()
                    .is_some_and(tf_editor::Buffer::is_dirty)
                || self.local_project_root.as_ref().is_some_and(|root| {
                    self.editor_buffers
                        .get(&root.join(&entry.path))
                        .is_some_and(tf_editor::Buffer::is_dirty)
                }));
        let label = entry
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| entry.path.to_string_lossy().into_owned());
        div()
            .id(SharedString::from(format!(
                "file-tree-{}",
                entry.path.display()
            )))
            .h(px(26.0))
            .pl(px(tokens::space::SM + entry.depth as f32 * 14.0))
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
            .when(entry.is_dir, |d| {
                d.cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_file_path(folder_path.clone(), cx);
                    }))
            })
            .when(!entry.is_dir, |d| {
                d.cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_project_file(file_path.clone(), cx);
                    }))
            })
            .hover(|style| {
                style
                    .bg(with_alpha(hsla(t.text), 0.05))
                    .text_color(hsla(t.text))
            })
            .child(
                div()
                    .w(px(12.0))
                    .text_color(hsla(t.text_faint))
                    .child(if entry.is_dir {
                        if collapsed {
                            "▸"
                        } else {
                            "▾"
                        }
                    } else {
                        "·"
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(SharedString::from(label)),
            )
            .when(dirty, |d| {
                d.child(div().text_color(hsla(t.accent)).child("●"))
            })
    }

    fn smart_file_row(
        &self,
        entry: &ProjectFileEntry,
        icon: &'static str,
        descendants: usize,
    ) -> impl IntoElement {
        let t = &self.theme;
        let label = entry.path.to_string_lossy().into_owned();
        div()
            .h(px(26.0))
            .pl(px(tokens::space::SM))
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
            .when(entry.is_dir, |d| {
                d.child(
                    div()
                        .ml_auto()
                        .text_size(px(tokens::text::XS))
                        .text_color(hsla(t.text_faint))
                        .child(SharedString::from(format!("{descendants} items"))),
                )
            })
    }

    fn project_files(root: &Path, cancel: &std::sync::atomic::AtomicBool) -> Vec<ProjectFileEntry> {
        fn visit(
            root: &Path,
            dir: &Path,
            depth: usize,
            ignore: &ProjectIgnore,
            cancel: &std::sync::atomic::AtomicBool,
            output: &mut Vec<ProjectFileEntry>,
        ) {
            if depth > MAX_PROJECT_DEPTH || cancel.load(std::sync::atomic::Ordering::Relaxed) {
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
                if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                let path = entry.path();
                let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
                let relative = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
                if ignore.ignores(&relative, is_dir) {
                    continue;
                }
                output.push(ProjectFileEntry {
                    path: relative,
                    depth,
                    is_dir,
                });
                if is_dir {
                    visit(root, &path, depth + 1, ignore, cancel, output);
                }
            }
        }

        let mut output = Vec::new();
        visit(
            root,
            root,
            0,
            &ProjectIgnore::from_root(root),
            cancel,
            &mut output,
        );
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

    /// Build the actionable system nodes and topology edges for the 2D `GraphCanvas` viewport.
    pub(super) fn build_canvas_nodes(
        &self,
    ) -> (Vec<surface::CanvasNode>, Vec<surface::CanvasEdge>) {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();

        let daemon_status = crate::remote_session::daemon_status();
        let (daemon_subtitle, daemon_detail, daemon_healthy) = match &daemon_status {
            crate::remote_session::DaemonStatus::Connected => (
                format!("IPC connected · protocol v{}", tf_proto::PROTOCOL_VERSION),
                "Background session supervisor & SQLite store online".to_owned(),
                true,
            ),
            crate::remote_session::DaemonStatus::VersionMismatch { server, client } => (
                format!("Protocol mismatch · server v{server} != client v{client}"),
                "Running in non-blocking embedded PTY fallback mode".to_owned(),
                false,
            ),
            crate::remote_session::DaemonStatus::Offline(reason) => (
                "Embedded PTY fallback".to_owned(),
                if reason.is_empty() {
                    "forged daemon offline · local PTYs remain active".to_owned()
                } else {
                    format!("forged offline ({reason}) · local PTYs remain active")
                },
                false,
            ),
        };
        let daemon_id = "daemon:forged".to_owned();
        nodes.push(surface::CanvasNode {
            id: daemon_id.clone(),
            title: "forged daemon".into(),
            subtitle: daemon_subtitle,
            detail: daemon_detail,
            kind: surface::CanvasNodeKind::Daemon,
            world_x: 36.0,
            world_y: 44.0,
            healthy: daemon_healthy,
            session_pane: None,
            file_target: None,
        });

        // AI Agent CLI discovery nodes in column 0 below the daemon
        for (agent_idx, &(label, exe)) in surface::AGENT_CLIS.iter().enumerate() {
            let installed = surface::on_path(exe);
            let agent_id = format!("agent:{exe}");
            nodes.push(surface::CanvasNode {
                id: agent_id.clone(),
                title: format!("✦ {label}"),
                subtitle: if installed {
                    format!("{exe} · CLI ready")
                } else {
                    format!("{exe} · not on PATH")
                },
                detail: "Click or launch in Agent Fleet".into(),
                kind: surface::CanvasNodeKind::Agent,
                world_x: 36.0,
                world_y: 164.0 + (agent_idx as f32 * 108.0),
                healthy: installed,
                session_pane: None,
                file_target: None,
            });
        }

        let active_branch = self
            .tabs
            .get(self.active_tab)
            .and_then(|tab| tab.branch.clone())
            .or_else(|| self.discover_branch())
            .unwrap_or_else(|| "main".into());
        let changes_summary = match &self.changes {
            Some(changes) => format!(
                "{} staged · {} unstaged",
                changes.iter().filter(|c| c.staged).count(),
                changes.iter().filter(|c| c.unstaged).count()
            ),
            None => "Working tree ready".into(),
        };
        let primary_wt_id = format!("worktree:{active_branch}");
        nodes.push(surface::CanvasNode {
            id: primary_wt_id.clone(),
            title: format!("⎇ {active_branch}"),
            subtitle: self.project_name(),
            detail: changes_summary,
            kind: surface::CanvasNodeKind::Worktree,
            world_x: 280.0,
            world_y: 44.0,
            healthy: self.git_error.is_none(),
            session_pane: None,
            file_target: None,
        });
        edges.push(surface::CanvasEdge {
            from: daemon_id.clone(),
            to: primary_wt_id.clone(),
        });

        for (wt_idx, wt) in self.worktrees.iter().take(2).enumerate() {
            let branch_label = wt.branch.as_deref().unwrap_or("detached");
            if branch_label == active_branch {
                continue;
            }
            let wt_id = format!("worktree:{branch_label}");
            nodes.push(surface::CanvasNode {
                id: wt_id.clone(),
                title: format!("⎇ {branch_label}"),
                subtitle: wt.path.clone(),
                detail: "Linked Git worktree".into(),
                kind: surface::CanvasNodeKind::Worktree,
                world_x: 280.0,
                world_y: 164.0 + (wt_idx as f32 * 110.0),
                healthy: true,
                session_pane: None,
                file_target: None,
            });
            edges.push(surface::CanvasEdge {
                from: daemon_id.clone(),
                to: wt_id,
            });
        }

        if let Some(tab) = self.tabs.get(self.active_tab) {
            let focused_id = tab.layout.focused();
            let mut pane_ids = tab.layout.ids();
            for retained_id in tab.retained_panes.keys() {
                if !pane_ids.contains(retained_id) {
                    pane_ids.push(*retained_id);
                }
            }
            for (idx, pane_id) in pane_ids.into_iter().enumerate() {
                let (title, cwd, running) = if pane_id == focused_id {
                    let label = self
                        .title
                        .clone()
                        .or_else(|| tab.name.clone())
                        .unwrap_or_else(|| format!("{} #{pane_id}", self.spawn.profile.name));
                    let location = self.cwd.clone().unwrap_or_else(|| self.project_name());
                    (label, location, self.exited().is_none())
                } else if let Some(slot) = tab
                    .panes
                    .get(&pane_id)
                    .or_else(|| tab.retained_panes.get(&pane_id))
                {
                    let label = slot
                        .title
                        .clone()
                        .unwrap_or_else(|| format!("{} #{pane_id}", slot.spawn.profile.name));
                    let location = slot.cwd.clone().unwrap_or_else(|| self.project_name());
                    let alive = slot
                        .session
                        .as_ref()
                        .is_none_or(|s| s.exit_status().is_none());
                    (label, location, alive)
                } else {
                    (format!("Area #{pane_id}"), self.project_name(), true)
                };
                let node_id = format!("session:{pane_id}");
                nodes.push(surface::CanvasNode {
                    id: node_id.clone(),
                    title,
                    subtitle: format!("PTY Area #{pane_id}"),
                    detail: cwd,
                    kind: surface::CanvasNodeKind::Session,
                    world_x: 540.0,
                    world_y: 44.0 + (idx as f32 * 108.0),
                    healthy: running,
                    session_pane: Some(pane_id),
                    file_target: None,
                });
                edges.push(surface::CanvasEdge {
                    from: primary_wt_id.clone(),
                    to: node_id,
                });
            }
        }

        for (task_idx, task) in self
            .tasks
            .items
            .iter()
            .filter(|t| t.state != tf_proto::TaskState::Done)
            .take(3)
            .enumerate()
        {
            let task_id = format!("task:{}", task.id);
            let cmd = task
                .command
                .clone()
                .unwrap_or_else(|| "Interactive task target".into());
            nodes.push(surface::CanvasNode {
                id: task_id.clone(),
                title: task.title.clone(),
                subtitle: format!("{:?} target", task.state),
                detail: cmd,
                kind: surface::CanvasNodeKind::Task,
                world_x: 800.0,
                world_y: 44.0 + (task_idx as f32 * 108.0),
                healthy: task.state != tf_proto::TaskState::Blocked,
                session_pane: None,
                file_target: None,
            });
            edges.push(surface::CanvasEdge {
                from: primary_wt_id.clone(),
                to: task_id,
            });
        }

        // Execution targets: devcontainers, Dockerfiles, and configured SSH hosts.
        let mut target_idx = self
            .tasks
            .items
            .iter()
            .filter(|t| t.state != tf_proto::TaskState::Done)
            .take(3)
            .count();
        if let Some(root) = self
            .local_project_root
            .as_ref()
            .or(self.project_root.as_ref())
        {
            for candidate in [
                ".devcontainer/devcontainer.json",
                "Dockerfile",
                "compose.yaml",
                "docker-compose.yml",
            ] {
                if root.join(candidate).is_file() {
                    let container_id = format!("container:{candidate}");
                    nodes.push(surface::CanvasNode {
                        id: container_id.clone(),
                        title: candidate.into(),
                        subtitle: "Container Target".into(),
                        detail: root.join(candidate).display().to_string(),
                        kind: surface::CanvasNodeKind::Container,
                        world_x: 800.0,
                        world_y: 44.0 + (target_idx as f32 * 108.0),
                        healthy: true,
                        session_pane: None,
                        file_target: None,
                    });
                    edges.push(surface::CanvasEdge {
                        from: primary_wt_id.clone(),
                        to: container_id,
                    });
                    target_idx += 1;
                    break;
                }
            }
        }
        for host in self.ssh_hosts.iter().take(2) {
            let ssh_id = format!("container:ssh:{host}");
            nodes.push(surface::CanvasNode {
                id: ssh_id.clone(),
                title: format!("ssh://{host}"),
                subtitle: "Remote Execution Target".into(),
                detail: format!("SSH host {host}"),
                kind: surface::CanvasNodeKind::Container,
                world_x: 800.0,
                world_y: 44.0 + (target_idx as f32 * 108.0),
                healthy: self.spawn.ssh_host.as_deref() == Some(host.as_str()),
                session_pane: None,
                file_target: None,
            });
            edges.push(surface::CanvasEdge {
                from: daemon_id.clone(),
                to: ssh_id,
            });
            target_idx += 1;
        }

        (nodes, edges)
    }

    pub(super) fn build_canvas_nodes_for_lens(
        &self,
        lens: crate::panes::CanvasLens,
    ) -> (Vec<surface::CanvasNode>, Vec<surface::CanvasEdge>) {
        match lens {
            crate::panes::CanvasLens::SystemAgents => self.build_canvas_nodes(),
            crate::panes::CanvasLens::Architecture => {
                if let Some(root) = self
                    .local_project_root
                    .as_ref()
                    .or(self.project_root.as_ref())
                {
                    let (mut arch_nodes, arch_edges) =
                        surface::analyze_workspace_architecture(root);
                    if !arch_nodes.is_empty() {
                        let (runtime_nodes, _) = self.build_canvas_nodes();
                        for mut rnode in runtime_nodes {
                            if rnode.session_pane.is_some() {
                                rnode.world_x = 868.0;
                                arch_nodes.push(rnode);
                            }
                        }
                        return (arch_nodes, arch_edges);
                    }
                }
                self.build_canvas_nodes()
            }
            crate::panes::CanvasLens::FunctionFlow => {
                if let Some(buf) = &self.editor_buffer {
                    let (fn_nodes, fn_edges) =
                        surface::analyze_function_flow(buf.path(), buf.text());
                    if !fn_nodes.is_empty() {
                        return (fn_nodes, fn_edges);
                    }
                }
                if let Some(root) = self
                    .local_project_root
                    .as_ref()
                    .or(self.project_root.as_ref())
                {
                    let fallback_rel = PathBuf::from("bins/termforge/src/panes.rs");
                    let candidate = if root.join(&fallback_rel).is_file() {
                        Some(fallback_rel)
                    } else {
                        self.project_file_entries
                            .iter()
                            .find(|e| {
                                !e.is_dir
                                    && e.path.extension().and_then(|s| s.to_str()) == Some("rs")
                            })
                            .map(|e| e.path.clone())
                    };
                    if let Some(rel) = candidate {
                        if let Ok(source) = std::fs::read_to_string(root.join(&rel)) {
                            let (fn_nodes, fn_edges) =
                                surface::analyze_function_flow(&rel, &source);
                            if !fn_nodes.is_empty() {
                                return (fn_nodes, fn_edges);
                            }
                        }
                    }
                }
                self.build_canvas_nodes()
            }
        }
    }

    pub(super) fn set_canvas_lens(
        &mut self,
        area_id: Option<AreaId>,
        lens: crate::panes::CanvasLens,
        cx: &mut Context<'_, Self>,
    ) {
        self.canvas_state.lens = lens;
        self.canvas_state.reset_view();
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            for area in tab.layout.areas_mut() {
                if area_id.is_none() || area_id == Some(area.id) {
                    if let EditorKind::GraphCanvas(state) = &mut area.editor {
                        state.lens = lens;
                        state.reset_view();
                    }
                }
            }
        }
        cx.notify();
    }

    /// Select a node on the 2D `GraphCanvas` viewport:
    /// - Updates `selected_node` on the canvas state
    /// - Routes the node's properties into any visible `Inspector` area (`inspector_state`)
    /// - Routes its streaming I/O or PTY attach into any visible `Terminal` area
    /// - Or opens its source file + line in `CodeEditor` if it's an Architecture/Function node
    pub(super) fn select_canvas_node(&mut self, node_id: String, cx: &mut Context<'_, Self>) {
        let (lens_nodes, _) = self.build_canvas_nodes_for_lens(self.canvas_state.lens);
        let matched = lens_nodes
            .into_iter()
            .find(|n| n.id == node_id)
            .or_else(|| {
                let (fallback_nodes, _) = self.build_canvas_nodes();
                fallback_nodes.into_iter().find(|n| n.id == node_id)
            });
        self.canvas_state.selected_node = Some(node_id.clone());
        self.inspector_state.target = Some(node_id.clone());
        self.inspector_state.section = matched
            .as_ref()
            .map(|n| n.kind.badge().to_owned())
            .unwrap_or_else(|| "NODE".into());

        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            for area in tab.layout.areas_mut() {
                match &mut area.editor {
                    EditorKind::GraphCanvas(state) => {
                        state.selected_node = Some(node_id.clone());
                    }
                    EditorKind::Inspector(state) => {
                        state.target = Some(node_id.clone());
                        state.section = self.inspector_state.section.clone();
                    }
                    _ => {}
                }
            }
        }

        if let Some((rel_path, line)) = matched.as_ref().and_then(|n| n.file_target.clone()) {
            self.open_project_file(rel_path.clone(), cx);
            if let Some(line_no) = line {
                if let Some(root) = self
                    .local_project_root
                    .as_ref()
                    .or(self.project_root.as_ref())
                {
                    self.editor_pending_position = Some((
                        root.join(&rel_path),
                        tf_editor::TextPosition {
                            line: line_no.saturating_sub(1),
                            utf16_column: 0,
                        },
                    ));
                    self.apply_pending_editor_position();
                }
            }
        }

        if let Some(pane_id) = matched.and_then(|n| n.session_pane) {
            if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                let terminal_area = tab
                    .layout
                    .areas()
                    .into_iter()
                    .find(|area| matches!(area.editor, EditorKind::Terminal(_)))
                    .map(|area| area.id);
                if let Some(term_id) = terminal_area {
                    let _ = tab
                        .layout
                        .set_editor(term_id, EditorKind::Terminal(pane_id));
                }
            }
            self.focus_pane(pane_id, cx);
        }
        cx.notify();
    }

    pub(super) fn zoom_canvas(
        &mut self,
        area_id: Option<AreaId>,
        delta: f32,
        cx: &mut Context<'_, Self>,
    ) {
        self.canvas_state.zoom_by(delta);
        let next = self.canvas_state.clone();
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            for area in tab.layout.areas_mut() {
                if area_id.is_none() || area_id == Some(area.id) {
                    if let EditorKind::GraphCanvas(state) = &mut area.editor {
                        *state = next.clone();
                    }
                }
            }
        }
        cx.notify();
    }

    pub(super) fn pan_canvas(
        &mut self,
        area_id: Option<AreaId>,
        dx: f32,
        dy: f32,
        cx: &mut Context<'_, Self>,
    ) {
        self.canvas_state.pan_by(dx, dy);
        let next = self.canvas_state.clone();
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            for area in tab.layout.areas_mut() {
                if area_id.is_none() || area_id == Some(area.id) {
                    if let EditorKind::GraphCanvas(state) = &mut area.editor {
                        *state = next.clone();
                    }
                }
            }
        }
        cx.notify();
    }

    pub(super) fn reset_canvas_view(
        &mut self,
        area_id: Option<AreaId>,
        cx: &mut Context<'_, Self>,
    ) {
        self.canvas_state.reset_view();
        let next = self.canvas_state.clone();
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            for area in tab.layout.areas_mut() {
                if area_id.is_none() || area_id == Some(area.id) {
                    if let EditorKind::GraphCanvas(state) = &mut area.editor {
                        *state = next.clone();
                    }
                }
            }
        }
        cx.notify();
    }

    /// Render the interactive 2D pan/zoom `GraphCanvas` surface.
    pub(super) fn render_graph_canvas(
        &self,
        area_id: Option<AreaId>,
        state: &CanvasState,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        let active_lens = state.lens;
        let (nodes, edges) = self.build_canvas_nodes_for_lens(active_lens);
        let node_count = nodes.len();
        let edge_count = edges.len();
        let selected_id = state
            .selected_node
            .as_deref()
            .or(self.canvas_state.selected_node.as_deref());
        let zoom_pct = (state.zoom * 100.0).round() as i32;
        let card_w = (224.0 * state.zoom).clamp(164.0, 320.0);
        let port_y_offset = 42.0 * state.zoom;
        let node_positions: HashMap<String, (f32, f32)> = nodes
            .iter()
            .map(|n| (n.id.clone(), state.world_to_screen(n.world_x, n.world_y)))
            .collect();
        let (origin_x, origin_y) = state.world_to_screen(0.0, 0.0);

        div()
            .id(SharedString::from(match area_id {
                Some(id) => format!("graph-canvas-area-{id}"),
                None => "workbench-overview".into(),
            }))
            .flex_1()
            .size_full()
            .min_h(px(0.0))
            .relative()
            .overflow_hidden()
            .bg(hsla(t.bg_app))
            .on_scroll_wheel(cx.listener(move |this, ev: &ScrollWheelEvent, _, cx| {
                let delta = ev.delta.pixel_delta(px(20.0));
                if ev.modifiers.control || ev.modifiers.platform {
                    let zoom_step = if delta.y > px(0.0) { 0.1 } else { -0.1 };
                    this.zoom_canvas(area_id, zoom_step, cx);
                } else {
                    this.pan_canvas(area_id, f32::from(delta.x), f32::from(delta.y), cx);
                }
            }))
            .child(
                // Multi-scale 2D studio coordinate grid + origin crosshair axes
                div()
                    .absolute()
                    .top(px(0.0))
                    .left(px(0.0))
                    .right(px(0.0))
                    .bottom(px(0.0))
                    .child(
                        div()
                            .absolute()
                            .left(px(0.0))
                            .right(px(0.0))
                            .top(px(origin_y))
                            .h(px(1.0))
                            .bg(with_alpha(hsla(t.border_strong), 0.25)),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(0.0))
                            .bottom(px(0.0))
                            .left(px(origin_x))
                            .w(px(1.0))
                            .bg(with_alpha(hsla(t.border_strong), 0.25)),
                    )
                    .children((0..12).flat_map(|row| {
                        (0..16).map(move |col| {
                            let wx = (col as f32) * 96.0 + 16.0;
                            let wy = (row as f32) * 72.0 + 16.0;
                            let (sx, sy) = state.world_to_screen(wx, wy);
                            let major = row % 3 == 0 && col % 3 == 0;
                            div()
                                .absolute()
                                .left(px(sx))
                                .top(px(sy))
                                .size(px(if major { 3.5 } else { 2.0 }))
                                .rounded_full()
                                .bg(with_alpha(
                                    hsla(t.border_strong),
                                    if major { 0.45 } else { 0.22 },
                                ))
                        })
                    })),
            )
            .children(edges.into_iter().filter_map(|edge| {
                let &(x1, y1) = node_positions.get(&edge.from)?;
                let &(x2, y2) = node_positions.get(&edge.to)?;
                let start_x = x1 + card_w;
                let start_y = y1 + port_y_offset;
                let end_x = x2;
                let end_y = y2 + port_y_offset;
                let mid_x = (start_x + end_x) * 0.5;
                let h1_left = start_x.min(mid_x);
                let h1_w = (mid_x - start_x).abs().max(2.0);
                let v_top = start_y.min(end_y);
                let v_h = (end_y - start_y).abs().max(2.0);
                let h2_left = mid_x.min(end_x);
                let h2_w = (end_x - mid_x).abs().max(2.0);
                let wire_color = with_alpha(hsla(t.accent), 0.45);
                Some(
                    div()
                        .absolute()
                        .top(px(0.0))
                        .left(px(0.0))
                        .child(
                            div()
                                .absolute()
                                .left(px(h1_left))
                                .top(px(start_y))
                                .w(px(h1_w))
                                .h(px(2.0))
                                .bg(wire_color),
                        )
                        .child(
                            div()
                                .absolute()
                                .left(px(mid_x))
                                .top(px(v_top))
                                .w(px(2.0))
                                .h(px(v_h))
                                .bg(wire_color),
                        )
                        .child(
                            div()
                                .absolute()
                                .left(px(h2_left))
                                .top(px(end_y))
                                .w(px(h2_w))
                                .h(px(2.0))
                                .bg(wire_color),
                        ),
                )
            }))
            .children(nodes.into_iter().map(|node| {
                let (sx, sy) = state.world_to_screen(node.world_x, node.world_y);
                let is_selected = selected_id == Some(node.id.as_str());
                let badge_color = match node.kind {
                    surface::CanvasNodeKind::Daemon | surface::CanvasNodeKind::Agent => {
                        if node.healthy {
                            t.success
                        } else {
                            t.warning
                        }
                    }
                    surface::CanvasNodeKind::Worktree
                    | surface::CanvasNodeKind::Crate
                    | surface::CanvasNodeKind::Symbol => t.accent,
                    surface::CanvasNodeKind::Session | surface::CanvasNodeKind::Function => {
                        t.success
                    }
                    surface::CanvasNodeKind::Task | surface::CanvasNodeKind::Container => {
                        if node.healthy {
                            t.accent
                        } else {
                            t.warning
                        }
                    }
                };
                let status_pill = if is_selected {
                    "SELECTED"
                } else if node.file_target.is_some() {
                    "OPEN CODE"
                } else if node.healthy {
                    "READY"
                } else {
                    "ATTENTION"
                };
                let node_id = node.id.clone();
                div()
                    .id(SharedString::from(format!("canvas-node-{}", node.id)))
                    .absolute()
                    .left(px(sx))
                    .top(px(sy))
                    .w(px(card_w))
                    .rounded(px(tokens::radius::MD))
                    .border_1()
                    .border_color(hsla(if is_selected {
                        t.accent
                    } else {
                        t.border_strong
                    }))
                    .bg(hsla(if is_selected {
                        t.bg_selected
                    } else {
                        t.bg_panel
                    }))
                    .shadow_md()
                    .cursor_pointer()
                    .hover(|style| style.bg(hsla(t.bg_hover)).border_color(hsla(t.accent)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_canvas_node(node_id.clone(), cx);
                    }))
                    // Left input socket port
                    .child(
                        div()
                            .absolute()
                            .left(px(-4.0))
                            .top(px(port_y_offset - 3.0))
                            .size(px(7.0))
                            .rounded_full()
                            .border_1()
                            .border_color(hsla(t.bg_app))
                            .bg(hsla(badge_color)),
                    )
                    // Right output socket port
                    .child(
                        div()
                            .absolute()
                            .right(px(-4.0))
                            .top(px(port_y_offset - 3.0))
                            .size(px(7.0))
                            .rounded_full()
                            .border_1()
                            .border_color(hsla(t.bg_app))
                            .bg(hsla(badge_color)),
                    )
                    // Node header bar
                    .child(
                        div()
                            .h(px(24.0))
                            .px(px(tokens::space::SM))
                            .flex()
                            .items_center()
                            .justify_between()
                            .border_b_1()
                            .border_color(hsla(t.border))
                            .bg(with_alpha(hsla(badge_color), 0.12))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(5.0))
                                    .child(div().size(px(6.0)).rounded_full().bg(hsla(badge_color)))
                                    .child(
                                        div()
                                            .text_size(px(10.0))
                                            .text_color(hsla(badge_color))
                                            .child(node.kind.badge()),
                                    ),
                            )
                            .child(
                                div()
                                    .px(px(4.0))
                                    .py(px(1.0))
                                    .rounded(px(3.0))
                                    .bg(hsla(t.bg_app))
                                    .text_size(px(9.0))
                                    .text_color(hsla(if is_selected {
                                        t.accent
                                    } else {
                                        t.text_faint
                                    }))
                                    .child(status_pill),
                            ),
                    )
                    // Node body & telemetry footer
                    .child(
                        div()
                            .p(px(tokens::space::SM))
                            .flex()
                            .flex_col()
                            .gap(px(3.0))
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(px(tokens::text::SM))
                                    .text_color(hsla(t.text))
                                    .child(SharedString::from(node.title)),
                            )
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(px(tokens::text::XS))
                                    .text_color(hsla(t.text_muted))
                                    .child(SharedString::from(node.subtitle)),
                            )
                            .child(
                                div()
                                    .mt(px(3.0))
                                    .px(px(6.0))
                                    .py(px(3.0))
                                    .rounded(px(tokens::radius::SM))
                                    .bg(hsla(t.bg_app))
                                    .border_1()
                                    .border_color(hsla(t.border))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(px(10.0))
                                    .text_color(hsla(t.text_faint))
                                    .child(SharedString::from(node.detail)),
                            ),
                    )
            }))
            .child(
                // Top-left Canvas Lens Switcher (Architecture | Function Flow | Agents & Runtime)
                div()
                    .absolute()
                    .top(px(tokens::space::SM))
                    .left(px(tokens::space::SM))
                    .px(px(4.0))
                    .h(px(28.0))
                    .flex()
                    .items_center()
                    .gap(px(3.0))
                    .rounded(px(tokens::radius::MD))
                    .border_1()
                    .border_color(hsla(t.border))
                    .bg(hsla(t.bg_elevated))
                    .text_size(px(tokens::text::XS))
                    .children(crate::panes::CanvasLens::ALL.iter().copied().map(|lens| {
                        let active = lens == active_lens;
                        div()
                            .id(SharedString::from(format!("canvas-lens-{}", lens.id())))
                            .px(px(7.0))
                            .h(px(20.0))
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .rounded(px(tokens::radius::SM))
                            .bg(if active {
                                with_alpha(hsla(t.accent), 0.16)
                            } else {
                                hsla(t.bg_elevated)
                            })
                            .text_color(hsla(if active { t.accent } else { t.text_muted }))
                            .cursor_pointer()
                            .hover(|s| s.bg(hsla(t.bg_hover)).text_color(hsla(t.text)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_canvas_lens(area_id, lens, cx);
                            }))
                            .child(div().text_size(px(10.0)).child(lens.icon()))
                            .child(lens.label())
                    }))
                    .child(div().px(px(3.0)).text_color(hsla(t.text_faint)).child("·"))
                    .child(div().pr(px(4.0)).text_color(hsla(t.text_faint)).child(
                        SharedString::from(format!("{node_count} nodes · {edge_count} links")),
                    )),
            )
            .child(
                // Viewport HUD toolbar (top-right)
                div()
                    .absolute()
                    .top(px(tokens::space::SM))
                    .right(px(tokens::space::SM))
                    .px(px(tokens::space::SM))
                    .h(px(26.0))
                    .flex()
                    .items_center()
                    .gap(px(tokens::space::XS))
                    .rounded(px(tokens::radius::MD))
                    .border_1()
                    .border_color(hsla(t.border))
                    .bg(hsla(t.bg_elevated))
                    .text_size(px(tokens::text::XS))
                    .text_color(hsla(t.text_muted))
                    .child(
                        div()
                            .id("canvas-pan-left")
                            .px(px(4.0))
                            .cursor_pointer()
                            .hover(|s| s.text_color(hsla(t.text)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.pan_canvas(area_id, -40.0, 0.0, cx);
                            }))
                            .child("←"),
                    )
                    .child(
                        div()
                            .id("canvas-pan-up")
                            .px(px(4.0))
                            .cursor_pointer()
                            .hover(|s| s.text_color(hsla(t.text)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.pan_canvas(area_id, 0.0, -30.0, cx);
                            }))
                            .child("↑"),
                    )
                    .child(
                        div()
                            .id("canvas-pan-down")
                            .px(px(4.0))
                            .cursor_pointer()
                            .hover(|s| s.text_color(hsla(t.text)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.pan_canvas(area_id, 0.0, 30.0, cx);
                            }))
                            .child("↓"),
                    )
                    .child(
                        div()
                            .id("canvas-pan-right")
                            .px(px(4.0))
                            .cursor_pointer()
                            .hover(|s| s.text_color(hsla(t.text)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.pan_canvas(area_id, 40.0, 0.0, cx);
                            }))
                            .child("→"),
                    )
                    .child(div().text_color(hsla(t.text_faint)).child("·"))
                    .child(
                        div()
                            .id("canvas-zoom-out")
                            .px(px(4.0))
                            .cursor_pointer()
                            .hover(|s| s.text_color(hsla(t.text)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.zoom_canvas(area_id, -0.15, cx);
                            }))
                            .child("−"),
                    )
                    .child(
                        div()
                            .id("canvas-zoom-reset")
                            .px(px(5.0))
                            .py(px(1.0))
                            .rounded(px(3.0))
                            .bg(hsla(t.bg_app))
                            .cursor_pointer()
                            .hover(|s| s.text_color(hsla(t.text)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.reset_canvas_view(area_id, cx);
                            }))
                            .child(SharedString::from(format!("{zoom_pct}%"))),
                    )
                    .child(
                        div()
                            .id("canvas-zoom-in")
                            .px(px(4.0))
                            .cursor_pointer()
                            .hover(|s| s.text_color(hsla(t.text)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.zoom_canvas(area_id, 0.15, cx);
                            }))
                            .child("+"),
                    )
                    .child(div().text_color(hsla(t.text_faint)).child("·"))
                    .child(
                        div()
                            .id("artifact-primary-action")
                            .px(px(6.0))
                            .py(px(1.0))
                            .rounded(px(tokens::radius::SM))
                            .bg(with_alpha(hsla(t.accent), 0.15))
                            .text_color(hsla(t.accent))
                            .cursor_pointer()
                            .hover(|s| s.bg(hsla(t.bg_hover)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.new_local_tab(cx);
                            }))
                            .child("+ Terminal"),
                    ),
            )
            .child(
                // Bottom-left Topology Legend & Camera Coordinates HUD
                div()
                    .absolute()
                    .bottom(px(tokens::space::SM))
                    .left(px(tokens::space::SM))
                    .px(px(tokens::space::SM))
                    .h(px(24.0))
                    .flex()
                    .items_center()
                    .gap(px(tokens::space::SM))
                    .rounded(px(tokens::radius::MD))
                    .border_1()
                    .border_color(hsla(t.border))
                    .bg(hsla(t.bg_elevated))
                    .text_size(px(10.0))
                    .text_color(hsla(t.text_faint))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .child(div().size(px(5.0)).rounded_full().bg(hsla(t.success)))
                            .child("Agents / Functions / PTY"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .child(div().size(px(5.0)).rounded_full().bg(hsla(t.accent)))
                            .child("Crates / Types / Git"),
                    )
                    .child(div().child("·"))
                    .child(SharedString::from(format!(
                        "X:{:.0} Y:{:.0} · {zoom_pct}%",
                        state.pan_x, state.pan_y
                    ))),
            )
    }

    /// Render a non-terminal [`EditorKind`] mounted inside a polymorphic split-tree leaf.
    pub(super) fn render_non_terminal_area(
        &self,
        id: AreaId,
        editor: &EditorKind,
        cx: &mut Context<'_, Self>,
    ) -> gpui::AnyElement {
        match editor {
            EditorKind::GraphCanvas(state) => self
                .render_graph_canvas(Some(id), state, cx)
                .into_any_element(),
            EditorKind::Inspector(state) => {
                self.render_area_inspector(id, state, cx).into_any_element()
            }
            EditorKind::Outliner(state) => {
                self.render_area_outliner(id, state, cx).into_any_element()
            }
            EditorKind::Timeline(_) => self.render_area_timeline(id, cx).into_any_element(),
            EditorKind::CodeEditor => self.workbench_files(cx).into_any_element(),
            EditorKind::GitDiff => self.workbench_changes(cx).into_any_element(),
            EditorKind::TaskBoard => self.workbench_tasks(cx).into_any_element(),
            EditorKind::Services => self.render_area_services(id, cx).into_any_element(),
            EditorKind::Environment => self.render_area_environment(id, cx).into_any_element(),
            EditorKind::Terminal(_) => div().size_full().into_any_element(),
        }
    }

    fn render_area_inspector(
        &self,
        id: AreaId,
        state: &InspectorState,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        let target_id = state
            .target
            .as_deref()
            .or(self.inspector_state.target.as_deref());
        let (lens_nodes, _) = self.build_canvas_nodes_for_lens(self.canvas_state.lens);
        let selected_node = target_id.and_then(|tid| {
            lens_nodes.into_iter().find(|n| n.id == tid).or_else(|| {
                let (fallback_nodes, _) = self.build_canvas_nodes();
                fallback_nodes.into_iter().find(|n| n.id == tid)
            })
        });
        let (section_title, badge_text, facts): (&str, String, Vec<(&'static str, String)>) =
            if let Some(node) = selected_node {
                (
                    "NODE PROPERTIES",
                    node.kind.badge().to_owned(),
                    vec![
                        ("ID", node.id),
                        ("TYPE", node.kind.badge().to_owned()),
                        ("TITLE", node.title),
                        ("CONTEXT", node.subtitle),
                        ("TELEMETRY", node.detail),
                        (
                            "COORDS",
                            format!("X {:.0} · Y {:.0}", node.world_x, node.world_y),
                        ),
                    ],
                )
            } else {
                let (open, active, blocked) = self.task_summary();
                (
                    "WORKSPACE PROPERTIES",
                    self.workspace.label().to_owned(),
                    vec![
                        ("PROJECT", self.project_name()),
                        (
                            "BRANCH",
                            self.discover_branch().unwrap_or_else(|| "main".into()),
                        ),
                        ("LAYOUT", self.workspace.label().to_owned()),
                        (
                            "TASKS",
                            format!("{open} open · {active} active · {blocked} blocked"),
                        ),
                        (
                            "DAEMON",
                            crate::remote_session::daemon_status()
                                .status_bar_label()
                                .unwrap_or_else(|| "Connected (v10)".into()),
                        ),
                    ],
                )
            };
        div()
            .id(("area-inspector", id))
            .size_full()
            .overflow_y_scroll()
            .p(px(tokens::space::SM))
            .bg(hsla(t.bg_panel))
            .flex()
            .flex_col()
            .gap(px(tokens::space::SM))
            .child(
                div()
                    .rounded(px(tokens::radius::MD))
                    .border_1()
                    .border_color(hsla(t.border))
                    .bg(hsla(t.bg_app))
                    .overflow_hidden()
                    .child(
                        div()
                            .h(px(26.0))
                            .px(px(tokens::space::SM))
                            .flex()
                            .items_center()
                            .justify_between()
                            .border_b_1()
                            .border_color(hsla(t.border))
                            .bg(hsla(t.bg_elevated))
                            .text_size(px(10.0))
                            .child(
                                div()
                                    .text_color(hsla(t.text_muted))
                                    .child(SharedString::from(section_title.to_owned())),
                            )
                            .child(
                                div()
                                    .px(px(5.0))
                                    .py(px(1.0))
                                    .rounded(px(3.0))
                                    .bg(with_alpha(hsla(t.accent), 0.16))
                                    .text_color(hsla(t.accent))
                                    .child(SharedString::from(badge_text)),
                            ),
                    )
                    .child(
                        div().flex().flex_col().children(
                            facts
                                .into_iter()
                                .map(|(label, value)| self.inspector_fact(label, &value)),
                        ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(self.inspector_action(
                        "area-inspector-new-terminal",
                        "+  Spawn Terminal Session",
                        cx.listener(|this, _, _, cx| this.new_local_tab(cx)),
                    ))
                    .child(self.inspector_action(
                        "area-inspector-new-task",
                        "○  Create Project Task",
                        cx.listener(|this, _, _, cx| this.open_task_form("", "", cx)),
                    ))
                    .child(self.inspector_action(
                        "area-inspector-palette",
                        "⌘  Open Command Palette",
                        cx.listener(|this, _, _, cx| this.open_palette(cx)),
                    )),
            )
    }

    fn render_area_outliner(
        &self,
        id: AreaId,
        _state: &ProjectTreeState,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        let entries = &self.project_file_entries;
        div()
            .id(("area-outliner", id))
            .size_full()
            .overflow_y_scroll()
            .p(px(tokens::space::SM))
            .bg(hsla(t.bg_panel))
            .child(self.file_tree(entries, cx))
    }

    pub(super) fn workbench_tasks(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let t = &self.theme;
        let visible_indices = self.filtered_task_indices();
        let task_groups: Vec<_> = [
            (tf_proto::TaskState::Active, "IN PROGRESS"),
            (tf_proto::TaskState::Blocked, "BLOCKED"),
            (tf_proto::TaskState::Planned, "PLANNED"),
            (tf_proto::TaskState::Done, "COMPLETED"),
        ]
        .into_iter()
        .filter_map(|(state, label)| {
            let rows: Vec<_> = self
                .filtered_task_indices()
                .iter()
                .copied()
                .filter(|index| self.tasks.items[*index].state == state)
                .map(|index| {
                    self.task_row(index, &self.tasks.items[index], cx)
                        .into_any_element()
                })
                .collect();
            (!rows.is_empty()).then_some((label, rows))
        })
        .collect();
        let suggestions: Vec<_> = self
            .current_suggestions()
            .into_iter()
            .enumerate()
            .map(|(index, (title, command, reason))| {
                self.suggestion_row(index, title, command, reason, cx)
                    .into_any_element()
            })
            .collect();
        let suggestion_count = suggestions.len();
        let suggestion_count_label = suggestion_count.to_string();
        let selected_task = self
            .tasks
            .selected
            .and_then(|id| self.tasks.items.iter().find(|task| task.id == id))
            .cloned();
        let showing_detail = selected_task.is_some();
        div()
            .id("workbench-tasks")
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .bg(hsla(t.bg_app))
            .child(
                div()
                    .px(px(tokens::space::LG))
                    .py(px(tokens::space::MD))
                    .border_b_1()
                    .border_color(hsla(t.border))
                    .bg(hsla(t.bg_panel))
                    .child(self.artifact_heading(
                        "Project tasks",
                        "Plan durable work and turn repeated project commands into one-click tasks",
                        cx,
                    ))
                    .child(
                        div()
                            .mt(px(tokens::space::SM))
                            .flex()
                            .items_center()
                            .gap(px(tokens::space::SM))
                            .child(
                                div()
                                    .id("tasks-filter")
                                    .px(px(tokens::space::SM))
                                    .py(px(4.0))
                                    .rounded(px(tokens::radius::MD))
                                    .border_1()
                                    .border_color(hsla(t.border))
                                    .text_size(px(tokens::text::XS))
                                    .text_color(hsla(t.text_muted))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(hsla(t.bg_hover)))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.tasks.filter = this.tasks.filter.next();
                                        this.persist_layout();
                                        cx.notify();
                                    }))
                                    .child(SharedString::from(format!(
                                        "Filter: {}",
                                        self.tasks.filter.label()
                                    ))),
                            )
                            .child(
                                div()
                                    .id("tasks-sort")
                                    .px(px(tokens::space::SM))
                                    .py(px(4.0))
                                    .rounded(px(tokens::radius::MD))
                                    .border_1()
                                    .border_color(hsla(t.border))
                                    .text_size(px(tokens::text::XS))
                                    .text_color(hsla(t.text_muted))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(hsla(t.bg_hover)))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.tasks.sort = this.tasks.sort.next();
                                        this.persist_layout();
                                        cx.notify();
                                    }))
                                    .child(SharedString::from(format!(
                                        "Sort: {}",
                                        self.tasks.sort.label()
                                    ))),
                            )
                            .child(
                                div()
                                    .text_size(px(tokens::text::XS))
                                    .text_color(hsla(t.text_faint))
                                    .child(SharedString::from(format!(
                                        "{} shown",
                                        visible_indices.len()
                                    ))),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .child(
                        div()
                            .id("workbench-task-board")
                            .flex_1()
                            .min_w(px(0.0))
                            .h_full()
                            .overflow_y_scroll()
                            .p(px(tokens::space::LG))
                            .child(
                                div()
                                    .rounded(px(tokens::radius::LG))
                                    .border_1()
                                    .border_color(hsla(t.border))
                                    .bg(hsla(t.bg_panel))
                                    .overflow_hidden()
                                    .child(self.workbench_card_header(
                                        "YOUR TASKS",
                                        &self.tasks.items.len().to_string(),
                                    ))
                                    .when(self.tasks.loading, |d| {
                                        d.child(
                                            div()
                                                .p(px(tokens::space::LG))
                                                .flex()
                                                .items_center()
                                                .gap(px(tokens::space::SM))
                                                .text_size(px(tokens::text::SM))
                                                .text_color(hsla(t.text_faint))
                                                .child("◌")
                                                .child("Loading project tasks…"),
                                        )
                                    })
                                    .when_some(self.tasks.error.clone(), |d, error| {
                                        d.child(
                                            div()
                                                .p(px(tokens::space::LG))
                                                .flex()
                                                .items_start()
                                                .justify_between()
                                                .gap(px(tokens::space::MD))
                                                .bg(with_alpha(hsla(t.danger), 0.07))
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .text_size(px(tokens::text::SM))
                                                        .text_color(hsla(t.danger))
                                                        .child(SharedString::from(error)),
                                                )
                                                .child(
                                                    div()
                                                        .id("tasks-retry")
                                                        .px(px(tokens::space::SM))
                                                        .py(px(4.0))
                                                        .rounded(px(tokens::radius::SM))
                                                        .border_1()
                                                        .border_color(hsla(t.border))
                                                        .text_size(px(tokens::text::XS))
                                                        .text_color(hsla(t.text_muted))
                                                        .cursor_pointer()
                                                        .hover(|style| style.bg(hsla(t.bg_hover)))
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            this.refresh_tasks(cx);
                                                        }))
                                                        .child("Retry"),
                                                ),
                                        )
                                    })
                                    .when(
                                        self.tasks.items.is_empty()
                                            && !self.tasks.loading
                                            && self.tasks.error.is_none(),
                                        |d| {
                                        d.child(
                                            div()
                                                .p(px(tokens::space::LG))
                                                .text_size(px(tokens::text::SM))
                                                .text_color(hsla(t.text_faint))
                                                .child("No saved tasks. Choose a suggestion or create one from scratch."),
                                        )
                                    },
                                    )
                                    .child(
                                        div()
                                            .p(px(tokens::space::XS))
                                            .children(task_groups.into_iter().map(|(label, rows)| {
                                                div()
                                                    .pb(px(tokens::space::SM))
                                                    .child(
                                                        div()
                                                            .h(px(28.0))
                                                            .px(px(tokens::space::SM))
                                                            .flex()
                                                            .items_center()
                                                            .text_size(px(tokens::text::XS))
                                                            .text_color(hsla(t.text_faint))
                                                            .child(label),
                                                    )
                                                    .children(rows)
                                            })),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .w(px(340.0))
                            .min_w(px(300.0))
                            .h_full()
                            .flex_none()
                            .flex()
                            .flex_col()
                            .border_l_1()
                            .border_color(hsla(t.border))
                            .bg(hsla(t.bg_panel))
                            .child(self.workbench_card_header(
                                if showing_detail {
                                    "TASK DETAILS"
                                } else {
                                    "SUGGESTED FOR THIS PROJECT"
                                },
                                if showing_detail {
                                    ""
                                } else {
                                    &suggestion_count_label
                                },
                            ))
                            .when_some(selected_task, |d, task| {
                                let task_id = task.id;
                                let selected_dependencies = task.dependencies.clone();
                                let dependency_options: Vec<_> = self
                                    .tasks
                                    .items
                                    .iter()
                                    .filter(|candidate| candidate.id != task_id)
                                    .map(|candidate| {
                                        let dependency_id = candidate.id;
                                        let checked =
                                            selected_dependencies.contains(&dependency_id);
                                        div()
                                            .id(SharedString::from(format!(
                                                "detail-dependency-{dependency_id}"
                                            )))
                                            .px(px(tokens::space::SM))
                                            .py(px(4.0))
                                            .flex()
                                            .items_center()
                                            .gap(px(tokens::space::SM))
                                            .rounded(px(tokens::radius::SM))
                                            .text_size(px(tokens::text::SM))
                                            .text_color(hsla(t.text_muted))
                                            .cursor_pointer()
                                            .hover(|style| style.bg(hsla(t.bg_hover)))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.toggle_existing_task_dependency(
                                                    task_id,
                                                    dependency_id,
                                                    cx,
                                                );
                                            }))
                                            .child(if checked { "☑" } else { "☐" })
                                            .child(SharedString::from(candidate.title.clone()))
                                    })
                                    .collect();
                                d.child(
                                    div()
                                        .p(px(tokens::space::MD))
                                        .flex()
                                        .flex_col()
                                        .gap(px(tokens::space::MD))
                                        .child(
                                            div()
                                                .flex()
                                                .items_start()
                                                .justify_between()
                                                .gap(px(tokens::space::SM))
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .text_size(px(tokens::text::LG))
                                                        .text_color(hsla(t.text))
                                                        .child(SharedString::from(task.title)),
                                                )
                                                .child(
                                                    div()
                                                        .id("task-detail-close")
                                                        .size(px(24.0))
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .rounded(px(tokens::radius::SM))
                                                        .text_color(hsla(t.text_faint))
                                                        .cursor_pointer()
                                                        .hover(|style| style.bg(hsla(t.bg_hover)))
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            this.tasks.selected = None;
                                                            cx.notify();
                                                        }))
                                                        .child("×"),
                                                ),
                                        )
                                        .child(self.task_detail_fact(
                                            "STATE",
                                            match task.state {
                                                tf_proto::TaskState::Planned => "Planned",
                                                tf_proto::TaskState::Active => "In progress",
                                                tf_proto::TaskState::Blocked => "Blocked",
                                                tf_proto::TaskState::Done => "Completed",
                                            },
                                        ))
                                        .child(self.task_detail_fact(
                                            "PRIORITY",
                                            match task.priority {
                                                tf_proto::TaskPriority::Low => "Low",
                                                tf_proto::TaskPriority::Normal => "Normal",
                                                tf_proto::TaskPriority::High => "High",
                                                tf_proto::TaskPriority::Urgent => "Urgent",
                                            },
                                        ))
                                        .when(!task.context.is_empty(), |d| {
                                            d.child(self.task_detail_fact("CONTEXT", &task.context))
                                        })
                                        .when_some(task.command, |d, command| {
                                            d.child(self.task_detail_fact("COMMAND", &command))
                                        })
                                        .when(!dependency_options.is_empty(), |d| {
                                            d.child(
                                                div()
                                                    .flex()
                                                    .flex_col()
                                                    .gap(px(3.0))
                                                    .child(
                                                        div()
                                                            .text_size(px(tokens::text::XS))
                                                            .text_color(hsla(t.text_faint))
                                                            .child("PREREQUISITES"),
                                                    )
                                                    .children(dependency_options),
                                            )
                                        }),
                                )
                            })
                            .when(!showing_detail, |d| {
                                d.child(
                                    div()
                                        .px(px(tokens::space::MD))
                                        .py(px(tokens::space::SM))
                                        .text_size(px(tokens::text::XS))
                                        .text_color(hsla(t.text_faint))
                                        .child("Detected from project manifests and commands you run repeatedly."),
                                )
                                .when(suggestions.is_empty(), |d| {
                                    d.child(
                                        div()
                                            .p(px(tokens::space::MD))
                                            .text_size(px(tokens::text::SM))
                                            .text_color(hsla(t.text_faint))
                                            .child("No new suggestions right now."),
                                    )
                                })
                                .child(
                                    div()
                                        .id("workbench-task-suggestions")
                                        .flex_1()
                                        .min_h(px(0.0))
                                        .overflow_y_scroll()
                                        .p(px(tokens::space::XS))
                                        .children(suggestions),
                                )
                            })
                    ),
            )
    }

    fn task_detail_fact(&self, label: &'static str, value: &str) -> impl IntoElement {
        let t = &self.theme;
        div()
            .flex()
            .flex_col()
            .gap(px(3.0))
            .child(
                div()
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

    fn artifact_heading(
        &self,
        title: &str,
        subtitle: &str,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        let tasks = self.workspace == Workspace::Work;
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

    pub(super) fn git_dialog_overlay(
        &self,
        cx: &mut Context<'_, Self>,
    ) -> Option<impl IntoElement> {
        let dialog = self.git_dialog.as_ref()?;
        let t = &self.theme;
        let (title, detail, action, destructive, can_submit) = match dialog {
            GitDialog::Discard(change) => (
                "Discard local changes?",
                format!(
                    "This permanently removes the working-tree changes in {}.",
                    change.path
                ),
                "Discard changes",
                true,
                true,
            ),
            GitDialog::Commit(message) => (
                "Commit staged changes",
                "Only files in Staged Changes will be committed.".to_owned(),
                "Commit",
                false,
                surface::validate_commit_message(message).is_ok(),
            ),
        };
        Some(
            div()
                .id("git-dialog-scrim")
                .absolute()
                .top(px(0.0))
                .bottom(px(0.0))
                .left(px(0.0))
                .right(px(0.0))
                .flex()
                .items_start()
                .justify_center()
                .bg(gpui::black().opacity(0.45))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.git_dialog = None;
                    cx.notify();
                }))
                .child(
                    div()
                        .id("git-dialog")
                        .mt(px(72.0))
                        .w(px(520.0))
                        .max_w_full()
                        .p(px(tokens::space::LG))
                        .flex()
                        .flex_col()
                        .gap(px(tokens::space::MD))
                        .rounded(px(tokens::radius::XL))
                        .bg(hsla(t.bg_elevated))
                        .border_1()
                        .border_color(hsla(t.border_strong))
                        .shadow_lg()
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .text_size(px(tokens::text::LG))
                                .text_color(hsla(if destructive { t.danger } else { t.text }))
                                .child(title),
                        )
                        .child(
                            div()
                                .text_size(px(tokens::text::SM))
                                .text_color(hsla(t.text_muted))
                                .child(SharedString::from(detail)),
                        )
                        .children(match dialog {
                            GitDialog::Commit(message) => Some(
                                div()
                                    .h(px(38.0))
                                    .px(px(tokens::space::MD))
                                    .flex()
                                    .items_center()
                                    .rounded(px(tokens::radius::MD))
                                    .bg(hsla(t.bg_app))
                                    .border_1()
                                    .border_color(hsla(t.accent))
                                    .text_color(hsla(if message.is_empty() {
                                        t.text_faint
                                    } else {
                                        t.text
                                    }))
                                    .child(SharedString::from(if message.is_empty() {
                                        "Commit message…▏".to_owned()
                                    } else {
                                        format!("{message}▏")
                                    })),
                            ),
                            GitDialog::Discard(_) => None,
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .text_size(px(tokens::text::XS))
                                .text_color(hsla(t.text_faint))
                                .child("Enter confirm   Esc cancel")
                                .child(
                                    div()
                                        .id("git-dialog-submit")
                                        .h(px(30.0))
                                        .px(px(tokens::space::LG))
                                        .flex()
                                        .items_center()
                                        .rounded(px(tokens::radius::MD))
                                        .bg(hsla(if can_submit {
                                            if destructive {
                                                t.danger
                                            } else {
                                                t.accent
                                            }
                                        } else {
                                            t.bg_hover
                                        }))
                                        .text_color(hsla(if can_submit {
                                            t.accent_text
                                        } else {
                                            t.text_faint
                                        }))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if can_submit {
                                                this.submit_git_dialog(cx);
                                            }
                                        }))
                                        .child(action),
                                ),
                        ),
                ),
        )
    }

    fn inspector_fact(&self, label: &'static str, value: &str) -> impl IntoElement {
        let t = &self.theme;
        div()
            .px(px(tokens::space::SM))
            .py(px(6.0))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(tokens::space::SM))
            .border_b_1()
            .border_color(hsla(t.border))
            .child(
                div()
                    .w(px(68.0))
                    .flex_none()
                    .text_size(px(10.0))
                    .text_color(hsla(t.text_faint))
                    .child(label),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(tokens::text::XS))
                    .text_color(hsla(t.text))
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
            .h(px(28.0))
            .px(px(tokens::space::SM))
            .flex()
            .items_center()
            .rounded(px(tokens::radius::SM))
            .border_1()
            .border_color(hsla(t.border))
            .bg(hsla(t.bg_app))
            .text_size(px(tokens::text::XS))
            .text_color(hsla(t.text_muted))
            .cursor_pointer()
            .hover(|style| style.bg(hsla(t.bg_hover)).text_color(hsla(t.text)))
            .on_click(on_click)
            .child(label)
    }
}

fn launch_external_editor(path: &Path) -> std::io::Result<()> {
    if let Some(editor) = std::env::var_os("VISUAL").or_else(|| std::env::var_os("EDITOR")) {
        let editor = editor.to_string_lossy();
        let mut parts = editor.split_whitespace();
        if let Some(program) = parts.next() {
            std::process::Command::new(program)
                .args(parts)
                .arg(path)
                .spawn()?;
            return Ok(());
        }
    }

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", ""])
            .arg(path)
            .spawn()?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(path).spawn()?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(path).spawn()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ProjectFileGroup, ProjectIgnore, ProjectIgnoreRule, TerminalView};
    use std::path::Path;

    #[test]
    fn editor_segments_show_selection_across_syntax_and_lines() {
        let text = "let α = 1\nnext";
        let spans = tf_editor::highlight(text, tf_editor::Language::Rust);
        let start = text.find('α').unwrap();
        let end = text.find("next").unwrap() + 2;
        let lines = super::highlighted_editor_lines(text, &spans, start..end);
        let selected: Vec<String> = lines
            .iter()
            .map(|line| {
                line.iter()
                    .filter(|(_, _, selected)| *selected)
                    .map(|(text, _, _)| text.as_str())
                    .collect()
            })
            .collect();
        assert_eq!(selected, vec!["α = 1", "ne"]);
    }

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

    #[test]
    fn project_ignore_hides_generated_directories_and_honors_common_gitignore_rules() {
        let ignore = ProjectIgnore {
            rules: vec![
                ProjectIgnoreRule {
                    pattern: "generated".into(),
                    directory_only: true,
                    anchored: false,
                    negated: false,
                },
                ProjectIgnoreRule {
                    pattern: "*.log".into(),
                    directory_only: false,
                    anchored: false,
                    negated: false,
                },
                ProjectIgnoreRule {
                    pattern: "keep.log".into(),
                    directory_only: false,
                    anchored: false,
                    negated: true,
                },
            ],
        };

        assert!(ignore.ignores(Path::new("web/node_modules"), true));
        assert!(ignore.ignores(Path::new("generated"), true));
        assert!(ignore.ignores(Path::new("logs/build.log"), false));
        assert!(!ignore.ignores(Path::new("logs/keep.log"), false));
        assert!(!ignore.ignores(Path::new("src/main.rs"), false));
    }

    #[test]
    fn project_ignore_supports_anchored_and_recursive_patterns() {
        let ignore = ProjectIgnore {
            rules: vec![
                ProjectIgnoreRule {
                    pattern: "private".into(),
                    directory_only: true,
                    anchored: true,
                    negated: false,
                },
                ProjectIgnoreRule {
                    pattern: "**/snapshots/*.tmp".into(),
                    directory_only: false,
                    anchored: false,
                    negated: false,
                },
            ],
        };

        assert!(ignore.ignores(Path::new("private"), true));
        assert!(!ignore.ignores(Path::new("src/private"), true));
        assert!(ignore.ignores(Path::new("src/snapshots/a.tmp"), false));
        assert!(!ignore.ignores(Path::new("src/snapshots/a.rs"), false));
    }
}
