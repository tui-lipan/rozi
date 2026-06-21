# Layouts & panes

`hyprmux` arranges panes the way a Hyprland-style tiling window manager does: layout is
computed as explicit geometry, then every pane (tiled or floating) is placed at an animated
target rectangle. Each workspace carries its own layout.

## Tiled layouts

Each workspace has a **layout kind**, toggled with `m` (*Toggle layout* in the palette):

### Dwindle (default)

Panes form a binary split tree. A **new pane always splits the _focused_ pane**, and the
split axis is chosen from the focused tile's aspect ratio — wide tiles split vertically,
tall tiles split horizontally (Hyprland's dwindle behavior, never the cursor position).

Because terminal cells are roughly twice as tall as they are wide, the axis decision applies
a `split_width_multiplier` (2.0) so the visual aspect ratio — not the cell count — drives the
choice.

- **Flip the focused split axis** with `Space`.
- **Grow / shrink** the focused split with `]`/`+` and `[`/`-`.
- **Resize mode** (`r`) gives `hjkl` control over split ratios until you press `Esc`.

### Master

The first tiled pane becomes the **master** on the left; the remaining tiled panes stack on
the right. Toggle back to dwindle with `m`.

## Floating panes

Toggle the focused pane between tiling and floating with `t`. A floating pane:

- carries its own explicit rectangle instead of a slot in the tile tree,
- renders with a distinct double border and a `floating` badge in its titlebar,
- can be moved (`modifier`+left-drag) and resized (`modifier`+right-drag) freely with the
  mouse, including slightly off-screen (a margin keeps it grabbable).

## Fullscreen

Toggle the focused pane fullscreen with `f`. A fullscreen pane fills the workspace area
(below the top bar) and shows a `fullscreen` badge. Toggle again to restore its previous
tiled or floating geometry.

## Focus and movement

- **Focus** moves *spatially* to the nearest pane in a direction with `h/j/k/l` or the arrow
  keys — not merely the next pane in a list.
- **Move** the focused pane with `Shift+h/j/k/l` (or `Shift`+arrows). In dwindle this
  rearranges the tile tree; floating panes move in the chosen direction.
- Clicking a pane or its titlebar focuses it. The focused pane gets an accent border and a
  highlighted titlebar (these color changes animate when `focus_chrome` is enabled).

## Titlebars

Each pane shows a titlebar with an icon (tiled / floating / fullscreen), the pane id, and a
title. Toggle titlebars on/off with the *Toggle pane titlebars* palette command.

The displayed title is, in order of preference:

1. a **custom title** you set by renaming the pane (`n`),
2. the **program's terminal title** (what the running program sets via the OSC 0/2 escape,
   e.g. the shell's `$PWD` or `vim`'s filename),
3. the pane's default label (`shell`).

A pane may also show a subtitle (its launch command, else its working directory) when that
identity is known — for example, after being restored from a project profile. See
[Project profiles & pane identity](project-profiles.md).

## Workspaces

There are **9 workspaces**. Switch with `1`–`9`; move the focused pane to a workspace with
`Shift+1`–`Shift+9`. The top bar renders a tab per workspace (at least 5 shown, growing to
include the highest occupied one and the active one), each labeled with its number and live
pane count. Tabs are clickable.

When a workspace empties, it shows an "Empty workspace" panel prompting you to spawn a shell.
When the **last pane in the whole app** closes, `hyprmux` quits.

## Animation policy

Layout changes animate **position and opacity** but **snap size**. During an active move or
resize, or when the terminal viewport changes, the affected pane's transition becomes
instant. This avoids issuing a `pty.resize` (SIGWINCH) on every animation frame, which would
make the shell reflow continuously. See the [`[animations]`](configuration.md#animations)
config section to tune or disable individual transitions.
