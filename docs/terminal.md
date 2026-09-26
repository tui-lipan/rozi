# Terminal features

This page covers how panes behave as terminals: the shell they start, working directories, mouse and
clipboard handling, copy mode and search, links, scrollback, images, and pane logging.

Programs in a pane work as they would in any terminal, including mouse input, titles, clipboard
access, and resizing. Each pane runs on the session server, so it keeps running in a named session
after you detach. See [Core concepts](core-concepts.md#sessions-and-clients) and
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

See [Configuration](configuration.md#shell-integration-settings) for modes and per-shell behavior.

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

The `v` command key and `Ctrl+V` paste text from the system clipboard as a bracketed paste. When the
clipboard holds something other than text, `Ctrl+V` pressed directly passes through so the pane
program can handle it. The prefix, held-modifier, and command-palette paste commands paste text
only.

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

The `u` command key starts hint mode. It labels visible URLs, paths with optional line numbers, Git
commit ids, and your `[[hints]]` patterns, including ones that wrap across lines. Type a lowercase
label to copy the target, or end the label with an uppercase character to open it when it can be
opened.

### Copy the last command output

**Copy last command output** and the prompt jumps in copy mode need shell integration; see
[Working directories and shell metadata](#working-directories-and-shell-metadata). For scripts,
`rozi capture-pane --last-output` captures the same output.

## Open links

Hold `Ctrl` and click a visible URL to open it with the system handler. Links that programs mark
explicitly (OSC 8 hyperlinks) also work, and take precedence over URLs detected in plain text.
rozi shows an error for an unsupported destination instead of passing it to the operating system.

## Scrollback

Each pane keeps the last `scrollback` lines of output, 5000 by default. Typing returns the view to
live output.

The session server keeps history so you can reattach, and each attached client keeps its own copy.
Memory use grows with pane width, the amount of history, and the number of attached clients. Lower
`scrollback` if memory matters more than deep history.

The limit is fixed when a pane's terminal screen is created. After a config reload, a new
`scrollback` value applies to new screens, such as new panes; existing ones keep their old limit.
Restart a named session to rebuild its screens with the new limit.

**Edit scrollback** saves the focused pane's history to a private file in the state directory and
opens it in `$EDITOR`, then `$VISUAL`, then `vi`. rozi keeps about the 20 most recent files. These
files may contain passwords, tokens, and other private output.

## Titles and urgency

Programs set the pane title with OSC 0 or OSC 2. A custom title set with the `N` command key takes
precedence. See [Layouts and panes](layouts-and-panes.md#titles-and-exited-panes).

With `[notifications] bell = true`, a bell from a pane you are not watching marks its workspace. You
are watching a pane only when both the terminal window and the pane have focus.

## Images

Pane programs can show images with the Kitty graphics protocol. rozi displays them in whatever
format the host terminal supports, including Kitty, iTerm2, sixel, or a text-cell fallback. Images
scroll with the terminal and belong to the screen (main or alternate) they were drawn on.

Limits:

- Kitty protocol animation frames are not supported. Programs that redraw an image can still
  animate it.
- The session server and each attached client keep up to 32 MiB of image data per pane, evicting
  old images when needed. A client that attaches receives the retained images.
- Remote panes send image data inline, because the local client cannot read a file path on the
  server.
- In a session that may have several clients, rozi refuses image transfers through temporary files
  or shared memory, which can be read only once.

## Take a screenshot

To save a PNG of what rozi shows, run one of these from the command palette, or bind its id under
`[keys]`:

| Command | Id | Saves |
| --- | --- | --- |
| **Screenshot pane** | `screenshot-pane` | The focused pane's visible screen, without its border or title. |
| **Screenshot UI** | `screenshot-ui` | The whole window as drawn: bar, sidebar, borders, titles, and every visible pane. |

**Screenshot UI** leaves out the command palette that ran it. A toast shows where the file went, and
the pane or window flashes briefly in the theme's accent color. The flash comes after the picture
is taken, so it never appears in the file. With `[animations] enabled = false` or `focus_chrome = false`, only
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
client sees the same state.

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
