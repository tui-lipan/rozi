# Control socket and automation

When the UI starts, hyprmux binds a per-process control endpoint at `control-<pid>.sock` in its
runtime directory. If binding fails, the UI still starts and shows a warning; panes only receive
`HYPRMUX_SOCKET` when an endpoint is available.

## Endpoints per platform

| | Runtime directory | Transport |
| --- | --- | --- |
| Linux | `$XDG_RUNTIME_DIR/hyprmux`, else a private per-uid temp directory | Unix-domain socket at that path |
| macOS | a private directory under `$TMPDIR` | Unix-domain socket at that path |
| Windows | `%LOCALAPPDATA%\hyprmux\run` | Named pipe `\\.\pipe\hyprmux.<user-sid>.control-<pid>` |

On Windows the file in the runtime directory is a **discovery entry**, not the transport: it stands
for a named pipe whose name is derived from the entry's own name and your SID. Point `--socket` and
`HYPRMUX_SOCKET` at the entry, exactly as you would at a socket path on Unix — the same command
line works on all three platforms. Nothing trusts the entry's contents; the pipe name is recomputed
rather than read out of it, and every connection completes an authenticated handshake regardless.

Endpoints are private to the user who created them (mode `0600` on Unix; a protected
current-user-SID DACL plus `PIPE_REJECT_REMOTE_CLIENTS` on Windows, which makes a pipe unreachable
over SMB). An entry left behind by a crashed process is replaced on the next bind; a *live* one
cannot be squatted.

Every spawned pane receives:

- `HYPRMUX=1`
- `HYPRMUX_PANE=<pane id>`
- `HYPRMUX_SOCKET=<endpoint path>` when control is available

## CLI

Control commands do not mount the UI. Endpoint discovery order is `--socket PATH`, `HYPRMUX_SOCKET`,
then exactly one live endpoint in the runtime directory.

```bash
hyprmux list-panes
hyprmux metrics
hyprmux focus 3
hyprmux send-text 'cargo test
'
hyprmux send-keys C-c
hyprmux send-keys 'echo hi' Enter
hyprmux send-keys -l C-c
hyprmux send-keys -- -n hello
hyprmux split 'claude --agent helper'
hyprmux split 'cargo watch' --focus
hyprmux run-action toggle-float
hyprmux capture-pane --target 3
hyprmux capture-pane --scrollback full
hyprmux capture-pane --last-output
hyprmux capture-pane --scrollback 200 --target 3
hyprmux run-action copy-last-output
hyprmux switch-workspace 2
hyprmux move-to-workspace 3
hyprmux status blocked --reason "needs approval"
hyprmux status --clear
```

Replies are JSON on stdout. Errors are JSON when returned by the server, or plain stderr for client
discovery/connect failures.

`split`/`new-pane` holds its reply until the new pane's PTY reports ready, so `pty_ready: true`
means input sent to the returned `id` will reach the shell. A spawn that has not come up after five
seconds answers `pty_ready: false` rather than leaving the caller on the connection timeout — the
pane is still starting, not broken. A spawn that fails answers with a JSON error.

`split`/`new-pane` leaves focus alone. The control endpoint is an automation surface, and a pane
spawned from a script or an agent must not move focus, and the active workspace, away from whoever
is typing. Pass `--focus` (or `"focus": true` in JSON) to move to the new pane. This overrides the
`focus` field of a matched `[[rules]]` entry; the rule still decides workspace, float, and
fullscreen.

`send-text`/`send-keys` targeting a pane whose PTY is still starting are queued and written as
type-ahead once it is ready, matching what typing into a freshly split pane already does. Input to a
pane that has exited or failed to spawn is rejected with `PTY is not running`.

`metrics` is a read-only, render-neutral snapshot of bounded runtime resources. It reports local
client queues, the SSH pipe buffer when present, orphan output, and the latest cached server sample
for PTY ingress, aggregate client outboxes, and resurrection writes. The command returns
immediately and asks the session server to refresh its cache asynchronously; it never waits for
that round trip. Consequently, `server` can be `null` on the first request. Cached server samples
include monotonic reception `age_ms` and become `"stale":true` at 15 seconds.

Resurrection reports two durations. `last_duration_us` / `max_duration_us` cover the whole attempt,
capture plus durable write, and stay comparable with measurements taken when snapshots were written
synchronously. `last_blocking_us` / `max_blocking_us` cover only the part the server loop is held
for while it captures pane replay bytes; the write, sync, and rename run on a snapshot worker. It is
the blocking figure, not the total, that bounds input latency during a snapshot.

