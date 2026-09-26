# Profiles

A profile is a saved recipe for a workspace: pane commands, working directories, names, and layouts.
Launching a profile starts fresh panes from that recipe. This page covers saving and launching
profiles, the file format, and what capture can and cannot record.

## Save and launch a profile

1. Arrange your panes the way you want them.
2. Press `Ctrl+A`, then `O`, or run **Save session as profile** from the command palette.
3. Enter a name. If a profile with that name exists, press `Enter` again to overwrite it.

The live session is not changed. The profile is saved as a TOML file:

```text
~/.config/rozi/profiles/<name>.toml
```

Profile names use letters, numbers, `_`, and `-`.

To launch the profile as a session with the same name:

```bash
rozi dev
rozi --session dev
```

Both commands attach to session `dev` if it is running, and otherwise start it from
`profiles/dev.toml`. They report an error if neither the session nor the profile exists.

To launch the same recipe under a different session name:

```bash
rozi sessions new review --profile dev
```

A profile and the sessions started from it are independent. Editing the profile does not change a
running session. A session may record which profile created it, but only as information about its
origin.

## Use the profile picker

Press `Ctrl+A`, then `o` to open **Profiles**.

| Key | Action |
| --- | --- |
| `Enter` | Attach to or launch the profile's same-name session |
| `Ctrl+O` | Launch under another name, or as a temporary session with an empty name |
| `Ctrl+N` | Capture the current session under a new profile name |
| `Ctrl+R` twice | Replace the current session's panes with the selected profile |
| `Ctrl+F` | Set or clear the selected profile as the default |
| `Ctrl+D` twice | Delete the profile file |

If you are in a temporary session with running panes and opening the profile would end that
session, `Enter` asks you to press it again. Set `[confirm] load_profile = false` to skip this.

Replacing closes all of the session's panes and their processes, then launches the recipe. The
session keeps its name and attached clients.

## Choose a default profile

```toml
[profile]
default = "dev"
```

The default profile seeds any new session created without another recipe. To make a bare `rozi`
open the default profile's same-name session, also set:

```toml
[session]
startup = "profile"
```

If the default profile is missing or invalid, a new session starts with a single fresh pane. A
command that names a missing or invalid profile reports an error instead.

Git worktree sessions ignore `[profile] default`, because its pane directories usually point at
another checkout. Set `[worktrees] profile` to choose the profile they use; its repository paths are
rebased onto the new checkout. See [Worktrees](worktrees.md).

## Write a profile

This example uses the common fields:

```toml
version = 1
active_workspace = 0

[[workspaces]]
index = 0
name = "code"
layout = "dwindle"
focused_pane = 0

[[workspaces.panes]]
id = 0
name = "server"
cwd = "~/code/my-app"
command = "cargo run"
keep_open = true

[[workspaces.panes]]
id = 1
name = "editor"
cwd = "~/code/my-app"
argv = ["nvim", "src/main.rs"]

[[workspaces]]
index = 1
name = "logs"
layout = "rows"
synchronized = true

[[workspaces.panes]]
id = 0
name = "api"
scrollable_width = 0.55
```

A commented example is in [`examples/profiles/dev.toml`](../examples/profiles/dev.toml).

### Choose `command` or `argv`

Each pane can set `command` or `argv`, not both. A profile that sets both on one pane is rejected.

- `command` is typed into the pane's interactive shell at its first prompt. Aliases, functions, and
  `PATH` changes from shell startup are available, and the pane returns to the shell when the
  command exits.
- `argv` launches the program directly, without a shell, and keeps argument boundaries intact:

  ```toml
  argv = ["ssh", "--", "host with spaces"]
  ```

  Set `keep_open = true` to get an interactive shell in the pane after the program exits.

## Profile file reference

### Top-level fields

| Field | Type | Meaning |
| --- | --- | --- |
| `version` | integer | Profile format version. Use `1`. |
| `active_workspace` | integer | Zero-based workspace selected after launch. |
| `workspaces` | array of tables | Saved workspace entries. |

