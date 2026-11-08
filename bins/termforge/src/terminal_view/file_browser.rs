//! Explorer navigation state, filtering and keyboard input.
use super::workbench::ProjectFileEntry;
use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum BrowserFocus {
    Search,
    Tree,
}

#[derive(Default)]
pub(super) struct FileBrowserState {
    pub query: String,
    pub focus: Option<BrowserFocus>,
    pub selected: Option<PathBuf>,
    pub initialized: bool,
    pub scroll: gpui::ScrollHandle,
}

pub(super) fn visible_indices(
    entries: &[ProjectFileEntry],
    collapsed: &std::collections::HashSet<PathBuf>,
    query: &str,
) -> Vec<usize> {
    let query = query.trim().to_lowercase().replace('\\', "/");
    let mut visible: Vec<_> = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            if query.is_empty() {
                !entry
                    .path
                    .ancestors()
                    .skip(1)
                    .any(|parent| collapsed.contains(parent))
            } else {
                let path = entry
                    .path
                    .to_string_lossy()
                    .to_lowercase()
                    .replace('\\', "/");
                !entry.is_dir && query.split_whitespace().all(|part| path.contains(part))
            }
        })
        .map(|(index, _)| index)
        .collect();
    if !query.is_empty() {
        visible.sort_by_key(|&index| {
            let name = entries[index]
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase();
            if name == query {
                0
            } else if name.starts_with(&query) {
                1
            } else {
                2
            }
        });
    }
    if !query.is_empty() {
        visible.truncate(200);
    }
    visible
}

impl TerminalView {
    pub(super) fn reveal_file_path(&mut self, path: &Path) {
        for parent in path.ancestors().skip(1) {
            self.collapsed_file_paths.remove(parent);
        }
        self.file_browser.selected = Some(path.to_path_buf());
    }

    pub(super) fn reveal_active_file(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(path) = self.selected_project_file.clone() {
            self.file_browser.query.clear();
            self.file_explorer_mode = FileExplorerMode::Tree;
            self.reveal_file_path(&path);
            self.scroll_file_browser();
            cx.notify();
        }
    }

    fn browser_entries(&self) -> Vec<usize> {
        visible_indices(
            &self.project_file_entries,
            &self.collapsed_file_paths,
            &self.file_browser.query,
        )
    }

    pub(super) fn scroll_file_browser(&self) {
        let visible = self.browser_entries();
        if let Some(row) = visible.iter().position(|&index| {
            Some(&self.project_file_entries[index].path) == self.file_browser.selected.as_ref()
        }) {
            self.file_browser.scroll.scroll_to_item(row);
        }
    }

    fn browser_move(&mut self, delta: isize) {
        let visible = self.browser_entries();
        if visible.is_empty() {
            self.file_browser.selected = None;
            return;
        }
        let current = visible.iter().position(|&index| {
            Some(&self.project_file_entries[index].path) == self.file_browser.selected.as_ref()
        });
        let next = current.map_or(0, |index| {
            (index as isize + delta).clamp(0, visible.len() as isize - 1) as usize
        });
        self.file_browser.selected = Some(self.project_file_entries[visible[next]].path.clone());
        self.scroll_file_browser();
    }

