# Docker extension

This canonical extension implements `docker.containers` using only the public extension manifest,
environment, and Rozi CLI. It discovers containers from Docker's line-delimited JSON output and
opens a grouped picker:

- running containers are grouped first and marked active;
- stopped containers remain available for start, inspect, logs, and removal;
- paused, restarting, removing, and dead containers remain visible with a disabled reason;
- `r` refreshes, `s` starts, `x` stops, `Ctrl-r` restarts, `i` inspects, `l` follows logs, and
  `e` opens a container shell;
- `Ctrl-d` removes a stopped container only after Rozi's deliberate second-press confirmation.

Enter opens the selected container's inspection output. Start, stop, restart, remove, and refresh
replace the rows in the existing picker instead of closing and reopening it. Inspect, logs, and
shell open a focused pane; the shell action runs `sh` inside the container.

## Prerequisites

- Rozi with extension API 1;
- Python 3 available as `python`, with no third-party packages;
- the Docker CLI;
- a reachable Docker daemon and permission for the current user to use it.

A missing CLI, unavailable daemon, or permission failure is shown as one concise disabled status
row. Command failures use a concise error notification and keep mutation workflows in the picker.

## Install

Validate and link the example checkout, then reload:

```bash
rozi extensions check ./docker
rozi extensions install --link ./docker
rozi run-action reload-extensions
rozi extensions list --verbose
```

## Manual flow

Start the picker from the command palette or run:

```bash
rozi run-action docker.containers
```

Try `s` or `x` and confirm that the row moves between the Running and Stopped groups without the
picker reopening. Use `i`, `l`, or `e` to open a pane. To remove a stopped container, press
`Ctrl-d`, review the armed row, then press `Ctrl-d` again.

## Pane launch boundary

Docker discovery and every management mutation use structured subprocess argv; container names are
display-only and are never inserted into commands. Inspect, logs, and shell use
`rozi split --argv` so the Python executable, helper path, mode, and validated full hexadecimal
container ID cross into the pane as literal process arguments without shell parsing.
