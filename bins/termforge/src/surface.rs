//! Working-surface model: what a region of the workspace is showing.
//!
//! Plain data with no GPUI or session types, so it can be tested without a
//! display. A surface is a *view* of an existing resource; opening one never
//! creates a process.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Tabs offered by the bottom utility dock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DockTab {
    /// Running terminal sessions of this window.
    #[default]
    Sessions,
    /// Working-tree changes of the active session's repository.
    Changes,
    /// Live output of another workspace's session (read-only view).
    Output,
    /// Processes and agents TermForge launched, with observed state.
    Processes,
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
    /// The index differs from HEAD.
    pub staged: bool,
    /// The working tree differs from the index. A path can be both staged and
    /// unstaged and should then be represented in both review groups.
    pub unstaged: bool,
    pub path: String,
    /// Source path of a rename or copy, when reported by Git.
    pub original_path: Option<String>,
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
        let original_path = if matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C') {
            entries.next().map(str::to_owned)
        } else {
            None
        };
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
            unstaged: y != ' ' || (x == '?' && y == '?'),
            path,
            original_path,
        });
    }
    out
}

fn git_repository_root(dir: &Path) -> Result<PathBuf, String> {
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(dir)
        .output()
        .map_err(|error| format!("could not start git: {error}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    let root = String::from_utf8(out.stdout)
        .map_err(|_| "repository path is not valid UTF-8".to_owned())?;
    let root = root.strip_suffix('\n').unwrap_or(&root);
    let root = root.strip_suffix('\r').unwrap_or(root);
    if root.is_empty() {
        return Err("Git did not return a repository root".into());
    }
    Ok(PathBuf::from(root))
}

fn run_git_path_command(dir: &Path, args: &[&str], path: &str) -> Result<(), String> {
    run_git_paths_command(dir, args, &[path])
}

fn run_git_paths_command(dir: &Path, args: &[&str], paths: &[&str]) -> Result<(), String> {
    let root = git_repository_root(dir)?;
    let out = Command::new("git")
        .arg("--literal-pathspecs")
        .args(args)
        .arg("--")
        .args(paths)
        .current_dir(root)
        .output()
        .map_err(|error| format!("could not start git: {error}"))?;
    if out.status.success() {
        Ok(())
    } else {
        let detail = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        Err(if detail.is_empty() {
            format!("git exited with {}", out.status)
        } else {
            detail
        })
    }
}

/// Add exactly one literal repository-relative path to the index.
pub fn git_stage(dir: &Path, path: &str) -> Result<(), String> {
    run_git_path_command(dir, &["add"], path)
}

/// Restore exactly one path in the index from HEAD while preserving its
/// working-tree contents.
pub fn git_unstage(dir: &Path, path: &str) -> Result<(), String> {
    run_git_path_command(dir, &["reset", "-q"], path)
}

pub fn git_unstage_change(dir: &Path, change: &GitChange) -> Result<(), String> {
    if let Some(original) = change.original_path.as_deref() {
        run_git_paths_command(dir, &["reset", "-q"], &[original, &change.path])
    } else {
        git_unstage(dir, &change.path)
    }
}

/// Discard the working-tree version of one path. Untracked files use Git's
/// path-scoped clean operation; callers must obtain explicit confirmation.
pub fn git_discard(dir: &Path, path: &str, untracked: bool) -> Result<(), String> {
    if untracked {
        run_git_path_command(dir, &["clean", "-f"], path)
    } else {
        run_git_path_command(dir, &["restore"], path)
    }
}

pub fn validate_commit_message(message: &str) -> Result<&str, &'static str> {
    let message = message.trim();
    if message.is_empty() {
        Err("Commit message cannot be empty")
    } else if message.chars().any(|character| character.is_control()) {
        Err("Commit message must be one printable line")
    } else if message.chars().count() > 200 {
        Err("Commit message must be 200 characters or fewer")
    } else {
        Ok(message)
    }
}

/// Commit the current index without involving a shell or interpolating the
/// message into command syntax.
pub fn git_commit(dir: &Path, message: &str) -> Result<(), String> {
    let message = validate_commit_message(message).map_err(str::to_owned)?;
    let out = Command::new("git")
        .args(["commit", "-m", message])
        .current_dir(dir)
        .output()
        .map_err(|error| format!("could not start git: {error}"))?;
    if out.status.success() {
        Ok(())
    } else {
        let detail = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        Err(if detail.is_empty() {
            format!("git exited with {}", out.status)
        } else {
            detail
        })
    }
}

/// Changed files in the repository containing `dir`. `None` when `dir` is not
/// in a repository or `git` is unavailable, which is distinct from "clean".
pub fn git_changes(dir: &Path) -> Option<Vec<GitChange>> {
    let root = git_repository_root(dir).ok()?;
    let out = Command::new("git")
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .current_dir(root)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(parse_porcelain(&String::from_utf8(out.stdout).ok()?))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DiffScope {
    Staged,
    #[default]
    Unstaged,
}

/// Unified diff of one path for one review scope.
pub fn git_diff(
    dir: &Path,
    path: &str,
    original_path: Option<&str>,
    scope: DiffScope,
    untracked: bool,
) -> Option<String> {
    let root = git_repository_root(dir).ok()?;
    let mut cmd = Command::new("git");
    cmd.arg("--literal-pathspecs");
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
    } else if scope == DiffScope::Staged {
        cmd.args(["diff", "--no-color", "--no-ext-diff", "--cached", "--"]);
        if let Some(original) = original_path {
            cmd.arg(original);
        }
        cmd.arg(path);
    } else {
        cmd.args(["diff", "--no-color", "--no-ext-diff", "--", path]);
    }
    let out = cmd.current_dir(root).output().ok()?;
    // `--no-index` exits 1 when files differ.
    (out.status.success() || (untracked && out.status.code() == Some(1)))
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A command TermForge launched into a workspace of its own. TermForge only
/// claims control of what it started: it types the command into that shell
/// and observes the shell's own command marks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Managed {
    pub command: String,
    /// A coding-agent CLI rather than a development service.
    pub agent: bool,
}

/// The last command a shell reported through OSC 133 marks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LastCommand {
    Running,
    Finished(Option<i32>),
}

