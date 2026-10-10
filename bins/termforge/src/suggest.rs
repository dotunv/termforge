//! Task suggestions: common commands for the project in front of you, and
//! commands you keep re-running.
//!
//! Everything here is pure and local: it looks at files in the project root
//! and at command lines the UI hands it. Nothing is sent anywhere.

use std::collections::HashMap;
use std::path::Path;

/// A command worth turning into a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    pub title: String,
    pub command: String,
    /// Why this is suggested, shown under the title.
    pub reason: String,
}

impl Suggestion {
    fn new(title: &str, command: impl Into<String>, reason: &str) -> Self {
        Self {
            title: title.to_owned(),
            command: command.into(),
            reason: reason.to_owned(),
        }
    }
}

/// Most suggestions returned for one project.
const MAX_PROJECT_SUGGESTIONS: usize = 12;

/// Common commands for the project rooted at `root`, detected from the files
/// in it. Ordered with the most generally useful first.
pub fn for_project(root: &Path) -> Vec<Suggestion> {
    let mut out = Vec::new();
    cargo(root, &mut out);
    node(root, &mut out);
    make(root, &mut out);
    just(root, &mut out);
    go(root, &mut out);
    python(root, &mut out);
    docker(root, &mut out);
    let mut seen = std::collections::HashSet::new();
    out.retain(|s| seen.insert(s.command.clone()));
    out.truncate(MAX_PROJECT_SUGGESTIONS);
    out
}

fn cargo(root: &Path, out: &mut Vec<Suggestion>) {
    let Ok(manifest) = std::fs::read_to_string(root.join("Cargo.toml")) else {
        return;
    };
    let workspace = manifest.contains("[workspace]");
    let why = "Rust project (Cargo.toml)";
    if root.join("xtask").join("Cargo.toml").is_file() {
        out.push(Suggestion::new(
            "Run project checks",
            "cargo xtask ci",
            "This repo has an xtask",
        ));
    }
    out.push(Suggestion::new("Build", "cargo build", why));
    out.push(Suggestion::new(
        "Test",
        if workspace {
            "cargo test --workspace"
        } else {
            "cargo test"
        },
        why,
    ));
    out.push(Suggestion::new("Lint", "cargo clippy --all-targets", why));
    out.push(Suggestion::new("Format", "cargo fmt --all", why));
    out.push(Suggestion::new("Run", "cargo run", why));
}

/// Script names worth suggesting, in the order they are listed.
const NODE_SCRIPTS: &[&str] = &[
    "dev",
    "start",
    "build",
    "test",
    "lint",
    "typecheck",
    "format",
    "preview",
];

fn node(root: &Path, out: &mut Vec<Suggestion>) {
    let Ok(text) = std::fs::read_to_string(root.join("package.json")) else {
        return;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    let Some(scripts) = value.get("scripts").and_then(|s| s.as_object()) else {
        return;
    };
    let manager = if root.join("pnpm-lock.yaml").is_file() {
        "pnpm"
    } else if root.join("yarn.lock").is_file() {
        "yarn"
    } else if root.join("bun.lockb").is_file() || root.join("bun.lock").is_file() {
        "bun"
    } else {
        "npm"
    };
    for name in NODE_SCRIPTS {
        if scripts.contains_key(*name) {
            let command = match (manager, *name) {
                ("npm", "start") => "npm start".to_owned(),
                ("npm", "test") => "npm test".to_owned(),
                ("npm", other) => format!("npm run {other}"),
                (manager, other) => format!("{manager} {other}"),
            };
            out.push(Suggestion::new(
                &capitalize(name),
                command,
                "Script in package.json",
            ));
        }
    }
}

/// Make/just target names worth suggesting.
const TARGETS: &[&str] = &[
    "build", "test", "lint", "check", "run", "dev", "fmt", "format", "clean", "install",
];

fn make(root: &Path, out: &mut Vec<Suggestion>) {
    let Some(text) = ["Makefile", "makefile", "GNUmakefile"]
        .iter()
        .find_map(|name| std::fs::read_to_string(root.join(name)).ok())
    else {
        return;
    };
    let targets = recipe_names(&text);
    for name in TARGETS {
        if targets.iter().any(|target| target == name) {
            out.push(Suggestion::new(
                &capitalize(name),
                format!("make {name}"),
                "Target in Makefile",
            ));
        }
    }
}

fn just(root: &Path, out: &mut Vec<Suggestion>) {
    let Some(text) = ["justfile", "Justfile", ".justfile"]
        .iter()
        .find_map(|name| std::fs::read_to_string(root.join(name)).ok())
    else {
        return;
    };
    let recipes = recipe_names(&text);
    for name in TARGETS {
        if recipes.iter().any(|recipe| recipe == name) {
            out.push(Suggestion::new(
                &capitalize(name),
                format!("just {name}"),
                "Recipe in justfile",
            ));
        }
    }
}

/// Names defined as `name:` or `name args:` at the start of a line. Good
/// enough for suggestions; it does not try to be a Make parser.
fn recipe_names(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| !line.starts_with([' ', '\t', '#', '.']))
        .filter_map(|line| {
            let head = line.split_once(':')?.0;
            // `VAR := value` and `VAR = value` are not targets.
            if line.contains(":=") && head.trim().chars().all(|c| c.is_alphanumeric() || c == '_') {
                return None;
            }
            let name = head.split_whitespace().next()?;
            name.chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
                .then(|| name.to_owned())
        })
        .collect()
}

