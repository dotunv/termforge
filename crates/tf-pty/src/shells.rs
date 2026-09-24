use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    Pwsh,
    WindowsPowerShell,
    Cmd,
    GitBash,
    Wsl,
    Bash,
    Zsh,
    Fish,
    Other,
}

/// A launchable shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellProfile {
    pub name: String,
    pub kind: ShellKind,
    pub program: PathBuf,
    pub args: Vec<String>,
}

impl ShellProfile {
    fn new(name: &str, kind: ShellKind, program: PathBuf, args: &[&str]) -> Self {
        Self {
            name: name.to_owned(),
            kind,
            program,
            args: args.iter().map(|s| (*s).to_owned()).collect(),
        }
    }
}

/// Shells available on this machine, best default first.
pub fn discover_shells() -> Vec<ShellProfile> {
    let mut out = Vec::new();
    platform(&mut out);
    // `/bin` is often a symlink to `/usr/bin`; keep the first of each target.
    let mut seen = Vec::new();
    out.retain(|s| {
        let key = std::fs::canonicalize(&s.program).unwrap_or_else(|_| s.program.clone());
        if seen.contains(&key) {
            false
        } else {
            seen.push(key);
            true
        }
    });
    out
}

#[cfg(windows)]
fn platform(out: &mut Vec<ShellProfile>) {
    if let Some(p) = which("pwsh.exe") {
        out.push(ShellProfile::new(
            "PowerShell",
            ShellKind::Pwsh,
            p,
            &["-NoLogo"],
        ));
    }
    if let Some(p) = which("powershell.exe") {
        out.push(ShellProfile::new(
            "Windows PowerShell",
            ShellKind::WindowsPowerShell,
            p,
            &["-NoLogo"],
        ));
    }
    for base in ["ProgramFiles", "ProgramW6432", "LOCALAPPDATA"] {
        if let Some(root) = std::env::var_os(base) {
            let candidates = [
                Path::new(&root).join("Git").join("bin").join("bash.exe"),
                Path::new(&root)
                    .join("Programs")
                    .join("Git")
                    .join("bin")
                    .join("bash.exe"),
            ];
            if let Some(p) = candidates.into_iter().find(|p| p.is_file()) {
                if !out.iter().any(|s| s.program == p) {
                    out.push(ShellProfile::new(
                        "Git Bash",
                        ShellKind::GitBash,
                        p,
                        &["--login", "-i"],
                    ));
                }
            }
        }
    }
    if let Some(p) = which("wsl.exe") {
        out.push(ShellProfile::new("WSL", ShellKind::Wsl, p, &[]));
    }
    if let Some(p) = which("cmd.exe") {
        out.push(ShellProfile::new("Command Prompt", ShellKind::Cmd, p, &[]));
    }
}

#[cfg(not(windows))]
fn platform(out: &mut Vec<ShellProfile>) {
    let kind_of = |p: &Path| match p.file_name().and_then(|n| n.to_str()) {
        Some("bash") => ShellKind::Bash,
        Some("zsh") => ShellKind::Zsh,
        Some("fish") => ShellKind::Fish,
        Some("pwsh") => ShellKind::Pwsh,
        _ => ShellKind::Other,
    };
    if let Some(sh) = std::env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
    {
        let name = sh
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("shell")
            .to_owned();
        out.push(ShellProfile::new(&name, kind_of(&sh), sh, &["-l"]));
    }
    for name in ["zsh", "bash", "fish", "pwsh"] {
        if let Some(p) = which(name) {
            if !out.iter().any(|s| s.program == p) {
                out.push(ShellProfile::new(name, kind_of(&p), p, &["-l"]));
            }
        }
    }
}

/// Minimal `PATH` lookup.
fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_does_not_panic_and_has_no_duplicates() {
        let shells = discover_shells();
        for (i, a) in shells.iter().enumerate() {
            assert!(
                shells[i + 1..].iter().all(|b| b.program != a.program),
                "duplicate {a:?}"
            );
        }
    }
}