/// What can actually be observed about a managed process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    /// The shell has not reported a command yet (still starting, or the
    /// shell has no integration, in which case nothing more is knowable).
    Pending,
    Running,
    Succeeded,
    Failed(i32),
    /// The shell session itself ended.
    SessionEnded(Option<u32>),
}

pub fn process_state(ended: Option<Option<u32>>, last: Option<LastCommand>) -> ProcessState {
    if let Some(code) = ended {
        return ProcessState::SessionEnded(code);
    }
    match last {
        None => ProcessState::Pending,
        Some(LastCommand::Running) => ProcessState::Running,
        Some(LastCommand::Finished(Some(code))) if code != 0 => ProcessState::Failed(code),
        Some(LastCommand::Finished(_)) => ProcessState::Succeeded,
    }
}

impl ProcessState {
    pub fn label(self) -> String {
        match self {
            Self::Pending => "waiting for shell".into(),
            Self::Running => "running".into(),
            Self::Succeeded => "finished".into(),
            Self::Failed(code) => format!("failed ({code})"),
            Self::SessionEnded(Some(code)) => format!("session ended ({code})"),
            Self::SessionEnded(None) => "session ended".into(),
        }
    }
}

/// Coding-agent CLIs offered when found on `PATH`: (label, executable).
pub const AGENT_CLIS: &[(&str, &str)] = &[
    ("Claude Code", "claude"),
    ("Codex", "codex"),
    ("Gemini CLI", "gemini"),
];

