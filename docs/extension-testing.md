# Extension testing

Local extension tests must run with an isolated home and isolated XDG directories. A disposable
session name by itself is not isolation. Rozi can write config, data, state, cache, runtime files,
and extension-owned files before the session is removed.

Do not copy unfinished extensions into your normal extension directory. Do not alter your normal
SSH config, GitHub CLI config, or other user configuration for a test.

## Create an isolated lab

Build Rozi first, then run this setup from the repository root in a dedicated shell:

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
    activity-dashboard
do
    "$ROZI_BIN" extensions check "examples/extensions/$extension"
    "$ROZI_BIN" extensions install --link "examples/extensions/$extension"
done

"$ROZI_BIN" extensions list --verbose
"$ROZI_BIN" sessions new "$ROZI_LAB_SESSION"
```

Run the manual checks from panes in that UI. Detach when finished. The setup shell then kills the
session and removes the whole lab. Its trap also runs after interruption.

Every local test command in this page assumes this environment. If you open another shell, export
the same isolated `HOME`, all five XDG home/runtime variables, `TMPDIR`, and `ROZI_BIN` before
running anything.

On Windows, use a fresh temporary directory and set `USERPROFILE`, `HOME`, `APPDATA`,
`LOCALAPPDATA`, and Rozi's config, data, state, cache, and runtime locations to children of it.
Remove the temporary tree and kill the test session in a `finally` block.

## Run non-interactive checks

Validate every installed copy:

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

These commands still require the lab environment. A test that currently appears not to use user
directories may begin doing so later.

## Test Git tools locally

This flow needs only Git and Python:

```sh
repo="$LAB/git repo"
git init -b main "$repo"
git -C "$repo" config user.name "Rozi test"
git -C "$repo" config user.email "rozi-test@example.invalid"
git -C "$repo" commit --allow-empty -m initial
cd "$repo"
```

From that pane:

```sh
"$ROZI_BIN" run-action git-tools.branches
```

Check that:

1. `Ctrl-N` creates a branch and refreshes the open picker.
2. Enter switches to an eligible branch.
3. A dirty worktree disables branch switching.
4. `Ctrl-D` requires a second press and uses non-forcing deletion.
5. `r` refreshes without closing.
6. Esc cancels without an error notification.

Then run:

```sh
"$ROZI_BIN" run-action git-tools.worktrees
```

Create a worktree, open it in a focused pane, make it dirty, and verify removal remains disabled
until it is clean. All repositories and worktrees must stay below `$LAB`.

## Test SSH discovery locally

Create an SSH fixture under the isolated home:

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

Check that concrete aliases appear, wildcard entries do not, and editing the isolated include then
pressing `r` refreshes the open picker. Do not select a row unless you intentionally want to start
an SSH connection.

## Test pane status and activity

No external account or service is needed:

```sh
"$ROZI_BIN" status working --reason "run local checks"
"$ROZI_BIN" status blocked --reason "needs input"
"$ROZI_BIN" status done --reason "checks passed"
"$ROZI_BIN" status --clear
"$ROZI_BIN" run-action agent-activity.open
```

Confirm that one stable row changes status, duplicate blocked status does not repeat its
notification, activation focuses the owning pane, and clearing status withdraws the row.

For the activity dashboard:

```sh
"$ROZI_BIN" run-action activity-dashboard.open
"$ROZI_BIN" status working --reason "local dashboard event"
"$ROZI_BIN" status --clear
```

Its state file is allowed only because the installed copy is inside `$LAB`. Reload and confirm the
test-owned history survives its service restart.

## Test reload and service cleanup

Perform lifecycle edits only in the isolated installed copy:

```sh
manifest="$XDG_DATA_HOME/rozi/extensions/activity-dashboard/extension.toml"
```

Check these cases:

1. Change a process-facing service field, reload, and confirm the old service and streams retire.
2. Change only title, description, or version, reload, and confirm the service remains running.
3. Add the extension ID to the isolated config's `[extensions].disabled`, reload, and confirm its
   commands, service, picker, rows, and subscriptions disappear.
4. Make the manifest invalid, reload, and confirm `extensions list --verbose` reports the error
   without keeping the old generation active.
5. Repair the manifest, validate it, reload, and confirm one service starts.
6. Detach the only client and confirm client-side services stop.

After interruption or failure, run the setup shell's `cleanup` function. Do not remove `$LAB`
before stopping its client and session because services may still have files open.

## Opt-in GitHub integration

This test contacts GitHub and may consume API quota. It is not part of the local test pass.

Use an explicit short-lived token in the isolated lab instead of copying or modifying normal
GitHub CLI configuration:

```sh
export GH_TOKEN
gh auth status
```

Open a pane inside a disposable clone below `$LAB`, focus away and back, then run:

```sh
"$ROZI_BIN" run-action pr-dashboard.open
```

Compare the picker with `gh pr status --json ...`. Verify refresh, browser actions, status
transitions, and service retirement on detach. Unset `GH_TOKEN` when finished.

## Opt-in Docker integration

This test creates containers in the configured Docker daemon. It is not part of the local test
pass. Use unique names and add their cleanup to the lab trap before creating them:

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

## Visual checks

At narrow and wide terminal sizes, inspect:

- long paths, Unicode, and punctuation
- group order and disabled reasons
- active, focused, and armed rows
- action hints and prompt transitions
- useful empty and error states
- Activity title, status, reason, elapsed time, and activation

Close every picker, popup, and spawned pane before detaching. The outer trap remains responsible for
the session, supervised services, temporary repositories, installed test extensions, runtime files,
and the lab directory.
