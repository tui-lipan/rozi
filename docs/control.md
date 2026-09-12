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

The two endpoints return the same `{ok, data, error}` document and the same tables, so a script
reads one format either way.

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
| Linux | Unix-domain socket under `$XDG_RUNTIME_DIR/rozi`, or a private per-user temporary directory |
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

`--session` column: whether the command also works against a session server with no UI attached.

| Command | Purpose | `--session` |
| --- | --- | --- |
| `list-panes [--format text\|json]` | List panes visible to this endpoint. | yes |
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

Control commands reject launch-only options: `--remote`, `--config`, `--read-only`, `--profile`,
and `--pick`. `--session <NAME>` is the one target they accept, and only a local one — reaching a
session on another machine still means running `rozi` there, over `ssh`.

A `no` command refused against a session says what it needed a UI for. Focus, the active workspace,
toasts, pickers, and actions are client-local by design: a session server has no screen to move
focus on and no overlay to draw.

## Output

`list-panes`, `metrics`, and `capture-pane` print human-readable output to a terminal and stable JSON
when redirected. Use `--format text` or `--format json` to choose explicitly.

Other successful one-shot commands print a short acknowledgement on a terminal. Redirected output
keeps the JSON response. Errors go to stderr in human mode.

`list-panes` describes only the endpoint that answered. From a UI it includes the current
attachment and client-local scratch panes, not every named session; from `--session` it includes
every pane in that session, including panes whose process has exited, which report
`exited (<CODE>)` instead of `ready`. Use `rozi sessions list` to discover session servers.

## Target selection

Commands that accept `--target` use it first. Otherwise the CLI sends `ROZI_PANE` as
`source_pane`.

A UI endpoint then falls back to the focused pane. A session endpoint has no focus, so it resolves
a session with exactly one pane and otherwise fails with the pane ids to choose from:

```text
session `dev` has 3 panes and no focused pane; pass --target (ids: 1, 2, 5)
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

Against `--session`, `split` appends the pane to the named workspace (workspace 1 by default) and
commits the layout revision itself, so a client attaching later finds the pane already placed. The
workspace's tiling arrangement is left alone: the new pane is tiled beside the others when a client
draws it, and a deliberate split ratio survives. `--focus` is refused, since there is no focus to
move, and a session that has panes but has never had a client — and therefore has no layout
document — refuses the spawn rather than committing one that claims its other panes do not exist.

A positional `COMMAND` is interpreted by the configured `command_shell`. `--argv` launches a
program directly and consumes the remaining arguments, so all pane options must come first.

```sh
rozi split --cwd "/repo with spaces" --title Tests --keep-open 'cargo test'
rozi split --workspace 9 --focus --argv cargo test -- --nocapture
```

The response waits up to five seconds for PTY readiness. `pty_ready: false` means the pane still
exists but has not reported ready yet.

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
shell-integration command output.

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
prompts, and live replacement. The first input line is picker metadata and may contain initial
rows. Later input lines replace the complete row set.

```json
{"title":"Branches","rows":[{"id":"main","label":"main","active":true},{"id":"old","label":"old","disabled":"protected"}]}
```

JSON mode prints selection, cancellation, and action objects. An action without `close: true` keeps
the picker open so the producer can send refreshed rows. See
[Picker protocol](control-protocol.md#picker-stream).

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

There is no event stream against a session endpoint. `subscribe` reports UI events, which a server
does not raise; poll `list-panes` for pane lifecycle, reported status, and detected agent state
instead — all three are server-owned and current in every reply.

## Session lifecycle

These commands use session endpoints rather than a UI control endpoint:

```sh
rozi dev
rozi sessions attach dev
rozi sessions attach dev --read-only
rozi sessions new dev
rozi sessions new review --profile dev
rozi sessions list
rozi sessions kill dev
```

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
