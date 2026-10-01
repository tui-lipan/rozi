# Extensions

An extension adds features to `rozi` without changing `rozi` itself. It can contribute commands,
long-running services, sidebar tabs, coding-agent definitions, and suggested keybindings, and it
takes settings from your `config.toml`. The first half of this page is for installing and managing
extensions; the second half is for writing them.

An extension is a directory containing an `extension.toml` manifest and, usually, the programs that
manifest launches. Those programs talk to `rozi` through the same
[`rozi` control commands](control.md) that scripts use.

**Extensions are not sandboxed.** Their programs run with your user account's permissions.
Installing an extension is equivalent to installing any other software from that source.

## Use extensions

### Review an extension before installing

Before installing an extension:

1. Get the source into a temporary or project directory.
2. Review `extension.toml` and every executable it references.
3. Check its direct dependencies and any network access.
4. Validate the unpacked directory:

   ```sh
   rozi extensions check ./rozi-git-tools
   ```

Validation checks the manifest, API version, IDs, launch declarations, environment, and executable
paths. It does not make untrusted code safe.

`rozi` never loads extensions from a project-local directory such as `.rozi/extensions`, so opening
a checkout does not run its code.

### Discover public extensions

Open the command palette and choose **Extensions…**, then switch to the **Discover** tab. It lists
public GitHub repositories that carry the `rozi-extension` topic.

