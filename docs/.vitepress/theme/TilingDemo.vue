<script setup lang="ts">
/**
 * A scripted replay of a rozi window: each step is a real layout rozi would
 * produce from the chord shown beside it. Panes are keyed by id and positioned
 * in percentages, so moving between steps animates the same way the app does -
 * geometry glides, content does not reflow.
 */
import { computed, onBeforeUnmount, onMounted, ref } from "vue";

type Line = { t: string; c?: string };

type Pane = {
  id: string;
  title: string;
  x: number;
  y: number;
  w: number;
  h: number;
  focused?: boolean;
  floating?: boolean;
};

type Step = {
  chord: string[];
  caption: string;
  panes: Pane[];
};

const CONTENT: Record<string, Line[]> = {
  shell: [
    { t: "~/src/rozi", c: "path" },
    { t: "$ cargo test", c: "cmd" },
    { t: "test result: ok. 412 passed", c: "ok" },
    { t: "$ ", c: "cmd caret" },
  ],
  build: [
    { t: "$ cargo watch -x run", c: "cmd" },
    { t: "Compiling rozi v0.2.0", c: "dim" },
    { t: "Finished dev profile", c: "ok" },
    { t: "Running target/debug/rozi", c: "dim" },
  ],
  edit: [
    { t: "src/tiling.rs", c: "path" },
    { t: "fn split(&mut self, dir: Dir) {", c: "code" },
    { t: "    let ratio = self.aspect();", c: "code" },
    { t: "    self.dwindle(ratio)", c: "code" },
    { t: "}", c: "code" },
  ],
  logs: [
    { t: "$ git status -sb", c: "cmd" },
    { t: "## master", c: "dim" },
    { t: " M src/tiling.rs", c: "warn" },
  ],
  scratch: [
    { t: "scratchpad", c: "path" },
    { t: "$ rozi list-sessions", c: "cmd" },
    { t: "dev      3 panes", c: "ok" },
    { t: "notes    1 pane", c: "dim" },
  ],
};

const STEPS: Step[] = [
  {
    chord: ["rozi"],
    caption: "one shell, nothing to arrange",
    panes: [{ id: "shell", title: "zsh", x: 0, y: 0, w: 100, h: 100, focused: true }],
  },
  {
    chord: ["Ctrl-a", "Enter"],
    caption: "a new pane splits the wider side",
    panes: [
      { id: "shell", title: "zsh", x: 0, y: 0, w: 50, h: 100 },
      { id: "build", title: "cargo watch", x: 50, y: 0, w: 50, h: 100, focused: true },
    ],
  },
  {
    chord: ["Ctrl-a", "Enter"],
    caption: "this one is taller than wide, so it splits across",
    panes: [
      { id: "shell", title: "zsh", x: 0, y: 0, w: 50, h: 100 },
      { id: "build", title: "cargo watch", x: 50, y: 0, w: 50, h: 50 },
      { id: "edit", title: "nvim", x: 50, y: 50, w: 50, h: 50, focused: true },
    ],
  },
  {
    chord: ["Ctrl-a", "Enter"],
    caption: "dwindle keeps folding into the focused tile",
    panes: [
      { id: "shell", title: "zsh", x: 0, y: 0, w: 50, h: 100 },
      { id: "build", title: "cargo watch", x: 50, y: 0, w: 50, h: 50 },
      { id: "edit", title: "nvim", x: 50, y: 50, w: 25, h: 50 },
      { id: "logs", title: "git", x: 75, y: 50, w: 25, h: 50, focused: true },
    ],
  },
  {
    chord: ["Ctrl-a", "t"],
    caption: "float a pane on top without disturbing the tiling",
    panes: [
      { id: "shell", title: "zsh", x: 0, y: 0, w: 50, h: 100 },
      { id: "build", title: "cargo watch", x: 50, y: 0, w: 50, h: 50 },
      { id: "edit", title: "nvim", x: 50, y: 50, w: 25, h: 50 },
      { id: "logs", title: "git", x: 75, y: 50, w: 25, h: 50 },
      {
        id: "scratch",
        title: "scratchpad",
        x: 16,
        y: 16,
        w: 62,
        h: 60,
        focused: true,
        floating: true,
      },
    ],
  },
  {
    chord: ["Ctrl-a", "f"],
    caption: "fullscreen the focused pane, then bring the others back",
    panes: [{ id: "edit", title: "nvim", x: 0, y: 0, w: 100, h: 100, focused: true }],
  },
];

const STEP_MS = 2600;

const index = ref(0);
const paused = ref(false);
const step = computed(() => STEPS[index.value]);

let timer: ReturnType<typeof setInterval> | undefined;

function advance() {
  if (paused.value) return;
  index.value = (index.value + 1) % STEPS.length;
}

function select(i: number) {
  index.value = i;
  restart();
}

function restart() {
  if (timer) clearInterval(timer);
  timer = setInterval(advance, STEP_MS);
}

onMounted(() => {
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)");
  if (reduced.matches) {
    // Land on the fully tiled step and stay there.
    index.value = 3;
    return;
  }
  restart();
});

onBeforeUnmount(() => {
  if (timer) clearInterval(timer);
});
</script>

<template>
  <div
    class="tdemo"
    @mouseenter="paused = true"
    @mouseleave="paused = false"
  >
    <div class="tdemo-window">
      <div class="tdemo-titlebar">
        <span class="tdemo-dot r" />
        <span class="tdemo-dot y" />
        <span class="tdemo-dot g" />
        <span class="tdemo-session">dev</span>
      </div>

      <div class="tdemo-canvas">
        <TransitionGroup name="tpane">
          <div
            v-for="pane in step.panes"
            :key="pane.id"
            class="tdemo-pane"
            :class="{ focused: pane.focused, floating: pane.floating }"
            :style="{
              left: pane.x + '%',
              top: pane.y + '%',
              width: pane.w + '%',
              height: pane.h + '%',
            }"
          >
            <div class="tdemo-pane-inner">
              <div class="tdemo-pane-title">{{ pane.title }}</div>
              <div class="tdemo-pane-body">
                <div
                  v-for="(line, i) in CONTENT[pane.id]"
                  :key="i"
                  class="tdemo-line"
                  :class="line.c"
                >
                  {{ line.t }}
                </div>
              </div>
            </div>
          </div>
        </TransitionGroup>
      </div>

      <div class="tdemo-workbar">
        <span
          v-for="ws in 5"
          :key="ws"
          class="tdemo-ws"
          :class="{ on: ws === 1 }"
          >{{ ws }}</span
        >
        <span class="tdemo-spacer" />
        <span class="tdemo-badge">dwindle</span>
        <span class="tdemo-badge"
          >{{ step.panes.length }}
          {{ step.panes.length === 1 ? "pane" : "panes" }}</span
        >
      </div>
    </div>

    <div class="tdemo-caption">
      <span class="tdemo-chord">
        <kbd v-for="key in step.chord" :key="key">{{ key }}</kbd>
      </span>
      <span class="tdemo-text">{{ step.caption }}</span>
    </div>

    <div class="tdemo-steps" role="tablist" aria-label="Layout steps">
      <button
        v-for="(s, i) in STEPS"
        :key="i"
        type="button"
        role="tab"
        class="tdemo-step"
        :class="{ on: i === index }"
        :aria-selected="i === index"
        :aria-label="s.caption"
        @click="select(i)"
      />
    </div>
  </div>
</template>
