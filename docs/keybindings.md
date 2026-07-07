# Keybindings

`hyprmux` offers two equivalent control paths for window-management commands, plus normal
typing that goes straight to the focused shell.

## The two control paths

### 1. Prefix mode (always works)

Press the **prefix key** - `Ctrl-a` by default - then a single command key. This is the most
portable path because it does not depend on the terminal delivering modified keys reliably.

- A held modifier with the prefix is ignored; press the prefix *then* release, then the key.
- `Ctrl-a` again (while in prefix mode) sends a **literal** `Ctrl-a` to the focused pane.
- `Esc` cancels prefix mode. An unrecognized key is forwarded to the pane.
- The top bar shows a yellow **PREFIX** indicator while you are in prefix mode.

### 2. Held modifier (direct)

Hold the **WM modifier** and press an active command key to run the same action without the
prefix. The default modifier is **`Alt`**, because `Super` is rarely delivered reliably by
terminal emulators. You can switch to `Super` in the config (see [Configuration](configuration.md)).

The direct path uses the same active keymap as prefix mode: if a command key is rebound, the
modifier path follows that rebind; if a modifier chord is not in the active keymap, it is
forwarded to the focused shell.

You can also configure an exact held chord in `[keys]`, for example `spawn = "alt-enter"`.

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
| Paste from clipboard | `v` |
| Move pane left / down / up / right | `Shift+h/j/k/l` or `Shift+←/↓/↑/→` |
| Swap pane with neighbor | `modifier`+`Ctrl`+`h/j/k/l` or `Ctrl+←/↓/↑/→` |
| Promote pane to master | `.` (also palette) |

**Swap vs. Move:** *Move* re-inserts the focused pane at a neighbor's split (changing the
tree); *Swap* exchanges the two panes' positions in place without restructuring.

**Paste** reads the system clipboard and sends it to the focused pane's PTY, wrapped in
bracketed-paste markers so shells/editors that opt in treat it as one paste instead of
simulated keystrokes.

### Focus

Spatial focus moves to the nearest pane in a direction (not just the next in a list).

| Command | Keys |
| --- | --- |
| Focus left / down / up / right | `h/j/k/l` or `←/↓/↑/→` |
| Cycle focus to next / previous tiled pane | `Tab` / `Shift+Tab` |

### Layout

| Command | Keys |
| --- | --- |
| Flip focused split axis | `Space` |
| Grow split | `]` or `+` |
| Shrink split | `-` |
| Enter resize mode | `r` |
| Cycle layout (dwindle → master → grid → monocle) | `m` |

See [Layouts and panes](layouts-and-panes.md) for what each layout does. "Zoom" is provided by
*Toggle fullscreen* (`f`), which temporarily maximizes the focused pane.

### Workspaces

There are 9 workspaces. The top bar shows a tab per occupied workspace (at least 5), each
with a live pane count.

| Command | Keys |
| --- | --- |
| Switch to workspace _N_ | `1`–`9` |
| Move focused pane to workspace _N_ | `Shift+1`–`Shift+9` (or the shifted symbols `!@#$%^&*(`); switches to the target workspace |
| Move whole workspace to workspace _N_ | `Ctrl+Shift+1`–`Ctrl+Shift+9`; moves every pane, the layout, and the workspace name, then switches there. An empty target slot receives the content; an occupied target swaps with the source so both layouts stay intact |
| Rename workspace | *Rename workspace* in the command palette (no default key) |

