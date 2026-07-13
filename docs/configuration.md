# Configuration

`hyprmux` reads a single TOML config file at startup. All keys are optional; anything you
omit keeps its default. A read or parse failure does **not** crash the app - it loads
defaults and reports the problem as a startup toast.

## Config file location

`hyprmux` resolves the config path in this order:

1. `$HYPRMUX_CONFIG` (a full path; `~` and `~/...` expand to `$HOME`).
2. `$XDG_CONFIG_HOME/hyprmux/hyprmux.toml`, else `~/.config/hyprmux/hyprmux.toml` — on Windows,
   `%APPDATA%\hyprmux\hyprmux.toml`.

On startup a toast reports `Loaded config from <path>` on success, or a warning if the file
could not be read or parsed.

### Where hyprmux keeps its files

Every path below is the *base* directory; profiles, themes, session snapshots, and pane logs live
under it. macOS follows the XDG convention rather than `~/Library`, matching tmux, neovim, and
alacritty. A relative `XDG_*` override is rejected outright rather than being resolved against the
working directory, which would make the config directory move every time you `cd`.

| | Linux/macOS | Windows |
| --- | --- | --- |
| Config (`hyprmux.toml`, `themes/`, `profiles/`) | `$XDG_CONFIG_HOME/hyprmux`, else `~/.config/hyprmux` | `%APPDATA%\hyprmux` |
| State (session autosave, resurrection snapshots, pane logs) | `$XDG_STATE_HOME/hyprmux`, else `~/.local/state/hyprmux` | `%LOCALAPPDATA%\hyprmux` |
| Cache (generated shell-integration scripts) | `$XDG_CACHE_HOME/hyprmux`, else `~/.cache/hyprmux` | `%LOCALAPPDATA%\hyprmux\cache` |
| Runtime (control and session endpoints) | `$XDG_RUNTIME_DIR/hyprmux`, else a private per-uid temp directory | `%LOCALAPPDATA%\hyprmux\run` |

### Live reload and editing

hyprmux watches the config file and applies every save live - config fields, `[keys]`
bindings/user commands, theme (including switching which file the theme watcher follows), and
workbar segments - without touching running panes, workspaces, or the active session. A parse
failure reloads to defaults and reports it as a toast, same as at startup; fix the file and
save again. Changes hyprmux persists itself (theme selection, appearance toggles, the default
profile) are already applied and don't trigger a reload.

The **Open config file** command-palette entry (`open-config`) opens the file in `$EDITOR`
(falling back to `$VISUAL`, then `vi`) in a new pane. It is an ordinary action, so it also
works as `hyprmux run-action open-config` over the control socket (see `docs/control.md`).

## Full example

