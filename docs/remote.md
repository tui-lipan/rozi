# Remote SSH sessions (`--remote`)

`--remote` attaches a **local** hyprmux client to a session server that runs on another host over
SSH. The remote host owns every PTY and its filesystem; your local config (theme, keybindings,
clipboard, hooks) stays on this machine.

The workbar identifies the active location with a `󰒍 <host>` badge before the named-session badge.
Switching to a local or another remote session retains this attachment and its SSH transport in the
background; switching back restores its live screens immediately. If a retained link drops, it is
marked offline and reconnects in place when selected again.

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

## Supported platforms

| Client (local) | Remote (server) | Status |
| --- | --- | --- |
| Linux / macOS | Linux / macOS | Supported. |
| Windows | Linux / macOS | Supported. |
| any | **Windows** | **Supported** — verified live against Windows 11 + OpenSSH; see below. |

hyprmux itself runs on Windows, and the session server and `--remote-serve` proxy are
platform-neutral. A Windows host works as the *remote* end of `--remote`, verified end to end
against a real Windows 11 + OpenSSH host (stock `cmd.exe` default shell): probe, install, attach,
detach/reattach, a concurrent second client, and — the previous hard blocker — a session that
survives the SSH link dropping.

What the client does for a Windows remote:

- **Shell family detection.** The probe first runs one fixed, shell-agnostic command
  (`echo hyprmux_family=%OS%`, which only `cmd.exe` expands to `Windows_NT`), then dispatches a
  POSIX (`sh -s`) or PowerShell probe accordingly. Probe output is still parsed by fixed keys and
  never treated as argv.
- **Platform detection.** `MINGW*`/`MSYS*`/`CYGWIN*` `uname -s` output is matched by family prefix,
  and the PowerShell probe reports `platform=windows` directly (`PROCESSOR_ARCHITECTURE` gives the
  machine — `AMD64` normalises to `x86_64`).
- **`.exe`-aware install.** The Windows install writes `%USERPROFILE%\.local\bin\hyprmux.exe` (no
  `chmod`). `connect.rs` then invokes `--remote-serve` with the returned `.exe` path verbatim.
- **Server lifetime (the former hard blocker, now solved).** Windows OpenSSH runs each session
  inside a Job Object and terminates it on disconnect; a plain `DETACHED_PROCESS` does not escape a
  job. `spawn_detached_server` adds `CREATE_BREAKAWAY_FROM_JOB`, falling back to a plain detached
  spawn if the job refuses the flag (`ACCESS_DENIED`). Verified live: breakaway *is* permitted by
  OpenSSH's job on Windows 11, so a session started over `--remote` keeps `running` after the SSH
  link is dropped. (The fallback stays as insurance for a differently-configured host; it was not
  needed here.)

Two OpenSSH-for-Windows quirks that live testing surfaced, both handled in the client:

- **Large command stdin deadlocks.** Piping more than the channel's stdin buffer (~64 KB) to a
  remote command over win32-OpenSSH stalls hard — a real ~11 MB binary never finishes. So the
  Windows install uploads the binary with `scp` (the sftp subsystem has real flow control) and then
  runs a small no-stdin finalize step, rather than streaming it through a command's stdin the way
  the POSIX install does.
- **`powershell -Command -` truncates a stdin script.** A multi-line script fed to
  `powershell -Command -` on stdin runs only its first statements. So both the PowerShell probe and
  the install finalize are delivered as a base64 `-EncodedCommand` (also quoting-proof through
  cmd.exe), which runs the whole script and needs no stdin.

Measured against a real Windows 11 host over OpenSSH:

- **Line endings are not a problem.** Non-pty stdio under the stock `cmd.exe` shell is byte-clean in
  both directions — `0x0A` stays `0x0A`, and the framed preamble arrives intact. No `DefaultShell`
  change is required. (An earlier revision of this page warned otherwise; that warning was wrong.)
- The Windows session server runs correctly, binds its named pipe, and is discoverable by
  `list-sessions` on that host.
