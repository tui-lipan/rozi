<script setup lang="ts">
import { useData, withBase } from "vitepress";
import { onMounted, ref } from "vue";
import RoziStage from "./composition/RoziStage.vue";
import ConfigTabs from "./ConfigTabs.vue";
import InstallTabs from "./InstallTabs.vue";
import RoziIntro from "./RoziIntro.vue";
import { HERO_CUES, HERO_SCENES } from "./composition/scenes";
import { highlightToml } from "./toml";

/* The pre-paint script in config.ts has already decided this and hidden the
   page accordingly; reading its class back is what keeps the two in step. */
const introPlaying = ref(false);
onMounted(() => {
  introPlaying.value =
    document.documentElement.classList.contains("rozi-intro-pending");
});

// From Cargo.toml via config.ts - see NavTitleMeta.vue.
const { theme } = useData();

const GITHUB = "https://github.com/tui-lipan/rozi";
const SPONSOR = "https://github.com/sponsors/Razuer";
const EXAMPLES = `${GITHUB}/tree/master/examples/extensions`;

/* Counted out of the source at build time by config.ts, not typed here. The
   layout count is fixed by the design rather than by a list, so it stays a
   literal. */
const stats = theme.value.roziFacts;

/* Split rather than pre-joined: the number carries the weight in the chip and
   the label sits under it, which is what makes the strip readable at a glance
   instead of a row of small sentences. */
const facts: [string, string][] = [
  ["7", "layouts"],
  [String(stats.themes), "themes"],
  [String(stats.commands), "rebindable commands"],
  [String(stats.agents), "agents detected"],
  ["3", "supported platforms"],
];

/* Two sentences each. The card is a claim and a link, not a paragraph - the
   page under it is where the detail belongs. */
const features = [
  {
    title: "Automatic pane layouts",
    body: "New panes split the focused pane along the side with more room. Rozi has seven layouts, and panes can float, fill the screen, or move with the mouse.",
    link: "/layouts-and-panes",
    linkText: "Layouts and panes",
  },
  {
    title: "Named sessions keep running",
    body: "A background server owns every PTY in a named session. Detach, then use rozi sessions attach dev to return to its layout, running programs, and scrollback from another window or over SSH.",
    link: "/sessions",
    linkText: "Sessions",
  },
  {
    title: "Configuration reloads on save",
    body: "Rozi reloads config.toml when you save it without closing panes. Changes made in the command palette are written back to the file.",
    link: "/themes",
    linkText: "Themes",
  },
  {
    title: "Terminal features",
    body: `Rozi supports mouse reporting, selection, inline images, true color, scrollback search, and a vi-style copy mode. It includes ${stats.themes} themes, and the active theme sets pane colors.`,
    link: "/terminal",
    linkText: "Terminal features",
  },
  {
    title: "Coding-agent status detection",
    body: `Rozi reads coding-agent CLI output and marks the pane as working, blocked, or finished. The state appears on the pane border, workspace tab, and sidebar. Definitions for ${stats.agents} agents are included as editable tables.`,
    link: "/sidebar",
    linkText: "Agent activity",
  },
  {
    title: "Windows support",
    body: "On Windows, Rozi uses ConPTY for terminals and named pipes for IPC. It stores runtime files in a SID-protected directory. Its native executables require no separate runtime.",
    link: "/platform-support",
    linkText: "Platform support",
  },
];

/**
 * Both paths to the same action, side by side. Every built-in default is
 * mirrored onto `<modifier>-<key>` as well as `<prefix> <key>`, and the held
 * modifier is the one that changes how the thing feels to use - so the table
 * shows it as a column of its own rather than mentioning it in a sentence
 * above and then listing prefix chords.
 */
const firstKeys = [
  { key: "Enter", mod: "Alt-Enter", what: "Open another pane" },
  { key: "h j k l", mod: "Alt-h…l", what: "Move focus by direction" },
  { key: "f", mod: "Alt-f", what: "Fullscreen the focused pane" },
  { key: "t", mod: "Alt-t", what: "Let the pane float on top" },
  { key: "1…9", mod: "Alt-1…9", what: "Jump to a workspace" },
  { key: "p", mod: "Alt-p", what: "Search every command" },
  { key: "?", mod: "Alt-?", what: "Show all keys" },
  { key: "d", mod: "Alt-d", what: "Leave. A named session lives on" },
];

/**
 * The inventory, as names only. It used to carry a line of description under
 * every entry, which turned the section into six columns of small print; the
 * group's own doc link is a better place to send anybody who does not
 * recognize a name than a five-word gloss beside it.
 */
