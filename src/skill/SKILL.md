---
name: rozi
description: "Inspect and control Rozi terminal panes and sessions. Use when the user asks to use Rozi, or requests a live terminal preview in a verified Rozi pane. UI pane control requires ROZI=1 and a non-empty ROZI_SOCKET."
---

# Rozi

Rozi is a terminal multiplexer with a CLI for controlling a running UI or a detached named session.
Use this skill when the user asks to use Rozi or control a Rozi pane or session, or requests a
live terminal preview while this agent has a verified Rozi pane context.

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

The installed binary defines current syntax. Check it when unsure, and check a capability before
relying on a newer feature:

```bash
rozi --help
rozi agents --help
rozi api describe    # JSON: API, schema, and session protocol versions, plus capabilities
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
rozi pane reveal --target <PANE_ID>                     # Scrollable: scroll to it, keep focus
rozi layout set --workspace 2 master --master-ratio 0.6
```

Each ratio works only with the layout that uses it; anything else fails with `unsupported`.
`pane reveal` is not a layout write: it scrolls only this UI's view. To show the user a pane in a
Scrollable strip, prefer it over `focus`.

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

Never sleep and then capture to read a command's answer. Make the send wait for it instead:

```bash
rozi send-keys --target <PANE_ID> 'cargo test' Enter --wait-for 'test result:' --timeout 10m --capture text
rozi send-keys --target <PANE_ID> 'ls' Enter --settle 500ms --timeout 30s --capture text
rozi capture-pane --target <PANE_ID> --wait-for 'Ready' --timeout 1m
```

`--wait-for` waits for literal text on one row, `--settle` for the screen to stop changing, and
`--timeout` is always required. A send only counts output that arrives after its input, so an old
prompt on screen does not end the wait; prefer `--wait-for` with a string the command itself will
print, and use `--settle` when there is none. `capture-pane --wait-for` accepts text already on
screen. On `timeout` or `pane-not-running` the reply still carries the screen, so read it before
retrying.

Use `--format json` for agent-readable output. `list-panes`, `layout get`, `capture-pane`,
`capture-ui`, `metrics`, and `agents list`/`get` support it. Capture options include
`--scrollback 200`, `--scrollback full`, and `--last-output`.

When colors or layout matter, look at the screen instead of its text:
`capture-pane --target <PANE_ID> --render png --output pane.png` saves an image of the visible
screen in the pane's theme colors, and `--render ansi` keeps the colors as SGR-styled text. Both
cover the visible screen only, not scrollback. Images a program displayed, such as `kitty icat`
output, are drawn in a PNG; in text and ANSI output they appear as `▀` half blocks. Always pass
`--output` for a PNG; Rozi refuses to write one to a terminal. Keep the default scale for images
you will look at yourself; add `--scale 2` only for a screenshot meant for a person.

To check colors or attributes without an image, such as which line is red, bold, or selected, use
`--render spans --format text`: one JSON line whose `rows` hold runs of text, each with `x`,
`width`, and only the styles that differ from the default (`fg`, `bg`, `bold`, `underline`, and so
on), plus the cursor's position and shape. Colors are ANSI names like `red`, 256-color indexes, or
`#rrggbb`; `palette` says what the names look like.

`capture-ui --render png --output ui.png` captures the whole UI as drawn instead: the bar,
borders, overlays, and every visible pane. It needs a UI; `--session` refuses it.

To watch a pane over time rather than sample it, record it in the session server:
`rozi --session dev record start --target <PANE_ID>`, then `record mark "<label>"` at
interesting moments and `record stop --target <PANE_ID>`. The start reply's `path` is the file,
which the server names on its own host; pass an absolute `--output` to choose it. From a local
session pane, drop `--session`: the UI sends the request to that pane's session
(`ROZI_SESSION_INSTANCE`), even when another session is on screen. Each command acts on the
calling pane unless given `--target`. A scratch or popup pane still needs `--session`. Only record
when the user asked: the file holds everything the pane shows, and the pane's title shows `● REC`
while it records.
To record what the user sees instead, chrome included, run `rozi record start ui` in a pane inside
rozi and `rozi record stop --ui` to finish; the UI writes the file on its own machine and shows a
`● REC` chip on its bar meanwhile, or `● UI REC` in a fullscreen pane's title.
`rozi record export <FILE> --to png-frames <DIR>` or `--to cast <FILE>` turns it into files.
For video conversion after terminal resizes, `--to video-frames <DIR>` pads PNGs to a fixed canvas.
Both frame exports write `frames.ffconcat` with actual durations; use it with ffmpeg rather than
assuming a constant frame rate. For short animation reviews, request `--max-fps 60` at recording
start. Live recordings include real timers and subprocess output.

