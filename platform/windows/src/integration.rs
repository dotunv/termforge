//! Shell integration bootstrap.
//!
//! TermForge's command blocks and the anchored input editor depend on OSC 133
//! prompt marks.  Rather than asking users to edit their PowerShell profile,
//! we write a small integration script to the config directory and launch
//! pwsh/powershell with `-NoExit -File <script>` so the marks (plus OSC 7
//! cwd reports) are emitted out of the box.
//!
//! Controlled by `shell.auto_integration` in termforge.toml (default on).

use std::path::PathBuf;

/// The PowerShell hook: OSC 133 D (previous exit) + A (prompt start) and an
/// OSC 7 cwd report on every prompt, OSC 133 B at prompt end, and OSC 133 C
/// on Enter via PSReadLine so command output is attributed to a block.
const PS_INTEGRATION: &str = r#"# TermForge shell integration (auto-generated; edits are overwritten)
function global:prompt {
    $__tf_exit = if ($?) { 0 } else { 1 }
    $e = [char]27; $b = [char]7
    $p = $executionContext.SessionState.Path.CurrentLocation.ProviderPath -replace '\\', '/'
    [Console]::Write("$e]133;D;$__tf_exit$b$e]133;A$b$e]7;file://localhost/$p$b")
    # Visible prompt suppressed: TermForge renders command blocks (cwd/exit live
    # in the block header + bottom input bar), so the shell emits only the OSC
    # 133 B prompt-end mark and no "PS C:\...>" text.
    "$e]133;B$b"
}
if (Get-Module -ListAvailable -Name PSReadLine) {
    Set-PSReadLineKeyHandler -Key Enter -ScriptBlock {
        $e = [char]27; $b = [char]7
        [Console]::Write("$e]133;C$b")
        [Microsoft.PowerShell.PSConsoleReadLine]::AcceptLine()
    }
}
"#;

fn config_dir() -> PathBuf {
    let appdata = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    appdata.join("TermForge")
}

/// Write (or refresh) the integration script. Returns its path.
pub fn ensure_integration_script() -> Option<PathBuf> {
    let dir = config_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("integration.ps1");
    std::fs::write(&path, PS_INTEGRATION).ok()?;
    Some(path)
}

/// Wrap a bare shell executable into a command line that sources the
/// integration script. Non-PowerShell shells are returned unchanged.
pub fn shell_command_line(shell: &str) -> String {
    let exe = std::path::Path::new(shell)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(shell)
        .to_ascii_lowercase();
    if exe != "pwsh" && exe != "powershell" {
        return shell.to_string();
    }
    match ensure_integration_script() {
        Some(p) => format!(
            "{shell} -NoExit -ExecutionPolicy Bypass -File \"{}\"",
            p.display()
        ),
        None => shell.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_powershell_untouched() {
        assert_eq!(shell_command_line("cmd.exe"), "cmd.exe");
        assert_eq!(shell_command_line("nu.exe"), "nu.exe");
    }

    #[test]
    fn powershell_gets_integration_args() {
        let line = shell_command_line("pwsh.exe");
        assert!(line.starts_with("pwsh.exe -NoExit -ExecutionPolicy Bypass -File \""));
        assert!(line.contains("integration.ps1"));
    }
}
