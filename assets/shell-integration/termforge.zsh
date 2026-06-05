# TermForge shell integration for zsh — OSC 133 prompt boundary detection.
# Source this in your .zshrc: source /path/to/termforge.zsh

precmd()  { printf '\e]133;A\e\\' }   # prompt start
preexec() { printf '\e]133;C\e\\' }   # command start (preexec receives the command string)

# Report exit code after command finishes
_termforge_exit_code() {
    local code=$?
    printf "\e]133;D;${code}\e\\"
    return $code
}
precmd_functions+=(_termforge_exit_code)
