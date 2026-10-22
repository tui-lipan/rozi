# Project status extension

The second of three examples. Where [hello](../hello/) runs once per command, this one runs a
service beside the active session, listens to events, publishes a row, and keeps state between runs.
[agent-board](../agent-board/) comes next.

This extension uses the Python standard library, `git`, and the public `rozi` CLI.

## What it gives you

An Activity row for the project in the focused pane: its folder name and Git branch, and either
`clean` or how many files are uncommitted and for how long, such as
`3 uncommitted files for 2h`. The row follows pane and session switches; activating it focuses
the pane whose project it describes. Outside a Git repository the row disappears.

## Requirements

- rozi with extension API 1 and the `remote-extension-runtime` and `extension-state-dir`
  capabilities, on the client and every host it runs on
- Python 3.10 or newer available as `python3` on the active session's host
- `git` on the active session's host

## Install

```sh
rozi extensions check ./project-status
rozi extensions install --link ./project-status
rozi run-action reload-extensions
```

## How it works

- The `watch` service uses `placement = "active-session"` to run `bin/project_status.py` on the
  session's host, locally or over SSH. Python and Git read the same files as the focused pane.
  Switching sessions restarts the service beside the new session. rozi restarts it if it fails
  and stops it when the extension is reloaded, disabled, or removed.
- The service reads the focused pane from `rozi layout get --format json` on each poll.
  `rozi subscribe focus-changed pane-exited` requests an earlier refresh when focus changes or
  a pane exits; otherwise the service waits five seconds between polls.
  `rozi list-panes --format json` gives the focused pane's directory.
- `rozi publish` takes a snapshot of rows on each line. The service targets the focused pane
  with `ROZI_PANE` and its session instance with `ROZI_SESSION_INSTANCE` from the pane's reference.
  When that identity changes, it closes and reaps the old publisher before opening its replacement.
  Losing focus withdraws the row until a focused pane is available.
- The time each project became dirty is kept in `$ROZI_EXTENSION_STATE_DIR/dirty-since.json` on
  that session's host, so it survives reloads, updates, and reinstalling. Each host keeps its own
  repository state. The service writes a temporary file and renames it, because every process of
  the extension on that host shares the directory. `rozi extensions remove project-status --purge`
  deletes state on the machine where you run it.

## Test

```sh
python3 -m unittest discover -s tests
```

The tests point the state at a temporary directory and never start `rozi`.

Installed extensions are trusted local executable code: they run with your permissions and are
not sandboxed.
