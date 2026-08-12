# rozi shell integration for bash (cross-platform plan Phase 8).
#
# Emits OSC 7 (cwd), OSC 133 A/B/C/D (command lifecycle), and a rozi-namespaced OSC 133
# `rozi_exe=` parameter carrying only the executable basename - never a full command line - so
# rozi's smart-focus and pane-runtime-state tracking work without polling `/proc`. Installed by
# rozi itself via `bash --rcfile <generated wrapper>`; never edits `~/.bashrc` or any other
# dotfile. Safe to source more than once (idempotent) and a no-op in a non-interactive shell.

if [ -n "${ROZI_SHELL_INTEGRATION_LOADED:-}" ] || [ -z "${PS1:-}" ]; then
    return 0 2>/dev/null || exit 0
fi
ROZI_SHELL_INTEGRATION_LOADED=1

# Percent-encode everything outside the URI-unreserved set, matching the framework's decoder.
__rozi_urlencode() {
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
__rozi_urlencode_path() {
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

__rozi_osc7() {
    local host
    host=$(hostname 2>/dev/null || printf '%s' "${HOSTNAME:-}")
    printf '\e]7;file://%s%s\e\\' "$(__rozi_urlencode "$host")" "$(__rozi_urlencode_path "$PWD")"
}

# `B` (end of prompt) is embedded directly in PS1 (see below) rather than emitted from a function,
# since it must be the very last thing printed before the input area for the terminal to attribute
# the boundary correctly.
__rozi_osc133_a() { printf '\e]133;A\e\\'; }

__rozi_osc133_c() {
    local cmdline="$1" exe
    exe="${cmdline%% *}"
    exe="${exe##*/}"
    if [ -n "$exe" ]; then
        printf '\e]133;C;rozi_exe=%s\e\\' "$(__rozi_urlencode "$exe")"
    else
        printf '\e]133;C\e\\'
    fi
}

__rozi_osc133_d() {
    printf '\e]133;D;%d\e\\' "$1"
}

# `PROMPT_COMMAND` (precmd-equivalent): report the previous command's exit status (if one ran),
# refresh the reported cwd, then mark the start of a new prompt.
__rozi_precmd() {
    local status=$?
    if [ -n "${__rozi_have_last_command:-}" ]; then
        __rozi_osc133_d "$status"
        unset __rozi_have_last_command
    fi
    __rozi_osc7
    __rozi_osc133_a
    return $status
}

# Armed as the *last* prompt-pipeline step (see the `PROMPT_COMMAND` install below) and consumed
# by the first DEBUG trap firing afterwards, i.e. by whatever bash runs once readline returns.
# Preserves `$?` like `__rozi_precmd` so a plain `$?` in `PS1` still shows the real status.
__rozi_arm() {
    local status=$?
    __rozi_at_prompt=1
    return $status
}

# DEBUG trap (preexec-equivalent): fires before every simple command, including every command run
# by other `PROMPT_COMMAND` hooks (zoxide, starship, ...) and by function bodies. Only the first
# firing after `__rozi_arm` armed the prompt is a genuinely typed command; matching
# `$BASH_COMMAND` against `$PROMPT_COMMAND` instead (the previous guard) misses hooks held in the
# bash >= 5.1 `PROMPT_COMMAND` *array* and every command inside a hook's function body, which left
# stray `C;rozi_exe=<hook>` reports after each prompt.
__rozi_preexec() {
    [ -n "${COMP_LINE:-}" ] && return
    [ -z "${__rozi_at_prompt:-}" ] && return
    [ "${BASH_SUBSHELL:-0}" -eq 0 ] && __rozi_at_prompt=""
    # An empty Enter re-runs the prompt pipeline while still armed; our own precmd is its first
    # command, so seeing it means nothing was typed.
    case "$BASH_COMMAND" in
    __rozi_precmd | __rozi_arm) return ;;
    esac
    __rozi_have_last_command=1
    __rozi_osc133_c "$BASH_COMMAND"
}

# `PROMPT_COMMAND` may be a plain string or an array (bash >= 5.1; starship/zoxide/bash-preexec
# append array elements). Install `__rozi_precmd` first and `__rozi_arm` last, after every
# other hook, so anything the DEBUG trap sees in between is prompt machinery, never a typed
# command. `${PROMPT_COMMAND[*]}` reads the whole array (and degrades to the string form).
case "${PROMPT_COMMAND[*]:-}" in
*__rozi_precmd*) ;;
*)
    if [[ "$(declare -p PROMPT_COMMAND 2>/dev/null)" == "declare -a"* ]]; then
        PROMPT_COMMAND=(__rozi_precmd "${PROMPT_COMMAND[@]}" __rozi_arm)
    elif [ -z "${PROMPT_COMMAND:-}" ]; then
        PROMPT_COMMAND='__rozi_precmd;__rozi_arm'
    else
        PROMPT_COMMAND="__rozi_precmd;$PROMPT_COMMAND;__rozi_arm"
    fi
    ;;
esac

# `\[...\]` marks the escape sequence as zero-width for readline's prompt-length accounting.
case "$PS1" in
*'\[\e]133;B\e\\\]'*) ;;
*) PS1="${PS1}\[\e]133;B\e\\\]" ;;
esac

# Installed last so none of this script's own setup commands above spuriously trigger `C`/`D`
# events before the shell has actually reached an interactive prompt.
trap '__rozi_preexec' DEBUG
