---
name: tui-lipan-app-builder
description: >-
  Build and refactor stateful end-user applications with tui-lipan in any
  repository. Use when a task needs Component/State/Message structure, props,
  callback boundaries, focus routing, async commands, reusable shells,
  app-specific styled widget helpers, app-author docs/examples, or integration
  with project patterns. Use `tui-lipan-visual` for anything about how a screen
  looks: sketching a new one, or reviewing and polishing a rendered UI.
---

# TUI-lipan App Building

Build with current project and framework patterns, not generic Rust TUI habits. Prefer concise `ui!`, minimal configuration, reusable styled helpers, keyed focus routing, and command-driven async work.

If this skill conflicts with the current workspace docs or source, follow the workspace.

## Scope and handoffs

- Use this skill for the application layer: `Component`, `State`, `Message`, props, callbacks, focus routing, commands, composition, and project integration.
- Hand off to `tui-lipan-visual` for anything visual: sketching a brand-new screen whose composition is undecided, or reviewing, comparing, and polishing a rendered UI. Bring the stable view helper back here for promotion.
- If the rendered result points to wrong rects, measurement, or allocation, treat it as a sizing-usage bug first: re-check `Length` choices (`Auto` vs `Flex` vs fixed), container-vs-leaf defaults, padding, and gaps before suspecting the framework. Confirm a widget's documented `Length` defaults before overriding them.

## Start With Project Truth

Start by locating the project's real source of truth before applying framework habits.

Read the smallest useful local set first:

- `Cargo.toml` and `Cargo.lock` to confirm the tui-lipan version, enabled features, and workspace shape
- `README.md`, `docs/`, `AGENTS.md`, and app-specific guides for local conventions and verification commands
- `src/`, `examples/`, and tests for the patterns the project already uses for shells, state, messages, focus, and theme
- existing `ui!`, `mockup!`, `Component`, and widget usage sites to match imports, composition style, and naming

If the workspace is the tui-lipan framework repo, read the relevant framework docs directly:

- `docs/quick-start.md` for imports, feature flags, app config, and `mockup!`
- `docs/macros.md` for `ui!` syntax, constructor keys, child sugar, control flow, and mixed builder usage
- `docs/components.md` for `Component`, `Update`, `child()`, props, callbacks, and commands
- `docs/styling.md` for inheritance, theme precedence, color, padding, length defaults, and palettes
- `docs/focus.md` for focus routing, key bubbling, and accordion focus policy
- `docs/keybindings.md` for keymap.conf, chord parsing, and `tui_lipan::input`
- `docs/patterns.md` for reusable shells, overlays, coalescing, and anti-patterns
- `docs/widgets/index.md` plus the relevant widget category docs for exact widget APIs

If the workspace is an app repo and local docs are thin, inspect the dependency version and then consult matching upstream tui-lipan docs or Context7 documentation for that version.

Read bundled references only when needed:

- `references/app-patterns.md` for the default build workflow and app-author rules
- `references/widget-selection.md` for picking widgets, wrapper-vs-raw-widget checks, payload gotchas, and defaults
- `references/examples-map.md` for local-first and upstream example lookup by app type

## Follow This Workflow

1. Inspect the nearest existing app entry point, screen module, or example before introducing new structure.
2. Decide whether the task is a design sketch, a reusable app shell, or a full stateful component.
3. If the look is unsettled, sketch first with `tui-lipan-visual`; do not bury layout experiments inside state/message plumbing.
4. Convert the stable sketch or view helper to a `Component` only when state, messages, lifecycle, keyboard handling, or async work are needed.
5. Write view code in `ui!` by default; use builder helpers where parameterized reuse is clearer.
6. Extract repeated chrome and configured widgets into named helper functions or composite widgets that return `Element`.
7. Add stable keys before wiring focus, overlays, or dynamic children.
8. Push blocking work into `ctx.link().command(...)`; use keyed commands with `TaskPolicy::LatestOnly` for live search or filtering.
9. For user-visible UI changes, run a visual checkpoint before final validation.
10. Verify with the project-local workflow at the end.

## Full App Delivery Rules

When building a complete tui-lipan app, treat the framework's test and render loop
as core development infrastructure, not optional polish.

- Use `tui-lipan-rag` or local upstream docs/source to confirm current widget APIs, defaults, feature flags, and examples before guessing.
- Tell delegated subagents which tui-lipan skills and project rules apply; do not assume they inherited the visual/testing workflow.
- Make behavior tests the source of truth for keyboard flows, message routing, async commands, and regression-prone state transitions.
- Make visual evidence mandatory for meaningful UI work: sketch uncertain screens first, then inspect `TestBackend` snapshots and PNGs at narrow, realistic, and roomy viewports.
- If the app needs a clean pattern that tui-lipan lacks, either improve tui-lipan in the framework repo or document the framework gap instead of hiding a brittle workaround in app code.
- Keep app docs/examples aligned with user-visible behavior, especially controls, keybindings, setup, and validation commands.

## Use Visual Checkpoints

Do not judge a visible UI change from source alone when tui-lipan can render the result headlessly.

