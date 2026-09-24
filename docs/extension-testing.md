# Extension testing

This page shows how to test an extension, including the bundled examples, without touching your own
`rozi` setup. Every test runs in an isolated lab: a temporary home and XDG directories that are
removed afterwards.

A disposable session name alone is not isolation. `rozi` and extensions can write config, data,
state, cache, runtime, and extension-owned files before the session is removed. Never copy
unfinished extensions into your normal extension directory, and never change your normal SSH
config, GitHub CLI config, or other user configuration for a test.

## Set up an isolated lab

From the repository root, in a dedicated shell, build `rozi` and run:

```sh
cargo build

LAB=$(mktemp -d "${TMPDIR:-/tmp}/rozi-extension-lab.XXXXXX")
ROZI_BIN="$PWD/target/debug/rozi"
ROZI_LAB_SESSION="extension-lab-$$"
export LAB ROZI_BIN

cleanup() {
    trap - EXIT HUP INT TERM
    "$ROZI_BIN" sessions kill "$ROZI_LAB_SESSION" >/dev/null 2>&1 || true
    rm -rf "$LAB"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

export HOME="$LAB/home"
export XDG_CONFIG_HOME="$LAB/config"
export XDG_DATA_HOME="$LAB/data"
export XDG_STATE_HOME="$LAB/state"
export XDG_CACHE_HOME="$LAB/cache"
export XDG_RUNTIME_DIR="$LAB/runtime"
export TMPDIR="$LAB/tmp"

mkdir -p \
    "$HOME" \
    "$XDG_CONFIG_HOME/rozi" \
    "$XDG_STATE_HOME" \
    "$XDG_CACHE_HOME" \
    "$XDG_RUNTIME_DIR" \
    "$TMPDIR"
chmod 700 "$XDG_RUNTIME_DIR"
: > "$XDG_CONFIG_HOME/rozi/config.toml"

for extension in \
    git-tools \
    pr-dashboard \
    docker \
    ssh-tools \
    agent-activity \
    activity-dashboard \
    snippets \
    tasks
do
    "$ROZI_BIN" extensions check "examples/extensions/$extension"
    "$ROZI_BIN" extensions install --link "examples/extensions/$extension"
done

"$ROZI_BIN" extensions list --verbose
"$ROZI_BIN" sessions new "$ROZI_LAB_SESSION"
```

This opens a `rozi` UI inside the lab. Run the manual checks below from panes in that UI, then
detach. The setup shell's trap kills the session and removes the whole lab, including after an
interruption.

Every command on this page assumes the lab environment. If you open another shell, first export the
same `HOME`, all five XDG variables (`XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_STATE_HOME`,
`XDG_CACHE_HOME`, `XDG_RUNTIME_DIR`), `TMPDIR`, and `ROZI_BIN`.

On Windows, create a fresh temporary directory and point `USERPROFILE`, `HOME`, `APPDATA`,
`LOCALAPPDATA`, and `rozi`'s config, data, state, cache, and runtime locations at children of it.
Kill the test session and remove the temporary tree in a `finally` block.

## Run the automated checks

Validate every installed extension:

```sh
for extension in "$XDG_DATA_HOME/rozi/extensions"/*
do
    "$ROZI_BIN" extensions check "$extension"
done
```

Run the example unit tests:

```sh
for test_file in examples/extensions/*/tests/test_*.py
do
    python "$test_file"
done
```

Run these inside the lab too. A test that does not touch user directories today may start doing so
later.

## Test Git tools

This needs only Git and Python. Create a repository inside the lab:

```sh
repo="$LAB/git repo"
git init -b main "$repo"
git -C "$repo" config user.name "Rozi test"
git -C "$repo" config user.email "rozi-test@example.invalid"
git -C "$repo" commit --allow-empty -m initial
cd "$repo"
```

From that pane, open the branch picker:

```sh
"$ROZI_BIN" run-action git-tools.branches
```

Check that:

1. `Ctrl+N` creates a branch and refreshes the open picker.
2. `Enter` switches to an eligible branch.
3. A dirty worktree disables branch switching.
4. `Ctrl+D` needs a second press and deletes without forcing.
5. `r` refreshes without closing the picker.
6. `Esc` cancels without an error notification.

Then open the worktree picker:

```sh
"$ROZI_BIN" run-action git-tools.worktrees
```

Create a worktree, open it in a focused pane, make it dirty, and check that removal stays disabled
until it is clean. Keep every repository and worktree below `$LAB`.

## Test SSH discovery

