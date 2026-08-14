# Extension recipes

Worked examples that combine the [control socket](control.md), [published rows](sidebar.md),
[services](configuration.md#services), and [hooks](hooks.md) into things rozi does not ship.

None of these need a plugin runtime. A supervised service that subscribes to events, publishes
sidebar rows, and raises a picker when it needs a decision *is* a plugin — written in whatever
language you like, running out of process, unable to take the UI down with it.

Everything below assumes `ROZI_SOCKET` is set, which it is inside any rozi pane.

## Pick a branch, worktree, or file

`rozi pick` renders rozi's own palette. In its default mode stdin is one label per line and stdout
is the chosen line, so it drops into a pipeline with no `jq`:

```bash
git branch --format='%(refname:short)' | rozi pick --title Branch | xargs -r git switch
```

Bind it to a chord so it works from anywhere:

```toml
[keys]
"ctrl-a b" = { run = "git branch --format='%(refname:short)' | rozi pick --title Branch | xargs -r git switch" }
```

`--json` buys what a plain list cannot express — sections, right-aligned badges, and rows that stay
visible while explaining why they are unavailable:

```bash
#!/usr/bin/env bash
# Worktrees, with the ones already open in a pane greyed out rather than hidden.
open=$(rozi list-panes | jq -r '.data[].cwd // empty')
git worktree list --porcelain \
  | awk '/^worktree /{print $2}' \
  | jq -R --arg open "$open" --arg here "$PWD" '{
      id: ., label: (split("/") | last), description: .,
      group: (if . == $here then "Current" else "Other worktrees" end),
      disabled: (if ($open | split("\n") | index(.)) then "Already open" else null end)
    }' \
  | jq -sc '{rows: .}' \
  | rozi pick --json --title Worktree \
  | jq -r .selected \
  | xargs -r -I{} rozi new-pane --cwd {} --focus
```

The `disabled` field is the part a generic fuzzy finder cannot do: the row stays on screen with the
reason attached instead of silently vanishing from the list.

## Publish live rows into the sidebar

`rozi publish` is a two-way stream: it reads `{"rows":[…]}` on stdin and writes `{"activate":"<id>"}`
on stdout when someone clicks a row. Rows carry a status the sidebar renders as a badge, and rozi
keeps an elapsed clock per row for as long as its status is not quiescent.

```bash
#!/usr/bin/env bash
# One row per cargo target, with a live clock while it builds.
while :; do
  status=$(pgrep -q cargo && echo working || echo idle)
  printf '{"rows":[{"id":"build","title":"cargo build","status":"%s"}]}\n' "$status"
  sleep 2
done | rozi publish
```

A publisher no longer has to be a recognized AI agent — that gate was lifted, so a build watcher, a
job runner, or a deploy script publishes exactly the rows an agent does. Publishing rows and
claiming the pane's *agent* state are separate concerns; a publisher that is not the pane's agent
does not take over its detection.

## A PR dashboard that survives the night

Combining `[[services]]` with `publish` gives a supervised poller whose rows are clickable. The
service starts with the client, restarts up a backoff ladder if it crashes, and dies on detach.

```toml
[[services]]
name = "pr-watch"
run = "~/.config/rozi/pr-watch.sh"
restart = "on-failure"
```

```bash
#!/usr/bin/env bash
# ~/.config/rozi/pr-watch.sh - a row per open PR, coloured by its checks.
poll() {
  while :; do
    gh pr list --json number,title,statusCheckRollup --jq '{rows: [.[] | {
        id: ("pr-" + (.number|tostring)),
        title: .title,
        badge: ("#" + (.number|tostring)),
        status: (if   .statusCheckRollup[0].conclusion == "FAILURE"   then "blocked"
                 elif .statusCheckRollup[0].status     == "IN_PROGRESS" then "working"
                 else "idle" end)
      }]}'
    sleep 30
  done
}
poll | rozi publish | while read -r line; do
  number=$(jq -r '.activate' <<<"$line" | sed 's/^pr-//')
  rozi new-pane "gh pr checkout $number" --focus
done
```

A failing check shows as `blocked` with an elapsed clock; clicking the row checks the branch out.

## Notifications that are not stupid

A hook runs one process per event and remembers nothing, so it cannot coalesce a burst or skip the
pane you are already looking at. A service holding a `subscribe` stream can:

```toml
[[services]]
name = "notify"
run = "~/.config/rozi/notify.py"
```

```python
#!/usr/bin/env python3
# Debounce bells and ignore the focused pane. `subscribe` streams one JSON event per line.
import json, os, socket, time

focused, last = None, 0.0
sock = socket.socket(socket.AF_UNIX)
sock.connect(os.environ["ROZI_SOCKET"])
sock.sendall(b'{"cmd":"subscribe"}\n')
for line in sock.makefile():
    event = json.loads(line)
    if event.get("event") == "focus-changed":
        focused = event.get("pane")
    elif event.get("event") == "bell" and event.get("pane") != focused:
        if time.monotonic() - last > 5.0:
            os.system("notify-send 'rozi' 'a background pane rang'")
            last = time.monotonic()
```

## Use yazi as rozi's file router

[yazi](https://github.com/sxyazi/yazi)'s chooser mode makes it a front end for the whole
multiplexer. Open it in a popup, and route whatever it returns into a pane:

```toml
[keys]
"ctrl-a e" = { popup = "yazi --chooser-file=/tmp/rozi-chosen && rozi new-pane \"$EDITOR $(cat /tmp/rozi-chosen)\" --focus", keep_open = false }
```

The file tree in the sidebar already passes an activated path as `ROZI_FILE`, so a `[keys] run`
entry never needs a filename spliced into its command line.

## Search your command history and re-send it

Shell integration records each command's output, so `capture-pane --last-output` plus `pick` gives a
searchable history that types the winner back into the pane:

```bash
history | sed 's/^ *[0-9]* *//' | rozi pick --title History \
  | xargs -r -0 rozi send-text
```

## What is still out of scope

- **Rows without a pane.** `publish` is pane-scoped: its rows belong to the pane whose program
  opened the stream, and they go away with it. A daemon with no pane cannot publish. Poll-based
  [command tabs](sidebar.md) cover part of that gap.
- **Services on the server.** Services are client-side, like hooks. They cannot react while nothing
  is attached, and they have no `ROZI_SOCKET` to talk to when detached — the control endpoint
  belongs to the UI process.
- **Two pickers at once.** A second `pick` while one is open is refused rather than queued, so a
  caller never blocks on an unbounded human delay.
