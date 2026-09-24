# Shared sessions

Several rozi clients can attach to the same named session. They see the same panes and one shared
workspace layout, while each client keeps its own focus, active workspace, scrollback position,
overlays, theme, and sidebar. This page covers joining a session, handing over layout control,
limiting input, and removing collaborators.

## Join a session

Start another client with the same target:

```bash
rozi sessions attach dev
rozi --remote workbox sessions attach dev
rozi sessions attach dev --read-only
```

When another client is already using the session, rozi asks whether to follow, ask for layout
control, or cancel. With immediate takeover enabled, choosing control takes it at once.

The session picker shows when other clients are attached. The workbar shows `CTRL` while this client
controls the layout and `FOLLOW` while it follows.

A joining client receives the pane list and layout first, then each pane's terminal contents. Output
produced while a pane is being sent arrives after it, so the pane never shows live output in the
middle of its history.

### Limits while joining

- The server queues at most 4 MiB of pane history for a joining client at a time. There is no limit
  on the total history sent.
- If more than 8 MiB of live output builds up behind a slow join, the server disconnects that
  client. Clients that are already attached are not delayed.
- Pane history up to 256 KiB is sent from memory. Larger history is written to an unnamed file in
  rozi's private cache directory, sent in 256 KiB pieces, and deleted afterwards. If the cache is
  unavailable, rozi sends it from memory instead.

Using the cache directory keeps rozi's own memory use bounded, and lets the operating system
reclaim those pages under memory pressure. The system may still count them as memory while they
are cached, and a memory-backed `XDG_CACHE_HOME` stores them in memory.

## Layout control

One writable client controls the layout at a time. The controller can split, close, move, resize,
float, or fullscreen panes, and edit workspaces. Followers receive those changes without losing
their terminal screens or scrollback.

Followers can still:

- focus panes and switch workspaces locally
- type into panes unless input is locked
- use copy, search, hints, overlays, and the sidebar
- request layout control

Press `Ctrl+A`, then `g` to take or request control. With `[session] allow_takeover = true`, the
default, control moves immediately. When it is `false`, the controller receives a request and can
grant it with `Ctrl+A`, then `e`, or through [Collaborators](#collaborators).

The controller can change the running session's policy with **Toggle immediate control takeover**.
`allow_takeover` only sets the policy for newly started session servers; it does not change one
that is already running.

A client that switches the session to the background gives up control. If the controller
disconnects, the writable client that has been attached longest becomes controller. Background and
read-only clients are skipped.

## Watching a drag

While the controller drags a pane, other clients lift the same pane out of the tiling and follow
the drag live. A client that attaches during the drag starts following at the next update. The
tiles the pane leaves reflow on every screen, and the dragged pane is drawn in the `FOLLOW` badge
color to show that someone else is moving it.

A drag in progress is not part of the shared layout. It is never saved into a profile or restored
with a session. If the controller disconnects mid-drag, the pane returns to the last committed
layout.

Each follower animates layout changes with its own animation settings. A dragged pane is the
exception: it tracks the controller's pointer directly.

## Terminal size

The controller's content area sets the terminal size for every pane in the session. Followers show
that area inside their own window: a larger window leaves unused space, and a smaller one cuts off
the edges.

Showing or hiding the controller's sidebar changes the shared width and resizes every pane. A
follower's sidebar is local and does not resize anything.

When control moves, the new controller's size applies. Full-screen programs may redraw for the new
size.

Dragging a pane does not resize the programs in it. The tiles reflow on screen during the drag, but
each affected pane is resized once, when the pane lands.

## Input control

By default, writable followers can type even though they cannot change the layout. The controller
can turn on **Input lock** so that only the controller can type into panes. The lock moves with
layout control.

For a viewer who should never type or change the layout, attach read-only:

```bash
rozi sessions attach dev --read-only
```

A read-only client cannot send terminal input, request or receive layout control, change the layout,
or stop the session.

Pane synchronization is a separate feature: it copies one client's typing to several panes in the
active workspace. See [Layouts and panes](layouts-and-panes.md#pane-synchronization).

## Collaborators

Open **Collaborators** from the command palette (`Ctrl+A`, then `p`) when another client is
attached. Type to filter the list.

| Key | Action |
| --- | --- |
| `Enter` | Give layout control to the selected writable client |
| `Ctrl+D` | Decline the selected control request |
| `Ctrl+K` twice | Remove the selected client |
| `Esc` | Close the list |

Only the controller can remove another client. Removal disconnects that client and tells it who
removed it; the session and its panes keep running. The removed client does not reconnect to that
session on its own.

Actions the session server does not support are unavailable. When sharing between installations
with different rozi versions, update the clients and the server together.

## What is and is not shared

The server shares:

- pane processes, output, runtime status, and names
- workspace membership, order, names, and layout kinds
- split ratios, floating geometry, fullscreen state, and synchronization state
- the controller role, input lock, and takeover policy

Each client keeps its own active workspace, focus, copy and search state, scrollback position, theme,
sidebar, overlays, and notifications. Scratch panes and popups are not part of the shared layout.

## Security and caveats

Local sessions are reachable only by the operating-system user who owns them. Remote sharing goes
through SSH to the remote user's private sessions. rozi opens no network port for sessions.

Anyone who can attach as a writable client can type into panes, including followers unless input
lock is on. A writable follower can also stop the session, even when another client controls the
layout and even with input lock on. When collaborators should not have that authority, give them a
separate operating-system account or restrict their SSH access.

Immediate takeover suits sessions where every client is yours. Turn it off when sharing with other
people: taking control changes the terminal size and can make another person's full-screen program
redraw.

A client that stops responding is disconnected so it cannot hold layout control or block the
session. A client that responds but cannot keep up with a pane's output stays attached: rozi skips
the output it fell behind on and redraws the pane from its current screen when the client catches
up. Other clients are not affected.

A named session keeps running as long as its server does, whether or not clients are attached. See
[Sessions](sessions.md) for detaching and resurrection, and [Remote sessions](remote.md) for SSH
setup.
