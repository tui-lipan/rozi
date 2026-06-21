# Project Profiles and Pane Identity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add user-controlled pane identity plus project profile save/restore so hyprmux can launch and recreate named terminal workspaces without pretending to support tmux-style detach/reattach.

**Architecture:** Keep PTYs owned by the single UI process. Store stable pane metadata and serializable project profiles separately from live terminal state, then create fresh PTYs from profile recipes during startup. Reuse the existing `Action`/`Msg`/overlay patterns for renaming and the existing `DwindleTree`/workspace model for restoring layouts.

**Tech Stack:** Rust 2024, `tui-lipan` path dependency with `terminal` and `theme-reload`, `serde`, `toml`.

## Global Constraints

- Rust edition is 2024 and `rust-version` is `1.85`.
- Do not add a daemon, IPC server, or detach/reattach.
- Restored panes spawn fresh PTYs; no shell process resurrection is attempted.
- Preserve the animation invariant: animate position/opacity, but avoid animated terminal size changes that spam PTY resize/SIGWINCH.
- Keep command palette/help metadata generated from `input::command_bindings()`.
- Config/read/profile failures must warn through startup messages or toasts; do not silently pretend a file was loaded.
- Do not commit unless the user explicitly asks for commits; commit checkpoints below are review checkpoints unless commit permission is granted.

---

## File Structure

- Create `src/identity_ops.rs`: rename-pane modal actions and helpers.
- Create `src/profiles.rs`: serializable profile schema, save/load, state snapshot, and restore helpers.
- Create `docs/project-profiles.md`: user-facing profile format and limitations.
- Modify `src/main.rs`: new modules/messages, optional startup profile, multi-pane startup spawning.
- Modify `src/state.rs`: pane identity fields, profile config, rename modal state, profile-aware constructor.
- Modify `src/view.rs`: custom title/status rendering and rename overlay.
- Modify `src/input.rs`, `src/actions.rs`, `src/update.rs`, `src/focus_ops.rs`: action/message/focus plumbing.
- Modify `src/pane_lifecycle.rs`: per-pane PTY config and multi-startup spawn command.
- Modify `src/config.rs`: parse `[profile] path = "..."`.
- Modify `README.md`: short docs link and feature mention.

## MVP Behavior

1. Pane title priority: custom title -> terminal OSC title -> fallback `pane.title`.
2. Empty rename input clears the custom title.
3. Profiles load at startup from `[profile].path` in `hyprmux.toml`.
4. Restore recreates workspaces, panes, floating/fullscreen flags, layout kind, split ratios, dwindle tree shape, focus, cwd, command, and names.
5. `command` runs through the configured shell as `shell -lc <command>`.
6. `Save project profile` writes current layout to `[profile].path`; without a path it shows an explanatory toast.
7. Runtime reload of a profile is not included because running PTYs need lifecycle policy.
8. Saving a hand-built session records names, geometry, focus, and layout. It does not discover live shell cwd/argv; `cwd` and `command` are only saved when already present in pane identity, usually because the pane came from a profile.
9. A profile `command` that exits will exit its pane. Long-lived command panes should use an exec-shell idiom such as `cargo run; exec ${SHELL:-/bin/sh}` when the user wants the pane to remain open.

## Recommended Execution Split

- **PR/phase A: Pane identity and rename** — Tasks 1-2. This is independently shippable and low risk.
- **PR/phase B: Project profiles** — Tasks 3-8 plus docs. Start this phase with Task 3 as a de-risking spike because recursive TOML serialization of `ProfileTree` is the most likely schema issue.
- If implementing everything in one branch, Task 3 may still be run first before Task 1 because it does not depend on pane identity.

---

### Task 1: Pane identity model

**Files:** Modify `src/state.rs:288-453`.

**Interfaces:** Consumes `Pane::new(id, scrollback, floating_rect) -> Pane`. Produces `PaneIdentity`, `PaneRenameState`, `Pane::display_title`, `Pane::set_custom_title`, `Pane::clear_custom_title`, `Pane::subtitle`.

