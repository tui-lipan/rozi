# Extension test lab

This lab exercises the canonical extensions as installed third-party code. Run it against a
disposable Rozi session and inspect every extension before installing it. The manifests use
Python 3 through the portable `python` spelling; adjust that argv locally if the platform exposes
Python only through another launcher.

## Install the examples

From the Rozi repository on Linux/macOS:

```bash
export ROZI_EXTENSIONS="${XDG_DATA_HOME:-$HOME/.local/share}/rozi/extensions"
mkdir -p "$ROZI_EXTENSIONS"
for extension in git-tools pr-dashboard docker ssh-tools agent-activity; do
  rozi check-extension "examples/extensions/$extension"
  test ! -e "$ROZI_EXTENSIONS/$extension" || {
    echo "refusing to replace $ROZI_EXTENSIONS/$extension" >&2
    exit 1
  }
  cp -R "examples/extensions/$extension" "$ROZI_EXTENSIONS/$extension"
done
rozi run-action reload-config
rozi list-extensions --verbose
```

On Windows, copy the same five directories below
`%LOCALAPPDATA%\rozi\extensions`, validate each path, then reload. Python launcher spellings in the
manifests may need to match the local Python 3 installation.

Expected: all five entries are `loaded`; diagnostics show public IDs, resolved argv, command/service
cwd, injected environment, and no secret environment values.

## Git tools

Prerequisites: Git and Python 3.

```bash
LAB="$(mktemp -d)/rozi git 東京"
mkdir -p "$LAB/repo"
cd "$LAB/repo"
git init
git config user.email rozi-test@example.invalid
git config user.name "Rozi test"
printf 'main\n' > README.md
git add README.md
git commit -m main
git branch feature/ready
git branch old-delete
git worktree add "../worktree feature" feature/ready
printf 'dirty\n' >> README.md
```

Open a pane in `$LAB/repo`, then:

```bash
rozi run-action git-tools.branches
```

Check:

1. current and other branches are grouped and searchable;
2. the current branch is active and protected from deletion;
3. create prompts for input and refreshes the open picker;
4. confirmed delete removes `old-delete` and refreshes;
5. switching with dirty work either succeeds safely or reports Git's exact refusal;
6. Esc cancels without a success toast;
7. `git-tools.worktrees` lists both paths and opens the selected worktree in a focused pane;
8. reloading while the picker is open closes the retired picker cleanly.

## PR dashboard

Prerequisites: Python 3, `gh`, an authenticated GitHub account, and a repository with a pull request.

```bash
gh auth status
gh pr status
rozi run-action pr-dashboard.open
```

Check:

1. focusing a pane in the repository gives the service repository context;
2. a concise Activity row appears for the current pull request/check state;
3. activating the row or running `pr-dashboard.open` opens the checks/actions picker;
4. refresh updates the picker without reopening it;
5. transition a check between pending, success, and failure and verify only meaningful transitions
   notify;
6. focus a non-repository pane and confirm the service degrades to an inert diagnostic rather than
   restarting;
7. temporarily hide `gh` from `PATH` and confirm the dependency failure is clear.

Leave the service running for several minutes. It must remain quiet between bounded refreshes and
must not create one process per Rozi event.

## Docker

Prerequisites: Python 3 and a reachable Docker daemon.

```bash
docker run -d --name rozi-running alpine sleep 600
docker create --name rozi-stopped alpine sleep 600
rozi run-action docker.containers
```

Check:

1. running and stopped groups, image, and status are readable;
2. refresh replaces rows in the open picker;
3. start, stop, and restart update the row in place;
4. inspect/logs open a readable result and shell opens only for a running container;
5. destructive removal requires deliberate confirmation and cannot remove a running container by
   accident;
6. stopping the daemon or hiding `docker` produces a useful diagnostic rather than a traceback.

Cleanup:

```bash
docker rm -f rozi-running rozi-stopped
```

## SSH tools

Prerequisites: Python 3, OpenSSH, and at least two concrete `Host` aliases in `~/.ssh/config`.

```ssh-config
Host rozi-local
  HostName localhost
  User YOUR_USER

Host rozi-other
  HostName example.invalid
  User nobody
```

```bash
rozi run-action ssh-tools.hosts
```

Check:

1. concrete aliases are listed; wildcard-only blocks are not;
2. hostname/user descriptions come from effective SSH configuration;
3. Include files are represented when supported by the local OpenSSH configuration;
4. refresh keeps the picker open;
5. selecting `rozi-local` opens `ssh rozi-local` in a focused pane;
6. a missing config or `ssh` executable gives an actionable empty/error state.

The pane launch API accepts a command line rather than structured argv. This extension only inserts
validated concrete aliases and uses platform-aware quoting; do not broaden the test to hostile
free-form command text.

## Agent activity

Prerequisite: Python 3. No AI provider or API key is required; any pane can simulate the public
status contract.

In one pane:

```bash
rozi status working --reason "implementing parser"
sleep 2
rozi status blocked --reason "permission required"
sleep 2
rozi status done --reason "tests passed"
```

In another:

```bash
rozi run-action agent-activity.open
```

Check:

1. working, blocked, and done panes become stable Activity rows;
2. blocked/done transitions notify once rather than on every refresh;
3. activating a row focuses its pane;
4. the picker groups and describes current reported states and refreshes in place;
5. `rozi status --clear` withdraws the pane after refresh;
6. pane exit removes its row;
7. service restart reconstructs rows from `list-panes` without private state.

## Adversarial lifecycle

Use `pr-dashboard` or `agent-activity`, whose service owns publish and subscribe streams.

### Runtime definition A → B

