---
name: rozi
description: "Inspect and control Rozi terminal panes and sessions. Use only when the user explicitly asks to use Rozi. UI pane control requires ROZI=1 and a non-empty ROZI_SOCKET."
---

# Rozi

Rozi is a terminal multiplexer with a CLI for controlling a running UI or a detached named session.
Use this skill only when the user explicitly asks to use Rozi or control a Rozi pane or session.

## Choose the endpoint

- A **UI endpoint** controls the session shown by a running Rozi, including focus and overlays.
  Commands use `--socket PATH`, then `ROZI_SOCKET`, then the only live local UI endpoint.
- A **session endpoint** controls an existing local named session without opening or attaching a UI.
  Select it with `--session <NAME>`.

Before using a UI endpoint, confirm this agent is inside one of its local panes:

```bash
test "${ROZI:-}" = 1 && test -n "${ROZI_SOCKET:-}"
```

If that fails, UI pane control is unavailable. Never control an arbitrary focused UI from outside
one of its panes. This check is not required when the user explicitly names a detached session.

The installed binary defines current syntax. Check it when unsure:

```bash
rozi --help
```

Do not run bare `rozi` for discovery. It launches or attaches the TUI. Likewise, `rozi dev` launches
a UI; `rozi --session dev <COMMAND>` controls the `dev` session server.

## Inspect, then act

Always read live pane ids. Never infer an id from pane order, a title, or an example.

```bash
# Session shown by this UI
rozi list-panes --format json
rozi layout get --format json
rozi capture-pane --target <PANE_ID> --format json

# Detached named session
rozi --session dev list-panes --format json
rozi --session dev layout get --format json
rozi --session dev capture-pane --target <PANE_ID> --format json
```

`layout get` shows each workspace's layout and where every pane sits. `rect` is the pane's
position on the session's shared canvas, in cells. Every endpoint reports the same `rect`. From a
UI, `client` gives the focused pane and the workspace it shows, and `view_rect` gives where that UI
draws each pane. Use these rects to work out which pane is left of or above another. Do not guess
from pane ids or titles.

Change the layout only when the user asks. Every write names what it changes and never moves focus:

```bash
rozi layout set --workspace 2 master
rozi pane set --target <PANE_ID> --floating true --rect 10,5,80,24
rozi pane set --target <PANE_ID> --fullscreen false --if-revision <REVISION>
rozi pane move --target <PANE_ID> --workspace 3
rozi pane swap --target <PANE_ID> --with <PANE_ID>
rozi pane set --target <PANE_ID> --split-ratio 0.6      # Dwindle
rozi pane set --target <PANE_ID> --width-ratio 0.5      # Scrollable
rozi layout set --workspace 2 master --master-ratio 0.6
```

Each ratio works only with the layout that uses it; anything else fails with `unsupported`.

`pane close --target <PANE_ID>` ends the pane's process without asking. Run it only for a pane the
user asked you to close, or one you opened yourself.

Writes set a state; repeating one returns `changed:false`. Pass the `revision` from `layout get`
as `--if-revision` so a layout someone changed meanwhile fails with `conflict` instead of being
overwritten, then read the layout again. `not-controller` means another client is arranging the
session: leave it alone.

Inspect a pane before sending input unless the user gave an exact live id and exact input. Target
other panes explicitly so focus never decides where input goes.

```bash
rozi send-text --target <PANE_ID> 'literal text'
rozi send-keys --target <PANE_ID> Enter
rozi send-keys --target <PANE_ID> C-c
rozi send-keys --target <PANE_ID> 'echo hi' Enter
rozi status working --reason 'running tests'
rozi status --clear
rozi --session dev send-keys --target <PANE_ID> 'cargo test' Enter
rozi --session dev status --target <PANE_ID> working --reason 'running tests'
rozi --session dev status --clear --target <PANE_ID>
```

`send-text` sends its argument exactly and does not press Enter. `send-keys` accepts text and
tmux-style names such as `Enter`, `Escape`, `C-c`, arrows, `Tab`, and `F1` through `F12`. Add
`--literal` when a key-like argument such as `C-c` must be typed literally.

Use `--format json` for agent-readable output. `list-panes`, `layout get`, `capture-pane`, `capture-ui`, and
`metrics` support it. Capture options include `--scrollback 200`, `--scrollback full`, and `--last-output`.