Discover reads an index of those repositories, published at
[`tui-lipan/rozi-extension-index`](https://github.com/tui-lipan/rozi-extension-index). The index
records each repository's metadata at an exact default-branch commit. It is not a package registry:
it hosts no packages, resolves no dependencies, and makes no claim that an entry is audited,
reviewed, safe, or endorsed. Installing from Discover clones the repository at the indexed commit;
from then on it is an ordinary Git installation that updates from its remote.

To install from Discover:

1. Press `Enter` on a row to open its installation report. The report names the repository and
   commit, and shows compatibility, contribution counts, and the trust warning.
2. Optionally press `Ctrl+L` to open the repository in your browser at that exact commit. This is
   the source `rozi` installs, even if the default branch has moved since the index was built.
3. Press `Enter` again to install that commit.

`rozi` validates the complete extension before moving it into its private data directory. While it
installs, a progress modal names the extension, repository, and commit. `Esc` hides the modal
without cancelling; when installation finishes, `rozi` loads the extension and reports the result,
even if the manager was closed. Reopening that entry's report shows the progress again. Only one
installation runs at a time, whether it started from a report or from the install prompt.

Extensions you already have stay listed, marked `installed`, and their report says so instead of
offering to install them again.

`rozi` fetches the index in the background when **Extensions…** opens, so the manager stays
responsive while the request runs or when you are offline. It keeps the last fetched index in its
cache directory and lists it immediately. A cached index up to ten minutes old is used as is; an
older one is refreshed in the background, and a spinner above the hints shows the fetch. If a fetch
fails, the listed entries stay and a row reports the failure. `Ctrl+R` on this tab always refetches.

An index entry that `rozi` cannot validate is left out of **Discover**. Only an unreadable index or
an unsupported index schema version makes discovery unavailable. For a repository that is not in the
index, use the `Ctrl+I` install prompt or the CLI.

To list your own extension, see [Publish to Discover](#publish-to-discover).

### Install an extension

Install a reviewed local directory, HTTPS Git remote, or SSH Git remote:

```sh
rozi extensions install ./rozi-git-tools
rozi extensions install https://github.com/user/rozi-git-tools.git
rozi extensions install git@github.com:user/rozi-git-tools.git
```

Then run `rozi run-action reload-extensions` in each running client that should load it.

`rozi` validates the extension before installing it. It copies a local directory and clones a Git
repository into its own storage. In both cases it:

- uses the manifest ID as the installation name;
- refuses to overwrite an existing installation or a conflicting ID;
- removes the ID from the disabled list.

A Git installation remembers its remote and installed commit, which
[`rozi extensions update`](#update-an-extension) uses later. Installing never enables background
updates. When installation completes, the report summarizes the extension's
[navigation targets](#navigation-targets) and whether each
[suggested keybinding](#suggested-keybindings) is active, conflicting, or suppressed.

The installation directory is private `rozi` data; you never need to create or edit it. `rozi` also
never installs editor plugins or other external integrations on an extension's behalf.

To develop an extension, link your checkout instead of copying it:

```sh
rozi extensions install --link ./rozi-git-tools
```

`rozi` stores only a symlink to a linked extension, and the checkout stays yours. Changes in the
checkout take effect after an extension reload.

### Manage installed extensions

Open the command palette and choose **Extensions…**. The **Installed** tab groups your extensions by
status; the **Discover** tab lists the public index. `Tab`, `Shift+Tab`, `←`, and `→` switch tabs
while the search field keeps focus, and typing filters the active tab.

On the **Installed** tab:

| Key | Action |
| --- | --- |
| `Enter` | Enable or disable the selected extension |
| `Ctrl+D` | Open the full report |
| `Ctrl+I` | Open the install prompt |
| `Ctrl+U` | Check for, or apply, an update to a Git-managed extension |
| `Ctrl+R` | Rescan extension manifests |
| `Ctrl+O` | Open `extension.toml` |
| `Ctrl+K` | Remove the installation (press twice) |

In the report, `Ctrl+Y` copies it, `Ctrl+O` opens `extension.toml`, `Ctrl+U` updates a Git-managed
installation, and `Ctrl+L` opens the manifest's `homepage` when it declares one. The report is
read-only and wraps long command, path, and diagnostic lines.

Rows show where an extension came from: a linked checkout shows `linked`, and a Git installation
with an update shows both versions, such as `0.2.1 → 0.2.2 · git`.

The install prompt accepts the same local paths and Git HTTPS or SSH URLs as
`rozi extensions install <SOURCE>`, and shows the same progress modal. Use the CLI's `--link` option
when the checkout must stay yours.

Scripts and other tools can read the same information from the CLI:

```sh
rozi extensions list
rozi extensions list --verbose
rozi extensions list --json
rozi extensions check ./git-tools --json
```

The list includes loaded, disabled, invalid, incompatible, and duplicate extensions. `--verbose`
adds installation paths; public command, service, agent, and sidebar tab IDs; navigation targets;
resolved executables; and validation errors.

### Disable or remove an extension

Press `Enter` on a loaded extension in **Extensions…** to disable it, and again to enable it. `rozi`
writes the ID to `config.toml`, reloads extension contributions, and keeps the overlay open. You can
also edit the list yourself:

```toml
[extensions]
disabled = ["git-tools"]
```

When the config is saved, `rozi` removes the extension's commands, agents, sidebar tabs, and
navigation targets, stops its services, and closes any pickers, activity rows, and event
subscriptions it opened. Its sidebar tab placement is remembered, so re-enabling it puts the tab
back where you had it.

To remove an installation, press `Ctrl+K` twice in **Extensions…**, or use the CLI with the
manifest ID:

```sh
rozi extensions remove git-tools
```

Removal deletes a copied extension or a `rozi`-owned Git clone. For a linked extension, it deletes
only the symlink, never the checkout it points to. After a CLI removal, run
`rozi run-action reload-extensions` in running clients.

### Bind a key to an extension command

An extension command's public ID is `<extension-id>.<command-id>`. Bind it in `[keys]` like any
other action:

```toml
[keys]
"git-tools.branches" = "ctrl-a b"
```

A binding may name an extension that is not loaded. It becomes active when that extension loads and
inactive while it is disabled or absent, and `rozi` warns that the binding is preserved but
inactive. See [Keybindings](keybindings.md) for key notation.

### Update an extension

`rozi` never updates extensions automatically. Update one Git-managed installation explicitly:

```sh
rozi extensions update git-tools
rozi run-action reload-extensions
rozi extensions list --verbose
```

The update clones the recorded remote into a staging area, validates it, and replaces the old
checkout only if the new version is valid. It refuses to replace a checkout with local changes.
Copied local extensions and linked checkouts cannot be updated this way.

**Extensions…** checks Git remotes in the background whenever it opens or reloads. A muted spinner
beside a row's version shows its check running, and the **Installed** tab counts the updates found.
An outdated row shows `installed → latest`, using the version from the remote `extension.toml`, or a
short commit hash when the remote changed without changing its version. The report's **Update** row
shows the same result, or the error when a remote could not be checked.

On a row with a known update, `Ctrl+U` runs the same update as the CLI and reloads the current
client when it succeeds. On any other Git-managed row, `Ctrl+U` checks the remote again. After a CLI
update, reload explicitly, because `rozi` does not watch extension directories.

## Write an extension

### Create your first extension

Generate a scaffold, validate it, and link it:

```sh
rozi extensions new my-extension
cd my-extension
rozi extensions check .
rozi extensions install --link .
rozi run-action reload-extensions
rozi run-action my-extension.hello
```

`rozi extensions new` requires Python 3 and creates:

```text
my-extension/
├── extension.toml
├── bin/
│   └── hello.py
└── README.md
```

The manifest declares the extension and one command:

```toml
[extension]
id = "my-extension"
title = "my-extension"
description = "A Rozi extension"
version = "0.1.0"
api = 1

[[commands]]
id = "hello"
label = "Hello from my-extension"
exec = ["python3", "{extension_dir}/bin/hello.py"]
```

In outline, `bin/hello.py` calls back into `rozi` through the environment it receives:

```python
import os
import subprocess

rozi = os.environ.get("ROZI_BIN", "rozi")
extension = os.environ["ROZI_EXTENSION"]
subprocess.run([rozi, "notify", f"Hello from {extension}"], check=False)
```

The scaffold picks the Python launcher available on your machine (`python3`, `python`, or `py -3` on
Windows); adjust `exec` if you share the extension with a platform whose launcher differs.

Keep mutable files out of the extension directory. Write state, caches, and runtime files to the
normal user state, cache, or runtime directories; only immutable package data belongs beside the
manifest.

### Extension metadata

Every manifest starts with an `[extension]` table:

```toml
[extension]
id = "my-extension"
title = "My extension"
description = "Project commands"
version = "0.1.0"
api = 1
min_rozi = "0.0.25"
platforms = ["linux", "macos"]
homepage = "https://github.com/you/rozi-my-extension"
```

| Field | Required | Meaning |
| --- | --- | --- |
| `id` | yes | Stable identifier, matching `[a-z0-9_-]+`. |
| `api` | yes | Extension API version. Must be `1`. |
| `title` | no | Display name. |
| `description` | no | One-line summary. |
| `version` | no | Version shown in lists and update reports. |
| `min_rozi` | no | Oldest `rozi` version the extension works with. |
| `platforms` | no | Operating systems it runs on: `linux`, `macos`, `windows`, `freebsd`, `netbsd`. Omit for all. |
| `homepage` | no | An `http` or `https` URL where users can read more. `rozi` displays it but never fetches it. |

The IDs `app`, `command`, `rozi`, `user`, and `workspace` are reserved. Keep `id` stable: every
command, service, tab, and agent the extension contributes is named after it, and users' bindings
and settings refer to it.

`api` must match exactly, while `min_rozi` means "this version or newer". If `min_rozi` is newer
than the running `rozi`, or this machine is not in `platforms`, the extension loads as
**incompatible**, with the reason stated. Nothing runs, and `rozi extensions check` says why.

An unrecognized platform name, a `min_rozi` that is not a version, or a `homepage` that is not an
`http(s)` URL makes the manifest **invalid**, not incompatible, so a typo is reported rather than
silently excluding the extension. When several problems apply, `rozi` reports the most specific:

1. a mismatched `api`;
2. any other manifest error, including those in the fields above;
3. `min_rozi` or `platforms` excluding this machine.

`rozi extensions list --verbose` prints `min_rozi`, `platforms`, and `homepage` when declared,
`rozi extensions check` shows `homepage`, and the `--json` output of both includes all three.

Versions of `rozi` released before these three fields existed reject a manifest that uses them as
an unknown field. To stay loadable by those versions, leave all three out.

The manifest schema is [`schemas/extension.schema.json`](../schemas/extension.schema.json). One
invalid command, service, agent, sidebar tab, navigation target, suggested keybinding, or setting
makes the whole extension invalid; `rozi` loads an extension whole or not at all.

### Commands

A command runs when the user invokes it from the command palette, a key binding, or the CLI:

```toml
[[commands]]
id = "choose"
label = "Choose item"
exec = ["python", "{extension_dir}/bin/choose.py"]
```

```sh
rozi run-action my-extension.choose
```

A command ID must match `[a-z0-9_-]+`. Its public ID is `<extension-id>.<command-id>`, such as
`my-extension.choose`. Each command declares exactly one action:

| Field | Meaning |
| --- | --- |
| `exec = ["program", "arg"]` | Run the program directly with these arguments, without a shell. |
| `shell = "command"` | Run the command line through `command_shell`. Use only when you need shell syntax. |
| `send = "text"` | Send text to the target pane. |

Prefer `exec`: it preserves argument boundaries and avoids shell interpretation.

- Commands run in the focused pane's working directory.
- An executable path starting with `./` or `../` resolves from the extension directory when the
  manifest loads.
- `{extension_dir}` is replaced inside `exec` arguments. `$VAR`, `${VAR}`, and `%VAR%` are not
  expanded.
- An `exec` or `shell` command gets no input, and its output is discarded.
- If a command exits with a non-zero status, `rozi` shows an error notification. A command that has
  already reported its failure with `rozi notify` should exit `0` to avoid a second, vaguer
  message.

A command can also suggest a key; see [Suggested keybindings](#suggested-keybindings).

### Services

A service is a long-running program that `rozi` starts and supervises:

```toml
[[services]]
name = "watch"
exec = ["python", "{extension_dir}/bin/watch.py"]
restart = "on-failure"

[services.env]
POLL_SECONDS = "30"
```

A service name must match `[a-z0-9_-]+`, and its public ID is `<extension-id>.<service-name>`. It
declares exactly one of `exec` or `shell`.

| Field | Type | Default |
| --- | --- | --- |
| `name` | string | required |
| `exec` | string array | mutually exclusive with `shell` |
| `shell` | string | mutually exclusive with `exec` |
| `cwd` | path string | extension directory; a relative path resolves from there |
| `restart` | `on-failure`, `always`, or `never` | `on-failure` |
| `env` | string map | empty |

Services run in the client, not the session server. A service starts while a UI with the extension
is attached, receives that UI's [control environment](#runtime-environment), and stops when the
client detaches or the extension is disabled or removed. A reload that changes a process-facing
field or setting restarts the service; a reload that changes only `title`, `description`, or
`version` leaves it running. Services do not run in a detached session.

Let `rozi` supervise the process rather than daemonizing it or writing your own restart loop. Use a
service for long-lived work such as [`rozi subscribe`](control.md#subscriptions) or
[`rozi publish`](control.md#published-activity). Extensions cannot declare [hooks](hooks.md).

### Settings

Declare the settings an extension understands, each with its default value:

```toml
[settings]
runner = "auto"
rows = 50
notify = true
ignore = ["target", "node_modules"]
```

A setting is a string, integer, boolean, or list of strings. Any other type, including floats and
nested tables, makes the extension invalid. `rozi extensions check` lists the declared settings and
their defaults.

Users override settings per extension in `config.toml`:

```toml
[extensions.tasks]
runner = "just"
rows = 20
```

Every command, service, and sidebar tab process receives the merged result as compact JSON in
`ROZI_EXTENSION_CONFIG`:

```json
{"ignore":["target","node_modules"],"notify":true,"rows":20,"runner":"just"}
```

An undeclared key or a value of the wrong type in the user's config is reported and ignored, and the
extension keeps its default; a leftover line therefore survives an update that drops a setting. A
`[extensions.<id>]` table for an extension that is not installed is also reported, but a disabled
extension's table is not. Changing a setting restarts the extension's services with the new value.

### Suggested keybindings

An extension can suggest keys in two ways. Both are resolved once, when extensions load; key input
still goes through `rozi`'s own command registry, and extension code never sees key events.

#### Suggest a chord for a command

A command's `key` field suggests a chord inside the `<prefix> x` space, which `rozi` reserves for
extensions:

```toml
[[commands]]
id = "run"
label = "Run task…"
exec = ["python", "{extension_dir}/bin/tasks.py", "run"]
key = "r"
```

With the default prefix, that command answers to `Ctrl+A`, then `x`, then `r`. `rozi` binds nothing
to `x` itself, so a suggestion never collides with a built-in and a later release cannot take it
away.

The suggestion loses to anything already bound, including any longer chord it is a prefix of. Losing
produces a warning and nothing else: the command stays in the palette, and the user can bind it
directly. An explicit `[keys]` entry for the command always wins and silences the suggestion:

```toml
[keys]
"tasks.run" = "ctrl-a t"
```

#### Suggest a key for a core action

`[[suggested_keybindings]]` suggests a key for a core action that `rozi` exposes to extensions:

```toml
[[suggested_keybindings]]
action = "smart-focus-left"
key = "ctrl-h"
```

Extension API 1 exposes only `smart-focus-left`, `smart-focus-down`, `smart-focus-up`, and
`smart-focus-right`. Any other action makes the manifest invalid.

`rozi` resolves bindings in this order:

1. explicit `[keys]` configuration, including an empty list that unbinds the action;
2. built-in defaults;
3. extension suggestions.

- Configuring the target action suppresses its suggestion, even when the suggested key is free.
- A key already used by the user or by a default makes the suggestion an inactive conflict.
- Identical suggestions for the same action and key are merged.
- If extensions suggest different actions for the same free key, none of them wins, and each is
  reported as a conflict.
- Different free keys for the same unbound action can coexist.

A conflicting suggestion stays inactive but does not invalidate the extension. Disabling, removing,
or reloading an extension rebuilds the keymap without its suggestions. `rozi extensions list
--verbose` and **Extensions…** show each suggestion's source extension and whether it is active,
suppressed, or conflicting.

### Sidebar tabs

An extension can add [sidebar](sidebar.md) tabs with `[[sidebar_tabs]]`. A tab takes the launcher
and command forms that [`[sidebar]` tab tables](configuration.md#sidebar) accept, except the options
that apply only to the built-in `files` and `git` trees.

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
| `group_prefix` | string | none; command tabs only |

- `{extension_dir}` is replaced in `command` and in action strings, and the processes they start
  receive the same [`ROZI_EXTENSION*` environment](#runtime-environment) as a command.
- A command tab runs in the focused pane's working directory and re-lists when that directory
  changes.
- Every line a command tab prints is a clickable row unless it starts with `group_prefix`, which
  makes it a section header. Print status and empty-state lines with the prefix.
- `on_click` with `send` may use `{line}`. With `run`, `popup`, or `exec`, the clicked row arrives in
  `ROZI_ROW` instead; quote it (`"$ROZI_ROW"`).

A tab's public ID is `<extension-id>.<name>`, so an extension can add tabs but never replace a
built-in one. If a `config.toml` tab has the same ID, it wins and the extension's tab is skipped.
Out-of-range values are clamped without a warning, unlike the same settings in `config.toml`.

Extension tabs go in the first panel unless `[sidebar] panels` already places them. Users can drag
them anywhere, and `rozi` remembers the arrangement as it does for built-in tabs. A placement for a
tab whose extension is disabled, updating, or failing to load is kept silently and restored when the
extension returns. It is dropped only after the extension is gone from disk, and only the next time
the user rearranges the sidebar; `rozi` never rewrites the layout on load.

### Navigation targets

An extension can tell `rozi` which foreground programs manage their own splits, so
[split-aware navigation](keybindings.md#split-aware-navigation) forwards focus keys to them:

```toml
[[navigation_targets]]
name = "vim"
programs = ["vim", "nvim", "view", "vimdiff"]
```

`name` identifies the declaration within the extension and must match `[a-z0-9_-]+`. `programs`
lists executable basenames, not paths. Names are trimmed and matched case-insensitively, without a
Windows `.exe` suffix. Duplicates, within one declaration or across built-in and extension targets,
are harmless.

These declarations are data. `rozi` validates them when the extension loads; the extension runs no
code, receives no keys, and takes no part in detecting the foreground program. Disabling or removing
the extension drops its targets on the next extension reload.

An explicit user list replaces every built-in and extension target:

```toml
[navigation]
editors = []
```

When `editors` is absent, enabled extension targets add to the built-in list.

The [vim-rozi-navigator](https://github.com/tui-lipan/vim-rozi-navigator) repository pairs this
declaration with its Vim and Neovim plugin. Install the `rozi` side directly:

```sh
rozi extensions install https://github.com/tui-lipan/vim-rozi-navigator.git
```

### Agent definitions

An extension can include `[[agents]]` entries in the same format as
[user agent definitions](agents.md), to teach `rozi` to recognize a coding-agent CLI and read its
state. Each agent ID is namespaced as `<extension-id>.<id>`, so an extension can add an agent but
cannot replace a built-in one.

### Runtime environment

Every extension command and service receives:

| Variable | Value |
| --- | --- |
| `ROZI_EXTENSION` | The extension's manifest ID. |
| `ROZI_EXTENSION_DIR` | Absolute installation directory. |
| `ROZI_EXTENSION_CONFIG` | Merged settings as a compact JSON object; `{}` when none are declared. |
| `ROZI_EXTENSION_GENERATION` | Opaque token identifying the currently loaded extension. |
| `ROZI_BIN` | The running `rozi` executable, when available. |
| `ROZI_SOCKET` | The current UI's control endpoint, when available. |

A service's `env` cannot override the four `ROZI_EXTENSION*` variables.

Call `rozi` through `ROZI_BIN` rather than assuming it is on `PATH`, and pass `ROZI_SOCKET` back to
it:

```sh
"$ROZI_BIN" --socket "$ROZI_SOCKET" notify "extension task finished"
```

When `ROZI_EXTENSION` is set, the `rozi` CLI tags each request with the extension's identity and
generation. After the extension is disabled or reloaded, requests from its old processes are
rejected; a process whose request is rejected should exit rather than retry. The generation keeps
stale processes from acting, but it is not authentication: other processes running as the same user
are not blocked. Pickers, activity rows, and subscriptions an extension opens close when its
generation is retired.

A session server cannot check the generation, so it refuses requests from extensions; see
"Extensions and `--session`" in [Control](control.md). For stream ownership, see
[Control protocol](control-protocol.md#stream-ownership), and for portable command examples, see
[Scripting](scripting.md).

### Test and debug

Validate after every manifest edit:

```sh
rozi extensions check .
```

Then test in the isolated environment described in [Extension testing](extension-testing.md).
Do not test by copying unfinished code into your normal extension directory.

After changing a linked or installed extension:

```sh
rozi run-action reload-extensions
rozi extensions list --verbose
```

If a command fails, run its resolved argv from the verbose output yourself, in an isolated test
environment. Because command output is not shown anywhere, have the process write deliberate
diagnostics to a test-owned file, or run it in a test pane.

### Publish to Discover

To list a public GitHub repository in **Discover**:

1. Put `extension.toml` at the repository root.
2. Include `id`, `title`, `description`, `version`, and `api`.
3. Add `min_rozi`, `platforms`, and `homepage` when they clarify compatibility.
4. Add the `rozi-extension` topic to the repository.

The index reads the root `extension.toml` from the default branch and records the exact commit. It
keeps each entry short, and leaves out a repository whose metadata exceeds a limit:

| Field | Maximum length |
| --- | --- |
| `id`, `version`, `min_rozi` | 64 characters |
| `title` | 80 characters |
| `description` | 280 characters |
| `homepage` | 256 characters |

The generated index and its schema are public at
[`tui-lipan/rozi-extension-index`](https://github.com/tui-lipan/rozi-extension-index).

### Stability

Extension API 1 is frozen. Within API 1, `rozi` will not break:

- the manifest keys documented on this page, and the schema at
  [`schemas/extension.schema.json`](../schemas/extension.schema.json);
- namespacing: every contributed ID is `<extension-id>.<local-id>`, and an extension can only add,
  never replace, a built-in;
- navigation targets as static load-time declarations: `rozi` owns foreground-program matching and
  key forwarding, and an explicit `[navigation] editors` list replaces all declarations;
- suggested keybindings: they target only the documented action allowlist, rank below user and
  built-in bindings, and never put extension code on the input path;
- the `ROZI_EXTENSION*` environment variables and their meanings;
- the `rozi` control commands documented in [Control](control.md), and their exit codes;
- atomic validity: an extension loads whole or not at all;
- the `<prefix> x` chord space reserved for extension key suggestions;
- `--json` diagnostics with `schema_version` `1`.

`rozi` may add manifest keys, control commands, and diagnostic fields within API 1; a manifest that
does not use them keeps working. Any change that would invalidate a working manifest — a removed
key, a narrowed value, a changed default, a renamed environment variable — requires `api = 2`, and
an extension declaring `api = 1` keeps loading under the API 1 rules.

In practice:

- read `ROZI_EXTENSION_CONFIG` with your declared defaults as fallbacks;
- do not assume a suggested chord was granted;
- do not depend on behavior that this page and [Control](control.md) do not document.

### Examples

The repository includes these example extensions:

- [Git tools](../examples/extensions/git-tools/) — grouped branch and worktree pickers
- [PR dashboard](../examples/extensions/pr-dashboard/) — a supervised pull request monitor
- [Docker](../examples/extensions/docker/) — container controls
- [SSH tools](../examples/extensions/ssh-tools/) — SSH host discovery and pane launch
- [Agent activity](../examples/extensions/agent-activity/) — mirrored pane status
- [Activity dashboard](../examples/extensions/activity-dashboard/) — general published activity
- [Snippets](../examples/extensions/snippets/) — saved commands pasted into the focused pane
- [Tasks](../examples/extensions/tasks/) — `just`, `make`, and `package.json` tasks, with a sidebar
  tab

See [Automation recipes](recipes.md) for smaller building blocks.