fn go(root: &Path, out: &mut Vec<Suggestion>) {
    if !root.join("go.mod").is_file() {
        return;
    }
    let why = "Go module (go.mod)";
    out.push(Suggestion::new("Build", "go build ./...", why));
    out.push(Suggestion::new("Test", "go test ./...", why));
    out.push(Suggestion::new("Vet", "go vet ./...", why));
}

fn python(root: &Path, out: &mut Vec<Suggestion>) {
    let pyproject = std::fs::read_to_string(root.join("pyproject.toml")).unwrap_or_default();
    let has_project = !pyproject.is_empty()
        || root.join("requirements.txt").is_file()
        || root.join("setup.py").is_file();
    if !has_project {
        return;
    }
    let why = "Python project";
    if pyproject.contains("pytest")
        || root.join("tests").is_dir()
        || root.join("pytest.ini").is_file()
    {
        out.push(Suggestion::new("Test", "pytest", why));
    }
    if pyproject.contains("ruff") || root.join("ruff.toml").is_file() {
        out.push(Suggestion::new("Lint", "ruff check .", why));
        out.push(Suggestion::new("Format", "ruff format .", why));
    }
    if pyproject.contains("mypy") {
        out.push(Suggestion::new("Type check", "mypy .", why));
    }
}

fn docker(root: &Path, out: &mut Vec<Suggestion>) {
    let compose = [
        "compose.yaml",
        "compose.yml",
        "docker-compose.yaml",
        "docker-compose.yml",
    ]
    .iter()
    .any(|name| root.join(name).is_file());
    if compose {
        let why = "Compose file found";
        out.push(Suggestion::new("Start services", "docker compose up", why));
        out.push(Suggestion::new("Stop services", "docker compose down", why));
    }
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

// ---- commands typed in the terminal --------------------------------------

/// Where a prompt usually ends and the command begins.
const PROMPT_MARKERS: &[&str] = &["❯ ", "➜ ", "› ", "» ", "λ ", "$ ", "# ", "% ", "> "];

/// Commands that are not worth a task however often they are typed.
const TRIVIAL: &[&str] = &[
    "cd", "ls", "ll", "la", "l", "clear", "cls", "pwd", "exit", "logout", "history", "fg", "bg",
    "jobs", "reset", "tf", "z",
];

/// Substrings that suggest a secret; such lines are never suggested.
const SECRETISH: &[&str] = &[
    "password",
    "passwd",
    "secret",
    "token",
    "apikey",
    "api_key",
    "api-key",
    "authorization",
    "bearer",
    "private_key",
];

/// Pull the command out of a terminal row holding "prompt + command", the
/// moment before Enter is pressed. The prompt is stripped heuristically, so
/// this returns `None` rather than guess when nothing looks like a prompt.
pub fn extract_command(row: &str) -> Option<String> {
    let row = row.trim_end();
    let (index, marker) = PROMPT_MARKERS
        .iter()
        .filter_map(|marker| row.find(marker).map(|index| (index, *marker)))
        .min_by_key(|(index, _)| *index)?;
    let rest = &row[index + marker.len()..];
    // A right-aligned prompt segment is separated by a wide gap.
    let rest = rest.split("    ").next().unwrap_or(rest).trim();
    if rest.is_empty() || rest.chars().count() > 200 || rest.chars().any(char::is_control) {
        return None;
    }
    let first = rest.split_whitespace().next()?;
    if TRIVIAL.contains(&first) {
        return None;
    }
    let lower = rest.to_ascii_lowercase();
    if SECRETISH.iter().any(|word| lower.contains(word)) {
        return None;
    }
    Some(rest.to_owned())
}

/// Counts how often each command line is entered in a window.
#[derive(Debug, Default)]
pub struct Frequency {
    counts: HashMap<String, u32>,
}

impl Frequency {
    /// Distinct commands remembered; bounds memory in a long session.
    const MAX_ENTRIES: usize = 500;

    pub fn record(&mut self, command: &str) {
        if self.counts.len() >= Self::MAX_ENTRIES && !self.counts.contains_key(command) {
            return;
        }
        *self.counts.entry(command.to_owned()).or_insert(0) += 1;
    }

    /// Commands entered at least `min` times, most frequent first, skipping
    /// any `skip` says are already tasks.
    pub fn frequent(&self, min: u32, skip: impl Fn(&str) -> bool) -> Vec<(String, u32)> {
        let mut out: Vec<_> = self
            .counts
            .iter()
            .filter(|(command, count)| **count >= min && !skip(command))
            .map(|(command, count)| (command.clone(), *count))
            .collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn project(files: &[(&str, &str)]) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tf-suggest-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, content) in files {
            let path = dir.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        dir
    }

    fn commands(root: &Path) -> Vec<String> {
        for_project(root).into_iter().map(|s| s.command).collect()
    }

    #[test]
    fn cargo_workspace_with_xtask() {
        let root = project(&[
            ("Cargo.toml", "[workspace]\nmembers = []\n"),
            ("xtask/Cargo.toml", ""),
        ]);
        let found = commands(&root);
        assert_eq!(found[0], "cargo xtask ci");
        assert!(found.contains(&"cargo test --workspace".to_owned()));
        assert!(found.contains(&"cargo clippy --all-targets".to_owned()));
    }

    #[test]
    fn node_scripts_use_the_detected_package_manager() {
        let root = project(&[
            (
                "package.json",
                r#"{"scripts":{"dev":"vite","build":"vite build","test":"vitest","other":"x"}}"#,
            ),
            ("pnpm-lock.yaml", ""),
        ]);
        assert_eq!(commands(&root), ["pnpm dev", "pnpm build", "pnpm test"]);
        let npm = project(&[(
            "package.json",
            r#"{"scripts":{"test":"jest","build":"tsc"}}"#,
        )]);
        assert_eq!(commands(&npm), ["npm run build", "npm test"]);
    }

    #[test]
    fn makefile_targets_ignore_variables_and_dotted_names() {
        let root = project(&[(
            "Makefile",
            "CC := gcc\n.PHONY: build\nbuild: main.o\n\tcc -o app main.o\ntest:\n\t./app\nobscure:\n",
        )]);
        assert_eq!(commands(&root), ["make build", "make test"]);
    }

    #[test]
    fn go_python_and_compose() {
        let root = project(&[
            ("go.mod", "module x\n"),
            ("pyproject.toml", "[tool.ruff]\n[tool.pytest]\n"),
            ("compose.yaml", ""),
        ]);
        let found = commands(&root);
        for expected in [
            "go test ./...",
            "pytest",
            "ruff check .",
            "docker compose up",
        ] {
            assert!(
                found.contains(&expected.to_owned()),
                "{expected}: {found:?}"
            );
        }
    }

    #[test]
    fn empty_project_has_no_suggestions() {
        assert!(commands(&project(&[("README.md", "")])).is_empty());
    }

    #[test]
    fn extracts_commands_after_common_prompts() {
        assert_eq!(
            extract_command("❯ cargo test --workspace").as_deref(),
            Some("cargo test --workspace")
        );
        assert_eq!(
            extract_command("dotun@pc:~/proj$ npm run build").as_deref(),
            Some("npm run build")
        );
        // Right-aligned prompt segments are dropped.
        assert_eq!(
            extract_command("❯ make test            12:04:11").as_deref(),
            Some("make test")
        );
        // A redirect after the prompt marker is part of the command.
        assert_eq!(
            extract_command("$ echo hi > out.txt").as_deref(),
            Some("echo hi > out.txt")
        );
    }

    #[test]
    fn refuses_trivial_secret_or_promptless_lines() {
        assert_eq!(extract_command("❯ ls"), None);
        assert_eq!(extract_command("❯ cd /tmp"), None);
        assert_eq!(extract_command("❯ export TOKEN=abc"), None);
        assert_eq!(extract_command("❯ curl -H 'Authorization: x' u"), None);
        assert_eq!(extract_command("no prompt here"), None);
        assert_eq!(extract_command("❯ "), None);
    }

    #[test]
    fn frequency_ranks_and_skips_existing_tasks() {
        let mut freq = Frequency::default();
        for _ in 0..4 {
            freq.record("cargo test");
        }
        for _ in 0..3 {
            freq.record("cargo fmt --all");
        }
        freq.record("git status");
        let found = freq.frequent(3, |command| command == "cargo fmt --all");
        assert_eq!(found, vec![("cargo test".to_owned(), 4)]);
        assert_eq!(freq.frequent(3, |_| false).len(), 2);
    }
}
