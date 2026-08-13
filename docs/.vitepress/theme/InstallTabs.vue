<script setup lang="ts">
import { ref } from "vue";

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
let clear = 0;

function select(channel: Channel) {
  active.value = channel;
  copied.value = false;
}

async function copy() {
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
      :class="{ done: copied }"
      :aria-label="`Copy the ${active.id} install command`"
      @click="copy"
    >
      <code>{{ active.command }}</code>
      <span class="lp-copy" aria-hidden="true">{{ copied ? "copied" : "copy" }}</span>
    </button>

    <p class="lp-install-note">{{ active.note }}</p>
  </div>
</template>
