# TermForge shell integration for PowerShell — OSC 133 prompt boundary detection.
# Add to your $PROFILE: . /path/to/termforge.ps1

function prompt {
    # Report previous command's exit code (133;D)
    $code = if ($?) { 0 } else { if ($LASTEXITCODE) { $LASTEXITCODE } else { 1 } }
    [Console]::Write("`e]133;D;$code`e\")
    # Prompt start (133;A)
    [Console]::Write("`e]133;A`e\")
    # Render the actual prompt
    $p = "PS $($executionContext.SessionState.Path.CurrentLocation)$('>' * ($nestedPromptLevel + 1)) "
    # Prompt end (133;B) — marks boundary between prompt text and user input
    [Console]::Write($p)
    [Console]::Write("`e]133;B`e\")
    return " "
}

# Command start hook: PSReadLine fires AcceptLine before executing the command.
if (Get-Module PSReadLine) {
    Set-PSReadLineKeyHandler -Key Enter -ScriptBlock {
        # Emit command start (133;C) then accept the line
        [Console]::Write("`e]133;C`e\")
        [Microsoft.PowerShell.PSConsoleReadLine]::AcceptLine()
    }
}
