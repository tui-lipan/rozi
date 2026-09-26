# Configuration

This page is the reference for `config.toml`, the one TOML file that holds rozi's settings: where
it lives, how it reloads, and every key by section. For a guided tour of themes, layouts, pane
styles, and keys, see [Customize rozi](customize.md).

Every key is optional; anything you leave out uses its default.
[`examples/config.toml`](../examples/config.toml) lists every setting commented out, so you can copy
it and uncomment only what you need.

## File location

rozi uses the first of these that applies:

1. `--config <PATH>`, which also sets `ROZI_CONFIG` for the process.
2. `ROZI_CONFIG`. `~` and `~/…` expand to the home directory.
3. `$XDG_CONFIG_HOME/rozi/config.toml`, or `~/.config/rozi/config.toml`.
4. `%APPDATA%\rozi\config.toml` on Windows.

`--config` works with launches, session servers, extension inspection, and the session lifecycle
commands that load configuration. Control commands do not load configuration and reject
`--config`.

## Minimal example

```toml
cwd = "~/code"

[input]
modifier = "super"
modifier_shortcuts = false

[layout]
default = "columns"

[theme]
name = "lipan"
```

## Edit settings in rozi

Changes made in rozi's Settings, Appearance, Profiles, or Themes UI are written to `config.toml`
and take effect at once. rozi replaces the whole file on save, so an interrupted write cannot leave
it truncated. If `config.toml` is a symlink, rozi writes the file it points to and keeps the link.

