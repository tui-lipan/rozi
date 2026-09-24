# Hooks

Hooks run a shell command when a rozi UI observes an event, such as a pane exiting or a session
attaching. Use them for small, fire-and-forget reactions; use a
[service](configuration.md#services) with `rozi subscribe` when automation needs state or retries.

## Add a hook

Add a `[[hooks]]` entry to your config for each command:

```toml
[[hooks]]
event = "pane-exited"
run = "notify-send \"pane $ROZI_PANE exited with code $ROZI_CODE\""

[[hooks]]
event = "session-attached"
run = "~/.config/rozi/on-attach.sh"
```

Each entry needs `event`, one of the [event IDs](#events-and-fields) below, and `run`, a nonempty
command string. rozi runs `run` through [`command_shell`](configuration.md#top-level-keys) with the
event's fields in the [environment](#environment).

- Several entries may use the same event. rozi launches them in config order.
- Hook commands run asynchronously and may overlap.
- At most 32 hook and detached `exec` jobs run at once. Further launches are skipped until a slot
  frees.
- An unknown event ID or an empty command produces a config warning, and that entry is skipped.
- Reloading the config applies hook changes.

## Events and fields

| Event | When it fires | Fields |
| --- | --- | --- |
| `pane-spawned` | A workspace pane is created. | `pane`, `workspace`, `command`, `cwd` |
| `pane-exited` | A pane's process exit reaches the client. | `pane`, `code`, `focused` |
| `pane-status-changed` | A pane's reported status changes or clears. Seeding status on attach does not fire it. | `pane`, `status`, `reason`, `previous_status`, `previous_reason`, `focused` |
| `bell` | A pane rings the terminal bell. | `pane`, `focused` |
| `focus-changed` | Focus moves to another workspace pane. | `pane` |
| `workspace-switched` | The active workspace changes. | `workspace` |
| `layout-changed` | The attached session's layout reaches a new accepted revision. | `revision`, `author` |
| `session-attached` | The client finishes attaching. | `session`, `client_id`, `controller`, `read_only` |
| `session-detached` | The client intentionally leaves or switches sessions. | `session` |
| `session-renamed` | The attached session is renamed. | `session`, `previous` |
| `session-created` | A named session is created from an empty target. | `session` |
| `controller-changed` | Layout control changes or is released. | `controller`, `self_controller`, `reason` |
| `client-joined` | A client joins the attached session's roster. | `client_id`, `client_name`, `count` |
| `client-left` | A client leaves the attached session's roster. | `client_id`, `client_name`, `count` |
| `profile-loaded` | A profile seeds a newly created session. | `profile`, `path`, `session` |
| `profile-applied` | A profile replaces the panes in an existing session. | `profile`, `path`, `session` |
| `profile-saved` | A profile is saved or overwritten. | `profile`, `path` |
| `config-reloaded` | A live config reload applies a usable document. It fires even with field-level warnings, but not for a rejected document. | `path` |

Every field is a string:

- `workspace` is one-based.
- `command` and `cwd` are empty when the pane inherited them.
- `focused`, `read_only`, and `self_controller` are `"true"` or `"false"`.
- `controller` and the status fields are empty when there is no value.
- `layout-changed.author` is `self`, `client`, or `server`.
- `controller-changed.reason` is `released`, `expired`, or `granted`.

[`rozi subscribe`](control.md#subscriptions) uses the same event names and fields, nested under
`data` in each event object.

## Environment

Hook commands inherit the client's environment and also receive:

| Variable | Value |
| --- | --- |
| `ROZI_EVENT` | Event ID. Always present. |
| `ROZI_<FIELD>` | One variable per event field, with the field name in uppercase: `pane` becomes `ROZI_PANE`. |
| `ROZI_BIN` | Path to the running `rozi` executable, when available. |
| `ROZI_SOCKET` | The UI's control endpoint, when control is available. |
| `ROZI_REMOTE_HOST` | The resolved remote host, while this client is attached to a remote session. |

To call back into rozi from a hook, use `ROZI_BIN` and `ROZI_SOCKET`. Check both first, because a
UI can run without a control endpoint:

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

Hooks run on the client machine, even when the session lives on a remote host.

rozi discards a hook's stdin, stdout, stderr, and exit status; redirect output in the command if
you need it. rozi does not wait for, retry, or supervise hook processes, and a burst of events can
skip hooks once 32 jobs are already running. For state, retries, or long-lived event handling, run
`rozi subscribe` from a [`[[services]]`](configuration.md#services) entry instead.

### Shared sessions

Hooks belong to UI clients, not to session servers. Each attached client loads its own hooks, so
most events in a [shared session](shared-sessions.md) run matching hooks on every client.
`pane-status-changed` is the exception: every client delivers it to its own subscribers, but only
the client holding layout control runs its hooks.

No hooks run while every client is detached, and a client that crashes cannot run a final
`session-detached` hook.

## Migrate from `[hooks]`

The old flat `[hooks]` table is no longer supported, and a config that uses it fails to load:

```toml
# Old
[hooks]
pane-exited = "notify-send 'pane exited'"
```

Convert each value to a `[[hooks]]` entry:

```toml
[[hooks]]
event = "pane-exited"
run = "notify-send 'pane exited'"
```
