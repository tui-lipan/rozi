# Hooks

Hooks run client-side commands when a Rozi UI observes an event.

```toml
[[hooks]]
event = "pane-exited"
run = "notify-send 'pane exited'"

[[hooks]]
event = "session-attached"
run = "~/.config/rozi/on-attach.sh"
```

Each entry needs `event` and `run`. Multiple entries may use the same event. Rozi launches matching
commands in config order through [`command_shell`](configuration.md#top-level-keys). Commands run
asynchronously and may overlap.

Unknown event IDs and empty commands produce warnings and are skipped. Config reload applies hook
changes.

## Events and fields

Every hook receives `ROZI_EVENT`. Event fields become uppercase `ROZI_*` variables.

| Event | When it fires | Fields |
| --- | --- | --- |
| `pane-spawned` | A workspace pane is created. | `pane`, `workspace`, `command`, `cwd` |
| `pane-exited` | A pane process exit reaches the client. | `pane`, `code`, `focused` |
| `pane-status-changed` | Server-owned reported status changes or clears. Initial attach seeding does not fire. | `pane`, `status`, `reason`, `previous_status`, `previous_reason`, `focused` |
| `bell` | A pane emits BEL. | `pane`, `focused` |
| `focus-changed` | Focus moves to another workspace pane. | `pane` |
| `workspace-switched` | The active workspace changes. | `workspace` |
| `session-attached` | The client finishes attaching. | `session`, `client_id`, `controller`, `read_only` |
| `session-detached` | The client intentionally leaves or switches sessions. | `session` |
| `session-renamed` | The attached session is renamed. | `session`, `previous` |
| `session-created` | A named session is created from an empty target. | `session` |
| `controller-changed` | Layout control changes or is released. | `controller`, `self_controller`, `reason` |
| `client-joined` | A client joins the attached session roster. | `client_id`, `client_name`, `count` |
| `client-left` | A client leaves the attached session roster. | `client_id`, `client_name`, `count` |
| `profile-loaded` | A profile seeds a newly created session. | `profile`, `path`, `session` |
| `profile-applied` | A profile replaces panes in an existing session. | `profile`, `path`, `session` |
| `profile-saved` | A profile is saved or overwritten. | `profile`, `path` |
| `config-reloaded` | A live config reload finishes without warnings. | `path` |

Field details:

- `workspace` is one-based.
- `command` and `cwd` are empty when inherited.
- `focused`, `read_only`, and `self_controller` are `"true"` or `"false"`.
- `controller` and optional status values are empty when absent.
- `controller-changed.reason` is `released`, `expired`, or `granted`.

The same event names and fields are used by [`rozi subscribe`](control.md#subscriptions). Subscription
objects keep these fields under `data`.

## Environment

Hook commands inherit the client environment and receive:

| Variable | Value |
| --- | --- |
| `ROZI_EVENT` | Event ID. Always present. |
| `ROZI_BIN` | Path to the running Rozi executable when available. |
| `ROZI_SOCKET` | Current UI endpoint when control is available. |
| `ROZI_REMOTE_HOST` | Resolved remote host while this client is remote-attached. |
| `ROZI_<FIELD>` | One variable for each event field. |

Hooks always run on the client machine. Test `ROZI_SOCKET` before calling back because a UI may run
without a control endpoint.

Use the injected executable and endpoint:

```toml
[[hooks]]
event = "pane-exited"
run = '''
if [ -n "${ROZI_SOCKET:-}" ] && [ -n "${ROZI_BIN:-}" ]; then
    "$ROZI_BIN" --socket "$ROZI_SOCKET" switch-workspace 1
fi
'''
```

## Lifecycle

Rozi discards hook stdin, stdout, stderr, and exit status. Redirect output in the command if it
matters. Rozi does not wait, retry, or supervise hook processes. Use a
[`[[services]]`](configuration.md#services) entry with `rozi subscribe` when automation needs
state, retries, or long-lived event handling.

Hooks belong to UI clients, not session servers. Each attached client loads its own hooks. Most
shared-session events can therefore launch equivalent hooks on several clients.
`pane-status-changed` is the exception: every client publishes it to local subscribers, but only the
layout controller runs matching hooks.

No hooks run while all clients are detached. A client crash cannot run a final
`session-detached` hook.

## Migrating from `[hooks]`

The old flat table is not supported and makes the config fail to load:

```toml
# Old
[hooks]
pane-exited = "notify-send 'pane exited'"
```

Convert each value to an array entry:

```toml
[[hooks]]
event = "pane-exited"
run = "notify-send 'pane exited'"
```
