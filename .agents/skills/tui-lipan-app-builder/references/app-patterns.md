# App Patterns

## Default Workflow

1. Start from the local project docs and source: `Cargo.toml`, `README.md`, `docs/`, `AGENTS.md`, existing screens, examples, and tests.
2. Confirm whether the workspace is the framework repo or an app built on tui-lipan.
3. Decide whether `mockup!`, a view helper, a composite widget, or a full `Component` is the smallest useful abstraction.
4. Build the screen in `ui!` first.
5. Extract repeated shells and configured widgets into helpers that return `Element`.
6. Add state, messages, callbacks, focus keys, and commands only where needed.
7. Run the local formatting and verification commands.

## Prefer This Shape

- `State` for mutable UI data
- `Msg` for every user or background event
- `update()` for state mutation and command launching
- `view()` for declarative tree composition
- `init()` for startup loading when needed

Use nested components only when a subtree needs its own state or lifecycle. Otherwise prefer reusable view helpers.

## Prefer `ui!` For View Code

Use `ui!` as the default way to express screens, forms, panes, and overlays.

Treat `rsx!` as older syntax that may still exist in legacy code or examples.

Use builder API when it is clearer for:

- reusable panel helpers
- parameterized configured widgets
- places where chained builder methods read better than long `ui!` props

Mix both freely inside the same view.

When working outside the framework repo, search for project-local wrapper widgets or helper constructors before instantiating raw framework widgets directly.

## Reuse Configured UI

Do not duplicate the same `Frame`, `List`, `Input`, or status-row configuration in multiple places.

Extract one of these:

- helper function returning `Element`
- small composite widget struct with a builder API
- nested component with props and callback props

Good candidates for extraction:

- panel chrome
- focused frame styling
- search bars and status rows
- list/table wrappers with consistent spacing and scrollbar behavior
- modal bodies and confirmation footers

## Async And Commands

- Never block in `update()` or `view()`.
- Use `ctx.link().command(...)` for blocking I/O or expensive work.
- Use `ctx.link().command_keyed(..., TaskPolicy::LatestOnly, ...)` for live search and filtering.
- Return `Update::full()` only when visible state changed.
- Return `Update::none()` for no-op events and child-to-parent callback forwarding.

## Focus And Keys

- Give stable `.key(...)` values to dynamic children and focus targets.
- Use `ctx.request_focus(...)` after mode changes, dialog open/close, and panel jumps.
- Use `ctx.has_focus_within_key(...)` to drive active panel chrome.
- Keep global shortcuts in `on_key()`.

## Styling Rules

- Do not set defaults explicitly.
- Do not repeat inherited `bg` on every child.
- Do not expect `fg` to inherit.
- Use `Frame` only when you need chrome.
- Prefer app theme plus targeted overrides over per-widget styling everywhere.
- Prefer `Color::rgb(...)` for precise interactive contrast.

## Common Traps

- Forgetting `Clone + PartialEq` on props
- Using `Text::new("")` instead of `Element::empty()`
- Using `TaskPolicy::QueueAll` for live queries
- Copying the same widget configuration instead of extracting it
- Writing layout-only groups as nested `Frame`s
- Setting `focusable(true)`, `border(false)`, `Length::Flex(1)`, or other defaults explicitly
- Skipping project-local docs and examples in favor of upstream framework docs too early
