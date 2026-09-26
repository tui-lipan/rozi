# Automation recipes

These recipes are complete scripts that automate rozi through its CLI, pickers, hooks, services,
and published activity. Copy one, adapt it, and see [Control CLI](control.md) for every command and
flag they use.

## Call rozi from a script

A script that rozi launches — from a pane, a key binding, a hook, or a service — should call the
same `rozi` binary and UI that launched it. rozi provides both in `ROZI_BIN` and `ROZI_SOCKET`:

```sh
ROZI=${ROZI_BIN:-rozi}
if [ -n "${ROZI_SOCKET:-}" ]; then
    set -- "$ROZI" --socket "$ROZI_SOCKET"
else
    set -- "$ROZI"
fi
```

After this setup, `"$@" <COMMAND>` runs a rozi command. Each shell recipe below repeats the block
so it can be copied on its own.

## Pick and switch a Git branch

Choose a branch from a picker and switch to it:

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

`rozi pick` reads one row per input line and prints the selected row. It exits `1` when the user
cancels, which `|| exit 0` turns into a quiet exit. See [Pickers](control.md#pickers) for JSON rows
with groups, disabled entries, and actions.

## Open a worktree in a pane

Choose a Git worktree and open a shell in it:

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

Keep `"$worktree"` quoted: picker output is untrusted text, and paths may contain spaces.

## Use Yazi as a file router

Pick a file in [Yazi](https://yazi-rs.github.io/) inside a popup and open it in your editor in a new
pane.

1. Create `~/.config/rozi/scripts/yazi-router`:

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

2. Make it executable with `chmod +x ~/.config/rozi/scripts/yazi-router`.
3. Bind it to `Ctrl+A`, then `Shift+E`:

   ```toml
   [keys]
   "ctrl-a shift-e" = { popup = "~/.config/rozi/scripts/yazi-router", keep_open = false }
   ```

The script writes Yazi's choice to a private temporary file, removes it on exit, and passes the
path to the editor as a separate argument, never as part of a shell command.

The sidebar's file tree follows the same rule: an `on_click` action receives the selected path in
`ROZI_FILE`. Read the variable instead of inserting the path into a command:

```toml
[sidebar]
tabs = [
  { name = "files", label = "", on_click = { run = '''"${EDITOR:-vi}" "$ROZI_FILE"''' } },
]
```

## Watch events

Notify when a background pane rings the bell, at most once every five seconds. Save this as
`~/.config/rozi/scripts/bell-watch.py` and make it executable:

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
                    ["notify-send", "rozi", "A background pane rang"],
                    check=False,
                )
                last_notification = now
```

Run it as a supervised service:

```toml
[[services]]
name = "bell-watch"
run = "~/.config/rozi/scripts/bell-watch.py"
restart = "on-failure"
```

Unlike a [hook](hooks.md), a subscriber keeps state between events: here it remembers the focused
pane and when it last notified. Each event's fields are under `data`; see
[Hooks](hooks.md#events-and-fields) for the event list.

## Run a job in a detached session and collect its output

Run a command in a session nobody is attached to, wait for it to finish, and print its output. This
needs no UI, so it works from cron, CI, or an SSH login, as long as the session (here `dev`) is
running on the same machine.

```sh
#!/bin/sh
set -eu
session=dev

# Fail early if the session is not running.
rozi --session "$session" list-panes >/dev/null

pane=$(rozi --session "$session" split --workspace 9 --title nightly 'cargo test' |
    jq -r '.data.id')

# The job is done when the pane's process exits; list-panes then reports `exited (<CODE>)`.
while status=$(rozi --session "$session" list-panes --format json |
    jq -r --argjson p "$pane" '.data[] | select(.id == $p) | .status'); do
    case "$status" in
        exited*) break ;;
        "") echo "pane $pane disappeared" >&2; exit 1 ;;
    esac
    sleep 5
done

