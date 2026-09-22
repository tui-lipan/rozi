# Control protocol

This page documents Rozi's raw control transports. Prefer the portable commands in
[Control CLI](control.md) when a process can invoke `rozi`. The CLI handles endpoint discovery,
Windows named-pipe derivation, extension provenance, timeouts, and stream bridging.

There are two transports carrying the same request and response documents:

- A running UI serves newline-delimited JSON on a per-process control endpoint. Everything below
  describes this one unless it says otherwise.
- A named session server serves the same requests, wrapped in one length-prefixed session-protocol
  frame, on its session endpoint. See [Session transport](#session-transport).

## Transport

Each running UI creates one private control endpoint.

| Platform | Transport | Discovery path |
| --- | --- | --- |
| Linux | Unix-domain socket | `$XDG_RUNTIME_DIR/rozi/control-<pid>.sock`, else `/run/user/<uid>/rozi`, else the private fallback runtime directory |
| macOS | Unix-domain socket | Rozi's private runtime directory |
| Windows | Current-user named pipe | `%LOCALAPPDATA%\rozi\run\control-<pid>.sock` discovery entry |

On Windows, the discovery entry is not the pipe. Rozi derives the pipe name from the entry name and
the current user SID, then performs an authenticated handshake. The entry contents are not an
authority. The pipe rejects remote clients and uses a current-user DACL.

Unix endpoints use owner-only permissions. Runtime directory and endpoint validation reject unsafe
ownership, modes, and symlinks.

`ROZI_SOCKET` always names the discovery path accepted by the CLI. It is a socket path on Unix and
a discovery-entry path on Windows.

## Framing

The protocol is UTF-8 newline-delimited JSON. Incoming request and stream-update lines, including
the trailing newline, are at most 1 MiB. A larger incoming line is a protocol error and Rozi closes
the connection. Replies Rozi writes, including `capture-pane --scrollback full`, are not capped at
1 MiB.

For a one-shot command:

1. Connect to one endpoint.
2. Write one request object followed by `\n`.
3. Read one response object followed by `\n`.
4. The server closes the connection.

Rozi reads only the first request line. `subscribe`, `pick`, and `publish` keep the connection open
after the initial response.

The initial request line must arrive within three seconds. A one-shot command, stream authorization,
or stream-open acknowledgement may wait up to ten seconds for the UI.

## Request envelope

Every request has `cmd`. These optional envelope fields apply to commands:

| Field | Type | Meaning |
| --- | --- | --- |
| `source_pane` | integer or null | Calling pane. Used as the default target where supported. Ignored by a session endpoint, which is a different pane namespace — see [Session transport](#session-transport). |
| `extension` | object | Extension ownership with `id` and opaque `generation`. The CLI adds it from the extension environment. |

Do not synthesize extension provenance. A retired generation is rejected.

## Responses

A success has `ok: true` and may have `data`:

```json
{"ok":true,"data":{"id":3,"accepted":true,"pty_ready":true}}
```

A success with no payload is:

```json
{"ok":true}
```

An error has `ok: false`, a stable machine-readable `code`, and a human-readable `error`:

```json
{"ok":false,"code":"pane-not-found","error":"pane 3 not found"}
```

Scripts should branch on `code`; the message may gain context or change wording. Error codes are:
`invalid-request`, `request-timeout`, `message-too-large`, `extension-inactive`, `unknown-event`,
`pane-not-found`, `target-required`, `pane-not-running`, `session-not-attached`,
`session-not-connected`, `input-locked`, `read-only`, `not-controller`, `unsupported`,
`invalid-argument`, `spawn-failed`, `conflict`, `unavailable`, `agent-gone`, `agent-blocked`,
`agent-replaced`, `stale-reference`, `timeout`, and `request-failed`.
`request-failed` is the fallback for failures without a narrower category. Older servers may omit
`code`, so clients that support version skew must still handle that shape.

The shape inside `data` depends on `cmd`. CLI JSON output preserves this envelope.

| Command | `data` |
| --- | --- |
| `list-panes` | Array of pane objects. |
| `metrics` | Client counters and the most recent cached server counters. |
| `capture-pane` | `{ "id": number, "text": string, "title": string or null }` |
| `new-pane` | `{ "id": number, "accepted": bool, "pty_ready": bool }` |
| Other one-shot commands | Absent on success. |

A `list-panes` object has `session`, `id`, `reference`, `agent_ref`, `title`, `workspace`,
`command`, `argv`, `foreground_program`, `foreground_programs`, `foreground_arguments`, `cwd`,
`status`, `reported_status`, `status_reason`, `agent`, and `agent_state`. `reference` identifies one
server instance, pane id, and PTY generation. `agent_ref` adds the semantic incarnation and is null
when the pane has no detected agent. Optional values are JSON null. Scratch panes use workspace `0`
and do not expose a reference. `foreground_programs` contains the normalized basename-only
process-group evidence used by split-aware navigation; it may contain more than one entry for
wrappers and pipelines.

The metrics object has `sampled_at_unix_ms`, `client_inbound`, `client_outbound`, `piped_remote`,
`orphan_output`, and `server`. Queue and byte-buffer objects report current, high-water, and
capacity bytes. Cached server data also reports `age_ms` and `stale`.

Server metrics include `attach_seed`. Its `queued_bytes`, `peak_queued_bytes`, and
`send_window_bytes` measure baseline replay in socket outboxes. `live_catch_up_bytes`,
`peak_live_catch_up_bytes`, and `live_catch_up_limit_bytes` measure changes waiting behind replay.
The object also reports active clients, panes remaining, lifetime replay bytes, completed and
disconnected attach counts, last and maximum duration, and the last disconnect reason.

Server metrics also include `client_resync`, for clients that fell too far behind a pane's output.
Rozi drops that output for the client and replays the pane from the server's screen instead.
`active_clients` counts clients replaying now. `started` and `completed` count resyncs. `exports`
counts pane screens exported for them. `requeued_panes` counts panes a client fell behind again
before their replay finished, during an attach or a resync. `shed_bytes` is the pane output dropped
rather than delivered. `last_export_us` and `max_export_us` time one pane export for any replay,
which the server loop, and so every client, waits on.

## One-shot requests

### Pane inspection

```json
{"cmd":"list-panes"}
{"cmd":"metrics"}
{"cmd":"capture-pane","target":3}
{"cmd":"capture-pane","target":3,"scrollback":200}
{"cmd":"capture-pane","scrollback":"full"}
{"cmd":"capture-pane","scrollback":"last-output"}
```

`capture-pane.target` defaults to `source_pane`, then the focused pane. `scrollback` is a
nonnegative line count, `"full"`, or `"last-output"`.

`list-panes` reports launch intent in either `command` or `argv`. It also reports current foreground
program data, reported status, and detected agent data when available.

### Focus and input

```json
{"cmd":"focus","target":3}
{"cmd":"send-text","target":3,"text":"cargo test\n"}
{"cmd":"send-keys","target":3,"keys":["C-c","Enter"]}
{"cmd":"send-keys","target":3,"keys":["C-c"],"literal":true}
```

`send-text.target` and `send-keys.target` are optional. Rozi falls back to `source_pane`, then the
focused pane. `keys` uses the names documented in [Control CLI](control.md#sending-keys-and-capturing-output).

### Pane creation

Use at most one of `command` or `argv`. Omit both for an interactive shell:

```json
{"cmd":"new-pane","command":"cargo test","cwd":"/repo","title":"tests","keep_open":true}
{"cmd":"new-pane","argv":["cargo","test","--","path with spaces"],"workspace":9,"focus":false}
```

| Field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `command` | string or null | null | Command line interpreted by `command_shell`. |
| `argv` | string array or null | null | Direct executable and arguments. |
| `cwd` | string or null | focused pane cwd | Working directory. |
| `title` | string or null | generated | Initial title. |
| `keep_open` | bool | `false` | Replaces a finished command with a shell. |
| `focus` | bool | `false` | Focuses the new pane and its workspace. |
| `workspace` | integer or null | rule or current workspace | One-based workspace, `1..=9`. |

The response waits up to five seconds for the PTY ready signal and includes `id`, `accepted`, and
`pty_ready`.

### Actions, workspaces, status, and notifications

```json
{"cmd":"run-action","action":"toggle-float"}
{"cmd":"switch-workspace","index":2}
{"cmd":"move-to-workspace","index":3}
{"cmd":"set-status","target":3,"status":"blocked","reason":"needs approval"}
{"cmd":"set-status","target":3,"status":null}
{"cmd":"notify","message":"deploy finished"}
{"cmd":"notify","message":"tests failed","title":"Build","level":"error"}
```

Workspace indices are `1..=9`. `set-status.target` falls back to `source_pane`, then the focused
pane. Status and reason text is display-sanitized, trimmed, and limited to 64 and 256 characters.
An empty status clears the report. `notify.level` is `"info"` or `"error"`.

### Popup

```json
{"cmd":"popup","command":"fzf","cwd":"/repo","width":0.7,"height":0.6,"title":"files","keep_open":false}
```

`command` is interpreted by `command_shell`. Width and height are viewport fractions clamped to
`0.2..=0.95` and default to `0.6`. `cwd` defaults to the focused pane cwd. `keep_open` defaults to
`true`. Only one popup may exist at a time.

### Pane logging

```json
{"cmd":"pane-logging","target":3,"enabled":true}
{"cmd":"pane-logging","target":3,"enabled":false}
{"cmd":"pane-logging","target":3}
```

Omit `enabled` to toggle. Omit `target` to use `source_pane`, then the focused pane.

## Session transport

A named session server answers control requests on its own endpoint, with no UI in the picture.
This is what makes a detached session scriptable.

The endpoint is the session's, not a UI's: `$XDG_RUNTIME_DIR/rozi/session-<NAME>.sock` on Linux,
Rozi's private runtime directory on macOS, and a current-user named pipe behind an equivalent
discovery entry on Windows. The same ownership, mode, symlink, and DACL rules apply.

Unlike the UI endpoint, this one is framed, not line-delimited: it is the
[session protocol](sessions.md), so each message is a 4-byte big-endian length, a 1-byte frame
kind, and a JSON body. One exchange is:

1. Connect to the session endpoint.
2. Write one `session-control` frame.
3. Read one `session-control-result` frame.
4. The server closes the connection.

```json
{"type":"session-control","session":"dev","protocol_version":6,"min_protocol_version":6,
 "request":{"cmd":"capture-pane","target":3}}
```

```json
{"type":"session-control-result","effective_protocol":6,
 "response":{"ok":true,"data":{"id":3,"text":"…","title":"zsh"}}}
```

`response` is the same envelope the UI endpoint returns. A wrong session name or an incompatible
build is answered with a session-protocol `error` frame carrying `session-mismatch` or
`protocol-mismatch` instead, because neither is a rejected command.

The connection never becomes a client. It gets no client id, does not appear in the session roster
or client count (including `metrics`, which counts attached clients rather than open sockets),
never holds layout control, and receives no replay. A script cannot make an idle session look
occupied — and equally gains nothing an attached client would not have.

Supported requests are `list-panes`, `agents-list`, `agent-get`, `agent-read`, `agent-wait`,
`agent-prompt`, `agent-report`, `agent-release`, `metrics`, `capture-pane`, `send-text`,
`send-keys`, `new-pane`, `set-status`, and `pane-logging`. Every other `cmd` is answered with
`ok: false` and a reason naming what it needed a UI for; none is silently accepted.

Differences from the same request against a UI:

| Request | Against a session server |
| --- | --- |
| Any `target` | `source_pane` is ignored, and there is no focused-pane fallback. A pane id carries no session identity, so an inherited one would address a stranger in the named session. A session with one pane resolves to it; otherwise the error lists the pane ids. |
| `list-panes` | Every pane in the session, including exited ones, whose `status` is `exited (<CODE>)`. `workspace` comes from the shared layout, or `0` when the session has no layout document. |
| `metrics` | `server` only, sampled at request time, so `age_ms` is `0` and `stale` is `false`. Client counters are absent. |
| `new-pane` | `focus` must be `false`. Refused while any client holds layout control. The server re-reads `[[rules]]` and the configured shell from its own config, picks the pane id, appends the pane to the resolved workspace (default 1) in the shared layout, and broadcasts the new revision authored by client `0`. `pty_ready` reports whether the PTY spawned. |
| `send-text`, `send-keys` | Refused while the session's input lock is on, which only an attached client can release. |
| Any request with `extension` provenance | Refused. The generation is a fencing token minted per UI process; a server cannot check it and does not act on an extension's behalf without checking. |

The CLI speaks this transport for `rozi --session <NAME> <COMMAND>`. As with the UI endpoint,
prefer invoking `rozi` over opening the endpoint yourself: the framing, the Windows pipe
derivation, and the session-name validation are all handled there.

## Subscription stream

Open with:

```json
{"cmd":"subscribe","events":["pane-exited","pane-status-changed"]}
```

An empty or absent `events` array subscribes to all events. Unknown event IDs reject the request.
After `{"ok":true}`, the server writes event objects until either side disconnects:

```json
{"event":"pane-exited","data":{"pane":"3","code":"1","focused":"false"}}
```

All event fields are strings nested under `data`. See [Hooks](hooks.md#events-and-fields) for the
event list and fields. A subscriber queue holds 128 events. A slow subscriber is disconnected
instead of blocking the UI.

Portable bridge:

```sh
rozi subscribe pane-exited pane-status-changed
```

## Picker stream

Open with:

```json
{"cmd":"pick","title":"Branches","placeholder":"Filter","empty":"No branches","width":72,"actions":[{"id":"new","key":"ctrl-n","label":"new","prompt":"Branch name"}]}
```

If another picker or modal overlay is open, Rozi returns an error and closes the connection.
Otherwise it sends `{"ok":true}`. `title` defaults to `"Pick"`, `placeholder` defaults to
`"Search…"`, and `width` defaults to 60 columns and is clamped to `30..=120`. `empty` is optional
producer copy for a row list that is empty while the filter is empty. A nonempty filter with no
matching rows always shows `No matches`; omitted `empty` leaves the list's ordinary empty
appearance. Actions with an empty ID or invalid key chord are omitted.

`prompt` and any future form are substates of this picker, not a second overlay. A second `pick`
while one is open is still refused.

`tabs` turns the picker into pages under one title, shown as a tab strip above the query. Each tab
has its own rows, filter text, and highlight, so switching away and back keeps what was typed.
`tab` names the tab to open on; an omitted or unknown `tab` opens the first. Tabs with an empty or
repeated `id` are omitted, and Rozi keeps at most 32. Actions, `placeholder`, `empty`, and `width`
apply to every tab.

A Rozi without tab support ignores `tabs` and `tab`, so every snapshot would replace one shared
list. Check for the `picker-tabs` capability in `rozi api describe` before declaring tabs.

```json
{"cmd":"pick","title":"Git","tabs":[{"id":"branches","label":"Branches"},{"id":"worktrees","label":"Worktrees"}],"tab":"branches"}
```

| Field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `id` | string | required | Names the tab in row snapshots and replies. Row IDs only need to be unique within a tab. |
| `label` | string | `id` | Tab strip text. |

`Tab`/`Shift+Tab` and `Right`/`Left` cycle tabs, and a click on a tab selects it, unless an action
claims that key.

The client may then write row snapshots. Each line replaces the full row set. Rozi keeps at most
512 rows from each snapshot. In a tabbed picker, a snapshot names its tab and replaces only that
tab's rows; one without `tab` or with an undeclared `tab` is ignored, as is a snapshot with `tab`
sent to an untabbed picker. Rows may arrive for a hidden tab at any time.

```json
{"tab":"worktrees","rows":[{"id":"/src/rozi-review","label":"rozi-review","description":"~/src/rozi-review"}]}
```

```json
{"rows":[{"id":"main","label":"main","description":"current","group":"Local","active":true},{"id":"old","label":"old","disabled":"protected"}]}
```

Row fields:

| Field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `id` | string | `label` | Value returned as `selected`. |
| `label` | string | required | Visible row text. |
| `description` | string | none | Right-aligned detail. Clipped, or dropped entirely, when the label needs the room. |
| `group` | string | none | Section label. |
| `disabled` | string | none | Makes the row inert and shows the reason. |
| `active` | bool | `false` | Marks the current item. |
| `priority` | integer | `0` | Adds sorting weight. |

Selection writes one terminal object:

```json
{"selected":"main"}
```

Cancellation writes:

```json
{"cancelled":true}
```

An action writes an object and keeps the picker open unless that action declared `close: true`:

```json
{"action":"delete","selected":"old"}
{"action":"new","input":"feat/api","selected":"main"}
```

A tabbed picker adds the active tab to selections and actions, and reports each tab switch without
closing, so a producer can fill a tab only when it is first shown:

```json
{"tab":"worktrees"}
{"action":"delete","selected":"old","tab":"branches"}
{"selected":"main","tab":"branches"}
```

Lines that keep the picker open, actions and tab switches, are queued for the client. If it stops
reading and 64 are waiting, later ones are dropped. The terminal line — selection, cancellation, or
a closing action — is never dropped and always arrives last.

Action fields:

| Field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `id` | string | required | Returned as `action`. |
| `key` | string | required | One valid key chord. |
| `label` | string | required | Footer label. |
| `prompt` | string or object | none | Opens a stacked text prompt over the picker and returns `input`. A string is the title. An object may also set `placeholder`, a seed `value`, and `masked`. |
| `close` | bool | `false` | Closes after the action. |
| `confirm` | bool | `false` | Requires a second press on the same row. |

Prompt object fields:

| Field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `title` | string | required | Prompt title. |
| `placeholder` | string | none | Empty-field hint. |
| `value` | string | none | Initial contents. |
| `masked` | bool | `false` | Hide typed characters on screen. The submitted `input` is still plaintext on the stream. |

The string form is the shortcut for `{ "title": "…" }`:

```json
{"prompt":"Command"}
{"prompt":{"title":"Edit command","placeholder":"git status","value":"git status --short"}}
{"prompt":{"title":"Token","masked":true}}
```

The CLI bridge uses a simpler plain-line mode or a JSON mode. In JSON mode, the first stdin line
contains picker metadata and optional initial `rows`, which fill the tab the picker opens on; later
lines are row snapshots:

```sh
printf '%s\n' '{"title":"Branches","rows":[{"id":"main","label":"main"}]}' |
  rozi pick --json
```

## Published activity stream

Open with:

```json
{"cmd":"publish","source_pane":3}
```

After `{"ok":true}`, the publisher writes complete snapshots:

```json
{"rows":[{"id":"job-1","title":"Run tests","status":"working","reason":"crate core","active":true}]}
```

Row fields:

| Field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `id` | string | required | Stable publisher-owned identity. |
| `title` | string | required | Activity title. May be empty. |
| `status` | string | required | Status value. |
| `reason` | string or null | null | Supporting detail. |
| `active` | bool | `false` | Row currently visible inside the publisher. At most one should be active. |
| `work_started_at` | integer or null | server-owned | Values sent by publishers are replaced. |

An empty list withdraws the rows. EOF or any stream failure also withdraws them.
IDs, titles, and statuses are display-sanitized and limited to 64 characters; reasons are limited to
256. Rows with an empty ID or status are dropped. Rozi keeps `active: true` only on the first active
row in a snapshot.

When a user activates a row, Rozi focuses its pane and writes:

```json
{"activate":"job-1"}
```

The publisher must keep reading activations. Rozi drops a stream whose activation backlog reaches
its bound and withdraws its rows.

`source_pane` selects row ownership. If it is absent, Rozi resolves the focused live pane when the
stream opens. This permits a supervised service to publish, but the rows still belong to that
resolved pane.

Nonempty published rows make the program's own activity list authoritative for that pane. If Rozi
has already identified an agent, it aggregates the rows into that agent's displayed state using
blocked first, then any status other than `idle` or `done`, then quiescent rows. A publisher in an
otherwise unrecognized pane still gets Activity rows but does not invent a detected agent identity.

Portable bridge:

```sh
rozi publish
```

## Stream ownership

The CLI attaches extension `id` and `generation` when both are present in its environment.
Extension-owned picker, publisher, and subscription streams close when the generation retires due
to disable, removal, or a process-facing extension change. Metadata-only changes keep the
generation.
