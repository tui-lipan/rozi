# Named profiles

Named profiles are saved workspace layouts stored as TOML files in:

```
~/.config/hyprmux/profiles/<name>.toml
```

Each version-1 profile records workspaces, pane titles, layout metadata, and optional launch
identity (`cwd`, `command`, `keep_open`, floating geometry). Profiles do **not** save live
PTY state. Launching a session from one starts fresh shells or commands; applying one in place
replaces the destination session's panes after an unconditional two-press confirmation.

Profiles are recipes for named sessions. Opening `dev` attaches to a running session named `dev`,
or launches that session from `profiles/dev.toml`, or creates an empty named session when neither
exists. Existing version-1 profile files remain compatible.

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

## Open a profile-backed session

Launch a named profile from the command line:

```bash
hyprmux dev
hyprmux --session dev
```

An explicit target always uses attach-or-launch-or-create semantics. A bare `hyprmux` still starts
an ephemeral scratch session unless `[session] startup = "last"` is configured. In ephemeral mode,
`[profile] default` remains the first layout seed, followed by session autosave.

Set a default profile in config:

```toml
[profile]
default = "dev"
```

Or use the in-app **Profiles** command (command palette): highlight a profile and press
`Ctrl+f` to set it as the startup default.

If a profile file is missing or fails to parse, hyprmux shows a startup warning and falls
through to the next source (or a fresh layout).

## In-app commands

| Command | Action |
| --- | --- |
| **Save profile** | Prompts for a session-compatible name and writes `profiles/<name>.toml`. Named sessions prefill their own name; overwriting requires a second **Enter**. |
| **Profiles** | Lists saved profiles with in-picker actions (see below). |
| **Apply profile into session...** | Replaces the current session's panes from a profile without changing its name. |

### Profile picker actions

Open **Profiles** from the command palette, then:

| Key | Action |
| --- | --- |
| **Enter** | Attach to the running same-named session, or launch it from the profile. Destructive replacement of a live ephemeral session requires a second press. |
| **Ctrl+r** | Apply the highlighted profile into the current session. Press twice to replace every pane. |
| **Ctrl+f** | Set the highlighted profile as `[profile] default` in `hyprmux.toml`. |
| **Ctrl+d** | Delete the highlighted profile file. Press **Ctrl+d** again on the same row to confirm. |

Profiles marked **default** in the list match your current `[profile] default` setting.
Deleting the default profile clears that config entry when the file is removed.

Profile names use letters, numbers, `_`, or `-` because the same names can identify sessions.

## Command lifetime

Without `keep_open = true`, a pane closes when its `command` exits.

Set `keep_open = true` to drop into an interactive shell instead. When the command finishes, the
session server prints its exit status into the pane and replaces the dead PTY with your interactive
shell **in place** — same pane, same scrollback, so the command's output is still there above the
new prompt. The shell starts in the directory the command left the pane in, when that is known.

See also [Project profiles & pane identity](project-profiles.md) for pane titles, saving
limitations, and session autosave details.