rozi --session "$session" capture-pane --target "$pane" --scrollback full --format text
case "$status" in
    "exited (0)") exit 0 ;;
    *) exit 1 ;;
esac
```

The script exits `0` only if the job did.

- The pane stays in workspace 9 with its scrollback, so a failure is still there to inspect when
  someone attaches. A dedicated workspace also keeps the new pane from re-tiling a workspace
  someone is using.
- `split` fails with `not-controller` while an attached client holds layout control, so run jobs
  like this in a session nobody is working in.
- To run the job on another machine, add `--remote <HOST>` before each `--session`, for example
  `rozi --remote workbox --session dev list-panes`. See [Control CLI](control.md#commands).

## Report a script's progress into a session

Show a long job's progress as the status of the pane it runs in, where the sidebar's Activity list
and the pane border display it:

```sh
ROZI=${ROZI_BIN:-rozi}
pane=${ROZI_PANE:?run this inside a rozi pane, or pass a pane id}
"$ROZI" --session dev status working --target "$pane" --reason "building"
trap '"$ROZI" --session dev status --clear --target "$pane"' EXIT
```

A session endpoint always needs `--target`, because a pane id does not say which session it
belongs to. Pass `$ROZI_PANE` only to the session that pane is in: against any other session, the
same number names a different pane.

## Publish a build row

Show a sidebar Activity row that says whether Cargo is running. `rozi publish` reads complete JSON
row snapshots on stdin:

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

The row belongs to a pane:

- A publisher running in a pane uses that pane, from `ROZI_PANE`.
- A supervised service has no pane, so rozi uses the focused live pane when the stream opens. To
  pin the row to a specific pane, start the publisher with `ROZI_PANE` set to a pane ID from
  `list-panes`.

While a pane has published rows, they decide the displayed state of any agent rozi has detected in
that pane, instead of what is on screen, until the publisher sends an empty snapshot or
disconnects. A publisher in a pane with no detected agent still gets its Activity row.

## Make published rows clickable

List open pull requests as Activity rows, and check one out in a new pane when the user activates
its row. `rozi publish` writes an activation object to stdout for each click:

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

Keep reading stdout for as long as the publisher runs. If unread activations pile up, rozi closes
the stream and withdraws its rows.

## Record a pane or the whole UI as a GIF or video

To record one pane, use [`rozi record`](recording.md). It writes every change with its exact time,
from inside the session server, with or without a UI, and exports frames with an ffmpeg listing:

```sh
rozi --session dev record pane --target 3 --output demo.rozirec   # Ctrl+C to stop
rozi record export demo.rozirec --to png-frames frames --scale 2
ffmpeg -f concat -safe 0 -i frames/frames.ffconcat \
  -vf "split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=none" demo.gif
```

To record everything rozi draws, bar and borders included, record the UI instead. Run these from
a shell inside rozi; the file lands in the directory you run them in:

```sh
rozi record start ui --output ui.rozirec
rozi record stop --ui                       # when you are done
rozi record export ui.rozirec --to png-frames frames --scale 2
ffmpeg -f concat -safe 0 -i frames/frames.ffconcat \
  -vf "pad=ceil(iw/2)*2:ceil(ih/2)*2" -pix_fmt yuv420p -fps_mode vfr ui.mp4
```

Both keep every change with the moment it happened, so the GIF or video plays in real time. See
[Export frames, a GIF, or a video](recording.md#export-frames-a-gif-or-a-video) for the options.

## Package a script as an extension

Turn a script into an [extension](extensions.md) to give it a stable command ID, lifecycle
management, and a manifest you can share. This one packages the branch picker from
[Pick and switch a Git branch](#pick-and-switch-a-git-branch):

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

The command's ID is `git-tools.pick-branch`. Bind it to `Ctrl+A`, then `b`, like any other
action:

```toml
[keys]
"git-tools.pick-branch" = "ctrl-a b"
```

See [Extensions](extensions.md) for installation, trust, manifests, services, and testing.
