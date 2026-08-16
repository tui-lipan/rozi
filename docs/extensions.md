# Extensions

Extensions are ordinary out-of-process programs with a small discovery and launch manifest. They
use Rozi's public CLI/control protocol at runtime; they do not link to Rozi's Rust internals.

The mental model is deliberately narrow:

```text
extension.toml = identity + commands/services Rozi launches
control protocol = runtime behavior and UI interaction
```

The manifest is not a declarative UI framework. Pickers, published rows, notifications, and event
handling come from `rozi pick`, `publish`, `notify`, and `subscribe`.

## Author journey

Create a working extension before designing its final behavior:

```bash
rozi new-extension my-extension
cd my-extension
rozi check-extension .
```

The generated command is intentionally small but real. Its README is the shortest complete loop:

```text
create → validate → copy/clone into the user extension directory
       → reload-config → list-extensions --verbose → invoke
```

Once that works, add interaction with `pick`, add durable activity with `publish`, and add a
supervised `[[services]]` process only for behavior that must remain alive. Validate after every
manifest edit; reload only when the installed copy should change. The repository-local
[`rozi-extension` agent skill](../.agents/skills/rozi-extension/SKILL.md) is the compressed public
contract for coding agents.

## Structure and identity

```text
rozi-git-tools/
├── extension.toml
├── bin/
│   ├── branches
│   └── watch
└── README.md
```

The stable identity is declared in the manifest, not inferred from the installation directory:

```toml
[extension]
id = "git-tools"
title = "Git tools"
description = "Branches, worktrees, and stashes"
version = "0.1.0"
api = 1
```

An id must match `[a-z0-9_-]+`; dots, whitespace, uppercase, path separators, and Rozi's internal
namespaces are rejected. Moving `rozi-git-tools/` or cloning it under another directory does not
change the public namespace:

| Declaration | Local id | Public id |
| --- | --- | --- |
| extension | `git-tools` | `git-tools` |
| command | `branches` | `git-tools.branches` |
| service | `watch` | `git-tools.watch` |

Two installation directories declaring the same id are both rejected as ambiguous. Rozi never
picks whichever happened to be scanned first.

## Extension API compatibility

`api` is the integer generation of the complete supported extension contract: manifest semantics,
environment, command/service behavior, extension-facing control commands, and lifecycle
guarantees. This release supports API `1`.

Missing, malformed, older, and newer generations are incompatible and contribute nothing. Rozi
does not guess at compatibility. `list-extensions` retains the failed candidate and explains the
generation mismatch.

The extension API is intentionally separate from Rozi's internal Rust APIs. Rust modules and types
may change freely without changing the extension API; the generation changes when the external
contract changes incompatibly.

## Commands and services

```toml
[[commands]]
id = "branches"
label = "Switch branch"
exec = ["python", "{extension_dir}/bin/branches.py", "--all"]

[[services]]
name = "watch"
exec = ["./bin/watch", "--json"]
restart = "on-failure"
```

A command is invoked behavior: it may be keyless, bound under `[keys]`, selected from the palette,
or called with `rozi run-action git-tools.branches`. A service is a long-lived helper supervised
with restart policy and backoff.

`exec` is an argv array and launches directly without a command shell. Use `shell = "..."` only
when shell syntax such as a pipeline is intentional. Extension manifests do not accept the
application config's legacy shell-string `exec`/`run` forms. A command may instead declare
`send = "..."`; each command must declare exactly one action, and each service exactly one of
`exec` or `shell`. A validation error invalidates the extension atomically rather than loading only
a surprising subset.

Hooks are intentionally absent. A service holding `rozi subscribe` retains state and avoids one
process per event. Sidebar/workbar/picker declarations are also absent: runtime UI is expressed
through the protocol.

## Runtime surfaces

Extension programs can compose:

- [`rozi pick`](control.md) — searchable rows, groups, disabled reasons, actions, and text prompts;
- [`rozi publish`](sidebar.md) — live actionable activity rows;
- [`rozi notify`](control.md) — useful off-screen results and failures;
- [`rozi subscribe`](control.md) — a stream of application events;
- [`rozi run-action`](control.md) — built-in, named, and namespaced extension commands.

Use `ROZI_BIN` when launching the matching running binary instead of assuming `rozi` is on `PATH`.
Canonical third-party-style examples exercise distinct surfaces without internal APIs:

- [Git tools](../examples/extensions/git-tools/) — grouped/actionable branch and worktree pickers;
- [PR dashboard](../examples/extensions/pr-dashboard/) — supervised `gh` monitoring with
  subscribe/publish/notify/pick;
- [Docker](../examples/extensions/docker/) — dynamic external-process controls and refresh;
- [SSH tools](../examples/extensions/ssh-tools/) — standard SSH config discovery and pane launch;
- [Agent activity](../examples/extensions/agent-activity/) — generalized pane status mirrored into
  actionable Activity rows.

## Environment and paths

Every extension command and service receives:

| Variable | Value |
| --- | --- |
| `ROZI_EXTENSION` | Stable manifest id, such as `git-tools` |
| `ROZI_EXTENSION_DIR` | Absolute lexical installation directory |
| `ROZI_EXTENSION_GENERATION` | Opaque runtime fencing token |

