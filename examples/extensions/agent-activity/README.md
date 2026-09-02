# Agent activity extension

This extension turns the public status reported by any pane into a stable, actionable row in
Rozi's Activity sidebar. It deliberately knows nothing about a particular coding agent: any
program that runs `rozi status` can participate.

The supervised `agent-activity.watch` service:

- seeds and periodically reconciles state with `rozi list-panes`;
- subscribes to `pane-status-changed`, `focus-changed`, `pane-exited`, and `config-reloaded`;
- opens one `rozi publish` stream for each reporting pane, using that pane's public
  `ROZI_PANE` identity so the row has the correct owner;
- keeps the row id `pane:<id>` stable across status, title, and focus changes;
- focuses the owning pane when Rozi sends a row activation back;
- notifies only on transitions into blocked, finished, or error states, plus a non-zero exit from
  a pane that was reporting activity. Changing a blocked/error reason does not repeat the toast.

`agent-activity.open` opens a searchable command picker. Enter or `f` focuses the selected pane;
`r` refreshes the rows without closing the picker.

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

1. `working` creates an Activity row owned by this pane.
2. `blocked` updates the same row and emits one error-styled notification.
3. Repeating the same blocked state does not notify again.
4. `done` updates the row and emits one finished notification.
5. `--clear` withdraws the row without another notification.

Open another pane and report a status there to verify that each row focuses its own pane:

```bash
rozi status working --reason "run checks"
rozi run-action agent-activity.open
```

The four built-in status names receive Rozi's normal presentation. Other non-empty status strings
are published unchanged; `error`, `errored`, `failed`, and `failure` are additionally treated as
error transitions by this example.

## Reload and lifecycle expectations

Existing server-owned pane statuses are rebuilt from `list-panes` after a service restart; this
extension stores no private state. Closing a publish stream withdraws its row, so a crash cannot
leave stale Activity entries behind.

A presentation-only reload may keep the service generation running, in which case the
`config-reloaded` event triggers an immediate reconciliation. A material extension change rotates
`ROZI_EXTENSION_GENERATION`: Rozi closes the old generation's subscribe/publish streams and rejects
later calls carrying its stale token. The service treats stream closure (or a failed heartbeat
write) as a failure so supervision can start the active generation. Disabling, removing, or
invalidating the extension stops the service and withdraws all rows.

`list-panes` does not currently identify the focused pane. Rows restored solely from the initial
snapshot therefore start with `active = false`; the next focus or pane-status event corrects the
active marker without changing row identity.
