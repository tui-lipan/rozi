# Layouts and panes

Rozi gives each workspace its own pane layout. Press `m` to cycle layouts, or `Shift+M` to open the
layout picker. The picker previews each layout. Press `Ctrl+F` there to make the highlighted layout
the default for new workspaces.

The default layout is Dwindle. Profiles can choose a different layout per workspace. See
[Profiles](profiles.md) for saved layouts and [Keybindings](keybindings.md) for the full default-key
reference.

## Work with panes

Use these command keys after the default `Ctrl+A` prefix, or hold the default `Alt` modifier while
pressing them:

| Task | Command key |
| --- | --- |
| Open a pane | `Enter` |
| Open a floating pane | `Shift+Enter` |
| Close the focused pane | `w` |
| Focus left, down, up, right | `h`, `j`, `k`, `l`, or arrow keys |
| Swap with a neighbor | `Shift+h/j/k/l`, or `Shift` plus arrows |
| Move and reinsert beside a neighbor | `Ctrl+h/j/k/l`, or `Ctrl` plus arrows |
| Cycle focus | `Tab` or `Shift+Tab` |
| Grow or shrink | `=` or `-` |
| Enter resize mode | `r` |
| Toggle floating | `t` |
| Toggle fullscreen | `f` |
| Promote to master | `.` |
| Rename the pane | `Shift+N` |

Swap exchanges two pane positions without changing the layout shape. Move removes the focused pane
and reinserts it beside the neighbor, so the split tree can change.

In resize mode, use `h/j/k/l` or the arrow keys and press `Esc` or `Enter` when done. Ratio-less
layouts ignore resize commands. A floating pane resizes its own rectangle instead.

A new pane starts in the focused pane's current working directory when Rozi can determine it.
Otherwise it uses the configured `cwd`. See [Terminal features](terminal.md#working-directories-and-shell-metadata).

## Compare layouts

| Layout | Arrangement | Resizable |
| --- | --- | --- |
| Dwindle | Each new tiled pane splits the focused pane. | Split ratios |
| Master | One master pane on the left, with the rest stacked on the right. | Master width |
| Grid | Near-square, row-major grid. | No |
| Columns | Equal full-height columns. | No |
| Rows | Equal full-width rows. | No |
| Scrollable | Ordered full-height columns on a horizontal strip. | Each pane width |
| Monocle | Every tiled pane fills the workspace, with the focused pane on top. | No |

### Dwindle

Dwindle chooses the new split axis from the focused tile's shape. Wide tiles split vertically and
tall tiles split horizontally. `[layout].split_width_multiplier`, which defaults to `2.3`, adjusts
for terminal cells being taller than they are wide.

Press `Space` to flip the focused split axis. Use `=` and `-`, resize mode, or drag a split boundary
to change ratios.

### Master

The first tiled pane is the master. Press `.` to promote the focused pane. Use `=` and `-`, resize
mode, or drag the master boundary to change the master width.

### Grid, columns, and rows

These layouts derive pane sizes from pane count and order. Grid fills rows, Columns gives every
pane the full workspace height, and Rows gives every pane the full workspace width. Grow, shrink,
and resize mode have no effect.

### Scrollable

Scrollable keeps panes as full-height columns. One pane fills the viewport. With more panes, Rozi
keeps a width for each column and scrolls the strip to reveal the focused pane.

The default saved width is `0.45` of the tile viewport, clamped from `0.20` to `0.80`. Use `=` and
`-`, horizontal resize-mode keys, or horizontal mouse resizing to change the focused column.

### Monocle

Monocle places every tiled pane over the same area. Use directional focus, `Tab`, or `Shift+Tab` to
choose which pane is visible. Hidden panes keep running.

Use fullscreen instead when you want to maximize one pane temporarily without changing the
workspace layout.

## Floating and fullscreen panes

Press `t` to move the focused pane between the tiled layout and a floating rectangle.
`Shift+Enter` creates a new floating shell near the pointer, or centered when Rozi has not seen a
pointer position.

Hold the WM modifier and left-drag to move a pane. Right-drag to resize it. You can press the prefix
before the drag instead of holding the modifier.

Press `f` to make the focused pane fill the workspace. Focus and layout movement stay locked to
that pane until you leave fullscreen. A newly focused spawn can take over fullscreen while the old
pane returns to its prior position.

## Titles and exited panes

The displayed pane title uses this order:

1. A custom title set with `Shift+N`.
2. A title supplied by the application.
3. The current working directory.
4. The pane's fallback label.

Submitting an empty pane name clears the custom title. `[pane] titlebar` selects `bar`, `border`,
`integrated`, or `inset`, and `[pane] show_titles` can hide titlebars. Exact settings are listed in
[Configuration](configuration.md#pane).

With `[pane] hold_on_exit = true`, an exited pane stays in its layout slot. Run **Respawn exited
pane** from the command palette to restart its saved launch command and working directory.

## Workspaces

Rozi has nine workspaces.

| Task | Command key |
| --- | --- |
| Switch to workspace 1 through 9 | `1` through `9` |
| Move the focused pane and switch | `Shift+1` through `Shift+9` |
| Move or swap the whole workspace | `Ctrl+Shift+1` through `Ctrl+Shift+9` |
| Rename the workspace | `n` |

Bare `n` renames the workspace. `Shift+N` renames the focused pane.

Each workspace keeps its layout, name, focused pane, and pane order. An empty workspace remains
available and prompts you to start a shell. Profiles save workspace names and layouts. Live named
sessions preserve their server-owned panes until you kill the session. See [Sessions](sessions.md).

## Popups and scratch panes

A popup is a one-shot pane opened by a configured `popup` command or the control interface. It is
not part of the workspace layout. A completed popup stays open by default so you can read its
output, then closes with `Enter`, `Esc`, or `Space`.

The scratchpad opens with the backtick command key. Its panes live on a private server owned by the
current UI client. They survive workspace changes and attached-session switches, but they are not
shared, saved in profiles, or included in resurrection snapshots. They close when the client exits.
See [Sessions](sessions.md#scratch-panes).

## Pane synchronization

**Pane synchronization** has no default key and is available in the command palette. When enabled
for a workspace, terminal input sent to one workspace pane is copied to the other eligible panes in
that workspace. Profiles can restore the `synchronized` workspace setting.

Use this with care. A command intended for one shell can run in every synchronized pane.
