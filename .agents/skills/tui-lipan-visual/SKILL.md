---
name: tui-lipan-visual
description: >-
  See what a tui-lipan UI actually looks like, and drive it. Use when designing
  a new screen, dashboard, form, panel, or visual variant; when reviewing or
  polishing an existing UI after a change; when checking chrome, spacing,
  truncation, focus, contrast, colour, or viewport behavior; and when
  investigating how a running app behaves - clicking, typing, opening a modal,
  reproducing a UI bug, or inspecting which widget is where. Also use when a
  capture came back blank, empty, or missing content and you need to work out
  why. Covers headless capture, PNG inspection, kept design sketches,
  recordings, and live agent-driven sessions over the control channel. Use
  tui-lipan-app-builder for state, messages, props, and async wiring.
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
running, it is worth keeping as a sketch — it costs three lines. Add
`.baseline()` when it should fail CI; without that it is a viewer, not a check.

## Pick the cheapest capture that answers your question

| Situation | Capture | New code |
|-----------|---------|----------|
| An app, example, or binary already runs | `TUI_LIPAN_SNAPSHOT` env var | none |
| A new screen whose layout is not settled | `Sketch` in `examples/sketches/` | one kept file |
| You need a regression that can fail CI | `TestBackend` or `Sketch` with `.baseline()` | one kept test + committed PNG |
| You need to *explore* a running app | control channel (below) | none, but a human must start it |

Work down that list. Most visual questions about existing code are answered by
the first row without touching a single source file. Reach for the control
channel only when one-shot capture genuinely cannot answer the question -
because you need to look, decide, then act on what you saw.

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
| `TUI_LIPAN_SNAPSHOT_VIEWPORTS` | unset | Comma-separated sizes, e.g. `80x24,120x30,160x40`. Writes suffixed files. Wins over `_VIEWPORT`. A malformed entry fails the run |
| `TUI_LIPAN_SNAPSHOT_FRAMES` | `1` | Render/message passes before capture; raise it when `init()` starts work |
| `TUI_LIPAN_SNAPSHOT_FOCUS` | `0` | Focus advances before capture, for visible focus chrome |
| `TUI_LIPAN_SNAPSHOT_KEYS` | unset | Keys-only shorthand, e.g. `tab,tab,enter` |
| `TUI_LIPAN_SNAPSHOT_SCRIPT` | unset | Full action script (below); wins over `_KEYS` |
| `TUI_LIPAN_SNAPSHOT_ADVANCE_MS` | `0` | Virtual-clock advance before capture: ticks animations and fires `Command::after` timers. Use for which-key, spinners, settled transitions |
| `TUI_LIPAN_SNAPSHOT_SETTLE_MS` | `0` | **Real** time waited before the script, pumping messages. Use when content arrives from a subprocess, socket, or background thread |
| `TUI_LIPAN_SNAPSHOT_DIAGNOSTIC` | unset | `1` captures with `UiSnapshotOptions::diagnostic()` |

**If the app does anything asynchronous, you need `_SETTLE_MS`.** The capture loop
is synchronous and never sleeps, so a spawned process, a socket read, or a
background task has not finished by the time the artifact is written. No amount of
`_ADVANCE_MS` helps: a virtual clock cannot make another thread finish. This is the
single most common reason a capture comes back as empty chrome.

### Reaching states behind a click or a keystroke

An action script drives the UI before the capture, so a modal behind a button, a
filled-in form, or a scrolled list is reachable without writing anything:

```bash
TUI_LIPAN_SNAPSHOT=/tmp/filled.png \
TUI_LIPAN_SNAPSHOT_SCRIPT="focus:#draft; type:buy milk; click:#add; wait:200" \
cargo snap todo
```

Steps are separated by `;` or newlines:

| Step | Effect |
|------|--------|
| `key:ctrl+n` | One key event, in keybinding syntax |
| `type:hello world` | Literal text, one key event per character |
| `click:#submit` | Left click the widget keyed `submit` |
| `click:12,7` | Left click a cell |
| `rclick:` / `mclick:` | Right / middle click |
| `hover:#sidebar` | Move the pointer over a widget |
| `focus:#email` | Focus a widget directly |
| `focus:next` / `focus:prev` | Move focus one step |
| `scroll:#list,down` | Scroll over a widget (`up` / `down`) |
| `drag:#card>#column` | Press, move, release |
| `wait:500` | Advance the *virtual* clock 500ms: animations and `Command::after` |
| `sleep:500` | Wait 500ms of *real* time, pumping messages, for async work |

**`wait:` does not wait.** It moves a virtual clock — instant, deterministic, and
the right choice for animations and timers. `sleep:` really does spend the wall
time, and is only for waiting on another thread. Reach for `wait:` first; if the
thing you need is a process or a socket, only `sleep:` works.

