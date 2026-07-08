# Control socket and automation

When the UI starts, hyprmux tries to bind an in-process Unix control socket at
`$XDG_RUNTIME_DIR/hyprmux/control-<pid>.sock` (falling back to the system temp directory). If binding
fails, the UI still starts and shows a warning; panes only receive `HYPRMUX_SOCKET` when a socket is
available.

Every spawned pane receives:

- `HYPRMUX=1`
- `HYPRMUX_PANE=<pane id>`
- `HYPRMUX_SOCKET=<socket path>` when control is available

## CLI

Control commands do not mount the UI. Socket discovery order is `--socket PATH`, `HYPRMUX_SOCKET`,
then exactly one live socket in the runtime directory.

```bash
hyprmux list-panes
hyprmux focus 3
hyprmux send-text 'cargo test
'
hyprmux split 'claude --agent helper'
hyprmux run-action toggle-float
hyprmux capture-pane --target 3
hyprmux switch-workspace 2
hyprmux move-to-workspace 3
```

Replies are JSON on stdout. Errors are JSON when returned by the server, or plain stderr for client
discovery/connect failures. `split`/`new-pane` replies as soon as the pane is accepted by the UI; the
PTY may still be starting (`pty_ready: false`). Named `send-keys` compatibility is not implemented;
`send-keys` currently aliases literal text.

`run-action` takes any keybindable action's stable id (the same ids used in `[keys]` config and
shown in the command palette/help overlay), e.g. `toggle-float`, `spawn`, `close`. `capture-pane`
returns the plain text of a pane's current visible snapshot grid, defaulting to the request's
`source_pane` or the focused pane when `--target`/`target` is omitted. `switch-workspace` and
`move-to-workspace` take a 1-9 workspace number, matching the on-screen tabs.
Destructive `run-action` calls honor `[confirm]` settings; a first call can arm a confirmation
toast and a second matching call within the toast window confirms it.

Session lifecycle commands are separate from the per-run control socket:

```bash
hyprmux --attach dev        # attach to/start a persistent named session
hyprmux list-sessions       # list connectable named sessions
hyprmux kill-session dev    # request clean shutdown of a named session
```

See [Sessions](sessions.md) for attach/detach semantics.

## Wire protocol

The socket accepts one newline-delimited JSON request per connection and returns one JSON response.

Requests use a `cmd` field: `list-panes`, `focus`, `send-text`, `new-pane`, `run-action`,
`capture-pane`, `switch-workspace`, or `move-to-workspace`. A client may include `source_pane`; the
CLI derives it from `HYPRMUX_PANE`.

Examples:

```json
{"cmd":"list-panes","source_pane":1}
{"cmd":"focus","target":2,"source_pane":1}
{"cmd":"send-text","target":2,"text":"hello"}
{"cmd":"new-pane","command":"cargo test","cwd":"/repo","title":"tests","keep_open":true}
{"cmd":"run-action","action":"toggle-float"}
{"cmd":"capture-pane","target":2}
{"cmd":"switch-workspace","index":2}
{"cmd":"move-to-workspace","index":3}
```

Responses have `ok`, and either `data` or `error`.