1. Record the service PID and confirm its Activity row.
2. Change a runtime field in its installed `extension.toml` (argv, service env, cwd, or restart).
3. Run `rozi run-action reload-config`.
4. Confirm a new PID starts and the old Activity rows disappear before the new snapshot arrives.
5. From a saved shell with generation A's environment, try `notify`, `pick`, `publish`, and
   `subscribe`; each must report `extension generation is not active`.

### Metadata-only edit

1. Record the service PID.
2. Change only title, description, version, or a command label.
3. Reload.
4. Confirm the new text appears and the service PID/generation remains unchanged.

### Disable and re-enable

1. Bind one command in `[keys]`.
2. Add its extension ID to `[extensions] disabled`, then reload.
3. Confirm the service terminates, rows disappear, picker/streams close, and
   `rozi run-action <id>` reports the extension command unavailable.
4. Confirm the configured binding remains preserved but inactive.
5. Remove the ID, reload, and confirm the same binding becomes active.

### Invalid manifest

1. Break the installed manifest while its service runs.
2. Reload and confirm the old generation terminates and contributes no rows or commands.
3. Run `rozi list-extensions --verbose`; it must identify the manifest error.
4. Repair, validate, and reload.

### Rename and unusual paths

1. Install an extension under a directory name containing spaces and Unicode.
2. Reload and invoke it.
3. Rename only the installation directory, reload, and confirm public IDs stay stable,
   `ROZI_EXTENSION_DIR` changes, and the process-facing generation rotates.

## Service supervision

For a service configured `restart = "on-failure"`:

1. terminate the child process unexpectedly and confirm one supervised replacement;
2. make startup fail repeatedly and confirm Rozi eventually reports the service dormant;
3. repair the executable or definition, reload, and confirm one fresh backoff generation;
4. disable the extension and confirm no restart remains pending.

## Visual acceptance

At 80×24 and 120×30, inspect each JSON picker for:

- group headers and active/disabled rows;
- long descriptions truncating without hiding labels;
- action hints in the footer;
- prompt replacement and return to the same filter/highlight;
- in-place row refresh;
- Activity titles, status, reason, elapsed time, and activation.

The automated picker and extension smoke tests cover protocol behavior; this pass is for contrast,
spacing, real external-tool text, and the timing of visible lifecycle transitions.

## Authoring friction record

| Class | Evidence | Decision |
| --- | --- | --- |
| A — documentation | Picker first-line/update rules, generation ownership, and command/service cwd were spread across reference prose. | Fixed in the author journey, skill, scaffold README, and this lab. |
| A — documentation | Existing examples read subscribed event fields at the top level even though the wire shape is `{event,data}`. | Fixed the examples and made nesting explicit in control docs and the skill. |
| B — tooling | Authors could validate syntax but could not see full argv, cwd, injected env, or redacted manifest env keys. | Fixed in `check-extension` text and JSON diagnostics. |
| B — tooling | Bare direct executables such as Python were accepted even when absent from `PATH`, leaving failure to invocation/supervision. | Fixed during manifest validation with a command-specific missing-executable error. |
| B — tooling | Starting a valid checkout required hand-writing boilerplate. | Fixed with `new-extension`; no template matrix was needed. |
| B — tooling | A service that could not spawn eventually stopped with no operating-system error, and `restart = "never"` failed silently. | Fixed dormant-service errors to include the launch/exit failure and report non-restarting launch rejection immediately. |
| C — convenience | Worktree/remote helpers needed pane cwd/title options that existed in JSON but not the flat CLI. | Fixed by exposing `--cwd`, `--title`, and `--keep-open` on `split`/`new-pane`; no protocol change. |
| C — convenience | Repeated copy into the user extension directory is awkward. | Deliberately left as copy/clone or an explicit user-created symlink; no cross-platform link manager. |
| C — convenience | Continuous validation would shorten edit cycles. | No `--watch`: reliable cross-platform signal handling/output coalescing is disproportionate, and validation is already fast and side-effect free. |
| D — protocol | SSH and Docker need a pane running external argv, while `new-pane` accepts a shell command line. | Documented, not changed. Current examples constrain/quote machine-derived identifiers; a structured pane-spawn design should be considered only with broader real usage. |
| D — protocol | A service has no initial focused-pane identity until it observes focus or derives context from public pane data. | Left visible in PR dashboard behavior; one example does not justify changing API v1. |
| D — protocol | Picker action replies carried a `selected` field, so the CLI mistook mutations and refreshes for terminal selection. | Fixed the stream bridge to emit action events without exiting; only selection and cancellation terminate a picker. |
| D — protocol | Dropping a wedged publisher on activation backpressure removed its stream but left its last rows visible. | Fixed the failure path to withdraw the pane's rows from the session server immediately. |
| E — extension-specific | Git branch protection policy, GitHub polling cadence, Docker deletion policy, and SSH Include interpretation vary by tool/user. | Kept inside each extension. |

No manifest UI DSL, domain action, mandatory SDK, project-local discovery, package manager, or
permission model is introduced by this lab.

## Architecture evidence

All five canonical extensions use only `extension.toml`, injected process context, and public Rozi
CLI streams. No Rust module, internal state file, test hook, or domain-specific core action is used.
The repeated Python code is ordinary subprocess/NDJSON lifecycle handling; it stayed small relative
to each tool's domain logic and did not produce a common abstraction stable enough to justify an
SDK. A small optional helper can be reconsidered after independent authors repeat the same mistakes.

No new protocol primitive was required. The strongest remaining candidate is structured argv for a
pane spawn, exposed by both Docker and SSH, but those examples remain safe with constrained,
encoded values and platform quoting. Changing pane launch/session identity for that convenience
would be premature during API v1 validation.

The public runtime is ready to accumulate real usage. Package installation may eventually be the
next ecosystem-sized problem, but author feedback on interpreter portability, initial service
context, and pane process launching should come first; none currently warrants more architecture.
