# Extensions

An extension is a directory containing `extension.toml` plus any scripts or executables it needs.
It packages the same named commands and supervised services available in `config.toml`; no separate
plugin protocol is involved.

## Location and installation

Extensions live in the user data directory:

| Platform | Directory |
| --- | --- |
| Linux/macOS | `$XDG_DATA_HOME/rozi/extensions`, else `~/.local/share/rozi/extensions` |
| Windows | `%LOCALAPPDATA%\rozi\extensions` |

The directory name is the extension id and must match `[a-z0-9_-]+`. There is no project-local
`.rozi/extensions` directory and no installer in v1. Install by cloning or copying explicitly:

```bash
git clone https://example.com/git-tools \
  "${XDG_DATA_HOME:-$HOME/.local/share}/rozi/extensions/git-tools"
rozi list-extensions
```

`rozi list-extensions [--json]` reports every directory rozi found, its metadata, command and
service counts, and manifest errors.

## Manifest

```toml
[extension]
title = "Git tools"
description = "Branches, worktrees, and stashes"
version = "0.1.0"

[[commands]]
id = "branches"
label = "Switch branch"
exec = "./bin/branches"

[[services]]
name = "watch"
run = "./bin/watch"
restart = "on-failure"
```

`title`, `description`, and `version` are descriptive. Command fields match
[`[[commands]]`](configuration.md#commands); service fields match
[`[[services]]`](configuration.md#services). Hooks are not a manifest surface: a long-lived service
using `rozi subscribe` handles persistent event processing without spawning one process per event.

The directory id namespaces contributions. The example registers command `git-tools.branches` and
service `git-tools.watch`. The command palette groups all commands under the extension title,
falling back to its id.

Relative command programs such as `./bin/branches` are resolved against the extension directory
when the manifest loads. Commands still run with the focused pane's current directory, so a Git
command acts on the repository currently in view. A service defaults its working directory to the
extension directory; an explicit relative `cwd` is resolved there too.

Every contributed command and service receives:

| Variable | Value |
| --- | --- |
| `ROZI_EXTENSION` | Extension directory id |
| `ROZI_EXTENSION_DIR` | Absolute extension directory path |

Disable an extension without removing it:

```toml
[extensions]
disabled = ["git-tools"]
```

After editing a manifest, run `rozi run-action reload-config` or choose **Reload config** from the
command palette. Rozi deliberately does not watch the extension tree because extensions may write
their own state there.

## Trust boundary

An extension is executable code with your user account's permissions. Rozi validates names and
manifest structure, but does not sandbox commands or services. Inspect an extension before cloning
it into the data directory. Repository-owned extensions are intentionally unsupported: merely
opening an untrusted checkout must never authorize its code to run.
