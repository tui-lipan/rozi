# Sessions

A session is a set of workspaces and panes that keeps running in a background session server. This
page covers opening, switching, naming, and leaving sessions, what happens at startup, and how rozi
restores sessions after the server stops.

By default, a bare `rozi` opens the session picker without creating or attaching to a session. From
there you choose a running session, restore a saved one, create a named session, or start a
temporary shell. The rozi window is a client of the session server, even for temporary work, which
is why a named session keeps its shells running after you leave. See
[Core concepts](core-concepts.md).

## Temporary and named sessions

| | Temporary | Named |
| --- | --- | --- |
| Typical use | Short work without choosing a name | Work you plan to return to |
| Leaving untouched | Closes | Keeps running |
| Leaving after use | Asks whether to name or close it | Keeps running |
| No clients attached | Closes after 45 seconds | Persists until killed |
| Picker label | `ephemeral` | Session name |
| Reattach after a client crash | During the 45-second recovery window | Until the session is killed |

Naming a temporary session renames its existing server. Panes and processes keep running.

## Open a session from the command line

```bash
rozi                         # open the default picker
rozi dev                     # attach to dev, or launch profile dev
rozi --session dev           # same target, explicit spelling
rozi sessions attach dev              # attach only
rozi sessions attach dev --read-only  # attach without input or layout authority
rozi sessions new dev                 # create a fresh empty named session
rozi sessions new review --profile dev
rozi sessions new api --cwd ~/src/api  # first pane starts in ~/src/api
rozi sessions list
rozi sessions kill dev
```

`rozi dev` attaches to a running session named `dev`. If there is none, it launches the profile
named `dev`. If neither exists, it reports an error; it never creates an empty session with an
unknown name. `rozi sessions kill <NAME>` also reports an error when no live or restorable session
has that name.

`--cwd` applies only to `sessions new` and cannot be combined with `--profile`. With `--remote`, it
names a directory on the remote host and is passed through unchanged, so use an absolute path.

Subcommand names cannot be bare session or profile targets. For a session or profile literally
named `attach`, use `rozi --session attach`.

`rozi sessions list --format json` includes an `origin` object for a session created from a profile
or a Git worktree. A worktree origin holds the checkout path on the session host. Restorable
sessions keep their origin.

Remote hosts use the same session commands. See [Remote sessions](remote.md).

## Use the session picker

Press `Ctrl+A`, then `s` to open **Sessions**, or click the session badge in the workbar.

<CaptureGallery title="rozi">
<img src="./assets/captures/session-picker.webp" alt="The session picker listing the running sessions api, docs, and infra with their pane counts" data-caption="Sessions lists every running and restorable session. Type to filter, Enter to attach, or type a new name and press Ctrl+N.">
</CaptureGallery>

Sessions has a **Local** tab, a tab for each remote host you are using, and **All**, which lists
every host's sessions together and labels remote ones `name@host`. It opens on the host of the
session you are in. To open on **All** every time, choose **Settings → Sessions → Open Sessions on
→ All sessions** or set `session.picker_open_on = "all"`.

Type to filter the selected tab, and press `Enter` to attach. To create a session, type a name and
press `Ctrl+N`; the footer shows which host it will be created on, such as **new on workbox**.
`Ctrl+T` starts a temporary shell, and with an empty list `Enter` does the same. In a temporary
session, `Ctrl+S` names it. `Ctrl+R` opens **Remote hosts**, and `Ctrl+X` disconnects the selected
host.

The marker before each name shows how the session stands with this client:

| Marker | Session |
| --- | --- |
| `●` | The one you are in |
| `◐` | Held by this client in the background |
| spinner | Held by this client and reconnecting |
| `×` | Held by this client, with its connection lost; `Enter` reconnects |
| `○` | Running, with this client not attached |
| `◌` | Saved; `Enter` restores it |
| `·` | Remembered from a host, or not answering |

