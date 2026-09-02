# Profiles

A profile is a reusable launch recipe. It starts fresh panes from saved commands, working
directories, names, and layouts.

A live named session is different. Its server keeps the actual PTYs and processes running while
clients detach. Resurrection is different again. It recreates a named session after its server has
gone, restarts commands, and replays saved terminal history.

Use a profile to reproduce a workspace. Use a named session to keep work running. Use resurrection
to recover the shape and history of a stopped named session. See [Sessions](sessions.md).

## Create and launch profiles

Profiles are TOML files in:

```text
~/.config/rozi/profiles/<name>.toml
```

Capture the current session with the `Shift+O` command key or **Capture session as profile** in the
command palette. Rozi prompts for a name and leaves the live session unchanged. Overwriting an
existing file requires a second `Enter`.

Launch the canonical same-name session:

```bash
rozi dev
rozi --session dev
```

These commands attach to session `dev` when it is running. Otherwise they start session `dev` from
`profiles/dev.toml`. They report an error if neither exists.

Launch the same recipe under another session name:

```bash
rozi sessions new review --profile dev
```

A profile and a session remain independent. Editing the profile does not change a running session.
The session may remember which profile created it, but that value is origin information only.

## Use the profile picker

Open **Profiles** with the `o` command key.

| Key | Action |
| --- | --- |
| `Enter` | Attach to or launch the profile's same-name session |
| `Ctrl+O` | Launch under another name, or as a temporary session with an empty name |
| `Ctrl+N` | Capture the current session under a new profile name |
| `Ctrl+R` twice | Replace the current session's panes with the selected profile |
| `Ctrl+F` | Set or clear the selected profile as the default |
| `Ctrl+D` twice | Delete the profile file |

Replacing a session closes all of its panes and processes, then launches the recipe while retaining
the session name and attached clients.

Profile names use letters, numbers, `_`, and `-`.

## Choose a default profile

```toml
[profile]
default = "dev"
```

The default seeds sessions created without another recipe. To make a bare `rozi` open its named
canonical session, set:

```toml
[session]
startup = "profile"
```

A missing or invalid default falls back to a fresh pane for ordinary new-session creation. An
explicit target that depends on a missing or invalid profile reports an error.

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

A commented example is available at [`examples/profiles/dev.toml`](../examples/profiles/dev.toml).

## Complete profile schema

### Top-level fields

| Field | Type | Meaning |
| --- | --- | --- |
| `version` | integer | Profile format version. Use `1`. |
| `active_workspace` | integer | Zero-based workspace selected after launch. |
| `workspaces` | array of tables | Saved workspace entries. |

Rozi has nine workspaces, indexed `0` through `8`. An out-of-range active workspace falls back to
workspace `0`. Out-of-range workspace entries are ignored.

### Workspace fields

| Field | Type | Meaning |
| --- | --- | --- |
| `index` | integer | Zero-based workspace number. |
| `name` | string | Optional workspace name. |
| `synchronized` | boolean | Whether terminal input is synchronized across eligible panes. Defaults to `false`. |
| `layout` | string | `dwindle`, `master`, `grid`, `columns`, `rows`, `scrollable`, or `monocle`. |
| `split_ratios` | array of numbers | Stored layout ratios. Usually written by capture. |
| `focused_pane` | integer | Local pane `id` to focus. |
| `tree` | table | Optional Dwindle split tree. |
| `panes` | array of tables | Pane launch entries. |

If `tree` is absent or unusable, Rozi builds a Dwindle tree from tiled pane order. A tree is either:

```toml
[workspaces.tree]
kind = "leaf"
pane = 0
```

or a recursive split:

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

`axis` is `horizontal` or `vertical`. Leaf `pane` values refer to pane `id` fields in the same
workspace.

### Pane fields

| Field | Type | Meaning |
| --- | --- | --- |
| `id` | integer | Profile-local pane identity used by `focused_pane` and `tree`. |
| `pane_id` | integer | Optional server pane identity written by capture and snapshots. Hand-written profiles can omit it. |
| `name` | string | Pane title restored by the profile. |
| `title` | string | Older title field used when `name` is absent. New files should use `name`. |
| `cwd` | path | Starting directory. `~` and `~/...` expand from the user's home. |
| `command` | string | Command typed into the interactive shell at its first prompt. |
| `argv` | array of strings | Program and arguments launched directly without a shell. |
| `keep_open` | boolean | After a directly launched process exits, replace it with an interactive shell in the same pane. Shell-replayed `command` entries already return to their prompt. |
| `floating` | boolean | Start the pane floating. |
| `fullscreen` | boolean | Start the pane fullscreen. |
| `rect` | table | Floating rectangle with `x`, `y`, `w`, and `h` numbers. |
| `scrollable_width` | number | Scrollable column width as a viewport fraction. Defaults to `0.45` and clamps from `0.20` to `0.80`. |

`command` and `argv` are mutually exclusive. A profile containing both for one pane is rejected.

`command` runs through the interactive shell, so aliases, functions, and shell startup `PATH`
changes are available. The pane returns to that shell after the command exits.

`argv` preserves argument boundaries and does not invoke a shell:

```toml
argv = ["ssh", "--", "host with spaces"]
```

## What capture saves

Capture saves:

- workspace names, layouts, synchronization, split trees, and ratios
- pane names, floating and fullscreen state, floating rectangles, and Scrollable widths
- each pane's usable local working directory
- explicit launch intent or a command still running at capture time

An idle shell saves no command. Rozi does not replay the last command merely because it remains in
terminal history.

For a running command, capture tries to keep the executable and its arguments. It uses a full
executable path when the server cannot resolve the bare name on its own `PATH`.

Some process details are unavailable:

- On macOS, capture can identify the executable but may omit its argument vector.
- On Windows, capture relies on shell integration for the reported program name and cannot inspect
  native process arguments.
- Unmatched wrappers omit arguments when Rozi cannot safely associate them with the reported
  program.
- Arguments containing control characters are omitted rather than serialized into an unsafe shell
  command.
- Remote panes do not save remote paths as local `cwd` values and keep only portable command
  information.

Capture stores command lines in plain text. Arguments may contain access tokens, passwords, private
URLs, or other secrets. Review a profile before sharing or committing it.

## Session autosave

`[session] autosave = true` writes the current layout in the same profile format when a local client
leaves. It restores layout and launch intent, not live PTYs. A configured default profile takes
precedence over this autosave when seeding a session.

For live process continuity, use a named session. For terminal history recovery after the server is
gone, use [session resurrection](sessions.md#resurrection).
