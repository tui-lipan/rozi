---
name: rozi
description: "Control rozi, a Hyprland-style tiling terminal multiplexer for coding agents. Use only when the user explicitly asks to control rozi panes or sessions, or asks to use rozi. Pane control requires ROZI=1 and a non-empty ROZI_SOCKET."
---

# Rozi

Rozi runs real terminal panes in a tiling UI and can keep named session servers alive across
clients. Use this skill only for an explicit rozi request.

Before any pane-control command, verify that this agent is running in a rozi-managed pane with a
local UI endpoint:

```bash
test "${ROZI:-}" = 1 && test -n "${ROZI_SOCKET:-}"
```

If the check fails, say that pane control is unavailable and stop. Do not inspect or control an
arbitrary focused UI from outside a managed pane.

## Learn the current CLI

The installed binary is the authority for syntax. Start with:

```bash
rozi --help
```

Do not run bare `rozi` for discovery: it launches or attaches the TUI.

## Endpoint and caller context

The control endpoint is private and belongs to the **local rozi UI process**. Control endpoint
selection is `--socket PATH`, then `ROZI_SOCKET`, then the only live local endpoint found in the
runtime directory. `ROZI_SOCKET` is the endpoint path, not a named-session server endpoint.

Use the injected endpoint explicitly when needed:

```bash
rozi --socket "$ROZI_SOCKET" list-panes
```

Every pane receives `ROZI_PANE=<numeric live pane id>`. The CLI copies that value into
`source_pane` when a command supports source targeting. An omitted target normally means the source
pane, otherwise the UI-focused pane. `focus` requires a numeric id and `capture-pane` accepts
`--target`; the CLI forms of `send-text`, `send-keys`, `split`, and `status` act on the injected
source pane and do not accept a pane-id argument.

`--remote` is **not** a control-socket option. When the UI is attached with `--remote`, its
`ROZI_SOCKET` is still the local UI endpoint and controls the session shown by that UI. Do not
try to point `--socket` at an SSH transport or remote server endpoint. Remote session discovery and
server shutdown use the separate `list-sessions --remote` and `kill-session --remote` helpers below.
Processes inside a remote pane intentionally do not receive that local `ROZI_SOCKET`, so the
initial pane-control check fails there rather than exposing the local UI endpoint remotely.

## Inspect and control panes

Control replies are JSON on stdout, normally `{"ok":true,"data":...}`; server-side failures are
JSON errors, while local discovery/connect failures are plain stderr. Pane ids are numeric. Read
live ids from `list-panes` JSON and reuse those ids; do not predict ids from pane order or examples.

```bash
rozi list-panes
rozi focus <PANE_ID>
rozi send-text 'cargo test
'
rozi send-keys C-c
rozi send-keys 'echo hi' Enter
rozi send-keys -l C-c
rozi send-keys -- -n hello
rozi split [COMMAND]
rozi split [COMMAND] --focus  # also move focus to the new pane
rozi new-pane [COMMAND]       # alias of split
rozi capture-pane
rozi capture-pane --target <PANE_ID>
rozi capture-pane --scrollback 200
rozi capture-pane --scrollback full
rozi capture-pane --last-output
rozi status <VALUE> [--reason <TEXT>]
rozi status --clear
rozi notify <MESSAGE> [--title <TEXT>] [--level info|error]
rozi pick [--title <TEXT>] [--placeholder <TEXT>]
rozi publish
```

`send-keys` accepts tmux-style names such as `C-c`, `M-x`, `Enter`, `Escape`, `Space`, `Tab`,
`BSpace`, arrows, `Home`/`End`, `PgUp`/`PgDn`, and `F1`..`F12`, mixed with literal text. `-l` or
`--literal` makes every argument literal. `--` ends option parsing.

`capture-pane` defaults to the visible grid. `--scrollback N` captures trailing history,
`--scrollback full` captures all retained history, and `--last-output` captures the last shell
integration command output. `status` reports a short pane status; `status --clear` removes it.
`notify` raises a toast, for a result the user cannot otherwise see - a command that finished with
no pane to print in. Do not use it to announce something already visible on screen.
`pick` streams candidate rows from stdin into a modal search palette and prints the selected item id to stdout upon user choice (or exits 1 if cancelled).
`publish` is for a program running several agents or activities in one pane: it bridges stdin/stdout to
rozi, publishing one JSON row list per line and reading back `{"activate":"<id>"}` when a user
clicks a row. It runs until closed, and closing withdraws the pane's rows. See
`docs/control.md`.

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
only an id listed by `rozi --help` or the command palette, never a guessed id.

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
rozi list-sessions
rozi list-sessions --format json
rozi attach <NAME>
rozi attach <NAME> --read-only
rozi new <NAME>
rozi new <NAME> --profile <RECIPE>
rozi kill-session <NAME>
rozi list-sessions --remote <HOST>
rozi kill-session <NAME> --remote <HOST>
```

`attach` is attach-only; `new` explicitly creates a named session. `kill-session <NAME>` is the
sole canonical server-stop spelling. It destroys that one per-user named session server and its
PTYs for every attached client. It is not a generic process killer; never use it for an arbitrary
process, pane, or session that the user did not explicitly ask to destroy. Remote lifecycle helpers
run the same command over SSH and never use a local forced-termination fallback against the SSH
transport.

## Safety rules

- Mutate or kill only panes and sessions that the user explicitly requested or this agent created.
- Prefer `ROZI_PANE` or ids read from fresh JSON; never infer a live id from a row position.
- Avoid stealing focus with `focus`, `--focus`, or a layout command when the task does not require
  it.
- Read `pty_ready` from the split response instead of assuming either answer.
- Never treat `kill-session` as a generic process killer.
