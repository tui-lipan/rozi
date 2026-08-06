---
name: tui-lipan-visual
description: >-
  See what a tui-lipan UI actually looks like, and act on it. Use when designing
  a new screen, dashboard, form, panel, or visual variant; when reviewing or
  polishing an existing UI after a change; and when checking chrome, spacing,
  truncation, focus, contrast, colour, or viewport behavior without a live
  terminal. Covers headless capture, PNG inspection, and kept design sketches.
  Use tui-lipan-app-builder for state, messages, props, and async wiring.
---

# TUI-lipan Visual Work

You do not need a human staring at a terminal to see a tui-lipan UI. Render it
headlessly, write a PNG, and look at it with the `Read` tool.

This one skill covers both directions of visual work: designing a screen that
does not exist yet, and reviewing one that does. They share the same loop, so
they share the same skill.

If this skill conflicts with the current workspace docs or source, follow the
workspace.

## The rule that matters most

**Never write a snapshot harness you intend to delete.**

Every capture below is either zero new code or a kept file. Writing a throwaway
`main()` to render something once, then deleting it, produces code that appears
and disappears across commits for no lasting benefit. If a capture was worth
running, it is worth keeping as a sketch — it costs three lines and becomes a
regression check.

## Pick the cheapest capture that answers your question

| Situation | Capture | New code |
|-----------|---------|----------|
| An app, example, or binary already runs | `TUI_LIPAN_SNAPSHOT` env var | none |
| A new screen whose layout is not settled | `Sketch` in `examples/sketches/` | one kept file |
| You need an assertion that runs in CI | `TestBackend` in a real test | one kept test |

Work down that list. Most visual questions about existing code are answered by
the first row without touching a single source file.

## 1. Capture a running app or example — no code at all

Set `TUI_LIPAN_SNAPSHOT` and run it normally. The runner renders off-screen,
writes one artifact, and exits without entering raw mode. No terminal is needed,
so this works in agent sessions and on CI.

```bash
TUI_LIPAN_SNAPSHOT=/tmp/todo.png cargo snap todo
```

`cargo snap <example>` is an alias for
`cargo run --features ui-snapshot-png,ui-snapshot-json --example`. Use it for
every capture so the feature set stays fixed and snapshot runs stop invalidating
your normal `cargo check` / `cargo test` build artifacts.

| Variable | Default | Effect |
|----------|---------|--------|
| `TUI_LIPAN_SNAPSHOT` | unset | Output path. Setting it enables headless mode. `.png`, `.json`, or markdown by extension |
| `TUI_LIPAN_SNAPSHOT_VIEWPORT` | `100x30` | Layout size, `WIDTHxHEIGHT` |
| `TUI_LIPAN_SNAPSHOT_FRAMES` | `1` | Render/message passes before capture; raise it when `init()` starts work |
| `TUI_LIPAN_SNAPSHOT_FOCUS` | `0` | Focus advances before capture, for visible focus chrome |
| `TUI_LIPAN_SNAPSHOT_KEYS` | unset | Key script dispatched before capture, e.g. `tab,tab,enter` |
| `TUI_LIPAN_SNAPSHOT_DIAGNOSTIC` | unset | `1` captures with `UiSnapshotOptions::diagnostic()` |

### Reaching states behind a keystroke

`TUI_LIPAN_SNAPSHOT_KEYS` scripts input, so a modal, an error, or a filled-in
form is capturable without writing anything:

```bash
# Type into the focused input and submit
TUI_LIPAN_SNAPSHOT=/tmp/filled.png \
TUI_LIPAN_SNAPSHOT_KEYS="b,u,y,space,m,i,l,k,enter" \
cargo snap todo
```

Entries use ordinary keybinding syntax (`ctrl+n`, `esc`, `f12`). Each key is
dispatched, its messages drained, and the tree re-rendered before the next -
exactly what the event loop does - so typed text accumulates properly.

**Check where focus starts before scripting `tab`.** Many apps focus their
primary input on mount, so a leading `tab` moves focus *off* the thing you meant
to type into. Capture markdown first and read `focus_key`.

```bash
# Roomy viewport, focus moved once, diagnostic markdown for a vanishing widget
TUI_LIPAN_SNAPSHOT=/tmp/app.md \
TUI_LIPAN_SNAPSHOT_VIEWPORT=140x40 \
TUI_LIPAN_SNAPSHOT_FOCUS=1 \
TUI_LIPAN_SNAPSHOT_DIAGNOSTIC=1 \
cargo snap dashboard
```

This is also the way to capture a *user's* app: it needs no cooperation from
their source.

## 2. Sketch a new screen — one kept file

`Sketch` renders a plain view function at one or more viewports and writes the
artifacts. No `Component`, no `State`, no `Message`, no `update()`.

