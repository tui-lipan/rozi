# Configuration

`hyprmux` reads a single TOML config file at startup. All keys are optional; anything you
omit keeps its default. A read or parse failure does **not** crash the app - it loads
defaults and reports the problem as a startup toast.

## Config file location

`hyprmux` resolves the config path in this order:

1. `$HYPRMUX_CONFIG` (a full path; `~` and `~/...` expand to `$HOME`).
2. `$XDG_CONFIG_HOME/hyprmux/hyprmux.toml`.
3. `~/.config/hyprmux/hyprmux.toml`.

On startup a toast reports `Loaded config from <path>` on success, or a warning if the file
could not be read or parsed.

### Reloading and editing

Two command-palette entries avoid a restart after hand-editing the file:

- **Open config file** (`open-config`) opens it in `$EDITOR` (falling back to `$VISUAL`, then
  `vi`) in a new pane.
- **Reload config** (`reload-config`) re-reads the file and applies it live - config fields,
  `[keys]` bindings/user commands, theme (including switching which file the theme watcher
  follows), and bar segments - without touching running panes, workspaces, or the active
  session. A parse failure reloads to defaults and reports it as a toast, same as at startup;
  fix the file and reload again.

Both are ordinary actions, so they also work as `hyprmux run-action reload-config` /
`hyprmux run-action open-config` over the control socket (see `docs/control.md`).

## Full example

```toml
# Shell and working directory for new panes
shell = "/bin/zsh"          # default: $SHELL chosen by the system
cwd = "~/code"              # default: the directory hyprmux was launched from
scrollback = 10000          # default: 5000 lines per pane

# Window-management input
modifier = "alt"            # held WM modifier: "alt" (default) or "super"
prefix = "ctrl-a"           # prefix key (default: ctrl-a)

[input]                      # alternative place for the same two keys
modifier = "alt"
prefix = "ctrl-a"

[pane]
focus_on_hover = true         # mouse hover focuses panes (default: true)
highlight_focused_background = false  # keep focused pane bg unchanged by default
show_top_bar = true           # top bar with workspace tabs and mode chips (default: true)
show_titles = true            # pane titlebars (default: true)

[animations]
enabled = true               # master switch (default: true)
spawn = true                 # animate new panes
close = true                 # animate closing panes
fullscreen = true            # animate fullscreen toggle
tile_float = true            # animate tiling <-> floating
axis_change = true           # animate split-axis flips
focus_chrome = true          # animate focus border/title color changes
geometry_ms = 220            # base geometry transition duration (default: 220)
close_ms = 120               # close transition duration (default: 120)
focus_chrome_ms = 160        # focus-chrome transition duration (default: 160)
open_delay_ms = 36           # delay before a spawned pane fades in (default: 36)

[theme]
name = "tokyo-night"        # built-in preset, `system`, or a file in ~/.config/hyprmux/themes/

[profile]
default = "dev"              # named profile in ~/.config/hyprmux/profiles/

[clipboard]
enable_osc52 = true          # allow programs to set the system clipboard via OSC52 (default: true)

[notifications]
enabled = false              # desktop notifications are opt-in (default: false)
pane_exit = true             # notify on natural pane process exits when enabled

[session]
autosave = true              # save the live layout on quit, restore it next launch (default: false)
# path = "~/.local/state/hyprmux/session.toml"  # default location if omitted

[scratchpad]
command = "btop"             # default: the normal shell
cwd = "~"                    # default: the configured cwd
height = 0.4                 # fraction of the viewport height, 0.1–0.9 (default: 0.4)

[bar]
left = ["title", "workspaces"]   # default
right = ["session", "clock"]      # default: empty
clock_format = "%H:%M"            # strftime, only used by a clock segment

[keys]
# Rebind any action to one or more tui-lipan keybindings. Held chords
# (alt-enter) or prefix sequences (prefix c / ctrl-a c). Configuring an
# action replaces its defaults.
spawn = ["alt-enter", "prefix c"]
close = "prefix q"
copy-mode = "prefix y"
```

## Top-level keys

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `shell` | string | system `$SHELL` | Program launched in each new pane. |
| `cwd` | path | launch directory | Working directory for new panes. `~` expands to `$HOME`. |
| `scrollback` | integer | `5000` | Scrollback buffer size, in lines, per pane (minimum 1). |
| `modifier` | string | `alt` | Held WM modifier for direct command keys and mouse gestures; `alt`/`mod` or `super`/`meta`/`logo`/`win`. |
| `prefix` | string | `ctrl-a` | Prefix key, e.g. `ctrl-a`, `ctrl-b`. |

`modifier` and `prefix` can also live under `[input]`; the top-level keys take precedence if
both are present.

### Prefix syntax

