# Sessions

`hyprmux` currently supports two runtime modes:

- **Local mode** (default): `hyprmux`, `hyprmux dev`, or `hyprmux --profile dev` starts PTYs in
  the UI process, preserving the original single-process behavior.
- **Attached session mode**: `hyprmux --attach dev` or `hyprmux --session dev` connects to a
  named background session server. If the session socket is missing or stale, hyprmux attempts to
  start `hyprmux --session dev --server` and retries the attach handshake.

## Named sessions

Session sockets live under hyprmux's runtime directory as `session-<name>.sock`. Names are
sanitized by the server when constructing the socket path.

Useful commands:

```bash
hyprmux --attach dev        # attach UI to session "dev", starting it if needed
hyprmux --session dev       # equivalent attach form
hyprmux --session dev --server  # run the server process directly
hyprmux list-sessions       # list connectable sessions with pane/layout status
hyprmux kill-session dev    # attach-handshake then request a clean Shutdown
```

`kill-session` only talks to hyprmux's session socket and sends the protocol `Shutdown` message; it
does not kill arbitrary processes or remove unrelated files.

## Attach, detach, and quit

Attached panes are server-backed. The UI receives terminal snapshots from the server and sends
input, resize, scroll, palette, search, and kill requests back through the session protocol.

`Ctrl-q` quits the UI. In attached mode this detaches the UI process; server PTYs remain in the
session until the panes exit naturally or the session is shut down. If the server disconnects while
the UI is attached, server-backed panes are marked errored and input is rejected with a toast rather
than silently dropped.

## Layout persistence

The session server stores a layout blob pushed by attached clients. Session layout serialization
includes stable server pane ids, so reattaching preserves pane ids and dwindle tree shape even when
ids are non-contiguous. Older profile blobs without stable pane ids still restore with sequential
local ids for compatibility.

Local named profiles remain separate: `hyprmux dev` and `hyprmux --profile dev` load
`~/.config/hyprmux/profiles/dev.toml` in local mode. Use `--attach dev` when you want a persistent
server session instead.

## Stale sockets and limits

On attach, a stale socket that cannot complete the attach handshake is removed and a server is
started once. The attach handshake has a timeout so an unresponsive socket does not hang the UI
startup indefinitely.

Known limitation: `list-sessions` reports connectable session sockets only; stale or foreign
sockets are skipped so the command does not hang.
