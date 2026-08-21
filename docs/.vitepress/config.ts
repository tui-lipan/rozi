import { defineConfig } from "vitepress";
import { fileURLToPath } from "node:url";
import { copyFileSync, readFileSync } from "node:fs";
import vueJsx from "@vitejs/plugin-vue-jsx";
import { repoLinksPlugin } from "./repoLinks";

const srcDir = fileURLToPath(new URL("..", import.meta.url)).replace(/\/$/, "");

/** What the landing page calls itself, in a tab and when shared. */
const LANDING_TITLE = "rozi - a tiling terminal multiplexer";

/**
 * Read from the manifest rather than written down here. The version used to be
 * typed into three files, which is three chances for the site to advertise a
 * release that does not exist.
 */
const ROZI_VERSION = (() => {
  const manifest = readFileSync(
    fileURLToPath(new URL("../../Cargo.toml", import.meta.url)),
    "utf8",
  );
  const found = /^version\s*=\s*"([^"]+)"/m.exec(manifest);
  if (!found) throw new Error("no version in Cargo.toml");
  return found[1];
})();

/**
 * The counts the landing page advertises, taken from the source instead of
 * written down. Same reason as the version above: a theme count typed into a
 * template is a claim nothing checks, and the first person to add a theme has
 * no way of knowing the site disagrees with the binary.
 *
 * Every pattern points at a load-bearing constant, and a miss throws rather
 * than falling back to a number - a build that fails naming the file that
 * moved is cheaper than a page quietly advertising last year's totals.
 */
