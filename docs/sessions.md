# Sessions

`hyprmux` uses an **always-server** model (like `tmux`/`zellij`). A background session server
owns every PTY; the UI client always attaches to a session and parses the raw PTY byte stream into
its own terminal screens. There is no in-process ("local") PTY mode.

- **Bare launch** (`hyprmux`) attaches to a per-process
  **ephemeral** session named `eph-<pid>`, autostarting its server. Ephemeral sessions are
  disposable: leaving closes an untouched one and asks whether to keep one that contains work. The
  `eph-<pid>` name is an implementation detail - the workbar shows no session badge for it, and the
  picker lists it as `ephemeral`.
- **Named target** (`hyprmux dev` or `hyprmux --session dev`) attaches to a running persistent
  session, or launches its canonical same-name profile. It errors when neither exists; empty
  sessions are created explicitly with `hyprmux new <name>`.

Because the server owns PTYs from startup, naming a session is a **rename** in place (no pane
movement): the running shells and their scrollback are untouched. See
[Naming and renaming the current session](#naming-and-renaming-the-current-session).

## Ephemeral vs named

| | Ephemeral | Named (`dev`) |
|---|---|---|
| Created by | bare `hyprmux` launch | profile launch, explicit `new`, or *Rename session* |
| Leaving (`prefix q` / `prefix d`), untouched | server shuts down | n/a |
| Leaving, after you worked in it | prompts: name it to keep, or close it | server keeps running |
| Switch to another session | retained if used, discarded if untouched | retained in this client |
| UI crash | server left running (reattachable) | server left running |
| Self-reap when no client is attached | after a short grace period | never (durable until killed) |
| Session badge (workbar) | hidden | shows the name |
| Discoverable in the picker | yes (shown as `ephemeral`) | yes |
| User-typable name | no - the `eph-` prefix is reserved | yes |

## Named sessions

Session endpoints live under hyprmux's runtime directory as `session-<name>.sock` (on Windows, that
entry stands for the named pipe `\\.\pipe\hyprmux.<user-sid>.session-<name>` — see
[Control](control.md#endpoints-per-platform)). Names are sanitized when constructing the endpoint.

```bash
hyprmux dev                     # attach "dev", or launch canonical profile "dev"
hyprmux --session dev           # equivalent target spelling
hyprmux attach dev              # attach only; error unless "dev" is running
hyprmux attach dev --read-only  # attach as a viewer without input authority
hyprmux new dev                 # create a fresh empty session; error if running
hyprmux new review --profile dev # create "review" from profile "dev"
hyprmux --session dev --server  # run the server process directly
hyprmux list-sessions           # list connectable sessions with pane/layout status
hyprmux kill-session dev        # attach-handshake then request a clean Shutdown
hyprmux --remote workbox dev    # same attach/launch surface on a remote host over SSH
hyprmux list-sessions --remote workbox
hyprmux kill-session dev --remote workbox
```

For SSH attach, bootstrap/install, protocol negotiation, and the local-vs-remote feature split, see
[Remote SSH sessions](remote.md).

The positional target performs exactly three steps: attach if that session is running; otherwise
launch the canonical same-name profile; otherwise report an error suggesting `hyprmux new <name>`.
It never silently creates an unknown target. `hyprmux attach <name> [--read-only]` is attach-only,
and `hyprmux new <name> [--profile <recipe>]` is create-only. `attach` and `new` are reserved command
words; use the `--session` spelling to target a session or canonical profile binding literally named
`attach` or `new`.

Sessions and profiles are independent. A profile is a reusable launch recipe, and its same-name
session is only its default canonical binding. A newly seeded session optionally records
`created_from_profile`; this is origin metadata, not a live link or the session's identity.
Explicit `new` means fresh creation: if an old resurrection snapshot exists for that name, hyprmux
removes it before starting the new server rather than restoring it.

`kill-session` only talks to hyprmux's session endpoint and sends the protocol `Shutdown` message;
it does not kill arbitrary processes or remove unrelated files. Any authenticated writable client
may make that explicit destructive request, so it also works while another client holds layout
control; read-only clients cannot stop the server.

## How a server starts and stops

A server is started in the background by whichever client first needs it (a profile-backed target,
explicit `new`, or configured startup policy), fully detached from that client's terminal — `DETACHED_PROCESS` with no
inherited console on Windows.

Stopping one is **always** the authenticated protocol `Shutdown` message first. That is the
mechanism on every platform; it is what `kill-session`, the picker's kill action, and a clean quit
of an ephemeral session all send, and it is what lets the server snapshot, close its PTYs, and
retire its endpoint on the way out. Only when that fails — an unresponsive server, or one too old
to even complete the handshake — does hyprmux escalate to forced termination: `SIGTERM` and then
`SIGKILL` on Unix, `TerminateProcess` on Windows.

A signal or console event asking the *server* to stop (`SIGTERM`/`SIGHUP`, or a Windows console
close) is routed onto that same graceful teardown rather than killing it where it stands. On Windows
the server additionally places itself and every ConPTY child in a kill-on-close Job Object, so even
a crash or a forced kill cannot leave orphaned shells behind.

Closing the terminal your *client* is running in (`SIGHUP`, or a Windows console close/logoff) is
treated as a **detach**, not a crash: the layout is mirrored to disk and a named session's server is
left running for you to reattach to.

After a clean client exit, hyprmux leaves a compact reattach command in terminal scrollback for the
named session that was on screen. Remote sessions include their remote target. Ephemeral and
sessionless exits print nothing because there is no durable named session to reattach to. A remote
hint is also omitted when its target would require shell-specific quoting, rather than leaving an
unsafe command to copy.

## Naming and renaming the current session

*Rename session* (`prefix Shift+S`) renames the **current** session in place.
The server keeps its live panes and simply becomes discoverable under the new name - this is the
headline continuity win (colors, scrollback, alt-screen, and titles all stay intact, with zero pane
movement). It works both to give an ephemeral session its first real name and to change an
already-named session's name.

Renaming to a name that is already owned by a running session, or to a reserved `eph-` name, is
rejected with an error toast.

## Switching sessions in-app (the picker)

Open the session picker (*Sessions…* in the command palette). The picker always moves you to a
**separate** session - it never renames the one you're in. Footer hints use connection vocabulary:

- **Connect** (`Enter` on a session you are not connected to) — establish the connection and make
  it active immediately.
- **Switch** (`Enter` on a background-connected session) — make that already-live attachment active.
- Type a new name and press `Ctrl+N` to create a fresh empty session under that name. The filter
  text carries into the create prompt, so a search that matched nothing becomes the name of the
  session you make instead — no retyping. Creation is explicit and fails if the name is already
  running.
- **Disconnect** (`Ctrl+W`) — close this client's connection to a background session; the server
  keeps running for later reconnect. Does not apply to the current session.
- **Kill** (`Ctrl+K` twice) — destroy the highlighted session for everyone. Ephemeral sessions are
  removed completely.
- **Restart** (`Ctrl+E` twice) — shut the highlighted session down and recreate it as the active
  session, with fresh panes.
- **Ephemeral shell** (`Ctrl+T`, or `Enter` when the list is empty) — go to this client's scratch
  session: start it when there is none, switch to it when there already is. The hint borrows the
  word the rows use, so it names the same thing the session becomes once it exists. Started from the
  launcher it takes whatever layout the launch prepared, the same as `Enter` on the launcher panel,
  so the startup picker need not be dismissed first. The key always works; the footer only spends a
  pill on it when the list cannot point the way itself — with nothing to pick, `Enter` is free and
  carries it, and once the scratch session exists it is a row on the list like any other.

Switching retains the current attachment in the background. Its client, screens, scrollback,
layout, and focus remain live, and background output continues to be parsed. Selecting it again is
instant and does not reconnect. This applies to named and ephemeral sessions and to local and remote
attachments; a parked remote link reconnects in place if it drops while another session is current.

A **parked** session is connected, not occupied. It gives up the layout-control lease while it is in
the background, so keeping several sessions open costs nobody else anything: another client
attaching to a session you have merely parked gets control of it normally instead of joining as your
follower. Returning to a parked session takes the lease back if nobody else claimed it meanwhile.

The one session switching does **not** retain is an ephemeral one that hyprmux created for you and
you never used — the startup scratch session, typically. Nothing was done in it, so it is shut down
rather than left running behind the session you actually wanted. An ephemeral you typed in or
reshaped is real work and is parked like anything else.

Creating a named session (`Ctrl+N`) parks the current session in the background exactly like
switching to one — it is not destructive, so a single `Enter` commits with no confirmation. The
session you were on stays live and is one selection away.

Killing the **current** session is allowed, either with `Ctrl+K` in the picker or with *Kill
session* in the command palette: its server is shut down (its PTYs die) and the client stays alive
rather than quitting. See [Where the client lands](#where-the-client-lands) for what comes next.
*Restart session* (`Ctrl+E` in the picker, or the command palette) shuts the server down and
immediately recreates it, keeping the client attached.

### Where the Client Lands

When the session on screen is taken away rather than left — killed, or the host it lives on
disconnected — the client does **not** quit and does **not** auto-attach elsewhere. It lands on the
first of these that applies:

1. The **session picker**, when another meaningful choice remains: a local or remote running
   session, a parked background attachment, a restorable snapshot, or a cached remote host session.
   The user picks what to open next.
2. The **launcher** — no session at all, picker closed — when nothing remains to choose from.
   Killing a session is not a request for a replacement, so nothing is created in its place.

Killing a **background** session leaves the active session unchanged. Killing from the open picker
keeps the picker up, drops the killed row, and keeps the nearest selection; if the list empties and
no other candidates remain, the picker closes into the launcher.

Detaching and quitting are unaffected — those are deliberate exits, not the current session being
taken away.

The picker auto-refreshes while it is open (sessions started or killed by other UIs appear and
disappear on their own), so there is no manual refresh key. A session row also reports other clients
sharing it besides your own connection (for example, `2 panes · shared with 1 other`), so you know
before attaching that you may be asked whether to follow that client or ask it for control. When available, it also shows
the creation recipe as `from <profile>`. This `created_from_profile` value survives detach,
reattach, and resurrection snapshots; replacing the session with another profile does not rewrite
its historical creation origin.

### Opening the picker at startup

By default (`[session] startup = "picker"`, also reachable with `--pick`) a bare launch asks which
session to attach to. **Opening the picker attaches nothing** - no session is created until you
choose one, so launching hyprmux never leaves a stray session behind.

The picker is shown only when there is something to pick: a running named session, a resurrection
snapshot, or a remote host with cached sessions. With nothing to choose from, the launch falls
through to an ephemeral session, which is the only thing it could have offered anyway.

Dismissing the picker with `Esc` leaves the client in the **launcher**: attached to nothing, with a
panel saying how to start a shell, reopen the picker, or detach. A client with no session is a
normal state - the launcher is also where killing your last session lands you.

Starting a shell there gives you an ephemeral session with whatever layout the launch had prepared.
Because no pane is competing for the keyboard in the launcher, a bare `Enter` does it; the ordinary
`spawn` binding works there too - in either spelling, `<prefix> Enter` or `<mod>+Enter` - and is
what you keep using once a session is up. Other PTY-creating actions (`open-config`, `[keys]` /
sidebar `run`/`popup`, scratchpad, control `new-pane`) also start that ephemeral first, then run
the requested command once the session is attached.

Set `[session] startup = "ephemeral"` to skip all of this and start a scratch session immediately.

Set `[session] startup = "last"` to reopen the exact most recently attached named session, whether
it is still running or restorable from a snapshot or its canonical same-name profile. If that
session is gone, the picker opens with its name highlighted rather than silently attaching some
other session. Explicit targets and `--pick` take precedence.

## The Sessions sidebar (grouped view)

The sidebar's **Sessions** tab groups sessions by location: a `LOCAL` group followed by one group
per **known remote host**. A host is *known* — and therefore listed even while offline — when it is
configured (`[remote.hosts.*]` or `default_host`), a recently used ad-hoc `--remote` target, or a
host a live attachment currently targets. A remote host is a location you return to, so it does not
vanish from the list just because its link is down.

- **Connect on demand.** Each host header shows its status — `● Online`, `◌ Connecting…`, or
  `○ Offline` (a failed connect shows the SSH error inline). Nothing is contacted over SSH until you
  connect a host:
  - An **offline** host shows a **Click to connect** row and, beneath it, its last-seen sessions
    (from the cache) so its known workplaces stay visible without contacting it. Connecting probes
    the host and lists its live sessions.
  - An **online** host shows **Click to disconnect** — a second click confirms, the app's usual
    click-again pattern — then its live sessions and a `+ New session`. Disconnecting closes every
    attachment to the host (their servers keep running) and returns it to offline.
  A host you are already attached to is online automatically.
- **Offline workplaces persist.** The last-seen sessions on each host are cached under
  `state_dir/host-sessions.json` (session name, ephemeral flag, and pane count only — never any
  credential; SSH handles authentication out of band), shown as muted *"N panes · last seen"* rows
  under an offline host; selecting one connects and attaches.
- **Sharing stays compact.** A live session shared with other clients shows a right-aligned `󰍺 N`
  badge beside its name, where `N` is the total attached-client count. A parked attachment instead
  takes that badge slot as `○ background` / spinner + `reconnecting` / `× offline` — the same
  chrome the session picker uses. Pane count and creation profile stay on the detail line, so
  narrow sidebars do not spend that line spelling out "shared with N others".
- **Create and connect from the list.** The local group and each online host end with a
  `+ New session`, and a `+ Connect a host…` row at the bottom opens the connect-remote-host prompt.
  Creating a session on a host parks the current session in the background and attaches the new one,
  exactly like switching.

Activating a session row attaches to it with the same background-retention semantics as the picker.

## Attach and leave

All panes are server-backed. The UI receives raw pane output frames and sends input, resize,
palette, pane-status, layout-commit, rename, and kill requests back through the session protocol.
Each live pane's reported status and optional reason are server-owned runtime metadata. They remain
available when every client detaches and are seeded to clients on reattach without generating a
status-change event. Resizes are
applied to the client screen only when the server acknowledges them (`Resized`), so both parsers
resize at the same byte position and wrap state stays identical. The server is authoritative for the
layout: the controlling client commits a revisioned `SharedLayout` on every change and the server
broadcasts it (see [Shared live layouts](#shared-live-layouts)).

- **Switch session** (picking another session) retains the current attachment in the background.
  Switching back restores its already-live screens and layout immediately. Remote links remain
  attached and reconnect independently after a transport failure.
- **Leaving** (`prefix q` / `Alt+q`, or `prefix d`) exits the client. There is one way out: *detach*
  and *quit* are the same action, because in a client that visits many sessions "how you left" says
  nothing useful. What happens is decided per session:
  - A **named** session is detached and its server left running, always. Same for every named
    session retained in the background.
  - A **temporary** session you never touched is closed silently. Nothing can reattach to it by
    name and it holds no work, so keeping it alive would only clutter the picker.
  - A **temporary session you worked in** is the one case leaving could destroy something, so it
    asks. The **Keep this session?** prompt appears (bringing the session forward first if it was
    parked, so you can see what you are deciding about):
    - Type a name and `Enter` → renamed in place (same server, same panes), left running, client
      exits. Any other temporary session gets its own prompt on the way out.
    - `Enter` on an empty name → closes it. This takes a second press, and the prompt itself says
      what that press closes (*"Enter again closes this temporary session and quits"*). Set
      `[confirm] quit_ephemeral = false` to close on the first press.
    - `Esc` → nothing is torn down and you return to the session.
- Leaving over the **control socket** (`hyprmux run-action quit`) never prompts and never closes a
  session you worked in — there is nobody to answer, and automation must not be what destroys work.
- If the server disconnects unexpectedly while attached, hyprmux marks panes errored and attempts a
  reconnect. Ephemeral sessions autostart a replacement server; a dead named session surfaces as an
  error rather than a silent empty resurrection.

## Shared live layouts

Multiple clients can attach to one session at the same time and share a single, live window-manager
layout - a jaw-dropping way to pair or mirror a session across terminals. The server owns the
authoritative layout as a revisioned `SharedLayout` document (workspace membership and order, tiling
trees and ratios, layout kind, floating/fullscreen geometry, workspace names, the synchronized flag,
and pane identity). Purely local view state - focus, active workspace, overlays, copy/search mode,
  scrollback position, theme, and sidebar visibility/active tab - is **never** shared, so each
  client browses independently.

- **Controller vs follower.** Exactly one attached client holds the layout-control **lease** (the
  *controller*); the rest are *followers*. The first client to attach is granted control; when the
  controller leaves, the oldest remaining client that is actually using the session is promoted
  automatically. Clients that have the session merely **parked** in the background hold no lease and
  are never promoted into one — a background connection is not an occupant.
- **Following is chosen, never assigned.** Attaching to a session another client is actively
  driving raises a prompt: **Follow**, **Ask for control**, or **Cancel** (which leaves the session
  alone and returns you to where you were). When immediate takeover is enabled, **Ask for control**
  becomes **Take control**. Nothing follows or transfers control silently.
- **Live commits.** The controller commits a new layout revision on every change (split, move,
  resize, float, workspace edit, …); the server bumps the revision and broadcasts it, and every
  follower reconciles its local state toward it without disturbing live terminal screens or
  scrollback.
- **Followers are read-only for layout.** A follower that tries a layout-mutating action gets a
  toast nudging it to request control; focus, workspace switching, copy/search, the palette, and
  terminal input all still work locally.
- **Taking control.** *Take layout control* (`prefix g`, or the command palette) transfers the lease
  to the asking client immediately. This is the default (`[session].allow_takeover = true`) because
  every client that can attach is already the same OS account, and the usual second client is you on
  another machine: waiting to be granted the lease there means walking back to the first keyboard.
  It is symmetric — the other client takes it back the same way — and nothing is destroyed; the
  lease and the canonical PTY size move.
- **Cooperative requests.** With `[session].allow_takeover = false`, the same key *asks* instead.
  The requester is flagged in the client roster (a `wants control` badge) and the controller gets a
  single non-intrusive toast (the server debounces repeats, and an identical toast renews in place
  rather than stacking, so a held key cannot spam it). The request toast shows the live
  *Grant layout control* binding (`prefix e` by default, following any `[keys]` override), which
  hands the lease to the requester in one keystroke; the controller can also **grant** or
  **decline** a specific client from the *Manage collaborators* dialog, and a decline notifies the
  requester. When *no* client holds the lease (e.g. right after the controller left), a request is
  auto-granted so control is never stuck either way. The current controller can change the running
  session's policy with `toggle-control-takeover`. A truly wedged controller still auto-releases via
  the heartbeat timeout below.
- **Workbar chip.** While more than one client is attached, the workbar shows a `CTRL` badge (you
  control the layout) or `VIEW` badge (you are following), and the session badge folds in the client
  count (`dev ·2`). A solo session shows neither. When you control the layout and another client has
  a pending control request, the badge turns to the warning color and gains a `●` dot.
- **Canonical size and letterboxing.** The controller owns the canonical PTY size. Followers do not
  resize the PTYs; instead they render the controller's canonical canvas centered in their own
  viewport (letterboxed), so a larger terminal shows a border of dead space and a smaller one clips
  at its edges. When control moves, the new controller's size becomes canonical in a single resize
  wave, avoiding SIGWINCH thrash in the panes. A controller showing or hiding its local sidebar
  changes the content width and therefore publishes a new canonical size, reflowing PTYs for the
  whole session. A follower's sidebar never commits or resizes PTYs; its remaining local content
  area simply letterboxes or clips that same canonical canvas.
- **Heartbeat.** The server pings each client and drops one that stops responding (≈15s), releasing
  its lease. Pongs are answered by the transport thread, and time the server itself spends blocked
  in a PTY or filesystem operation is excluded from the deadline. Slow clients that fall too far
  behind are disconnected rather than allowed to stall the broadcast to everyone else.

A UI crash (e.g. `kill -9`) leaves the ephemeral server running with its panes intact, so you can
reattach to it (shown as `ephemeral`) from the picker and recover the scrollback.

> Known limitation: the scratchpad is controller-only in this version (its pane id is shared and
> would collide across clients).

### Collaboration commands and input control

Sharing is driven from ordinary command-palette rows grouped under **Collaboration**. Each appears
only while it would do something, so the group tracks the session rather than listing dead controls:

| Row | Appears when |
| --- | --- |
| *Take / Request layout control* (`request-control`) | you are a writable follower; the verb follows the takeover policy |
| *Grant layout control to requester* (`grant-control`) | you are the controller and a request is pending |
| *Enable / Disable input lock* (`toggle-input-lock`) | you are the writable controller and somebody else is attached |
| *Enable / Disable immediate control takeover* (`toggle-control-takeover`) | you are the writable controller, including alone — it decides what happens to the *next* client |
| *Manage collaborators…* (`collaborators`) | at least one other client is attached |

**Manage collaborators…** is the one that opens a dialog, because it acts on a live list of people
rather than toggling a setting. Your own client and role ride the dialog's top border as a right
header (`razuer #2077 · ctrl`, or `· follow` / `· ro`); each other client is a row carrying its
`ctrl`, `ro`, `parked`, or `wants ctrl` markers. Typing filters the roster.

The query input owns focus, so every action key is a Ctrl chord — a bare letter belongs to the
filter. The keys act on the highlighted client, and each is advertised in the footer only while it
would do something:

- `Enter` — grant it layout control (writable, non-parked targets only)
- `ctrl+d` — decline its pending control request
- `ctrl+k` — **remove** it from the session. This runs on the same arm-then-confirm window as a
  session kill or pane close: the first press strikes the row through (`again to confirm`), a
  second press within a few seconds sends it, and an arming left alone lapses on its own

A filter that hides the highlighted client takes its keys with it: nothing can be granted or removed
that is not on screen.

A removed client is told why, dropped to whatever session it still has (or the launcher), and does
*not* reconnect; the session, its panes, and every other client are untouched. Removing requires a
session server speaking wire protocol 16 or newer — against an older server the key is not offered.

`toggle-input-lock` restricts terminal input to the current controller. The lock follows
the control lease automatically. Clients attached with `--read-only` cannot type, request control,
commit layouts, or receive a grant. These policies are enforced by the session server.

`toggle-control-takeover` enables or disables immediate takeover for the running
session. Only the current writable controller may change it. `[session].allow_takeover` (default
`true`) supplies the initial value when the server starts; changing the config does not retroactively
alter an existing server. An input lock moves with the lease when control is taken.

Turn takeover off for a session shared with another *person*, where a silent takeover would reflow
their panes mid-thought. For a person who should only watch, `--read-only` is stronger than either
setting: such a client can never take or be granted control regardless of the policy.

## Crash recovery and reaping

An **ephemeral** server self-reaps once no client has been attached for a short grace period
(~45s), regardless of pane state - this backstops crashes and abnormal exits so orphaned ephemeral
servers do not accumulate. Normal transitions already tear the ephemeral server down client-side, so
the grace timer rarely fires. The picker's kill action also cleans one up on demand. A **named**
server never self-reaps from client absence: it stays alive until explicitly killed
(`kill-session`, or `Ctrl+K` in the picker).

## Ephemeral session lifecycle

```text
UI start (no target) ── spawn+attach ──▶ ATTACHED-EPHEMERAL(eph-<pid>)
ATTACHED-EPHEMERAL: leave, unused        ⇒ Shutdown ⇒ server exits (held nothing)
                    leave, used          ⇒ prompt: name it (keep) or empty ×2 (close)
                    Rename / name on exit⇒ ATTACHED-NAMED (same server, same panes)
                    attach-elsewhere     ⇒ parked if used, otherwise Shutdown (disposable)
                    kill-session         ⇒ Shutdown ⇒ picker if choices remain, else launcher
                    restart-session      ⇒ Shutdown ⇒ recreate and stay attached
                    UI crash             ⇒ ORPHAN-EPHEMERAL
ORPHAN-EPHEMERAL:   reattach ⇒ ATTACHED-EPHEMERAL
                    no client for grace period ⇒ server exits (any pane state)
                    picker kill / kill-session ⇒ server exits
ATTACHED-NAMED:     leave       ⇒ server keeps running (never self-reaps)
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
versioned JSON metadata plus per-pane terminal replay files. The metadata includes the optional
`created_from_profile` origin. Writes replace the complete snapshot
atomically with private directory (`0700`) and file (`0600`) permissions.

Starting a named server restores its layout, pane commands, working directories, titles, palette,
and saved scrollback. Processes themselves are not checkpointed: each pane command starts again in
a fresh PTY, with the old terminal history replayed above its new output. Missing replay files,
working directories, or individual pane spawn failures do not prevent the rest of the session from
loading. Unsupported or malformed snapshots are left on disk and reported without blocking startup.

Reported pane status is intentionally not written to resurrection snapshots. It survives client
detach/reattach while the same session server remains alive, but is cleared when a dead server is
restarted and its panes are resurrected. Agents should report fresh status after their process
restarts.

`kill-session` and a clean in-protocol session shutdown mean **forget**: they remove the snapshot as
well as stopping the server. A crash, `SIGKILL`, or ordinary detach preserves it for resurrection.

Profiles are reusable launch recipes. `hyprmux dev` attaches to `dev` when running, otherwise loads
`~/.config/hyprmux/profiles/dev.toml` into a fresh canonical session named `dev`; use `hyprmux new
review --profile dev` to create an independently named session from the same recipe. Session
resurrection, local autosave, and `[session] startup = "last"` retain their existing precedence and
lifecycle behavior. See [profiles.md](profiles.md).

## Stale sockets and limits

On attach, a stale socket that cannot complete the attach handshake is removed and a server is
started once. The attach handshake has a timeout so an unresponsive socket does not hang UI startup.

Known limitation: `list-sessions` reports connectable session sockets only; stale or foreign sockets
are skipped so the command does not hang.

Client and server negotiate a session wire protocol version in a supported range (this build speaks
protocol 20 only). After upgrading hyprmux, restart existing named session servers before attaching
when the new client's minimum is higher than the old server's maximum — otherwise attach fails with
a message naming both sides. See [Remote SSH sessions](remote.md#protocol-negotiation).
