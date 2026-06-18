---
name: tui-lipan-ui-sketch
description: >-
  Sketch new tui-lipan screens before wiring state: use a design-first loop of
  `Mockup` view helper → PNG render → visual inspection → refinement →
  promotion. Use when creating a new screen, dashboard layout, form, settings
  page, panel, or visual variant whose look is not settled. Use before writing a
  `Component` when visual composition is still moving. Distinct from
  `tui-lipan-visual-design` (reviews/verifies existing UIs) and
  `tui-lipan-app-builder` (structures stateful apps with messages, props, focus,
  and async work).
---

# TUI-lipan UI Sketch

Render sketches to PNG and inspect them directly. Do not reason about tui-lipan layout blind when a visual artifact is available.

The mistake to avoid: writing a full `Component` with state, messages, and `update()` for a feature whose layout isn't even nailed down yet. You spend effort on plumbing, the user looks at the result, the layout is wrong, and you tear it down. Sketch first, plumb later.

If this skill conflicts with the current workspace docs or source, follow the workspace.

## Scope and boundaries

- The user asks to build a new screen, view, panel, or layout.
- The look matters and you cannot see the live terminal.
- You're tempted to write `Component`, `State`, `Message` boilerplate before you've decided what the screen looks like.
- A previous attempt looked wrong and you need to redesign visually.

If the screen already exists and the user wants to *verify* or *polish* it, use `tui-lipan-visual-design` instead.

If the screen needs interaction, async, or state from day one and the layout is already well understood, use `tui-lipan-app-builder` to structure the full component.

## Sketch loop

```
sketch loop:
[ ] write/edit the view in a Mockup closure
[ ] render at one or more viewports → save PNG
[ ] open the PNG and look at it (Read tool on the .png path)
[ ] note what's wrong: hierarchy, spacing, colors, focus chrome
[ ] edit the view, repeat
[ ] exercise data states (empty, populated, overflow)
[ ] only promote to Component once layout is stable
```

The PNG step is non-optional. Markdown snapshots are good for **assertions**; PNGs
are required for **design judgment** (color, focus chrome, proportion, whitespace
weight). PNGs use antialiased real-font text by default when a system font is
available, with bitmap rendering as the fallback. Use a font family/path for Nerd
Font captures, or force bitmap rendering for deterministic coarse cell
deliverables. Do not skip looking at the actual rendered image.

## Step 1 — sketch with Mockup

`Mockup` is a `Component` impl that takes a closure returning `Element`. No `State`, no `Message`, no `update()`. It exists for exactly this workflow.

```rust
use tui_lipan::prelude::*;

fn login_screen() -> Element {
    Frame::new()
        .title("Sign In")
        .border(true)
        .child(
            VStack::new()
                .gap(1)
                .padding(1)
                .child(Text::new("Welcome back."))
                .child(Input::new("alice@example.com").placeholder("Email"))
                .child(Input::new("").mask(Some('*')).placeholder("Password"))
                .child(
                    HStack::new()
                        .gap(2)
                        .child(Button::new("Cancel"))
                        .child(Button::new("Log In")),
                ),
        )
        .into()
}
```

Keep view functions plain — no state references, no callbacks wired. The sketch is about composition and visual feel, not behavior.

## Step 2 — render to PNG

Use `TestBackend` to render headlessly and `to_png()` to encode. **Capture at multiple viewports** — a single viewport hides flex/layout behavior.

```rust
use std::fs;
use tui_lipan::{PngOptions, TestBackend};
use tui_lipan::prelude::*;

fn main() -> Result<()> {
    let mut backend = TestBackend::new(Mockup::new(login_screen));

    // Prime layout so content_min_size() can measure.
    backend.set_viewport(Rect { x: 0, y: 0, w: 80, h: 24 });
    backend.render();

    // Tight: exactly minimum content size — pure structure check.
    let tight = backend
        .capture_frame_with_margin(0, 0)
        .to_png(&PngOptions::default());
    fs::write("/tmp/sketch_tight.png", &tight)?;

    // Roomy: min + margin — reveals flex distribution / floating elements.
    let roomy = backend
        .capture_frame_with_margin(20, 8)
        .to_png(&PngOptions::default());
    fs::write("/tmp/sketch_roomy.png", &roomy)?;

    // Real terminal: what a typical user sees.
    backend.set_viewport(Rect { x: 0, y: 0, w: 100, h: 30 });
    backend.render();
    let real = backend.capture_frame().to_png(&PngOptions::default());
    fs::write("/tmp/sketch_real.png", &real)?;

    Ok(())
}
```

Run with the feature flag:

```bash
cargo run --example my_sketch --features ui-snapshot-png
```

Don't forget root-only imports: `PngOptions` and `PngTextRenderer` are not in the
prelude.

## Step 3 — look at the PNG

