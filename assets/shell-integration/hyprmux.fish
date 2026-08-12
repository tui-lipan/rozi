# hyprmux shell integration for fish (cross-platform plan Phase 8).
#
# Emits OSC 7 (cwd), OSC 133 A/B/C/D (command lifecycle), and a hyprmux-namespaced OSC 133
# `rozi_exe=` parameter carrying only the executable basename - never a full command line - so
# hyprmux's smart-focus and pane-runtime-state tracking work without polling `/proc`. Installed by
# hyprmux as a vendor `conf.d` file discovered via `XDG_DATA_DIRS`; never edits the user's real
# fish config. Fish supports multiple independent handlers per event (unlike bash/zsh's single
# `PROMPT_COMMAND`/`precmd_functions` slot), so this composes with fish's own built-in terminal
# integration and any other `fish_prompt`/`fish_preexec` handlers without stepping on them.
#
# **Cross-compile-equivalent status**: written per fish's documented event-handler contract and
# manually reviewed, but not executed against a real fish - none is installed in this environment.
# The sibling `hyprmux.bash` script implements the identical protocol and was exercised against a
# real bash; treat this file as unverified-by-execution until a fish host runs it.

if set -q ROZI_SHELL_INTEGRATION_LOADED
    return 0
end
if not status is-interactive
    return 0
end
set -gx ROZI_SHELL_INTEGRATION_LOADED 1

# Percent-encode everything outside the URI-unreserved set, matching the framework's decoder.
function __hyprmux_urlencode
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
function __hyprmux_urlencode_path
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

function __hyprmux_osc7 --on-event fish_prompt
    set -l host (hostname 2>/dev/null; or echo "")
    printf '\e]7;file://%s%s\e\\' (__hyprmux_urlencode $host) (__hyprmux_urlencode_path $PWD)
end

function __hyprmux_osc133_a --on-event fish_prompt
    printf '\e]133;A\e\\'
end

function __hyprmux_osc133_d --on-event fish_prompt
    if set -q __hyprmux_have_last_command
        printf '\e]133;D;%d\e\\' $status
        set -e __hyprmux_have_last_command
    end
end

function __hyprmux_osc133_c --on-event fish_preexec
    set -g __hyprmux_have_last_command 1
    set -l cmdline $argv[1]
    set -l exe (string split -m1 ' ' -- $cmdline)[1]
    set -l exe (path basename -- $exe 2>/dev/null; or basename -- $exe)
    if test -n "$exe"
        printf '\e]133;C;rozi_exe=%s\e\\' (__hyprmux_urlencode $exe)
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
    functions -c fish_prompt __hyprmux_original_fish_prompt
end
function fish_prompt
    if functions -q __hyprmux_original_fish_prompt
        __hyprmux_original_fish_prompt
    end
    printf '\e]133;B\e\\'
end
