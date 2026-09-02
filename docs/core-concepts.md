# Core concepts

## Client and session server

The rozi interface is a client. A session server owns the PTYs and processes inside panes. This
separation lets a named session continue when a client closes or detaches.

A client does not need to be attached to a session. By default, a bare `rozi` opens the session
picker without creating a session. If you dismiss the picker, the launcher remains open with no
session attached.

## Panes, workspaces, and layouts

A pane is one terminal backed by a PTY. New panes split the focused pane unless you ask for a
floating pane.

A workspace is a group of panes. Each of the nine workspaces has its own layout and can have a
name. Moving to another workspace changes what the client displays, not which session it is
attached to.

A layout decides how tiled panes share the workspace. Layout changes do not restart the programs
inside panes. Floating and fullscreen are pane states layered on top of the tiled arrangement.

See [Layouts and panes](layouts-and-panes.md).

## Named and temporary sessions

A named session is the durable choice. It keeps running after the last client detaches and remains
available until you kill it. Create one from the picker by typing a name and pressing `Ctrl+N`, or
from a shell with:

```bash
rozi sessions new dev
```

Attach to an existing named session with:

```bash
rozi sessions attach dev
```

A temporary session has no durable user name. `Enter` or `Ctrl+T` in the startup picker creates
one. After its last client leaves, it has a recovery window of about 45 seconds before its server
stops. Do not use that window as storage for work you need to keep.

See [Sessions](sessions.md) for naming, recovery, shared clients, and session shutdown.

## Attach, detach, and kill

Attaching connects a client to a running session. Detaching closes the client connection but does
not stop a named session. The default detach binding is `Ctrl+A`, then `d`.

Killing a session stops its server and the processes in its panes. Detach when you plan to return.
Kill only when the session is finished.

## Sessions and profiles

A session contains live processes. A profile is a reusable recipe for starting panes, commands,
working directories, and layouts.

Detaching and reattaching to a named session returns to the same running processes. Starting a
profile creates new processes. Profiles are useful for repeatable setups, but they do not replace a
live named session.

See [Profiles](profiles.md).

## Prefix and modifier controls

The default prefix is `Ctrl+A`. Press it, release it, then press a command key. For example,
`Ctrl+A`, then `Enter` opens another pane.

Most default commands also have a direct `Alt` shortcut. Prefix mode is the portable control path
and keeps ordinary typing available to the focused terminal. Both control paths can be rebound.

See [Keybindings](keybindings.md).

## Shared and local client state

When several clients attach to one session, they share the panes and layout. One client controls
layout changes at a time. Each client still keeps its own focused pane, active workspace,
scrollback position, overlays, sidebar, and theme.

See [Sessions](sessions.md#share-a-live-session).
