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
    Ssh,
    Other,
}

/// Build a shell profile that delegates connection and authentication to the
/// installed OpenSSH client. The host is passed as one argv element, never
/// interpreted by a shell.
pub fn ssh_profile(host: &str) -> Option<ShellProfile> {
    let host = host.trim();
    if host.is_empty() || host.starts_with('-') || host.chars().any(char::is_whitespace) {
        return None;
    }
    let program = which(if cfg!(windows) { "ssh.exe" } else { "ssh" })?;
    Some(ShellProfile {
        name: format!("SSH {host}"),
        kind: ShellKind::Ssh,
        program,
        args: vec![host.to_owned()],
    })
}

/// Discover concrete aliases declared by `Host` directives in the user's
/// OpenSSH config. Wildcard patterns are intentionally omitted because they
/// are connection rules, not selectable hosts. OpenSSH remains authoritative
/// when the selected alias is launched.
pub fn discover_ssh_hosts() -> Vec<String> {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" });
    let Some(home) = home else { return Vec::new() };
    let path = PathBuf::from(home).join(".ssh").join("config");
    let Ok(config) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    parse_ssh_hosts(&config)
}

fn parse_ssh_hosts(config: &str) -> Vec<String> {
    let mut hosts = Vec::new();
    for line in config.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        let Some(separator) = line.find(|c: char| c.is_whitespace() || c == '=') else {
            continue;
        };
        if !line[..separator].eq_ignore_ascii_case("host") {
            continue;
        }
        let patterns =
            line[separator..].trim_start_matches(|c: char| c.is_whitespace() || c == '=');
        for host in patterns.split_whitespace() {
            if host.starts_with('!')
                || host.starts_with('-')
                || host.chars().any(|c| matches!(c, '*' | '?' | '[' | ']'))
                || hosts
                    .iter()
                    .any(|known: &String| known.eq_ignore_ascii_case(host))
            {
                continue;
            }
            hosts.push(host.to_owned());
        }
    }
    hosts
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
    /// Whether `tf-shell` can inject OSC 133 integration into this shell.
    ///
    /// Without it there are no command blocks, no exit status and no cwd
    /// tracking — the terminal still works, but it is a much poorer thing.
    /// Callers should surface this rather than let the user discover it by
    /// noticing that blocks never appear.
    pub fn supports_integration(&self) -> bool {
        matches!(
            self.kind,
            ShellKind::Pwsh | ShellKind::WindowsPowerShell | ShellKind::Bash | ShellKind::GitBash
        )
    }
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
        // `--cd ~` rather than inheriting the Windows cwd, which wsl.exe
        // would otherwise try to translate and sometimes fail outright.
        // The integration script is passed by *path*: wsl.exe copies the
        // working directory into the distro, but a Windows path is not
        // readable there. tf-shell rewrites this to a /mnt path at spawn.
        out.push(ShellProfile::new("WSL", ShellKind::Wsl, p, &["--cd", "~"]));
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
    // The login shell comes first: `$SHELL` can be inherited from whatever
    // launched the app (a launcher, another terminal) and not be the user's.
    let candidates = [login_shell(), std::env::var_os("SHELL").map(PathBuf::from)];
    for sh in candidates.into_iter().flatten().filter(|p| p.is_file()) {
        if out.iter().any(|s| same_file(&s.program, &sh)) {
            continue;
        }
        let name = sh
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("shell")
            .to_owned();
        out.push(ShellProfile::new(&name, kind_of(&sh), sh, &["-l"]));
    }
    for name in ["zsh", "bash", "fish", "pwsh"] {
        if let Some(p) = which(name) {
            if !out.iter().any(|s| same_file(&s.program, &p)) {
                out.push(ShellProfile::new(name, kind_of(&p), p, &["-l"]));
            }
        }
    }
}

/// The user's login shell from the passwd database. Directory-service users
/// (macOS, LDAP) are not in `/etc/passwd`; callers fall back to `$SHELL`.
#[cfg(not(windows))]
fn login_shell() -> Option<PathBuf> {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .ok()?;
    shell_from_passwd(&std::fs::read_to_string("/etc/passwd").ok()?, &user)
}

#[cfg(not(windows))]
fn shell_from_passwd(passwd: &str, user: &str) -> Option<PathBuf> {
    passwd.lines().find_map(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        (fields.len() >= 7 && fields[0] == user && !fields[6].is_empty())
            .then(|| PathBuf::from(fields[6]))
    })
}

/// Whether two paths are the same file, so `/bin/fish` and `/usr/bin/fish`
/// (a symlinked `/bin`) are not listed twice.
#[cfg(not(windows))]
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
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
    #[cfg(not(windows))]
    #[test]
    fn login_shell_comes_from_passwd() {
        let passwd = "root:x:0:0::/root:/bin/bash\ndotun:x:1000:1000::/home/dotun:/bin/fish\n";
        assert_eq!(
            super::shell_from_passwd(passwd, "dotun"),
            Some(std::path::PathBuf::from("/bin/fish"))
        );
        assert_eq!(super::shell_from_passwd(passwd, "nobody"), None);
    }

    use super::*;

    #[test]
    fn integration_support_matches_what_tf_shell_can_inject() {
        // Kept in step with `tf_shell::inject` so the UI can warn before the
        // user discovers missing blocks the hard way.
        let expected = |k: ShellKind| {
            matches!(
                k,
                ShellKind::Pwsh
                    | ShellKind::WindowsPowerShell
                    | ShellKind::Bash
                    | ShellKind::GitBash
            )
        };
        for kind in [
            ShellKind::Pwsh,
            ShellKind::WindowsPowerShell,
            ShellKind::Cmd,
            ShellKind::GitBash,
            ShellKind::Wsl,
            ShellKind::Bash,
            ShellKind::Zsh,
            ShellKind::Fish,
            ShellKind::Ssh,
            ShellKind::Other,
        ] {
            let p = ShellProfile {
                name: "x".into(),
                kind,
                program: PathBuf::from("x"),
                args: vec![],
            };
            assert_eq!(
                p.supports_integration(),
                expected(kind),
                "{kind:?} disagrees with tf-shell"
            );
        }
    }

    #[test]
    fn wsl_starts_in_its_own_home() {
        // Only meaningful on Windows, where WSL is discovered.
        let Some(p) = discover_shells()
            .into_iter()
            .find(|s| s.kind == ShellKind::Wsl)
        else {
            return;
        };
        assert!(
            p.args.windows(2).any(|w| w == ["--cd", "~"]),
            "WSL should not inherit a Windows cwd: {p:?}"
        );
    }

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

    #[test]
    fn ssh_hosts_are_single_safe_arguments() {
        assert!(ssh_profile("").is_none());
        assert!(ssh_profile("-oProxyCommand=bad").is_none());
        assert!(ssh_profile("host extra").is_none());
        if let Some(profile) = ssh_profile("devbox") {
            assert_eq!(profile.kind, ShellKind::Ssh);
            assert_eq!(profile.args, ["devbox"]);
        }
    }

    #[test]
    fn ssh_config_discovers_only_concrete_unique_aliases() {
        let hosts = parse_ssh_hosts(
            r#"
                Host devbox staging
                  HostName 100.64.0.8
                host *.internal !bastion
                HOST DevBox
                Host=prod
                Host -oBad
                # Host ignored
            "#,
        );
        assert_eq!(hosts, ["devbox", "staging", "prod"]);
    }
}
