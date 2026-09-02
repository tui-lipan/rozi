# Remote sessions

`--remote` keeps the Rozi UI on your machine and runs the session server, PTYs, and pane processes
on another host over SSH.

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

The session commands behave like local commands. A named target attaches to its running session or
launches its same-name profile. See
[Sessions](sessions.md#open-a-session-from-the-command-line).

A bare `rozi --remote workbox` names no session, so `[session] startup` decides — applied on
`workbox` rather than locally. Under the default `picker` it connects, lists that host's sessions,
and creates nothing; `ephemeral` starts a temporary session there; `last` reattaches the session it
remembers on that host if the host still has it; `profile` opens or creates the default-profile
session there. Each falls back to `Sessions · workbox`. See
[Choose startup behavior](sessions.md#choose-startup-behavior).

Rozi supports Linux, macOS, and Windows as either client or remote server hosts. The local machine
needs `ssh` and `curl` on `PATH`. Automatic installation also needs `tar` for a Linux or macOS
target, or `unzip` and `scp` for a Windows target.

## Set up SSH authentication

Rozi uses your OpenSSH configuration, keys, agent, `known_hosts`, jump hosts, and other SSH policy.
It does not manage credentials.

Batch mode is enabled by default, so authentication must complete without an interactive prompt.
Load a key into your SSH agent or set an identity file:

```toml
[remote.hosts.workbox]
host = "workbox.example.com"
user = "dev"
identity_file = "~/.ssh/id_ed25519"
```

Set `[remote] batch_mode = false` to allow SSH prompts. This applies to probing, installing,
attaching, listing, and killing sessions. Prefer a loaded agent for regular use.

### Prompts inside the UI

With `batch_mode = false`, a running Rozi client answers SSH prompts in a modal instead of letting
them reach the terminal:

- A password or key passphrase is masked and never appears on screen.
- A host-key question shows the full fingerprint and takes `yes` or `no` unmasked.
- A rejected password is reported on the prompt when SSH asks again.
- `Esc` ends that connection's attempt. SSH re-asks three times per connection, so a refusal also
  declines those retries and gives up on the host probe that raised them. Activating the host again
  starts a new connection and prompts normally.
- A prompt left unanswered for five minutes fails its connection and closes.

Rozi sets `SSH_ASKPASS`, `SSH_ASKPASS_REQUIRE=force`, and its own endpoint variables on every `ssh`
and `scp` it runs, overriding a desktop `SSH_ASKPASS` for those commands only. This needs OpenSSH
8.4 or newer on the client; older clients prompt on the terminal, where the prompt will overwrite
the UI.

Command-line runs have no UI to cover, so `rozi sessions list --remote`, `rozi sessions kill
--remote`, and the pre-launch install prompt keep SSH's ordinary terminal prompt.

## Shared connections

Opening a host runs several SSH commands: a shell-family probe, a capability probe, a re-check, and
the attach. On Linux and macOS these share one connection, so a host is authenticated once instead
of once per command — a password host asks for the password a single time, and later commands skip
the handshake entirely.

Rozi passes `ControlMaster=auto`, a `ControlPath` in its runtime directory, and `ControlPersist=60`,
overriding any `ControlMaster` or `ControlPath` in your SSH config for its own commands. The shared
connection closes on its own about a minute after the last one finishes. `ServerAliveInterval` and
`ServerAliveCountMax` apply to it, since a shared connection's settings come from whichever command
opened it.

The `ControlPath` is per user rather than per client, so two Rozi clients on the same machine share
one master per host. This is why disconnecting from a host does not close it: exiting a master also
terminates the sessions riding on it, including another client's attachment.

Windows clients have no SSH connection multiplexing and authenticate per command.

Test authentication directly when setup fails:

```bash
ssh workbox
```

## Configure aliases and defaults

`--remote` accepts an SSH config alias, a bare hostname, or an
`ssh://[user@]host[:port]` URL.

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

With `default_host` set, `rozi --remote` uses that target. Its host entry also supplies inherited
`user`, `port`, `identity_file`, `ssh_args`, and `binary_path` values to aliases that do not set
their own values. An alias-specific value wins. A non-empty alias-specific `ssh_args` list replaces
the inherited list.

For an `ssh://` URL, the URL's user and port win. Settings may come from a host entry with the same
hostname, then from the default host entry.

Use `binary_path` when Rozi already exists at a fixed remote path:

```toml
[remote.hosts.workbox]
binary_path = "/opt/rozi/bin/rozi"
```

## Choose an install policy

Before connecting, Rozi checks for a compatible remote binary.

| `[remote] install` | Interactive terminal | Non-interactive run |
| --- | --- | --- |
| `prompt` | Ask before installing. This is the default. | Fail without changing the host. |
| `always` | Install when needed without asking. | Fail without changing the host. |
| `never` | Fail when no compatible binary is found. | Fail without changing the host. |

On Linux and macOS, automatic installation writes `$HOME/.local/bin/rozi`. On Windows it writes
`%USERPROFILE%\.local\bin\rozi.exe`.

When client and server platforms match, Rozi can copy the running executable. For a different
platform, it downloads the matching release archive, verifies its checksum, and uploads the binary.
Set `ROZI_REMOTE_BINARY` to upload a specific local binary, or `ROZI_RELEASE_BASE_URL` to use a
release mirror.

Rozi refuses to overwrite a non-regular install target. Set `install = "never"` or pin
`binary_path` if remote installation is not acceptable.

The current remote protocol supports one version. Update both ends together. If a running named
server is incompatible after an update, restart that server. A configured or discovered binary
mismatch may be rejected before attachment when Rozi can detect it.

## Understand the client and server boundary

| Local client | Remote server |
| --- | --- |
| Theme, keybindings, overlays, and sidebar UI | PTYs and pane processes |
| Copy, search, and hint interfaces | Shared layout authority |
| Local control socket | Session discovery and resurrection |
| Hooks and desktop notifications | Pane working directories |
| Local clipboard access | Agent definitions and agent detection |
| File-tree rendering | Remote directory and Git data |

The remote server chooses the shell. Local `[shell]` and local shell-integration files are not sent
to it. A remote shell that emits OSC 7 can still report its current directory. Without that signal,
directory tracking may remain at the pane's launch directory.

The Files and Git sidebar tabs read the remote filesystem while attached remotely. `git` must be on
the remote server's `PATH` for change markers. File search only includes directories the client has
already expanded.

Hooks run locally and receive `ROZI_REMOTE_HOST`. The UI control socket also remains local.
`sessions list --remote` and `sessions kill --remote` are separate SSH commands.

Clipboard reads and writes happen on the client where the UI runs. OSC52 from a remote pane can
therefore update the local clipboard when enabled. Direct rich-clipboard access by the pane still
refers to the remote host.

See [Terminal features](terminal.md) for shell metadata, clipboard behavior, and image limits.

## Work on a host without a session

A client can be scoped to a host while holding no session there:

```text
REMOTE · workbox
Not attached. A shell starts on workbox.
```

This is a resting state, not a failure. It says which machine the launcher acts on — `Enter` starts
a temporary shell on `workbox` — and it implies no live SSH connection. `rozi --remote workbox`
under `startup = "picker"` lands here, and so does dismissing `Sessions · workbox` with nothing
attached.

Opening **Sessions** from this state is still global: its `Ctrl+N` and `Ctrl+T` create local
sessions. See [Scope](sessions.md#scope-where-an-action-happens).

## Reconnection and switching

Switching to another local or remote session parks the current attachment in the background.
Returning to it reuses its live terminal screens.

If a retained SSH connection drops, Rozi marks it offline. Selecting that attachment attempts to
reconnect in place. The remote named server keeps running independently of the SSH connection.
Temporary servers still follow their no-client recovery timer.

## Disconnect from a host

`Ctrl+W` detaches one session attachment. `Ctrl+X` disconnects this client from the whole host:

- every attachment to that host closes, current and background;
- named remote servers keep running, whichever client started them;
- a temporary session this client created there and you never worked in is closed, because
  nothing can reattach to it by name;
- a launcher scoped to that host stops being scoped to it.

Only `Ctrl+K` kills a session. Disconnecting never does, even for a named session this client
created. Use the session picker or Sessions sidebar to reconnect.

Disconnecting does not close the shared SSH connection. That master is per user, not per client, so
another Rozi client may be riding it; it closes on its own once the last command using it finishes
its `ControlPersist` window.

## Troubleshooting

| Message | Check |
| --- | --- |
| `ssh not installed here` | Install OpenSSH on the client and check `PATH`. |
| `Unknown host name` | Check DNS, SSH config, and the host alias. |
| `Host not responding` | Check power, network, VPN, routing, and firewalls. |
| `SSH port closed` | Check `sshd`, the configured port, and port forwarding. |
| `SSH login rejected` | Check the user, key, agent, and server authorization. |
| `Host key not trusted` | Connect with `ssh` and inspect `known_hosts`. |
| `No rozi on host` | Allow installation or set `binary_path`. |
| Incompatible version | Update both ends and restart the named server. |
| Git markers missing | Install `git` on the remote server and check its `PATH`. |

Run `rozi --remote <host>` from a shell to see the underlying SSH error when a sidebar summary is
not enough.

## Security

- SSH provides transport authentication and encryption. Rozi opens no network session port.
- Session endpoints on the remote host remain private local IPC for that user.
- Rozi does not forward the client's environment into remote panes. This avoids copying local
  display endpoints and configured credentials.
- `ssh_args` and `binary_path` are trusted configuration. Keep executable paths to one
  shell-safe token.
- OSC52 lets a remote pane write to the local clipboard when enabled. Disable it with
  `[clipboard].enable_osc52 = false` if the remote programs should not have that access.
- A writable remote client has the same session authority described in
  [Shared sessions](shared-sessions.md#security-and-caveats).
