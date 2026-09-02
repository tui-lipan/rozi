# Extensions

Rozi extensions are directories containing `extension.toml` and, when needed, out-of-process
programs. They add commands, supervised services, agent definitions, sidebar tabs, and static
navigation targets, and they take settings from the user's `config.toml`. Runtime interaction uses
the same
[`rozi` control commands](control.md) available to scripts.

## Inspect before installing

Extension programs run with your user account's permissions. Rozi does not sandbox them. Installing
an extension is equivalent to installing other software from that source.

Before installation:

1. Obtain the source in a temporary or project directory.
2. Review `extension.toml` and every executable it references.
3. Check direct dependencies and network access.
4. Validate the unpacked directory.

```sh
rozi extensions check ./rozi-git-tools
```

Validation checks the manifest, API, IDs, launch declarations, environment, and executable paths.
It does not make untrusted code safe.

Rozi does not discover project-local `.rozi/extensions` directories. Merely opening a checkout does
not authorize its code.

## Install

Install a reviewed local directory, HTTPS Git remote, or SSH Git remote:

```sh
rozi extensions install ./rozi-git-tools
rozi extensions install https://github.com/user/rozi-git-tools.git
rozi extensions install git@github.com:user/rozi-git-tools.git
```

Rozi validates the extension before installing it. Local directories are copied. Git repositories
are cloned into Rozi-owned storage. In both cases, Rozi uses the manifest ID as the installation
name, rejects an existing destination or conflicting ID, and removes the ID from the disabled list.
Run `rozi run-action reload-extensions` in each running client that should load the new extension.

Git installations keep their original remote and installed commit in private installation
metadata. This information is reserved for explicit lifecycle commands such as a future
`rozi extensions update <ID>`. Installation does not enable background updates.

For extension development, link a checkout instead:

```sh
rozi extensions install --link ./rozi-git-tools
```

Rozi stores only a symlink for a linked extension. Changes in the checkout are visible after an
extension reload. The checkout remains user-owned.

There is no extension registry, dependency resolver, or automatic installer for editor plugins.
The installation directory is private Rozi data. Users do not need to create or edit it.

## List and inspect installed extensions

Open the command palette and choose **Extensions…**. The overlay groups installed extensions by
status. `Enter` enables or disables the selected extension, `Ctrl+D` opens its full report,
`Ctrl+I` opens a source prompt, `Ctrl+U` updates a selected Git-managed extension, `Ctrl+R` rescans
extension manifests, `Ctrl+O` opens `extension.toml`, and `Ctrl+Y` copies the report. Linked
checkouts show `linked`; Git installs whose remote HEAD changed show `update available`.

The install prompt accepts the same local paths and Git HTTPS/SSH URLs as
`rozi extensions install <SOURCE>`. Use the CLI's `--link` option when the checkout must remain
user-owned. The detail view is read-only and wraps long command, path, and diagnostic lines; it
also exposes `Ctrl+U` for Git-managed installations.

Use the CLI when a script or external tool needs the same information:

```sh
rozi extensions list
rozi extensions list --verbose
rozi extensions list --json
```

The report includes loaded, disabled, invalid, incompatible, and duplicate candidates. Verbose
output adds paths, public command, service, agent, and sidebar tab IDs, navigation targets,
resolved executables, and validation errors.

`extensions check --json` and `extensions list --json` are available for tooling:

```sh
rozi extensions check ./git-tools --json
rozi extensions list --json
```

## Disable or remove

Open **Extensions…** and press `Enter` on a loaded extension to disable it. Press `Enter` again to
enable it. Rozi writes the stable ID to `config.toml`, reloads extension contributions, and keeps
the overlay open.

`Ctrl+K` removes the selected installation after a second press. A linked development checkout is
unlinked; Rozi does not delete the checkout the link points to. The CLI accepts the stable
manifest ID:

```sh
rozi extensions remove git-tools
```

Removal deletes local copies and Rozi-owned Git clones. For a linked extension, it deletes only the
symlink. Run `rozi run-action reload-extensions` in running clients after removal.

You can also disable an extension without removing it by editing the stable ID list:

```toml
[extensions]
disabled = ["git-tools"]
```

When the config file is saved, Rozi removes the extension's commands, agents, sidebar tabs, and
navigation targets, stops its services, and closes its owned picker, publisher, and subscription
streams. A disabled extension's sidebar placement is remembered, so re-enabling it puts its tab
back where you had it.