Create an SSH config inside the lab home:

```sh
mkdir -p "$HOME/.ssh/conf.d"
cat > "$HOME/.ssh/config" <<'EOF'
Include conf.d/*.conf

Host local-test
    HostName 127.0.0.1
    User rozi-test

Host *
    ServerAliveInterval 30
EOF

cat > "$HOME/.ssh/conf.d/extra.conf" <<'EOF'
Host extra-test
    HostName 192.0.2.1
EOF

chmod 700 "$HOME/.ssh"
chmod 600 "$HOME/.ssh/config" "$HOME/.ssh/conf.d/extra.conf"
"$ROZI_BIN" run-action ssh-tools.hosts
```

Check that concrete aliases appear, wildcard entries do not, and that editing the included file and
pressing `r` refreshes the open picker. Do not select a row unless you intend to start an SSH
connection.

## Test pane status and activity

These need no external account or service:

```sh
"$ROZI_BIN" status working --reason "run local checks"
"$ROZI_BIN" status blocked --reason "needs input"
"$ROZI_BIN" status done --reason "checks passed"
"$ROZI_BIN" status --clear
"$ROZI_BIN" run-action agent-activity.open
```

Check that one stable row changes status, a repeated blocked status does not repeat its
notification, activating the row focuses the owning pane, and clearing status removes the row.

For the activity dashboard:

```sh
"$ROZI_BIN" run-action activity-dashboard.open
"$ROZI_BIN" status working --reason "local dashboard event"
"$ROZI_BIN" status --clear
```

The dashboard keeps a history file, which is acceptable only because the installed copy is inside
`$LAB`. Reload extensions and check that the history survives the service restart.

## Test reload and service cleanup

Make lifecycle edits only to the installed copy in the lab:

```sh
manifest="$XDG_DATA_HOME/rozi/extensions/activity-dashboard/extension.toml"
```

Check each case:

1. Change a process-facing service field, reload, and confirm the old service and its streams stop.
2. Change only `title`, `description`, or `version`, reload, and confirm the service keeps running.
3. Add the extension ID to `[extensions] disabled` in the lab config, reload, and confirm its
   commands, service, picker, rows, and subscriptions disappear.
4. Make the manifest invalid, reload, and confirm `extensions list --verbose` reports the error and
   the previously loaded version is no longer active.
5. Repair the manifest, validate it, reload, and confirm exactly one service starts.
6. Detach the only client and confirm its services stop.

## Test GitHub integration (opt-in)

This test contacts GitHub and may use API quota, so it is not part of the local pass.

Use an explicit short-lived token in the lab rather than copying or modifying your normal GitHub CLI
configuration:

```sh
export GH_TOKEN
gh auth status
```

Open a pane in a disposable clone below `$LAB`, focus away and back, then run:

```sh
"$ROZI_BIN" run-action pr-dashboard.open
```

Compare the picker with `gh pr status --json …`. Check refresh, browser actions, status transitions,
and that the service stops on detach. Unset `GH_TOKEN` when finished.

## Test Docker integration (opt-in)

This test creates containers in the configured Docker daemon, so it is not part of the local pass.
Use unique names, and add their cleanup to the lab trap before creating them:

```sh
DOCKER_RUNNING="rozi-lab-running-$$"
DOCKER_STOPPED="rozi-lab-stopped-$$"

cleanup_docker() {
    docker rm -f "$DOCKER_RUNNING" "$DOCKER_STOPPED" >/dev/null 2>&1 || true
}
trap 'cleanup_docker; cleanup' EXIT

docker run -d --name "$DOCKER_RUNNING" alpine sleep 600
docker create --name "$DOCKER_STOPPED" alpine sleep 600
"$ROZI_BIN" run-action docker.containers
```

Check grouping, start, stop, restart, inspect, logs, and confirmed removal. Run `cleanup_docker`
before leaving the lab, including after a failed check.

## Check the visuals

At narrow and wide terminal sizes, inspect:

- long paths, Unicode, and punctuation
- group order and disabled reasons
- active, focused, and armed rows
- action hints and prompt transitions
- empty and error states that tell the user something useful
- activity title, status, reason, elapsed time, and activation

## Clean up

Close every picker, popup, and spawned pane, then detach. The setup shell's trap handles the rest:
the session, supervised services, temporary repositories, installed test extensions, runtime files,
and the lab directory.

After an interruption or failure, run the setup shell's `cleanup` function. Do not remove `$LAB`
before stopping its client and session, because services may still have files open.