`last_exported_panes` / `last_reused_panes` / `last_exported_bytes` explain a slow snapshot without
reproducing it. A snapshot only re-exports panes whose terminal changed since the last successful
one and hard-links the rest, so cost tracks exported panes and their bytes rather than the session's
pane count. A session where every pane is continuously producing output exports every pane, and
these counters are what shows that is happening.

The response field names and resource nesting are deterministic; timestamps, counters, optional
resources, and byte values reflect the sampled process:

```json
{"ok":true,"data":{"sampled_at_unix_ms":1000,"client_inbound":{"current_bytes":0,"high_water_bytes":4096,"capacity_bytes":8388608,"queued_items":0},"client_outbound":{"current_bytes":0,"high_water_bytes":512,"capacity_bytes":8388608,"queued_items":0},"piped_remote":null,"orphan_output":{"current_bytes":0,"high_water_bytes":0,"capacity_bytes":4194304,"keys":0,"capacity_keys":4096},"server":{"sampled_at_unix_ms":990,"pty_ingress":{"current_bytes":0,"high_water_bytes":8192,"capacity_bytes":4194304,"queued_items":0},"client_outboxes":{"current_bytes":0,"high_water_bytes":16384,"capacity_bytes":16777216,"clients":2},"resurrection":{"attempts":1,"successes":1,"failures":0,"last_duration_us":2400,"max_duration_us":2400,"last_blocking_us":310,"max_blocking_us":310,"last_exported_panes":1,"last_reused_panes":7,"last_exported_bytes":184320},"age_ms":10,"stale":false}}}
```

`send-keys` accepts tmux-style key names (`C-c`, `M-x`, `Enter`, `Escape`, `Space`, `Tab`,
`BSpace`, arrows, `Home`/`End`, `PgUp`/`PgDn`, `F1`..`F12`) mixed with literal text arguments.
Unknown tokens are sent as literal UTF-8. `-l` / `--literal` forces every argument to be treated as
literal text (so `send-keys -l C-c` types the characters `C-c` instead of Ctrl+C). A `--`
terminator stops flag parsing so arguments that look like options can be typed literally
(`send-keys -- -n hello`). All arguments are validated before any are sent; if a later key fails
to deliver, earlier keys may already have reached the PTY — automation clients that retry on
error should treat the pane as partially updated.

`run-action` takes any keybindable action's stable id (the same ids used in `[keys]` config and
shown in the command palette/help overlay), e.g. `toggle-float`, `spawn`, `close`. `capture-pane`
returns the plain text of a pane's current visible snapshot grid by default, or scrollback history
with `--scrollback N` / `--scrollback full`, or the last shell-integration command's output with
`--last-output` (JSON: `"scrollback":"last-output"`). It defaults to the request's `source_pane`
or the focused pane when `--target`/`target` is omitted. `switch-workspace` and
`move-to-workspace` take a 1-9 workspace number, matching the on-screen tabs.
Destructive `run-action` calls honor `[confirm]` settings; a first call can arm a confirmation
toast and a second matching call within the toast window confirms it.

`status <value> [--reason <text>]` reports a short, free-form status for the source/focused pane;
use `status --clear` to remove it. An explicit control request may instead supply `target`. Status is
server-owned and visible to every attached client. The values `working`, `blocked`, `done`, and
`idle` receive built-in presentation and notification behavior, but other values are accepted.
The command's successful reply means the validated update was queued to the session server; it is
not a synchronous acknowledgement that the server has applied it. Read-only clients cannot set
status. `list-panes` exposes this metadata as `reported_status` and `status_reason`; its existing
`status` field continues to describe terminal readiness.

Session lifecycle commands are separate from the per-run control socket:

```bash
hyprmux dev                 # attach, or launch canonical same-name profile
hyprmux attach dev          # attach only; error unless running
hyprmux attach dev --read-only
hyprmux new dev             # explicitly create a fresh named session
hyprmux new review --profile dev # create from a reusable launch recipe
hyprmux list-sessions       # list connectable named sessions
hyprmux kill-session dev    # stop the named session server and its PTYs
hyprmux list-sessions --remote workbox
hyprmux kill-session dev --remote workbox
```

`kill-session <NAME>` is the sole canonical command for stopping a named session server. There is
one server per session; the command asks it to shut down through the authenticated protocol first,
then uses local stale-server recovery only when that local protocol transport cannot complete. It
destroys the server's PTYs for every attached client and is not a generic process killer. The
`--remote` form runs the same command over SSH and never force-terminates the local SSH transport.

