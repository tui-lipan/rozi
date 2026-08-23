# Rozi agent guide

Rozi is a cross-platform, modern tiling terminal multiplexer built in Rust on `tui-lipan`. Its
tiling layout and keyboard flow take their cues from the Hyprland window manager.

## Always

- Use Cargo. The crate uses Rust 2024 and supports Rust `1.90` or newer.
- Every PTY belongs to a session server. A client that displays panes attaches to that server, but
  the startup launcher may remain sessionless until the user chooses or creates a session.
- Preserve unrelated worktree changes. Never discard or overwrite changes you did not create.
- Do not edit `target/` or commit generated output, personal config, runtime data, logs, socket
  paths, credentials, or captures unless a test fixture explicitly requires scrubbed capture data.
- Tests must never write to the developer's config, state, cache, or runtime directories. An
  integration test that builds `AppRoot` calls `rozi::test_support::isolate_user_dirs()` first.
- Do not run Cargo commands concurrently in this workspace; they only contend on build locks.
- New OS-specific behavior belongs behind `src/platform/`. Existing exceptions are not precedent.
- Keep user-facing docs in sync with behavior, CLI, configuration, and workflow changes.
- Prefer a clean breaking change over aliases or compatibility shims unless the user asks for
  compatibility or a protocol test intentionally covers version skew.
- The repository is MPL-2.0. Contributions require a DCO sign-off.

## Baseline checks

Use the narrowest useful command while editing. Before handing off a Rust application change, run:

```bash
cargo fmt --all -- --check
cargo test
cargo clippy --all-targets -- -D warnings
git diff --check
```

Run `cargo build` for broader feature work. Report any check you could not run or that failed.

## Read before editing

Read the matching file before you edit in one of these areas. Each carries constraints this guide
does not repeat, and violating one is usually invisible until review or CI.

| Work | Instructions |
| --- | --- |
| Build, run, benchmark, or CI | [.agents/instructions/development.md](.agents/instructions/development.md) |
| User documentation or the docs website | [.agents/instructions/documentation.md](.agents/instructions/documentation.md) |
| Runtime architecture, panes, layouts, or sessions | [.agents/instructions/architecture.md](.agents/instructions/architecture.md) |
| Platform, IPC, shell integration, clipboard, or security | [.agents/instructions/platform-security.md](.agents/instructions/platform-security.md) |
| Actions, commands, config keys, hooks, or environment contracts | [.agents/instructions/config-actions.md](.agents/instructions/config-actions.md) |
| Overlays, prompts, palettes, toasts, or animation | [.agents/instructions/ui.md](.agents/instructions/ui.md) |
| Tests, fixtures, or benchmarks | [.agents/instructions/testing.md](.agents/instructions/testing.md) |
| Headless UI capture or visual debugging | the `tui-lipan-visual` skill, including its "Rozi capture specifics" section |
| `tui-lipan`, dependency sources, or lockfile updates | [.agents/instructions/framework.md](.agents/instructions/framework.md) |
| Commits, history repair, push, or release work | [.agents/instructions/git-contributions.md](.agents/instructions/git-contributions.md) |
| Agent detection rules or screen fixtures | [.agents/skills/agent-screens/SKILL.md](.agents/skills/agent-screens/SKILL.md) |
| Extension development | [.agents/skills/rozi-extension/SKILL.md](.agents/skills/rozi-extension/SKILL.md) |

## Repository notes

- `CLAUDE.md` is a symlink to this file. Edit `AGENTS.md`; do not replace the symlink.