const catalog: {
  title: string;
  link: string;
  linkText: string;
  names: string[];
}[] = [
  {
    title: "Window management",
    link: "/layouts-and-panes",
    linkText: "Layouts and panes",
    names: [
      "Seven layouts",
      "aspect-ratio splits",
      "floating",
      "fullscreen",
      "move, swap, and promote",
      "resize mode",
      "nine workspaces",
      "merged borders",
      "gaps and padding",
      "scratchpad",
      "animations",
    ],
  },
  {
    title: "The terminal itself",
    link: "/terminal",
    linkText: "Terminal features",
    names: [
      "True color",
      "inline images",
      "mouse reporting",
      "scrollback search",
      "vi copy mode",
      "clipboard and OSC 52",
      "hints",
      "edit scrollback",
      "shell integration",
      "pane logging",
      "synchronized typing",
    ],
  },
  {
    title: "Sessions and sharing",
    link: "/sessions",
    linkText: "Sessions",
    names: [
      "Named sessions",
      "temporary sessions with brief recovery",
      "multi-client",
      "layout-control lease",
      "remote over SSH",
      "resurrect",
      "autosave",
      "profiles",
      "bare launch opens the session picker",
      "session launcher",
    ],
  },
  {
    title: "Look and feel",
    link: "/configuration",
    linkText: "Configuration",
    names: [
      `${stats.themes} themes`,
      "system theme",
      "hot reload",
      "terminal palette",
      "workbar segments",
      "powerline caps",
      "titlebar styles",
      "sidebar tabs",
      "settings dialog",
      "alert marks",
      "notifications and sounds",
    ],
  },
  {
    title: "Automation",
    link: "/control",
    linkText: "Control socket",
    names: [
      "Control socket",
      "pick",
      "publish",
      "subscribe",
      "notify",
      "run-action",
      "capture-pane",
      `${stats.hookEvents} hook events`,
      "services",
      "user commands",
      "extensions",
      "agent skill",
      "editor navigator",
    ],
  },
  {
    title: "How it ships",
    link: "/installation",
    linkText: "Installation",
    names: [
      "Linux, macOS, and Windows",
      "native executables with no separate runtime",
      "signed releases",
      "managed updates",
      "rollback",
      "published performance measurements",
      "bounded work queues",
      "private IPC endpoints",
      "MPL-2.0",
    ],
  },
];

/* The public surfaces an extension is written against. These are the CLI
   spellings on purpose - `rozi pick` is the whole API for a picker. */
const extensionSurfaces = [
  ["rozi pick", "Opens a searchable picker and returns the selected value"],
  ["rozi publish", "Publishes actionable rows in the Activity sidebar"],
  ["rozi subscribe", "Streams events to a long-running process"],
  ["rozi notify", "Shows a toast for a result produced off screen"],
  ["rozi run-action", "Runs a built-in, user-defined, or extension command"],
];

const authorLoop = [
  ["rozi extensions new", "scaffold"],
  ["rozi extensions check", "validate"],
  ["rozi run-action reload-extensions", "install"],
  ["rozi extensions list", "confirm"],
];

const exampleExtensions = [
  ["git-tools", "Branch and worktree pickers with groups"],
  ["pr-dashboard", "Monitors pull requests with gh and publishes rows"],
  ["docker", "Controls Docker through a supervised process"],
  ["ssh-tools", "Reads SSH config and opens panes"],
  ["agent-activity", "Publishes pane status as actionable rows"],
];

/**
 * The documentation index, read out of the sidebar rather than written here.
 * The sidebar is already where a new page has to be registered - see
 * AGENTS.md - so deriving from it means the landing page cannot fall behind
 * the docs, and there is no second list to remember. "Home" is the landing
 * itself and carries no items, which is also what filters it out.
 */
type DocPage = { text: string; link: string };
type DocGroup = { text: string; items?: DocPage[] };

/* One dated audit report per audit, forever. They belong in the sidebar, where
   somebody is reading the performance docs, and not in a directory of the
   documentation - `Performance Records` is the page that indexes them. */
const isArchive = (page: DocPage) => page.link.startsWith("/performance/audits/");

const docGroups = (theme.value.sidebar as DocGroup[])
  .map((group) => ({ ...group, items: (group.items ?? []).filter((p) => !isArchive(p)) }))
  .filter((group) => group.items.length > 0);

const docPages = docGroups.reduce((total, group) => total + group.items.length, 0);

const CONFIG_SAMPLE = highlightToml(`[theme]
name = "catppuccin-mocha"

[input]
prefix = "ctrl-a"           # the key that starts a command
modifier = "alt"            # or "super"

[layout]
default = "dwindle"         # how new workspaces arrange panes

[pane]
border_style = "rounded"`);

const EXTENSION_SAMPLE = highlightToml(`[extension]
id = "git-tools"
title = "Git tools"
version = "0.1.0"
api = 1

[[commands]]
id = "branches"                 # invoked as git-tools.branches
label = "Switch branch"
exec = ["python", "{extension_dir}/bin/branches.py"]

[[services]]
name = "watch"
exec = ["./bin/watch", "--json"]
restart = "on-failure"`);
</script>

