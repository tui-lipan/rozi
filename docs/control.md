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
```

Replies are JSON on stdout. Errors are JSON when returned by the server, or plain stderr for client
discovery/connect failures. `split`/`new-pane` replies as soon as the pane is accepted by the UI; the
PTY may still be starting (`pty_ready: false`). Named `send-keys` compatibility is not implemented;
`send-keys` currently aliases literal text.

## Wire protocol

The socket accepts one newline-delimited JSON request per connection and returns one JSON response.

Requests use a `cmd` field: `list-panes`, `focus`, `send-text`, or `new-pane`. A client may include
`source_pane`; the CLI derives it from `HYPRMUX_PANE`.

Examples:

```json
{"cmd":"list-panes","source_pane":1}
{"cmd":"focus","target":2,"source_pane":1}
{"cmd":"send-text","target":2,"text":"hello"}
{"cmd":"new-pane","command":"cargo test","cwd":"/repo","title":"tests","keep_open":true}
```

Responses have `ok`, and either `data` or `error`.
