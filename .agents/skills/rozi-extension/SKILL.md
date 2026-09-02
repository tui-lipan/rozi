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
rozi extensions new my-extension
cd my-extension
rozi extensions check .
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

# Optional: register foreground executable basenames that manage their own splits. This is static
# policy compiled by Rozi at load time; no extension process receives navigation keys.
[[navigation_targets]]
name = "vim"
programs = ["vim", "nvim", "view", "vimdiff"]

# Optional: propose a key for an explicitly extension-bindable core action. User bindings and core
# defaults win; conflicts remain visible but do not invalidate the extension.
[[suggested_keybindings]]
action = "smart-focus-left"
key = "ctrl-h"

[[commands]]
id = "branches"
label = "Branches…"
exec = ["python", "{extension_dir}/bin/git_tools.py", "branches"]
# Optional: suggested chord, written as the steps inside the reserved <prefix> x space.
key = "b"

# Optional: settings this extension understands, at their defaults. Users override them in
# [extensions.git-tools]; the merged result arrives as JSON in ROZI_EXTENSION_CONFIG.
[settings]
protected = ["main", "master"]
confirm = true

[[services]]
name = "watch"
exec = ["python", "{extension_dir}/bin/git_tools.py", "service"]
cwd = "."
restart = "on-failure"

# Optional: teach Rozi to recognize a coding-agent CLI in a pane and read its state.
[[agents]]
id = "mytool"
label = "My Tool"
match = { names = ["mytool"], paths = ["@acme/mytool"] }

[[agents.states]]
state = "blocked"
screen = { any_of = ["approve? (a/d)"] }

# Optional: contribute a sidebar tab. Launcher form shown; a command form takes
# command/interval/group_prefix instead of entries.
[[sidebar_tabs]]
name = "agents"
label = "Agents"
entries = [
  { label = "rozi", group = "claude", run = "cd ~/Projects/rozi && claude" },
]
```

- Keep the extension `id` stable. IDs match `[a-z0-9_-]+`; do not derive identity from the
  installation directory.
- Use the API generation printed in current docs/scaffolds. An absent or different generation is
  incompatible and contributes nothing.
- `[[navigation_targets]]` names must match `[a-z0-9_-]+`. Programs are executable basenames, not
  paths. Enabled targets augment built-ins only when the user omitted `[navigation] editors`; an
  explicit list, including `[]`, replaces every built-in and extension target. Declarations are
  data only and never put an extension process on the input path.
- `[[suggested_keybindings]]` may target only the documented extension-bindable actions. User
  configuration (including an explicit unbind) and core defaults win. Identical suggestions
  deduplicate; different actions proposed for the same free key both become visible conflicts.
  Suggestions are resolved at load time and never put extension code on the input path.
- Use `exec = ["program", "arg"]` for direct, argument-preserving execution.
- Use `shell = "..."` only when shell syntax is intentional. Never put a shell command string in
  `exec`.
- A command declares exactly one of `exec`, `shell`, or `send`.
- A service declares exactly one of `exec` or `shell`; restart is `on-failure` (default), `always`,
  or `never`.
- `{extension_dir}` is the only substitution in direct argv, and it is also substituted into a
  `[[sidebar_tabs]]` `command`, entry action, and `on_click` — a tab is neither a command nor a
  service, so nothing else would tell it where its own program lives. Generic `$VAR`, `${VAR}`, and
  `%VAR%` expansion does not occur in argv.
- `./` and `../` executable paths resolve from the extension directory. Commands run in the focused
  pane's live cwd. Services default to the extension directory; a relative service `cwd` resolves
  there.
- `[[agents]]` is declarative data, not a process: no exec, no environment, no path resolution. Ids
  are namespaced `<extension>.<id>`, so an extension can add an agent but never replace a built-in
  one. Rules are evaluated by precedence (blocked, working, idle, unknown), not declaration order;
  scope a `working` rule to `footer` or a transcript quoting its own hints reads as a live run. Full
  format in `docs/agents.md`. Publish rows instead when the program knows its own state.
- `[[sidebar_tabs]]` takes the launcher and command forms `[sidebar]` tab tables accept, minus the
  `files`/`git` tree options. Ids are namespaced `<extension>.<name>`, so a tab can only be added,
  never substituted for a built-in one, and a `config.toml` tab of the same id wins. Out-of-range
  values are clamped silently here rather than warned about. Full format in `docs/extensions.md`.
- A user may drag an extension tab anywhere; that placement is persisted and survives the extension
  being disabled or failing to load, and is only dropped once the extension leaves the disk.
- A tab's processes receive the same `ROZI_EXTENSION*` environment a command does, settings
  included.
- A command tab runs in the focused pane's working directory and re-lists when that changes. Its
  cached rows belong to the directory they were collected in and are dropped when the pane moves, so
  never treat one poll's output as still on screen later.
- Every line a command tab prints is a clickable row unless it starts with `group_prefix`, which
  makes it an inert section header. Print status and empty-state lines with the prefix, or clicking
  one types it into the user's shell.
- `on_click` `send` may substitute `{line}`; `run`, `popup`, and `exec` receive the clicked row in
  `ROZI_ROW` instead and reject `{line}`, the same bargain tree actions make with `ROZI_FILE`. Quote
  `"$ROZI_ROW"`: a row is command output and must never compose a command line. Send no trailing
  newline unless a stray click should execute.
- `[settings]` values are strings, integers, booleans, or string lists. Read them from
  `ROZI_EXTENSION_CONFIG` (compact JSON, `{}` when none are declared), never from a file the
  extension writes into its own directory. A user's unknown key or wrong type is reported and
  ignored, so always code against your declared default being present.
- A command's `key` is a suggestion inside `<prefix> x`, never a bare key and never the held
  modifier. It loses to any existing binding, including a chord it merely extends, and losing is
  a warning rather than a failure. Do not assume it was granted.
- Any invalid command, service, agent, sidebar tab, navigation target, suggested keybinding, or
  setting invalidates the whole extension atomically. A valid suggestion that merely conflicts
  remains inactive without invalidating the extension.
- A non-zero exit from a command raises Rozi's own error toast. If the program already called
  `rozi notify`, exit 0 — otherwise the user gets the specific message and a vaguer duplicate.
- Extensions install under the data directory (`${XDG_DATA_HOME:-~/.local/share}/rozi/extensions`),
  not beside `config.toml`, and that data directory must stay owner-only or Rozi refuses to start.
  `rozi extensions list` names the directory it scanned when it finds nothing.
- Disable an installed extension with `[extensions] disabled = ["git-tools"]` in `config.toml`.
- Extension API 1 is frozen: manifest keys, id namespacing, the `ROZI_EXTENSION*` variables, the
  control commands, atomic validity, the `<prefix> x` chord space, and `schema_version: 1`
  diagnostics. Rozi may add keys inside API 1; anything that would break a working manifest needs
  `api = 2`. Do not rely on behavior that is not in `docs/extensions.md` or `docs/control.md`.

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
rozi run-action reload-extensions
rozi run-action git-tools.branches
rozi split --cwd /project --focus --argv ssh -- devbox
```