Prefix strings use tui-lipan keybinding syntax. Modifiers include `ctrl`/`control`, `alt`,
`shift`, and `super`/`cmd`/`command`/`meta`/`win`. Named keys include `enter`/`return`,
`esc`/`escape`, `space`, `tab`, `backspace`, arrows, navigation keys, and function keys.
Examples: `ctrl-a`, `ctrl-b`, `alt-space`, `f12`. The prefix must be one key; an unparseable
prefix is reported as a warning and the default is kept.

## `[pane]`

Pane focus and chrome behavior.

| Key | Default | Notes |
| --- | --- | --- |
| `focus_on_hover` | `true` | Moving the mouse over a pane focuses it. The palette toggle writes this back to config. |
| `highlight_focused_background` | `false` | Give the focused pane the theme panel background. When `false`, focus changes only border/titlebar chrome, not the pane background. The palette toggle writes this back to config. |
| `show_top_bar` | `true` | Show the top bar (workspace tabs, mode chips, configured segments). When `false`, panes use the full viewport height with no top gap. |
| `show_titles` | `true` | Show per-pane titlebars. The palette toggle writes this back to config. |

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
| `focus_chrome` | `true` | Border/titlebar color transitions when focus moves. |
| `geometry_ms` | `220` | Base geometry transition duration; scratchpad slide uses two-thirds of this. |
| `close_ms` | `120` | Close transition duration. |
| `focus_chrome_ms` | `160` | Focus-chrome transition duration. |
| `open_delay_ms` | `36` | Delay before a spawned pane begins fading in. |

> **Size changes are snapped, not animated.** During an active move/resize or a viewport
> change, transitions become instant for the affected pane. This is deliberate: animating a
> pane's *size* would spam `pty.resize` / SIGWINCH and reflow the shell on every frame.
> Position and opacity animate; size lands in one step.

## `[theme]`

| Key | Default | Notes |
| --- | --- | --- |
| `name` | `lipan` | The active theme: a built-in preset id, `system` (host-derived colors), or the stem of a file in `~/.config/hyprmux/themes/`. A custom file shadows a built-in of the same name. |

Custom themes are **hot-reloaded** on change while active. If the name matches nothing, or a
custom file fails to load, `hyprmux` falls back to `lipan` and reports a warning. See
[Themes](themes.md) for the preset list and how terminal ANSI colors are derived from the
active theme.

## `[profile]`

| Key | Default | Notes |
| --- | --- | --- |
| `default` | _none_ | Name of a profile in `~/.config/hyprmux/profiles/` to load on startup (unless overridden by a CLI profile). Also writable via **Ctrl+f** in the **Profiles** picker. |

See [Named profiles](profiles.md) and [Project profiles & pane identity](project-profiles.md) for the profile format.

## `[clipboard]`

| Key | Default | Notes |
| --- | --- | --- |
| `enable_osc52` | `true` | Allow programs running in a pane to set the system clipboard via the OSC52 escape sequence. |

See [Terminal features](terminal.md) for clipboard and selection behavior.

## `[notifications]`

Desktop notifications are disabled by default. When enabled, hyprmux currently sends only natural
pane-exit notifications (not user-initiated pane closes) via `notify-send` if it is available.
Failures are ignored and never block the UI.

| Key | Default | Notes |
| --- | --- | --- |
| `enabled` | `false` | Master switch for desktop notifications. |
| `pane_exit` | `true` | Notify when a pane's process exits naturally. |

## `[session]`

Optional session auto-save: persist the live layout when `hyprmux` exits and restore it on the
next launch, without any daemon. Like profiles, this restores *layout and launch intent*, not
live PTY state.

| Key | Default | Notes |
| --- | --- | --- |
| `autosave` | `false` | Write the layout on quit and restore it on startup. |
| `path` | `$XDG_STATE_HOME/hyprmux/session.toml` | Session file location (falls back to `~/.local/state/...`). |

A CLI profile or `[profile] default` takes precedence over the autosaved session at startup.

## `[scratchpad]`

