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
| **Remote hosts** | Host management | Add a host | — |
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

Because Sessions never contacts a host, it says so rather than implying otherwise. Each remote
group's header carries the host's state — `REMOTE · workbox · disconnected` — and every row on a
host this client holds no attachment to is marked `last seen`, with the pane count the host
reported the last time it answered:

```text
REMOTE · workbox · disconnected
dev@workbox                               3 panes · last seen
```

`Enter` still works on those rows: it connects the host and attaches, which is the point of keeping
them listed. `Ctrl+E` and `Ctrl+K` are withheld, because there is no confirmed live server to
restart or kill. To act on a host's sessions directly, connect it first — `Ctrl+R`, then the host —
or expand it in the Sessions sidebar.

### Browse remote hosts

Press `Ctrl+R` in Sessions to open **Remote hosts**. It is a persistent host manager, not a list of
machines that happen to be reachable: the list combines configured hosts, hosts you added by hand,
recently used hosts, and hosts with a live attachment. Opening or returning to this list is local
and does not contact any machine.

| Key | Remote hosts | Sessions · host |
| --- | --- | --- |
| `Enter` | Connect the selected host, or open it if it is already connected | Attach or switch to the selected session |
| `Ctrl+N` | Add a host | Create a named session on this host |
| `Ctrl+E` | Edit the selected host | Restart the selected session (twice) |
| `Ctrl+R` | Connect the selected host again | — |
| `Ctrl+T` | — | Create or switch to a temporary session on this host |
| `Ctrl+K` twice | Forget the selected host | Kill the selected session |
| `Ctrl+W` | — | Disconnect a retained session attachment |
| `Ctrl+X` | — | Disconnect this client from the host |
| `Esc` | Cancel a connecting probe, otherwise return to Sessions | Return to Remote hosts |

#### Connecting and opening are two steps

`Enter` on a disconnected host contacts it and **leaves you on Remote hosts**. The row changes from
`○` to `●` and reports its session count, and a toast confirms it. Nothing is attached and no shell
is started — reaching a machine says nothing about wanting to work on it yet. A second `Enter` opens
`Sessions · <host>`.

While one host is being contacted its row shows a spinner and `connecting…`, and `Esc` cancels that
probe. The rest of the list stays usable: you can move the highlight, read your other machines, and
edit or forget them. What waits is a *second* connection — `Enter` and `Ctrl+R` are held until the
outstanding one answers, and a host added meanwhile is saved and selected rather than connected.

`rozi --remote <host>` is the exception. A launch that named a machine has already said where it
wants to work, so its first probe goes straight on to that host's sessions.

#### Adding a host

`Ctrl+N` opens the same three lines *Edit host* uses:

```text
› Host       adam@10.0.0.5
  Username   adam
  Port       22
```

The host line is allowed to answer more than its own question. Typing `adam@10.0.0.5` — or
`ssh://adam@workbox:2222` — fills the login in and makes that line read-only, so the two can never
disagree about who logs in; clearing the `user@` hands the line back with whatever you had typed in
it. A login left empty is a real answer, and hands the question to `~/.ssh/config`.

`Tab` and `Shift+Tab` move between the lines, `Enter` saves, `Esc` cancels.

**The host is saved before the connection is attempted, and stays saved however it ends.** A
sleeping laptop, a VPN that is down, or a login typed wrong is not a reason to lose the entry.
Rozi never stores a password: OpenSSH asks for one when it needs one, through its own prompt.

The roster is written to `saved-hosts` in the state directory, which rozi keeps private to you. If
that write still fails, the host is listed and usable for the rest of the session anyway and a
warning names the reason; only the memory of it across restarts is lost.

#### When a connection fails

The row stays, marked `!` in the error colour with a short reason, and a toast carries the same
message:

```text
!  workbox                                       SSH login rejected
```

The failure stays on the row until you retry it, edit it, forget it, or it succeeds. `Enter` retries,
`Ctrl+R` retries a host in any state, `Ctrl+E` corrects it, `Ctrl+K` twice forgets it.

#### Editing and forgetting

`Ctrl+E` opens those lines filled in with what is stored, so a wrong username or a non-default port
is a correction rather than a re-entry. Editing rewrites the entry in place and leaves you on the
list; it does not connect. Rozi deliberately exposes only host, username, and port —
`~/.ssh/config` remains the advanced layer, and it is still what resolves aliases, keys, agents,
and `ProxyJump`.

