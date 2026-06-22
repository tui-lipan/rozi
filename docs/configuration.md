# Configuration

`hyprmux` reads a single TOML config file at startup. All keys are optional; anything you
omit keeps its default. A read or parse failure does **not** crash the app — it loads
defaults and reports the problem as a startup toast.

## Config file location

`hyprmux` resolves the config path in this order:

1. `$HYPRMUX_CONFIG` (a full path; `~` and `~/...` expand to `$HOME`).
2. `$XDG_CONFIG_HOME/hyprmux/hyprmux.toml`.
3. `~/.config/hyprmux/hyprmux.toml`.

On startup a toast reports `Loaded config from <path>` on success, or a warning if the file
could not be read or parsed.

## Full example

```toml
# Shell and working directory for new panes
shell = "/bin/zsh"          # default: $SHELL chosen by the system
cwd = "~/code"              # default: the directory hyprmux was launched from
scrollback = 10000          # default: 5000 lines per pane

# Window-management input
modifier = "alt"            # held modifier: "alt" (default) or "super"
prefix = "ctrl-a"           # prefix key (default: ctrl-a)

[input]                      # alternative place for the same two keys
modifier = "alt"
prefix = "ctrl-a"

[animations]
enabled = true               # master switch (default: true)
spawn = true                 # animate new panes
close = true                 # animate closing panes
fullscreen = true            # animate fullscreen toggle
tile_float = true            # animate tiling <-> floating
axis_change = true           # animate split-axis flips
focus_chrome = true          # animate focus border/title color changes
geometry_ms = 220            # geometry transition duration (default: 220)
close_ms = 120               # close transition duration (default: 120)
focus_chrome_ms = 160        # focus-chrome transition duration (default: 160)
open_delay_ms = 36           # delay before a spawned pane fades in (default: 36)

[theme]
preset = "one-dark"         # built-in preset (default: one-dark)
path = "~/.config/hyprmux/theme.toml"  # optional custom theme file (hot-reloaded)

[profile]
path = "~/code/my-app/hyprmux-profile.toml"  # optional project profile

[clipboard]
enable_osc52 = true          # allow programs to set the system clipboard via OSC52 (default: true)

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
# Rebind any action to one or more keys. Held chords (alt-enter) or prefix
# sequences (prefix c / ctrl-a c). Configuring an action replaces its defaults.
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
| `modifier` | string | `alt` | Held WM modifier; `alt`/`mod` or `super`/`meta`/`logo`/`win`. |
| `prefix` | string | `ctrl-a` | Prefix key, e.g. `ctrl-a`, `ctrl-b`. |

`modifier` and `prefix` can also live under `[input]`; the top-level keys take precedence if
both are present.

### Prefix syntax

Prefix strings are `-` or `+` separated modifier+key combinations. Recognized modifiers:
`ctrl`/`control`, `alt`, `super`/`meta`, `shift`. Recognized named keys: `enter`/`return`,
`esc`/`escape`, `space`, `tab`, `backspace`, and the arrow keys `left`/`right`/`up`/`down`.
Any single character (e.g. `a`, `b`) is a literal key. Examples: `ctrl-a`, `ctrl-b`,
`alt-space`. An unparseable prefix is reported as a warning and the default is kept.

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
| `geometry_ms` | `220` | Geometry transition duration. |
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
| `preset` | `one-dark` | One of the built-in presets (see [Themes](themes.md)). |
| `path` | _none_ | A custom theme TOML file. When set, it is loaded at startup and **hot-reloaded** on change. |

If `path` is set and the file fails to load, `hyprmux` falls back to the `preset` theme and
reports a warning. See [Themes](themes.md) for the preset list and how terminal ANSI colors
are derived from the active theme.

## `[profile]`

| Key | Default | Notes |
| --- | --- | --- |
| `path` | _none_ | A project profile TOML file to load on startup and write back to via the *Save project profile* palette command. |

See [Project profiles & pane identity](project-profiles.md) for the profile format.

## `[clipboard]`

| Key | Default | Notes |
| --- | --- | --- |
| `enable_osc52` | `true` | Allow programs running in a pane to set the system clipboard via the OSC52 escape sequence. |

See [Terminal features](terminal.md) for clipboard and selection behavior.

## `[session]`

Optional session auto-save: persist the live layout when `hyprmux` exits and restore it on the
next launch, without any daemon. Like profiles, this restores *layout and launch intent*, not
live PTY state.

| Key | Default | Notes |
| --- | --- | --- |
| `autosave` | `false` | Write the layout on quit and restore it on startup. |
| `path` | `$XDG_STATE_HOME/hyprmux/session.toml` | Session file location (falls back to `~/.local/state/...`). |

A profile loaded via `[profile].path` takes precedence over the autosaved session at startup.

## `[scratchpad]`

The dropdown scratchpad (toggle: `` ` ``). The shell stays alive while hidden.

| Key | Default | Notes |
| --- | --- | --- |
| `command` | the normal shell | Program to run in the scratchpad (e.g. `btop`). |
| `cwd` | the configured `cwd` | Working directory for the scratchpad shell. |
| `height` | `0.4` | Fraction of the viewport height; clamped to `0.1`–`0.9`. |

## `[bar]`

Customize the top bar. The default reproduces the original bar (the `hyprmux` badge then the
workspace tabs). The `PREFIX`/`RESIZE`/`COPY` mode chips always render regardless of config.

| Key | Default | Notes |
| --- | --- | --- |
| `left` | `["title", "workspaces"]` | Ordered left-region segments. |
| `right` | `[]` | Ordered right-region segments. |
| `clock_format` | `"%H:%M"` | strftime format, used by a `clock` segment. |

Segment kinds: `title` (the badge), `workspaces` (the tabs), `session` (the active profile/
session name), `clock`, `layout` (active workspace layout name), and `text:<literal>` with
`{host}`, `{workspace}`, `{layout}`, `{session}` placeholders. Unknown segment names emit a
warning and are skipped. A `clock` segment enables a once-a-second repaint; without one the bar
never wakes an idle app.

## `[keys]`

Rebind window-management actions. Each entry maps an **action id** to one key string or a list
of them. A binding is either a **held chord** (`alt-enter`) or a **prefix sequence**
(`prefix c`, or the explicit `ctrl-a c`). Configuring an action **replaces** its default keys
(so you fully control that command); the rest keep their defaults. Unknown action ids and
unparseable keys emit a startup warning and are skipped. Workspace digits (`1`–`9`) are not
individually rebindable, and `Ctrl-q` (quit) is always hardwired. The help overlay (`?`) shows
your active bindings.

Action ids: `spawn`, `close`, `focus-left/down/up/right`, `move-left/down/up/right`,
`swap-left/down/up/right`, `cycle-focus-next`, `cycle-focus-prev`, `promote-to-master`,
`toggle-float`, `toggle-fullscreen`, `rename-pane`, `flip-split`, `grow-split`, `shrink-split`,
`resize-mode`, `toggle-layout`, `copy-mode`, `scratchpad`, `search`, `save-profile`,
`choose-theme`, `command-palette`, `help`, `toggle-titles`.
