# Sidebar

hyprmux can reserve a resizable column on either side of the app for local navigation. The sidebar
is hidden by default and is composed from tui-lipan draggable tab bars, splitters, scrolling rows,
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
at least one canvas column. `position` is `left` or `right`. The outer divider moves the sidebar,
pane canvas, and PTYs live through the normal resize debounce; releasing it writes the final width
back to this key.

`tabs` is the catalog of available tab definitions. `panels` is a separate placement recipe made
only of those tabs' stable IDs, so persisting a drag never rewrites a custom tab definition. Enable
the split to render the two saved panel groups vertically:

```toml
panels = [["agents"], ["panes", "sessions"]]
split = true
split_ratio = 0.5
```

With two `panels` arrays, `split` defaults to `true`; spelling it out makes the presentation choice
explicit. Set it to `false` to keep the recipe while showing one bar.

Both tab bars share a drag group, so tabs reorder live within one bar and move between bars. The
panel divider is also draggable. Dragging its junction with the outer divider resizes both axes at
once; the junction ends at the sidebar gutter and does not extend onto pane borders. Tab
order/assignment and both splitter sizes persist to `hyprmux.toml` and remain live-editable there.
Toggling the sidebar (`toggle-sidebar` / `b`) writes `visible` the same way. Disabling `split`
temporarily presents the saved groups as one tab bar; it does not merge or erase
the `panels` recipe, so enabling it again restores the previous groups.

