<script setup lang="ts">
import { Content, withBase } from "vitepress";
import { onMounted, ref } from "vue";
import RoziStage from "./composition/RoziStage.vue";
import RoziIntro from "./RoziIntro.vue";
import { HERO_CUES, HERO_SCENES } from "./composition/scenes";

/* The pre-paint script in config.ts has already decided this and hidden the
   page accordingly; reading its class back is what keeps the two in step. */
const introPlaying = ref(false);
onMounted(() => {
  introPlaying.value =
    document.documentElement.classList.contains("rozi-intro-pending");
});

const GITHUB = "https://github.com/tui-lipan/rozi";
const SPONSOR = "https://github.com/sponsors/Razuer";

const INSTALL = "git clone https://github.com/tui-lipan/rozi\ncd rozi && ./install.sh";

const copied = ref(false);

async function copyInstall() {
  try {
    await navigator.clipboard.writeText(INSTALL);
    copied.value = true;
    setTimeout(() => (copied.value = false), 1600);
  } catch {
    // Clipboard permission denied - the text is on screen and selectable.
  }
}

const facts = [
  "7 layouts",
  "9 workspaces",
  "29 themes",
  "Linux · macOS · Windows",
  "MIT OR Apache-2.0",
];

const features = [
  {
    title: "Panes that arrange themselves",
    body: "New panes split the pane you are looking at, along whichever side has more room. Seven arrangements are a keypress away — dwindle, master-and-stack, grid, columns, rows, a scrolling strip, and monocle. Any pane can float, go fullscreen, or be dragged and resized with the mouse.",
    link: "/layouts-and-panes",
    linkText: "Layouts & panes",
  },
  {
    title: "Nothing is lost when you close the window",
    body: "A background server owns every PTY, so leaving does not kill your work. rozi attach dev brings it all back — same layout, same running programs, same scrollback. Several windows can attach at once, and --remote myserver reaches a session on another machine over SSH.",
    link: "/sessions",
    linkText: "Sessions",
  },
  {
    title: "Change anything without restarting",
    body: "Save config.toml and it applies immediately — keys, colors, status bar — with your panes untouched. It works the other way too: flip a setting from the command palette and rozi writes it back into your file, so what you tried out is what you keep.",
    link: "/configuration",
    linkText: "Configuration",
  },
  {
    title: "A real terminal, not an approximation",
    body: "Mouse support, text selection, images, scrollback with search, copy mode with vi-style motions, clipboard paste, and true color. 29 built-in themes plus a system theme that follows your desktop — and the colors inside your panes follow whichever one is active.",
    link: "/terminal",
    linkText: "Terminal features",
  },
];

const firstKeys = [
  { keys: ["Ctrl-a", "Enter"], what: "Open another pane" },
  { keys: ["Ctrl-a", "h j k l"], what: "Move focus left / down / up / right" },
  { keys: ["Ctrl-a", "f"], what: "Make the focused pane fullscreen" },
  { keys: ["Ctrl-a", "t"], what: "Let the pane float on top" },
  { keys: ["Ctrl-a", "1…9"], what: "Jump to a workspace" },
  { keys: ["Ctrl-a", "p"], what: "Search every command by name" },
  { keys: ["Ctrl-a", "?"], what: "Show all keys" },
  { keys: ["Ctrl-a", "d"], what: "Leave — a named session keeps running" },
];

const alsoInBox = [
  ["Command palette", "Fuzzy-search every command by name"],
  ["Side panel", "Files, git changes, panes, sessions, coding agents"],
  ["Saved layouts", "Relaunch a setup with the same panes and commands"],
  ["Scratchpad", "A pane that drops over your work and hides again"],
  ["Copy mode & search", "Walk the scrollback with vi motions"],
  ["Synchronized typing", "Send what you type to every pane at once"],
  ["Shared control", "Several people attach; one drives the layout"],
  ["Scripting", "focus, send-text, new-pane, capture-pane"],
  ["Hooks", "Run your own commands when something happens"],
  ["Your own shortcuts", "Bind a key to open a pane or send text"],
  ["Status bar", "Built-in readouts, your text, or a timed command"],
  ["Placement rules", "Send panes running a command to a chosen spot"],
  ["Vim/Neovim navigator", "One set of keys for editor splits and panes"],
];