`run-action` uses stable built-in, config-command, or extension IDs. Prefer
`split --argv PROGRAM [ARG...]` for pane processes; place pane options before `--argv` because it
consumes the rest. Use a positional command line only when shell syntax is intentional. Popup
commands remain command lines.

## Development loop

```bash
rozi extensions new my-extension
cd my-extension
rozi extensions check .
rozi extensions install --link .
rozi run-action reload-extensions
rozi extensions list --verbose
rozi run-action my-extension.hello
```

Use `rozi extensions install <SOURCE>` for a local directory or Git HTTPS/SSH URL. Rozi owns the
installed copy or clone. Use `--link <PATH>` only for a development checkout that should remain
user-owned. Update a Rozi-managed Git clone with `rozi extensions update <ID>` and remove any
installation with `rozi extensions remove <ID>`. Linked and copied local extensions do not update
from their original source.

## Debug failures

- Unsupported API: use the generation documented by the running Rozi; do not guess compatibility.
- Invalid or duplicate ID: fix the manifest; IDs are stable namespaces and duplicates are atomic
  failures.
- Executable/path missing: inspect resolved argv and cwd in `rozi extensions check PATH`.
- Service restart loop: run its resolved argv manually with the documented cwd/env keys, then use
  `restart = "never"` while diagnosing if repeated runs are harmful.
- Generation rejected or stream closed after reload: exit; the process belongs to a retired
  generation.
- Command absent: check validation, `rozi extensions list --verbose`, disabled configuration, and
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

- [ ] `rozi extensions check .` succeeds and shows intended IDs, argv, cwd, and injected environment.
- [ ] Commands work from a path containing spaces and Unicode.
- [ ] Missing optional executables produce a concise notification/error.
- [ ] Picker cancellation, input actions, disabled rows, and refresh behave correctly.
- [ ] Services use supervision, bounded polling, and clean stream shutdown.
- [ ] Runtime-affecting reload retires old streams; metadata-only reload preserves healthy services.
- [ ] Disable/re-enable and invalid-manifest reload behavior were tested.
- [ ] README lists dependencies, installation, invocation, and manual verification.
