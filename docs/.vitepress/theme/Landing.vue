<script setup lang="ts">
import { useData, withBase } from "vitepress";
import { computed, onMounted, ref } from "vue";
import RoziStage from "./composition/RoziStage.vue";
import ConfigTabs from "./ConfigTabs.vue";
import InstallTabs from "./InstallTabs.vue";
import RoziIntro from "./RoziIntro.vue";
import { HERO_CUES, HERO_SCENES } from "./composition/scenes";
import { highlightToml } from "./toml";
import captureClip from "../../assets/captures/record-demo.mp4";
import capturePoster from "../../assets/captures/record-demo-poster.webp";
import shotWorkspace from "../../assets/captures/workspace.webp";
import shotTokyoNight from "../../assets/captures/theme-tokyo-night.webp";
import shotRosePineDawn from "../../assets/captures/theme-rose-pine-dawn.webp";
import shotMerged from "../../assets/captures/style-merged.webp";
import shotPowerline from "../../assets/captures/style-powerline.webp";
import shotPersonalized from "../../assets/captures/personalized.webp";

/* The pre-paint script in config.ts has already decided this and hidden the
   page accordingly; reading its class back is what keeps the two in step. */
const introPlaying = ref(false);
/* The capture clip is left paused, with controls, for anybody who asked for
   less motion; everybody else sees it loop. Started here rather than with
   `autoplay` so the server-rendered page never plays it before that is known. */
