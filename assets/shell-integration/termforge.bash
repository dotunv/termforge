# TermForge shell integration for bash — OSC 133 prompt boundary detection.
# Source this in your .bashrc: source /path/to/termforge.bash

_termforge_preexec() { printf '\e]133;C\e\\'; }
_termforge_precmd()  {
    local code=$?
    printf "\e]133;D;${code}\e\\"
    printf '\e]133;A\e\\'
}

trap '_termforge_preexec' DEBUG
PROMPT_COMMAND="_termforge_precmd${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
