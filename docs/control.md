# Control CLI

The `rozi` CLI can inspect and control a running UI without mounting another interface, and can
inspect and drive a named session that has no UI at all. Use it for shell scripts, hooks, services,
and extensions. See [Scripting](scripting.md) for a short start and
[Control protocol](control-protocol.md) for raw transport and NDJSON.

## Two endpoints

A control command talks to one of two things:

| Endpoint | Selected by | Serves |
| --- | --- | --- |
| A running UI | `--socket`, `ROZI_SOCKET`, or discovery | Every command. |
| A named session server | `--session <NAME>` | The commands a server can answer without a screen. |

`--session` needs nothing to be running but the session itself. A detached `dev` can be listed,
captured, typed into, and grown a pane from a shell script or an SSH login that never starts a
terminal UI.

```sh
rozi --session dev list-panes
rozi --session dev capture-pane --target 3
rozi --session dev send-keys --target 3 'cargo test' Enter
rozi --session dev split --workspace 9 --argv cargo watch -x test
```

The two endpoints return the same `{ok, code, data, error}` document and the same tables, so a
script reads one format either way. `code` appears on failures and is stable for automation.

A session endpoint serves what a server can decide on its own. It does not gain a script any
authority an attached client would not have: opening a pane still needs the layout-control lease to
be free, typing still respects the session's input lock, and a request carrying extension
provenance is refused because a server cannot check whether that extension is still active (see
[Extensions and `--session`](#extensions-and-session)).

`--session` and `--socket` name different endpoints and cannot be combined. A bare session name is
a launch target, not a control target: `rozi dev` starts a UI, so `rozi dev list-panes` is refused
and points at `--session dev` instead.

## Endpoint discovery

Without `--session`, control commands choose a UI endpoint in this order:

1. `--socket PATH`
2. `ROZI_SOCKET`
3. The only live control endpoint in the runtime directory

Discovery fails if the runtime directory contains no live endpoints or more than one. Pass
`--socket` when several UIs are running.

| Platform | Endpoint named by `ROZI_SOCKET` |
| --- | --- |
| Linux | Unix-domain socket under `$XDG_RUNTIME_DIR/rozi`, `/run/user/<uid>/rozi`, or a private per-user temporary directory |
| macOS | Unix-domain socket in Rozi's private runtime directory |
| Windows | Discovery entry under `%LOCALAPPDATA%\rozi\run` for a current-user named pipe |

On Windows, pass the discovery-entry path to the CLI. Do not read the entry and do not construct a
pipe name.

Every local pane receives `ROZI=1`, `ROZI_PANE`, and, when control is available, `ROZI_SOCKET` and
`ROZI_BIN`. Remote panes do not receive the local client's `ROZI_SOCKET` or `ROZI_BIN`, and neither
does a pane opened by `rozi --session <NAME> split`: there is no UI for those to name. Such a pane
still reaches its own session with `rozi --session <NAME>`.

## Commands

Put `--socket PATH` before the command when selecting an endpoint explicitly.

Run `rozi api describe` to inspect the control API version, schema version, session protocol
version, and capabilities implemented by the installed binary. It prints JSON and does not connect
to a UI or session:

```json
{
  "api": 1,
  "schema": 1,
  "session_protocol": 9,
  "capabilities": [
    "agent-waits",
    "layout-control",
    "pane-control",
    "published-activity",
    "remote-control",
    "session-control"
  ]
}
```

`schema` names the version of [the JSON Schema](#json-schema) below. It is its own number: the
session protocol bumps when two rozi binaries change how they frame messages to each other, which
does not affect the JSON anything else reads.

`--session` column: whether the command also works against a session server with no UI attached.

| Command | Purpose | `--session` |
| --- | --- | --- |
| `list-panes [--format text\|json]` | List panes visible to this endpoint. | yes |
| `layout get [--workspace 1-9] [--format text\|json]` | Report workspaces and where each pane sits. | yes |
| `layout set --workspace 1-9 <LAYOUT> [--if-revision N]` | Set a workspace's tiling layout. | yes |
| `pane set --target ID [--floating B] [--fullscreen B] [--rect X,Y,W,H \| --rect-fraction X,Y,W,H] [--if-revision N]` | Float, tile, place, or fullscreen a pane. | yes |
| `agents list [--format text\|json]` | List effective agent runtimes and exact references. | yes |
| `agents get --target ID` | Read one semantic agent record. | yes |
| `agents read --target ID [--scrollback N\|full]` | Capture an agent's terminal. | yes |
| `agents wait --target ID --until STATE [--timeout DURATION]` | Wait atomically for semantic state. | yes |
| `agents prompt --target ID [--wait STATE] TEXT` | Validate, submit, and optionally wait atomically. | yes |
| `agents report --agent ID --integration TOKEN --state STATE --seq N` | Publish incarnation- and sequence-fenced native agent state. | yes |
| `agents release --integration TOKEN --seq N` | Release integration authority. | yes |
| `metrics [--format text\|json]` | Read bounded client and cached server resource counters. | yes |
| `focus <PANE_ID>` | Focus a pane. | no |
| `send-text [--target <PANE_ID>] <TEXT>` | Send literal UTF-8 text. | yes |
| `send-keys [--target <PANE_ID>] [-l\|--literal] [--] <KEY\|TEXT>...` | Send named keys and text. | yes |
| `split [OPTIONS] [COMMAND \| --argv PROGRAM [ARG...]]` | Spawn a pane. | yes |
| `run-action <ACTION_ID>` | Run a built-in, configured, or extension command ID. | no |
| `capture-pane [--target ID] [--scrollback N\|full] [--last-output] [--format text\|json]` | Print pane text. | yes |
| `switch-workspace <1-9>` | Switch the active workspace. | no |
| `move-to-workspace <1-9>` | Move the focused pane. | no |
| `status [--target <PANE_ID>] <VALUE> [--reason TEXT]` | Report status for a pane. | yes |
| `status --clear [--target <PANE_ID>]` | Clear reported status. | yes |
| `notify <MESSAGE> [--title TEXT] [--level info\|error]` | Show a toast. | no |
| `subscribe [EVENT...]` | Stream events as NDJSON. An empty list subscribes to all events. | no |
| `pick [--title TEXT] [--placeholder TEXT] [--json]` | Open a modal picker using stdin and stdout. | no |
| `publish` | Publish Activity rows over stdin and receive activations on stdout. | no |

Control commands reject launch-only options: `--config`, `--read-only`, `--profile`, and `--pick`.

`--session <NAME>` is the target they accept, and `--remote <HOST>` says which machine that session
is on:

```bash
rozi --remote workbox --session dev list-panes
rozi --remote workbox --session dev agents prompt --target 3 --wait idle "run the tests"
```

Every command a local session answers, a remote one answers the same way, with the same output and
the same exit code. The request is forwarded, not the command line, and the answer is rendered
locally — so a remote command in a terminal prints the same table a local one does.

This reuses the SSH transport `--remote` attach already uses: saved hosts, connection multiplexing,
the discovered remote binary, and the same askpass rules. There is no daemon, no network port, and
no account. `--remote` without `--session` is refused, because a control command addresses a session
server and the far host's UI is not one.

Waits are not cut short by the hop. `agents wait` and `agents prompt --wait` are served by the
remote session server and run to their own deadline.

The far host needs a rozi that understands forwarding; an older one is reported as version skew
rather than as a broken command. Check with `rozi api describe` there, which lists
`remote-control` among its capabilities.

A `no` command refused against a session says what it needed a UI for. Focus, the active workspace,
toasts, pickers, and actions are client-local by design: a session server has no screen to move
focus on and no overlay to draw.

## Output

`list-panes`, `layout get`, `metrics`, and `capture-pane` print human-readable output to a terminal and stable JSON
when redirected. Use `--format text` or `--format json` to choose explicitly.

Other successful one-shot commands print a short acknowledgement on a terminal. Redirected output
keeps the JSON response. Errors go to stderr in human mode.

`list-panes` describes only the endpoint that answered. From a UI it includes the current
attachment and client-local scratch panes, not every named session; from `--session` it includes
every pane in that session, including panes whose process has exited, which report
`exited (<CODE>)` instead of `ready`. Use `rozi sessions list` to discover session servers.

## JSON Schema

Every shape on this page is described by
[`docs/schema/rozi-control-v1.schema.json`](https://github.com/tui-lipan/rozi/blob/master/docs/schema/rozi-control-v1.schema.json):
requests, the response envelope, error codes, each command's `data` payload, published activity
rows, agent records and references, and the event envelope.

The file is generated from the Rust types that serialize the wire format, and CI fails if
regenerating it produces a diff — so it cannot describe an API Rozi no longer has. Regenerate it
with:

```bash
cargo run --features schema-gen --bin rozi-api-schema
```

Two conventions worth knowing when you validate against it:

- **Objects accept unknown properties.** Responses gain fields; a client validating against an
  older copy of the schema keeps working. Do not reject a document for carrying something you do
  not recognize.
- **Enumerations are closed.** Error codes, agent states, wait conditions, and event names are
  fixed vocabularies, which is what makes validating against them useful. A new value there is an
  API change and moves the schema version.

`ControlResponse.data` is untyped in the envelope, because one envelope carries every command's
answer. The schema names each payload separately — `PaneInfo`, `AgentInfo`, `PaneCapture`,
`AgentWaitResult`, and the rest — so pick the one for the command you sent.

## Target selection

Commands that accept `--target` use it first.

Against a **UI endpoint**, the CLI otherwise sends `ROZI_PANE` as `source_pane`, and Rozi falls
back to the focused pane.

Against a **session endpoint**, `ROZI_PANE` is not sent and not honoured. A pane id says nothing
about which session it belongs to, and `--session` names a different one than the caller is
sitting in: a script inside pane 3 of `work` running `rozi --session dev send-text …` would
otherwise type into `dev`'s pane 3, a pane it never looked at. So a session endpoint takes
`--target` or resolves a session with exactly one pane, and otherwise fails with the ids to choose
from:

```text
session `dev` has 3 panes and no focused pane; pass --target (ids: 1, 2, 5)
```

`agents report` and `agents release` always require `--target` with `--session`, including for a
one-pane session. Integration hooks must opt into the named session's pane namespace explicitly.

A pane addressing its own session names itself explicitly:

```sh
rozi --session dev status working --target "$ROZI_PANE"
```

Target a pane explicitly when a script drives a pane it created:

```sh
ROZI_CMD=${ROZI_BIN:-rozi}
pane=$("$ROZI_CMD" split --workspace 9 --argv bash | jq -r '.data.id')
"$ROZI_CMD" send-text --target "$pane" 'printf "ready\n"'
"$ROZI_CMD" send-keys --target "$pane" Enter
```

Input sent while a PTY starts is queued. Input to an exited or failed PTY is rejected.

## Spawning panes

`split` leaves focus unchanged unless `--focus` is present.

Options:

- `--cwd DIR`
- `--title TEXT`
- `--workspace 1-9`
- `--focus`
- `--keep-open`
- `--argv PROGRAM [ARG...]`

Against `--session`, `split` commits the layout revision itself, so a client attaching later finds
the pane already placed. `[[rules]]` apply exactly as they do to a pane a person opens: a rule may
float it, make it fullscreen, and choose its workspace, and an explicit `--workspace` still wins
over the rule. Without either, the pane lands in workspace 1. The workspace's tiling arrangement is
left alone: the new pane is tiled beside the others when a client draws it, and a deliberate split
ratio survives.

Three refusals are specific to a session endpoint:

- **A client holds layout control.** Opening a shared pane means committing a layout revision over
  whatever that client is arranging, which is the controller's call — the session protocol already
  refuses the same thing from a non-controller client. Detach it, or ask it to open the pane.
  Reading and typing never needed the lease and keep working.
- **`--focus`** — there is no focus to move.
- **The session has panes but no layout document**, which happens only if nothing ever attached to
  place them. Committing one would claim the other panes do not exist, so the spawn is refused
  instead.

A headless pane's environment is `ROZI` and `ROZI_PANE` only. `ROZI_SOCKET` and `ROZI_BIN` name a
UI process and there is not one, and the desktop variables a client forwards (`DISPLAY`,
`WAYLAND_DISPLAY`, and whatever `[environment] forward` adds) are deliberately not taken from the
one-shot CLI process either: that process is gone seconds later, and the pane is not.

`[[rules]]` and the configured shell are read from the server's config when the spawn happens, not
when the server started, so an edited rule applies to the next headless `split` without restarting
a session that has been running for days.

A positional `COMMAND` is interpreted by the configured `command_shell`. `--argv` launches a
program directly and consumes the remaining arguments, so all pane options must come first.

```sh
rozi split --cwd "/repo with spaces" --title Tests --keep-open 'cargo test'
rozi split --workspace 9 --focus --argv cargo test -- --nocapture
```

The response waits up to five seconds for PTY readiness. `pty_ready: false` means the pane still
exists but has not reported ready yet.

## Layout

`layout get` reports every workspace and where each of its panes sits. `--workspace N` narrows the
report to one workspace.

```sh
rozi layout get --format json
rozi --session dev layout get --workspace 2 --format json
```

The report answers two separate questions, and keeps them apart:

- **How the session is arranged.** `workspaces` describes the session's shared layout document.
  The session server owns it and every client follows it, so both endpoints give the same answer.
- **What one UI shows.** `client` and each pane's `view_rect` describe a single UI's screen: its
  focus, the workspace it shows, and where it draws each pane. They appear only when a UI answered.

```json
{
  "session": "dev",
  "revision": 18,
  "canvas": { "cols": 160, "rows": 47 },
  "workspaces": [
    {
      "index": 1,
      "name": null,
      "layout": "dwindle",
      "synchronized": false,
      "panes": [
        {
          "id": 7,
          "reference": { "session_instance": "…", "pane_id": 7, "generation": 1 },
          "order": 0,
          "floating": false,
          "fullscreen": false,
          "rect": { "x": 0, "y": 0, "width": 96, "height": 47 },
          "rect_fraction": { "x": 0.0, "y": 0.0, "width": 0.6, "height": 1.0 },
          "view_rect": { "x": 0, "y": 1, "width": 95, "height": 46 }
        }
      ]
    }
  ],
  "client": {
    "active_workspace": 1,
    "focused_pane": 7,
    "controller": true,
    "committed": true,
    "viewport": { "cols": 160, "rows": 48 }
  }
}
```

| Field | Meaning |
| --- | --- |
| `revision` | The layout revision described. Null until something places a pane. A controlling UI sends any change it is still holding back before it answers, and reports the revision that change will have. |
| `canvas` | The canonical canvas `rect` is measured against: the pane area of the client that last controlled the layout. |
| `workspaces` | Every workspace in the layout document, including empty ones, so a script can see a workspace's layout before using it. `index` is one-based. A UI always reports all nine. A document the server started for a headless `split` holds only the workspaces it placed panes in; the others take each client's configured default layout. |
| `layout` | `dwindle`, `master`, `grid`, `columns`, `rows`, `scrollable`, or `monocle`. |
| `order` | The pane's position in the tiling order that every layout except Dwindle arranges panes in. Null for a floating pane. |
| `rect` | Where the pane sits on the canonical canvas, in whole cells. Gaps, borders, and the workbar are left out, because each client draws those differently. |
| `rect_fraction` | The pane's position as fractions of the canvas, rounded to six decimal places. Floating panes are stored this way, so their fractions are exact. |
| `view_rect` | Where this UI draws the pane, in cells of its own terminal, gaps and chrome included. Only for panes in the workspace the UI shows. |
| `unplaced_panes` | Session endpoint only: panes the server runs that no layout places yet. |
| `client.controller` | Whether this UI holds the layout-control lease. A UI without a shared session controls its own layout. |
| `client.committed` | False until the server has confirmed `revision`. A controlling UI sends a change without waiting for the confirmation. |

Tiled panes are listed in tiling order, then floating panes. Some geometry needs care:

- **Scrollable:** the strip of columns can be wider than the canvas, so a `rect` can extend past
  the right edge, and `rect_fraction` can exceed `1.0`. A UI scrolls the strip to follow focus.
  The shared `rect` always starts the strip at its first column, while `view_rect` shows where this
  UI has scrolled it.
- **Monocle:** every tiled pane has the same `rect`. The focused one is drawn on top.
- **Fullscreen:** `rect` is where the pane returns to afterwards. `view_rect` covers the screen
  while it is fullscreen.
- **Followers:** a follower centres the controller's canvas in its own window, so its
  `view_rect` values can start at a negative position or run past its edges.

### Changing the layout

`layout set` chooses a workspace's tiling layout. `pane set` floats, tiles, moves, or fullscreens
one pane.

```sh
rozi layout set --workspace 2 master
rozi pane set --target 7 --floating true --rect 10,5,80,24
rozi pane set --target 7 --fullscreen true
rozi --session dev pane set --target 3 --floating false --if-revision 18
```

Every write names what it changes: `layout set` needs `--workspace`, and `pane set` needs
`--target`. Neither falls back to focus or `ROZI_PANE`, and neither moves focus.

A write sets a state rather than toggling it. Options you leave out keep their current value.
Repeating a write, or asking for a state that already holds, succeeds with `changed: false`, and
creates no new revision. A refused write changes nothing.

`pane set` accepts:

| Option | Effect |
| --- | --- |
| `--floating true` | Float the pane. A tiled pane lifts off centred on the tile it leaves, at the default float size of 42% of the canvas, unless you also pass a rect. A pane that already floats stays where it is. |
| `--floating false` | Return the pane to the tiling, at the end of the tiling order. |
| `--fullscreen true\|false` | Make the pane fullscreen, or restore it. |
| `--rect X,Y,W,H` | Place a floating pane, in canvas cells. `X` may be negative. |
| `--rect-fraction X,Y,W,H` | Place a floating pane, as fractions of the canvas. |

A rect places a floating pane, so it needs a pane that floats already or `--floating true`.
Rects are clamped the same way a dragged float is: part of the pane may leave the canvas, but a
margin always stays on screen to grab. The float lands on whole cells.

The reply has the same shape from both endpoints:

```json
{ "changed": true, "revision": 19, "committed": false, "workspace": { "index": 1, "…": "…" } }
```

`workspace` is the affected workspace in the shape `layout get` reports it. `revision` is the
revision the layout has with the change applied. A session endpoint commits the change itself, so
`committed` is always `true` there. A UI sends its commit and answers before the server confirms
it; if the server rejects the commit, the UI takes the server's layout back.

`--if-revision N` refuses the write with `conflict` unless the layout is still at revision `N`. Read
the revision with `layout get`, decide, then write with `--if-revision`, so that a change someone
made in the meantime is not overwritten. Each successful write's reply gives the revision to pass
to the next one.

A write needs layout authority, the same as a person rearranging panes:

- A UI must hold layout control. A follower fails with `not-controller`, and a read-only UI with
  `read-only`.
- A session endpoint refuses with `not-controller` while any client holds layout control. It
  fails with `unavailable` for a session that has panes but no layout document, which is also when
  `split` is refused.
- A scratch pane is client-local and has no shared layout, so `pane set` refuses it with
  `unsupported`.
- A pane the layout does not place fails with `pane-not-found`.

## Sending keys and capturing output

`send-keys` recognizes tmux-style names including `C-c`, `M-x`, `Enter`, `Escape`, `Space`, `Tab`,
`BSpace`, arrows, `Home`, `End`, `PgUp`, `PgDn`, and `F1` through `F12`. Unknown tokens are sent as
literal text. `--literal` makes every token literal. `--` ends option parsing.

```sh
rozi send-keys C-c
rozi send-keys 'echo hi' Enter
rozi send-keys --literal C-c
rozi send-keys -- -n hello
```

`capture-pane` returns the visible grid by default. `--scrollback N` returns trailing retained
lines, `--scrollback full` returns all retained lines, and `--last-output` returns the most recent
shell-integration command output. A full-scrollback reply can exceed 1 MiB; the CLI reads Rozi's
responses without the incoming request size cap.

## Actions, status, and notifications

`run-action` accepts:

- built-in action IDs such as `toggle-float`
- IDs from `[[commands]]`, such as `branches`
- extension command IDs, such as `git-tools.branches`

Destructive actions honor `[confirm]`.

`status` accepts short free-form values. `working`, `blocked`, `done`, and `idle` have built-in
presentation. Values are limited to 64 characters and reasons to 256 after display-text
sanitization. `--target` may be written on either side of the value, and is the only way to name a
pane from a script that is not running inside one. The update is queued to the session server, so a
successful reply does not guarantee that every client has rendered it.

Use `notify` for failures and successful results that are otherwise off screen:

```sh
rozi notify "tests failed" --title Build --level error
```

## Subscriptions

`subscribe` prints one object per line until the endpoint closes:

```sh
rozi subscribe pane-exited pane-status-changed |
  jq -r 'select(.event == "pane-status-changed") | [.data.pane, .data.status] | @tsv'
```

Every event has `event` and `data`. Event fields are under `data`. See
[Hooks](hooks.md#events-and-fields) for the event field table.

## Pickers

Plain mode reads one label per input line and prints the selected label:

```sh
branch=$(git branch --format='%(refname:short)' | rozi pick --title Branch) || exit 0
git switch -- "$branch"
```

Selection exits `0`, cancellation exits `1`, and transport failure exits `2`.

Use `--json` for stable row IDs, descriptions, groups, disabled or active rows, custom actions,
prompts, empty-collection copy, tabs, and live replacement. The first input line is picker metadata
and may contain initial rows. Later input lines replace the complete row set.

```json
{"title":"Branches","rows":[{"id":"main","label":"main","active":true},{"id":"old","label":"old","disabled":"protected"}]}
```

JSON mode prints selection, cancellation, and action objects. An action without `close: true` keeps
the picker open so the producer can send refreshed rows. `empty` is producer copy for an empty row
list while the filter is empty; a filter miss always says `No matches`. `prompt` may be a title
string or an object with `title`, `placeholder`, `value`, and `masked`.

Declare `tabs` to show several related lists under one picker. Each tab keeps its own rows, filter,
and highlight; `Tab`/`Shift+Tab` or `Right`/`Left` switch between them. Row snapshots name their
tab, and JSON mode prints `{"tab":"…"}` on each switch so the producer can load a tab when it is
first shown:

```json
{"title":"Git","tabs":[{"id":"branches","label":"Branches"},{"id":"worktrees","label":"Worktrees"}],"rows":[{"id":"main","label":"main"}]}
{"tab":"worktrees","rows":[{"id":"/src/rozi-review","label":"rozi-review"}]}
```

Selections and actions from a tabbed picker carry the tab: `{"selected":"main","tab":"branches"}`.
See [Picker protocol](control-protocol.md#picker-stream).

## Published activity

`publish` keeps a bidirectional stream open. Write complete row snapshots to stdin:

```json
{"rows":[{"id":"job-1","title":"Run tests","status":"working","active":true}]}
```

Read activation requests from stdout:

```json
{"activate":"job-1"}
```

An empty row list or a closed stream withdraws the rows. Use stable IDs. A process with
`ROZI_PANE` publishes for that pane. A supervised service has no pane ID, so Rozi uses the focused
live pane when the stream opens. Extension-owned streams close when their runtime generation
retires.

Published rows appear even when the pane has no detected agent. When a known agent publishes rows,
Rozi derives that agent's displayed state from the rows instead of trying to assign one visible
screen to several activities.

See [Published activity protocol](control-protocol.md#published-activity-stream) and
[Sidebar](sidebar.md).

## Driving a detached session

`--session <NAME>` reaches the session server directly. Nothing has to be attached, and nothing
becomes attached: the request is answered and the connection closes, so the session's client count
and layout control are untouched and a script cannot make an empty session look occupied.

```sh
#!/bin/sh
# Start a build in a session nobody is looking at, then read the result back.
pane=$(rozi --session dev split 'cargo test' | jq -r '.data.id')
until rozi --session dev list-panes --format json |
  jq -e --argjson p "$pane" '.data[] | select(.id == $p and (.status | startswith("exited")))' \
  >/dev/null; do
  sleep 2
done
rozi --session dev capture-pane --target "$pane" --scrollback full --format text
```

The session must already exist. Start one with `rozi sessions new dev`, or leave a detached
`rozi dev` running.

There is no general event stream against a session endpoint. `subscribe` reports UI events, which a
server does not raise. Agent state is the exception: `agents wait` registers a semantic predicate
inside the server without polling or requiring an attached UI.

## Extensions and `--session`

The CLI stamps every request with the calling extension's id and generation when it finds
`ROZI_EXTENSION` in the environment. That generation is a fencing token a running rozi mints on
each config reload, so a disabled or reloaded extension's leftover processes stop being obeyed.

A session server cannot check it — the token is minted per UI process and the server never sees it
— so it refuses such requests rather than honouring a fence nobody checked:

```text
a session server cannot check whether extension `git-tools` is still active, and will not act on
its behalf; reach a running rozi instead, or clear ROZI_EXTENSION when the caller is not the
extension
```

An extension that wants to drive a session should go through a running rozi, which does check. A
person typing in a pane that an extension happened to open inherits `ROZI_EXTENSION` from it and
hits the same refusal; `env -u ROZI_EXTENSION rozi --session …` says the request is theirs, not the
extension's.

## Session lifecycle

These commands use session endpoints rather than a UI control endpoint:

```sh
rozi dev
rozi sessions attach dev
rozi sessions attach dev --read-only
rozi sessions new dev
rozi sessions new review --profile dev
rozi sessions new api --cwd ~/src/api
rozi sessions list
rozi sessions kill dev
```

Git checkouts have their own `rozi worktrees` namespace. See
[Worktrees from the command line](sessions.md#worktrees-from-the-command-line).

Remote forms are limited to session lifecycle:

```sh
rozi sessions list --remote workbox
rozi sessions kill dev --remote workbox
```

`--session` control commands are local only. To drive a session on another machine, run the same
command over `ssh`, where it is local again:

```sh
ssh workbox rozi --session dev capture-pane --target 3
```

See [Sessions](sessions.md) and [Remote sessions](remote.md).
