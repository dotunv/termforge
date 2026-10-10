//! Workspace metadata that is not terminal state: git branch discovery and
//! the layout document persisted by `forged` between UI launches.

use std::path::{Path, PathBuf};

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

/// Order, names and pins of the workspaces in a window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Layout {
    pub tabs: Vec<SavedTab>,
    pub active: Option<String>,
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
        };
        assert_eq!(Layout::decode(&layout.encode()), layout);
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
}