const ROZI_FACTS = (() => {
  const read = (rel: string) =>
    readFileSync(fileURLToPath(new URL(rel, import.meta.url)), "utf8");

  const many = (source: string, pattern: RegExp, what: string) => {
    const hits = source.match(pattern)?.length ?? 0;
    if (hits === 0) throw new Error(`counted no ${what} - the pattern moved`);
    return hits;
  };

  const one = (source: string, pattern: RegExp, what: string) => {
    const found = pattern.exec(source);
    if (!found) throw new Error(`could not read ${what} - the pattern moved`);
    return Number(found[1]);
  };

  return {
    // The palette, the help overlay, and `[keys]` all render from this list.
    // Anchored and indented so the struct's own declaration is not an entry.
    commands: many(
      read("../../src/commands.rs"),
      /^ {4}BuiltinCommand \{$/gm,
      "BUILTIN_COMMANDS entries",
    ),
    hookEvents: one(
      read("../../src/events.rs"),
      /pub const ALL: \[Self; (\d+)\]/,
      "the EventKind::ALL length",
    ),
    agents: many(
      read("../../src/agent_detection/builtin.toml"),
      /^\[\[agents\]\]/gm,
      "[[agents]] tables",
    ),
    // `ThemePreset::all` includes the hidden ANSI fallback but not the selectable
    // `system` theme, so those two sides cancel in the public theme total.
    themes: one(
      read("../../src/state/appearance.rs"),
      /impl ThemePreset \{\s*pub fn all\(\) -> \[Self; (\d+)\]/,
      "the ThemePreset::all length",
    ),
  };
})();

/**
 * The one-liner installers advertised on the landing page are served from this
 * site, so `curl -fsSL https://rozi.tui-lipan.dev/install | bash` fetches the
 * very script that lives at the repository root. Copying rather than
 * symlinking keeps the checked-in tree the single source: `docs/public/install`
 * and `docs/public/install.ps1` are generated and gitignored. This runs at
 * config load, which covers `docs:dev` (the public directory is served live)
 * as well as `docs:build`.
 */
(() => {
  const root = new URL("../../", import.meta.url);
  const publicDir = new URL("../public/", import.meta.url);
  // The Unix helper is served extensionless so the advertised URL stays short.
  for (const [from, to] of [
    ["install.sh", "install"],
    ["install.ps1", "install.ps1"],
  ]) {
    copyFileSync(
      fileURLToPath(new URL(from, root)),
      fileURLToPath(new URL(to, publicDir)),
    );
  }
})();

export default defineConfig({
  title: "rozi",
  description:
    "A tiling terminal multiplexer that feels like a modern window manager - self-arranging panes, persistent sessions, and live config reload on Linux, macOS, and Windows.",
  cleanUrls: true,
  lastUpdated: true,
  appearance: "force-dark",

  head: [
    ["link", { rel: "icon", type: "image/svg+xml", href: "/favicon.svg" }],
    [
      "link",
      { rel: "icon", type: "image/png", sizes: "96x96", href: "/favicon-96x96.png" },
    ],
    ["link", { rel: "icon", href: "/favicon.ico" }],
    [
      "link",
      { rel: "apple-touch-icon", sizes: "180x180", href: "/apple-touch-icon.png" },
    ],
    ["link", { rel: "manifest", href: "/site.webmanifest" }],
    ["meta", { name: "theme-color", content: "#06070f" }],
    ["meta", { property: "og:type", content: "website" }],
    ["meta", { property: "og:url", content: "https://rozi.tui-lipan.dev" }],
    [
      "meta",
      { property: "og:image", content: "https://rozi.tui-lipan.dev/og-image.png" },
    ],
    ["meta", { name: "twitter:card", content: "summary_large_image" }],
    ["meta", { property: "og:title", content: LANDING_TITLE }],
    [
      "meta",
      {
        property: "og:description",
        content:
          "Split your terminal into panes, arrange them automatically, and pick up where you left off.",
      },
    ],
    // Decide whether the landing intro plays before the browser paints, so the
    // page underneath never flashes into view and back out. Vue is far too
    // late for that - it hydrates after the server-rendered HTML is on screen.
    // The failsafe matters: if the bundle never arrives, the class must not be
    // allowed to leave the page hidden forever.
    [
      "script",
      {},
      `(function () {
  try {
    if (location.pathname.replace(/\\/$/, "") !== "") return;
    if (location.hash) return;
    if (sessionStorage.getItem("rozi:intro-seen")) return;
    if (innerWidth < 900 || innerHeight < 560) return;
    if (matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    document.documentElement.classList.add("rozi-intro-pending");
    setTimeout(function () {
      document.documentElement.classList.remove("rozi-intro-pending");
    }, 4000);
  } catch (e) {}
})();`,
    ],
    ["link", { rel: "preconnect", href: "https://fonts.googleapis.com" }],
    [
      "link",
      { rel: "preconnect", href: "https://fonts.gstatic.com", crossorigin: "" },
    ],
    [
      "link",
      {
        rel: "stylesheet",
        href: "https://fonts.googleapis.com/css2?family=JetBrains+Mono:wght@400;500;700;800&display=swap",
      },
    ],
  ],

  markdown: { theme: { light: "night-owl", dark: "night-owl" } },

  // `/` is served from `index.md`, so its <h1> would title the tab "rozi
  // documentation | rozi" even though the landing page renders a hero rather
  // than that file. Name it for what the page is instead. This lives here
  // rather than in `index.md`'s frontmatter because GitHub renders YAML
  // frontmatter as a table at the top of the file, and that file is also the
  // repository's documentation index.
  transformPageData(pageData) {
    if (pageData.relativePath === "index.md") {
      pageData.title = LANDING_TITLE;
      pageData.titleTemplate = false;
    }
  },

  // `performance/README.md` keeps its name so GitHub renders it when browsing
  // that folder; the site serves it as the directory's index page.
  rewrites: { "performance/README.md": "performance/index.md" },

  // The landing composition is ported from a React piece and is far more
  // legible as JSX than as a template full of `:style` bindings.
  vite: { plugins: [vueJsx(), repoLinksPlugin(srcDir)] },

  themeConfig: {
    logo: "/logo.svg",
    roziVersion: ROZI_VERSION,
    roziFacts: ROZI_FACTS,
    outline: [2, 3],
    nav: [
      { text: "tui-lipan", link: "https://tui-lipan.dev" },
      { text: "Framework docs", link: "https://docs.tui-lipan.dev" },
    ],
    sidebar: [
      { text: "Home", link: "/" },
      {
        text: "Getting Started",
        collapsed: false,
        items: [
          { text: "Getting Started", link: "/getting-started" },
          { text: "Installation & Releases", link: "/installation" },
          { text: "Feature Overview", link: "/features" },
        ],
      },
      {
        text: "Using rozi",
        collapsed: false,
        items: [
          { text: "Keybindings", link: "/keybindings" },
          { text: "Layouts & Panes", link: "/layouts-and-panes" },
          { text: "Terminal Features", link: "/terminal" },
          { text: "Sidebar", link: "/sidebar" },
          { text: "Themes", link: "/themes" },
        ],
      },
      {
        text: "Configuration",
        collapsed: false,
        items: [
          { text: "Configuration Reference", link: "/configuration" },
          { text: "Agent Definitions", link: "/agents" },
          { text: "Named Profiles", link: "/profiles" },
          { text: "Project Profiles", link: "/project-profiles" },
        ],
      },
      {
        text: "Sessions",
        collapsed: false,
        items: [
          { text: "Sessions", link: "/sessions" },
          { text: "Remote SSH Sessions", link: "/remote" },
        ],
      },
      {
        text: "Automation",
        collapsed: false,
        items: [
          { text: "Extensions", link: "/extensions" },
          { text: "Extension Test Lab", link: "/extension-testing" },
          { text: "Control Socket", link: "/control" },
          { text: "Hooks", link: "/hooks" },
          { text: "Agent Skill", link: "/agent-skill" },
          { text: "Extension Recipes", link: "/recipes" },
        ],
      },
      {
        text: "Performance",
        collapsed: true,
        items: [
          { text: "Benchmarks & Profiling", link: "/benchmarks" },
          { text: "Performance Records", link: "/performance/" },
          { text: "Audit Playbook", link: "/performance/audit-playbook" },
          { text: "Audit - 2026-08-04", link: "/performance/audits/2026-08-04" },
          { text: "Audit - 2026-08-03", link: "/performance/audits/2026-08-03" },
        ],
      },
    ],
    editLink: {
      pattern: "https://github.com/tui-lipan/rozi/edit/master/docs/:path",
      text: "Edit this page on GitHub",
    },
    search: {
      provider: "local",
      options: {
        translations: {
          button: { buttonText: "Search...", buttonAriaLabel: "Search" },
        },
        // `index.md` is the repository's folder index; `/` serves the landing
        // page instead of rendering it. Indexing it would answer searches with
        // hits on text that is nowhere on the page they open.
        _render: (src, env, md) =>
          env.relativePath === "index.md" ? "" : md.render(src, env),
      },
    },
    footer: { message: "MPL-2.0", copyright: "© Adam Mikołajczyk" },
  },
});
