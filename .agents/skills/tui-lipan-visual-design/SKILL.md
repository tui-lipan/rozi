---
name: tui-lipan-visual-design
description: >-
  Review, verify, and polish existing tui-lipan UIs with headless visual
  snapshots. Use when a component/screen already exists, after a UI code change,
  or when checking chrome, spacing, truncation, focus, contrast, viewport
  behavior, or regressions without a live terminal. Use `tui-lipan-ui-sketch`
  instead for design-first creation of new screens; use `tui-lipan-app-builder`
  for full stateful app/component structure.
---

# TUI-lipan Visual Review

You do **not** need a human staring at a terminal to judge an existing tui-lipan UI. Render it headlessly, export a snapshot, and read the result like a design review artifact.

If this skill conflicts with the current workspace docs or source, follow the workspace.

## Scope and boundaries

- Review or polish an existing screen, component, example, or app flow.
- Verify titles, selection, focus, truncation, masking, contrast, or viewport behavior.
- Check how a screen looks after a code change.
- Compare before/after at multiple viewport sizes.
- Debug "it looks wrong" reports when source alone is ambiguous.

Use `tui-lipan-ui-sketch` first when the user is asking for a brand-new screen whose layout is not settled. Pair with `tui-lipan-app-builder` for component structure and `tui-lipan-layout-debug` when rects/measurement are wrong.

## Review loop

Copy this checklist and iterate until the snapshot looks right:

```
Visual review progress:
- [ ] Pick a realistic viewport (often 80x24; also try narrow/wide breakpoints)
- [ ] Render the existing component/screen
- [ ] Export snapshot (markdown default; PNG when judging design; JSON when structured parsing helps)
- [ ] Read ## Widgets (semantics) and ## Render (fixed grid)
- [ ] Exercise key states (selection, focus, empty, error, modal open)
- [ ] Re-export and compare
- [ ] Remove or gate any temporary snapshot harness before finishing
```

**Default rule:** read a snapshot before declaring visual polish done.

## Fastest path: headless snapshot

Add a focused test, example, or small binary - render, capture, inspect output:

```rust
use tui_lipan::TestBackend;
use tui_lipan::prelude::*;

let mut backend = TestBackend::new(MyScreen);
backend.set_viewport(Rect { x: 0, y: 0, w: 80, h: 24 });
backend.render();

let snapshot = backend.capture_ui_snapshot_with_margin(
    20,
    8,
    &UiSnapshotOptions::default(),
);
std::fs::write("/tmp/ui-snapshot.md", snapshot.to_markdown())?;

#[cfg(feature = "ui-snapshot-png")]
std::fs::write("/tmp/ui-snapshot.png", snapshot.to_png_default())?;
```

Run it:

```bash
cargo test my_screen_snapshot -- --nocapture
cargo run --example ui_snapshot --features ui-snapshot-json,ui-snapshot-png
```

Framework repo reference: `examples/ui_snapshot.rs`, `tests/ui_snapshot.rs`, `docs/components.md`.

## What to read in a snapshot

| Section | Use it for |
|---------|------------|
| `## Focus` / `focus_key` | Which widget owns keyboard focus |
| `## Widgets` | Kind, key, rect, selection, labels, values, masking |
| `## Render` | Fixed-width ASCII grid - spacing, clipping, alignment |

Semantic fields worth checking:

- `selected_index`, `scroll_offset`, `item_labels`, `total_items`
- `value_masked` (secrets must not leak in `value`)
- `placeholder` vs `label` on inputs
- `checkbox_state` for tri-state checkboxes

## Exercise UI states before judging

```rust
backend.render();
backend.focus_next();
backend.render();
backend.dispatch(MyMsg::OpenModal).ok();
backend.render();
backend.set_viewport(Rect { x: 0, y: 0, w: 40, h: 12 });
backend.render();
```

Capture after each meaningful state change.

## Use PNG capture for design judgment

Markdown and JSON snapshots are best for structure and assertions; PNG capture is
the artifact to inspect for spacing, color, focus chrome, hierarchy, and flex
behavior. For design review, capture with `capture_ui_snapshot_with_margin(20, 8,
&UiSnapshotOptions::default())` or `capture_frame_with_margin(20, 8)` so the UI
has enough spare room to expose flex layout issues.

PNG output uses antialiased real-font text by default when a system font is
available, with font8x8 bitmap rendering as the fallback. Use a font family/path
for system or Nerd Font captures; force bitmap rendering for deterministic coarse
cell screenshots and fallback-style reviews. Default discovery checks a small
monospace/Nerd Font stack plus generic `monospace`, so set `font_family` or
`font_path` explicitly for Cascadia, Hack, IBM Plex Mono, or project-specific
fonts.

## Export formats

`PngOptions` and `PngTextRenderer` are not in `prelude::*`; import them from the
crate root with `use tui_lipan::{PngOptions, PngTextRenderer};` when customizing
PNG output.

| Need | API |
|------|-----|
| Agent-readable report (default) | `snapshot.to_markdown()` |
| Structured parsing | `ui-snapshot-json` feature: `to_json()` |
| Design judgment PNG | `ui-snapshot-png` feature: `snapshot.to_png_default()` |
| Custom PNG bytes | `ui-snapshot-png` feature: `snapshot.to_png(&PngOptions)` or `frame.to_png(&PngOptions)` |
| PNG bytes with errors | `ui-snapshot-png` feature: `try_to_png_default()` or `try_to_png(&PngOptions)` |
| Layout-only ASCII | `capture_frame().to_fixed_grid()` |
| Colors/styles | `capture_frame().to_ansi()` or `to_ansi_diff(prev)` |

```rust
let options = UiSnapshotOptions {
    max_list_items: 5,
    ..Default::default()
};
let snapshot = backend.capture_ui_snapshot_with_options(&options);
```

## Live running apps

```rust
// In component State:
slot: UiSnapshotSlot,

ctx.request_ui_snapshot_to("/tmp/ui-snapshot.md");
ctx.request_ui_snapshot_to("/tmp/ui-snapshot.png"); // ui-snapshot-png; current viewport
ctx.request_ui_snapshot_to_slot(&ctx.state.slot);

if let Some(snap) = ctx.state.slot.take() { /* inspect */ }
```

Delivery is after the next paint, not synchronous from `update()`.

## Visual review checklist

- Hierarchy: frames/panels read clearly; titles not clipped
- Density: enough padding; no accidental edge-to-edge text
- Selection/focus: `focus_key` and `selected_index` match the scenario
- Empty/error states: render those explicitly
- Overlays: modals/popovers appear in widgets + render grid
- Responsive: re-check narrow and wide viewports
- Secrets: masked inputs show `value_masked`, not raw values

## Temporary harness pattern

```rust
#[test]
fn design_review_dashboard() {
    let mut backend = TestBackend::new(Dashboard);
    backend.set_viewport(Rect { x: 0, y: 0, w: 80, h: 24 });
    backend.render();
    let md = backend.capture_ui_snapshot().to_markdown();
    eprintln!("{md}");
    assert!(md.contains("Expected title"));
}
```

## Additional resources

- API reference: [references/snapshot-api.md](references/snapshot-api.md)
- New-screen sketching: `tui-lipan-ui-sketch`
- App structure: `tui-lipan-app-builder`
- Layout bugs: `tui-lipan-layout-debug`
