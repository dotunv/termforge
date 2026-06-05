# TermForge shell integration for PowerShell — OSC 133 prompt boundary detection.
# Add to your $PROFILE: . /path/to/termforge.ps1

function prompt {
    $lastCode = $LASTEXITCODE
    # Command finished
    [Console]::Write("`e]133;D;$lastCode`e\")
    # Prompt start
    [Console]::Write("`e]133;A`e\")
    # Render the actual prompt
    "PS $($executionContext.SessionState.Path.CurrentLocation)$('>' * ($nestedPromptLevel + 1)) "
}

# Command start hook via PSReadLine
if (Get-Module PSReadLine) {
    Set-PSReadLineOption -PromptText "`e]133;B`e\"
}
