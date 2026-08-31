# Control protocol

This page documents Rozi's raw UI control transport. Prefer the portable commands in
[Control CLI](control.md) when a process can invoke `rozi`. The CLI handles endpoint discovery,
Windows named-pipe derivation, extension provenance, timeouts, and stream bridging.

## Transport

Each running UI creates one private control endpoint.

| Platform | Transport | Discovery path |
| --- | --- | --- |
| Linux | Unix-domain socket | `$XDG_RUNTIME_DIR/rozi/control-<pid>.sock`, or the private fallback runtime directory |
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

The protocol is UTF-8 newline-delimited JSON.

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
| `source_pane` | integer or null | Calling pane. Used as the default target where supported. |
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

An error has `ok: false` and `error`:

```json
{"ok":false,"error":"pane 3 not found"}
```

The shape inside `data` depends on `cmd`. CLI JSON output preserves this envelope.

| Command | `data` |
| --- | --- |
| `list-panes` | Array of pane objects. |
| `metrics` | Client counters and the most recent cached server counters. |
| `capture-pane` | `{ "id": number, "text": string, "title": string or null }` |
| `new-pane` | `{ "id": number, "accepted": bool, "pty_ready": bool }` |
| Other one-shot commands | Absent on success. |

A `list-panes` object has `session`, `id`, `title`, `workspace`, `command`, `argv`,
`foreground_program`, `foreground_arguments`, `cwd`, `status`, `reported_status`,
`status_reason`, `agent`, and `agent_state`. Optional values are JSON null. Scratch panes use
workspace `0`.

The metrics object has `sampled_at_unix_ms`, `client_inbound`, `client_outbound`, `piped_remote`,
`orphan_output`, and `server`. Queue and byte-buffer objects report current, high-water, and
capacity bytes. Cached server data also reports `age_ms` and `stale`.

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
{"cmd":"pick","title":"Branches","placeholder":"Filter","width":72,"actions":[{"id":"new","key":"ctrl-n","label":"new","prompt":"Branch name"}]}
```

If another picker or modal overlay is open, Rozi returns an error and closes the connection.
Otherwise it sends `{"ok":true}`. `title` defaults to `"Pick"`, `placeholder` defaults to
`"Search…"`, and `width` defaults to 60 columns and is clamped to `30..=120`. Actions with an empty
ID or invalid key chord are omitted.

The client may then write row snapshots. Each line replaces the full row set:

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

Action fields:

| Field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `id` | string | required | Returned as `action`. |
| `key` | string | required | One valid key chord. |
| `label` | string | required | Footer label. |
| `prompt` | string | none | Replaces the picker with a text prompt and returns `input`. |
| `close` | bool | `false` | Closes after the action. |
| `confirm` | bool | `false` | Requires a second press on the same row. |

The CLI bridge uses a simpler plain-line mode or a JSON mode. In JSON mode, the first stdin line
contains picker metadata and optional initial `rows`; later lines are row snapshots:

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
