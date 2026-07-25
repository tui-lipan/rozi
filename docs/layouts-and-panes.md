# Layouts & panes

`hyprmux` arranges panes the way a Hyprland-style tiling window manager does: layout is
computed as explicit geometry, then every pane (tiled or floating) is placed at an animated
target rectangle. Each workspace carries its own layout.

## Tiled layouts

Each workspace has a **layout kind**. `m` (*Toggle layout* in the palette) **cycles** through
them: dwindle → master → grid → monocle → dwindle.

### Dwindle (default)

Panes form a binary split tree. A **new pane always splits the _focused_ pane**, and the
split axis is chosen from the focused tile's aspect ratio - wide tiles split vertically,
tall tiles split horizontally (Hyprland's dwindle behavior, never the cursor position).

Because terminal cells are taller than they are wide, the axis decision applies the configurable
`[layout].split_width_multiplier` (default `2.3`) so the visual aspect ratio - not the cell count -
drives the choice. Set it to your terminal cell height divided by cell width.

- **Flip the focused split axis** with `Space`.
- **Grow / shrink** the focused split with `]`/`+` and `[`/`-`.
- **Resize mode** (`r`) gives `hjkl` control over split ratios until you press `Esc`.

### Master

The first tiled pane becomes the **master** on the left; the remaining tiled panes stack on
the right. The master/stack divider ratio is adjustable (resize mode, `]`/`-`, or dragging the
gap with the mouse).

### Grid

Panes fill a near-square grid (`ceil(√N)` columns), row-major over the tiled panes. The last
row stretches its (possibly fewer) cells to fill the width. Order-driven, like master - there
are no split ratios to adjust.

### Monocle

Every tiled pane fills the whole area; the focused pane is on top. Switch which pane is shown
by cycling focus (`Tab`/`Shift+Tab`) or focusing directionally. PTYs for the hidden panes keep
running. (For a quick one-pane maximize that restores afterward, use fullscreen `f` instead.)

> **Resize and grid/monocle:** grid and monocle have no split ratios, so resize mode and the
> grow/shrink keys have no effect there.

## Floating panes

Toggle the focused pane between tiling and floating with `t`. A floating pane:

- carries its own explicit rectangle instead of a slot in the tile tree,
- renders with a distinct double border and a `floating` badge in its titlebar,
- can be moved (`modifier`+left-drag) and resized (`modifier`+right-drag) freely with the
  mouse, including slightly off-screen (a margin keeps it grabbable).

## Fullscreen

Toggle the focused pane fullscreen with `f`. A fullscreen pane fills the workspace area
(below the workbar) and shows a `fullscreen` badge. Toggle again to restore its previous
tiled or floating geometry.

## Focus and movement

- **Focus** moves *spatially* to the nearest pane in a direction with `h/j/k/l` or the arrow
  keys - not merely the next pane in a list.
- **Move** the focused pane with `Shift+h/j/k/l` (or `Shift`+arrows). In dwindle this
  rearranges the tile tree; floating panes move in the chosen direction.
- **Swap** the focused pane with a neighbor (`modifier`+`Ctrl`+`h/j/k/l`) exchanges the two
  panes' positions *in place* - unlike Move, it does not restructure the split tree.
- **Cycle focus** through the tiled panes in order with `Tab` (next) / `Shift+Tab` (previous),
  wrapping around. Handy in monocle to bring each pane to the top.
- **Promote to master** (`.`, or the palette) swaps the focused pane into the first/master
  slot.
- Clicking a pane or its titlebar focuses it. The focused pane gets an accent border and a
  highlighted titlebar (these color changes animate when `focus_chrome` is enabled).

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

Each pane shows a titlebar with an icon (tiled / floating / fullscreen) and a title. Toggle
titlebars on/off with the *Toggle pane titlebars* palette command.

The displayed title uses this precedence:

1. a **custom title** set by renaming the pane (`n`),
2. an **application-provided terminal title**, such as a filename set by an editor,
3. the pane's **current working directory**,
4. the pane's generic fallback label, normally `shell`.

A custom or application title is qualified with location context: `<primary title> · <path>`. Inside
a detected Git project, the path is compact but project-qualified (`hyprmux/src/view`); at the
project root it is just the project name (`hyprmux`). Outside a project, it is the home-relative or
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
pane count. Tabs are clickable.

When a workspace empties, it shows an "Empty workspace" panel prompting you to spawn a shell.
The app keeps running until you detach or quit explicitly.

## Animation policy

Layout changes animate **position and opacity** but **snap size**. During an active move or
resize, or when the terminal viewport changes, the affected pane's transition becomes
instant. This avoids issuing a `pty.resize` (SIGWINCH) on every animation frame, which would
make the shell reflow continuously. See the [`[animations]`](configuration.md#animations)
config section to tune or disable individual transitions.
