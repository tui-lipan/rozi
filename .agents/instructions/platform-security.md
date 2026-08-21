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

## Windows checks

This workspace cannot execute Windows code. Type-check it before pushing platform changes:

```bash
cargo check --target x86_64-pc-windows-gnu --all-targets
cargo clippy --target x86_64-pc-windows-gnu --all-targets
```

CI is the first runtime verification on Windows.
