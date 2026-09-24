# Agent skill

`rozi` ships an [Agent Skill](../src/skill/SKILL.md): a `SKILL.md` file that teaches coding agents
such as Claude Code how to inspect and control `rozi` panes and sessions with the `rozi` CLI. This
page covers what the skill lets an agent do and how to install, refresh, and remove it.

## What the agent can do

With the skill installed, an agent that you ask to use `rozi` can:

- list panes, read the layout, and capture pane output;
- split panes and run commands in them;
- watch and read coding agents running in other panes through `rozi agents`;
- control a detached named session with `--session`.

The skill tells the agent to act only when you explicitly ask it to use `rozi`, to read live pane
IDs instead of guessing them, and to change only the panes and sessions you name or it created. It
controls the current UI only from inside one of that UI's panes. For the commands themselves, see
[Control CLI](control.md) and [Scripting](scripting.md).

## Install the skill

Install it for one project, from the project root:

```sh
rozi skill install
```

This writes `.agents/skills/rozi/SKILL.md`. Use a project install when the instructions should
travel with one repository.

Or install it for your user, so agents see it in every project:

```sh
rozi skill install --global
```

This writes `~/.agents/skills/rozi/SKILL.md`, or `%USERPROFILE%\.agents\skills\rozi\SKILL.md` on
Windows.

Start or restart the coding-agent session afterwards so it discovers the skill.

## Check, refresh, or remove

Check the project or user installation:

```sh
rozi skill status
rozi skill status --global
```

`rozi skill install` is safe to rerun. It refreshes an unchanged copy that `rozi` installed, and
refuses to replace a modified or untracked copy unless you add `--force`. `rozi` records each
installation's path, content hash, and `rozi` version in its state directory.

When `rozi` manages its own installation, `rozi update` and `rozi update --rollback` also refresh
the global skill and every registered project skill whose content still matches what `rozi`
installed. They leave locally edited copies alone and print a warning. If a skill refresh fails, the
binary update still succeeds and prints a warning. Restart coding-agent sessions to load the
refreshed instructions.

A project skill installed before `rozi` tracked installations is not registered, so an update run
from elsewhere cannot find it. Run the update from that project and `rozi` warns about it; then run
`rozi skill install --force` there to replace and register it. Add `--global` to do the same for the
user copy.

Remove only the installation that `rozi` manages:

```sh
rozi skill uninstall
rozi skill uninstall --global
```

## Print without installing

```sh
rozi skill print
```

This writes the embedded `SKILL.md` to stdout. `rozi --skill` does the same and must be used
without other arguments.

## Claude compatibility entry

`.agents/skills/rozi` is the canonical installation. If `claude` or `claude-code` is on `PATH`,
installation also tries to create `.claude/skills/rozi`, so Claude Code finds the skill:

| Platform | Compatibility entry |
| --- | --- |
| Linux and macOS | Directory symlink |
| Windows | Directory junction, or a managed `SKILL.md` copy when a junction cannot be created |

A project symlink is relative where possible. If the compatibility entry cannot be created, the canonical installation stays in place.
Uninstalling removes the compatibility entry only when `rozi` manages it; an unrelated directory
already at `.claude/skills/rozi` is left alone.
