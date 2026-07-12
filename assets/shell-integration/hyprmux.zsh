# hyprmux shell integration for zsh (cross-platform plan Phase 8).
#
# Emits OSC 7 (cwd), OSC 133 A/B/C/D (command lifecycle), and a hyprmux-namespaced OSC 133
# `hyprmux_exe=` parameter carrying only the executable basename - never a full command line - so
# hyprmux's smart-focus and pane-runtime-state tracking work without polling `/proc`. Installed by
# hyprmux via a `ZDOTDIR` shim directory (see the generated `.zshrc` alongside this file); never
# edits the user's real `ZDOTDIR`/`~/.zshrc`. Uses `add-zsh-hook` so it composes with any other
# `precmd`/`preexec` hooks (Starship, Powerlevel10k, etc.) instead of overwriting them.
#
# **Cross-compile-equivalent status**: written per zsh's documented `add-zsh-hook`/prompt-escape
# contract and manually reviewed, but not executed against a real zsh - none is installed in this
# environment. The sibling `hyprmux.bash` script implements the identical protocol and was
# exercised against a real bash; treat this file as unverified-by-execution until a zsh host runs
# it (Milestone 1's macOS CI, once the workflow actually runs, is the first real exercise of it).

[[ -n "$HYPRMUX_SHELL_INTEGRATION_LOADED" ]] && return 0
[[ -o interactive ]] || return 0
HYPRMUX_SHELL_INTEGRATION_LOADED=1

autoload -Uz add-zsh-hook

# Percent-encode everything outside the URI-unreserved set, matching the framework's decoder.
# zsh string subscripting is 1-based and `$c[1]`-style indexing yields whole characters (not
# bytes), unlike bash's byte-oriented `${s:i:1}`.
__hyprmux_urlencode() {
    local input="$1" output="" c i
    for ((i = 1; i <= ${#input}; i++)); do
        c="${input[i]}"
        case "$c" in
        [a-zA-Z0-9.~_-]) output+="$c" ;;
        *) output+=$(printf '%%%02X' "'$c") ;;
        esac
    done
    print -rn -- "$output"
}

# Same as above but keeps `/` literal - it is a reserved-but-permitted path separator in a
# `file://` URI, not something the framework's decoder expects escaped.
__hyprmux_urlencode_path() {
    local input="$1" output="" c i
    for ((i = 1; i <= ${#input}; i++)); do
        c="${input[i]}"
        case "$c" in
        [a-zA-Z0-9._~/-]) output+="$c" ;;
        *) output+=$(printf '%%%02X' "'$c") ;;
        esac
    done
    print -rn -- "$output"
}

__hyprmux_osc7() {
    local host
    host="${HOST:-$(hostname 2>/dev/null)}"
    printf '\e]7;file://%s%s\e\\' "$(__hyprmux_urlencode "$host")" "$(__hyprmux_urlencode_path "$PWD")"
}

__hyprmux_precmd() {
    local status=$?
    if [[ -n "$__hyprmux_have_last_command" ]]; then
        printf '\e]133;D;%d\e\\' "$status"
        unset __hyprmux_have_last_command
    fi
    __hyprmux_osc7
    printf '\e]133;A\e\\'
}

__hyprmux_preexec() {
    local cmdline="$1" exe
    exe="${cmdline%% *}"
    exe="${exe:t}"
    __hyprmux_have_last_command=1
    if [[ -n "$exe" ]]; then
        printf '\e]133;C;hyprmux_exe=%s\e\\' "$(__hyprmux_urlencode "$exe")"
    else
        printf '\e]133;C\e\\'
    fi
}

add-zsh-hook precmd __hyprmux_precmd
add-zsh-hook preexec __hyprmux_preexec

# `%{...%}` marks the escape sequence as zero-width for zsh's prompt-length accounting. Appended
# once, at load time, to whatever `PROMPT`/`PS1` the user's real config already set, so it is
# always the last thing printed before input reading begins.
PROMPT="${PROMPT}%{$(printf '\e]133;B\e\\')%}"
