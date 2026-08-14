# Control socket and automation

When the UI starts, rozi binds a per-process control endpoint at `control-<pid>.sock` in its
runtime directory. If binding fails, the UI still starts and shows a warning; panes only receive
`ROZI_SOCKET` when an endpoint is available.

## Endpoints per platform

| | Runtime directory | Transport |
| --- | --- | --- |
| Linux | `$XDG_RUNTIME_DIR/rozi`, else a private per-uid temp directory | Unix-domain socket at that path |
| macOS | a private directory under `$TMPDIR` | Unix-domain socket at that path |
| Windows | `%LOCALAPPDATA%\rozi\run` | Named pipe `\\.\pipe\rozi.<user-sid>.control-<pid>` |

On Windows the file in the runtime directory is a **discovery entry**, not the transport: it stands
for a named pipe whose name is derived from the entry's own name and your SID. Point `--socket` and
`ROZI_SOCKET` at the entry, exactly as you would at a socket path on Unix — the same command
line works on all three platforms. Nothing trusts the entry's contents; the pipe name is recomputed
rather than read out of it, and every connection completes an authenticated handshake regardless.

Endpoints are private to the user who created them (mode `0600` on Unix; a protected
current-user-SID DACL plus `PIPE_REJECT_REMOTE_CLIENTS` on Windows, which makes a pipe unreachable
over SMB). An entry left behind by a crashed process is replaced on the next bind; a *live* one
cannot be squatted.

Every spawned pane receives:

- `ROZI=1`
- `ROZI_PANE=<pane id>`
- `ROZI_SOCKET=<endpoint path>` when control is available
- `ROZI_BIN=<path to this rozi binary>`, so a script can call back without assuming a `PATH`
  install. Not set for a pane reached over `--remote`: that PTY runs on the other host, where
  this client's path names nothing.

## CLI

Control commands do not mount the UI. Endpoint discovery order is `--socket PATH`, `ROZI_SOCKET`,
then exactly one live endpoint in the runtime directory.

```bash
rozi list-panes
rozi metrics
rozi focus 3
rozi send-text 'cargo test
'
rozi send-keys C-c
rozi send-keys 'echo hi' Enter
rozi send-keys -l C-c
rozi send-keys -- -n hello
rozi split 'claude --agent helper'
rozi split 'cargo watch' --focus
rozi run-action toggle-float
rozi capture-pane --target 3
rozi capture-pane --scrollback full
rozi capture-pane --last-output
rozi capture-pane --scrollback 200 --target 3
rozi run-action copy-last-output
rozi switch-workspace 2
rozi move-to-workspace 3
rozi status blocked --reason "needs approval"
rozi status --clear
rozi pick --title "Branch"
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
`focus` field of a matched `[[rules]]` entry; the rule still decides workspace, float, position, and
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
rozi dev                 # attach, or launch canonical same-name profile
rozi attach dev          # attach only; error unless running
rozi attach dev --read-only
rozi new dev             # explicitly create a fresh named session
rozi new review --profile dev # create from a reusable launch recipe
rozi list-sessions       # list connectable named sessions
rozi kill-session dev    # stop the named session server and its PTYs
rozi list-sessions --remote workbox
rozi kill-session dev --remote workbox
```

`kill-session <NAME>` is the sole canonical command for stopping a named session server. There is
one server per session; the command asks it to shut down through the authenticated protocol first,
then uses local stale-server recovery only when that local protocol transport cannot complete. It
destroys the server's PTYs for every attached client and is not a generic process killer. The
`--remote` form runs the same command over SSH and never force-terminates the local SSH transport.

Agents that need to control panes or session lifecycle should first read the operational contract
printed by `rozi --skill`.

An unknown positional target errors instead of silently creating a session. `attach` and `new` are
reserved command words; use `rozi --session attach` or `rozi --session new` to target those
literal names. See [Sessions](sessions.md) for profile resolution and attach/detach semantics.

The per-run control socket always belongs to the **local UI process**. When you are attached with
`--remote`, automation against that UI still uses the local `ROZI_SOCKET` / `--socket` path.
`list-sessions --remote` and `kill-session --remote` are separate SSH helpers that talk to rozi
on the remote host; they are not control-socket commands. See [Remote SSH sessions](remote.md).

Launch-time options are rejected on a control command rather than ignored, so `rozi --remote box
list-panes` is an error instead of a local answer that looks like a remote one. That covers
`--remote`, `--config`, `--read-only`, `--pick`, `--profile`, and a session target.

## Wire protocol

