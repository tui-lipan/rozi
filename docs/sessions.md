# Sessions

By default, a bare `rozi` opens the session picker without creating or attaching to a session.
Choose a running session, restore one, create a named session, or start a temporary shell.

Every PTY belongs to a session server. The UI is a client of that server, even for temporary work.
This is why named sessions can keep shells running after you leave.

## Temporary and named sessions

| | Temporary | Named |
| --- | --- | --- |
| Typical use | Short work without choosing a name | Work you plan to return to |
| Leaving untouched | Closes | Keeps running |
| Leaving after use | Asks whether to name or close it | Keeps running |
| No clients attached | Closes after 45 seconds | Persists until killed |
| Picker label | `ephemeral` | Session name |
| Reattach after a client crash | During the 45-second recovery window | Until the session is killed |

Naming a temporary session renames its existing server. It does not move panes or restart
processes.

## Open a session from the command line

```bash
rozi                         # open the default picker
rozi dev                     # attach to dev, or launch profile dev
rozi --session dev           # same target, explicit spelling
rozi sessions attach dev              # attach only
rozi sessions attach dev --read-only  # attach without input or layout authority
rozi sessions new dev                 # create a fresh empty named session
rozi sessions new review --profile dev
rozi sessions list
rozi sessions kill dev
```

`rozi dev` first looks for a running session named `dev`. If none exists, it launches the
same-name profile. It reports an error when neither exists. It never creates an unknown empty
session silently. `rozi sessions kill <NAME>` also reports an error when no live or restorable
session has that name.

Namespace names and retired CLI spellings cannot be bare session or profile targets. Use
`rozi --session attach` when the intended session or profile is literally named `attach`.

Remote targets use the same session commands. See [Remote sessions](remote.md).

## Scope: where an action happens

Every surface names the scope it acts in, and its keys act only in that scope.

| Surface | Scope | `Ctrl+N` | `Ctrl+T` |
| --- | --- | --- | --- |
| **Sessions** | Global — every host at once | New local named session | Local temporary shell |
| **Remote hosts** | Host navigation | Connect a new host | — |
| **Sessions · host** | That one host | New named session on the host | Temporary session on the host |

Sessions stays global even while a remote session fills the screen behind it. Attached to
`backend@workbox`, `Ctrl+N` there still creates a *local* session; the footer reads `new local`
whenever a remote host is in play, so the key says what it does before you press it. To create
another session on `workbox`, go through its own surface: `Ctrl+R`, the host, then `Ctrl+N`.

## Use the session picker

Open **Sessions** with the `s` command key.

| Key | Action |
| --- | --- |
| `Enter` | Connect, switch to a background attachment, or restore a snapshot |
| Type a name, then `Ctrl+N` | Create and switch to a local named session |
| `Ctrl+K` twice | Kill a live session, or forget a snapshot |
| `Ctrl+E` twice | Restart a live session with fresh panes |
| `Ctrl+W` | Disconnect this client from a background session |
| `Ctrl+X` | Disconnect a remote host |
| `Ctrl+R` | Open Remote hosts |
| `Ctrl+T` | Open or switch to this client's local temporary shell |
| `Esc` | Return to the sessionless launcher |

The picker updates local session state while it is open. A row can show whether a session is
attached in the background, shared with other clients, restorable, or created from a profile.
Opening Sessions does not contact configured remote hosts. Remote sessions already known from the
last successful host discovery remain available from cache.

### Browse remote hosts

Press `Ctrl+R` in Sessions to open **Remote hosts**. The list combines configured hosts, recently
used hosts, and hosts with a live attachment. Opening or returning to this list is local and does
not contact any machine.

| Key | Remote hosts | Sessions · host |
| --- | --- | --- |
| `Enter` | Discover and browse the selected host | Attach or switch to the selected session |
| `Ctrl+N` | Enter a new SSH target | Create a named session on this host |
| `Ctrl+T` | — | Create or switch to a temporary session on this host |
| `Ctrl+K` twice | Forget an offline Recent host | Kill the selected session |
| `Ctrl+E` twice | — | Restart the selected session |
| `Ctrl+W` | — | Disconnect a retained session attachment |
| `Ctrl+X` | — | Disconnect this client from the host |
| `Esc` | Cancel a connecting probe, otherwise return to Sessions | Return to Remote hosts |

Activating a host probes it in place. The host row shows a spinner and `connecting…`; navigation
is locked and `Esc` cancels. A successful result opens that host's sessions and replaces the
cache. If the host is unreachable, the row stays on Remote hosts with the failure state. A
successfully reached new host is remembered even when it has no sessions; a failed target is not
remembered. OpenSSH remains responsible for aliases, keys, agents, and `ProxyJump`.

Opening a host never creates or attaches a session, and `[session] startup` does not apply again.
Browsing a host is an explicit request to look at it, so it always lands on that host's launcher.
The same is true of `Ctrl+N` on Remote hosts: a new target that discovers successfully opens
`Sessions · <host>` and waits.

That request outlives the overlay. `Esc` steps back to Remote hosts to let you look at the other
machines; it does not withdraw the host you opened, so a client with nothing attached is still
scoped to it once the picker closes. `Ctrl+X` is what leaves a host, and it is offered on
`Sessions · <host>` whenever this client is tied to that machine at all — including when the only
tie is the scope itself.

Only offline Recent hosts can be forgotten. Configured hosts remain defined by configuration, and
a host with a live or connecting attachment must be disconnected first. Forgetting also removes
its cached session metadata.

Switching sessions keeps the old attachment connected in the background. Its screens and
scrollback continue to receive output. A background attachment gives up layout control. Returning
to it takes control when nobody else has claimed it.

