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

The complete request vocabulary, including `context` and the agent requests (`agents-list`, `agent-get`,
`agent-read`, `agent-wait`, `agent-prompt`, `agent-report`, and `agent-release`) that this page
does not spell out, is described by the [JSON Schema](control.md#json-schema).

## Caller identity and prompt results

`{"cmd":"context","source_session":"<instance>","source_pane":3}` on a session returns a
`CallerContext`. The instance must match the answering server. A live pane yields its fenced
reference and selected agent; a missing or exited pane yields `status: "stale-pane"`.
On a UI, `context` returns attachment metadata, `displayed_session` and `held_sessions`, as session
instance ids. The CLI combines these with the verified caller identity for
[`rozi context`](control.md#verify-caller-context).

`caller_context` optionally fences a request to a verified `PaneRef`. For UI requests, `source_pane` and
`source_session` must match. The UI refuses it if that session is no longer displayed or the
calling pane generation has changed. Direct and forwarded session requests also check the pane
generation. The CLI uses this for `--here` mutations through the caller's UI; it does not retry a
submitted mutation through another route.

`agent-prompt` accepts `wait: "turn"` and optional `capture: "text"` or `"ansi"`. `turn` is a
post-prompt wait that resolves on `blocked`, `idle`, `done`, or `error`, after departure from the
initial state. `agent-wait` does not accept `turn`. Capture requires a wait other than
`gone`. The `AgentWaitResult` names the exact prompted agent in `ref`, with its runtime in `agent`.
When requested, `result` is `{"kind":"terminal","capture":<PaneCapture>}`, taken when the wait
resolves. It describes the visible terminal, not a conversational message.

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

rozi reads only the first request line. `subscribe`, `pick`, `prompt`, and `publish` keep the connection open
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
| `credential` | string | A placed extension process's credential, from `ROZI_EXTENSION_CREDENTIAL`. The CLI adds it. |

Do not synthesize extension provenance. A request with a retired generation is rejected.

A request that arrives through an extension runtime's bridge — from a
[placed](writing-extensions.md#run-on-the-session-host) extension process — must carry a `credential`. The
UI identifies the caller from it alone: it replaces `extension` with the extension and generation
the credential was issued for, and refuses a credential that was revoked or issued for another
host's runtime. The credential distinguishes hosts and generations, not processes on one host: any
process of the same user there can read it. It then admits each request by the rules in
[What a placed process may do](writing-extensions.md#what-a-placed-process-may-do).

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
`agent-replaced`, `stale-reference`, `timeout`, `request-failed`, `not-permitted`,
`out-of-scope`, `action-not-found`, `action-busy`, `action-failed`, `invalid-arguments`,
`invalid-result`, `invocation-cancelled`, `invocation-depth`, `invocation-limit`,
`invocation-not-found`, and `invocation-timeout`.

An error normally has no `data`. A failed [pane wait](#pane-waits) is the exception: `timeout` and
`pane-not-running` carry the pane's capture in `data` when the pane still exists.

`not-permitted` and `out-of-scope` answer only placed extension processes: the first for a command
no placed process may send, the second for one that acts on a session outside the host or session
the process was placed for.

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
| `pane-reveal` | `{ "changed": bool, "workspace": number }`; see [Changing the layout](control.md#changing-the-layout). |
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
- `foreground_pid` is the process group ID of the command running in the pane, which is its first
  process's ID. It is omitted when nothing runs, when the platform cannot read it, and for a remote
  attachment, whose processes are on another machine.
- `agent_session` is the conversation the pane's agent last named with
  `agents report --native-session`, while that report owns the pane. It is omitted otherwise. A
  client that keeps several conversations open reports the one it shows.

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
- `requeued_panes` counts panes that need another replay because a client fell behind again before
  their replay finished, during an attach or a resync.
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
`"rozi-spans"`, and an integer `version`, currently `2`, apart from the control API's versions.
Consumers must reject versions newer than they support. The version changes when older readers
cannot preserve frame semantics, including additions required for correct rendering. Version 2
preserves image stacking and underlying glyphs. Consumers may ignore other unknown fields.
The frame is `SpanFrame` in the
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
it first. `fill_cell_box: true` stretches pixels to the cell box instead of keeping the source
aspect ratio. `z_index` preserves Kitty stacking order: negative planes draw beneath glyphs and
non-negative planes draw over them. Missing `z_index` defaults to zero.

Cells covered by an image may hold `▀` half-block approximations in the rows. `underlying_cells`
preserves the original cells beneath these stand-ins so PNG replay can draw glyphs and their
backgrounds before blending image pixels. It is a row-major array over the image's cell area,
with null entries for hidden or offscreen cells and text fully covered by opaque non-negative
image planes. Each non-null entry uses the same text and style
fields as a run, with `width: 1` and `x` relative to the image's left edge. Wide glyph continuation
cells keep empty `text`. The array is absent when no original cells were captured.
Coverage is checked across the complete image stack: a translucent patch cannot expose text
already hidden by an opaque image below it. A wide glyph's metadata remains when either half
can contribute to the final frame.
Glyph spans include image-free continuation cells and private-use icon room in a neighboring
blank. A block cursor can redraw the glyph above opaque images. Other visible cursor shapes
preserve needed styles and span as anonymous whitespace when the text itself is hidden.

`visible` is absent when the image shows in every cell of its area; otherwise it has one entry
per row of the area, each an array of `[x, width]` column ranges still showing the image.
`png_base64` holds the pixels as a PNG, with alpha, when the request set `image_pixels`. Pixels on
a cell that does not show the image, because something covers it or it is off the grid, are fully
transparent, so the PNG never reveals what the capture hides. Pixels are fitted from the top-left
corner into cells twice as tall as they are wide, or stretched when `fill_cell_box` is true. A
pixel that straddles a hidden cell is cleared.

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
`attached-control` in `rozi api describe` advertises. See
[Record a pane or the UI](recording.md) for what a recording holds.

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
the file. `hide_indicator: true` keeps the `REC` chip off the screen, and so out of the recording;
left out, `[recording] ui_indicator` decides. It answers once the first frame is in the file, with
`path`, `started_at_unix_ms`, `max_fps`, `duration_ms`, and `max_bytes`. A UI already recording
itself fails with `conflict`.

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
{"cmd":"pane-reveal","target":7}
```

- `layout-set.workspace` and the `target` of every `pane-*` request are required. None of them falls
  back to `source_pane`.
- `layout-set` needs `layout`, `master_ratio`, or both.
- `pane-set` needs at least one of `floating`, `fullscreen`, `rect`, `rect_fraction`,
  `split_ratio`, and `width_ratio`. `rect` and `rect_fraction` exclude each other.
- Ratios run from `0.2` to `0.8`.
- A stale `if_revision` fails with `conflict`.
- `pane-reveal` scrolls only the UI's own view: it takes no `if_revision`, needs no layout control,
  and a session server refuses it with `unsupported`. `pane-reveal` in `rozi api describe`
  advertises it.

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
{"cmd":"new-pane","argv":["my-tui"],"workspace":9,"size":{"cols":100,"rows":220}}
```

| Field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `command` | string or null | null | Command line interpreted by `command_shell`. |
| `argv` | string array or null | null | Direct executable and arguments. |
| `cwd` | string or null | focused pane cwd | Working directory on the session host. |
| `local_cwd` | string or null | null | Working directory for a local pane, taking precedence over `cwd`. Ignored for a remote session. The CLI supplies its resolved local directory here. |
| `title` | string or null | generated | Initial title. |
| `keep_open` | bool | `false` | Replaces a finished command with a shell. |
| `focus` | bool | `false` | Focuses the new pane and its workspace. |
| `workspace` | integer or null | rule or current workspace | One-based workspace, `1..=9`. |
| `size` | object or null | tile size | `{ "cols": number, "rows": number }`, each `1..=1000`. The size the pane starts at; see [Choose the size of a pane](control.md#choose-the-size-of-a-pane). |

The response waits up to five seconds for the pane's terminal to be ready, then returns `id`,
`accepted`, and `pty_ready`.

A binary advertises `size` with the `split-size` capability in
[`rozi api describe`](control.md#check-the-installed-api); a UI that does not advertise it ignores
the field.

### Actions, workspaces, status, and notifications

```json
{"cmd":"run-action","action":"toggle-float"}
{"cmd":"switch-workspace","index":2}
{"cmd":"move-to-workspace","index":3}
{"cmd":"set-status","target":3,"status":"blocked","reason":"needs approval"}
{"cmd":"set-status","target":3,"status":null}
{"cmd":"notify","message":"deploy finished"}
{"cmd":"notify","message":"tests failed","title":"Build","level":"error"}
{"cmd":"command-visibility","command":"install-hooks","visible":false}
```

- Workspace indices are `1..=9`.
- `set-status` reports a generic status for the pane: it appears in Activity, but does not make the
  pane an agent. In a pane with a detected agent it sets that agent's state. See
  [Activities and agents](#activities-and-agents).
- `set-status.target` falls back to `source_pane`, then the focused pane. Status and reason text is
  sanitized for display, trimmed, and limited to 64 and 256 characters. An empty or null status
  clears the report.
- `notify.level` is `"info"` or `"error"`.
- `command-visibility` needs the `extension` provenance the CLI attaches inside an extension
  process. `command` is the manifest `id` or the public `<extension>.<id>` of one of that
  extension's own commands. The choice lasts as long as the extension's runtime generation,
  even after the calling command exits. A process-facing change rotates the generation and
  restarts its services, resetting the choice to the manifest's `hidden`. A presentation-only
  or no-op reload keeps both the generation and the choice. A binary advertises support with
  the `command-visibility` capability.

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
{"type":"session-control","session":"dev","protocol_version":37,"min_protocol_version":37,
 "request":{"cmd":"capture-pane","target":3}}
```

```json
{"type":"session-control-result","effective_protocol":37,
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
`pane-swap`, `pane-close`, `context`, `agents-list`, `agent-get`, `agent-read`, `agent-wait`,
`agent-prompt`, `agent-report`, `agent-release`, `metrics`, `capture-pane`, `send-text`,
`send-keys`, `new-pane`, `set-status`, `pane-logging`, `record-start`, `record-stop`, `record-list`,
`record-mark`, `extensions-deploy`, `extensions-undeploy`, and `extensions-status`. It refuses
every other `cmd` with `ok: false` and a reason naming what the request needs a UI for.

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

### Extension deployments

A session server can keep an extension for itself, so the files outlive the client that sent them.
An extension installed on the client is not in the session's store until it is deployed there; the
`session-deployments` capability says a server accepts these requests.

`extensions-deploy` carries one chunk of a bundle:

```json
{"cmd":"extensions-deploy","id":"git-tools","digest":"3f2a…","size":262144,
 "offset":0,"data":"<base64>","last":false}
```

`digest` names the whole bundle, `size` is its length in bytes, and `offset` is where this chunk
starts. A chunk that does not continue the previous one is refused, so a client starts at `0` and
sends the chunks in order; `last` marks the one that completes the announced size. The server
verifies the assembled bundle against `digest` and its manifest against `extension` before it keeps
anything. The reply is `{extension, digest, received, size, complete}`, with `generation` once
`complete` is true:

```json
{"ok":true,"data":{"extension":"git-tools","digest":"3f2a…","received":262144,
 "size":262144,"complete":true,"generation":3}}
```

`extensions-status` lists what the session holds, verified at request time:

```json
{"ok":true,"data":{"session":"dev","deployments":[{"extension":"git-tools","digest":"3f2a…",
 "generation":3,"deployed_at_unix_ms":1767225600000,"title":"Git tools","version":"0.2.1",
 "files":14,"size":262144,"status":"deployed"}]}}
```

A deployment whose files no longer verify reports `"status":"unavailable"` and a `reason` instead
of disappearing. `extensions-undeploy` takes `{id}` and answers `{extension, removed}`,
`removed` being `false` when the session held nothing by that name. Deployments are kept across
restarts and resurrection, and removed when the session is deleted.

## Subscription stream

Open a UI event subscription with:

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

### Session subscriptions

A session server serves `subscribe` itself, through `rozi --session <NAME> subscribe` or
`rozi --remote <HOST> --session <NAME> subscribe`. It accepts `snapshot` or `after`, but not both. A
UI refuses either one.

```json
{"cmd":"subscribe","events":["pane-spawned","pane-exited"],"snapshot":true}
{"cmd":"subscribe","events":["layout-changed"],"after":"4a2b:42"}
```

The CLI prints one typed record per line:

```json
{"type":"snapshot","session_instance":"4a2b","session":"dev","seq":42,"controller":1,"client_count":2,"layout_rev":3,"panes":[]}
{"type":"event","session_instance":"4a2b","seq":43,"kind":"pane-spawned","data":{"pane":"2"}}
{"type":"lagged"}
```

A cursor the session cannot honour fails with error code `cursor-expired`, whose `data` holds
`current_instance`, `oldest_seq`, and `latest_seq`. A subscriber that falls behind receives a
`lagged` record and the connection closes. See
[Session subscriptions](control.md#session-subscriptions) for the events and retention.

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
| `confirm` | bool | `false` | Requires a second press on the same row. Built-in pickers bind destructive actions to `ctrl-k` with `confirm`; use the same key for delete, remove, or kill actions. |

rozi omits an action with an empty `id` or an invalid key chord.

A `prompt` object has these fields; the string form is shorthand for `{ "title": "…" }`:

| Field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `title` | string | required | Prompt title. |
| `placeholder` | string | none | Empty-field hint. |
| `value` | string | none | Initial contents. |
| `from_query` | bool | `false` | Starts with the text typed in the picker's search field. `value` applies while the field is empty. |
| `masked` | bool | `false` | Hides typed characters on screen. The submitted `input` is still plain text on the stream. |

```json
{"prompt":"Command"}
{"prompt":{"title":"Edit command","placeholder":"git status","value":"git status --short"}}
{"prompt":{"title":"Token","masked":true}}
{"prompt":{"title":"New branch","from_query":true}}
```

With `from_query`, someone can type a name into the search field and press the action's key to
create it, the way `Ctrl+N` in the session picker starts a new session from the search text.

### Tabs

`tabs` shows several pages under one title, with a tab strip below the query. Each tab keeps its own
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

## Prompt stream

A prompt stream asks for one line of text in a modal prompt, with no picker behind it.

```json
{"cmd":"prompt","title":"Open URL","placeholder":"https://example.com"}
```

| Field | Default | Meaning |
| --- | --- | --- |
| `title` | required | Prompt title. |
| `placeholder` | none | Empty-field hint. |
| `value` | none | Initial contents. |
| `masked` | `false` | Hides typed characters on screen. The submitted `input` is still plain text on the stream. |

rozi refuses a prompt while a picker or another modal overlay is open, as it does a `pick`.
Otherwise it replies `{"ok":true}`, then writes one final line: `{"input":"…"}` when the user
submits, or `{"cancelled":true}` when they cancel. A prompt from an extension that is disabled or
reloaded while it is open is cancelled with a `reason`, as a picker is. The client writes nothing
after the request.

`rozi prompt` speaks this stream; see [Prompts](control.md#prompts).

## Published activity stream

A published activity stream lets a program report its own activities as rows in the sidebar's
Activity list, and hear when the user activates one. A publisher declares once, when the stream
opens, whether it is an agent. Every published row appears in Activity; only an agent publisher's
rows are also agents.

Open with:

```json
{"cmd":"publish","source_pane":3}
{"cmd":"publish","source_pane":3,"kind":"agent","name":"sessions"}
```

| Field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `kind` | string | `"activity"` | `"activity"` or `"agent"`. See [Activities and agents](#activities-and-agents). |
| `name` | string or null | null | Tells apart several publishers from one caller in the same pane. 1–64 lowercase letters, digits, `-`, or `_`; anything else is refused with `invalid-argument`. |

After `{"ok":true}`, the publisher writes complete snapshots:

```json
{"rows":[{"id":"job-1","title":"Run tests","status":"working","reason":"crate core","active":true}]}
```

| Field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `id` | string | required | Stable identity chosen by the publisher. Unique within this publisher; another publisher in the same pane may use the same ID. |
| `title` | string | required | Activity title. May be empty. |
| `status` | string | required | Status value. |
| `reason` | string or null | null | Supporting detail. |
| `active` | bool | `false` | The row currently visible inside the publisher. At most one of a publisher's rows should be active. |
| `cwd` | string or null | null | Absolute directory the activity works in, on the pane's host. Absent means the pane's directory. |
| `native_session` | string or null | null | The agent's own conversation ID for this row, such as the ID `claude --resume` takes. Kept exactly as sent; one longer than 4096 bytes or containing a control character is dropped. Used only on an agent publisher's rows, and dropped from an activity's. |
| `live_session` | string or null | null | The session ID the process running this row reports its hooks under, when the conversation moved to a process with a new ID but is still resumed by `native_session`. Validated like `native_session`. Used only on an agent publisher's rows, and dropped from an activity's. |
| `reported` | bool | set by rozi | Whether an agent report, rather than the publisher, set the row's `status` and `reason`. rozi replaces any value a publisher sends. |
| `work_started_at` | integer or null | set by rozi | rozi replaces any value a publisher sends. |
| `project` | object or null | set by rozi | The Git project containing `cwd`: `root`, `branch`, and `repository` when `root` is a linked worktree. rozi replaces any value a publisher sends. |
| `publisher` | string | set by rozi | `<client>/<identity>`: the rozi window that holds the stream, and the publisher's identity in it; see [Several publishers in one pane](#several-publishers-in-one-pane). rozi replaces any value a publisher sends. |
| `kind` | string | set by rozi | The `kind` the stream declared. rozi replaces any value a publisher sends. |

A row with its own `cwd` is grouped in Activity under that directory's project and branch instead
of its pane's. Use it when one program runs several activities in different checkouts, such as
coding-agent sessions that each work in their own Git worktree. A relative `cwd` is dropped.

- An empty `rows` list withdraws this publisher's rows. End of input or any stream failure also
  withdraws them.
- IDs, titles, and statuses are sanitized for display and limited to 64 characters; reasons are
  limited to 256.
- Rows with an empty ID or status are dropped. A publisher may have at most 64 rows in a pane.
- Only the first of a publisher's rows with `active: true` in a snapshot stays active.

When the user activates a row, rozi focuses its pane and writes to the stream that published it:

```json
{"activate":"job-1"}
```

The publisher must keep reading activations. If its backlog of unread activations fills up, rozi
closes that stream and withdraws its rows.

### Activities and agents

An activity is something happening; an agent is something doing work. Declare `"kind":"agent"`
only when each row is an agent's run or conversation, such as the sessions of a coding-agent
client. Anything else, such as a build, a watcher, or a list of pull requests, keeps the default
`"activity"`.

| Origin | Activity | Agents view, `rozi agents`, keep-awake |
| --- | --- | --- |
| An agent rozi [detects](agents.md) on screen | yes | yes |
| An [agent report](agents.md#report-state-from-agent-hooks) | yes | yes |
| A publisher with `"kind":"agent"` | yes | yes, one agent per row |
| A publisher with `"kind":"activity"` | yes | no |
| A plain [`set-status`](#actions-workspaces-status-and-notifications) | yes | no, unless the pane has a detected agent, whose state it then sets |

Only agents are listed by `agents list`, satisfy `agents wait`, can be targeted by `agents prompt`,
appear in the Agents view and in host monitor summaries, and keep the host awake under
[`keep_awake_while_agents_work`](configuration.md#session). An activity row never takes on the
identity or the report of an agent in the same pane.

### Row ownership

`source_pane` decides which pane owns the rows. If it is absent, rozi uses the focused live pane at
the moment the stream opens; this lets a supervised service publish, but its rows still belong to
that pane. To pin a service's rows to a specific pane, give it `ROZI_PANE`.

A stream also belongs to one session, because pane numbers repeat across sessions. With
`source_session`, which the CLI sends from `ROZI_SESSION_INSTANCE`, it belongs to that session, and
is refused with `session-not-attached` when that session is not attached to this rozi. Without it,
the stream belongs to the session on screen when it opens, or to this client for a scratchpad or
popup pane. Either way its rows keep reaching that session after you switch to another one, and
never land on the other session's pane with the same number. Activating a row reaches the stream
of the session on screen.

A [placed extension process](writing-extensions.md#what-a-placed-process-may-do) is the exception: like
its other pane commands, its stream opens only into the session on screen, and only when that
session lies within its placement, whatever `source_session` it names; otherwise it is refused
with `out-of-scope`. Its rows are delivered only while that same session is on screen, and are
dropped, not redirected, while another session is.

While an agent publisher has rows in a pane, they are that pane's agents, and rozi stops reading
the pane's screen for agent state. If rozi has detected an agent in the pane, the rows carry its
identity, and the agent's displayed state is derived from them: blocked first, then any status
other than `idle` or `done`, then the remaining rows. Once no agent publisher has rows, screen
detection speaks for the pane again.

An activity publisher's rows never replace, hide, or rename the pane's detected or reported agent.
Screen detection carries on beside them, and the pane's agent stays listed next to the rows.

Rows and a live [agent report](agents.md#report-state-from-agent-hooks) coexist. The report keeps its
claim and its sequence fence, and its state and reason replace those of the agent row whose
reporting session — `live_session`, or `native_session` when the row has none — exactly matches the
report's, everywhere the row appears: Activity, the Agents view, `agents list`, and the row's run
clock and finish alert. A report that matches no agent row is not listed while an agent publisher
has rows.

While one integration owns the pane, a report from another integration token is accepted for the
row whose reporting session it names, and only for that row. Publish `live_session` when a row's
conversation runs in a process other than the pane's own, so that process's hooks, not the pane
owner's last report, speak for it. A pane id given to `agents get`, `read`, `wait`, or `prompt`
names the active agent row; a row that is not active is reached only by its exact reference.
`read` and `prompt` refuse such a reference with `conflict`, because the input would land in the
conversation on screen instead. When the publisher marks none of its rows active, the refusal says
it cannot tell which conversation is on screen. A released row report's token, or one whose
conversation no row names any more, is retired.

When a session is restored, the pane's active agent row's `native_session` is the conversation rozi
resumes, ahead of the report's. Activity rows are never resumed. When the window holding that agent
publisher leaves the session, its rows are withdrawn but the conversation they named is still the
one rozi resumes, until agent rows are published again, the pane's shell returns to its prompt, or
a different agent replaces it.

### Several publishers in one pane

A pane may have several publishers at once, such as an agent client's session list beside a
pull-request watcher. Each publisher owns only its own rows: when one closes, crashes, restarts, or
is dropped for not reading its activations, the others' rows stay exactly as they were. Run clocks,
finish alerts, and activations follow a row by its publisher and ID together, so two publishers
may use the same row ID.

A stream lives in the rozi window it connected to. When several windows are attached to one
session, each window's publishers are its own: two windows' publishers with the same identity are
two publishers, and neither can replace or withdraw the other's rows. A row's `publisher` names
the window as well as the identity. When a window leaves the session, by detaching, exiting, or
losing its connection, the rows of its publishers are withdrawn; a window that reconnects reports
its live publishers' rows again.

Each window runs its own copy of an extension, so `--kind agent` publishers are the exception: one
window's publisher speaks for the pane, and the same identity from another window waits. When the
speaking one withdraws its rows or its window leaves, a waiting one takes over, and rows with the
same ID keep their run clocks. A publisher that withdraws its rows and lists them again while the
pane runs the same program, as a service does when its window shows another session and returns,
continues their run clocks too. Selecting an agent row sends the activation to the selecting
window's own publisher with that identity, whichever window's row is listed.

Every attached window lists every window's activity rows in Activity, but only the window holding
a stream can send it activations. Selecting another window's activity row focuses its pane; the
program behind it does not switch to that row, because its stream is in the other window.

A publisher's identity in its window is:

| Caller | Identity |
| --- | --- |
| An extension process | `<extension>`, or `<extension>.<name>` with `name` |
| Any other process | `publish:default`, or `publish:<name>` with `name` |

A stream that opens with the same identity in the same pane of the same session, in the same
window, replaces the earlier one, so a publisher that restarts takes over its own stale stream and
nobody else's. Each window may have up to 16 publishers in a pane; a stream beyond that is
refused with `conflict` when it opens, before its `{"ok":true}`. Give each publisher of one caller
in a pane its own `name`.

`active` is a rule per publisher, so a pane can have several active rows. The pane's active row,
which session restore resumes, is the first active row of an agent publisher, taking publishers in
the order they first published in the pane.

CLI bridge:

```sh
rozi publish [--kind agent|activity] [--name NAME]
```

## Sidebar stream

A sidebar stream publishes rows into a [stream tab](sidebar.md#stream-tabs) and hears what the user
does with them. Unlike a published activity stream, it belongs to no pane: the rows belong to the
tab.

Open with:

```json
{"cmd":"sidebar-publish","tab":"deploys"}
{"cmd":"sidebar-publish","tab":"machines","name":"prod"}
```

| Field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `tab` | string | required | An extension process names its own extension's tab by its local name (`machines` for `<extension>.machines`). Any other caller names a `config.toml` stream tab by its ID. |
| `name` | string or null | null | Tells apart several producers of one caller in the same place. Validated like a [publisher's](#published-activity-stream) `name`. |

A tab that does not exist or is not a stream tab is refused with `invalid-argument`; another
extension's tab, or an extension's tab from any other caller, with `not-permitted`.

After `{"ok":true}`, the producer writes complete snapshots of its rows, one JSON object per line:

```json
{"rows":[
  {"id":"api","label":"api","tone":"working","description":"deploying"},
  {"id":"api/eu","parent":"api","label":"eu-west-1","tone":"success","expandable":true}
],
 "actions":[{"id":"rollback","key":"r","label":"roll back","confirm":true},
            {"id":"remove","key":"d","label":"remove","close":true}]}
```

| Row field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `id` | string | required | Stable identity, unique among this producer's rows. Another producer may use the same ID. |
| `label` | string | required | The row's text. |
| `parent` | string or null | null | The `id` of the row this one sits under. A parent missing from the snapshot makes it a top-level row; rows in a parent cycle are not shown. |
| `description` | string or null | null | Right-aligned on the label's line. |
| `meta` | string or null | null | A short value beside the label, such as how long something has run, in the colour of the row's `tone`. |
| `detail` | string or null | null | A second line under the label. |
| `tone` | string | `"normal"` | `normal`, `muted`, `info`, `working`, `success`, `warning`, `blocked`, or `error`. rozi picks the marker and colour. |
| `active` | bool | `false` | The current one of its kind. |
| `disabled` | string or null | null | Shows the row dimmed with this reason as its second line; it cannot be activated and takes no actions. |
| `expandable` | bool | `false` | The row has children, even before any are listed. Expanding it writes an `expand` event. |
| `expanded` | bool or null | null | Initial expansion. A row with listed children starts expanded, one with none starts collapsed. The user's own choice wins once made. |
| `heading` | bool | `false` | Shows a top-level row as a section heading. Its children are always shown, it never writes `activate`, and its `description` sits at the right edge. Ignored on a row with a parent. |
| `actions` | string array or null | null | IDs of this producer's actions that apply to the row. Absent means all of them. |

| Action field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `id` | string | required | Returned as `action`. |
| `key` | string | required | One key, spelled as in [`[keys]`](configuration.md#keys), such as `d` or `ctrl-r`. Keys the focused sidebar uses for itself — arrows, `h`, `j`, `k`, `l` with any modifier, `g`, `G`, `x`, `?`, `Space`, `Enter`, `Esc`, `Tab`, `Home`, `End`, `Page Up`, and `Page Down` — and chords of more than one key are dropped. |
| `label` | string | required | What it does, in a few words. |
| `confirm` | bool | `false` | Require a second press on the same row. |
| `close` | bool | `false` | Show the action as the row's `✕`, reached also with `x`. Always confirms. Only the first such action gets the `✕`. |

- Each snapshot replaces this producer's rows and nobody else's. `actions` replaces the producer's
  actions when present and keeps them when absent, so a producer can declare them once.
- rozi keeps the first 500 rows and 16 actions, drops rows with an empty or repeated `id` or an
  empty `label`, strips control sequences, and shortens labels to 160 characters, descriptions to
  80, `meta` to 24, and details to 160. A line that is not a snapshot is ignored and the last good one stays.
- End of input or any stream failure withdraws the producer's rows. So does removing the tab from
  the configuration, and, for an extension's process, the extension's generation retiring.

rozi writes events back, one JSON object per line:

```json
{"visible":true}
{"activate":"api/eu"}
{"expand":"api/eu"}
{"action":"rollback","row":"api"}
```

| Event | When |
| --- | --- |
| `visible` | When the stream opens, and whenever the tab starts or stops being on screen. A producer can stop its work while nobody looks. |
| `activate` | The user clicked the row or pressed `Enter` on it. |
| `expand` | The user expanded an `expandable` row. List its children in the next snapshot; until then rozi shows a loading line under it. |
| `action` | The user fired an action on a row, confirmed when it asks for confirmation. |

The producer must keep reading events. If its backlog of unread events fills up, rozi withdraws
its rows.

### Several producers in one tab

A tab may have several producers at once. A producer is identified by where it runs — for a
[placed extension process](writing-extensions.md#run-on-the-session-host), the host and, for a
session-bound placement, the session it was placed for; for anything else, this client — plus its
`name`. A producer that opens a new stream with the same identity replaces its own earlier stream:
the rows it published stay until its first snapshot, and the earlier stream closing afterwards
withdraws nothing. Events reach only the producer that owns the row.

With more than one producer, the tab heads each producer's rows with where it runs: `LOCAL` for
this machine or the host's name, then the session and the `name` when there are any, such as
`WORKBOX · backend`.

CLI bridge:

```sh
rozi sidebar publish --tab TAB [--name NAME]
```

## Stream ownership

The CLI attaches the extension `id` and `generation` when both are present in its environment, and
a placed process's `credential`.
Picker, publisher, and subscription streams owned by an extension close when its generation
retires, and its sidebar streams' rows are withdrawn: when the extension is disabled, removed, or has a change that affects its processes.
Changes to metadata only keep the generation.

### Session service credentials

The `session-services` capability allows deployed services with `placement = "session"`.
Session-issued credentials use the `session-v1:` namespace and bind requests to one session
instance and deployment generation. They are accepted directly by that session endpoint, and
refused by UI endpoints. Client-issued worker credentials and client generation provenance are
refused directly by session endpoints. A credential takes precedence over caller-supplied extension
provenance. Retiring a generation revokes its credentials and closes its held connections.

`extensions-status` includes each deployment's `services`, with `name`, `state`, optional `pid`,
and optional `reason`. A state is `starting`, `running`, `restarting`, or `stopped`.

### Session actions

The `session-actions` capability allows these commands on a session endpoint, and on a UI endpoint,
which forwards them to the session it is about:

```json
{"cmd":"invoke","action":"git-tools.run-tests","args":{"package":"rozi"},"timeout_ms":120000}
{"cmd":"invocation-list"}
{"cmd":"invocation-cancel","invocation_id":"7f3a..."}
```

`args` defaults to `{}`, and `timeout_ms` can only shorten the action's deadline. `invoke` holds its
answer until the call ends: `data` is an `InvocationResult` with `invocation_id` and `result`.
`invocation-list` answers an array of `InvocationInfo`, with `invocation_id`, `action`, `generation`,
`depth`, and `deadline_unix_ms`. `invocation-cancel` answers the cancelled call's `InvocationInfo`;
the call itself then answers `invocation-cancelled`. Closing the connection cancels its calls, on
either kind of endpoint.

Each call's process holds its own `session-v1:` credential, valid until the call ends. A nested call
made with it counts one deeper than its caller and inherits the time its caller has left. Requests
with client-issued provenance are refused. See [Actions](writing-extensions.md#actions) for the
process contract and limits, and [Call an action](control.md#call-an-action) for the error codes.

### Session notifications

`notify` accepts a session endpoint and returns a `SessionNotification` in `data`, with `id`,
`level`, nullable `title`, `message`, and `seq`. It raises `session-notification` with flat string
fields `id`, `level`, `title`, `message`, and `session`. The event envelope's `seq` equals the
notification's sequence. All attached UIs receive the event; headless subscribers receive it too.

Session subscription snapshots include a `notifications` array. Attaching a UI receives the same
retained inbox before subsequent live events. The inbox retains up to 64 notifications and 256 KiB
of message and title text for this server instance. The combined message and title limit is 8192
bytes. The inbox clears when the server restarts. Read state is per UI and does not change the
server's inbox or another client's presentation.