When colors or layout matter, look at the screen instead of its text:
`capture-pane --target <PANE_ID> --render png --output pane.png` saves an image of the visible
screen in the pane's theme colors, and `--render ansi` keeps the colors as SGR-styled text. Both
cover the visible screen only, not scrollback, and neither includes inline images a program drew:
a blank area in the capture may be one.

`capture-ui --render png --output ui.png` captures the whole UI as drawn instead: the bar,
borders, overlays, and every visible pane. It needs a UI; `--session` refuses it.

Re-read pane ids before acting after a delay or any layout or session change.

## Target rules

On a UI endpoint, omitted targets use this pane's numeric `ROZI_PANE`, then the focused pane. Use
`--target` for another pane.

A session endpoint ignores `ROZI_PANE` because `--session` selects a different pane namespace. It
accepts `--target`, or falls back only when that session has exactly one pane. Agents should always
pass `--target` after reading that session's ids. `--socket` and `--session` cannot be combined.

## Split and UI-only commands

```bash
rozi split [COMMAND]
rozi split --workspace 9 --argv cargo test -- --nocapture
rozi --session dev split --workspace 9 --argv cargo test -- --nocapture
```

By default, `split` does **not** move focus. Its JSON response contains the new pane id;
`pty_ready:true` means its shell can receive input.

If `pty_ready:false`, the pane exists and is still starting. **Do not split again.** Send input to
its id; Rozi queues it as type-ahead.

A detached split applies current `[[rules]]`, but it is refused while an attached client holds
layout control. `--focus` has no meaning without a UI.

Other UI-only tools:

```bash
rozi focus <PANE_ID>                         # only when the user asks
rozi run-action <ACTION_ID>                  # never guess an id
rozi notify 'tests failed' --title Build --level error
branch=$(git branch --format='%(refname:short)' | rozi pick --title Branch) || exit 0
rozi subscribe pane-exited pane-status-changed
```

Use `notify` only for results the user cannot already see. Plain `pick` prints the chosen input line;
cancellation exits 1. `subscribe` streams `{event,data}` JSON rows. `publish` is a long-lived
bidirectional stream: write complete `{"rows":[…]}` snapshots and read `{"activate":"<id>"}`;
closing it withdraws the rows. `switch-workspace` and `move-to-workspace` also require a UI.

A detached endpoint supports `list-panes`, `layout get`, `layout set`, the `pane` commands,
`metrics`, `send-text`, `send-keys`, `capture-pane`, `split`, and `status`. Input still obeys the session's input lock.

## Detached-session limits

`--session` control is local. To control another host, run Rozi there:

```bash
ssh workbox rozi --session dev capture-pane --target <PANE_ID>
```

A pane created through detached `split` receives `ROZI` and `ROZI_PANE`, but no `ROZI_SOCKET` or
`ROZI_BIN` because no UI exists. Requests carrying `ROZI_EXTENSION` are refused because a session
server cannot validate the extension generation. Clear it only when the caller is a person who
inherited the variable, not the extension itself.

## Session lifecycle

```bash
rozi sessions list --format json
rozi sessions attach <NAME>
rozi sessions new <NAME> [--profile <PROFILE> | --cwd <DIR>]
rozi sessions kill <NAME>
rozi sessions list --remote <HOST>
rozi sessions kill <NAME> --remote <HOST>
```

`sessions kill` destroys the named server and all its PTYs for every client. Never use it as a
generic process killer.

Git worktrees live on the session host, and `--remote <HOST>` goes before `worktrees`:

```bash
rozi worktrees list [--cwd <DIR>] --format json
rozi worktrees create <BRANCH> [--base <REV>] [--path <DIR>] --format json
rozi worktrees remove <PATH> [--force]
rozi worktrees exclude [DIR]
```

`worktrees open` and `create --open` attach a UI, so run them only for a person at a terminal.
Removal never deletes the branch and refuses a checkout that a session records as its origin.

## Safety

- Mutate or kill only panes and sessions the user explicitly requested or this agent created.
- A read-only client cannot type, set status, or change layout. Do not retry those failures.
- Layout changes through a UI require controller status. Treat `not controller` as final until
  control changes hands.
- Input lock can block writable followers and detached control. Do not bypass it or repeatedly retry.
