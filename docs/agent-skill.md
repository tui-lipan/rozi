---
name: hyprmux
description: "Control hyprmux, a Hyprland-style tiling terminal multiplexer for coding agents. Use only when the user explicitly asks to control hyprmux panes or sessions, or asks to use hyprmux. Pane control requires HYPRMUX=1 and a non-empty HYPRMUX_SOCKET."
---

# Hyprmux

Hyprmux runs real terminal panes in a tiling UI and can keep named session servers alive across
clients. Use this skill only for an explicit hyprmux request.

Before any pane-control command, verify that this agent is running in a hyprmux-managed pane with a
local UI endpoint:

```bash
test "${HYPRMUX:-}" = 1 && test -n "${HYPRMUX_SOCKET:-}"
```

If the check fails, say that pane control is unavailable and stop. Do not inspect or control an
arbitrary focused UI from outside a managed pane.

## Learn the current CLI

The installed binary is the authority for syntax. Start with:

```bash
hyprmux --help
```

Do not run bare `hyprmux` for discovery: it launches or attaches the TUI.

## Endpoint and caller context

The control endpoint is private and belongs to the **local hyprmux UI process**. Control endpoint
selection is `--socket PATH`, then `HYPRMUX_SOCKET`, then the only live local endpoint found in the
runtime directory. `HYPRMUX_SOCKET` is the endpoint path, not a named-session server endpoint.

Use the injected endpoint explicitly when needed:

```bash
hyprmux --socket "$HYPRMUX_SOCKET" list-panes
```

Every pane receives `HYPRMUX_PANE=<numeric live pane id>`. The CLI copies that value into
`source_pane` when a command supports source targeting. An omitted target normally means the source
pane, otherwise the UI-focused pane. `focus` requires a numeric id and `capture-pane` accepts
`--target`; the CLI forms of `send-text`, `send-keys`, `split`, and `status` act on the injected
source pane and do not accept a pane-id argument.

`--remote` is **not** a control-socket option. When the UI is attached with `--remote`, its
`HYPRMUX_SOCKET` is still the local UI endpoint and controls the session shown by that UI. Do not
try to point `--socket` at an SSH transport or remote server endpoint. Remote session discovery and
server shutdown use the separate `list-sessions --remote` and `kill-session --remote` helpers below.
Processes inside a remote pane intentionally do not receive that local `HYPRMUX_SOCKET`, so the
initial pane-control check fails there rather than exposing the local UI endpoint remotely.

## Inspect and control panes

Control replies are JSON on stdout, normally `{"ok":true,"data":...}`; server-side failures are
JSON errors, while local discovery/connect failures are plain stderr. Pane ids are numeric. Read
live ids from `list-panes` JSON and reuse those ids; do not predict ids from pane order or examples.

```bash
hyprmux list-panes
hyprmux focus <PANE_ID>
hyprmux send-text 'cargo test
'
hyprmux send-keys C-c
hyprmux send-keys 'echo hi' Enter
hyprmux send-keys -l C-c
hyprmux send-keys -- -n hello
hyprmux split [COMMAND]
hyprmux split [COMMAND] --focus  # also move focus to the new pane
hyprmux new-pane [COMMAND]       # alias of split
hyprmux capture-pane
hyprmux capture-pane --target <PANE_ID>
hyprmux capture-pane --scrollback 200
hyprmux capture-pane --scrollback full
hyprmux capture-pane --last-output
hyprmux status <VALUE> [--reason <TEXT>]
hyprmux status --clear
```

`send-keys` accepts tmux-style names such as `C-c`, `M-x`, `Enter`, `Escape`, `Space`, `Tab`,
`BSpace`, arrows, `Home`/`End`, `PgUp`/`PgDn`, and `F1`..`F12`, mixed with literal text. `-l` or
`--literal` makes every argument literal. `--` ends option parsing.

`capture-pane` defaults to the visible grid. `--scrollback N` captures trailing history,
`--scrollback full` captures all retained history, and `--last-output` captures the last shell
integration command output. `status` reports a short pane status; `status --clear` removes it.

`split`/`new-pane` waits for the pane's PTY and replies with a numeric `id`, `accepted:true`, and
`pty_ready`. `pty_ready:true` means input sent to that id will reach the shell. A slow spawn (a
`--remote` session, say) can still answer `pty_ready:true` late or fall back to `pty_ready:false`
after about five seconds; a `pty_ready:false` pane is starting, not broken. A spawn that fails
answers with a JSON error instead.

`split`/`new-pane` does **not** move focus. The user keeps typing wherever they were, and the new
pane is reachable by id. Pass `--focus` only when the user asked to be taken to the new pane. A
matched `[[rules]]` entry still decides workspace, float, and fullscreen placement.

`send-text`/`send-keys` aimed at a pane whose PTY is still starting are queued as type-ahead and
written once the shell is up, the same as typing into a freshly split pane. Input to a pane that has
exited or failed to spawn still fails with `PTY is not running`.

Other current control commands are `metrics`, `run-action <ACTION_ID>`, `switch-workspace <1-9>`,
and `move-to-workspace <1-9>`. `run-action` uses stable keybinding/command-palette action ids; use
only an id listed by `hyprmux --help` or the command palette, never a guessed id.

## Controller, read-only, and input-lock limits

- A layout controller is required for `split`/`new-pane`, layout-mutating `run-action` calls, and
  moving a pane to another workspace. A writable follower receives `not controller` until it takes
  control; do not repeatedly retry it.
- Read-only clients cannot type, set pane status, or mutate shared layout.
- Input lock blocks typing from writable followers; the controller can still input.
- `focus`, `capture-pane`, and other local view operations do not grant layout control. Avoid
  changing focus unless the user asks for it.

## Named sessions

Session lifecycle commands are separate from the local UI control endpoint:

```bash
hyprmux list-sessions
hyprmux list-sessions --format json
hyprmux attach <NAME>
hyprmux attach <NAME> --read-only
hyprmux new <NAME>
hyprmux new <NAME> --profile <RECIPE>
hyprmux kill-session <NAME>
hyprmux list-sessions --remote <HOST>
hyprmux kill-session <NAME> --remote <HOST>
```

`attach` is attach-only; `new` explicitly creates a named session. `kill-session <NAME>` is the
sole canonical server-stop spelling. It destroys that one per-user named session server and its
PTYs for every attached client. It is not a generic process killer; never use it for an arbitrary
process, pane, or session that the user did not explicitly ask to destroy. Remote lifecycle helpers
run the same command over SSH and never use a local forced-termination fallback against the SSH
transport.

## Safety rules

- Mutate or kill only panes and sessions that the user explicitly requested or this agent created.
- Prefer `HYPRMUX_PANE` or ids read from fresh JSON; never infer a live id from a row position.
- Avoid stealing focus with `focus`, `--focus`, or a layout command when the task does not require
  it.
- Read `pty_ready` from the split response instead of assuming either answer.
- Never treat `kill-session` as a generic process killer.
