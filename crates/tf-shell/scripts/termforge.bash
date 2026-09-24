# TermForge shell integration for bash >= 4.4 (OSC 133 + OSC 7).
# Loaded via --rcfile; sources the user's normal startup files first.

if [[ -z "$__TF_SOURCED_USER_RC" ]]; then
    __TF_SOURCED_USER_RC=1
    if shopt -q login_shell; then
        for f in ~/.bash_profile ~/.bash_login ~/.profile; do [[ -r "$f" ]] && { . "$f"; break; }; done
    elif [[ -r ~/.bashrc ]]; then
        . ~/.bashrc
    fi
fi

[[ "$TERM_PROGRAM" == "TermForge" && -z "$__TF_LOADED" ]] || return 0
__TF_LOADED=1

# Sets __TF_URL without forking (subshells are slow in Git Bash).
__tf_urlencode() {
    local LC_ALL=C s="$1" out="" c i
    for (( i = 0; i < ${#s}; i++ )); do
        c="${s:i:1}"
        case "$c" in [a-zA-Z0-9/._~-]) out+="$c" ;; *) printf -v c '%%%02X' "'$c"; out+="$c" ;; esac
    done
    __TF_URL="$out"
}

__tf_precmd() {
    local code=$?
    # D is emitted before every prompt except the first. A D that isn't
    # preceded by a C (empty Enter) is ignored by the block detector.
    if [[ -n "$__TF_PROMPTED" ]]; then
        printf '\e]133;D;%s\a' "$code"
    fi
    __TF_PROMPTED=1
    __tf_urlencode "$PWD"
    printf '\e]7;file://%s%s\a' "${HOSTNAME:-localhost}" "$__TF_URL"
    return $code
}

if [[ "$PROMPT_COMMAND" != *__tf_precmd* ]]; then
    PROMPT_COMMAND="__tf_precmd${PROMPT_COMMAND:+; $PROMPT_COMMAND}"
fi
# \[ \] keep readline's width calculation correct.
PS1="\[\e]133;A\a\]${PS1}\[\e]133;B\a\]"
# PS0 is expanded after a command is read and before it runs.
PS0="${PS0}\e]133;C\a"