`Ctrl+K` twice forgets a host rozi owns: one you added, or one it remembers from a past connection.
Configured hosts stay defined by configuration, a host known only through a live attachment goes
when that attachment does, and a host with a live or connecting attachment must be disconnected
first. Forgetting also removes its cached session metadata.

#### Scope

Opening a host never creates or attaches a session, and `[session] startup` does not apply again.
Opening one is an explicit request to work there, so it scopes the launcher to that machine.

That request outlives the overlay. `Esc` steps back to Remote hosts to let you look at the other
machines; it does not withdraw the host you opened, so a client with nothing attached is still
scoped to it once the picker closes. `Ctrl+X` is what leaves a host, and it is offered on
`Sessions · <host>` whenever this client is tied to that machine at all — including when the only
tie is the scope itself.

Switching sessions keeps the old attachment connected in the background. Its screens and
scrollback continue to receive output. A background attachment gives up layout control. Returning
to it takes control when nobody else has claimed it.

An untouched temporary session is discarded when you switch away. A temporary session that has
been used stays available in the background.

## Go to an agent

`a` opens **Agents**: every coding agent Rozi currently knows about, on this machine and on every
[connected host](remote.md#connected-host-monitoring), in one list ordered by what wants attention.
Blocked agents lead, then working ones, then finished, then idle.

```text
Agents
 !  Codex · workbox/backend                              Blocked
 ⠋  Claude #2 · dev                                 Working · 4m
 ✓  Claude · api                                            Done
```

Each row names the agent and where it is: the session on its own for an agent in the session on
screen, `host/session` for one anywhere else. Both halves are searchable, so typing a host name
narrows the list to that machine.

`Enter` goes to the highlighted agent. In the session already on screen that is a focus change.
Anywhere else, Rozi attaches to that session first and then lands on the pane — and on the published
row inside it, for a program running several agents at once. Nothing has to be opened first: the
host need not be showing in a picker, and the session need not be one you have visited. A session
already retained in the background switches in instantly, as it does from the session picker.

Only rows in the session on screen carry an age. A summary from another machine is stamped by that
machine's clock, and presenting the difference between two clocks as a duration would be wrong by
however far they have drifted. What the [Activity tab](sidebar.md#activity) shows about a local
agent — its current activity, project, and branch — likewise stays behind a real attachment; see
[Agents on a machine you are not in](remote.md#agents-on-a-machine-you-are-not-in).

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
- commands running inside a shell pane, including their arguments
- pane titles and terminal palette
- saved terminal history

Processes are not checkpointed. Each command starts again in a fresh PTY, and saved history is
replayed above the new output. Missing directories, missing replay files, and individual spawn
failures do not prevent the rest of the snapshot from loading.

### Commands a pane was running

A pane created with a command, such as `rozi new-pane -- btop` or a pane from a profile, comes back
running it. That command is what the pane is for.

A pane that is a plain shell comes back as a plain shell, but a snapshot also records whatever was
running *in* it. An agent, an editor, a log tail, or a build started by typing at the prompt is
captured with its arguments, and restoring types it back into the new shell. When it exits you are
left at that shell, exactly as you would be had you typed the command yourself.

Only a command genuinely mid-flight is recorded. A pane sitting at a prompt has nothing running,
so nothing is captured, and prompt machinery like directory-jump hooks is never mistaken for work.

Rozi cannot tell `btop` from `terraform apply` by looking at the command, so
`[session] resurrect_foreground` decides how much benefit of the doubt an observed command gets:

| Value | Restoring a shell that was running something |
| --- | --- |
| `auto` (default) | Types the command back and runs it. |
| `hold` | Types the command back and leaves it at the prompt. `Enter` runs it. |
| `never` | Restores the shell and its scrollback. No command is written to the snapshot. |

Set `hold` for a workspace where re-running a command unasked would be worse than typing it again.
The session server loads this setting when it starts. During restore it applies the current value,
so starting the server with `never` also stops an existing snapshot from replaying anything.
`never` omits commands from later snapshots as well. Changing the setting for a running server
takes effect after that server restarts.

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
