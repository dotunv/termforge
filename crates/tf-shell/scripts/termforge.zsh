# TermForge shell integration for zsh (OSC 133 + OSC 7).
# Source from ~/.zshrc:  [[ $TERM_PROGRAM == TermForge ]] && source /path/to/termforge.zsh

[[ "$TERM_PROGRAM" == "TermForge" && -z "$__TF_LOADED" ]] || return 0
__TF_LOADED=1
autoload -Uz add-zsh-hook

__tf_precmd() {
    local code=$?
    if [[ -n "$__TF_RUNNING" ]]; then
        printf '\e]133;D;%s\a' "$code"
        __TF_RUNNING=
    fi
    printf '\e]7;file://%s%s\a' "${HOST}" "${PWD// /%20}"
    printf '\e]133;A\a'
}
__tf_preexec() {
    __TF_RUNNING=1
    printf '\e]133;C\a'
}
add-zsh-hook precmd __tf_precmd
add-zsh-hook preexec __tf_preexec
PS1="${PS1}%{$(printf '\e]133;B\a')%}"