Add `examples/sketches/<name>.rs`:

```rust
use tui_lipan::prelude::*;
use tui_lipan::{Result, Sketch};

fn view() -> Element {
    Frame::new()
        .header_left("Sign In")
        .border(true)
        .child(
            VStack::new()
                .gap(1)
                .padding(1)
                .child(Text::new("Welcome back."))
                .child(Input::new("alice@example.com").placeholder("Email").key("email"))
                .child(Input::new("").mask(Some('*')).placeholder("Password").key("password")),
        )
        .into()
}

pub fn sketch() -> Result<()> {
    Sketch::view("login", view)
        .viewport(80, 24)
        .fit(20, 8)
        .focus_next(1)
        .write()?;
    Ok(())
}
```

Then add `mod <name>;` and one row to `SKETCHES` in `examples/sketches/main.rs`.
**No `Cargo.toml` change is needed** — cargo discovers the whole directory as one
example.

```bash
cargo snap sketches -- login     # one sketch
cargo snap sketches              # all of them
```

Artifacts land in `target/ui-sketches/`, which is already outside version
control. `write()` prints every path it wrote; `Read` those paths.

### Sketch builder

| Method | Effect |
|--------|--------|
| `Sketch::view(name, fn)` | Sketch a plain `Fn() -> Element` |
| `Sketch::component(name, c)` | Sketch a real `Component` (default props) |
| `.viewport(w, h)` | Capture at an exact size; repeat for breakpoints |
| `.fit(margin_w, margin_h)` | Capture at content minimum size plus margin |
| `.focus_next(n)` | Advance focus `n` times, for visible focus chrome |
| `.keys(script)` | Dispatch a key script before capturing, e.g. `"tab,enter"` |
| `.options(opts)` | Describe options, e.g. `UiSnapshotOptions::diagnostic()` |
| `.markdown(b)` / `.png(b)` / `.json(b)` | Toggle formats (md + png on by default) |
| `.dir(path)` | Write somewhere other than `target/ui-sketches/` |
| `.baseline(dir)` | Compare each capture against a stored baseline image |
| `.tolerance(r)` | Max differing-pixel fraction still counted as a match |
| `.quiet(true)` | Stop printing written paths |
| `.write()` | Render everything; returns the written paths |
| `.check()` | Render and return the baseline comparisons |
| `.assert_baseline()` | Render and fail if any capture regressed |

With no explicit viewport, `Sketch` captures `80x24` plus a fit-to-content pass.
**Always capture both.** A single viewport hides the entire floating-buttons
class of bug: the tight pass looks fine while the roomy pass shows `Flex(1)`
children drifting apart.

## Lock a screen against drift with baselines

Keeping a sketch is only half a regression check; something has to notice when
the picture changes. `.baseline(dir)` stores one image per capture and compares
the next render against it.

```rust
#[test]
fn login_screen_has_not_drifted() -> Result<()> {
    Sketch::view("login", login_screen)
        .viewport(80, 24)
        .baseline("tests/ui-baselines")
        .assert_baseline()
}
```

The first run records baselines and passes. Later runs fail listing every changed
capture, each naming a `*.diff.png` written beside its baseline — unchanged
pixels dimmed for context, changed pixels in magenta, so what moved is obvious at
a glance.

```bash
TUI_LIPAN_UPDATE_BASELINES=1 cargo test    # accept the new output
```

Notes that matter:

- Baseline captures force bitmap rendering. System font discovery differs per
  machine, so a font-rendered baseline fails on someone else's machine for
  reasons that have nothing to do with the UI. Font-rendered artifacts are still
  written for you to look at.
- `Created` is not a failure — a new sketch records its baseline and passes.
- **Commit the baselines.** They are the reference; a baseline that only exists
  on your machine protects nothing.
- Prefer removing nondeterminism over raising `.tolerance()`. A tolerance wide
  enough to hide a real regression is worse than no baseline at all.

## 3. Assert in a test — for CI, not for looking

When the goal is a regression that fails the build, write a real test:

```rust
#[test]
fn dashboard_shows_the_active_route() {
    let mut backend = TestBackend::new(Dashboard);
    backend.set_viewport(Rect { x: 0, y: 0, w: 80, h: 24 });
    backend.render();

    let snapshot = backend.capture_ui_snapshot();
    assert!(snapshot.to_markdown().contains("Overview"));
}
```

This is a kept test with an assertion, not a print-and-delete harness. If you
only want to *look* at the output, use row 1 or row 2 instead.

## Look at the PNG

Use the `Read` tool on each `.png` path; the image renders inline.

