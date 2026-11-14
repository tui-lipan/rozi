---
name: rozi
description: "Drive real terminals with the `rozi` multiplexer CLI: run interactive or long-running programs in a PTY, send keys and wait for their output, capture screens as text or PNG, record them, arrange panes, show the user a live preview, and prompt or supervise other coding agents. Use when the user mentions Rozi, panes, sessions, or a live preview, and whenever a task needs a real terminal: testing a TUI or REPL, screenshotting terminal output, keeping a dev server or watcher running beside your work, or delegating work to another agent CLI."
---

# Rozi

Rozi is a terminal multiplexer, and everything it does is scriptable through the `rozi` CLI. It
gives you real terminals: start programs in them, type into them, wait for what they print, look
at the screen as text or as an image, and leave them running while you do other work. It also
tracks the coding agents running in its panes, so you can hand a task to another agent and wait
for its answer.

## What you can do with it

Reach for Rozi whenever one shell command is not enough: the program needs a TTY, outlives a
single command, needs keystrokes, or its result is what the screen looks like.

| Goal | Start with |
| --- | --- |
| Keep a dev server, watcher, REPL, or debugger running while you work | [your own session](#work-in-your-own-session), then `send-keys … --wait-for` |
| Test a terminal app as a user would: press keys, read the screen, check colors and layout | `send-keys`, then `capture-pane --render png` or `--render spans` |
| Show the user a live preview beside their work | [`split` in their session](#show-the-user-a-preview) |
| Screenshot a pane or the whole UI for a report or pull request | `capture-pane --render png`, `capture-ui --render png` |
| Record a flow and export it as frames, video input, or an asciinema cast | [`record`](#record-a-pane-or-the-ui) |
| Ask another coding agent, or several in parallel, and get each answer as it arrives | [`agents ask --wait`](#coding-agents-in-panes) as a background command |
| Type exact input into an agent, such as a slash command | [`agents prompt`](#coding-agents-in-panes) |
| Read what another pane shows, such as a failing build beside you | `list-panes`, then `capture-pane --scrollback` |
| Arrange panes and workspaces when the user asks | [`layout` and `pane`](#arrange-panes) |
| Ask the user to choose or type something, or report a result | `pick`, `prompt`, `notify`, `status` |
| React to panes exiting or changing status | `subscribe` |
| Do any of this on another machine | `--remote <HOST> --session <NAME>` |

Your own detached session needs no running UI and works from any shell where `rozi` is installed,
inside Rozi or not. Only the user's own panes need the checks below.

The installed binary defines current syntax. Check it when unsure, and check a capability before
relying on a newer feature:

```bash
rozi --help
rozi agents --help
rozi api describe    # JSON: API, schema, and session protocol versions, plus capabilities
rozi api schema      # JSON Schema of every control command, response, and event
rozi api commands    # control commands by id; `rozi api command <ID>` for one's full contract
```

`api describe` lists, for example, `session-no-resurrect`, `split-placement`, and `remote-control`
when the binary supports those features.

Do not run bare `rozi` for discovery. It launches or attaches the TUI. Likewise, `rozi dev` launches
a UI; `rozi --session dev <COMMAND>` controls the `dev` session server.

## Choose where to work

Choose by who needs to see the pane:

- **The user asked to see it, or named one of their panes:** work in the user's session through
  its UI.
- **Anything else**, such as test runs, captures, recordings, or apps you only inspect yourself:
  create [your own detached session](#work-in-your-own-session). Never borrow one of the user's
  workspaces for it.

Every command reaches one of two endpoints:

- A **UI endpoint** controls the session shown by a running Rozi, including focus and overlays.
  Commands use `--socket PATH`, then a running `ROZI_SOCKET`, then the only UI showing the calling
  pane's session, then the only live local UI endpoint.
- A **session endpoint** controls a named session with no UI opened or attached. Select it with
  `--session <NAME>`. `--socket` and `--session` cannot be combined.

| Operation | UI endpoint | Named session |
| --- | --- | --- |
| Split, capture, input, status, layout and pane writes, pane recording, `agents` | Yes | Yes |
| `focus`, `pane reveal`, workspace switching, `capture-ui`, UI recording, `notify`, `pick`, `prompt` | Yes | No |
| Export or play a recording file | No endpoint needed | No endpoint needed |

Before using a UI endpoint, confirm you are inside one of its local panes:

```bash
rozi context --format json
```

Continue only when `data.status` is `verified`. Otherwise UI pane control is unavailable: never
control an arbitrary focused UI from outside one of its panes. This check is not required for a
session you created or one the user explicitly names.

If `data.ui.displays_session` is not `true`, the UI a plain command reaches shows another session.
Put `--here` before each command to target your own verified session:

- `--here` uses the authority of the one live UI displaying your session for splits, layout
  changes, pane close, and input. With no such UI, or several, it controls the session directly,
  where layout control must be free and input locks apply.
- If the UI switches session before the command runs, the command is refused.
- Inside a remotely attached pane, `--here` controls the session directly, because UI authority
  cannot cross hosts. If the remote attachment holds the authority the command needs, it returns
  `unsupported`: send the command through the UI socket on the UI host instead. Reads still work.

### Targets

Always read live pane ids. Never infer an id from pane order, a title, or an example, and read
them again after a delay or any layout or session change.

On a UI endpoint, an omitted target means this pane's numeric `ROZI_PANE`, then the focused pane.
Pass `--target` for any other pane so focus never decides where input goes.

A session endpoint ignores `ROZI_PANE` because `--session` selects a different pane namespace. It
falls back only when that session has exactly one pane, so always pass `--target` there.

## Read panes

```bash
# Session shown by this UI
rozi list-panes --format json
rozi layout get --format json
rozi capture-pane --target <PANE_ID> --format json

# Named session
rozi --session dev list-panes --format json
rozi --session dev layout get --format json
rozi --session dev capture-pane --target <PANE_ID> --format json
```

Use `--format json` for output you parse; `list-panes`, `layout get`, `capture-pane`, `capture-ui`,
`metrics`, and `agents list`/`get` support it. A capture reads the visible screen unless you add
`--scrollback 200`, `--scrollback full`, or `--last-output`.

`layout get` shows each workspace's layout and where every pane sits. `rect` is the pane's position
on the session's shared canvas, in cells, and is the same from every endpoint. From a UI, `client`
gives the focused pane and the workspace on screen, and `view_rect` gives where that UI draws each
pane. Use the rects to work out which pane is left of or above another.

When colors or layout matter, look at the screen instead of its text. These renders cover the
visible screen only:

| Render | Gives you |
| --- | --- |
| `--render png --output pane.png` | An image in the pane's theme colors, including images a program displayed. Always pass `--output`; Rozi refuses to write a PNG to a terminal. Add `--scale 2` only for a screenshot meant for a person. |
| `--render ansi` | The text with its colors as SGR sequences. Displayed images appear as `▀` half blocks, as they do in plain text. |
| `--render spans --format text` | One JSON line to check which text is red, bold, or selected without an image. `rows` hold runs of text with `x`, `width`, and only the styles that differ from the default; it also carries the cursor's position and shape. Colors are ANSI names, 256-color indexes, or `#rrggbb`, and `palette` says what the names look like. |

`rozi capture-ui --render png --output ui.png` captures the whole UI as drawn: the bar, borders,
overlays, and every visible pane.

## Send input and wait for the result

Inspect a pane before sending input unless the user gave an exact live id and exact input.

```bash
rozi send-text --target <PANE_ID> 'literal text'
rozi send-keys --target <PANE_ID> Enter
rozi send-keys --target <PANE_ID> C-c
rozi send-keys --target <PANE_ID> 'echo hi' Enter
rozi --session dev send-keys --target <PANE_ID> 'cargo test' Enter
```

`send-text` sends its argument exactly and does not press Enter. `send-keys` accepts text and
tmux-style names such as `Enter`, `Escape`, `C-c`, arrows, `Tab`, and `F1` through `F12`. Add
`--literal` when a key-like argument such as `C-c` must be typed literally.

Never sleep and then capture to read a command's answer. Make the send wait for it:

```bash
rozi send-keys --target <PANE_ID> 'cargo test' Enter --wait-for 'test result:' --timeout 10m --capture text
rozi send-keys --target <PANE_ID> 'ls' Enter --settle 500ms --timeout 30s --capture text
rozi capture-pane --target <PANE_ID> --wait-for 'Ready' --timeout 1m
```

- `--wait-for` waits for literal text on one row, `--settle` for the screen to stop changing.
  `--timeout` is always required.
- A send counts only output that arrives after its input, so an old prompt on screen does not end
  the wait. Prefer `--wait-for` with a string the command itself prints, and use `--settle` when
  there is none.
- `capture-pane --wait-for` also accepts text already on screen.
- On `timeout` or `pane-not-running` the reply still carries the screen. Read it before retrying.

## Open panes

```bash
rozi split [COMMAND]
rozi split --title Tests --argv cargo test -- --nocapture
rozi split --target <PANE_ID> --split-ratio 0.3 --argv cargo test   # split that pane, new pane 30%
```

By default, `split` does **not** move focus. Its JSON reply carries the new pane's id in `data.id`;
save it. `pty_ready:true` means its shell can receive input. If `pty_ready:false`, the pane exists
and is still starting. **Do not split again.** Send input to its id; Rozi queues it as type-ahead.

A split divides a pane as splitting it by hand would:

- From a UI it divides the calling pane, or the focused one in a `--workspace` you name.
  `--target <PANE_ID>` divides that pane instead, in its own workspace, and cannot go with
  `--workspace`.
- The direction follows the pane's shape and cannot be chosen: a wide pane splits side by side, a
  tall one stacks. Outside Dwindle the new pane goes at the end of the layout's order.
- `--split-ratio` sets the new pane's share. It is refused with `unsupported` outside Dwindle or
  when the pane would be a lone tile.

### Show the user a preview

Build the app first, read live pane ids and the layout, then launch the executable directly:

```bash
rozi split --cwd /absolute/project --title 'Preview' --argv /absolute/project/target/debug/app
rozi capture-pane --target <RETURNED_ID> --wait-for 'Ready' --timeout 30s --format json
rozi capture-pane --target <RETURNED_ID> --render png --output /absolute/scratch/preview.png
```

Wait for recognizable output from that app instead of the placeholder `Ready`, then inspect the
PNG yourself. Leave `--workspace` out so the pane opens in the workspace on screen, unless the user
names one. A requested preview permits creating its pane, not changing unrelated panes. Leave it
running for approval and report its controls.

To bring the preview into view in a Scrollable layout, use `rozi pane reveal --target <PANE_ID>`:
it scrolls only this UI's view and keeps focus. `split --focus` needs a verified UI endpoint and a
request that includes activating the preview.

### Work in your own session

```bash
rozi sessions new agent-<TASK> --detach --no-resurrect
rozi --session agent-<TASK> split --cwd /absolute/project --argv /absolute/project/target/debug/app
rozi --session agent-<TASK> capture-pane --target <RETURNED_ID> --wait-for 'Ready' --timeout 30s
rozi sessions kill agent-<TASK>
```

- `sessions new --detach` starts the session with no panes and returns; the first `split` opens
  one. Without `--detach` it opens a UI in the calling terminal, so never run it that way.
- Always add `--no-resurrect`, so a session you fail to kill leaves no `Restorable` entry in the
  user's session picker.
- Pick a name that says what the session is for. If the name is already running the command fails:
  choose another name rather than reusing a session you did not create.
- Kill the session when you are done, unless the user wants to keep it.

A pane created this way receives `ROZI` and `ROZI_PANE`, but no `ROZI_SOCKET` or `ROZI_BIN`,
because no UI exists. A split through a session endpoint applies the current `[[rules]]`, and is
refused while an attached client holds layout control.

## Coding agents in panes

Rozi detects coding-agent CLIs in panes and tracks each one's state: `working`, `blocked`, `idle`,
`done`, `error` (the run failed), or `unknown`. To delegate, start the agent's CLI in a pane with
`split`, wait for Rozi to detect it, then prompt it. Use the `agents` commands rather than scraping
the screen or typing into an agent pane with `send-keys`:

```bash
rozi agents list --format json
rozi agents get --target <PANE_ID> --format json
rozi agents read --target <PANE_ID> --scrollback 200
rozi agents wait --target <PANE_ID> --until quiescent --timeout 2m
rozi agents ask --ref '<JSON>' --wait --timeout 30m --format text -- 'Fix the failing test and tell me the cause'
rozi --here agents prompt --ref '<JSON>' --wait turn --timeout 10m '/compact'
rozi agents response --target <PANE_ID> --format json
rozi --session dev agents wait --target <PANE_ID> --until quiescent --timeout 2m
```

**Addressing.** The pane id always goes after `--target`; `agents get 3` is refused. From a UI
pane, a pane-number target addresses the session on screen; use `--here` or `--session <NAME>` for
another. `list` and `get` return an opaque `ref`, and a prompt wait reply carries it too. Save it
and pass `--ref '<json>'` instead of `--target` on later commands: a replacement agent in the same
pane then fails the command instead of receiving it. A pane can list several agents, such as a
client's background conversations. `--target` names the one on screen; the others need their `ref`.

**Prompting.** `agents prompt` submits the text and Enter as one operation, only to the agent it
resolved. It never types into a `blocked` agent, which may be showing an approval dialog, and
refuses a `working` one without `--allow-working`. Prompt an agent the user runs only when the
user asks.

**Waiting.** `--until` and `--wait` need `--timeout` and take any state, `quiescent` (idle, done,
or error), or `gone`.

- For a turn you submitted, use `prompt --wait turn`. It resolves on `blocked`, `done`, `error`,
  or `idle` after a transition. Check the returned state and reason: `blocked` means the agent
  needs an answer, and `error` means the turn failed.
- To catch the end of someone else's turn, wait for `quiescent`, not `idle`: a finished agent
  stays `done` until the user visits its pane. `quiescent` excludes `blocked`.
- Right after `split`, `wait` waits for the new pane's agent to be detected. `get` and `prompt`
  refuse a pane with no agent yet.

**Reading the answer.** Add `--result` to a prompt wait to get the agent's response with the
reply; `agents response` reads the latest one later. The JSON `response` object has `text`,
`source`, `fidelity`, `complete`, and `truncated`:

- `fidelity: "semantic"` is the agent's original text; `"rendered"` is terminal text.
- `source: "terminal"` can include earlier turns and interface text.
- Check `complete` and `truncated` before treating the text as the whole answer.

`--capture text` or `--capture ansi` adds a terminal snapshot to the wait reply and combines with
`--result`. `result.kind` is then `terminal` and `result.capture` holds the visible terminal,
chrome and older output included. Treat it as terminal content, not an assistant message.

**Asking.** To get an answer from another agent, ask it. A request reaches the agent without
interrupting it, and its answer is the agent's own reply text:

```bash
rozi agents ask --ref '<JSON>' --wait --timeout 30m --format text -- 'Review the parser changes in src/parse.rs'
```

- Run `ask --wait` as a **background command** when your harness has them. Your turn can end, the
  command exits with the answer as its output, and you are notified then. Ask several agents at
  once with one background command each. Keep `--format text`: without it, captured output is JSON.
- If the wait fails, the error names the request id: `rozi agents await` it, do not ask again.
- Rozi types the request into the target only once it is stopped, shows no dialog, and nobody has
  typed into its pane for two seconds; until then it is `queued`, with a `reason`.
- Without `--wait`, `ask` returns the request and its `id` at once; `rozi agents await <ID>
  --timeout <DUR>` waits for it later. A timeout leaves the request in place, so await again.
- `agents prompt` types exact text, such as a slash command, and its `--result` falls back to the
  terminal. Use it when the input itself matters, not to collect an answer.

**Answering.** A request that arrives in your own prompt starts with `Request <id>` and ends by
naming the reply command. Pipe the whole answer to it; the reply text is all the asker receives:

```bash
rozi agents reply <ID> --stdin <<'EOF'
The parser is fine; the lexer drops a trailing newline in src/lex.rs.
EOF
```

- To answer after your turn ends, such as when a background command finishes, run
  `rozi agents reply <ID> --later` before the turn ends, then `reply` when you have the answer.
  Otherwise the end of your turn answers the request with your terminal. You can take other
  requests meanwhile. While you owe more than one request, an `ask` you make for one of them
  needs `--for <ID>`.
- If your harness asks before each command, an approval for `rozi agents` keeps requests flowing;
  without it, the other agent's request waits on your approval.
- If `await` refuses because the other agent has asked you something (a wait cycle), read that
  request with `rozi agents request <ID>`, `reply` to it, then await again. A background `ask
  --wait` leaves your turn free, so requests to you still arrive.
- `rozi agents requests --incoming` or `--outgoing` lists them, and `rozi agents cancel <ID>`
  withdraws one you asked. A request is `queued`, `active`, `answered`, `failed`, or `cancelled`.

**Failures.** They distinguish a gone agent, a replacement, a stale reference, and a timeout. Do
not blindly retry them. When `agents prompt` refuses because the pane does not say which
conversation is on screen, check the screen before typing anything. An agent's prompt can read text
and Enter that arrive together as a paste and leave the text unsent, so send the text first, then
`Enter` in a separate `send-keys`, and confirm the agent started.

## Arrange panes

Change the user's layout only when the user asks. Every write names what it changes and never
moves focus:

```bash
rozi layout set --workspace 2 master
rozi layout set --workspace 2 master --master-ratio 0.6
rozi pane set --target <PANE_ID> --floating true --rect 10,5,80,24
rozi pane set --target <PANE_ID> --fullscreen false --if-revision <REVISION>
rozi pane set --target <PANE_ID> --split-ratio 0.6      # Dwindle
rozi pane set --target <PANE_ID> --width-ratio 0.5      # Scrollable
rozi pane move --target <PANE_ID> --workspace 3
rozi pane swap --target <PANE_ID> --with <PANE_ID>
rozi pane close --target <PANE_ID>
```

- Each ratio works only with the layout that uses it; anything else fails with `unsupported`.
- Writes set a state, so repeating one returns `changed:false`.
- Pass the `revision` from `layout get` as `--if-revision`. A layout someone changed meanwhile
  then fails with `conflict` instead of being overwritten; read the layout again.
- `pane close` ends the pane's process without asking.

These need a UI and act on what the user sees, so run them only when the user asks:

```bash
rozi focus <PANE_ID>
rozi switch-workspace <1-9>
rozi move-to-workspace <1-9>
rozi run-action <ACTION_ID>                  # never guess an id
```

## Record a pane or the UI

To watch a pane over time rather than sample it, record it:

```bash
rozi --session dev record start --target <PANE_ID>     # the reply's `path` is the file
rozi --session dev record mark "<label>" --target <PANE_ID>
rozi --session dev record stop --target <PANE_ID>
rozi record export <FILE> --to png-frames <DIR>        # or --to cast <FILE>
```

- The session server writes the file and names it on its own host; pass an absolute `--output` to
  choose it. The pane's title shows `● REC` while it records.
- From a local session pane, drop `--session`: the request goes to that pane's own session
  (`ROZI_SESSION_INSTANCE`), even when another session is on screen, and each command acts on the
  calling pane unless given `--target`. A scratch or popup pane still needs `--session`.
- To record what the user sees, chrome included, run `rozi record start ui` in a pane inside Rozi
  and `rozi record stop --ui` to finish. The UI writes the file on its own machine and shows a
  `● REC` chip on its bar, or `● UI REC` in a fullscreen pane's title.
- For video, `--to video-frames <DIR>` pads the PNGs to a fixed canvas across terminal resizes.
  Both frame exports write `frames.ffconcat` with actual durations; give it to ffmpeg rather than
  assuming a constant frame rate. For a short animation review, start with `--max-fps 60`.

Record one of the user's panes or their UI only when the user asked: the file holds everything
shown, real timers and subprocess output included.

## Talk to the user and to scripts

```bash
rozi status working --reason 'running tests'
rozi status --clear
rozi --session dev status --target <PANE_ID> working --reason 'running tests'
rozi --session dev status --clear --target <PANE_ID>
rozi notify 'tests failed' --title Build --level error
branch=$(git branch --format='%(refname:short)' | rozi pick --title Branch) || exit 0
name=$(rozi prompt --title 'Branch name') || exit 0
rozi subscribe pane-exited pane-status-changed
rozi actions list --format json              # a session's deployed actions, with their schemas
rozi invoke <EXT>.<ACTION> --args-json '{}'  # a deployed action's JSON result is in data.result
```

- `status` sets the status the UI shows for a pane; set it on your own pane during long work.
- Use `notify` only for results the user cannot already see.
- `pick` prints the chosen input line and `prompt` the entered text. Cancellation exits 1.
- `subscribe` streams `{event,data}` JSON rows. `rozi --session <NAME> subscribe
  [--snapshot | --after INSTANCE:SEQ]` streams a session's typed records with no UI.
- `publish` is a long-lived bidirectional stream: write complete `{"rows":[…]}` snapshots and read
  `{"activate":"<id>"}`. Closing it withdraws the rows. Its rows are activities unless it opens
  with `--kind agent`. `status` is an activity too; only agents appear in `rozi agents`.

A session endpoint refuses requests carrying `ROZI_EXTENSION`, because it cannot validate the
extension generation. Clear the variable only when the caller is a person who inherited it, not
the extension itself.

## Sessions, remote hosts, and worktrees

```bash
rozi sessions list --format json
rozi sessions attach <NAME> [--read-only | --follow | --take-control]
rozi sessions new <NAME> [--profile <PROFILE> | --cwd <DIR>]
rozi sessions new <NAME> --detach [--no-resurrect]
rozi sessions kill <NAME>
```

`sessions attach` and `sessions new` without `--detach` open a UI, so they are for a person at a
terminal. `sessions kill` destroys the named server and all its PTYs for every client. Never use it
as a generic process killer.

To control a session on another host, put `--remote <HOST>` (an SSH alias or `ssh://` URL) in front
of `--session`. The command is forwarded over SSH and answered exactly as a local session would
answer it:

```bash
rozi --remote workbox --session dev list-panes --format json
rozi --remote workbox --session dev agents wait --target <PANE_ID> --until quiescent --timeout 5m
rozi sessions list --remote <HOST>
rozi sessions kill <NAME> --remote <HOST>
```

`--remote` without `--session` is refused for control commands: they address a session server,
never the far host's UI. An older Rozi on that host reports version skew.

Git worktrees live on the session host, and `--remote <HOST>` goes before `worktrees`:

```bash
rozi worktrees list [--cwd <DIR>] --format json
rozi worktrees create <BRANCH> [--base <REV>] [--path <DIR>] --format json
rozi worktrees remove <BRANCH|PATH> [--cwd <DIR>] [--force]
rozi worktrees unlock <BRANCH|PATH> [--cwd <DIR>]
rozi worktrees exclude [DIR]
```

- `worktrees open` and `create --open` attach a UI, so run them only for a person at a terminal.
- A bare name is matched both as a branch and as a path from `--cwd`. When those are different
  checkouts the command fails; pass `--branch <NAME>` or `./<NAME>` to choose.
- Removal never deletes the branch, and refuses a checkout that is locked or that a session
  records as its origin.
- In `list` JSON, a checkout's `lock` holds Git's `reason` and `stale`, which is true when the
  process the reason names has exited. Unlock only a stale lock unless the user asks: another
  agent may still be working in a checkout with a live lock.

## Safety

- Mutate, close, or kill only panes and sessions the user explicitly requested or you created.
- `not-controller` means another client is arranging the session. Layout changes through a UI
  require controller status; treat the failure as final until control changes hands.
- Input lock can block writable followers and session endpoints. Do not bypass it or retry.
- A read-only client cannot type, set status, or change layout. Do not retry those failures.
- `[session] on_attach` defaults to `"ask"` when another client controls the session. `"follow"`
  follows without prompting; `"take"` takes or requests control under the running server's policy.
  `sessions attach --follow` and `--take-control` override it for one attach and conflict with
  each other and with `--read-only`.