An untouched temporary session is discarded when you switch away. A temporary session that has
been used stays available in the background.

## Choose startup behavior

`[session] startup` decides where a launch lands when you name no session:

| Value | Behavior |
| --- | --- |
| `picker` | Open the picker without attaching. This is the default. |
| `ephemeral` | Start a temporary shell immediately. |
| `last` | Reopen the most recently attached named session. |
| `profile` | Open the session named by `[profile] default`. |

The policy runs in the scope the launch names. A bare `rozi` applies it locally;
`rozi --remote workbox` applies the same four values on `workbox`:

| Value | `rozi --remote workbox` |
| --- | --- |
| `picker` | Connect, discover, and open `Sessions · workbox`. No session is created. |
| `ephemeral` | Create or attach a temporary session on `workbox`. |
| `last` | Attach the last session used on `workbox` if it is still there, else `Sessions · workbox`. |
| `profile` | Open or create the default-profile session on `workbox`, else `Sessions · workbox`. |

`last` is remembered per host, so a local launch never reaches for a name that only exists on
`workbox`, and the reverse.

`last` reopens a session; it never revives one. On a remote host the launch opens
`Sessions · workbox` and attaches the remembered session only if the host's own discovery still
lists it, so a session killed while Rozi was away stays dead and you land on the picker. Nothing
blocks on SSH before the first frame. `profile` does create its session — that is the difference
between the two modes.

Explicit session targets, `sessions attach`, `sessions new`, and `--pick` take precedence: a
session you name is always the one you get. If `last` or `profile` cannot resolve its requested
session, Rozi falls back to that scope's picker and reports why.

From the sessionless launcher, bare `Enter` starts a temporary shell. The configured spawn command
also works there.

### The sessionless launcher has a scope

A launcher can be scoped to a host without holding a session or an SSH connection to it:

```text
REMOTE · workbox
Not attached. A shell starts on workbox.
```

That is where `rozi --remote workbox` lands under `startup = "picker"` once you dismiss the picker,
and where dismissing `Sessions · workbox` leaves a client with nothing attached. `Enter` there
starts a temporary shell on `workbox`. Opening Sessions from it is still global.

The scope follows the session you are working in, so killing a session leaves you in that
machine's launcher rather than silently back on this one. Three things change it: opening another
host, disconnecting this one (`Ctrl+X`), and forgetting it. Browsing the host list, or closing the
picker without choosing anything, leaves it where it is.

## Name or rename a session

Use **Name session** or **Rename session**, with the default `Shift+S` command key. The same server,
panes, processes, and scrollback continue under the new name.

Rozi rejects names already used by a running session and names reserved for temporary servers.

A profile and a session are separate. Profiles are launch recipes. Sessions are live server-owned
PTYs. Creating a session from a profile records its origin when available, but later profile edits
do not change the running session. See [Profiles](profiles.md).

## Leave Rozi

The `q` and `d` command keys run the same leave flow.

- Named sessions detach and keep running, including named sessions connected in the background.
- Untouched temporary sessions close without a prompt.
- Used temporary sessions open **Keep this session?**. Enter a name to keep one running, submit an
  empty name twice to close it, or press `Esc` to return.

Set `[confirm] quit_ephemeral = false` to remove the second empty-name confirmation.

`rozi run-action quit` does not prompt and does not close a used temporary session. Automation
cannot answer the naming prompt, so the temporary server is left for recovery.

Killing the current session does not exit the client. Rozi opens the picker if another useful
choice remains, or returns to the sessionless launcher. Killing a named session also removes its
resurrection snapshot.

## Recover a temporary session

If the UI crashes or disconnects abnormally, a temporary server waits 45 seconds after its last
client leaves. During that window, it appears as `ephemeral` in the picker and can be reattached.
After 45 seconds with no client, it shuts down even if panes were running.

Named servers do not use this timer. They persist until explicitly killed.

## Resurrection

With `[session] resurrect = true`, which is the default, Rozi snapshots named sessions
periodically and after the last client detaches following a change.

When the server no longer exists, the picker lists a usable snapshot as `restorable`. Restoring
recreates:

- workspace layouts, names, and pane placement
- pane commands and working directories
- pane titles and terminal palette
- saved terminal history

Processes are not checkpointed. Each command starts again in a fresh PTY, and saved history is
replayed above the new output. Missing directories, missing replay files, and individual spawn
failures do not prevent the rest of the snapshot from loading.

Use `Ctrl+K` twice on a restorable row to forget the snapshot. Explicit
`rozi sessions new <name>` also starts fresh rather than restoring an old snapshot with that name.

## Scratch panes

The scratchpad is client-owned, not part of the attached session. Its PTYs run on one private
session server for the lifetime of the UI client. Scratch panes therefore survive switching among
local and remote sessions.

Scratch panes are not discoverable as a normal session. They are not shared with collaborators,
saved in profiles, or included in resurrection snapshots. Exiting the UI shuts down their private
server.

## Share a live session

More than one client can attach to a session. One client controls the shared layout while followers
keep local focus, scrollback, overlays, theme, and sidebar state. Read
[Shared sessions](shared-sessions.md) for control transfer, read-only attachment, input locking, and
collaborator removal.

## Limits and failure cases

- `sessions list` lists connectable sessions. Stale or foreign endpoints are skipped.
- If a server cannot be contacted, the client reports the failure instead of inventing a blank
  named session.
- A named server and client must be compatible. After upgrading Rozi, restart an incompatible
  server or update the other end.
- A restart kills the session's processes and starts fresh panes. It is not the same as detaching
  and reattaching.
- A session name belongs to one host. The same spelling on local and remote hosts identifies
  different sessions.
