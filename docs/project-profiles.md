# Project profiles and pane identity

Project profiles are reusable launch recipes. A profile is a TOML file that records workspaces,
pane names, layout metadata, and optional launch identity for each pane.

Profiles do **not** save or restore live PTY state. Restoring a profile starts fresh shells or commands in new PTYs; it does not resurrect the shell processes, scrollback, environment, or running programs from an earlier `rozi` process.

## Pane identity

Each pane carries an *identity* - the information `rozi` knows about it beyond its live
shell:

- **Custom title** - a name you set with the `Shift+N` keybinding (or *Rename pane* in the command
  palette). It overrides the program's terminal title. Submitting an empty name clears it.
- **Profile name** - the name a pane was restored with from a profile.
- **cwd / command** - the local working directory and launch command. Saves use live server runtime
  metadata, reject remote-host cwd reports, and capture the non-shell executable basename of a
  command that is still running (a pane idling at its prompt saves no command).

The titlebar shows the custom title if set, then an application-provided terminal title, then the
current working directory. A custom or application title is followed by a compact project-qualified
path (or the shortened cwd outside a project). Shell prompt titles are reduced to their directory
and qualified by the username only after switching away from the original account. See
[Layouts & panes › Titlebars](layouts-and-panes.md#titlebars) for the full behavior, and
[Keybindings](keybindings.md) for the rename flow.

## Capture and launch profiles

Named profiles live in `~/.config/rozi/profiles/<name>.toml`. Capture the current session with
the **Capture session as profile...** command in the command palette; it prompts for the profile
name and writes that file.

Open its canonical same-name session from the command line:

```bash
rozi dev
rozi --session dev
```

The target attaches when `dev` is already running, otherwise launches named session `dev` from the
profile. It reports an error rather than silently creating an empty session when neither exists.
The profile and session remain independent; the same-name session is only the canonical default
binding. Launch the recipe under another name explicitly with:

```bash
rozi new review --profile dev
```

Open **Profiles** to use the canonical flow with `Enter`, open the recipe under another name or as
an ephemeral session with `Ctrl+O`, capture a new profile with `Ctrl+N`, replace the current session
with `Ctrl+R` twice, delete profiles, or toggle one as the ephemeral startup default:

```toml
[profile]
default = "dev"
```

Explicit targets take precedence. On a bare ephemeral launch, `[profile] default` precedes local
session autosave; `[session] startup = "last"` and `= "profile"` instead open a named session — the
latter the one named after this default. The default also seeds sessions created later in the run,
whenever no recipe is named for them. See [Named profiles](profiles.md) for picker controls.

Replacing a session from the picker is destructive: it closes all panes and running processes,
then launches the recipe while retaining the session name and attached clients. A session created
from a profile can retain `created_from_profile` as historical origin metadata. The session picker
displays it as `from <profile>`, and resurrection snapshots persist it; applying a different profile
later does not rewrite that origin.

## Profile shape

Each profile has a version, the active workspace, and workspace entries. A workspace entry may
set `name` (its custom name, settable at runtime with *Rename workspace* - see
[Keybindings](keybindings.md#workspaces)) and `layout`, one of `dwindle`, `master`, `grid`,
`columns`, `scrollable`, or `monocle`. Pane entries can use these fields:

- `name`: pane title shown by `rozi` and restored on startup.
- `cwd`: directory used when launching that pane's fresh shell or command. `~` and `~/...` expand to `HOME`.
- `command`: command line typed into the pane's interactive shell at its first prompt.
- `argv`: direct executable and arguments, launched without a shell; mutually exclusive with `command`.
- `keep_open`: kept for round-tripping; a command pane returns to its interactive shell on exit.
- `floating`: whether the pane is floating instead of tiled.
- `fullscreen`: whether the pane starts fullscreen.
- `rect`: floating geometry as `{ x, y, w, h }`; used for floating panes.
- `scrollable_width`: optional Scrollable column width as a fraction of the tile viewport
  (default `0.45` when absent; clamped to `0.20`–`0.80`).

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

A restored pane launches your interactive shell in its `cwd`. A pane with a `command` has that
command typed into the shell's first prompt, so aliases, shell functions, and rc-file `PATH`
entries resolve exactly as if you ran it yourself, and the pane returns to the prompt when the
command exits:

```toml
command = "cargo run"
```

For a direct process with argument boundaries preserved across platforms, use `argv` instead:

```toml
argv = ["ssh", "--", "host with spaces"]
```

## Session auto-save

Beyond explicit profile capture, local `rozi` launches can **auto-save the live layout on
quit** and restore it on the next launch. Enable it with `[session] autosave = true` (see
[Configuration](configuration.md#session)). It reuses the profile format and the same honesty
caveats: it restores layout and launch intent, not live PTY state. `[profile] default` takes
precedence over the autosaved session.

For live PTY persistence across detach/reattach, use a named attached session instead. Use
`rozi attach <name>` for attach-only behavior, `rozi <name>` for attach-or-canonical-profile
resolution, or `rozi new <name>` for explicit creation (see [Sessions](sessions.md)).

## Saving limitations

Saving preserves pane names, workspace layout, split tree/ratios, floating state, fullscreen
state, floating geometry, Scrollable pane widths, and each pane's detected local working directory. A cwd reported by a
remote host, such as an SSH session, is never saved as a local path; rozi falls back to the
pane's original local launch directory instead.

When a pane is running a command at save time, saving records how that program was launched as
`command`: the executable and the arguments it is running with, so
`claude --dangerously-skip-permissions` comes back with its flag rather than as a bare `claude`.
Interactive shells are filtered out, a pane idling at its prompt saves no command at all (the last
command you ran is not replayed), and an explicit launch command is kept when nothing is running.
Rename panes explicitly when you want stable profile titles.

The program is saved as a bare name whenever a bare name can launch it again. If the session
server cannot resolve the name on its own `PATH` - you started the program through a shell alias,
or ran a binary out of `./target/release` - the executable's full path is saved instead, so
restoring runs the same program rather than reporting `command not found`.

Arguments come from the running process itself, so what is saved is what the program received
after the shell was done with it: aliases, globs, and variables appear expanded. Some cases save
the program without its arguments:

- **macOS and Windows.** Reading another process's arguments is Linux-only in rozi today.
- **Wrapped programs.** When the process holding the terminal is not the program the pane reports
  running - an `npx`-style runner, a shell function, a launcher script - its arguments belong to
  the wrapper, not to what you ran, so they are dropped rather than guessed at.
- **Arguments that cannot be typed back.** A restored command is replayed at a shell prompt, so an
  argument containing a control character would not survive as one argument; the whole vector is
  dropped rather than replayed in part.
- **`--remote` panes.** Both the path and the arguments describe a process on the far host, so
  those panes keep the bare program name.

A saved `command` is a literal record of a command line, so treat a profile like any other file
that quotes your shell history: check it before sharing one that captured a program you passed a
token or password to on the command line.