<template>
  <div class="lp">
    <!-- Outside .lp's subtree: the page is faded out while the intro runs, and
         the overlay must not fade with it. -->
    <Teleport to="body">
      <RoziIntro v-if="introPlaying" @done="introPlaying = false" />
    </Teleport>

    <!-- Decoration only: the mark at a size where it reads as texture rather
         than as a logo, plus the bloom the hero lockup already sits in. Behind
         everything, out of the accessibility tree, and untouchable. -->
    <div class="lp-bg" aria-hidden="true">
      <span class="lp-bg-bloom b1"></span>
      <span class="lp-bg-grid"></span>
      <span class="lp-bg-mark m1"></span>
      <span class="lp-bg-bloom b2"></span>
      <span class="lp-bg-mark m2"></span>
      <span class="lp-bg-glyph"></span>
      <span class="lp-bg-bloom b3"></span>
      <span class="lp-bg-mark m3"></span>
    </div>

    <header class="lp-top">
      <a class="lp-brand" :href="withBase('/')">
        <img :src="withBase('/logo.svg')" alt="" width="24" height="24" />
        <span class="lp-brand-name">rozi</span>
        <span class="lp-chip">v{{ theme.roziVersion }}</span>
      </a>
      <span class="lp-top-spacer" />
      <a class="lp-top-link" :href="withBase('/getting-started')">Docs</a>
      <a
        class="lp-top-link"
        href="https://tui-lipan.dev"
        target="_blank"
        rel="noopener noreferrer"
        >tui-lipan ↗</a
      >
      <a
        class="lp-top-link lp-sponsor"
        :href="SPONSOR"
        target="_blank"
        rel="noopener noreferrer"
      >
        <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
          <path
            fill="none"
            stroke="currentColor"
            stroke-width="1.5"
            stroke-linecap="round"
            stroke-linejoin="round"
            d="M8 14.25s-5.5-3.4-5.5-7.5a3.25 3.25 0 0 1 5.5-2.35A3.25 3.25 0 0 1 13.5 6.75c0 4.1-5.5 7.5-5.5 7.5z"
          />
        </svg>
        <span>Sponsor</span>
      </a>
      <a
        class="lp-top-github"
        :href="GITHUB"
        target="_blank"
        rel="noopener noreferrer"
        >GitHub ↗</a
      >
    </header>

    <main class="lp-main">
      <section class="lp-hero">
        <div class="lp-hero-copy">
          <p class="lp-eyebrow">Tiling terminal multiplexer</p>
          <!-- Same lockup the intro lands on, so the handoff is a match rather
               than a resemblance. -->
          <div class="lp-lockup">
            <img
              class="lp-mark"
              :src="withBase('/logo.svg')"
              alt=""
              width="120"
              height="120"
            />
            <h1 class="lp-wordmark">rozi</h1>
          </div>
          <p class="lp-tagline">
            A terminal multiplexer with automatic pane layouts.
          </p>
          <p class="lp-sub">
            A bare launch opens the session picker. Only named sessions persist.
            Temporary sessions remain recoverable for a short time after the
            client exits. Linux, macOS, and Windows are supported.
          </p>

          <InstallTabs />

          <div class="lp-cta">
            <a class="lp-cta-btn primary" :href="withBase('/getting-started')"
              >Get started →</a
            >
            <a class="lp-cta-btn" :href="withBase('/overview')">What it does</a>
            <a
              class="lp-cta-btn"
              :href="GITHUB"
              target="_blank"
              rel="noopener noreferrer"
              >Source ↗</a
            >
          </div>
        </div>
      </section>

      <!-- Breaks out past --lp-max: the composition is authored at 1920 wide,
           and below roughly 1200 its terminal text stops being readable. -->
      <div class="lp-hero-stage">
        <RoziStage
          :scenes="HERO_SCENES"
          :cue-overrides="HERO_CUES"
          :halted="introPlaying"
          ratio="1920 / 940"
          fade-loop-edges
        />
      </div>

      <ul class="lp-facts">
        <li v-for="[value, label] in facts" :key="label">
          <b>{{ value }}</b><span>{{ label }}</span>
        </li>
      </ul>

      <section class="lp-section">
        <header class="lp-head">
          <h2>Included features</h2>
          <p class="lp-head-note">Available with the default configuration</p>
        </header>
        <div class="lp-features">
          <article v-for="f in features" :key="f.title" class="lp-feature">
            <h3>{{ f.title }}</h3>
            <p>{{ f.body }}</p>
            <a :href="withBase(f.link)">{{ f.linkText }} →</a>
          </article>
        </div>
      </section>

      <section class="lp-section lp-two">
        <div>
          <header class="lp-head">
            <h2>First five minutes</h2>
          </header>
          <p class="lp-lead">
            Default commands support prefix keys and Alt shortcuts. You can
            rebind both schemes or use <kbd>Super</kbd> as the modifier.
          </p>
          <table class="lp-keytable">
            <thead>
              <tr>
                <th>Prefix, then a key</th>
                <th class="lp-modhead">Hold Alt</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="row in firstKeys" :key="row.what">
                <td class="lp-keycell">
                  <kbd>Ctrl-a</kbd><kbd>{{ row.key }}</kbd>
                </td>
                <td class="lp-keycell">
                  <kbd class="mod">{{ row.mod }}</kbd>
                </td>
                <td>{{ row.what }}</td>
              </tr>
            </tbody>
          </table>
          <a class="lp-more" :href="withBase('/keybindings')"
            >Full key reference →</a
          >
        </div>

        <div>
          <header class="lp-head">
            <h2>Configuration file</h2>
          </header>
          <p class="lp-lead">
            On Linux and macOS, the path is
            <code>~/.config/rozi/config.toml</code>. Rozi reloads it on save. If
            parsing fails, Rozi uses the defaults and reports the error.
          </p>
          <div class="lp-code">
            <pre><code v-html="CONFIG_SAMPLE"></code></pre>
          </div>
          <a class="lp-more" :href="withBase('/configuration')"
            >Every setting →</a
          >
        </div>
      </section>

      <section class="lp-section">
        <header class="lp-head">
          <h2>Configuration examples</h2>
          <p class="lp-head-note">
            One file · live reload · {{ stats.commands }} rebindable commands
          </p>
        </header>
        <ConfigTabs />
      </section>

      <section class="lp-section">
        <header class="lp-head">
          <h2>Extensions are ordinary programs</h2>
          <p class="lp-head-note">Any language · public protocol · api = 1</p>
        </header>

        <ol class="lp-flow">
          <li v-for="[cmd, step] in authorLoop" :key="cmd">
            <code>{{ cmd }}</code><span>{{ step }}</span>
          </li>
        </ol>

        <div class="lp-ext">
          <div class="lp-code">
            <pre><code v-html="EXTENSION_SAMPLE"></code></pre>
          </div>
          <div class="lp-ext-api">
            <h3 class="lp-h3">Public commands</h3>
            <dl class="lp-defs">
              <template v-for="[name, desc] in extensionSurfaces" :key="name">
                <dt><code>{{ name }}</code></dt>
                <dd>{{ desc }}</dd>
              </template>
            </dl>
          </div>
        </div>

        <h3 class="lp-h3">Worked examples</h3>
        <div class="lp-examples">
          <a
            v-for="[name, desc] in exampleExtensions"
            :key="name"
            class="lp-example"
            :href="`${EXAMPLES}/${name}`"
            target="_blank"
            rel="noopener noreferrer"
          >
            <b>{{ name }} ↗</b>
            <span>{{ desc }}</span>
          </a>
        </div>
        <a class="lp-more" :href="withBase('/recipes')"
          >Write an extension →</a
        >
      </section>

      <section class="lp-section">
        <header class="lp-head">
          <h2>Feature index</h2>
          <p class="lp-head-note">These entries apply to the current release</p>
        </header>
        <div class="lp-index">
          <article v-for="group in catalog" :key="group.title" class="lp-area">
            <div class="lp-area-head">
              <h3>{{ group.title }}</h3>
              <a :href="withBase(group.link)">{{ group.linkText }} →</a>
            </div>
            <p class="lp-area-names">
              <template v-for="(name, i) in group.names" :key="name">
                <span v-if="i" class="lp-dot" aria-hidden="true">·</span>{{ name }}
              </template>
            </p>
          </article>
        </div>
      </section>

      <!-- Read out of the sidebar, so this list cannot fall behind the docs -
           see the script block. -->
      <section class="lp-section lp-docs">
        <header class="lp-head">
          <h2>Documentation</h2>
          <p class="lp-head-note">
            {{ docPages }} pages, all in the repository
          </p>
        </header>
        <div class="lp-doc-index">
          <div v-for="group in docGroups" :key="group.text" class="lp-doc-group">
            <h3>{{ group.text }}</h3>
            <ul>
              <li v-for="page in group.items" :key="page.link">
                <a :href="withBase(page.link)">{{ page.text }}</a>
              </li>
            </ul>
          </div>
        </div>
      </section>
    </main>

    <footer class="lp-footer">
      <div class="lp-footer-inner">
        <span
          >rozi, built on
          <a href="https://tui-lipan.dev" target="_blank" rel="noopener noreferrer"
            >tui-lipan</a
          ></span
        >
        <span class="lp-top-spacer" />
        <span>MPL-2.0 · © Adam Mikołajczyk</span>
      </div>
    </footer>
  </div>
</template>
