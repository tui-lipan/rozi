# Remote SSH sessions (`--remote`)

`--remote` attaches a **local** hyprmux client to a session server that runs on another host over
SSH. The remote host owns every PTY and its filesystem; your local config (theme, keybindings,
clipboard, hooks) stays on this machine.

This is the inverse of “ssh in and run hyprmux there”: the UI and config stay local, while the
session server stays remote.

## Quick start

```bash
hyprmux --remote workbox                  # ephemeral session on workbox
hyprmux --remote workbox dev              # attach/launch named session "dev"
hyprmux --remote ssh://user@host:2222     # explicit user, host, port
hyprmux --remote workbox attach dev       # attach-only
hyprmux --remote workbox new review       # create-only
hyprmux list-sessions --remote workbox
hyprmux kill-session dev --remote workbox
```

`--remote` composes with the same session surface as a local launch (`attach`, `new`, `--session`,
`--profile`, `--read-only`). It cannot be combined with `--server` or `--fresh-server`.

## Authentication

Requires `ssh` on `PATH`. By default hyprmux passes `BatchMode=yes`, so auth must succeed without a
prompt — typically a loaded agent or an `identity_file` under `[remote.hosts.*]`.

Set `[remote] batch_mode = false` to let ssh prompt. One switch governs every remote invocation
(probe, install, attach, `list-sessions --remote`, `kill-session --remote`), deliberately: a mix
would give you a host that lists fine and then hangs on attach.

The caveat: on the **attach** path ssh's stdin carries the session protocol, so ssh falls back to
prompting on the controlling terminal — which is the terminal hyprmux is drawing in. Expect the
prompt to land on top of the UI. `batch_mode = false` is most useful for the CLI helpers, or with a
passphrase-protected key you unlock once before attaching.

## How it works

The remote side does **not** teach the session server to speak stdio. Local hyprmux runs:

```text
ssh <host> -- <remote-hyprmux> --remote-serve <NAME>
```

That hidden `--remote-serve` process connects to the **normal** session endpoint on the remote host
(Unix socket or named pipe) and pumps framed bytes between that connection and its own stdin/stdout.
The local client treats those pipes as an `IpcConnection::Piped` transport and speaks the usual
session protocol over them. Remote attach does not reuse the local connect retry/backoff loop —
autostart lives inside `--remote-serve` on the far side — but it does kill+retry once when a running
remote server's protocol cannot be negotiated.

Consequences:

- Multi-client still works on the remote host: a local client on that box and your `--remote`
  client can share one session and the layout-control lease.
- Discovery, resurrect, and `kill-session` on the remote host keep using the existing local
  endpoints; `--remote` only tunnels a client.
- `peer_pid` is unset for piped connections, so a failed graceful shutdown never falls back to
  terminating a local `ssh` process.

## Target syntax

`--remote` accepts:

| Form | Meaning |
| --- | --- |
| `workbox` | ssh_config `Host` alias, bare hostname, or `[remote.hosts.workbox]` alias |
| `ssh://host` | Host only |
| `ssh://user@host` | User and host |
| `ssh://user@host:2222` | User, host, and port |

