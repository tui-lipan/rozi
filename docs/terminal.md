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
  settles — size changes are snapped rather than animated; see
  [Layouts & panes](layouts-and-panes.md#animation-policy)).
- When the shell process exits, its pane closes. When the last pane closes, the app quits.

The shell and starting directory come from the [config](configuration.md) (`shell`, `cwd`),
falling back to the system `$SHELL` and the launch directory.

## Mouse support

With the configured WM modifier **not** held, mouse events go to the program in the pane —
so mouse-aware TUIs (vim, htop, tmux-in-a-pane, etc.) work normally. Mouse event bytes are
forwarded to the PTY.

Hold the WM modifier to address the window manager instead:

- `modifier` + left-drag moves the pane.
- `modifier` + right-drag resizes it from the nearest corner.

The mouse scroll wheel over a pane scrolls its terminal scrollback.

## Text selection and clipboard

- **Selection** — drag to select terminal text; the selection is styled with the theme's
  selection color.
- **OSC52 clipboard** — programs running in a pane can set the system clipboard via the OSC52
  escape sequence. This is enabled by default and can be turned off with
  `[clipboard].enable_osc52 = false` in the [config](configuration.md#clipboard).

## Window / program titles

Programs set their title via the OSC 0/2 escape sequence (shells often set it to `$PWD`,
editors to the open filename). `hyprmux` reads that title and shows it in the pane's titlebar,
unless you've set a custom title by renaming the pane. See
[Layouts & panes › Titlebars](layouts-and-panes.md#titlebars).

## Scrollback

Each pane keeps a scrollback buffer (`scrollback` lines, default 5000 — see
[Configuration](configuration.md)). Scroll it with the mouse wheel. Typing a key snaps the
view back to the live bottom of the buffer.

## Scrollback search

Press `/` (or *Search scrollback* in the palette) to search the focused pane's scrollback:

- Type to search; the status line shows the match count (`1 / N matches`) or "No matches".
- `Enter` jumps to the **next** match; `Shift+Enter` jumps to the **previous** one.
- Selecting a match scrolls the pane to that position; `Esc` closes the search and the pane
  returns to where it was.

Search is **app-side**: `hyprmux` scans the terminal snapshot by walking the scrollback offset
in viewport-sized windows, de-duplicating overlapping scans (preferring the lowest offset so
already-visible matches don't jump), then restores the original scroll position. Matching is
case-insensitive. This works regardless of the program running in the pane, because it reads
rendered terminal lines rather than relying on an in-terminal highlight search.

## What `hyprmux` does *not* do

- **No detach/reattach, no daemon.** PTYs live inside the single UI process. There is no
  server to reconnect to; closing `hyprmux` ends its shells.
- **Profiles restore layout and launch intent, not live state.** Restoring a
  [project profile](project-profiles.md) starts fresh shells/commands — it does not resurrect
  previous processes, scrollback, or environment.
