# Scripting

rozi can be driven from a shell script: open panes, type into them, read what they print, ask the
user to pick something, and report progress. This page walks through the most common tasks. The
[Control CLI](control.md) reference lists every command and option.

## Before you start

Every task on this page uses the `rozi` command. It finds the right rozi for you, on every
platform, so a script never needs to open a socket or pipe itself.

Inside a rozi pane — and in hooks, services, and extension processes — rozi sets a few environment
variables:

| Variable | Meaning |
| --- | --- |
| `ROZI_BIN` | The path of the running `rozi` executable. |
| `ROZI_SOCKET` | How to reach the rozi window this pane belongs to. The CLI reads it for you. |
| `ROZI_PANE` | The id of the pane the script runs in, when there is one. |

Prefer `ROZI_BIN` so the script uses the same version of rozi that is running:

```sh
ROZI_CMD=${ROZI_BIN:-rozi}
"$ROZI_CMD" list-panes --format json
```

Outside rozi, the CLI uses the only running rozi window. If several are running, choose one with
`--socket PATH`. To drive a session without any window open, see
[Drive a session with no window](#drive-a-session-with-no-window).

Commands print readable tables in a terminal and JSON when piped, so `jq` works on their output.
Pass `--format json` to be explicit.

## Send a command to a pane

```sh
ROZI_CMD=${ROZI_BIN:-rozi}
pane=$("$ROZI_CMD" list-panes --format json | jq -r '.data[0].id')
"$ROZI_CMD" send-text --target "$pane" 'cargo test'
"$ROZI_CMD" send-keys --target "$pane" Enter
```

`send-text` types literal text. `send-keys` sends named keys such as `Enter`, `C-c`, and `F2`, and
can mix them with text: `send-keys --target "$pane" 'cargo test' Enter`.

## Open a pane that runs a program

```sh
ROZI_CMD=${ROZI_BIN:-rozi}
"$ROZI_CMD" split --workspace 9 --title Tests --focus --argv cargo test -- --nocapture
```

Put pane options such as `--title` and `--workspace` before `--argv`. Everything after `--argv` is
the program and its arguments, passed as-is without a shell. To run a shell command line instead,
pass it as one argument: `split 'cargo test | tee log'`.

The reply includes the new pane's id, so a script can keep driving it:

```sh
pane=$("$ROZI_CMD" split --argv bash | jq -r '.data.id')
```

## Read a pane's output

```sh
ROZI_CMD=${ROZI_BIN:-rozi}
"$ROZI_CMD" capture-pane --target 3 --scrollback full --format text
```

Without `--scrollback`, you get only what is visible. To keep colors and layout, capture an image
or ANSI text of the visible screen:

```sh
"$ROZI_CMD" capture-pane --target 3 --render png --output pane.png
"$ROZI_CMD" capture-pane --target 3 --render ansi --format text | less -R
```

`rozi capture-ui --render png --output ui.png` captures the whole window as drawn: the bar,
borders, overlays, and every visible pane. See
[Sending keys and capturing output](control.md#sending-keys-and-capturing-output) and
[Capturing the whole UI](control.md#capturing-the-whole-ui).

## Ask the user to pick something

```sh
ROZI_CMD=${ROZI_BIN:-rozi}
branch=$(git branch --format='%(refname:short)' | "$ROZI_CMD" pick --title Branch) || exit 0
[ -n "$branch" ] || exit 0
git switch -- "$branch"
```

`pick` shows each input line in a picker and prints the chosen one. If the user presses `Esc`, it
exits with status `1`, which this script treats as a normal stop. For richer rows, see
[Pickers](control.md#pickers).

## Report progress

Mark the pane as working, done, or blocked while a job runs. rozi shows the state on the pane and
in the sidebar.

```sh
ROZI_CMD=${ROZI_BIN:-rozi}
"$ROZI_CMD" status working --reason "running tests"
if cargo test; then
  "$ROZI_CMD" status done --reason "tests passed"
else
  "$ROZI_CMD" status blocked --reason "tests failed"
  "$ROZI_CMD" notify "tests failed" --title Tests --level error
fi
```

`notify` shows a toast, which is useful when the pane is off screen. Clear the status with
`"${ROZI_BIN:-rozi}" status --clear`.

## React to events

```sh
ROZI_CMD=${ROZI_BIN:-rozi}
"$ROZI_CMD" subscribe pane-exited pane-status-changed |
  jq -r 'select(.event == "pane-exited") | "pane \(.data.pane) exited \(.data.code)"'
```

`subscribe` prints one JSON object per event until rozi exits. Event fields are under `data`; see
[Hooks](hooks.md#events-and-fields) for every event. To run a command on an event without keeping a
script alive, use a [hook](hooks.md) instead.

## Drive a session with no window

`--session <NAME>` sends a command straight to a named session, even when no rozi window is
attached to it. A cron job or an SSH login can inspect and drive a detached session:

```sh
rozi --session dev list-panes
rozi --session dev send-keys --target 3 'cargo test' Enter
rozi --session dev capture-pane --target 3 --scrollback full --format text
```

With `--session`, always name the pane with `--target`; the script's own `ROZI_PANE` belongs to a
different session. A pane addressing its own session passes `--target "$ROZI_PANE"`. Commands that
need a screen — `focus`, `run-action`, `notify`, `pick`, `subscribe`, and switching workspaces —
say so instead of running. Add `--remote <HOST>` to reach a session on another machine. See
[Two endpoints](control.md#two-endpoints).

## Other things you can script

- Run any command-palette action: `rozi run-action toggle-sidebar`. Configured `[[commands]]` IDs
  and extension command IDs work too.
- Read where every pane sits, or rearrange them, with `rozi layout get` and `rozi pane set`; see
  [Layout](control.md#layout).
- Wait for a coding agent to finish, or prompt it safely; see
  [Inspect and wait for agents](agents.md#inspect-and-wait-for-agents).

## Next steps

- [Automation recipes](recipes.md) — longer, complete scripts.
- [Hooks](hooks.md) — run commands when something happens.
- [Control CLI](control.md) — every command, option, and error.
- [Control protocol](control-protocol.md) — for a client that cannot run the `rozi` command.
