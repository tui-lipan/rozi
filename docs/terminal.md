# Terminal features

Every pane is a PTY owned by a session server. Programs receive normal terminal input, output,
resize, mouse, title, clipboard, and shell-metadata sequences. A client can detach while a named
server keeps its PTYs running. See [Sessions](sessions.md).

## Choose the shell and starting directory

The `[shell]` and `cwd` settings choose the interactive shell and fallback working directory. When
they are unset, Rozi uses the platform's default shell and the directory where it was launched.

Remote panes use the remote server's shell and filesystem, not the client's `[shell]` value. See
[Remote sessions](remote.md#understand-the-client-and-server-boundary).

## Working directories and shell metadata

Rozi uses shell metadata for pane titles, new-pane directory inheritance, prompt navigation, last
command output, and foreground-program detection.

With `[shell_integration] mode = "auto"`, Rozi configures bash, zsh, fish, and PowerShell for the
pane process without editing shell startup files. It does not edit the Windows `AutoRun` registry
key. `cmd.exe` can report its directory and prompt boundaries but cannot report a command before it
runs.

Directory lookup follows this order:

1. A valid local OSC 7 or OSC 9;9 directory report.
2. Native process inspection on Linux or macOS.
3. The pane's launch directory.
4. Configured `cwd`.

Windows has no native process-inspection fallback. Shell integration is therefore needed for live
directory and foreground-program updates there.

A directory report for another host may be shown as metadata but is not used as a local spawn path.
A new local pane never tries to start in a path that belongs to an SSH host.

See [Configuration](configuration.md#shell_integration) for modes and per-shell behavior.

## Use the mouse

Without a pending prefix or the WM modifier, mouse events go to the program in the pane. This keeps
mouse-aware editors and TUIs working normally.

| Gesture | Action |
| --- | --- |
| Drag over terminal text | Select text |
| Wheel over a pane | Scroll history |
| `Ctrl` plus click a visible link | Open it |
| WM modifier plus left-drag | Move a pane |
| WM modifier plus right-drag | Resize a pane |
| Prefix, then left-drag | Move a pane |
| Prefix, then right-drag | Resize a pane |
| Drag a tiled split boundary | Resize the split |

Rozi limits forwarded pointer-motion events to the configured frame rate so a mouse-tracking
program does not build an input backlog. Presses, releases, and wheel events are not coalesced.

## Select, copy, and paste

Drag to select text, then press `Ctrl+C` to copy it. Selection anchors remain attached to scrollback
lines while you scroll.

The `v` command key and direct `Ctrl+V` send text from the system clipboard with bracketed-paste
markers. Direct `Ctrl+V` passes through when the clipboard contains a non-text format, allowing a
pane program to handle it. Prefix, modifier, and palette paste commands remain text-only.

Programs can write to the system clipboard with OSC52 when
`[clipboard].enable_osc52 = true`, the default. Disable it if pane programs should not control the
clipboard. Restart Rozi after changing this setting.

For a remote attachment, OSC52 targets the local client's clipboard. A pane program that directly
opens a clipboard API sees the remote host's clipboard.

## Copy, search, and hints

Press the `[` command key to enter copy mode. It provides keyboard cursor movement, selection,
prompt jumps, and last-output copying. Press `/` to search the focused pane from copy mode.
The complete local key table is in [Keybindings](keybindings.md#copy-mode).

Press the `/` command key outside copy mode to search retained history. `Tab` changes the scope
among the focused pane, workspace, and all panes. `Ctrl+N` and `Ctrl+P` move among results. Search
retains at most 2000 matches for navigation and marks the count with `+` only when more exist.

ASCII letters match without case. Other text remains case-sensitive. New pane output restarts an
open search so result positions remain valid.

Press the `u` command key for hint mode. It recognizes visible URLs, paths with optional line
numbers, Git commit ids, and configured `[[hints]]` patterns. A lowercase label copies the target.
An uppercase final label character opens eligible targets. Soft-wrapped targets are rejoined before
matching.

**Copy last command output** and copy-mode prompt jumps require shell-integration prompt markers.
`rozi capture-pane --last-output` provides the same last-output capture for automation.

## Open links

Hold `Ctrl` over a visible URL and click it to open with the system handler. Explicit OSC 8 links
also work and take precedence over plain-text URL detection. Unsupported destinations produce an
error instead of being passed to the operating system.

## Scrollback

Each terminal screen retains the configured `scrollback` number of lines, which defaults to 5000.
Typing returns the view to live output.

The server retains history for reattachment, and each attached client keeps its own screen. Memory
use grows with pane width, populated history, and attached-client count. Use a smaller scrollback
limit when memory matters more than deep history.

The limit is set when a terminal screen is created. Reloading config changes new screens, not
existing ones. Restart a named server to rebuild its retained screens with a new limit.

**Edit scrollback** writes the focused pane's retained text to a private file under the state
directory and opens it with `$EDITOR`, then `$VISUAL`, then `vi`. Rozi keeps the directory near 20
files. Scrollback files may contain passwords, tokens, and private output.

## Titles and urgency

Programs set terminal titles with OSC 0 or OSC 2. A custom title set with the `Shift+N` command key
takes precedence. See [Layouts and panes](layouts-and-panes.md#titles-and-exited-panes).

With `[notifications] bell = true`, BEL from an unattended pane marks its workspace. A pane is
attended only when its window and the pane itself are focused.

## Images

Pane programs can use the Kitty graphics protocol. Rozi displays images through a format supported
by the host terminal, including Kitty, iTerm2, sixel, or text-cell fallback.

Images follow terminal scrolling and alternate-screen lifetime. These limits apply:

- Images drawn before a client attaches are not replayed.
- Kitty protocol animation frames are not supported. Programs that redraw an image can still
  animate.
- Decoded pixels are capped per pane and old images are evicted when needed.
- Remote panes cannot use a server-side file path as an image handoff to the local client, so they
  use inline image data.
- Temporary-file and shared-memory handoff forms that can only be consumed once are refused in a
  session that may have several clients.

## Pane logging

Run **Pane logging** from the command palette, or bind `toggle-pane-logging`, to append raw PTY
output from the focused pane to a log file. The titlebar shows a `log` badge while logging.

Logging belongs to the server, so all clients see its state. It stops after a write error or after
reaching `[logging] max_bytes`.

Logs keep escape sequences and carriage returns. Each logging run begins with a header containing
the session, pane, generation, size, and start time. Raw logs can contain credentials and private
terminal output. Store and share them accordingly.

## Persistence boundaries

- Named sessions retain live PTYs while their server runs.
- Profiles restore layout and launch intent with fresh PTYs.
- Resurrection restarts commands and replays saved text history.
- Scratch panes live on a private client-lifetime server and are not saved or shared.
- Images drawn before attach are not reconstructed from text replay.

See [Profiles](profiles.md) and [Shared sessions](shared-sessions.md) for those workflows.