Markdown and JSON are for structure and assertions. **PNG is for design
judgment** — colour, focus chrome, proportion, whitespace weight. Do not declare
visual polish done without looking at one.

| Check | What you're looking for |
|-------|-------------------------|
| Hierarchy | Can you scan top-to-bottom and find the primary action? |
| Focus chrome | Is the focused widget visually distinct? (typically teal/accent border) |
| Selection | Is the selected list/tab obvious? |
| Whitespace | Is empty space intentional, or did flex distribute it accidentally? |
| Proportions | Do panels and columns feel balanced at the roomy viewport? |
| Density | Enough breathing room between sections; no edge-to-edge text |
| Truncation | Do long strings clip cleanly, or leak past borders? |
| Overlays | Do modals and popovers appear where expected? |
| Secrets | Masked inputs show `value_masked`, never a raw value |

PNG text uses an antialiased system font when one is available, falling back to
the built-in `font8x8` bitmap renderer. Set `font_family` or `font_path` on
`PngOptions` for Nerd Font or project-specific captures — the default discovery
stack is small, so Cascadia, Hack, and IBM Plex Mono need naming explicitly.
Force `PngTextRenderer::Bitmap` for deterministic coarse output.

## Read the markdown for structure

| Section | Use it for |
|---------|------------|
| `## Focus` / `focus_key` | Which widget owns keyboard focus |
| `## Widgets` | Kind, key, rect, selection, labels, values, masking |
| `## Render` | Fixed-width ASCII grid: spacing, clipping, alignment |

Semantic fields worth checking: `selected_index`, `scroll_offset`,
`item_labels`, `total_items`, `value_masked`, `placeholder` vs `label`,
`checkbox_state`.

**When content vanishes**, re-capture with diagnostic options
(`TUI_LIPAN_SNAPSHOT_DIAGNOSTIC=1` or `UiSnapshotOptions::diagnostic()`) and look
for `zero-area` flags *before* changing code. The usual cause is a fixed sibling
or a default `Flex(1)` stack/frame eating the viewport.

## Exercise real states before judging

A login form looks fine with `alice@example.com`. It looks wrong with
`a-very-long-corporate-address@subsidiary.parent-corp.example.com`. Never ship a
layout that has only seen placeholder strings.

Minimum matrix:

- **Empty** — no items, no value, blank inputs
- **Populated** — realistic typical data
- **Overflow** — very long strings, many items (200+)
- **Edge content** — unicode, mixed widths, special characters
- **Error / loading** — render those explicitly

For a `Sketch`, take a parameter and register one entry per state. For a live
app, drive it through `TestBackend` (`dispatch`, `focus_next`, `set_viewport`)
and capture after each meaningful change.

## Promote a sketch to a Component

Stay in `Sketch::view` while the answer to all of these is no:

- Does this view own state that changes over time?
- Does it dispatch messages or call callbacks?
- Does it need `update()` logic?
- Does it route focus or handle keys beyond defaults?
- Does it run async commands?

When any becomes yes, promote — and **keep the sketch file**. Point it at the
promoted component with `Sketch::component("login", LoginScreen)` so the same
artifacts keep being produced. Keep the view body shape identical during
promotion; only state reads change. That preserves the look you settled on.

Hand off to `tui-lipan-app-builder` for the state, message, props, and async
wiring.

## Anti-patterns

- **Writing a harness you plan to delete.** The whole reason this skill ranks
  captures by cost: there is always a zero-code or kept-file option.
- **One PNG, one viewport, done.** You will miss the floating-buttons class of
  bug. Fixed plus fit is the minimum.
- **Markdown-only review for design.** Grids cannot show colour, focus chrome, or
  flex behavior.
- **Placeholder data forever.** Realistic and adversarial data expose truncation
  and alignment bugs a 4-character string never will.
- **Writing a `Component` before the layout is settled.** You will wire messages
  for a layout that gets rejected in thirty seconds. Sketch first.
- **Raising tolerance to make a baseline pass.** That is deleting the check while
  keeping the ceremony. Find the nondeterminism, or update the baseline
  deliberately.
- **Suspecting the framework first.** Wrong rects are usually a sizing-usage bug:
  re-check `Length` choices, container-vs-leaf defaults, padding, and gaps.
  `VStack`, `HStack`, and `Frame` default to `Flex(1)`; fixed headers, footers,
  and status bars need `Length::Px(...)`. If it really is a framework bug, use
  `tui-lipan-layout-debug`.

## Additional resources

- API reference: `references/snapshot-api.md`
- Kept sketch template: `examples/sketches/login.rs`
- App structure, state, and async: `tui-lipan-app-builder`
- Measurement and rect bugs: `tui-lipan-layout-debug`
- Framework repo reference: `docs/components.md`, `tests/ui_snapshot.rs`
