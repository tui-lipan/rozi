# hyprmux shell integration for PowerShell (cross-platform plan Phase 8).
#
# Emits OSC 7 and OSC 9;9 (cwd), OSC 133 A/B/C/D (command lifecycle), and a hyprmux-namespaced
# OSC 133 `hyprmux_exe=` parameter carrying only the executable basename - never a full command
# line - so hyprmux's smart focus and pane-runtime-state tracking work without process inspection
# (which is deliberately unsupported on Windows).
#
# Loaded by hyprmux itself via `-NoExit -Command ". <this file>"`, which runs *after* the user's
# `$PROFILE`, so every prompt customization and PSReadLine setting is already in place and gets
# wrapped rather than replaced. No dotfile and no registry key is ever modified. Also safe to
# dot-source from `$PROFILE` by hand (see docs/terminal.md) for shells hyprmux did not launch;
# sourcing it twice is a no-op.
#
# Compatible with Windows PowerShell 5.1 (hence `[char]0x1b` rather than the `` `e `` escape, which
# only exists in PowerShell 6+).

if ($env:HYPRMUX_SHELL_INTEGRATION_LOADED) {
    return
}
# Not an interactive session (`-Command`/`-File`, or a hyprmux `command_shell` runner): there is no
# prompt to instrument and no user to instrument it for.
if (-not [Environment]::UserInteractive -or $null -eq $Host.UI.RawUI) {
    return
}
$env:HYPRMUX_SHELL_INTEGRATION_LOADED = "1"

$Global:__hyprmuxEsc = [char]0x1b

function Global:__hyprmux_emit([string] $Body) {
    # `Write-Host -NoNewline` writes straight to the host without disturbing the pipeline, which is
    # what a prompt function needs: returning the string would print it as prompt text.
    Write-Host -NoNewline ($Global:__hyprmuxEsc + $Body + $Global:__hyprmuxEsc + '\')
}

# Percent-encode everything outside the URI-unreserved set, matching the framework's decoder.
function Global:__hyprmux_urlencode([string] $Value, [string] $Keep = '') {
    $builder = [System.Text.StringBuilder]::new()
    foreach ($byte in [System.Text.Encoding]::UTF8.GetBytes($Value)) {
        $char = [char] $byte
        if (($char -match '[A-Za-z0-9._~-]') -or ($Keep.Length -gt 0 -and $Keep.Contains($char))) {
            [void] $builder.Append($char)
        }
        else {
            [void] $builder.AppendFormat('%{0:X2}', $byte)
        }
    }
    return $builder.ToString()
}

function Global:__hyprmux_cwd() {
    # `$PWD` can point at a non-filesystem PowerShell drive (Registry::, Cert:, ...). Those have no
    # meaningful working directory for a pane to inherit, so report nothing rather than a path that
    # would fail `Command::current_dir`.
    if ($PWD.Provider.Name -ne 'FileSystem') {
        return
    }
    $path = $PWD.ProviderPath

    # OSC 9;9 carries the native path verbatim - the form Windows terminals and hyprmux's own
    # observer prefer, with no encoding to get wrong.
    __hyprmux_emit "]9;9;$path"

    # OSC 7 carries the same directory as a `file://` URI for parity with the Unix integrations.
    # A Windows drive path becomes an absolute URI path by prefixing a slash and flipping the
    # separators: `C:\Users\x` -> `file:///C:/Users/x`.
    $uriPath = '/' + ($path -replace '\\', '/')
    $encoded = __hyprmux_urlencode $uriPath '/:'
    __hyprmux_emit "]7;file://$([System.Net.Dns]::GetHostName())$encoded"
}

# Wrap - never replace - the prompt the user's `$PROFILE` (or a prompt theme like oh-my-posh or
# Starship) already installed. The original is captured once and invoked from inside the wrapper.
if (-not $Global:__hyprmuxOriginalPrompt) {
    $Global:__hyprmuxOriginalPrompt = $function:Prompt
}

function Global:Prompt() {
    # Capture both status sources before anything else runs and clobbers them. `$?` covers cmdlets
    # and script failures; `$LASTEXITCODE` covers native executables, which is the case that
    # actually carries an interesting number.
    $succeeded = $?
    $lastExit = $LASTEXITCODE

    if ($Global:__hyprmuxRanCommand) {
        $code = if ($succeeded) { 0 } elseif ($null -ne $lastExit) { $lastExit } else { 1 }
        __hyprmux_emit "]133;D;$code"
        $Global:__hyprmuxRanCommand = $false
    }
    __hyprmux_cwd
    __hyprmux_emit ']133;A'

    $rendered = & $Global:__hyprmuxOriginalPrompt

    # `B` marks the end of the prompt and the start of the input area, so it must be the very last
    # thing emitted - hence appending it to the prompt string rather than writing it out here.
    return "$rendered$($Global:__hyprmuxEsc)]133;B$($Global:__hyprmuxEsc)\"
}

# `C` (a command is about to execute) has no hook of its own. PSReadLine routes every interactive
# line through `PSConsoleHostReadLine`, so wrapping that gives us the accepted command line at
# exactly the right moment: after the user pressed Enter, before the shell runs it.
if (Get-Module -Name PSReadLine) {
    if (-not $Global:__hyprmuxOriginalReadLine) {
        $Global:__hyprmuxOriginalReadLine = $function:PSConsoleHostReadLine
    }

    function Global:PSConsoleHostReadLine() {
        $line = & $Global:__hyprmuxOriginalReadLine

        # Only the executable's basename ever leaves this shell - never arguments, never the full
        # command line. Treat the terminal as an untrusted channel and the user's command line as
        # theirs alone.
        $exe = ($line -split '\s+', 2)[0]
        $exe = $exe.Trim('"', "'")
        if ($exe) {
            $exe = [System.IO.Path]::GetFileName($exe)
        }

        $Global:__hyprmuxRanCommand = $true
        if ($exe) {
            __hyprmux_emit "]133;C;hyprmux_exe=$(__hyprmux_urlencode $exe)"
        }
        else {
            __hyprmux_emit ']133;C'
        }
        return $line
    }
}