- [ ] **Step 1: Write failing tests.** Add `src/state.rs` tests for: custom title beats terminal title; blank title clears to `None`; `subtitle()` returns command before cwd.
- [ ] **Step 2: Run failure check.** Run `cargo test pane_display_title pane_subtitle -- --nocapture`. Expected: FAIL because APIs do not exist.
- [ ] **Step 3: Add `PaneIdentity`.** Add `#[derive(Clone, Debug, Default, PartialEq, Eq)] pub struct PaneIdentity { pub custom_title: Option<String>, pub profile_name: Option<String>, pub cwd: Option<String>, pub command: Option<String> }` and `set_custom_title` that trims and stores `None` for empty strings.
- [ ] **Step 4: Add rename modal state.** Add `pub struct PaneRenameState { pub target: PaneId, pub input: TextInput }` with `new(target, initial)`.
- [ ] **Step 5: Extend `Pane`.** Add `pub identity: PaneIdentity` to `Pane`, initialize it in `Pane::new`, and add `display_title`, `set_custom_title`, `clear_custom_title`, and `subtitle` methods.
- [ ] **Step 6: Extend `State`.** Add `pub rename: Option<PaneRenameState>` to `State` and initialize it as `None`.
- [ ] **Step 7: Verify.** Run `cargo test pane_display_title pane_subtitle empty_custom_title_is_cleared -- --nocapture && cargo fmt --check`. Expected: PASS.
- [ ] **Step 8: Review checkpoint.** Inspect `git diff -- src/state.rs`. Commit only if explicitly allowed: `git add src/state.rs && git commit -m "feat: add pane identity model"`.

---

### Task 2: Rename pane command and modal

**Files:** Create `src/identity_ops.rs`; modify `src/main.rs`, `src/input.rs`, `src/actions.rs`, `src/update.rs`, `src/focus_ops.rs`, `src/view.rs`.

**Interfaces:** Consumes `Pane::set_custom_title`, `State::rename`, `find_pane_mut`. Produces `Action::RenamePane`, `Msg::CloseRenamePane`, `Msg::RenamePaneChanged(InputEvent)`, `Msg::SubmitRenamePane`, `identity_ops::{open_rename_pane, apply_rename_pane, close_rename_pane}`, `focus_ops::request_rename_focus`, `view::rename_input_key`.

- [ ] **Step 1: Write failing helper test.** In `src/identity_ops.rs`, test `rename_pane_in_workspaces(&mut [Workspace], PaneId, &str)` by creating a workspace with one pane, renaming it to `logs`, and asserting `pane.identity.custom_title.as_deref() == Some("logs")`.
- [ ] **Step 2: Run failure check.** Run `cargo test rename_pane_by_id_sets_custom_title -- --nocapture`. Expected: FAIL.
- [ ] **Step 3: Implement helpers.** Add `rename_pane_in_workspaces`, `open_rename_pane`, `apply_rename_pane`, and `close_rename_pane` in `src/identity_ops.rs`. Opening rename should close palette/help/search, set `Mode::Normal`, seed the input from existing custom title, and focus the rename input.
- [ ] **Step 4: Wire module/action.** Add `mod identity_ops;` in `main.rs`. Add `Action::RenamePane`, command binding id `pane.rename`, label `Rename pane`, keys `n`, category `Panes`, palette `true`, and key mapping for `n`/`N`.
- [ ] **Step 5: Wire messages.** Add `CloseRenamePane`, `RenamePaneChanged(InputEvent)`, and `SubmitRenamePane` to `Msg`; handle them in `update.rs`; ensure `Action::RenamePane` does not immediately refocus the terminal in the `Msg::RunAction` post-action focus logic.
- [ ] **Step 6: Wire focus.** Add `request_rename_focus(ctx)` in `focus_ops.rs`, using `ctx.request_focus(view::rename_input_key())`.
- [ ] **Step 7: Render overlay.** Add a `Modal` in `view.rs` with `Input::bound(&rename.input)`, placeholder `Pane name, empty clears custom title`, `Esc` to close, `Enter` to submit, and key `rename_input_key()`.
- [ ] **Step 8: Render identity.** Change pane title rendering to `pane.display_title(pane.terminal.title())`; append `pane.subtitle()` after the title when present without changing the frame/titlebar structure.
- [ ] **Step 9: Verify.** Run `cargo test rename_pane_by_id_sets_custom_title pane_display_title -- --nocapture && cargo test && cargo fmt --check`. Expected: PASS.
- [ ] **Step 10: Review checkpoint.** Inspect `git diff -- src/main.rs src/input.rs src/actions.rs src/update.rs src/focus_ops.rs src/view.rs src/identity_ops.rs src/state.rs`. Commit only if explicitly allowed.