`_SETTLE_MS` runs *before* the script, so the script acts on an app that has
finished starting. Waiting *between* steps is `sleep:`'s job — which is also how you
capture mid-animation, since a settle after the script would run every animation the
script just started to completion.

**Target by key, not coordinate.** `click:#submit` resolves through the current
tree and *fails loudly* when the key is absent; `click:42,7` silently clicks
empty space after a layout change and still reports success. Use coordinates
only for widgets that have no key of their own.

**Give widgets keys to make them scriptable.** A markdown snapshot lists every
key an app exposes - read it first to learn what you can target.

**Check where focus starts before scripting `tab`.** Many apps focus their
primary input on mount, so a leading `tab` moves focus *off* the thing you meant
to type into. Capture markdown first and read `focus_key`.

**`hover_key` only reports keyed widgets.** An unkeyed widget can be hovered
while `hover_key` stays empty - read the per-widget `[hovered]` flag instead.
The same applies to containers, whose centre usually lands on an unkeyed child.

```bash
# Roomy viewport, focus moved once, diagnostic markdown for a vanishing widget
TUI_LIPAN_SNAPSHOT=/tmp/app.md \
TUI_LIPAN_SNAPSHOT_VIEWPORT=140x40 \
TUI_LIPAN_SNAPSHOT_FOCUS=1 \
TUI_LIPAN_SNAPSHOT_DIAGNOSTIC=1 \
cargo snap dashboard
```

This is also the way to capture a *user's* app: rendering needs no cooperation
from their source. **State is a different matter.** Apps that write config,
cache, or session files will still touch the developer's live directories unless
you isolate them. Redirect `XDG_*` at the **child process**, then run the
already-built binary:

```bash
cargo build --features ui-snapshot
S=/tmp/app-snap && mkdir -p $S/config
env XDG_CONFIG_HOME=$S/config XDG_STATE_HOME=$S/state XDG_CACHE_HOME=$S/cache \
    XDG_RUNTIME_DIR=$S/run TUI_LIPAN_SNAPSHOT=$S/out.png ./target/debug/<bin>
```

Two gotchas:

- Run the **built binary**, not `cargo run` / `cargo snap`. Redirecting `XDG_*`
  also redirects mise's trust store, and the wrapper refuses to start.
- This is not the "never mutate `HOME` / `XDG_*` in tests" rule. That rule is
  about `std::env::set_var` being unsound beside parallel test threads. It has
  nothing to do with environment variables you pass to a child process.

Time-gated UI (a which-key panel behind `App::command_chord_reveal_delay`, a
spinner, a settled transition) needs a clock, not more frames.
`TUI_LIPAN_SNAPSHOT_FRAMES` renders back to back with no wall clock. Advance
virtual time instead:

```bash
TUI_LIPAN_SNAPSHOT=/tmp/which-key.png \
TUI_LIPAN_SNAPSHOT_KEYS="ctrl+a" \
TUI_LIPAN_SNAPSHOT_ADVANCE_MS=400 \
cargo snap myapp
```

Virtual advancement affects tui-lipan-managed time and animation state.
Application-owned wall-clock timers and `Instant::now()` are not advanced.
Headless `TUI_LIPAN_SNAPSHOT_ADVANCE_MS` runs the live runner ticker (blink,
spinner, images, chord reveal). `TestBackend::advance` / `Sketch::advance` share
the clock but a thinner ticker — tree animations, transitions, overlays, chord
reveal — and captures force the cursor visible. Photograph a specific spinner or
blink phase with the env-var path.

Several breakpoints in one run:

```bash
TUI_LIPAN_SNAPSHOT=/tmp/app.png \
TUI_LIPAN_SNAPSHOT_VIEWPORTS=80x24,120x30,160x40 \
cargo snap myapp
```

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
| `.advance(dt)` | Advance the virtual clock by `dt` before capturing, ticking animations |
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

## 3. Baselined TestBackend or Sketch — a check that can fail

Row 3 is a **regression check**: a kept test whose capture is compared against a
committed baseline PNG, so a visual change fails CI. Colour, density, and
proportion decisions have no string assertion to write; the baseline *is* the
assertion.

```rust
#[test]
fn login_screen_has_not_drifted() -> Result<()> {
    Sketch::view("login", login_screen)
        .viewport(80, 24)
        .baseline("tests/ui-baselines")
        .assert_baseline()
}
```

Apps that need real state use the same affordance on `TestBackend`:

```rust
#[test]
fn dashboard_has_not_drifted() -> Result<()> {
    let mut backend = TestBackend::new(Dashboard);
    backend.set_viewport(Rect { x: 0, y: 0, w: 80, h: 24 });
    backend.render();
    backend.advance(Duration::from_millis(400)); // if the UI is time-gated

    backend
        .baseline("tests/ui-baselines")
        .name("dashboard-80x24")
        .assert_baseline()
}
```

`TUI_LIPAN_UPDATE_BASELINES=1 cargo test` accepts the current render. Commit the
baselines; a baseline that only exists on your machine protects nothing.

Assertion-less `*_visual.rs` files that only emit PNGs (this repo's examples,
downstream apps) are **viewers**, not checks. They belong in row 1 or row 2.
Looking at a PNG is how you judge colour and proportion; `.baseline()` is how
that judgment survives the next refactor. Do not treat a print-and-keep dump as
row 3.

## Record a demo instead of a still

When the thing you need to show is a *flow* rather than a screen, record it.
A recording is text (asciinema cast v2), so a few seconds is usually smaller
than one PNG frame, and it needs no feature flag:

```bash
TUI_LIPAN_RECORD=/tmp/demo.cast \
TUI_LIPAN_RECORD_KEYS="tab,enter" \
cargo run --example todo
```

| Variable | Default | Effect |
|----------|---------|--------|
| `TUI_LIPAN_RECORD` | unset | Output path; enables headless recording |
| `TUI_LIPAN_RECORD_VIEWPORT` | `100x30` | Recorded terminal size |
| `TUI_LIPAN_RECORD_FPS` | `30` | Capture rate |
| `TUI_LIPAN_RECORD_KEYS` | unset | Key script to play |
| `TUI_LIPAN_RECORD_KEY_DELAY_MS` | `400` | Pause after each key |
| `TUI_LIPAN_RECORD_SETTLE_MS` | `1200` | Hold on the final frame |
| `TUI_LIPAN_RECORD_FRAMES` | unset | Directory for truecolor PNG frames (needs `ui-snapshot-png`) |

### Turning a recording into GIF or MP4

Hand the user a `.cast` by default. Convert only when the destination cannot play
one — a README needs a GIF, Slack needs an MP4.

```bash
# GIF (agg); --idle-time-limit is the biggest size win
agg --theme dracula --idle-time-limit 1 demo.cast demo.gif

# MP4, quick: via GIF, so 256 colours
ffmpeg -i demo.gif -pix_fmt yuv420p -movflags +faststart \
       -vf "scale=trunc(iw/2)*2:trunc(ih/2)*2" demo.mp4

# MP4, truecolor: PNG frames straight from the renderer
TUI_LIPAN_RECORD=demo.cast TUI_LIPAN_RECORD_FRAMES=frames \
  cargo snap todo
ffmpeg -framerate 30 -i frames/frame_%05d.png \
       -pix_fmt yuv420p -movflags +faststart demo.mp4
```

Frame export prints the exact `ffmpeg` line with paths filled in — paste it back.

Rough sizes for the same 7-second demo: `.cast` 9 KB, MP4-via-GIF 26 KB, GIF
44 KB, truecolor MP4 55 KB. Frames are a bulky intermediate (~200 files, ~19 MB
at 30fps); delete them after encoding.

In code, `Recording` mirrors `Sketch` (`Recording::view(title, fn).keys("tab").write(path)?`).

**You cannot read a `.cast` with the `Read` tool** - it is a timeline, not an
image. Use a recording to *hand the user something to watch*; use a PNG when you
need to judge the result yourself. If you need both, capture a snapshot at the
interesting moment with the same key script.

Recordings use a synthetic clock, so they are reproducible but do not wait for
real-time work (PTY output, network responses).

## Drive a running app — live sessions

A capture replays a fixed script from process start. When the question is
*"what happens if I click this, and then what?"*, that is not enough: you need to
look, decide, and act on what you saw. `TUI_LIPAN_CONTROL` opens a socket for
exactly that.

**You cannot start the app yourself.** It needs a real terminal, which an agent
session does not have. Ask the operator to run it and tell you when it is up:

```bash
TUI_LIPAN_CONTROL=/tmp/app.sock cargo run --example todo
```

Use a **short** socket path - `AF_UNIX` caps them near 100 bytes. Then talk to it
with `examples/control/client.py` in the framework repo, or a few lines of your
own; the protocol is one command per line and a length-prefixed reply.

| Command | Reply |
|---------|-------|
| `ping` | `pong` |
| `keys` | Reconciliation keys currently rendered |
| `snapshot` / `snapshot json` / `snapshot png <path>` | The widget tree |
| `act <script>` | Runs an action script (same syntax as above) |
| `highlight <key>` / `highlight <col>,<row>` / `highlight clear` | Outline a widget |
| `quit` | Ask the app to exit |

### The loop that makes this work