A session with nothing running to attach to says so first — `Restorable`, `Last seen`, `Busy`, or
`Unavailable`. The rest of the row shows its pane count, what its agents are doing, how many other
clients share it, and the profile it was created from.

Opening Sessions never contacts a remote host. Sessions on remote hosts are listed from the last
time each host answered. A host's tab shows the host's state in the picker's top-right corner, and
rows on a host this client is not attached to are marked `Last seen`, with the pane count the host
last reported:

```text
╭Sessions────────────────────────────────────Disconnected╮
│ Search sessions…                                   1/1 │
├───────────────────────────────────────────────────────┤
│ Local   workbox                                        │
│                                                       │
│ · dev                              Last seen · 3 panes │
```

On a `Last seen` row:

- `Enter` connects the host and attaches to the session.
- `Ctrl+E` is unavailable, because rozi cannot confirm the session is still running.
- `Ctrl+K` twice forgets the entry locally, without contacting the host. If the host still reports
  the session later, it is listed again.

To kill or restart a session on a remote host, connect the host first: press `Ctrl+R`, then connect
the host and open its Sessions tab, or expand it in the Sessions sidebar.

## Scope: where an action happens

Each picker names the scope it acts in, and its keys act only in that scope.

| Surface | Scope | `Ctrl+N` | `Ctrl+T` |
| --- | --- | --- | --- |
| **Sessions** | The browsing host; on All, the foreground or launcher host | New named session on that host | Temporary shell on that host |
| **Remote hosts** | Host management | Add a host | — |

