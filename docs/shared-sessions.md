# Shared sessions

Several Rozi clients can attach to the same named session. They see the same panes and one shared
workspace layout, while each client keeps its own focus, active workspace, scrollback position,
overlays, theme, and sidebar.

Start another client with the same target:

```bash
rozi sessions attach dev
rozi --remote workbox sessions attach dev
rozi sessions attach dev --read-only
```

## Join a session

When another client is already using the session, Rozi asks whether to follow, ask for layout
control, or cancel. If immediate takeover is enabled, the control option takes control directly.

The session picker shows when other clients are attached. The workbar shows `CTRL` while this client
controls the layout and `FOLLOW` while it follows.

Joining copies the current pane list and layout first, then streams each pane's retained terminal
state. Replayable output produced before a pane snapshot is part of that snapshot. Output produced
afterwards waits behind it, so a client never receives live bytes in the middle of a pane replay.

The server keeps at most 4 MiB of encoded replay queued for an attaching client. Snapshot size has
no total replay limit. If live changes waiting behind a slow attach exceed 8 MiB, the server
disconnects that client without delaying clients which are already live.

Replays up to 256 KiB stay in memory. A larger pane is exported through an unnamed file in Rozi's
private cache directory and read back through 256 KiB frames; closing the replay removes the file.
If that cache is unavailable, attach falls back to the in-memory export instead of failing.

With the normal disk-backed cache, large pane replays live in file cache instead of anonymous heap
memory. This bounds Rozi's process working set and lets the kernel reclaim replay pages under
memory pressure. It does not proportionally reduce instantaneous system-accounted memory while the
replay remains cached. A memory-backed `XDG_CACHE_HOME` accounts those pages according to that
filesystem instead.

## Layout control

One writable client controls layout changes at a time. The controller can split, close, move,
resize, float, or fullscreen panes and can edit workspaces. Followers receive those layout changes
without losing their local terminal screens or scrollback.

Followers can still:

- focus panes and switch workspaces locally
- type into panes unless input is locked
- use copy, search, hints, overlays, and the sidebar
- request layout control

Use the `g` command key to take or request control. With `[session].allow_takeover = true`, the
default, control transfers immediately. When it is false, the controller receives a request and can
grant it with the `e` command key or through **Manage collaborators**.

The current controller can change the running session's takeover policy with **Toggle immediate
control takeover**. The config value sets the initial policy for new servers and does not rewrite a
server that is already running.

A client that moves the session into the background gives up control. If the controller disconnects,
the oldest active writable follower becomes controller. Parked and read-only clients are skipped.

## Watching a drag

While the controller drags a pane, attached clients lift the same pane out of the tiling and follow
each drag update live. A client that attaches mid-gesture starts following with the next update. The
tiles it vacates reflow on every screen, and the carried pane is drawn in the color of the `FOLLOW`
badge so it reads as someone else's gesture rather than a pane moving on its own.

A drag is not part of the shared layout. It is never saved into a profile, never restored with a
session, and disappears if the controller disconnects mid-gesture — the pane falls back into the
last committed layout.

Followers animate layout changes with their own settings: a new layout arrives as a destination and
each client eases toward it locally. A pane being dragged is the exception and tracks the controller
directly, since easing would leave it trailing the pointer.

## Terminal size

The controller's content area determines the shared PTY size. Followers display that canvas inside
their own available area. A larger follower viewport has unused space, and a smaller one clips.

Showing or hiding the controller's sidebar changes the shared content width and resizes PTYs.
Changing a follower's sidebar is local and does not resize the session.

Transferring control makes the new controller's size authoritative. Full-screen programs may
reflow when this happens.

Dragging a pane does not resize any PTY. The tiles reflow on screen for the length of the gesture,
but the programs inside them keep their grid until the pane lands, so a drag across a workspace
costs one reflow per affected pane instead of one per frame.

## Input control

By default, writable followers may type even though they cannot edit the layout. The controller can
enable **Input lock** to restrict terminal input to the controller. The lock follows the layout
control role when control transfers.

For a viewer who should never type or control layout, attach with:

```bash
rozi sessions attach dev --read-only
```

A read-only client cannot send terminal input, request or receive layout control, commit layouts, or
stop the server.

Pane synchronization is separate from collaboration. It copies one client's terminal input across
eligible panes in the active workspace. See
[Layouts and panes](layouts-and-panes.md#pane-synchronization).

## Manage collaborators

Open **Manage collaborators** from the command palette when another client is attached. Type to
filter the roster.

| Key | Action |
| --- | --- |
| `Enter` | Give layout control to the selected writable active client |
| `Ctrl+D` | Decline the selected control request |
| `Ctrl+K` twice | Remove the selected client |
| `Esc` | Close the roster |

Only the writable controller can remove another client. Removal disconnects that client and tells it
who removed it. It does not kill the session or its panes. The removed client does not reconnect to
that session automatically.

If the server does not support a collaboration action, Rozi leaves that action unavailable. Update
the clients and server together when sharing across installations with different Rozi versions.

## What is and is not shared

The server owns and shares:

- pane processes, output, runtime status, and names
- workspace membership, order, names, and layout kinds
- split ratios, floating geometry, fullscreen state, and synchronization state
- the controller role, input lock, and takeover policy

Each client keeps its own active workspace, focus, copy and search state, scrollback position, theme,
sidebar, overlays, and notifications. Scratch panes and popups are not part of the shared layout.

## Security and caveats

Local session endpoints are private to the operating-system user. Remote sharing uses SSH and the
remote user's private session endpoint. Rozi does not open a network session port.

Anyone who can attach as a writable client can type into panes. Unless input lock is enabled, this
includes followers. A writable follower can also stop the session even when another client controls
the layout. Use a separate operating-system account or SSH access policy when collaborators should
not have that authority.

Immediate takeover is convenient when all clients belong to one person. Disable it for cooperative
sharing, since taking control changes the canonical terminal size and can reflow another person's
full-screen program.

A client that stops responding is disconnected so it cannot keep layout control or block session
traffic. Live named sessions continue while at least the server remains running. See
[Sessions](sessions.md) for detach and resurrection, and [Remote sessions](remote.md) for SSH setup.
