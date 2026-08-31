# Sidebar

Press the `b` command key to show or hide the sidebar. Press `Shift+B` to show it and move keyboard
focus into its rows. The sidebar is client-local and does not become part of a shared session
layout.

## Operate the sidebar

The default sidebar has Activity, Panes, Sessions, Files, and Git tabs. `PageUp` and `PageDown`
switch tabs while the sidebar is visible. `\` switches between one and two panels without losing
the saved tab assignment.

After `Shift+B` focuses the sidebar:

| Key | Action |
| --- | --- |
| `j/k` or arrows | Move through selectable rows |
| `PageUp`, `PageDown` | Move by a page |
| `g`, `G`, `Home`, `End` | First or last row |
| `Enter` | Activate the selected row |
| `Tab`, `Shift+Tab` | Next or previous tab |
| `h/l`, arrows, `Space` | Collapse, expand, or toggle directories |
| `Ctrl+Shift+Left/Right` | Reorder the active tab |
| `Ctrl+Up/Down` | Focus the other panel |
| `Ctrl+Shift+Up/Down` | Move the active tab to the other panel |
| `Shift+Left/Right` | Resize the sidebar |
| `Shift+Up/Down` | Resize the panel split |
| `s` | Toggle one or two panels |
| `Esc` | Return focus to the pane |

Clicking a row runs its action without moving keyboard focus away from the pane. The sidebar is not
part of the normal `Tab` focus order.

Drag the outside edge to resize the sidebar. Drag the panel divider to change the split. Tab order,
panel assignment, width, split state, and split ratio are saved to `config.toml`. Runtime visibility
and the selected tab are not.

## Configure tabs and panels

```toml
[sidebar]
visible = false
width = 32
position = "left"
tabs = ["activity", "panes", "sessions", "files", "git"]
panels = [["activity", "panes", "sessions"], ["files", "git"]]
split = true
split_ratio = 0.5
```

`tabs` declares available tab definitions. `panels` assigns those stable tab ids to the top and
bottom panel. If `tabs` is set without `panels`, all tabs use one panel. Duplicate ids after the
first are ignored.

Set `split = false` to display one panel while retaining the two saved groups. Set `position` to
`"right"` to move the sidebar. See [Configuration](configuration.md#sidebar) for defaults, size
limits, tree options, and custom tab syntax. A complete example is in
[`examples/sidebar.toml`](../examples/sidebar.toml).

## Activity

Activity lists detected coding agents and rows published with `rozi publish`. Rows are grouped by
the Git project that contains the pane's working directory, with branch and workspace context.
Selecting a row focuses its pane and, for published slots, asks the program to show that activity.

The row state comes from the session server, so all attached clients see the same result. `blocked`,
`working`, `done`, and `idle` use their configured status roles. A completed run stays marked until
its pane is attended.

Rozi ships definitions for common coding-agent CLIs. Add or override definitions with `[[agents]]`.
See [Agent definitions](agents.md).

## Panes

Panes groups live panes by workspace. A row shows the pane title, foreground program, and working
directory. Activating a row switches workspace and focuses that pane.

Hover a pane row and click `x` twice to close it. The confirmation always applies to this pointer
action, independently of `[confirm]` key settings.

Pane titles follow the precedence described in
[Layouts and panes](layouts-and-panes.md#titles-and-exited-panes).

## Sessions

Sessions groups local sessions and known remote hosts. Activating a live row attaches or switches
to it. Restorable sessions and cached offline remote rows remain visible when available.

An offline host has a connect row. An online host has a disconnect row and a new-session row.
Disconnecting a host detaches this client but leaves named servers running.

The final **+ Connect a host…** row opens the same new-host prompt as **Sessions → Remote hosts →
Ctrl+N**. A valid target is remembered only after successful discovery. Expanding or explicitly
connecting a host probes on demand; merely opening Sessions or the sidebar does not contact every
configured host.

Hover a live session row and click `x` twice to kill it. Killing the active session leaves the
client in the picker or sessionless launcher. See [Sessions](sessions.md).

Session names are host-local. A local `dev` session and a remote `dev` session are separate rows.
See [Remote sessions](remote.md) for authentication and host setup.

## Files

Files browses the focused pane's working directory. Directories load when expanded. Activating a
file runs the tab's `on_click` action, which types the path into the focused pane by default without
pressing Enter.

The tree follows focus and current-directory reports. Expansion state is remembered for the life of
the client. While attached remotely, directory data comes from the remote session server and search
only covers directories already fetched by expansion.

## Git

Git shows changed files for the repository containing the focused pane's working directory. It
includes status markers and line-change counts when available. The root is the repository, so a
pane in a subdirectory still sees changes from the whole project.

The tab reports a clean repository, a non-repository directory, loading, or unavailable changes
instead of showing an ambiguous empty tree. Remote Git status requires `git` on the server's
`PATH`.

Files and Git refresh while visible. Hiding the sidebar or switching both panels away from them
stops polling.

## Custom tabs

Custom launcher tabs contain configured `run`, `send`, or `popup` entries. These use the same
actions as custom key commands.

An entry's `group` puts it under a section header, the same header the Activity, Panes, and Sessions
tabs use. Entries are clustered by group in the order each group first appears, and entries without
one lead the list unheaded. A tab of agents grouped by which agent to start is the shape this is
for: one click opens a pane running that agent in that project.

```toml
{ name = "agents", label = "Agents", entries = [
  { label = "rozi", group = "claude", run = "cd ~/Projects/rozi && claude" },
  { label = "rozi", group = "codex", run = "cd ~/Projects/rozi && codex" },
] }
```

Command-backed tabs run a command when the tab becomes visible and repeat at the configured
interval, with a minimum of five seconds. Each run has:

- a five-second timeout
- 64 KiB combined output capture
- at most 500 rows
- at most 4096 characters retained per raw row
- at most 160 characters displayed per row

Rozi strips terminal control sequences from output. Spawn failures, timeouts, standard error, and
non-zero exits appear as non-clickable error rows.

Extensions contribute tabs too, with `[[sidebar_tabs]]` in their manifest and the same launcher and
command forms. Their IDs are namespaced `<extension>.<name>`, they land in the first panel until you
drag them somewhere, and a placement survives the extension being disabled or broken. See
[Extensions](extensions.md#sidebar-tabs).

A command tab groups its own output: set `group_prefix`, and each output line starting with it
becomes a section header carrying the rest of the line. A line holding nothing but the prefix is
dropped. Headers are never selectable.

An `on_click` `send` action may use `{line}`. Rozi sends the selected sanitized row literally. It
does not quote or evaluate it. `{line}` is rejected in `run` and `popup`.

`run`, `popup`, and `exec` actions receive the selected row in `ROZI_ROW` instead, the same way tree
actions receive a path in `ROZI_FILE`. Quote `"$ROZI_ROW"` when passing it to another program: a row
is command output and must never compose a command line.

Command tabs also run where the focused pane is, so a tab describes the project you are working in
and re-lists when you `cd`. Under `--remote` the pane's directory belongs to the server, so the
client's own directory stands.

Tree `send` actions may use `{path}`. Tree `run` and `popup` actions receive the selected path in
`ROZI_FILE` instead of inserting it into command text. Quote `"$ROZI_FILE"` when passing it to
another program.

Custom commands run as the current user through the configured `command_shell`. Treat sidebar
configuration as executable code, and treat command output and filenames as untrusted input.

## Shared sessions

The sidebar's visibility, tabs, panels, selection, and caches are local to each client. The
controller's sidebar still affects the shared terminal width because the controller defines the
canonical pane canvas. A follower's sidebar does not resize shared PTYs. See
[Shared sessions](shared-sessions.md#terminal-size).
