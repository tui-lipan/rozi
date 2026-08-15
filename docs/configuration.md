# Configuration

`rozi` reads a single TOML config file at startup. All keys are optional; anything you
omit keeps its default. A read or parse failure does **not** crash the app - it loads
defaults and reports the problem as a startup toast. Unknown keys are parse failures, so a
misspelled setting is reported rather than silently ignored; the message lists the keys the
table accepts.

[`examples/config.toml`](../examples/config.toml) is the copyable version of this page: every
setting, commented out at its default value, so the file as shipped behaves exactly like having
no config at all.

## Config file location

`rozi` resolves the config path in this order:

1. `--config <PATH>`, which sets `$ROZI_CONFIG` for the run.
2. `$ROZI_CONFIG` (a full path; `~` and `~/...` expand to `$HOME`).
3. `$XDG_CONFIG_HOME/rozi/config.toml`, else `~/.config/rozi/config.toml` — on Windows,
   `%APPDATA%\rozi\config.toml`.

`--config` applies to every command that reads config — a launch, `--server`, and the remote forms
of `list-sessions` / `kill-session`. Control commands never load config and reject it.

On startup a toast reports any warning raised while reading or parsing the file. A config that
loads cleanly is silent - the settings taking effect is the confirmation.

### Where rozi keeps its files

Every path below is the *base* directory; profiles, themes, session snapshots, and pane logs live
under it. macOS follows the XDG convention rather than `~/Library`, matching tmux, neovim, and
alacritty. A relative `XDG_*` override is rejected outright rather than being resolved against the
working directory, which would make the config directory move every time you `cd`.

| | Linux/macOS | Windows |
| --- | --- | --- |
| Config (`config.toml`, `themes/`, `profiles/`) | `$XDG_CONFIG_HOME/rozi`, else `~/.config/rozi` | `%APPDATA%\rozi` |
| State (session autosave, resurrection snapshots, pane logs) | `$XDG_STATE_HOME/rozi`, else `~/.local/state/rozi` | `%LOCALAPPDATA%\rozi` |
| Cache (generated shell-integration scripts) | `$XDG_CACHE_HOME/rozi`, else `~/.cache/rozi` | `%LOCALAPPDATA%\rozi\cache` |
| Runtime (control and session endpoints) | `$XDG_RUNTIME_DIR/rozi`, else a private per-uid temp directory | `%LOCALAPPDATA%\rozi\run` |

### Live reload and editing

rozi watches the config file and applies every save live - config fields, `[keys]`
bindings/user commands, theme (including switching which file the theme watcher follows), and
workbar segments - without touching running panes, workspaces, or the active session. A parse
failure reloads to defaults and reports it as a toast, same as at startup; fix the file and
save again.

Toggles and cycles in Settings, Appearance, and Alerts write the new value back to this file.
Those writes, along with theme selection and the default profile, are already applied and don't
trigger a reload.

The **Open config file** command-palette entry (`open-config`) opens the file in `$EDITOR`
(falling back to `$VISUAL`, then `vi`) in a new pane. From the sessionless launcher it starts an
ephemeral session first so the editor has a live PTY. It is an ordinary action, so it also
works as `rozi run-action open-config` over the control socket (see `docs/control.md`).

## Full example

The copyable file is [`examples/config.toml`](../examples/config.toml): every setting, commented
out at its default. Uncomment only the lines you want to change. The rest of this page is the
reference for each key.

```toml
#shell = "/bin/sh"
#cwd = "~/code"
#scrollback = 5000

[input]
#modifier = "alt"
#prefix = "ctrl-a"

[layout]
#default = "dwindle"

[pane]
#show_workbar = true
#titlebar = "bar"

[theme]
#name = "lipan"

[session]
#startup = "picker"
```

## Top-level keys

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `shell` | string or array | see below | Interactive shell launched in each new pane. |
| `command_shell` | string or array | see below | Shell used to run one-off command lines (pane/popup commands, hooks, workbar `command:` segments, `[keys] run`, control-socket run requests). |
| `shell_integration.mode` | `auto` or `off` | `auto` | Inject OSC cwd/command metadata into supported interactive shells. |
| `cwd` | path | launch directory | Working directory for new panes. `~` expands to `$HOME`. |
| `scrollback` | integer | `5000` | Scrollback lines per pane (minimum 1). Existing screens keep their current capacity; restart a named server or create new panes after changing it. |

Both `shell` and `command_shell` accept either a bare string (a program with no arguments - the
historical form) or an argument-preserving array whose first element is the program, e.g.
`shell = ["pwsh.exe", "-NoLogo"]`. Resolution order when unset:

| Purpose | Linux/macOS | Windows |
| --- | --- | --- |
| `shell` | `$SHELL`, else `/bin/sh` | `pwsh.exe`, else `powershell.exe`, else `%COMSPEC%`, else `cmd.exe` (found via `PATH` + `PATHEXT`) |
| `command_shell` | `["/bin/sh", "-c"]` (fixed, never probes `$SHELL`) | `[%COMSPEC%, "/D", "/S", "/C"]` (fixed) |

`command_shell` is deliberately never detection-based, so a `[keys] run`/hook/workbar-command
snippet using it behaves identically regardless of the invoking user's interactive shell choice.
Both are resolved by the client (not the session server) at spawn/command-run time, so a
detached/persistent named-session server never falls back to its own process environment or a
stale on-disk config after the client-side config hot-reloads.

## `[shell_integration]`

With `mode = "auto"` (the default), rozi injects its shell integration into supported
**interactive** shell panes only. It never changes dotfiles, registry settings, or the
noninteractive `command_shell` runner. The integration emits OSC 7 current-directory updates and
OSC 133 prompt/command lifecycle markers; it sends only the executable basename for smart focus,
never a full command line.