- For a brand-new screen whose visual shape is still moving, switch to `tui-lipan-visual` and iterate on a kept `Sketch` plus PNGs before adding state plumbing.
- For an existing or newly implemented screen, capture it with no code at all: `TUI_LIPAN_SNAPSHOT=/tmp/app.png cargo snap <example>`, then inspect artifacts at realistic, narrow, and roomy viewports.
- When layout vanishes or a bar/panel is suspiciously sized, capture with `UiSnapshotOptions::diagnostic()` so zero-area nodes and chrome appear in markdown snapshots.
- Bring stable view helpers and app-specific styled helpers back here for `Component`, props, callback, focus, command, and integration work.
- Never write a snapshot harness you intend to delete. Capture a running app through `TUI_LIPAN_SNAPSHOT`, or keep the sketch file.
- Keep this skill focused on app structure. Let `tui-lipan-visual` own the detailed `Sketch`, `TestBackend`, `UiSnapshot`, and PNG workflow.

## Prefer These App-Building Rules

- Follow the project's import style first. If there is no stronger local convention, use `use tui_lipan::prelude::*;`.
- Prefer `ui!` in `view()` for readability, autocomplete, and easier formatting.
- Treat `rsx!` as legacy. Only keep or touch it when matching existing code or a narrow compatibility need.
- Prefer helper functions returning `Element` for reusable shells; use a composite widget struct when a builder-style API is helpful; use a nested `Component` only when that reused region owns state or events.
- Keep mutable UI data in `State`; keep immutable inputs and callbacks in `Properties`.
- Derive `Clone + PartialEq` on every props struct.
- Mount component instances, not just types, so DI stays easy.
- Use `Element::empty()` for empty conditional branches instead of placeholder text.

## Avoid Framework-Specific Mistakes

- Do not set defaults explicitly. If docs mark a value as default, omit it.
- Do not repeat `bg` on every child or sub-style. Set shared background on the nearest parent that paints it.
- Do not use `Frame` for plain layout. Use it only for border, header/footer labels, tabs, clipping, or decoration.
- Do not block in `update()` or `view()`.
- Do not use `TaskPolicy::QueueAll` for filter-as-you-type.
- Do not forget stable `.key(...)` values on dynamic children and focus targets.
- Do not assume `fg` inherits from the parent. Set text color explicitly where needed.

## Reuse UI Intentionally

When the same configured UI appears in multiple places, extract it instead of copying props around.

Treat these helpers as the app's small design-system layer. A helper can be as
small as `primary_button(label)` or `panel(title, child)`, but it should own the
colors, padding, borders, focus chrome, spacing, default sizing, and widget
variant choices that define this app's visual language. Call sites should pass
semantic differences such as labels, keys, selected state, children, and
callbacks; they should not repeat long builder chains just to recreate the same
look.

This prevents UI drift: the same action button, panel, toolbar, badge, or list
row should not slowly acquire different padding, colors, borders, or focus styles
in different screens. Before adding another raw widget chain, look for the local
helper that already represents that app-specific primitive, or create one at the
first repeated use.

Prefer this progression:

1. View helper returning `Element` for simple shells and mockup-to-app reuse.
2. Small composite widget struct for reusable builder-style panels.
3. Nested `Component` with props and callback props when the reused area needs local state, lifecycle, or scoped messages.

Keep `ui!` focused on composition. Move chrome, repeated styling, and prepared widget configuration into helpers.

## Style With Minimal Noise

- Set app-wide defaults with `App::theme(...)`.
- Use `ThemeProvider` for subtree theming.
- Prefer `Color::rgb(...)` when exact contrast matters.
- Let containers fill space with their default `Length::Flex(1)` unless you need something else.
- Let leaves size naturally with `Length::Auto` unless the layout requires fixed or proportional sizing.
- Use focus styling on the container around an interactive region so active panels are obvious.

## Handle Interaction Correctly

- Use `ctx.request_focus(...)` and `ctx.has_focus_within_key(...)` for panel routing.
- Focused `Button` activates `on_click` on plain `Enter` and `Space`; custom `on_key` runs first and can consume the key.
- In headless tests, use `TestBackend::focus_next()` / `focus_prev()` for traversal and `TestBackend::focused_key()` to assert the keyed focus target.
- Use `KeyUpdate::handled(...)` to stop bubbling and `KeyUpdate::unhandled(...)` to allow it.
- Mirror controlled widget state in the parent only when the parent truly needs ownership.
- Emit child-to-parent communication through callback props, not shared mutable access.
- Use state flags for overlays and dialogs.
- Use `ctx.toast()` for transient feedback.

## Finish With Repo Validation

Follow project-local scripts and validation commands first.

When the workspace provides a helper for formatting macro-heavy Rust files, use it before the normal checks. In the tui-lipan framework repo this is `./scripts/format-rust-with-macros <changed-files>`; it runs `ui-fmt`, then `rsx-fmt`, then `rustfmt`.

Then run:

1. the smallest project-local verification command that covers the change
2. `cargo build` or `cargo check` if the project does not provide a better default
3. `cargo clippy` when enabled in the project
4. `cargo fmt`
5. `cargo test` or the smallest relevant test target