Agents that need to control panes or session lifecycle should first read the operational contract
printed by `hyprmux --skill`.

An unknown positional target errors instead of silently creating a session. `attach` and `new` are
reserved command words; use `hyprmux --session attach` or `hyprmux --session new` to target those
literal names. See [Sessions](sessions.md) for profile resolution and attach/detach semantics.

The per-run control socket always belongs to the **local UI process**. When you are attached with
`--remote`, automation against that UI still uses the local `HYPRMUX_SOCKET` / `--socket` path.
`list-sessions --remote` and `kill-session --remote` are separate SSH helpers that talk to hyprmux
on the remote host; they are not control-socket commands. See [Remote SSH sessions](remote.md).

## Wire protocol

The socket accepts one newline-delimited JSON request per connection and returns one JSON response.
`subscribe` is the exception: after its acknowledgement, the connection remains open and streams
newline-delimited event objects until disconnected.

Requests use a `cmd` field: `list-panes`, `metrics`, `focus`, `send-text`, `send-keys`, `new-pane`,
`run-action`, `capture-pane`, `switch-workspace`, `move-to-workspace`, `set-status`, `popup`, or
`subscribe`. A client may include `source_pane`; the CLI derives it from `HYPRMUX_PANE`.

Examples:

```json
{"cmd":"send-keys","keys":["C-c"]}
{"cmd":"send-keys","keys":["echo hi","Enter"]}
{"cmd":"send-keys","keys":["C-c"],"literal":true}
{"cmd":"capture-pane","target":3}
{"cmd":"capture-pane","scrollback":"full"}
{"cmd":"capture-pane","scrollback":"last-output"}
{"cmd":"capture-pane","scrollback":200,"target":3}
```

```json
{"cmd":"metrics"}
{"cmd":"list-panes","source_pane":1}
{"cmd":"focus","target":2,"source_pane":1}
{"cmd":"send-text","target":2,"text":"hello"}
{"cmd":"new-pane","command":"cargo test","cwd":"/repo","title":"tests","keep_open":true}
{"cmd":"new-pane","command":"cargo watch","focus":true}
{"cmd":"run-action","action":"toggle-float"}
{"cmd":"capture-pane","target":2}
{"cmd":"switch-workspace","index":2}
{"cmd":"move-to-workspace","index":3}
{"cmd":"set-status","target":2,"status":"blocked","reason":"needs approval"}
{"cmd":"set-status","target":2,"status":null}
{"cmd":"popup","command":"fzf","width":0.7,"height":0.6,"title":"files"}
{"cmd":"subscribe","events":["pane-exited","pane-status-changed","workspace-switched"]}
```

Responses have `ok`, and either `data` or `error`.

Popup dimensions are viewport fractions, clamped to `0.2`-`0.95`; omitted dimensions default to
`0.6`. Only one popup may exist at a time. `cwd` defaults to the focused pane's working directory.
`keep_open` defaults to `true`: the popup holds after its command exits, printing the exit status
and retaining its final screen as a read-only result, so short commands stay readable. Enter,
Escape, or Space dismisses a completed popup. Pass `"keep_open":false` to have the popup close with
its command. A popup also closes when its backdrop is clicked or through the normal close action;
while the command is running, Escape is sent to the popup application.

Subscriptions support `pane-spawned`, `pane-exited`, `pane-status-changed`, `focus-changed`,
`workspace-switched`, `session-attached`, `session-detached`, `session-renamed`,
`controller-changed`, `client-joined`, `client-left`, `profile-loaded`, `profile-applied`,
`profile-saved`, `session-created`, and `config-reloaded`. An empty `events` list subscribes to all
16. `pane-status-changed` carries
`pane`, `status`, `reason`, `previous_status`, and `previous_reason`; cleared or absent values are
empty strings. Initial status received while attaching is state seeding, not a transition, and does
not emit this event.
Event names and existing fields are stable; later versions may add events or fields. See
[Hooks](hooks.md#events-and-fields) for the complete field table. Slow subscribers are bounded and
disconnected rather than blocking the UI. Example:
`printf '%s\n' '{"cmd":"subscribe"}' | socat - UNIX-CONNECT:$HYPRMUX_SOCKET | jq`.

## Pane logging

`{"cmd":"pane-logging","target":3,"enabled":true}` enables logging for pane 3. Omit `target`
to use the focused/source pane and omit `enabled` to toggle.