The socket accepts one newline-delimited JSON request per connection and returns one JSON response.
`subscribe`, `publish`, and `pick` are the exceptions: after their acknowledgement the connection stays
open. `subscribe` then streams newline-delimited event objects until disconnected, and
`publish` runs in both directions.

Requests use a `cmd` field: `list-panes`, `metrics`, `focus`, `send-text`, `send-keys`, `new-pane`,
`run-action`, `capture-pane`, `switch-workspace`, `move-to-workspace`, `set-status`, `popup`,
`publish`, `pick`, or `subscribe`. A client may include `source_pane`; the CLI derives it from
`ROZI_PANE`.

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
{"cmd":"pick","title":"Branch","placeholder":"Search branches…"}
```

Responses have `ok`, and either `data` or `error`.

Popup dimensions are viewport fractions, clamped to `0.2`-`0.95`; omitted dimensions default to
`0.6`. Only one popup may exist at a time. `cwd` defaults to the focused pane's working directory.
`keep_open` defaults to `true`: the popup holds after its command exits, printing the exit status
and retaining its final screen as a read-only result, so short commands stay readable. Enter,
Escape, or Space dismisses a completed popup. Pass `"keep_open":false` to have the popup close with
its command. A popup also closes when its backdrop is clicked or through the normal close action;
while the command is running, Escape is sent to the popup application.

Subscriptions support `pane-spawned`, `pane-exited`, `pane-status-changed`, `bell`, `focus-changed`,
`workspace-switched`, `session-attached`, `session-detached`, `session-renamed`,
`controller-changed`, `client-joined`, `client-left`, `profile-loaded`, `profile-applied`,
`profile-saved`, `session-created`, and `config-reloaded`. An empty `events` list subscribes to all
17. `pane-status-changed` carries
`pane`, `status`, `reason`, `previous_status`, `previous_reason`, and `focused`; cleared or absent values are
empty strings. Initial status received while attaching is state seeding, not a transition, and does
not emit this event.
Event names and existing fields are stable; later versions may add events or fields. See
[Hooks](hooks.md#events-and-fields) for the complete field table. Slow subscribers are bounded and
disconnected rather than blocking the UI. Example:
`printf '%s\n' '{"cmd":"subscribe"}' | socat - UNIX-CONNECT:$ROZI_SOCKET | jq`.

## Picker protocol

An external program can ask rozi to render a modal fuzzy search palette and return the user's choice.
By default stdin is one row label per line and stdout is the chosen line, so a shell pipeline needs
no `jq` at either end:

```bash
git branch --format='%(refname:short)' | rozi pick --title Branch
ls | rozi pick --title Open | xargs -r $EDITOR
```

Plain mode sends the whole list once stdin closes. Pass `--json` to speak the wire format instead —
for groups, badges, disabled rows, or to replace the row set while the palette is already open:

```bash
git branch --format='{"id":"%(refname:short)","label":"%(refname:short)"}' \
  | jq -sc '{rows:.}' | rozi pick --json --title Branch
```

In `--json` mode the **first stdin line is the request** - `title`, `placeholder`, `width`,
`actions`, and optionally an initial `rows` - and every later line is a rows update. That is the
only way to declare `width` and `actions`, which have no flag spelling:

```bash
{ echo '{"title":"Branch","width":72,"actions":[{"id":"delete","key":"ctrl-d","label":"delete"}],"rows":[…]}'
  # …later lines refresh the list while the palette is open
} | rozi pick --json
```

`--json` also prints the raw terminal line (`{"selected":"…"}`) rather than the bare id, and is the
only mode that reports a cancellation on stdout. Both modes exit 0 on a selection, 1 on a
cancellation, and 2 on a transport failure.

Unlike one-shot commands, `pick` opens a streaming connection:

1. The client sends `{"cmd":"pick","title":"...","placeholder":"..."}`.
2. Rozi checks whether a picker can be opened:
   - If a picker is already open, it immediately returns `{"ok":false,"error":"a picker is already open"}` and closes.
   - If any modal overlay (palette, help, settings, search, rename prompt) is open, it immediately returns `{"ok":false,"error":"an overlay is open"}` and closes.
   - Otherwise, rozi acknowledges with `{"ok":true}`.
3. The client writes row updates (`{"rows":[...]}`), each line replacing the row set.
4. When the user selects a row or cancels (Esc), rozi writes exactly one terminal line and closes:
   - `{"selected":"<id>"}` on activation (CLI exits with status 0). If `id` is omitted in the row, `label` is returned.
   - `{"cancelled":true}` on cancellation (CLI exits with status 1).

### Actions

Beyond select and cancel, a caller can declare extra chords. They are advertised in the footer the
way every built-in picker advertises its own, and reported when pressed:

```json
{"cmd":"pick","title":"Switch branch","width":72,
 "actions":[{"id":"create","key":"ctrl-n","label":"new branch","prompt":"Branch name","close":true},
            {"id":"delete","key":"ctrl-d","label":"delete"}]}
