# Agent skill

Rozi ships one built-in [Agent Skill](https://github.com/tui-lipan/rozi/blob/master/skills/rozi/SKILL.md)
that tells a coding agent how to control panes and sessions through the CLI.

Install it once; supported agents then discover it from the provider-neutral location.

## Install

Project-local (from the repository you want the agent to use):

```bash
rozi skill install
```

writes:

```text
.agents/skills/rozi/SKILL.md
```

User-wide:

```bash
rozi skill install --global
```

writes:

```text
~/.agents/skills/rozi/SKILL.md
```

(`%USERPROFILE%\.agents\skills\rozi\SKILL.md` on Windows.)

Re-run `rozi skill install` after upgrading Rozi to refresh an outdated copy. The installed file
always matches the skill embedded in that binary.

```bash
rozi skill status [--global]
rozi skill uninstall [--global]
```

## Claude compatibility

`.agents/skills` is the canonical location. Claude Code currently looks under `.claude/skills`, so
when a Claude CLI (`claude` or `claude-code`) is on `PATH`, install also creates a compatibility
entry:

| | Linux / macOS | Windows |
| --- | --- | --- |
| Canonical skill | `.agents/skills/rozi` | `.agents/skills/rozi` |
| Claude entry | directory symlink | directory junction |
| Fallback | — | managed copy of `SKILL.md` |

A project install uses a relative symlink where practical, so moving the project does not break it.
Failure to create the Claude entry does not roll back the canonical `.agents` install.

`rozi skill uninstall` removes only a Rozi-managed compatibility link or copy. It never deletes an
unrelated directory at `.claude/skills/rozi`.

## Print and `--skill`

```bash
rozi skill print
rozi --skill
```

Both write the embedded `SKILL.md` to stdout with no extra heading. `--skill` remains as a
compatibility alias for `skill print`.

The in-repo source is [`skills/rozi/SKILL.md`](../skills/rozi/SKILL.md), which Agent Skills tooling
can read directly from a checkout.

See [Control socket](control.md) for the commands the skill describes.
