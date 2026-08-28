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
| `theme/Layout.vue` | Sends `/` to the landing, everything else to the default doc layout plus `.doc-bg` |
| `theme/Landing.vue` | The landing page, including its own topbar |
| `theme/InstallTabs.vue` | The hero's tabbed install box — see below |
| `theme/ConfigTabs.vue` | The landing's tabbed `config.toml` samples — see below |
| `theme/toml.ts` | Colours the landing's TOML samples, which Shiki never sees |
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
on top. Under `prefers-reduced-motion` the stage holds one frame and never starts. Hovering deliberately does *not* pause it, and the stage is
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

`docs/index.md` is the documentation table of contents GitHub renders when
browsing the folder, and only that. **The landing page does not render it.**

The landing's closing section is a four-column index of grouped links built
from `themeConfig.sidebar` — the same list a new page has to be added to
anyway, per AGENTS.md, so the section cannot fall behind the docs and there is
no second list to maintain. Pages under `performance/audits/` are filtered out:
there is one per audit, forever, and `Performance Records` is the page that
indexes them.

It used to render `index.md` through `<Content />` and reflow its table into
cards, which kept the two in step at the cost of twenty-four cards of
page-and-sentence — a wall on a landing page, while being exactly right on
GitHub. The sidebar is the better single source, and the summaries were the
reference doc's job rather than the landing's.

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

## The background

`.lp-bg` is one decorative layer spanning the whole landing page, behind
everything: three wide colour blooms, the composition's dot grid carried a
little past the picture's edge, and four pieces of the mark at 3–4% opacity
bleeding off the sides. It is `aria-hidden`, `pointer-events: none`, and
positioned in page percentages — at that opacity these are atmosphere, and
atmosphere does not need to line up with a heading.

The blooms do most of the work. Cards are opaque, so a crisp shape behind one
is simply gone, while a field that soft still reads in the gutters and in every
gap between plates.

Doc pages get a quieter version of the same idea: `.doc-bg` in `style.css`,
rendered by `Layout.vue`, holding eleven much smaller marks at 3%. The two
bands are not the same shape: the right one is wide — gap, outline, and gutter
— so it takes seven, anchored to three columns (where the prose ends, the
middle of the band, the viewport edge) rather than only its two edges, which
left a channel of nothing straight down it. The left one is a gutter and
nothing else, and four is already enough there. It is fixed, so
it costs no layout, and **masked to everything outside the reading column** —
no paragraph, heading, or table ever has a shape behind it. The outline column
does: it has no background of its own, and a mark that faint behind a link is
texture rather than interference, which is what buys the right side its width.

The mask is where the layout measurements live — a `268px` sidebar, a `1172px`
container, and an `288px` aside band (a 64px gap plus 224px of outline) that
holds at every width the outline is shown at. The clear zone is asserted
against the real content box at 1280 / 1366 / 1440 / 1600 / 1920 / 2560; a
wrong constant shows up as a mark creeping under the text rather than as
silence. Below 1280px VitePress drops the outline, there is no free space
either side, and the layer turns off.

The two data URIs are the paths out of `public/logo.svg` with its gradient
inlined. Regenerate them from that file rather than editing path data by hand.

`.lp-main` and `.lp-footer` carry `position: relative; z-index: 1` so the layer
stays under them. A negative `z-index` on the layer would not work — it would
fall behind `.lp`'s own background and disappear.

## Cards and rules

One convention holds the landing page together: **a filled box with a border
and a radius is a card, and a card is a thing you click.** Feature cards, the
worked-extension links, and the contents cards are all links. Static data —
the facts strip, the extension author loop, the index section — is laid out
with hairline rules instead. The two strips started out as rows of small
filled boxes and read as keycaps nobody could press, which is unsurprising:
that is the exact recipe `.lp kbd` uses.

The hero composition is a picture and takes `pointer-events: none`. Its
negative block margin puts it over the bottom of the CTA row, where its mask
has already faded it to nothing — so without that line the buttons look
untouched and quietly stop taking clicks.

## The counted facts

The numbers the landing page advertises — themes, rebindable commands, hook
events, recognized coding agents — are not written down anywhere on the site.
`config.ts` counts them out of `src/` at build time and passes them through
`themeConfig.roziFacts`; the facts strip, two feature cards, and two catalogue
rows read that. Adding a theme or an agent updates the site by itself.

Each count anchors on a load-bearing constant — `BUILTIN_COMMANDS`,
`EventKind::ALL`, `ThemePreset::all`, and the `[[agents]]` tables in
`agent_detection/builtin.toml` — and **a pattern that stops matching throws**
rather than falling back to a number. A build that fails naming the file that
moved is cheaper than a page quietly advertising last year's totals, and this
is the only place a Rust refactor can break the site.

Two counts stay literals in `Landing.vue`: seven layouts and nine workspaces
are fixed by the design rather than by a list to count.

## The config box

`theme/ConfigTabs.vue` is the "Make it yours" panel: one tab per customization
surface, each holding a real `config.toml` fragment. The fragments come from
`examples/config.toml` and `examples/sidebar.toml` rather than being composed
for the page, because a reader's next move is to paste one.

**Every snippet is exactly thirteen lines.** The tabs sit above the panel, so a
tab that resized the box would drag the row the reader just clicked out from
under the pointer — but the fix is equal snippets, not a reserved height. An
earlier version padded the panel out to the tallest sample and bought the
stable height with a hundred empty pixels under every shorter one, which is the
same bargain the install box refuses. Trim a new snippet to thirteen lines.

## Syntax highlighting

Doc pages get Shiki through markdown. The landing page's samples live inside
Vue components, where the markdown pipeline never reaches them, so `theme/toml.ts`
colours them instead — a small TOML scanner emitting `tk-*` spans that
`landing.css` styles in night-owl's colours, the same theme `config.ts` gives
the fenced blocks. Shipping Shiki to the client to highlight five constant
strings would cost more than the strings weigh.

Its output is inserted with `v-html`, which is safe **because every input is a
constant written in this repository**. Do not point it at anything else without
reading the escaping in that file first.

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

A Windows visitor lands on the `powershell` tab instead of the default one.
That is the only preselection there is: `curl` already covers Linux and macOS,
so no other platform has a tab to move to. It runs in `onMounted` because the
page is prerendered — deciding during render would bake one platform's tab into
the static HTML, or disagree with it on hydration — so the first paint is always
`curl` and the switch arrives with hydration. A click always wins afterwards.

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
