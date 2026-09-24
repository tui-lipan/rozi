# Layouts and panes

This page covers how to open, arrange, and resize panes, how each layout places them, and how
workspaces, floating panes, popups, and the scratchpad fit around the layout. For what panes,
workspaces, and layouts are, see [Core concepts](core-concepts.md#panes-workspaces-and-layouts).

Each workspace has its own layout. The default is Dwindle. Press `Ctrl+A`, then `m` to cycle
layouts, or use the `M` command key to open **Layouts**, which previews each one. Press `Ctrl+F` in
**Layouts** to make the highlighted layout the default for new workspaces. Profiles can set a layout
per workspace; see [Profiles](profiles.md).

## Work with panes

Press these command keys after the `Ctrl+A` prefix, or hold `Alt` while pressing them. See
[Keybindings](keybindings.md) for the full reference.

| Task | Command key |
| --- | --- |
| Open a pane | `Enter` |
| Open a floating pane | `Shift+Enter` |
| Close the focused pane | `w` |
| Focus left, down, up, right | `h`, `j`, `k`, `l`, or arrow keys |
| Swap with a neighbor | `H/J/K/L`, or `Shift` plus arrows |
| Move and reinsert beside a neighbor | `Ctrl+H/J/K/L`, or `Ctrl` plus arrows |
| Cycle focus | `Tab` or `Shift+Tab` |
| Grow or shrink | `=` or `-` |
| Enter resize mode | `r` |
| Toggle floating | `t` |
| Toggle fullscreen | `f` |
| Promote to master | `.` |
| Rename the pane | `N` |

**Swap** exchanges two panes without changing the layout's shape. **Move** takes the focused pane
out and reinserts it beside the neighbor, which can change the split tree.

In resize mode, use `h/j/k/l` or the arrow keys, then press `Esc` or `Enter`. Layouts without
adjustable sizes ignore resizing. A floating pane resizes its own rectangle.

A new pane starts in the focused pane's current working directory when rozi can detect it, and
otherwise in the configured `cwd`. See
[Terminal features](terminal.md#working-directories-and-shell-metadata).

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

The same five panes in each layout:

<CaptureGallery title="~/src/rozi — web">
<img src="./assets/captures/layout-dwindle.webp" alt="Five panes in the dwindle layout, each split taking half of the previous pane" data-label="dwindle" data-caption="Each new pane splits the focused one along its longer side." data-code='[layout]\ndefault = "dwindle"'>
<img src="./assets/captures/layout-master.webp" alt="Five panes in the master layout: one large pane on the left and four stacked on the right" data-label="master" data-caption="One master pane on the left, the rest stacked beside it." data-code='[layout]\ndefault = "master"'>
<img src="./assets/captures/layout-grid.webp" alt="Five panes in a near-square grid" data-label="grid" data-caption="A near-square grid, filled row by row." data-code='[layout]\ndefault = "grid"'>
<img src="./assets/captures/layout-columns.webp" alt="Five panes as equal full-height columns" data-label="columns" data-caption="Equal full-height columns." data-code='[layout]\ndefault = "columns"'>
<img src="./assets/captures/layout-scrollable.webp" alt="Full-height columns on a strip wider than the screen, scrolled to the focused pane" data-label="scrollable" data-caption="Full-height columns on a strip that scrolls to keep the focused pane in view." data-code='[layout]\ndefault = "scrollable"'>
<img src="./assets/captures/layout-monocle.webp" alt="One pane filling the workspace in the monocle layout" data-label="monocle" data-caption="One pane at a time, filling the workspace." data-code='[layout]\ndefault = "monocle"'>
</CaptureGallery>

### Dwindle

Dwindle picks the split direction from the focused tile's shape: wide tiles split side by side, and
tall tiles split top and bottom. Terminal cells are taller than they are wide, so
`[layout].split_width_multiplier` (default `2.3`) corrects for that when comparing width and height.

Press `Space` to flip the focused split's direction. To change ratios, use `=` and `-`, resize mode,
or drag a split boundary.

### Master

The first tiled pane is the master. Press `.` to promote the focused pane. To change the master
width, use `=` and `-`, resize mode, or drag the master boundary.

### Grid, columns, and rows

These layouts size panes from their count and order. Grid fills rows, Columns gives every pane the
full workspace height, and Rows gives every pane the full workspace width. Grow, shrink, and resize
mode have no effect.

### Scrollable

Scrollable arranges panes as full-height columns on a strip wider than the screen. One pane fills
the view. With more panes, each column keeps its own width, and the strip scrolls to show the
focused pane.

- Directional focus stops at the ends of the strip instead of wrapping around. `Tab` and
  `Shift+Tab` still cycle through every pane and wrap.
- Focus on hover does not scroll. Pointing at a partly hidden column focuses it in place; the strip
  scrolls when a key or click reaches that pane.
- Moving, swapping, or dragging a column does not scroll the strip if the column ends up fully
  visible. If it lands partly hidden, the strip scrolls just far enough to show it.

A column's default width is `0.45` of the view, and widths are limited to `0.20` through `0.80`.
To change the focused column's width, use `=` and `-`, the horizontal resize-mode keys, or drag
with the mouse.

### Monocle

Monocle stacks every tiled pane in the same area. Use directional focus, `Tab`, or `Shift+Tab` to
choose which pane is visible. Hidden panes keep running.

To maximize one pane briefly without changing the workspace layout, use fullscreen instead.

## Floating and fullscreen panes

Press `t` to move the focused pane between the tiled layout and a floating rectangle.
`Shift+Enter` opens a new floating shell near the pointer, or centered if rozi has not seen the
pointer yet.

To move a pane with the mouse, hold the modifier (`Alt` by default) and left-drag. Right-drag to
resize.
Instead of holding the modifier, you can press the prefix before dragging.

Press `f` to make the focused pane fill the workspace. Focus and layout commands stay on that pane
until you leave fullscreen. If a new pane opens and takes focus, it becomes the fullscreen pane, and
the old one returns to its place.

## Pane open and close animation styles

Set `[animations].pane_style`, or use **General › Animations › Pane open/close** in Settings, to
choose how panes appear and disappear.

| Style | Effect |
| --- | --- |
| `off` | The pane appears and disappears at once. Neighboring panes still animate into place. |
| `scale` | The pane grows from its center with a soft fade. This is the default. |
| `slide` | A tiled pane slides in from its split edge, clipped to its tile, while neighbors spring into their new size. Floating panes use `scale`. |
| `portal` | The pane's contents appear radially from the center, ringed by sparse punctuation. |
| `scan` | The pane's contents appear along a diagonal sweep from the top-left corner. |

`portal` and `scan` work for tiled and floating panes, including popups. The pane keeps its final
size throughout, so the program inside does not see a resize on every frame.

- `portal`, `scan`, and tiled `slide` use `geometry_ms` for their duration. Floating `slide` behaves
  like `scale` and uses `close_ms` when closing.
- Inside the scratchpad, these styles apply to individual panes; the scratchpad's dropdown keeps its
  own animation.
- The panes a new session starts with, such as the launcher's shell or a profile's layout, do not
  play these effects. They arrive with the session's switching animation; see
  [Sessions](sessions.md).
- Turning animations off, or changing the `enabled`, `spawn`, or `close` switch, finishes any
  running pane animation at once.

You can set durations, the motion curve, and one parameter per style: where `scale` grows from,
where `portal` opens, and which corner `scan` sweeps from. See
[Pane animation curves and effect settings](configuration.md#pane-animation-curves-and-effect-settings).

## Titles and exited panes

A pane's title comes from the first of these that is set:

1. A custom title set with the `N` command key.
2. A title supplied by the application.
3. The current working directory.
4. The pane's fallback label.

Submitting an empty name clears the custom title.

`[pane] titlebar` selects `bar`, `border`, `integrated`, or `inset`, and `[pane] show_titles` can hide
titlebars. See [Configuration](configuration.md#pane) for every option.

With `[pane] hold_on_exit = true`, a pane whose program exits stays in its place in the layout. Run
**Respawn exited pane** from the command palette to restart its original command in its original
working directory.

## Borders

`[pane] border_mode` chooses separate frames, merged frames, no frames, or split dividers. Merged
frames join within a layer. The scratchpad sits above the workspace, so its frames do not join the
tiles underneath.

Separate settings choose the frame style for each kind of pane and for pickers:

| Setting | Applies to |
| --- | --- |
| `border_style` | Tiled panes |
| `float_border_style` | Floating panes and popups |
| `scratch_border_style` | Scratchpad panes |
| `fullscreen_border_style` | Fullscreen panes |
| `picker_border_style` | Pickers |
| `picker_tab_background`, `picker_tab_style` | Picker category tabs |
| `picker_selection_style` | The highlighted picker row |

See [Configuration](configuration.md#pane).

## Workspaces

rozi has nine workspaces.

| Task | Command key |
| --- | --- |
| Switch to workspace 1 through 9 | `1` through `9` |
| Move the focused pane and switch | `Shift+1` through `Shift+9` |
| Move or swap the whole workspace | `Ctrl+Shift+1` through `Ctrl+Shift+9` |
| Rename the workspace | `n` |

Bare `n` renames the workspace; `N` renames the focused pane.

Each workspace keeps its own layout, name, focused pane, and pane order. An empty workspace stays
available and offers to start a shell. Profiles save workspace names and layouts. In a named
session, panes keep running until you kill the session. See [Sessions](sessions.md).

### Workspace switching animation

When you switch, the workspace content slides sideways: higher-numbered workspaces enter from the
right and lower-numbered ones from the left. Jumping several workspaces shows only the source and
the destination. The workbar, sidebar, and overlays stay still, and pane sizes do not change during
the slide.

To turn the slide off, use **General › Animations › Workspace switching** in Settings, or set
`[animations] workspace = false`. `workspace_ms` sets the duration (default 220 ms). The master
`[animations] enabled` switch also turns it off.

## Popups and scratch panes

A popup is a one-off pane opened by a configured `popup` command or by the
[control interface](control.md). It is not part of the workspace layout. By default, a popup stays
open after its command finishes so you can read the output; then `Enter`, `Esc`, or `Space` closes
it.

The scratchpad is a dropdown layer over the workspace, opened with the `` ` `` (backtick) command
key.

- Its panes belong to your client alone. They survive workspace changes and switching to another
  session, but they are not shared, saved in profiles, or included in resurrection snapshots. They
  close when the client exits.
- Scratch panes support the usual tiling, floating, and fullscreen commands, but the scratchpad is
  not another workspace.
- Tiled scratch panes use the dropdown's remembered height. Floating scratch panes can move and
  resize anywhere in the client window. A fullscreen scratch pane covers the whole window, and
  leaving fullscreen restores its previous position and size.

See [Sessions](sessions.md#scratch-panes).

## Pane synchronization

**Pane synchronization** copies what you type in one pane to the other eligible panes in the same
workspace. It has no default key: turn it on for a workspace from the command palette, or bind
`toggle-pane-synchronization`. Profiles can restore it through the workspace's `synchronized`
setting.

Use it with care: a command meant for one shell runs in every synchronized pane.