const CONFIG_SAMPLE = `[theme]
name = "catppuccin-mocha"

[input]
prefix = "ctrl-a"           # the key that starts a command
modifier = "alt"            # or "super"

[layout]
default = "dwindle"         # how new workspaces arrange panes

[pane]
border_style = "rounded"`;
</script>

<template>
  <div class="lp">
    <!-- Outside .lp's subtree: the page is faded out while the intro runs, and
         the overlay must not fade with it. -->
    <Teleport to="body">
      <RoziIntro v-if="introPlaying" @done="introPlaying = false" />
    </Teleport>

    <header class="lp-top">
      <a class="lp-brand" :href="withBase('/')">
        <img :src="withBase('/logo.svg')" alt="" width="24" height="24" />
        <span class="lp-brand-name">rozi</span>
        <span class="lp-chip">v0.2.0</span>
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
            A terminal multiplexer that feels like a modern window manager.
          </p>
          <p class="lp-sub">
            Split your terminal into panes, arrange them automatically, and pick up
            where you left off. Native on Linux, macOS, and Windows.
          </p>

          <div class="lp-install">
            <pre><code>{{ INSTALL }}</code></pre>
            <button
              type="button"
              class="lp-copy"
              :class="{ done: copied }"
              @click="copyInstall"
            >
              {{ copied ? "copied" : "copy" }}
            </button>
          </div>

          <div class="lp-cta">
            <a class="lp-btn primary" :href="withBase('/getting-started')"
              >Get started →</a
            >
            <a class="lp-btn" :href="withBase('/features')">What it does</a>
            <a
              class="lp-btn"
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
        <li v-for="fact in facts" :key="fact">{{ fact }}</li>
      </ul>

      <section class="lp-section">
        <h2 class="lp-h2">What you get</h2>
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
          <h2 class="lp-h2">First five minutes</h2>
          <p class="lp-lead">
            Everything happens through the prefix key — hold
            <kbd>Ctrl-a</kbd>, let go, then press one key. Prefer chords?
            <kbd>Alt</kbd> + the same key does the same thing. Both are fully
            rebindable.
          </p>
          <table class="lp-keytable">
            <tbody>
              <tr v-for="row in firstKeys" :key="row.what">
                <td class="lp-keycell">
                  <kbd v-for="k in row.keys" :key="k">{{ k }}</kbd>
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
          <h2 class="lp-h2">One file configures it</h2>
          <p class="lp-lead">
            <code>~/.config/rozi/config.toml</code> on Linux and macOS. Save it and
            the change lands immediately; a file that will not parse falls back to
            the defaults and tells you so.
          </p>
          <div class="lp-code">
            <pre><code>{{ CONFIG_SAMPLE }}</code></pre>
          </div>
          <a class="lp-more" :href="withBase('/configuration')"
            >Every setting →</a
          >
        </div>
      </section>

      <section class="lp-section">
        <h2 class="lp-h2">Also in the box</h2>
        <div class="lp-box-grid">
          <div v-for="[name, desc] in alsoInBox" :key="name" class="lp-box">
            <span class="lp-box-name">{{ name }}</span>
            <span class="lp-box-desc">{{ desc }}</span>
          </div>
        </div>
      </section>

      <!-- docs/index.md - the same table of contents GitHub shows for the
           folder, so the site and the repository never drift apart.
           The class goes on <Content> rather than a wrapper so the markup
           matches a doc page exactly (.vp-doc > div > p); a wrapper would add
           a level and the prose-measure rules in style.css would miss. -->
      <section class="lp-section lp-docs">
        <Content class="vp-doc" />
      </section>
    </main>

    <footer class="lp-footer">
      <div class="lp-footer-inner">
        <span
          >rozi — built on
          <a href="https://tui-lipan.dev" target="_blank" rel="noopener noreferrer"
            >tui-lipan</a
          ></span
        >
        <span class="lp-top-spacer" />
        <span>MIT OR Apache-2.0 · © Adam Mikołajczyk</span>
      </div>
    </footer>
  </div>
</template>
