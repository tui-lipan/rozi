# Terminal features

This page covers how panes behave as terminals: the shell they start, working directories, mouse and
clipboard handling, copy mode and search, links, scrollback, images, and pane logging.

Programs in a pane work as they would in any terminal, including mouse input, titles, clipboard
access, and resizing. They see rozi as their terminal, whichever terminal rozi itself runs in: each
pane sets `TERM=xterm-256color`, `COLORTERM=truecolor`, `TERM_PROGRAM=rozi`, and
`TERM_PROGRAM_VERSION` to the version of rozi running the session, and a program that asks the
terminal for its name and version gets `rozi` and that same version. Each pane runs on the session
server, so it keeps running in a named session after you detach. See [Core concepts](core-concepts.md#sessions-and-clients) and
[Sessions](sessions.md).

## Choose the shell and starting directory

The `[shell]` setting chooses the interactive shell, and `cwd` sets the fallback working directory.
When they are unset, rozi uses the platform's default shell and the directory it was launched from.

Remote panes use the remote server's shell and filesystem, not the client's `[shell]` value. See
[Remote sessions](remote.md#understand-the-client-and-server-boundary).

## Working directories and shell metadata

Shell integration lets rozi know each pane's directory, prompts, and running command. rozi uses this
for pane titles, opening new panes in the same directory, prompt navigation, copying the last
command's output, and detecting the program in the foreground.

With `[shell_integration] mode = "auto"`, the default, rozi sets up bash, zsh, fish, and PowerShell
for each pane without editing your shell startup files. It does not edit the Windows `AutoRun`
registry key. `cmd.exe` can report its directory and prompt boundaries, but not the command it is
about to run.

rozi finds a pane's working directory from the first of these that is available:

1. A valid local directory report from the shell (OSC 7 or OSC 9;9).
2. Process inspection, on Linux and macOS.
3. The pane's launch directory.
4. The configured `cwd`.

Windows has no process inspection, so live directory and foreground-program updates there need shell
integration.

A directory reported for another host, such as inside an SSH session, can appear in titles, but
rozi never starts a new local pane in it.

See [Configuration](configuration.md#shell-integration) for modes and per-shell behavior.

## Use the mouse

Mouse events go to the program in the pane unless you hold the modifier (`Alt` by default) or have
pressed the prefix. Mouse-aware editors and other terminal apps keep working normally.

| Gesture | Action |
| --- | --- |
| Drag over terminal text | Select text and copy it on release |
| Middle click (Linux) | Paste the primary selection into the pane under the pointer |
| Wheel over a pane | Scroll history |
| `Ctrl` plus click a visible link | Open it |
| Modifier plus left-drag | Move a pane |
| Modifier plus right-drag | Resize a pane |
| Prefix, then left-drag | Move a pane |
| Prefix, then right-drag | Resize a pane |
| Drag a tiled split boundary | Resize the split |

When you drag after pressing the prefix, the which-key strip hides during the drag. The `PREFIX`
badge stays until you release the button, which also ends the prefix.

The wheel scrolls history, sidebar tabs, and pickers three lines for every scroll your terminal
sends. Change the multiplier with **Scroll multiplier** in
[Settings](customize.md#use-the-settings-picker) or `scroll_multiplier` in
[`[input]`](configuration.md#input). It stacks with your terminal's own scroll multiplier: if
Ghostty is set to `3` and rozi to `3`, one notch scrolls nine lines, so lower one of them if
scrolling is too fast. A program that tracks the mouse, such as an editor, receives the wheel
itself and scrolls by its own step.

rozi limits pointer-motion events sent to a mouse-tracking program to the configured frame rate, so
the program does not fall behind. Presses, releases, and wheel events are always sent.

## Select, copy, and paste

Drag to select text. When you release the mouse, rozi copies the selection to the regular clipboard,
and on Linux also to the primary selection. The selection stays visible after copying and stays on
the same lines while you scroll. `Ctrl+C` also copies the active selection to the regular clipboard.

On Linux, middle click pastes the primary selection into the pane under the pointer, not the
focused pane. Clicking another pane focuses it first; a click on a pane border or titlebar pastes
nothing.

Right click does nothing clipboard-related by default, so it stays available to the pane program and
to rozi. If you set `right_click`, it follows the same pointer rule as middle click, and
`"copy-or-paste"` copies only a selection in the pane you click.

You can change all of these under [`[clipboard]`](configuration.md#clipboard); changes apply when
the config reloads. If the system has no primary selection, regular clipboard copying still works,
and primary-only gestures are turned off.

The `v` command key and `Ctrl+V` paste text from the system clipboard as a bracketed paste.
In a local session, direct `Ctrl+V` passes non-text formats to the pane program;
the prefix and command-palette paste commands read text.

When your local rozi window is attached to a remote session, `Ctrl+V` and the
paste command also accept clipboard images and copied files. rozi uploads them
and pastes their remote paths without submitting the prompt. Codex and Claude
Code can use the image paths as attachments. Other files arrive as paths for the
pane program to interpret. An empty clipboard keeps direct `Ctrl+V` available
to the program. If the connection is unavailable, pasting reports an error;
reconnect and paste again.

Copy up to 16 regular files at once from a file manager. Each file may be up to
16 MiB; clipboard images may be up to 10,000,000 bytes after PNG encoding.
Directories and symbolic links are rejected. A single absolute PNG, JPEG, GIF,
WebP, or BMP path emitted by the terminal is also uploaded when it names a
local file. Text read by rozi's clipboard shortcut or paste command stays literal,
even if it names a local image. Paths absent from your computer also stay text.
A terminal can emit identical text for its own paste gestures and file drops;
use rozi's `Ctrl+V` or paste command to keep a copied path literal. If the terminal
consumes the shortcut before rozi receives it, use the `v` command key or the
paste command in the command palette.

Uploaded files survive detach and reconnect while the session server runs.
They are removed when that server stops and are not restored with a saved
session. A session can retain up to 256 files or 256 MiB of uploads. Reaching a
limit reports an error. Files copied from your computer remain unchanged.

This transfer works for panes in managed remote sessions. An ordinary `ssh`
command inside a local pane, another SSH hop inside a remote pane, and programs
inside containers need their own file transfer or shared filesystem.

Programs can set the system clipboard through OSC 52 while `[clipboard].enable_osc52 = true`, the
default. Turn it off if pane programs should not control your clipboard. The change applies when the
config reloads.

When attached to a remote session, mouse selection and OSC 52 use your local clipboard. A pane
program that calls a clipboard API directly uses the remote host's clipboard.

## Copy, search, and hints

### Copy mode

Press `Ctrl+A`, then `[` to enter copy mode. It lets you move a cursor with the keyboard, select
text, jump between shell prompts, and copy the last command's output. Press `/` in copy mode to
search the focused pane. See [Keybindings](keybindings.md#copy-mode) for every key.

### Search scrollback

Outside copy mode, the `/` command key searches the retained history.

- `Tab` switches the scope between the focused pane, the workspace, and all panes.
- The arrow and paging keys move through results.
- Workspace results are grouped by pane. All-pane results also name the workspace. Panes are
  numbered in display order.
- Search keeps up to 2000 matches. The count shows `+` when there are more.

ASCII letters match regardless of case; other characters are case-sensitive. Lines that wrapped only
because of the pane width are joined before matching, so they count as one line and one result.
When the pane prints new output, an open search runs again so its results stay accurate.

### Hint mode

The `u` command key starts hint mode. It labels visible URLs, links that programs mark explicitly
(OSC 8 hyperlinks), paths with optional line numbers, Git commit ids, and your `[[hints]]` patterns,
including ones that wrap across lines. Type a lowercase label to copy the target, or end the label
with an uppercase character to open it when it can be opened. After each key, labels show only the
keys left to press. URL hints use the same handlers as clicked links. A configured `file` handler
enables path hints; custom patterns can define `on_open` commands. See
[Link handlers](configuration.md#links-handlers).

What counts as a target:

- **Links** copy and open their destination, not their visible text, so `PR #200` linked to a pull
  request gives the pull request URL.
- **URLs** end at whitespace, quotes, backticks, and angle brackets. Trailing sentence punctuation
  and closing brackets are left out.
- **Paths** start with `./`, `../`, `~/`, `/`, or a drive letter, or are relative paths such as
  `src/main.rs` whose file name has an extension. A path starting with `/` needs a second `/` or an
  extension, so `/etc/hosts` is a path and a slash command such as `/status` is not. Neither is a
  bare `and/or` or `feature/branch`.
- **Git commit ids** are 7 to 40 hex digits containing both a digit and a letter.

A URL or path containing `…` is skipped: the program shortened it, and what remains is not the real
target.

### Copy the last command output

**Copy last command output** and the prompt jumps in copy mode need shell integration; see
[Working directories and shell metadata](#working-directories-and-shell-metadata). For scripts,
`rozi capture-pane --last-output` captures the same output.

## Open links

Hold `Ctrl` and click a visible URL to open it with a [configured link handler](configuration.md#links-handlers),
or the system handler when none matches. Links that programs mark
explicitly (OSC 8 hyperlinks) also work, and take precedence over URLs detected in plain text.
File URLs and other schemes need a configured handler. rozi shows an error for unsupported destinations.

## Scrollback

Each pane keeps the last `scrollback` lines of output, 5000 by default. Typing returns the view to
live output.

The session keeps each pane's history so you can reattach. Every attached client uses that pane's
history limit, even when its own `scrollback` setting differs.
Memory use grows with pane width, the amount of history, and the number of attached clients. Lower
`scrollback` if memory matters more than deep history.

The limit is fixed when a pane is created. After a config reload, the session host's
`scrollback` value applies to new panes; existing panes keep their old limit, including after
reconnect or resurrection. Two panes in one session can have different limits.
Open replacement panes to use the new limit.

**Edit scrollback** saves the focused pane's history to a private file in the state directory and
opens it in `$EDITOR`, then `$VISUAL`, then `vi`. rozi keeps about the 20 most recent files. These
files may contain passwords, tokens, and other private output.

## Titles and urgency

Programs set the pane title with OSC 0 or OSC 2. A custom title set with the `N` command key takes
precedence. See [Layouts and panes](layouts-and-panes.md#titles-and-exited-panes).

rozi also titles your terminal window, so window switchers and status bars can tell rozi windows
apart. By default the title reads `rozi · <session> · <focused pane>`. A temporary session has no
name, so its title reads `rozi · <focused pane>`. An agent's working spinner is left out. Set the
format, or turn it off, with [`window_title`](configuration.md#top-level-keys).

With `[notifications] bell = true`, a bell from a pane you are not watching marks its workspace. You
are watching a pane only when both the terminal window and the pane have focus. A bell from a pane
running an agent does not mark it: the agent's own finished or blocked mark covers it. A pane with
only a `rozi status` activity is not running an agent, so its bell still marks it.

Switching to a workspace preserves its panes' attention marks, including finished-agent marks,
even when a marked pane is selected automatically. Click or explicitly focus the pane, scroll
through it, type into it, or paste into it to clear its marks. Scrolling also clears marks when the
pane's program handles the mouse wheel.

## Images

Pane programs can show images with the Kitty graphics protocol. rozi displays them in whatever
format the host terminal supports, including Kitty, iTerm2, sixel, or a text-cell fallback. Images
scroll with the terminal and belong to the screen (main or alternate) they were drawn on.

Limits:

- Kitty protocol animation frames are not supported. Programs that redraw an image can still
  animate it.
- Each pane keeps up to 200 MiB of image data, and evicts the oldest images beyond that. A client
  that attaches to a running session receives the images still on screen.
- A single image upload is limited to 64 MiB; larger images must be sent in chunks.
- Programs can also hand images over in regular files. Images keep updating when someone attaches
  over SSH or disconnects. Temporary-file and shared-memory transfers are not supported.

If images draw incorrectly in a Kitty-protocol terminal such as Ghostty, start rozi with
`TUI_LIPAN_TERMINAL_IMAGES=composed`. rozi then draws every image itself instead of handing
placements to the terminal. If image-heavy panes are slow while you resize the window, set
`TUI_LIPAN_IMAGE_RESIZE_PAUSE_MS=180` so rozi waits for the resize to settle before it redraws them.

## Take a screenshot

To save a PNG of what rozi shows, run one of these from the command palette, or bind its id under
`[keys]`:

| Command | Id | Saves |
| --- | --- | --- |
| **Screenshot pane** | `screenshot-pane` | The focused pane's visible screen, without its border or title. |
| **Screenshot UI** | `screenshot-ui` | The whole window as drawn: bar, sidebar, borders, titles, and every visible pane. |

**Screenshot UI** leaves out the command palette that ran it. The **Screenshot saved** toast shows
the file path beneath its title. Left-click to open the PNG in your default image viewer, or
right-click to copy its full path. The pane or window flashes briefly in the theme's accent color.
The flash comes after the picture is taken,
so it never appears in the file. With `[animations] enabled = false` or `focus_chrome = false`, only
the toast appears.

Files go into `[capture] dir`, which defaults to `captures` in the state directory, and are named
`rozi-pane-<ID>-<YYYYMMDD-HHMMSS>.png` or `rozi-ui-<YYYYMMDD-HHMMSS>.png` in local time. A second
screenshot in the same second gets `-2`, `-3`, and so on; an existing file is never replaced.
`[capture] scale` (1 to 3) enlarges the image. A screenshot is always written on the machine
running this rozi window, even when the session is on another host.

Screenshots use the same drawing as `rozi capture-pane --render png` and
`rozi capture-ui --render png`, which scripts should use instead; see
[Capturing the whole UI](control.md#capturing-the-whole-ui). Those commands never flash.

## Pane logging

To save a pane's raw output to a file, run **Pane logging** from the command palette or bind
`toggle-pane-logging`. The titlebar shows a `log` badge while logging is on, and every attached
client sees the same state. When logging starts, a toast shows the log path; right-click it to copy
the full path. The file is on the session server's host.

Logging stops after a write error or when the file reaches `[logging] max_bytes` (64 MiB by default;
`0` means no limit). Logs are written to `[logging] dir`, which defaults to `logs` in the state
directory.

Logs keep escape sequences and carriage returns. Each logging run starts with a header that records
the session, pane, generation, size, and start time. Raw logs can contain credentials and private
output, so store and share them with care.

## Persistence boundaries

- Named sessions keep their panes running while their server runs.
- Profiles restore the layout and launch commands in fresh panes.
- Resurrection restarts commands and replays saved terminal history, including retained Kitty
  images.
- Scratch panes belong to one client and are never saved or shared.
- Retained Kitty images are restored when you attach to a live session.

See [Profiles](profiles.md) and [Shared sessions](shared-sessions.md) for those workflows.

## Program status

A program can report its progress through
[OSC 7501](https://www.superlogical.com/rex/docs/build/program-status), and rozi shows it in the
[Activity](sidebar.md#activity) sidebar tab. For example, this reports a build at 40 percent:

```sh
printf '\033]7501;state=working:app=cargo:progress=40\033\\'
```

| State | Meaning |
| --- | --- |
| `working` | The program is busy, optionally with a percentage. |
| `blocked` | The program needs your input. |
| `done` | The work finished. |
| `error` | The work failed. |

Messages and titles are base64-encoded UTF-8. The Activity tab shows one row per pane: if a pane
reports several things, the most urgent wins, in the order blocked, error, working, done, idle.
Selecting the row focuses the pane.

A report lasts until the program replaces it. A new prompt or the program exiting clears `working`
and `blocked`; `done` and `error` stay until the program reports again or the terminal is reset.
Reports keep arriving while no client is attached, and a client that attaches sees the current
state.

`blocked` and `error` notify you like a finished agent does, following your
[notification settings](configuration.md#notifications), and tint the pane border in the warning and
error colors. Override or turn off those colors with the `blocked` and `error` keys in
[`[pane.alert]`](configuration.md#pane-alert). Interacting with the pane clears its attention mark.
`working`, `done`, and progress updates never notify.

A program status report is not an agent, even when it says `app=codex` or `app=claude-code`: it
does not change agent detection, keep-awake, or conversation restore. When the report names an
agent rozi found in that pane, such as `app=claude-code` from Claude Code, it describes that agent
instead of adding a row of its own: the agent's row shows the report's message as what it is doing,
and its title names the run. With one row per conversation, as an extension like
claude-rozi-sessions publishes, the report describes the conversation on screen. Reports from other
programs in the same pane keep their own row. A report left behind after the agent exits is an
activity named after the agent.
