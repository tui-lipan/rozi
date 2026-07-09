# Sessions

`hyprmux` uses an **always-server** model (like `tmux`/`zellij`). A background session server
owns every PTY; the UI client always attaches to a session and parses the raw PTY byte stream into
its own terminal screens. There is no in-process ("local") PTY mode.

- **Bare launch** (`hyprmux`, `hyprmux dev`, `hyprmux --profile dev`) attaches to a per-process
  **ephemeral** session named `eph-<pid>`, autostarting its server. Ephemeral sessions are
  disposable: a clean quit shuts the server down. The `eph-<pid>` name is an implementation detail —
  the workbar shows no session badge for it, and the picker lists it as `unnamed`.
- **Named session** (`hyprmux --attach dev` or `hyprmux --session dev`) attaches to a persistent,
  user-named session, starting `hyprmux --session dev --server` if it is not already running.

Because the server owns PTYs from startup, naming a session is a **rename** in place (no pane
movement): the running shells and their scrollback are untouched. See
[Naming and renaming the current session](#naming-and-renaming-the-current-session).

## Ephemeral vs named

| | Ephemeral (unnamed) | Named (`dev`) |
|---|---|---|
| Created by | bare `hyprmux` launch | `--attach`/`--session`, or *Rename session* on the current one |
| Clean quit (`prefix q` / `Alt+q`) | server shuts down | server keeps running |
| Detach (`prefix d`) | prompts for a name first (see below) | server left running (reattachable) |
| Attach-elsewhere | server shuts down (disposable) | server left running |
| UI crash | server left running (reattachable) | server left running |
| Self-reap when no client is attached | after a short grace period | never (durable until killed) |
| Session badge (workbar) | hidden | shows the name |
| Discoverable in the picker | yes (shown as `unnamed`) | yes |
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

## Naming and renaming the current session

*Rename session* (in the command palette; no default key) renames the **current** session in place.
The server keeps its live panes and simply becomes discoverable under the new name — this is the
headline continuity win (colors, scrollback, alt-screen, and titles all stay intact, with zero pane
movement). It works both to give an ephemeral session its first real name and to change an
already-named session's name.

Renaming to a name that is already owned by a running session, or to a reserved `eph-` name, is
rejected with an error toast.

## Switching sessions in-app (the picker)

Open the session picker (*Sessions…* in the command palette). The picker always switches you to a
**separate** session — it never renames the one you're in:

- Highlight an existing session and press `Enter` to attach to it.
- Type a new name and press `Ctrl+N` to create and attach to a brand-new named session.
- Press `Ctrl+D` to detach the current session and drop onto a fresh ephemeral one.
- Press `Ctrl+K` twice to kill the highlighted session.

Switching away is a **release** of the current session: a named session's layout is pushed and the
client detaches (leaving that server running for reattach), while an ephemeral session is shut down
(it is disposable, so it does not leak an orphan server).

Killing the **current** session is allowed: its server is shut down (its PTYs die) and the UI hops
onto a fresh ephemeral session, so the client stays alive rather than quitting.

The picker auto-refreshes while it is open (sessions started or killed by other UIs appear and
disappear on their own), so there is no manual refresh key.

### Opening the picker at startup

By default a bare launch attaches straight to an ephemeral session. To be asked which session to
attach to instead, launch with `--pick` or set `[session] startup = "picker"` (see
[configuration.md](configuration.md)). The startup picker is only shown when at least one **named**
session already exists; otherwise the launch falls through to a normal ephemeral attach. Dismissing
the picker with `Esc` attaches a fresh ephemeral session, so a launch is never left without a
terminal.

## Attach, detach, and quit

All panes are server-backed. The UI receives raw pane output frames and sends input, resize,
palette, layout, rename, and kill requests back through the session protocol. Resizes are applied to
the client screen only when the server acknowledges them (`Resized`), so both parsers resize at the
same byte position and wrap state stays identical.

- **Attach-elsewhere** (picking another session from the picker) releases the current session first:
  a named session's layout is pushed and its client detaches (server left running), while an
  ephemeral session is shut down (it is disposable). Then the target is attached.
- **Detach** (`prefix d`) leaves the TUI back to your shell, tmux-style, keeping the session server
  running for later reattach. Because an *anonymous* ephemeral session has no name to reattach by,
  detaching one first prompts you to **name** it:
  - Type a name and confirm → the session is renamed in place (same server, same panes) and the UI
    detaches, leaving the now-named server running.
  - Cancel (`Esc`) → the detach is treated as a **quit**: the ephemeral server is shut down and the
    UI exits.
  - A session that is already named detaches immediately, leaving its server running.
- **Quit** (`prefix q` / `Alt+q`) exits the client. It shuts down the current server only when it is
  ephemeral; named servers keep running.
- If the server disconnects unexpectedly while attached, hyprmux marks panes errored and attempts a
  reconnect. Ephemeral sessions autostart a replacement server; a dead named session surfaces as an
  error rather than a silent empty resurrection.

## Crash recovery and multi-client

A UI crash (e.g. `kill -9`) leaves the ephemeral server running with its panes intact, so you can
reattach to it (shown as `unnamed`) from the picker and recover the scrollback. Because clients parse
raw bytes and the server broadcasts pane output, more than one client can attach to the same session.

An **ephemeral** server self-reaps once no client has been attached for a short grace period
(~45s), regardless of pane state — this backstops crashes and abnormal exits so orphaned ephemeral
servers do not accumulate. Normal transitions already tear the ephemeral server down client-side, so
the grace timer rarely fires. The picker's kill action also cleans one up on demand. A **named**
server never self-reaps from client absence: it stays alive until explicitly killed
(`kill-session`, or `Ctrl+K` in the picker).

## Ephemeral session lifecycle

```text
UI start (no --attach) ── spawn+attach ──▶ ATTACHED-EPHEMERAL(eph-<pid>)
ATTACHED-EPHEMERAL: quit                 ⇒ Shutdown ⇒ server exits (panes die)
                    Rename / detach+name ⇒ ATTACHED-NAMED (same server, same panes)
                    attach-elsewhere     ⇒ Shutdown ⇒ server exits (disposable)
                    UI crash             ⇒ ORPHAN-EPHEMERAL
ORPHAN-EPHEMERAL:   reattach ⇒ ATTACHED-EPHEMERAL
                    no client for grace period ⇒ server exits (any pane state)
                    picker kill / kill-session ⇒ server exits
ATTACHED-NAMED:     quit/detach ⇒ server keeps running (never self-reaps)
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