Re-read pane ids before acting after a delay or any layout or session change.

## Coding agents in panes

Rozi detects coding-agent CLIs in panes and tracks their state: `working`, `blocked`, `idle`,
`done`, `error` (the run failed), or `unknown`. Use the `agents` commands rather than scraping the screen or typing into an
agent pane with `send-keys`:

```bash
rozi agents list --format json
rozi agents get --target <PANE_ID> --format json
rozi agents read --target <PANE_ID> --scrollback 200
rozi agents wait --target <PANE_ID> --until quiescent --timeout 2m
rozi --here agents prompt --ref '<JSON>' --wait turn --timeout 10m --capture text 'Fix the failing test'
rozi --session dev agents wait --target <PANE_ID> --until quiescent --timeout 2m
```

The pane id always goes after `--target`; `agents get 3` is refused. A pane can list several
agents, such as a client's background conversations: `--target` names the one on screen, and the
others, even a lone background one, need their `--ref` from `agents list`. From a UI pane, every
`agents` pane-number target addresses the session on screen. Use `--here` to target your own
verified session, or `--session <NAME>` for a named one. `rozi context --format json` verifies
your caller pane and session and reports stale identity. It works without a UI socket.
`--here` uses your displayed UI's authority for splits, layout changes, pane close, and input.
If `ROZI_SOCKET` is absent, stale, or displays another session, it searches for exactly one live
UI displaying your verified session. If none or several match, it uses direct session control,
where layout control must be free and input locks apply. A UI session switch before execution
refuses the command.
UI authority cannot cross hosts. Inside a remotely attached pane, `--here` uses direct session
control unless a suitable UI runs on the session host. When the remote attachment holds the
required authority, `--here` returns `unsupported`. Submit the mutation through the UI socket on
the UI host, detach the controller for layout changes, or unlock input for sends and prompts.
Read operations still work.
`--until` and `--wait` take `working`, `blocked`, `idle`, `done`, `error`, `quiescent` (idle, done,
or error), or `gone`, and both need `--timeout`. An agent that finished a turn stays `done` until the
user visits its pane, so wait for `quiescent`, not `idle`, to catch the end of a turn; check the
returned state, since a failed turn also ends the wait, as `error`. Right after `split`,
`wait` waits for the new pane's agent to be detected; `get` and `prompt` refuse a pane with no agent
yet.

For a submitted turn, use `prompt --wait turn`: it resolves on `blocked`, `done`, `error`, or
`idle` after a transition, and returns the ordinary state and reason. `quiescent` excludes blocked.
Add `--capture text` or `--capture ansi` to retrieve a terminal snapshot with the wait reply.
`result.kind` is `terminal`; `result.capture` contains the visible terminal, including chrome and
older output. Treat it as terminal content, not an assistant message. Discover an agent, save its
`ref`, and use `--ref` for later operations. A prompt wait reply also carries that exact `ref`.

`agents prompt` submits the text and Enter as one operation, only to the agent incarnation it
resolved. It never types into a `blocked` agent, which may be showing an approval dialog, and it
refuses a `working` one unless `--allow-working` is given. Prompt another agent only when the user
asks. `list` and `get` return an opaque `ref`; pass it back with `--ref '<json>'` instead of
`--target` to fence a later command to that exact agent, so a replacement in the same pane fails
instead of receiving it. Failures distinguish a gone agent, a replacement, a stale reference, and a
timeout; do not blindly retry them.

## Target rules

On a UI endpoint, omitted targets use this pane's numeric `ROZI_PANE`, then the focused pane. Use
`--target` for another pane.