The built-in IDs are `agents`, `panes`, `sessions`, `files`, and `git`. IDs are stable machine
identities used for selection and reload reconciliation, while custom `label` values are
display-only. Duplicate IDs are skipped after the first. See
[Configuration](configuration.md#sidebar) for file tree options and for custom launcher and
command-backed table syntax.

A complete two-panel configuration with built-in, launcher, and command tabs lives at
[`examples/sidebar.toml`](../examples/sidebar.toml).

## Built-in Tabs

The **Panes** tab groups every live workspace pane under its workspace, marks local focus, and
switches workspace and focus when a row is clicked. Group headers read `Workspace 2`, or
`Workspace 2: mine` when the workspace carries a custom name — the number stays visible either way,
since that is what keybindings address.

A row names the pane, badges the current foreground program on the right, and shows the working
directory beneath:

```text
▍ hyprmux                  bash
▍ ~/Work/Projects/hyprmux
  nvim src/view/sidebar/p… nvim
  ~/Work/Projects/hyprmux
  service                  psql
  …/deploy/backend/api/service
```

Hovering a row reveals a `✕` in place of the program badge; clicking it kills that pane. See
[Closing from a row](#closing-from-a-row).

The name is a user-set title where there is one. Otherwise the terminal title is used only when a
program actually chose it (`nvim src/main.rs`); a shell sets the title to its prompt — the same
`user@host:` on every row followed by a path that clips before it says anything — so those rows are
named by the working directory's leaf instead, which is the part that differs between panes. The
directory line clips from the *left* (`…/deploy/backend/api/service`), because the tail is what
identifies a path.

`$HOME` shows as `~`. A directory the shell reports on another machine keeps its full path and
gains an `@host` suffix, uncompressed — a remote home is not this machine's. When nothing is known
about where a pane is, the program takes the second line instead so the row keeps its shape.

The **Agents** tab lists detected coding-agent processes from every workspace. The session server
inspects the foreground process group and its arguments, so agents launched through Node, Python,
shell, and package-manager wrappers are recognized without relying only on the executable name.
`HYPRMUX_AGENT` or `HERDR_AGENT` can provide an explicit agent-name hint for an unusual launcher.
The built-in catalog includes Claude Code, OpenCode, Codex, Aider, Gemini CLI, Goose, Amp, and other
common terminal agents; ordinary shells, editors, and other panes are excluded. Rows show the
normalized agent name over a detail line carrying how long the current status has held and what the
agent is doing; clicking a row switches workspace and focuses it. Closing panes, the scratchpad, and
popups are excluded.

The right edge of the name line carries the agent's workspace as `2`, or `2:mine` when the
workspace has a custom name, matching how the workbar's workspace tabs spell the same thing. Groups are
projects and a project's agents can be spread across workspaces, so this is the cross-reference to
the Panes tab, which groups the other way round. A long agent name truncates rather than pushing
the badge off the edge.

An agent working below its project root is badged with where it sits as well, as
`services/api · 2`. The group header names the project, not the directory, so in a monorepo this is
the only thing separating two rows that otherwise read identically. A deep path keeps its tail
(`…/api`), which is the part that says which component the agent is in.

The detail line reads `<elapsed> <activity>`. Elapsed time is dated from the agent run's
server-side start timestamp, so it survives a detach and reattach. A block and later resume keep
the same run start rather than resetting the timer; idle and done end the run. It coarsens as it
grows (`45s`, `12m`, `3h`, `2d`). An idle agent shows no elapsed time: how long a state that
prompts no action has lasted measures the reader rather than the agent. Its row still gets a second
line, carrying the status word alone, so rows stay two lines tall whatever the state.

A finished run is the exception: it reports how long the run *took*, measured when it ended, and
that number never moves again. The attention pulse already says the finish is recent, so a figure
climbing after the work stopped would measure nothing worth knowing. A client that attached after a
run had already finished never saw it start and shows no duration at all rather than an invented
one.

Activity is the reason published alongside a reported status, falling back to the terminal title the
agent set — agents write their current task there, which is the only activity signal a detected-only
agent offers. A title is dropped when it says nothing the row does not already: the working
directory in any spelling (`/home/you/repo`, `~/repo`, `repo`), or the agent's own name. Leading
status glyphs agents decorate their titles with are stripped, since the row has its own glyph
column. OpenCode's fixed `OC | ` title prefix is also omitted. The text is truncated to the
configured sidebar width.

The detail line always names a subject, so the elapsed time is never a bare number with nothing to
modify. Where there is an activity, that is the subject and the status word is dropped — `working`,
`blocked`, `done`, and `idle` each have their own themed glyph, so repeating them in text would only
spend width. Where there is no activity, the status word takes the slot instead (`idle`). A
custom status such as `compacting` renders as a neutral `•` and keeps its word either way, having
no glyph of its own to lean on.

Agents are grouped by project: the Git repository containing the working directory the session
server reports for the pane, falling back to the working directory itself outside a repository. An
agent launched in `hyprmux/src` therefore belongs to `hyprmux` rather than to a project called
`src`. Each group is headed by the project's basename; two projects sharing a basename are
disambiguated with one parent segment (`work/api`, `oss/api`), and a remote working directory gains
an `@host` suffix. Group order is alphabetical and stable — never by status, so blocks do not jump
while agents change state. Agents without a usable working directory collect in a trailing
`elsewhere` group; when that is the only group, rows render flat with no header. Within a group,
statuses sort as `blocked`, `working`, custom values, `done`, then `idle`. Well-known statuses are
matched case-insensitively and use themed status glyphs, while custom status spelling is shown
unchanged.

The right edge of a project header carries the branch that project has checked out, in the badge
column the rows below it use. Two worktrees of one repository are two directories and so two
groups, under names that need not say anything about the work in them (`api`, `api-2`) — the branch
is what tells them apart, and knowing which branch an agent is committing to is the difference
between reading its output and trusting it. A branch too long for half the header keeps its tail
(`…/pricing-v2`); a detached `HEAD` shows the short commit id instead. Nothing else about the
repository appears here. How far a branch has diverged from its upstream cannot be known without
fetching, and a stale count presented as current is worse than no count.

The branch is read from the repository's `HEAD` on the session server's host — so under `--remote`
it is the remote repository's branch — and re-read every couple of seconds, since `git checkout`
moves it without the working directory changing. No `git` process is run for it, and a host without
`git` installed still shows it.

A project header carries no aggregated status glyph, and its rows are not nested under it — groups
never collapse, so every row a summary glyph would stand for is already on screen directly beneath
it, and the glyph plus the indent it forced cost four cells on every row of the narrowest surface in
the app.

Elapsed times refresh once a second, and only while the Agents tab is the visible tab with at least
one row showing a still-advancing one — a screen of finished runs, whose figures are frozen, stops
the refresh entirely — as does a screen of idle agents, which show no elapsed time at all. It
repaints only when the text it would draw actually changed, so a row sitting at `12m` costs a
comparison per second rather than sixty redraws.

When an agent finishes a run — its effective status goes from `working` to a quiescent state such
as `idle` or `done` — the row shows a filled attention dot in the success color instead of the calm
idle glyph, so a completed run does not blend in with agents that were idle all along. `blocked` is
never replaced by the pulse, since it already has its own glyph, and an agent that resumes working
drops the pulse. The pulse is cleared as soon as
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
session picker's confirmation. Hovering a live session row reveals a `✕` that kills it; see
[Closing from a row](#closing-from-a-row).

Session names are per-machine. `dev` on a remote host and `dev` here are unrelated sessions that
merely share a spelling, and both are listed — attaching to one never hides the other.

Only hosts you have **connected** are contacted over ssh, and they keep being contacted until you
disconnect them. A probe that fails does not disconnect the host: the row shows why and the next
sweep tries again, so a blip — a lid closing, a VPN reconnecting — heals on its own rather than
leaving the host stuck `Offline` with its sessions missing. Sessions found on a connected host are
always listed even when the local scan fails at the same moment; the two are gathered independently.

A remote host that fails to connect says why on a third line. The badge already says `Offline`, so
the line says *which kind* of offline, in the state it found rather than the ssh error behind it:

```text
  WINVM                ○ Offline
  Click to connect
  SSH port closed
```

| Line | What happened | Where to look |
| --- | --- | --- |
| `Host not responding` | Nothing answered. | The machine is off, on another network, or behind a firewall that drops. |
| `SSH port closed` | Something answered and refused. | The machine is up but nothing is accepting SSH: `sshd` stopped, wrong port, or a firewall that rejects. Also what a stopped VM behind a published port forward gives. |
| `Host unreachable` | No route to the address. | Routing or the local network segment. |
| `Unknown host name` | The name did not resolve. | DNS, `/etc/hosts`, or the `[remote.hosts]` alias. |
| `SSH login rejected` | Reached `sshd`, it refused the login. | Keys, agent, or the username in the target. |
| `Host key not trusted` | The host key is unknown or changed. | `known_hosts`. |
| `No hyprmux on host` | Logged in, could not run `hyprmux`. | Install it there, or set `binary_path` / `HYPRMUX_REMOTE_BINARY`. |
| `ssh not installed here` | No `ssh` on this machine's `PATH`. | This machine, not the remote one. |
| `Connection failed` | Anything else. | Run `hyprmux --remote <HOST>` from a shell for the raw ssh output. |

The underlying ssh message is kept, not discarded — `hyprmux --remote <HOST>` from a shell shows it
whenever the phrase is not enough.

When the client is attached with `--remote`, both tabs are served by the session server instead of
the local filesystem: directory listings and git status come over the session, and the widget
renders them the same way. Roots follow the focused pane's server-relative directory. See
[Remote sessions](remote.md).

The **Files** and **Git** tabs are two projections of one lazy-loading file tree. **Files** browses
the focused pane's working directory. **Git** shows only paths git reports as changed, grouped under
their directories, with `M`/`A`/`D`/`?` markers and `+N -M` diff stats; it is rooted at the
repository rather than the pane's directory, so a pane sitting in `src/` still sees changes across
the whole repo. Both re-root when focus moves or the pane reports a new working directory, and a
pane on a remote host says so instead of showing a tree that does not exist locally. The same applies
when the whole UI is attached with [`--remote`](remote.md): the Files/Git tabs report that the tree
follows the remote session host; browsing remote directories over the session is not wired yet.

Both tabs are inert until visible: the tree mounts only as the active tab of a visible sidebar, and
directory reads and `git status` both run off the UI thread. Git status is refreshed when the
focused pane's command finishes rather than on a timer, so a build, commit, or checkout updates the
tab immediately while reading it costs nothing. Change markers are text rather than Nerd Font
glyphs, and icons are off by default, so neither tab assumes a patched font.

The tree scrolls internally and is not part of the sidebar's own scroll view. Clicking a directory
expands or collapses it. Clicking a file runs the tab's `on_click`, which defaults to typing the
path at the focused pane's prompt without a newline — so a click inserts the path and nothing runs
until you press Enter. Only files run the action; activating a directory never does.

### Closing from a Row

Hovering a pane row (Panes) or a live session row (Sessions) reveals a `✕` at the right edge of its
title line, taking the badge's place rather than competing with it for the narrow column. It always
takes two clicks. The first arms the row: its title strikes through and its detail line gives way to
a red `Click again to confirm`, the same cue the session picker and the host disconnect row use. The
`✕` itself does not change: the confirming click has to land back on it, and red is what *hovering*
it means — an armed `✕` painted red would answer the pointer with no change and stop reading as
something you can click.

```text
  hyprmux                     ✕      hovered
  ~/Work/Projects/hyprmux

  h̶y̶p̶r̶m̶u̶x̶                     ✕      armed
  Click again to confirm
```

| Tab | What `✕` does |
| --- | --- |
| Panes | Kills the pane, the same as [`close-pane`](keybindings.md). |
| Sessions | Kills the session — shuts its server down, the same as the session picker's `Ctrl+K`. Killing the session on screen keeps the client alive; see [where it lands](sessions.md#where-the-client-lands). |

Only live rows carry one. The last-seen rows under an offline host do not: there is nothing running
there to kill.

The confirmation is deliberately not governed by [`[confirm]`](configuration.md), which gates the
keyboard close/kill actions. The `✕` is a small pointer target sitting on a row whose ordinary click
merely focuses a pane or attaches a session, so it always confirms.

An armed `✕` stays visible even after the pointer leaves, so a live confirmation is never invisible.
It is abandoned by clicking anything else in the sidebar or by moving the keyboard cursor, and it
lapses on its own after the [confirmation window](#confirmation-window). There is no keyboard
equivalent — `Enter` runs the row's own action, and closing stays a pointer gesture.

## Confirmation Window

Every destructive confirmation in hyprmux stays armed for **3 seconds**, then lapses on its own. The
window is the same everywhere:

- the sidebar's `✕` on a pane or session row,
- the sidebar's `Click to disconnect` on a remote host,
- the session picker's `Ctrl+K` kill,
- the profile picker's delete and its ends-the-ephemeral-session open guard,
- the keyboard close/kill/quit actions gated by [`[confirm]`](configuration.md), whose confirm toast
  is shown for exactly that long — the toast disappearing means the confirmation expired.

Arming a second thing always releases the first, so only one confirmation is ever live.

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

## Keyboard Navigation

The sidebar has three states: hidden, visible but passive, and visible with the keyboard in it.
While it is passive it shows no selection at all — the row cursor is a keyboard affordance, so with
the keyboard elsewhere nothing is highlighted and the mouse gets its feedback from hover instead. A
click is a one-shot gesture that runs the row's action and leaves focus in the pane; it never parks
a highlight on the row it touched.

`focus-sidebar` moves the keyboard into the row list, revealing the sidebar first if it was hidden.
This is the only way in. The sidebar is deliberately outside the Tab ring, so Tab still belongs to
the focused pane's program, and it is outside click-to-focus, so clicking a row cannot take the
keyboard away from a running command by accident.

| Key | Action |
| --- | --- |
| `↑` / `↓` | Move the cursor. Section headers are skipped, never selected. |
| `PageUp` / `PageDown` | Move by one visible page of selectable rows. |
| `Enter` | Activate the row — the same action a click on it would run. |
| `Tab` / `Shift-Tab` | Cycle sidebar tabs. |
| `←` / `h`, `→` / `l` / `Space` | Collapse, expand, or toggle directories in the Files and Git tabs. |
| `Ctrl+Shift+←` / `Ctrl+Shift+→` | Reorder the active tab within its panel. |
| `Ctrl+↑` / `Ctrl+↓` | Move keyboard focus to the top or bottom panel. |
| `Ctrl+Shift+↑` / `Ctrl+Shift+↓` | Move the active tab to the top or bottom panel and follow it. Moving down from a single panel creates the split. |
| `Shift+←` / `Shift+→` | Move the outer splitter left or right, resizing the sidebar. |
| `Shift+↑` / `Shift+↓` | Move the panel splitter up or down. |
| `s` | Split the sidebar, or merge a split sidebar back into one panel. |
| `Esc` | Leave the sidebar and give the keyboard back to the focused pane. |

A ` SIDEBAR ` badge appears in the workbar while the sidebar holds the keyboard, alongside the
`RESIZE` / `COPY` / `HINT` mode badges. It is not a mode: the sidebar owning the keyboard is
ordinary widget focus, so anything that moves focus elsewhere — clicking a pane, hiding the
sidebar — clears it without a separate exit step.

Rows distinguish *current* from *selected*. The accent bar marks the current thing (the focused
pane, the attached session) and stays put; the selection highlight marks where the cursor is. Both
can be visible at once, and on the same row.

## Actions

- `toggle-sidebar` shows or hides the sidebar for this client. Bound to `<prefix> b` / `Alt+b`.
- `focus-sidebar` moves the keyboard into the row list, revealing the sidebar first if needed.
  Bound to `<prefix> B` / `Alt+Shift+B`.
- `toggle-sidebar-split` shows the saved panel recipe as one or two panels without discarding either
  panel's tab assignment. Bound to `<prefix> \` / `Alt+\`; while the sidebar is focused, `s` is
  also a local alias.
- `sidebar-next-tab` and `sidebar-prev-tab` cycle configured tabs while visible. Bound to
  `<prefix> PageDown` / `Alt+PageDown` and `<prefix> PageUp` / `Alt+PageUp` respectively.
- `focus-next-blocked-pane` scans all workspaces in pane order, wraps after the focused pane, and
  focuses the next pane whose reported status is `blocked`. It skips closing and special panes and
  does nothing when the current pane is the only blocked pane.
- `focus-next-blocked-pane` is unbound by default. All six sidebar actions can be rebound under
  `[keys]` or invoked with `hyprmux run-action <id>`.

Visibility, active tabs, focused panel, cursors, and caches are local runtime state. A config reload
reapplies `visible`, panel assignment, split mode, split ratio, and width, then reconciles selected
tabs by stable ID. If a selected tab was removed, the first tab in that panel becomes active.
Visibility toggles and active selections are not written to disk. Tab drag/reorder, split mode,
outer resize, and panel
resize are preferences and update `panels`, `split`, `width`, or `split_ratio` in `hyprmux.toml`.
Self-writes are ignored by the live-reload watcher, while later external edits remain authoritative.
`toggle-sidebar` remains usable while the scratchpad is open. Closing the sidebar, changing tabs,
attaching or detaching, and reloading config invalidate in-flight session discovery so an old result
cannot repopulate or restart the tab. The same epoch policy applies independently to command-tab
polling, including when command tabs are active in both panels.

## Shared Sessions

The sidebar itself, its dock, width, panel layout, visibility, selection, and caches are never serialized into
`SharedLayout`. The controller's effective content area is nevertheless the canonical pane canvas:
showing or hiding the controller sidebar changes that width and causes the normal shared layout and
PTY resize flow. Showing or hiding a follower sidebar is purely local and emits no layout commit or
PTY resize. Followers center the controller's canonical canvas in their remaining content area and
clip it when that area is smaller.

Fullscreen panes, scratchpads, popups, and modal overlays are scoped to app content, not the
sidebar. A left dock is accounted for only when translating terminal-space pointer coordinates;
pane canvas placement remains content-local for both dock positions.
