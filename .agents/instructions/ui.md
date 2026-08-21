# UI conventions

## Feedback and copy

- Do not toast a successful state change already visible on screen. Lossless config normalization
  is silent. Use toasts for failures, rejections, destructive confirmations, and useful off-screen
  results.
- Modals present structured data, not paragraphs. Use rows, badges, terse labels, right-aligned
  descriptions, footer hints, and frame header labels.
- Avoid sentence-shaped body lines. Put context such as user, role, or padding mode into its own row
  or chrome label.
- Prefer compact status tokens such as `ctrl`, `follow`, and `ro` when the layout supplies context.
  Put explanations in documentation.
- Boolean rows use `Enabled` and `Disabled`. Grey unavailable rows through `disabled_reason`.
- Rozi has no checkbox glyph convention. Do not introduce one.

See `docs/configuration.md#in-app-toasts` for the feedback policy.

## Shared overlay shells

Do not hand-roll filtering, fuzzy matching, keyboard navigation, scrolling, hover, or palette
chrome.

- Searchable pickers use `view::shared_search_palette::<T>(ctx, height, highlight_matches)`, then
  add entries and selection callbacks. Wrap them with `action_palette_modal` and
  `action_palette_frame`. Use `search_entries_with_groups` for grouped rows.
- Text prompts add a thin wrapper around `view/overlays/prompts.rs::prompt_overlay`.
- Plain modals use `view::styled_modal`.

For a new picker:

1. Add its open state and selected row in `state/`.
2. Add select and activate messages in `msg.rs`, routed through `update/mod.rs`.
3. Handle activation in `update/overlays.rs`. Disabled rows do nothing. Settings overlays reopen,
   restore their highlight, and reclaim focus after changes.
4. Add the opening `Action` to both `input.rs` and `commands.rs`.
5. Use `ops/overlay_return.rs` only when the picker can open from another overlay.

Right-align row status with `ItemDescription`. Follow nearby overlays rather than exporting tiny
private styling helpers only to share three lines.

## Motion

Geometry animation is app-driven. Position and opacity may animate; terminal dimensions snap to
avoid repeated PTY resize and SIGWINCH reflow.