Attached to `backend@workbox`, Sessions opens on the `workbox` tab, so `Ctrl+N` creates a session on
`workbox`. Switch to **Local** to create one on this machine. See
[Manage remote hosts](remote.md#manage-remote-hosts).

## Switch sessions

Switching to another session keeps the previous one connected in the background. Its panes keep
receiving output, so its screens and scrollback are current when you return. A background session
gives up layout control; returning to it takes control again when no other client has claimed it.
`Ctrl+W` in the picker disconnects a background session.

An untouched temporary session is discarded when you switch away. A temporary session you have used
stays in the background.

When you switch sessions, a portal opens onto the incoming one. To use a fade instead or turn the effect off, change
**Settings** › General › Animations › Session switching, or `session` in
[`[animations]`](configuration.md#animations).

## Go to an agent

Press `Ctrl+A`, then `a` to open **Agents**. It lists agents, not every Activity row: detected
coding agents, agents that report their own state, and rows from a publisher that declared itself
an agent. It covers the current session, other running named local sessions, and every
[connected host](remote.md#connected-host-monitoring), with the ones that need you first: agents
waiting for you, then failed ones, then finished runs you have not looked at, then agents still
working, then finished runs you have seen, then idle agents.

```text
Agents
 api   dev   backend@workbox   All
 ▲  fix login · Claude Code          Permission required · dev
 ×  Codex                              Thread failed · backend@workbox
 ●  add filesystem watcher · Claude Code                 dev
 ⠋  review PR · Claude Code                              dev

 Blocked · 2m10s · Permission required · api (fix-login)
```

The picker opens on **All**. Each session with agents also gets its own tab; switch tabs with
`Tab`, `Shift+Tab`, the arrow keys, or a click, and type to filter.

Each row leads with the agent's task when it names one, followed by the agent's name; two rows that
would read the same are numbered. The glyph shows the agent's state. The right side adds the reason
an agent gave for waiting or failing, and, on **All**, where it runs: the session name for local
agents and `session@host` for remote agents. The line below the list describes the highlighted
agent in full: its state, how long it has held or what its run took, the reason, and the project
and branch it works in.

Search matches all of it — the task, the agent, the session, the host, the project and branch, and
the reason — so typing a branch or a host name narrows the list to the agents there.

`Enter` goes to the highlighted agent. In the session on screen, it moves focus to the agent's pane.
Anywhere else, rozi attaches to that session first and then focuses the pane — and, for a program
running several agents, the agent's own row inside it. The host and session do not have to be open
already. A session already in the background switches in immediately.

Only agents in the session on screen have a time. What an agent's terminal shows, as the
[Activity tab](sidebar.md#activity) reads it, needs an attached session; see
[Agents on a machine you are not in](remote.md#agents-on-a-machine-you-are-not-in).

To keep the agents that need you on screen while you work, show the sidebar's
[Agents tab](sidebar.md#agents).

## Name or rename a session

Press `Ctrl+A`, then `S` to run **Name session** or **Rename session**. The same server, panes,
processes, and scrollback continue under the new name.

rozi rejects a name already used by a running session and names reserved for temporary sessions.

A profile is a launch recipe; a session is what is running. A session created from a profile
records that profile as its origin, but later edits to the profile do not change the running
session. See [Profiles](profiles.md).

## Leave rozi

`Ctrl+A`, then `q`, and `Ctrl+A`, then `d` run the same leave flow:

- Named sessions detach and keep running, including named sessions in the background.
- Untouched temporary sessions close without a prompt.
- Used temporary sessions open **Keep this session?**. Enter a name to keep the session running,
  submit an empty name twice to close it, or press `Esc` to go back.

Set `[confirm] quit_ephemeral = false` to close a used temporary session on the first empty name.

`rozi run-action quit` does not prompt and does not close a used temporary session. The temporary
server is left running so you can [recover it](#recover-a-temporary-session).

Killing the current session does not exit the client. rozi opens the picker if another session is
available, or returns to the sessionless launcher. Killing a named session also deletes its
[resurrection](#resurrection) snapshot.

## Choose startup behavior

`[session] startup` decides where rozi starts when you do not name a session:

| Value | Behavior |
| --- | --- |
| `picker` | Open the picker without attaching. This is the default. |
| `ephemeral` | Start a temporary shell immediately. |
| `last` | Reopen the most recently attached named session. |
| `profile` | Open the session named by `[profile] default`. |

A bare `rozi` applies this setting locally. `rozi --remote workbox` applies it on `workbox`:

| Value | `rozi --remote workbox` |
| --- | --- |
| `picker` | Connect, list sessions, and open **Sessions** on the **workbox** tab. No session is created. |
| `ephemeral` | Create or attach a temporary session on `workbox`. |
| `last` | Attach the last session used on `workbox` if it is still there, else **Sessions** on the **workbox** tab. |
| `profile` | Open or create the default-profile session on `workbox`, else **Sessions** on the **workbox** tab. |

`last` is remembered separately for each host. It only reattaches a session that is still running;
it never restores or creates one. On a remote host, rozi opens **Sessions** on the host’s tab and
attaches the remembered session only if the host still lists it, without waiting on SSH before
drawing the first frame. `profile`, by contrast, creates its session when needed.

A session you name explicitly — as a target, with `sessions attach` or `sessions new`, or through
`--pick` — overrides `[session] startup`. If `last` or `profile` cannot find its session, rozi opens
that host's picker and reports why.

### The sessionless launcher

When no session is attached, rozi shows the sessionless launcher. Press `Enter` to start a temporary
shell, or use the spawn command.

A launcher can be scoped to a remote host without a session or an open SSH connection there:

```text
╭─REMOTE · workbox──────────────────────────╮
│ Enter / Ctrl+A Enter  shell on workbox    │
│ Ctrl+A s              pick a session      │
│ Ctrl+A q / Ctrl+A d   leave               │
╰───────────────────────────────────────────╯
```

You land here when you dismiss the picker after `rozi --remote workbox` with `startup = "picker"`,
or close **Sessions** on the **workbox** tab with nothing attached. `Enter` then starts a temporary
shell on `workbox`. Sessions opened from this launcher starts on the `workbox` tab, and switching
tabs there moves the launcher's scope with it.

The scope follows the session you work in, so killing a remote session leaves you in that host's
launcher. Opening another host, switching tabs in Sessions, disconnecting the host with `Ctrl+X`,
or forgetting it changes the scope. Browsing the host list or closing a picker without choosing does
not.

## Recover a temporary session

If the client crashes or disconnects abnormally, a temporary session server waits 45 seconds after
its last client leaves. During that time, it appears as `ephemeral` in the picker and you can
reattach to it. After 45 seconds with no client, it shuts down, even if panes were still running.

Named sessions have no such timer. They persist until you kill them.

## Resurrection

With `[session] resurrect = true`, the default, rozi saves a snapshot of each named session
periodically and when the last client detaches after a change.

When the session server is no longer running, the picker lists a usable snapshot as `Restorable`.
Restoring recreates:

- workspace layouts, names, and pane placement
- pane commands and working directories
- commands running inside a shell pane, including their arguments
- pane titles and terminal palette
- saved terminal history, including its colors and text styling

Processes themselves are not saved. Each command starts again in a fresh terminal, with the saved
history shown above the new output. A missing directory, a missing history file, or a command that
fails to start affects only its own pane; the rest of the snapshot still loads.

To discard a snapshot, press `Ctrl+K` twice on its `restorable` row. `rozi sessions new <name>` also
starts fresh instead of restoring a snapshot with that name.

### Commands a pane was running

A pane created with a command, such as `rozi split -- btop` or a pane from a profile, restores by
running that command again.

A plain shell pane restores as a shell. If a command was running in it when the snapshot was taken —
an agent, an editor, a log tail, or a build typed at the prompt — rozi records the command with its
arguments and types it back into the new shell. When the command exits, you are left at the shell.
A pane sitting at a prompt records nothing, and prompt hooks such as directory jumpers are not
recorded as commands.

Commands launched through a Node wrapper script, such as Cursor's `agent`, restore with their
arguments too. Commands under other interpreters restore by name only.

`[session] resurrect_foreground` decides what happens to a recorded command:

| Value | Restoring a shell that was running something |
| --- | --- |
| `auto` | Types the command back and runs it. |
| `hold` (default) | Types the command back and leaves it at the prompt. `Enter` runs it. |
| `never` | Restores the shell and its scrollback. No command is written to the snapshot. |

The default, `hold`, leaves the command ready for you to confirm with `Enter`. Set `auto` to run
recorded commands automatically. A running session keeps the value it started with; restart it to
apply a change.

### Reopen an agent conversation

An agent that reports a native session reference (`rozi agents report --native-session`), and whose
definition declares `[agents.resume]`, restores into its previous conversation instead of starting a
new one.

The resume command comes from the agent's current definition, so a change to its resume flags
applies to sessions saved earlier.

A pane restores the first of these that applies: a conversation to reopen, a command it was
running, the command it was created with, or a plain shell. Only one pane reopens a given
conversation.

`resurrect_foreground` applies here too:

| Value | Restoring a pane with a reported conversation |
| --- | --- |
| `auto` | Runs the resume command as the pane's first process. |
| `hold` (default) | Types the resume command at the shell's prompt and leaves it there. When the agent declares an [`attach`](agents.md#declare-native-resume-support) command, as Claude Code does for a session still running in the background, the line falls back to it. |
| `never` | Writes no conversation reference to the snapshot at all. |

If the resume command fails, the pane keeps the agent's error on screen, names the failure, and
leaves a usable shell below it. rozi never starts a new conversation in place of one it could not
reopen. From then on, that pane is saved and restored as a plain shell. A pane whose command simply
exited still restores by running the command again.

Set `[session] resurrect_agents = false` to keep agent session references out of snapshots. It
defaults to `true`. Only references reported by a live agent integration are saved, never values
read from terminal output.

## Scratch panes

The scratchpad belongs to the client, not to the attached session. Its panes run on a private
session server that lasts as long as the client, so scratch panes stay available while you switch
between local and remote sessions.

Scratch panes do not appear as a session. They are not shared with collaborators, saved in
profiles, or included in resurrection snapshots. Exiting rozi shuts down their server.

## Script a session with no client attached

A named session can be driven while no client is attached. Put a control command after
`rozi --session <NAME>`:

```bash
rozi --session dev list-panes
rozi --session dev capture-pane --target 3
rozi --session dev send-keys --target 3 'cargo test' Enter
rozi --session dev split --workspace 9 --argv cargo watch -x test
```

This does not attach a client, so the session's client count and layout control are unchanged. A
pane opened this way is already in place when a client attaches.

A script has no more authority than an attached client. `split` is refused while any client holds
layout control, and typing respects the session's input lock. Commands that need a screen — focus,
workspace switching, toasts, pickers, actions, and event subscriptions — are refused with a reason.
See [Control CLI](control.md#two-endpoints) for the full list and
[Automation recipes](recipes.md) for examples.

## Worktrees

rozi can open each Git worktree of a repository in its own named session, and create or remove
checkouts from the **Worktrees** picker or `rozi worktrees`. See [Worktrees](worktrees.md).

## Share a live session

More than one client can attach to a session. One client controls the shared layout, while each
client keeps its own focus, scrollback, overlays, theme, and sidebar. See
[Shared sessions](shared-sessions.md) for handing over control, read-only attachment, input lock,
and removing collaborators.

## Limits and failure cases

- `rozi sessions list` lists sessions it can connect to. Stale or unrecognized session entries are
  skipped.
- If a session server cannot be reached, the client reports the failure. It does not create a blank
  session with that name.
- If a client loses its connection to a local session, it reconnects in place, retrying a busy
  server for up to 15 seconds. If that fails, the client leaves the session and opens the picker or
  the launcher. The server and its panes keep running if the server is still alive.
- If a remote session's connection drops or stops answering, a reconnecting overlay retries for up
  to two minutes; `Esc` cancels it. rozi never silently replaces a remote session that is gone. See
  [Reconnection and switching](remote.md#reconnection-and-switching).
- A session server and client must be compatible. After upgrading rozi, restart an incompatible
  named session or update the other end.
- Restarting a session kills its processes and starts fresh panes. It is not the same as detaching
  and reattaching.
- A session name belongs to one host. The same name on the local machine and on a remote host
  refers to two different sessions.

See also [Troubleshooting](troubleshooting.md).

## Keep the system awake

Enable **Settings → Sessions → Keep awake while agents work**, or set
`[session] keep_awake_while_agents_work = true`. The default is disabled. Each session
server keeps its host awake while any agent is Working, including agents in local panes
and sessions whose UI has detached. Agents are the ones listed by `rozi agents list`: detected
agents, [agent reports](agents.md#report-state-from-agent-hooks), and rows from a publisher that
declared itself an agent. A `rozi status` or a generic published row is an activity and does not
keep the host awake. Blocked, Idle, Done, exited panes, and no agents
allow sleep after a five-second grace period. Working again during that grace period
keeps the existing lock. Disabling the setting releases it immediately.

Screen blanking and locking remain allowed, and this setting adds no lid-switch lock.
Linux requires `systemd-inhibit` and logind and blocks sleep requests while held.
Older systemd helpers are supported; authentication prompts are disabled when the
installed helper supports that option.
macOS uses `caffeinate` to prevent idle system sleep; Windows uses a system power
request to prevent automatic sleep. Explicit sleep and lid handling remain controlled
by the OS on macOS and Windows. Unsupported hosts allow sleep and log an error.

Saving the setting or reloading config asks attached servers to read their own host
config; only a writable controller can do this. Remote servers use the remote host's
setting. While attached remotely, this Settings row is disabled and shows
**Configure on remote host**; edit the remote host's config and reload it instead.
Detached servers pick it up at their next controller reload or restart.
If acquisition fails, the server allows sleep and logs once until the next working
epoch rather than retrying continuously.
