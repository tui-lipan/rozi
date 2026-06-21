# Keybindings

`hyprmux` offers two equivalent control paths for window-management commands, plus normal
typing that goes straight to the focused shell.

## The two control paths

### 1. Prefix mode (always works)

Press the **prefix key** — `Ctrl-a` by default — then a single command key. This is the most
portable path because it does not depend on the terminal delivering modified keys reliably.

- A held modifier with the prefix is ignored; press the prefix *then* release, then the key.
- `Ctrl-a` again (while in prefix mode) sends a **literal** `Ctrl-a` to the focused pane.
- `Esc` cancels prefix mode. An unrecognized key is forwarded to the pane.
- The top bar shows a yellow **PREFIX** indicator while you are in prefix mode.

### 2. Held modifier (direct)

Hold the **WM modifier** and press a command key to run the same action without the prefix.
The default modifier is **`Alt`**, because `Super` is rarely delivered reliably by terminal
emulators. You can switch to `Super` in the config (see [Configuration](configuration.md)).

Both paths map to the same actions, so every command below can be triggered either way:
`prefix` then key, or `modifier`+key.

> `Ctrl-q` quits the app and is handled before any routing, so it always works.

## Command reference

### Panes

| Command | Keys |
| --- | --- |
| New shell pane | `Enter` or `c` |
| Close focused pane | `w` or `x` |
| Toggle floating / tiling | `t` |
| Toggle fullscreen | `f` |
| Rename pane | `n` |
| Move pane left / down / up / right | `Shift+h/j/k/l` or `Shift+←/↓/↑/→` |

### Focus

Spatial focus moves to the nearest pane in a direction (not just the next in a list).

| Command | Keys |
| --- | --- |
| Focus left / down / up / right | `h/j/k/l` or `←/↓/↑/→` |

### Layout

| Command | Keys |
| --- | --- |
| Flip focused split axis | `Space` |
| Grow split | `]` or `+` |
| Shrink split | `[` or `-` |
| Enter resize mode | `r` |
| Toggle dwindle ⇄ master layout | `m` |

### Workspaces

There are 9 workspaces. The top bar shows a tab per occupied workspace (at least 5), each
with a live pane count.

| Command | Keys |
| --- | --- |
| Switch to workspace _N_ | `1`–`9` |
| Move focused pane to workspace _N_ | `Shift+1`–`Shift+9` (or the shifted symbols `!@#$%^&*(`) |

### App & overlays

| Command | Keys |
| --- | --- |
| Command palette | `p` |
| Show keybindings (help) | `?` |
| Search scrollback | `/` |
| Quit | `Ctrl-q` |

The **command palette** (`p`) is a fuzzy-search list of commands that are awkward to reach by
keyboard — save profile, choose theme, toggle titlebars, plus discoverable extras (search,
resize mode, toggle layout, help). Frequent single-key actions (spawn/close/float/fullscreen/
rename/flip/grow/shrink) live in the help overlay only, since the key is faster than a search
box. Theme selection and "Save project profile" / "Toggle pane titlebars" are palette-only —
they have no default key.

The **help overlay** (`?`) is the complete keybinding reference and lists every binding,
including the workspace digits and mouse gestures.

## Resize mode

Press `r` (or run *Resize mode* from the palette) to enter **resize mode**: use `h/j/k/l` to
adjust the focused pane's split ratios, and `Esc` to leave. The top bar shows a green
**RESIZE hjkl Esc** indicator while active.

## Mouse

Mouse gestures require the configured WM modifier held down (so they don't conflict with the
shell's own mouse usage):

| Gesture | Action |
| --- | --- |
| `modifier` + left-drag | Move the pane (tiled panes lift into a float-like drag; floats move freely) |
| `modifier` + right-drag | Resize the pane from the nearest corner |
| Click a pane / its titlebar | Focus that pane |
| Scroll wheel over a pane | Scroll the terminal's scrollback |

Workspace tabs in the top bar are also clickable to switch workspaces.

## Overlays and modal keys

While an overlay is open (command palette, help, theme picker, search, rename):

- `Esc` closes the overlay.
- `Enter` activates the selection (run command, pick theme, jump to next match, submit rename).
- In **search**, `Enter` jumps to the next match and `Shift+Enter` jumps to the previous one.
- In **rename**, submitting an empty name clears the custom title (falling back to the
  program's terminal title, then the pane's default label).
