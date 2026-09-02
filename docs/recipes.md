# Automation recipes

These recipes use Rozi's public CLI, hooks, services, pickers, and published activity.

Scripts launched by Rozi should use `ROZI_BIN` and `ROZI_SOCKET`:

```sh
ROZI=${ROZI_BIN:-rozi}
if [ -n "${ROZI_SOCKET:-}" ]; then
    set -- "$ROZI" --socket "$ROZI_SOCKET"
else
    set -- "$ROZI"
fi
```

The examples below use `"$@"` after this setup.

## Pick and switch a Git branch

```sh
#!/bin/sh
set -eu

ROZI=${ROZI_BIN:-rozi}
if [ -n "${ROZI_SOCKET:-}" ]; then
    set -- "$ROZI" --socket "$ROZI_SOCKET"
else
    set -- "$ROZI"
fi

branch=$(git branch --format='%(refname:short)' | "$@" pick --title "Git branches") || exit 0
[ -n "$branch" ] || exit 0
git switch "$branch"
"$@" notify "switched to $branch"
```

Plain picker input is one row per line. Output is the selected row. See
[Control CLI](control.md#pickers) for grouped, disabled, and actionable JSON rows.

## Open a worktree in a pane

```sh
#!/bin/sh
set -eu

ROZI=${ROZI_BIN:-rozi}
if [ -n "${ROZI_SOCKET:-}" ]; then
    set -- "$ROZI" --socket "$ROZI_SOCKET"
else
    set -- "$ROZI"
fi

worktree=$(
    git worktree list --porcelain |
        awk '$1 == "worktree" { sub(/^worktree /, ""); print }' |
        "$@" pick --title "Git worktrees"
) || exit 0
[ -n "$worktree" ] || exit 0
"$@" split --cwd "$worktree" --focus
```

Quoting `"$worktree"` is required because picker output is untrusted text and paths may contain
spaces.

## Use Yazi as a file router

Create `~/.config/rozi/scripts/yazi-router`:

```sh
#!/bin/sh
set -eu

ROZI=${ROZI_BIN:-rozi}
choice=$(mktemp "${TMPDIR:-/tmp}/rozi-yazi.XXXXXX")
trap 'rm -f "$choice"' EXIT HUP INT TERM

yazi --chooser-file="$choice"
IFS= read -r selected < "$choice" || exit 0
[ -n "$selected" ] || exit 0

if [ -n "${ROZI_SOCKET:-}" ]; then
    "$ROZI" --socket "$ROZI_SOCKET" split --focus --argv "${EDITOR:-vi}" "$selected"
else
    "$ROZI" split --focus --argv "${EDITOR:-vi}" "$selected"
fi
```

Make it executable, then bind the helper:

```toml
[keys]
"ctrl-a shift-e" = { popup = "~/.config/rozi/scripts/yazi-router", keep_open = false }
```

The helper creates a private temporary chooser file, removes it on exit, and passes the selected
path as a direct argument. It does not share a predictable file or insert the path into shell
source.

For file-tree activation, Rozi supplies the selected path in `ROZI_FILE`. Read that variable instead
of inserting the path into a command:

```toml
[sidebar]
tabs = [
  { name = "files", label = "", on_click = { run = '''"${EDITOR:-vi}" "$ROZI_FILE"''' } },
]
```

## Watch events

This Python service ignores bells from the focused pane and debounces notifications:

```python
#!/usr/bin/env python3
import json
import os
import subprocess
import time

rozi = os.environ.get("ROZI_BIN", "rozi")
command = [rozi]
if socket_path := os.environ.get("ROZI_SOCKET"):
    command += ["--socket", socket_path]
command += ["subscribe", "focus-changed", "bell"]

focused = None
last_notification = 0.0
with subprocess.Popen(
    command,
    stdout=subprocess.PIPE,
    text=True,
) as process:
    assert process.stdout is not None
    for line in process.stdout:
        event = json.loads(line)
        data = event.get("data", {})
        if event.get("event") == "focus-changed":
            focused = data.get("pane")
        elif event.get("event") == "bell" and data.get("pane") != focused:
            now = time.monotonic()
            if now - last_notification >= 5:
                subprocess.run(
                    ["notify-send", "Rozi", "A background pane rang"],
                    check=False,
                )
                last_notification = now
```

Configure it as a supervised service:

```toml
[[services]]
name = "bell-watch"
run = "~/.config/rozi/scripts/bell-watch.py"
restart = "on-failure"
```

Unlike a hook, a subscriber can keep state and coalesce related events. Event fields are under
`event.data`. See [Hooks](hooks.md#events-and-fields) for the event list.

## Publish a build row

`rozi publish` reads complete JSON row snapshots. This publisher reports whether Cargo is running:

```sh
#!/bin/sh
set -eu

ROZI=${ROZI_BIN:-rozi}
if [ -n "${ROZI_SOCKET:-}" ]; then
    set -- "$ROZI" --socket "$ROZI_SOCKET"
else
    set -- "$ROZI"
fi

publish_rows() {
    while :; do
        if pgrep -x cargo >/dev/null 2>&1; then
            status=working
        else
            status=idle
        fi
        printf '{"rows":[{"id":"build","title":"Cargo build","status":"%s"}]}\n' "$status"
        sleep 2
    done
}

publish_rows | "$@" publish
```

The row belongs to the source pane. A publisher launched in a pane uses `ROZI_PANE`; a supervised
service resolves the focused live pane when its stream opens. A service that needs stable ownership
can start a separate publisher subprocess with `ROZI_PANE` set to a pane ID from `list-panes`.

Nonempty published rows are authoritative for the activity state of an already recognized agent in
that pane, so screen-derived state is not used until the publisher sends an empty snapshot or
disconnects. Publishing from an unrecognized program creates an Activity row but does not invent an
agent identity.

## Make published rows clickable

The publish stream writes activation objects to stdout. This example checks out an activated pull
request in a new pane:

```sh
#!/bin/sh
set -eu

ROZI=${ROZI_BIN:-rozi}
if [ -n "${ROZI_SOCKET:-}" ]; then
    set -- "$ROZI" --socket "$ROZI_SOCKET"
else
    set -- "$ROZI"
fi

produce_rows() {
    while :; do
        gh pr list --json number,title,statusCheckRollup |
            jq -c '{
                rows: map({
                    id: ("pr-" + (.number | tostring)),
                    title: ("#" + (.number | tostring) + " " + .title),
                    status: (
                        if any(.statusCheckRollup[]?; .conclusion == "FAILURE")
                        then "blocked"
                        else "idle"
                        end
                    )
                })
            }'
        sleep 30
    done
}

produce_rows |
    "$@" publish |
    while IFS= read -r message; do
        number=$(printf '%s\n' "$message" | jq -r '.activate // empty' | awk -F- '{print $2}')
        [ -n "$number" ] || continue
        "$@" split --focus --argv gh pr checkout "$number"
    done
```

Keep reading stdout for the lifetime of a publisher. An unread activation backlog causes Rozi to
close the stream and withdraw its rows.

## Package a script as an extension

An extension gives scripts stable command IDs, lifecycle management, and a distributable manifest:

```text
git-tools/
├── extension.toml
└── scripts/
    └── branch-picker
```

```toml
[extension]
id = "git-tools"
title = "Git tools"
version = "0.1.0"
api = 1

[[commands]]
id = "pick-branch"
label = "Pick Git branch"
exec = ["{extension_dir}/scripts/branch-picker"]
```

The command is available as `git-tools.pick-branch`:

```toml
[keys]
"ctrl-a b" = { run = "git-tools.pick-branch" }
```

See [Extensions](extensions.md) for installation, trust, manifests, services, and testing.