`keys` after every action. Widgets that appear announce themselves - opening a
confirm dialog adds `modal-cancel` and `modal-delete` to the list, so you learn
the app's structure by acting on it rather than by reading its source.

```
act click:61,3     # the row's ✕ has no key, so target its cell
keys               # → modal-cancel, modal-delete appeared
act click:#modal-delete
keys               # → both modal keys gone, and the row with them
```

### Highlighting

`highlight` marks a widget for the operator, drawn over the finished frame. It
does **not** depend on the widget styling itself, so it marks anything - static
text, layout containers, widgets with no hover style at all. Hover only marks
widgets that style themselves for it.

Cell targeting resolves to the *smallest* widget covering that cell, which is how
unkeyed widgets stay inspectable. The reply is the resolved rect, so you learn
what you actually hit. Clear it when you are done - a forgotten outline is
confusing to whoever is watching the screen.

### Etiquette

- The operator is watching. Narrate what you are about to do before doing it.
- `quit` closes their app. Ask first unless they told you to.
- Leave the UI as you found it: clear highlights, close modals you opened.

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

## When a capture comes back empty or wrong

**Re-capture as diagnostic markdown before changing anything.** This is the highest
value move in the whole skill and the easiest to skip:

```bash
TUI_LIPAN_SNAPSHOT=/tmp/tree.md TUI_LIPAN_SNAPSHOT_DIAGNOSTIC=1 <run the app>
```

Then read the tree and work down this list. The order matters — the cheap checks
rule out the expensive theories.

| The tree says | Cause | Fix |
|---------------|-------|-----|
| The widget is absent entirely | It is not being rendered; state, not layout | Check the condition that gates it |
| Present with a `zero-area` flag | A fixed sibling or a default `Flex(1)` stack/frame ate the viewport | Re-check `Length` choices, padding, gaps |
| Present at a sensible rect, nothing drawn | **The app has not finished starting** | `_SETTLE_MS` for async work, `_ADVANCE_MS` for a timer or animation gate |
| Present and correct, but the PNG is markdown | Binary built without the snapshot features | Rebuild with them; see below |

**"Present at a sensible rect but nothing drawn" is the trap.** The tree looks
perfect — right kind, right key, right rect — because the node genuinely is there.
What is missing is an effect the dump does not report: an `Animated` sitting at
opacity 0 because the reveal timer has not fired, or a widget whose content arrives
from a process that has not answered. Do not conclude "the framework is not
rendering it" from a healthy-looking rect; conclude "it is there and invisible", and
ask *what is it waiting for*.

A quick way to separate the two: if the app has a config or a flag that disables the
fade or the animation, capture with it off. If the content appears, the geometry was
never the problem and you are waiting on time or a thread.

**A capture with no diagnostic is not evidence.** An empty canvas plus rendered
chrome looks exactly the same whether the app is mid-startup, the widget is
zero-area, or the state is genuinely empty. Silence is not a result — get the tree.

### Rebuild trap

Any plain `cargo test`, `cargo build`, or `cargo clippy` run rebuilds the binary
*without* the snapshot features, after which a `.png` path silently receives markdown
(it warns on stderr, which is easy to miss when you are tailing output). If a `.png`
suddenly will not open, rebuild with the features before suspecting anything else.

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
- **Reaching for a live session when a capture would do.** It needs a human to
  start the app and watch it. Use it when you must act on what you just saw, not
  as a default.
- **Raising tolerance to make a baseline pass.** That is deleting the check while
  keeping the ceremony. Find the nondeterminism, or update the baseline
  deliberately.
- **Explaining an empty capture without a diagnostic tree.** The failure modes are
  indistinguishable from the image alone, so a confident cause guessed from a blank
  canvas is usually the wrong one. Capture the tree, then explain.
- **Reaching for `_ADVANCE_MS` when the wait is on a thread.** Advancing a virtual
  clock a whole simulated minute still will not make a subprocess answer. Async work
  needs `_SETTLE_MS` or `sleep:`.
- **Suspecting the framework first.** Wrong rects are usually a sizing-usage bug:
  re-check `Length` choices, container-vs-leaf defaults, padding, and gaps.
  `VStack`, `HStack`, and `Frame` default to `Flex(1)`; fixed headers, footers,
  and status bars need `Length::Px(...)`. If it really is a framework bug, use
  `tui-lipan-layout-debug`.

## Additional resources

- API reference: `references/snapshot-api.md`
- Kept sketch template: `examples/sketches/login.rs`
- Control-channel client: `examples/control/client.py`
- App structure, state, and async: `tui-lipan-app-builder`
- Measurement and rect bugs: `tui-lipan-layout-debug`
- Framework repo reference: `docs/testing.md`, `tests/ui_snapshot.rs`
