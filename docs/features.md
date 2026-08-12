# Feature overview

A single-page inventory of everything `rozi` does. Each section links to the reference doc that
explains the feature in depth; this page exists to answer "what is in here?" without reading
sixteen documents.

`rozi` is a Hyprland-style tiling **terminal multiplexer** built on the
[`tui-lipan`](https://crates.io/crates/tui-lipan) TUI framework. Panes are live PTY shells arranged
by a window manager: tiling layouts, floating windows, workspaces, animated geometry, and
tmux-style prefix commands. It builds natively on Linux, macOS, and Windows.

**Contents**

- [Architecture](#architecture)
- [Layouts and tiling](#layouts-and-tiling)
- [Panes](#panes)
- [Workspaces](#workspaces)
- [Sessions](#sessions)
- [Multi-client collaboration](#multi-client-collaboration)
- [Remote sessions over SSH](#remote-sessions-over-ssh)
- [Profiles](#profiles)
- [Terminal features](#terminal-features)
- [Scrollback, search, and copy](#scrollback-search-and-copy)
- [Input model](#input-model)
- [Overlays and pickers](#overlays-and-pickers)
- [Sidebar](#sidebar)
- [Workbar](#workbar)
- [Appearance and themes](#appearance-and-themes)
- [Configuration](#configuration)
- [Automation and extensibility](#automation-and-extensibility)
- [Platform support](#platform-support)
- [Installation and releases](#installation-and-releases)
- [Performance](#performance)

## Architecture

rozi is **always-server**. A background session server owns every PTY; the UI is always a
client that attaches to it and parses raw pane output into its own `TerminalScreen`. This is not an
optional mode — even the disposable scratch session a bare launch can fall into (`eph-<pid>`) is a
real server-backed session, and a client attached to nothing is a normal state.

```text
CLI / thin main.rs
  |
  v
lib.rs -> app.rs: AppRoot (tui-lipan Component)
  |-- State / Msg model in state/ + msg.rs
  |-- update/::handle_msg dispatches messages to focused ops modules
  |-- key_routing routes prefix / held-modifier / terminal keys
  |-- actions dispatches Action values
  |-- view renders Canvas, panes, workbar, and overlays
  |
  +--> Pane -> TerminalPane -> client TerminalScreen (parses raw bytes)
  |
  +--> session/client <-> session/server <-> server-owned PTYs
```

The app is Elm-style: one root `Component`, a central `State`, and `Msg` updates. `tui-lipan`
supplies runtime primitives (canvas, frames, transitions, mouse regions, overlays, terminal
widgets); rozi owns all window-manager policy.

Consequences worth knowing:

- **Leaving preserves named sessions.** Reattach later and PTY state is intact.
- **Temporary sessions are dispositioned on leave:** untouched ones close; worked-in ones ask
  whether to be named and kept or closed.
- **Several clients can attach to one session** and share a revisioned layout document.
- **Profiles restore layout and launch intent only.** A live session preserves actual PTY state;
  a profile starts fresh shells.

See [AGENTS.md](../AGENTS.md) for the full module map.

## Layouts and tiling

Seven layouts, selected per workspace:

| Layout | Behavior |
| --- | --- |
| `dwindle` | New panes split the **focused** tile along its aspect ratio (default). |
| `master` | One master pane plus a stack. |
| `grid` | Even grid. |
| `columns` | Equal-width columns. |
| `rows` | Equal-height rows. |
| `scrollable` | Fixed-width columns that scroll horizontally past the viewport. |
| `monocle` | One pane fills the workspace; the rest are hidden but navigable. |

- **Split direction follows the focused tile's aspect ratio**, scaled by
  `layout.split_width_multiplier` — the same rule Hyprland's dwindle uses.
- **Layout picker** (`shift+m`) with live preview, plus a `toggle-layout` cycle and a configurable
  per-workspace default.
- **Resize** by keyboard (`grow-split` / `shrink-split`, or a dedicated resize mode) or by dragging
  split seams with the mouse. Splits resize in whole cells.
- **Flip split axis** on the focused pair.
- **Gaps, borders, and border merging** are configurable; merged seams keep their titlebars.

See [Layouts & panes](layouts-and-panes.md).

## Panes

Every pane is a real PTY.

**Lifecycle** — spawn, close, respawn an exited pane, and kill a whole workspace. Exit status is
surfaced rather than silently closing.

**Placement** — toggle any pane to **floating** or **fullscreen**. Floats are moved and resized
with the mouse and remember their geometry.

**Movement** — two distinct operations:

- `move-pane-<dir>` lifts the pane out of the tree and re-inserts it beside the neighbor,
  reshaping the layout.
- `swap-pane-<dir>` trades slots with the neighbor, keeping the layout's shape.
- `promote-to-master` moves the focused pane into the master slot.

**Focus** — directional (`focus-<dir>`), directional-without-wrap, cycle next/previous, and
`focus-next-blocked-pane` to jump to a pane waiting on input (reported and screen-detected blocked
states agree). Unfocused blocked and finished-unseen agents can mark pane borders and workspace tabs;
`cycle-alert-border` and `cycle-workbar-alert` each select off/static/pulse in **Settings**.
**Focus-on-hover** is a Settings preference. Focus survives layout changes.

**Smart focus (vim-aware)** — `smart-focus-<dir>` moves rozi focus *unless* the focused pane
runs a split-aware program listed in `[navigation] editors`, in which case it forwards the matching
`Ctrl-h/j/k/l` to that program. One binding navigates both rozi panes and vim/neovim splits,
vim-tmux-navigator style. A companion [Vim/Neovim plugin](../integrations/vim-rozi-navigator/)
handles the editor side.

**Identity** — rename a pane (`Shift+N`); titles otherwise follow the program's OSC title. Pane identity
carries a per-spawn `env` map that is never persisted.

**Pane synchronization** — broadcast typed input to every pane in the workspace. The state persists
into profiles.

**Pane logging** — `toggle-pane-logging` appends the pane's **raw PTY output** to a log file under
the state directory (`[logging] dir` overrides the location). Active logging shows a `[log]`
titlebar badge and is shared with every attached client, including clients that attach after
logging starts. Logging stops automatically on a write error.

> **Credentials caveat:** raw logs contain escape sequences and anything typed or printed,
> including secrets. Files are created `0600` inside a `0700` directory. View with `less -R` and
> treat them as sensitive data.

**Rules** — `[[rules]]` applies first-match placement to interactive command-carrying spawns
(including control-socket `new-pane` and `[keys] run`). A rule matches the command by substring or
regex and can set `float`, `width`, `height`, `workspace`, `focus`, and `fullscreen`.

**Scratchpad** — a toggleable drop-down pane that overlays the current workspace.

**Popups and hints** — `[[hints]]` defines contextual popup actions; hint mode overlays selectable
targets.

See [Layouts & panes](layouts-and-panes.md) and [Terminal features](terminal.md).

## Workspaces

- **Nine workspaces** with a workbar tab strip showing live pane counts.
- `switch-workspace-<n>` and `move-to-workspace-<n>`.
- `relocate-workspace-<n>` (`Ctrl+Shift+1`–`9`) moves *every* pane and the workspace name into the
  target slot, then switches there.
- **Named workspaces** — rename a workspace (`n`) to display `<number>:<name>` in the tabs. The name is
  usable in the `{workspace}` workbar placeholder and is saved with profiles and session autosave.
- **Kill workspace** closes every pane in it.
- Layout, gaps, and pane-synchronization state are per workspace.

## Sessions

Sessions are server-backed and survive client detach.

| Kind | Created by | Lifetime |
| --- | --- | --- |
| Ephemeral | a launch that settles on no named session: `startup = "ephemeral"`, nothing to pick, or a shell started from the launcher | `eph-<pid>`; shut down on a clean quit, which asks first when it holds work |
| Named | `rozi new <name>`, `rozi <name>`, `startup = "last"` / `"profile"`, picker `Ctrl+N`, or renaming a live session | Survives detach; explicit shutdown |
| Temporary | `new-temporary-session` action | In-session scratch session |

Which of these a bare `rozi` lands on is startup policy's decision (below).

**Target resolution** — `rozi <name>` (or `--session <name>`) attaches to session `<name>` or
launches its canonical same-name profile. Unknown targets do **not** silently create a session;
use `rozi new <name>` for that. `rozi attach <name>` is attach-only.

**Actions** — attach, detach (`prefix d`), rename (`prefix Shift+S`), kill, and **restart** (shut the server down and
immediately recreate it while staying attached).

**Session picker** — a fuzzy modal listing local sessions, attached sessions, configured remote
hosts, and cached remote sessions, with `ctrl+t` to reach a scratch ephemeral shell.

**Startup policy** — `[session] startup` decides what a bare launch does: `picker` (default, attaches
nothing until you choose), `ephemeral`, `last` (the most recent named session), or `profile` (the
session named after `[profile] default`). Explicit targets, `--pick`, and `--remote` take precedence.
Settable in **Settings → Startup**, alongside the default profile it depends on.

**Autosave** — `[session] autosave` persists layout locally and restores it on next launch.

**Resurrect** — `[session] resurrect` snapshots named sessions so layout, commands, and scrollback
survive a *server* restart, not just a client detach. Snapshots are written off the server loop and
reuse unchanged panes' replay files.

**Read-only attach** — `rozi attach <name> --read-only` joins as a viewer.

See [Sessions](sessions.md).

## Multi-client collaboration

The server is **layout-authoritative**. Multiple clients attach to one session and share a
revisioned `SharedLayout` document (wire protocol negotiated in a supported range).

- **One controller** holds the layout-control lease and commits layout changes.
- **Followers** reconcile via `apply_shared_layout` without touching live screens, and letterbox to
  the controller's canonical PTY size.
- **`take-control` (`prefix g`)** claims the lease instantly.
- **Request / grant / decline control** for a negotiated handoff, plus
  `toggle-immediate-control-takeover` to allow instant seizure.
- **Collaborators dialog** shows your identity and the controller as context, the other clients as
  a selectable roster, and a type-to-filter query. `Enter` grants control, `ctrl+d` declines,
  `ctrl+k` twice evicts a client.
- **Input lock** (`toggle-input-lock`) blocks input from a client.

**Local view state is never shared**: focus, active workspace, overlays, copy/search state,
scrollback position, and theme are per client.

## Remote sessions over SSH

`--remote <HOST|ssh://URL>` attaches to a session on another machine over SSH, via a remote-side
`--remote-serve` stdio proxy.

- **Bootstrap and install** — rozi can install or update its own binary on the remote host.
  `ROZI_REMOTE_BINARY` forces which local binary is shipped.
- **Configured hosts** — `[remote]` / `[[remote.host]]` entries appear directly in the session
  picker, with cached session lists for hosts that are currently unreachable.
- **`ROZI_REMOTE_HOST`** is injected into hooks while attached remotely.
- The workbar `location` badge shows the active remote (`󰒍 workbox`) or a count of retained remote
  attachments, colored by connection state.
- `rozi list-sessions --remote <host>` and `rozi kill-session <name> --remote <host>` work
  without attaching.

See [Remote SSH sessions](remote.md) for the local-vs-remote feature split.

## Profiles

Profiles are named, reusable launch recipes stored in `~/.config/rozi/profiles/`.

- **Capture** the current session as a profile (`save-profile`).
- **Launch** the canonical same-name session, or open a profile under another name.
- **Apply** a profile to replace the current session's contents.
- **Profile picker** with fuzzy search.
- `[profile] default` seeds every session opened without a recipe, not just the startup one, and
  with `[session] startup = "profile"` also names the session a bare launch opens.
- Profiles store the layout tree, pane commands and identities, workspace names, layout kinds, and
  pane-synchronization state, via the serde-stable tree shared with session layout documents.

Profiles restore *launch intent*, not live process state — use a named session for that.

See [Named profiles](profiles.md) and [Project profiles & pane identity](project-profiles.md).

## Terminal features

Provided by `tui-lipan`'s terminal primitives and wired into rozi panes:

- True color, and **terminal images** (`terminal-images`).
- **Mouse reporting** forwarded to programs that request it.
- **Text selection** with the mouse.
- **Clipboard** — paste (`v`), copy from selection and copy mode, image clipboard support
  (`clipboard-images`), and **OSC 52** clipboard, gated by `[clipboard].enable_osc52`.
- **Scroll-wheel scrollback**, with a configurable `scrollback` line budget.
- **Shell integration** — per-shell injection emitting OSC 133 prompt/command boundary markers and
  OSC 7 cwd. It emits only an executable basename, never a command line, and never modifies a
  dotfile, `$PROFILE`, or the `AutoRun` registry key.
- **`copy-last-output`** uses those markers to yank the previous command's output.
- **Terminal ANSI palette** is derived from the active rozi theme.
- **Desktop notifications** via `[notifications]`.

See [Terminal features](terminal.md).

## Scrollback, search, and copy

- **Scrollback search** (`/`) — search a pane's scrollback and jump between matches. Search runs in
  bounded cooperative slices and restarts on live pane output, so a broad search never blocks the
  UI.
- **Copy mode** — vi-style scrollback review with `hjkl`, word/WORD motions (`w`/`b`/`e`), line
  motions (`0`/`^`/`$`), selection, and clipboard yank. The motions reuse `tui-lipan`'s vim-mode
  `TextArea` algorithms rather than a separate implementation.
- **Edit scrollback** — open the pane's scrollback in an editor.
- **`capture-pane`** over the control socket dumps a pane's contents for external tooling.

## Input model

Three routing paths, resolved in `key_routing.rs`:

1. **Prefix mode** — a tmux-style `Ctrl-a` prefix that always works.
2. **Held modifier** — an `Alt`/`Super` direct path for active command keys, no prefix needed.
3. **Terminal passthrough** — everything else goes to the focused PTY.

- `input.rs` is the single source of truth for command/action metadata; the help overlay and
  command palette are generated from it, so they cannot drift from the real bindings.
- **`[keys]`** rebinds any built-in action by command id, unbinds it with an empty list, or defines
  entirely new **user commands** keyed by a literal trigger, using `run` (open a pane) or `send`
  (send text) tables.
- **Mouse gestures** — click to focus, drag split seams, drag floats, drag workbar tabs, and
  prefix-modified mouse gestures.
- **Confirmations** — `[confirm]` controls which destructive actions prompt; the command palette
  can bypass confirmation deliberately.

See [Keybindings](keybindings.md).

## Overlays and pickers

- **Command palette** (`p`) — fuzzy search over every available command, with live availability
  filtering and dynamic labels for toggles.
- **Help overlay** (`?`) — the full keybinding reference, generated from `input.rs`.
- **Layout picker** (`shift+m`) with live preview.
- **Theme picker** — built-in presets, custom themes, and `system`, in one fuzzy modal.
- **Settings dialog** — fuzzy-search durable appearance, focus, alert, notification, sound, startup,
  and session-persistence preferences; step values with left/right arrows.
- **Session picker**, **profile picker**, and **collaborators dialog**.
- **Launcher** — a startup surface when there is no obvious session to attach to.
- **DevTools overlay** — `tui-lipan`'s built-in inspector (`devtools` feature).
- **Toasts** — reserved for failures, rejections, destructive confirmations, and useful off-screen
  results. Successful state changes already visible on screen are never toasted.

Overlays present **structured data, not prose**: rows, badges, and chrome labels rather than
explanatory sentences.

## Sidebar

A dockable panel with tabs, toggled with `toggle-sidebar` and entered with `focus-sidebar` (it sits
outside the Tab focus ring). It can be docked to either side and split.

Built-in tabs:

| Tab | Contents |
| --- | --- |
| `agents` | Detected coding-agent processes running in panes |
| `panes` | Live pane list for the session, grouped by workspace |
| `sessions` | Local, attached, and remote sessions |
| `files` | File tree rooted at the focused pane's working directory |
| `git` | Repository-rooted tree of changed paths, with status markers and diff stats |

`files` and `git` are two projections of one lazy-loading tree and accept `root`, `show_hidden`,
`icons`, `explorer`, `diff_stats`, and `max_entries` in table form. Both re-root as focus moves,
mount only while visible, and read directories and `git status` off the UI thread.

User-defined tabs:

- **Launcher tabs** — a named list of entries that spawn panes or run commands.
- **Command tabs** — run a shell command on an interval and render its output, with an optional
  `on_click` action.

Activating a file runs the tab's `on_click`, which defaults to typing the path at the focused
pane's prompt without a newline. A configured `run`/`popup` action receives the path as the
`ROZI_FILE` environment variable instead, so a filename is never spliced into a command string.
Tabs support keyboard navigation, drag-and-drop reordering, scrolling, and tree guides.

See [Sidebar](sidebar.md).

## Workbar

A status bar with left and right segment regions. Each segment renders as a themed badge.

**Built-in segments:** `title`, `workspaces`, `location`, `session`, `clock`, `activity`, `layout`,
plus `text` placeholders and `command:<interval>:<cmdline>` segments.

- Segments are written as a bare name (`"clock"`) or a table overriding badge color by **theme
  role** — `accent`, `info`, `success`, `warning`, `error`, `neutral`, `panel` — so a badge tracks
  the active theme rather than a literal color.
- **`command` segments** run through the resolved `command_shell` on a scheduled worker, never the
  UI thread: 5-second timeout, 64 KiB capture cap per stream, first stdout line displayed, failures
  render blank without a toast. Identical command strings share one scheduled run, and runs
  reschedule only while still configured, so reloads never leak polling threads.
- **Styling** — `workbar_badge_style`, `workbar_tab_style`, and `workbar_style` each cycle through
  `padded` / `round` / `arrow` (plus `half` for the bar caps), and `workbar_powerline` chains
  trailing badges.
- Position (top/bottom), gap, and visibility are all toggleable at runtime.
- Workspace tabs and the `location`/`session` badges are clickable.

## Appearance and themes

**Themes** — 29 built-in presets, a host-derived `system` theme, an `ansi` fallback, and drop-in
custom theme files in `~/.config/rozi/themes/` that can `extends` another theme. Themes
**hot-reload** on file change (`theme-reload`), and the terminal ANSI palette is derived from the
active theme.

Presets: `lipan` (default), `one-dark`, `dracula`, `nord`, `gruvbox-dark`, `gruvbox-light`,
`catppuccin-mocha`, `catppuccin-latte`, `catppuccin-frappe`, `catppuccin-macchiato`,
`tokyo-night`, `tokyo-night-day`, `solarized-dark`, `solarized-light`, `monokai`, `rose-pine`,
`rose-pine-moon`, `rose-pine-dawn`, `kanagawa`, `everforest`, `ayu-dark`, `ayu-mirage`,
`ayu-light`, `nightfox`, `nordfox`, `night-owl`, `material-palenight`, `oxocarbon`, `zenburn`.

**Appearance preferences** — titlebar on/off and layout style, titlebar cap style, border mode
and style, focused-pane background / border / titlebar highlighting, background-follows-terminal,
alert-border mode,
animations on/off, and every workbar style knob listed above. Changes apply live from **Settings**;
their stable action ids remain available for `[keys]` and `run-action`.

**Animations** — spawn, close, fullscreen, tile/float, and split-axis transitions, each
individually configurable. Geometry animation is app-driven: position and opacity animate, but
terminal **size** changes snap, to avoid repeated `pty.resize` / SIGWINCH reflow.

See [Themes](themes.md).

## Configuration

One TOML file: `$ROZI_CONFIG` or `~/.config/rozi/config.toml`. It **live-reloads** on change.

Top-level tables:

| Table | Purpose |
| --- | --- |
| `shell`, `command_shell`, `cwd`, `scrollback` | Process and buffer basics |
| `[shell_integration]` | Per-shell OSC 133 / OSC 7 injection |
| `[input]` | Prefix key, held modifier, timeouts |
| `[animations]` | Per-transition animation settings |
| `[theme]` | Theme selection and overrides |
| `[profile]` | Default profile |
| `[session]` | Autosave, resurrect, lifecycle |
| `[remote]`, `[[remote.host]]` | SSH hosts and bootstrap behavior |
| `[layout]` | Default layout, `split_width_multiplier` |
| `[pane]` | Borders, titlebars, gaps, workbar powerline |
| `[clipboard]` | OSC 52 and clipboard behavior |
| `[notifications]` | Desktop notifications |
| `[navigation]` | `editors` list for vim-aware smart focus |
| `[confirm]` | Which destructive actions prompt |
| `[scratchpad]` | Scratchpad geometry and command |
| `[sidebar]` | Tabs, dock side, width, tree options |
| `[[rules]]` | Command-matched pane placement |
| `[[hints]]` | Contextual popup actions |
| `[[hooks]]` | Event-triggered commands |
| `[logging]` | Pane-log directory |
| `[workbar]` | Segments, styles, position |
| `[keys]` | Rebinds and user-defined commands |

Config normalization is **lossless and silent** — it does not toast. Unknown keys warn rather than
failing the load.

See [Configuration](configuration.md) for the complete reference.

## Automation and extensibility

**Control socket** — a per-user, per-run private endpoint (`control-<pid>.sock`, or a named pipe on
Windows) with a JSON protocol and a CLI front end.

| Command | Purpose |
| --- | --- |
| `list-panes` | Enumerate panes with identity and status |
| `metrics` | Runtime resource counters |
| `focus <id>` | Focus a pane |
| `send-text` | Send literal text to a pane |
| `send-keys` | Send key names or chords (`C-c`, `Enter`), with `-l` for literal |
| `new-pane` / `split` | Spawn a pane, honoring `[[rules]]` |
| `popup` | Open a transient popup pane |
| `run-action <id>` | Invoke any bindable action by command id |
| `capture-pane` | Dump contents — `--scrollback full\|<n>`, `--last-output`, `--target` |
| `switch-workspace` / `move-to-workspace` | Workspace control |
| `pane-logging` | Toggle raw PTY logging for a pane |
| `set-status` | Set or clear a pane status badge (`status blocked --reason ...`) |
| `agent-slots` | **Publish the agents running inside one pane**, one sidebar row each |
| `subscribe` | **Stream UI events** to an external process |

`subscribe` is the push counterpart to `[[hooks]]`: instead of spawning a command per event, a
long-lived client receives the event stream over the socket.

`ROZI_SOCKET` points the CLI at a live UI control socket.

**Injected pane environment** — `ROZI=1`, `ROZI_PANE`, and `ROZI_SOCKET` are set in every
spawned pane. `PaneIdentity::env` adds never-persisted per-spawn variables.

**Hooks** — `[[hooks]]` runs client-side commands for 17 UI events, injecting `ROZI_EVENT`, the
event's fields, `ROZI_SOCKET`, and `ROZI_REMOTE_HOST` when remote:

`pane-spawned`, `pane-exited`, `pane-status-changed`, `bell`, `focus-changed`, `workspace-switched`,
`session-attached`, `session-detached`, `session-renamed`, `session-created`,
`controller-changed`, `client-joined`, `client-left`, `profile-loaded`, `profile-applied`,
`profile-saved`, `config-reloaded`.

**Runtime metrics** — `rozi metrics` exposes resource counters for monitoring.

**Editor integration** — the [Vim/Neovim navigator](../integrations/vim-rozi-navigator/) plugin
pairs with `smart-focus-<dir>`.

See [Control socket](control.md) and [Hooks](hooks.md).

## Platform support

Linux, macOS, and Windows are all built natively in CI. All OS-specific behavior lives behind
`src/platform/`; nothing above that layer touches `std::os::unix`, `/proc`, Win32, XDG/AppData
variables, Unix permission bits, or named-pipe APIs directly.

| Submodule | Responsibility |
| --- | --- |
| `paths` | Config/state/cache/runtime directories, reported-cwd normalization |
| `fs_security` | Private directories: Unix mode/ownership, Windows SID DACL |
| `user` | uid/SID, `USER` vs `USERNAME`, hostname |
| `command` | Shell and command-runner resolution, Windows `PATH`/`PATHEXT` lookup |
| `ipc/*` | Unix sockets / Windows named pipes behind one `IpcEndpoint` API |
| `server_lifecycle` | Detached spawn, hangup and console-control handling, protocol-first shutdown, Job Object containment, ConPTY availability |
| `shell_integration` | Per-shell injection |
| `process/*` | `ProcessInspector` — Linux/macOS only by design |
| `notifications` | Desktop notifications |

**Security properties worth preserving when editing this layer:**

- Runtime-dir safety checks: Unix ownership/mode/symlink validation; Windows reparse-point
  rejection and a protected current-user-SID DACL.
- `PIPE_REJECT_REMOTE_CLIENTS` (blocks remote reachability) and `FILE_FLAG_FIRST_PIPE_INSTANCE`
  (blocks name squatting) on the Windows backend.
- Windows discovery entries are **hints only** — never read a pipe name out of one; derive it.
  Every endpoint still completes the authenticated protocol handshake.
- Session endpoints are scoped to validated rozi session names, with defensive stale-endpoint
  handling.

See the [platform support matrix](getting-started.md#platform-support).

## Installation and releases

- **Bootstrap scripts** — `install.sh` and `install.ps1`.
- **Managed installs** — `rozi install`, `rozi update check|apply|rollback`.
- **Signed releases** — Ed25519-signed manifests verified against keys in `release-keys.json`,
  with checksum validation and rollback to the previous version.
- **Release archives** for Linux x86_64/arm64, macOS x86_64/arm64, and Windows x86_64, built on a
  `v*` tag with checksums and extracted-binary smoke tests.

> Production publication is intentionally blocked until the maintainer-generated `release-2026-a`
> public key is committed; `release-keys.json` currently ships an empty key list.

See [Installation & releases](installation.md).

## Performance

Performance is treated as a measured property, not an assertion.

- **Criterion suites** for terminal ingest, snapshot rebuilding, protocol framing, the end-to-end
  session pipeline, app render, scrollback search, and server fairness. Corpora are deterministic
  and generated — never captured terminal output.
- **Dated audit reports** in [`docs/performance/audits/`](performance/) record revision,
  environment, exact commands, statistics, findings, and a verdict, so results stay comparable
  over time.
- **Bounded work everywhere it matters**: scrollback search runs in cooperative slices, workbar
  command polling is epoch-gated, remote and orphan buffering is capped, and durable resurrection
  snapshots write off the server loop and reuse unchanged panes' replay files.
- **Release profile** uses thin LTO, one codegen unit, symbol stripping, and `panic = "abort"`.

See [Benchmarks & profiling](benchmarks.md) and [Performance records](performance/README.md).

## Further reading

- [Getting started](getting-started.md) — requirements, platform support, build, run, quit.
- [Documentation index](index.md) — full table of contents.
- [AGENTS.md](../AGENTS.md) — architecture notes and module map.
