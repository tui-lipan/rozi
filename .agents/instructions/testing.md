# Testing

Unit tests usually live beside Rust modules. Integration and smoke tests live under `tests/`.
Prefer a focused test while editing, then run the baseline checks from the root guide before handoff.

## Coverage and test design

- Test observable production behavior. UI composition tests render `AppRoot` and production views,
  rather than miniature apps that reconstruct their layout, styling, or callbacks. Assert outcomes,
  not merely that rendering completes.
- Prefer real temporary files, typed channels, and isolated processes over mocks. Use a test double
  at an external boundary when the real dependency would make the test unreliable or impractical;
  do not mock the Rozi behavior being tested or mutate global process state to substitute for it.
- Framework widget and rendering algorithms belong in `tui-lipan` tests. Check coverage against the
  version pinned in `Cargo.lock` before adding or removing overlapping tests. Keep Rozi composition,
  configuration, routing, and lifecycle assertions even when the underlying widget is covered.
- When reducing a test matrix, inspect the production paths. Similar output or shared row ownership
  does not make two cases equivalent if they exercise separate composition or update branches.
  Preserve each distinct behavioral contract, including transport-specific assertions.
- Extend existing scenarios and parameterize meaningful variants instead of copying setup or
  building every combination of independent options. Fixture checks use the actual benchmark or
  tool builders. Full `AppRoot` views use `rozi::test_support::on_large_stack`; nested pane, border,
  or animation renders use `rozi::test_support::on_deeper_stack`.
- Include detailed frames in assertion failures or explicit diagnostics, rather than dumping them
  on every successful run.

## Isolation

Tests must not write to the developer's config, state, cache, or runtime directories. Rozi actions
can persist sidebar preferences, session autosaves, and shell integration, while a running client
live-reloads config. An unisolated test can alter the UI the developer is using.

When a test spawns a worker, call `isolate_user_dirs()` and acquire config and persisted-state locks
on that worker thread, not the caller. The scopes below apply on the thread that performs the work.

- Isolation covers reads as well as writes. The runtime directory is what session discovery
  enumerates, so an unisolated sweep lists the developer's live sessions and grows a row list a test
  has already measured. Endpoints go in a per-process directory inside the real `XDG_RUNTIME_DIR`
  rather than under the scratch root, because a socket path must stay under `SUN_LEN`.
- Unit tests get a per-process scratch root through `PlatformEnv::from_process` under `cfg(test)`.
- Tests that read or write `last-sessions.json`, `recent-remotes`, `saved-hosts`, `reconnect-hosts`, or
  `host-sessions.json` must hold `lock_persisted_state()` on the thread that performs the I/O for
  the whole read/write/assert sequence. Persist helpers take that lock themselves; nested calls on
  the same thread reuse it.
- An integration helper that builds `AppRoot` must call
  `rozi::test_support::isolate_user_dirs()` before constructing its `TestBackend`.
- Every test in one binary shares one scratch config file, and Settings actions save into it. A
  test that reads the saved config back, writes the file itself, or needs it in a known state holds
  `rozi::test_support::lock_config_file()` for the whole sequence, on the thread that dispatches.
  Saves made by that thread's own dispatches go through; other tests' saves wait.
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
deterministic; do not add captured terminal output. See `dev/benchmarks.md`.

A render with nothing to assert - a colour or layout judgement made by looking - is a sketch, not a
test. Add it as a scenario in `tools/ui-sketches/main.rs` (`cargo run --features ui-snapshot
--example ui_sketches -- <scenario>`) instead of an assertion-free `#[test]`.

For headless component captures and snapshot debugging, load the `tui-lipan-visual` skill and read
its "Rozi capture specifics" section. For live pane/UI review and recordings through rozi, use
`.agents/skills/rozi-visual/SKILL.md`.
