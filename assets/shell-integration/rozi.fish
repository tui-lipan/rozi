# rozi shell integration for fish (cross-platform plan Phase 8).
#
# Emits OSC 7 (cwd), OSC 133 A/B/C/D (command lifecycle), and a rozi-namespaced OSC 133
# `rozi_exe=` parameter carrying only the executable basename - never a full command line - so
# rozi's smart-focus and pane-runtime-state tracking work without polling `/proc`. Installed by
# rozi as a vendor `conf.d` file discovered via `XDG_DATA_DIRS`; never edits the user's real
# fish config. Fish supports multiple independent handlers per event (unlike bash/zsh's single
# `PROMPT_COMMAND`/`precmd_functions` slot), so this composes with fish's own built-in terminal
# integration and any other `fish_prompt`/`fish_preexec` handlers without stepping on them.
#
# **Cross-compile-equivalent status**: written per fish's documented event-handler contract and
# manually reviewed, but not executed against a real fish - none is installed in this environment.
# The sibling `rozi.bash` script implements the identical protocol and was exercised against a
# real bash; treat this file as unverified-by-execution until a fish host runs it.

if set -q ROZI_SHELL_INTEGRATION_LOADED
    return 0
end
if not status is-interactive
    return 0
end
set -gx ROZI_SHELL_INTEGRATION_LOADED 1

# Percent-encode everything outside the URI-unreserved set, matching the framework's decoder.
function __rozi_urlencode
    set -l input $argv[1]
    set -l output ""
    for c in (string split '' -- $input)
        if string match -qr '^[a-zA-Z0-9.~_-]$' -- $c
            set output "$output$c"
        else
            set output (printf '%s%%%02X' $output (printf '%d' \'$c))
        end
    end
    printf '%s' $output
end

# Same as above but keeps `/` literal - it is a reserved-but-permitted path separator in a
# `file://` URI, not something the framework's decoder expects escaped.
function __rozi_urlencode_path
    set -l input $argv[1]
    set -l output ""
    for c in (string split '' -- $input)
        if string match -qr '^[a-zA-Z0-9._~/-]$' -- $c
            set output "$output$c"
        else
            set output (printf '%s%%%02X' $output (printf '%d' \'$c))
        end
    end
    printf '%s' $output
end

# Resolved and encoded once, not per prompt - see `rozi.bash` for why this must be a snapshot
# rather than a live lookup. `$hostname` is fish's own start-time global; the fork remains only as
# a fallback for a shell that cleared it.
set -l __rozi_host $hostname
if test -z "$__rozi_host"
    set __rozi_host (hostname 2>/dev/null; or echo "")
end
set -g __rozi_host_encoded (__rozi_urlencode "$__rozi_host")

function __rozi_osc7
    printf '\e]7;file://%s%s\e\\' "$__rozi_host_encoded" (__rozi_urlencode_path $PWD)
end

function __rozi_prompt --on-event fish_prompt
    set -l command_status $status
    if set -q __rozi_have_last_command
        # Recover from a foreground TUI that exited without restoring the terminal before drawing
        # the prompt. Fish can re-enable any modes its line editor needs afterwards.
        printf '\e]133;D;%d\e\\\e[?1049l\e[?1000l\e[?1002l\e[?1003l\e[?1004l\e[?1005l\e[?1006l\e[?2004l\e[?1l\e>' $command_status
        set -e __rozi_have_last_command
    end
    __rozi_osc7
    printf '\e]133;A\e\\'
end

function __rozi_osc133_c --on-event fish_preexec
    set -g __rozi_have_last_command 1
    set -l cmdline $argv[1]
    set -l exe (string split -m1 ' ' -- $cmdline)[1]
    set -l exe (path basename -- $exe 2>/dev/null; or basename -- $exe)
    if test -n "$exe"
        printf '\e]133;C;rozi_exe=%s\e\\' (__rozi_urlencode $exe)
    else
        printf '\e]133;C\e\\'
    end
end

# `fish_prompt` (the *event*) fires before fish calls the `fish_prompt` *function* that actually
# prints the prompt text, so `A`/`cwd`/`D` above land correctly before the visible prompt - but `B`
# (end of prompt, start of input) must land *after* the printed prompt, and fish has no dedicated
# post-prompt event for that. The documented technique (also used by prompt frameworks like
# Starship) is to wrap the `fish_prompt` function itself: keep whatever prompt was already defined
# (fish ships a built-in default, and this runs from `vendor_conf.d` which loads early) and append
# the marker after it runs.
#
# Load-order caveat: if a prompt framework's own conf.d file defines `fish_prompt` *after* this one
# loads (common for plugin-manager-installed themes), it will silently replace this wrapper and
# `B` will stop being emitted; `A`/`cwd`/`D`/`C` still emit unaffected either way. Not something a
# vendor `conf.d` file can fully control - fish interleaves all `conf.d` files (user, system,
# vendor) into one alphabetically sorted load order.
if functions -q fish_prompt
    functions -c fish_prompt __rozi_original_fish_prompt
end
function fish_prompt
    if functions -q __rozi_original_fish_prompt
        __rozi_original_fish_prompt
    end
    printf '\e]133;B\e\\'
end
