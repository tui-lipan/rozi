# Platform and security

Rozi targets Linux, macOS, and Windows. Higher-level modules should use `src/platform/` for paths,
filesystem security, process inspection, IPC, server lifecycle, notifications, shell integration,
and OS identity. This migration is incremental. Existing direct `cfg` or `std::os` use outside the
platform layer is not permission to add another one; move touched behavior behind the boundary when
the change remains focused.

Read `src/platform/mod.rs` before changing platform code. Its module documentation records what is
implemented and what remains unavailable on an OS.

## Endpoint invariants

- Runtime directories remain private. Preserve Unix owner/mode/symlink checks and Windows
  current-user SID DACL and reparse-point checks.
- Keep `PIPE_REJECT_REMOTE_CLIENTS` and `FILE_FLAG_FIRST_PIPE_INSTANCE` on Windows named pipes.
  They prevent remote access and pipe-name squatting.
- Windows discovery files are hints, never authorities. Derive the pipe name rather than reading it
  from a discovery entry, then require the authenticated protocol handshake.
- Session and control endpoint names remain scoped and validated.
- Session integration tests use typed protocol and platform IPC helpers from `tests/common`. Do not
  replace them with raw Unix sockets.

## Other boundaries

- Shell integration emits an executable basename, not a command line. It never edits shell
  dotfiles, PowerShell profiles, or the Windows `AutoRun` registry key.
- Keep `[clipboard].enable_osc52` enforcement intact; clipboard and OSC52 can expose copied data.
- Plain stdout styling goes through `platform::ansi`, including `NO_COLOR`, `CLICOLOR`,
  `CLICOLOR_FORCE`, and `TERM=dumb`. Do not emit raw escapes at call sites.
- Keep local config, state, cache, runtime endpoints, logs, and unsanitized captures out of the
  repository. The agent-screen workflow may add scrubbed captures as test fixtures.

## Checks over untrusted input

Pane output, link targets, and names or paths that arrive from another machine are untrusted. When
a change adds a check over them, or turns opt-in behavior into a default, do this before opening
the pull request:

- Judge the input as the OS reads it, not as the string looks: Windows drops trailing periods and
  spaces from file names, Unix runs by permission and content rather than by extension, and a path
  or `file:` URL means something only on the machine it names.
- Write the bypass attempts as test cases, not only the case the check was written for.
- When removing a gate or making something default, list what the gate prevented, such as a
  file-system or network probe on the UI thread.
- Walk every client and host OS pairing when a path or file name crosses machines.
- Review the diff for security separately from writing it, and state any assumption you could not
  verify under Verification rather than settling it silently.

## Windows checks

This workspace cannot execute Windows code. Type-check it before pushing platform changes:

```bash
cargo check --target x86_64-pc-windows-gnu --all-targets
cargo clippy --target x86_64-pc-windows-gnu --all-targets
```

CI is the first runtime verification on Windows.
