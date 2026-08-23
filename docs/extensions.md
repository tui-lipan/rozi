# Extensions

Rozi extensions are directories containing `extension.toml` and out-of-process programs. They add
commands, supervised services, and agent definitions. Runtime interaction uses the same
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
rozi check-extension ./rozi-git-tools
```

Validation checks the manifest, API, IDs, launch declarations, environment, and executable paths.
It does not make untrusted code safe.

Rozi does not discover project-local `.rozi/extensions` directories. Merely opening a checkout does
not authorize its code.

## Install

There is no extension registry or automatic installer. Copy or clone the reviewed directory into
the user data directory:

| Platform | Extension directory |
| --- | --- |
| Linux and macOS | `${XDG_DATA_HOME:-$HOME/.local/share}/rozi/extensions/` |
| Windows | `%LOCALAPPDATA%\rozi\extensions\` |

Linux example:

```sh
install_root="${XDG_DATA_HOME:-$HOME/.local/share}/rozi/extensions"
mkdir -p "$install_root"
cp -R ./rozi-git-tools "$install_root/git-tools"
rozi check-extension "$install_root/git-tools"
rozi run-action reload-config
```

The installation directory name is not the extension identity. Identity comes from
`extension.id`. Two installed directories declaring the same ID are both rejected.

## List and inspect installed extensions

```sh
rozi list-extensions
rozi list-extensions --verbose
rozi list-extensions --json
```

The report includes loaded, disabled, invalid, incompatible, and duplicate candidates. Verbose
output adds paths, public command, service, and agent IDs, resolved executables, and validation
errors.

`check-extension --json` and `list-extensions --json` are available for tooling:

```sh
rozi check-extension ./git-tools --json
rozi list-extensions --json
```

## Disable or remove

Disable an extension by stable ID:

```toml
[extensions]
disabled = ["git-tools"]
```

Run `rozi run-action reload-config` after changing the setting. Rozi removes the extension's
commands and agents, stops its services, and closes its owned picker, publisher, and subscription
streams.

To remove an extension, delete its installed directory and reload. Disable it first if you want the
running client to stop its processes before deleting files.

Bindings may refer to an unavailable extension:

```toml
[keys]
"ctrl-a b" = { run = "git-tools.branches" }
```

The binding becomes active when that compatible extension is loaded and inactive when it is
disabled or absent.

## Update

Rozi does not update extensions automatically. Review incoming changes before replacing installed
files. For a Git checkout:

```sh
install_root="${XDG_DATA_HOME:-$HOME/.local/share}/rozi/extensions"
git -C "$install_root/git-tools" fetch --all
git -C "$install_root/git-tools" diff HEAD..origin/main
git -C "$install_root/git-tools" pull --ff-only
rozi check-extension "$install_root/git-tools"
rozi run-action reload-config
rozi list-extensions --verbose
```

An explicit reload is required because Rozi does not watch extension directories. Process-facing
changes rotate the extension's opaque generation and retire old control streams. Metadata-only
changes such as title, description, and package version keep the generation.

## Create an extension

Create and validate a scaffold:

```sh
rozi new-extension my-extension
cd my-extension
rozi check-extension .
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
extension cannot replace a built-in agent. One invalid command, service, or agent definition makes
the whole extension invalid.

## Runtime environment

Every extension command and service receives:

| Variable | Value |
| --- | --- |
| `ROZI_EXTENSION` | Stable manifest ID. |
| `ROZI_EXTENSION_DIR` | Absolute lexical installation directory. |
| `ROZI_EXTENSION_GENERATION` | Opaque token for the currently loaded process contract. |
| `ROZI_BIN` | Running Rozi executable when available. |
| `ROZI_SOCKET` | Current UI endpoint when available. |

Services may not override the three `ROZI_EXTENSION*` variables.

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
rozi check-extension .
```

Then use the isolated procedure in [Extension testing](extension-testing.md). Do not test by first
copying unfinished code into the normal extension directory.

For an installed extension:

```sh
rozi run-action reload-config
rozi list-extensions --verbose
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
