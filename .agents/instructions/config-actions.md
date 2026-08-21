# Actions, configuration, and external contracts

## Actions and commands

`input.rs` is the source of truth for `Action`, `Action::id()`, and `BINDABLE_ACTIONS`.
`commands.rs` owns `BUILTIN_COMMANDS`, including labels, descriptions, groups, and default keys.
Help and command palettes render from that registry. Adding an action normally requires both files.

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
- Spawned panes receive `ROZI=1`, `ROZI_PANE`, `ROZI_SOCKET`, and `ROZI_BIN`. Remote panes suppress
  local `ROZI_SOCKET` and `ROZI_BIN`.
- `PaneIdentity::env` carries per-spawn values that must never be persisted. File-tree actions pass
  paths through `ROZI_FILE`; never splice a selected filename into a command.
- Hook commands receive `ROZI_EVENT`, event fields, `ROZI_SOCKET`, and `ROZI_BIN`, plus
  `ROZI_REMOTE_HOST` for remote attachments. Use `events::EventKind::ALL` as the current event list
  rather than copying a count into documentation.

See `docs/configuration.md`, `docs/keybindings.md`, `docs/control.md`, and `docs/hooks.md` for the
public contracts.