- `--remote-serve` emits a valid preamble reporting `platform: windows`.
- The proxy's stdout pump must set the named pipe non-blocking on its **reader clone only**; a
  blocking `ReadFile` on a duplicated pipe handle stalls `WriteFile` on its sibling, and setting
  `PIPE_NOWAIT` on the writer breaks `write_all`. See `session::remote::proxy`.

Cross-platform install between *supported* platforms downloads the matching release archive and
needs `tar` on the client (`tar.exe` ships with Windows 10 and later). Checksum verification is done
in-process, so it needs no external tool anywhere.

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
that binary. Override the download base with `ROZI_RELEASE_BASE_URL` for mirrors/tests.

Checksums are computed in-process rather than via `sha256sum`/`shasum`, so verification works
identically on every client platform and can never be skipped because a tool is missing.

Overrides:

- `[remote.hosts.<alias>] binary_path` — use that path; skip probe/install.
- `ROZI_REMOTE_BINARY=/path/to/hyprmux` — stream that local file onto the remote (same install
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

This build speaks protocol **19** only (`MIN_SUPPORTED_PROTOCOL` is also 19). SharedLayout gained
`columns` and `scrollable` layout kinds that older peers cannot deserialize, so pre-19 servers and
clients are rejected rather than shimmed. Within 19, messages introduced earlier in the lineage
remain available (13 file-tree browsing, 14 parked, 15 control-takeover policy, 16 evict client, 18
runtime metrics). Restart existing session servers after upgrading.

## Local vs remote feature split

| Stays local (client) | Lives on the remote (server) |
| --- | --- |
| Theme, keys, config, overlays, copy/search UI | PTYs, pane processes, layout authority |
| Clipboard / OSC52 (paste into the local terminal) | Working directories and spawn `cwd` paths |
| Hooks (`[[hooks]]` run on the client) | Session discovery endpoints on that host |
| Control socket for *this* UI process | Resurrect / autosave paths on that host |
| File tree rendering, icons, search | The pane shell and its resolution |

Notes:

- **The shell of a remote pane is resolved on the server, not the client.** A pane spawned under
  `--remote` is sent with an empty shell argv, so the remote session server picks its own platform
  default (`$SHELL` on Unix, `cmd.exe`/PowerShell on Windows) — the local `[shell]` setting and the
  client's shell-integration rc-file (a local path a different-OS server cannot run) do not travel.
  A consequence is that hyprmux's shell integration (OSC 133 prompt markers, OSC 7 cwd reporting,
  agent-status detection) is **not** injected into remote panes, so those features are limited there
  until the server grows its own shell-integration path. A remote pane still shows the directory it
  was **launched** in — the server reports the cwd it spawned the pane in — but that display does not
  track live `cd`s without OSC 7 (or native process inspection, which is Linux/macOS only). A shell
  that already emits OSC 7 reports its live cwd as normal.
- Pane `cwd` reports are **server-relative**. New splits and popups still inherit them so the remote
  server can spawn correctly; local filesystem helpers (for example opening a path on this machine)
  skip those paths while `--remote` is attached.
- Hooks receive `ROZI_REMOTE_HOST` when attached remotely. See [Hooks](hooks.md).
- The sidebar **Files** / **Git** tabs browse the **remote** filesystem. The client asks the session
  server to read each directory (`ListDirectory`) and to scan the repository for changes
  (`ListChanges`), then renders the replies locally, so expansion, icons, search, and theming stay
  client-side while the data comes from the server's host. `git` must be on the remote server's
  `PATH` for change markers; without it the tree still browses, just without decorations. A remote
  server older than protocol 13 cannot answer, and the tab says so rather than spinning.
  - **File-tree search is scoped to already-fetched directories.** Search runs over the listings the
    client currently holds, and under `--remote` those are only the directories that have been
    expanded (each expansion triggers one `ListDirectory`). Collapsed subtrees are not fetched, so
    matches inside them do not appear until they are expanded. This is a known limitation, not a bug.
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
- [Hooks](hooks.md) — `ROZI_REMOTE_HOST`.
- [Sidebar](sidebar.md) — file tree behavior under `--remote`.
