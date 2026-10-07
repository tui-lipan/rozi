# Actions, configuration, and external contracts

## Actions and commands

`input.rs` is the source of truth for `Action`, `Action::id()`, and `BINDABLE_ACTIONS`.
`commands/catalog.rs` owns `BUILTIN_COMMANDS`, including labels, descriptions, groups, and default keys.
`commands/registry.rs` registers those commands, resolves bindings, and computes availability.
Help and command palettes render from that registry. Adding an action normally requires `input.rs`
and `commands/catalog.rs`.

`[keys]` may rebind built-in actions or define `run` and `send` commands. Keep parsing in
`config/input.rs` and routing through the existing action/command paths.

## Configuration keys

Adding or renaming a config key requires all three:

1. The serde model in `config/file.rs` or its focused sibling module.
2. A reference row in `docs/configuration.md`.
3. The default as an inert setting in `examples/config.toml`.

In `examples/config.toml`, prose uses `# like this` and settings use `#key = value`. Tests
uncomment settings and load the whole file, so keep that distinction. Extend the reference example
instead of adding an unlinked snippet.

## Process and environment contracts

- `ROZI_CONFIG` and `--config <PATH>` select config for every command that loads it.
- `ROZI_SOCKET` points control commands at a live UI.
- The session server stamps `ROZI_SESSION_INSTANCE` (its `SessionInstanceId`) into every shared
  pane on spawn, and blanks it for local panes. The CLI sends it as `source_session`; a UI routes
  pane-originated recording requests by it rather than by the session on screen, and refuses a
  bare `source_pane` without it.
  Agent reports and releases carrying it also route to the caller's current or background
  attachment instead of the session on screen.
- Spawned panes receive `ROZI=1`, `ROZI_PANE`, `ROZI_SOCKET`, and `ROZI_BIN`. Remote panes suppress
  local `ROZI_SOCKET` and `ROZI_BIN`, and so does a pane spawned headlessly through
  `rozi --session <NAME> split` or restored by resurrection, which has no UI to name.
- `agents report`/`release` with no `--target`, `--socket`, `--session`, or `ROZI_SOCKET` go to the
  pane's own session server, found by matching `ROZI_SESSION_INSTANCE` against the `instance` each
  server reports in `SessionInfo`. The request keeps `source_session`, and a session endpoint
  refuses a request whose `source_session` is another instance (`instance-mismatch`).
- `--session <NAME>` before a control command routes it to that session server instead of a UI.
  `src/session/server/headless.rs` decides which commands a server can answer; adding a
  `ControlCommand` means choosing a side there.
- A session endpoint ignores `source_pane`/`ROZI_PANE`: a pane id carries no session identity, and
  `--session` names a different namespace than the caller is in. Targets there are explicit.
- The server's `ServerSettings` are a startup snapshot, except agent definitions (refreshed by
  `ReloadAgents`), the host sleep policy (refreshed by controller-only `ReloadSleepPolicy`), and
  spawn policy — `[[rules]]`, shell, command runner — which `reload_spawn_policy` re-reads for each headless spawn, since they describe the next pane and a
  detached session has no client to send a reload message.
- `PaneIdentity::env` carries per-spawn values that must never be persisted. File-tree actions pass
  paths through `ROZI_FILE`; never splice a selected filename into a command.
- Hook commands receive `ROZI_EVENT`, event fields, `ROZI_SOCKET`, and `ROZI_BIN`, plus
  `ROZI_REMOTE_HOST` for remote attachments. Use `events::EventKind::ALL` as the current event list
  rather than copying a count into documentation.

See `docs/configuration.md`, `docs/keybindings.md`, `docs/control.md`, and `docs/hooks.md` for the
public contracts.
