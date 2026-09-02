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
controls the layout and `VIEW` while it follows.

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

## Terminal size

The controller's content area determines the shared PTY size. Followers display that canvas inside
their own available area. A larger follower viewport has unused space, and a smaller one clips.

Showing or hiding the controller's sidebar changes the shared content width and resizes PTYs.
Changing a follower's sidebar is local and does not resize the session.

Transferring control makes the new controller's size authoritative. Full-screen programs may
reflow when this happens.

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
