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

  // The landing page takes its title from the <h1> of the docs index it renders
  // as its closing section, which reads "rozi documentation | rozi". Name it
  // for what the page is instead. This lives here rather than in `index.md`'s
  // frontmatter because GitHub renders YAML frontmatter as a table at the top
  // of the file, and that file is also the repository's documentation index.
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
      },
    },
    footer: { message: "MIT OR Apache-2.0", copyright: "© Adam Mikołajczyk" },
  },
});