These names are owned by Rozi. A service manifest attempting to override one is invalid.

`ROZI_EXTENSION_GENERATION` is not authentication. The CLI automatically attaches the id and token
to extension-originated control traffic, and Rozi rejects a retired generation. Its purpose is to
fence a stale process after reload even if process-tree termination fails; another process running
as the same user remains inside the same trust boundary.

`ROZI_EXTENSION_DIR` is lexical rather than canonicalized. A symlink installed as
`extensions/git-tools` therefore reports that installation path, not a surprising target elsewhere.
Moving the directory updates the value on reload while preserving the manifest id.

Relative executable paths beginning `./` or `../` are resolved against the installation directory
at load time. Explicit absolute and `~` paths are normalized too. A command still executes with the
focused pane/project as its working directory. A service defaults its cwd to the installation
directory; its explicit relative `cwd` is resolved there.

Direct argv supports one platform-independent substitution:
`{extension_dir}`. Generic `$VAR`, `${VAR}`, and `%VAR%` expansion is deliberately absent from
direct execution. An explicit `shell` command may use that shell's normal environment syntax.

Symlinked installation directories are supported. Broken links, missing manifests or executable
targets, inaccessible files, and non-UTF-8 installation paths that cannot be represented in the
public environment remain visible as invalid diagnostics.

## Validate and debug

Validate a checkout without installing it:

```bash
rozi check-extension ./rozi-git-tools
rozi check-extension ./rozi-git-tools --json
```

Validation reports all independent manifest, id, API, command, service, environment, and obvious
path errors it can find. For every valid process it also prints the resolved direct argv or explicit
shell command, cwd policy, injected extension environment, restart policy, and configured
environment keys with their values redacted. Failure exits non-zero.

`list-extensions` is the authoritative discovery report:

```bash
rozi list-extensions
rozi list-extensions --verbose
rozi list-extensions --json
```

Normal output shows loaded, disabled, invalid, incompatible, and duplicate candidates. Verbose
output adds installation and manifest paths, API generation, public command/service ids, resolved
executable paths, and every validation error. JSON is a tooling contract with its own
`schema_version` (currently `1`), independent of extension runtime API `1`; `list-extensions`
returns `{ "schema_version": 1, "extensions": [...] }`, while `check-extension` returns one
`extension` field. Public ids such as `git-tools.branches` are exposed, never internal registry
ids. The manifest JSON Schema is [`schemas/extension.schema.json`](../schemas/extension.schema.json).

## Author workflow and reload

```bash
rozi new-extension my-extension
cd my-extension
rozi check-extension .
# edit
rozi run-action reload-config
rozi list-extensions --verbose
```

Rozi deliberately does not watch extension trees because extensions may write their own state
there. A successful explicit reload makes commands, bindings, palette entries, services, open
extension pickers, published rows, and subscriptions agree with the newly valid/enabled set.
Removed services terminate, materially changed services restart once, and unchanged services keep
running. Presentation-only edits such as title, label, description, or package version do not
rotate the runtime generation; process-facing command, service, API, environment, or path changes
do. Returning from revision A to B and back to A creates a new opaque token rather than reviving the
original A generation.

Disable without deleting:

```toml
[extensions]
disabled = ["git-tools"]
```

A portable dotfiles binding may refer to an unavailable extension:

```toml
[keys]
"git-tools.branches" = "g"
```

Rozi preserves the override, warns that it is inactive, and activates it automatically when a
valid compatible extension appears. Disabling or removing the extension deactivates the binding
without rewriting the config.

## Manual installation

There is no package manager or registry. Installing is an explicit clone/copy into the user data
directory:

| Platform | Directory |
| --- | --- |
| Linux/macOS | `$XDG_DATA_HOME/rozi/extensions`, else `~/.local/share/rozi/extensions` |
| Windows | `%LOCALAPPDATA%\rozi\extensions` |

```bash
git clone https://example.com/rozi-git-tools \
  "${XDG_DATA_HOME:-$HOME/.local/share}/rozi/extensions/rozi-git-tools"
rozi check-extension \
  "${XDG_DATA_HOME:-$HOME/.local/share}/rozi/extensions/rozi-git-tools"
rozi run-action reload-config
```

On Windows, clone below `%LOCALAPPDATA%\rozi\extensions` and pass that directory to
`check-extension`.

For local development, an explicit symlink from that user extension directory to a checkout is
supported on platforms where the user can create one. Rozi does not provide `link-extension`:
portable Windows junction/symlink creation and safe removal need materially more platform machinery
than an authoring convenience justifies. Merely entering a project directory never discovers or
executes project-local code.

Use the [manual extension test lab](extension-testing.md) to exercise the canonical Git, PR/CI,
Docker, SSH, and agent-activity extensions plus adversarial reload and fencing cases.

## Trust boundary

Extension code runs with your user account's permissions. Installing an extension is equivalent to
installing software. Rozi validates its public declarations but does not sandbox it and does not
pretend a permissions list would provide isolation.

Project-local `.rozi/extensions/` discovery remains intentionally unsupported: opening an
untrusted checkout must not authorize its code. There is no automatic installer, updater,
dependency resolver, signature system, registry, or trust store.
