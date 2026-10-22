# Write an extension

This page is for extension authors. It walks through a first extension, then covers the manifest,
commands, services, sidebar tabs, and the environment an extension runs in. To install and manage
extensions, see [Extensions](extensions.md); to test one in isolation, see
[Extension testing](extension-testing.md).

An extension is a directory containing an `extension.toml` manifest and, usually, the programs that
manifest launches. Those programs talk to `rozi` through the same [control commands](control.md)
that scripts use.

## Create your first extension

Generate a scaffold, validate it, link it, and run its command:

```sh
rozi extensions new my-extension
cd my-extension
rozi extensions check .
rozi extensions install --link .
rozi run-action reload-extensions
rozi run-action my-extension.hello
```

`rozi extensions new` prints these steps after it creates the scaffold. `--link` makes this
checkout the installed extension, so after an edit you only run `reload-extensions` again. The
last command shows a "Hello from my-extension" notification in the running `rozi`; run the commands
from a pane in it, or open the command palette and choose **Hello from my-extension**.

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

Keep mutable files out of the extension directory: only immutable package data belongs beside the
manifest. Write anything the extension keeps to its [state directory](#keep-state).

## Extension metadata

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

## Commands

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

Set `hidden = true` to leave a command out of the command palette until the extension shows it;
see [Show a command only when it applies](#show-a-command-only-when-it-applies).

- Commands run in the focused pane's working directory when that pane is on the machine running
  the command. By default a command runs on the client, so in a session attached with `--remote`
  it starts in `rozi`'s own directory: the pane's path belongs to the remote host, even when the
  same path exists locally. To run it on the session's host instead, in that pane's directory
  there, see [Run on the session host](#run-on-the-session-host).
- An executable path starting with `./` or `../` resolves from the extension directory when the
  manifest loads.
- `{extension_dir}` is replaced inside `exec` arguments. `$VAR`, `${VAR}`, and `%VAR%` are not
  expanded.
- An `exec` or `shell` command gets no input. Its output goes to the extension's
  [log](#read-an-extensions-logs), not to a pane.
- Started from the command palette, an `exec` or `shell` command keeps the palette open until it
  exits or opens a [picker](control.md#pickers), so a picker replaces the palette without the panes
  flashing in between. If the command is still running after a moment, the palette shows that it
  is running. `Esc` closes the palette; the command keeps running.
- If a command exits with a non-zero status, `rozi` shows an error notification. A command that has
  already reported its failure with `rozi notify` should exit `0` to avoid a second, vaguer
  message.

A command can also suggest a key; see [Suggested keybindings](#suggested-keybindings).

### Show a command only when it applies

Some commands are worth offering only in some situations, such as an installer whose work is
already done. The extension knows that, so it decides at runtime:

```toml
[[commands]]
id = "install-hooks"
label = "Install hooks"
exec = ["python", "{extension_dir}/bin/install_hooks.py"]
hidden = true
```

```sh
"$ROZI_BIN" show-command install-hooks   # list it in the command palette
"$ROZI_BIN" hide-command install-hooks   # and take it out again
```

- Only the command palette honors this. A hidden command still runs from a key binding and from
  `rozi run-action`.
- An extension shows or hides only its own commands, by manifest `id` or public
  `<extension-id>.<command-id>`, and only from its own commands and services. Any other caller is
  refused.
- The choice lasts as long as the extension's runtime generation, even after the command that
  made it exits. A process-facing change (a changed command, service, or setting) rotates the
  generation and restarts its services, resetting the choice to the manifest's `hidden`. Disabling
  and enabling the extension also starts a new generation. A presentation-only or no-op reload
  keeps both the generation and the choice.
- An older `rozi` rejects an unknown manifest field such as `hidden`. Set `min_rozi` to a release
  with the `command-visibility` capability (see
  [Check the installed API](control.md#check-the-installed-api)).

## Services

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
| `placement` | `client`, `active-session`, `each-host`, or `each-session` | `client` |

By default, services run in the client, not the session server; `placement` runs them on the hosts
of attached sessions instead (see [Run on the session host](#run-on-the-session-host)). A service
starts while a UI with the extension is attached, receives that UI's
[control environment](#runtime-environment), and stops when the client detaches or the extension is
disabled or removed. A reload that changes a process-facing
field or setting, any file in the extension directory other than `extension.toml`, or the
checkout itself (as `rozi extensions update` replaces it) restarts the service; editing only
`title`, `description`, or `version` in place leaves it running. An extension of more than 32 MiB or 4096 files is not checked for file changes. Services do not run in a detached session.

Let `rozi` supervise the process rather than daemonizing it or writing your own restart loop. Use a
service for long-lived work such as [`rozi subscribe`](control.md#subscriptions) or
[`rozi publish`](control.md#published-activity). Extensions cannot declare [hooks](hooks.md).

A service's output, and the reason it stopped, are in its [log](#read-an-extensions-logs). When a
service fails to start or keeps failing, the notification names that log.

An extension's published rows are activities unless its stream opens with `--kind agent`, which
it should do only when each row is an agent's run or conversation. Each extension has its own
publisher identity in a pane, so its rows never replace another publisher's; give a second stream
in the same pane a `--name`. See
[Several publishers in one pane](control-protocol.md#several-publishers-in-one-pane).

A service's stdin stays open while its client runs and reaches end of file once the client is gone,
even if it was killed or crashed. Watch stdin and exit at end of file, so the service does not
outlive a client that could not stop it. In that case, Linux also kills the service process, but
not processes it started, and Windows kills the service with everything it started.

## Run on the session host

By default, an extension's commands, services, and command tabs run on the client, the machine
showing `rozi`. A contribution that works with a session's files, tools, or agents can run on that
session's host instead, with `placement`. When you attach to a remote machine with
[`--remote`](remote.md), a placed contribution runs there, in the remote session's directories and
with its programs, and talks to your client through `rozi` as if it ran locally.

```toml
[[services]]
name = "watch"
exec = ["python3", "./bin/watch.py"]
placement = "each-host"

[[commands]]
id = "open"
label = "Open in project"
exec = ["./bin/open"]
placement = "active-session"
```

| `placement` | Where it runs | Allowed on |
| --- | --- | --- |
| `client` | On the client. The default. | commands, services, command tabs |
| `active-session` | On the host of the session on screen. A service follows that session: switching sessions restarts it beside the new one. | commands, services, command tabs |
| `each-host` | One instance on every host with an attached session, including the client itself when a local session is attached. Several sessions on one host share it. | services |
| `each-session` | One instance per attached session, on that session's host. | services |

A placed contribution runs from a copy of the extension's files, not from an installation on the
host:

- When the extension loads, `rozi` takes a snapshot of its directory and identifies it by a content
  digest. A host receives exactly that snapshot, checks it against the digest, and stores it
  read-only. Nothing needs to be installed on the host except `rozi` itself.
- The snapshot leaves out `.git`, `.hg`, `.svn`, `__pycache__`, and `.pyc` files, and is limited to
  32 MiB and 4096 files. A symbolic link is copied as the file it points to. A link to a directory
  or to anything outside the extension, or a directory past the limits, makes an extension with
  placed contributions invalid.
- Editing a [linked](extensions.md#install-an-extension) extension's files and reloading takes a new snapshot,
  and placed processes restart from it under a new [generation](#runtime-environment).
- The host checks the snapshot again before every launch. If a file has changed or regained write
  permission, the host discards it and the client sends a fresh copy before anything runs.

Placed contributions name their files relative to the extension, because the client's paths mean
nothing on another machine:

- A direct `exec` program is either a name looked up on the host's `PATH` or a `./` path inside the
  extension. An absolute path, `~`, or a path that leaves the extension is invalid.
- `{extension_dir}` and `ROZI_EXTENSION_DIR` name the snapshot's directory on the host.
- A placed service's `cwd` must be a directory inside the extension; it defaults to the extension's
  root. A placed command or command tab runs in the focused pane's directory on the session's host.
  When that directory does not exist there, it runs in the runtime's own directory instead: the
  user's home directory on a remote host.
- `send` commands, and launcher tabs, already act in a pane and take no `placement`.

`platforms` applies to the machine that runs each contribution. An extension limited to Linux can
still load on a macOS client and run its placed contributions on a Linux host; its client
contributions are left out.

### Availability

A placed contribution never falls back to running on the client. When a host cannot run it, `rozi`
shows a notification with the reason, once per reason, and the contribution stays unavailable:

| Reason | Meaning |
| --- | --- |
| `unsupported-platform` | `platforms` excludes the host's operating system. |
| `runtime-unsupported` | The host's `rozi` is too old to run extension processes or to give them a [state directory](#keep-state). Update `rozi` on the host. |
| `missing-executable` | The program is not on the host's `PATH`. Install it, then run `reload-extensions`. |
| `bundle-failed` | The extension's files could not be stored or verified on the host. |
| `runtime-unreachable` | The SSH connection to the host failed or dropped. `rozi` reconnects with backoff. |
| `no-session` | An `active-session` contribution ran with no session attached. |
| `spawn-failed` | The process could not be started for another reason. |

`rozi extensions status` lists every host `rozi` runs placed processes on and every placed process:
where it runs, its generation, the digest of the files it runs from, its process ID, and why
anything is unavailable. `--json` prints the same report, and `rozi extensions list --verbose` and
`--json` include it when a UI is running.

### Lifecycle

For each host, the client starts one extension runtime: in-process for the client's own machine,
and over a separate SSH connection for a remote host. A remote runtime belongs to the client that
started it. It ends, with everything it runs, when the client detaches the host's last session,
exits, or loses the connection. Two clients attached to the same host each run their own. Nothing
placed keeps running in a detached session; contributions that must, such as
[agent definitions](#agent-definitions), come from extensions installed on the host itself.

A placed service follows its `restart` policy like a client one. Disabling, removing, or changing the
extension stops its placed processes on every host.

### What a placed process may do

A placed process uses the normal `rozi` commands through `ROZI_BIN` and `ROZI_SOCKET`, including
streams such as `rozi publish`, `rozi sidebar publish`, `rozi subscribe`, and `rozi pick`. Each process receives a
credential in `ROZI_EXTENSION_CREDENTIAL` that tells the client which extension, generation, and
host it was started for. The `rozi` CLI sends it automatically; never pass it on or log it. The
client ignores any other identity a request claims, and refuses a credential that was revoked, that
belongs to a retired generation, or that arrives from a different host.

What the client honors depends on the command:

- `pick`, `notify`, and `show-command`/`hide-command` work anywhere. Pickers and notifications from
  a remote host name that host.
- `sidebar publish` works anywhere, into the extension's own stream tabs. Its rows are headed with
  the process's host and, for a session-bound placement, its session.
- Commands that act on panes, such as `list-panes`, `capture-pane`, `send-text`, `send-keys`,
  `split`, `focus`, `publish`, `agents list`, and `agents prompt`, apply only while the session on
  screen is on the process's host (and, for an `active-session` or `each-session` placement, is its
  session).
  Otherwise they fail with `out-of-scope`, and `list-panes` and `agents list` return nothing. Panes
  of the scratchpad and popups, which always run on the client, are never reachable.
- `run-action` runs only the extension's own `active-session` commands.
- Anything that could start a process on the client or read the whole client fails with
  `not-permitted`: other actions, popups, UI captures, recordings, pane logging, metrics,
  `extensions status`, `agents list --all`, `agents go`, and `agents report`/`release`.
- `subscribe` receives only events that happen while a session within the process's binding is on
  screen.

These rules apply to placed processes on the client's own machine too, so a placed contribution
behaves the same wherever it runs.

### Security

Placement is not a sandbox. The rules above limit what a placed process may ask `rozi` to do. The
process itself runs as the user on its host, like any program they start, and can do anything their
account can there. An extension of any placement is software the user trusts; see
[Extensions](extensions.md).

The trust boundary is the host, not the process. Any process running as the same user on a host can
read the credentials of the placed processes there, and can change their files, so a credential
does not tell one process on a host from another. What it does is tie a request to one host, one
extension, and one live generation: it stops a retired or restarted process from acting, keeps one
host from speaking for another, and attributes requests to the extension that made them.

Treat a remote host as able to do anything the placed processes on it together may do, as
described above, and nothing more. That is what the client enforces, whatever runs on the host:
every request is checked against those rules, a host's runtime can only answer for the processes it
was asked to start, the resources a host can make the client hold are bounded, and nothing a host
sends can start a process on the client. A host that breaks the protocol or goes past a bound is
disconnected.

The snapshot's digest checks integrity rather than security. Its read-only copy protects against
accidental changes, and verification before every launch catches any change, deliberate or not,
but a process on the host that can change the snapshot can also change what runs there in other
ways.

Placed contributions need `rozi` with the `remote-extension-runtime` and `extension-state-dir`
capabilities on the client and on every host they run on. Set `min_rozi` to a release that has it (see
[Check the installed API](control.md#check-the-installed-api)); an older `rozi` rejects the
`placement` key.

## Settings

Declare the settings an extension understands, each with its default value:

```toml
[settings]
runner = "auto"
rows = 50
notify = true
ignore = ["target", "node_modules"]
```

A setting is a string, integer, boolean, or list of strings. Any other type, including floats, makes
the extension invalid. `rozi extensions check` lists the declared settings and their defaults.

To say how a setting reads in rozi's [tab settings](sidebar.md#tab-settings), declare it as a table
with its `default` and any of these fields:

```toml
[settings]
group = { default = "status", label = "Group by", choices = ["status", { value = "repo", label = "Repository" }] }
rows = { default = 50, label = "Rows", description = "Rows kept per job", min = 1, max = 500 }
done = { default = false, label = "Show finished" }
```

| Field | Applies to | Meaning |
| --- | --- | --- |
| `default` | every setting | Required. The value when the user sets none, and the setting's type. |
| `label` | every setting | The setting's name in the picker. Defaults to the key. |
| `description` | every setting | One line shown under the list while the setting is highlighted. |
| `choices` | strings | The only values allowed: each a string, or `{ value, label }` to show other words. `default` must be one of them. |
| `min`, `max` | integers | Bounds. `default` must lie within them. |

A table with an unknown field, choices on a non-string, bounds on a non-integer, or a default
outside its choices or bounds makes the extension invalid. The value your processes receive is the
same either way. An older `rozi` rejects the table form; set `min_rozi` to a release with the
`sidebar-tab-settings` capability (see [Check the installed API](control.md#check-the-installed-api)).

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

An undeclared key, a value of the wrong type, or a value outside its `choices` in the user's config
is reported and ignored, and the extension keeps its default; a leftover line therefore survives an
update that drops a setting. An integer outside its `min` and `max` is clamped. A
`[extensions.<id>]` table for an extension that is not installed is also reported, but a disabled
extension's table is not. Changing a setting restarts the extension's services with the new value.

## Suggested keybindings

An extension can suggest keys in two ways. Both are resolved once, when extensions load; key input
still goes through `rozi`'s own command registry, and extension code never sees key events.

### Suggest a chord for a command

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

### Suggest a key for a core action

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

## Sidebar tabs

An extension can add [sidebar](sidebar.md) tabs with `[[sidebar_tabs]]`. A tab takes the launcher,
command, and stream forms that [`[sidebar]` tab tables](configuration.md#tab-tables) accept, and can
offer some of the extension's settings with `settings`; see [Tab settings](#tab-settings).

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
| `entries` | array of tables | exactly one of `entries`, `command`, or `stream = true` |
| `group` (per entry) | string | none |
| `command` | string | exactly one of `entries`, `command`, or `stream = true` |
| `format` | `lines` or `json` | `lines`; command tabs only |
| `stream` | bool | `false` |
| `interval` | integer seconds | `30`, minimum `5` |
| `on_click` | action table | none |
| `group_prefix` | string | none; line command tabs only |
| `placement` | `client` or `active-session` | `client`; command tabs only |
| `settings` | array of setting keys | none |

- `{extension_dir}` is replaced in `command` and in action strings, and the processes they start
  receive the same [`ROZI_EXTENSION*` environment](#runtime-environment) as a command.
- A command tab runs in the focused pane's working directory and re-lists when that directory
  changes. Like a command, it runs on the client unless it has a `placement`, so by default it uses
  no pane directory in a `--remote` session.
- `placement = "active-session"` runs a command tab's `command`, and an `on_click` `exec`, on the
  host of the session on screen; see [Run on the session host](#run-on-the-session-host).
- Every line a command tab prints is a clickable row unless it starts with `group_prefix`, which
  makes it a section header. Print status and empty-state lines with the prefix.
- `on_click` with `send` may use `{line}`. With `run`, `popup`, or `exec`, the clicked row arrives in
  `ROZI_ROW` instead; quote it (`"$ROZI_ROW"`).
- A command tab with `format = "json"` prints a tree of rows; a clicked row's `id` also arrives in
  `ROZI_ROW_ID`. See [JSON command tabs](sidebar.md#json-command-tabs).

### Tab settings

`settings` lists [settings](#settings) the tab offers in its settings picker, which users open with
`Ctrl+E` in **Sidebar tabs…** or **Sidebar tab settings…** in the command palette. The picker lists
them in this order, edits them in `[extensions.<id>]`, and puts one back to its default with
`Ctrl+R`:

```toml
[settings]
group = { default = "status", label = "Group by", choices = ["status", "repo"] }
done = { default = false, label = "Show finished" }

[[sidebar_tabs]]
name = "jobs"
label = "Jobs"
stream = true
settings = ["group", "done"]
```

A boolean shows as `Enabled` or `Disabled`, a string with `choices` as a list of its values, and an
integer or other string as a typed value. Each key must be a declared setting, listed once, and not
a list of strings; otherwise the extension is invalid. Several tabs may offer the same setting. An
older `rozi` rejects `settings` on a tab, as it does the table form above.

A change is the same as the user editing `config.toml`: it reaches every process in
`ROZI_EXTENSION_CONFIG`, so a command tab's next run reads it and services restart with it. Read the
value there rather than tracking it per tab.

### Stream tabs

A stream tab's rows come from the extension's own processes, usually a service, through
[`rozi sidebar publish`](control.md#sidebar-streams). The process names the tab by its local
`name`; it cannot publish to another extension's tab or to one in `config.toml`.

```toml
[[sidebar_tabs]]
name = "machines"
label = "Machines"
stream = true

[[services]]
name = "machines"
exec = ["{extension_dir}/machines-watch"]
placement = "each-host"
```

```sh
# machines-watch: one snapshot per change, and read what the user did on stdout.
rozi sidebar publish --tab machines
```

A tab can have several producers at once. With `placement = "each-host"`, every host's instance
publishes its own rows to the same tab, and the tab heads each with its host, so a row called
`main` on two machines stays two rows. With `each-session` or `active-session`, the heading also
names the session. Each producer's rows, actions, and events are its own: a click on a host's row
is reported only to that host's instance, and an instance that stops takes only its rows with it.
A producer restarted in the same place replaces its earlier rows. Several processes in one place
tell themselves apart with `--name`.

Stream tabs take no `placement`; place the services that publish to them instead.

A tab's public ID is `<extension-id>.<name>`, so an extension can add tabs but never replace a
built-in one. If a `config.toml` tab has the same ID, it wins and the extension's tab is skipped.
Out-of-range values are clamped without a warning, unlike the same settings in `config.toml`.

An extension tab may declare `suggested_location = { dock = "right", panel = 2 }`, with a one-based
panel number from 1 to 3. An explicit saved placement wins. Tabs without a suggestion start in the
first panel of the left dock, or of the right dock when only it holds tabs. The user's hidden state and placement survive disabling, failed loads, updates,
removal, and reinstallation. Unavailable tabs remain listed in **Sidebar tabs…**.

Extensions may also declare `[[sidebar_presets]]` with `name`, `label`, and `layout`. The layout uses
the same `left`, `right`, and `hidden` fields as `[sidebar] layout`. Names of this extension's tabs
are automatically namespaced inside panel tab lists. Preset names become `<extension>.<name>`.
Users select and apply a preset through **Settings… → Bars → Sidebar layout preset…**; installing
or reloading an extension never applies one. Applying copies the preset into user preferences, preserving saved
assignments omitted by the preset. Presets leave client visibility and startup preferences unchanged.
Later edits belong to the user.

```toml
[[sidebar_presets]]
name = "review"
label = "Review workspace"
layout = { right = { panel_count = 1, panels = [{ weight = 1.0, tabs = ["agents"] }] } }
```

## Navigation targets

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

## Agent definitions

An extension can include `[[agents]]` entries in the same format as
[user agent definitions](agents.md), to teach `rozi` to recognize a coding-agent CLI and read its
state. Each agent ID is namespaced as `<extension-id>.<id>`, so an extension can add an agent but
cannot replace a built-in one.

## Link handlers

Declare `[[link_handlers]]` to route terminal clicks and uppercase hint selections to one of your
extension's commands:

```toml
[[commands]]
id = "open"
exec = ["python3", "{extension_dir}/bin/open.py"]

[[link_handlers]]
schemes = ["http", "https"]
command = "open"
```

`command` is the local command ID, not a namespaced ID. It must name a process command in this
manifest; `send` commands and commands belonging to another extension are invalid. Empty or invalid
URI schemes invalidate the whole extension. Matching ignores case. User handlers precede extension
handlers; extensions are ordered by installation directory name and handlers by declaration order.

The command receives the link context documented in [Link handlers](configuration.md#links-handlers),
including `ROZI_URL`, `ROZI_FILE`, `ROZI_HINT`, `ROZI_SOURCE_PANE`, and `ROZI_SOURCE_SESSION`.
Its ordinary extension settings and generation fencing still apply. A command placed on the active
session receives the same context on that host and starts in the source pane's directory.
Command references resolve at invocation time, so disabling, removal, or invalid-manifest reload
cannot leave a callable stale handler. These are static declarations; no extension runs on mouse
movement or terminal rendering. Check `rozi api describe` for `extension-link-handlers` before
relying on this additive extension API 1 feature.

## Keep state

Every extension process receives `ROZI_EXTENSION_STATE_DIR`, a directory where the extension can
keep files between runs. `rozi` creates it before starting the process, readable only by the user.
It is the same directory for every process of the extension on one machine: commands, services,
command tabs, link handlers, and [placed](#run-on-the-session-host) processes.

```python
import json, os
from pathlib import Path

path = Path(os.environ["ROZI_EXTENSION_STATE_DIR"]) / "history.json"
history = json.loads(path.read_text()) if path.exists() else []
```

The directory belongs to the extension ID and the machine, and nothing else:

- It survives updates, reloads, disabling and enabling, reinstalling, and removal.
  `rozi extensions remove <id> --purge` deletes it on the machine where you run it.
- It is never copied, never part of a placed contribution's snapshot, and never moved between
  machines. A placed process uses the state directory of the host it runs on, so an
  `active-session` contribution keeps its state on the host of the session it ran for, not with you.
  Attaching to another host gives it that host's state.
- Every process of the extension on a machine shares it, including `each-session` instances and
  processes started by different `rozi` clients. Use your own locking or atomic writes (write a
  temporary file, then rename it) when more than one process may write.
- To keep state per session, name files after `ROZI_SESSION_NAME`, the name of the session the
  process works for; see [Runtime environment](#runtime-environment) for which processes have one.

Settings stay in `[settings]` and `ROZI_EXTENSION_CONFIG`; the state directory is for what the
extension itself writes.

## Runtime environment

Every extension command and service receives:

| Variable | Value |
| --- | --- |
| `ROZI_EXTENSION` | The extension's manifest ID. |
| `ROZI_EXTENSION_DIR` | Absolute installation directory. |
| `ROZI_EXTENSION_STATE_DIR` | The extension's [state directory](#keep-state) on this machine. |
| `ROZI_EXTENSION_CONFIG` | Merged settings as a compact JSON object; `{}` when none are declared. |
| `ROZI_EXTENSION_GENERATION` | Opaque token identifying the currently loaded extension. |
| `ROZI_BIN` | The running `rozi` executable, when available. |
| `ROZI_SOCKET` | The current UI's control endpoint, when available. |

A [placed](#run-on-the-session-host) contribution receives the same variables, with these
differences:

| Variable | Value |
| --- | --- |
| `ROZI_EXTENSION_DIR` | The extension's snapshot directory on the host that runs it. |
| `ROZI_EXTENSION_STATE_DIR` | The extension's state directory on the host that runs it. |
| `ROZI_BIN` | The host's `rozi` executable. |
| `ROZI_SOCKET` | The host's extension runtime, which relays requests to your client. |
| `ROZI_EXTENSION_CREDENTIAL` | The process's own credential. Never pass it on or log it. |
| `ROZI_SERVICE` | The service's public ID, for a placed service. |

Client `exec` commands receive `ROZI_SOURCE_PANE` and `ROZI_SOURCE_SESSION` for the focused pane
and its session instance. Link handlers receive these variables for the source pane, along with
the [link context](configuration.md#links-handlers), for both client and placed commands.
Ordinary `run` and `popup` commands and services do not receive these variables automatically.

`ROZI_SESSION_NAME` holds the name of the session a process works for. Client `exec` commands, link
handlers, and command tabs receive the session on screen, and an `active-session` or
`each-session` contribution receives its own session. It is empty when no session is attached.
Services that run on the client and `each-host` services serve no single session and do not
receive it. Renaming a session restarts its `active-session` and `each-session` services with
the updated name.

For a session attached with `--remote`, `run` panes outside the scratchpad and all `popup`
commands open on the remote host. Neither receives `ROZI_EXTENSION_STATE_DIR`. A `run` pane in
the scratchpad opens on the client and receives the client's state directory.

A service's `env` cannot override the `ROZI_EXTENSION*` variables, and the runtime replaces any
`ROZI_BIN`, `ROZI_SOCKET`, or `ROZI_EXTENSION_CREDENTIAL` a placed service's `env` sets.

Call `rozi` through `ROZI_BIN` rather than assuming it is on `PATH`, and pass `ROZI_SOCKET` back to
it:

```sh
"$ROZI_BIN" --socket "$ROZI_SOCKET" notify "extension task finished"
```

When `ROZI_EXTENSION` is set, the `rozi` CLI tags each request with the extension's identity and
generation. After the extension is disabled or reloaded, requests from its old processes are
rejected; a process whose request is rejected should exit rather than retry. For a client
contribution, the generation keeps stale processes from acting, but it is not authentication: other
processes running as the same user are not blocked. A placed contribution's requests carry its
credential instead, which ties them to its host and generation; see
[Security](#security). Pickers, activity rows, and subscriptions an extension
opens close when its generation is retired.

A session server cannot check the generation, so it refuses requests from extensions; see
"Extensions and `--session`" in [Control](control.md). For stream ownership, see
[Control protocol](control-protocol.md#stream-ownership), and for portable command examples, see
[Scripting](scripting.md).

## Test and debug

Validate after every manifest edit:

```sh
rozi extensions check .
```

Besides errors, `check` prints hints: the likely fix for a common mistake, such as a misspelled
key, `exec` written as a string, or a script path that a command would look for in the focused
pane's directory instead of the extension's. A hint does not make an extension invalid, so fix a
hinted manifest even when it loads. `check --json` lists them in `hints`.

The report ends with **Discover**: whether the public index would list the extension, or what stops
it. This checks catalog eligibility separately from install validity, so **Ready** can appear
alongside errors in commands or services. Fix those errors before publishing. `check --json` reports
the eligibility result in `discover`, as `ready` and `problems`. See
[Publish to Discover](#publish-to-discover).

Then test in the isolated environment described in [Extension testing](extension-testing.md).
Do not test by copying unfinished code into your normal extension directory.

After changing a linked or installed extension:

```sh
rozi run-action reload-extensions
rozi extensions list --verbose
```

For [placed](#run-on-the-session-host) contributions, `rozi extensions status` shows where each one
runs and why one is unavailable.

If a command fails, `rozi` shows a notification that names the extension's log. To reproduce it
yourself, run the resolved argv from the verbose output in an isolated test environment.

### Read an extension's logs

What a command, a service, or a placed process writes to standard output or standard error is kept
in a log, with a timestamped line when the process starts and when it ends. A service that `rozi`
stops, because its extension was reloaded, disabled, or removed, is recorded as stopped:

```sh
rozi extensions logs my-extension
rozi extensions logs my-extension --unit service.watch --lines 200
```

Without `--unit`, the command prints the last 50 lines of every log of the extension; `--lines`
changes the count. Each kind of process has its own log, listed by its heading:

| Log | Holds the output of |
| --- | --- |
| `commands` | `exec` and `shell` commands that run on the client |
| `service.<name>` | The service with that `name` |
| `command.<id>` | The [placed](#run-on-the-session-host) command with that `id` |
| `tab.<id>` | The placed sidebar tab with that `id` |

The name and ID are the ones in `extension.toml`, without the extension's own ID in front.

A log keeps only the most recent output, about 100 KB per log. An extension that prints a lot
should print less or write its own file in its [state directory](#keep-state).

If the command reports that another `rozi` is writing a log right now, run it again.

Logs belong to the machine the process ran on. A placed process logs on its host, so run
`rozi extensions logs` there to read it. Removing an extension deletes its logs; `--purge` is only
needed for [state](#keep-state).

## Publish to Discover

Listing a public GitHub repository in **Discover** needs no submission or review:

1. Put `extension.toml` at the repository root.
2. Include `id`, `title`, `description`, `version`, and `api`.
3. Add `min_rozi`, `platforms`, and `homepage` when they clarify compatibility. Use distinct, exact
   lowercase platform names: `linux`, `macos`, `windows`, `freebsd`, or `netbsd`.
4. Run `rozi extensions check .` and fix everything its **Discover** section lists.
5. Push to the default branch, and add the `rozi-extension` topic to the repository.

The index is rebuilt hourly. Each build reads the root `extension.toml` from the default branch,
records the exact commit, and never runs the repository's code. **Install** in Discover installs
that commit, and `rozi` validates the whole extension again before installing it. Pushing a new
commit updates the listing at the next build.

A repository with the topic that a build leaves out is listed, with the reason, in
[`skipped.json`](https://tui-lipan.github.io/rozi-extension-index/v1/skipped.json). `check`
reports the same problems before you publish. It also needs `extension.toml` at the root of a Git
repository. It cannot tell whether the repository is public, has the topic, or is archived,
disabled, or a fork.

Being listed is not a review. The index only checks that the metadata is well formed, and anyone
can list a repository, so Discover shows what it found, not what anyone vouched for. Read an
extension's source before you install it; see
[Review an extension before installing](extensions.md#review-an-extension-before-installing).

The index keeps each entry short, and leaves out a repository whose metadata exceeds a limit:

| Field | Maximum length |
| --- | --- |
| `id`, `version`, `min_rozi` | 64 characters |
| `title` | 80 characters |
| `description` | 280 characters |
| `homepage` | 256 characters |

The generated index and its schema are public at
[`tui-lipan/rozi-extension-index`](https://github.com/tui-lipan/rozi-extension-index).

## Stability

Extension API 1 is frozen. Within API 1, `rozi` will not break:

- the manifest keys documented on this page, and the schema at
  [`schemas/extension.schema.json`](../schemas/extension.schema.json);
- namespacing: every contributed ID is `<extension-id>.<local-id>`, and an extension can only add,
  never replace, a built-in;
- navigation targets as static load-time declarations: `rozi` owns foreground-program matching and
  key forwarding, and an explicit `[navigation] editors` list replaces all declarations;
- suggested keybindings: they target only the documented action allowlist, rank below user and
  built-in bindings, and never put extension code on the input path;
- the `ROZI_EXTENSION*` environment variables and their meanings, and `ROZI_SESSION_NAME`;
- the state directory: one per extension ID and machine, kept until purged;
- `placement` and what a placed process may do, as described under
  [Run on the session host](#run-on-the-session-host);
- the `rozi` control commands documented in [Control](control.md), and their exit codes;
- atomic validity: an extension loads whole or not at all;
- the `<prefix> x` chord space reserved for extension key suggestions;
- `--json` diagnostics with `schema_version` `1`.

`rozi` may add manifest keys, control commands, and diagnostic fields within API 1; a manifest that
does not use them keeps working. A manifest that uses a key added later is invalid on an older
`rozi`, which does not know the key; the page notes the capability each such key comes with. Any change that would invalidate a working manifest — a removed
key, a narrowed value, a changed default, a renamed environment variable — requires `api = 2`, and
an extension declaring `api = 1` keeps loading under the API 1 rules.

In practice:

- read `ROZI_EXTENSION_CONFIG` with your declared defaults as fallbacks;
- do not assume a suggested chord was granted;
- do not depend on behavior that this page and [Control](control.md) do not document.

## Examples

Three examples build on each other, each with its own tests:

1. [Hello](../examples/extensions/hello/) — a command, and a picker when you want one
2. [Project status](../examples/extensions/project-status/) — a service on the active session's
   host that subscribes to events, publishes an Activity row, and keeps per-host state
3. [Agent board](../examples/extensions/agent-board/) — an agent definition, a sidebar stream tab,
   and a service that follows the active session, local or remote

The repository also includes these example extensions:

- [Git tools](../examples/extensions/git-tools/) — grouped branch and worktree pickers
- [PR dashboard](../examples/extensions/pr-dashboard/) — a supervised pull request monitor
- [Docker](../examples/extensions/docker/) — container controls
- [SSH tools](../examples/extensions/ssh-tools/) — SSH host discovery and pane launch
- [Agent activity](../examples/extensions/agent-activity/) — notifications and a picker for pane status
- [Activity dashboard](../examples/extensions/activity-dashboard/) — general published activity
- [Snippets](../examples/extensions/snippets/) — saved commands pasted into the focused pane
- [Tasks](../examples/extensions/tasks/) — `just`, `make`, and `package.json` tasks, with a sidebar
  tab

See [Automation recipes](recipes.md) for smaller building blocks.
