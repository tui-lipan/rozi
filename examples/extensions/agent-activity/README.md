# Agent activity extension

This extension builds on the public status any pane reports with `rozi status`. It deliberately
knows nothing about a particular coding agent: any program that runs `rozi status` can participate.
rozi already lists each reported status in its Activity sidebar, so this extension adds what the
sidebar does not.

The supervised `agent-activity.watch` service:

- seeds and periodically reconciles state with `rozi list-panes`;
- subscribes to `pane-status-changed`, `pane-exited`, and `config-reloaded`;
- notifies only on transitions into blocked, finished, or error states, plus a non-zero exit from
  a pane that was reporting activity. Changing a blocked/error reason does not repeat the toast.

`agent-activity.open` opens a searchable command picker of every pane reporting a status. Enter or
`f` focuses the selected pane; `r` refreshes the rows without closing the picker.

A status reported with `rozi status` is an activity, not an agent: it never appears in rozi's
Agents view or `rozi agents`, and this extension does not change that.

## Install

Python 3 available as `python` is the only runtime dependency. Validate and link the example
checkout, then reload Rozi explicitly:

```bash
rozi extensions check ./agent-activity
rozi extensions install --link ./agent-activity
rozi run-action reload-extensions
rozi extensions list --verbose
```

Rozi does not watch extension source trees. After editing the linked checkout, run
`rozi run-action reload-extensions` again.

## Manual simulation

Run these commands inside any live Rozi pane. They are the same public contract a coding agent,
build runner, or other long-lived tool can use:

```bash
rozi status working --reason "implement extension protocol"
rozi status blocked --reason "approval required"
rozi status blocked --reason "approval required" # no duplicate blocked toast
rozi status done --reason "extension protocol ready"
rozi status --clear
```

Expected results:

1. `blocked` emits one error-styled notification.
2. Repeating the same blocked state does not notify again.
3. `done` emits one finished notification.
4. `--clear` emits no notification.

rozi's own Activity row for the pane follows each status; the extension adds no second row.

Open another pane and report a status there to verify that each picker row focuses its own pane:

```bash
rozi status working --reason "run checks"
rozi run-action agent-activity.open
```

The four built-in status names receive Rozi's normal presentation. Other non-empty status strings
are shown unchanged; `error`, `errored`, `failed`, and `failure` are additionally treated as
error transitions by this example.

## Reload and lifecycle expectations

Existing server-owned pane statuses are rebuilt from `list-panes` after a service restart; this
extension stores no private state.

A presentation-only reload may keep the service generation running, in which case the
`config-reloaded` event triggers an immediate reconciliation. A material extension change rotates
`ROZI_EXTENSION_GENERATION`: Rozi closes the old generation's subscribe stream and rejects later
calls carrying its stale token. The service treats stream closure as a failure so supervision can
start the active generation. Disabling, removing, or invalidating the extension stops the service.
