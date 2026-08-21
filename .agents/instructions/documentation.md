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

Existing pages are grouped by subject rather than nested directories:

- Getting started and installation: `getting-started.md`, `installation.md`
- Configuration and interaction: `configuration.md`, `keybindings.md`, `themes.md`
- Runtime behavior: `layouts-and-panes.md`, `terminal.md`, `sessions.md`, `remote.md`
- Profiles and sidebar: `profiles.md`, `project-profiles.md`, `sidebar.md`
- Automation: `control.md`, `hooks.md`, `extensions.md`, `recipes.md`
- Agent integration: `agents.md`, `agent-skill.md`
- Development evidence: `benchmarks.md`, `performance/`

Keep that flat layout until a section has enough pages to justify a directory. If it does, move the
whole subject together and update VitePress navigation and inbound links in the same change.
