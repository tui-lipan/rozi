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
| `agent-state-changed` | An agent's state changes, including when it appears in a pane and when it goes. Seeding agents on attach does not fire it. | `pane`, `agent`, `label`, `ref`, `state`, `previous_state`, `reason`, `focused` |
| `foreground-changed` | The program running in a pane changes, including when the pane returns to its shell prompt. | `pane`, `program`, `previous_program`, `pid`, `focused` |
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
- `agent-state-changed` describes each agent as [`rozi agents list`](agents.md#inspect-and-wait-for-agents)
  does. `state` and `previous_state` are `working`, `blocked`, `idle`, `done`, or `error`;
  `previous_state` is empty when the agent appears, and `state` is empty when it goes. `agent` is
  the agent's ID, such as `claude`, and `label` its name. `reason` is why the agent is in its
  state, when it says. `ref` is the agent's exact reference as JSON: pass it to
  `rozi agents get --ref` or `agents wait --ref`. An agent keeps its `ref` across every turn it
  works through and finishes. An agent replaced in the same pane, such as one relaunched, has a
  new `ref`, so the replacement reads as one agent going and another appearing.
- `foreground-changed.program` is the executable name of the program the pane runs, such as
  `cargo`, and is empty when the pane is at its shell prompt. The pane's shell is the one it was
  started with, or a common shell such as `bash` or `zsh`, on the machine running the session.
  `pid` is that program's process group on the same machine, and is empty when nothing runs or the
  platform cannot read it. The same program restarted in a new process group fires again. rozi
  samples a pane's foreground, so a command that starts and finishes between two samples may not
  fire.
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
skip hooks once 32 jobs are already running. For state, retries, or long-lived event handling, run `rozi subscribe` or `rozi --session <NAME> subscribe` from a [`[[services]]`](configuration.md#services) entry instead.

### Shared sessions

Hooks belong to UI clients, not to session servers. Each attached client loads its own hooks.
Session-event hooks (`pane-spawned`, `pane-exited`, `pane-status-changed`, `agent-state-changed`,
`foreground-changed`, `bell`, `layout-changed`, `client-joined`, `client-left`, `controller-changed`,
`session-renamed`) run only on the controller client holding layout control, while client-local hooks
(`focus-changed`, `workspace-switched`, `session-attached`, `session-detached`) run on the client
where the event occurred.

For automation that must receive session events even when no UI is attached, subscribe directly
to the session endpoint with `rozi --session <NAME> subscribe` (see [Session subscriptions](control.md#session-subscriptions)).

No hooks run while every client is detached, and a client that crashes cannot run a final
`session-detached` hook.