```toml
# Shell and working directory for new panes
shell = "/bin/zsh"          # default: $SHELL chosen by the system
# shell = ["pwsh.exe", "-NoLogo"]  # argument-preserving array form; first element is the program
cwd = "~/code"              # default: the directory hyprmux was launched from
scrollback = 10000          # default: 5000 lines per pane

# Deterministic shell used to run one-off command lines: pane/popup commands, hooks, workbar
# `command:` segments, `[keys] run`, profile commands, and control-socket run requests. Unlike
# `shell`, this is never detection-based, so a config snippet using it behaves the same on every
# machine. Accepts the same bare-string or argument-preserving-array forms as `shell`.
# command_shell = ["/bin/sh", "-c"]  # default on Linux/macOS
# command_shell = ["cmd.exe", "/D", "/S", "/C"]  # default on Windows (via %COMSPEC%)

[shell_integration]
# Emit OSC 7/133 cwd and command-lifecycle metadata from recognized interactive shells.
# `auto` (default) injects bash, zsh, and fish without editing dotfiles; `off` leaves shell
# initialization entirely untouched.
mode = "auto"

[input]
modifier = "alt"             # held WM modifier: "alt" (default) or "super"
prefix = "ctrl-a"            # prefix key (default: ctrl-a)
modifier_shortcuts = true     # mirror each built-in default onto Alt+<key> (default: true)

[layout]
split_width_multiplier = 2.3  # terminal cell height / width for dwindle splits (default: 2.3)

[pane]
focus_on_hover = true         # mouse hover focuses panes (default: true)
highlight_focused_background = false  # keep focused pane bg unchanged by default
show_workbar = true           # workbar with workspace tabs and mode chips (default: true)
workbar_gap = true            # 1-line gap between workbar and panes (default: true)
workbar_at_bottom = false     # draw the workbar below the panes (default: false)
show_titles = true            # pane titlebars (default: true)
padding = 0                   # blank cells between border and terminal (default: 0)
                              # scalar = all sides; [v, h]; or [top, right, bottom, left]
title_style = "padded"        # titlebar end caps: padded|half|round|arrow (default: padded)
workbar_badge_style = "padded" # workbar badge caps: padded|round|arrow (default: padded)
workbar_powerline = true      # chain trailing badges into a powerline (default: true)
workbar_tab_style = "padded" # workspace tab caps: padded|round|arrow (default: padded)
workbar_style = "padded"      # workbar end caps: padded|half|round|arrow (default: padded)
background_follows_terminal = false  # pin surface.backdrop to the host terminal bg (default: false)

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
bell = true                  # mark unfocused panes/workspaces urgent on BEL

[navigation]
# Programs that handle their own splits: smart-focus-* forwards Ctrl-h/j/k/l to them
# instead of moving pane focus. Matched case-insensitively on the pane's foreground process.
editors = ["vim", "nvim", "vi", "view", "vimdiff", "hx", "helix", "kak", "emacs", "fzf"]

[confirm]
close_pane = false             # confirm closing a pane with a live process (default: false)
kill_workspace = true          # confirm killing all panes on a workspace (default: true)
kill_session = true            # confirm shutting down the attached session (default: true)
quit_ephemeral = true          # confirm quitting an ephemeral session that still has a live pane (default: true)
new_temporary_session = true   # confirm discarding the current ephemeral session for a fresh one (default: true)

[session]
autosave = true              # save the live layout on quit, restore it next launch (default: false)
resurrect = true             # snapshot named sessions for restart after server loss (default: true)
startup = "picker"           # "ephemeral" (default) attaches directly; "picker" shows the session picker
# path = "~/.local/state/hyprmux/session.toml"  # default location if omitted

[scratchpad]
command = "btop"             # default: the normal shell
cwd = "~"                    # default: the configured cwd
height = 0.4                 # fraction of the viewport height, 0.1–0.9 (default: 0.4)

[workbar]
left = ["title", "workspaces"]              # default
right = ["session"]                         # default
# A segment can be a table to override its badge color by theme role:
# right = [{ segment = "clock", color = "info" }, "session"]
clock_format = "%H:%M"                       # strftime, only used by a clock segment

[keys]
# A bare key replaces an action's key while following the input scheme.
copy-mode = "b"
# An additive override keeps the generated defaults and adds another binding.
spawn = { add = "super-enter" }
# A literal replacement can intentionally make one action prefix-only.
close = "ctrl-a q"
```

## Top-level keys

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `shell` | string or array | see below | Interactive shell launched in each new pane. |
| `command_shell` | string or array | see below | Shell used to run one-off command lines (pane/popup commands, hooks, workbar `command:` segments, `[keys] run`, profile commands, control-socket run requests). |
| `shell_integration.mode` | `auto` or `off` | `auto` | Inject OSC cwd/command metadata into supported interactive shells. |
| `cwd` | path | launch directory | Working directory for new panes. `~` expands to `$HOME`. |
| `scrollback` | integer | `5000` | Scrollback buffer size, in lines, per pane (minimum 1). |

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

With `mode = "auto"` (the default), hyprmux injects its shell integration into supported
**interactive** shell panes only. It never changes dotfiles, registry settings, or the
noninteractive `command_shell` runner. The integration emits OSC 7 current-directory updates and
OSC 133 prompt/command lifecycle markers; it sends only the executable basename for smart focus,
never a full command line.

