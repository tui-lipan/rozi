---
name: rozi-extension
description: >-
  Build and modify Rozi extensions using extension.toml, structured commands,
  supervised services, and the public pick, publish, notify, subscribe, and
  run-action interfaces. Use when creating, debugging, testing, or documenting
  an out-of-process Rozi extension.
---

# Rozi extension authoring

Build against the public manifest, environment, and `rozi` CLI. Never import Rozi Rust modules or
reach into its state files.

## Start here

```bash
rozi new-extension my-extension
cd my-extension
rozi check-extension .
```

A normal extension is an independent directory:

```text
my-extension/
├── extension.toml
├── bin/
│   ├── command
│   └── service
└── README.md
```

`extension.toml` declares identity and what Rozi launches. Runtime UI and behavior belong to the
control protocol.

## Manifest contract

```toml
[extension]
id = "git-tools"
title = "Git tools"
description = "Branch and worktree workflows"
version = "0.1.0"
api = 1

[[commands]]
id = "branches"
label = "Branches…"
exec = ["python", "{extension_dir}/bin/git_tools.py", "branches"]

[[services]]
name = "watch"
exec = ["python", "{extension_dir}/bin/git_tools.py", "service"]
cwd = "."
restart = "on-failure"
```

- Keep the extension `id` stable. IDs match `[a-z0-9_-]+`; do not derive identity from the
  installation directory.
- Use the API generation printed in current docs/scaffolds. An absent or different generation is
  incompatible and contributes nothing.
- Use `exec = ["program", "arg"]` for direct, argument-preserving execution.
- Use `shell = "..."` only when shell syntax is intentional. Never put a shell command string in
  `exec`.
- A command declares exactly one of `exec`, `shell`, or `send`.
- A service declares exactly one of `exec` or `shell`; restart is `on-failure` (default), `always`,
  or `never`.
- `{extension_dir}` is the only substitution in direct argv. Generic `$VAR`, `${VAR}`, and `%VAR%`
  expansion does not occur there.
- `./` and `../` executable paths resolve from the extension directory. Commands run in the focused
  pane's live cwd. Services default to the extension directory; a relative service `cwd` resolves
  there.
- Disable an installed extension with `[extensions] disabled = ["git-tools"]` in `config.toml`.

Manifest command `branches` under extension `git-tools` becomes `git-tools.branches`. Invoke it with:

```bash
rozi run-action git-tools.branches
```

Bind the same public ID:

```toml
[keys]
"git-tools.branches" = "g"
```

## Environment

Every extension command and service receives:

- `ROZI_EXTENSION`: stable manifest ID.
- `ROZI_EXTENSION_DIR`: absolute lexical installation directory.
- `ROZI_EXTENSION_GENERATION`: opaque runtime fencing token.
- `ROZI_BIN`: the matching running Rozi executable when control is available.
- `ROZI_SOCKET`: the current UI endpoint when control is available.

Call `${ROZI_BIN:-rozi}` conceptually, but preserve argv in real code: read `ROZI_BIN` and pass it as
the program to the process API. Do not open `ROZI_SOCKET` directly; the `rozi` CLI bridge handles
Unix sockets and Windows named pipes correctly.

Generation fencing is automatic. Do not copy tokens, recreate current state manually, or retry a
rejected stale generation. Open pickers, subscriptions, and published rows are owned by the
extension generation and close when that generation retires.

## Command or service

```text
command
    invoked behavior, normally short-lived

service
    long-running supervised behavior reacting to events/state
```

Use `[[services]]` instead of daemonizing, backgrounding, or implementing a restart loop. A service
should remain alive while healthy and let Rozi enforce its restart policy.

## Runtime primitives

Use the public binary from `ROZI_BIN`.

### Pick

Plain labels:

```bash
printf '%s\n' main feature/x | rozi pick --title Branch
```

Use `rozi pick --json` for groups, descriptions, disabled/active rows, actions, input prompts, or
live refresh. The first stdin line contains request metadata and optional rows; later lines replace
the complete row set:

```json
{"title":"Branches","actions":[{"id":"new","key":"ctrl-n","label":"new","prompt":"Branch name"}],"rows":[{"id":"main","label":"main","group":"Current","active":true},{"id":"old","label":"old","disabled":"protected"}]}
```

Read `{"selected":"main"}`, `{"action":"new","input":"feat/x","selected":"main"}`, or
`{"cancelled":true}` from stdout. An action stays open unless it declares `"close":true`; send a new
`{"rows":[...]}` line after mutation to refresh in place.

### Publish

Keep `rozi publish` open, write replacement activity snapshots, and read activations:

```json
{"rows":[{"id":"job-1","title":"Run tests","status":"working","active":true}]}
{"activate":"job-1"}
```

Use stable row IDs. An empty row list or closed stream withdraws the rows.

### Notify

```bash
rozi notify "tests failed" --title Build --level error
```

Notify failures and useful off-screen outcomes, not successful state already visible in a picker.

### Subscribe

```bash
rozi subscribe pane-status-changed pane-exited config-reloaded
```

Read newline-delimited `{"event":"pane-status-changed","data":{...}}` objects until the stream
closes. Event fields live under `data`, not at the top level. An empty event list subscribes to all
events.

### Run actions and panes

```bash
rozi run-action reload-config
rozi run-action git-tools.branches
rozi new-pane "ssh devbox" --cwd /project --focus
```

`run-action` uses stable built-in, config-command, or extension IDs. Pane/popup commands are command
lines, not structured argv; quote only validated values and document platform assumptions.

## Development loop

```bash
rozi new-extension my-extension
cd my-extension
rozi check-extension .
# copy or clone below the user extension directory
rozi run-action reload-config
rozi list-extensions --verbose
rozi run-action my-extension.hello
```

Linux/macOS installs live under `$XDG_DATA_HOME/rozi/extensions` or
`~/.local/share/rozi/extensions`; Windows uses `%LOCALAPPDATA%\rozi\extensions`. Manual clone/copy
and explicit development symlinks remain the installation model.

## Debug failures

- Unsupported API: use the generation documented by the running Rozi; do not guess compatibility.
- Invalid or duplicate ID: fix the manifest; IDs are stable namespaces and duplicates are atomic
  failures.
- Executable/path missing: inspect resolved argv and cwd in `rozi check-extension PATH`.
- Service restart loop: run its resolved argv manually with the documented cwd/env keys, then use
  `restart = "never"` while diagnosing if repeated runs are harmful.
- Generation rejected or stream closed after reload: exit; the process belongs to a retired
  generation.
- Command absent: check validation, `rozi list-extensions --verbose`, disabled configuration, and
  then reload.
- No shortcut: commands are keyless by default; invoke the public ID or add `[keys]`.
- Manifest became invalid: the old valid generation is retired on reload; repair and reload again.

## Trust and portability

An installed extension is trusted local executable code. Rozi validates declarations and lifecycle
ownership; it does not sandbox code or enforce capability permissions.

- Prefer structured argv and standard process APIs.
- Avoid shell unless pipelines/redirection are required; never assume `/bin/sh`.
- Treat spaces and Unicode in installation/cwd paths as normal.
- Do not hard-code `/tmp`, `/home`, socket paths, or Rozi's binary path.
- Detect optional tools (`git`, `gh`, `docker`, `ssh`, Python) and report their absence clearly.
- Python launcher names differ (`python3`, `python`, `py -3`); document and test the chosen spelling.
- Use machine-readable external-tool output rather than parsing presentation text.

## Completion checklist

- [ ] `rozi check-extension .` succeeds and shows intended IDs, argv, cwd, and injected environment.
- [ ] Commands work from a path containing spaces and Unicode.
- [ ] Missing optional executables produce a concise notification/error.
- [ ] Picker cancellation, input actions, disabled rows, and refresh behave correctly.
- [ ] Services use supervision, bounded polling, and clean stream shutdown.
- [ ] Runtime-affecting reload retires old streams; metadata-only reload preserves healthy services.
- [ ] Disable/re-enable and invalid-manifest reload behavior were tested.
- [ ] README lists dependencies, installation, invocation, and manual verification.