Bindings may refer to an unavailable extension:

```toml
[keys]
"ctrl-a b" = { run = "git-tools.branches" }
```

The binding becomes active when that compatible extension is loaded and inactive when it is
disabled or absent.

## Update

Rozi does not update extensions automatically. Update one Git-managed installation explicitly:

```sh
rozi extensions update git-tools
rozi run-action reload-extensions
rozi extensions list --verbose
```

The command clones the recorded remote into staging, validates it, and replaces the old checkout
only when the new extension is valid. It refuses to replace a managed checkout with local changes.
Copied local extensions and linked development checkouts do not expose update actions.

The Extensions picker checks Git remotes in the background and marks changed installations with
`update available`. `Ctrl+U` runs the same update operation as the CLI. An explicit CLI reload is
required because Rozi does not watch extension directories; picker updates reload the current
client after a successful replacement.

## Create an extension

Create and validate a scaffold:

```sh
rozi extensions new my-extension
cd my-extension
rozi extensions check .
```

An extension normally has this structure:

```text
my-extension/
├── extension.toml
├── bin/
│   └── command
└── README.md
```

Keep generated state outside the installed extension directory unless it is immutable package
data. Use normal user state, cache, or runtime directories for mutable files.

## Write the manifest

The manifest starts with metadata:

```toml
[extension]
id = "my-extension"
title = "My extension"
description = "Project commands"
version = "0.1.0"
api = 1
```

`id` is required, must match `[a-z0-9_-]+`, and must not use a reserved ID. `api = 1` is required.
`title`, `description`, and `version` are optional metadata.

The schema is [`schemas/extension.schema.json`](../schemas/extension.schema.json).

### Navigation targets

An extension can teach Rozi which foreground programs manage their own splits:

```toml
[[navigation_targets]]
name = "vim"
programs = ["vim", "nvim", "view", "vimdiff"]
```

`name` identifies the declaration within the extension and must match `[a-z0-9_-]+`. `programs`
contains executable basenames, not paths. Names are trimmed and matched case-insensitively;
duplicates within one declaration or across built-in and extension targets are harmless.

Rozi validates these declarations when the extension loads and compiles enabled targets into its
in-memory split-aware program set. The extension does not run code, receive key events, or
participate in foreground-process lookup. Disabling or removing it drops its target declarations
on the next extension reload.

An explicit user list wins completely:

```toml
[navigation]
editors = []
```