| Shell | Injection mechanism | What you get |
| --- | --- | --- |
| bash | Generated `--rcfile` wrapper | Everything. Chains `/etc/bash.bashrc` and `~/.bashrc`, then the integration. Login-shell configurations are intentionally left untouched, because bash ignores `--rcfile` for login shells. |
| zsh | Temporary `ZDOTDIR` shim | Everything. Chains the original `ZDOTDIR` (or `$HOME`) `.zshenv`/`.zshrc`, then the integration. |
| fish | Temporary `XDG_DATA_DIRS` vendor `conf.d` entry | Everything. Composes with Fish event hooks; prompt frameworks loaded later can replace its final prompt marker. |
| PowerShell | `-NoExit -Command . <script>` | Everything. Runs *after* your `$PROFILE`, so your prompt (oh-my-posh, Starship, a hand-rolled `prompt` function) and PSReadLine configuration are wrapped, not replaced. A pane whose `shell` already carries `-Command`/`-File` is left alone — that is a "run this and exit" launch, not an interactive session. |
| cmd.exe | `PROMPT` environment variable | Working directory and prompt boundaries only. cmd has no pre-execution hook, and hyprmux will not touch the `AutoRun` registry key, so there is no way to report the running command or its exit status. Install [Clink](https://chrisant996.github.io/clink/) if you want the rest. |

Set `mode = "off"` if your shell already emits suitable OSC metadata or if you want hyprmux to
leave shell startup completely unchanged.

### PowerShell sessions hyprmux did not launch

The `-NoExit -Command` injection above only applies to panes hyprmux starts. For a PowerShell
reached some other way — nested inside a pane, or launched through a `command =` pane — add one
line to your `$PROFILE` instead:

```powershell
. "$env:LOCALAPPDATA\hyprmux\cache\shell-integration\hyprmux.ps1"
```

(On Linux/macOS the script lives under `~/.cache/hyprmux/shell-integration/`.) The script is
idempotent, so having both the `$PROFILE` line and the automatic injection is harmless. The
equivalent for cmd.exe is `hyprmux.cmd` in the same directory, which just sets `PROMPT`.

## `[input]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `modifier` | string | `alt` | Held WM modifier for generated direct command keys and mouse gestures; `alt`/`mod` or `super`/`meta`/`logo`/`win`. |
| `prefix` | string | `ctrl-a` | Prefix key used by generated leader chords, e.g. `ctrl-a`, `ctrl-b`. |
| `modifier_shortcuts` | bool | `true` | When true, every built-in default key is also bound as a held `<modifier>+<key>` chord (e.g. `Alt+q`) alongside its `<prefix> <key>` leader chord. Set to `false` to drop the held-modifier layer entirely and keep prefix-only bindings, so held `Alt`/`Super` chords pass through to the focused pane. |

This is an all-or-nothing switch. To drop the mirror for a single command only, override it in
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
| `split_width_multiplier` | float | `2.3` | Terminal cell height divided by cell width. Dwindle uses this to compare a focused pane's visual width and height when choosing the next split axis. Must be positive. Increase it when panes that look taller than wide split side-by-side. |

## `[pane]`

Pane focus and chrome behavior.

| Key | Default | Notes |
| --- | --- | --- |
| `focus_on_hover` | `true` | Moving the mouse over a pane focuses it. The palette toggle writes this back to config. |
| `hold_on_exit` | `false` | Keep naturally exited panes in the layout. Their title shows the exit code and the `respawn-pane` action restarts the retained command and cwd in place. `keep_open = true` launch identities normally replace an exited command with a shell, so they generally do not reach this state. |
| `highlight_focused_background` | `false` | Give the focused pane the theme panel background. When `false`, focus changes only border/titlebar chrome, not the pane background. The palette toggle writes this back to config. |
| `show_workbar` | `true` | Show the workbar (workspace tabs, mode chips, configured segments). When `false`, panes use the full viewport height with no top gap. |
| `workbar_gap` | `true` | Show a 1-line gap between the workbar and the panes area. |
| `workbar_at_bottom` | `false` | Draw the workbar on the last row (below the panes) instead of the first row. The gap, when enabled, moves to sit between the panes and the workbar. The palette/appearance toggle writes this back to config. |
| `show_titles` | `true` | Show per-pane titlebars. The palette toggle writes this back to config. |
| `padding` | `0` | Blank cells inserted between each pane's border and its terminal grid, painted with the pane's frame background. Accepts a single number (all sides), or a CSS-style array of `[vertical, horizontal]` (2 values) or `[top, right, bottom, left]` (4 values); other lengths are ignored with a warning. Purely cosmetic: each cell of padding costs a column/row of usable terminal space. Each side is clamped to `8`. The Appearance → Terminal padding editor writes the two-value `[vertical, horizontal]` form; saving there intentionally normalizes any four-side asymmetric padding. |
| `title_style` | `padded` | Titlebar end-cap style: `padded` (flush bar, blank side padding), `half` (`▐`/`▌` half-block caps), `round` or `arrow` (powerline pill/point caps). `round` and `arrow` need a patched/Nerd font, like the titlebar icons. The appearance cycle writes this back to config. |
| `workbar_badge_style` | `padded` | End-cap style for the workbar's colored badges. The `hyprmux` title chip caps on its right and the mode chips (`PREFIX`/`RESIZE`/`COPY`) cap on their left, so each pill rounds off toward the workbar's edge. Same values and font requirements as `title_style`, except `half` is not available for badges. Existing configs without `workbar_tab_style` also apply this value to workspace tabs. The appearance cycle writes this back to config. |
| `workbar_powerline` | `true` | Whether the trailing badges (mode chips + right-region badges such as `session`) chain into a powerline: the gap between them collapses and each cap blends into its left neighbor's color. Adjacent badges with the same color retain a contrasting seam (`` for arrow caps, `▏` for round and padded badges). When `false`, trailing badges keep a 1-cell gap and each cap is drawn over the panel bar. Independent of `workbar_badge_style`, which only controls the pill shape. The appearance toggle writes this back to config. |
| `workbar_tab_style` | `padded` | End-cap style for workspace tabs in the workbar. Only the active and hovered tab are capped (tabs are peers, so they do not chain). Same values and font requirements as `workbar_badge_style`. When unset, `workbar_badge_style` is used for backward-compatible appearance. The appearance cycle writes this back to config. |
| `workbar_style` | `padded` | End-cap style for the workbar itself, so the whole panel bar reads as a pill/point over the backdrop instead of a flush edge-to-edge bar. The caps replace the bar's outer side padding rather than widening it. Same values and font requirements as `title_style`. The appearance cycle writes this back to config. |
| `background_follows_terminal` | `false` | Pin `surface.backdrop` (canvas gaps, unfocused pane frames) to the host terminal's own background, overriding whatever the active theme authored - including a preset or custom theme file that sets a concrete color. See [Matching the host terminal's background](themes.md#matching-the-host-terminals-background). The appearance toggle writes this back to config. |

## `[[rules]]`

Window rules apply to command-carrying panes spawned through control `new-pane` and `[keys]`
`run` commands. Matching is a case-sensitive command substring and the first matching rule wins.
Plain shell-pane spawns, profile restoration, and scratchpads do not use rules. Rules are
command-based only; terminal titles arrive after spawn and are not matched.

```toml
[[rules]]
match = "btop"
float = true
width = 0.7
height = 0.7

[[rules]]
match = "cargo watch"
workspace = 9
focus = false
```

| Key | Default | Notes |
| --- | --- | --- |
| `match` | required | Non-empty command substring. |
| `float` | `false` | Spawn centered as a floating pane. |
| `width`, `height` | `0.6` when floating | Fractions of the pane canvas, clamped to `0.1..=1.0`. |
| `workspace` | current | 1-based target workspace (`1..=9`). |
| `focus` | `true` | Switch to and focus the spawned pane. When false, the target workspace remembers the new pane as its own focus without stealing the current view. |
| `fullscreen` | `false` | Spawn with fullscreen enabled. |

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
| `bell` | `true` | Mark an unfocused pane urgent on BEL; focusing it clears urgency. Independent of desktop notifications. |

## `[navigation]`

Controls the vim-aware `smart-focus-left` / `-down` / `-up` / `-right` actions, which power
seamless `Ctrl-h/j/k/l` navigation across both hyprmux panes and editor splits (see
[Seamless vim / neovim navigation](keybindings.md#seamless-vim--neovim-navigation) for the full
wiring). These actions are unbound by default.

When a smart-focus action runs, hyprmux checks the focused pane's **foreground process name**. If
it matches one of the `editors`, the matching `Ctrl-h/j/k/l` is forwarded to that program so it can
move its own split; otherwise hyprmux moves pane focus in that direction.

| Key | Default | Notes |
| --- | --- | --- |
| `editors` | vim family + `hx`/`helix`/`kak`/`emacs`/`emacsclient`/`fzf` | Foreground process names (matched case-insensitively) that should receive `Ctrl-h/j/k/l` themselves. Setting this **replaces** the default list. Names match the executable basename as seen by the OS (e.g. `nvim`, not a full path). |

Foreground detection prefers what the shell itself reports (see
[`[shell_integration]`](#shell_integration)), and falls back to `/proc` on Linux and `libproc` on
macOS. Windows has no fallback — process inspection is deliberately unsupported — so a Windows pane
whose shell is not reporting metadata is treated as running an unknown program, and smart-focus
simply moves pane focus.

## `[confirm]`

`[confirm]` governs **one** confirmation layer: the destructive *shortcuts* - the actions that
happen the instant you press a key, hold a modifier chord, or send a control-socket `run-action`.
Each key below toggles whether that shortcut asks first. An armed confirmation shows a red-bordered
toast and expires with it (3 seconds); the next press within that window fires, otherwise it arms
again. Running the same command from the **command palette** always skips the confirmation - picking
it from a searchable list is already a deliberate choice.

| Key | Default | Confirms before… |
| --- | --- | --- |
| `close_pane` | `false` | Closing a pane whose process is still running. |
| `kill_workspace` | `true` | Closing every pane on the active workspace. |
| `kill_session` | `true` | Shutting down the attached named session. |
| `quit_ephemeral` | `true` | Quitting while on an ephemeral session with a live pane (quitting shuts its server down and kills those PTYs). Quitting a named session, or an ephemeral one with no live pane, is unaffected. |
| `new_temporary_session` | `true` | Discarding the current ephemeral session to start a fresh one (its panes are killed). Named sessions are detached and left running, so switching from one does not require confirmation. |

**Not covered by `[confirm]`:** the session picker and the session-naming prompt carry their own
built-in confirmations that are **always on** and cannot be disabled here, because they read off the
affected UI element rather than a toast (a second `Enter`/`Ctrl+K` after a visible cue). These are:
killing a session in the picker (`Ctrl+K` twice), attaching away from an ephemeral session (`Enter`
turns the target row amber), and creating a named session from an ephemeral one (`Enter` turns the
name prompt's border red). See [sessions.md](sessions.md#switching-sessions-in-app-the-picker).

## `[session]`

Optional **local** session auto-save: persist the live layout when a local `hyprmux` client exits
and restore it on the next local launch. Like profiles, this restores *layout and launch intent*,
not live PTY state.

This is separate from named attached sessions (`hyprmux --attach <name>`), which run PTYs in a
background session server and can be detached/reattached with live terminal state intact.

| Key | Default | Notes |
| --- | --- | --- |
| `autosave` | `false` | Write the layout on quit and restore it on startup. |
| `startup` | `"ephemeral"` | Startup session behavior. `"ephemeral"` attaches directly to a fresh ephemeral session; `"picker"` opens the session picker first (equivalent to `--pick`). |
| `path` | `$XDG_STATE_HOME/hyprmux/session.toml` | Session file location (falls back to `~/.local/state/...`). |

A CLI profile or `[profile] default` takes precedence over the autosaved session at startup.

With `startup = "picker"` (or `--pick`), the session picker is shown at launch **only when at least
one named session already exists**; otherwise the launch attaches to an ephemeral session as usual.
Dismissing the picker with `Esc` attaches a fresh ephemeral session. See [Sessions](sessions.md).

When several clients attach to one session they share a live, server-authoritative layout with a
single controlling client and cooperative control requests (`request-control`, default `g`, which
asks the controller to grant rather than stealing). This needs no configuration - the request
notification debounce and client heartbeat are fixed built-in constants - see
[Shared live layouts](sessions.md#shared-live-layouts).

## `[scratchpad]`

The dropdown scratchpad (toggle: `` ` ``). The shell stays alive while hidden.

| Key | Default | Notes |
| --- | --- | --- |
| `command` | the normal shell | Program to run in the scratchpad (e.g. `btop`). |
| `cwd` | the configured `cwd` | Working directory for the scratchpad shell. |
| `height` | `0.4` | Fraction of the viewport height it opens at; clamped to `0.1`–`0.9`. |

Drag the scratchpad's top edge (its title/top-border row) up or down to resize it while it is
open. The dragged height overrides `height` for the rest of the session; it resets to `height` on
restart.

## `[workbar]`

Customize the workbar. The default reproduces the original workbar (the `hyprmux` badge and the
workspace tabs on the left, the `session` badge on the right). Every configured segment renders as
a colored badge; each kind has a curated default color that you can override by theme role (see
below). The `PREFIX`/`RESIZE`/`COPY`/`HINT` mode chips render only while `show_workbar` is enabled, and sit
to the left of the right-region segments so a `session` badge stays pinned to the trailing edge.
With `workbar_powerline` on (the default) the mode chips and right-region badges lose the gap
between them and interlock into a powerline: each chip's cap blends into its left neighbor's color.
`workbar_badge_style` controls the pill shape (rounded/pointed vs flush) independently. Workspace
tab caps are controlled separately with `workbar_tab_style`.

| Key | Default | Notes |
| --- | --- | --- |
| `left` | `["title", "workspaces"]` | Ordered left-region segments. |
| `right` | `["session"]` | Ordered right-region segments. |
| `clock_format` | `"%H:%M"` | strftime format, used by a `clock` segment. |

Segment kinds: `title` (the badge), `workspaces` (the tabs), `session` (the active profile/
session name), `clock`, `layout` (active workspace layout name), `activity` (count of panes with
unseen output), `text:<literal>` with `{host}`, `{workspace}`, `{layout}`, `{session}`
placeholders, and `command:<shell command>` / `command:<interval_secs>:<shell command>` to run a
shell command on a timer and show the first line of its stdout. Unknown segment names emit a warning
and are skipped. A `clock` segment enables a once-a-second repaint; without one the workbar never
wakes an idle app.

Each segment can be written either as a bare name (`"clock"`) or as a table that overrides its
badge color by theme role: `{ segment = "clock", color = "info" }`. Colors are named theme roles,
not literal values, so a badge tracks the active theme. Valid roles: `accent`, `info`, `success`,
`warning`, `error`, `neutral`, `panel` (`panel` blends into the bar, i.e. no visible pill). An
unknown role name warns and falls back to the segment's curated default. Curated defaults:
`title`/`session` = `accent`, `clock` = `info`, `activity` = `warning`, and `layout`/`text`/
`command` = `neutral`.

A `command` segment runs through `$SHELL -c` on a background thread (never the UI thread) and
refreshes every `interval_secs` (default `60`, minimum `1`); a failing command or one with no
output renders as blank rather than an error. The same command string reuses one poller even if
it appears in multiple segments.

```toml
[workbar]
right = [
    { segment = "command:30:uptime -p", color = "success" },
    "session",
]
```

Workspaces can be given a custom name with the *Rename workspace* command palette entry (action id
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

hyprmux disables tui-lipan's built-in global `Ctrl-q` (`App::global_quit(None)`) so it never
conflicts with app routing. Use hyprmux `[keys]` actions (`detach`, `quit`, …) instead; for
example `quit = "ctrl-q"` restores a direct quit shortcut through hyprmux routing.

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

The parser is tui-lipan's `KeyBinding` parser. Use names like `shift-=`, not a bare `+`, for
the plus shortcut because `+` is a modifier separator.

Action ids: `spawn`, `close`, `focus-left/down/up/right`, `focus-left-no-wrap`,
`focus-down-no-wrap`, `focus-up-no-wrap`, `focus-right-no-wrap`,
`swap-left/down/up/right`, `cycle-focus-next`, `cycle-focus-prev`, `promote-to-master`,
`toggle-float`, `toggle-fullscreen`, `rename-pane`, `rename-workspace`, `paste`, `flip-split`,
`grow-split`, `shrink-split`, `resize-mode`, `toggle-layout`, `copy-mode`, `scratchpad`, `search`,
`save-profile`, `open-profile`, `sessions`, `rename-session`, `request-control`, `grant-control`, `detach`, `quit`, `kill-workspace`, `kill-session`,
`choose-theme`, `command-palette`,
`help`, `toggle-titles`, `toggle-workbar`, `toggle-workbar-gap`, `toggle-workbar-position`,
`toggle-workbar-powerline`, `toggle-animations`, `toggle-focus-on-hover`,
`toggle-highlight-focused-background`, `cycle-border-style`, `cycle-title-style`,
`cycle-workbar-badge-style`, `cycle-workbar-tab-style`, `cycle-workbar-style`,
`toggle-pane-synchronization`, `open-config`. These same ids also work with `hyprmux run-action <id>` over the control socket
(see `docs/control.md`).

`paste` (default `v` or direct `Ctrl+V`) reads the system clipboard and sends it to the focused
pane's PTY, wrapped in bracketed-paste markers so shells/editors that opt in treat it as one paste
instead of simulated keystrokes.

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
- `popup = "<command>"` runs the command in a centered transient popup instead of a workspace pane.
- Exactly one of `run`/`send`/`popup` must be set; a table with multiple values or none is warned about and
  skipped.
- The map key here is the trigger itself (`prefix g`, `alt-t`, ...), parsed the same way as a
  binding value elsewhere in `[keys]` - it is *not* an action id, so it can't collide with one.
- Each command shows up in the help overlay (under "Custom") and the command palette with a
  generated label (`Run: lazygit`, `Send: ls -la\n`), so its trigger stays discoverable even
  though it has no stable action id. It still can't be rebound elsewhere or invoked via
`hyprmux run-action` - only the trigger you configured runs it.

## `[hooks]`

Hooks map control event names to shell commands. Commands run detached through `$SHELL -c` and
receive `HYPRMUX_EVENT` plus available event fields such as `HYPRMUX_PANE`, `HYPRMUX_CODE`,
`HYPRMUX_WORKSPACE`, `HYPRMUX_COMMAND`, and `HYPRMUX_CWD`.

```toml
[hooks]
pane-exited = "notify-send 'pane exited'"
workspace-switched = "logger workspace=$HYPRMUX_WORKSPACE"
```

Valid names are `pane-spawned`, `pane-exited`, `focus-changed`, and `workspace-switched`.
Unknown names are warned and ignored.

## Pane synchronization

The *Toggle pane synchronization* palette command toggles synchronized input for the active
workspace. When enabled, normal key events sent to the focused/source tiled pane are also sent to
every tiled, non-floating, non-closing pane in that workspace. Prefix/held window-management
commands still intercept first; mouse input, paste/raw non-key input, focus reports, floating panes,
and the scratchpad are not broadcast. The workspace flag is saved in profiles and session autosaves.

## `[logging]`

`[logging]` accepts an optional `dir` path. By default logs are stored below
`$XDG_STATE_HOME/hyprmux/logs` (or `~/.local/state/hyprmux/logs`). Session directories use mode
`0700` and log files use mode `0600`.

```toml
[logging]
dir = "~/.local/state/hyprmux/logs"
```