const captureVideo = ref<HTMLVideoElement | null>(null);
const captureStill = ref(false);
onMounted(() => {
  introPlaying.value =
    document.documentElement.classList.contains("rozi-intro-pending");
  captureStill.value = window.matchMedia(
    "(prefers-reduced-motion: reduce)",
  ).matches;
  if (!captureStill.value) {
    captureVideo.value?.play().catch(() => {
      captureStill.value = true;
    });
  }
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
    title: "Panes arrange themselves",
    body: "Each new pane splits the focused one along its longer side, so the screen stays balanced without dragging borders. Switch between seven layouts, float a pane, or fill the screen with one key.",
    link: "/layouts-and-panes",
    linkText: "Layouts and panes",
  },
  {
    title: "Sessions keep running",
    body: "Detach and your shells, editors, and servers carry on. Come back later, from another window, to the same panes and scrollback. After a reboot, rozi restores the layout, the commands, and their history.",
    link: "/sessions",
    linkText: "Sessions",
  },
  {
    title: "Other machines over SSH",
    body: "rozi --remote devbox opens a session on another machine, installs rozi there if you allow it, and reconnects after a dropped link. Remote sessions are listed next to local ones.",
    link: "/remote",
    linkText: "Remote sessions",
  },
  {
    title: "Coding agents at a glance",
    body: `rozi reads what coding-agent CLIs draw and marks each pane working, blocked, or finished, on its border, its workspace tab, and in the sidebar. ${stats.agents} agents are built in, and you can add your own.`,
    link: "/agents",
    linkText: "Agent detection",
  },
  {
    title: "Change it live",
    body: "Themes, frames, layouts, and keys change from inside rozi, previewed as you browse. Your choice is saved to config.toml, and an edit to the file applies the moment you save it.",
    link: "/customize",
    linkText: "Customize rozi",
  },
  {
    title: "A complete terminal in every pane",
    body: "True color, inline images, mouse support, scrollback search, and a vi-style copy mode. With shell integration, new panes open in the directory you are working in.",
    link: "/terminal",
    linkText: "Terminal features",
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
  { key: "Enter", mod: "Alt+Enter", what: "Open another pane" },
  { key: "h j k l", mod: "Alt+H…L", what: "Move focus by direction" },
  { key: "f", mod: "Alt+F", what: "Fullscreen the focused pane" },
  { key: "t", mod: "Alt+T", what: "Float the pane on top" },
  { key: "1…9", mod: "Alt+1…9", what: "Jump to a workspace" },
  { key: "p", mod: "Alt+P", what: "Search every command" },
  { key: "?", mod: "Alt+?", what: "Show and edit every key" },
  { key: "d", mod: "Alt+D", what: "Leave; named sessions keep running" },
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
      "shape-aware splits",
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
      "temporary sessions",
      "several windows per session",
      "shared sessions",
      "remote over SSH",
      "restore after reboot",
      "autosave",
      "profiles",
      "session picker",
    ],
  },
  {
    title: "Look and feel",
    link: "/customize",
    linkText: "Customize rozi",
    names: [
      `${stats.themes} themes`,
      "system theme",
      "live reload",
      "host terminal colors",
      "workbar segments",
      "powerline caps",
      "titlebar styles",
      "sidebar tabs",
      "Settings with live preview",
      "keybinding editor",
      "attention alerts",
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
      "capture-ui",
      "PNG and ANSI screenshots",
      "pane and UI recording",
      "GIF, video, and cast export",
      `${stats.hookEvents} hook events`,
      "services",
      "user commands",
      "extensions",
      "agent skill",
      "Vim and Neovim navigation",
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
      "published benchmarks",
      "private control sockets",
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

/* "Make it yours": one workspace, and the config lines that turn it into each
   tab. On a wide screen the lines sit in the copy column, which has room to
   spare beside the picture; stacked, the gallery keeps them under its own
   frame, next to the tabs that change them. */
const THEME_SHOTS = [
  {
    src: shotWorkspace,
    alt: "A rozi workspace with Neovim, a Git log, lazygit, and btop in the default theme",
    label: "rozi",
    code: `[theme]
name = "rozi"`,
  },
  {
    src: shotTokyoNight,
    alt: "The same workspace in the Tokyo Night theme",
    label: "Tokyo Night",
    code: `[theme]
name = "tokyo-night"`,
  },
  {
    src: shotRosePineDawn,
    alt: "The same workspace in the light Rose Pine Dawn theme",
    label: "Rose Pine Dawn",
    code: `[theme]
name = "rose-pine-dawn"`,
  },
  {
    src: shotMerged,
    alt: "Panes whose frames join into one grid, with rounded titles in the border",
    label: "Merged",
    code: `[pane]
border_mode = "merged"
titlebar = "integrated"
title_style = "round"`,
  },
  {
    src: shotPowerline,
    alt: "Thick pane frames with arrow-shaped titles, tabs, and badges",
    label: "Powerline",
    code: `[pane]
border_style = "thick"
title_style = "arrow"
workbar_style = "arrow"
workbar_tab_style = "arrow"`,
  },
  {
    src: shotPersonalized,
    alt: "Gruvbox Dark, a master layout, merged thick frames, a bottom workbar, and the sidebar on the right",
    label: "All together",
    code: `[theme]
name = "gruvbox-dark"

[pane]
border_mode = "merged"
border_style = "thick"
workbar_at_bottom = true

[sidebar]
visible = true
position = "right"`,
  },
];
const themeShot = ref(0);
const themeCode = computed(() => highlightToml(THEME_SHOTS[themeShot.value].code));

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

    <!-- Every link stays on the bar at every width; the 720px block in
         landing.css tightens them until they fit rather than hiding them
         behind a button. -->
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
            Your terminals, tiled like a window manager.
          </p>
          <p class="lp-sub">
            rozi arranges panes as you open them, keeps named sessions running
            after you leave, reaches other machines over SSH, and shows which
            coding agents need you. Change any of it live, from inside rozi. On
            Linux, macOS, and Windows.
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
          <p class="lp-head-note">All of it works out of the box</p>
        </header>
        <div class="lp-features">
          <article v-for="f in features" :key="f.title" class="lp-feature">
            <h3>{{ f.title }}</h3>
            <p>{{ f.body }}</p>
            <a :href="withBase(f.link)">{{ f.linkText }} →</a>
          </article>
        </div>
      </section>

      <section class="lp-section">
        <header class="lp-head">
          <h2>Make it yours</h2>
          <p class="lp-head-note">Previewed live · saved to config.toml · no restart</p>
        </header>
        <div class="lp-two lp-capture lp-themes">
          <div>
            <p class="lp-lead">
              Open <b>Settings</b> and move through a list: the theme, the pane
              frames, or the layout change behind the picker as you go.
              <kbd>Enter</kbd> keeps a value and writes it to
              <code>config.toml</code>; <kbd>Esc</kbd> puts the old one back.
            </p>
            <p class="lp-lead">
              Every tab here is the same workspace. Only the lines under the
              picture differ, and the programs in the panes pick up the theme
              too.
            </p>
            <a class="lp-more" :href="withBase('/customize')">Customize rozi →</a>
            <a class="lp-more lp-more-next" :href="withBase('/themes')"
              >{{ stats.themes }} themes →</a
            >
            <div class="lp-code lp-theme-code">
              <pre><code v-html="themeCode"></code></pre>
            </div>
          </div>
          <CaptureGallery v-model:active="themeShot" title="~/src/rozi — dev">
            <img
              v-for="shot in THEME_SHOTS"
              :key="shot.src"
              :src="shot.src"
              :alt="shot.alt"
              :data-label="shot.label"
              :data-code="shot.code"
            />
          </CaptureGallery>
        </div>

        <div class="lp-two lp-capture lp-capture-flip">
          <div>
            <h3 class="lp-h3">Rebind keys without a manual</h3>
            <p class="lp-lead">
              <kbd>Ctrl+A</kbd> then <kbd>?</kbd> lists every key in effect.
              Pick a command, press the new chord, and rozi shows it before
              saving, including any command that already uses it. The binding
              works the moment you confirm.
            </p>
            <p class="lp-lead">
              The prefix and the held modifier are rows in the same list, so
              moving from <kbd>Alt</kbd> to <kbd>Super</kbd> takes one change.
            </p>
            <a class="lp-more" :href="withBase('/keybindings#edit-keybindings-in-rozi')"
              >Edit keybindings →</a
            >
          </div>
          <CaptureGallery mode="steps" title="Keybindings">
            <img src="../../assets/captures/keys-find.webp" alt="The Keybindings overlay filtered to new pane" data-label="Find it">
            <img src="../../assets/captures/keys-record.webp" alt="The recording card showing the chord Ctrl+Alt+N" data-label="Press a key">
            <img src="../../assets/captures/keys-conflict.webp" alt="The recording card warning that Alt+W is already bound to Close pane" data-label="See conflicts">
            <img src="../../assets/captures/keys-saved.webp" alt="The Keybindings list showing New pane bound to Ctrl+Alt+N" data-label="Saved">
          </CaptureGallery>
        </div>
      </section>

      <section class="lp-section lp-two">
        <div>
          <header class="lp-head">
            <h2>First five minutes</h2>
          </header>
          <p class="lp-lead">
            Every command works two ways: press the prefix, then a key, or
            hold <kbd>Alt</kbd> and press the key. Rebind either, or hold
            <kbd>Super</kbd> instead.
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
                  <kbd>Ctrl+A</kbd><kbd>{{ row.key }}</kbd>
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
            Everything Settings changes lives in one file,
            <code>~/.config/rozi/config.toml</code> on Linux and macOS. rozi
            applies it when you save, without closing a pane, and a mistake
            never takes it down: a bad value falls back to its default with a
            warning.
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
          <h2>Screenshots and recordings</h2>
          <p class="lp-head-note">For agents, scripts, demos, and bug reports</p>
        </header>
        <div class="lp-two lp-capture">
          <div>
            <p class="lp-lead">
              <code>rozi capture-ui</code> returns the screen as rozi drew it:
              every pane, border, and overlay, and the images programs displayed.
              <code>rozi capture-pane</code> does the same for one pane, and works
              on a detached session too. Ask for plain text, ANSI, or a PNG, so a
              coding agent can look at what it is working in.
            </p>
            <p class="lp-lead">
              <code>rozi record</code> keeps a pane's screen over time, change by
              change. The session records it, so it carries on after you detach:
              review in the morning what an agent did overnight. Record the whole
              window instead for a demo, then replay it in a terminal or export a
              GIF, a video, or an asciinema cast.
            </p>
            <div class="lp-code">
              <pre><code><span class="tk-comment"># one pane, as text with its colors</span>
rozi capture-pane --target 3 --render ansi
<span class="tk-comment"># record this pane, label a moment, stop</span>
rozi record start --output ~/demo.rozirec
rozi record mark "tests started"
rozi record stop
<span class="tk-comment"># watch it back, or turn it into frames</span>
rozi record play ~/demo.rozirec --speed 2
rozi record export ~/demo.rozirec --to png-frames frames</code></pre>
            </div>
            <a class="lp-more" :href="withBase('/control#capturing-the-whole-ui')"
              >Capturing the screen →</a
            >
            <a class="lp-more lp-more-next" :href="withBase('/recording')"
              >Recording →</a
            >
          </div>
          <figure class="lp-shot">
            <video
              ref="captureVideo"
              :src="captureClip"
              :poster="capturePoster"
              :controls="captureStill"
              width="1920"
              height="1088"
              muted
              loop
              playsinline
              preload="metadata"
              aria-label="A rozi window with an editor and a shell. The command palette saves a screenshot of the editor pane. The shell shows the rozi logo as an image, then starts recording its own pane, marks the moment, and stops."
            ></video>
            <figcaption>Recorded by rozi itself.</figcaption>
          </figure>
        </div>
      </section>

      <section class="lp-section">
        <header class="lp-head">
          <h2>Feature index</h2>
          <p class="lp-head-note">Everything in v{{ theme.roziVersion }}</p>
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