Use the `Read` tool on each `.png` path. The image renders inline.

What to judge from the image:

| Check | What you're looking for |
|-------|-------------------------|
| **Hierarchy** | Can you scan top-to-bottom and find the primary action? |
| **Focus chrome** | Is the focused widget visually distinct? (typically teal/accent border) |
| **Selection** | Is the selected list/tab obvious? |
| **Whitespace** | Is empty space *intentional* or did flex distribute it accidentally? |
| **Proportions** | Do panels/columns feel balanced at the roomy viewport? |
| **Density** | Are there enough breathing-room rows/cols between sections? |
| **Truncation** | Do long strings clip cleanly or leak past borders? |

Common smells the roomy capture exposes that the tight one hides:
- Buttons floating in mid-screen, disconnected from the form they belong to (VStack's default Flex(1) per child)
- Sidebar growing wider than intended (HStack Flex(1) on a sized panel)
- Status bar drifting off the bottom (missing explicit row sizing)

## Step 4 — exercise data states

A login form looks fine with `alice@example.com`. It looks wrong with `a-very-long-corporate-email-address@subsidiary.parent-corp.example.com`. Don't ship a sketch that's only been tested with placeholder strings.

Minimum state matrix:

- **Empty**: no items, no value, blank inputs
- **Populated**: realistic typical data
- **Overflow**: very long string, many items (e.g. 200 list items)
- **Edge content**: unicode, mixed widths, special chars

Re-render and re-capture for each.

## Step 5 — promote or hand off

Stay in `Mockup` while the layout is still moving and the answer to all of these is *no*:

- Does this view own state that changes over time?
- Does it dispatch messages or call callbacks?
- Does it need `update()` logic?
- Does it route focus or handle keys beyond defaults?
- Does it run async commands?

After the visual shape is stable, promote when **any** answer becomes yes. If the promoted screen is part of a larger flow, hand off to `tui-lipan-app-builder` for state, messages, props, focus, and async wiring. The promotion is mechanical:

```rust
// Before (Mockup):
fn login_screen() -> Element { /* ... */ }

// After (Component):
struct LoginScreen;

impl Component for LoginScreen {
    type Message = LoginMsg;
    type Properties = ();
    type State = LoginState;

    fn create_state(&self, _: &()) -> Self::State { LoginState::default() }

    fn view(&self, ctx: &Context<Self>) -> Element {
        // Same composition as the Mockup closure, now reading ctx.state
    }

    fn update(&mut self, msg: Self::Message, ctx: &mut Context<Self>) -> Update { /* ... */ }
}
```

Keep the view body shape identical to the mockup — only state reads change. That preserves the look you settled on.

## Anti-patterns

- **Writing `Component` first.** You'll spend an hour wiring messages for a layout the user will reject in 30 seconds. Sketch first.
- **One PNG, one viewport, done.** You'll miss the floating-buttons class of bugs. Tight + roomy + real terminal is the minimum.
- **Markdown-only review for design.** Markdown grids are for asserting structure. They cannot show color, focus chrome, or flex behavior. Always look at the PNG too.
- **Placeholder data forever.** Realistic + adversarial data exposes truncation, alignment, and overflow bugs that a 4-char placeholder string never will.
- **Promoting before the layout is stable.** Every change after promotion costs more because state plumbing has to follow. Get the picture right first.
- **Deleting your sketch example.** Once you've promoted to a real Component, keep the sketch file (in `examples/` or a `dev-snapshots/` dir) — regressions during refactors are caught by re-running it.

## Quick-reference cheat sheet

```rust
// Minimum sketch
use tui_lipan::{PngOptions, TestBackend};
use tui_lipan::prelude::*;
use std::fs;

fn view() -> Element { /* your composition */ }

fn main() -> Result<()> {
    let mut backend = TestBackend::new(Mockup::new(view));
    backend.set_viewport(Rect { x: 0, y: 0, w: 80, h: 24 });
    backend.render();
    let png = backend
        .capture_frame_with_margin(20, 8)
        .to_png(&PngOptions::default());
    fs::write("/tmp/sketch.png", &png)?;
    Ok(())
}
```

Then: `Read` `/tmp/sketch.png`. Adjust the view. Repeat.

## See also

- [`tui-lipan-visual-design`](../tui-lipan-visual-design/SKILL.md) — verify/review an existing UI, snapshot-based regression checks.
- [`tui-lipan-app-builder`](../tui-lipan-app-builder/SKILL.md) — structure the full app once a screen graduates from Mockup.
- [`tui-lipan-layout-debug`](../tui-lipan-layout-debug/SKILL.md) — when a screen is rendered but the rects are wrong.
- Framework docs: `src/mockup.rs`, `docs/quick-start.md` (mockup section), `examples/ui_snapshot.rs`.
