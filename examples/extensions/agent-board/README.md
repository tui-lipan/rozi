# Agent board extension

The third of three examples, after [hello](../hello/) and [project-status](../project-status/). It
teaches Rozi to recognize a coding agent, lists the active session's agents in a sidebar tab, and
runs on that session's host.

This extension uses the Python standard library and the public `rozi` CLI.

## What it gives you

- An **Agents** sidebar tab with one row per agent pane, showing what each one is doing. Click a row,
  or press `Enter` on it, to focus that pane. The rows belong to the session on screen and appear
  under its heading. Switching sessions replaces them with that session's agents.
- An Activity row that sums them up, such as `1 waiting for you, 2 working, 3 in all`.
- A demo agent, `bin/demo_agent.py`, and the `[[agents]]` definition that recognizes it. Run it in
  a pane to see the board react:

  ```sh
  python3 path/to/agent-board/bin/demo_agent.py
  ```

## Requirements

- Rozi with extension API 1 and the `remote-extension-runtime` and `extension-state-dir`
  capabilities, on the client and on every host it runs on
- Python 3.10 or newer available as `python3` on each session's host

## Install

```sh
rozi extensions check ./agent-board
rozi extensions install --link ./agent-board
rozi run-action reload-extensions
```

Open **Sidebar tabs…** and show **Agents**.

## How it works

- `[[agents]]` matches any process with `demo_agent.py` among its arguments. Its two state rules
  read the demo agent's screen: "esc to interrupt" in the footer means working, and its question
  means blocked. Rozi's common rules handle the rest.
- `[[sidebar_tabs]]` with `stream = true` declares a tab whose rows come from the extension's own
  processes.
- The `board` service has `placement = "active-session"`, so Rozi runs it on the active session's
  host: locally, or over SSH for a session attached with `--remote`. Switching sessions stops the
  old board and starts one for the session on screen. It runs from
  a verified copy of this directory, so nothing has to be installed on that host except `rozi`.
- The service polls `rozi list-panes --format json`, writes rows to `rozi sidebar publish --tab
  agents`, and answers `{"activate": …}` with `rozi focus`. It also writes one row to
  `rozi publish` for Activity.
- The board shows only the session on screen. It does not collect or retain background-session
  rows. A detected blocked screen overrides an idle or done report, matching Rozi's agent state.

## Test

```sh
python -m unittest discover -s tests
```

The tests check the rows and the Activity summary, and that the manifest's state rules match what
the demo agent prints.

Installed extensions are trusted local executable code: they run with your permissions and are
not sandboxed.