---

### Task 3: Profile schema and TOML round-trip

**Files:** Create `src/profiles.rs`; modify `src/main.rs`.

**Interfaces:** Produces `HyprmuxProfile`, `WorkspaceProfile`, `PaneProfile`, `ProfileTree`, `ProfileRect`, `ProfileLayoutKind`, `ProfileSplitAxis`, `HyprmuxProfile::{to_toml_string, from_toml_str}`.

**Risk gate:** Run this task early in the profiles phase. Recursive `ProfileTree` TOML round-tripping is the highest schema risk; do not proceed to snapshot/restore until this test passes.

- [ ] **Step 1: Write failing round-trip test.** Create a profile with one workspace, one named pane, `cwd`, `command`, and `ProfileTree::Leaf { pane: 0 }`; serialize with `to_toml_string`; parse with `from_toml_str`; assert equality.
- [ ] **Step 2: Run failure check.** Run `cargo test profile_round_trips_named_pane_and_tree -- --nocapture`. Expected: FAIL.
- [ ] **Step 3: Add module.** Add `mod profiles;` in `src/main.rs`.
- [ ] **Step 4: Implement schema.** In `src/profiles.rs`, define serde-backed structs for profile/workspace/pane/tree/rect. Use `#[serde(default)]` on structs and `#[serde(tag = "kind", rename_all = "kebab-case")]` on `ProfileTree`.
- [ ] **Step 5: Implement conversions.** Implement defaults, `HyprmuxProfile::to_toml_string`, `HyprmuxProfile::from_toml_str`, and conversions between profile layout/axis/rect types and `LayoutKind`/`SplitAxis`/`FloatRect`.
- [ ] **Step 6: Fallback if TOML rejects the tree shape.** If internally tagged recursive `ProfileTree` fails to round-trip, switch only the tree encoding to an externally tagged enum or a table shape with explicit `leaf`/`split` fields, then re-run the same round-trip test. Do not weaken the test.
- [ ] **Step 7: Verify.** Run `cargo test profile_round_trips_named_pane_and_tree -- --nocapture && cargo fmt --check`. Expected: PASS.
- [ ] **Step 8: Review checkpoint.** Inspect `git diff -- src/main.rs src/profiles.rs`. Commit only if explicitly allowed.

---

### Task 4: Snapshot current state into a profile

**Files:** Modify `src/profiles.rs`.

**Interfaces:** Consumes `State`, `Workspace`, `Pane`, `DwindleTree`. Produces `profile_from_state(state: &State) -> HyprmuxProfile`.

