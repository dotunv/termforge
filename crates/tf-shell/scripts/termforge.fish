# TermForge shell integration for fish (OSC 133 + OSC 7).
# Source from config.fish:  test "$TERM_PROGRAM" = TermForge; and source /path/to/termforge.fish

if test "$TERM_PROGRAM" != TermForge; or set -q __tf_loaded
    exit 0
end
set -g __tf_loaded 1

function __tf_prompt_start --on-event fish_prompt
    if set -q __tf_running
        printf '\e]133;D;%s\a' $__tf_status
        set -e __tf_running
    end
    printf '\e]7;file://%s%s\a' (hostname) (string escape --style=url -- $PWD)
    printf '\e]133;A\a'
end
function __tf_preexec --on-event fish_preexec
    set -g __tf_running 1
    printf '\e]133;C\a'
end
function __tf_postexec --on-event fish_postexec
    set -g __tf_status $status
end
