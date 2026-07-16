# Sidebar

hyprmux can reserve a fixed-width column on either side of the app for local navigation. The
sidebar is hidden by default and is composed from ordinary tui-lipan tabs, frames, scrolling rows,
and mouse regions. It does not become part of the shared layout document.

## Configure

```toml
[sidebar]
visible = false
width = 32
position = "left"
tabs = ["agents", "panes", "sessions"]
```

`width` is clamped to 16-80 columns. The effective width may be smaller on narrow terminals: the
sidebar yields space before the pane canvas is allowed to fall below its minimum, and always leaves
at least one canvas column. `position` is `left` or `right`.

The built-in IDs are `agents`, `panes`, and `sessions`. IDs are stable machine identities used for
selection and reload reconciliation, while custom `label` values are display-only. Duplicate IDs
are skipped after the first. See [Configuration](configuration.md#sidebar) for custom launcher and
command-backed table syntax.

## Built-in Tabs

The **Panes** tab groups every live workspace pane under its workspace,
shows the pane title and current foreground program, marks local focus, and switches workspace and
focus when a row is clicked.

The **Agents** tab lists detected coding-agent processes from every workspace. It currently
recognizes Claude Code (`claude`/`claude-code`), OpenCode, Codex, Aider, Gemini CLI, Goose, and
Amp by their foreground executable; ordinary shells, editors, and other panes are excluded. Rows
show the normalized agent name, reported status, and a shortened reason; clicking a row switches
workspace and focuses it. An agent without a reported status is shown as `idle`, rather than
`no status`. Statuses sort as `blocked`, `working`, custom values, `done`, then `idle`.
Well-known statuses are matched case-insensitively and use themed status glyphs, while custom
status spelling is shown unchanged. Closing panes, the scratchpad, and popups are excluded.

The **Sessions** tab discovers running named sessions and includes the currently attached session,
including the current ephemeral session. Foreign ephemeral sessions are hidden. Discovery runs off
the UI thread immediately when the visible tab is activated and refreshes while that tab remains
active. Clicking a discovered row attaches to that already-running session without autostarting a
replacement if it disappeared. Incompatible sessions are rejected. Leaving a disposable ephemeral
session requires clicking the target row a second time; this confirmation is independent from the
session picker's confirmation.

## User Tabs

Launcher tabs render configured entries and reuse the same actions as user-defined `[keys]`
commands. `run` opens a pane in the active workspace, `send` writes literal bytes to the focused
pane after the normal input checks, and `popup` opens a transient popup. A queued click records the
config generation, tab name, and entry index; if config reloads before it is handled, it is ignored
instead of resolving to a different entry.

Command-backed tabs run their `command` immediately when the tab becomes active and visible, then
at the configured interval (minimum five seconds). They retain their last rows while inactive and
refresh immediately when revisited. Only one run per tab may be active. Closing the sidebar,
switching tabs, or reloading config invalidates pending runs and timers; stale results cannot repaint
or restart polling. Each run has a five-second timeout, captures at most 64 KiB of stdout and stderr,
stores at most 500 rows, and bounds both raw and displayed row lengths. ANSI/OSC escapes and control
characters are removed. Timeouts, spawn failures, stderr, and non-zero exits appear as non-clickable
error rows.

An `on_click` action may use `{line}` only inside `send`. The sanitized raw row replaces every
literal `{line}` occurrence and is written directly to the PTY; it is not shell-quoted or evaluated
by hyprmux. `run` and `popup` actions are fixed commands and configurations containing `{line}` in
either are rejected with a warning. Row clicks carry both the raw row and its output generation, so
a click queued across a refresh cannot act on replaced output.

### Security

Sidebar commands and launcher actions are trusted local configuration and execute with the user's
account through the resolved `command_shell`; hyprmux never chooses an extra `/bin/sh` fallback for
polling. Command output is untrusted display data and is sanitized and bounded before storage.
Because `{line}` can contain command syntax, use it only where literal terminal input is intended;
the receiving program or shell still decides how that typed text is interpreted when submitted.

## Actions

- `toggle-sidebar` shows or hides the sidebar for this client.
- `sidebar-next-tab` and `sidebar-prev-tab` cycle configured tabs while visible.
- `focus-next-blocked-pane` scans all workspaces in pane order, wraps after the focused pane, and
  focuses the next pane whose reported status is `blocked`. It skips closing and special panes and
  does nothing when the current pane is the only blocked pane.
- All four are unbound by default and can be assigned under `[keys]` or invoked with
  `hyprmux run-action <id>`.

Visibility and the active tab are local runtime state. A config reload reapplies `visible` and
reconciles the selected tab by stable ID; if that tab was removed, the first configured tab becomes
active. Runtime toggles are not written to disk. `toggle-sidebar` remains usable while the
scratchpad is open. Closing the sidebar, changing tabs, attaching or detaching, and reloading config
invalidate in-flight session discovery so an old result cannot repopulate or restart the tab.
The same epoch policy applies independently to command-tab polling.

## Shared Sessions

The sidebar itself, its dock, width, visibility, selection, and caches are never serialized into
`SharedLayout`. The controller's effective content area is nevertheless the canonical pane canvas:
showing or hiding the controller sidebar changes that width and causes the normal shared layout and
PTY resize flow. Showing or hiding a follower sidebar is purely local and emits no layout commit or
PTY resize. Followers center the controller's canonical canvas in their remaining content area and
clip it when that area is smaller.

Fullscreen panes, scratchpads, popups, and modal overlays are scoped to app content, not the
sidebar. A left dock is accounted for only when translating terminal-space pointer coordinates;
pane canvas placement remains content-local for both dock positions.