CLI values merge with `[remote]` / `[remote.hosts.<alias>]` (see [Configuration](configuration.md#remote)).
Per-host `binary_path`, `identity_file`, and `ssh_args` override the defaults for that alias.

## Bootstrap and install

Before connect, hyprmux probes the remote for a compatible `hyprmux` binary (one that speaks
`--remote-serve` and overlaps the client's protocol range).

| `[remote] install` | Interactive TTY (before the TUI starts) | Non-interactive / CI |
| --- | --- | --- |
| `prompt` (default) | Asks `[y/N]` on stdin; installs only after an explicit yes | Never mutates; fails with a clear message |
| `always` | Installs without asking when missing/incompatible | Never mutates |
| `never` | Fail if missing | Fail if missing |

When the local and remote platforms match, install copies `current_exe()`. When they differ, hyprmux
downloads the matching GitHub release asset for this version, verifies its `.sha256`, then uploads
that binary. Override the download base with `HYPRMUX_RELEASE_BASE_URL` for mirrors/tests.

Overrides:

- `[remote.hosts.<alias>] binary_path` — use that path; skip probe/install.
- `HYPRMUX_REMOTE_BINARY=/path/to/hyprmux` — stream that local file onto the remote (same install
  location), regardless of platform match — you are responsible for architecture fit.
- `[remote] default_host` — used when `--remote` is passed without a host argument; also supplies
  shared `identity_file` / `ssh_args` / `binary_path` defaults for other aliases.

Install writes atomically under `$HOME/.local/bin/hyprmux` on the remote and refuses to overwrite a
non-regular file. Non-interactive runs never issue a mutating install command.

If attach finds a running remote server whose protocol range does not overlap this client, hyprmux
kills that session once over ssh and retries attach (so a fresh `--remote-serve` can autostart a
compatible server).

## Protocol negotiation

Client and server advertise a max and min protocol version on attach/query. They negotiate
`effective = min(client_max, server_max)` and reject only when that value falls below either side's
minimum. Within a supported range, wire changes are additive (`#[serde(default)]`); breaking changes
bump `MIN_SUPPORTED_PROTOCOL`.

This build speaks protocol **13**, with **12** as the minimum. Protocol 13 adds the file-tree
browsing messages, so a 13-client attached to a 12-server negotiates 12 and simply does not send
them — everything else works. Servers at v11 and earlier still do strict equality, so attaching to
one surfaces the usual “kill it and start a new one” message, after which a fresh remote server
speaks the negotiated range.

## Local vs remote feature split

| Stays local (client) | Lives on the remote (server) |
| --- | --- |
| Theme, keys, config, overlays, copy/search UI | PTYs, pane processes, layout authority |
| Clipboard / OSC52 (paste into the local terminal) | Working directories and spawn `cwd` paths |
| Hooks (`[[hooks]]` run on the client) | Session discovery endpoints on that host |
| Control socket for *this* UI process | Resurrect / autosave paths on that host |
| File tree rendering, icons, search | File tree listings and git status |

Notes:

- Pane `cwd` reports are **server-relative**. New splits and popups still inherit them so the remote
  server can spawn correctly; local filesystem helpers (for example opening a path on this machine)
  skip those paths while `--remote` is attached.
- Hooks receive `HYPRMUX_REMOTE_HOST` when attached remotely. See [Hooks](hooks.md).
- The sidebar **Files** / **Git** tabs browse the **remote** filesystem. The client asks the session
  server to read each directory (`ListDirectory`) and to scan the repository for changes
  (`ListChanges`), then renders the replies locally, so expansion, icons, search, and theming stay
  client-side while the data comes from the server's host. `git` must be on the remote server's
  `PATH` for change markers; without it the tree still browses, just without decorations. A remote
  server older than protocol 13 cannot answer, and the tab says so rather than spinning.
- The UI control socket remains on the client machine. Automating a remote-attached UI still uses
  that local control endpoint; `list-sessions --remote` / `kill-session --remote` are separate SSH
  CLI helpers, not control-socket commands.

## Security model

- Transport is whatever your `ssh` config already trusts (keys, known_hosts, jump hosts via
  `ssh_args`). hyprmux does not open a network-facing session port.
- Runtime endpoints on the remote host stay per-user local IPC; the proxy is just another client of
  that endpoint.
- Install copies a binary you already run locally onto the remote home directory; set
  `install = "never"` or pin `binary_path` if you do not want that path.
- Clipboard content from remote panes still lands on the **local** clipboard via OSC52 / selection
  when enabled — same exposure as a local session.

## Related

- [Sessions](sessions.md) — attach/detach, ephemeral vs named, multi-client.
- [Configuration](configuration.md#remote) — `[remote]` keys.
- [Control](control.md) — local control socket vs remote list/kill CLI.
- [Hooks](hooks.md) — `HYPRMUX_REMOTE_HOST`.
- [Sidebar](sidebar.md) — file tree behavior under `--remote`.
