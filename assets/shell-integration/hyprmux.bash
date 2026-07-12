# hyprmux shell integration for bash (cross-platform plan Phase 8).
#
# Emits OSC 7 (cwd), OSC 133 A/B/C/D (command lifecycle), and a hyprmux-namespaced OSC 133
# `hyprmux_exe=` parameter carrying only the executable basename - never a full command line - so
# hyprmux's smart-focus and pane-runtime-state tracking work without polling `/proc`. Installed by
# hyprmux itself via `bash --rcfile <generated wrapper>`; never edits `~/.bashrc` or any other
# dotfile. Safe to source more than once (idempotent) and a no-op in a non-interactive shell.

if [ -n "${HYPRMUX_SHELL_INTEGRATION_LOADED:-}" ] || [ -z "${PS1:-}" ]; then
    return 0 2>/dev/null || exit 0
fi
HYPRMUX_SHELL_INTEGRATION_LOADED=1

# Percent-encode everything outside the URI-unreserved set, matching the framework's decoder.
__hyprmux_urlencode() {
    local input="$1" output="" i char
    for ((i = 0; i < ${#input}; i++)); do
        char="${input:i:1}"
        case "$char" in
        [a-zA-Z0-9.~_-]) output+="$char" ;;
        *) printf -v char '%%%02X' "'$char"
            output+="$char" ;;
        esac
    done
    printf '%s' "$output"
}

# Percent-encode a path for a `file://` URI, keeping `/` literal (it is a reserved-but-permitted
# path separator, not something the framework's decoder expects escaped).
__hyprmux_urlencode_path() {
    local input="$1" output="" i char
    for ((i = 0; i < ${#input}; i++)); do
        char="${input:i:1}"
        case "$char" in
        [a-zA-Z0-9._~/-]) output+="$char" ;;
        *) printf -v char '%%%02X' "'$char"
            output+="$char" ;;
        esac
    done
    printf '%s' "$output"
}

__hyprmux_osc7() {
    local host
    host=$(hostname 2>/dev/null || printf '%s' "${HOSTNAME:-}")
    printf '\e]7;file://%s%s\e\\' "$(__hyprmux_urlencode "$host")" "$(__hyprmux_urlencode_path "$PWD")"
}

# `B` (end of prompt) is embedded directly in PS1 (see below) rather than emitted from a function,
# since it must be the very last thing printed before the input area for the terminal to attribute
# the boundary correctly.
__hyprmux_osc133_a() { printf '\e]133;A\e\\'; }

__hyprmux_osc133_c() {
    local cmdline="$1" exe
    exe="${cmdline%% *}"
    exe="${exe##*/}"
    if [ -n "$exe" ]; then
        printf '\e]133;C;hyprmux_exe=%s\e\\' "$(__hyprmux_urlencode "$exe")"
    else
        printf '\e]133;C\e\\'
    fi
}

__hyprmux_osc133_d() {
    printf '\e]133;D;%d\e\\' "$1"
}

# `PROMPT_COMMAND` (precmd-equivalent): report the previous command's exit status (if one ran),
# refresh the reported cwd, then mark the start of a new prompt.
__hyprmux_precmd() {
    local status=$?
    if [ -n "${__hyprmux_have_last_command:-}" ]; then
        __hyprmux_osc133_d "$status"
        unset __hyprmux_have_last_command
    fi
    __hyprmux_osc7
    __hyprmux_osc133_a
    return $status
}

# DEBUG trap (preexec-equivalent): fires before every simple command, including ones inside
# `PROMPT_COMMAND` itself - `__hyprmux_in_precmd` and the `$COMP_LINE`/`$COMP_POINT` completion
# guard keep it from firing for anything but a real typed command about to execute.
__hyprmux_preexec() {
    [ -n "${COMP_LINE:-}" ] && return
    [ "$BASH_COMMAND" = "__hyprmux_precmd" ] && return
    case "$PROMPT_COMMAND" in
    *"$BASH_COMMAND"*) return ;;
    esac
    __hyprmux_have_last_command=1
    __hyprmux_osc133_c "$BASH_COMMAND"
}

case "$PROMPT_COMMAND" in
*__hyprmux_precmd*) ;;
"") PROMPT_COMMAND='__hyprmux_precmd' ;;
*) PROMPT_COMMAND="__hyprmux_precmd;$PROMPT_COMMAND" ;;
esac

# `\[...\]` marks the escape sequence as zero-width for readline's prompt-length accounting.
case "$PS1" in
*'\[\e]133;B\e\\\]'*) ;;
*) PS1="${PS1}\[\e]133;B\e\\\]" ;;
esac

# Installed last so none of this script's own setup commands above spuriously trigger `C`/`D`
# events before the shell has actually reached an interactive prompt.
trap '__hyprmux_preexec' DEBUG
