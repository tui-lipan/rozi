# Sessions

`hyprmux` uses an **always-server** model (like `tmux`/`zellij`). A background session server
owns every PTY; the UI client always attaches to a session and parses the raw PTY byte stream into
its own terminal screens. There is no in-process ("local") PTY mode.

- **Bare launch** (`hyprmux`, `hyprmux dev`, `hyprmux --profile dev`) attaches to a per-process
  **ephemeral** session named `eph-<pid>`, autostarting its server. Ephemeral sessions are
  disposable: a clean quit shuts the server down. The `eph-<pid>` name is an implementation detail -
  the workbar shows no session badge for it, and the picker lists it as `ephemeral`.
- **Named session** (`hyprmux --attach dev` or `hyprmux --session dev`) attaches to a persistent,
  user-named session, starting `hyprmux --session dev --server` if it is not already running.

Because the server owns PTYs from startup, naming a session is a **rename** in place (no pane
movement): the running shells and their scrollback are untouched. See
[Naming and renaming the current session](#naming-and-renaming-the-current-session).

## Ephemeral vs named

| | Ephemeral | Named (`dev`) |
|---|---|---|
| Created by | bare `hyprmux` launch | `--attach`/`--session`, or *Rename session* on the current one |
| Clean quit (`prefix q` / `Alt+q`) | server shuts down | server keeps running |
| Detach (`prefix d`) | prompts for a name first (see below) | server left running (reattachable) |
| Attach-elsewhere | server shuts down (disposable) | server left running |
| UI crash | server left running (reattachable) | server left running |
| Self-reap when no client is attached | after a short grace period | never (durable until killed) |
| Session badge (workbar) | hidden | shows the name |
| Discoverable in the picker | yes (shown as `ephemeral`) | yes |
| User-typable name | no - the `eph-` prefix is reserved | yes |

## Named sessions

Session sockets live under hyprmux's runtime directory as `session-<name>.sock`. Names are
sanitized when constructing the socket path.

```bash
hyprmux --attach dev            # attach UI to session "dev", starting it if needed
hyprmux --session dev           # equivalent attach form
hyprmux --attach dev --read-only # attach as a viewer without input authority
hyprmux --session dev --server  # run the server process directly
hyprmux list-sessions           # list connectable sessions with pane/layout status
hyprmux kill-session dev        # attach-handshake then request a clean Shutdown
```

`kill-session` only talks to hyprmux's session socket and sends the protocol `Shutdown` message; it
does not kill arbitrary processes or remove unrelated files.

## Naming and renaming the current session

*Rename session* (in the command palette; no default key) renames the **current** session in place.
The server keeps its live panes and simply becomes discoverable under the new name - this is the
headline continuity win (colors, scrollback, alt-screen, and titles all stay intact, with zero pane
movement). It works both to give an ephemeral session its first real name and to change an
already-named session's name.

Renaming to a name that is already owned by a running session, or to a reserved `eph-` name, is
rejected with an error toast.

## Switching sessions in-app (the picker)

Open the session picker (*Sessions…* in the command palette). The picker always switches you to a
**separate** session - it never renames the one you're in:

- Highlight an existing session and press `Enter` to attach to it.
- Type a new name and press `Ctrl+N` to create and attach to a brand-new named session.
- Press `Ctrl+D` to detach the current named session and exit the client, leaving its server running
  for later reattach.
- Press `Ctrl+K` twice to kill the highlighted named session or reset a highlighted ephemeral one.

Switching away is a **release** of the current session: a named session's client detaches (leaving
that server running for reattach - the server already holds the authoritative layout from live
commits), while an ephemeral session is shut down (it is disposable, so it does not leak an orphan
server).

Because releasing an ephemeral session throws its panes away, leaving one *while you are on it* asks
first, and the confirmation lives in the affected UI rather than a toast:

- **Attaching** to another row (`Enter`): the first press turns the target row amber with an
  *"again to confirm (ends ephemeral)"* hint; a second `Enter` commits. Moving the highlight, editing
  the query, or `Esc` cancels the arming.
- **Creating** a named session (`Ctrl+N`): the name prompt's border and title turn red with an
  *"again to confirm (ends ephemeral session)"* caption; a second `Enter` commits, editing the name
  re-arms, and `Esc` cancels.

Switching between two named sessions parks the old one and needs no confirmation.

