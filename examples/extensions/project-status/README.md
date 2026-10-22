# Project status extension

The second of three examples. Where [hello](../hello/) runs once per command, this one runs a
service the whole time Rozi does, listens to events, publishes a row, and keeps state between runs.
[agent-board](../agent-board/) comes next.

This extension uses the Python standard library, `git`, and the public `rozi` CLI.

## What it gives you

An Activity row for the project in the focused pane: its folder name and Git branch, and either
`clean` or how many files are uncommitted and for how long, such as
`3 uncommitted files for 2h`. Outside a Git repository the row disappears.

## Requirements

- Rozi with extension API 1 and the `extension-state-dir` capability
- Python 3.10 or newer available as `python`
- `git`

## Install

```sh
rozi extensions check ./project-status
rozi extensions install --link ./project-status
rozi run-action reload-extensions
```

## How it works

- `[[services]]` starts `bin/project_status.py` when the extension loads and restarts it if it
  fails. Rozi stops it when the extension is reloaded, disabled, or removed.
- The service opens `rozi subscribe focus-changed pane-exited`, then reads the initial focused
  pane from `rozi layout get --format json`. Focus events keep it up to date, and
  `rozi list-panes --format json` gives that pane's directory.
- `rozi publish` takes a snapshot of rows on each line. Rozi attaches a service's rows to the pane
  that was focused when the stream opened.
- The time each project became dirty is kept in `$ROZI_EXTENSION_STATE_DIR/dirty-since.json`, so it
  survives reloads, updates, and reinstalling. The service writes a temporary file and renames
  it, because every process of the extension on this machine shares the directory. Delete the
  state with `rozi extensions remove project-status --purge`.

## Test

```sh
python -m unittest discover -s tests
```

The tests point the state at a temporary directory and never start `rozi`.

Installed extensions are trusted local executable code: they run with your permissions and are
not sandboxed.