The dropdown scratchpad (toggle: `` ` ``). The shell stays alive while hidden.

| Key | Default | Notes |
| --- | --- | --- |
| `command` | the normal shell | Program to run in the scratchpad (e.g. `btop`). |
| `cwd` | the configured `cwd` | Working directory for the scratchpad shell. |
| `height` | `0.4` | Fraction of the viewport height; clamped to `0.1`–`0.9`. |

## `[bar]`

Customize the top bar. The default reproduces the original bar (the `hyprmux` badge then the
workspace tabs). The `PREFIX`/`RESIZE`/`COPY` mode chips render only while `show_top_bar` is
enabled.

| Key | Default | Notes |
| --- | --- | --- |
| `left` | `["title", "workspaces"]` | Ordered left-region segments. |
| `right` | `[]` | Ordered right-region segments. |
| `clock_format` | `"%H:%M"` | strftime format, used by a `clock` segment. |

Segment kinds: `title` (the badge), `workspaces` (the tabs), `session` (the active profile/
session name), `clock`, `layout` (active workspace layout name), `text:<literal>` with
`{host}`, `{workspace}`, `{layout}`, `{session}` placeholders, and `command:<shell command>` /
`command:<interval_secs>:<shell command>` to run a shell command on a timer and show the first
line of its stdout. Unknown segment names emit a warning and are skipped. A `clock` segment
enables a once-a-second repaint; without one the bar never wakes an idle app.

A `command` segment runs through `$SHELL -c` on a background thread (never the UI thread) and
refreshes every `interval_secs` (default `60`, minimum `1`); a failing command or one with no
output renders as blank rather than an error. The same command string reuses one poller even if
it appears in multiple segments.

```toml
[bar]
right = ["command:30:uptime -p", "session"]
```

Workspaces can be given a custom name with the *Rename workspace* command palette entry (action id
`rename-workspace`). Once set, the `workspaces` tabs show `<number>:<name>` (e.g. `1:code`) and the
`{workspace}` placeholder resolves to the name instead of the number. Names are saved with profiles
and the session autosave (`[[workspaces]] name` in the profile TOML - see
[Project profiles](project-profiles.md)).

## `[keys]`

Rebind window-management actions. Each entry maps an **action id** to one tui-lipan keybinding
string or a list of them. Comma-separated alternatives also work inside one string. A binding is
either an exact **held chord** (`alt-enter`) or a **prefix sequence** (`prefix c`, or the explicit
`ctrl-a c`). Prefix bindings also define the held-modifier direct path: `prefix c` can be run as
`modifier+c`. Configuring an action **replaces** its default keys. Empty values intentionally clear
an action's defaults, for example `scratchpad = []` or `scratchpad = ""`. Invalid non-empty
replacements are warned and skipped. Workspace digits (`1`–`9`) are not individually rebindable,
and `Ctrl-q` (quit) is always hardwired. The help overlay (`?`) shows real active bindings and
`not set` for bindable commands with no active key.

Examples:

```toml
[keys]
toggle-pane-synchronization = "prefix s"
save-profile = "prefix S"
scratchpad = []
```

The parser is tui-lipan's `KeyBinding` parser. Use names like `shift-=`, not a bare `+`, for
the plus shortcut because `+` is a modifier separator.

Action ids: `spawn`, `close`, `focus-left/down/up/right`, `move-left/down/up/right`,
`swap-left/down/up/right`, `cycle-focus-next`, `cycle-focus-prev`, `promote-to-master`,
`toggle-float`, `toggle-fullscreen`, `rename-pane`, `rename-workspace`, `paste`, `flip-split`,
`grow-split`, `shrink-split`, `resize-mode`, `toggle-layout`, `copy-mode`, `scratchpad`, `search`,
`save-profile`, `open-profile`, `sessions`, `detach-session`, `choose-theme`, `command-palette`,
`help`, `toggle-titles`, `toggle-top-bar`, `toggle-focus-on-hover`,
`toggle-highlight-focused-background`, `toggle-pane-synchronization`, `reload-config`,
`open-config`. These same ids also work with `hyprmux run-action <id>` over the control socket
(see `docs/control.md`).

`paste` (default `v`) reads the system clipboard and sends it to the focused pane's PTY, wrapped
in bracketed-paste markers so shells/editors that opt in treat it as one paste instead of
simulated keystrokes.

### User-defined command keybindings

Instead of an action id, a `[keys]` entry can map a **literal trigger binding** to a table
defining a new command that doesn't otherwise exist as an `Action`:

```toml
[keys]
"prefix g" = { run = "lazygit" }
alt-t = { run = "btop" }
"prefix e" = { send = "ls -la\n" }
```

- `run = "<command>"` opens a new pane running that shell command (the same mechanism as the
  scratchpad's `command`), so full-screen interactive programs like `lazygit` or `btop` work.
- `send = "<text>"` writes the literal text straight to the focused pane's PTY - TOML escapes
  like `\n` work as usual, so a binding can submit a ready-to-run command.
- Exactly one of `run`/`send` must be set; a table with both or neither is warned about and
  skipped.
- The map key here is the trigger itself (`prefix g`, `alt-t`, ...), parsed the same way as a
  binding value elsewhere in `[keys]` - it is *not* an action id, so it can't collide with one.
- Each command shows up in the help overlay (under "Custom") and the command palette with a
  generated label (`Run: lazygit`, `Send: ls -la\n`), so its trigger stays discoverable even
  though it has no stable action id. It still can't be rebound elsewhere or invoked via
  `hyprmux run-action` - only the trigger you configured runs it.

## Pane synchronization

The *Toggle pane synchronization* palette command toggles synchronized input for the active
workspace. When enabled, normal key events sent to the focused/source tiled pane are also sent to
every tiled, non-floating, non-closing pane in that workspace. Prefix/held window-management
commands still intercept first; mouse input, paste/raw non-key input, focus reports, floating panes,
and the scratchpad are not broadcast. The workspace flag is saved in profiles and session autosaves.
