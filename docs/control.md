# Control CLI

The `rozi` CLI can inspect and control a running UI without mounting another interface. Use it for
shell scripts, hooks, services, and extensions. See [Scripting](scripting.md) for a short start and
[Control protocol](control-protocol.md) for raw transport and NDJSON.

## Endpoint discovery

Control commands choose an endpoint in this order:

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
`ROZI_BIN`. Remote panes do not receive the local client's `ROZI_SOCKET` or `ROZI_BIN`.

## Commands

Put `--socket PATH` before the command when selecting an endpoint explicitly.

| Command | Purpose |
| --- | --- |
| `list-panes [--format text\|json]` | List panes visible to this UI attachment. |
| `metrics [--format text\|json]` | Read bounded client and cached server resource counters. |
| `focus <PANE_ID>` | Focus a pane. |
| `send-text [--target <PANE_ID>] <TEXT>` | Send literal UTF-8 text. |
| `send-keys [--target <PANE_ID>] [-l\|--literal] [--] <KEY\|TEXT>...` | Send named keys and text. |
| `split [OPTIONS] [COMMAND \| --argv PROGRAM [ARG...]]` | Spawn a pane. `new-pane` is an alias. |
| `new-pane [OPTIONS] [COMMAND \| --argv PROGRAM [ARG...]]` | Spawn a pane. |
| `run-action <ACTION_ID>` | Run a built-in, configured, or extension command ID. |
| `capture-pane [--target ID] [--scrollback N\|full] [--last-output] [--format text\|json]` | Print pane text. |
| `switch-workspace <1-9>` | Switch the active workspace. |
| `move-to-workspace <1-9>` | Move the focused pane. |
| `status <VALUE> [--reason TEXT]` | Report status for the source or focused pane. |
| `status --clear` | Clear reported status. |
| `notify <MESSAGE> [--title TEXT] [--level info\|error]` | Show a toast. |
| `subscribe [EVENT...]` | Stream events as NDJSON. An empty list subscribes to all events. |
| `pick [--title TEXT] [--placeholder TEXT] [--json]` | Open a modal picker using stdin and stdout. |
| `publish` | Publish Activity rows over stdin and receive activations on stdout. |

Control commands reject launch-only options such as `--remote`, `--config`, `--read-only`,
`--profile`, `--pick`, and a session target. The endpoint always belongs to a local UI process.

## Output

`list-panes`, `metrics`, and `capture-pane` print human-readable output to a terminal and stable JSON
when redirected. Use `--format text` or `--format json` to choose explicitly.

Other successful one-shot commands print a short acknowledgement on a terminal. Redirected output
keeps the JSON response. Errors go to stderr in human mode.

`list-panes` describes only the UI endpoint that answered. It includes the current attachment and
client-local scratch panes, not every named session. Use `rozi list-sessions` to discover session
servers.

## Target selection

Commands that accept `--target` use it first. Otherwise the CLI sends `ROZI_PANE` as
`source_pane`. If neither is available, Rozi uses the focused pane.

Target a pane explicitly when a script drives a pane it created:

```sh
ROZI_CMD=${ROZI_BIN:-rozi}
pane=$("$ROZI_CMD" new-pane --workspace 9 --argv bash | jq -r '.data.id')
"$ROZI_CMD" send-text --target "$pane" 'printf "ready\n"'
"$ROZI_CMD" send-keys --target "$pane" Enter
```

Input sent while a PTY starts is queued. Input to an exited or failed PTY is rejected.

## Spawning panes

`split` and `new-pane` leave focus unchanged unless `--focus` is present.

Options:

- `--cwd DIR`
- `--title TEXT`
- `--workspace 1-9`
- `--focus`
- `--keep-open`
- `--argv PROGRAM [ARG...]`

A positional `COMMAND` is interpreted by the configured `command_shell`. `--argv` launches a
program directly and consumes the remaining arguments, so all pane options must come first.

```sh
rozi new-pane --cwd "/repo with spaces" --title Tests --keep-open 'cargo test'
rozi new-pane --workspace 9 --focus --argv cargo test -- --nocapture
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
sanitization. The update is queued to the session server, so a successful reply does not guarantee
that every client has rendered it.

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

## Session lifecycle

These commands use session endpoints rather than a UI control endpoint:

```sh
rozi dev
rozi attach dev
rozi attach dev --read-only
rozi new dev
rozi new review --profile dev
rozi list-sessions
rozi kill-session dev
```

Remote forms are limited to session lifecycle:

```sh
rozi list-sessions --remote workbox
rozi kill-session dev --remote workbox
```

See [Sessions](sessions.md) and [Remote sessions](remote.md).
