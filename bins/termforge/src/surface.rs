//! Working-surface model: what a region of the workspace is showing.
//!
//! Plain data with no GPUI or session types, so it can be tested without a
//! display. A surface is a *view* of an existing resource; opening one never
//! creates a process.

use std::path::Path;
use std::process::Command;

/// Tabs offered by the bottom utility dock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DockTab {
    /// Running terminal sessions of this window.
    #[default]
    Sessions,
    /// Working-tree changes of the active session's repository.
    Changes,
}

/// What the optional right panel inspects.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Inspector {
    /// Facts about the active session: directory, branch, program status.
    #[default]
    Session,
    /// The diff of one changed file.
    Diff(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

impl ChangeKind {
    pub fn letter(self) -> &'static str {
        match self {
            Self::Modified => "M",
            Self::Added => "A",
            Self::Deleted => "D",
            Self::Renamed => "R",
            Self::Untracked => "U",
            Self::Conflicted => "!",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitChange {
    pub kind: ChangeKind,
    pub staged: bool,
    pub path: String,
}

/// Parse `git status --porcelain=v1 -z` output.
pub fn parse_porcelain(raw: &str) -> Vec<GitChange> {
    let mut out = Vec::new();
    let mut entries = raw.split('\0').filter(|e| !e.is_empty());
    while let Some(entry) = entries.next() {
        let mut chars = entry.chars();
        let (Some(x), Some(y)) = (chars.next(), chars.next()) else {
            continue;
        };
        let path = entry.get(3..).unwrap_or_default().to_owned();
        if x == 'R' || x == 'C' {
            // The original path follows as its own NUL-terminated entry.
            entries.next();
        }
        let kind = match (x, y) {
            ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D') => ChangeKind::Conflicted,
            ('?', '?') => ChangeKind::Untracked,
            ('R' | 'C', _) | (_, 'R' | 'C') => ChangeKind::Renamed,
            ('A', _) | (_, 'A') => ChangeKind::Added,
            ('D', _) | (_, 'D') => ChangeKind::Deleted,
            _ => ChangeKind::Modified,
        };
        out.push(GitChange {
            kind,
            staged: x != ' ' && x != '?',
            path,
        });
    }
    out
}

/// Changed files in the repository containing `dir`. `None` when `dir` is not
/// in a repository or `git` is unavailable, which is distinct from "clean".
pub fn git_changes(dir: &Path) -> Option<Vec<GitChange>> {
    let out = Command::new("git")
        .args(["status", "--porcelain=v1", "-z"])
        .current_dir(dir)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| parse_porcelain(&String::from_utf8_lossy(&out.stdout)))
}

/// Unified diff of one path against HEAD (working tree and index combined).
pub fn git_diff(dir: &Path, path: &str, untracked: bool) -> Option<String> {
    let mut cmd = Command::new("git");
    if untracked {
        cmd.args([
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-index",
            "--",
            "/dev/null",
            path,
        ]);
    } else {
        cmd.args(["diff", "--no-color", "--no-ext-diff", "HEAD", "--", path]);
    }
    let out = cmd.current_dir(dir).output().ok()?;
    // `--no-index` exits 1 when files differ.
    (out.status.success() || untracked).then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// One checked-out worktree of a repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: String,
    /// Branch name, or `None` when detached or bare.
    pub branch: Option<String>,
}

/// Parse `git worktree list --porcelain` output.
pub fn parse_worktrees(raw: &str) -> Vec<Worktree> {
    let mut out: Vec<Worktree> = Vec::new();
    for line in raw.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            out.push(Worktree {
                path: path.to_owned(),
                branch: None,
            });
        } else if let (Some(branch), Some(last)) = (line.strip_prefix("branch "), out.last_mut()) {
            last.branch = Some(
                branch
                    .strip_prefix("refs/heads/")
                    .unwrap_or(branch)
                    .to_owned(),
            );
        }
    }
    out
}

/// Worktrees of the repository containing `dir`; empty when not in one.
pub fn git_worktrees(dir: &Path) -> Vec<Worktree> {
    Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(dir)
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| parse_worktrees(&String::from_utf8_lossy(&out.stdout)))
        .unwrap_or_default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLine {
    Header,
    Hunk,
    Added,
    Removed,
    Context,
}

pub fn classify_diff_line(line: &str) -> DiffLine {
    if line.starts_with("+++")
        || line.starts_with("---")
        || line.starts_with("diff ")
        || line.starts_with("index ")
        || line.starts_with("new file")
        || line.starts_with("deleted file")
    {
        DiffLine::Header
    } else if line.starts_with("@@") {
        DiffLine::Hunk
    } else if line.starts_with('+') {
        DiffLine::Added
    } else if line.starts_with('-') {
        DiffLine::Removed
    } else {
        DiffLine::Context
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_parses_kinds_and_staging() {
        let raw = " M src/a.rs\0A  src/b.rs\0?? new.txt\0R  new.rs\0old.rs\0UU merge.rs\0";
        let changes = parse_porcelain(raw);
        assert_eq!(changes.len(), 5);
        assert_eq!(changes[0].kind, ChangeKind::Modified);
        assert!(!changes[0].staged);
        assert_eq!(changes[1].kind, ChangeKind::Added);
        assert!(changes[1].staged);
        assert_eq!(changes[2].kind, ChangeKind::Untracked);
        assert!(!changes[2].staged);
        assert_eq!(changes[3].kind, ChangeKind::Renamed);
        assert_eq!(changes[3].path, "new.rs");
        assert_eq!(changes[4].kind, ChangeKind::Conflicted);
    }

    #[test]
    fn worktrees_parse_branch_and_detached() {
        let raw =
            "worktree /a\nHEAD 111\nbranch refs/heads/main\n\nworktree /b\nHEAD 222\ndetached\n";
        let wt = parse_worktrees(raw);
        assert_eq!(wt.len(), 2);
        assert_eq!(wt[0].branch.as_deref(), Some("main"));
        assert_eq!(wt[1].branch, None);
        assert_eq!(wt[1].path, "/b");
    }

    #[test]
    fn diff_lines_are_classified() {
        assert_eq!(classify_diff_line("+++ b/x"), DiffLine::Header);
        assert_eq!(classify_diff_line("@@ -1 +1 @@"), DiffLine::Hunk);
        assert_eq!(classify_diff_line("+x"), DiffLine::Added);
        assert_eq!(classify_diff_line("-x"), DiffLine::Removed);
        assert_eq!(classify_diff_line(" x"), DiffLine::Context);
    }
}
