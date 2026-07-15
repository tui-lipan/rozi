# Project profiles and pane identity

Project profiles are reusable launch recipes. A profile is a TOML file that records workspaces,
pane names, layout metadata, and optional launch identity for each pane.

Profiles do **not** save or restore live PTY state. Restoring a profile starts fresh shells or commands in new PTYs; it does not resurrect the shell processes, scrollback, environment, or running programs from an earlier `hyprmux` process.

## Pane identity

Each pane carries an *identity* - the information `hyprmux` knows about it beyond its live
shell:

- **Custom title** - a name you set with the `n` keybinding (or *Rename pane* in the command
  palette). It overrides the program's terminal title. Submitting an empty name clears it.
- **Profile name** - the name a pane was restored with from a profile.
- **cwd / command** - the local working directory and launch command. Saves use live server runtime
  metadata, reject remote-host cwd reports, and capture a non-shell foreground executable basename
  when no explicit launch command exists.

The titlebar shows the custom title if set, otherwise the program's terminal title, otherwise
the default label `shell`. See [Layouts & panes › Titlebars](layouts-and-panes.md#titlebars)
for the full precedence, and [Keybindings](keybindings.md) for the rename flow.

## Capture and launch profiles

Named profiles live in `~/.config/hyprmux/profiles/<name>.toml`. Capture the current session with
the **Capture session as profile...** command in the command palette; it prompts for the profile
name and writes that file.

Open its canonical same-name session from the command line:

```bash
hyprmux dev
hyprmux --session dev
```

The target attaches when `dev` is already running, otherwise launches named session `dev` from the
profile. It reports an error rather than silently creating an empty session when neither exists.
The profile and session remain independent; the same-name session is only the canonical default
binding. Launch the recipe under another name explicitly with:

```bash
hyprmux new review --profile dev
```

Open **Profiles** to use the canonical flow with `Enter`, open the recipe as a newly named session
with `Ctrl+Enter`, capture a new profile with `Ctrl+N`, replace the current session with `Ctrl+R`
twice, delete profiles, or toggle one as the ephemeral startup default:

```toml
[profile]
default = "dev"
```

Explicit targets take precedence. On a bare ephemeral launch, `[profile] default` precedes local
session autosave; `[session] startup = "last"` instead opens a named session. See
[Named profiles](profiles.md) for picker controls.

Replacing a session from the picker is destructive: it closes all panes and running processes,
then launches the recipe while retaining the session name and attached clients. A session created
from a profile can retain `created_from_profile` as historical origin metadata. The session picker
displays it as `from <profile>`, and resurrection snapshots persist it; applying a different profile
later does not rewrite that origin.

## Profile shape

Each profile has a version, the active workspace, and workspace entries. A workspace entry may
set `name` (its custom name, settable at runtime with *Rename workspace* - see
[Keybindings](keybindings.md#workspaces)) and `layout`, one of `dwindle`, `master`, `grid`, or
`monocle`. Pane entries can use these fields:

- `name`: pane title shown by `hyprmux` and restored on startup.
- `cwd`: directory used when launching that pane's fresh shell or command. `~` and `~/...` expand to `HOME`.
- `command`: command string run through the configured `command_shell` when launching the pane.
- `keep_open`: start the configured interactive shell in the same pane after `command` exits.
- `floating`: whether the pane is floating instead of tiled.
- `fullscreen`: whether the pane starts fullscreen.
- `rect`: floating geometry as `{ x, y, w, h }`; used for floating panes.

Example:

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
name = "logs"
cwd = "~/code/my-app"

[[workspaces]]
index = 1
layout = "master"

[[workspaces.panes]]
id = 0
name = "scratch"
floating = true
rect = { x = 8.0, y = 4.0, w = 80.0, h = 24.0 }
```

## Command lifetime

Profile commands run through the configured `command_shell`. If the command exits, `hyprmux` closes that pane. Set `keep_open = true` to preserve the pane and start the configured interactive shell in place after the command finishes:

```toml
command = "cargo run"
keep_open = true
```

## Session auto-save

Beyond explicit profile capture, local `hyprmux` launches can **auto-save the live layout on
quit** and restore it on the next launch. Enable it with `[session] autosave = true` (see
[Configuration](configuration.md#session)). It reuses the profile format and the same honesty
caveats: it restores layout and launch intent, not live PTY state. `[profile] default` takes
precedence over the autosaved session.

For live PTY persistence across detach/reattach, use a named attached session instead. Use
`hyprmux attach <name>` for attach-only behavior, `hyprmux <name>` for attach-or-canonical-profile
resolution, or `hyprmux new <name>` for explicit creation (see [Sessions](sessions.md)).

## Saving limitations

Saving preserves pane names, workspace layout, split tree/ratios, floating state, fullscreen
state, floating geometry, and each pane's detected local working directory. A cwd reported by a
remote host, such as an SSH session, is never saved as a local path; hyprmux falls back to the
pane's original local launch directory instead.

When the server can identify a foreground executable, saving records its basename as `command`.
Interactive shells are filtered out, and an explicit launch command always takes precedence.
hyprmux cannot reconstruct the foreground program's original arguments, so a detected `nvim`
process is saved as `command = "nvim"`, not its full invocation. Rename panes explicitly when you
want stable profile titles.
