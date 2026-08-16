# Extension recipes

Worked examples that combine the [control socket](control.md), [published rows](sidebar.md),
[services](configuration.md#services), and [hooks](hooks.md) into things rozi does not ship.

None of these need a plugin runtime. A supervised service that subscribes to events, publishes
sidebar rows, and raises a picker when it needs a decision *is* a plugin — written in whatever
language you like, running out of process, unable to take the UI down with it.

Everything below assumes `ROZI_SOCKET` is set, which it is inside any rozi pane.

## Four things that will bite you first

**Call rozi through `$ROZI_BIN`, not `PATH`.** Panes, hooks, and services all receive `ROZI_BIN`
holding the path of the running binary, precisely so a recipe does not have to assume an install on
`PATH` — a build started with `cargo run` is not on it, and neither is a binary installed under
another name. Prefer it, and fall back for the case where it is genuinely absent (a remote pane,
where this client's path means nothing on the other host):

```bash
if ! command -v "${ROZI_BIN:-rozi}" >/dev/null 2>&1; then
  echo "rozi is not on PATH (and ROZI_BIN is unset)" >&2
  exit 127
fi
rozi() { command "${ROZI_BIN:-rozi}" "$@"; }   # must come *after* the check,
                                               # or `command -v` finds this function
```

**Cancelling exits 1, and that is not an error.** `rozi pick` reports a cancellation with status 1
so a `&&` chain stops rather than acting on an empty choice. A script must not propagate it: an
`exec` binding toasts on any non-zero exit, so pressing Esc would look like a broken command.

```bash
chosen=$(… | rozi pick …) || true   # a cancel is a decision, not a failure
[ -n "$chosen" ] || exit 0
```

**A closing action is silent unless you say something.** An `exec` binding has no pane and toasts
only on failure, so a script that succeeds and exits reports nothing at all. Use `rozi notify` when
the result is off screen:

```bash
git switch "$branch" && rozi notify "switched to $branch"
```

**A pipeline reports its last stage, not the failed one.** `git branch | rozi pick | xargs -r git
switch` exits `0` even when `rozi` is missing entirely, because `xargs -r` with no input succeeds —
so a `keep_open` pane cheerfully prints `exited with status 0` over the error. `set -o pipefail`
fixes it in bash but is not POSIX, so prefer a command substitution and let `&&` carry the status:

```bash
branch=$(git branch --format='%(refname:short)' | rozi pick --title Branch) && git switch "$branch"
```

## Pick a branch, worktree, or file

`rozi pick` renders rozi's own palette. In its default mode stdin is one label per line and stdout
is the chosen line, so it drops into a pipeline with no `jq`:

```bash
branch=$(git branch --format='%(refname:short)' | rozi pick --title Branch) && git switch "$branch"
```

Straight from a shell that is fine — a cancelled `&&` chain just does nothing. From a keybinding,
guard it as shown above so Esc does not toast.

That plain list cannot say which branch you are already on. `--json` can, and the convention to
follow is the layout picker's: a right-aligned `current` badge, plus `active` to tint the row -
the colour alone is too quiet to rely on. Ordering by commit date beats alphabetical here too:

```bash
branch=$(git for-each-ref refs/heads/ --sort=-committerdate \
      --format='%(HEAD)%09%(refname:short)%09%(committerdate:relative)' \
  | jq -Rc 'split("\t") | {
      id: .[1], label: .[1], active: (.[0] == "*"),
      description: (if .[0] == "*" then "current · " + .[2] else .[2] end)
    }' \
  | jq -sc '{rows: .}' \
  | rozi pick --json --title "Switch branch" \
  | jq -r '.selected // empty') && git switch "$branch"
```

`description` is one right-aligned string, so a marker and a detail share that column rather than
occupying separate slots.

Bind it to a chord so it works from anywhere:

```toml
[keys]
i = { exec = "~/.config/rozi/branch-pick.sh", label = "Switch branch" }
```

`--json` buys what a plain list cannot express — sections, right-aligned badges, and rows that stay
visible while explaining why they are unavailable:

```bash
#!/usr/bin/env bash
# Worktrees, with the ones already open in a pane greyed out rather than hidden.
open=$(rozi list-panes | jq -r '.data[].cwd // empty')
chosen=$(git worktree list --porcelain \
  | awk '/^worktree /{print $2}' \
  | jq -R --arg open "$open" --arg here "$PWD" '{
      id: ., label: (split("/") | last), description: .,
      group: (if . == $here then "Current" else "Other worktrees" end),
      disabled: (if ($open | split("\n") | index(.)) then "Already open" else null end)
    }' \
  | jq -sc '{rows: .}' \
  | rozi pick --json --title Worktree \
  | jq -r '.selected // empty') || true
[ -n "$chosen" ] || exit 0
rozi new-pane --cwd "$chosen" --focus
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
"ctrl-a shift-e" = { popup = "yazi --chooser-file=/tmp/rozi-chosen && rozi new-pane \"$EDITOR $(cat /tmp/rozi-chosen)\" --focus", keep_open = false }
```

The file tree in the sidebar already passes an activated path as `ROZI_FILE`, so a `[keys] run`
entry never needs a filename spliced into its command line.

## Search your command history and re-send it

Shell integration records each command's output, so `capture-pane --last-output` plus `pick` gives a
searchable history that types the winner back into the pane:

```bash
cmd=$(history | sed 's/^ *[0-9]* *//' | rozi pick --title History) \
  && rozi send-text "$cmd"
```

## Package the branch picker as an extension

The branch picker above becomes distributable by giving it a manifest and stable namespaced id:

```text
~/.local/share/rozi/extensions/git-tools/
├── extension.toml
└── bin/branch-pick
```

```toml
[extension]
title = "Git tools"
version = "0.1.0"

[[commands]]
id = "branches"
label = "Switch branch"
exec = "./bin/branch-pick"
```

Rozi registers it as `git-tools.branches`. It is immediately available in the **Git tools**
palette group and through `rozi run-action git-tools.branches`; a key is optional:

```toml
[keys]
"git-tools.branches" = "i"
```

The command receives `ROZI_EXTENSION_DIR` for its own assets while its working directory remains
the focused pane's repository. See [Extensions](extensions.md) for the full manifest and trust
model.

## What is still out of scope

- **Rows without a pane.** `publish` is pane-scoped: its rows belong to the pane whose program
  opened the stream, and they go away with it. A daemon with no pane cannot publish. Poll-based
  [command tabs](sidebar.md) cover part of that gap.
- **Services on the server.** Services are client-side, like hooks. They cannot react while nothing
  is attached, and they have no `ROZI_SOCKET` to talk to when detached — the control endpoint
  belongs to the UI process.
- **Two pickers at once.** A second `pick` while one is open is refused rather than queued, so a
  caller never blocks on an unbounded human delay.
