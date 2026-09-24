# Core concepts

This page explains the handful of terms the rest of the documentation relies on: sessions, clients,
panes, workspaces, layouts, and profiles.

## Sessions and clients

A **session** is where your terminals live. It runs as a background server that owns every program
in its panes. The rozi window you look at is a **client**: it displays a session and sends your
keystrokes to it.

Because the two are separate, closing the client does not stop a named session. Your shells,
editors, and builds keep running, and you can attach to them again later, from the same terminal or
another one.

A client does not have to show a session. A bare `rozi` opens the session picker without starting
anything. If you dismiss the picker, rozi stays open with no session attached until you choose one.

## Panes, workspaces, and layouts

A **pane** is one terminal. Opening a new pane splits the focused one, unless you ask for a floating
pane.

A **workspace** is a group of panes, like a virtual desktop. Each session has nine workspaces, each
with its own layout and an optional name. Switching workspaces changes what you see, not which
session you are in.

A **layout** decides how tiled panes share a workspace, such as a spiral of splits or equal columns.
Changing the layout never restarts the programs inside panes. Floating and fullscreen are states a
single pane can take on top of the tiled arrangement.

See [Panes and layouts](layouts-and-panes.md).

## Named and temporary sessions

A **named session** is the durable choice. It keeps running after the last client detaches and
stays available until you kill it. Create one from the picker by typing a name and pressing
`Ctrl+N`, or from a shell:

```bash
rozi sessions new dev
```

Attach to it again with:

```bash
rozi sessions attach dev
```

A **temporary session** has no name you chose. Start one from the picker with `Enter` while the
list is empty, or with `Ctrl+T` once it shows sessions. After its last client leaves, it waits about 45 seconds so a
crashed client can reconnect, then stops. Do not rely on that window to keep work.

See [Sessions](sessions.md) for naming, recovery, and shutdown.

## Attach, detach, and kill

- **Attaching** connects a client to a running session.
- **Detaching** closes the client but leaves a named session running. The default key is `Ctrl+A`,
  then `d`.
- **Killing** a session stops it and every program in its panes.

Detach when you plan to return. Kill only when the work is finished.

## Sessions and profiles

A session holds live programs. A **profile** is a reusable recipe for starting panes, commands,
working directories, and layouts.

Reattaching to a named session brings back the same running programs. Launching a profile starts
new ones. Profiles make a setup repeatable, but they do not replace a live named session.

See [Profiles](profiles.md).

## The prefix and the modifier

Most commands start with the **prefix**, `Ctrl+A` by default: press it, release it, then press a
**command key**. For example, `Ctrl+A`, then `Enter` opens another pane.

Most command keys also work with a held modifier, `Alt` by default, so `Alt+Enter` does the same
thing. The prefix works in every terminal and leaves ordinary typing to the focused program. Both
can be rebound.

See [Keybindings](keybindings.md).

## Shared and per-client state

Several clients can attach to one session at once. They share the panes and the layout, and one
client at a time controls layout changes. Each client keeps its own focused pane, active workspace,
scroll position, open overlays, sidebar, and theme.

See [Shared sessions](shared-sessions.md).
