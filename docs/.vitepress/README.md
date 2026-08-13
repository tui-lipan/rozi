# rozi.tui-lipan.dev

The `docs/` folder is both the repository's documentation and the source for
<https://rozi.tui-lipan.dev>. Every `.md` file beside this one is a page on the
site; there is no separate copy to keep in sync.

The site is a sibling of [docs.tui-lipan.dev](https://docs.tui-lipan.dev) and
shares its VitePress theme structure. Only the palette differs — see the header
comment in `theme/style.css`.

## Develop

```bash
cd docs
npm install
npm run docs:dev
```

Requires Node.js 22+.

## Build

```bash
npm run docs:build     # output: docs/.vitepress/dist
npm run docs:preview
```

The build fails on dead links. That check is worth keeping: it is what catches a
page renamed out from under a cross-reference.

## Layout

| Path | What it is |
|------|------------|
| `config.ts` | Site metadata, sidebar, search, `performance/README.md` → `/performance/` rewrite, the intro's pre-paint gate |
| `repoLinks.ts` | Rewrites links that leave `docs/` into GitHub URLs (see below) |
| `theme/style.css` | Doc-page theme; palette derived from the app icon |
| `theme/landing.css` | Landing page chrome |
| `theme/Layout.vue` | Sends `/` to the landing, everything else to the default doc layout |
| `theme/Landing.vue` | The landing page, including its own topbar |
| `theme/InstallTabs.vue` | The hero's tabbed install box — see below |
| `theme/RoziIntro.vue` | First-visit intro overlay |
| `theme/composition/` | The animated rozi window — see below |
| `../public/` | Favicons, `og-image.png`, `CNAME`, web manifest, plus the two generated installer copies |

### The composition

`theme/composition/` holds one 1920×1080 animation, played two ways.

| Path | What it is |
|------|------------|
| `RoziScene.jsx` | The drawing: a pure function of authored time `T` |
| `runtime.ts` | Easings, the tween, the scene warp, the `px()` style shim, the rAF clock |
| `scenes.ts` | The two cuts and their cue tables |
| `RoziStage.vue` | Scales the scene into a box and drives its clock |

The scene is ported from a React piece and stays JSX, which is why `config.ts`
loads `@vitejs/plugin-vue-jsx`. Two differences from React to keep in mind when
editing it: Vue does not append `px` to bare numbers in a style object, hence
`px()` at every `style={}`, and vendor-prefixed properties are written as
hyphenated string keys.

A scene carries `dur` (how long it plays) and `nat` (how much authored time it
covers). Splitting those is what lets the same piece run as a 12.8s intro and an
11.4s hero loop without the cues moving. The opening is the exception: it is
pinned to absolute authored seconds, so `Boot` must always cover 2.4 of them.

The two cuts park each other's acts past the end of time rather than carrying a
mode flag: the hero pushes `Tile`/`Mark`/`Name` out to 9000 so the logo resolve
holds on its first frame, and the intro does the same to the six layout acts. A
parked act costs nothing and renders as if it had not started. **Adding an act
to one cut means parking it in the other**, or its cue reads `undefined` and
every value derived from it becomes `NaN`.

Anything added to the loop has to keep its pace. The acts around it are
continuously busy — text typing, panes snapping in on `easeOutBack`, a line of
output every few tenths — so a move that eases in from rest and then holds still
reads as a stall even when it is short. Lead in fast, decelerate, and keep the
hold to about a second.

The hero's `ratio` prop crops the piece shorter than 16:9 to cut its generous
margin. That crop runs through the dot grid and the screen's drop shadow, which
are both still faintly painted out there, so `.fit-width` fades its own top and
bottom to avoid a hard seam across the page. **The two are coupled**: change the
ratio and the mask stops have to be rechecked against the screen plate, which
sits at composition y 170–930 of 1080 and must stay at full strength.

The clock idles when the stage is off screen, the tab is hidden, or the intro is
on top. Under `prefers-reduced-motion` or below 700px the stage holds one frame
and never starts. Hovering deliberately does *not* pause it, and the stage is
not selectable — it is a picture of a terminal, not a terminal.

### The first-visit intro

`/` plays the full arc once per tab, then dissolves into a hero already showing
the same lockup — mark left, wordmark right, same typeface and gradient — so the
handoff lands on matching artwork rather than a resemblance. Changing one of the
two means changing the other: `theme/Landing.vue`'s `.lp-lockup` and the closing
frame of `RoziScene.jsx`. It is skipped on a deep link, under
reduced motion, and on a viewport too small to read it; while it runs, a click,
a scroll, Escape, or the skip button ends it.

The decision is taken by an inline script in `config.ts` **before first paint** —
Vue hydrates after the server-rendered HTML is already on screen, which is far
too late to hide it without a flash. The script sets `rozi-intro-pending` on
`<html>`; `landing.css` hides the page while it is there, `RoziIntro.vue` reads
it back and removes it on dismissal. The script's 4s failsafe is load-bearing: if
the bundle never arrives, that class must not leave the page invisible.

### Links that leave `docs/`

Pages link to repository files that are not part of the site — `../AGENTS.md`,
`../examples/config.toml`, `../integrations/…`. Those work on GitHub and in an
editor but would be dead links once only `docs/` is published, so
`repoLinks.ts` rewrites them to `github.com/tui-lipan/rozi` URLs before
VitePress parses the markdown. Links to a directory with no index page, such as
`performance/audits/`, get the same treatment — that is a request for a file
listing, which only GitHub can answer.

Write links the way GitHub wants them. The site adapts.

### The landing page and `index.md`

`docs/index.md` is left as a plain documentation table of contents so GitHub
renders it when browsing the folder. The landing page renders it verbatim as its
closing section through `<Content />`, so the site and the repository cannot
drift apart.

## Deploy (Cloudflare Workers Builds)

| Setting | Value |
|---------|-------|
| Production branch | `master` |
| Root directory | `docs` |
| Build command | `npm run docs:build` |
| Deploy command | `npx wrangler deploy` |
| Node version | 22 |
| Custom domain | `rozi.tui-lipan.dev` |

**Leave the root directory as `docs`.** It is where both commands run and where
`wrangler.jsonc` lives, which is what Cloudflare recommends for this layout.

`docs/wrangler.jsonc` is committed deliberately. Without it, `wrangler deploy`
runs its framework auto-detection, which writes an `assets.directory` of
`docs/.vitepress/dist` — a path relative to the *repository* root. With the root
directory already set to `docs`, that resolves to `docs/docs/.vitepress/dist`
and the deploy fails on a path that never existed. Auto-detection also ran the
VitePress build a second time. Every path in that file is relative to `docs/`.

Wrangler is a devDependency so the deploy uses a pinned version rather than
whatever `npx` fetches that day; Workers Builds prefers the declared one.

`public/CNAME` carries the same domain for GitHub Pages, if the site ever moves
there.

## The version chip

Nothing to update. `config.ts` reads the version out of `../../Cargo.toml` at
build time and passes it through `themeConfig.roziVersion`; the chips in
`theme/NavTitleMeta.vue` and `theme/Landing.vue` render that. Bumping the crate
is the only step.

## The install box

`theme/InstallTabs.vue` holds every channel the landing page advertises, one
entry per tab. `command` is exactly what reaches the clipboard, so nothing a
shell must not receive belongs in it; `note` is the line under it.

**Both are one line, and that is what keeps the box a fixed height.** The
earlier version reserved two lines of code height so the box would not resize
when a two-line command was selected, which traded a resize for a permanently
visible blank line - worse, because it showed on every tab instead of during a
switch. Keeping every command and note to one line removes the need for either.
A new channel that does not fit on one line needs a different design, not a
taller box.

The whole command strip is the copy button, so the click target is the command
itself rather than the small label at its end. That label is a `span`; a
`button` there would be a nested button inside the strip's own `button`.

## The hosted installers

`curl -fsSL https://rozi.tui-lipan.dev/install | bash` is advertised on the
landing page and in the README, so the site has to serve the script. `config.ts`
copies the repository's `install.sh` to `public/install` and `install.ps1` to
`public/install.ps1` when it loads, which covers `docs:dev` as well as
`docs:build`. Both copies are gitignored — the root scripts are the source, and
nothing needs syncing by hand. The Unix one loses its extension so the
advertised URL stays short.

Cloudflare's build checks out the whole repository and only sets its root
directory to `docs`, so `../install.sh` resolves there the same as it does
locally.
