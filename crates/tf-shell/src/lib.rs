//! Shell integration: the scripts that make shells emit OSC 133 marks and
//! working-directory updates, plus the launch-time arguments that load them
//! automatically.
//!
//! Scripts only activate when `TERM_PROGRAM=TermForge`, never replace the
//! user's own prompt or key bindings, and are safe to load twice.

use std::io;
use std::path::{Path, PathBuf};

use tf_pty::{ShellKind, ShellProfile};

pub const PWSH: &str = include_str!("../scripts/termforge.ps1");
pub const BASH: &str = include_str!("../scripts/termforge.bash");
pub const ZSH: &str = include_str!("../scripts/termforge.zsh");
pub const FISH: &str = include_str!("../scripts/termforge.fish");

/// All scripts with their file names.
pub const SCRIPTS: &[(&str, &str)] = &[
    ("termforge.ps1", PWSH),
    ("termforge.bash", BASH),
    ("termforge.zsh", ZSH),
    ("termforge.fish", FISH),
];

/// Write every script into `dir` (created if missing).
pub fn install(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for (name, body) in SCRIPTS {
        let path = dir.join(name);
        // Avoid rewriting unchanged files so mtimes stay stable.
        if std::fs::read_to_string(&path).ok().as_deref() != Some(*body) {
            std::fs::write(path, body)?;
        }
    }
    Ok(())
}

/// Return a copy of `profile` whose arguments load the integration script
/// from `dir`, or `None` when automatic injection isn't supported for that
/// shell (zsh and fish are sourced from the user's rc file for now).
pub fn inject(profile: &ShellProfile, dir: &Path) -> Option<ShellProfile> {
    let script = |name: &str| -> PathBuf { dir.join(name) };
    let mut p = profile.clone();
    match profile.kind {
        ShellKind::Pwsh | ShellKind::WindowsPowerShell => {
            let path = script("termforge.ps1");
            let path = path.to_string_lossy().replace('\'', "''");
            p.args = vec![
                "-NoLogo".into(),
                "-NoExit".into(),
                "-Command".into(),
                format!(". '{path}'"),
            ];
        }
        ShellKind::Bash | ShellKind::GitBash => {
            p.args = vec![
                "--rcfile".into(),
                script("termforge.bash").to_string_lossy().into(),
                "-i".into(),
            ];
        }
        _ => return None,
    }
    Some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_emit_all_marks() {
        for (name, body) in SCRIPTS {
            for mark in ["133;A", "133;C", "133;D"] {
                assert!(body.contains(mark), "{name} is missing OSC {mark}");
            }
            assert!(
                body.contains("TERM_PROGRAM"),
                "{name} must gate on TERM_PROGRAM"
            );
        }
    }

    #[test]
    fn powershell_script_is_51_compatible_and_non_invasive() {
        // `e is PowerShell 7 only; 5.1 needs [char]27.
        assert!(!PWSH.contains("`e"));
        assert!(
            !PWSH.contains("Set-PSReadLineKeyHandler"),
            "must not replace user key bindings"
        );
    }

    #[test]
    fn install_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        install(dir.path()).unwrap();
        install(dir.path()).unwrap();
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            SCRIPTS.len()
        );
    }

    #[test]
    fn inject_pwsh_quotes_path() {
        let p = ShellProfile {
            name: "PowerShell".into(),
            kind: ShellKind::Pwsh,
            program: "pwsh.exe".into(),
            args: vec![],
        };
        let dir = Path::new("C:/Users/O'Neil/tf");
        let out = inject(&p, dir).unwrap();
        // Separator differs by platform; the quoting is what matters.
        let expected = dir
            .join("termforge.ps1")
            .to_string_lossy()
            .replace('\'', "''");
        assert!(expected.contains("O''Neil"));
        assert_eq!(out.args.last().unwrap(), &format!(". '{expected}'"));
    }
}
