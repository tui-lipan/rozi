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
hyprmux focus 3
hyprmux send-text 'cargo test
'
hyprmux send-keys C-c
hyprmux send-keys 'echo hi' Enter
hyprmux send-keys -l C-c
hyprmux split 'claude --agent helper'
hyprmux run-action toggle-float
hyprmux capture-pane --target 3
hyprmux capture-pane --scrollback full
hyprmux capture-pane --scrollback 200 --target 3
hyprmux switch-workspace 2
hyprmux move-to-workspace 3
hyprmux status blocked --reason "needs approval"
hyprmux status --clear
```

Replies are JSON on stdout. Errors are JSON when returned by the server, or plain stderr for client
discovery/connect failures. `split`/`new-pane` replies as soon as the pane is accepted by the UI; the
PTY may still be starting (`pty_ready: false`).

`send-keys` accepts tmux-style key names (`C-c`, `M-x`, `Enter`, `Escape`, `Space`, `Tab`,
`BSpace`, arrows, `Home`/`End`, `PgUp`/`PgDn`, `F1`..`F12`) mixed with literal text arguments.
Unknown tokens are sent as literal UTF-8. `-l` / `--literal` forces every argument to be treated as
literal text (so `send-keys -l C-c` types the characters `C-c` instead of Ctrl+C).

`run-action` takes any keybindable action's stable id (the same ids used in `[keys]` config and
shown in the command palette/help overlay), e.g. `toggle-float`, `spawn`, `close`. `capture-pane`
returns the plain text of a pane's current visible snapshot grid by default, or scrollback history
with `--scrollback N` / `--scrollback full`. It defaults to the request's `source_pane` or the
focused pane when `--target`/`target` is omitted. `switch-workspace` and
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
hyprmux kill-session dev    # request clean shutdown of a named session
```

An unknown positional target errors instead of silently creating a session. `attach` and `new` are
reserved command words; use `hyprmux --session attach` or `hyprmux --session new` to target those
literal names. See [Sessions](sessions.md) for profile resolution and attach/detach semantics.

## Wire protocol

The socket accepts one newline-delimited JSON request per connection and returns one JSON response.
`subscribe` is the exception: after its acknowledgement, the connection remains open and streams
newline-delimited event objects until disconnected.

Requests use a `cmd` field: `list-panes`, `focus`, `send-text`, `send-keys`, `new-pane`, `run-action`,
`capture-pane`, `switch-workspace`, `move-to-workspace`, `set-status`, `popup`, or `subscribe`. A
client may include `source_pane`; the CLI derives it from `HYPRMUX_PANE`.

Examples:

```json
{"cmd":"send-keys","keys":["C-c"]}
{"cmd":"send-keys","keys":["echo hi","Enter"]}
{"cmd":"send-keys","keys":["C-c"],"literal":true}
{"cmd":"capture-pane","target":3}
{"cmd":"capture-pane","scrollback":"full"}
{"cmd":"capture-pane","scrollback":200,"target":3}
```

```json
{"cmd":"list-panes","source_pane":1}
{"cmd":"focus","target":2,"source_pane":1}
{"cmd":"send-text","target":2,"text":"hello"}
{"cmd":"new-pane","command":"cargo test","cwd":"/repo","title":"tests","keep_open":true}
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
`0.6`. Only one popup may exist at a time. It closes when its command exits, when its backdrop is
clicked, or through the normal close action. Escape is sent to the popup application.

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