- [ ] **Step 1: Write failing snapshot test.** Create `State::new`, set first pane custom title/cwd/command, push a floating second pane with a known `FloatRect`, call `profile_from_state`, and assert pane metadata and rect are preserved.
- [ ] **Step 2: Run failure check.** Run `cargo test snapshot_preserves_custom_name_and_floating_rect -- --nocapture`. Expected: FAIL.
- [ ] **Step 3: Implement snapshot conversion.** Implement `profile_from_state`, `workspace_profile_from_state`, and `profile_tree_from_dwindle`. Build `HashMap<PaneId, usize>` so serialized tree leaves refer to pane indices instead of runtime pane IDs.
- [ ] **Step 4: Preserve honest save semantics.** Do not infer live shell cwd or argv. Snapshot only `pane.identity.cwd` and `pane.identity.command`; panes spawned manually will usually save those fields as absent.
- [ ] **Step 5: Verify tree fidelity.** Ensure `DwindleTree::Split { axis, ratio, first, second }` maps recursively to `ProfileTree::Split` with the same axis and ratio.
- [ ] **Step 6: Verify.** Run `cargo test snapshot_preserves_custom_name_and_floating_rect profile_round_trips_named_pane_and_tree -- --nocapture && cargo fmt --check`. Expected: PASS.
- [ ] **Step 7: Review checkpoint.** Inspect `git diff -- src/profiles.rs`. Commit only if explicitly allowed.

---

### Task 5: Restore state from profile

**Files:** Modify `src/profiles.rs`, `src/state.rs`.

**Interfaces:** Produces `restore_state_from_profile(config, theme, profile) -> State`, `State::from_profile(...) -> State`.

- [ ] **Step 1: Write failing restore test.** Create a profile with two workspaces, active workspace `1`, layout `Master`, focused pane index `1`, one tiled named pane and one floating named pane. Restore it and assert `active_workspace`, `layout_kind`, `focused_pane`, pane names, floating flag, `tile_tree.is_some()`, and `next_pane_id`.
- [ ] **Step 2: Run failure check.** Run `cargo test restore_recreates_focus_identity_and_tree -- --nocapture`. Expected: FAIL.
- [ ] **Step 3: Implement restore.** Create `(0..WORKSPACE_COUNT).map(Workspace::new)`, assign new sequential `PaneId`s starting at `1`, copy identity fields from `PaneProfile`, reconstruct `DwindleTree` with pane-index-to-id mapping, and fall back to `append_tiled_window` for tiled panes when no profile tree exists.
- [ ] **Step 4: Add state wrapper.** Add `State::from_profile(config, theme, profile)` in `state.rs`, delegating to `profiles::restore_state_from_profile`.
- [ ] **Step 5: Empty profile fallback.** If the profile has no panes, return `State::new(config, theme)`.
- [ ] **Step 6: Verify.** Run `cargo test restore_recreates_focus_identity_and_tree snapshot_preserves_custom_name_and_floating_rect -- --nocapture && cargo test && cargo fmt --check`. Expected: PASS.
- [ ] **Step 7: Review checkpoint.** Inspect `git diff -- src/profiles.rs src/state.rs`. Commit only if explicitly allowed.

---

### Task 6: Config loading and startup profile restore

**Files:** Modify `src/state.rs`, `src/config.rs`, `src/profiles.rs`, `src/main.rs`.

**Interfaces:** Produces `HyprmuxProfileConfig { path: Option<PathBuf> }`, `[profile].path` parsing, `profiles::load_profile`, `HyprmuxApp.startup_profile`.

- [ ] **Step 1: Write failing config test.** In `src/config.rs`, parse `[profile] path = "~/code/hyprmux/dev.toml"` into `FileConfig` and assert `parsed.profile.path.as_deref() == Some("~/code/hyprmux/dev.toml")`.
- [ ] **Step 2: Run failure check.** Run `cargo test file_config_parses_profile_path -- --nocapture`. Expected: FAIL.
- [ ] **Step 3: Add profile config.** Add `HyprmuxProfileConfig { path: Option<PathBuf> }` to `state.rs`; add `pub profile: HyprmuxProfileConfig` to `HyprmuxConfig`; initialize with default.
- [ ] **Step 4: Parse profile config.** Add `ProfileFileConfig { path: Option<String> }` to `config.rs`, add `profile: ProfileFileConfig` to `FileConfig`, and set `config.profile.path = Some(expand_path(path))` when non-empty.
- [ ] **Step 5: Load profile file.** In `profiles.rs`, add `load_profile(path: &Path) -> Result<HyprmuxProfile, String>` using `std::fs::read_to_string` and `HyprmuxProfile::from_toml_str`, with error messages including `path.display()`.
- [ ] **Step 6: Use startup profile.** Add `startup_profile: Option<profiles::HyprmuxProfile>` to `HyprmuxApp`; in `create_state`, call `State::from_profile` when present; in `main()`, load `loaded.config.profile.path` before moving config and push success/error messages into `startup_messages`.
- [ ] **Step 7: Verify.** Run `cargo test file_config_parses_profile_path profile_round_trips_named_pane_and_tree -- --nocapture && cargo test && cargo fmt --check`. Expected: PASS.
- [ ] **Step 8: Review checkpoint.** Inspect `git diff -- src/state.rs src/config.rs src/profiles.rs src/main.rs`. Commit only if explicitly allowed.

