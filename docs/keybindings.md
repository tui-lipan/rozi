# Keybindings

This page lists rozi's default keys and explains how to change them. To see the keys currently in
effect, including your overrides, press `Ctrl+A`, then `?` to open the **Keybindings** overlay. You
can edit bindings there or under `[keys]` in `config.toml`.

## Prefix and held modifier

Every command key in the tables below works in two ways by default:

- Press the prefix `Ctrl+A`, release it, then press the command key.
- Hold `Alt` and press the command key.

For example, close a pane with `Ctrl+A`, then `w`, or with `Alt+W`.

- Press `Ctrl+A` twice to send a literal `Ctrl+A` to the focused pane.
- `Esc` cancels a pending prefix.
- A key with no binding ends prefix mode and is not sent to the pane.

Set `[input] prefix`, `[input] modifier`, or `[input] modifier_shortcuts` to change this scheme.
With `modifier_shortcuts = false`, prefix commands keep working and held-modifier chords go to the
pane instead. See [Configuration](configuration.md#input) and
[Core concepts](core-concepts.md#the-prefix-and-the-modifier).

## Key notation

rozi writes keys the same way everywhere: on this page, in the **Keybindings** overlay, in footers,
and in the which-key strip.

| Kind | Notation | Examples |
| --- | --- | --- |
| Printable key with `Ctrl`, `Alt`, or `Super` | Every modifier written out; letters uppercase, case means nothing | `Ctrl+A`, `Ctrl+Shift+A`, `Alt+Shift+W`, `Ctrl+Shift+/` |
| Printable key on its own, such as after the prefix | The character it types | `s`, `S`, `?` |
| Named key | Every modifier written out | `Tab`, `Shift+Tab`, `Ctrl+Shift+Left` |

So `S` after the prefix means `Shift+S`, while `Ctrl+S` never includes `Shift`.

## Default command keys

### Panes and focus

| Command | Command key |
| --- | --- |
| New pane | `Enter` |
| New floating pane | `Shift+Enter` |
| Close pane | `w` |
| Toggle floating | `t` |
| Toggle fullscreen | `f` |
| Rename pane | `N` |
| Paste text | `v` |
| Paste text directly | `Ctrl+V` |
| Promote to master | `.` |
| Swap left, down, up, right | `H/J/K/L`, or `Shift` plus arrows |
| Move and reinsert left, down, up, right | `Ctrl+H/J/K/L`, or `Ctrl` plus arrows |
| Focus left, down, up, right | `h/j/k/l`, or arrows |
| Cycle focus forward or backward | `Tab` or `Shift+Tab` |

`Ctrl+V` pressed directly, without the prefix, pastes only when the clipboard holds text. For other
clipboard formats, it passes through to the pane. The prefix and held-modifier forms always paste
text.

### Layout and workspaces

| Command | Command key |
| --- | --- |
| Flip split axis | `Space` |
| Grow split or master area | `=` |
| Shrink split or master area | `-` |
| Resize mode | `r` |
| Cycle layout | `m` |
| Layouts | `M` |
| Rename workspace | `n` |
| Switch to workspace 1 through 9 | `1` through `9` |
| Move pane to workspace 1 through 9 | `Shift+1` through `Shift+9` |
| Move or swap whole workspace | `Ctrl+Shift+1` through `Ctrl+Shift+9` |

Terminals that report shifted symbols may send the shifted workspace keys as `!@#$%^&*(`. rozi
accepts both forms.

Bare `n` renames the workspace; `N` renames the pane. See
[Layouts and panes](layouts-and-panes.md) for layout behavior.

### App, profiles, sessions, and collaboration

| Command | Command key |
| --- | --- |
| Command palette | `p` |
| Keybindings help | `?` |
| Copy mode | `[` |
| Hint mode | `u` |
| Scratchpad | `` ` `` (backtick) |
| Search scrollback | `/` |
| Profiles | `o` |
| Save session as profile | `O` |
| Sessions | `s` |
| Agents | `a` |
| Rename or name current session | `S` |
| Take or request layout control | `g` |
| Grant layout control | `e` |
| Quit client | `q` |
| Detach | `d` |
| Toggle DevTools | `F12` |

Quit and detach do the same thing. Named sessions keep running; temporary sessions follow the rules
in [Sessions](sessions.md#leave-rozi).

### Sidebar

| Command | Command key |
| --- | --- |
| Toggle sidebar | `b` |
| Toggle one or two panels | `\` |
| Focus sidebar | `B` |
| Next sidebar tab | `PageDown` |
| Previous sidebar tab | `PageUp` |

See [Sidebar keys](#sidebar-keys) for the keys that work once the sidebar has focus.

## Commands without default keys

These actions are available in the command palette, and you can bind them under `[keys]`:

- `toggle-pane-synchronization`
- `toggle-pane-logging`
- `respawn-pane`
- `focus-next-blocked-pane`
- `smart-focus-left`, `smart-focus-down`, `smart-focus-up`, `smart-focus-right`
- `settings`
- `extensions`
- `update-rozi` (listed only when a newer release is known)
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
- `screenshot-pane` (listed only when a pane has focus)
- `screenshot-ui`
- `toggle-pane-recording` (listed when the focused pane can be recorded)
- `mark-pane-recording` (listed while the focused pane records)

Appearance actions live in **Settings** and can also be bound by their action ids.

To run an action from a script, use `rozi run-action <id>`. Built-in actions and named
`[[commands]]` entries have stable ids; inline commands defined directly under `[keys]` do not.

## Resize mode

After the `r` command key:

| Key | Action |
| --- | --- |
| `h/j/k/l` or arrows | Resize toward that direction |
| `Esc` or `Enter` | Leave resize mode |

Other keys are ignored. Floating panes resize their own rectangle. In the Scrollable layout, only
horizontal resizing applies, and it changes the focused column's width.

## Copy mode

After the `[` command key:

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

Prompt jumps and last-output copying need shell integration. See
[Terminal features](terminal.md#copy-search-and-hints).

## Hint mode

After the `u` command key, rozi labels visible URLs, paths, Git commit ids, and your custom
`[[hints]]` patterns.

- Type a lowercase label to copy its target.
- Type the label with an uppercase final character to open an eligible URL or custom target.
- `Esc` or `q` exits.

Other keys are not sent to the pane while hint mode is active.

## Sidebar keys

After the `B` command key focuses the sidebar:

| Key | Action |
| --- | --- |
| `j/k` or arrows | Move through rows |
| `PageUp`, `PageDown` | Move by a page |
| `g`, `G`, `Home`, `End` | First or last row |
| `Enter` | Activate the selected row |
| `x` | Close the selected row; press again to confirm |
| `Tab`, `Shift+Tab` | Next or previous tab |
| `h/l`, arrows, `Space` | Collapse, expand, or toggle file-tree directories |
| `Ctrl+Shift+Left/Right` | Reorder the active tab |
| `Ctrl+Up/Down` | Focus the other panel |
| `Ctrl+Shift+Up/Down` | Move the active tab between panels |
| `Shift+Left/Right` | Resize the sidebar |
| `Shift+Up/Down` | Resize the panel split |
| `s` | Toggle one or two panels |
| `Esc` | Return focus to the pane |

See [Sidebar](sidebar.md) for what each tab shows.

## Picker keys

### Sessions

| Key | Action |
| --- | --- |
| `Enter` | Connect, switch, or restore |
| `Ctrl+N` | Create a named session |
| `Ctrl+T` | Open the temporary shell |
| `Ctrl+K` twice | Kill a live session, or forget a snapshot or last-seen cache entry |
| `Ctrl+E` twice | Restart a live session |
| `Ctrl+W` | Disconnect a background attachment |
| `Ctrl+X` | Disconnect a remote host |
| `Ctrl+R` | Open **Remote hosts** |

Opening **Sessions** does not probe remote hosts. See
[Sessions](sessions.md#use-the-session-picker).

### Remote hosts

| Key | Action |
| --- | --- |
| `Enter` | Connect the selected host and stay on the list; press again to open a connected host |
| `Ctrl+N` | Add a host |
| `Ctrl+E` | Edit the selected host |
| `Ctrl+R` | Connect the selected host again |
| `Ctrl+K` twice | Forget the selected host |
| `Esc` | Cancel the connection in progress, or close **Remote hosts** when none is running |

Only one host connects at a time. While it does, `Enter` and `Ctrl+R` wait for it, but navigation,
`Ctrl+E`, and `Ctrl+K` still work on the other rows. In the host editor, `Tab` and `Shift+Tab` move
between lines.

A host's own **Sessions** view uses `Enter` to attach, `Ctrl+N` for a named session, `Ctrl+T` for a
temporary session, `Ctrl+K` twice to kill a live session or forget a last-seen cache entry, `Ctrl+E`
twice to restart, `Ctrl+W` to disconnect a retained attachment, and `Ctrl+X` to disconnect from the
host. `Esc` goes back to **Remote hosts**, then to **Sessions**.

### Agents

**Agents** lists every agent rozi knows about, on this machine and on every connected host, with
the ones that need attention first. `Enter` goes to the highlighted agent: rozi changes focus if
the agent is in the session on screen, or attaches to its session first if it is not. See
[Go to an agent](sessions.md#go-to-an-agent).

### Profiles

| Key | Action |
| --- | --- |
| `Enter` | Open the session with the profile's name |
| `Ctrl+O` | Launch under another name |
| `Ctrl+N` | Capture the current session |
| `Ctrl+R` twice | Replace the current session |
| `Ctrl+F` | Toggle the default profile |
| `Ctrl+D` twice | Delete the profile |

See [Profiles](profiles.md#use-the-profile-picker).

### Worktrees

**Worktrees** has no default command key. Open it from the command palette or from the sidebar's
Worktrees tab.

| Key | Action |
| --- | --- |
| `Enter` | Open the checkout's session |
| `Ctrl+N` | Create a checkout |
| `Ctrl+R` | Refresh |
| `Ctrl+K` | Remove a linked checkout |

In the new-worktree form, `Ctrl+E` adds a worktree directory inside the repository to
`.git/info/exclude` when Git does not already ignore it. See [Worktrees](worktrees.md).

## Other overlay keys

In every overlay, `Esc` closes it, or returns to the parent overlay if one opened another. `Enter`
activates the selected row or submits a prompt.

- **Layouts:** `Ctrl+F` saves the highlighted layout as the default.
- **Scrollback search:** the arrow and paging keys move through results, `Enter` selects one, and
  `Tab` changes the scope.
- **Settings:** `Tab`, `Shift+Tab`, and `Left`/`Right` switch categories, each remembering its last
  highlighted row. `Enter` toggles a two-option row. A row with more values shows `…` after its
  label; `Enter` opens a small picker where the highlight previews the value, `Enter` saves, and
  `Esc` restores the old one. `Shift+Enter` cycles the live value without opening the picker.
  **Theme** and **Terminal padding** have their own editors.
- **Extensions:** `Tab`, `Shift+Tab`, and `Left`/`Right` switch between **Installed** and
  **Discover**. See the list below.
- **Collaborators:** `Enter` grants control, `Ctrl+D` declines a request, and `Ctrl+K` twice removes
  a client.
- **Rename prompts:** `Enter` submits. An empty pane or workspace name clears it.

In **Extensions**:

| Key | Action |
| --- | --- |
| `Enter` on an installed row | Enable or disable it |
| `Ctrl+D` on an installed row | Open its report |
| `Enter` on a **Discover** row | Open its installation report |
| `Enter` again in a discovery report | Install the exact indexed commit |
| `Ctrl+I` | Open the manual install prompt |
| `Ctrl+U` | Update a Git-managed installation, or check again when no update is known |
| `Ctrl+R` | Rescan installed manifests on **Installed**; refetch the index on **Discover** |
| `Ctrl+O` | Open an installed manifest |
| `Ctrl+Y` | Copy an installed report |
| `Ctrl+K` twice | Remove an installation |
| `Ctrl+L` | Open the report's link: a discovery entry's source at its indexed commit, or an installed extension's homepage |
| Arrow and paging keys | Scroll details |

## Rebind a command

Bind actions under `[keys]` in `config.toml`. A bare command key follows the current prefix and
modifier scheme:

```toml
[keys]
copy-mode = "b"
```

An explicit chord replaces both the prefix and the held-modifier forms:

```toml
[keys]
spawn = "ctrl-b c"
detach = "ctrl-a d"
```

Use `add` to keep the defaults and add another binding:

```toml
[keys]
spawn = { add = "super-enter" }
```

A modified key is literal unless it starts with `scheme:`. For example,
`copy-mode = "scheme:ctrl-t"` creates both the prefix form and the held-modifier form.

`prefix:` and `mod:` use one half of the scheme. Unlike literal chords, they follow later changes to
the prefix or modifier:

```toml
[keys]
spawn = "prefix:enter"      # Ctrl+A Enter, and Ctrl+B Enter after a prefix change
copy-mode = "mod:b"         # Alt+B, dormant while modifier_shortcuts is off
detach = "ctrl-a d"         # always Ctrl+A D
```

See [Configuration](configuration.md#keys) for lists, `run`, `send`, and named commands.

## Edit keybindings in rozi

The **Keybindings** overlay (the `?` command key) edits bindings without opening `config.toml`.
Type to filter the list; the search field keeps focus while you navigate.

| Key | Action |
| --- | --- |
| `↑` / `↓`, `PageUp` / `PageDown`, `Home` / `End` | Move the selection |
| `←` / `→`, `Tab` / `Shift+Tab` | Switch tabs |
| `Enter` or click | Change the selected binding |
| `Ctrl+U` | Unbind the selected action |
| `Ctrl+D` | Reset the selected binding, Prefix, or Mod to its default |
| `Ctrl+R` | Reset every keybinding override, after confirmation |
| `Esc` | Close |

<CaptureGallery mode="steps" title="Keybindings">
<img src="./assets/captures/keys-find.webp" alt="The Keybindings overlay filtered to new pane, with the New pane command selected" data-label="Find the command" data-caption="Ctrl+A, then ?, opens Keybindings. Typing new pane narrows the list to the command.">
<img src="./assets/captures/keys-record.webp" alt="The recording card showing the chord Ctrl+Alt+N as the new key" data-label="Press the new key" data-caption="Enter starts recording. Pressing Ctrl+Alt+N shows the chord on the card before anything is saved.">
<img src="./assets/captures/keys-conflict.webp" alt="The recording card warning that Alt+W is already bound to Close pane" data-label="See conflicts" data-caption="A key that is already taken says so. Here Alt+W already closes the pane, so the card asks before taking it.">
<img src="./assets/captures/keys-saved.webp" alt="The Keybindings list showing New pane bound to Ctrl+Alt+N" data-label="Saved" data-caption="Enter saves the binding to config.toml, and it works at once. The row shows the new chord next to the default.">
</CaptureGallery>

### Record a binding

1. Select a row and press `Enter`.
2. Press the new key or chord. On terminals with enhanced keyboard support, held modifiers appear as
   you press them (`Ctrl+`, then `Ctrl+Shift+`, then `Ctrl+Shift+A`).
3. Review the captured binding. Press `Enter` to save it, or `Esc` to discard it and record again.
   Other keys are ignored at this step.

Press `Esc` before capturing anything to close the card without changing the binding.

rozi saves exactly the modifiers the terminal reports. It never adds `Shift` because a letter is
uppercase, since Caps Lock also produces capitals. Terminals without enhanced keyboard reporting
send `Ctrl+A` and `Ctrl+Shift+A` identically, so both record as `Ctrl+A` there.

Each change is written to `config.toml` at once and takes effect immediately. A recorded command key
such as `Enter` is saved as `"enter"` and follows the scheme; a chord with `Ctrl`, `Alt`, or `Super`
is saved literally.

### Resolve conflicts

If the new binding already belongs to another action, the card lists every conflict. Recording the
prefix key itself reports a conflict with Prefix rather than with every command after it.

- Press `Enter` to take the binding. Only the colliding chords move: taking `Alt+Enter` from New
  pane leaves it bound to `"prefix:enter"`.
- Press `Esc` to record a different binding.

You cannot take the prefix key for a command this way. Change Prefix first.

### Change the prefix or modifier

- The **Prefix** row records one key, like any other binding.
- The **Mod** row opens a chooser. Use `←` / `→` to pick `Alt`, `Super`, or `Off`, then press
  `Enter`. `Off` turns off held-modifier shortcuts and remembers the modifier for when you turn them
  back on.

Every command key that follows the scheme moves with the new Prefix or Mod. Literal chords such as
`ctrl-a q` stay as written. If some literal chords use the old Prefix or Mod, the card counts them,
and `Tab` offers to convert them to `prefix:` or `mod:` forms so they move too. Nothing is converted
unless you turn that on.

rozi refuses a Prefix or Mod change that would make two commands collide, and lists the colliding
pairs.

### Read the list

- An unchanged binding shows its keys.
- An override shows `current ← default`. An unbound override shows `— ← default`.
- **Unbind** moves the action to the **Unbound** tab.
- Workspace ranges, mouse gestures, and the **Modes** tab are reference only.

Built-in actions and named `[[commands]]` entries are editable. Inline `run` and `send` entries in
`[keys]` have no stable action id, so you can change them only in `config.toml`.

## Split-aware navigation

The `smart-focus-left`, `smart-focus-down`, `smart-focus-up`, and `smart-focus-right` actions let
one set of keys move between editor splits and rozi panes. The
[vim-rozi-navigator](https://github.com/tui-lipan/vim-rozi-navigator) extension suggests these
bindings when they are free:

```toml
[keys]
smart-focus-left = "ctrl-h"
smart-focus-down = "ctrl-j"
smart-focus-up = "ctrl-k"
smart-focus-right = "ctrl-l"
```

With that extension installed, this block is optional. Keep it to use these keys whatever the
extension's state, or to override a conflicting binding. Any explicit entry for these actions,
including `[]` to leave one unbound, replaces the extension's suggestion.

When you press a smart-focus key, rozi checks whether the focused pane is running an editor listed
in `[navigation] editors`:

- If it is, rozi forwards `Ctrl+H/J/K/L` to the pane, and the editor moves between its own splits.
  At an outer edge, the editor plugin calls `rozi run-action focus-<direction>` to move rozi's
  focus. Integrations that should not wrap around at rozi's outer edge can call
  `focus-<direction>-no-wrap` instead, such as `focus-left-no-wrap`.
- If it is not, rozi moves pane focus itself.

On Linux and macOS, rozi checks every program in the pane's foreground process group, so it finds an
editor started through a shell function, package runner, or other wrapper. On other platforms, it
uses the command name the shell reports.

An enabled extension can add program names through `[[navigation_targets]]`. An explicit
`[navigation] editors` entry, even an empty list, replaces both the built-in and the extension
names.

The editor plugin manages its own splits and uses the same `rozi run-action` calls in local and
attached sessions. See the
[Vim and Neovim navigator](https://github.com/tui-lipan/vim-rozi-navigator), whose repository
contains both the editor plugin and the rozi extension.

## Platform caveats

`Alt` is the default held modifier because terminal emulators usually deliver it. Many desktop
environments reserve `Super`, and Windows intercepts most Windows-key chords before a console
application receives them.

Windows Terminal and the classic console also intercept some `Alt` chords: `Alt+Enter` toggles
fullscreen, and `Alt+Space` opens the window menu. Other host bindings can collide with rozi's
held-modifier shortcuts. To avoid this, use the `Ctrl+A` prefix, unbind the host keys you want rozi
to receive, or rebind the commands under `[keys]`.

On Windows, `Ctrl+C` reaches the focused pane, and closing the console window detaches the client.
Terminals report modified arrows and shifted symbols differently, so rozi registers the common
forms of its default keys.

A bare `F12` goes to the pane application. rozi's DevTools command uses only the prefix or
held-modifier form.
