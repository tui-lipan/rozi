# Agent skill

Rozi includes an [Agent Skill](../src/skill/SKILL.md) that tells coding agents how to inspect and
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

For managed Rozi installations, `rozi update` and `rozi update --rollback` refresh global and previously registered project
skills when their contents still match what Rozi installed. It leaves locally edited skills alone
and prints a warning. A project skill installed before this tracking was introduced cannot be
identified safely unless you run the update from that project; then Rozi warns about it. Run
`rozi skill install --force` there (or add `--global` for the user copy) to replace and register it.

`rozi skill install` is safe to rerun: it refreshes an unchanged registered copy and refuses to
replace a modified or untracked copy without `--force`. Rozi stores installation paths, the
installed content hash, and the Rozi version in its state directory. If a skill refresh fails,
the binary update still succeeds and prints a warning. Restart coding-agent sessions to load the
refreshed instructions.

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
