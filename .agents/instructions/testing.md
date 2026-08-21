# Testing

Unit tests usually live beside Rust modules. Integration and smoke tests live under `tests/`.
Prefer a focused test while editing, then run the baseline checks from the root guide before handoff.

## Isolation

Tests must not write to the developer's config, state, cache, or runtime directories. Rozi actions
can persist sidebar preferences, session autosaves, and shell integration, while a running client
live-reloads config. An unisolated test can alter the UI the developer is using.

- Unit tests get a per-process scratch root through `PlatformEnv::from_process` under `cfg(test)`.
- An integration helper that builds `AppRoot` must call
  `rozi::test_support::isolate_user_dirs()` before constructing its `TestBackend`.
- Never redirect an in-process test with `std::env::set_var` for `HOME`, `XDG_*`, `APPDATA`, or
  `ROZI_CONFIG`; parallel mutation is unsound.
- Passing environment variables to an isolated child process is a separate, safe mechanism.

## Protocol and fixtures

- Session integration tests use `tests/common` and the real typed protocol and platform IPC helpers.
  Do not reimplement framing or use raw Unix sockets.
- Agent detection rules require evidence from the real CLI in `tests/fixtures/agents/<id>.toml`.
  Load `.agents/skills/agent-screens/SKILL.md` for that workflow. Adding a built-in agent requires a
  screen fixture or an explicit corpus-gap admission.

## Benchmarks

Benchmarks are local performance evidence, not timing assertions. `cargo check --all-targets`
compiles them. Run `cargo bench` on a stable, idle machine. Keep benchmark corpora generated and
deterministic; do not add captured terminal output. See `docs/benchmarks.md`.

For visual review and snapshot debugging, load the `tui-lipan-visual` skill and read its
"Rozi capture specifics" section.
