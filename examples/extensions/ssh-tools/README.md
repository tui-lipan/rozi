# SSH tools extension

This independent example uses only the public extension manifest/environment and the
`rozi pick --json`, `rozi notify`, and `rozi new-pane` commands. `ssh-tools.hosts` lists concrete
aliases from the standard user SSH config and opens the selected host in a focused pane.

## Prerequisites

- Rozi with extension API 1;
- Python 3 available as `python`;
- an OpenSSH client available as `ssh` on `PATH`;
- a UTF-8 user config at `~/.ssh/config` (also `%USERPROFILE%\.ssh\config` through Python's home
  directory handling on Windows).

The picker follows `Include` directives recursively, including quoted paths and glob patterns.
Relative includes resolve below `~/.ssh`, matching user-config behavior. Include cycles are
deduplicated. Concrete tokens from every `Host` line are shown; negated tokens and tokens containing
SSH wildcard syntax (`*`, `?`, or `[`) are skipped. This is syntactic discovery, not a full
evaluation of conditional `Match` blocks.

Descriptions come from `ssh -G -- <alias>` where that command succeeds promptly. They show the
effective user, host, non-default port, and proxy jump without connecting to the remote host.
Configurations whose `Match exec` rules do work may still cause that work while `ssh -G` evaluates
the config; each description probe is bounded to two seconds.

## Install and run

Copy or clone this directory anywhere below Rozi's user extension directory, then run:

```bash
rozi check-extension ./ssh-tools
rozi run-action reload-config
rozi run-action ssh-tools.hosts
```

The installation directory may be renamed; `id = "ssh-tools"` remains the public identity. Press
`r` in the picker to reread the root config, traversed includes, and effective descriptions. A
missing config, an empty concrete alias set, and unreadable included files appear as disabled
structured rows. A missing `ssh` executable or pane-launch failure produces an error notification.

## Command-string boundary

`rozi new-pane` currently accepts the pane program as one command string, not structured argv. The
extension therefore does not splice the selected alias into shell text. It URL-safe-base64 encodes
the discovered `ssh` path and alias, renders only fixed/safe tokens with `shlex.join` for the
POSIX `/bin/sh -c` default or `subprocess.list2cmdline` for the Windows `cmd /D /S /C` default, and
starts a short Python trampoline in the pane. The trampoline decodes both values and replaces itself
with `ssh` through `os.execv` and the argv `["ssh", "--", alias]`.

This protects spaces, Unicode, quotes, and shell metacharacters in aliases and paths at the current
string-only boundary. An arbitrarily customized Rozi `command_shell` may use quoting rules unlike
the platform default and is outside what an extension can discover through the public environment.

## Manual checks

1. Run `rozi check-extension ./ssh-tools` and confirm `ssh-tools.hosts` uses a direct argv manifest.
2. Add concrete aliases, a wildcard-only pattern, and an included file to the user config. Include
   at least one quoted path or alias containing spaces/Unicode if supported by the installed
   OpenSSH build.
3. Open `ssh-tools.hosts`; confirm concrete aliases appear, wildcard/negated patterns do not, and
   effective descriptions match `ssh -G -- <alias>`.
4. Change an included file, press `r`, and confirm the open picker updates in place.
5. Select a test host and confirm a new pane receives focus and runs SSH for exactly that alias.
6. Temporarily test with the root config absent, an unreadable include, and `ssh` absent from
   `PATH`; confirm disabled rows or a concise error notification replace a traceback.
