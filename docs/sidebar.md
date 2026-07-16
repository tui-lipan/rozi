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

The **Agents** tab lists ordinary workspace panes from every workspace. Rows show the pane title,
reported status, and a shortened reason; clicking a row switches workspace and focuses it. Statuses
sort as `blocked`, `working`, custom values, `done`, `idle`, then panes with no reported status.
Well-known statuses are matched case-insensitively and use themed status glyphs, while custom status
spelling is shown unchanged. Closing panes, the scratchpad, and popups are excluded.

The **Sessions** tab discovers running named sessions and includes the currently attached session,
including the current ephemeral session. Foreign ephemeral sessions are hidden. Discovery runs off
the UI thread immediately when the visible tab is activated and refreshes while that tab remains
active. Clicking a discovered row attaches to that already-running session without autostarting a
replacement if it disappeared. Incompatible sessions are rejected. Leaving a disposable ephemeral
session requires clicking the target row a second time; this confirmation is independent from the
session picker's confirmation.

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
