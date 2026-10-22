# Agent board extension

The third of three examples, after [hello](../hello/) and [project-status](../project-status/). It
teaches Rozi to recognize a coding agent, lists the agents of every attached session in a sidebar
tab, and runs beside each session, on that session's host.

This extension uses the Python standard library and the public `rozi` CLI.

## What it gives you

- An **Agents** sidebar tab with one row per agent pane, showing what each one is doing. Click a row,
  or press `Enter` on it, to focus that pane. With several sessions attached, each session's agents
  appear under its own heading.
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
- The `board` service has `placement = "each-session"`, so Rozi starts one per attached session,
  on the session's host: locally, or over SSH for a session attached with `--remote`. It runs from
  a verified copy of this directory, so nothing has to be installed on that host except `rozi`.
- The service polls `rozi list-panes --format json`, writes rows to `rozi sidebar publish --tab
  agents`, and answers `{"activate": …}` with `rozi focus`. It also writes one row to
  `rozi publish` for Activity.
- A placed process sees panes only while its own session is on screen. While you look at another
  session, `list-panes` returns nothing, so the board keeps showing the rows it last had.

## Test

```sh
python -m unittest discover -s tests
```

The tests check the rows and the Activity summary, and that the manifest's state rules match what
the demo agent prints.

Installed extensions are trusted local executable code: they run with your permissions and are
not sandboxed.
