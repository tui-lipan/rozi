---
name: tui-lipan-app-builder
description: >-
  Build, refactor, and review stateful end-user applications with tui-lipan in
  any repository. Use when a task needs component structure, state/messages,
  props, callback boundaries, focus routing, async commands, reusable shells,
  app-author docs/examples, or integration with project patterns. Use
  `tui-lipan-ui-sketch` first for design-only creation of new screens whose look
  is not settled; use `tui-lipan-visual-design` for snapshot review/polish of
  existing UIs.
---

# TUI-lipan App Building

Build with current project and framework patterns, not generic Rust TUI habits. Prefer concise `ui!`, minimal configuration, reusable view shells, keyed focus routing, and command-driven async work.

If this skill conflicts with the current workspace docs or source, follow the workspace.

## Scope and handoffs

- Use this skill for the application layer: `Component`, `State`, `Message`, props, callbacks, focus routing, commands, composition, and project integration.
- Hand off to `tui-lipan-ui-sketch` when the user wants a brand-new screen and the visual composition is still undecided. Bring the stable view helper back here for promotion.
- Hand off to `tui-lipan-visual-design` when a screen already exists and the task is to review, compare, or polish its rendered appearance.
- Hand off to `tui-lipan-layout-debug` when the rendered result points to measurement, rect allocation, or cache bugs.

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
3. If the look is unsettled, sketch first with `tui-lipan-ui-sketch`; do not bury layout experiments inside state/message plumbing.
4. Convert the stable sketch or view helper to a `Component` only when state, messages, lifecycle, keyboard handling, or async work are needed.
5. Write view code in `ui!` by default; use builder helpers where parameterized reuse is clearer.
6. Extract repeated chrome and configured widgets into helper functions or composite widgets that return `Element`.
7. Add stable keys before wiring focus, overlays, or dynamic children.
8. Push blocking work into `ctx.link().command(...)`; use keyed commands with `TaskPolicy::LatestOnly` for live search or filtering.
9. Verify with the project-local workflow at the end.

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
- Do not use `Frame` for plain layout. Use it only for border, title, status, tabs, clipping, or decoration.
- Do not block in `update()` or `view()`.
- Do not use `TaskPolicy::QueueAll` for filter-as-you-type.
- Do not forget stable `.key(...)` values on dynamic children and focus targets.
- Do not assume `fg` inherits from the parent. Set text color explicitly where needed.

## Reuse UI Intentionally

When the same configured UI appears in multiple places, extract it instead of copying props around.

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