/// Whether `name` resolves to a file in one of the `PATH` directories.
pub fn on_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    let exts: &[&str] = if cfg!(windows) {
        &["", ".exe", ".cmd", ".bat"]
    } else {
        &[""]
    };
    std::env::split_paths(&path).any(|dir| {
        exts.iter()
            .any(|ext| dir.join(format!("{name}{ext}")).is_file())
    })
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRow {
    pub kind: DiffLine,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub text: String,
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

fn parse_hunk_start(line: &str) -> Option<(u32, u32)> {
    let body = line.strip_prefix("@@ -")?;
    let (old, rest) = body.split_once(" +")?;
    let (new, _) = rest.split_once(" @@")?;
    let old = old.split(',').next()?.parse().ok()?;
    let new = new.split(',').next()?.parse().ok()?;
    Some((old, new))
}

/// Parse unified diff output into display rows with source-accurate old and
/// new line numbers. Metadata and hunk headers intentionally have no numbers.
pub fn parse_diff(raw: &str) -> Vec<DiffRow> {
    let mut old_line = None;
    let mut new_line = None;
    let mut in_hunk = false;
    raw.lines()
        .map(|text| {
            if text.starts_with("diff --git ") {
                in_hunk = false;
                old_line = None;
                new_line = None;
            }
            let kind = if text.starts_with("@@ ") {
                DiffLine::Hunk
            } else if in_hunk {
                match text.as_bytes().first() {
                    Some(b'+') => DiffLine::Added,
                    Some(b'-') => DiffLine::Removed,
                    Some(b' ') => DiffLine::Context,
                    _ => DiffLine::Header,
                }
            } else {
                classify_diff_line(text)
            };
            if kind == DiffLine::Hunk {
                if let Some((old, new)) = parse_hunk_start(text) {
                    old_line = Some(old);
                    new_line = Some(new);
                    in_hunk = true;
                }
                return DiffRow {
                    kind,
                    old_line: None,
                    new_line: None,
                    text: text.to_owned(),
                };
            }
            let (row_old, row_new) = match kind {
                DiffLine::Removed => {
                    let row = (old_line, None);
                    old_line = old_line.map(|line| line + 1);
                    row
                }
                DiffLine::Added => {
                    let row = (None, new_line);
                    new_line = new_line.map(|line| line + 1);
                    row
                }
                DiffLine::Context if old_line.is_some() || new_line.is_some() => {
                    let row = (old_line, new_line);
                    old_line = old_line.map(|line| line + 1);
                    new_line = new_line.map(|line| line + 1);
                    row
                }
                _ => (None, None),
            };
            DiffRow {
                kind,
                old_line: row_old,
                new_line: row_new,
                text: text.to_owned(),
            }
        })
        .collect()
}

/// Pair each consecutive block of removals and additions without moving lines
/// across context or hunk boundaries. Unequal blocks leave an empty cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SplitDiffRow<'a> {
    pub left: Option<&'a DiffRow>,
    pub right: Option<&'a DiffRow>,
}

