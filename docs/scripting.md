# Scripting

Rozi's CLI is the portable automation interface. It handles Unix sockets and Windows named pipes,
so scripts should not open `ROZI_SOCKET` themselves.

Inside a Rozi pane, hook, service, or extension process:

- `ROZI_BIN` is the matching running Rozi executable.
- `ROZI_SOCKET` identifies the current UI endpoint.
- `ROZI_PANE` identifies the calling pane when one exists.

Use the injected binary when available:

```sh
ROZI_CMD=${ROZI_BIN:-rozi}
"$ROZI_CMD" list-panes --format json
```

The CLI discovers the endpoint from `ROZI_SOCKET`. Outside Rozi, pass one explicitly with
`--socket PATH`, or let the CLI use the only live endpoint in the runtime directory.

## Copyable tasks

### Run an action

```sh
ROZI_CMD=${ROZI_BIN:-rozi}
"$ROZI_CMD" run-action toggle-sidebar
```

`run-action` accepts built-in action IDs, configured `[[commands]]` IDs, and extension command IDs.

### Open a process without shell parsing

```sh
ROZI_CMD=${ROZI_BIN:-rozi}
"$ROZI_CMD" new-pane --workspace 9 --title Tests --focus --argv cargo test -- --nocapture
```

Place pane options before `--argv`. Everything after it is the executable and its arguments.

### Send text to a pane

```sh
ROZI_CMD=${ROZI_BIN:-rozi}
pane=$("$ROZI_CMD" list-panes --format json | jq -r '.data[0].id')
"$ROZI_CMD" send-text --target "$pane" 'cargo test'
"$ROZI_CMD" send-keys --target "$pane" Enter
```

Use `send-text` for literal text and `send-keys` for named keys such as `Enter`, `C-c`, and `F2`.

### Pick and switch a branch

```sh
ROZI_CMD=${ROZI_BIN:-rozi}
branch=$(git branch --format='%(refname:short)' | "$ROZI_CMD" pick --title Branch) || exit 0
[ -n "$branch" ] || exit 0
git switch -- "$branch"
```

Picker cancellation exits with status `1`. The script treats cancellation as a normal stop.

### Watch events

```sh
ROZI_CMD=${ROZI_BIN:-rozi}
"$ROZI_CMD" subscribe pane-exited pane-status-changed |
  jq -r 'select(.event == "pane-exited") | "pane \(.data.pane) exited \(.data.code)"'
```

Event fields are under `data`.

### Report work state

```sh
ROZI_CMD=${ROZI_BIN:-rozi}
"$ROZI_CMD" status working --reason "running tests"
if cargo test; then
  "$ROZI_CMD" status done --reason "tests passed"
else
  "$ROZI_CMD" status blocked --reason "tests failed"
fi
```

Clear the report with:

```sh
"${ROZI_BIN:-rozi}" status --clear
```

Use [Automation recipes](recipes.md) for longer tasks, [Control CLI](control.md) for every command,
and [Control protocol](control-protocol.md) when writing a client that cannot invoke the CLI.
