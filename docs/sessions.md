# Sessions

`hyprmux` uses an **always-server** model (like `tmux`/`zellij`). A background session server
owns every PTY; the UI client always attaches to a session and parses the raw PTY byte stream into
its own terminal screens. There is no in-process ("local") PTY mode.

- **Bare launch** (`hyprmux`, `hyprmux dev`, `hyprmux --profile dev`) attaches to a per-process
  **ephemeral** session named `eph-<pid>`, autostarting its server. Ephemeral sessions are
  disposable: a clean quit shuts the server down.
- **Named session** (`hyprmux --attach dev` or `hyprmux --session dev`) attaches to a persistent,
  user-named session, starting `hyprmux --session dev --server` if it is not already running.

Because the server owns PTYs from startup, turning an ephemeral session into a named one is a
**rename** (no pane movement): the running shells and their scrollback are untouched.

## Ephemeral vs named

| | Ephemeral (`eph-<pid>`) | Named (`dev`) |
|---|---|---|
| Created by | bare `hyprmux` launch | `--attach`/`--session`, or renaming an ephemeral |
| Clean quit | server shuts down | server keeps running |
| Detach (`prefix d`) / attach-elsewhere / UI crash | server left running (reattachable) | server left running |
| Discoverable in the picker | yes (labeled `(ephemeral)`) | yes |
| User-typable name | no — the `eph-` prefix is reserved | yes |

## Named sessions

Session sockets live under hyprmux's runtime directory as `session-<name>.sock`. Names are
sanitized when constructing the socket path.

```bash
hyprmux --attach dev            # attach UI to session "dev", starting it if needed
hyprmux --session dev           # equivalent attach form
hyprmux --session dev --server  # run the server process directly
hyprmux list-sessions           # list connectable sessions with pane/layout status
hyprmux kill-session dev        # attach-handshake then request a clean Shutdown
```

`kill-session` only talks to hyprmux's session socket and sends the protocol `Shutdown` message; it
does not kill arbitrary processes or remove unrelated files.

## Creating and renaming sessions in-app

Open the session picker and type a name, then create it (`Ctrl+N`):

- From an **ephemeral** session, this **renames** the current session in place. The server keeps
  its live panes and simply becomes discoverable under the new name — this is the headline
  continuity win (colors, scrollback, alt-screen, and titles all stay intact).
- From a **named** session, this attaches elsewhere: the current session's layout is pushed and the
  client detaches (leaving that server running), then attaches to the new named session.

Renaming to a name that is already owned by a running session is rejected with an error toast.

## Attach, detach, and quit

All panes are server-backed. The UI receives raw pane output frames and sends input, resize,
palette, layout, rename, and kill requests back through the session protocol. Resizes are applied to
the client screen only when the server acknowledges them (`Resized`), so both parsers resize at the
same byte position and wrap state stays identical.

- **Attach-elsewhere** (picking another session, or creating a named one from a named session)
  pushes the current layout, detaches, and leaves the current server running. Leaving an ephemeral
  session toasts that it is still running and reattachable from the picker.
- **Detach** parks the current session and switches the UI to a fresh ephemeral session, so you
  always have a working terminal.
- **Quit** shuts down the current server only when it is ephemeral; named servers keep running.
- If the server disconnects unexpectedly while attached, hyprmux marks panes errored and attempts a
  reconnect. Ephemeral sessions autostart a replacement server; a dead named session surfaces as an
  error rather than a silent empty resurrection.

## Crash recovery and multi-client

A UI crash (e.g. `kill -9`) leaves the ephemeral server running with its panes intact, so you can
reattach to `eph-<pid>` from the picker and recover the scrollback. Because clients parse raw bytes
and the server broadcasts pane output, more than one client can attach to the same session.

An orphaned ephemeral server whose panes have all exited shuts itself down after a short grace
period; the picker's kill action also cleans one up on demand.

## Ephemeral session lifecycle

```text
UI start (no --attach) ── spawn+attach ──▶ ATTACHED-EPHEMERAL(eph-<pid>)
ATTACHED-EPHEMERAL: quit          ⇒ Shutdown ⇒ server exits (panes die)
                    rename(name)  ⇒ ATTACHED-NAMED (same server, same panes)
                    detach / attach-elsewhere / UI crash ⇒ ORPHAN-EPHEMERAL
ORPHAN-EPHEMERAL:   reattach ⇒ ATTACHED-EPHEMERAL
                    all panes exit + grace period ⇒ server exits
                    picker kill / kill-session ⇒ server exits
ATTACHED-NAMED:     quit/detach ⇒ server keeps running
```

## Layout persistence

The session server stores a layout blob pushed by attached clients. Session layout serialization
includes stable server pane ids, so reattaching preserves pane ids and dwindle tree shape even when
ids are non-contiguous.

Named profiles are separate startup layouts: `hyprmux dev` / `hyprmux --profile dev` load
`~/.config/hyprmux/profiles/dev.toml` into a fresh ephemeral session. See
[profiles.md](profiles.md).

## Stale sockets and limits

On attach, a stale socket that cannot complete the attach handshake is removed and a server is
started once. The attach handshake has a timeout so an unresponsive socket does not hang UI startup.

Known limitation: `list-sessions` reports connectable session sockets only; stale or foreign sockets
are skipped so the command does not hang.
