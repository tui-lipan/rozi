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

The built-in IDs are `agents`, `panes`, `sessions`, `files`, and `git`. IDs are stable machine
identities used for selection and reload reconciliation, while custom `label` values are
display-only. Duplicate IDs are skipped after the first. See
[Configuration](configuration.md#sidebar) for file tree options and for custom launcher and
command-backed table syntax.

## Built-in Tabs

The **Panes** tab groups every live workspace pane under its workspace,
shows the pane title and current foreground program, marks local focus, and switches workspace and
focus when a row is clicked.

The **Agents** tab lists detected coding-agent processes from every workspace. The session server
inspects the foreground process group and its arguments, so agents launched through Node, Python,
shell, and package-manager wrappers are recognized without relying only on the executable name.
`HYPRMUX_AGENT` or `HERDR_AGENT` can provide an explicit agent-name hint for an unusual launcher.
The built-in catalog includes Claude Code, OpenCode, Codex, Aider, Gemini CLI, Goose, Amp, and other
common terminal agents; ordinary shells, editors, and other panes are excluded. Rows show the
normalized agent name, inferred or reported status, and a shortened reason; clicking a row switches
workspace and focuses it. Closing panes, the scratchpad, and popups are excluded.

Agents are grouped by project: the working directory the session server reports for the pane. Each
group is headed by the directory basename plus the group's most urgent status glyph; two projects
sharing a basename are disambiguated with one parent segment (`work/api`, `oss/api`), and a remote
working directory gains an `@host` suffix. Group order is alphabetical and stable — never by
status, so blocks do not jump while agents change state. Agents without a usable working directory
collect in a trailing `elsewhere` group; when that is the only group, rows render flat with no
header. Within a group, statuses sort as `blocked`, `working`, custom values, `done`, then `idle`.
Well-known statuses are matched case-insensitively and use themed status glyphs, while custom
status spelling is shown unchanged.

When an agent finishes a run — its effective status goes from `working` to a quiescent state such
as `idle` or `done` — the row shows a filled attention dot in the success color instead of the calm
idle glyph, so a completed run does not blend in with agents that were idle all along. The pulse
also surfaces on the project header. `blocked` is never replaced by the pulse, since it already has
its own glyph, and an agent that resumes working drops the pulse. The pulse is cleared as soon as
the pane is focused through any path, so looking at a finished agent acknowledges it.

Process detection infers `working` and `blocked` from server-owned terminal state where an agent
exposes recognizable progress or prompt markers. Agent integrations can publish a more reliable
status through the control socket; reported status takes precedence over inference.

### OpenCode Status Plugin

OpenCode exposes lifecycle events that provide authoritative `working`, `idle`, and blocked states.
Install the included plugin by linking or copying
`integrations/opencode/hyprmux-agent-state.js` into `~/.config/opencode/plugins/`, then restart
OpenCode inside hyprmux. The plugin has no package dependencies and does nothing outside hyprmux.
It uses the injected `HYPRMUX_SOCKET` and `HYPRMUX_PANE` values to update only its own pane.

The **Sessions** tab discovers running named sessions and includes the currently attached session,
including the current ephemeral session. Foreign ephemeral sessions are hidden. Discovery runs off
the UI thread immediately when the visible tab is activated and refreshes while that tab remains
active. Clicking a discovered row attaches to that already-running session without autostarting a
replacement if it disappeared. Incompatible sessions are rejected. Leaving a disposable ephemeral
session requires clicking the target row a second time; this confirmation is independent from the
session picker's confirmation.

The **Files** and **Git** tabs are two projections of one lazy-loading file tree. **Files** browses
the focused pane's working directory. **Git** shows only paths git reports as changed, grouped under
their directories, with `M`/`A`/`D`/`?` markers and `+N -M` diff stats; it is rooted at the
repository rather than the pane's directory, so a pane sitting in `src/` still sees changes across
the whole repo. Both re-root when focus moves or the pane reports a new working directory, and a
pane on a remote host says so instead of showing a tree that does not exist locally.

Both tabs are inert until visible: the tree mounts only as the active tab of a visible sidebar, and
directory reads and `git status` both run off the UI thread. Git status is refreshed when the
focused pane's command finishes rather than on a timer, so a build, commit, or checkout updates the
tab immediately while reading it costs nothing. Change markers are text rather than Nerd Font
glyphs, and icons are off by default, so neither tab assumes a patched font.

The tree scrolls internally and is not part of the sidebar's own scroll view. It does not join the
keyboard focus ring: keys belong to the panes, so the tree is mouse- and wheel-driven like the rest
of the sidebar. Clicking a directory expands or collapses it. Clicking a file runs the tab's
`on_click`, which defaults to typing the path at the focused pane's prompt without a newline — so a
click inserts the path and nothing runs until you press Enter. Only files run the action;
activating a directory never does.

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

The file tree's `{path}` placeholder follows the same rule for the same reason. A path comes from
the filesystem, so a checked-out repository can contain a filename carrying command syntax.
Substitution happens only into `send` text, `run` and `popup` configurations containing `{path}` are
rejected with a warning, and the default action appends no newline — the path is typed at the prompt
and nothing runs until the user submits it. File tree clicks also carry the config generation, so a
click queued across a config reload cannot act on a replaced tab.

A file tree `run`/`popup` action still receives the activated path, as the `HYPRMUX_FILE`
environment variable rather than as text spliced into the command. The command decides where the
value goes by referencing `"$HYPRMUX_FILE"`, and a quoted parameter expansion is a single word that
the shell does not re-scan for operators — so a file named `; rm -rf ~` reaches the command as an
argument instead of a second command. This is what makes launching a diff viewer or editor for the
clicked file safe; see [Configuration](configuration.md#opening-a-diff-viewer-or-editor-from-a-row).

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
