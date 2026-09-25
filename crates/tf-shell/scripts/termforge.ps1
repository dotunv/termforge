# TermForge shell integration for PowerShell 7+ and Windows PowerShell 5.1.
# Emits OSC 133 (prompt/command marks) and OSC 9;9 (working directory).
# Safe to load more than once; preserves the user's own prompt and PSReadLine setup.

if ($env:TERM_PROGRAM -ne 'TermForge' -or $global:__TermForgeLoaded) { return }
$global:__TermForgeLoaded = $true

$global:__TfEsc = [char]27
$global:__TfBel = [char]7
$global:__TfOriginalPrompt = $function:prompt

function global:prompt {
    # Capture status before anything else can overwrite it.
    $success = $?
    $native = $global:LASTEXITCODE
    $code = if ($success) { 0 } elseif ($native) { $native } else { 1 }

    $e = $global:__TfEsc; $a = $global:__TfBel
    $out = ''
    if ($global:__TfCommandRunning) {
        $out += "$e]133;D;$code$a"
        $global:__TfCommandRunning = $false
    }
    $loc = $executionContext.SessionState.Path.CurrentLocation
    if ($loc.Provider.Name -eq 'FileSystem') {
        $out += "$e]9;9;`"$($loc.ProviderPath)`"$a"
    }
    $out += "$e]133;A$a"

    __TfWrapReadLine
    $userPrompt = & $global:__TfOriginalPrompt
    $global:LASTEXITCODE = $native
    return "$out$userPrompt$e]133;B$a"
}

# Mark command start after the line is read, without replacing key handlers.
# PSReadLine may not be loaded yet when this script runs (for example under
# -Command), so wrapping is retried from the prompt until it succeeds.
function global:__TfWrapReadLine {
    if ($global:__TfReadLineWrapped) { return }
    $original = Get-Command PSConsoleHostReadLine -CommandType Function -ErrorAction SilentlyContinue
    if (-not $original) { return }
    $global:__TfOriginalReadLine = $original.ScriptBlock
    $global:__TfReadLineWrapped = $true
    function global:PSConsoleHostReadLine {
        $line = & $global:__TfOriginalReadLine
        $global:__TfCommandRunning = $true
        [Console]::Write("$($global:__TfEsc)]133;C$($global:__TfBel)")
        return $line
    }
}
__TfWrapReadLine
