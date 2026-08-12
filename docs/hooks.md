# Hooks

Hooks run shell commands when a hyprmux UI client observes an event. They are useful for desktop
notifications, audit logs, and automation that calls back into the live UI through its control
socket.

## Configuration

Each hook is a structured `[[hooks]]` entry with an event id and a command:

```toml
[[hooks]]
event = "pane-exited"
run = "notify-send 'pane exited'"

[[hooks]]
event = "session-attached"
run = "~/.config/hyprmux/on-attach.sh"
```

The former flat `[hooks]` table is no longer supported. See
[Migrating from the old table](#migrating-from-hooks) below.

Commands run through the configured [`command_shell`](configuration.md#top-level-keys). An unknown
event id or an empty `run` value is ignored with a config warning. Hook changes are applied by the
normal config hot reload.

## Events and fields

Every event sets `ROZI_EVENT`. Event fields are also available as environment variables using
an uppercase `ROZI_` prefix, for example the `client_id` field becomes
`ROZI_CLIENT_ID`.

| Event | When it fires | Fields |
| --- | --- | --- |
| `pane-spawned` | A workspace pane is created. | `pane`: pane id; `workspace`: 1-based workspace; `command`: configured command, or empty for a shell pane; `cwd`: configured working directory, or empty when inherited. |
| `pane-exited` | A pane process exit is reported to the client. | `pane`: pane id; `code`: process exit code; `focused`: `true` or `false`. |
| `pane-status-changed` | A pane's server-owned reported status changes or is cleared. Initial attach seeding does not fire an event. | `pane`: pane id; `status`: new status; `reason`: new reason; `previous_status`: prior status; `previous_reason`: prior reason; `focused`: `true` or `false`. Missing or cleared values are empty strings. |
| `bell` | A pane emits BEL. | `pane`: pane id; `focused`: `true` or `false`. |
| `focus-changed` | Focus moves to another workspace pane. | `pane`: newly focused pane id. |
| `workspace-switched` | The active workspace changes, including a move or relocation that switches workspaces. | `workspace`: new 1-based workspace. |
| `session-attached` | This client finishes attaching to an ephemeral or named session. | `session`: session name; `client_id`: this client's numeric id; `controller`: controller client id, or empty if none; `read_only`: `true` or `false`. |
| `session-detached` | This client intentionally leaves or switches away from its current session. | `session`: session name. |
| `session-renamed` | The attached session is renamed. | `session`: new name; `previous`: previous name. |
| `session-created` | A named session is created from an empty target. | `session`: session name. |
| `controller-changed` | The shared session's layout controller changes or is released. | `controller`: new controller client id, or empty if none; `self_controller`: `true` when this client is now controller; `reason`: `released`, `expired`, or `granted`. |
| `client-joined` | A client appears in the attached session roster. | `client_id`: joined client id; `client_name`: its display label; `count`: roster size after the change. |
| `client-left` | A client disappears from the attached session roster. | `client_id`: departed client id; `client_name`: its last display label; `count`: roster size after the change. |
| `profile-loaded` | A profile successfully supplies the launch seed for a newly created session. It does not fire for in-place replacement. | `profile`: profile name; `path`: profile file path; `session`: created destination session. |
| `profile-applied` | A profile successfully replaces the panes of an existing session in place. | `profile`: profile name; `path`: profile file path; `session`: destination session. |
| `profile-saved` | A profile is saved or overwritten. | `profile`: profile name; `path`: profile file path. |
| `config-reloaded` | A live config reload completes without warnings. | `path`: config file path. |

The same event ids and fields are used by control-socket
[`subscribe`](control.md#wire-protocol) clients.

## Environment contract

Hook commands inherit the client process environment and receive these additional variables:

| Variable | Value |
| --- | --- |
| `ROZI_EVENT` | The event id from the table above. Always set. |
| `ROZI_SOCKET` | The current UI control endpoint path. Set only when the client successfully created a control endpoint. On Windows this is the discovery-entry path accepted by `hyprmux --socket`, not a raw named-pipe name. |
| `ROZI_REMOTE_HOST` | Set when the UI is attached via `--remote`; the resolved remote host string. Hooks still run on the **client** machine. |
| `ROZI_<FIELD>` | One variable for each field listed for the event. Field names are uppercased; underscores are retained. |

All injected values are strings. Optional event values are represented by an empty string rather
than an omitted field. Test `ROZI_SOCKET` before calling back into the UI because control endpoint
creation can fail without preventing hyprmux from starting.

## Multiple hooks and command lifecycle

Several `[[hooks]]` entries may use the same event. Every matching entry is launched, in config
order, as a separate command. Commands are asynchronous and may overlap; completion order is not
guaranteed.

Hook execution is detached: hyprmux does not wait for completion, retry failed commands, or manage
the child after launch. Hook stdout, stderr, and exit status are discarded by the hook API; redirect
output explicitly if it matters. A hook command cannot block the UI event loop, and a later hook
does not wait for an earlier one.

## Client-side semantics

Hooks belong to the UI client, not the background session server. Each attached client uses its own
loaded config and launches hooks for events it observes. Consequently, one shared-session change can
run equivalent hooks on several attached clients. Pane status is the deliberate exception: every
client publishes `pane-status-changed` to its local control-socket subscribers, but only the current
layout controller launches matching hooks. This prevents one agent status update from starting the
same side effect on every attached client. Once all clients are detached, the server may continue
owning PTYs, but it does not load client hooks or launch hook commands.

Server-only transitions such as server startup and resurrection snapshot writes are not hook
events. An abrupt client crash also cannot run a final `session-detached` hook. Hook execution is
best-effort automation, not a durable server-side job system.

## Examples

Run two independent commands for the same event:

```toml
[[hooks]]
event = "pane-exited"
run = "notify-send \"pane $ROZI_PANE exited with $ROZI_CODE\""

[[hooks]]
event = "pane-exited"
run = '''printf '%s pane=%s code=%s\n' "$ROZI_EVENT" "$ROZI_PANE" "$ROZI_CODE" >> "$HOME/.local/state/hyprmux-events.log"'''
```

Use the injected endpoint to talk back to the client that emitted the event. This POSIX-shell
example switches to workspace 1 after a pane exits:

```toml
[[hooks]]
event = "pane-exited"
run = '''
if [ -n "$ROZI_SOCKET" ]; then
    hyprmux --socket "$ROZI_SOCKET" switch-workspace 1
fi
'''
```

The explicit `--socket` is optional because the CLI also discovers the endpoint from
`ROZI_SOCKET`, but spelling it out makes the callback target clear. Redirect any command output
inside the hook if it must be retained.

## Migrating from `[hooks]`

This is a breaking config change. Convert each key/value pair in the old flat table into one array
entry:

```toml
# Old syntax: no longer supported
[hooks]
pane-exited = "notify-send 'pane exited'"
workspace-switched = "logger workspace=$ROZI_WORKSPACE"
```

```toml
# New syntax
[[hooks]]
event = "pane-exited"
run = "notify-send 'pane exited'"

[[hooks]]
event = "workspace-switched"
run = "logger workspace=$ROZI_WORKSPACE"
```

A config that still contains `[hooks]` fails to load and reports a migration warning pointing to
`[[hooks]]`. The structured form permits multiple commands for one event and leaves room for future
per-hook options without another table-shape migration.