```

Action attributes:
- `id` (string, required): returned as `action`.
- `key` (string, required): the chord, spelled as in `[keys]` (`ctrl-n`). An unparseable chord drops
  the action rather than leaving a footer hint that never fires.
- `label` (string, required): footer text.
- `prompt` (string, optional): opens a text prompt with this title; the entered text rides back as
  `input`. The prompt **replaces** the picker rather than stacking on it, matching every other
  nested dialog; cancelling reports nothing and rebuilds the picker with its filter text intact.
- `close` (bool, default `false`): whether firing it ends the picker.
- `confirm` (bool, default `false`): require a second press, the way the session picker arms a
  kill. The armed row is struck through in the error colour with an `again to <label>` cue; moving
  the highlight, or a refresh that drops that row, disarms it.

A fired action writes one line and, unless it declared `close`, **leaves the palette open** so the
caller can answer with an updated `{"rows":[...]}` - deleting a row and re-listing is one round
trip, not a reopen:

```json
{"action":"delete","selected":"feat/x"}
{"action":"create","input":"feat/y","selected":"feat/x"}
```

`selected` is the row under the cursor at the time, or `null` when the list is empty, so an action
can be about a row without the caller tracking the highlight itself.

`width` sets the modal width in columns, clamped to 30-120; omitted, it uses the same 60 every
built-in palette uses.

Row attributes supported in `rows`:
- `id` (string): identifier returned in `selected`. Falls back to `label` when omitted.
- `label` (string, required): entry text.
- `description` (string, optional): right-aligned status badge.
- `group` (string, optional): section category header.
- `disabled` (string, optional): replaces the status text, renders muted, and makes the row inert on activation.
- `active` (boolean, optional): highlights entry as active.
- `priority` (integer, optional): boosts sorting rank.

## Agent slots

A pane is one terminal, but a program running inside it may be running several agents at once — a
client with its own tab bar, a parent session and its subagents. Screen detection can only ever see
the one on screen, so it reports a single state for all of them and cannot say which it belongs to.
Such a program publishes them instead:

```bash
rozi publish
```

The command bridges stdin and stdout to rozi and runs until either side closes. Write one JSON
object per line to publish the current list; read one per line to learn that a user clicked a row:

```json
{"rows":[{"id":"ses_a","title":"audit the widget layer","status":"working","active":true},
         {"id":"ses_b","title":"fix the flaky test","status":"blocked","reason":"permission required"}]}
```

```json
{"activate":"ses_b"}
```

Each row becomes its own item in the sidebar's [Activity tab](sidebar.md), with its own elapsed time
and its own finished pulse — so a background tab that finishes, or one that stops for a permission
prompt, is visible without switching to it. `id` is yours and opaque to rozi; keep it stable, as
it is what ties a run to its clock across reordering and retitling. `active` marks the row you
currently have on screen, which is how rozi knows a finish on a *different* row has not been
seen. `status` takes the same values as `set-status`.

`title` is what the row shows, and it outranks `reason`. Send an empty one while you have nothing
real to say — an agent that has not titled its work yet — and the row falls back to `reason` until
you do; never send an id or a placeholder, which would pin the row to it. Publishing again with a
title swaps the row over immediately.

Publishing replaces the whole list, and an empty list withdraws it. Closing the connection also
withdraws it, so a publisher that exits or crashes never leaves rows behind. While a pane publishes
rows, rozi stops scraping its screen entirely and takes the pane's own state from them: blocked
if any row is blocked, working if any is working, idle once all are.

Activating a row focuses the pane and writes `{"activate":"<id>"}` back to you; bringing that row
on screen is your side of the exchange. Use `rozi publish` rather than opening
`ROZI_SOCKET` yourself — on Windows that variable names a discovery entry whose pipe name has to
be derived rather than read, so the bridge is what makes a publisher portable.

`integrations/opencode/rozi-agent-state.js` is a worked example of the simpler `set-status` form.

## Pane logging

`{"cmd":"pane-logging","target":3,"enabled":true}` enables logging for pane 3. Omit `target`
to use the focused/source pane and omit `enabled` to toggle.
