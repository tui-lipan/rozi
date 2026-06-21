# UI Snapshot API Reference (tui-lipan)

Quick reference for agent visual design workflows. Confirm against `docs/components.md` and `docs/enums.md` in the workspace version you are using.

## Headless capture

```rust
use tui_lipan::{TestBackend, UiSnapshotOptions};
use tui_lipan::prelude::*;

let mut backend = TestBackend::new(MyComponent);
backend.set_viewport(Rect { x: 0, y: 0, w: 80, h: 24 });
backend.render();

let snapshot = backend.capture_ui_snapshot();
let frame = backend.capture_frame();
```

| Method | Returns | Purpose |
|--------|---------|---------|
| `TestBackend::new(component)` | backend | Headless app host |
| `set_viewport(rect)` | — | Layout size |
| `render()` | — | Full layout + paint |
| `dispatch(msg)` | `Result` | Run `update()` |
| `focus_next()` / `focus_prev()` | — | Move keyboard focus |
| `capture_ui_snapshot()` | `UiSnapshot` | Visual + semantic |
| `capture_ui_snapshot_with_options(&opts)` | `UiSnapshot` | Truncation/chrome toggles |
| `capture_ui_snapshot_with_margin(20, 8, &opts)` | `UiSnapshot` | Fit-to-content plus design-review margin |
| `capture_frame()` | `CapturedFrame` | Pixel buffer only |
| `capture_frame_with_margin(20, 8)` | `CapturedFrame` | Fit-to-content plus design-review margin |
| `focused_key()` / `hovered()` | `Option<Key>` | Interaction helpers |

## UiSnapshot export

| Method | Feature | Output |
|--------|---------|--------|
| `to_markdown()` | always | Agent-readable report |
| `to_json()` | `ui-snapshot-json` | Compact JSON |
| `to_json_pretty()` | `ui-snapshot-json` | Pretty JSON |
| `to_json_with_options(&fmt)` | `ui-snapshot-json` | Optional `include_cells` |
| `to_png(&PngOptions)` | `ui-snapshot-png` | PNG bytes with custom font/bitmap options |
| `to_png_default()` | `ui-snapshot-png` | PNG bytes with `PngOptions::default()` |
| `try_to_png(&PngOptions)` | `ui-snapshot-png` | PNG bytes with encoder errors surfaced |
| `try_to_png_default()` | `ui-snapshot-png` | Default PNG bytes with encoder errors surfaced |

`Cargo.toml` for JSON in app or test:

```toml
[dependencies]
tui-lipan = { version = "...", features = ["ui-snapshot-json"] }
```

## CapturedFrame export

| Method | Output |
|--------|--------|
| `plain_text()` | Trimmed text (lossy for layout) |
| `to_fixed_grid()` | Full-width rows, trailing spaces preserved |
| `to_fixed_grid_lines()` | Row vec |
| `to_ansi()` | Full ANSI repaint |
| `to_ansi_diff(prev)` | Incremental ANSI |
| `to_png(&PngOptions)` | PNG bytes (`ui-snapshot-png`) |
| `try_to_png(&PngOptions)` | PNG bytes with encoder errors surfaced (`ui-snapshot-png`) |
| `cell(x, y)` | Single cell colors/symbol |
| `styled_lines()` | Style runs per row |

## UiWidgetDesc fields (semantic)

| Field | Meaning |
|-------|---------|
| `kind` | `UiWidgetKind` (Frame, List, Input, …) |
| `key` | Reconciliation key |
| `rect` | Bounds in viewport |
| `focused` / `hovered` | Interaction flags |
| `title` / `label` / `value` | Text semantics |
| `placeholder` | Input placeholder (not `label`) |
| `value_masked` | Secret omitted from `value` |
| `checkbox_state` | `CheckboxState` tri-state |
| `selected_index` | List/tab selection |
| `scroll_offset` | Scroll position |
| `item_labels` / `total_items` | Preview + full count when truncated |
| `child_count` | Structural containers |

## UiSnapshotOptions

| Field | Default | Effect |
|-------|---------|--------|
| `include_zero_area` | `false` | Zero-size nodes |
| `include_chrome` | `false` | Spacers/dividers |
| `max_list_items` | `20` | Label preview cap |

## PngOptions (ui-snapshot-png)

`PngOptions` and `PngTextRenderer` are available from the crate root, not
`prelude::*`:

```rust
#[cfg(feature = "ui-snapshot-png")]
use tui_lipan::{PngOptions, PngTextRenderer};
```

| Field | Default | Effect |
|-------|---------|--------|
| `cell_width` | `8` | Cell width before scaling |
| `cell_height` | `16` | Cell height before scaling |
| `scale` | `2` | Output cell scale multiplier |
| `default_fg` | `Color::White` | Fallback foreground |
| `default_bg` | `Color::Black` | Fallback background |
| `render_cursor` | `true` | Draw visible cursor outline |
| `text_renderer` | `PngTextRenderer::Auto` | Auto font rendering with bitmap fallback; `Font` or `Bitmap` to force a path |
| `font_family` | `None` | Preferred system font family, such as a Nerd Font |
| `font_path` | `None` | Explicit font file path; takes precedence over family lookup |

For design review, prefer `capture_ui_snapshot_with_margin(20, 8,
&UiSnapshotOptions::default())` or `capture_frame_with_margin(20, 8)`, then write
`snapshot.to_png_default()` to inspect spacing, color, focus chrome, and flex
behavior.

PNG output uses antialiased real-font text by default when a system font is
available, with font8x8 bitmap rendering as the fallback. Use `font_family` or
`font_path` for system/Nerd Font captures, especially when the desired family is
outside the small default monospace/Nerd Font discovery stack (for example
Cascadia, Hack, or IBM Plex Mono). Force `PngTextRenderer::Bitmap` for
deterministic coarse cell output and fallback-style screenshot deliverables.

## Live delivery (running app)

| API | Behavior |
|-----|----------|
| `Context::request_ui_snapshot_to(path)` | Write markdown, `.json`, or `.png` after next paint |
| `Context::request_ui_snapshot_to_slot(&slot)` | Deliver to `UiSnapshotSlot` |
| `UiSnapshotSlot::take()` | Consume delivered snapshot |
| `UiSnapshotSlot::is_ready()` | Poll without consuming |

Pending requests are **last-writer-wins**. Both request methods schedule a **full repaint** so idle apps still deliver.

## JSON wire conventions (ui-snapshot-json)

- Colors: stable strings — `rgb(r,g,b)`, `indexed(n)`, snake_case names (not `Debug`)
- Checkbox: `"unchecked"`, `"checked"`, `"indeterminate"`
- Keys: reconciliation key strings via `Key::as_ref()`

## Markdown conventions

- User strings escaped for backticks; embedded newlines shown as `\n` inside inline code
- `item_labels` rendered as nested bullet list
- `## Render` contains a fenced fixed grid
