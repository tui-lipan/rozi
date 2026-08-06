# UI Snapshot API Reference (tui-lipan)

Quick reference for agent visual design workflows. Confirm against `docs/components.md` and `docs/enums.md` in the workspace version you are using.

## Headless snapshot from the environment (no code)

Set `TUI_LIPAN_SNAPSHOT` and run any app or example normally. `AppRunner::run`
renders off-screen, writes one artifact, and returns without entering raw mode -
so it needs no tty and works on CI.

```bash
TUI_LIPAN_SNAPSHOT=/tmp/app.png cargo snap todo
```

| Variable | Default | Effect |
|----------|---------|--------|
| `TUI_LIPAN_SNAPSHOT` | unset | Output path; enables headless mode. Format from extension |
| `TUI_LIPAN_SNAPSHOT_VIEWPORT` | `100x30` | `WIDTHxHEIGHT` layout size |
| `TUI_LIPAN_SNAPSHOT_FRAMES` | `1` | Render/message passes before capture |
| `TUI_LIPAN_SNAPSHOT_FOCUS` | `0` | Focus advances before capture |
| `TUI_LIPAN_SNAPSHOT_KEYS` | unset | Key script, e.g. `tab,tab,enter` |
| `TUI_LIPAN_SNAPSHOT_DIAGNOSTIC` | unset | `1` uses `UiSnapshotOptions::diagnostic()` |

`cargo snap <example>` = `cargo run --features ui-snapshot-png,ui-snapshot-json --example`.

## Sketch (kept design captures)

```rust
use tui_lipan::{Result, Sketch};

Sketch::view("login", view)      // Fn() -> Element, via Mockup
    .viewport(80, 24)
    .fit(20, 8)
    .focus_next(1)
    .write()?;
```

| Method | Effect |
|--------|--------|
| `Sketch::view(name, fn)` | Sketch a plain `Fn() -> Element` |
| `Sketch::component(name, c)` | Sketch a `Component` with default props |
| `.viewport(w, h)` | Exact-size capture; repeatable |
| `.fit(margin_w, margin_h)` | Content-minimum plus margin capture |
| `.focus_next(n)` | Advance focus `n` times before capturing |
| `.keys(script)` | Dispatch a key script, e.g. `"tab,enter"` |
| `.options(opts)` | Describe options (e.g. `diagnostic()`) |
| `.markdown(b)` / `.png(b)` / `.json(b)` | Toggle formats; md + png default on |
| `.dir(path)` | Override output directory |
| `.baseline(dir)` | Compare captures against stored baseline images |
| `.tolerance(r)` | Max differing-pixel fraction counted as a match (default `0.0`) |
| `.quiet(b)` | Suppress printing written paths |
| `.write()` | Run every pass; returns `Result<Vec<PathBuf>>` |
| `.check()` | Run and return `Vec<BaselineComparison>` |
| `.assert_baseline()` | Run and fail if any capture regressed |

## Visual baselines

```rust
Sketch::view("login", view)
    .viewport(80, 24)
    .baseline("tests/ui-baselines")
    .assert_baseline()?;      // first run records, later runs compare
```

`TUI_LIPAN_UPDATE_BASELINES=1` accepts current output as the new baseline.
Changed captures write a `*.diff.png` beside the baseline: unchanged pixels
dimmed, changed pixels magenta.

Baseline captures force `PngTextRenderer::Bitmap` - the default font discovery
differs per machine, which makes pixel comparison meaningless across CI and
local. `BaselineOutcome::is_regression()` is the single check for pass/fail;
`Created` (first run) is not a failure.

Defaults: `80x24` plus a fit-to-content pass, written to `target/ui-sketches/`
(override with `TUI_LIPAN_SKETCH_DIR`). Live in `examples/sketches/`, registered
in that directory's `main.rs` - no `Cargo.toml` entry required.

## Headless capture

```rust
use tui_lipan::prelude::*;
use tui_lipan::{TestBackend, UiSnapshotOptions};

let mut backend = TestBackend::new(MyComponent);
backend.set_viewport(Rect { x: 0, y: 0, w: 80, h: 24 });
backend.render();

let snapshot = backend.capture_ui_snapshot();
let frame = backend.capture_frame();
```

| Method | Returns | Purpose |
|--------|---------|---------|
| `TestBackend::new(component)` | backend | Headless app host |
| `set_viewport(rect)` | - | Layout size |
| `render()` | - | Full layout + paint |
| `dispatch(msg)` | `Result` | Run `update()` |
| `focus_next()` / `focus_prev()` | - | Move keyboard focus |
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
| `to_png(&PngOptions)` | `ui-snapshot-png` | `Result<Vec<u8>>` with custom font/bitmap options |
| `to_png_default()` | `ui-snapshot-png` | `Result<Vec<u8>>` with `PngOptions::default()` |

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
| `to_png(&PngOptions)` | `Result<Vec<u8>>` (`ui-snapshot-png`) |
| `cell(x, y)` | Single cell colors/symbol |
| `styled_lines()` | Style runs per row |

## UiWidgetDesc fields (semantic)

| Field | Meaning |
|-------|---------|
| `kind` | `UiWidgetKind` (Frame, List, Input, ...) |
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
`snapshot.to_png_default()?` to inspect spacing, color, focus chrome, and flex
behavior. Both PNG methods return `Result`, so an encoder failure surfaces at the
call instead of landing as a 0-byte file.

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

Pending requests are last-writer-wins. Both request methods schedule a full repaint so idle apps still deliver.

## JSON wire conventions (ui-snapshot-json)

- Colors: stable strings: `rgb(r,g,b)`, `indexed(n)`, snake_case names (not `Debug`)
- Checkbox: `"unchecked"`, `"checked"`, `"indeterminate"`
- Keys: reconciliation key strings via `Key::as_ref()`

## Markdown conventions

- User strings escaped for backticks; embedded newlines shown as `\n` inside inline code
- `item_labels` rendered as nested bullet list
- `## Render` contains a fenced fixed grid
