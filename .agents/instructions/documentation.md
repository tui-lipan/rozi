# Documentation

`docs/` is both user documentation and the VitePress source for `rozi.tui-lipan.dev`.
`README.md` is the public overview and documentation index.

- Add new documentation pages to the sidebar in `docs/.vitepress/config.ts`.
- Write links that leave `docs/` in the form GitHub expects. The VitePress build rewrites them for
  the site; see `docs/.vitepress/README.md`.
- Keep product behavior in user docs, not in agent instructions. Start at `docs/index.md`; use
  `docs/features.md` as the feature inventory.
- Update docs with user-visible behavior, CLI flags, config keys, environment contracts, workflows,
  and platform support.

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
- Document what users see and control. Leave out implementation detail that does not change what a
  user does or expects, such as how a list is cached or why a design was chosen.
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

## Layout

Existing pages are grouped by subject rather than nested directories:

- Orientation: `overview.md`, `getting-started.md`, `installation.md`, `core-concepts.md`,
  `platform-support.md`, `troubleshooting.md`, `features.md`
- Configuration and interaction: `customize.md`, `configuration.md`, `keybindings.md`, `themes.md`
- Runtime behavior: `layouts-and-panes.md`, `terminal.md`, `sessions.md`, `worktrees.md`,
  `shared-sessions.md`, `remote.md`
- Profiles and sidebar: `profiles.md`, `sidebar.md`
- Automation: `scripting.md`, `control.md`, `control-protocol.md`, `hooks.md`, `extensions.md`,
  `extension-testing.md`, `recipes.md`
- Agent integration: `agents.md`, `agent-skill.md`
- Development: `release-process.md`, `benchmarks.md`, `performance/`

Keep that flat layout until a section has enough pages to justify a directory. If it does, move the
whole subject together and update VitePress navigation and inbound links in the same change.