pub fn split_diff_rows(rows: &[DiffRow]) -> Vec<SplitDiffRow<'_>> {
    let mut result = Vec::with_capacity(rows.len());
    let mut index = 0;
    while index < rows.len() {
        if matches!(rows[index].kind, DiffLine::Removed | DiffLine::Added) {
            let start = index;
            while index < rows.len()
                && matches!(rows[index].kind, DiffLine::Removed | DiffLine::Added)
            {
                index += 1;
            }
            let block = &rows[start..index];
            let removed: Vec<_> = block
                .iter()
                .filter(|row| row.kind == DiffLine::Removed)
                .collect();
            let added: Vec<_> = block
                .iter()
                .filter(|row| row.kind == DiffLine::Added)
                .collect();
            for offset in 0..removed.len().max(added.len()) {
                result.push(SplitDiffRow {
                    left: removed.get(offset).copied(),
                    right: added.get(offset).copied(),
                });
            }
        } else {
            result.push(SplitDiffRow {
                left: Some(&rows[index]),
                right: Some(&rows[index]),
            });
            index += 1;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_parses_kinds_and_staging() {
        let raw =
            " M src/a.rs\0A  src/b.rs\0?? new.txt\0R  new.rs\0old.rs\0UU merge.rs\0MM both.rs\0";
        let changes = parse_porcelain(raw);
        assert_eq!(changes.len(), 6);
        assert_eq!(changes[0].kind, ChangeKind::Modified);
        assert!(!changes[0].staged);
        assert!(changes[0].unstaged);
        assert_eq!(changes[1].kind, ChangeKind::Added);
        assert!(changes[1].staged);
        assert!(!changes[1].unstaged);
        assert_eq!(changes[2].kind, ChangeKind::Untracked);
        assert!(!changes[2].staged);
        assert!(changes[2].unstaged);
        assert_eq!(changes[3].kind, ChangeKind::Renamed);
        assert_eq!(changes[3].path, "new.rs");
        assert_eq!(changes[3].original_path.as_deref(), Some("old.rs"));
        assert_eq!(changes[4].kind, ChangeKind::Conflicted);
        assert!(changes[5].staged);
        assert!(changes[5].unstaged);
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
    fn process_state_reports_only_what_was_observed() {
        use LastCommand::*;
        assert_eq!(process_state(None, None), ProcessState::Pending);
        assert_eq!(process_state(None, Some(Running)), ProcessState::Running);
        assert_eq!(
            process_state(None, Some(Finished(Some(0)))),
            ProcessState::Succeeded
        );
        assert_eq!(
            process_state(None, Some(Finished(None))),
            ProcessState::Succeeded
        );
        assert_eq!(
            process_state(None, Some(Finished(Some(2)))),
            ProcessState::Failed(2)
        );
        // A dead session wins over any earlier command mark.
        assert_eq!(
            process_state(Some(Some(1)), Some(Running)),
            ProcessState::SessionEnded(Some(1))
        );
    }

    #[test]
    fn missing_executable_is_not_on_path() {
        assert!(!on_path("definitely-not-a-real-binary-xyz"));
    }

    #[test]
    fn diff_lines_are_classified() {
        assert_eq!(classify_diff_line("+++ b/x"), DiffLine::Header);
        assert_eq!(classify_diff_line("@@ -1 +1 @@"), DiffLine::Hunk);
        assert_eq!(classify_diff_line("+x"), DiffLine::Added);
        assert_eq!(classify_diff_line("-x"), DiffLine::Removed);
        assert_eq!(classify_diff_line(" x"), DiffLine::Context);
    }

    #[test]
    fn diff_rows_track_old_and_new_line_numbers() {
        let rows = parse_diff("@@ -10,3 +20,4 @@\n same\n-old\n+new\n+extra\n tail");
        assert_eq!((rows[1].old_line, rows[1].new_line), (Some(10), Some(20)));
        assert_eq!((rows[2].old_line, rows[2].new_line), (Some(11), None));
        assert_eq!((rows[3].old_line, rows[3].new_line), (None, Some(21)));
        assert_eq!((rows[4].old_line, rows[4].new_line), (None, Some(22)));
        assert_eq!((rows[5].old_line, rows[5].new_line), (Some(12), Some(23)));
    }

    #[test]
    fn diff_hunks_distinguish_content_from_headers_and_no_newline_markers() {
        let rows = parse_diff("diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n---old\n+++new\n\\ No newline at end of file\n context");
        assert_eq!(rows[4].kind, DiffLine::Removed);
        assert_eq!(rows[4].old_line, Some(1));
        assert_eq!(rows[5].kind, DiffLine::Added);
        assert_eq!(rows[5].new_line, Some(1));
        assert_eq!((rows[6].old_line, rows[6].new_line), (None, None));
        assert_eq!((rows[7].old_line, rows[7].new_line), (Some(2), Some(2)));
    }

    #[test]
    fn split_diff_aligns_replacements_and_keeps_hunk_boundaries() {
        let rows = parse_diff("@@ -1,3 +1,4 @@\n same\n-old one\n-old two\n+new one\n+new two\n+new three\n tail\n@@ -9 +10 @@\n-removed\n+added");
        let split = split_diff_rows(&rows);
        assert_eq!(split.len(), 8);
        assert_eq!(split[2].left.unwrap().text, "-old one");
        assert_eq!(split[2].right.unwrap().text, "+new one");
        assert_eq!(
            (
                split[3].left.unwrap().old_line,
                split[3].right.unwrap().new_line
            ),
            (Some(3), Some(3))
        );
        assert!(split[4].left.is_none());
        assert_eq!(split[4].right.unwrap().text, "+new three");
        assert_eq!(split[5].left.unwrap().kind, DiffLine::Context);
        assert_eq!(split[6].left.unwrap().kind, DiffLine::Hunk);
        assert_eq!(split[7].left.unwrap().text, "-removed");
        assert_eq!(split[7].right.unwrap().text, "+added");
    }

    #[test]
    fn commit_messages_are_single_bounded_printable_lines() {
        assert_eq!(validate_commit_message("  ship it  "), Ok("ship it"));
        assert!(validate_commit_message("").is_err());
        assert!(validate_commit_message("first\nsecond").is_err());
        assert!(validate_commit_message(&"x".repeat(201)).is_err());
    }

    #[test]
    fn git_path_mutations_are_scoped_and_commit_only_the_index() {
        use std::fs;
        use std::time::{SystemTime, UNIX_EPOCH};

        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("termforge-git-surface-{nonce}"));
        fs::create_dir_all(&root).expect("create repository directory");
        let git = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(&root)
                .output()
                .expect("run git")
        };
        assert!(git(&["init", "-q"]).status.success());
        assert!(git(&["config", "user.name", "TermForge Test"])
            .status
            .success());
        assert!(git(&["config", "user.email", "test@termforge.local"])
            .status
            .success());
        fs::write(root.join("tracked.txt"), "base\n").expect("write fixture");
        assert!(git(&["add", "tracked.txt"]).status.success());
        assert!(git(&["commit", "-qm", "initial"]).status.success());

        fs::write(root.join("tracked.txt"), "changed\n").expect("modify fixture");
        git_stage(&root, "tracked.txt").expect("stage path");
        assert!(git_changes(&root).expect("status")[0].staged);
        git_unstage(&root, "tracked.txt").expect("unstage path");
        assert!(git_changes(&root).expect("status")[0].unstaged);
        git_stage(&root, "tracked.txt").expect("restage path");
        git_commit(&root, "update tracked fixture").expect("commit index");
        assert!(git_changes(&root).expect("status").is_empty());

        fs::write(root.join("tracked.txt"), "discard me\n").expect("modify fixture");
        git_discard(&root, "tracked.txt", false).expect("discard tracked change");
        assert_eq!(
            fs::read_to_string(root.join("tracked.txt")).unwrap(),
            "changed\n"
        );
        fs::write(root.join("untracked.txt"), "temporary\n").expect("write untracked fixture");
        git_discard(&root, "untracked.txt", true).expect("discard untracked file");
        assert!(!root.join("untracked.txt").exists());
        fs::remove_dir_all(root).expect("remove repository fixture");
    }

    #[cfg(unix)]
    #[test]
    fn git_paths_are_literal_and_root_relative_from_subdirectories() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let nested = root.join("nested");
        std::fs::create_dir(&nested).unwrap();
        let git = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(root)
                .output()
                .unwrap()
        };
        assert!(git(&["init", "-q"]).status.success());
        assert!(git(&["config", "user.name", "TermForge Test"])
            .status
            .success());
        assert!(git(&["config", "user.email", "test@termforge.local"])
            .status
            .success());
        std::fs::write(root.join("a*.txt"), "original wildcard\n").unwrap();
        std::fs::write(root.join("alpha.txt"), "original other\n").unwrap();
        assert!(git(&["add", "."]).status.success());
        assert!(git(&["commit", "-qm", "initial"]).status.success());
        std::fs::write(root.join("a*.txt"), "changed wildcard\n").unwrap();
        std::fs::write(root.join("alpha.txt"), "changed other\n").unwrap();
        let changes = git_changes(&nested).unwrap();
        assert!(changes.iter().any(|change| change.path == "a*.txt"));
        assert!(
            git_diff(&nested, "a*.txt", None, DiffScope::Unstaged, false)
                .unwrap()
                .contains("changed wildcard")
        );
        git_stage(&nested, "a*.txt").unwrap();
        let changes = git_changes(&nested).unwrap();
        assert!(changes
            .iter()
            .any(|change| change.path == "a*.txt" && change.staged));
        assert!(changes
            .iter()
            .any(|change| change.path == "alpha.txt" && !change.staged && change.unstaged));
        git_unstage(&nested, "a*.txt").unwrap();
        git_discard(&nested, "a*.txt", false).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("a*.txt")).unwrap(),
            "original wildcard\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("alpha.txt")).unwrap(),
            "changed other\n"
        );
        std::fs::write(root.join("u*.txt"), "discard").unwrap();
        std::fs::write(root.join("unrelated.txt"), "keep").unwrap();
        git_discard(&nested, "u*.txt", true).unwrap();
        assert!(!root.join("u*.txt").exists());
        assert_eq!(
            std::fs::read_to_string(root.join("unrelated.txt")).unwrap(),
            "keep"
        );
    }

    #[test]
    fn staged_diff_and_unstage_work_before_first_commit() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        assert!(Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .status()
            .unwrap()
            .success());
        std::fs::write(root.join("new.txt"), "initial content\n").unwrap();
        git_stage(root, "new.txt").unwrap();
        let diff = git_diff(root, "new.txt", None, DiffScope::Staged, false).unwrap();
        assert!(diff.contains("+initial content"));
        git_unstage(root, "new.txt").unwrap();
        let change = git_changes(root)
            .unwrap()
            .into_iter()
            .find(|change| change.path == "new.txt")
            .unwrap();
        assert_eq!(change.kind, ChangeKind::Untracked);
        assert_eq!(
            std::fs::read_to_string(root.join("new.txt")).unwrap(),
            "initial content\n"
        );
    }

    #[test]
    fn staged_rename_review_and_unstage_include_both_paths() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let git = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(root)
                .output()
                .unwrap()
        };
        assert!(git(&["init", "-q"]).status.success());
        assert!(git(&["config", "user.name", "TermForge Test"])
            .status
            .success());
        assert!(git(&["config", "user.email", "test@termforge.local"])
            .status
            .success());
        std::fs::write(root.join("old.txt"), "retained content\n").unwrap();
        assert!(git(&["add", "old.txt"]).status.success());
        assert!(git(&["commit", "-qm", "initial"]).status.success());
        assert!(git(&["mv", "old.txt", "new.txt"]).status.success());
        let change = git_changes(root)
            .unwrap()
            .into_iter()
            .find(|change| change.path == "new.txt")
            .unwrap();
        assert_eq!(change.kind, ChangeKind::Renamed);
        assert_eq!(change.original_path.as_deref(), Some("old.txt"));
        let diff = git_diff(
            root,
            &change.path,
            change.original_path.as_deref(),
            DiffScope::Staged,
            false,
        )
        .unwrap();
        assert!(diff.contains("rename from old.txt") && diff.contains("rename to new.txt"));
        git_unstage_change(root, &change).unwrap();
        let changes = git_changes(root).unwrap();
        assert!(changes.iter().all(|change| !change.staged));
        assert_eq!(
            std::fs::read_to_string(root.join("new.txt")).unwrap(),
            "retained content\n"
        );
        assert!(!root.join("old.txt").exists());
    }
}
