<script setup lang="ts">
import { ref } from "vue";

/**
 * Every channel the landing page advertises. `command` is what the copy button
 * puts on the clipboard, so it stays exactly what a shell should receive - the
 * `note` carries anything a reader needs that a shell must not.
 */
type Channel = {
  id: string;
  command: string;
  /** One line. It shares a fixed-height row with nothing else. */
  note: string;
};

const channels: Channel[] = [
  {
    id: "curl",
    command: "curl -fsSL https://rozi.tui-lipan.dev/install | bash",
    note: "Linux and macOS \u00b7 installs the current release, links ~/.local/bin/rozi",
  },
  {
    id: "powershell",
    command: "irm https://rozi.tui-lipan.dev/install.ps1 | iex",
    note: "Windows \u00b7 installs under %LOCALAPPDATA%\\rozi",
  },
  {
    id: "cargo",
    command: "cargo install rozi",
    note: "Builds from crates.io \u00b7 needs Rust 1.90 or newer",
  },
  {
    id: "source",
    command: "git clone https://github.com/tui-lipan/rozi\ncd rozi && cargo install --path .",
    note: "Builds this checkout \u00b7 needs Rust 1.90 or newer",
  },
];

const active = ref(channels[0]);
const copied = ref(false);

function select(channel: Channel) {
  active.value = channel;
  copied.value = false;
}

async function copy() {
  try {
    await navigator.clipboard.writeText(active.value.command);
    copied.value = true;
    setTimeout(() => (copied.value = false), 1600);
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

    <div class="lp-install-body">
      <pre><code>{{ active.command }}</code></pre>
      <button
        type="button"
        class="lp-copy"
        :class="{ done: copied }"
        :aria-label="`Copy the ${active.id} install command`"
        @click="copy"
      >
        {{ copied ? "copied" : "copy" }}
      </button>
    </div>

    <p class="lp-install-note">{{ active.note }}</p>
  </div>
</template>