---

### Task 7: Launch restored panes with cwd/command

**Files:** Modify `src/pane_lifecycle.rs`, `src/main.rs`.

**Interfaces:** Produces `pty_config_for_pane(config: &HyprmuxConfig, pane: &Pane) -> TerminalPtyConfig`, `startup_command(spawns, theme_tick) -> Option<Command>`.

- [ ] **Step 1: Write failing PTY config test.** In `src/pane_lifecycle.rs`, set `config.shell = Some("/bin/bash")`, `config.cwd = Some("/repo")`, pane cwd `/repo/backend`, pane command `cargo run`, call `pty_config_for_pane`, format with `Debug`, and assert it contains `/bin/bash`, `-lc`, `cargo run`, and `/repo/backend`.
- [ ] **Step 2: Run failure check.** Run `cargo test pane_config_prefers_pane_cwd_and_wraps_command_in_shell -- --nocapture`. Expected: FAIL.
- [ ] **Step 3: Implement per-pane config.** If `pane.identity.command` is non-empty, use `TerminalPtyConfig::new(shell).arg("-lc").arg(command.clone())`; otherwise use existing `pty_config(config)`. Pane cwd overrides config cwd. Always set `term("xterm-256color")`.
- [ ] **Step 4: Implement multi-spawn.** Add `startup_command(spawns, theme_tick)` that loops all `(PaneId, TerminalPtyConfig, Option<Duration>)`, calls `spawn_pty`, sends `Msg::FinishOpen(id)` after optional delay, and sends `Msg::ThemeTick` when needed. Keep `initial_command` as a one-element wrapper.
- [ ] **Step 5: Launch all startup panes.** In `HyprmuxApp::init`, collect every non-closing pane in every workspace and call `startup_command` so profile startup launches all restored panes.
- [ ] **Step 6: Add a multi-pane startup test or smoke fixture.** Exercise at least three restored panes across two workspaces so the new "spawn every restored pane" path is covered, not only the old single-focused-pane path.
- [ ] **Step 7: Verify.** Run `cargo test pane_config_prefers_pane_cwd_and_wraps_command_in_shell -- --nocapture && cargo test && cargo build && cargo fmt --check`. Expected: PASS/build success.
- [ ] **Step 8: Review checkpoint.** Inspect `git diff -- src/pane_lifecycle.rs src/main.rs`. Commit only if explicitly allowed.

---

### Task 8: Save profile command

**Files:** Modify `src/profiles.rs`, `src/input.rs`, `src/actions.rs`.

**Interfaces:** Produces `save_profile(path, profile) -> Result<(), String>`, `Action::SaveProfile`, command entry `profile.save`.

