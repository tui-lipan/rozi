# Agent skill

Rozi includes an [Agent Skill](../skills/rozi/SKILL.md) that tells coding agents how to inspect and
control Rozi panes with the CLI.

## Install for one project

Run this from the project root:

```sh
rozi skill install
```

Rozi writes:

```text
.agents/skills/rozi/SKILL.md
```

Start or restart the coding-agent session from that project so it can discover the file.

## Install for your user

```sh
rozi skill install --global
```

Rozi writes:

```text
~/.agents/skills/rozi/SKILL.md
```

On Windows the path is `%USERPROFILE%\.agents\skills\rozi\SKILL.md`.

Use a project install when the instructions should travel with one repository. Use a global install
when agents should have the instructions in every project.

## Check, refresh, or remove

Check the project installation:

```sh
rozi skill status
```

Check the user installation:

```sh
rozi skill status --global
```

Run the matching install command again after upgrading Rozi. It replaces the managed skill with the
copy embedded in the current binary.

Remove only the managed project or user installation:

```sh
rozi skill uninstall
rozi skill uninstall --global
```

## Print without installing

```sh
rozi skill print
```

This writes the embedded `SKILL.md` to stdout. `rozi --skill` is an equivalent compatibility form
and must be used without other arguments.

## Claude compatibility entry

`.agents/skills/rozi` is the canonical installation. If `claude` or `claude-code` is available on
`PATH`, installation also tries to create `.claude/skills/rozi`:

| Platform | Compatibility entry |
| --- | --- |
| Linux and macOS | Directory symlink |
| Windows | Directory junction, or a managed `SKILL.md` copy when a junction cannot be created |

A project symlink is relative where possible. Failure to create the compatibility entry does not
remove the canonical installation.

Uninstall removes a compatibility entry only when Rozi manages it. It does not delete an unrelated
directory already present at `.claude/skills/rozi`.

For the commands described by the skill, see [Control CLI](control.md) and
[Scripting](scripting.md).
