# Terminal features

Every `hyprmux` pane is a **real terminal**: a live PTY shell rendered by a full VT emulator.
The terminal primitives come from [`tui-lipan`](../../tui-lipan)'s `terminal` feature
(`portable-pty` + `alacritty_terminal`); `hyprmux` drives them and adds window management,
identity, and scrollback search on top.

## A pane is a live shell

`hyprmux` runs an [always-server](sessions.md) model: a background session server owns every PTY,
and the UI client parses the raw PTY byte stream into its own `TerminalScreen`.

- The server spawns each PTY and broadcasts its raw output as pane frames; the client feeds those
  bytes into a `TerminalScreen` (the VT emulator) and re-renders a snapshot the UI displays.
- Query responses (DA/DSR/OSC) are answered by the server's own screen; the client parses the same
  bytes and discards its responses so the two screens stay in lockstep.
- Resizing a pane sends a resize request to the server; the client resizes its emulator only when
  the server acknowledges it, so both parsers resize at the same byte position and wrap state stays
  identical. Size changes are snapped rather than animated (see
  [Layouts & panes](layouts-and-panes.md#animation-policy)).
- When the shell process exits, its pane closes (keep-open panes can respawn). The app keeps running
  until you detach or quit.

The shell and starting directory come from the [config](configuration.md) (`shell`, `cwd`),
falling back to the system `$SHELL` and the launch directory.

## Shell metadata

The server tracks runtime metadata independently from terminal rendering and shares it with every
attached client. It recognizes OSC 7 `file://` current-directory reports, OSC 9;9 Windows-style
directory reports, and OSC 133 prompt/input/execution/completion boundaries. Valid local OSC cwd
reports take precedence over native process inspection; remote OSC 7 hosts are shown as metadata
but are never used as spawn directories.

In the default `[shell_integration]` `auto` mode, bash, zsh, fish, and PowerShell panes emit these
markers without modifying dotfiles, registry keys, or `$PROFILE`. Their execution marker includes a
hyprmux-namespaced executable basename only, never a command line — treat everything a terminal
tells you as untrusted, including your own shell's report of what you just typed. See
[Configuration](configuration.md#shell_integration) for the per-shell setup and opt-out.

cmd.exe is the exception: it reports its working directory and prompt boundaries but nothing about
the command it is running, because it offers no pre-execution hook and hyprmux will not install an
`AutoRun` registry key.

### Smart focus and cwd inheritance

A new pane opens in the **focused pane's current working directory** when it can be discovered,
falling back to the configured `cwd`. Likewise, smart focus needs to know what program a pane is
running. Both resolve through the same precedence:

| | cwd | Foreground program |
| --- | --- | --- |
| 1 | A valid **local** OSC 7 / OSC 9;9 report | The shell's own OSC 133 execution marker |
| 2 | Linux `/proc` / macOS `libproc` inspection of the PTY's process | Linux `/proc` / macOS foreground process group |
| 3 | The pane's launch directory | — (treated as unknown) |
| 4 | The configured `cwd` | — |

A path that decodes to something not absolute — a Windows drive-relative `C:foo`, a rooted-but-
driveless `\foo`, a bare `foo` — falls through to the next tier rather than being repaired. A path
we would have to guess at is a path we should not be handing to a new pane.

**Windows has no tier 2.** Process inspection is deliberately unsupported: hyprmux never probes a
PEB or walks a process tree. Shell integration is therefore the only source of this metadata on
Windows, which is why the PowerShell integration is worth having and why cmd.exe panes will not do
smart focus.

An OSC 7 report carrying a *remote* host (an SSH session with the integration installed on the far
side) is displayed but never used as a spawn directory — the path is real, but not on this machine.
A host hyprmux cannot resolve is treated as remote, which is the safe direction to be wrong in.

The live cwd is also what *Capture session as profile* records (see [Project profiles](project-profiles.md)).

## Mouse support

With the configured WM modifier **not** held, mouse events go to the program in the pane -
so mouse-aware TUIs (vim, htop, tmux-in-a-pane, etc.) work normally. Mouse event bytes are
forwarded to the PTY.

Hold the WM modifier to address the window manager instead:

- `modifier` + left-drag moves the pane.
- `modifier` + right-drag resizes it from the nearest corner.

The mouse scroll wheel over a pane scrolls its terminal scrollback.

## Text selection and clipboard

- **Selection** - drag to select terminal text; the selection is styled with the theme's
  selection color.
- **OSC52 clipboard** - programs running in a pane can set the system clipboard via the OSC52
  escape sequence. This is enabled by default and can be turned off with
  `[clipboard].enable_osc52 = false` in the [config](configuration.md#clipboard).
- **Paste** (`v`, `Ctrl+V`, or *Paste from clipboard* in the palette) reads the system clipboard
  and sends it to the focused pane's PTY, wrapped in bracketed-paste markers so shells/editors that
  opt in treat it as one paste instead of simulated keystrokes.

## Copy mode

Press `[` (or *Copy mode* in the palette) for a keyboard-driven way to review scrollback and
yank text without the mouse. A cursor moves with `h/j/k/l`/arrows (scrolling into history or
toward the live view at the top/bottom edges); `w`/`b`/`e` and `W`/`B`/`E` move by word/WORD
(forward, backward, to word end), `0`/`^`/`$` jump to the line start, first non-blank, or line
end (these row-local motions reuse `tui-lipan`'s vim-mode `TextArea` motion algorithms);
`Ctrl-u`/`Ctrl-d` page by half a screen; and `g`/`G` jump to the top of history / the live
bottom. Press `/` to search within the focused pane (same overlay as scrollback search, scoped
to this pane); `Enter` parks the copy cursor on the match and returns to copy mode, `Esc`
cancels back to copy mode, and `n`/`N` cycle later matches while keeping any selection anchor.
Press `v` (or `Space`) to start a selection, then `y` (or `Enter`) to copy it to the
system clipboard and exit, or `Esc`/`q` to leave without copying. The workbar shows a **COPY**
indicator while active, and the selection is highlighted with the theme's selection color. Yank
uses the system clipboard, reaching it over SSH via OSC52 when enabled.

## Hint mode

Press `u` (or *Hint mode* in the palette) to detect URLs, filesystem paths containing `/` (with an
optional `:line` suffix), and 7-40 character Git SHAs in the visible terminal snapshot, plus any
additive `[[hints]]` patterns from config. Each match receives a home-row label. A lowercase label
copies the match; an uppercase final label character opens URL matches (and custom hints with
`open = true`) and copies other kinds. `Esc`/`q` exits. Scroll first to hint older output.

## Bell urgency

With `[notifications].bell = true` (the default), BEL from an unfocused pane marks its workspace
tab with `!`. Focusing that pane clears the marker. Attach replay and BEL used to terminate an OSC
sequence do not create false urgency.

## Window / program titles

Programs set their title via the OSC 0/2 escape sequence (shells often set it to `$PWD`,
editors to the open filename). `hyprmux` reads that title and shows it in the pane's titlebar,
unless you've set a custom title by renaming the pane. See
[Layouts & panes › Titlebars](layouts-and-panes.md#titlebars).

## Scrollback

Each pane keeps a scrollback buffer (`scrollback` lines, default 5000 - see
[Configuration](configuration.md)). Scroll it with the mouse wheel. Typing a key snaps the
view back to the live bottom of the buffer.

## Scrollback search

Press `/` (or *Search scrollback* in the palette) to search the focused pane's scrollback:

- Type to search; the status line shows the match count (`1 / N matches`) and the active scope,
  or "No matches".
- `Enter` jumps to the **next** match; `Shift+Enter` jumps to the **previous** one.
- `Tab` cycles the **scope**: the focused pane, the whole workspace, or all panes. Jumping to a
  match in another pane (or workspace) switches focus there before scrolling to it.
- Selecting a match scrolls the pane to that position; `Esc` closes the search and the pane
  returns to where it was.

Search is **app-side**: `hyprmux` scans retained scrollback via `TerminalScreen`'s plain-text
export (no scrollback-offset mutation), then maps absolute lines back to viewport coordinates
for jumping. Matching is case-insensitive. This works regardless of the program running in the
pane, because it reads rendered terminal lines rather than relying on an in-terminal highlight
search.

## Edit scrollback

*Edit scrollback in $EDITOR* (palette / `edit-scrollback` action) dumps the focused pane's full
retained scrollback to a private file under the state directory
(`~/.local/state/hyprmux/scrollback/pane-<id>-<timestamp>.txt`, mode `0600`) and opens it in
`$EDITOR` (then `$VISUAL`, then `vi`) as a tiled pane — the same pattern as opening the config
file. Older dumps are pruned so the directory stays near 20 files.

**Credentials caveat:** like pane logging, scrollback dumps can contain secrets typed or printed
in the terminal (tokens, passwords, private URLs). Treat the dump directory as sensitive local
data; do not share those files.

## Runtime persistence boundaries

- **The server owns live state.** PTYs live in the session server, not the UI process. A bare
  launch attaches to a disposable ephemeral session (`eph-<pid>`); a clean quit shuts it down, while
  a UI crash leaves it running so you can reattach and recover scrollback. See [Sessions](sessions.md).
- **Attach seeding replays real VT bytes.** When a client attaches, the server serializes each live
  pane's full screen state (scrollback + primary + alt + modes + cursor + title) to a synthesized VT
  byte stream (`TerminalScreen::export_replay_bytes`) and streams it to the client, which replays it
  through the same parser it uses for live output - one code path, exact reconstruction.
- **Named sessions persist across detach.** `hyprmux <name>` connects to a named server
  whose PTYs survive client detach/quit and can be reattached later.
- **Profiles restore layout and launch intent, not live state.** Restoring a
  [project profile](project-profiles.md) starts fresh shells/commands - it does not resurrect
  previous processes, scrollback, or environment.

## Pane logging

Use the `toggle-pane-logging` action to append a pane's raw PTY output to a log file. Active
logging is shown by a `[log]` title badge and is shared with every client, including clients that
attach after logging starts. Raw logs may contain terminal escape sequences and credentials; view
them with `less -R` and protect them as sensitive data. Logging stops automatically after a write
error.