| Shell | Injection mechanism | What you get |
| --- | --- | --- |
| bash | Generated `--rcfile` wrapper | Everything. Chains `/etc/bash.bashrc` and `~/.bashrc`, then the integration. Login-shell configurations are intentionally left untouched, because bash ignores `--rcfile` for login shells. |
| zsh | Temporary `ZDOTDIR` shim | Everything. Chains the original `ZDOTDIR` (or `$HOME`) `.zshenv`/`.zshrc`, then the integration. |
| fish | Temporary `XDG_DATA_DIRS` vendor `conf.d` entry | Everything. Composes with Fish event hooks; prompt frameworks loaded later can replace its final prompt marker. |
| PowerShell | `-NoExit -Command . <script>` | Everything. Runs *after* `$PROFILE`, wrapping your prompt and PSReadLine rather than replacing them. A pane whose `shell` already carries `-Command`/`-File` is left alone. |
| cmd.exe | `PROMPT` environment variable | Working directory and prompt boundaries only. cmd has no pre-execution hook; rozi will not touch the `AutoRun` registry key. Install [Clink](https://chrisant996.github.io/clink/) for the rest. |

Set `mode = "off"` if your shell already emits suitable OSC metadata or if you want rozi to
leave shell startup completely unchanged.

### PowerShell sessions rozi did not launch

A pane whose `shell` already carries `-Command`/`-File` is a "run this and exit" launch, not an
interactive session, so injection is skipped.

The `-NoExit -Command` injection above only applies to panes rozi starts. For a PowerShell
reached some other way — nested inside a pane, or launched through a `command =` pane — add one
line to your `$PROFILE` instead:

```powershell
. "$env:LOCALAPPDATA\rozi\cache\shell-integration\rozi.ps1"
```

(On Linux/macOS the script lives under `~/.cache/rozi/shell-integration/`.) The script is
idempotent, so having both the `$PROFILE` line and the automatic injection is harmless. The
equivalent for cmd.exe is `rozi.cmd` in the same directory, which just sets `PROMPT`.

## `[input]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `modifier` | string | `alt` | Held WM modifier for generated direct command keys and mouse gestures; `alt`/`mod` or `super`/`meta`/`logo`/`win`. |
| `prefix` | string | `ctrl-a` | Prefix key used by generated leader chords, e.g. `ctrl-a`, `ctrl-b`. |
| `modifier_shortcuts` | bool | `true` | Also bind every built-in default as a held `<modifier>+<key>` chord (e.g. `Alt+q`) alongside its `<prefix> <key>` leader. Set `false` so held `Alt`/`Super` chords reach the focused pane. |
| `which_key` | string | `short` | The which-key strip - a compact table of what the prefix can do next - beside the workbar while a prefix chord is pending, and how long the prefix is held before it appears: `off` (never), `instant` (no wait), `short` (300ms), or `long` (750ms). |

### The which-key strip

Pressing the prefix leaves rozi waiting for a second key, and while it waits the strip lists the
chords that second key can be. It is drawn from the live command registry, so `[keys]` overrides and
unbound commands are reflected without any separate table to keep in sync, and only commands that
can act right now are listed.

Unless `which_key` is `instant`, it waits out that delay first, so a chord you finish from muscle
memory never flashes it - the strip is for the moment you hesitate. The workbar's `PREFIX` badge
and the withheld pane caret are not delayed: those confirm the keystroke landed, which has to be
immediate.

Three things keep it small enough to sit over live panes:

- Directional families collapse into one row (`hjkl Focus pane`, `HJKL Swap pane`,
  `ctrl+hjkl Move pane`, `1-9 Workspace`). Rebinding any member of a family expands it back into
  individual rows, so a customized binding is never misreported.
- Commands that need a second tile - focus, swap, move, split resize, promote - are left out in a
  workspace that only has one pane.
- The strip is capped at a fifth of the viewport height. Anything that does not fit is counted in
  the top-right corner (`+12 · ? all`) rather than paged, and the listed key opens the full
  keybindings overlay.

Held `<modifier>+<key>` chords resolve on a single keypress and never leave a chord pending, so the
strip only ever appears for the leader scheme. The workbar's `PREFIX` badge is independent of this
setting and stays either way.

The key is also reachable from Settings (command palette, `prefix p`, then **Settings…**) under
General, as **Which-key**, which cycles `Off` / `Instant` / `Short` / `Long`.

The `modifier_shortcuts` switch is all-or-nothing. To drop the mirror for a single command only, override it in
`[keys]` with a literal leader-only binding, e.g. `detach = "ctrl-a d"`. Bare-key replacements
and generated defaults follow `modifier_shortcuts`; literal bindings and additive literal bindings
are used exactly as specified.

### Prefix syntax

Prefix strings use tui-lipan keybinding syntax. Modifiers include `ctrl`/`control`, `alt`,
`shift`, and `super`/`cmd`/`command`/`meta`/`win`. Named keys include `enter`/`return`,
`esc`/`escape`, `space`, `tab`, `backspace`, arrows, navigation keys, and function keys.
Examples: `ctrl-a`, `ctrl-b`, `alt-space`, `f12`. The prefix must be one key; an unparseable
prefix is reported as a warning and the default is kept.

## `[layout]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `split_width_multiplier` | float | `2.3` | Terminal cell height divided by cell width. Dwindle uses this to choose the next split axis. Must be positive. Increase it when panes that look taller than wide split side-by-side. |
| `default` | string | `"dwindle"` | Layout every fresh workspace starts in: `dwindle`, `master`, `grid`, `columns`, `rows`, `scrollable`, or `monocle`. Profiles override this per workspace. |

The *Choose layout…* picker can persist `default` with `ctrl+f` on a mode.

## `[pane]`

Pane focus and chrome behavior.

| Key | Default | Notes |
| --- | --- | --- |
| `focus_on_hover` | `true` | Moving the mouse over a pane focuses it. |
| `hold_on_exit` | `false` | Keep naturally exited panes in the layout. Their title shows the exit code; `respawn-pane` restarts the retained command and cwd in place. |
| `highlight_focused_background` | `false` | Give the focused pane the theme panel background. When `false`, focus does not change the pane background. |
| `highlight_focused_border` | `true` | Give the focused pane the theme's active border color. In `separate`/`merged` this accents the frame; in `dividers` it accents only the internal seams that touch the focused pane. |
| `highlight_focused_titlebar` | `true` | Use focused titlebar colors and emphasis in every titlebar layout. |
| `show_workbar` | `true` | Show the workbar (workspace tabs, mode chips, configured segments). When `false`, panes use the full viewport height with no top gap. |
| `workbar_gap` | `true` | Show a 1-line gap between the workbar and the panes area. |
| `workbar_at_bottom` | `false` | Draw the workbar on the last row (below the panes) instead of the first. The gap, when enabled, moves with it. |
| `show_titles` | `true` | Show the selected pane titlebar layout. Toggling this does not change the selected `titlebar` layout. |
| `titlebar` | `bar` | Pane title layout: `bar`, `border`, `integrated`, or `inset`. See [Titlebar layouts](#titlebar-layouts). |
| `border_mode` | `separate` | Pane border presentation: `separate` (one frame per pane), `merged` (shared junctions), `none` (no frames), or `dividers` (lines along internal tiled splits only). |
| `border_style` | `rounded` | Frame glyphs for `separate` and `merged`: `rounded`, `plain`, `double`, or `thick`. Does not affect `dividers`. The Settings row is disabled when the selected mode has no pane frames. |
| `alert_border` | `pulse` | `off`, `static`, or `pulse` for unfocused attention borders. The Alerts action is `cycle-alert-border`; it is disabled in `none` mode. |
| `keep_special_borders` | `true` | Keep double frames around floating panes, popups, and the scratchpad in `none` and `dividers` modes. Set `false` to let those panes go frameless. Fullscreen panes follow the selected global mode. |
| `padding` | `0` | Blank cells between each pane's border and its terminal grid. A single number, `[vertical, horizontal]`, or `[top, right, bottom, left]`. Each side is clamped to `8`. |
| `title_style` | `padded` | End-cap style for `titlebar = "bar"` or `"integrated"`: `padded`, `half`, `round`, or `arrow`. See [End-cap styles](#end-cap-styles). |
| `workbar_badge_style` | `padded` | End-cap style for the workbar's colored badges: `padded`, `round`, or `arrow` (`half` is not available). See [End-cap styles](#end-cap-styles). |
| `workbar_powerline` | `true` | Chain trailing badges into a powerline: gaps collapse and each cap blends into its left neighbor. Independent of `workbar_badge_style`. |
| `toast_opacity` | `0.8` | Toast background opacity over the pane it covers, in `[0.0, 1.0]`. `1.0` is solid; below that the panel blends with whatever is behind. Raise it if toasts read poorly. |
| `workbar_tab_style` | `padded` | End-cap style for workspace and sidebar tabs. Same values as `workbar_badge_style`. When unset, `workbar_badge_style` is used. |
| `workbar_style` | `padded` | End-cap style for the workbar itself, so the panel reads as a pill/point over the backdrop. Same values as `title_style`. |
| `background_follows_terminal` | `false` | Pin `surface.backdrop` (canvas gaps, unfocused pane frames) to the host terminal's background, overriding the active theme. |

`hold_on_exit` governs panes with no launch command of their own (a plain shell you typed `exit`
in). A pane launched with a command follows that command's `keep_open` instead, which replaces the
dead PTY with a live shell rather than retaining a husk.

See [Matching the host terminal's background](themes.md#matching-the-host-terminals-background)
for `background_follows_terminal`.

### Titlebar layouts

- `bar` — a separate full-width title row above the frame (default).
- `border` — icon and title in the top frame border.
- `integrated` — fills the top border row as a compact title strip.
- `inset` — top border unbroken; title on the first row inside the frame, with no background of its own.

`border` and `integrated` each retain the terminal row that `bar` and `inset` consume.

### Padding

Accepts CSS-style shorthand: one number for all sides, two for vertical/horizontal, or four for
top/right/bottom/left. Other lengths are ignored with a warning. Each cell costs a column or row of
usable terminal space, painted with the pane's frame background.

Settings → Terminal padding writes the two-value `[vertical, horizontal]` form and normalizes any
four-side asymmetric padding.

### End-cap styles

`title_style`, `workbar_badge_style`, `workbar_tab_style`, and `workbar_style` share these values:
`padded` (flush bar, blank side padding), `half` (`▐`/`▌` half-block caps), `round` or `arrow`
(powerline pill/point caps). `round` and `arrow` need a patched/Nerd font, like the titlebar icons.

- `title_style` applies to `titlebar = "bar"` or `"integrated"`. Integrated half-block caps replace
  the frame corners; round and arrow caps sit immediately inside them. The Settings row is disabled
  under `border` and `inset`, which draw plain text with no strip to cap.
- `workbar_badge_style` is the same except `half` is not available. The `rozi` title chip caps on
  its right and the mode chips (`PREFIX`/`RESIZE`/`COPY`) cap on their left, so each pill rounds off
  toward the workbar's edge. Existing configs without `workbar_tab_style` also apply this value to
  workspace and sidebar tabs.
- `workbar_tab_style` caps only the active and hovered tab (tabs are peers, so they do not chain).
  When unset, `workbar_badge_style` is used.
- `workbar_style` caps the workbar itself so the panel reads as a pill/point over the backdrop.
  Caps replace the bar's outer side padding rather than widening it.

When `workbar_powerline` is `false`, trailing badges keep a 1-cell gap and each cap is drawn over
the panel bar. Adjacent badges with the same color retain a contrasting seam (`` for arrow caps,
`▏` for round and padded badges). The `[workbar]` section covers the same chaining for mode chips
and right-region badges.

### Toast opacity

The default reads as tinted glass: the theme's `surface.panel` blended per cell with the content
underneath. Contrast then depends on what the toast covers and how much headroom the theme's
panel/text pair has. Raise the value if your theme's toasts are hard to read. Values outside
`[0.0, 1.0]` warn and are ignored.

See also [In-app toasts](#in-app-toasts).

### `[pane.alert]`

`[pane.alert]` assigns badge/theme roles to agent states: `blocked = "error"` and unseen
`finished = "success"` default on; `working` and `idle` default `off` because they are ambient,
not normally actionable. Configured-off states fall through to the next applicable state. A finished
alert clears when you focus its pane; closing and exited panes never alert.

Blocked and finished alerts are visible state, so they do not create success toasts. The shared
breathe period is [`[animations] alert_pulse_ms`](#animations).

## `[[rules]]`

Window rules apply to ordinary workspace panes spawned with an explicit command, including control
`new-pane`, `[keys]` `run`, and other interactive command-spawn paths. Matching is either a
case-sensitive command substring (`match`) or a regex (`match_regex`); set exactly one. The first
matching rule wins. Plain shell-pane spawns, profile restoration, scratchpads, and popups do not
use rules. Rules are command-based only; OSC titles arrive after spawn and are never matched.

```toml
[[rules]]
match = "btop"
float = true
width = 0.7
height = 0.7
position = "cursor"

[[rules]]
match_regex = "^cargo\\s"
workspace = 9
focus = false
```

| Key | Default | Notes |
| --- | --- | --- |
| `match` | — | Non-empty command substring. Exactly one of `match` / `match_regex` is required. |
| `match_regex` | — | Regex matched against the full command line (`regex-lite`). Invalid patterns warn-and-skip. |
| `float` | `false` | Spawn as a floating pane. |
| `width`, `height` | `0.6` when floating | Fractions of the pane canvas, clamped to `0.1..=1.0`. |
| `position` | `center` | Float-only. Where the pane sits: `center`, `cursor` (centered on the mouse pointer), `top-left`, `top`, `top-right`, `left`, `right`, `bottom-left`, `bottom`, `bottom-right`. Corners flush that corner of the pane to the canvas; sides center it on that edge. Ignored (with a load warning) unless `float = true`. |
| `workspace` | current | 1-based target workspace (`1..=9`). |
| `focus` | `true` | Switch to and focus the spawned pane. When false, the target workspace remembers the new pane as its own focus without stealing the current view. |
| `fullscreen` | `false` | Spawn with fullscreen enabled. |

## `[[hints]]`

Additive hint patterns for hint mode (`u`). Built-in URL / path / Git-SHA detectors always run
first; each `[[hints]]` entry appends another regex over the visible snapshot. Invalid patterns
warn-and-skip on load/reload. There is no disable/override syntax for built-ins.

Built-ins win on overlap: a custom match that intersects an existing URL/path/SHA on the same row
is dropped (including its `open = true` behavior). Trailing `.,;:!?)]}` characters are trimmed from
**custom** matches as well as built-ins, so a pattern that deliberately ends in `)` will not keep
that character in the captured text.

```toml
[[hints]]
pattern = '\b(?:[0-9]{1,3}\.){3}[0-9]{1,3}\b'
open = false

[[hints]]
pattern = '\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b'
```

| Key | Default | Notes |
| --- | --- | --- |
| `pattern` | required | Non-empty `regex-lite` pattern. |
| `open` | `false` | When true, an uppercase final label character opens the match (same path as URLs). |

## `[animations]`

Geometry animation is entirely app-side. The master switch is `enabled`; each animation
category can also be toggled individually, and the durations are configurable in
milliseconds.

| Key | Default | Notes |
| --- | --- | --- |
| `enabled` | `true` | Master switch. When `false`, all transitions are instant. |
| `spawn` | `true` | New panes fade in; surrounding panes animate to make room. |
| `close` | `true` | Closing panes fade and animate out. |
| `fullscreen` | `true` | Fullscreen enter/exit geometry animation. |
| `tile_float` | `true` | Tiling ⇄ floating geometry animation. |
| `axis_change` | `true` | Split-axis flip animation. |
| `sidebar` | `true` | Sidebar slide. The pane column is resized as the panel arrives, so its near edge is pushed while its far edge stays put. Like every geometry animation, the panes resize as they move. |
| `focus_chrome` | `true` | Border/titlebar color transitions when focus moves. |
| `pane_style` | `"scale"` | Shape of the pane open/close animation: `scale` or `slide`. See below. |
| `geometry_ms` | `220` | Base geometry transition duration; the scratchpad's deploy and the sidebar slide use two-thirds of this. |
| `close_ms` | `120` | Close transition duration. Ignored by `pane_style = "slide"`, which uses `geometry_ms`. |
| `focus_chrome_ms` | `160` | Focus-chrome transition duration. |
| `alert_pulse_ms` | `1600` | Shared breathe period for `alert_border = "pulse"` and inactive workspace-tab breathing. Half-period is floored at 400 ms. |
| `open_delay_ms` | `36` | Delay before a spawned pane begins fading in. |

Pulse needs both `enabled` and `focus_chrome`. No pulse timer runs until an eligible alert is
visible.

### Pane open/close style

`pane_style` picks what an arriving or leaving pane does. Both styles honour `spawn` and `close`, so
either can still be turned off one direction at a time.

`"scale"` (the default) scales the pane toward the centre of its own rectangle with a fade riding on
top, and the surrounding tiles glide into their new sizes.

`"slide"` instead slides the pane in from the edge it was split off — right for a side-by-side split,
below for a stacked one — clipped to its destination tile, so it emerges from behind the seam rather
than flying across its neighbour. There is no fade: the clip is what reveals it. The pane keeps its
final size for the whole slide, so the terminal grid never reflows part-way. The **springy** part is
the tile that gave up the space: it overshoots its new size slightly and settles, rather than gliding.
A closing pane slides back out toward the same edge it arrived from.

Only tiled panes slide. A floating pane has no tile edge to emerge from and no neighbour to take
space from, so it keeps the scale whatever `pane_style` says; the same goes for popups. Panes that
never went through a split — the first pane in a workspace, a restored layout, a session you attach
to — slide up from the bottom, matching the scratchpad.

**The spring scales with the pane, the timing does not.** The overshoot is a fraction of the distance
the tile travels, so a fixed amplitude throws a big tile proportionally further — a pane halving from
240 columns would spring 24 of them. The amplitude is sized from the tile instead, so the nudge stays
about three cells whether the tile is 30 columns or 240. Arrival runs for `geometry_ms` at every size:
stretching it to match the distance covered makes a large pane crawl, which reads worse than the extra
speed ever did.

A closing pane leaves toward the same edge it arrived from, and the tile taking its place expands in
that direction by that distance — so both run for `geometry_ms` and their shared edge is a single
moving boundary, which is what makes the pane read as *pushed* out rather than dragged behind. That is
also why `close_ms` does not apply to a slide: it is tuned for the scale, where the close is a short
pop the fade rides on, and a slide has a whole tile to cross in step with its replacement.

> **Size changes are snapped, not animated.** During an active move/resize or a viewport
> change, transitions become instant for the affected pane. This is deliberate: animating a
> pane's *size* would spam `pty.resize` / SIGWINCH and reflow the shell on every frame.
> Position and opacity animate; size lands in one step.

## `[theme]`

| Key | Default | Notes |
| --- | --- | --- |
| `name` | `lipan` | The active theme: a built-in preset id, `system` (host-derived colors), or the stem of a file in `~/.config/rozi/themes/`. A custom file shadows a built-in of the same name. |

Custom themes are **hot-reloaded** on change while active. If the name matches nothing, or a
custom file fails to load, `rozi` falls back to `lipan` and reports a warning. See
[Themes](themes.md) for the preset list and how terminal ANSI colors are derived from the
active theme.

## `[profile]`

| Key | Default | Notes |
| --- | --- | --- |
| `default` | _none_ | Profile seeding every session opened without a recipe. Explicit named targets, `--profile`, and `startup = "last"` take precedence. Also writable via **Ctrl+f** in **Profiles**. |

It seeds the launch, each new temporary session, and each named session created without a recipe.
With `[session] startup = "profile"` it also names the session a bare launch opens. Clearing the
default while startup is `"profile"` resets startup to `"picker"` so that mode is not left pointing
at nothing.

See [Named profiles](profiles.md) and [Project profiles & pane identity](project-profiles.md) for the profile format.

## `[clipboard]`

| Key | Default | Notes |
| --- | --- | --- |
| `enable_osc52` | `true` | Allow programs running in a pane to set the system clipboard via the OSC52 escape sequence. Requires restart after changing. |

See [Terminal features](terminal.md) for clipboard and selection behavior.

## `[notifications]`

Desktop notifications are disabled by default. When enabled, rozi sends natural pane-exit
notifications (not user-initiated pane closes) and selected pane-status notifications via
`notify-send` if it is available. Status notifications run only on the current session controller
and are suppressed while that pane is attended, avoiding duplicate or distracting notices. A pane is
attended only when both its host window and the pane itself are focused.
Failures are ignored and never block the UI.

| Key | Default | Notes |
| --- | --- | --- |
| `enabled` | `false` | Master switch for desktop notifications. |
| `pane_exit` | `false` | Notify when a pane exits naturally with code `0`. Off by default because this edge cannot be attendance-gated — the pane is gone. |
| `pane_exit_error` | `true` | Notify when a naturally exiting pane returns a non-zero code. `pane_exit` now covers clean exits only. |
| `pane_blocked` | `true` | Notify when an unattended pane effectively becomes blocked, including detected-only agents. |
| `pane_done` | `false` | Notify on the unseen working→quiescent finished edge. Reported-only transitions without a detected agent do not arm this existing edge. |
| `bell` | `true` | Mark an unattended pane urgent on BEL; returning to its focused host window clears urgency. Independent of desktop notifications. |

## `[sounds]`

Built-in WAV cues are extracted into rozi's cache and played best-effort. `player`, when set,
 receives the cue path as its final argument. Settings persists its sound rows; do-not-disturb is a
separate in-memory command for this client lifetime and mutes desktop and sound cues only. BEL and pane-status
cues use the attended rule; non-zero exit cues play regardless of pane attendance.

| Key | Default | Notes |
| --- | --- | --- |
| `enabled` | `false` | Master sound switch. |
| `bell`, `blocked`, `done`, `error` | `true` | Per-cue switches. |
| `throttle_ms` | `2000` | Per-cue repeat suppression; clamped to 100–60000. |
| `bell_file`, `blocked_file`, `done_file`, `error_file` | empty | Optional WAV override. |
| `player` | empty | Optional executable override; the file path is appended as its final argument. |

## `[navigation]`

Controls the vim-aware `smart-focus-left` / `-down` / `-up` / `-right` actions, which power
seamless `Ctrl-h/j/k/l` navigation across both rozi panes and editor splits (see
[Seamless vim / neovim navigation](keybindings.md#seamless-vim--neovim-navigation) for the full
wiring). These actions are unbound by default.

When a smart-focus action runs, rozi checks the focused pane's **foreground process name**. If
it matches one of the `editors`, the matching `Ctrl-h/j/k/l` is forwarded to that program so it can
move its own split; otherwise rozi moves pane focus in that direction.

| Key | Default | Notes |
| --- | --- | --- |
| `editors` | vim family + `hx`/`helix`/`kak`/`emacs`/`emacsclient`/`fzf` | Foreground process names that should receive `Ctrl-h/j/k/l` themselves. Setting this **replaces** the default list. Names match the executable basename (e.g. `nvim`). |

Foreground detection prefers what the shell itself reports (see
[`[shell_integration]`](#shell_integration)), and falls back to `/proc` on Linux and `libproc` on
macOS. Windows has no fallback — process inspection is deliberately unsupported — so a Windows pane
whose shell is not reporting metadata is treated as running an unknown program, and smart-focus
simply moves pane focus.

## In-app toasts

Distinct from the desktop notifications above: these are the transient messages rozi draws
inside its own window. There is no verbosity setting, because it aims not to need one. A toast
appears only when something happened that you cannot already see:

- **State you can read off the screen is never toasted.** Attaching, switching, renaming, taking or
  losing layout control, and locking input all show in the workbar badges (`󰛤 name`,
  `CTRL`/`FOLLOW`/`READ ONLY`, `SYNC`). Deleting a profile or killing a session from a list removes
  the row you acted on. Applying a profile rebuilds the panes.
- **Lossless normalization is silent.** If rozi can safely make a config value usable without
  dropping data, the resulting visible state is the feedback; it does not produce a warning toast.
- **Off-screen results are toasted.** Where a profile was written, what log file a pane is recording
  to, what a copy put on the clipboard, what a detach left running, and every failure.
- **Rejections say why.** An action that cannot run (not attached, read-only, nothing to copy, no
  hints in this pane) reports its reason rather than doing nothing.
- **Repeats never stack.** An identical message that is still on screen has its timer restarted in
  place: it does not blink, move, or pile up a column of copies. Holding a key against a read-only
  pane keeps one toast alive instead of drawing a new one per repeat.
- **Progress is superseded, not buried.** `Reconnecting to X…` is replaced by its outcome in the
  same slot, and cycling the layout replaces the mode name rather than stacking one per press.

Validation errors from a prompt render *inside* the prompt, under the field they are about, and
clear as soon as you edit it.

Toasts render as tinted glass: the theme's panel color blended per cell with the pane content they
cover. [`[pane] toast_opacity`](#toast-opacity) controls how far, up to `1.0` for a solid panel.
Raise it if your theme's toasts are hard to read.

## `[confirm]`

`[confirm]` governs **one** confirmation layer: the destructive *shortcuts* - the actions that
happen the instant you press a key, hold a modifier chord, or send a control-socket `run-action`.
Each key below toggles whether that shortcut asks first. An armed confirmation shows a red-bordered
toast and expires with it, after the shared
[confirmation window](sidebar.md#confirmation-window) of 3 seconds; the next press within that
window fires, otherwise it arms again. Running the same command from the **command palette** always
skips the confirmation - picking it from a searchable list is already a deliberate choice.

| Key | Default | Confirms before… |
| --- | --- | --- |
| `close_pane` | `false` | Closing a pane whose process is still running. |
| `kill_workspace` | `true` | Closing every pane on the active workspace. |
| `kill_session` | `true` | Shutting down the attached named session. |
| `quit_ephemeral` | `true` | Closing a temporary session from the leave prompt: `Enter` on an empty name arms, a second `Enter` closes it. With this off, the first press closes. Named sessions are never closed by leaving. |
| `new_temporary_session` | `true` | Discarding the current ephemeral session to start a fresh one (its panes are killed). Named sessions are detached and left running, so switching from one does not require confirmation. |
| `load_profile` | `true` | Replacing a live disposable session by opening a profile-backed named target. |

**Not covered by `[confirm]`:** the session picker, the session-naming prompt, and the sidebar carry
their own built-in confirmations that are **always on** and cannot be disabled here, because they
read off the affected UI element rather than a toast (a second `Enter`/`Ctrl+K`/click after a visible
cue). These are: killing a session in the picker (`Ctrl+K` twice), attaching away from an ephemeral
session (`Enter` turns the target row amber), creating a named session from an ephemeral one (`Enter`
turns the name prompt's border red), and the sidebar's `✕` and host disconnect rows (see
[sidebar.md](sidebar.md#closing-from-a-row)). They run on the same 3-second window.
See [sessions.md](sessions.md#switching-sessions-in-app-the-picker).

## `[session]`

Optional **local** session auto-save: persist the live layout when a local `rozi` client exits
and restore it on the next local launch. Like profiles, this restores *layout and launch intent*,
not live PTY state.

This is separate from named sessions (`rozi <name>`), which run PTYs in a
background session server and can be detached/reattached with live terminal state intact.

| Key | Default | Notes |
| --- | --- | --- |
| `autosave` | `false` | Write the layout on quit and restore it on startup. Also **Settings → Sessions → Layout autosave**. |
| `resurrect` | `true` | Snapshot named sessions so layout, commands, and scrollback can be restored after the server exits. Also **Settings → Sessions → Resurrect named sessions**. |
| `startup` | `"picker"` | What a bare launch does: `"picker"`, `"ephemeral"`, `"last"`, or `"profile"`. Also **Settings → Sessions → Startup mode**. |
| `path` | `$XDG_STATE_HOME/rozi/session.toml` | Session file location (falls back to `~/.local/state/...`). |
| `allow_takeover` | `true` | Let a writable follower take the layout-control lease immediately with `request-control`. Set `false` to wait for the controller to grant it. Read-only and parked clients can never take control. |

Each mode has exactly one spelling; an unknown value warns and leaves `"picker"` in place.
**Settings → Sessions → Startup mode** cycles the four in this order but offers `"profile"` only
while a default profile is set. `resurrect` applies to servers started after the change; a running
server already read this value.

An explicit target takes precedence over startup configuration. `startup = "last"` and `startup =
"profile"` choose a *named session*, so they take precedence over `[profile] default` and autosave;
those two remain ephemeral-layout seeders. `--remote` bypasses startup policy entirely — the remote
host owns its sessions.

`startup = "profile"` makes a bare launch behave exactly like `rozi <the default profile's name>`:
it attaches to that session when it is running, and otherwise creates it from
`profiles/<name>.toml` (or its resurrection snapshot). With no `[profile] default` set, a name that
is not a usable session name, or nothing to open under that name, it warns and falls back to the
`picker` path rather than attaching something else.

With `startup = "picker"` (the default, also reachable with `--pick`), the picker is always shown at
launch, including when its list is empty. Opening the picker creates no session: nothing is attached
until you choose. `Enter` on an empty list starts an ephemeral shell, while `Ctrl+N` creates a named
session. Resurrection snapshots appear as restorable rows. Dismissing the picker with `Esc` leaves
the client in the launcher with no session, where `Enter` (or any `spawn` binding) starts a shell and
the picker can be reopened at any time. See [Sessions](sessions.md).

When several clients attach to one session they share a live, server-authoritative layout with a
single controlling client. By default `request-control` (`g`) transfers the lease immediately: every
client that can attach is already the same OS account, and the usual second client is the same
person on another machine, for whom waiting to be granted the lease means walking back to the first
keyboard. Taking control is symmetric — the other client takes it back the same way — and destroys
nothing; it moves the lease and the canonical PTY size.

Set `allow_takeover = false` for a session shared with another *person*, where a silent takeover
would reflow their panes mid-thought; `request-control` then waits to be granted or declined. For a
person who should only watch, `rozi attach <name> --read-only` is stronger than either setting:
a read-only client can never take or be granted control. The current controller can change the
running session's policy with `toggle-control-takeover`; that runtime change does not rewrite the
config file. See [Shared live layouts](sessions.md#shared-live-layouts).

## `[remote]`

SSH attach for session servers on another host (`rozi --remote <alias-or-url>`). The client and
config stay local; PTYs run on the remote. See [Remote SSH sessions](remote.md).

| Key | Default | Notes |
| --- | --- | --- |
| `default_host` | _none_ | Host used when `--remote` is passed without an argument; also a fallback `[remote.hosts.*]` profile for identity/`ssh_args`/`binary_path`. |
| `connection_timeout_secs` | `15` | Passed to ssh as `ConnectTimeout`. |
| `server_alive_interval_secs` | `15` | ssh `ServerAliveInterval` for the proxy connection. |
| `server_alive_count_max` | `3` | ssh `ServerAliveCountMax`. |
| `install` | `"prompt"` | `"prompt"`, `"always"`, or `"never"`. Interactive TTYs may copy a compatible binary to `~/.local/bin/rozi` on the remote when missing; non-interactive runs never mutate the remote. |
| `batch_mode` | `true` | Pass ssh `BatchMode=yes`, refusing every interactive prompt. Set `false` to allow password/passphrase prompts — see the caveat in [Remote SSH sessions](remote.md#authentication). |

### `[remote.hosts.<alias>]`

Optional per-alias overrides. The alias matches a bare `--remote <alias>` argument.

| Key | Default | Notes |
| --- | --- | --- |
| `host` | alias name | SSH hostname when different from the alias. |
| `user` | ssh_config / current user | Remote login user. |
| `port` | ssh default | Remote SSH port. |
| `identity_file` | _none_ | Path to an identity file (`~` expands). |
| `ssh_args` | `[]` | Extra arguments inserted into the ssh command line. |
| `binary_path` | _none_ | Absolute path to rozi on the remote; skips probe/install. |

```toml
[remote]
install = "prompt"
connection_timeout_secs = 15
# batch_mode = false   # allow ssh to prompt for a password or key passphrase

[remote.hosts.workbox]
user = "raz"
port = 22
identity_file = "~/.ssh/id_ed25519"
# binary_path = "/usr/local/bin/rozi"
ssh_args = ["-o", "ProxyJump=bastion"]
```

## `[scratchpad]`

The dropdown scratch workspace (toggle: `` ` ``). Its panes and PTYs stay alive while hidden.

| Key | Default | Notes |
| --- | --- | --- |
| `command` | the normal shell | Program for the first pane of an empty scratch workspace (e.g. `btop`). |
| `cwd` | the configured `cwd` | Working directory for that initial pane. |
| `height` | `0.4` | Fraction of the viewport height it opens at; clamped to `0.1`–`0.9`. |

Drag the scratchpad's top edge (its title/top-border row) up or down to resize it while it is
open; a pane against that edge also resizes it by right-drag or in resize mode, since the edge is
the workspace border and has no split to move. See [Keybindings](keybindings.md#scratchpad). The
adjusted height overrides `height` for the rest of the session; it resets to `height` on restart.

Additional panes use ordinary pane allocation and layout actions, but the scratch workspace is
client-local: it is never saved to profiles or `SharedLayout` and is discarded on a session switch.

## `[sidebar]`

The optional sidebar is a resizable local navigation surface docked beside the app content.
See [Sidebar](sidebar.md) for how the built-in tabs behave, interaction, and shared session sizing.

| Key | Default | Notes |
| --- | --- | --- |
| `visible` | `false` | Initial visibility. `toggle-sidebar` persists this the same way `toggle-sidebar-split` writes `split`. |
| `width` | `32` | Requested width in columns (`16..=80`). On a narrow terminal the sidebar yields columns so the pane canvas keeps usable space. |
| `position` | `left` | Dock side: `left` or `right`. |
| `tabs` | `["activity", "panes", "sessions", "files", "git"]` | Catalog of available tab definitions. Built-in names are `activity`, `panes`, `sessions`, `files`, and `git`; each tab identity must be unique. |
| `panels` | `[["activity", "panes", "sessions"], ["files", "git"]]` | Durable placement: one or two ordered arrays of IDs from `tabs`. Missing configured tabs are appended to the first panel so they cannot become inaccessible. |
| `split` | inferred from `panels` (`true` by default) | Render two saved panel groups vertically. Turning it off shows all tabs in one bar without changing `panels`; turning it back on restores the saved assignment. |
| `split_ratio` | `0.4` | Fraction of split-sidebar height assigned to the top panel, clamped to `0.15..=0.85`. Dragging the panel divider updates and persists it. |

Naming `tabs` replaces the built-in catalog and its two-panel placement together, so those configs
get one panel unless they also name `panels`. When `split` is omitted, it is true for two panel
arrays and false for one. Reordering tabs updates only `panels`; custom tab definitions in `tabs`
are never rewritten. Drag, persist, and live-reload behavior is in [Sidebar](sidebar.md#configure).

```toml
[sidebar]
visible = true
tabs = ["activity", "panes", "sessions", "files"]
panels = [["activity", "files"], ["panes", "sessions"]]
split = true
split_ratio = 0.6
```

See [`examples/sidebar.toml`](../examples/sidebar.toml) for a larger two-panel setup combining all
built-ins with launcher and command-backed tabs.

### File tree tabs

`files` and `git` are two projections of one tree; option defaults differ by view (`root`,
`diff_stats`). How they re-root, when they load, and git refresh are in
[Sidebar](sidebar.md#built-in-tabs).

| Key | Default | Description |
| --- | --- | --- |
| `root` | `"cwd"` for `files`, `"repo"` for `git` | `cwd` roots at the focused pane's directory; `repo` roots at the git repository containing it, so changes elsewhere stay visible from a subdirectory. Falls back to `cwd` outside a repository. |
| `show_hidden` | `false` | Show dot-prefixed entries. |
| `icons` | `false` | Show file-kind icons. Off by default because the glyphs assume a Nerd Font. |
| `explorer` | `false` | Show a fuzzy-find input above the tree. Respects `.gitignore`/`.ignore`. |
| `diff_stats` | `false` for `files`, `true` for `git` | Show `+N -M` beside change markers. |
| `max_entries` | `2000` | Cap entries read per directory (1-10000). |
| `on_click` | `{ send = "{path}" }` | What activating a row does; `{path}` is the activated path. |

```toml
[sidebar]
visible = true
tabs = ["activity", "files", "git"]
```

Both take the same table form as custom tabs when you want options:

```toml
tabs = [
  "activity",
  {
    name = "files",
    label = "",
    show_hidden = true,
    explorer = true,
    on_click = { send = "nvim {path}\n" },
  },
  "git",
]
```

`label` is ignored for a built-in, which keeps its own name. The default `on_click` types the path
at the prompt without a newline, so nothing executes until you press Enter.

#### Opening a diff viewer or editor from a row

A `run` action opens the command in a new pane and `popup` opens it in a centered floating pane, so
a row click can launch a full-screen TUI. The activated path is **not** substituted into those
commands — a repository can contain a file named `; rm -rf ~`, and a command line assembled from a
filename would execute it. Instead the path arrives as the `ROZI_FILE` environment variable, so
the command references it as `"$ROZI_FILE"`: a quoted expansion is one word, never re-parsed for
command syntax.

```toml
# lazygit scoped to the clicked file
{ name = "git", label = "", on_click = { run = "lazygit -f \"$ROZI_FILE\"" } }

# the file's diff in a floating popup, closing when the pager quits
{ name = "git", label = "", on_click = { popup = "git diff -- \"$ROZI_FILE\"", keep_open = false } }

# open the clicked file in an editor pane
{ name = "files", label = "", on_click = { run = "$EDITOR \"$ROZI_FILE\"" } }
```

Quote the expansion (`"$ROZI_FILE"`, or `"%ROZI_FILE%"` under `cmd.exe`) so paths containing
spaces arrive as one argument. `send` is unaffected: it starts no process, and its `{path}`
substitution is plain typed text — see [the sidebar security notes](sidebar.md#security) before
adding a trailing newline to a `send` action.

Each custom table needs a unique non-empty `name`, a non-empty display `label`, and exactly one of
`entries` or `command`. Launcher entries require exactly one of `run`, `send`, or `popup` and execute
with the same behavior as user-defined `[keys]` commands, including `keep_open` (default `true`, so a
launcher entry's output survives the command exiting). Command tabs may provide an `on_click`
action with the same shape. They poll only while active and visible, run immediately on activation,
and have a five-second minimum interval. Output, runtime, rows, and row lengths are bounded; see the
[Sidebar security policy](sidebar.md#security).

```toml
[sidebar]
visible = true
width = 32
position = "right"
tabs = [
  "panes",
  { name = "deploy", label = "Deploy", entries = [
    { label = "Build", run = "cargo build" },
    { label = "Test", send = "cargo test\n" },
    { label = "Logs", popup = "journalctl -f", keep_open = false },
  ] },
  {
    name = "todos",
    label = "Todos",
    command = "task list --plain",
    interval = 30,
    on_click = { send = "task view {line}\n" },
  },
]
```

Unknown built-ins, duplicate/reserved/empty names, tables with both or neither content form, and
invalid launcher entries produce warnings and skip only the invalid item. Unknown table fields are
strict parse errors. `{line}` is replaced only for command-tab `on_click.send` actions and is sent
as literal PTY text. It is rejected in sidebar `run` and `popup` actions; those commands are never
constructed from command output.

## `[workbar]`

Customize the workbar. By default the `rozi` badge and workspace tabs are on the left, while the
remote `location` and named `session` badges are on the right. Every configured segment renders as
a colored badge; each kind has a curated default color that you can override by theme role (see
below). The `PREFIX`/`RESIZE`/`COPY`/`HINT`/`SIDEBAR`/`SYNC`/`DND` mode chips render only while `show_workbar` is
enabled, and sit to the left of the right-region segments so a `session` badge stays pinned to the
trailing edge. `SYNC` marks a workspace where [pane synchronization](layouts-and-panes.md) is on, so
the state that multiplies every keystroke across panes is always visible rather than announced once.
With `workbar_powerline` on (the default) the mode chips and right-region badges lose the gap
between them and interlock into a powerline: each chip's cap blends into its left neighbor's color.
`workbar_badge_style` controls the pill shape (rounded/pointed vs flush) independently. Workspace
and sidebar tab caps are controlled separately with `workbar_tab_style`.

| Key | Default | Notes |
| --- | --- | --- |
| `left` | `["title", "workspaces"]` | Ordered left-region segments. |
| `right` | `["location", "session"]` | Ordered right-region segments. |
| `clock_format` | `"%H:%M"` | strftime format, used by a `clock` segment. |

`[workbar.alert]` controls workspace-tab alerts independently of `[pane.alert]`. A marked tab is
tinted toward its state's color - error for a bell or blocked agent, success for an unseen finished
one, info while working - and its label stays plain identity text, so a workspace never changes
width when an agent blocks and the tabs beside it never shift. The flags enable bell, blocked, and
finished only, and say *which* states mark a tab.

| Key | Default | Notes |
| --- | --- | --- |
| `mode` | `pulse` | How a marked tab is drawn: `off`, `static` (color only), or `pulse` (also breathes). Mirrors `pane.alert_border`. Settings: **Alerts → Workspace tab effect**. |
| `paint` | `background` | What a marked tab colors. `background` fills it with the same end caps as the active tab; `text` colors the label only. Settings: **Alerts → Workspace tab highlight**. |

The Settings actions are `cycle-workbar-alert` and `cycle-workbar-alert-paint`. The effect row is
disabled while the workbar is hidden.

Only **inactive** marked tabs are colored or breathed — the active workspace keeps its solid tab pill,
and its pane border already carries the alert. Several marked tabs share one phase, so they breathe in
unison rather than to separate rhythms, while still using their own state's color. Breathing uses the
shared `alert_pulse_ms` period. Markers stay visible when pane borders or colors are off, including
`alert_border = "off"` and `border_mode = "none"`.

Segment kinds: `title` (the badge), `workspaces` (the tabs), `location` (the active remote host, or
the number of retained remote connections while local), `session` (the active profile/session
name), `clock`, `layout` (active workspace layout name), `activity` (panes with unseen output, shown as
`●N` for the current session and `+M` for retained background sessions when they have unread),
`text:<literal>` with `{host}`, `{workspace}`, `{layout}`, `{session}`
placeholders, and `command:<shell command>` / `command:<interval_secs>:<shell command>` to run a
shell command on a timer and show the first line of its stdout. Unknown segment names emit a warning
and are skipped. A `clock` segment enables a once-a-second repaint; without one the workbar never
wakes an idle app.

Each segment can be written either as a bare name (`"clock"`) or as a table that overrides its
badge color by theme role: `{ segment = "clock", color = "info" }`. Colors are named theme roles,
not literal values, so a badge tracks the active theme. Valid roles: `accent`, `info`, `success`,
`warning`, `error`, `neutral`, `panel` (`panel` blends into the bar, i.e. no visible pill). An
unknown role name warns and falls back to the segment's curated default. Curated defaults:
`title`/`session` = `accent`, `location`/`clock` = `info`, `activity` = `warning`, and `layout`/`text`/
`command` = `neutral`.

The `location` badge uses the same shape and powerline path as `session`: `󰒍 workbox` for an active
remote attachment and `󰒍 2` for two retained remote attachments while the current session is local.
Connecting/reconnecting uses the warning role; an offline active remote uses the error role. The
`location` and `session` badges are clickable and open the Sessions picker.

A `command` segment runs through the current resolved `command_shell` on a scheduled worker (never
the UI thread) and refreshes every `interval_secs` (default `60`, minimum `1`). Each run has a
5-second timeout and captures at most 64 KiB from each output stream. The first stdout line is
trimmed for display; a timeout, spawn error, non-zero exit, or missing output renders as blank
without a toast. The same command string shares one scheduled run even if it appears in multiple
segments. Runs reschedule themselves only while the command remains configured, so config reloads
apply command, interval, and `command_shell` changes without leaving persistent polling threads.

```toml
[workbar]
right = [
    { segment = "command:30:uptime -p", color = "success" },
    "session",
]
```

Workspaces can be given a custom name with `prefix n` (*Rename workspace*; action id
`rename-workspace`). Once set, the `workspaces` tabs show `<number>:<name>` (e.g. `1:code`) and the
`{workspace}` placeholder resolves to the name instead of the number. Names are saved with profiles
and the session autosave (`[[workspaces]] name` in the profile TOML - see
[Project profiles](project-profiles.md)).

## `[keys]`

Rebind window-management actions. Each entry maps an **action id** to one binding string or a
list of them; comma-separated alternatives also work inside one string. Each binding candidate
takes one of three forms:

- **Bare key** - a single key carrying at most `shift` (e.g. `"b"`, `"shift-w"`, `"tab"`):
  replaces the action's default key and keeps following the `[input]` scheme, so
  `copy-mode = "b"` binds `<prefix> b` plus `<modifier>-b` while `modifier_shortcuts` is on.
  This is the recommended form - change `[input]` later and these bindings follow.
- **Scheme-marked key** - `scheme:` followed by exactly one key step carrying any modifiers
  (e.g. `"scheme:ctrl-t"`): explicitly expands through the same `[input]` scheme. This covers
  modified command keys that should follow later prefix/modifier changes, producing
  `<prefix> Ctrl+T` plus `<modifier>+Ctrl+T` in this example.
- **Literal binding** - a native tui-lipan `KeyBinding` string with a real modifier or several
  chord steps (e.g. `"ctrl-a c"`, `"alt-c"`, `"ctrl-b q"`): bound verbatim, never mirrored or
  rewritten when `[input]` changes. Use this to control each side exactly, for example
  `spawn = ["ctrl-b c", "super-enter"]`, or to make one action prefix-only by listing just its
  leader chord.

Configuring an action **replaces** all of its default keys. Empty values intentionally clear an
action's defaults, for example `scratchpad = []` or `scratchpad = ""`. If every binding for an
action fails to parse, that action keeps its default keys rather than becoming unbound; invalid
bindings are warned and skipped either way. Workspace digits (`1`–`9`) are not individually
rebindable.

To keep an action's generated defaults and only add shortcuts, use an additive table. `add` accepts
one binding or a list and applies the same bare, `scheme:`, and literal rules:

```toml
[keys]
spawn = { add = "super-enter" }
copy-mode = { add = ["b", "ctrl-shift-y"] }
```

Here `spawn` retains all of its normal prefix/modifier bindings and gains literal `Super+Enter`.
`copy-mode` retains its defaults, gains scheme-generated `<prefix> b` and `<modifier>-b`, and gains
literal `Ctrl+Shift+Y`. Bindings already present in the defaults are deduplicated.
Use `scheme:` inside the same list when a modified addition should follow `[input]`, for example
`copy-mode = { add = "scheme:ctrl-t" }`.

Because a bare key always expands through the `[input]` scheme, a built-in action can never be
bound to a plain unmodified key - by design, since such a binding would steal ordinary typing
from the focused terminal. User-defined commands (below) still accept any literal trigger.

rozi disables tui-lipan's built-in global `Ctrl-q` (`App::global_quit(None)`) so it never
conflicts with app routing. Use rozi `[keys]` actions (`detach`, `quit`, …) instead; for
example `quit = "ctrl-q"` restores a direct quit shortcut through rozi routing.

The help overlay (`?`) shows real active bindings and
`not set` for bindable commands with no active key.

Examples:

```toml
[keys]
copy-mode = "b"                      # bare key: <prefix> b + <modifier>-b
toggle-pane-synchronization = { add = "ctrl-a y" }
search = "scheme:ctrl-f"             # <prefix> Ctrl+F + <modifier>+Ctrl+F
save-profile = ["ctrl-a z", "alt-z"] # literal replacement
scratchpad = []
```

The parser is tui-lipan's `KeyBinding` parser. A bare `+` works as a chord step for the plus
key (also accept the name `plus`), but `+` inside a mixed step is still a modifier separator
(`ctrl+c`).

Action ids: `spawn`, `spawn-float`, `close`, `focus-left/down/up/right`, `focus-left-no-wrap`,
`focus-down-no-wrap`, `focus-up-no-wrap`, `focus-right-no-wrap`,
`move-left/down/up/right`, `swap-left/down/up/right`, `cycle-focus-next`, `cycle-focus-prev`, `promote-to-master`,
`toggle-float`, `toggle-fullscreen`, `rename-pane`, `rename-workspace`, `paste`, `flip-split`,
`grow-split`, `shrink-split`, `resize-mode`, `toggle-layout`, `choose-layout`, `copy-mode`, `scratchpad`, `search`,
`save-profile`, `open-profile`, `sessions`, `rename-session`, `collaborators`, `request-control`, `grant-control`, `toggle-input-lock`, `toggle-control-takeover`, `detach`, `quit`, `kill-workspace`, `kill-session`, `restart-session`,
`choose-theme`, `settings`, `change-appearance`, `command-palette`, `alerts`, `toggle-do-not-disturb`,
`help`, `toggle-devtools`, `toggle-titles`, `cycle-titlebar`, `toggle-workbar`, `toggle-workbar-gap`, `toggle-workbar-position`,
`toggle-workbar-powerline`, `toggle-sidebar`, `toggle-sidebar-split`, `focus-sidebar`, `sidebar-next-tab`, `sidebar-prev-tab`,
`toggle-animations`, `toggle-focus-on-hover`,
`toggle-highlight-focused-background`, `toggle-highlight-focused-border`,
`toggle-highlight-focused-titlebar`, `cycle-border-mode`, `cycle-border-style`, `cycle-alert-border`, `cycle-workbar-alert`, `cycle-title-style`,
`cycle-workbar-badge-style`, `cycle-workbar-tab-style`, `cycle-workbar-style`,
`toggle-pane-synchronization`, `open-config`. These same ids also work with `rozi run-action <id>` over the control socket
(see `docs/control.md`).

`paste` (default `v` or direct `Ctrl+V`) reads the system clipboard and sends it to the focused
pane's PTY, wrapped in bracketed-paste markers so shells/editors that opt in treat it as one paste
instead of simulated keystrokes.

### User-defined command keybindings

Instead of an action id, a `[keys]` entry can map a **literal trigger binding** to a table
defining a new command that doesn't otherwise exist as an `Action`:

```toml
[keys]
g = { run = "lazygit", label = "Git UI" }
alt-t = { run = "btop" }
"ctrl-a e" = { send = "ls -la\n" }
i = { exec = "git branch --format='%(refname:short)' | rozi pick --title Branch | xargs -r git switch", label = "Switch branch" }
```

- `run = "<command>"` opens a new pane running that shell command (the same mechanism as the
  scratchpad's `command`), so full-screen interactive programs like `lazygit` or `btop` work.
- `send = "<text>"` writes the literal text straight to the focused pane's PTY - TOML escapes
  like `\n` work as usual, so a binding can submit a ready-to-run command.
- `popup = "<command>"` runs the command in a centered transient popup instead of a workspace pane.
- `exec = "<command>"` runs the command detached, with no pane and no popup, and discards its
  output. Use it when the whole result is a side effect - the command drives rozi over the control
  socket, or hands off to another program - because a pane there is pure cost: the layout opens and
  closes around output nobody reads. A non-zero exit still raises an error toast, so a broken
  binding is quiet rather than silent. `keep_open` does not apply.
- Exactly one of `run`/`send`/`popup`/`exec` must be set; a table with multiple values or none is
  warned about and skipped.
- `keep_open` (default `true`, `run` and `popup` only) preserves command output after exit. A `run`
  pane prints the exit status and replaces the dead PTY with a shell. A `popup` prints the status
  and retains its final screen as a read-only result; Enter, Escape, or Space dismisses it. Set
  `keep_open = false` for a program that owns the pane for its whole life and should take the pane
  down with it:

```toml
[keys]
"ctrl-a g" = { run = "lazygit", keep_open = false }
"ctrl-a b" = { run = "cargo build" }               # holds, so build errors stay on screen
```
- `label = "<text>"` names the command in the help overlay and command palette. Without it the
  label is generated from the command (`Run: lazygit`, `Send: ls -la\n`), which truncates a
  pipeline into something unreadable.
- The map key here is the trigger itself (`g`, `alt-t`, `"ctrl-a e"`, ...), parsed by the same
  rules as a binding value elsewhere in `[keys]` - it is *not* an action id, so it can't collide
  with one. A **bare key** expands through the `[input]` scheme exactly as it does for a built-in
  action, so `g = { run = ... }` answers to both `<prefix> g` and `<modifier>-g` and follows a
  later `[input]` change. A **literal** chord (`"ctrl-a e"`, `alt-t`) binds only itself, which is
  how a command is pinned to the prefix alone.
- Each command shows up in the help overlay (under "Custom") and the command palette, so its
  trigger stays discoverable even though it has no stable action id. A scheme-expanded command
  shows the bare key there, the way a built-in does. It still can't be rebound elsewhere or invoked
  via `rozi run-action` - only the trigger you configured runs it.

## `[[hooks]]`

> **Breaking change:** the former flat `[hooks]` table is no longer supported. Convert every old
> key/value pair to a structured entry with `event` and `run`. A leftover `[hooks]` table prevents
> the config from loading and produces a migration warning.

```toml
[[hooks]]
event = "pane-exited"
run = "notify-send 'pane exited'"

[[hooks]]
event = "workspace-switched"
run = "logger workspace=$ROZI_WORKSPACE"
```

Multiple entries may target the same event; each command is launched asynchronously through
`command_shell`. Hooks receive `ROZI_EVENT`, event-specific `ROZI_*` fields, and
`ROZI_SOCKET` when the client control endpoint is available. Unknown event ids and empty commands
are warned and ignored.

See [Hooks](hooks.md) for all 17 events and fields, the complete environment contract, command
lifecycle and client-side semantics, migration examples, and control-socket callbacks.

## `[[services]]`

Supervised background services started alongside rozi and restarted if they exit. Services inherit `ROZI_SOCKET` and standard environment, with stdio discarded.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `name` | string | **required** | Unique identifier for the service. |
| `run` | string | **required** | Command to execute, launched via `command_shell`. |
| `cwd` | path | launch directory | Working directory for the service (`~` expands to `$HOME`). |
| `restart` | `on-failure`, `always`, or `never` | `on-failure` | Restart policy. `on-failure` restarts on non-zero exit; `always` restarts on any exit; `never` runs once. |
| `env` | table | `{}` | Key-value environment variables passed to the child process. |

```toml
[[services]]
name = "git-watcher"
run = "cargo-watch -x check"
cwd = "~"
restart = "on-failure"
env = { RUST_LOG = "info" }
```

### Restart backoff

Services use fixed exponential backoff on failure: 1s → 2s → 4s → 8s → 16s → 30s (capped).
The backoff delay and consecutive failure counter reset after 60 seconds of continuous uptime.
If a service fails 5 consecutive times inside 60 seconds, an error toast is shown and the service goes dormant until the next config reload.
All running services are cleanly terminated on session exit or detach.

## Pane synchronization

The *Toggle pane synchronization* palette command toggles synchronized input for the active
workspace. When enabled, normal key events sent to the focused/source tiled pane are also sent to
every tiled, non-floating, non-closing pane in that workspace. Prefix/held window-management
commands still intercept first; mouse input, paste/raw non-key input, focus reports, floating panes,
and the scratchpad are not broadcast. The workspace flag is saved in profiles and session autosaves.
While it is on, a `SYNC` chip stays in the workbar - the mode is not announced and then left
unmarked, because what it changes (every keystroke reaching several panes) is worth seeing at the
moment you type, not three seconds after you enabled it.

## `[logging]`

`[logging]` configures the files written by [`toggle-pane-logging`](terminal.md#pane-logging).

| Key | Default | Meaning |
| --- | --- | --- |
| `dir` | `$XDG_STATE_HOME/rozi/logs` (else `~/.local/state/rozi/logs`) | Where session log directories are created. |
| `max_bytes` | `67108864` (64 MiB) | Size ceiling for one pane log file. `0` disables the cap. |

Session directories use mode `0700` and log files use mode `0600`.

```toml
[logging]
dir = "~/.local/state/rozi/logs"
max_bytes = 67108864
```

A pane log that reaches `max_bytes` stops, records why in its last line, and reports the stop the
same way a write error does. The limit is enforced per file, and a chunk that would cross it is
refused whole rather than half-written, so a log never ends mid-escape-sequence.

An ephemeral session's log directory is deleted when its server exits: `eph-*` sessions are
disposable and nothing can reattach to read them later. Named sessions keep their logs.
