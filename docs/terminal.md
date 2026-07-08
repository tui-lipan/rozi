# Terminal features

Every `hyprmux` pane is a **real terminal**: a live PTY shell rendered by a full VT emulator.
The terminal primitives come from [`tui-lipan`](../../tui-lipan)'s `terminal` feature
(`portable-pty` + `alacritty_terminal`); `hyprmux` drives them and adds window management,
identity, and scrollback search on top.

## A pane is a live shell

- Each pane owns a `TerminalScreen` (the VT emulator) and an optional `TerminalPty`
  (the spawned process).
- The PTY is spawned on a background thread; its output is streamed back, fed into the
  screen, and re-rendered into a snapshot the UI displays.
- Resizing a pane resizes both the emulator and the PTY (a single SIGWINCH after the geometry
  settles - size changes are snapped rather than animated; see
  [Layouts & panes](layouts-and-panes.md#animation-policy)).
- When the shell process exits, its pane closes. The app keeps running until you detach or quit.

The shell and starting directory come from the [config](configuration.md) (`shell`, `cwd`),
falling back to the system `$SHELL` and the launch directory.

### New panes inherit the focused directory

A new pane opens in the **focused pane's current working directory** when it can be
discovered, falling back to the configured `cwd`. On Linux the directory is read on demand from
`/proc/<pid>/cwd` of the pane's shell - no shell configuration is required. On other platforms
(or if the pid is unavailable) it falls back to the configured `cwd`. The same live cwd is what
*Save profile* records (see [Project profiles](project-profiles.md)).

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
- **Paste** (`v`, or *Paste from clipboard* in the palette) reads the system clipboard and sends
  it to the focused pane's PTY, wrapped in bracketed-paste markers so shells/editors that opt in
  treat it as one paste instead of simulated keystrokes.

## Copy mode

Press `[` (or *Copy mode* in the palette) for a keyboard-driven way to review scrollback and
yank text without the mouse. A cursor moves with `h/j/k/l`/arrows (scrolling into history or
toward the live view at the top/bottom edges); `w`/`b`/`e` and `W`/`B`/`E` move by word/WORD
(forward, backward, to word end), `0`/`^`/`$` jump to the line start, first non-blank, or line
end (these row-local motions reuse `tui-lipan`'s vim-mode `TextArea` motion algorithms);
`Ctrl-u`/`Ctrl-d` page by half a screen; and `g`/`G` jump to the top of history / the live
bottom. Press `v` (or `Space`) to start a selection, then `y` (or `Enter`) to copy it to the
system clipboard and exit, or `Esc`/`q` to leave without copying. The workbar shows a **COPY**
indicator while active, and the selection is highlighted with the theme's selection color. Yank
uses the system clipboard, reaching it over SSH via OSC52 when enabled.

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

Search is **app-side**: `hyprmux` scans the terminal snapshot by walking the scrollback offset
in viewport-sized windows, de-duplicating overlapping scans (preferring the lowest offset so
already-visible matches don't jump), then restores the original scroll position. Matching is
case-insensitive. This works regardless of the program running in the pane, because it reads
rendered terminal lines rather than relying on an in-terminal highlight search.

## Runtime persistence boundaries

- **Local launches are still single-process.** In default local mode, PTYs live inside the UI
  process; quitting that process ends its shells unless you only restore layout later from a
  profile/autosave file.
- **Named attached sessions are the live-state path.** `hyprmux --attach <name>` connects to a
  background session server whose PTYs survive client detach/quit and can be reattached later.
  See [Sessions](sessions.md).
- **Profiles restore layout and launch intent, not live state.** Restoring a
  [project profile](project-profiles.md) starts fresh shells/commands - it does not resurrect
  previous processes, scrollback, or environment.
