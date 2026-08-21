<script setup lang="ts">
import { nextTick, ref } from "vue";

/**
 * Every channel the landing page advertises. `command` is what the clipboard
 * receives, so it stays exactly what a shell should get - the `note` carries
 * anything a reader needs that a shell must not.
 *
 * Both fields are one line. That is what lets the box keep a fixed height
 * without reserving blank space for the longest entry, so keep new channels
 * to a single command and a short note.
 */
type Channel = {
  id: string;
  command: string;
  note: string;
};

const channels: Channel[] = [
  {
    id: "curl",
    command: "curl -fsSL https://rozi.tui-lipan.dev/install | bash",
    note: "Linux and macOS · installs the current release, links ~/.local/bin/rozi",
  },
  {
    id: "powershell",
    command: "irm https://rozi.tui-lipan.dev/install.ps1 | iex",
    note: "Windows · installs under %LOCALAPPDATA%\\rozi",
  },
  {
    id: "cargo",
    command: "cargo install rozi",
    note: "Builds from crates.io · needs Rust 1.90 or newer",
  },
  {
    id: "source",
    command: "cargo install --git https://github.com/tui-lipan/rozi",
    note: "Builds the master branch · needs Rust 1.90 or newer",
  },
];

const active = ref(channels[0]);
const copied = ref(false);
const rippling = ref(false);
let clear = 0;

function select(channel: Channel) {
  active.value = channel;
  copied.value = false;
}

/**
 * The ripple runs on the press, not on the clipboard write: it is feedback for
 * the click, and a permission failure is not a reason to swallow it. The
 * `copied` label is the part that reports the outcome.
 *
 * Restarting the animation needs the class off for a frame - a keyframe
 * animation on an element that already carries the class does not replay. A
 * keyboard press has no coordinates (`detail` is 0), so it ripples from the
 * middle of the strip rather than from its top-left corner.
 */
async function copy(event: MouseEvent) {
  const strip = event.currentTarget as HTMLElement;
  const box = strip.getBoundingClientRect();
  const fromPointer = event.detail > 0;
  strip.style.setProperty(
    "--ripple-x",
    fromPointer ? `${event.clientX - box.left}px` : "50%",
  );
  strip.style.setProperty(
    "--ripple-y",
    fromPointer ? `${event.clientY - box.top}px` : "50%",
  );

  rippling.value = false;
  await nextTick();
  rippling.value = true;

  try {
    await navigator.clipboard.writeText(active.value.command);
    copied.value = true;
    window.clearTimeout(clear);
    clear = window.setTimeout(() => (copied.value = false), 1600);
  } catch {
    // Clipboard permission denied - the text is on screen and selectable.
  }
}
</script>

<template>
  <div class="lp-install">
    <div class="lp-install-tabs" role="tablist" aria-label="Install methods">
      <button
        v-for="channel in channels"
        :key="channel.id"
        type="button"
        role="tab"
        class="lp-install-tab"
        :class="{ active: channel.id === active.id }"
        :aria-selected="channel.id === active.id"
        @click="select(channel)"
      >
        {{ channel.id }}
      </button>
    </div>

    <!-- The whole row is the copy control, so the command itself is the
         target rather than a small button beside it. The `copy` label is a
         span, not a nested button, which would be invalid inside one. -->
    <button
      type="button"
      class="lp-install-cmd"
      :class="{ done: copied, rippling }"
      @animationend="rippling = false"
      :aria-label="`Copy the ${active.id} install command`"
      @click="copy"
    >
      <code>{{ active.command }}</code>
      <span class="lp-copy" aria-hidden="true">{{ copied ? "copied" : "copy" }}</span>
    </button>

    <p class="lp-install-note">{{ active.note }}</p>
  </div>
</template>