rozi has nine workspaces, indexed `0` through `8`. An out-of-range `active_workspace` falls back to
workspace `0`, and out-of-range workspace entries are ignored.

### Workspace fields

| Field | Type | Meaning |
| --- | --- | --- |
| `index` | integer | Zero-based workspace number. |
| `name` | string | Optional workspace name. |
| `synchronized` | boolean | Whether terminal input is synchronized across eligible panes. Defaults to `false`. |
| `layout` | string | `dwindle`, `master`, `grid`, `columns`, `rows`, `scrollable`, or `monocle`. |
| `split_ratios` | array of numbers | Stored layout ratios. Usually written by capture. |
| `focused_pane` | integer | Pane `id` to focus. |
| `tree` | table | Optional Dwindle split tree. |
| `panes` | array of tables | Pane entries. |

A `tree` is either a single leaf:

```toml
[workspaces.tree]
kind = "leaf"
pane = 0
```

or a split with two child trees, each of which may itself be a split:

```toml
[workspaces.tree]
kind = "split"
axis = "vertical"
ratio = 0.5

[workspaces.tree.first]
kind = "leaf"
pane = 0

[workspaces.tree.second]
kind = "leaf"
pane = 1
```

`axis` is `horizontal` or `vertical`. A leaf's `pane` refers to a pane `id` in the same workspace. If
`tree` is missing or unusable, rozi builds a Dwindle tree from the order of the tiled panes.

### Pane fields

| Field | Type | Meaning |
| --- | --- | --- |
| `id` | integer | Pane identity within the profile, used by `focused_pane` and `tree`. |
| `pane_id` | integer | Optional server pane identity written by capture and snapshots. Omit it in hand-written profiles. |
| `name` | string | Pane title. |
| `title` | string | Older title field, used when `name` is absent. Use `name` in new files. |
| `cwd` | path | Starting directory. `~` and `~/…` expand to your home directory. |
| `command` | string | Command typed into the interactive shell at its first prompt. |
| `argv` | array of strings | Program and arguments launched directly, without a shell. |
| `keep_open` | boolean | After an `argv` program exits, replace it with an interactive shell in the same pane. |
| `floating` | boolean | Start the pane floating. |
| `fullscreen` | boolean | Start the pane fullscreen. |
| `rect` | table | Floating rectangle with `x`, `y`, `w`, and `h` numbers. |
| `scrollable_width` | number | Column width in the Scrollable layout, as a fraction of the viewport. Defaults to `0.45`; clamped to `0.20`–`0.80`. |

## What capture saves

Capture saves:

- workspace names, layouts, synchronization, split trees, and ratios
- pane names, floating and fullscreen state, floating rectangles, and Scrollable widths
- each pane's working directory, when it is a usable local path
- a pane's explicit launch command, or the command still running when you capture

A pane sitting at an idle shell prompt saves no command. rozi does not replay your last command just
because it is still in the terminal history.

For a running command, capture keeps the executable and its arguments where it can. It writes the
executable's full path when the bare name is not on the session server's `PATH`.

Some details cannot be captured:

- On macOS, capture can identify the executable but may leave out its arguments.
- On Windows, capture uses shell integration for the program name and cannot read native process
  arguments.
- Arguments are left out when the program was started through a wrapper rozi cannot safely match to
  the reported program.
- Arguments containing control characters are left out rather than written into a shell command.
- Remote panes do not save their remote paths as `cwd` and keep only portable command information.

Capture stores command lines as plain text. Arguments may contain access tokens, passwords, private
URLs, or other secrets. Review a profile before sharing or committing it.

## Session autosave

With `[session] autosave = true`, rozi writes the current layout in the profile format when a local
client leaves. It restores layout and launch commands, not running processes. A configured default
profile takes precedence over the autosave when seeding a session.

## Profiles, sessions, and resurrection

These three features overlap, so choose by what you want to keep:

- A profile reproduces a workspace from scratch with fresh panes.
- A [named session](sessions.md) keeps your panes and processes running on its server while no
  client is attached.
- [Resurrection](sessions.md#resurrection) recreates a named session after its server has stopped.
  It restarts commands and replays saved terminal history.
