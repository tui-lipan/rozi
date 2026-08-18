# Keybindings

`rozi` offers two equivalent control paths for window-management commands, plus normal
typing that goes straight to the focused shell.

## The two control paths

### 1. Prefix mode (always works)

Press the **prefix key** - `Ctrl-a` by default - then a single command key. This is the most
portable path because it does not depend on the terminal delivering modified keys reliably.

- A held modifier with the prefix is ignored; press the prefix *then* release, then the key.
- `Ctrl-a` again (while in prefix mode) sends a **literal** `Ctrl-a` to the focused pane.
- `Esc` cancels prefix mode, and is consumed doing so - it does not also reach the pane.
- An unbound key does nothing and leaves prefix mode. Pressing the prefix is an explicit entry into
  rozi's command state, so every key from that point belongs to rozi: it runs a binding, cancels
  with `Esc`, is sent through by `Ctrl-a` again, or - being unbound - is simply swallowed. A
  mistyped chord never types a stray character into the shell.
- The workbar shows a yellow **PREFIX** indicator while you are in prefix mode.
- The focused pane's caret is withheld while the prefix is pending: the next key belongs to rozi,
  not to the shell.
- After a short hold, a **which-key strip** beside the workbar lists what the next key can be,
  capped at a fifth of the screen with a `+N · ? all` count for whatever did not fit. Turn it off
  with `[input] which_key = "off"` or retime it with the same key; see
  [the which-key strip](configuration.md#the-which-key-strip).

### 2. Held modifier (direct)

Hold the **WM modifier** and press an active command key to run the same action without the
prefix. The default modifier is **`Alt`**, because `Super` is rarely delivered reliably by
terminal emulators. You can switch to `Super` in the config (see [Configuration](configuration.md)).

The direct path uses the same active keymap as prefix mode: if a command key is rebound, the
modifier path follows that rebind; if a modifier chord is not in the active keymap, it is
forwarded to the focused shell.

The simplest rebind is a bare key in `[keys]`, for example `copy-mode = "b"`: it swaps the
action's default key and keeps following the `[input]` scheme, binding `<prefix> b` plus
`<modifier>-b`. You can also configure an exact literal chord instead, for example
`spawn = "alt-enter"` (never mirrored or rewritten when `[input]` changes). To retain an action's
generated defaults and add another shortcut, use `spawn = { add = "super-enter" }`; `add` accepts
bare keys, `scheme:`-marked keys, literal bindings, or a list mixing them.

A modified key is literal by default: `copy-mode = "ctrl-t"` means direct `Ctrl+T`. Prefix it with
`scheme:` when it should instead follow `[input]`: `copy-mode = "scheme:ctrl-t"` generates
`<prefix> Ctrl+T` and, while modifier shortcuts are enabled, `<modifier>+Ctrl+T`. The marker also
composes with additions, such as `copy-mode = { add = "scheme:ctrl-t" }`.

**Disabling the held-modifier layer:** every default key ships with both its `Ctrl-a <key>` leader
chord and an `Alt+<key>` mirror. If you would rather keep held `Alt`/`Super` chords free for the
shell and programs in your panes (readline word-editing, `Alt+Tab`, editor `Alt+Enter`, etc.), set
`[input] modifier_shortcuts = false` to drop the mirror entirely and use prefix mode only. To drop
the mirror for just one command instead, override it in `[keys]` with an explicit leader-only
binding, e.g. `detach = "ctrl-a d"`.

All exit and lifecycle commands are prefix/modifier actions like everything else. rozi
disables tui-lipan's built-in global `Ctrl-q` quit (`App::global_quit(None)`); bind
`quit = "ctrl-q"` under `[keys]` if you want that shortcut back through rozi. Bare `F12`
is also unbound so it reaches terminal panes; DevTools uses prefix/mod+`F12` instead
(`toggle-devtools`).

### Windows input notes

Key handling is the same on Windows, with two things worth knowing:

- **`Alt` chords reach rozi, `Super` (the Windows key) largely does not.** The shell intercepts
  most `Win+<key>` combinations system-wide before any console application sees them, so the default
  `Alt` modifier is not merely the better choice on Windows — it is close to the only workable one.
  `[input] modifier = "super"` will leave several commands unreachable.
- **`Ctrl+C` goes to your pane, not to rozi.** The TUI puts the console in raw mode, so `Ctrl+C`
  arrives as an ordinary key event and is forwarded to the program running in the focused pane,
  exactly as on Unix. It does not interrupt rozi. *Closing the console window* (or logging off)
  is what rozi treats as a clean detach — see
  [Sessions](sessions.md#how-a-server-starts-and-stops).

## Command reference

### Panes

| Command | Keys |
| --- | --- |
| New shell pane | `Enter` |
| New floating pane at pointer | `Shift+Enter` (action id `spawn-float`) |
| Close focused pane | `w` (press twice if `[confirm] close_pane` is enabled) |
| Toggle floating / tiling | `t` |
| Toggle fullscreen | `f` |
| Rename pane | `Shift+N` |
| Paste from clipboard | `v` or `Ctrl+V` |
| Swap pane left / down / up / right | `Shift+h/j/k/l` or `Shift+←/↓/↑/→` |
| Move pane left / down / up / right | `Ctrl+h/j/k/l` or `Ctrl+←/↓/↑/→` (a bare `Ctrl`+arrow with no `modifier` is forwarded to the focused pane for word-wise motion) |
| Promote pane to master | `.` (also palette) |
| Respawn exited pane | *Respawn exited pane* appears in the command palette when the focused pane is retained after exit (no default key; action id `respawn-pane`) |

**Swap vs. Move** are two different operations on the same neighbor, and both keep focus on the
pane you started with:

- **Swap** exchanges the two panes' slots. The layout keeps exactly the shape it had — only the
  contents of two slots change places, so each pane takes on the size of the slot it lands in. This
  is the everyday one, so it gets `Shift`.
- **Move** lifts the pane out of the layout and re-inserts it beside that neighbor, so the slot it
  vacated collapses and the layout changes shape. It is the keyboard equivalent of dragging a pane
  onto another one with the mouse. Moving left/up docks the pane before its neighbor, right/down
  after it. The pane brings its own size along: it and the neighbor divide the space in the
  proportion they already had, so a 70/30 pair stays 70/30 rather than being reset to even. Panes
  that already match on the axis being split — a side-by-side pair moved vertically — divide it
  evenly, which is what that reshape wants anyway.

Starting from pane `A` on the left with `B` over `C` on the right, pressing *right* on `A` gives:

```text
    start            swap A→B          move A→B
+-----+-----+     +-----+-----+     +-----+-----+
|     |  B  |     |     |  A  |     |  B  |  A  |
|  A  +-----+     |  B  +-----+     +-----+-----+
|     |  C  |     |     |  C  |     |     C     |
+-----+-----+     +-----+-----+     +-----------+
```

When the focused pane is **floating** it holds no slot, so there is nothing to trade and nothing to
re-insert beside: both actions slide it across the workspace by one step instead (4% of the canvas,
at least one cell), clamped so a sliver stays on screen.

**Paste** reads the system clipboard and sends it to the focused pane's PTY, wrapped in
bracketed-paste markers so shells/editors that opt in treat it as one paste instead of
simulated keystrokes. Direct `Ctrl+V` pastes text this way but passes through when the clipboard
contains files, an image, or another non-text format, allowing a clipboard-aware TUI in the pane
to handle it. Prefix/modifier and command-palette paste stay text-only. Under `--remote`, rich
pass-through can only reach the remote host's clipboard.

### Focus

Spatial focus moves to the nearest pane in a direction (not just the next in a list). Neighbors
must share cross-axis overlap, so focus stays orthogonal (no diagonal jumps). At an edge, focus
wraps to the opposite edge while preserving the current row or column when possible — including
the row/column you entered a spanning pane from. Reversing direction returns to the pane focus
entered from.

| Command | Keys |
| --- | --- |
| Focus left / down / up / right | `h/j/k/l` or `←/↓/↑/→` |
| Cycle focus to next / previous tiled pane | `Tab` / `Shift+Tab` |

#### Seamless vim / neovim navigation

rozi ships vim-aware focus actions and its own
[`vim-rozi-navigator`](../integrations/vim-rozi-navigator/) plugin. Together they make a
single `Ctrl-h/j/k/l` cross both rozi panes and editor splits. The rozi actions are **unbound
by default**, so you opt in explicitly.

| Action id | Behavior |
| --- | --- |
| `smart-focus-left` / `-down` / `-up` / `-right` | If the focused pane is running a split-aware program (see `[navigation] editors`), forward the matching `Ctrl-h/j/k/l` to it; otherwise move pane focus in that direction. |

The pieces that make this work:

- **rozi → editor:** bind the smart-focus actions to `Ctrl-h/j/k/l`. When the focused pane runs
  vim/neovim, the key is forwarded so the editor moves its own split; otherwise rozi moves focus.
- **editor → rozi:** when the editor is at its split edge it hands focus back by calling
  `rozi run-action focus-<dir>-no-wrap` over the [control socket](control.md) (every pane already
  has `ROZI`/`ROZI_SOCKET`/`ROZI_PANE` in its environment). The plugin can opt back into
  wrapping with `g:rozi_navigator_wrap = 1`.

Detection uses the pane's foreground process (Linux `/proc`), so it is accurate regardless of
shell/process depth and works for any program you list in `[navigation] editors`, not just vim.

Wire the rozi side in `[keys]`:

```toml
[keys]
smart-focus-left = "ctrl-h"
smart-focus-down = "ctrl-j"
smart-focus-up = "ctrl-k"
smart-focus-right = "ctrl-l"
```

Then install the bundled editor plugin. With lazy.nvim:

```lua
{
  dir = "/path/to/rozi/integrations/vim-rozi-navigator",
  name = "vim-rozi-navigator",
}
```

The plugin provides `:RoziNavigateLeft/Down/Up/Right/Previous`, normal and terminal-mode
mappings, optional save-on-switch behavior, and no-op fallback outside rozi. See its
[README](../integrations/vim-rozi-navigator/README.md) for Vim package installation and custom
mappings.

### Layout

| Command | Keys |
| --- | --- |
| Flip focused split axis | `Space` |
| Grow split | `=` |
| Shrink split | `-` |
| Enter resize mode | `r` |
| Cycle layout (dwindle → master → grid → columns → rows → scrollable → monocle) | `m` |
| Choose layout (picker; `ctrl+f` sets the default) | `Shift+M` |

See [Layouts and panes](layouts-and-panes.md) for what each layout does. "Zoom" is provided by
*Toggle fullscreen* (`f`), which temporarily maximizes the focused pane.

### Workspaces

There are 9 workspaces. The workbar shows a tab per occupied workspace (at least 5), each
with a live pane count.

| Command | Keys |
| --- | --- |
| Switch to workspace _N_ | `1`–`9` |
| Move focused pane to workspace _N_ | `Shift+1`–`Shift+9` (or the shifted symbols `!@#$%^&*(`); switches to the target workspace |
| Move whole workspace to workspace _N_ | `Ctrl+Shift+1`–`Ctrl+Shift+9`; moves every pane, the layout, and the workspace name, then switches there. An empty target receives the content; an occupied target swaps with the source. |
| Rename workspace | `n` |

A named workspace shows as `<number>:<name>` in the tabs (e.g. `1:code`) instead of just the
number, and the `{workspace}` [workbar placeholder](configuration.md#workbar) resolves to the
name. Names are saved with profiles and session autosave.

### App & overlays

| Command | Keys |
| --- | --- |
| Command palette | `p` |
| Show keybindings (help) | `?` |
| Toggle DevTools | `F12` |
| Copy mode | `[` |
| Search scrollback | `/` |
| Toggle scratchpad | `` ` `` (backtick) |
| Open profiles picker | `o` |
| Capture session as profile | `O` (`Shift+o`) |

### Leaving and lifecycle

| Command | Default keys | What it does |
| --- | --- | --- |
| Quit client / Detach | `q` / `d` | Leave the client. Both run the same behavior; `detach` remains valid for `[keys] detach` and `rozi run-action detach`. Named sessions keep running. |
| Kill workspace | *(no default)* | Close every pane on the active workspace (press twice to confirm; see `[confirm]`). Unbound by default; reach it via the command palette or bind `kill-workspace` under `[keys]`. |
| Kill session | *(palette only)* | Shut down the attached session. Opens the picker when another choice remains, otherwise the launcher. Ephemeral sessions are removed, not recreated. Bound/`run-action` uses `[confirm].kill_session`. |
| Restart session | *(palette / picker `Ctrl+E`)* | Shut down the selected (or attached) session's server and recreate it as the active session with fresh panes. Distinct from *Kill session*. Picker requires a second `Ctrl+E`. |

An untouched temporary session is closed silently. A temporary session you *worked in* raises
**Keep this session?**: type a name and `Enter` to keep it running, press `Enter` on an empty name
(twice — the prompt says what the second press closes) to close it, or `Esc` to stay. Disable the
second press via `[confirm] quit_ephemeral = false`.

Configured `[confirm]` prompts apply to key/chord and control-socket action triggers. Commands
chosen from the command palette are treated as explicit selections and run without an extra
second confirmation. `[confirm].kill_session` also gates a bound *Restart session*.

### Sessions

| Command | Default keys | What it does |
| --- | --- | --- |
| Sessions… | `s` | Open the session picker. Keys inside the picker are listed below. Opens at startup by default (`[session] startup = "picker"`, or `--pick`). |
| Rename session | `Shift+S` | Rename the **current** session in place, keeping every live pane and its scrollback. The palette shows **Name session** for an ephemeral session. See [Sessions](sessions.md). |
| New temporary session | *(palette only)* | Start a fresh empty ephemeral session. The current named session is detached and left running; a current ephemeral session is discarded. Bound/`run-action` uses `[confirm].new_temporary_session`. |
| Take / request layout control | `g` | Acquire the layout-control lease so you drive splits, moves, resizes, and workspace edits. Immediate when `[session].allow_takeover` is true; otherwise asks the controller. |
| Grant layout control | `e` | As the controller, hand the lease to the client that requested it (the earliest requester when several are waiting). Only active while a request is pending. |
| Toggle immediate control takeover | *(palette only)* | As the current controller, enable or disable immediate takeover for this running session. The `[session].allow_takeover` config sets the initial server policy. |

*Rename session* is distinct from leaving, which offers to name a temporary session on the way out.
You can also grant a specific client from *Manage collaborators* (`Enter`). When a control request
arrives, the request toast shows the grant key (following any rebind). `request-control` has no
effect when you already control the layout or only one client is attached; if nobody holds the
lease, it is always auto-granted. See [Shared live layouts](sessions.md#shared-live-layouts).

### Session picker

Full behavior is in [Switching sessions in-app](sessions.md#switching-sessions-in-app-the-picker).
While the picker is open:

| Key | Action |
| --- | --- |
| `Enter` | **Switch** a background-connected session, **connect** otherwise. Both make it active immediately and retain the current attachment. |
| Type a name + `Ctrl+N` | Create and switch to a fresh empty session (fails if that name is already running). |
| `Ctrl+K` (twice) | Kill the selected session. |
| `Ctrl+E` (twice) | Restart it as the active session with fresh panes. |
| `Ctrl+W` | Disconnect this client's background attachment (server keeps running). Does not apply to the current session. |
| `Ctrl+X` | Disconnect a whole remote host. |
| `Ctrl+R` | Open **Connect remote host…**. |
| `Ctrl+T` | Go to this client's scratch session (start it if needed). Also `Enter` when the list is empty. Hinted only when the list cannot point the way itself. |
| `Esc` | At startup, leave the client in the launcher with no session. A bare `Enter` — or any `spawn` binding — starts a shell. |

Detach is not offered in the picker — leave the client with *Quit* / *Detach* outside it. Killing
the current session opens the picker when another choice remains, otherwise the sessionless
launcher. The list auto-refreshes while open and shows `from <profile>` when creation-origin
metadata is available. It always opens at startup for `startup = "picker"` or `--pick`, even when
the list is empty, and attaches nothing until you choose.

### Sidebar

| Command | Default keys | What it does |
| --- | --- | --- |
| Toggle sidebar (`toggle-sidebar`) | `b` | Show or hide the current client's docked sidebar. It remains available while the scratchpad is open. |
| Toggle sidebar split (`toggle-sidebar-split`) | `\` | Show the saved panel assignment as one or two panels without erasing it. |
| Focus sidebar (`focus-sidebar`) | `shift-b` | Move the keyboard into the sidebar's row list, revealing the sidebar first if it was hidden. |
| Next sidebar tab (`sidebar-next-tab`) | `page-down` | Cycle forward through configured tabs while the sidebar is visible. |
| Previous sidebar tab (`sidebar-prev-tab`) | `page-up` | Cycle backward through configured tabs while the sidebar is visible. |
| Focus next blocked pane (`focus-next-blocked-pane`) | *(no default)* | Scan panes across all workspaces in deterministic order and focus the next reported or screen-detected blocked pane; wraps after the current focus and skips closing/exited/special panes. |

All six actions are available from the command palette and `rozi run-action`, and all are
rebindable under `[keys]`.

Once the sidebar has the keyboard, `↑`/`↓` move the cursor (skipping section headers), `Enter`
activates the row exactly as a click would, and `Tab`/`Shift-Tab` cycle sidebar tabs.
In Files and Git, `←`/`h` and `→`/`l` collapse/expand directories, while `Space` toggles them.
`Esc` returns the keyboard to the focused pane.
`Ctrl+Shift+←`/`Ctrl+Shift+→` reorder tabs; `Ctrl+↑`/`Ctrl+↓` switch panels;
`Ctrl+Shift+↑`/`Ctrl+Shift+↓` transfer the active tab; `Shift+←`/`Shift+→` resize the sidebar;
`Shift+↑`/`Shift+↓` resize the panel split; and `s` toggles its one/two-panel presentation.
The sidebar is deliberately excluded from the Tab focus ring and from click-to-focus, so `Tab` keeps
reaching the focused pane's program and clicking a row never steals the keyboard from a running
command. See [Sidebar](sidebar.md).

> All commands above can be rebound from `config.toml`. See the `[keys]` section in
> [Configuration](configuration.md). The help overlay (`?`) always shows your *active* bindings.

Beyond rebinding, `[keys]` can also define brand new key-triggered commands that open a
program in a new pane or send text to the focused pane's PTY - see [User-defined command
keybindings](configuration.md#user-defined-command-keybindings). They show up in the help
overlay (under "Custom") and command palette with a generated label, but are config-only: they
have no stable id, so they can't be rebound elsewhere or invoked via `rozi run-action`.

A [`[[commands]]`](configuration.md#commands) entry is the reusable alternative: its id binds under
`[keys]` exactly like a built-in, while the command remains palette-visible and reachable through
`rozi run-action` even when it has no key.

The **command palette** (`p`) is a fuzzy-search list of commands that are awkward to reach by
keyboard - capture session as profile, replace session with profile, open Settings, promote to
master, plus discoverable extras (new pane, new floating pane, close pane, rename pane, search,
copy mode, scratchpad, resize mode, toggle layout, do not disturb, and help). **Settings** groups
durable preferences for theme,
titlebar, workbar, focused-pane chrome, borders, focus-on-hover, alert markers, desktop
notifications, sounds, startup mode, and session persistence in one searchable list.
`change-appearance` and `alerts` remain bindable
deep links into that list; `toggle-do-not-disturb` remains a runtime command for this client. On
Settings toggle or cycle rows, `←` / `→` steps the value (Enter still activates, including Theme
and Terminal padding). Command results are separated into **Panes**, **Workspace**, **App**,
**Profile**, **Session**, **Collaboration**, and **Sidebar** sections.
Settings uses **General**, **Titlebar**, **Workbar**, **Panes**, **Alerts**, **Desktop notifications**,
**Sounds**, and **Sessions**, with one blank row separating adjacent sections. **Sessions** comes
last because its rows change what a later launch or server does, so unlike every group above it there
is nothing on screen to inspect after stepping a value.
Frequent single-key actions (float/fullscreen/flip/grow/shrink, plus directional focus/swap/move)
live in the help overlay only, since the key is faster than a search box. "Settings" has no default key; bind its
`settings` action under `[keys]` if desired. "Open config file" and "Reload config" are also
palette-only. `config.toml` changes reload automatically; the manual action also reloads extension
manifest changes - see
[Reloading and editing](configuration.md#reloading-and-editing).

The **help overlay** (`?`) is a keyboard reference, not a command picker. Press `/` to search
from the top-right border (Enter leaves the search; Esc clears the query, then closes). **Global**
(default) lists Prefix/Mod actions, led by the scheme itself (`Ctrl+a` Prefix · then key, `Alt`
Mod · hold + key); **Modes** lists direct keys that take over in copy mode and while the sidebar
is focused; **Unbound** lists commands with no key; **All** is exhaustive. Search stays inside the
active tab.

## Resize mode

Press `r` (or run *Resize mode* from the palette) to enter **resize mode**: use `h/j/k/l` (or the
arrow keys) to adjust the focused pane's split ratios, and `Esc` to leave. The workbar shows a green
**RESIZE hjkl Esc** indicator while active. A press moves the divider 4% of the split it sits in,
rounded to a whole cell and never less than one, so repeated presses step evenly.

A **floating** pane has no split to adjust, so it resizes its own rectangle instead: `l`/`j` grow it
and `h`/`k` shrink it, anchored at its top-left corner so it stays put while changing size. Pair
this with `Shift+h/j/k/l` to move the pane, since resize mode never repositions it.

## Hint mode

Press `u` (or choose *Hint mode* in the palette) to label URLs, paths (including optional line
numbers), and 7-40 character Git SHAs in the focused pane's current visible snapshot. Type a
lowercase label to copy it. Type the final label character uppercase to open a URL with the system
handler; non-URL matches still copy. `Esc` or `q` exits, and keys never leak to the PTY. Hint mode
uses the pane's current scrollback position.

## Copy mode

Press `[` (or run *Copy mode* from the palette) to enter **copy mode**: a keyboard-driven way
to review scrollback and yank text without a mouse. The workbar shows a **COPY hjkl wbe 0$^ v y
Esc** indicator while active.

| Key | Action |
| --- | --- |
| `h/j/k/l` or arrows | Move the cursor (scrolls into history / toward live at the edges) |
| `w` / `b` / `e` | Word forward / backward / to word end |
| `W` / `B` / `E` | WORD (whitespace-delimited) forward / backward / to WORD end |
| `0` / `^` / `$` | Line start / first non-blank / line end |
| `Ctrl-u` / `Ctrl-d` | Half-page up / down |
| `g` / `G` | Jump to the top of history / the live bottom |
| `/` | Search this pane's scrollback; Enter returns to copy mode on the match |
| `n` / `N` | Next / previous search match (after `/`) |
| `[` / `]` | Jump to previous / next shell prompt (OSC 133; requires shell integration) |
| `o` | Copy last command output (requires shell integration) |
| `v` or `Space` | Start a selection at the cursor |
| `y` or `Enter` | Copy the selection to the system clipboard and exit |
| `Esc` or `q` | Exit without copying |

The word/line motions are confined to the current row (they don't wrap to the next line) and
reuse `tui-lipan`'s vim-mode `TextArea` motion algorithms rather than a separate implementation.
The copy uses the system clipboard, working over SSH via OSC52 when `[clipboard].enable_osc52`
is on.

## Scratchpad

Press `` ` `` (backtick) to toggle a **dropdown scratch workspace**. It grows into view over the
current workspace and shrinks back out again with one key, always anchored to the bottom edge - so
its bottom border holds its row and only the edge it opens along moves. Its panes, PTYs, layouts, and scrollback stay alive while
hidden, and it follows you across workspace switches. It is not part of any attachment workspace,
shared layout, or profile. Configure its initial command / cwd / height under
`[scratchpad]` in [Configuration](configuration.md).

While open, ordinary pane actions target scratch panes: create/close, focus, move/swap, layout,
resize, float/fullscreen, rename, copy/search/paste, and terminal input. Workspace switching,
moving/renaming/killing workspaces, and session/profile actions remain unavailable because they
apply only to attachment workspaces. Pane synchronization is also unavailable in scratch because
owner-local panes are intentionally never broadcast. Closing or exiting the final scratch pane hides and empties
the dropdown; the next toggle starts the configured initial command and cwd again.

Mouse gestures work exactly as they do in a workspace: drag a split boundary to adjust its ratio,
`modifier`-drag a pane to move it, `modifier`-right-drag to resize it. Clicking anywhere inside the
dropdown stays in it; clicking the dimmed workspace above it dismisses it.

The dropdown's own height is the one extra control, and every way to reach it is the same gesture
you would use on a split that happened to be there. Its top edge is the scratch workspace's outer
border, so a pane sitting against it has no split above it:

| Gesture | Effect |
| --- | --- |
| Drag the dropdown's top chrome row | Resize its height |
| `modifier` / `prefix` + right-drag an upper corner of a top-edge pane | Resize its height; the horizontal half still resizes the pane |
| Resize mode `k` / `j` on a top-edge pane | Grow / shrink its height |

Panes further down keep their ordinary split resize on every one of those.

## Mouse

Mouse gestures require either the configured WM modifier held down or an active prefix listener
(so they don't conflict with the shell's own mouse usage). After pressing the prefix, it remains
active for the gesture and finishes when the mouse button is released:

| Gesture | Action |
| --- | --- |
| `modifier` or active `prefix` + left-drag | Move the pane (tiled panes lift into a float-like drag; floats move freely) |
| `modifier` or active `prefix` + right-drag | Resize the pane from the nearest corner |
| Drag the gap between two tiled panes | Adjust that split's ratio (dwindle and master) |
| Click a pane / its titlebar | Focus that pane |
| Scroll wheel over a pane | Scroll the terminal's scrollback |

Workspace tabs in the workbar are also clickable to switch workspaces.

## Overlays and modal keys

While an overlay is open (command palette, help, theme picker, search, rename):

- `Esc` closes the overlay.
- A dialog opened *from* another one goes **back** to it instead of to the pane, and its hint bar
  says `back esc` rather than `cancel esc`. The theme picker and the terminal-padding editor return
  to **Settings**; a naming prompt raised from a picker (`Ctrl+N` / `Ctrl+O` in **Profiles**,
  `Ctrl+N` / `Ctrl+R` / `Ctrl+S` in **Sessions**) returns to that picker, rebuilt with the query and
  highlighted row it had. Submitting returns the same way whenever the parent survives - capturing a
  profile or naming the current session lands back in the picker - while anything that attaches,
  creates, or detaches a session leaves the overlays for that session.
- `Enter` activates the selection (run command, pick theme, jump to next match, submit rename).
- In **search**, `Enter` jumps to the next match and `Shift+Enter` jumps to the previous one.
  `Tab` cycles the search **scope** (focused pane → workspace → all panes); jumping to a match
  in another pane (or workspace) switches focus there.
- In **rename**, submitting an empty name clears the custom title (falling back to an
  application-provided terminal title, then the contextual current working directory).

In the **Profiles** picker, `Enter` attaches to or launches the highlighted profile's canonical
same-name session. `Ctrl+O` launches that recipe under a new name, or as an ephemeral session when the
name is left empty. `Ctrl+N` captures the current session as a new profile, and `Ctrl+F` toggles the
selected startup default. While attached, `Ctrl+R` twice runs **Replace session with profile...**,
closing every pane and running process while keeping the session name and attached clients. The row
status describes only the canonical same-name session, not every session created from that recipe.

## Session collaboration

The command palette groups sharing commands under **Collaboration**: request/take control, grant
control, input lock, immediate takeover, and `collaborators` (*Manage collaborators…*). Each row
appears only while it applies to the current session, and each is bindable under `[keys]`.

`collaborators` opens the only dialog of the group: your identity and the controller as context, the
other clients as the selectable roster, and a type-to-filter query. The query input owns focus, so
every action key is a Ctrl chord: `Enter` grants control, `ctrl+d` declines a request, `ctrl+k`
twice removes a client. `Esc` closes it.
`toggle-pane-logging` starts or stops raw PTY output logging for the focused pane. It is available
in the command palette and can be assigned under `[keys]`.