A named workspace shows as `<number>:<name>` in the tabs (e.g. `1:code`) instead of just the
number, and the `{workspace}` [bar placeholder](configuration.md#bar) resolves to the name.
Names are saved with profiles and session autosave.

### App & overlays

| Command | Keys |
| --- | --- |
| Command palette | `p` |
| Show keybindings (help) | `?` |
| Copy mode | `[` |
| Search scrollback | `/` |
| Toggle scratchpad | `` ` `` (backtick) |
| Quit | `Ctrl-q` |

> All of the commands above (except `Ctrl-q`) can be rebound from `hyprmux.toml`. See the
> `[keys]` section in [Configuration](configuration.md). The help overlay (`?`) always shows
> your *active* bindings.

Beyond rebinding, `[keys]` can also define brand new key-triggered commands that open a
program in a new pane or send text to the focused pane's PTY - see [User-defined command
keybindings](configuration.md#user-defined-command-keybindings). They show up in the help
overlay (under "Custom") and command palette with a generated label, but are config-only: they
have no stable id, so they can't be rebound elsewhere or invoked via `hyprmux run-action`.

The **command palette** (`p`) is a fuzzy-search list of commands that are awkward to reach by
keyboard - save profile, choose theme, toggle titlebars, promote to master, plus discoverable
extras (search, copy mode, scratchpad, resize mode, toggle layout, toggle focus on hover, help). Frequent single-key
actions (spawn/close/float/fullscreen/rename/flip/grow/shrink) live in the help overlay only,
since the key is faster than a search box. Theme selection and "Save project profile" /
"Toggle pane titlebars" / "Toggle focus on hover" are palette-only - they have no default key.
So are "Open config file" and "Reload config" - see
[Reloading and editing](configuration.md#reloading-and-editing).

The **help overlay** (`?`) is the complete keybinding reference and lists every binding,
including the workspace digits and mouse gestures.

## Resize mode

Press `r` (or run *Resize mode* from the palette) to enter **resize mode**: use `h/j/k/l` to
adjust the focused pane's split ratios, and `Esc` to leave. The top bar shows a green
**RESIZE hjkl Esc** indicator while active.

## Copy mode

Press `[` (or run *Copy mode* from the palette) to enter **copy mode**: a keyboard-driven way
to review scrollback and yank text without a mouse. The top bar shows a **COPY hjkl wbe 0$^ v y
Esc** indicator while active.

| Key | Action |
| --- | --- |
| `h/j/k/l` or arrows | Move the cursor (scrolls into history / toward live at the edges) |
| `w` / `b` / `e` | Word forward / backward / to word end |
| `W` / `B` / `E` | WORD (whitespace-delimited) forward / backward / to WORD end |
| `0` / `^` / `$` | Line start / first non-blank / line end |
| `Ctrl-u` / `Ctrl-d` | Half-page up / down |
| `g` / `G` | Jump to the top of history / the live bottom |
| `v` or `Space` | Start a selection at the cursor |
| `y` or `Enter` | Copy the selection to the system clipboard and exit |
| `Esc` or `q` | Exit without copying |

The word/line motions are confined to the current row (they don't wrap to the next line) and
reuse `tui-lipan`'s vim-mode `TextArea` motion algorithms rather than a separate implementation.
The copy uses the system clipboard, working over SSH via OSC52 when `[clipboard].enable_osc52`
is on.

## Scratchpad

Press `` ` `` (backtick) to toggle a **dropdown scratchpad**: a single always-running terminal
that slides in over the current workspace and out again with one key. Its shell and scrollback
stay alive while hidden, and it follows you across workspace switches. It is not part of any
workspace and is not saved in profiles. Configure its command / cwd / height under
`[scratchpad]` in [Configuration](configuration.md).

## Mouse

Mouse gestures require the configured WM modifier held down (so they don't conflict with the
shell's own mouse usage):

| Gesture | Action |
| --- | --- |
| `modifier` + left-drag | Move the pane (tiled panes lift into a float-like drag; floats move freely) |
| `modifier` + right-drag | Resize the pane from the nearest corner |
| Drag the gap between two tiled panes | Adjust that split's ratio (dwindle and master) |
| Click a pane / its titlebar | Focus that pane |
| Scroll wheel over a pane | Scroll the terminal's scrollback |

Workspace tabs in the top bar are also clickable to switch workspaces.

## Overlays and modal keys

While an overlay is open (command palette, help, theme picker, search, rename):

- `Esc` closes the overlay.
- `Enter` activates the selection (run command, pick theme, jump to next match, submit rename).
- In **search**, `Enter` jumps to the next match and `Shift+Enter` jumps to the previous one.
  `Tab` cycles the search **scope** (focused pane → workspace → all panes); jumping to a match
  in another pane (or workspace) switches focus there.
- In **rename**, submitting an empty name clears the custom title (falling back to the
  program's terminal title, then the pane's default label).
