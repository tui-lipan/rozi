# Layouts & panes

`rozi` arranges panes the way a Hyprland-style tiling window manager does: layout is
computed as explicit geometry, then every pane (tiled or floating) is placed at an animated
target rectangle. Each workspace carries its own layout.

## Tiled layouts

Each workspace has a **layout kind**. `m` (*Switch layout* in the palette) **cycles** through
them: dwindle → master → grid → columns → rows → scrollable → monocle → dwindle. `Shift+M`
(*Choose layout…* in the palette) opens a picker to jump straight to any mode instead; highlighting
a mode **previews it live** and leaving without pressing Enter restores the layout you opened on.
`ctrl+f` there persists the highlighted mode as `[layout].default`, the layout every fresh
workspace starts in (profiles override it per workspace).

### Dwindle (default)

Panes form a binary split tree. A **new pane always splits the _focused_ pane**, and the
split axis is chosen from the focused tile's aspect ratio - wide tiles split vertically,
tall tiles split horizontally (Hyprland's dwindle behavior, never the cursor position).

Because terminal cells are taller than they are wide, the axis decision applies the configurable
`[layout].split_width_multiplier` (default `2.3`) so the visual aspect ratio - not the cell count -
drives the choice. Set it to your terminal cell height divided by cell width.

- **Flip the focused split axis** with `Space`.
- **Grow / shrink** the focused pane against its immediate sibling with `=` and `-`.
- **Resize mode** (`r`) gives `hjkl` control over the surrounding splits until you press `Esc`.

Every resize lands on a whole cell. A keyboard step is 4% of the split it moves, rounded to a
cell and never less than one, so each press of a key moves a given divider by the same amount;
dragging a divider with the mouse follows the pointer one cell per cell.

Each boundary offers exactly one handle, and it is never a pane's own border — a side border
carries the terminal's scrollbar, and clicking a border should reach the pane, not the divider.
Left|right boundaries are grabbed by the gap column between the panes. Stacked boundaries are
grabbed by the lower pane's titlebar row when `[pane] titlebar = "bar"` (the default); with
`border_mode = "dividers"` the drawn divider sitting above that bar is grabable with it. With the
other titlebar modes there is no bar row between the panes, so the gap (or the two touching border
rows) is the handle. Merged borders overlap, leaving the shared seam cell as the handle on either
axis.

### Master

The first tiled pane becomes the **master** on the left; the remaining tiled panes stack on
the right. The master/stack divider ratio is adjustable (resize mode, `]`/`-`, or dragging the
gap with the mouse).

### Grid

Panes fill a near-square grid (`ceil(√N)` columns), row-major over the tiled panes. The last
row stretches its (possibly fewer) cells to fill the width. Order-driven, like master - there
are no split ratios to adjust.

### Columns

Every tiled pane is a full-height column. Widths are equal (aside from an unavoidable cell
remainder) and together fill the tile bounds, respecting horizontal gaps and border merging.
Order-driven and ratio-less - new panes append; resize mode and grow/shrink have no effect.

### Rows

The transpose of Columns: every tiled pane is a full-width row of equal height, in the same order.
Heights differ by at most one cell where the tile bounds do not divide evenly. With the default
gaps the rows stack flush, because the tile gap carries a column between side-by-side panes but
none between stacked ones. Order-driven and ratio-less, exactly like Columns.

### Scrollable

Active tiled panes form ordered full-height columns on a horizontal strip. Each pane's stored
width fraction (default `0.45`, clamped to `0.20`–`0.80`) is a flex basis rounded to whole cells of
the canonical tile viewport - not a follower-local width. One tiled pane fills the whole tile
regardless of its stored basis. Two panes whose preferred widths plus the gap fit share the
remaining free cells evenly (order-balanced whole-cell remainder) so they exactly span the tile
while preserving their basis difference; if they overflow, they keep independent preferred widths
and the strip scrolls. Three or more panes always keep independent preferred widths (the default
strip). Grow/shrink (`=`/`-`), resize mode Left/Right, and modifier+mouse horizontal corner drag
adjust the focused pane's stored basis; Up/Down and vertical drag are no-ops, and there are no
shared split-divider strips. The viewport stays put when the focused tiled pane is already fully
visible; it scrolls only when that pane is horizontally clipped or outside the visible workspace.
Reveal is mirrored and minimal: a target clipped on the left aligns its left edge with the visible
left edge, and a target clipped on the right aligns its right edge with the visible right edge
(clamped to the valid scroll range). A pane wider than the viewport picks the edge matching the
focus movement direction and does not re-arm on reaffirm. Focusing a floating pane keeps the last
tiled anchor and reveal edge. Off-viewport columns are clipped by the canvas. Order-driven.

### Monocle

Every tiled pane fills the whole area; the focused pane is on top. Switch which pane is shown
by cycling focus (`Tab`/`Shift+Tab`) or focusing directionally. PTYs for the hidden panes keep
running. (For a quick one-pane maximize that restores afterward, use fullscreen `f` instead.)

> **Resize and ratio-less layouts:** grid, columns, rows, and monocle have no adjustable pane widths or
> split ratios, so resize mode and the grow/shrink keys have no effect there. Scrollable panes
> are independently width-resizable (see above).

## Floating panes

Toggle the focused pane between tiling and floating with `t`. A floating pane:

- carries its own explicit rectangle instead of a slot in the tile tree,
- renders with a distinct double border in frame modes and a `floating` badge in its titlebar,
- can be moved (`modifier`+left-drag) and resized (`modifier`+right-drag) freely with the
  mouse, including slightly off-screen (a margin keeps it grabbable).

`spawn-float` (`Shift+Enter`) opens a **new** shell pane already floating, centered on the
mouse pointer. Near an edge the rect is clamped so the pane stays on the
canvas. With no pointer yet this run, it falls back to a centered float. Bind a different chord
with `[keys] spawn-float = "..."`. A plain `spawn` still splits
the focused tile.

## Fullscreen

Toggle the focused pane fullscreen with `f`. A fullscreen pane fills the workspace area
(below the workbar) and shows a `fullscreen` badge. Toggle again to restore its previous
tiled or floating geometry.

Fullscreen is a lock, not just a size: everything behind the pane is hidden, so the focus stays on
it. Directional focus and Tab cycling do nothing until you leave fullscreen, and moving, resizing,
and split dragging are already refused there. Spawning a pane still works and the new pane **takes
the fullscreen over** - it opens covering the workspace and the previous pane returns to its tile
underneath, so the pane you are typing into is always the pane you can see. Only one pane per
workspace is fullscreen at a time. A spawn that does not take focus (a `[[rules]]` entry with
`focus = false`) lands in the tiling behind the fullscreen pane and leaves it alone.

Jumps to a *named* pane are not locked: the sidebar, `focus-next-blocked-pane`, and the control
socket's `focus-pane` still move focus out of a fullscreen pane, because those name a destination
rather than walking the layout.

`none` and `dividers` remove frames from tiled and fullscreen panes, but floating panes, popups,
and scratchpads keep their double frames: they sit on a layer above the tiles, and with no divider
or frame anywhere else, that border is the only thing marking where they end. Set the
config-file-only `keep_special_borders = false` for a fully borderless presentation instead;
fullscreen panes always follow the global mode either way.

## Focus and movement

- **Focus** moves *spatially* to the nearest pane in a direction with `h/j/k/l` or the arrow
  keys - not merely the next pane in a list. In the single-axis layouts this means the cross axis
  has nothing to land on: every tile spans the full extent, so `k`/`j` in columns or scrollable
  (and `h`/`l` in rows) leave focus where it is rather than jumping to an arbitrary tile. Use the
  layout's own axis, or `Tab` / `Shift+Tab` to walk the whole order. A *floating* pane genuinely
  above or below the strip is still reachable that way, since it really does sit across the axis.
- **Move** the focused pane with `Shift+h/j/k/l` (or `Shift`+arrows). In dwindle this
  rearranges the tile tree; floating panes move in the chosen direction.
- **Swap** the focused pane with a neighbor (`modifier`+`Ctrl`+`h/j/k/l`) exchanges the two
  panes' positions *in place* - unlike Move, it does not restructure the split tree.
- **Cycle focus** through the tiled panes in order with `Tab` (next) / `Shift+Tab` (previous),
  wrapping around. Handy in monocle to bring each pane to the top.
- **Promote to master** (`.`, or the palette) swaps the focused pane into the first/master
  slot.
- Clicking a pane or its titlebar focuses it. The focused pane gets an accent border and a
  highlighted titlebar when `[pane] highlight_focused_border` and
  `[pane] highlight_focused_titlebar` are enabled respectively (these color changes animate when
  `focus_chrome` is enabled). In `border_mode = "dividers"`, focused-border accent recolors only
  the internal seams that touch the focused pane - not a full ring and not unrelated splits.

Set `[pane] border_mode` to `separate`, `merged`, `none`, or `dividers`. Separate mode draws a
frame around every pane; merged mode fuses adjacent frame cells; none removes all pane chrome
except enabled titlebars; and dividers reserves one cell only at internal tiled splits, where
tui-lipan composes corners, tees, and crossings automatically. `border_style` selects frame glyphs
only, so its appearance control is unavailable in the two frameless modes. Merged panes use a
standalone terminal scrollbar rather than painting its thumb over a draggable shared seam.

Unfocused panes can signal a blocked agent with the configured error role, or a finished-unseen
agent with success; attending a finished pane (its host window and the pane are both focused) clears
that signal, while focused panes keep the active border. `separate` and `merged` draw the alert ring
(merged seams resolve quiet below alert below focus); `dividers` colors only touching internal seams,
and `none` draws no pane alert. Use
`[pane] alert_border = "pulse"` (the default) for a slow breathe, or `cycle-alert-border` in Settings.
Workspace markers remain available independently through `[workbar.alert]`, whose own `mode` key
(Settings row **Alerts → Workspace tab effect**, action `cycle-workbar-alert`) takes the same off/static/pulse
values and breathes inactive marked tabs.

A **new pane opens in the focused pane's current working directory** (when it can be
discovered; see [Terminal features](terminal.md)), falling back to the configured `cwd`.