When `editors` is present, Rozi uses exactly that normalized list and ignores built-in and
extension-provided targets. When it is absent, enabled extension targets augment the built-in list.
See [Split-aware navigation](keybindings.md#split-aware-navigation).

### Commands

```toml
[[commands]]
id = "choose"
label = "Choose item"
exec = ["python", "{extension_dir}/bin/choose.py"]
```

A command ID must match `[a-z0-9_-]+`. Rozi exposes it as
`<extension-id>.<command-id>`, such as `my-extension.choose`.

Each command declares exactly one action:

| Field | Meaning |
| --- | --- |
| `exec = ["program", "arg"]` | Direct argv execution without a command shell. |
| `shell = "command"` | Execution through `command_shell`. Use only when shell syntax is needed. |
| `send = "text"` | Send text to the target pane. |

Prefer `exec`. It preserves argument boundaries and avoids shell interpretation.

Commands run with the focused pane's working directory. Relative executable paths beginning with
`./` or `../` resolve from the extension directory when the manifest loads. Direct argv also
supports `{extension_dir}` inside an argument. It does not expand `$VAR`, `${VAR}`, or `%VAR%`.

Invoke a command from the palette, a key binding, or the CLI:

```sh
rozi run-action my-extension.choose
```

### Services

```toml
[[services]]
name = "watch"
exec = ["python", "{extension_dir}/bin/watch.py"]
restart = "on-failure"

[services.env]
POLL_SECONDS = "30"
```

A service declares exactly one of `exec` or `shell`. Its name must match `[a-z0-9_-]+` and is
exposed as `<extension-id>.<service-name>`.

| Field | Type | Default |
| --- | --- | --- |
| `name` | string | required |
| `exec` | string array | mutually exclusive with `shell` |
| `shell` | string | mutually exclusive with `exec` |
| `cwd` | path string | extension directory |
| `restart` | `on-failure`, `always`, or `never` | `on-failure` |
| `env` | string map | empty |

Services are client-side. They start while a UI with the extension is attached, receive the UI
control environment, and stop when that client detaches or the extension retires. They do not run
in a detached session server.

Use a service for long-lived work such as [`rozi subscribe`](control.md#subscriptions) or
[`rozi publish`](control.md#published-activity). Hooks are not part of an extension manifest.

### Agent definitions

An extension may include `[[agents]]` entries in the same format as
[user agent definitions](agents.md). Rozi namespaces each local ID under the extension ID. An
extension cannot replace a built-in agent. One invalid command, service, agent, sidebar tab,
navigation target, or setting declaration makes the whole extension invalid.

### Settings

An extension declares the settings it understands, with the value each takes when the user says
nothing:

```toml
[settings]
runner = "auto"
rows = 50
notify = true
ignore = ["target", "node_modules"]
```

A setting is a string, integer, boolean, or list of strings. Floats and nested tables are rejected,
and a setting Rozi cannot carry makes the extension invalid.

Users override them per extension in `config.toml`:

```toml
[extensions.tasks]
runner = "just"
rows = 20
```

An undeclared key or a value of the wrong type is reported and ignored; the extension keeps its own
default, so a stale line survives an update that drops a setting. A `[extensions.<id>]` table naming
nothing installed is reported too — being disabled is not enough to earn that warning, since the
settings are waiting for the extension to come back.

Every command and service receives the merged result as compact JSON in `ROZI_EXTENSION_CONFIG`:

```json
{"ignore":["target","node_modules"],"notify":true,"rows":20,"runner":"just"}
```

Changing a setting is a process-facing change: the generation rotates and services restart with the
new value. `rozi extensions check` lists the declared settings and their defaults.

### Default keybindings

A command may suggest a chord. It is written as the key steps *inside* the reserved extension space,
which is the leader prefix followed by `x`:

```toml
[[commands]]
id = "run"
label = "Run task…"
exec = ["python", "{extension_dir}/bin/tasks.py", "run"]
key = "r"
```

That command answers to `<prefix> x r` — `Ctrl+A x r` with the default prefix. Rozi assigns nothing
to `x` itself, so a suggestion can never collide with a built-in and a later Rozi release can never
take one away.

A suggestion is the weakest claim in the system. It loses to anything already bound: a `[keys]`
entry, another extension that asked first, and any chord that merely starts with it, since typing
those steps would fire the other command first. Losing is reported as a warning and costs nothing
else — the command stays in the palette, and the user can bind it by hand:

```toml
[keys]
"tasks.run" = "ctrl-a t"
```

An explicit `[keys]` entry for the command always wins, and silences the suggestion entirely.

### Sidebar tabs

An extension may contribute sidebar tabs with `[[sidebar_tabs]]`. These take the launcher and
command forms [`[sidebar]` tab tables](configuration.md#sidebar) accept, minus the options that only
apply to the built-in `files` and `git` trees.

```toml
[[sidebar_tabs]]
name = "agents"
label = "Agents"
entries = [
  { label = "rozi", group = "claude", run = "cd ~/Projects/rozi && claude" },
  { label = "rozi", group = "codex", run = "cd ~/Projects/rozi && codex" },
]

[[sidebar_tabs]]
name = "worktrees"
label = "Worktrees"
command = "git-tools worktrees --sidebar"
interval = 30
group_prefix = "## "
on_click = { send = "{line}" }
```

| Field | Type | Default |
| --- | --- | --- |
| `name` | string | required, `[a-z0-9_-]+` |
| `label` | string | required |
| `entries` | array of tables | mutually exclusive with `command` |
| `group` (per entry) | string | none |
| `command` | string | mutually exclusive with `entries` |
| `interval` | integer seconds | `30`, minimum `5` |
| `on_click` | action table | none |
| `group_prefix` | string | none, command tabs only |

A tab's `command` and its action strings substitute `{extension_dir}`, and the processes behind them
receive the same `ROZI_EXTENSION*` environment an extension command does. A command tab runs in the
focused pane's working directory and re-lists when that changes; its `on_click` `run`/`popup`/`exec`
receives the clicked row in `ROZI_ROW`.

The tab ID is `<extension-id>.<name>`, so an extension can only add a tab, never replace a built-in
one. A `config.toml` tab claiming the same ID wins and the extension's tab is skipped. Out-of-range
values are clamped silently rather than reported, unlike the same setting in `config.toml`.

Extension tabs are placed in the first panel unless `[sidebar] panels` already names them. Drag them
wherever you like: Rozi persists the arrangement the same way it does for built-in tabs.

A placement naming a tab whose extension is disabled, mid-update, or failing to load is kept, not
warned about, and restored when the extension comes back. It is only dropped once the extension is
gone from the extensions directory, and only the next time you rearrange the sidebar — Rozi never
rewrites the layout on load.

## Stability

Extension API 1 is frozen. Everything below is a contract Rozi will not break inside API 1:

- the manifest keys documented on this page, and the schema at
  [`schemas/extension.schema.json`](../schemas/extension.schema.json);
- namespacing: every contributed id is `<extension-id>.<local-id>`, and an extension can only add,
  never replace a built-in;
- navigation targets are static load-time declarations; core owns foreground-process matching and
  key forwarding, and an explicit `[navigation] editors` list replaces all declarations;
- the `ROZI_EXTENSION*` environment variables and their meanings;
- the `rozi` control commands documented in [Control](control.md), and their exit codes;
- atomic validity: an extension is loaded whole or not at all;
- the `<prefix> x` chord space reserved for extension key suggestions;
- `--json` diagnostics, whose `schema_version` is `1`.

Rozi may still add manifest keys, control commands, and diagnostic fields inside API 1. Additions
are the only compatible change; a manifest that does not use them keeps working. Anything that would
invalidate a working manifest — a removed key, a narrowed value, a changed default, a renamed
environment variable — requires `api = 2`, and an extension declaring `api = 1` keeps loading against
the API 1 rules.

Practically, for an extension author: read `ROZI_EXTENSION_CONFIG` through defaults, do not assume a
suggested chord was granted, and do not depend on undocumented behavior you happened to observe. If
something you need is not on this page or in [Control](control.md), it is not part of the contract.

## Runtime environment

Every extension command and service receives:

| Variable | Value |
| --- | --- |
| `ROZI_EXTENSION` | Stable manifest ID. |
| `ROZI_EXTENSION_DIR` | Absolute lexical installation directory. |
| `ROZI_EXTENSION_CONFIG` | Merged settings as a compact JSON object. `{}` when none are declared. |
| `ROZI_EXTENSION_GENERATION` | Opaque token for the currently loaded process contract. |
| `ROZI_BIN` | Running Rozi executable when available. |
| `ROZI_SOCKET` | Current UI endpoint when available. |

Services may not override the four `ROZI_EXTENSION*` variables.

Use `ROZI_BIN` instead of assuming `rozi` is on `PATH`, and pass `ROZI_SOCKET` back to it:

```sh
"$ROZI_BIN" --socket "$ROZI_SOCKET" notify "extension task finished"
```

The CLI attaches extension identity and generation to control traffic. A retired generation is
rejected. The generation is lifecycle fencing, not authentication from other processes running as
the same user.

See [Scripting](scripting.md) for portable command examples and
[Control protocol](control-protocol.md#stream-ownership) for stream ownership.

## Test and debug

Validate after each manifest edit:

```sh
rozi extensions check .
```

Then use the isolated procedure in [Extension testing](extension-testing.md). Do not test by first
copying unfinished code into the normal extension directory.

For an installed extension:

```sh
rozi run-action reload-extensions
rozi extensions list --verbose
```

If a command fails, run its resolved argv from the verbose validation output with an isolated test
environment. Extension process stdout and stderr are not an interactive debugging channel. Have
the process write deliberate diagnostics to a test-owned file or run it in a test pane.

## Examples

The repository has six canonical examples:

- [Git tools](../examples/extensions/git-tools/) for grouped branch and worktree pickers
- [PR dashboard](../examples/extensions/pr-dashboard/) for a supervised PR monitor
- [Docker](../examples/extensions/docker/) for external process controls
- [SSH tools](../examples/extensions/ssh-tools/) for SSH host discovery and pane launch
- [Agent activity](../examples/extensions/agent-activity/) for mirroring pane status
- [Activity dashboard](../examples/extensions/activity-dashboard/) for general published activity

See [Automation recipes](recipes.md) for smaller building blocks.
