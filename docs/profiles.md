# Named profiles

Named profiles are reusable launch recipes stored as TOML files in:

```
~/.config/hyprmux/profiles/<name>.toml
```

Each version-1 profile records workspaces, pane titles, layout metadata, and optional launch
identity (`cwd`, `command`, `keep_open`, floating geometry). Profiles do **not** save live PTY
state. Launching from one starts fresh shells or commands.

A profile and a session are independent objects. The same-name session is only the profile's
canonical default binding: opening profile `dev` uses session `dev`, while `hyprmux new review
--profile dev` launches the same recipe as an independent session named `review`. A session created
from a profile may record that recipe as `created_from_profile`, but it does not remain linked to
the file and later profile edits do not alter the running session. Existing version-1 profile files
remain compatible.

## Profile fields

Each pane entry supports:

| Field | Notes |
| --- | --- |
| `name` | Pane title shown in the titlebar. |
| `cwd` | Local working directory when the pane launches. `~` expands to `$HOME`; remote SSH paths are not captured as local paths. |
| `command` | Run through the configured `command_shell` when the pane opens. Saving keeps an explicit launch command, or captures a detected non-shell foreground executable basename. |
| `keep_open` | When `true`, drop into an interactive shell when the command exits instead of closing the pane. |
| `floating` | Start as a floating pane instead of tiled. |
| `fullscreen` | Start fullscreen. |
| `rect` | Floating geometry `{ x, y, w, h }`. |

Workspace entries may also include `synchronized = true` to restore pane synchronization for that
workspace. Omit `tree` in a workspace to let hyprmux auto-build a dwindle tree from pane order.

## Example: lazygit + nvim

```toml
version = 1
active_workspace = 0

[[workspaces]]
index = 0
layout = "dwindle"
focused_pane = 0

[[workspaces.panes]]
id = 0
name = "lazygit"
command = "lazygit"
keep_open = true

[[workspaces.panes]]
id = 1
name = "nvim"
command = "nvim"
```

A commented copy lives at [`examples/profiles/dev.toml`](../examples/profiles/dev.toml).

## Launch a profile

Use the profile's canonical same-name session:

```bash
hyprmux dev
hyprmux --session dev
```

These spellings attach to running session `dev`; if it is not running, they launch it from
`profiles/dev.toml`. If neither the session nor profile exists, hyprmux reports an error and tells
you to create the session explicitly. An unknown target never silently creates an empty session.

To create an independent session, optionally from any recipe, use:

```bash
hyprmux new review
hyprmux new review --profile dev
```

`attach` and `new` are reserved CLI command words. Use `hyprmux --session attach` or
`hyprmux --session new` when a session or canonical profile binding actually has one of those
names. A bare `hyprmux` still starts an ephemeral scratch session unless `[session] startup =
"last"` is configured. In ephemeral mode, `[profile] default` remains the first launch seed,
followed by session autosave.

Set a default profile in config:

```toml
[profile]
default = "dev"
```

Or use the in-app **Profiles** command (command palette): highlight a profile and press
`Ctrl+f` to toggle it as the startup default.

If a configured default profile is missing or fails to parse, hyprmux shows a startup warning and
falls through to the next bare-launch source (or a fresh layout). An explicit canonical target with
a missing or invalid profile reports an error instead.

## In-app commands

| Command | Action |
| --- | --- |
| **Capture session as profile...** | Prompts for a session-compatible name and writes `profiles/<name>.toml`. The creating profile is preferred as the initial name, then the session name; overwriting requires a second **Enter**. |
| **Profiles** | Lists saved profiles with in-picker actions (see below). |
| **Replace session with profile...** | Destructively replaces every pane in the current session from a profile without changing the session name or disconnecting its clients. |

### Profile picker actions

Open **Profiles** from the command palette, then:

| Key | Action |
| --- | --- |
| **Enter** | Attach to the running canonical same-name session, or launch that canonical session from the profile. Leaving a live ephemeral session may require a second press. |
| **Ctrl+Enter** | **Open as**: launch the highlighted recipe as a new session under a name you enter. The name must not already be running. |
| **Ctrl+n** | Capture the current session as a new profile. |
| **Ctrl+r** | Replace the current session with the highlighted profile. Press twice to close all panes and running processes and launch the recipe; the session name and attached clients are kept. |
| **Ctrl+f** | Toggle the highlighted profile as `[profile] default` in `hyprmux.toml`; pressing it on the current default clears the setting. |
| **Ctrl+d** | Delete the highlighted profile file. Press **Ctrl+d** again on the same row to confirm. |

The status beside a profile refers only to its canonical same-name session: **attached**,
**running**, or **launch**. It does not count independent sessions created from that profile under
other names. Profiles marked **default** match your current `[profile] default` setting. Deleting
the default profile clears that config entry when the file is removed.
The footer hints follow the selected row, showing **attach** or **launch** as appropriate; the
**default** hint remains a toggle.

Profile names use letters, numbers, `_`, or `-` because their canonical binding can identify a
same-named session.

## Command lifetime

Without `keep_open = true`, a pane closes when its `command` exits.

Set `keep_open = true` to drop into an interactive shell instead. When the command finishes, the
session server prints its exit status into the pane and replaces the dead PTY with your interactive
shell **in place** — same pane, same scrollback, so the command's output is still there above the
new prompt. The shell starts in the directory the command left the pane in, when that is known.

See also [Project profiles & pane identity](project-profiles.md) for pane titles, saving
limitations, and session autosave details.
