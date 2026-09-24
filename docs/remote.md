# Remote sessions

`--remote` runs your sessions on another machine over SSH while the rozi window stays on your own
computer. The shells and programs in the panes run on the remote host; the theme, keybindings,
overlays, and clipboard stay local. This page covers connecting, managing hosts, SSH setup,
installing rozi on the host, and what happens when a connection drops.

## Connect

```bash
rozi --remote workbox
rozi --remote workbox dev
rozi --remote ssh://user@host:2222
rozi --remote workbox sessions attach dev
rozi --remote workbox sessions new review
rozi sessions list --remote workbox
rozi sessions kill dev --remote workbox
```

Session commands behave as they do locally: a named target attaches to its running session or
launches the profile of the same name. See
[Sessions](sessions.md#open-a-session-from-the-command-line). The `rozi worktrees` commands also
accept `--remote`; see [Worktrees on a remote host](worktrees.md#worktrees-on-a-remote-host).

A bare `rozi --remote workbox` names no session, so `[session] startup` decides what happens, applied
on `workbox`. Under the default, `picker`, rozi connects, lists the host's sessions, and creates
nothing. See [Choose startup behavior](sessions.md#choose-startup-behavior) for the other values.

rozi supports Linux, macOS, and Windows as both client and remote host. The local machine needs
`ssh` on `PATH`; installing rozi automatically onto a Windows host also needs `scp`. rozi downloads
and verifies release files itself, so installing does not need `curl`, `tar`, or `unzip`.

For a host you use regularly, add it in [Remote hosts](#manage-remote-hosts) instead of typing it
each time. Use [`[remote.hosts.*]`](#configure-aliases-and-defaults) for a host that needs more
settings, such as an identity file, extra `ssh` arguments, or a fixed binary path.

## Manage remote hosts

Open **Sessions** (`Ctrl+A`, then `s`) and press `Ctrl+R` to open **Remote hosts**. It lists
configured hosts, hosts you added, recently used hosts, and hosts with a live attachment. Opening or
returning to this list does not contact any machine.

`Enter` on a connected host opens `Sessions · <host>`, the list of sessions on that host.

| Key | Remote hosts | Sessions · host |
| --- | --- | --- |
| `Enter` | Connect the selected host, or open it if it is already connected | Attach or switch to the selected session |
| `Ctrl+N` | Add a host | Create a named session on this host |
| `Ctrl+E` | Edit the selected host | Restart the selected session (twice) |
| `Ctrl+R` | Connect the selected host again | — |
| `Ctrl+T` | — | Create or switch to a temporary session on this host |
| `Ctrl+K` twice | Forget the selected host | Kill a live session, or forget a `last seen` entry |
| `Ctrl+W` | — | Disconnect a background session attachment |
| `Ctrl+X` | — | Disconnect this client from the host |
| `Esc` | Cancel a connection in progress, otherwise return to Sessions | Return to Remote hosts |

### Connect and open a host

Connecting and opening are separate steps:

1. Press `Enter` on a disconnected host. rozi contacts it and stays on **Remote hosts**. The row
   changes from `○` to `●` and shows the session count, and a toast confirms the connection. No
   session is attached and no shell starts.
2. Press `Enter` again to open `Sessions · <host>`.

While a host is connecting, its row shows a spinner and `connecting…`, and `Esc` cancels the
attempt. Only one connection runs at a time: `Enter` and `Ctrl+R` wait until it finishes, and a host
added in the meantime is saved and selected but not connected. You can still move through the list
and edit or forget other hosts.

`rozi --remote <host>` skips the extra step and opens the host's sessions as soon as it connects.

### Add a host

Press `Ctrl+N`. The form has the same three fields as the host editor:

```text
› Host       adam@10.0.0.5
  Username   adam
  Port       22
```

Typing a login in the host field, as in `adam@10.0.0.5` or `ssh://adam@workbox:2222`, fills in
**Username** and makes it read-only. Delete the `user@` part to edit **Username** again. Leave
**Username** empty to use the login from `~/.ssh/config`.

`Tab` and `Shift+Tab` move between fields, `Enter` saves, and `Esc` cancels.

rozi saves the host before it tries to connect, and keeps it whether or not the connection
succeeds. rozi never stores passwords; OpenSSH asks for one when needed.

Added hosts are saved to `saved-hosts` in the state directory, which only you can read. If saving
fails, the host is still usable until rozi exits, and a warning explains why.

### When a connection fails

The row stays in the list, marked `!` in the error colour with a short reason, and a toast shows the
same message:

```text
!  workbox                                       SSH login rejected
```

The error stays on the row until the host connects or you retry, edit, or forget it. `Enter` or
`Ctrl+R` retries, `Ctrl+E` edits the host, and `Ctrl+K` twice forgets it. See
[Troubleshooting](#troubleshooting) for what each message means.

### Edit or forget a host

`Ctrl+E` opens the host editor with the saved host, username, and port. Saving updates the entry in
place and returns to the list without connecting. For aliases, keys, SSH agents, `ProxyJump`, and
other advanced options, use `~/.ssh/config`.

`Ctrl+K` twice forgets a host you added or one rozi remembers from a past connection. Forgetting
also removes the host's cached session list. Some hosts cannot be forgotten here:

- A configured host stays as long as it is in the configuration.
- A host known only through a live attachment disappears when that attachment ends.
- A host with a live or connecting attachment must be disconnected first.

## Work on a host without a session

Opening a host scopes the launcher to that machine. Opening it never creates or attaches a session,
and `[session] startup` does not run again. A client can stay scoped to a host while holding no
session there:

```text
REMOTE · workbox
Not attached. A shell starts on workbox.
```

This is a normal state, not an error, and it does not mean an SSH connection is open. `Enter`
starts a temporary shell on `workbox`. You land here after `rozi --remote workbox` under
`startup = "picker"` once you dismiss the picker, and when you close `Sessions · workbox` with
nothing attached.

Pressing `Esc` in `Sessions · <host>` to go back to **Remote hosts** keeps the scope. To leave the
host, press `Ctrl+X` in `Sessions · <host>`; it is available whenever this client is tied to that
host, including by scope alone.

**Sessions** opened from this state is still global: its `Ctrl+N` and `Ctrl+T` create local
sessions. See [Scope](sessions.md#scope-where-an-action-happens) and
[The sessionless launcher](sessions.md#the-sessionless-launcher).

## Set up SSH authentication

rozi uses your OpenSSH configuration, keys, agent, `known_hosts`, jump hosts, and other SSH policy.
It does not manage credentials.

By default, SSH runs in batch mode, so authentication must complete without a prompt. Load a key
into your SSH agent or set an identity file:

```toml
[remote.hosts.workbox]
host = "workbox.example.com"
user = "dev"
identity_file = "~/.ssh/id_ed25519"
```

Set `[remote] batch_mode = false` to allow SSH prompts when probing, installing, attaching, listing,
and killing sessions. A loaded agent is still the better choice for regular use.

Test authentication directly when setup fails:

```bash
ssh workbox
```

### Prompts inside the UI

With `batch_mode = false`, a running rozi client shows SSH prompts in a dialog instead of letting
them reach the terminal:

- A password or key passphrase is masked and never shown.
- A host-key question shows OpenSSH's wording as is and has a text field, because OpenSSH accepts
  the fingerprint itself as an answer as well as `yes` and `no`. What you type is not masked. The
  full fingerprint is repeated on its own line below the question so you can compare it without a
  line break in the middle.
- Any other yes-or-no question from SSH offers `Yes` and `No`. `←` and `→` move between them, and
  `Enter` or a click chooses one.
- A rejected password is reported on the prompt when SSH asks again.
- `Esc` cancels the connection attempt. SSH asks up to three times per connection, so `Esc` also
  declines the remaining attempts and gives up on that host connection. Activating the host again
  starts a new connection and prompts normally.
- A prompt left unanswered for five minutes fails its connection and closes.

To do this, rozi sets `SSH_ASKPASS`, `SSH_ASKPASS_REQUIRE=force`, and its own variables on every
`ssh` and `scp` it runs, overriding a desktop `SSH_ASKPASS` for those commands only. This needs
OpenSSH 8.4 or newer on the client; older versions prompt on the terminal, and the prompt draws
over the rozi window.

Command-line runs keep SSH's ordinary terminal prompt: `rozi sessions list --remote`,
`rozi sessions kill --remote`, and the install prompt shown before launch.

## Configure aliases and defaults

`--remote` accepts an SSH config alias, a bare hostname, or an `ssh://[user@]host[:port]` URL.

```toml
[remote]
default_host = "workbox"
install = "prompt"

[remote.hosts.workbox]
host = "workbox.example.com"
user = "dev"
port = 2222
identity_file = "~/.ssh/id_ed25519"
ssh_args = ["-J", "bastion"]

[remote.hosts.staging]
host = "staging.example.com"
```

With `default_host` set, `rozi --remote` with no host uses that target. The default host's entry
also supplies `user`, `port`, `identity_file`, `ssh_args`, and `binary_path` to other aliases that
do not set them. A value set on the alias wins, and a non-empty `ssh_args` list on the alias
replaces the inherited list.

For an `ssh://` URL, the user and port in the URL win. Other settings come from a host entry with
the same hostname, then from the default host's entry.

Use `binary_path` when rozi is already installed at a fixed path on the host:

```toml
[remote.hosts.workbox]
binary_path = "/opt/rozi/bin/rozi"
```

## Shared SSH connections

Opening a host runs several SSH commands in a row. On Linux and macOS, they share one connection,
so you authenticate once per host: a password host asks for the password a single time, and later
commands skip the handshake.

For its own commands, rozi passes `ControlMaster=auto`, a `ControlPath` in its runtime directory,
and `ControlPersist=60`, overriding any `ControlMaster` or `ControlPath` in your SSH config. The
shared connection closes about a minute after the last command using it finishes.
`ServerAliveInterval` and `ServerAliveCountMax` apply to it, because a shared connection takes its
settings from the command that opened it.

The shared connection is per user, not per client, so two rozi clients on the same machine share
one connection per host. For that reason, disconnecting from a host in rozi does not close the
shared connection: closing it would also end another client's sessions on that host.

Windows clients do not share SSH connections and authenticate for each command.

## Choose an install policy

Before connecting, rozi checks the host for a compatible rozi binary, whether you open the host
inside rozi or start with `rozi --remote <host>`. When rozi needs to install one, a dialog shows
the host, install location, and version, with `Install` and `Cancel`; `Esc` also cancels. The
connection continues after installation.

| `[remote] install` | TUI connection or interactive terminal | Non-interactive run |
| --- | --- | --- |
| `prompt` | Ask before installing. This is the default. | Fail without changing the host. |
| `always` | Install when needed without asking. | Fail without changing the host. |
| `never` | Fail when no compatible binary is found. | Fail without changing the host. |

Background discovery and monitoring never install anything; installation happens only when you
connect to a host or session. Set `install = "never"` or pin `binary_path` if installing on the
host is not acceptable.

### Where rozi looks for an existing binary

On POSIX hosts, rozi checks `PATH` and common install locations: `~/.local/bin`, `~/.cargo/bin`,
`~/bin`, `~/.nix-profile/bin`, `/opt/homebrew/bin`, `/usr/local/bin`, and `/usr/bin`. On Windows
hosts, it checks `PATH`, `%USERPROFILE%\.local\bin`, and `%USERPROFILE%\.cargo\bin`. After these,
it checks its own managed install directory. Listing, attaching, monitoring, and killing sessions
all use the path found, so the host's non-interactive SSH `PATH` does not need these directories.
Use `binary_path` for an installation anywhere else. Found paths are cached briefly and checked
again after a remote command fails.

### Where rozi installs

Automatic installation never replaces a rozi you installed yourself. It installs a copy matching the
client's exact version, so several client versions can coexist:

| Host | Install path |
| --- | --- |
| POSIX, `XDG_DATA_HOME` set to an absolute path | `$XDG_DATA_HOME/rozi/remote/<version>/rozi` |
| POSIX, otherwise | `$HOME/.local/share/rozi/remote/<version>/rozi` |
| Windows | `%LOCALAPPDATA%\rozi\remote\<version>\rozi.exe` |
| Windows, `LOCALAPPDATA` unavailable | `%USERPROFILE%\.local\share\rozi\remote\<version>\rozi.exe` |

When the client and host run the same platform, rozi can copy its own executable. Otherwise, it
downloads the signed release manifest and the matching archive for the client's version and the
host's platform. The manifest is verified against a signing key built into rozi and checked for
expiry, and it is used to verify the archive and binary. A release server or
`ROZI_RELEASE_BASE_URL` mirror is only a download location; it cannot sign releases.

rozi uploads to a temporary path and checks that the binary runs and is compatible before moving it
into place. If the check fails, rozi removes the temporary file and leaves any existing install
untouched. rozi refuses to overwrite an install target that is not a regular file.

For development or CI:

- `ROZI_REMOTE_BINARY` uploads a specific local binary. It overrides `[remote] install`, including
  `install = "never"`, and uploads without asking.
- `ROZI_RELEASE_BASE_URL` points to an HTTPS directory containing the version's
  `rozi-release.json`, `rozi-release.signatures.json`, and release archives.

### Version compatibility

The client and the session server must speak the same session protocol version. This release
supports only one, so after an update that changes it, update both ends and restart any running
named sessions on the host. Optional features are agreed per connection: when one side lacks a
feature, only that feature is unavailable. A client update therefore does not always require
restarting a remote session and the processes in it. rozi may reject a configured or discovered
binary before attaching when it can tell the versions do not match.

## Understand the client and server boundary

| Local client | Remote server |
| --- | --- |
| Theme, keybindings, overlays, and sidebar UI | Terminals and pane processes |
| Copy, search, and hint interfaces | Shared layout authority |
| Local control socket | Session discovery and resurrection |
| Hooks and desktop notifications | Pane working directories |
| Local clipboard access | Agent definitions and agent detection |
| File-tree rendering | Remote directory and Git data |

The remote host chooses the shell. Your local `[shell]` settings and shell integration files are not
sent to it. A remote shell that emits OSC 7 still reports its current directory; without it,
directory tracking may stay at the pane's starting directory.

While attached remotely, the Files and Git sidebar tabs read the remote filesystem. Git change
markers need `git` on the remote host's `PATH`. File search covers only directories already
expanded in the tree.

Hooks run locally and receive `ROZI_REMOTE_HOST`. The control socket also stays local.
`rozi sessions list --remote`, `rozi sessions kill --remote`, and `rozi worktrees --remote` each run
as a separate SSH command. Quote `~` in their paths so your local shell does not expand it.

Clipboard reads and writes happen on the machine running the rozi window, so OSC 52 from a remote
pane can update your local clipboard when enabled. A program that accesses the clipboard directly
still uses the remote host's clipboard. See [Terminal features](terminal.md) for shell metadata,
clipboard behavior, and image limits.

## Reconnection and switching

Switching to another local or remote session keeps the current one connected in the background.
Returning to it reuses its live screens.

If an SSH connection drops — including a connection that silently stopped working after sleep, a
lost network, or missed heartbeats from the session — rozi shows a reconnecting overlay and retries
in place for up to two minutes. It reuses the rozi binary it already found on the host.

- `Esc` stops waiting and opens Sessions.
- An SSH password, passphrase, or host-key prompt covers the overlay while authentication is
  needed. `Esc` cancels that prompt, and the reconnecting overlay returns.
- If the remote session no longer exists, the overlay shows **session lost**. rozi never starts a
  replacement on its own: press `Enter` to recreate the session from the panes still on screen, or
  `Esc` to open Sessions without recreating it.
- If the host is still unreachable after two minutes, the session stays on screen as **offline**.
  `Enter` retries and `Esc` opens Sessions.

Named remote sessions keep running when the SSH connection drops. Temporary ones close after 45
seconds with no client attached; see
[Recover a temporary session](sessions.md#recover-a-temporary-session).

## Connected-host monitoring

Connecting a host also opens a separate channel that watches the host's sessions. It stays open
when you close the Sessions sidebar or switch to another machine. rozi never contacts a saved host
just because rozi starts or a picker opens.

Every two seconds, the channel checks the host's health and refreshes its session list. It does not
attach to sessions, receive terminal output, or take layout control. Each host reconnects on its
own, waiting between 500 ms and 30 seconds between attempts. While a host is unreachable, its last
session list stays visible with rows marked `last seen`. `Ctrl+K` twice on a `last seen` row forgets
it without contacting the host; if the host still reports the session later, it is listed again.

Background reconnects never prompt. If a host needs a password or host-key approval, select it and
reconnect with `Enter` or `Ctrl+R`; with `[remote] batch_mode = false`, the SSH dialog then handles
the prompt.

Monitoring runs `rozi sessions watch` on the host, a protocol meant only for rozi clients. It is
versioned separately from session attachment. If the host's rozi lacks a required feature
(`session-list` or `health-check`), the host shows an error until you update rozi on the host and
reconnect; a missing optional feature disables only that feature. Updating rozi on the host does not
restart its running sessions.

### Agents on a machine you are not in

When both ends support the `agent-summaries` feature, monitoring also reports what the agents in
each session are doing. Session rows then read:

```text
WORKBOX                                     ● Connected
  dev              2 panes · Codex working
  backend          4 panes · 2 blocked
```

Each row shows the most urgent state in the session — blocked, then working, then done — and names
the agent when only one is in that state. Idle agents are not shown. A session you are attached to
shows no summary, because the Agents tab already shows its live state.

When an agent becomes blocked or finishes, you get the same desktop notification and sound as in an
attached session, under the same `[notifications]` and `[sounds]` settings, and Do not disturb
silences them the same way. The first update after connecting only sets a baseline, so connecting
to a host whose agent has been waiting since yesterday does not notify.

Summaries contain only the session, pane, agent, state, and when the state last changed. Terminal
contents, scrollback, working directories, and layout need an attached session.

The [Agents view](sessions.md#go-to-an-agent) (`Ctrl+A`, then `a`) lists these alongside local
agents, most urgent first. `Enter` on a remote row attaches to that session and focuses the agent's
pane in one step.

## Disconnect from a host

`Ctrl+W` disconnects one session attachment. `Ctrl+X` disconnects this client from the whole host:

- Monitoring and its reconnect attempts stop.
- Every attachment to that host closes, both current and background.
- Named sessions on the host keep running, whichever client started them.
- A temporary session this client created there and never used is closed, since nothing could
  reattach to it by name.
- A launcher scoped to that host is no longer scoped to it.

Disconnecting never kills a session, even a named session this client created. Only `Ctrl+K` kills
a session. To reconnect, use the session picker or the Sessions sidebar.

Disconnecting does not close the [shared SSH connection](#shared-ssh-connections), which another
rozi client may be using. It closes on its own about a minute after the last command using it
finishes.

## Troubleshooting

| Message | Check |
| --- | --- |
| `ssh not installed here` | Install OpenSSH on the client and check `PATH`. |
| `Unknown host name` | Check DNS, SSH config, and the host alias. |
| `Host not responding` | Check power, network, VPN, routing, and firewalls. |
| `SSH port closed` | Check `sshd`, the configured port, and port forwarding. |
| `Host unreachable` | Check the network, VPN, and routing to the host. |
| `SSH login rejected` | Check the user, key, agent, and server authorization. |
| `Host key not trusted` | Connect with `ssh` and inspect `known_hosts`. |
| `Host key changed` | The host's identity does not match the recorded key, and OpenSSH refuses to connect. Verify the new key out of band before removing the old entry. |
| `No rozi on host` | Allow installation or set `binary_path`. |
| `Install cancelled` | You declined the install prompt. Connect again and choose `Install`, or set `binary_path`. |
| `Rozi update needed` | The host's rozi does not speak this client's session protocol. Install the matching version on the host. |
| Incompatible version | Update both ends and restart the named session. |
| `Connection failed` | Any other SSH failure. Run `rozi --remote <host>` from a shell to see the error. |
| Git markers missing | Install `git` on the remote host and check its `PATH`. |

When a sidebar message is not enough, run `rozi --remote <host>` from a shell to see the underlying
SSH error.

See also [Troubleshooting](troubleshooting.md).

## Security

- SSH provides authentication and encryption. rozi opens no network port for sessions.
- On the remote host, sessions are reachable only by the same user, through private local
  connections.
- rozi does not forward your local environment into remote panes, so local display connections and
  configured credentials are not copied to the host.
- `ssh_args` and `binary_path` are trusted configuration. Keep `binary_path` a single token without
  shell-special characters.
- When enabled, OSC 52 lets a remote pane write to your local clipboard. Set
  `[clipboard] enable_osc52 = false` if remote programs should not have that access.
- A writable remote client has the same authority over a session as a local one; see
  [Shared sessions](shared-sessions.md#security-and-caveats).
