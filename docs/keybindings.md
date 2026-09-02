# Keybindings

This page is the canonical reference for Rozi's default keys. The help overlay, opened with `?`,
shows the active keys after configuration overrides.

## Prefix and held modifier

Every command key in the tables works in two ways by default:

- Press `Ctrl+A`, release it, then press the command key.
- Hold `Alt` and press the command key.

For example, close a pane with `Ctrl+A`, then `w`, or with `Alt+W`. Press `Ctrl+A` twice to send a
literal `Ctrl+A` to the focused pane. `Esc` cancels a pending prefix. An unbound key leaves prefix
mode and is consumed.

Set `[input] prefix`, `[input] modifier`, or `[input] modifier_shortcuts` to change this scheme.
`modifier_shortcuts = false` keeps prefix commands and sends held-modifier chords to the pane.
See [Configuration](configuration.md#input).

## Default command keys

### Panes and focus

| Command | Command key |
| --- | --- |
| New pane | `Enter` |
| New floating pane | `Shift+Enter` |
| Close pane | `w` |
| Toggle floating | `t` |
| Toggle fullscreen | `f` |
| Rename pane | `Shift+N` |
| Paste text | `v` |
| Paste text directly | `Ctrl+V` |
| Promote to master | `.` |
| Swap left, down, up, right | `Shift+h/j/k/l`, or `Shift` plus arrows |
| Move and reinsert left, down, up, right | `Ctrl+h/j/k/l`, or `Ctrl` plus arrows |
| Focus left, down, up, right | `h/j/k/l`, or arrows |
| Cycle focus forward or backward | `Tab` or `Shift+Tab` |

Direct `Ctrl+V` invokes Rozi paste only for text clipboard content. Other clipboard formats pass
through to the pane. The prefix and held-modifier forms are always explicit text paste commands.

### Layout and workspaces

| Command | Command key |
| --- | --- |
| Flip split axis | `Space` |
| Grow split or master area | `=` |
| Shrink split or master area | `-` |
| Resize mode | `r` |
| Cycle layout | `m` |
| Choose layout | `Shift+M` |
| Rename workspace | `n` |
| Switch to workspace 1 through 9 | `1` through `9` |
| Move pane to workspace 1 through 9 | `Shift+1` through `Shift+9` |
| Move or swap whole workspace | `Ctrl+Shift+1` through `Ctrl+Shift+9` |

The shifted workspace keys may arrive as `!@#$%^&*(` on terminals that report shifted symbols.
Rozi accepts both terminal encodings for whole-workspace movement.

See [Layouts and panes](layouts-and-panes.md) for layout behavior. `Shift+N` renames a pane. Bare
`n` renames the workspace.

### App, profiles, sessions, and collaboration

| Command | Command key |
| --- | --- |
| Command palette | `p` |
| Keybindings help | `?` |
| Copy mode | `[` |
| Hint mode | `u` |
| Scratchpad | backtick |
| Search scrollback | `/` |
| Profiles | `o` |
| Capture session as profile | `Shift+O` |
| Sessions | `s` |
| Rename or name current session | `Shift+S` |
| Take or request layout control | `g` |
| Grant layout control | `e` |
| Quit client | `q` |
| Detach | `d` |
| Toggle DevTools | `F12` |

Quit and detach run the same leave flow. Named sessions keep running. Temporary sessions follow the
rules in [Sessions](sessions.md#leave-rozi).

### Sidebar

| Command | Command key |
| --- | --- |
| Toggle sidebar | `b` |
| Toggle one or two panels | `\` |
| Focus sidebar | `Shift+B` |
| Next sidebar tab | `PageDown` |
| Previous sidebar tab | `PageUp` |

See [Sidebar](sidebar.md) for keys used after the sidebar has focus.

## Commands without default keys

These actions are available in the command palette or can be bound under `[keys]`:

- `toggle-pane-synchronization`
- `toggle-pane-logging`
- `respawn-pane`
- `focus-next-blocked-pane`
- `smart-focus-left`, `smart-focus-down`, `smart-focus-up`, `smart-focus-right`
- `settings`
- `extensions`
- `open-config`
- `reload-extensions`
- `apply-profile`
- `collaborators`
- `new-temporary-session`
- `toggle-input-lock`
- `toggle-control-takeover`
- `kill-workspace`
- `kill-session`
- `restart-session`
- `edit-scrollback`
- `copy-last-output`

Appearance actions are managed in **Settings** and remain bindable by their action ids. Run
`rozi run-action <id>` to invoke a stable action from automation. User-defined `[keys]` commands do
not have stable ids. Named `[[commands]]` entries do.

## Split-aware navigation

The `smart-focus-left`, `smart-focus-down`, `smart-focus-up`, and `smart-focus-right` actions let
one key set cross both editor splits and rozi panes. The
[vim-rozi-navigator](https://github.com/tui-lipan/vim-rozi-navigator) extension suggests these
bindings when they are free:

```toml
[keys]
smart-focus-left = "ctrl-h"
smart-focus-down = "ctrl-j"
smart-focus-up = "ctrl-k"
smart-focus-right = "ctrl-l"
```

The explicit block above is optional after installing that extension. Keep it when you want those
keys regardless of extension state, or to override a conflicting binding. An explicit entry for an
action, including `[]` to leave it unbound, suppresses the extension suggestion.

Rozi keeps the synchronous routing mechanism in core. On Linux and macOS it compares
`[navigation] editors` with every program sampled from the terminal's foreground process group, so
an editor remains discoverable behind a shell function, package runner, or other wrapper. On
platforms without process-group inspection, it falls back to the shell-reported command name. A
match forwards `Ctrl-h/j/k/l` to the terminal; otherwise Rozi moves pane focus itself. The editor
plugin handles its own split layout and calls a public
`rozi run-action focus-<direction>` action only when it reaches an outer edge. Integrations can use
the corresponding `-no-wrap` actions when focus should stay put at Rozi's outer edge.

An enabled extension may add static foreground-program names through `[[navigation_targets]]`.
Rozi validates and merges those declarations while loading configuration; no extension process
intercepts keys or participates in the input hot path. An explicit `[navigation] editors` entry,
including an empty list, replaces both built-in and extension-provided names completely.

Editor-specific behavior remains in normal editor packages. The package owns its split layout and
uses the same CLI action boundary for local and attached sessions, while the Rozi extension
manifest only describes routing policy. See the
[Vim and Neovim navigator](https://github.com/tui-lipan/vim-rozi-navigator) for an integration whose
repository contains both sides without making either installation own the other.

## Rebind a command

A bare command key follows the current prefix and modifier scheme:

```toml
[keys]
copy-mode = "b"
```

An explicit chord replaces the generated prefix and modifier forms:

```toml
[keys]
spawn = "ctrl-b c"
detach = "ctrl-a d"
```

Use `add` to retain defaults and add a binding:

```toml
[keys]
spawn = { add = "super-enter" }
```

A modified key is literal unless it starts with `scheme:`. For example,
`copy-mode = "scheme:ctrl-t"` generates both the prefix form and the held-modifier form. See
[Configuration](configuration.md#keys) for lists, `run`, `send`, and named commands.

## Resize mode

Press the resize command, then use:

| Key | Action |
| --- | --- |
| `h/j/k/l` or arrows | Resize toward that direction |
| `Esc` or `Enter` | Leave resize mode |

Other keys are consumed. Floating panes resize their rectangle. Scrollable panes change width only
on the horizontal axis.

## Copy mode

| Key | Action |
| --- | --- |
| `h/j/k/l` or arrows | Move the cursor |
| `w/b/e` | Move by word |
| `W/B/E` | Move by whitespace-delimited word |
| `0`, `^`, `$` | Start, first non-blank, or end of row |
| `Ctrl+U`, `Ctrl+D` | Half page up or down |
| `g`, `G` | Top of history or live bottom |
| `/` | Search this pane |
| `n`, `N` | Next or previous search match |
| `[`, `]` | Previous or next shell prompt |
| `o` | Copy the last command output |
| `v` or `Space` | Start a selection |
| `y` or `Enter` | Copy and exit |
| `Esc` or `q` | Exit |

Prompt jumps and last-output copying require shell-integration markers. See
[Terminal features](terminal.md#copy-search-and-hints).

## Hint mode

Hint mode labels visible URLs, paths, Git commit ids, and configured custom patterns. Type a
lowercase label to copy its target. Use an uppercase final label character to open an eligible URL
or custom target. `Esc` or `q` exits. All other input stays out of the PTY.

## Sidebar keys

After `Shift+B` focuses the sidebar:

| Key | Action |
| --- | --- |
| `j/k` or arrows | Move through rows |
| `PageUp`, `PageDown` | Move by a page |
| `g`, `G`, `Home`, `End` | First or last row |
| `Enter` | Activate the selected row |
| `Tab`, `Shift+Tab` | Next or previous tab |
| `h/l`, arrows, `Space` | Collapse, expand, or toggle file-tree directories |
| `Ctrl+Shift+Left/Right` | Reorder the active tab |
| `Ctrl+Up/Down` | Focus the other panel |
| `Ctrl+Shift+Up/Down` | Move the active tab between panels |
| `Shift+Left/Right` | Resize the sidebar |
| `Shift+Up/Down` | Resize the panel split |
| `s` | Toggle one or two panels |
| `Esc` | Return focus to the pane |

## Picker keys

The session picker uses `Enter` to connect, switch, or restore. `Ctrl+N` creates a named session,
`Ctrl+K` twice kills or forgets, `Ctrl+E` twice restarts a live session, `Ctrl+W` disconnects a
background attachment, `Ctrl+X` disconnects a remote host, `Ctrl+R` opens **Remote hosts**, and
`Ctrl+T` opens the temporary shell. Opening Sessions performs no remote probes. See
[Sessions](sessions.md#use-the-session-picker).

In **Remote hosts**, `Enter` probes only the selected host in place. While connecting, the row
shows a spinner, navigation is locked, and `Esc` cancels. `Ctrl+N` opens the shared new-host
prompt, and `Ctrl+K` twice forgets an offline Recent host. The host-scoped Sessions view uses
`Enter` to attach, `Ctrl+N` for a named session, `Ctrl+T` for a temporary session, `Ctrl+K` twice to
kill, `Ctrl+E` twice to restart, `Ctrl+W` to disconnect a retained attachment, and `Ctrl+X` to
disconnect from that host. `Esc` returns from host sessions to Remote hosts, then to Sessions.

The profile picker uses `Enter` for the same-name session, `Ctrl+O` to launch under another name,
`Ctrl+N` to capture, `Ctrl+R` twice to replace the current session, `Ctrl+F` to toggle the default,
and `Ctrl+D` twice to delete. See [Profiles](profiles.md#use-the-profile-picker).

## Other overlay keys

`Esc` closes an overlay, or returns to its parent overlay when one opened another. `Enter` activates
the selected row or submits a prompt.

- Layout picker: `Ctrl+F` saves the highlighted layout as the default.
- Scrollback search: `Enter` selects a result, `Ctrl+N` and `Ctrl+P` move among results, `Tab`
  changes scope, and `Esc` closes.
- Help: `Tab` and `Shift+Tab` cycle the tabs, and `Left` and `Right` do the same while the search
  field is not focused. The arrow and paging keys scroll the list. `/` searches the current help
  tab. `Enter` or `Esc` leaves the search field, and a second `Esc` closes help.
- Settings: arrows move through rows. `Left` and `Right` change a setting where the row supports
  stepping.
- Extensions: `Enter` enables or disables the selected extension, `Ctrl+D` opens details,
  `Ctrl+I` opens the install prompt, `Ctrl+U` updates a Git-managed installation, `Ctrl+R` reloads,
  `Ctrl+O` opens the manifest, `Ctrl+Y` copies the report, and `Ctrl+K` twice removes the
  installation. Details are read-only; use the arrow and paging keys to scroll, `Ctrl+U` to update,
  `Ctrl+Y` to copy the report, and `Ctrl+O` to open the manifest.
- Collaborators: `Enter` grants control, `Ctrl+D` declines a request, `Ctrl+K` twice removes a
  client, and `Esc` closes.
- Rename prompts: `Enter` submits. An empty pane or workspace name clears it.

## Platform caveats

`Alt` is the default held modifier because terminal emulators usually deliver it. Many desktop
environments reserve `Super`, and Windows intercepts most Windows-key chords before a console
application can receive them.

Windows Terminal and the classic console also intercept some `Alt` chords before Rozi sees them.
`Alt+Enter` toggles fullscreen. `Alt+Space` opens the window menu. Other host bindings can collide
with Rozi's direct shortcuts. The `Ctrl+A` prefix avoids those collisions. You can also unbind the
host keys you want Rozi to receive, or rebind the commands under `[keys]`.

Rozi runs Windows consoles in raw mode, so `Ctrl+C` reaches the focused pane. Closing the console
window detaches the client. Modified arrow and shifted-symbol reporting varies by terminal, so Rozi
registers the common forms used by its defaults.

Bare `F12` is left for pane applications. Rozi's DevTools command uses the prefix or held-modifier
form.