Killing the **current** session is allowed, either with `Ctrl+K` in the picker or with *Kill
session* in the command palette: its server is shut down (its PTYs die) and the UI hops onto a fresh
ephemeral session, so the client stays alive rather than quitting.

The picker auto-refreshes while it is open (sessions started or killed by other UIs appear and
disappear on their own), so there is no manual refresh key. A session row also reports attached
clients besides the current UI (for example, `2 panes · 1 other client`), making it clear before
you attach that you will initially join that session as a follower.

### Opening the picker at startup

By default a bare launch attaches straight to an ephemeral session. To be asked which session to
attach to instead, launch with `--pick` or set `[session] startup = "picker"` (see
[configuration.md](configuration.md)). The startup picker is only shown when at least one **named**
session already exists; otherwise the launch falls through to a normal ephemeral attach. Dismissing
the picker with `Esc` attaches a fresh ephemeral session, so a launch is never left without a
terminal.

## Attach, detach, and quit

All panes are server-backed. The UI receives raw pane output frames and sends input, resize,
palette, layout-commit, rename, and kill requests back through the session protocol. Resizes are
applied to the client screen only when the server acknowledges them (`Resized`), so both parsers
resize at the same byte position and wrap state stays identical. The server is authoritative for the
layout: the controlling client commits a revisioned `SharedLayout` on every change and the server
broadcasts it (see [Shared live layouts](#shared-live-layouts)).

- **Attach-elsewhere** (picking another session from the picker) releases the current session first:
  a named session's client detaches (server left running, still layout-authoritative), while an
  ephemeral session is shut down (it is disposable). Then the target is attached.
- **Detach** (`prefix d`) leaves the TUI back to your shell, tmux-style, keeping the session server
  running for later reattach. Detaching never tears panes down. Because an *anonymous* ephemeral
  session has no name to reattach by, detaching one first prompts you to **name** it:
  - Type a name and confirm → the session is renamed in place (same server, same panes) and the UI
    detaches, leaving the now-named server running.
  - Cancel (`Esc`) → the prompt closes and you return to the session; nothing is shut down. To tear
    an ephemeral session down, quit instead (which asks for confirmation).
  - A session that is already named detaches immediately, leaving its server running.
- **Quit** (`prefix q` / `Alt+q`) exits the client. It shuts down the current server only when it is
  ephemeral; named servers keep running. Quitting an ephemeral session with a live pane asks for a
  second press first (see `[confirm].quit_ephemeral`) - this is the destructive counterpart to
  detach, which preserves the session.
- If the server disconnects unexpectedly while attached, hyprmux marks panes errored and attempts a
  reconnect. Ephemeral sessions autostart a replacement server; a dead named session surfaces as an
  error rather than a silent empty resurrection.

## Shared live layouts

Multiple clients can attach to one session at the same time and share a single, live window-manager
layout - a jaw-dropping way to pair or mirror a session across terminals. The server owns the
authoritative layout as a revisioned `SharedLayout` document (workspace membership and order, tiling
trees and ratios, layout kind, floating/fullscreen geometry, workspace names, the synchronized flag,
and pane identity). Purely local view state - focus, active workspace, overlays, copy/search mode,
scrollback position, and theme - is **never** shared, so each client browses independently.

- **Controller vs follower.** Exactly one attached client holds the layout-control **lease** (the
  *controller*); the rest are *followers*. The first client to attach is granted control; when the
  controller leaves, the oldest remaining client is promoted automatically.
- **Live commits.** The controller commits a new layout revision on every change (split, move,
  resize, float, workspace edit, …); the server bumps the revision and broadcasts it, and every
  follower reconciles its local state toward it without disturbing live terminal screens or
  scrollback.
- **Followers are read-only for layout.** A follower that tries a layout-mutating action gets a
  toast nudging it to request control; focus, workspace switching, copy/search, the palette, and
  terminal input all still work locally.
- **Cooperative control requests.** *Request layout control* (`prefix g`, or the command palette)
  asks the current controller for the lease - it never steals. The requester is flagged in the
  client roster (a `wants control` badge) and the controller gets a single non-intrusive toast
  (repeated presses are debounced, so a held key cannot spam it). The request toast shows the live
  *Grant layout control* binding (`prefix e` by default, following any `[keys]` override), which
  hands the lease to the requester in one keystroke; the controller can also **grant** or
  **decline** a specific client from the *Session clients* view, and a decline notifies the
  requester. When *no* client holds the lease (e.g. right after the controller left), a request is
  auto-granted so control is never stuck. A truly wedged controller still auto-releases via the
  heartbeat timeout below.
- **Workbar chip.** While more than one client is attached, the workbar shows a `CTRL` badge (you
  control the layout) or `VIEW` badge (you are following), and the session badge folds in the client
  count (`dev ·2`). A solo session shows neither. When you control the layout and another client has
  a pending control request, the badge turns to the warning color and gains a `●` dot.
- **Canonical size and letterboxing.** The controller owns the canonical PTY size. Followers do not
  resize the PTYs; instead they render the controller's canonical canvas centered in their own
  viewport (letterboxed), so a larger terminal shows a border of dead space and a smaller one clips
  at its edges. When control moves, the new controller's size becomes canonical in a single resize
  wave, avoiding SIGWINCH thrash in the panes.
- **Heartbeat.** The server pings each client and drops one that stops responding (≈15s), releasing
  its lease. Because pongs are answered on the UI thread, a wedged client loses control (a merely
  busy one has a generous timeout). Slow clients that fall too far behind are disconnected rather
  than allowed to stall the broadcast to everyone else.

A UI crash (e.g. `kill -9`) leaves the ephemeral server running with its panes intact, so you can
reattach to it (shown as `ephemeral`) from the picker and recover the scrollback.

> Known limitation: the scratchpad is controller-only in this version (its pane id is shared and
> would collide across clients).

### Client roster and input control

Open **Session clients** from the command palette (`session-clients`) to see every attached client,
including its label, id, and `you`, `controller`, `read-only`, or `wants control` markers. The
controller can select a writable client and press Enter or `g` to grant it layout control, or press
`d` to decline a pending request from the selected client.

The `toggle-input-lock` command restricts terminal input to the current controller. The lock follows
the control lease automatically. Clients attached with `--read-only` cannot type, request control,
commit layouts, or receive a grant. These policies are enforced by the session server.

## Crash recovery and reaping

An **ephemeral** server self-reaps once no client has been attached for a short grace period
(~45s), regardless of pane state - this backstops crashes and abnormal exits so orphaned ephemeral
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

The session server holds the authoritative layout as a revisioned `SharedLayout`, updated by the
controller's live commits (there is no client-pushed layout blob). It uses stable server pane ids
and direct tree references, so reattaching preserves pane ids and dwindle tree shape even when ids
are non-contiguous. On detach, the layout is also mirrored to disk (via the profile format) so a
fresh launch can restore it after the server is gone.

## Session resurrection

Named sessions are periodically snapshotted when `[session] resurrect = true` (the default), and
also immediately when their last client detaches after a change. Snapshots live under
`$XDG_STATE_HOME/hyprmux/sessions/<name>/` (or `~/.local/state/hyprmux/sessions/<name>/`) and use
versioned JSON metadata plus per-pane terminal replay files. Writes replace the complete snapshot
atomically with private directory (`0700`) and file (`0600`) permissions.

Starting a named server restores its layout, pane commands, working directories, titles, palette,
and saved scrollback. Processes themselves are not checkpointed: each pane command starts again in
a fresh PTY, with the old terminal history replayed above its new output. Missing replay files,
working directories, or individual pane spawn failures do not prevent the rest of the session from
loading. Unsupported or malformed snapshots are left on disk and reported without blocking startup.

`kill-session` and a clean in-protocol session shutdown mean **forget**: they remove the snapshot as
well as stopping the server. A crash, `SIGKILL`, or ordinary detach preserves it for resurrection.

Named profiles are separate startup layouts: `hyprmux dev` / `hyprmux --profile dev` load
`~/.config/hyprmux/profiles/dev.toml` into a fresh ephemeral session. See
[profiles.md](profiles.md).

## Stale sockets and limits

On attach, a stale socket that cannot complete the attach handshake is removed and a server is
started once. The attach handshake has a timeout so an unresponsive socket does not hang UI startup.

Known limitation: `list-sessions` reports connectable session sockets only; stale or foreign sockets
are skipped so the command does not hang.
The session wire protocol is version 7. After upgrading hyprmux, restart existing named session
servers before attaching with the new client.