**Settings…** in the command palette browses and previews most options by category. It has no
default key; bind the `settings` action under [`[keys]`](#keys) to add one. See
[Use the Settings picker](customize.md#use-the-settings-picker).

### Open the file in an editor

The `open-config` action opens the config file in `EDITOR`, then `VISUAL`, then `vi`:

```bash
rozi run-action open-config
```

The editor runs directly, not through a shell. rozi splits the variable into a program and its
arguments, honoring quotes around a path that contains spaces (`"/opt/my editor/bin/edit" --wait`),
and passes the config path as a separate argument. Shell syntax in `EDITOR` — pipes, redirection,
variable expansion — is not interpreted.

## Reloading

rozi watches the config file and applies changes without replacing panes or workspaces. If the new
file cannot be used (see [Invalid configuration](#invalid-configuration)), rozi keeps the last good
configuration.

rozi does not watch installed extension directories. After installing, updating, or removing an
extension, rescan them:

```bash
rozi run-action reload-extensions
```

### When a change takes effect

Most settings apply as soon as you save. These take effect later or more narrowly. A
[session server](core-concepts.md#sessions-and-clients) is the background process that owns a
session's panes.

| Setting | When it takes effect |
| --- | --- |
| `shell`, `shell_integration.mode`, `cwd`, `environment.forward` | New panes only. |
| `command_shell` | New command, hook, service, sidebar, and workbar executions. |
| `scrollback` | New terminal screens. Existing screens never resize; restart an existing session server before creating panes that should use the new capacity. |
| `frame_rate` | Next client launch or reattach. |
| `updates.interval_hours` | The next re-check. A check already waiting keeps the old interval. |
| `sidebar.visible` | Client startup only. Reload never opens or closes the sidebar. |
| `session.startup` | Next bare launch. |
| `session.resurrect`, `session.resurrect_foreground`, `session.resurrect_agents`, `session.allow_takeover` | Session servers started after the change. |
| `logging.*` | Session servers started after the change. |
| `remote.*` | New SSH connections. |
| `rules` | New pane spawns that carry a command. |
| `services` | On reload. Changed services restart, removed services stop, and unchanged services keep running. |
| `agents` | On reload, in the controlled session server and the local scratch session. |
| `extensions.disabled` | When the config file changes. |
| Extension manifests | After `reload-extensions`. |

## Invalid configuration

rozi handles problems at two levels:

- **The whole file is rejected** when it is unreadable, is not valid TOML, or has a value of the
  wrong type. At startup rozi uses the defaults; on a live reload it keeps the last good
  configuration. Either way it shows an error.
- **One setting is skipped** when a key is unknown or a value is not one of the allowed choices.
  rozi uses that setting's default, applies the rest of the file, and warns.

Several warnings share one toast, and each is also printed to stderr. Some settings are clamped
into range rather than skipped; their rows below say so.

## In-app toasts

rozi shows a toast for failures, rejected actions, destructive confirmations, and results that
have no other visible feedback. Changes that already show up in the workbar, pane layout, a picker,
or the sidebar do not add a toast. A repeated message renews the existing toast instead of stacking
copies.

Scripts can show their own result with [`rozi notify`](control.md#actions-status-and-notifications).
Release notices are described in [Update notices](#update-notices).

## User directories

| Purpose | Linux and macOS | Windows |
| --- | --- | --- |
| Config | `$XDG_CONFIG_HOME/rozi`, else `~/.config/rozi` | `%APPDATA%\rozi` |
| Data, including extensions | `$XDG_DATA_HOME/rozi`, else `~/.local/share/rozi` | `%LOCALAPPDATA%\rozi` |
| State | `$XDG_STATE_HOME/rozi`, else `~/.local/state/rozi` | `%LOCALAPPDATA%\rozi\state` |
| Cache | `$XDG_CACHE_HOME/rozi`, else `~/.cache/rozi` | `%LOCALAPPDATA%\rozi\cache` |
| Runtime endpoints | `$XDG_RUNTIME_DIR/rozi`, else `/run/user/<uid>/rozi`, else a private per-user temporary directory | `%LOCALAPPDATA%\rozi\run` |

`XDG_*` values must be absolute paths; relative values are ignored.

`XDG_RUNTIME_DIR` is often unset under Tailscale SSH, `su`, or cron. rozi then uses
`/run/user/<uid>` if that directory exists, belongs to you, and is private, so a session server
started there is visible to your desktop clients. Otherwise its sessions would be invisible to
those clients and would appear only as restorable.

## Top-level keys

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `shell` | string or string array | Platform shell | A string is one program with no arguments; an array keeps each argument. Unix uses `SHELL`, then `/bin/sh`. Windows tries `pwsh.exe`, `powershell.exe`, `COMSPEC`, then `cmd.exe`. |
| `command_shell` | string or string array | `["/bin/sh", "-c"]` on Unix, `[COMSPEC, "/D", "/S", "/C"]` on Windows | Runs command strings for panes, popups, hooks, services, workbar and sidebar commands, and config commands. |
| `cwd` | path string | Launch directory | Starting directory for new panes. `~` expands. |
| `scrollback` | integer | `5000` | Minimum `1`. |
| `frame_rate` | integer | `120` | Clamped to `15..=480` with a warning. |
| `nerd_icons` | bool | `true` | Uses [Nerd Font](https://www.nerdfonts.com/) glyphs in rozi's chrome. |

With `nerd_icons` on, rozi draws private-use glyphs for pane title icons, workbar location and
named-session badges, the Sessions sidebar client-count badge, directory chevrons, the Files
explorer search prefix, and `round` and `arrow` caps. With it off, those badges use `⌁`, `∞`, and
`⋈`, directory chevrons use `▶` and `▼`, the explorer prefix uses `⌕`, and pane titles drop the
icon. File icons also need a sidebar tree tab with `icons = true`.

New local panes receive `ROZI=1`, `ROZI_PANE`, `ROZI_SESSION_INSTANCE`, and, when available,
`ROZI_SOCKET` and `ROZI_BIN`.
See [Scripting](scripting.md) and [Control CLI](control.md).

<a id="shell-integration-settings"></a>

## `[shell_integration]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `mode` | string | `"auto"` | `"auto"` or `"off"`. Auto adds shell integration to recognized interactive shells without editing their startup files. |

Shell integration lets the shell report its working directory (OSC 7) and mark prompts and command
output (OSC 133). See
[Working directories and shell metadata](terminal.md#working-directories-and-shell-metadata).

## `[environment]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `forward` | array of strings | `[]` | Extra client environment variables to copy into new local panes. Empty names are removed and duplicates collapsed. Values are not persisted or forwarded through remote attachments. |

rozi already forwards the desktop session variables that Wayland, X11, D-Bus, and Hyprland need.
Existing panes keep their original environment.

## `[input]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `modifier` | string | `"alt"` | `"alt"` or `"super"`. `mod` is an alias for Alt; `meta`, `logo`, `win`, and `windows` are aliases for Super. |
| `prefix` | string | `"ctrl-a"` | One key step in [config key syntax](keybindings.md#key-notation), such as `"ctrl-b"`. |
| `modifier_shortcuts` | bool | `true` | Mirrors the generated prefix bindings onto held-modifier chords. |
| `which_key` | string | `"short"` | Delay before the which-key strip appears: `"off"`, `"instant"`, `"short"` (500 ms), or `"long"` (1000 ms). |

See [Keybindings](keybindings.md), and
[Prefix and held modifier](keybindings.md#prefix-and-held-modifier) for the which-key strip.

## `[layout]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `split_width_multiplier` | float | `2.3` | Must be finite and greater than zero. |
| `default` | string | `"dwindle"` | `"dwindle"`, `"master"`, `"grid"`, `"columns"`, `"rows"`, `"scrollable"`, or `"monocle"`. Profiles may override it per workspace. |

See [Layouts and panes](layouts-and-panes.md).

## `[pane]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `resize_debounce_ms` | integer | `16` | Minimum delay between batches of terminal resize reports. `0` forwards each report. |
| `focus_on_hover` | bool | `true` | Focuses a pane when the pointer enters it. In a Scrollable layout, a clipped column scrolls into view on the next key or click, not on hover. |
| `focus_on_hover_pause_modifier` | string | `"shift"` | Holding this modifier pauses hover focus; click focus still works. `"shift"`, `"ctrl"`, `"alt"`, or `"none"` for no pause. Other modifiers may be held too. |
| `hold_on_exit` | bool | `false` | Keeps shell panes open after they exit on their own. Command panes use their `keep_open` value. |
| `highlight_focused_background` | bool | `false` | Uses the panel background for the focused pane. |
| `highlight_focused_border` | bool | `true` | Uses the active border color for the focused pane. |
| `highlight_focused_titlebar` | bool | `true` | Uses focused titlebar styling. |
| `show_workbar` | bool | `true` | Shows the workbar. |
| `workbar_gap` | bool | `true` | Keeps one row between the workbar and panes. |
| `workbar_background` | bool | `true` | Paints the workbar as a distinct strip: `panel` on a solid canvas, `element` when the canvas follows the terminal. Off, the bar matches the canvas. |
| `workbar_at_bottom` | bool | `false` | Places the workbar below panes. |
| `show_titles` | bool | `true` | Shows pane titles without changing `titlebar`. |
| `titlebar` | string | `"bar"` | `"bar"`, `"border"`, `"integrated"`, or `"inset"`. |
| `border_mode` | string | `"separate"` | `"separate"`, `"merged"`, `"none"`, or `"dividers"`. |
| `alert_border` | string | `"pulse"` | `"off"`, `"static"`, or `"pulse"`. |
| `border_style` | string | `"rounded"` | Frame glyphs for tiled panes in framed modes. See the token list below. |
| `float_border_style` | string | `"double"` | Same tokens as `border_style`. Floating panes and popups. |
| `scratch_border_style` | string | `float_border_style` | Same tokens as `border_style`. Scratchpad panes. |
| `fullscreen_border_style` | string | `border_style` | Same tokens as `border_style`. Fullscreen panes. |
| `picker_border_style` | string | `"rounded"` | Same tokens as `border_style`. The command palette, Settings, Help, Search, and other pickers. |
| `picker_tab_background` | bool | `true` | Paints picker category tabs as a distinct strip, lifted like the sidebar tab strip. Off, the tabs share the picker body. |
| `picker_tab_style` | string | `workbar_tab_style` | `"padded"`, `"round"`, or `"arrow"`. |
| `picker_selection_style` | string | `"padded"` | `"padded"`, `"round"`, or `"arrow"`. Caps at the start and end of the selected picker row. |
| `keep_special_borders` | bool | `true` | Keeps frames on floating panes, popups, and scratchpads in borderless modes. |
| `padding` | integer or integer array | `0` | One value, `[vertical, horizontal]`, or `[top, right, bottom, left]`. Each side is clamped to `0..=8`. |
| `title_style` | string | `"padded"` | `"padded"`, `"half"`, `"round"`, or `"arrow"`. |
| `workbar_badge_style` | string | `"padded"` | `"padded"`, `"round"`, or `"arrow"`. Also sets the tab style when `workbar_tab_style` is absent. |
| `workbar_tab_style` | string | `workbar_badge_style` | `"padded"`, `"round"`, or `"arrow"`. |
| `workbar_style` | string | `"padded"` | `"padded"`, `"half"`, `"round"`, or `"arrow"`. |
| `workbar_powerline` | bool | `true` | Joins trailing workbar badges. |
| `toast_opacity` | float | `0.8` | Finite value in `0.0..=1.0`. Invalid values are ignored. |
| `background_follows_terminal` | bool | `false` | Uses the host terminal's background for the canvas. |

The border style tokens are `"rounded"`, `"plain"`, `"double"`, `"thick"`, `"light-double-dashed"`,
`"heavy-double-dashed"`, `"light-triple-dashed"`, `"heavy-triple-dashed"`,
`"light-quadruple-dashed"`, and `"heavy-quadruple-dashed"`. A default written as another key's name
means the setting follows that key when omitted.

See [Layouts and panes](layouts-and-panes.md), [Sidebar](sidebar.md), and [Themes](themes.md).

### `[pane.alert]`

Border colors for pane alert states. Each value is a theme role or `"off"`. Theme roles are
`accent`, `info`, `success`, `warning`, `error`, `neutral`, and `panel`.

| Key | Type | Default |
| --- | --- | --- |
| `blocked` | string | `"error"` |
| `finished` | string | `"success"` |
| `working` | string | `"off"` |
| `idle` | string | `"off"` |

## `[animations]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `enabled` | bool | `true` | Master switch. |
| `spawn` | bool | `true` | Animates pane creation. |
| `close` | bool | `true` | Animates pane close. |
| `fullscreen` | bool | `true` | Animates fullscreen transitions. |
| `tile_float` | bool | `true` | Animates tile and float transitions. |
| `axis_change` | bool | `true` | Animates split-axis changes. |
| `sidebar` | bool | `true` | Animates sidebar movement. |
| `workspace` | bool | `true` | Slides workspace content horizontally when switching. In Settings: General › Animations › Workspace switching. |
| `workspace_ms` | integer | `220` | Workspace slide duration in milliseconds. `0` switches instantly. |
| `session` | string or bool | `"fade"` | `"fade"`, `"portal"`, or `"off"`; `true` means `"fade"` and `false` means `"off"`. Described below. In Settings: General › Animations › Session switching. |
| `focus_chrome` | bool | `true` | Animates focus color changes and enables alert pulses. |
| `pane_style` | string | `"scale"` | `"off"`, `"scale"`, `"slide"`, `"portal"`, or `"scan"`, case-insensitive. Unknown values fall back to `"scale"` with a warning. |
| `geometry_ms` | integer | `220` | Base geometry duration in milliseconds. |
| `close_ms` | integer | `120` | Scale close duration in milliseconds. Tiled Slide, Portal, and Scan use `geometry_ms`; floating Slide uses Scale timing. |
| `focus_chrome_ms` | integer | `160` | Focus color duration in milliseconds. |
| `alert_pulse_ms` | integer | `1600` | Alert pulse period. The half-period is at least 400 ms. |
| `open_delay_ms` | integer | `36` | Spawn animation delay in milliseconds. |

`session` controls how the workbar and panes arrive when the foreground session changes: when you
switch sessions, when a session finishes connecting, or when you drop to the launcher.

- `"fade"` brings in the new session in place from slightly dimmed, over one and a half times
  `geometry_ms` (330 ms by default).
- `"portal"` opens a portal from the center onto the new session while the previous one recedes
  behind it, over `geometry_ms`. The ring uses the theme's accent colors.
- `"off"` switches at once.

Pane geometry always snaps and the sidebar stays still. The value is case-insensitive; an unknown
value keeps the fade and warns.

`pane_style = "off"` shows or hides a pane at once, with no fade and no spawn delay. `spawn`,
`close`, and `enabled` still decide whether neighboring panes animate, and they still reflow over
`geometry_ms`.

### Pane animation curves and effect settings

The four pane effects — Scale, Slide, Portal, and Scan — accept their own timing, motion curve, and
geometry parameter. These keys also live directly under `[animations]`:

```toml
[animations]
pane_style = "slide"
geometry_ms = 130
close_ms = 90
curve = [0.16, 1.0, 0.3, 1.0]
```

| Key | Type | Default | Applies to |
| --- | --- | --- | --- |
| `curve` | curve | the style's own | all |
| `close_curve` | curve | the reverse of `curve` | all |
| `fade` | bool | `true` | Scale, Portal, Scan |
| `scale_from` | float in `[0.1, 1]` | `0.9` | Scale |
| `portal_origin` | float pair in `[0, 1]` | `[0.5, 0.5]` | Portal |
| `scan_direction` | `top-left`, `top-right`, `bottom-left`, `bottom-right` | `top-left` | Scan |

A curve is either CSS cubic-Bézier control points, `[x1, y1, x2, y2]`, or the name of a built-in
easing: `linear`, `ease_in_quad`, `ease_out_quad`, `ease_in_out_cubic`, or `ease_in_out_sine`. The
x coordinates must be in `[0, 1]`. The y coordinates must be finite and in `[-4, 4]`; a value above
`1` overshoots and settles back. Without `close_curve`, closing runs `curve` backwards.

A config can set all three geometry keys at once; only the one for the current `pane_style` is
used. Slide has no geometry key: a pane slides in from the side its split placed it on. `fade` has
no effect on Slide, which stays inside its tile and is always fully opaque.

A value outside its range is dropped with a warning, not clamped. Each key is checked on its own, so
one bad value does not affect the others.

These keys cover only a pane arriving and leaving: its effect, its opacity, the spawn delay, and how
long a closing pane stays visible. Tiles rearranging around that pane, and fullscreen, tile/float,
and axis-change transitions, use `geometry_ms`. The sidebar, scratchpad, and focus colors keep
their own settings. A config change does not alter an animation that is already running.

See [Pane open and close animation styles](layouts-and-panes.md#pane-open-and-close-animation-styles).

## `[theme]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `name` | string | `"rozi"` | A built-in theme ID, `"system"`, or a file name (without extension) from the themes directory. An active custom theme reloads when its file changes. |

See [Themes](themes.md).

## `[profile]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `default` | string | none | Profile used when nothing with higher precedence chooses one. |

See [Profiles](profiles.md).

## `[worktrees]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `directory` | path | none | Where a new checkout goes when you give no path. An absolute path, or a single folder name kept inside the repository. |
| `profile` | string | none | Profile that seeds a new worktree session, from the Worktrees picker or `rozi worktrees open`. |

Without `directory`, checkouts go beside the repository in `<repo>-worktrees/<branch>`. Otherwise:

- An absolute path (after `~` expansion) holds every repository's checkouts as
  `<directory>/<repo>/<branch>`.
- A single folder name stays inside the repository: `".worktrees"` gives
  `<repo>/.worktrees/<branch>`.
- A nested or escaping value such as `"tools/.worktrees"` or `"../worktrees"` is ignored with a
  warning. Use an absolute path for a location outside the repository.

While Git does not ignore that directory, rozi warns and offers to add it to `.git/info/exclude`.
`directory` is read on the session host: a running session server uses the value it started with,
and a remote session uses the remote host's config.

With `profile` set, pane directories inside any checkout of the repository move to the new
checkout, directories outside it are kept, and panes without a directory start in the checkout. For
a worktree on a remote host, the profile applies only when every pane directory it names is inside
the repository, since an outside path names a directory on this machine. Otherwise the session
starts as one shell in its checkout. rozi reports why a profile failed to load or was skipped.

See [Worktrees](worktrees.md#configuration).

## `[clipboard]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `copy_on_select` | string | `"both"` on Linux, `"clipboard"` elsewhere | Where a finished mouse selection is copied: `"off"`, `"primary"`, `"clipboard"`, or `"both"`. |
| `middle_click_paste` | string | `"primary"` on Linux, `"off"` elsewhere | What a middle click pastes: `"off"`, `"primary"`, or `"clipboard"`. |
| `right_click` | string | `"off"` | What an otherwise unhandled right click does: `"off"`, `"paste"` from the clipboard, or `"copy-or-paste"`. |
| `enable_osc52` | bool | `true` | Lets programs in panes set the system clipboard with OSC 52, a terminal escape sequence for clipboard access. |

PRIMARY is the Linux selection clipboard. Where PRIMARY is unsupported, `copy_on_select` stops
using it but keeps copying to the regular clipboard, and a middle click that pastes PRIMARY does
nothing.

Middle-click and right-click paste go to the pane or text field under the pointer, which takes
focus first. A click anywhere else pastes nothing. `"copy-or-paste"` copies the selection in the
clicked pane, flashing it as `Ctrl+C` does, and pastes when there is no selection. Handlers in
rozi's widgets and in the program running in the pane take priority over `right_click`.

These settings apply on reload and are also in the General category of Settings. See
[Select, copy, and paste](terminal.md#select-copy-and-paste).

## `[updates]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `check` | bool | `true` | Looks for a newer release shortly after a client starts, then every `interval_hours`. Off, rozi never contacts the release host on its own. |
| `interval_hours` | integer | `6` | Hours between re-checks in a client that stays open. Minimum `1`; lower values are clamped with a warning. |

`rozi update --check` checks on demand, whether or not `check` is on. Turning `check` back on
schedules the next check one interval later, not immediately.

### Update notices

When a newer release exists, rozi shows a toast titled `rozi vX.Y.Z available` with the version
change and how to update. It stays up for 15 seconds; click it to dismiss it sooner. If the release
raises the extension API or session protocol version, the toast uses warning colors and names the
change. Each release is announced only once across clients, and a failed check shows nothing.

Every client that finds the release also lists **Update rozi to vX.Y.Z** (`update-rozi`) in the
command palette for as long as it runs:

1. Choosing it opens a popup that runs `rozi update`, or the upgrade command of the package manager
   that installed rozi. The popup stays open to show the result.
2. On success, a `rozi vX.Y.Z installed` toast asks you to quit and start rozi again, because the
   running client keeps its old build.
3. On failure, the command stays listed so you can retry.

The command is not listed while a remote session is in front, because the popup would run on that
host, or when rozi was installed from a distribution package or in a way it does not recognize.

## `[notifications]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `enabled` | bool | `false` | Master switch for desktop notifications. |
| `pane_exit` | bool | `false` | Notifies when a pane exits on its own with status zero. |
| `pane_exit_error` | bool | `true` | Notifies when a pane exits on its own with a nonzero status. |
| `pane_blocked` | bool | `true` | Notifies when an unattended pane becomes blocked. |
| `pane_done` | bool | `false` | Notifies when a pane you have not looked at goes from working to finished. |
| `bell` | bool | `true` | Marks an unattended pane urgent when it rings the terminal bell (BEL). Works even when `enabled` is off. |

Desktop notifications go through the platform's notification service and are best effort.

`pane_blocked` and `pane_done` also cover agents in other local sessions and in sessions on a
[connected remote host](remote.md#agents-on-a-machine-you-are-not-in) that this client is not
attached to. Because nothing on screen shows those panes, their alerts skip the checks for who
controls the pane and whether you are looking at it.

## `[sounds]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `enabled` | bool | `false` | Master switch for sounds. |
| `bell` | bool | `true` | Enables the bell cue. |
| `blocked` | bool | `true` | Enables the blocked cue. |
| `done` | bool | `true` | Enables the done cue. |
| `error` | bool | `true` | Enables the error cue. |
| `throttle_ms` | integer | `2000` | Clamped to `100..=60000` with a warning. |
| `bell_file` | path string | empty | WAV file that replaces the bell cue. `~` expands. |
| `blocked_file` | path string | empty | WAV file that replaces the blocked cue. `~` expands. |
| `done_file` | path string | empty | WAV file that replaces the done cue. `~` expands. |
| `error_file` | path string | empty | WAV file that replaces the error cue. `~` expands. |
| `player` | string | empty | Player executable. rozi passes the cue file as its last argument. |

## `[navigation]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `editors` | array of strings | Built-in and extension targets | Programs that receive split-aware navigation keys. Setting it replaces the whole list; `[]` turns split-aware detection off. |

When `editors` is omitted, rozi uses its built-in list of foreground programs plus any targets from
enabled extensions. Empty names are removed. Names are matched against the program's executable
name, ignoring case and a Windows `.exe` suffix.

See [Split-aware navigation](keybindings.md#split-aware-navigation).

## `[confirm]`

Each switch controls whether rozi asks before a destructive action. They apply to key bindings and
`rozi run-action`. Choosing a command from the command palette follows its own confirmation path.

| Key | Type | Default | Asks before |
| --- | --- | --- | --- |
| `close_pane` | bool | `false` | Closing a pane whose program is still running. |
| `kill_workspace` | bool | `true` | Killing a workspace. |
| `kill_session` | bool | `true` | Killing a session. |
| `quit_ephemeral` | bool | `true` | Closing a used temporary session when you leave it (the second empty-name confirmation; see [Sessions](sessions.md)). |
| `new_temporary_session` | bool | `true` | Replacing a temporary session only this client uses with a new one. |
| `load_profile` | bool | `true` | Loading a profile over a temporary session with live panes. |

## `[session]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `autosave` | bool | `false` | Saves and restores the local layout, not running programs. |
| `resurrect` | bool | `true` | Saves each named session's layout, commands, scrollback, and what to restart. |
| `resurrect_foreground` | string | `"auto"` | What a restore does with a command a pane was seen running, or an agent conversation it reported: `"auto"` runs it again, `"hold"` types it at the prompt without submitting it, `"never"` restores only the shell and records neither. |
| `resurrect_agents` | bool | `true` | Saves agents' native session references in named-session snapshots and reopens those conversations on restore. Set `false` to keep these references out of state storage. |
| `startup` | string | `"picker"` | What a bare `rozi` launch opens: `"picker"`, `"ephemeral"` (a temporary session), `"last"`, or `"profile"`. |
| `path` | path string | `session.toml` in the state directory | Autosave file. `~` expands. The default file is written with mode `0600` in rozi's state directory; a path you choose keeps ordinary permissions. |
| `allow_takeover` | bool | `true` | Lets a writable follower take layout control immediately. |

See [Sessions](sessions.md).

## `[remote]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `default_host` | string | none | Host used by `--remote` without a value. |
| `connection_timeout_secs` | integer | `15` | SSH `ConnectTimeout`. |
| `server_alive_interval_secs` | integer | `15` | Minimum `1`. |
| `server_alive_count_max` | integer | `3` | Minimum `1`. |
| `install` | string | `"prompt"` | Whether to install rozi on the remote host: `"prompt"`, `"always"`, or `"never"`. Noninteractive runs never install. |
| `batch_mode` | bool | `true` | Sets SSH `BatchMode=yes`. When `false`, a running client answers SSH prompts in a dialog; see [Remote sessions](remote.md#prompts-inside-the-ui). |

### `[remote.hosts.<alias>]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `host` | string | The alias | SSH hostname. |
| `user` | string | SSH default | Login user. |
| `port` | integer | SSH default | `0` is ignored. |
| `identity_file` | path string | none | SSH identity file. |
| `ssh_args` | array of strings | `[]` | Extra SSH arguments. |
| `binary_path` | string | none | Absolute path to rozi on the remote host. Skips probing and installation. |

See [Remote sessions](remote.md).

## `[scratchpad]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `command` | string | Normal shell | Command for the first scratch pane. |
| `cwd` | path string | Focused local pane's directory, then the configured `cwd` | `~` expands. Read when the scratchpad is first created. |
| `height` | float | `0.4` | Docked height as a fraction of the pane area. Clamped to `0.1..=0.9` with a warning. Does not limit floating or fullscreen scratch panes. |

See [Popups and scratch panes](layouts-and-panes.md#popups-and-scratch-panes).

## `[sidebar]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `visible` | bool | `false` | Whether the sidebar is open at startup. |
| `width` | integer | `32` | Clamped to `16..=80`. |
| `position` | string | `"left"` | `"left"` or `"right"`. |
| `tabs` | array | `["activity", "panes", "sessions", "files", "git", "worktrees"]` | Replaces the list of available tabs. IDs must be unique. |
| `panels` | array of one or two string arrays | `[["activity", "panes", "sessions"], ["files", "git", "worktrees"]]` | Orders tab IDs into panels. Unknown and duplicate IDs are skipped. |
| `split` | bool | `true`, or inferred from the number of panels | Shows two panels. |
| `split_ratio` | float | `0.4` | Finite value clamped to `0.15..=0.85`. |
| `background_follows_canvas` | bool | `false` | Paints the sidebar with the canvas background instead of the raised panel fill. |
| `gap` | bool | `true` | Keeps one row between each panel's tab bar and its list. |
| `background` | bool | `true` | Paints the tab strip as a distinct bar: a raised sidebar fill, or `element` when the sidebar follows the canvas. Off, the strip matches the body. |
| `tab_style` | string | `"padded"` | `"padded"`, `"round"`, or `"arrow"`. Round and arrow need `nerd_icons`. |

A configured tab that `panels` leaves out is added to the first panel, except `worktrees`, which
joins the panel holding `git` or `files`. A `panels` entry naming an extension tab that is not
currently loaded is kept without a warning.

### Tab tables

An entry in `tabs` can be a table instead of an ID. A table can configure the built-in `files` or
`git` tree tab, or define a custom tab: a launcher tab with fixed rows, or a command tab that lists
a command's output.

| Tab key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `name` | string | required | Unique tab ID. `activity`, `panes`, and `sessions` are reserved. |
| `label` | string | required for custom tabs | Built-in tree tab labels are fixed. |
| `entries` | array of tables | none | Launcher rows. A custom tab needs exactly one of `entries` or `command`. |
| `command` | string | none | Command whose output lines become the tab's rows. |
| `interval` | integer seconds | `30` | Minimum `5`. Command tabs only. |
| `on_click` | action table | none; tree tabs type `{path}` | Action for a command or tree row. |
| `group_prefix` | string | none | Command tabs only. Output lines starting with it become section headers. |
| `root` | string | `"cwd"` for files, `"repo"` for git | `"cwd"` or `"repo"`. Tree tabs only. |
| `show_hidden` | bool | `true` | Tree tabs only. |
| `icons` | bool | `false` | Tree tabs only. Also needs `nerd_icons`. |
| `explorer` | bool | `false` | Tree tabs only. |
| `diff_stats` | bool | `false` for files, `true` for git | Tree tabs only. |
| `max_entries` | integer | `2000` | Clamped to `1..=10000`. Tree tabs only. |

A launcher entry takes a `label`, exactly one of `run`, `send`, or `popup`, an optional `keep_open`
(default `true`), and an optional `group`. An `on_click` action takes an optional `label`, exactly
one of `run`, `send`, `popup`, or `exec`, and an optional `keep_open`. In `on_click`, `label` only
changes how the command is presented.

An extension can add the same kinds of tab from its manifest; see
[Extensions](extensions.md#sidebar-tabs).

### Grouping rows into sections

A launcher entry's `group` puts it under a section header. Sections appear in the order their group
first appears, and entries keep their order within a section. Entries without a `group` come first,
with no header.

```toml
[sidebar]
tabs = [
  { name = "agents", label = "Agents", entries = [
    { label = "rozi", group = "claude", run = "cd ~/Projects/rozi && claude" },
    { label = "docs", group = "claude", run = "cd ~/Projects/docs && claude" },
    { label = "rozi", group = "codex", run = "cd ~/Projects/rozi && codex" },
  ] },
]
```

A command tab groups its own output instead. With `group_prefix` set, an output line starting with
the prefix becomes a section header showing the rest of the line. A line holding only the prefix is
dropped, like a blank line. Headers cannot be selected, so `on_click` applies to every other row.

A command tab runs in the focused pane's working directory and lists again when that directory
changes. Its `on_click` `send` action may use `{line}` for the row's text. `run`, `popup`, and
`exec` receive the row in `ROZI_ROW` instead of having it inserted into the command.

### Opening a diff viewer or editor from a row

A tree tab's `send` action may use `{path}`, because the result is typed into the pane as literal
input. Do not end it with a newline unless you mean to run the text.

Tree `run`, `popup`, and `exec` actions receive the selected path in `ROZI_FILE`. `run` and `popup`
reject `{path}`, and `exec` does not expand it. Quote the variable for your command shell:

```toml
[sidebar]
tabs = [
  "activity",
  { name = "files", label = "", on_click = { run = '''"${EDITOR:-vi}" "$ROZI_FILE"''' } },
  { name = "git", label = "", on_click = { popup = '''git diff -- "$ROZI_FILE"''', keep_open = false } },
]
```

rozi never inserts a selected path into a command string. See [Sidebar files](sidebar.md#files).

## `[workbar]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `left` | segment array | `["title", "workspaces"]` | Segments on the left, in order. |
| `right` | segment array | `["location", "session"]` | Segments on the right, in order. |
| `clock_format` | string | `"%H:%M"` | A valid strftime format. Invalid formats are ignored. |

A segment is a name string or a table `{ segment = "…", color = "…" }`. Colors are `accent`,
`info`, `success`, `warning`, `error`, `neutral`, or `panel`.

| Segment | Shows |
| --- | --- |
| `title`, `workspaces`, `location`, `session`, `clock`, `layout`, `activity` | Built-in segments. |
| `text:<literal>` | Fixed text. It may use `{host}`, `{workspace}`, `{layout}`, and `{session}`. |
| `command:<shell command>` | The command's output, refreshed every 60 seconds. |
| `command:<interval seconds>:<shell command>` | The command's output, refreshed at the given interval (minimum 1 second). |

A command segment times out after 5 seconds and keeps at most 64 KiB from each output stream.

### `[workbar.alert]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `bell` | bool | `true` | Marks workspaces with a bell. |
| `blocked` | bool | `true` | Marks blocked workspaces. |
| `finished` | bool | `true` | Marks finished workspaces you have not looked at. |
| `working` | bool | `false` | Marks working workspaces. |
| `idle` | bool | `false` | Marks idle workspaces. |
| `mode` | string | `"pulse"` | `"off"`, `"static"`, or `"pulse"`. |
| `paint` | string | `"background"` | `"background"` or `"text"`. |

## `[logging]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `dir` | path string | `logs` in the state directory | `~` expands. |
| `max_bytes` | integer | `67108864` (64 MiB) | Limit per log file. `0` means no limit. |

See [Pane logging](terminal.md#pane-logging).

## `[capture]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `dir` | path string | `captures` in the state directory | Where **Screenshot pane** and **Screenshot UI** write PNGs. `~` expands. Created mode `0700` if missing. |
| `scale` | integer | `1` | `1` to `3`. Other values warn and use `1`. |

See [Take a screenshot](terminal.md#take-a-screenshot).

## `[recording]`

Defaults for [pane recordings](recording.md). The session server reads these from the configuration
on its own host each time a recording starts, so for a remote session they come from that host.

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `dir` | path string | `recordings` in the state directory | Where a recording started without `--output` is written, including from **Start pane recording**. `~` expands. Created mode `0700` if missing. |
| `max_fps` | integer | `30` | `1` to `120`. |
| `duration` | duration string | `"24h"` | Such as `"90s"`, `"8h"`, or `"1h30m"`; at most `"7d"`. |
| `max_bytes` | size string | `"1GiB"` | Such as `"512MiB"`; at least `"64KiB"`. |

A value out of range, or one that does not parse, warns and keeps the default. The `--max-fps`,
`--duration`, and `--max-bytes` options of `rozi record start` override these for one recording.

## `[[rules]]`

Rules set how new panes open. They apply to ordinary panes started with an explicit command, in the
order you declare them, and the first match wins.

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `match` | string | none | Case-sensitive substring of the command. Set exactly one matcher. |
| `match_regex` | string | none | `regex-lite` pattern. Set exactly one matcher. |
| `float` | bool | `false` | Opens a floating pane. |
| `width` | float | `0.6` when floating | Clamped to `0.1..=1.0`. |
| `height` | float | `0.6` when floating | Clamped to `0.1..=1.0`. |
| `position` | string | `"center"` | `center`, `cursor`, `top-left`, `top`, `top-right`, `left`, `right`, `bottom-left`, `bottom`, or `bottom-right`. Ignored unless floating. |
| `workspace` | integer | Current workspace | `1..=9`. Invalid values are ignored. |
| `focus` | bool | `true` | Focuses the pane and its workspace. |
| `fullscreen` | bool | `false` | Starts fullscreen. |

The control command `split --workspace` and `--focus` override `workspace` and `focus`. See
[Layouts and panes](layouts-and-panes.md).

## `[[agents]]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `id` | string | required | Lowercase letters, digits, `_`, and `-`. An entry with a built-in ID replaces that built-in. |
| `label` | string | The ID | Label shown in Activity. |
| `base` | bool | `true` | Enables the shared state patterns. |
| `match.names` | array of strings | `[]` | Executable names. |
| `match.paths` | array of strings | `[]` | Lowercase substrings of the path or arguments. |
| `states` | array of state tables | `[]` | State rules. |

A new definition needs at least one match name or path. A definition that replaces a built-in may
omit `match` and keep the built-in's process match.

| State key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `state` | string | required | `"unknown"`, `"blocked"`, `"working"`, or `"idle"`. States are checked in that order, not in declaration order. |
| `scope` | string | `"all"` | `"all"` or `"footer"`. Footer reads the last eight nonempty screen lines. |
| `screen` | pattern table | none | Set exactly one of `screen` or `title`. |
| `title` | pattern table | none | Set exactly one of `screen` or `title`. `scope` does not apply. |

Pattern tables accept `all_of`, `any_of`, and `none_of` string arrays, plus `regex`, a bool that
defaults to `false`. At least one of `all_of` or `any_of` is required. Matching reads lowercased
text. An invalid rule is skipped; an invalid definition is dropped.

See [Agent definitions](agents.md).

## `[[hints]]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `pattern` | string | required | Nonempty `regex-lite` pattern. Invalid patterns are skipped. |
| `open` | bool | `false` | Lets the uppercase hint label open the match. |

The built-in URL, path, and Git SHA hints run first and win where matches overlap. See
[Copy, search, and hints](terminal.md#copy-search-and-hints).

## `[[hooks]]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `event` | string | required | Public event ID. Unknown IDs are skipped. |
| `run` | string | required | Nonempty command string run through `command_shell`. |

Several hooks may use the same event. At most 32 hook and detached `exec` jobs run at once; further
launches are skipped. See [Hooks](hooks.md).

## `[[commands]]`

Named commands have stable IDs, so you can bind them under `[keys]` and run them with
`rozi run-action`.

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `id` | string | required | Lowercase letters, digits, `_`, and `-`. Dots, built-in IDs, and reserved prefixes are rejected. |
| `label` | string | Generated | Label in the command palette and help. |
| `run` | string | none | Opens a pane running the command through `command_shell`. |
| `send` | string | none | Sends literal text to the focused pane. |
| `popup` | string | none | Opens a centered popup running the command through `command_shell`. |
| `exec` | string | none | Runs the command in the background through `command_shell` and discards its output. Shares the 32-job limit with hooks. |
| `keep_open` | bool | `true` | For `run` and `popup`: keeps the pane or popup open after the command exits. |

Set exactly one of `run`, `send`, `popup`, or `exec`.

```toml
[[commands]]
id = "branches"
label = "Switch branch"
exec = "~/.config/rozi/branch-pick.sh"

[keys]
branches = "i"
```

## `[extensions]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `disabled` | array of strings | `[]` | Manifest IDs of extensions to disable. Directory names are not extension IDs. The Extensions overlay updates this key when you enable or disable an extension. |

An `[extensions.<id>]` table configures one installed extension, using the settings that extension
declares. rozi reports and ignores an undeclared key, a value of the wrong type, and a table for an
extension that is not installed.

```toml
[extensions]
disabled = ["docker"]

[extensions.tasks]
runner = "just"
rows = 20
```

Run `rozi extensions check <path>` to see the settings an extension declares and their defaults.

Extension commands may suggest a default key after the prefix and `x` — `Ctrl+A`, then `x`, then a
key, with the default prefix. A `[keys]` entry for the command overrides the suggestion.

See [Extensions](extensions.md).

## `[[services]]`

Services are long-running background commands that rozi starts and restarts for you.

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `name` | string | required | Nonempty and unique. |
| `run` | string | required | Nonempty command string run through `command_shell`. |
| `cwd` | path string | Launch directory | `~` expands when the service starts. |
| `restart` | string | `"on-failure"` | `"on-failure"`, `"always"`, or `"never"`. |
| `env` | string table | `{}` | Environment variables to set for the service. |

Services receive `ROZI=1`, `ROZI_SERVICE`, `ROZI_BIN`, and, when control is available,
`ROZI_SOCKET`.

A restarting service waits 1, 2, 4, 8, 16, then 30 seconds between attempts. After five consecutive
failures within 60 seconds, the service stays stopped until you change its definition. rozi stops
each service's process group when the client exits.

For packaged automation, use extension services instead. See [Extensions](extensions.md).

## `[keys]`

Each key in `[keys]` is a built-in action ID, a named command ID, an extension command ID, or a
trigger for an inline command.

### Action and named-command bindings

| Value form | Behavior |
| --- | --- |
| `"b"` or `["b", "super-enter"]` | Replaces the defaults. A bare key expands through the prefix and modifier scheme; a full chord is used as written. |
| `"scheme:ctrl-t"` | Expands one modified key through the prefix and modifier scheme. |
| `"prefix:w"` | Binds only the prefix form of one key step. Follows `[input] prefix`. |
| `"mod:v"` | Binds only the held-modifier form of one key step. Follows `[input] modifier`; inactive while `modifier_shortcuts` is `false`. |
| `{ add = "super-enter" }` | Adds a binding and keeps the defaults. `add` also accepts an array. |
| `""` or `[]` | Removes every binding for that action. |

A string may list alternatives separated by commas. If every nonempty replacement is invalid, rozi
keeps the action's defaults.

See [Keybindings](keybindings.md) for action IDs and key syntax.

### User-defined command keybindings

An inline command is a table bound directly to a trigger key:

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `label` | string | Generated | Label in the command palette and help. |
| `run` | string | none | Opens a pane. |
| `send` | string | none | Sends literal text to the focused pane. |
| `popup` | string | none | Opens a popup. |
| `exec` | string | none | Runs in the background and discards output. |
| `keep_open` | bool | `true` | Applies to `run` and `popup`. |

Set exactly one of `run`, `send`, `popup`, or `exec`. The trigger accepts the same forms as a
binding: `g`, `"scheme:ctrl-g"`, `"prefix:g"`, `"mod:g"`, or a full chord. Inline commands have no
stable ID, so `rozi run-action` cannot call them; use [`[[commands]]`](#commands) for that.

```toml
[keys]
g = { run = "lazygit", label = "Git UI", keep_open = false }
"ctrl-a e" = { send = "ls -la\n" }
u = { exec = "rozi run-action toggle-float", label = "Float pane" }
```
