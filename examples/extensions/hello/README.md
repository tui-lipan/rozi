# Hello extension

The first of three examples that build on each other: a command, then an optional picker. Read
[project-status](../project-status/) next for a service that keeps state, and
[agent-board](../agent-board/) after that for agents, a sidebar tab, and running on the session's
host.

This extension uses the Python standard library and the public `rozi` CLI.

## What it gives you

- `hello.greet` shows a "Hello, world!" notification.
- `hello.pick` asks whom to greet in a picker, then greets them. Cancelling does nothing.

## Requirements

- Rozi with extension API 1
- Python 3.10 or newer available as `python`

## Install

```sh
rozi extensions check ./hello
rozi extensions install --link ./hello
rozi run-action reload-extensions
rozi run-action hello.greet
```

## How it works

`extension.toml` declares two commands that run the same script with structured `exec`.
`{extension_dir}` names the extension's own directory, because a command runs in the focused
pane's directory.

`bin/hello.py` reads `ROZI_BIN` to call the running `rozi`. With `--pick`, it writes one JSON line
to `rozi pick --json` and reads `{"selected": …}` back.

## Test

```sh
python -m unittest discover -s tests
```

The tests replace `rozi` with a recorder, so they need no running Rozi.

Installed extensions are trusted local executable code: they run with your permissions and are
not sandboxed.
