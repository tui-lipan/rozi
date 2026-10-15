# Documentation

`docs/` is both user documentation and the VitePress source for `rozi.tui-lipan.dev`.
`README.md` is the public overview and documentation index.

Contributor and maintainer material — benchmarks, performance audits, the release process — lives
in `dev/`, outside the site. Do not add it to `docs/`: every page there is published and indexed by
the site search.

- Add new documentation pages to the sidebar in `docs/.vitepress/config.ts`.
- Write links that leave `docs/` in the form GitHub expects. The VitePress build rewrites them for
  the site; see `docs/.vitepress/README.md`.
- Register a new page in three places: the sidebar in `docs/.vitepress/config.ts`, the guide
  tables in `docs/index.md`, and, for a new capability, the matching section of `docs/overview.md`.
- Keep product behavior in user docs, not in agent instructions.
- Update docs with user-visible behavior, CLI flags, config keys, environment contracts, workflows,
  and platform support.
- The site deploys from the latest release tag, not from `master`, so documenting a feature in the
  pull request that adds it is correct. Do not mark pages as unreleased or add version banners.

## What stays out of user docs

User docs describe how rozi behaves now, for someone using it. Each sentence must change what a
reader does or expects. Before you add a paragraph, ask whether a user would act differently
without it; if not, leave it out. These do not belong in `docs/`:

- **Change history.** Describe the current behavior only. Avoid "now", "no longer", "former",
  "previously", "used to", "new in", and "restores the previous behavior". What changed in a
  release belongs in the pull request's Summary, which becomes the release notes.
- **Migration and upgrade notes.** Do not write "Migrate from …" sections, explain how old keys are
  converted, or tell users which protocol version to upgrade from. rozi reports a removed or
  renamed key when it loads the config; if users need a fix, add the exact message to
  `troubleshooting.md` with the current way to write it. Do not document deprecated keys.
- **Implementation detail.** Leave out threads, caches, buffer and queue sizes, internal timeouts,
  retry schedules, frame pacing, acknowledgements, protocol handshakes, capability names, file
  formats used internally, widget names, and which process does the work. Mention a limit only when
  a user can hit it and needs to know the number, such as a maximum file size.
- **Design rationale.** Do not explain why rozi is built a certain way, argue for a choice, or
  describe alternatives that were rejected. Code comments, `dev/`, and pull requests hold that.
- **Developer and test knobs.** Environment variables and flags meant for benchmarking, comparisons,
  or debugging rozi itself go in `dev/`, not user docs. A knob a user may need to work around a
  problem goes in `troubleshooting.md` or next to the feature, described by its effect.
- **Exhaustive edge cases.** Do not list every state transition or tie-breaking rule in a feature
  guide. Document the common case and the edge cases a user is likely to notice. The
  configuration and control references may be complete, but still describe effects, not mechanisms.

Append new text to the section it belongs to, and rewrite what is already there if it no longer
reads as one piece. Never add a paragraph at the end of a page or section only because that is
where the change happened to start.

## Style

Write for a user who wants to get something done, then for one who wants every detail.

- Call the product `rozi`, lowercase, even at the start of a sentence. The command is `rozi` in code
  formatting.
- Open every page with one to three sentences that say what it covers. Put the common task first,
  then options, then edge cases and limits. A reference table comes after the prose that explains
  when to use it.
- Use sentence-case headings phrased as a task ("Rename a session") or a subject ("Resurrection").
- Use one term for one idea: say "temporary session", and write `ephemeral` only as the literal
  config value or picker label. Say "command key" for a key pressed after the prefix.
- Format every key and chord in code: `Ctrl+A`, `Shift+Tab`, `Esc`, `Enter`. Follow the notation in
  `keybindings.md#key-notation`. In prose, describe a default prefix command as "`Ctrl+A`, then
  `s`" on first use in a page; later mentions and tables may say "the `s` command key".
- Use an em dash with a space on each side (` — `) for a break in a sentence. Do not use ` - `.
- Explain or link a term the first time a page uses it. Terms defined in `core-concepts.md` may link
  there; avoid internal terms such as PTY, lease, or endpoint in page introductions.
- Document what users see and control; see [What stays out of user docs](#what-stays-out-of-user-docs).
- State rules plainly, once. Avoid arguing for the design, repeating a rule in other words, and
  rhetorical asides.
- Prefer short paragraphs, numbered steps for procedures, and tables for enumerable facts. Keep
  explanations in prose, not table cells.

## Screenshots

Screenshots live in `docs/assets/captures/` and come only from `tools/docs-captures/`, which runs
the real binary in a sandbox; see its README. Regenerate the affected scenes when a change alters
what they show, and never edit a capture by hand or take one from a personal session.

Embed captures with the `CaptureGallery` component (`docs/.vitepress/theme/CaptureGallery.vue`):

```html
<CaptureGallery title="~/src/rozi — dev">
<img src="./assets/captures/theme-nord.webp" alt="The workspace in the Nord theme" data-label="nord" data-caption="One sentence." data-code='[theme]\nname = "nord"'>
</CaptureGallery>
```

- Keep the tags on contiguous lines with no blank line between them. GitHub then renders the images
  in order, and the site reads them into the component.
- Use the default `mode="tabs"` for variants of one screen, and `mode="steps"` for a flow in order.
  Steps advance on their own until the reader picks one.
- Give every image an `alt` that describes what it shows. `data-label` names the tab or step,
  `data-caption` is one sentence under the frame, and `data-code` is TOML with `\n` line breaks.
- A gallery with one image is a framed screenshot with a full-size view.

## Diagrams

Diagrams are hand-written SVGs in `docs/assets/diagrams/`, embedded as Markdown images with a
descriptive alt text. Use the site palette from `docs/.vitepress/theme/style.css`, a dark rounded
background, and monospace text, and keep each one to a single idea. Add one only where a picture
explains a relationship faster than prose.

## Layout

Existing pages are grouped by subject rather than nested directories:

- Orientation: `index.md` (the guide list GitHub shows; not rendered on the site), `overview.md`
  (what rozi does, by feature), `getting-started.md`, `installation.md`, `core-concepts.md`,
  `platform-support.md`, `troubleshooting.md`
- Configuration and interaction: `customize.md`, `configuration.md`, `keybindings.md`, `themes.md`
- Runtime behavior: `layouts-and-panes.md`, `terminal.md`, `sessions.md`, `worktrees.md`,
  `shared-sessions.md`, `remote.md`
- Profiles and sidebar: `profiles.md`, `sidebar.md`
- Automation: `scripting.md`, `control.md`, `control-protocol.md`, `recording.md`, `hooks.md`,
  `extensions.md` (using extensions), `writing-extensions.md` (authoring),
  `extension-testing.md`, `recipes.md`
- Agent integration: `agents.md`, `agent-skill.md`

Keep that flat layout until a section has enough pages to justify a directory. If it does, move the
whole subject together and update VitePress navigation and inbound links in the same change.