A session endpoint ignores `ROZI_PANE` because `--session` selects a different pane namespace. It
accepts `--target`, or falls back only when that session has exactly one pane. Agents should always
pass `--target` after reading that session's ids. `--socket` and `--session` cannot be combined.

## Show a requested preview

Build the app first, read live pane IDs and layout, then launch the executable directly:

```bash
rozi split --cwd /absolute/project --title 'Preview' --argv /absolute/project/target/debug/app
rozi capture-pane --target <RETURNED_ID> --wait-for 'Ready' --timeout 30s --format json
rozi capture-pane --target <RETURNED_ID> --render png --output /absolute/scratch/preview.png
```

Use recognizable output from that app instead of the placeholder `Ready`. Save the returned id
and inspect the PNG. A requested preview permits creating its pane, not changing unrelated panes.
Leave it running for approval and report its controls. In a Scrollable layout, `pane reveal`
shows it without moving focus. `split --focus` needs a verified UI endpoint and a request that
includes activating the preview; never combine it with `--session`.

| Operation | UI endpoint | Named session |
| --- | --- | --- |
| Split, pane capture, input, pane recording | Yes | Yes |
| Focus, reveal, whole-UI capture/recording | Yes | No |
| Export or play a local file | No endpoint needed | No endpoint needed |

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
closing it withdraws the rows. Its rows are activities unless it opens with `--kind agent`;
`status` is an activity too, and only agents appear in `rozi agents`. `switch-workspace` and `move-to-workspace` also require a UI.

A detached endpoint supports `list-panes`, `layout get`, `layout set`, the `pane` commands,
`metrics`, `send-text`, `send-keys`, `capture-pane`, `split`, `status`, `record`, and the `agents`
commands.
Input still obeys the session's input lock.

## Detached-session limits

To control a session on another host, add `--remote <HOST>` (an SSH alias or `ssh://` URL) in
front of `--session`. The command is forwarded over SSH and answered exactly as a local session
would answer it:

```bash
rozi --remote workbox --session dev list-panes --format json
rozi --remote workbox --session dev agents wait --target <PANE_ID> --until quiescent --timeout 5m
```

`--remote` without `--session` is refused: a control command addresses a session server, never the
far host's UI. An older rozi on that host reports version skew; `rozi api describe` there lists
`remote-control` when it can serve forwarded commands.

A pane created through detached `split` receives `ROZI` and `ROZI_PANE`, but no `ROZI_SOCKET` or
`ROZI_BIN` because no UI exists. Requests carrying `ROZI_EXTENSION` are refused because a session
server cannot validate the extension generation. Clear it only when the caller is a person who
inherited the variable, not the extension itself.

## Session lifecycle

```bash
rozi sessions list --format json
rozi sessions attach <NAME> [--read-only | --follow | --take-control]
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
rozi worktrees remove <BRANCH|PATH> [--cwd <DIR>] [--force]
rozi worktrees unlock <BRANCH|PATH> [--cwd <DIR>]
rozi worktrees exclude [DIR]
```

`worktrees open` and `create --open` attach a UI, so run them only for a person at a terminal.
A bare name is matched both as a branch and as a path from `--cwd`; when those are different
checkouts the command fails, so pass `--branch <NAME>` or `./<NAME>` to choose. Removal never deletes the branch and refuses a checkout that a session records as its origin, or
one that is locked. In `list` JSON, a checkout's `lock` holds Git's `reason` and `stale`, which is
true when the process the reason names has exited. Unlock only a stale lock unless the user asks:
another agent may still be working in a checkout with a live lock.

## Safety

- Mutate or kill only panes and sessions the user explicitly requested or this agent created.
- `[session] on_attach` defaults to `"ask"` when another client controls the session. `"follow"`
  follows without prompting; `"take"` takes or requests control under the running server's policy.
  `sessions attach --follow` and `--take-control` override it for one attach and conflict with
  each other and with `--read-only`.
- A read-only client cannot type, set status, or change layout. Do not retry those failures.
- Layout changes through a UI require controller status. Treat `not controller` as final until
  control changes hands.
- Input lock can block writable followers and detached control. Do not bypass it or repeatedly retry.