    pub(super) fn file_browser_key(
        &mut self,
        ev: &KeyDownEvent,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let key = ev.keystroke.key.as_str();
        let m = ev.keystroke.modifiers;
        let can_browse = !self.workspace.hosts_sessions()
            && self
                .workspace_layouts
                .get(&self.workspace)
                .is_some_and(|tree| {
                    tree.areas().iter().any(|area| {
                        matches!(
                            area.editor.tag(),
                            EditorKindTag::CodeEditor | EditorKindTag::Outliner
                        )
                    })
                });
        if (m.control || m.platform)
            && !m.shift
            && !m.alt
            && key.eq_ignore_ascii_case("p")
            && can_browse
        {
            let outliner = self
                .workspace_layouts
                .get(&self.workspace)
                .and_then(|tree| {
                    tree.areas()
                        .into_iter()
                        .find(|area| area.editor.tag() == EditorKindTag::Outliner)
                        .map(|area| area.id)
                });
            if let Some(id) = outliner {
                self.maximized_area = None;
                self.focus_area(id, cx);
            }
            self.file_explorer_mode = FileExplorerMode::Tree;
            self.file_browser.query.clear();
            self.file_browser.focus = Some(BrowserFocus::Search);
            cx.notify();
            return true;
        }
        let Some(focus) = self.file_browser.focus.filter(|_| can_browse) else {
            return false;
        };
        match key {
            "escape" => {
                self.file_browser.query.clear();
                self.file_browser.focus = None;
                self.reveal_active_file(cx);
            }
            "down" | "up" => {
                if focus == BrowserFocus::Search {
                    self.browser_move(0);
                } else {
                    self.browser_move(if key == "down" { 1 } else { -1 });
                }
                self.file_browser.focus = Some(BrowserFocus::Tree);
            }
            "home" | "end" => {
                let visible = self.browser_entries();
                let index = if key == "home" {
                    visible.first()
                } else {
                    visible.last()
                };
                self.file_browser.selected =
                    index.map(|&index| self.project_file_entries[index].path.clone());
            }
            "enter" => {
                if self.file_browser.selected.is_none() {
                    self.browser_move(0);
                }
                if let Some(path) = self.file_browser.selected.clone() {
                    if self
                        .project_file_entries
                        .iter()
                        .any(|entry| entry.path == path && entry.is_dir)
                    {
                        self.toggle_file_path(path, cx);
                    } else {
                        self.file_browser.focus = None;
                        self.select_project_file(path, cx);
                    }
                }
            }
            "right" | "left"
                if focus == BrowserFocus::Tree && self.file_browser.query.is_empty() =>
            {
                if let Some(path) = self.file_browser.selected.clone() {
                    let dir = self
                        .project_file_entries
                        .iter()
                        .any(|entry| entry.path == path && entry.is_dir);
                    if key == "right" {
                        if dir && self.collapsed_file_paths.remove(&path) {
                        } else if dir {
                            self.browser_move(1);
                        }
                    } else if dir && !self.collapsed_file_paths.contains(&path) {
                        self.collapsed_file_paths.insert(path);
                    } else if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty())
                    {
                        self.file_browser.selected = Some(parent.to_path_buf());
                    }
                }
            }
            "backspace" => {
                if m.control || m.platform {
                    self.file_browser.query.clear();
                } else {
                    self.file_browser.query.pop();
                }
                self.file_browser.selected = None;
                self.file_browser.focus = Some(BrowserFocus::Search);
            }
            "v" if m.control || m.platform => {
                if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    self.file_browser.query.extend(
                        text.chars()
                            .filter(|ch| !ch.is_control())
                            .take(256usize.saturating_sub(self.file_browser.query.chars().count())),
                    );
                }
                self.file_browser.selected = None;
            }
            _ if !m.control && !m.platform && !m.alt => {
                if let Some(text) = &ev.keystroke.key_char {
                    self.file_browser.query.extend(
                        text.chars()
                            .filter(|ch| !ch.is_control())
                            .take(256usize.saturating_sub(self.file_browser.query.chars().count())),
                    );
                    self.file_browser.selected = None;
                    self.file_browser.focus = Some(BrowserFocus::Search);
                }
            }
            _ => {
                self.file_browser.focus = None;
                return false;
            }
        }
        self.scroll_file_browser();
        cx.notify();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, is_dir: bool) -> ProjectFileEntry {
        ProjectFileEntry {
            path: path.into(),
            depth: path.matches('/').count(),
            is_dir,
        }
    }

    #[test]
    fn search_reaches_collapsed_descendants_and_ranks_names_before_paths() {
        let entries = vec![
            entry("src", true),
            entry("src/lib.rs.bak", false),
            entry("src/lib.rs", false),
            entry("docs/lib.rs/notes.md", false),
        ];
        let collapsed = [PathBuf::from("src")].into_iter().collect();
        assert_eq!(visible_indices(&entries, &collapsed, ""), vec![0, 3]);
        assert_eq!(
            visible_indices(&entries, &collapsed, "LIB.RS"),
            vec![2, 1, 3]
        );
        assert_eq!(
            visible_indices(&entries, &collapsed, "SRC\\LIB"),
            vec![1, 2]
        );
        assert_eq!(
            visible_indices(&entries, &collapsed, "src lib.rs"),
            vec![1, 2]
        );
        assert!(visible_indices(&entries, &collapsed, "missing").is_empty());
    }

    #[test]
    fn broad_queries_bound_rendered_results() {
        let entries: Vec<_> = (0..250)
            .map(|index| entry(&format!("src/file{index}.rs"), false))
            .collect();
        assert_eq!(
            visible_indices(&entries, &Default::default(), "file").len(),
            200
        );
        assert_eq!(
            visible_indices(&entries, &Default::default(), "").len(),
            250
        );
    }
}
