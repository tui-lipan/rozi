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
- The workbar shows a yellow **PREFIX** indicator while you are in prefix mode.

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

All exit and lifecycle commands are prefix/modifier actions like everything else. hyprmux
disables tui-lipan's built-in global `Ctrl-q` quit (`App::global_quit(None)`); bind
`quit = "ctrl-q"` under `[keys]` if you want that shortcut back through hyprmux. Bare `F12`
is also unbound so it reaches terminal panes; DevTools uses prefix/mod+`F12` instead
(`toggle-devtools`).

### Windows input notes

Key handling is the same on Windows, with two things worth knowing:

- **`Alt` chords reach hyprmux, `Super` (the Windows key) largely does not.** The shell intercepts
  most `Win+<key>` combinations system-wide before any console application sees them, so the default
  `Alt` modifier is not merely the better choice on Windows — it is close to the only workable one.
  `[input] modifier = "super"` will leave several commands unreachable.
- **`Ctrl+C` goes to your pane, not to hyprmux.** The TUI puts the console in raw mode, so `Ctrl+C`
  arrives as an ordinary key event and is forwarded to the program running in the focused pane,
  exactly as on Unix. It does not interrupt hyprmux. *Closing the console window* (or logging off)
  is what hyprmux treats as a clean detach — see
  [Sessions](sessions.md#how-a-server-starts-and-stops).

## Command reference

### Panes

| Command | Keys |
| --- | --- |
| New shell pane | `Enter` or `c` |
| Close focused pane | `w` or `x` (press twice if `[confirm] close_pane` is enabled) |
| Toggle floating / tiling | `t` |
| Toggle fullscreen | `f` |
| Rename pane | `n` |
| Paste from clipboard | `v` or `Ctrl+V` |
| Swap pane left / down / up / right | `Shift+h/j/k/l` or `Shift+←/↓/↑/→` |
| Move pane left / down / up / right | `Ctrl+h/j/k/l` or `Ctrl+←/↓/↑/→` (a bare `Ctrl`+arrow with no `modifier` is forwarded to the focused pane for word-wise motion) |
| Promote pane to master | `.` (also palette) |
| Respawn exited pane | *Respawn exited pane* appears in the command palette when the focused pane is retained after exit (no default key; action id `respawn-pane`) |

**Swap vs. Move** are two different operations on the same neighbor, and both keep focus on the
pane you started with:

- **Swap** exchanges the two panes' slots. The layout keeps exactly the shape it had — only the
  contents of two slots change places. This is the everyday one, so it gets `Shift`.
- **Move** lifts the pane out of the layout and re-inserts it beside that neighbor, so the slot it
  vacated collapses and the layout changes shape. It is the keyboard equivalent of dragging a pane
  onto another one with the mouse. Moving left/up docks the pane before its neighbor, right/down
  after it.

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

Spatial focus moves to the nearest pane in a direction (not just the next in a list). At an edge,
focus wraps to the opposite edge while preserving the current row or column when possible. When
multiple panes feed into one spanning pane, continuing or reversing direction returns to the pane
focus entered from.

| Command | Keys |
| --- | --- |
| Focus left / down / up / right | `h/j/k/l` or `←/↓/↑/→` |
| Cycle focus to next / previous tiled pane | `Tab` / `Shift+Tab` |

#### Seamless vim / neovim navigation

hyprmux ships vim-aware focus actions and its own
[`vim-hyprmux-navigator`](../integrations/vim-hyprmux-navigator/) plugin. Together they make a
single `Ctrl-h/j/k/l` cross both hyprmux panes and editor splits. The hyprmux actions are **unbound
by default**, so you opt in explicitly.

| Action id | Behavior |
| --- | --- |
| `smart-focus-left` / `-down` / `-up` / `-right` | If the focused pane is running a split-aware program (see `[navigation] editors` in [Configuration](configuration.md#navigation)), forward the matching `Ctrl-h/j/k/l` to it; otherwise move hyprmux pane focus in that direction. |

The pieces that make this work:

- **hyprmux → editor:** bind the smart-focus actions to `Ctrl-h/j/k/l`. When the focused pane runs
  vim/neovim, the key is forwarded so the editor moves its own split; otherwise hyprmux moves focus.
- **editor → hyprmux:** when the editor is at its split edge it hands focus back by calling
  `hyprmux run-action focus-<dir>-no-wrap` over the [control socket](control.md) (every pane already
  has `HYPRMUX`/`HYPRMUX_SOCKET`/`HYPRMUX_PANE` in its environment). The plugin can opt back into
  wrapping with `g:hyprmux_navigator_wrap = 1`.

Detection uses the pane's foreground process (Linux `/proc`), so it is accurate regardless of
shell/process depth and works for any program you list in `[navigation] editors`, not just vim.

Wire the hyprmux side in `[keys]`:

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
  dir = "/path/to/hyprmux/integrations/vim-hyprmux-navigator",
  name = "vim-hyprmux-navigator",
}
```

The plugin provides `:HyprmuxNavigateLeft/Down/Up/Right/Previous`, normal and terminal-mode
mappings, optional save-on-switch behavior, and no-op fallback outside hyprmux. See its
[README](../integrations/vim-hyprmux-navigator/README.md) for Vim package installation and custom
mappings.

### Layout

| Command | Keys |
| --- | --- |
| Flip focused split axis | `Space` |
| Grow split | `]` or `+` |
| Shrink split | `-` or `_` |
| Enter resize mode | `r` |
| Cycle layout (dwindle → master → grid → monocle) | `m` |

See [Layouts and panes](layouts-and-panes.md) for what each layout does. "Zoom" is provided by
*Toggle fullscreen* (`f`), which temporarily maximizes the focused pane.

### Workspaces

There are 9 workspaces. The workbar shows a tab per occupied workspace (at least 5), each
with a live pane count.

| Command | Keys |
| --- | --- |
| Switch to workspace _N_ | `1`–`9` |
| Move focused pane to workspace _N_ | `Shift+1`–`Shift+9` (or the shifted symbols `!@#$%^&*(`); switches to the target workspace |
| Move whole workspace to workspace _N_ | `Ctrl+Shift+1`–`Ctrl+Shift+9`; moves every pane, the layout, and the workspace name, then switches there. An empty target slot receives the content; an occupied target swaps with the source so both layouts stay intact |
| Rename workspace | *Rename workspace* in the command palette (no default key) |

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
| Detach | `d` | Leave the TUI back to your shell (tmux-style) while the session server keeps running for later reattach. Detaching never shuts panes down. An anonymous ephemeral session first prompts you to name it (confirm to detach durably; cancel returns to the session - use *Quit* to tear an ephemeral session down). A named session detaches immediately. |
| Quit client | `q` | Exit this UI. The current server keeps running unless the session is ephemeral, in which case it shuts down. Quitting an ephemeral session that still has a live pane asks for a second press first (press `q` again within the confirm window); disable via `[confirm] quit_ephemeral = false`. |
| Kill workspace | *(no default)* | Close every pane on the active workspace (press twice to confirm; see `[confirm]`). Rarely used and destructive, so it ships unbound - reach it via the command palette or bind `kill-workspace` under `[keys]`. |
| Kill session | *(palette only)* | Shut down the attached session and switch the UI to a fresh ephemeral session. Palette selection runs directly; if you bind this action or call it via `run-action`, `[confirm].kill_session` controls whether it needs a second trigger. |

Configured `[confirm]` prompts apply to key/chord and control-socket action triggers. Commands
chosen from the command palette are treated as explicit selections and run without an extra
second confirmation.

### Sessions

| Command | Default keys | What it does |
| --- | --- | --- |
| Sessions… | `s` | Open the session picker. `Enter` switches to the highlighted running session while retaining the current attachment, so switching back is instant; typing a name + `Ctrl+N` explicitly creates and switches to a fresh empty session, failing if that name is already running; `Ctrl+D` detaches the current named session and exits the client; `Ctrl+K` (twice) kills the selected named session or resets a selected ephemeral session. Killing or resetting the current session shuts its server down and hops the UI onto a fresh ephemeral session instead of quitting. `Ctrl+W` closes the client attachment to a session you are retaining in the background (its server keeps running, so you can reattach later); it does not apply to the current session. `Ctrl+X` disconnects a whole remote host — it closes every attachment (current and retained) to the selected row's host at once, leaving the remote servers running, and hops the UI onto a fresh local session if the current one was on that host. `Ctrl+R` opens **Connect remote host…**, a prompt (prefilled with your most recent ad-hoc target) where you type an SSH target — a hostname, `user@host:port`, an `ssh://` URL, or a configured `[remote.hosts]` alias — to attach to a fresh session on that host, retaining the current session in the background. Recently used ad-hoc targets are remembered under `$XDG_STATE_HOME/hyprmux/recent-remotes` (the target string only, never credentials). The list auto-refreshes while open and shows `from <profile>` when creation-origin metadata is available. Launch with `--pick` (or `[session] startup = "picker"`) to open this picker at startup when a named session exists; `Esc` there starts a fresh ephemeral session. |
| Rename session | *(palette only)* | Rename the **current** session in place, keeping every live pane and its scrollback. The palette label shows **Name session** for an ephemeral session (naming it for the first time, without leaving) and **Rename session** for an already-named one. Distinct from *Detach*, which names if needed and then leaves. See [Sessions](sessions.md). |
| New temporary session | *(palette only)* | Start a fresh empty ephemeral session. The current named session is detached and left running; a current ephemeral session is discarded and its panes are killed. Palette selection runs directly; if you bind this action or call it via `run-action`, `[confirm].new_temporary_session` controls confirmation before discarding an ephemeral session. |
| Request layout control | `g` | Ask the current controller for the layout-control lease when several clients share a session, so you (not another client) drive splits, moves, resizes, and workspace edits. It **requests** rather than steals: the controller sees a toast and a `wants control` badge in *Session clients* and grants (or declines) it there; if no client currently holds the lease the request is auto-granted. No effect when you already control the layout or a single client is attached. See [Shared live layouts](sessions.md#shared-live-layouts). |
| Grant layout control | `e` | As the controller, hand the lease to the client that requested it (the earliest requester when several are waiting). Only active while a request is pending; when it arrives the request toast shows this key (following any rebind). You can also grant a specific client from *Session clients* (`Enter`/`g`). |

### Sidebar

| Command | Default keys | What it does |
| --- | --- | --- |
| Toggle sidebar (`toggle-sidebar`) | `b` | Show or hide the current client's docked sidebar. It remains available while the scratchpad is open. |
| Focus sidebar (`focus-sidebar`) | `shift-b` | Move the keyboard into the sidebar's row list, revealing the sidebar first if it was hidden. |
| Next sidebar tab (`sidebar-next-tab`) | *(no default)* | Cycle forward through configured tabs while the sidebar is visible. |
| Previous sidebar tab (`sidebar-prev-tab`) | *(no default)* | Cycle backward through configured tabs while the sidebar is visible. |
| Focus next blocked pane (`focus-next-blocked-pane`) | *(no default)* | Scan panes across all workspaces in deterministic order and focus the next pane reporting `blocked`; wraps after the current focus and skips closing/special panes. |

All five actions are available from the command palette and `hyprmux run-action`, and all are
rebindable under `[keys]`.

Once the sidebar has the keyboard, `↑`/`↓` move the cursor (skipping section headers), `Enter`
activates the row exactly as a click would, `←`/`→` expand and collapse directories in the Files and
Git tabs, `Tab`/`Shift-Tab` cycle sidebar tabs, and `Esc` returns the keyboard to the focused pane.
The sidebar is deliberately excluded from the Tab focus ring and from click-to-focus, so `Tab` keeps
reaching the focused pane's program and clicking a row never steals the keyboard from a running
command. See [Sidebar](sidebar.md).

> All commands above can be rebound from `hyprmux.toml`. See the `[keys]` section in
> [Configuration](configuration.md). The help overlay (`?`) always shows your *active* bindings.

Beyond rebinding, `[keys]` can also define brand new key-triggered commands that open a
program in a new pane or send text to the focused pane's PTY - see [User-defined command
keybindings](configuration.md#user-defined-command-keybindings). They show up in the help
overlay (under "Custom") and command palette with a generated label, but are config-only: they
have no stable id, so they can't be rebound elsewhere or invoked via `hyprmux run-action`.

The **command palette** (`p`) is a fuzzy-search list of commands that are awkward to reach by
keyboard - capture session as profile, replace session with profile, change appearance, promote to master, plus discoverable extras (rename pane, search,
copy mode, scratchpad, resize mode, toggle layout, toggle focus on hover, help). The appearance
palette groups theme, titlebar, workbar, animation, and border controls.
Frequent single-key actions (spawn/close/float/fullscreen/flip/grow/shrink) live in the help
overlay only, since the key is faster than a search box. "Change appearance" and "Toggle
focus on hover" are palette-only - they have no default key. So are "Open config file" and
"Reload config" - see
[Reloading and editing](configuration.md#reloading-and-editing).

The **help overlay** (`?`) is the complete keybinding reference and lists every binding,
including the workspace digits and mouse gestures.

## Resize mode

Press `r` (or run *Resize mode* from the palette) to enter **resize mode**: use `h/j/k/l` (or the
arrow keys) to adjust the focused pane's split ratios, and `Esc` to leave. The workbar shows a green
**RESIZE hjkl Esc** indicator while active.

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

Press `` ` `` (backtick) to toggle a **dropdown scratchpad**: a single always-running terminal
that slides in over the current workspace and out again with one key. Its shell and scrollback
stay alive while hidden, and it follows you across workspace switches. It is not part of any
workspace and is not saved in profiles. Configure its command / cwd / height under
`[scratchpad]` in [Configuration](configuration.md).

While the scratchpad is open, application actions are suspended so they cannot change the
workspace or steal focus behind it. Press the scratchpad binding again to dismiss it. The local
`toggle-sidebar` action remains available because it changes only the surrounding app shell.

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
  to **Appearance**; a naming prompt raised from a picker (`Ctrl+N` / `Ctrl+O` in **Profiles**,
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
same-name session. `Ctrl+O` opens that recipe under a new name, or as an ephemeral session when the
name is left empty. `Ctrl+N` captures the current session as a new profile, and `Ctrl+F` toggles the
selected startup default. `Ctrl+R` twice runs **Replace session with profile...**, closing every
pane and running process while keeping the session name and attached clients. The row status
describes only the canonical same-name session, not every session created from that recipe.

## Session collaboration

The command palette exposes `session-clients` to inspect attached clients and grant control, and
`toggle-input-lock` to restrict terminal input to the current controller. Both ids can be used in
`[keys]` bindings.
`toggle-pane-logging` starts or stops raw PTY output logging for the focused pane. It is available
in the command palette and can be assigned under `[keys]`.