With `[pane] hold_on_exit = true`, a naturally exited workspace pane remains in its current
layout position with a dim border and `[exited N]` title suffix. Run `respawn-pane` from the
command palette (or bind it under `[keys]`) to restart its retained command and cwd with a fresh
PTY generation. `keep_open = true` commands normally continue into a shell instead of exiting.
In a shared session the controller's configuration decides whether an exited pane is retained;
the resulting layout and respawn generation propagate to followers.

## Popup runner

A popup is a transient, centered pane launched through the control socket or a `[keys]`
`popup = "command"` entry. Unlike the reusable bottom-anchored scratchpad, it is one-shot: only one
can be open, and it is never part of a workspace or shared layout. It opens in the focused pane's
working directory unless the caller names one.

By default the popup holds after its command exits (`keep_open`, see
[configuration](configuration.md#keys)): its final output and exit status remain as a read-only
result, so a popup running something short like `date` stays readable instead of flashing. Press
Enter, Escape, or Space to dismiss a completed popup. Set `keep_open = false` for a program that
owns the popup for its whole life, and the popup closes with it.

Close a popup by clicking outside it or with the normal *Close pane* action (`prefix w`). While its
command is running, Escape is deliberately not intercepted, so interactive tools such as `fzf` and
`lazygit` receive it normally. Popup entry and dismissal use the same configured spawn and close
transitions as workspace panes.

## Titlebars

Each pane can show its icon (tiled / floating / fullscreen) and title as a separate bar, embedded
in the frame border, as an integrated top strip, or inside the frame beneath the top border. Set
`[pane] titlebar` to `bar`, `border`, `integrated`, or `inset`; `border` and `integrated` preserve
the terminal row that `bar` and `inset` use. `bar` and `integrated` fill their row with the
titlebar color, while `border` and `inset` write plain text over the pane, so `[pane] title_style`
end caps apply only to the first two.
Set `[pane] show_titles = false` to hide the selected layout without losing it, or toggle titles
with the *Toggle pane titlebars* palette command. Set `[pane] highlight_focused_titlebar = false`
to keep focused and unfocused titlebars styled identically across all four layouts.
Border and integrated headers remain visible in frameless modes: tui-lipan gives them their own
row when no frame edge exists. In `border_mode = "dividers"`, a border title embeds in the
horizontal divider above the pane (like a Frame border header: `├─title────`, with a leading
dash after the junction and no trailing gap before the line continues), and an integrated title fills that
divider row in place of the line. Top-row panes still get a Frame header because nothing sits
above them to carry the title.

`inset` is the only layout that never touches a border or divider row: the title is the frame's
first interior row, so the top border stays unbroken and merged neighbors still fuse their borders
normally. It reads as the quiet counterpart to `border` - same colors, same column, one row lower,
and with an intact border above it. `[pane] padding` therefore applies to the terminal below the
title rather than around it, which keeps the title locked to the border at any padding. In
frameless modes there is no border above it, so it simply becomes the pane's first row.

The displayed title uses this precedence:

1. a **custom title** set by renaming the pane (`n`),
2. an **application-provided terminal title**, such as a filename set by an editor,
3. the pane's **current working directory**,
4. the pane's generic fallback label, normally `shell`.

A custom or application title is qualified with location context: `<primary title> · <path>`. Inside
a detected Git project, the path is compact but project-qualified (`rozi/src/view`); at the
project root it is just the project name (`rozi`). Outside a project, it is the home-relative or
absolute cwd.

Conventional shell titles shaped like `user@host:cwd` count as working-directory metadata, not
application titles. Their normal username and hostname are removed because the workbar already
identifies the active local or remote host. When the shell reports an account different from the
one that originally launched the pane, the changed account remains visible, for example
`root · /etc/nginx`. See [Project profiles & pane identity](project-profiles.md).

## Workspaces

There are **9 workspaces**. Switch with `1`–`9`; move the focused pane to a workspace with
`Shift+1`–`Shift+9`. The workbar renders a tab per workspace (at least 5 shown, growing to
include the highest occupied one and the active one), each labeled with its number and live
pane count. Tabs are clickable. Each workspace remembers its focused pane and restores it when you
return, including after switching away from and back to a retained session.

When a workspace empties, it shows an "Empty workspace" panel prompting you to spawn a shell.
The app keeps running until you detach or quit explicitly.

## Animation policy

Layout changes animate **position and opacity** but **snap size**. During an active move or
resize, or when the terminal viewport changes, the affected pane's transition becomes
instant. This avoids issuing a `pty.resize` (SIGWINCH) on every animation frame, which would
make the shell reflow continuously. See the [`[animations]`](configuration.md#animations)
config section to tune or disable individual transitions.

Panes open and close in one of two shapes, set by
[`pane_style`](configuration.md#pane-openclose-style): `scale` grows the pane from its own centre
with a fade, while `slide` brings it in from the edge it was split off — clipped to its tile, so it
emerges from behind the seam — and springs the tile that gave up the space into its new size. A slide
carries the pane at its final size throughout, so it never re-lays out the grid mid-animation.
