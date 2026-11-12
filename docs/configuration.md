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
and take effect at once. If `config.toml` is a symlink, rozi writes the file it points to and keeps
the link.

**Settings…** in the command palette browses options by category and previews static appearance
choices. Behavior and animation choices apply when confirmed with `Enter`. It has no
default key; bind the `settings` action under [`[keys]`](#keys) to add one. See
[Use the Settings picker](customize.md#use-the-settings-picker).

### Open the file in an editor

The `open-config` action opens the config file in `EDITOR`, then `VISUAL`, then `vi`:

```bash
rozi run-action open-config
```

The editor runs directly, not through a shell. Quotes around a path that contains spaces
(`"/opt/my editor/bin/edit" --wait`) are honored, and shell syntax in `EDITOR` is not interpreted.

## Cursor blinking

Choose **Blink** in Settings under General › Cursor, or set `[cursor]` in `config.toml`.
Changes apply immediately to pickers, prompts, text editors, and terminal panes.

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `cursor.blink` | string | `"inherit"` | `inherit`, `static`, or `blinking`. |

`inherit` uses the host terminal's blink preference, and panes follow it until a program requests
its own cursor style. `static` and `blinking` override blink requests throughout rozi. Programs
still control cursor shape and visibility.

## Mouse pointer

Choose **Mouse pointer** in Settings under General. Changes apply immediately and leave mouse
clicks, selection, and dragging enabled. The host terminal must support mouse pointer shapes.

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `cursor.pointer` | string | `"all"` | `off`, `panes`, or `all`. |

| Settings choice | Value | Behavior |
| --- | --- | --- |
| Off | `off` | Keep the normal arrow throughout rozi. |
| Pane apps only | `panes` | Follow mouse pointer requests from the pane application under the mouse. |
| Rozi and pane apps | `all` | Also show rozi's own pointers over editable fields, controls, and pane and divider gestures. |

Terminal support varies.

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
| `scrollback` | New terminal screens. Restart an existing session server for its new panes to use it. |
| `window_title` | On save. Changing it to `""` keeps the last title until rozi exits. |
| `frame_rate` | Next client launch or reattach. |
| `updates.interval_hours` | The next re-check. |
| `sidebar.startup` | Next client launch. Reload never opens or closes the sidebar. |
| `session.startup` | Next bare launch. |
| `session.on_attach` | Next writable attach by this client after reload. |
| `session.resurrect`, `session.resurrect_foreground`, `session.resurrect_agents`, `session.allow_takeover` | Session servers started after the change. |
| `logging.*` | Session servers started after the change. |
| `remote.reconnect_on_launch` | Next client launch. |
| Other `remote.*` settings | New SSH connections. |
| `rules` | New pane spawns that carry a command. |
| `services` | On reload. Changed services restart, removed services stop, and unchanged services keep running. |
| `agents` | On reload, in the controlled session server and the local scratch session. |
| `extensions.disabled` | When the config file changes. |
| Extension manifests | Automatically on install, update, or removal; after `reload-extensions` for edited checkouts. |

## Invalid configuration

rozi handles problems at two levels:

- **The whole file is rejected** when it is unreadable or is not valid TOML. At startup rozi uses
  the defaults; on a live reload it keeps the last good configuration. Either way it shows an
  error.
- **One setting is skipped** when a key is unknown, a value has the wrong type, or a value is not
  one of the allowed choices. rozi uses that setting's default, applies the rest of the file, and
  warns with the setting's full key, such as `animations.cursor_trail`. In a list of tables, such
  as `[[rules]]` or `[[hints]]`, a wrong value skips its whole entry, named by its position from
  zero: `rules[1]` is the second `[[rules]]` entry. The other entries still apply.

Several warnings share one toast, and each is also printed to stderr. Some settings are clamped
into range rather than skipped; their rows below say so.

## In-app toasts

rozi shows a toast for failures, rejected actions, destructive confirmations, and results that
have no other visible feedback. Changes that already show up in the workbar, pane layout, a picker,
or the sidebar do not add a toast. A repeated message renews the existing toast instead of stacking copies.
[`animations.toast`](#animations) sets how toasts arrive and leave.

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
started there is visible to your desktop clients.

## Top-level keys

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `shell` | string or string array | Platform shell | A string is one program with no arguments; an array keeps each argument. Unix uses `SHELL`, then `/bin/sh`. Windows tries `pwsh.exe`, `powershell.exe`, `COMSPEC`, then `cmd.exe`. |
| `command_shell` | string or string array | `["/bin/sh", "-c"]` on Unix, `[COMSPEC, "/D", "/S", "/C"]` on Windows | Runs command strings for panes, popups, hooks, services, workbar and sidebar commands, and config commands. |
| `cwd` | path string | Launch directory | Starting directory for new panes. `~` expands. |
| `scrollback` | integer | `5000` | Minimum `1`. |
| `frame_rate` | integer | `120` | Clamped to `15..=480` with a warning. |
| `nerd_icons` | bool | `true` | Uses [Nerd Font](https://www.nerdfonts.com/) glyphs in rozi's chrome. |
| `window_title` | string | `"rozi · {session} · {pane}"` | Terminal window title. Placeholders: `{session}` (empty for a temporary session), `{pane}` (the focused pane's title), `{cwd}`, `{host}`. An empty placeholder drops the text after it. `""` leaves the title alone. |

With `nerd_icons` on, rozi draws private-use glyphs for pane title icons, workbar location and
named-session badges, the Sessions sidebar client-count badge, directory chevrons, the Files
explorer search prefix, and `round` and `arrow` caps, and Files and Git use colored file icons and
Nerd Font Git marks. With it off, those badges use `⌁`, `∞`, and `⋈`, directory chevrons use `▶` and
`▼`, the explorer prefix uses `⌕`, pane titles drop the icon, and Files and Git use Unicode
expansion arrows and text Git marks.

New local panes receive `ROZI=1`, `ROZI_PANE`, `ROZI_SESSION_INSTANCE`, `TERM_PROGRAM=rozi`,
`TERM_PROGRAM_VERSION`, and, when available, `ROZI_SOCKET` and `ROZI_BIN`.
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
| `forward` | array of strings | `[]` | Extra client environment variables to copy into new local panes. Values are not forwarded through remote attachments. |

rozi already forwards the desktop session variables that Wayland, X11, D-Bus, and Hyprland need. A
client with no display of its own, such as one attached over SSH, gets the host's current desktop
instead on Linux with a systemd user session.

## `[input]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `modifier` | string | `"alt"` | `"alt"` or `"super"`. `mod` is an alias for Alt; `meta`, `logo`, `win`, and `windows` are aliases for Super. |
| `prefix` | string | `"ctrl-a"` | One key step in [config key syntax](keybindings.md#key-notation), such as `"ctrl-b"`. |
| `modifier_shortcuts` | bool | `true` | Mirrors the generated prefix bindings onto held-modifier chords. |
| `which_key` | string | `"short"` | Delay before the which-key strip appears: `"off"`, `"instant"`, `"short"` (500 ms), or `"long"` (1000 ms). |
| `scroll_multiplier` | integer | `3` | Multiplies the wheel scrolling your terminal sends across panes, sidebar tabs, and pickers, `1` to `50`. It stacks with the terminal's own multiplier. |

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
| `focus_on_hover` | bool | `true` | Focuses a pane when the pointer enters it. In a Scrollable layout, a clipped column scrolls into view on the next key or click. |
| `focus_on_hover_pause_modifier` | string | `"shift"` | Holding this modifier pauses hover focus; click focus still works. `"shift"`, `"ctrl"`, `"alt"`, or `"none"` for no pause. Other modifiers may be held too. |
| `hold_on_exit` | bool | `false` | Keeps shell panes open after they exit on their own. Command panes use their `keep_open` value. |
| `highlight_focused_background` | bool | `false` | Uses the panel background for the focused pane. |
| `highlight_focused_border` | bool | `true` | Uses the active border color for the focused pane. |
| `highlight_focused_titlebar` | bool | `true` | Uses focused titlebar styling. |
| `show_workbar` | bool | `true` | Shows the workbar. |
| `workbar_gap` | bool | `true` | Keeps one row between the workbar and panes. |
| `workbar_background` | bool | `true` | Paints the workbar as a distinct strip. Off, the bar matches the canvas. |
| `workbar_at_bottom` | bool | `false` | Places the workbar below panes. |
| `show_titles` | bool | `true` | Shows pane titles without changing `titlebar`. |
| `titlebar` | string | `"bar"` | `"bar"`, `"border"`, `"integrated"`, or `"inset"`. |
| `border_mode` | string | `"separate"` | `"separate"`, `"merged"`, `"none"`, or `"dividers"`. |
| `alert_border` | string | `"pulse"` | `"off"`, `"static"`, or `"pulse"`. Whether pane alerts show and whether they breathe. |
| `alert_paint` | string | `"both"` | `"border"`, `"content"`, or `"both"`. `content` lays a faint wash of the alert color over the pane; it works in every `border_mode`. |
| `border_style` | string | `"rounded"` | Frame glyphs for tiled panes in framed modes. See the token list below. |
| `float_border_style` | string | `"double"` | Same tokens as `border_style`. Floating panes and popups. |
| `scratch_border_style` | string | `float_border_style` | Same tokens as `border_style`. Scratchpad panes. |
| `fullscreen_border_style` | string | `border_style` | Same tokens as `border_style`. Fullscreen panes. |
| `picker_border_style` | string | `"rounded"` | Same tokens as `border_style`. The command palette, Settings, Help, Search, and other pickers. |
| `picker_tab_background` | bool | `true` | Paints picker category tabs as a distinct strip. Off, the tabs share the picker body. |
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
| `unfocused_dim` | float | `0.0` | How far an unfocused pane's content fades toward the canvas, `0.0..=1.0`; `0.0` is off. [Described in Dim unfocused panes.](layouts-and-panes.md#dim-unfocused-panes) |
| `background_follows_terminal` | bool | `false` | Uses the host terminal's background for the canvas. |

The border style tokens are `"rounded"`, `"plain"`, `"double"`, `"thick"`, `"light-double-dashed"`,
`"heavy-double-dashed"`, `"light-triple-dashed"`, `"heavy-triple-dashed"`,
`"light-quadruple-dashed"`, and `"heavy-quadruple-dashed"`. A default written as another key's name
means the setting follows that key when omitted.

See [Layouts and panes](layouts-and-panes.md), [Sidebar](sidebar.md), and [Themes](themes.md).

### `[pane.alert]`

Colors for pane alert borders and content tints. Each value is a theme role or `"off"`. Theme roles are
`accent`, `info`, `success`, `warning`, `error`, `neutral`, and `panel`.

| Key | Type | Default |
| --- | --- | --- |
| `blocked` | string | `"warning"` |
| `error` | string | `"error"` |
| `finished` | string | `"success"` |
| `working` | string | `"off"` |
| `idle` | string | `"off"` |

The focused pane still shows a finished run, and an agent waiting on you in any of its
conversations. That blocked alert holds still instead of breathing, and clears once you answer the
agent. A plain `rozi status blocked` marks a pane only while it is not focused.

An error alert clears once you use the pane, even while the failure lasts; the sidebar keeps showing
the failed row.

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
| `session` | string or bool | `"portal"` | `"portal"`, `"wave"`, `"fade"`, or `"off"`; `true` means `"portal"` and `false` means `"off"`. Described below. In Settings: General › Animations › Session switching. |
| `focus_chrome` | bool | `true` | Animates focus color changes and enables alert pulses. |
| `cursor_trail` | table | | A smear that follows the cursor when it jumps. See [Cursor trail](#cursor-trail). |
| `picker` | string | `"fade"` | `"off"`, `"fade"`, `"portal"`, or `"scan"`, case-insensitive. Applies to searchable pickers, Settings, and their nested cards. In Settings: General › Animations › Pickers. |
| `toast` | string | `"slide"` | `"off"`, `"fade"`, or `"slide"`, case-insensitive. Slide brings a toast in from the right edge; Fade fades it in place. In Settings: General › Animations › Toasts. |
| `pane_open_style` | string | `"scale"` | `"off"`, `"scale"`, `"slide"`, `"portal"`, `"scan"`, or `"particles"`, case-insensitive. |
| `pane_close_style` | string | `"scale"` | `"off"`, `"scale"`, `"slide"`, `"portal"`, `"scan"`, or `"particles"`, case-insensitive. |
| `geometry_ms` | integer | `220` | Base duration of pane geometry animations, in milliseconds. |
| `close_ms` | integer | `120` | Scale close duration in milliseconds. |
| `focus_chrome_ms` | integer | `160` | Focus color duration in milliseconds. |
| `alert_pulse_ms` | integer | `1600` | Alert pulse period. The half-period is at least 400 ms. |
| `open_delay_ms` | integer | `36` | Spawn animation delay in milliseconds. |

`session` controls how the workbar and panes arrive when the foreground session changes: when you
switch sessions, when a session finishes connecting, or when you drop to the launcher.

- `"portal"` opens a portal from the center onto the new session while the previous one recedes
  behind it, over `geometry_ms`.
- `"wave"` sweeps a glowing wave from left to right that wipes the previous session away, over four
  times `geometry_ms` (880 ms by default).
- `"fade"` crossfades from the previous session to the new one in place, over one and a half
  times `geometry_ms` (330 ms by default).
- `"off"` switches at once.

Pane geometry always snaps and the sidebar stays still. The value is case-insensitive.

Choose opening and closing independently with `pane_open_style` and `pane_close_style`, or use
**General › Animations › Pane open** and **Pane close** in Settings. For example, use `slide` to
open and `particles` to close. Both default to `scale`.

An `off` style shows or hides a pane at once, with no fade. Opening with `off` also skips the spawn
delay.

`particles` assembles panes from nearby motes that curve into place; closing bursts fragments
outward before gravity pulls them down. Opening takes `geometry_ms` (220 ms by default) and closing
four times that (880 ms). Fragments can travel over neighboring panes. This style also applies to
floating panes, popups, and scratch panes. Particles run at up to 30 FPS; a lower `frame_rate` also
lowers that.

### Pane animation curves and effect settings

The five pane effects — Scale, Slide, Portal, Scan, and Particles — accept timing and motion
curves. Scale, Portal, and Scan also have geometry settings. These keys also live directly under
`[animations]`:

```toml
[animations]
pane_open_style = "slide"
pane_close_style = "particles"
geometry_ms = 130
close_ms = 90
curve = [0.16, 1.0, 0.3, 1.0]
```

| Key | Type | Default | Applies to |
| --- | --- | --- | --- |
| `curve` | curve | the style's own | all |
| `close_curve` | curve | the reverse of `curve` | all |
| `fade` | bool | `true` for Scale, `false` for Portal, Scan, and Particles | Scale, Portal, Scan, Particles |
| `scale_from` | float in `[0.1, 1]` | `0.9` | Scale |
| `portal_origin` | float pair in `[0, 1]` | `[0.5, 0.5]` | Portal |
| `scan_direction` | `top-left`, `top-right`, `bottom-left`, `bottom-right` | `top-left` | Scan |

A curve is either CSS cubic-Bézier control points, `[x1, y1, x2, y2]`, or the name of a built-in
easing: `linear`, `ease_in_quad`, `ease_out_quad`, `ease_in_out_cubic`, or `ease_in_out_sine`. The
x coordinates must be in `[0, 1]`. The y coordinates must be finite and in `[-4, 4]`; a value above
`1` overshoots and settles back. Without `close_curve`, closing runs `curve` backwards.

Only the geometry key for the selected open or close style is used. Slide has no geometry key, and
`fade` has no effect on it.

A value outside its range is dropped with a warning, not clamped.

These keys cover only a pane arriving and leaving. Tiles rearranging around that pane, and
fullscreen, tile/float, and axis-change transitions, use `geometry_ms`.

See [Pane open and close animation styles](layouts-and-panes.md#pane-open-and-close-animation-styles).

### Cursor trail

`[animations.cursor_trail]` draws a smear that follows the cursor when it jumps. It is off until you
set `mode`. In Settings, **General › Cursor › Trail** chooses when it shows and **Trail style** how
it looks.

```toml
[animations.cursor_trail]
mode = "jump"
style = "sparks"
```

With `"jump"`, every jump starts its smear at once. A positive `settle_ms` makes jumps to another
line or to the start of the line wait for the cursor to hold still. A move to another pane always
draws at once.

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `mode` | string | `"off"` | `"off"`, `"focus"`, or `"jump"`, case-insensitive. `"focus"` draws a smear when focus moves to another pane or prompt; `"jump"` also draws one when the cursor moves at least 8 columns or 2 rows inside a pane. In Settings: General › Cursor › Trail. |
| `style` | string | `"smear"` | `"smear"` is a solid smear in the trail color; `"sparks"` is the smear throwing sparks; `"tint"` is a fainter smear that never covers text or the cursor. In Settings: General › Cursor › Trail style. |
| `motion` | string | `"spring"` | `"spring"` stretches the smear out behind the cursor and snaps it in; `"glide"` keeps the front on the cursor and eases the tail in over `glide_ms`. |
| `color` | string | `"cursor"` | `"cursor"` uses the cursor's color (the theme's [`caret`](themes.md#top-level-fields), else the terminal's cursor color, else its text color); `"accent"` uses the theme's focused-border color; or give a `"#rrggbb"` value. |
| `stiffness` | float | `0.5` | Spring pull on the front of the smear, from 0 to 1. |
| `trailing_stiffness` | float | `0.28` | Spring pull on its tail, from 0 to 1. Lower than `stiffness` stretches the smear. |
| `trailing_exponent` | float | `0.3` | How full the smear stays toward its tail, from 0.1 to 10; higher keeps it fuller. |
| `damping` | float | `0.82` | How much speed the smear loses each step, from 0 (bouncy) to 1 (no momentum). |
| `glide_ms` | integer | `160` | How long the tail of a `"glide"` takes to reach the cursor, in milliseconds. `0` turns the glide off. |
| `settle_ms` | integer | `0` | How long, in milliseconds, the cursor must hold still after a jump to another line or to the start of the line before its smear starts, from 0 to 1000. `0` starts every jump at once. |
| `settle_max_ms` | integer | `200` | The longest, in milliseconds, such a jump waits for a cursor that keeps moving, from 0 to 1000. |
| `max_length` | float | `0.0` | Caps how far the smear's tail trails the cursor, up to 500, in cell widths as they look on screen. `0` leaves it unbounded. |
| `opacity` | float | `1.0` | The smear's strength at the cursor, from 0 to 1. |
| `taper` | float | `0.0` | How much the smear fades toward its tail, from 0 (evenly) to 1 (to nothing). |
| `glyphs` | string | `"auto"` | The characters the smear's edges use: `"blocks"`, `"sextants"`, `"octants"` (the last two need a font with Symbols for Legacy Computing), or `"auto"`, which picks the richest set your terminal draws itself. |

`stiffness`, `trailing_stiffness`, and `damping` work as the settings of the same name in
smear-cursor.nvim. A value out of range is ignored with a warning.

`[animations.cursor_trail.particles]` tunes the sparks `style = "sparks"` throws off the moving
smear.

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `per_cell` | float | `1.5` | Sparks per cell the smear travels, up to 20. |
| `max` | integer | `100` | The most sparks alive at once, at least 1. |
| `lifetime_ms` | integer | `300` | The longest a spark lives, in milliseconds. |
| `spread` | float | `6.0` | The top speed of a spark flying off in a random direction, in cells per second, up to 200. |
| `inherit` | float | `0.25` | How much of the smear's own speed a spark keeps, from 0 to 1. |
| `gravity` | float | `20.0` | Downward pull in cells per second squared, from -500 to 500. Negative values make sparks rise. |
| `drag` | float | `3.0` | How quickly a spark slows, per second, up to 100. |

If your editor or terminal draws its own cursor trail, such as smear-cursor.nvim or Kitty's
`cursor_trail`, leave this off or use `"focus"`, so you do not see two trails at once.

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
| `delete_branch` | array | `[]` | Branch copies to delete when you remove a checkout: `"local"`, `"remote"`, or both. Only merged branches are deleted. |
| `sidebar_open` | string | `"session"` | What a click or `Enter` on a checkout in the Worktrees sidebar tab opens: `"session"` or `"pane"`. Ctrl+click and `Ctrl+Enter` open the other. |

Without `directory`, checkouts go beside the repository in `<repo>-worktrees/<branch>`. Otherwise:

- An absolute path (after `~` expansion) holds every repository's checkouts as
  `<directory>/<repo>/<branch>`.
- A single folder name stays inside the repository: `".worktrees"` gives
  `<repo>/.worktrees/<branch>`.
- A nested or escaping value such as `"tools/.worktrees"` or `"../worktrees"` is ignored with a
  warning. Use an absolute path for a location outside the repository.

While Git does not ignore that directory, rozi warns and offers to add it to `.git/info/exclude`.
`directory` is read on the session host.

With `profile` set, pane directories inside any checkout of the repository move to the new
checkout, directories outside it are kept, and panes without a directory start in the checkout. For
a worktree on a remote host, the profile applies only when every pane directory it names is inside
the repository; otherwise the session starts as one shell in its checkout. rozi reports why a
profile failed to load or was skipped.

With `delete_branch`, `"local"` deletes the branch with `git branch -d` and `"remote"` deletes the
branch on the remote it tracks. An unmerged branch, a branch another checkout still has checked out,
and a detached checkout are left alone, and rozi says when a branch was kept. An unknown entry
ignores the whole setting with a warning. `rozi worktrees remove` reads the value on the host that
owns the repository, and `--delete-branch` or `--keep-branch` overrides it. See
[Delete the branch too](worktrees.md#delete-the-branch-too).

See [Worktrees](worktrees.md#configuration).

## `[clipboard]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `copy_on_select` | string | `"both"` on Linux, `"clipboard"` elsewhere | Where a finished mouse selection is copied: `"off"`, `"primary"`, `"clipboard"`, or `"both"`. |
| `middle_click_paste` | string | `"primary"` on Linux, `"off"` elsewhere | What a middle click pastes: `"off"`, `"primary"`, or `"clipboard"`. |
| `right_click` | string | `"off"` | What an otherwise unhandled right click does: `"off"`, `"paste"` from the clipboard, or `"copy-or-paste"`. |
| `enable_osc52` | bool | `true` | Lets programs in panes set the system clipboard with OSC 52. |
| `copy_feedback` | string | `"flash"` | How copied text lights up: `"off"`, `"flash"`, `"fade"`, `"scan"`, or `"pulse"`. With the animation master switch off, all but `"off"` flash. |
| `copy_feedback_on_select` | bool | `false` | Also lights up text that `copy_on_select` copies when a mouse selection ends. |
| `clear_selection_after_copy` | bool | `false` | Clears a copied selection once the highlight ends, or at once when `copy_feedback` is `"off"`. |

PRIMARY is the Linux selection clipboard. Where it is unsupported, `copy_on_select = "both"` copies
only to the regular clipboard, `"primary"` copies nowhere, and a middle click that pastes PRIMARY
does nothing.

Middle-click and right-click paste go to the pane or text field under the pointer, which takes
focus first. `"copy-or-paste"` copies the selection in the clicked pane, and pastes when there is
none. Handlers in rozi's widgets and in the program running in the pane take priority over
`right_click`.

These settings apply on reload and are also in the General category of Settings. See
[Select, copy, and paste](terminal.md#select-copy-and-paste).

## `[updates]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `check` | bool | `true` | Looks for a newer release shortly after a client starts, then every `interval_hours`. Off, rozi never contacts the release host or the usage counter on its own. |
| `interval_hours` | integer | `6` | Hours between re-checks in a client that stays open. Minimum `1`; lower values are clamped with a warning. |
| `count_usage` | bool | `true` | Adds this install to the anonymous weekly [usage count](usage-count.md) during these checks. Has no effect while `check` is off. |
| `release_notes` | bool | `true` | Shows the release notes once, the first time rozi starts after an update. Works offline and while `check` is off. See [Release notes](#release-notes). |

`rozi update --check` checks on demand, whether or not `check` is on.

### Update notices

When a newer release exists, rozi shows a toast titled `rozi vX.Y.Z available` with the version
change and how to update; click it to dismiss it. The toast uses warning colors when the release
raises the extension API, or when running sessions must be restarted after the update, and says
so. Each release is announced only once across clients.

Every client that finds the release also lists **Update rozi to vX.Y.Z** (`update-rozi`) in the
command palette for as long as it runs:

1. Choosing it opens the `Updating rozi` dialog. For a rozi you installed with its installer, it
   shows the download and then the install. For one a package manager installed, it runs that
   manager's upgrade command and shows the last lines it prints. The command runs without a
   terminal, so one that stops to ask a question fails.
2. **Run in background** puts the dialog away while the update keeps going; choose **Update rozi**
   again to see it. **Restart rozi** waits until the update is done, and leaving rozi stops an
   update a package manager is still running.
3. On success, the dialog becomes the `rozi vX.Y.Z installed` dialog, which offers to
   [restart rozi on the new build](installation.md#restart-rozi-on-the-new-build), coming back to
   the same sessions. **Later** keeps the old build running until you choose **Restart rozi**. For
   an update in the background, a `rozi vX.Y.Z installed` toast says it is done; click it to
   restart now.
4. On failure, the dialog says what stopped the update, and **Try again** runs it again. For an
   update in the background, a toast says it failed and the command stays listed so you can retry.

The update always installs the rozi this window was started from, even while a remote session is in
front. The command is not listed when rozi was installed from a distribution package or in a way it
does not recognize.

### Release notes

The first time rozi starts after an update, a `What's new · rozi` dialog shows the release notes,
in one client only. A first install skips it, and so does going back to an older version.

The dialog has a tab for each of the latest 20 releases that have notes, newest first. The header
names the version you updated from, such as `updated from v0.0.37`, so every newer tab is a release
you skipped.

| Key | Action |
| --- | --- |
| `Tab`, `→` | Next, older release |
| `Shift+Tab`, `←` | Previous, newer release |
| `Ctrl+L` | Open the shown release's page on GitHub |
| `Enter` | Close the dialog |

**What's new in rozi vX.Y.Z** (`release-notes`) in the command palette shows the notes again at
any time.

The notes are part of the release archives published on GitHub. A rozi built from source, such as
with `cargo install rozi`, has none, so it shows neither the dialog nor the command.

## `[notifications]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `enabled` | bool | `false` | Master switch for desktop notifications. |
| `pane_exit` | bool | `false` | Notifies when a pane exits on its own with status zero. |
| `pane_exit_error` | bool | `true` | Notifies when a pane exits on its own with a nonzero status. |
| `pane_blocked` | bool | `true` | Notifies when an unattended pane becomes blocked. |
| `pane_done` | bool | `false` | Notifies when a pane you have not looked at goes from working to finished. |
| `pane_error` | bool | `true` | Notifies when an agent, one of its conversations, or a pane's own status fails in a pane you are not looking at. |
| `bell` | bool | `true` | Marks an unattended pane urgent when it rings the terminal bell (BEL), unless it is running an agent. Works even when `enabled` is off. |

Desktop notifications go through the platform's notification service and are best effort.

`pane_blocked`, `pane_done`, and `pane_error` also cover agents in other local sessions and in sessions on a
[connected remote host](remote.md#agents-on-a-machine-you-are-not-in) that this client is not
attached to.

`pane_done` and `pane_error` also cover [published Activity rows](control.md#published-activity) a
pane is not showing, such as a background conversation behind a Claude Code client. Looking at the
pane does not count as having seen such a row, and the notification names the row, for example
`fix login redirect in pane 23 is done`, rather than the pane.

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
enabled extensions. Names are matched against the program's executable name, ignoring case and a
Windows `.exe` suffix.

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
| `keep_awake_while_agents_work` | bool | `false` | Keeps the host awake while an agent is working; activities such as `rozi status` and generic published rows do not count. See [sleep policy](sessions.md#keep-the-system-awake). |
| `autosave` | bool | `false` | Saves and restores the local layout, not running programs. |
| `resurrect` | bool | `true` | Saves each named session's layout, commands, scrollback, and what to restart. |
| `resurrect_foreground` | string | `"hold"` | What a restore does with a command a pane was running or an agent conversation it reported: `"auto"` runs it, `"hold"` types it at the prompt, `"never"` restores only the shell. |
| `resurrect_agents` | bool | `true` | Saves agents' native session references in named-session snapshots and reopens those conversations on restore. |
| `picker_open_on` | string | `"current-host"` | Initial Sessions tab: `"current-host"` opens the current session’s host, the launcher's host, or Local; `"all"` opens All sessions. |
| `startup` | string | `"picker"` | What a bare `rozi` launch opens: `"picker"`, `"ephemeral"` (a temporary session), `"last"`, or `"profile"`. |
| `path` | path string | `session.toml` in the state directory | Autosave file. `~` expands. The default file is private (mode `0600`); a path you choose gets ordinary permissions. |
| `allow_takeover` | bool | `true` | Lets a writable follower take layout control immediately. |
| `on_attach` | string | `"ask"` | When another client controls the session: `"ask"` offers Follow, control, or Cancel; `"follow"` and `"take"` choose without prompting. `sessions attach --follow` or `--take-control` overrides it for one attach. |

See [Sessions](sessions.md).

## `[remote]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `default_host` | string | none | Host used by `--remote` without a value. |
| `reconnect_on_launch` | bool | `true` | Reconnect hosts you deliberately connected, without SSH prompts or installation. Disconnecting a host removes it from future launches. |
| `connection_timeout_secs` | integer | `15` | SSH `ConnectTimeout`. Also sets how long each probe, install step, and remote `sessions` command may run before rozi stops it. `0` turns off `ConnectTimeout` and uses the default command limit. |
| `server_alive_interval_secs` | integer | `15` | Minimum `1`. |
| `server_alive_count_max` | integer | `3` | Minimum `1`. |
| `install` | string | `"prompt"` | Whether to install rozi on the remote host: `"prompt"`, `"always"`, or `"never"`. Noninteractive runs never install. |
| `batch_mode` | bool | `false` | When `true`, refuses SSH prompts. By default, a running client answers them in a dialog. See [Remote sessions](remote.md#prompts-inside-the-ui). |

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
| `height` | float | `0.4` | Docked height as a fraction of the pane area. Clamped to `0.1..=0.9` with a warning. |

See [Popups and scratch panes](layouts-and-panes.md#popups-and-scratch-panes).

## `[sidebar]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `tabs` | array | `["activity", "agents", "panes", "sessions", "files", "git", "worktrees", "projects"]` | Tab definitions. IDs must be unique; hiding a tab does not remove its definition. |
| `layout.left`, `layout.right` | table | See below | Independent dock preferences. |
| `layout.hidden` | string array | `[]` | Hidden tab IDs; placement and definitions remain saved. |
| `startup` | bool | `false` | Open the remembered docks at client launch. When false, start hidden; `b` or `B` brings them back. |
| `layout.<dock>.width` | integer | `32` | Clamped to `16..=80`; both docks share the available width. |
| `layout.<dock>.panel_count` | integer | left `2`, right `1` | `1..=3`; reducing the count retains saved panels. |
| `layout.<dock>.panels` | table array | left session/repository groups, right empty | Each table contains `weight` (finite, positive) and ordered `tabs` (IDs). Unknown IDs retain their placement. |
| `background_follows_canvas` | bool | `false` | Paints the sidebar with the canvas background instead of the raised panel fill. |
| `gap` | bool | `true` | Keeps one row between each panel's tab bar and its list. |
| `background` | bool | `true` | Paints the tab strip as a distinct bar. Off, the strip matches the body. |
| `tab_style` | string | `"padded"` | `"padded"`, `"round"`, or `"arrow"`. Round and arrow need `nerd_icons`. |
| `settings.<tab>` | table | none | A built-in tab's settings. See [Built-in tab settings](#built-in-tab-settings). |

Unplaced tabs join the first panel of the left dock, or of the right dock when only it holds tabs;
Files, Git, Worktrees, and Projects join that dock's second panel. The default left panel weights are `0.4`
and `0.6`; the right panel weight is `1.0`. See [Sidebar](sidebar.md#configure-tabs-and-panels)
for arranging tabs and panels.

### Tab tables

An entry in `tabs` can be a table instead of an ID. A table defines a custom tab: a launcher tab
with fixed rows, a command tab that lists a command's output, or a stream tab that programs publish
rows to.

| Tab key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `name` | string | required | Unique tab ID. Built-in tab IDs are reserved. |
| `label` | string | required for custom tabs | Built-in tree tab labels are fixed. |
| `entries` | array of tables | none | Launcher rows. A custom tab needs exactly one of `entries`, `command`, or `stream = true`. |
| `command` | string | none | Command whose output becomes the tab's rows. |
| `format` | string | `"lines"` | `"lines"` or `"json"`. Command tabs only. See [JSON command tabs](sidebar.md#json-command-tabs). |
| `stream` | bool | `false` | A tab programs publish rows to. See [Stream tabs](sidebar.md#stream-tabs). |
| `interval` | integer seconds | `30` | Minimum `5`. Command tabs only. |
| `on_click` | action table | none | Action for a command tab's row. |
| `group_prefix` | string | none | Line command tabs only. Output lines starting with it become section headers. |

A launcher entry takes a `label`, exactly one of `run`, `send`, or `popup`, an optional `keep_open`
(default `true`), and an optional `group`. An `on_click` action takes an optional `label`, exactly
one of `run`, `send`, `popup`, or `exec`, and an optional `keep_open`. In `on_click`, `label` only
changes how the command is presented.

An extension can add the same kinds of tab from its manifest; see
[Extensions](writing-extensions.md#sidebar-tabs).

### Built-in tab settings

Built-in tabs are listed in `tabs` by name only. Activity, Agents, Files, and Git take settings in a table
of their own, `[sidebar.settings.<tab>]`. **Sidebar tabs…** edits the same tables; see
[Tab settings](sidebar.md#tab-settings).

```toml
[sidebar.settings.activity]
worktrees = "repository"

[sidebar.settings.files]
explorer = false
show_hidden = false
```

| Tab | Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- | --- |
| `activity` | `worktrees` | string | `"separate"` | `"separate"` or `"repository"`. See [Activity](sidebar.md#activity). |
| `activity` | `agents` | bool | `false` | Lists coding agents in Activity as well as in the Agents tab. See [Activity](sidebar.md#activity). |
| `agents` | `worktrees` | string | `"separate"` | `"separate"` or `"repository"`, independent of Activity's. See [Agents](sidebar.md#agents). |
| `files`, `git` | `explorer` | bool | `true` for files, `false` for git | Shows a search field above the tree. |
| `files`, `git` | `show_hidden` | bool | `true` | Lists entries whose names start with a dot. |
| `files`, `git` | `root` | string | `"cwd"` for files, `"repo"` for git | `"cwd"` for the focused pane's directory, `"repo"` for the repository containing it. |
| `files`, `git` | `diff_stats` | bool | `false` for files, `true` for git | Shows line-change counts beside Git marks. |
| `git` | `compact_dirs` | bool | `true` | Shows a directory whose changes are all in one subdirectory on that subdirectory's row, as in `src/view/sidebar`. |
| `files`, `git` | `max_entries` | integer | `2000` | Clamped to `1..=10000`. |
| `files`, `git` | `on_click` | action table | `{ send = "{path}" }` | What activating a file does. Not shown in **Sidebar tabs…**. |

### Grouping rows into sections

A launcher entry's `group` puts it under a section header. Sections appear in the order their group
first appears; entries without a `group` come first, with no header.

```toml
[sidebar]
tabs = [
  { name = "launch", label = "Launch", entries = [
    { label = "rozi", group = "claude", run = "cd ~/Projects/rozi && claude" },
    { label = "docs", group = "claude", run = "cd ~/Projects/docs && claude" },
    { label = "rozi", group = "codex", run = "cd ~/Projects/rozi && codex" },
  ] },
]
```

A command tab groups its own output with `group_prefix`: an output line starting with the prefix
becomes a section header showing the rest of the line. Headers cannot be selected, so `on_click`
applies to every other row. Its `on_click` `send` action may use `{line}` for the row's text;
`run`, `popup`, and `exec` receive the row in `ROZI_ROW` instead. See
[Command tabs](sidebar.md#command-tabs).

### Opening a diff viewer or editor from a row

A tree tab's `send` action may use `{path}`, because the result is typed into the pane as literal
input. Do not end it with a newline unless you mean to run the text.

Tree `run`, `popup`, and `exec` actions receive the selected path in `ROZI_FILE`. `run` and `popup`
reject `{path}`, and `exec` does not expand it. Quote the variable for your command shell:

```toml
[sidebar.settings.files]
on_click = { run = '''"${EDITOR:-vi}" "$ROZI_FILE"''' }

[sidebar.settings.git]
on_click = { popup = '''git diff -- "$ROZI_FILE"''', keep_open = false }
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

Click `title` to open the command palette. Click `location` or `session` to open Sessions.

A command segment times out after 5 seconds and keeps at most 64 KiB from each output stream.

### `[workbar.alert]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `bell` | bool | `true` | Marks workspaces with a bell. |
| `blocked` | bool | `true` | Marks blocked workspaces. |
| `error` | bool | `true` | Marks workspaces with an error you have not looked at. |
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

Defaults for [recordings](recording.md). A pane recording reads these on the session's host, so for a
remote session they come from that host. A [UI recording](recording.md#record-the-whole-ui) uses the
UI's own configuration.

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `dir` | path string | `recordings` in the state directory | Where a recording started without `--output` is written, including from **Start pane recording** and **Start UI recording**. `~` expands. Created mode `0700` if missing. |
| `max_fps` | integer | `30` | `1` to `120`. |
| `duration` | duration string | `"24h"` | Such as `"90s"`, `"8h"`, or `"1h30m"`; at most `"7d"`. |
| `max_bytes` | size string | `"1GiB"` | Such as `"512MiB"`; at least `"64KiB"`. |
| `ui_indicator` | bool | `true` | Whether a UI recording shows its `REC` chip. When `false`, the chip is neither on screen nor in the recording. `rozi record start ui --hide-indicator` hides it for one recording. |

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
| `history` | string | `"mouse-scroll"` | `"mouse-scroll"` lets `agents prompt --result` scroll the agent's transcript with the mouse wheel to read its [alternate-screen history](agents.md#read-the-agents-response); `"none"` never does. Use `"none"` if the wheel does anything else in this agent. |

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

## `[[links.handlers]]`

Choose what happens when you `Ctrl`-click a link or select a hint with an uppercase final
label character. The first handler matching the URI scheme wins. Unmatched URLs keep the
system opener, which supports `http`, `https`, and `mailto`; other schemes require a handler.
A `file` handler also opens file paths shown in plain text, by `Ctrl`-click or hint. A path is a
link only when it exists; on a [remote session](remote.md) rozi cannot check, so every path that
names a directory, such as `src/main.rs`, is one.
A path in a pane whose shell runs on another host, such as over `ssh`, is not a link. Installed
extensions can contribute
handlers automatically; user handlers run first. Disable an extension in **Extensions…** to remove
its handlers.

```toml
[[links.handlers]]
schemes = ["http", "https"]
run = 'terminal-browser open "$ROZI_URL"'
keep_open = false

[[links.handlers]]
schemes = ["file"]
popup = 'nvim ${ROZI_LINE:+"+$ROZI_LINE"} -- "$ROZI_FILE"'
keep_open = false
```

To reuse a terminal-browser, replace `run` with `exec = 'terminal-browser new-tab "$ROZI_URL"'`.

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `schemes` | string array | required | Nonempty URI schemes, matched without case sensitivity. |
| `files` | string array | all files | File-name patterns a `file` handler opens, such as `"*.png"`; `*` and `?` are wildcards and case is ignored. See [Open clicked files in popups](recipes.md#open-clicked-files-in-popups). |
| `command` | string | unset | Configured or extension command ID. Cannot be combined with inline action fields. |
| `run` | string | unset | Shell command in a new pane. Set exactly one of `command`, `run`, `popup`, or `exec`. |
| `popup` | string | unset | Shell command in a floating popup. |
| `exec` | string | unset | Shell command without a pane, with output discarded. |
| `keep_open` | bool | `true` | Preserve output after `run` or `popup` exits. |

To use a command supplied by an extension, name its public ID:

```toml
[[links.handlers]]
schemes = ["http", "https"]
command = "terminal-browser.open"
```

The [terminal-browser extension](https://github.com/tui-lipan/rozi-terminal-browser) contributes
HTTP and HTTPS handlers on installation and requires no handler configuration. Explicit command
references are useful for overriding its default behavior. A missing, disabled, or non-process
command reports an error rather than falling through to another handler.

Commands receive the following variables. Quote their expansions in shell commands; for PowerShell,
use `$env:ROZI_URL`, `$env:ROZI_FILE`, and so on. The examples above use POSIX shell syntax.

| Variable | Value |
| --- | --- |
| `ROZI_URL` | Original URI; empty for plain paths and custom hint actions. |
| `ROZI_FILE` | Decoded file-URI path, or a plain path without its `:line[:column]` suffix, made absolute (on a remote session, as shown); otherwise empty. |
| `ROZI_LINE`, `ROZI_COLUMN` | Position suffix of a plain path, or empty. |
| `ROZI_HINT` | Full original clicked URI or selected hint, including any position suffix. |
| `ROZI_SOURCE_PANE` | Pane containing the clicked link or selected hint. |
| `ROZI_SOURCE_SESSION` | Opaque session-server instance ID containing the source pane. |
| `ROZI_REMOTE_HOST` | Remote session host, or empty for local sessions. |

Referenced commands run at their declared placement. `send` commands cannot be used as handlers.
Custom hint actions can also reference commands, for example
`on_open = { command = "my-extension.inspect" }`.

`run` and `popup` execute on the session host in the source pane's working directory.
`exec` executes on the client; it inherits the source directory only for a local session and
receives `ROZI_PANE` identifying the source pane. A plain path opens only through a handler that
runs on the machine the file is on, since rozi does not fetch or translate remote paths: on a
remote session that is `run`, `popup`, or a command placed on the session host, and in the
[scratchpad](layouts-and-panes.md#popups-and-scratch-panes), `run` or `exec`.
A filename ending in `:123` is read as a position; use a file URI to keep that suffix as part of the
filename.

## `[[hints]]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `pattern` | string | required | Nonempty `regex-lite` pattern. Invalid patterns are skipped. |
| `open` | bool | `false` | Lets the uppercase hint label open the match through link handlers. |
| `on_open` | command table | unset | `command`, `run`, `popup`, or `exec` action for an uppercase label; enables the action even when `open = false`. Inline actions may set `keep_open`, which defaults to `true`. |

Custom hint actions receive `ROZI_HINT` containing the match:

```toml
[[hints]]
pattern = '\bISSUE-[0-9]+\b'
on_open = { exec = 'issue-viewer "$ROZI_HINT"' }
```

Lowercase labels always copy. Git SHA hints remain copy-only.
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
| `exec` | string | none | Runs the command in the background through `command_shell` and discards its output. Runs on this machine, in the focused pane's directory when that pane is local. Shares the 32-job limit with hooks. |
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
| `disabled` | array of strings | `[]` | Manifest IDs of extensions to disable; the Extensions overlay edits this key. |

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

For packaged automation, use extension services instead. See [Services](writing-extensions.md#services).

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