- [ ] **Step 1: Write failing save test.** In `src/profiles.rs`, write `HyprmuxProfile::default()` to a nested temp path under `std::env::temp_dir()`, assert the file contains `version = 1`, then remove the temp directory.
- [ ] **Step 2: Run failure check.** Run `cargo test save_profile_creates_parent_directory_and_file -- --nocapture`. Expected: FAIL.
- [ ] **Step 3: Implement save.** Add `save_profile`: create parent directory, serialize with `to_toml_string`, write file, return descriptive `String` errors.
- [ ] **Step 4: Add command.** In `input.rs`, add `Action::SaveProfile` and a palette command id `profile.save`, label `Save project profile`, empty keys, category `Profile`.
- [ ] **Step 5: Dispatch save.** In `actions.rs`, require `ctx.state.config.profile.path`; if absent, toast `Set [profile].path in hyprmux.toml before saving a profile.` Otherwise snapshot with `profiles::profile_from_state` and write with `profiles::save_profile`.
- [ ] **Step 6: Verify.** Run `cargo test save_profile_creates_parent_directory_and_file -- --nocapture && cargo test && cargo fmt --check`. Expected: PASS.
- [ ] **Step 7: Review checkpoint.** Inspect `git diff -- src/profiles.rs src/input.rs src/actions.rs`. Commit only if explicitly allowed.

---

### Task 9: Docs and final verification

**Files:** Create `docs/project-profiles.md`; modify `README.md`.

**Interfaces:** Produces user-facing docs for pane rename and project profiles.

- [ ] **Step 1: Add docs.** Create `docs/project-profiles.md` explaining that profiles restore layout and launch fresh shells/commands, not live PTY state. Include config sample `[profile] path = "~/code/my-app/hyprmux-profile.toml"`. Document pane fields: `name`, `cwd`, `command`, `floating`, `fullscreen`, `rect`.
- [ ] **Step 2: Document v1 save limitations.** State that saving a hand-built session preserves names/layout/geometry, but does not discover the live shell cwd or command; `cwd` and `command` are saved only when pane identity already knows them.
- [ ] **Step 3: Document command exit behavior.** State that `shell -lc <command>` exits the pane when the command exits, and show the keep-open idiom: `cargo run; exec ${SHELL:-/bin/sh}`.
- [ ] **Step 4: Update README.** Add bullets for project profiles and pane identity, plus a link to `docs/project-profiles.md`.
- [ ] **Step 5: Full verification.** Run `cargo fmt --check`, `cargo test`, `cargo clippy`, `cargo build`, and `git diff --check`. Expected: formatting clean, tests pass, clippy clean, build succeeds, no whitespace errors.
- [ ] **Step 6: Manual smoke test.** Create `/tmp/opencode/hyprmux-profile-smoke.toml` with at least three panes across two workspaces. At least one pane should use command `printf 'profile loaded\n'; exec ${SHELL:-/bin/sh}`. Point a temporary `HYPRMUX_CONFIG` at it, run `cargo run` in a real terminal, then quit with `Ctrl-q`.
- [ ] **Step 7: Smoke expected result.** Startup toast says profile loaded, pane titles show configured names, all restored panes spawn without resize/animation glitches, command prints `profile loaded`, `Prefix+n` opens rename modal, and saving through palette writes profile when `[profile].path` is set.
- [ ] **Step 8: Review checkpoint.** Inspect `git diff -- docs/project-profiles.md README.md`. Commit only if explicitly allowed.

---

## Self-Review

- Pane identity: Tasks 1 and 2.
- Rename command and overlay: Task 2.
- Profile schema: Task 3.
- Snapshot/save profile: Tasks 4 and 8.
- Startup restore: Tasks 5, 6, and 7.
- Fresh PTY launch with cwd/command: Task 7.
- Docs and validation: Task 9.

The plan explicitly excludes detach/reattach and runtime profile reload. Type names are consistent across tasks: `PaneIdentity`, `PaneRenameState`, `HyprmuxProfile`, `WorkspaceProfile`, `PaneProfile`, `ProfileTree`, `ProfileRect`, `Action::RenamePane`, and `Action::SaveProfile`.

Known implementation risks:

- Recursive `ProfileTree` TOML encoding is the highest-risk schema piece; Task 3 is an explicit early risk gate with a fallback encoding path.
- `TerminalPtyConfig` fields are private, so Task 7 tests use `Debug` output. If that proves brittle, replace that single test with a pure helper returning a testable launch spec and have `pty_config_for_pane` consume that helper.
- Multi-pane startup spawn is new behavior; Task 7 and Task 9 require exercising more than one restored pane.
