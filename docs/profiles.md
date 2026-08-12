# Named profiles

Named profiles are reusable launch recipes stored as TOML files in:

```
~/.config/rozi/profiles/<name>.toml
```

Each version-1 profile records workspaces, pane titles, layout metadata, and optional launch
identity (`cwd`, `command`, `keep_open`, floating geometry). Profiles do **not** save live PTY
state. Launching from one starts fresh shells or commands.

A profile and a session are independent objects. The same-name session is only the profile's
canonical default binding: opening profile `dev` uses session `dev`, while `rozi new review
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
| `command` | Typed into the pane's interactive shell at its first prompt when the pane opens, so aliases, shell functions, and rc-file `PATH` entries resolve exactly as if you ran it yourself. Saving keeps an explicit launch command, or captures the executable of a command that is still running. |
| `keep_open` | Kept for round-tripping; a restored command pane always returns to its interactive shell when the command exits. |
| `floating` | Start as a floating pane instead of tiled. |
| `fullscreen` | Start fullscreen. |
| `rect` | Floating geometry `{ x, y, w, h }`. |

Workspace entries may also include `synchronized = true` to restore pane synchronization for that
workspace. Omit `tree` in a workspace to let rozi auto-build a dwindle tree from pane order.

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
rozi dev
rozi --session dev
```

These spellings attach to running session `dev`; if it is not running, they launch it from
`profiles/dev.toml`. If neither the session nor profile exists, rozi reports an error and tells
you to create the session explicitly. An unknown target never silently creates an empty session.

To create an independent session, optionally from any recipe, use:

```bash
rozi new review
rozi new review --profile dev
```

`attach` and `new` are reserved CLI command words. Use `rozi --session attach` or
`rozi --session new` when a session or canonical profile binding actually has one of those
names. A bare `rozi` opens the session picker (or starts an ephemeral scratch session when there
is nothing to pick) unless `[session] startup = "last"` is configured. In ephemeral mode,
`[profile] default` remains the first launch seed, followed by session autosave.

Set a default profile in config:

```toml
[profile]
default = "dev"
```

Or use the in-app **Profiles** command (command palette): highlight a profile and press
`Ctrl+f` to toggle it as the startup default.

The default seeds **every session opened without a recipe**, not just the launch that started
rozi: the initial session, each new temporary session, and each named session created without a
profile. Paths that already name a recipe are unaffected — attaching to an existing session, a
canonical `rozi <name>` target, `new <name> --profile <recipe>`, and the picker's **Open as** all
keep using theirs. Sessions seeded this way record the default as their creating profile, so the
capture prompt offers its name back.

If a configured default profile is missing or fails to parse, rozi shows a startup warning and
falls through to the next bare-launch source (or a fresh layout); sessions created later fall back
to a single shell rather than failing to open. An explicit canonical target with a missing or
invalid profile reports an error instead.

## In-app commands

| Command | Action |
| --- | --- |
| **Capture session as profile...** | Prompts for a session-compatible name and writes `profiles/<name>.toml`. The creating profile is preferred as the initial name, then the session name; overwriting requires a second **Enter**. From a temporary session, **Enter** also names that session after the profile — see below. |
| **Profiles** | Lists saved profiles with in-picker actions (see below). |
| **Replace session with profile...** | Destructively replaces every pane in the current session from a profile without changing the session name or disconnecting its clients. |

### Capturing a temporary session

Capturing from a temporary session names that session after the profile on the same **Enter**, so
the running session and its recipe end up sharing one identity:

- while the session lives, `rozi <name>` reattaches to it,
- once it is gone, `rozi <name>` rebuilds a new one from the profile.

A session that is *already* named keeps its name — capturing `dev-full` out of session `dev` leaves
`dev` called `dev`. To capture a temporary session without keeping it, capture it and kill the
resulting named session afterwards.

Naming is skipped, and the capture reported as `session name already in use`, when another session
is already running under that name. The profile is still written.

### Profile picker actions

Open **Profiles** from the command palette, then:

| Key | Action |
| --- | --- |
| **Enter** | Attach to the running canonical same-name session, or launch that canonical session from the profile. Leaving a live ephemeral session may require a second press. |
| **Ctrl+o** | **Open as**: launch the highlighted recipe under a new session name, or leave the name empty for a fresh ephemeral session. A name must not already be running. |
| **Ctrl+n** | Capture the current session as a new profile. |
| **Ctrl+r** | Replace the current session with the highlighted profile. Press twice to close all panes and running processes and launch the recipe; the session name and attached clients are kept. |
| **Ctrl+f** | Toggle the highlighted profile as `[profile] default` in `config.toml`; pressing it on the current default clears the setting. |
| **Ctrl+d** | Delete the highlighted profile file. Press **Ctrl+d** again on the same row to confirm. |

The status beside a profile refers only to its canonical same-name session: **attached** or
**running**. It does not count independent sessions created from that profile under
other names. Profiles marked **default** match your current `[profile] default` setting. Deleting
the default profile clears that config entry when the file is removed.
The footer hints follow the selected row, showing **attach** or **launch** as appropriate; the
**default** hint remains a toggle.

Profile names use letters, numbers, `_`, or `-` because their canonical binding can identify a
same-named session.

## Command lifetime

A restored pane starts your interactive shell in its `cwd`; a pane with a `command` then has that
command typed into the shell's first prompt. Because the command runs inside a real interactive
shell, aliases, shell functions, and rc-file `PATH` entries resolve, the prompt's title/OSC
integration runs first, and when the command exits the pane simply returns to the prompt — the
command's output stays in the scrollback above it. (`keep_open` matters for panes spawned with a
command through the command-runner shell, such as `[[rules]]` targets or control `new-pane`; for
those, `keep_open = true` replaces the dead PTY with your interactive shell in place after the
command exits.)

See also [Project profiles & pane identity](project-profiles.md) for pane titles, saving
limitations, and session autosave details.
