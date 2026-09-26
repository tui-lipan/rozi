# Control protocol

This page is the wire reference for clients that cannot run the `rozi` CLI: how to connect to a
control endpoint, frame requests, and read responses and streams. Most scripts should call `rozi`
instead, as described in [Control CLI](control.md); the CLI handles endpoint discovery, Windows
named-pipe derivation, extension provenance, timeouts, and stream bridging for you.

rozi serves the same request and response documents on two transports:

- A running UI serves newline-delimited JSON on its own control endpoint. Everything on this page
  describes this transport unless it says otherwise.
- A named session server serves the same requests on its session endpoint, each wrapped in one
  length-prefixed session-protocol frame. See [Session transport](#session-transport).

The complete request vocabulary, including the agent requests (`agents-list`, `agent-get`,
`agent-read`, `agent-wait`, `agent-prompt`, `agent-report`, and `agent-release`) that this page
does not spell out, is described by the [JSON Schema](control.md#json-schema).

## Transport

Each running UI creates one private control endpoint.

| Platform | Transport | Discovery path |
| --- | --- | --- |
| Linux | Unix-domain socket | `$XDG_RUNTIME_DIR/rozi/control-<pid>.sock`, else `/run/user/<uid>/rozi`, else the private fallback runtime directory |
| macOS | Unix-domain socket | rozi's private runtime directory |
| Windows | Current-user named pipe | `%LOCALAPPDATA%\rozi\run\control-<pid>.sock` discovery entry |

`ROZI_SOCKET` always names the discovery path the CLI accepts: a socket path on Unix, a
discovery-entry path on Windows.

On Windows, the discovery entry is not the pipe, and its contents are not an authority. rozi derives
the pipe name from the entry name and the current user's SID, then performs an authenticated
handshake. The pipe rejects remote clients and uses a current-user DACL.

On Unix, endpoints have owner-only permissions, and rozi rejects runtime directories and endpoints
with unsafe ownership, modes, or symlinks.

## Framing

The protocol is UTF-8 newline-delimited JSON. For a one-shot command:

1. Connect to one endpoint.
2. Write one request object followed by `\n`.
3. Read one response object followed by `\n`.
4. The server closes the connection.

rozi reads only the first request line. `subscribe`, `pick`, and `publish` keep the connection open
after the initial response; see the stream sections below.

Limits:

- Each incoming line — a request or a stream update — is at most 1 MiB, including the trailing
  newline. A larger line is a protocol error, and rozi closes the connection.
- Replies from rozi have no size cap. `capture-pane --scrollback full` can exceed 1 MiB.
- The request line must arrive within three seconds.
- A one-shot command, a stream authorization, or a stream-open acknowledgement may wait up to ten
  seconds for the UI. A request with a [pane wait](#pane-waits) may wait for its `timeout_ms` plus
  ten seconds.

## Request envelope

Every request has `cmd`. Any request may also carry these fields:

| Field | Type | Meaning |
| --- | --- | --- |
| `source_pane` | integer or null | Calling pane, used as the default target where a command supports it. A session endpoint ignores it; see [Session transport](#session-transport). |
| `source_session` | string | The session server `source_pane` belongs to, from the caller's `ROZI_SESSION_INSTANCE`. Only the recording requests sent to a UI read it; see [Pane recordings](#pane-recordings). |
| `extension` | object | Extension ownership, with `id` and an opaque `generation`. The CLI adds it from the extension environment. |

Do not synthesize extension provenance. A request with a retired generation is rejected.

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

Branch on `code`; the message may gain context or change wording. The error codes are:

`invalid-request`, `request-timeout`, `message-too-large`, `extension-inactive`, `unknown-event`,
`pane-not-found`, `target-required`, `pane-not-running`, `session-not-attached`,
`session-not-connected`, `input-locked`, `read-only`, `not-controller`, `unsupported`,
`invalid-argument`, `spawn-failed`, `conflict`, `unavailable`, `agent-gone`, `agent-blocked`,
`agent-replaced`, `stale-reference`, `timeout`, and `request-failed`.

An error normally has no `data`. A failed [pane wait](#pane-waits) is the exception: `timeout` and
`pane-not-running` carry the pane's capture in `data` when the pane still exists.

`request-failed` is the fallback for a failure with no narrower category. Older servers may omit
`code`, so a client that supports version skew must also handle an error without it.

### Response data

The shape inside `data` depends on `cmd`. The CLI's JSON output keeps this envelope.

| Command | `data` |
| --- | --- |
| `list-panes` | Array of pane objects. |
| `layout-get` | A layout report; see [Control CLI](control.md#layout). |
| `layout-set`, `pane-set`, `pane-move`, `pane-swap` | `{ "changed": bool, "revision": number or null, "committed": bool, "workspace": object }`; see [Changing the layout](control.md#changing-the-layout). |
| `pane-close` | `{ "id": number, "revision": number or null, "committed": bool, "workspace": object or absent }` |
| `metrics` | Client counters and the most recent cached server counters. |
| `capture-pane` | `{ "id": number, "title": string or null, "render": "text" or "ansi", "text": string }`, for `png` `{ "id": number, "title": string or null, "render": "png", "png_base64": string }`, or for `spans` `{ "id": number, "title": string or null, "render": "spans", "frame": object }`; see [Spans frames](#spans-frames) |
| `capture-ui` | `{ "width": number, "height": number }` plus the same `render` and `text`, `png_base64`, or `frame` fields as `capture-pane` |
| `send-text`, `send-keys` with `capture` | The same capture as `capture-pane`, taken once the wait resolved. Absent without `capture`. |
| `new-pane` | `{ "id": number, "accepted": bool, "pty_ready": bool }` |
| Other one-shot commands | Absent on success. |

A `list-panes` pane object has these fields; optional values are JSON null:

`session`, `id`, `reference`, `agent_ref`, `title`, `workspace`, `command`, `argv`,
`foreground_program`, `foreground_programs`, `foreground_arguments`, `cwd`, `status`,
`reported_status`, `status_reason`, `agent`, and `agent_state`.

- `reference` identifies one server instance, pane id, and pane process generation.
- `agent_ref` adds the agent's incarnation, and is null when the pane has no detected agent.
- Scratch panes use workspace `0` and have no `reference`.
- `foreground_programs` lists the basenames of the pane's foreground processes. Wrappers and
  pipelines can produce more than one entry.

### Metrics

The `metrics` object has `sampled_at_unix_ms`, `client_inbound`, `client_outbound`, `piped_remote`,
`orphan_output`, and `server`.

Byte buffers report `current_bytes`, `high_water_bytes`, and `capacity_bytes`. Queues add
`queued_items`. `orphan_output` adds `keys` and `capacity_keys`.

`server` is the most recent cached server sample, with `age_ms` and `stale` beside these fields:

| Field | Contents |
| --- | --- |
| `sampled_at_unix_ms` | When the server took the sample. |
| `pty_ingress` | Queue of pane output read by the server and not yet processed. |
| `client_outboxes` | Byte buffer of output waiting to be sent to clients, plus `clients`, the number of attached clients. |
| `attach_seed` | Replay sent to clients while they attach. |
| `client_resync` | Replay for clients that fell too far behind a pane's output. |
| `resurrection` | Snapshots written for [resurrection](sessions.md#resurrection). |
| `recordings` | [Pane recordings](recording.md): `active` now, `started` and `finished` so far, and `frames`, `bytes`, and `dropped` across all of them. |

`attach_seed` fields:

- `queued_bytes`, `peak_queued_bytes`, and `send_window_bytes` measure baseline replay waiting in
  socket outboxes.
- `live_catch_up_bytes`, `peak_live_catch_up_bytes`, and `live_catch_up_limit_bytes` measure
  changes waiting behind the replay.
- `active_clients`, `panes_remaining`, `replay_bytes_total`, `completed`, `disconnected`,
  `last_duration_us`, `max_duration_us`, and `last_disconnect_reason` report progress and history.

`client_resync` covers a client that fell too far behind a pane: rozi drops that output for the
client and replays the pane from the server's screen instead.

- `active_clients` counts clients replaying now. `started` and `completed` count resyncs.
- `exports` counts pane screens exported for them.
- `requeued_panes` counts panes a client fell behind again before their replay finished, during an
  attach or a resync.
- `shed_bytes` is pane output dropped rather than delivered.
- `last_export_us` and `max_export_us` time one pane export for any replay. The server, and so
  every client, waits on each export.

`resurrection` reports `attempts`, `successes`, `failures`, `last_duration_us`, `max_duration_us`,
`last_blocking_us`, `max_blocking_us`, `last_exported_panes`, `last_reused_panes`, and
`last_exported_bytes`.

## One-shot requests

### Pane inspection

```json
{"cmd":"list-panes"}
{"cmd":"layout-get"}
{"cmd":"layout-get","workspace":2}
{"cmd":"metrics"}
{"cmd":"capture-pane","target":3}
{"cmd":"capture-pane","target":3,"scrollback":200}
{"cmd":"capture-pane","scrollback":"full"}
{"cmd":"capture-pane","scrollback":"last-output"}
{"cmd":"capture-pane","target":3,"render":"png"}
{"cmd":"capture-pane","target":3,"wait":{"text":"$ ","timeout_ms":5000}}
{"cmd":"capture-ui","render":"png"}
{"cmd":"capture-ui","render":"png","scale":2}
{"cmd":"capture-pane","target":3,"render":"spans","image_pixels":true}
```

`list-panes` reports how each pane was launched in either `command` or `argv`, plus its current
foreground program, reported status, and detected agent when available. `recording` is `true` while
the session server records the pane, and absent otherwise.

`layout-get.workspace` is optional and one-based. A number outside `1`–`9` fails with
`invalid-argument`.

`capture-pane` fields:

- `target` defaults to `source_pane`, then the focused pane.
- `scrollback` is a nonnegative line count, `"full"`, or `"last-output"`.
- `render` is `"text"` (the default), `"ansi"`, `"png"`, or `"spans"`. `ansi`, `png`, and `spans`
  capture the visible grid only, and fail with `invalid-argument` when `scrollback` is set.
- `scale` enlarges a PNG, from 1 to 3; it defaults to 1. It fails with `invalid-argument` when out
  of range or with any other `render`.
- `wait` holds the reply until the visible screen shows some text or settles; see
  [Pane waits](#pane-waits).
- `image_pixels`, when `true`, adds each image's pixels to a `spans` frame. It defaults to
  `false` and fails with `invalid-argument` with any other `render`.

`capture-ui` captures the whole UI as drawn and takes `render`, `scale`, and `image_pixels` the
same way. It answers from the next frame the UI paints, which the request forces. Requests that
arrive before that paint share it, and each distinct form among them is encoded once. Only a UI
answers `capture-ui`.

### Spans frames

A `spans` capture's `frame` describes the visible grid. It has its own `format`, always
`"rozi-spans"`, and an integer `version`, currently `1`, apart from the control API's versions.
The version changes only when a field changes meaning or is removed; fields are added without a
change, so a consumer must ignore fields it does not know. The frame is `SpanFrame` in the
[JSON Schema](control.md#json-schema).

| Field | Contents |
| --- | --- |
| `format`, `version` | `"rozi-spans"` and the frame format's version. |
| `width`, `height` | The grid's size in cells. |
| `palette` | `foreground`, `background`, and the 16 `ansi` colors, each `"#rrggbb"`: what a default color and each ANSI name look like in this capture, as a PNG of it draws them. |
| `cursor` | `x`, `y`, `visible`, `shape` (`"block"`, `"hollow-block"`, `"underline"`, or `"bar"`), `blinking`, and `color` when the cursor has its own. Absent when the frame has no cursor. |
| `rows` | One array per row, top to bottom, of runs. |
| `images` | Images a program displayed, back to front. Absent when there are none. |

Each run has `x`, `width`, and `text`, plus only the style fields that differ from the default:

- `fg`, `bg`, and `underline_color` are a color. An absent `underline_color` means the underline
  takes the text's color.
- `bold`, `dim`, `italic`, `reverse`, and `strikethrough` appear as `true`.
- `underline` is `"single"`, `"double"`, `"curly"`, `"dotted"`, or `"dashed"`.

Runs tile each row from column 0 in order, without gaps or overlaps, except that blank cells in the
default style at the end of a row are left out: a column past the last run is a space in default
colors, and a blank row is `[]`. A wide character covers two columns of its run's `width`, so
`width` can exceed the number of characters in `text`.

A color stays symbolic rather than being resolved to RGB, in one of three forms:

- an ANSI name: `"black"`, `"red"`, `"green"`, `"yellow"`, `"blue"`, `"magenta"`, `"cyan"`,
  `"white"`, or one of those prefixed with `bright-`, such as `"bright-black"`. ANSI slots 0–15
  are always named, so `SGR 31` and `SGR 38;5;1` are both `"red"` and can share a run;
- a number from 16 to 255, a 256-color palette index. Indexes 16–255 are the standard xterm color
  cube and gray ramp, and do not appear in `palette`;
- `"#rrggbb"`.

An image has `x`, `y`, `width`, and `height`, the cells it is laid out over, and `pixel_width` and
`pixel_height`, the size of its pixels as captured. An image that runs past the grid is cropped to
it first. The cells under a visible image hold `▀` half blocks in its top and bottom colors, so the
runs there describe a coarse copy of the picture. `visible` is absent when the image shows in every
cell of its area; otherwise it has one entry per row of the area, each an array of `[x, width]`
column ranges still showing the image. `png_base64` holds the pixels as a PNG, with alpha, when the
request set `image_pixels`. Pixels on a cell that does not show the image, because something covers
it or it is off the grid, are fully transparent, so the PNG never reveals what the capture hides.
Pixels are placed on cells as a `png` capture draws them: fitted inside the image's cells from the
top-left corner, keeping their shape, in cells twice as tall as they are wide. A pixel that
straddles a hidden cell is cleared.

### Pane recordings

```json
{"cmd":"record-start","target":3}
{"cmd":"record-start","target":3,"output":"/home/me/agent.rozirec","max_fps":30,"duration_ms":28800000}
{"cmd":"record-start","target":3,"output":"/home/me/demo.rozirec","follow":true}
{"cmd":"record-list"}
{"cmd":"record-mark","label":"tests started","target":3}
{"cmd":"record-stop","id":1}
```

A session server answers these. A UI passes `record-start`, `record-stop`, `record-list`, and
`record-mark` to the session it is attached to and returns the server's answer, which
`attached-control` in `rozi api describe` advertises. See [Record a pane](recording.md) for what a
recording holds.

`record-start` fields:

- `target` is the pane. With one pane in the session it may be left out.
- `output` is an absolute path on the session server's host. A relative path fails with
  `invalid-argument`; the CLI makes one absolute first. An existing path fails with `conflict`
  unless `force` is `true`, which replaces a regular file only. Without `output`, the server names
  a new file in its [recordings directory](recording.md#where-the-file-goes) and `force` has no
  effect.
- `max_fps` (1–120), `duration_ms` (up to 7 days), and `max_bytes` (at least 64 KiB) bound the
  recording. Each one left out takes the server's `[recording]` setting, by default 30, 24 hours,
  and 1 GiB.
- `follow`, when `true`, holds the reply until the recording ends and stops the recording when the
  connection closes. The reply is then one stopped recording, as described for `record-stop`.

Without `follow`, `record-start` answers at once with a recording: `id`, `session`, `pane`, `path`,
`started_at_unix_ms`, `elapsed_ms`, `max_fps`, `duration_ms`, `max_bytes`, `follow`, and the
running totals `frames`, `keyframes`, `deltas`, `images`, `marks`, `dropped`, and `bytes`.
`record-list` answers with an array of them.

`record-stop` takes `id`, or `target` to stop every recording of that pane; with neither, it stops
the one recording running. Naming both fails with `invalid-argument`, as does a `target` that is not
being recorded. It answers once every file is complete, with `stopped`: an array in `id` order, each
with `id`, `pane`, `path`, `reason`, `elapsed_ms`, the final totals, and `error` when writing
failed.

```json
{"ok":true,"data":{"stopped":[{"id":1,"pane":3,"path":"/home/me/agent.rozirec","reason":"stopped","elapsed_ms":61250,"frames":212,"keyframes":3,"deltas":209,"images":0,"marks":1,"dropped":0,"bytes":88213}]}}
```

`record-mark` takes a `label` of up to 256 characters and an optional `id` or `target`, marks every
running recording without either, and answers with the `ids` it marked. A recording that is
ending, or whose writer is too far behind, cannot take a mark: naming it fails with `unavailable`,
and a request without `id` fails when no running recording took the mark.

Sent to a UI, these requests differ in a few ways:

- A request with `source_session` goes to that session, even while the UI shows another one and
  keeps it in the background. Its pane numbers apply to `target`. A `source_session` the
  UI is not attached to fails with `session-not-attached`, and a `source_pane` without one fails
  with `unsupported`, since a pane number alone does not say whose pane it is. A request with
  neither goes to the session on screen.
- `record-start` without `target` records `source_pane`, then the focused pane, and the UI names
  that pane to the server. `target` names a session pane even when a scratch or popup pane has the
  same number. The focused pane fails with `unsupported` while the scratchpad has focus.
- `record-stop` and `record-mark` with neither `id` nor `target` act on `source_pane` when there is
  one.
- `output` is sent as written. The session server requires a path that is absolute on its own
  host, so a Windows session takes `C:\…` and a Linux one `/…`, whatever the caller runs on.
- `follow: true` fails with `invalid-argument`; a foreground recording needs the session transport.
- A UI with no session fails with `session-not-attached`, and one whose session disconnects before
  answering fails with `session-not-connected`.
- A UI attached read-only may send `record-list` only. The others fail with `read-only`. The
  session's input lock does not apply, because a recording reads a pane rather than typing into it.
- A request with `extension` provenance fails, as it does on the session transport.
- The UI waits up to 60 seconds for `record-stop`, then fails with `request-timeout`.

### UI recordings

```json
{"cmd":"record-ui-start"}
{"cmd":"record-ui-start","output":"/home/me/demo.rozirec","max_fps":20}
{"cmd":"record-ui-mark","label":"opened the palette"}
{"cmd":"record-ui-stop"}
```

A UI answers these, and records itself: every frame it paints, chrome included. A session server
refuses them with `unsupported`. `record-ui` in `rozi api describe` advertises them. See
[Record the whole UI](recording.md#record-the-whole-ui).

`record-ui-start` takes the `output`, `max_fps`, `duration_ms`, `max_bytes`, and `force` fields of
`record-start`, with the UI's own `[recording]` settings for the ones left out and its own host for
the file. It answers once the first frame is in the file, with `path`, `started_at_unix_ms`,
`max_fps`, `duration_ms`, and `max_bytes`. A UI already recording itself fails with `conflict`.

`record-ui-stop` answers once the file is complete, with one stopped recording as described for
`record-stop`, without `id` or `pane`. `record-ui-mark` takes a `label` and answers with no data.
Both fail with `invalid-argument` when the UI is not recording, and a mark fails with `unavailable`
while the recording is starting or ending. The UI waits up to 60 seconds for `record-ui-stop`.

### Recording format

A recording file is line-delimited JSON in UTF-8. The first line is a header, and every later line
is one event. rozi appends lines as the recording runs, so a file that was cut short is valid up to
its last complete line: a reader ignores a final line with no newline. The header and events are
`RecordingHeader` and `RecordingEvent` in the [JSON Schema](control.md#json-schema).

The header has:

| Field | Contents |
| --- | --- |
| `format`, `version` | `"rozi-recording"` and the format's version, currently `1`. |
| `rozi` | The version of rozi that wrote the file. |
| `target` | What was recorded: `{"kind":"pane","session":…,"pane":…}`, or a whole UI, `{"kind":"ui","session":…}`, where `session` is the one it showed when the recording started and is absent without one. A reader from this version on shows a `kind` it does not know as an unknown target; a rozi from before UI recordings refuses a `ui` file. |
| `width`, `height` | The screen's size in cells when the recording started. |
| `started_at_unix_ms` | When it started. |
| `max_fps`, `keyframe_interval_ms` | The frame-rate ceiling, and the longest gap between keyframes. |
| `spans_version` | The [spans frame](#spans-frames) version of every frame in the file. |
| `palette` | The colors the screen was drawn in at the start, as in a spans frame. |
| `compression` | Absent. Reserved for compressing the lines after the header; a reader refuses a value it does not know. |

The format's version changes only when a field changes meaning or goes away. A reader ignores
fields and event kinds it does not know, and refuses a file whose `version` or `spans_version` is
newer than it reads. The version is independent of the control API, the session
protocol, and the spans frame version.

Every event has `kind` and `t`, milliseconds since the recording started. Events are in time order,
and a mark or meta event follows the frame that was showing when it happened:

| `kind` | Contents |
| --- | --- |
| `keyframe` | `frame`, a whole [spans frame](#spans-frames). Written first, after a resize, and at least every `keyframe_interval_ms` or 1,000 deltas while the screen changes, so a player can start at any keyframe. |
| `delta` | What changed since the previous frame: `rows`, and `cursor`, `images`, or `palette` when they changed. A `cursor` of `null` means the frame no longer has one. |
| `resize` | `width` and `height`. A keyframe at the new size follows. |
| `image` | `id`, `pixel_width`, `pixel_height`, and `png_base64`: an image's pixels, stored once and written before the first frame that shows it. The pixels are whole, not cleared where something covers them; a frame's `visible` says which cells show them. |
| `mark` | `label`, from `record mark`. |
| `meta` | `event` and its fields: `title` (`title`), `command-started`, `command-finished` (`status`), `agent` (`agent`, `state`), `status` (`status`), or `exited` (`status`) for a pane; `focus` (`pane`, or `null`), `workspace` (`workspace`, counted from 1, and `name` when it has one), or `overlay` (`overlay`, such as `"palette"`, or `null` once none is open) for a UI. |
| `end` | `reason`, and the totals `frames`, `keyframes`, `deltas`, `images`, `marks`, `dropped`, and `bytes`. The last event of a finished recording. |

`reason` is `stopped`, `duration`, `max-bytes`, `pane-exited`, `pane-closed`, `session-ended`,
`server-shutdown`, `write-failed`, or `ui-exited`. `dropped` counts changes the writer skipped,
keeping the latest screen, because it had fallen behind. `bytes` is the file's size before the `end` line.

Each entry in a delta's `rows` has `y` and `runs`. Without `partial`, `runs` is the whole row, as a
spans frame writes it. With `"partial": true`, the runs replace only the columns they cover, from
the first run's `x` to the end of the last, and tile that range without gaps, blanks included; the
rest of the row is unchanged. A row can have several partial entries in one delta. After replacing
columns, the row is the same as the spans frame of that moment: runs as long as their style, and
default blanks at its end left out.

An image in a recorded frame has an `id` naming the `image` event that holds its pixels, and never
`png_base64`.

### Layout changes

```json
{"cmd":"layout-set","workspace":2,"layout":"master"}
{"cmd":"layout-set","workspace":2,"layout":"grid","if_revision":18}
{"cmd":"pane-set","target":7,"floating":true,"rect":{"x":10,"y":5,"width":80,"height":24}}
{"cmd":"pane-set","target":7,"rect_fraction":{"x":0.1,"y":0.1,"width":0.5,"height":0.5}}
{"cmd":"pane-set","target":7,"fullscreen":false,"if_revision":19}
{"cmd":"pane-set","target":7,"split_ratio":0.6}
{"cmd":"layout-set","workspace":2,"master_ratio":0.65}
{"cmd":"pane-move","target":7,"workspace":3}
{"cmd":"pane-swap","target":7,"with":4}
{"cmd":"pane-close","target":7,"if_revision":21}
```

- `layout-set.workspace` and the `target` of every `pane-*` request are required. None of them falls
  back to `source_pane`.
- `layout-set` needs `layout`, `master_ratio`, or both.
- `pane-set` needs at least one of `floating`, `fullscreen`, `rect`, `rect_fraction`,
  `split_ratio`, and `width_ratio`. `rect` and `rect_fraction` exclude each other.
- Ratios run from `0.2` to `0.8`.
- A stale `if_revision` fails with `conflict`.

### Focus and input

```json
{"cmd":"focus","target":3}
{"cmd":"send-text","target":3,"text":"cargo test\n"}
{"cmd":"send-keys","target":3,"keys":["C-c","Enter"]}
{"cmd":"send-keys","target":3,"keys":["C-c"],"literal":true}
{"cmd":"send-keys","target":3,"keys":["cargo test","Enter"],
 "wait":{"text":"test result:","timeout_ms":600000},"capture":"text"}
```

`send-text.target` and `send-keys.target` are optional; rozi falls back to `source_pane`, then the
focused pane. `keys` uses the names in
[Control CLI](control.md#sending-keys-and-capturing-output). With `literal: true`, every entry is
sent as literal text.

`wait` holds the reply until the pane answers the input; see [Pane waits](#pane-waits). `capture`
(`"text"`, `"ansi"`, `"png"`, or `"spans"`) returns the screen once the wait resolves, and needs
`wait`; `scale` works as for `capture-pane`. Both fail with `invalid-argument` otherwise. A send's
`spans` capture never carries image pixels.

### Pane waits

`capture-pane`, `send-text`, and `send-keys` accept a `wait` object. Without one, the reply is
immediate.

| Field | Type | Meaning |
| --- | --- | --- |
| `text` | string | Answer once this literal text appears within one visible row. Not empty, and no line breaks. |
| `settle_ms` | integer | Answer once the visible screen has not changed for this many milliseconds. With `text`, counted from when the text appears. |
| `timeout_ms` | integer | Required. Fail with `timeout` if the wait has not resolved this many milliseconds after the request arrived, from 1 to 3600000 (one hour). |

`wait` needs `text`, `settle_ms`, or both, and `settle_ms` must be less than `timeout_ms`; anything
else fails with `invalid-argument` before anything is sent or waited for.

- `capture-pane` matches the screen as it is, so text already showing answers at once.
- `send-text` and `send-keys` write their input first and match only output that arrives after it.
  Text on screen when the input was written does not count, even once it has scrolled. `settle_ms`
  counts from once the input has been confirmed and the screen right after it taken as the
  baseline, which on a UI attached to a remote session can be measurably later than the write.
- A changed screen means changed characters, colors, or styles. Cursor movement, title changes, and
  redraws of identical content are not changes.
- `timeout` and `pane-not-running` (the program exited, or the pane closed) carry the capture in
  `data` when the pane still exists. It uses the request's `render` and `scale`, or `capture` for
  a send, or text when a send asked for no capture.

A binary advertises support with the `capture-wait` capability in
[`rozi api describe`](control.md#check-the-installed-api). An older binary ignores `wait` and
answers at once.

### Pane creation

Set at most one of `command` or `argv`. Omit both for an interactive shell.

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

The response waits up to five seconds for the pane's terminal to be ready, then returns `id`,
`accepted`, and `pty_ready`.

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

- Workspace indices are `1..=9`.
- `set-status.target` falls back to `source_pane`, then the focused pane. Status and reason text is
  sanitized for display, trimmed, and limited to 64 and 256 characters. An empty or null status
  clears the report.
- `notify.level` is `"info"` or `"error"`.

### Popup

```json
{"cmd":"popup","command":"fzf","cwd":"/repo","width":0.7,"height":0.6,"title":"files","keep_open":false}
```

`command` is interpreted by `command_shell`. `width` and `height` are fractions of the viewport,
clamped to `0.2..=0.95`, and default to `0.6`. `cwd` defaults to the focused pane's cwd.
`keep_open` defaults to `true`. Only one popup can be open at a time.

### Pane logging

```json
{"cmd":"pane-logging","target":3,"enabled":true}
{"cmd":"pane-logging","target":3,"enabled":false}
{"cmd":"pane-logging","target":3}
```

Omit `enabled` to toggle. Omit `target` to use `source_pane`, then the focused pane.

## Session transport

A named session server answers control requests on its own endpoint, so a detached session can be
scripted with no UI running. The CLI uses this transport for `rozi --session <NAME> <COMMAND>`,
and handles its framing, Windows pipe derivation, and session-name validation.

The endpoint belongs to the session: `$XDG_RUNTIME_DIR/rozi/session-<NAME>.sock` on Linux, rozi's
private runtime directory on macOS, and a current-user named pipe behind an equivalent discovery
entry on Windows. The same ownership, mode, symlink, and DACL rules apply as for a UI endpoint.

### Frames

This endpoint speaks the [session protocol](sessions.md), not newline-delimited JSON. Each message
is a 4-byte big-endian length, a 1-byte frame kind, and a JSON body. One exchange is:

1. Connect to the session endpoint.
2. Write one `session-control` frame.
3. Read one `session-control-result` frame.
4. The server closes the connection.

```json
{"type":"session-control","session":"dev","protocol_version":19,"min_protocol_version":19,
 "request":{"cmd":"capture-pane","target":3}}
```

```json
{"type":"session-control-result","effective_protocol":19,
 "response":{"ok":true,"data":{"id":3,"title":"zsh","render":"text","text":"…"}}}
```

- `request` is the same document the UI endpoint accepts, and `response` is the same envelope it
  returns.
- `protocol_version` and `min_protocol_version` are the newest and oldest session protocol
  versions the client speaks. A server accepts only its own version, which `rozi api describe`
  reports as `session_protocol`. `effective_protocol` is the version the server used.
- Both frames may carry a `capabilities` object; a client may omit it.
- A wrong session name or an incompatible version is answered with a session-protocol `error` frame
  carrying `session-mismatch` or `protocol-mismatch`, not with a control response.
- A request with a pane wait holds the connection open until the wait resolves or times out. A
  client should read with a timeout longer than `timeout_ms`. `record-stop` holds it until the
  recording's file is complete, and `record-start` with `follow` until the recording ends; closing
  a followed recording's connection stops it.
- A reply travels in one frame of at most 8 MiB. A reply that would not fit, such as a PNG of a very
  large pane, a long `"full"` scrollback, or a `spans` frame carrying a large image's pixels, is
  answered with `message-too-large` instead. The UI endpoint has no such limit.

The connection never becomes a client. It gets no client id, is not counted in the session roster
or client count (including `metrics`), never holds layout control, and receives no replay. It also
gains no authority an attached client would not have.

### Supported requests

A session server answers `list-panes`, `layout-get`, `layout-set`, `pane-set`, `pane-move`,
`pane-swap`, `pane-close`, `agents-list`, `agent-get`, `agent-read`, `agent-wait`,
`agent-prompt`, `agent-report`, `agent-release`, `metrics`, `capture-pane`, `send-text`,
`send-keys`, `new-pane`, `set-status`, `pane-logging`, `record-start`, `record-stop`, `record-list`,
and `record-mark`. It refuses every other `cmd` with
`ok: false` and a reason naming what the request needs a UI for.

These requests behave differently than against a UI:

| Request | Against a session server |
| --- | --- |
| Any `target` | `source_pane` is ignored, and there is no focused-pane fallback, because a pane id does not say which session it belongs to. A session with one pane resolves to it; otherwise the error lists the pane ids. |
| `list-panes` | Lists every pane in the session, including exited ones, whose `status` is `exited (<CODE>)`. `workspace` comes from the shared layout, or is `0` when the session has no layout document. |
| `layout-get` | Reads the server's layout document. `client` and every `view_rect` are absent, since there is no screen. `unplaced_panes` lists panes the document does not place. Before anything places a pane, `revision` and `canvas` are null and `workspaces` is empty. |
| `pane-close` | Removes the pane from the layout document, ends its process, then commits the new revision; a refused request ends nothing. A pane the document does not place is still closed, without a revision. Refused with `not-controller` while any client holds layout control. |
| `layout-set`, `pane-set`, `pane-move`, `pane-swap` | Applied to the server's layout document and committed as a new revision by client `0`, which every attached client applies. `committed` is always `true`. Refused with `not-controller` while any client holds layout control, and with `unavailable` for a session that has panes but no layout document. |
| `metrics` | Returns only `sampled_at_unix_ms` and `server`, sampled at request time, so `age_ms` is `0` and `stale` is `false`. |
| `new-pane` | `focus` must be `false`. Refused with `not-controller` while any client holds layout control. The server reads `[[rules]]` and the shell from its own config, picks the pane id, appends the pane to the resolved workspace (default 1) in the shared layout, and broadcasts the new revision as client `0`. `pty_ready` reports whether the pane's process started. |
| `send-text`, `send-keys` | Refused while the session's input lock is on, which only an attached client can release. |
| Any request with `extension` provenance | Refused. The generation is minted per UI process, so a server cannot check it. |

## Subscription stream

Open with:

```json
{"cmd":"subscribe","events":["pane-exited","pane-status-changed"]}
```

An empty or absent `events` array subscribes to all events. An unknown event ID rejects the
request. After `{"ok":true}`, the server writes one event object per line until either side
disconnects:

```json
{"event":"pane-exited","data":{"pane":"3","code":"1","focused":"false"}}
```

Every event field is a string under `data`. See [Hooks](hooks.md#events-and-fields) for the event
list and fields.

A subscriber's queue holds 128 events. rozi disconnects a subscriber that falls behind rather than
blocking the UI.

CLI bridge:

```sh
rozi subscribe pane-exited pane-status-changed
```

## Picker stream

A picker stream opens a modal picker, receives rows from the client, and reports the user's
choice back.

### Open a picker

```json
{"cmd":"pick","title":"Branches","placeholder":"Filter","empty":"No branches","width":72,"actions":[{"id":"new","key":"ctrl-n","label":"new","prompt":"Branch name"}]}
```

If another picker or modal overlay is open, rozi returns an error and closes the connection.
Otherwise it replies `{"ok":true}`. A second `pick` while one is open is refused, including while
the open picker shows a prompt.

| Field | Default | Meaning |
| --- | --- | --- |
| `title` | `"Pick"` | Picker title. |
| `placeholder` | `"Search…"` | Query hint. |
| `empty` | none | Text shown when the row list and the filter are both empty. Without it, the empty list looks as it normally does. A filter that matches nothing always shows `No matches`. |
| `width` | 60 | Width in columns, clamped to `30..=120`. |
| `actions` | none | Extra key bindings; see [Actions](#actions). |
| `tabs` | none | Pages under one title; see [Tabs](#tabs). |
| `tab` | first tab | Tab to open on. |

### Send rows

After `{"ok":true}`, the client may write row snapshots. Each line replaces the full row set, and
rozi keeps at most 512 rows from each snapshot.

```json
{"rows":[{"id":"main","label":"main","description":"current","group":"Local","active":true},{"id":"old","label":"old","disabled":"protected"}]}
```

| Field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `id` | string | `label` | Value returned as `selected`. |
| `label` | string | required | Visible row text. |
| `description` | string | none | Right-aligned detail. Clipped, or dropped entirely, when the label needs the room. |
| `group` | string | none | Section label. |
| `disabled` | string | none | Makes the row inert and shows the reason. |
| `active` | bool | `false` | Marks the current item. |
| `priority` | integer | `0` | Adds sorting weight. |

### Read the result

Selection writes one final object:

```json
{"selected":"main"}
```

Cancellation writes:

```json
{"cancelled":true}
```

An action writes an object and keeps the picker open, unless the action declared `close: true`:

```json
{"action":"delete","selected":"old"}
{"action":"new","input":"feat/api","selected":"main"}
```

Lines that keep the picker open — actions and tab switches — are queued for the client. If the
client stops reading and 64 are waiting, later ones are dropped. The final line — a selection, a
cancellation, or a closing action — is never dropped and always arrives last.

### Actions

| Field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `id` | string | required | Returned as `action`. |
| `key` | string | required | One valid key chord. |
| `label` | string | required | Footer label. |
| `prompt` | string or object | none | Opens a text prompt over the picker and returns `input`. A string is the title. |
| `close` | bool | `false` | Closes the picker after the action. |
| `confirm` | bool | `false` | Requires a second press on the same row. |

rozi omits an action with an empty `id` or an invalid key chord.

A `prompt` object has these fields; the string form is shorthand for `{ "title": "…" }`:

| Field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `title` | string | required | Prompt title. |
| `placeholder` | string | none | Empty-field hint. |
| `value` | string | none | Initial contents. |
| `masked` | bool | `false` | Hides typed characters on screen. The submitted `input` is still plain text on the stream. |

```json
{"prompt":"Command"}
{"prompt":{"title":"Edit command","placeholder":"git status","value":"git status --short"}}
{"prompt":{"title":"Token","masked":true}}
```

### Tabs

`tabs` shows several pages under one title, with a tab strip above the query. Each tab keeps its own
rows, filter text, and highlight, so switching away and back keeps what was typed. `actions`,
`placeholder`, `empty`, and `width` apply to every tab.

```json
{"cmd":"pick","title":"Git","tabs":[{"id":"branches","label":"Branches"},{"id":"worktrees","label":"Worktrees"}],"tab":"branches"}
```

| Field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `id` | string | required | Names the tab in row snapshots and replies. Row IDs only need to be unique within a tab. |
| `label` | string | `id` | Tab strip text. |

- An omitted or unknown `tab` opens the first tab.
- Tabs with an empty or repeated `id` are omitted, and rozi keeps at most 32.
- `Tab`/`Shift+Tab` and `Right`/`Left` cycle tabs, and clicking a tab selects it, unless an action
  uses that key.

In a tabbed picker, each row snapshot names its tab and replaces only that tab's rows. Rows may
arrive for a hidden tab at any time. rozi ignores a snapshot without `tab` or with an undeclared
`tab`, and a snapshot with `tab` sent to a picker without tabs.

```json
{"tab":"worktrees","rows":[{"id":"/src/rozi-review","label":"rozi-review","description":"~/src/rozi-review"}]}
```

A tabbed picker adds the active tab to selections and actions, and reports each tab switch without
closing, so the client can fill a tab when it is first shown:

```json
{"tab":"worktrees"}
{"action":"delete","selected":"old","tab":"branches"}
{"selected":"main","tab":"branches"}
```

### CLI bridge

`rozi pick` has a plain one-label-per-line mode and a JSON mode. In JSON mode, the first stdin line
holds the picker fields and optional initial `rows`, which fill the tab the picker opens on; later
lines are row snapshots:

```sh
printf '%s\n' '{"title":"Branches","rows":[{"id":"main","label":"main"}]}' |
  rozi pick --json
```

## Published activity stream

A published activity stream lets a program report its own activities as rows in the sidebar's
Activity list, and hear when the user activates one.

Open with:

```json
{"cmd":"publish","source_pane":3}
```

After `{"ok":true}`, the publisher writes complete snapshots:

```json
{"rows":[{"id":"job-1","title":"Run tests","status":"working","reason":"crate core","active":true}]}
```

| Field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `id` | string | required | Stable identity chosen by the publisher. |
| `title` | string | required | Activity title. May be empty. |
| `status` | string | required | Status value. |
| `reason` | string or null | null | Supporting detail. |
| `active` | bool | `false` | The row currently visible inside the publisher. At most one should be active. |
| `work_started_at` | integer or null | set by rozi | rozi replaces any value a publisher sends. |

- An empty `rows` list withdraws the rows. End of input or any stream failure also withdraws them.
- IDs, titles, and statuses are sanitized for display and limited to 64 characters; reasons are
  limited to 256.
- Rows with an empty ID or status are dropped.
- Only the first row with `active: true` in a snapshot stays active.

When the user activates a row, rozi focuses its pane and writes:

```json
{"activate":"job-1"}
```

The publisher must keep reading activations. If its backlog of unread activations fills up, rozi
closes the stream and withdraws its rows.

### Row ownership

`source_pane` decides which pane owns the rows. If it is absent, rozi uses the focused live pane at
the moment the stream opens; this lets a supervised service publish, but its rows still belong to
that pane.

While a pane has published rows, they are the authoritative activity list for that pane. If rozi has
detected an agent in the pane, it derives the agent's displayed state from the rows: blocked first,
then any status other than `idle` or `done`, then the remaining rows. A publisher in a pane with no
detected agent still gets Activity rows, but rozi does not treat it as an agent.

CLI bridge:

```sh
rozi publish
```

## Stream ownership

The CLI attaches the extension `id` and `generation` when both are present in its environment.
Picker, publisher, and subscription streams owned by an extension close when its generation
retires: when the extension is disabled, removed, or has a change that affects its processes.
Changes to metadata only keep the generation.
