# Examples Map

Start with local examples, demos, tests, and existing screens in the current workspace. Use the upstream tui-lipan examples below only when the workspace does not already contain a closer match.

## Start Here

- Local app entry points and feature demos that already use the same shell or widget family
- `examples/todo.rs` for a practical app with focus chrome, modal confirmation, toasts, and startup loading
- `examples/showcase.rs` for a broad widget tour
- `examples/dashboard.rs` for a multi-panel dashboard shell

## Use Case Guide

- Forms and field widgets (includes ComboBox): `examples/forms.rs`
- Multi-select flows: `examples/multi_select.rs`
- Search palettes: `examples/search_palette_hub.rs`
- Logs and filtering: `examples/log_viewer.rs`
- Resizable panes: `examples/splitter.rs`
- Markdown editor and preview sync: `examples/markdown_editor_sync.rs`
- Inline mode apps: `examples/inline.rs`, `examples/inline_list_picker.rs`
- File tree plus terminal tooling: `examples/terminal_filetree_devtools.rs`
- Large IDE-style shell: `examples/lazygit.rs`
- Focus routing across panes: `examples/search_lists.rs`
- Rich messaging/chat layout: `examples/messenger.rs`
- Polished home or landing screen composition: `examples/opencode_home.rs`
- Mouse-heavy arcade/game interaction: `examples/whack_a_mole.rs`

## What To Notice In Examples

- Stable keys on panels, inputs, and dynamic children
- Focus-driven border or panel styling
- Reusable shell helpers instead of copied layout code
- Typed event payloads mapped into message enums
- `Update::with_command(...)` for background work
- `TaskPolicy::LatestOnly` for live queries
- Conditional overlays driven by state flags
- Shared state for synchronized widgets such as editor plus preview

## Learn In This Order

1. the nearest local screen, example, or test that already matches the task
2. `examples/todo.rs`
3. `examples/forms.rs`
4. `examples/search_palette_hub.rs`
5. `examples/splitter.rs`
6. `examples/lazygit.rs`
7. a feature-specific example that matches the task
