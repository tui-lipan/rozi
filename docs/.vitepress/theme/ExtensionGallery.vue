<script setup lang="ts">
import { computed, onMounted, reactive, ref } from "vue";
import { useData } from "vitepress";

/**
 * The public extension index as cards. Fetched in the browser rather than at build time: the site
 * deploys from release tags, and the index is rebuilt hourly.
 *
 * The URL and the trusted repositories come from `themeConfig.extensionIndex`, which the site
 * config reads out of `src/extension_catalog.rs`.
 */
type Entry = {
  repository: string;
  commit: string;
  id: string;
  title: string;
  description: string;
  version: string;
  platforms: string[];
  homepage: string | null;
  screenshots?: string[];
  stars: number;
  updated_at: string;
  commands: number;
  services: number;
  agents: number;
  sidebar_tabs: number;
  navigation_targets: number;
  suggested_keybindings: number;
};

const CONTRIBUTIONS: [keyof Entry, string, string][] = [
  ["commands", "command", "commands"],
  ["services", "service", "services"],
  ["sidebar_tabs", "sidebar tab", "sidebar tabs"],
  ["agents", "agent", "agents"],
  ["navigation_targets", "navigation target", "navigation targets"],
  ["suggested_keybindings", "keybinding", "keybindings"],
];

const PLATFORM_NAMES: Record<string, string> = {
  linux: "Linux",
  macos: "macOS",
  windows: "Windows",
  freebsd: "FreeBSD",
  netbsd: "NetBSD",
};

const { theme } = useData();
const index = theme.value.extensionIndex as { url: string; trusted: string[] };

const entries = ref<Entry[]>([]);
const status = ref<"loading" | "ready" | "failed">("loading");
const query = ref("");
const sort = ref<"stars" | "updated" | "title">("stars");
/** Which screenshot each card shows, by repository. */
const shown = reactive<Record<string, number>>({});
const copied = ref<string | null>(null);

onMounted(async () => {
  try {
    const response = await fetch(index.url);
    if (!response.ok) throw new Error(String(response.status));
    const document = await response.json();
    entries.value = Array.isArray(document?.extensions) ? document.extensions : [];
    status.value = "ready";
  } catch {
    status.value = "failed";
  }
});

const trusted = (entry: Entry) => index.trusted.includes(entry.repository);

const visible = computed(() => {
  const words = query.value.toLowerCase().split(/\s+/).filter(Boolean);
  const matches = entries.value.filter((entry) => {
    const text = [entry.title, entry.description, entry.id, entry.repository]
      .join(" ")
      .toLowerCase();
    return words.every((word) => text.includes(word));
  });
  const byTitle = (a: Entry, b: Entry) => a.title.localeCompare(b.title);
  const order: Record<typeof sort.value, (a: Entry, b: Entry) => number> = {
    stars: (a, b) => b.stars - a.stars || byTitle(a, b),
    updated: (a, b) => b.updated_at.localeCompare(a.updated_at) || byTitle(a, b),
    title: byTitle,
  };
  return [...matches].sort(order[sort.value]);
});

function screenshotUrl(entry: Entry, path: string): string {
  return encodeURI(`https://raw.githubusercontent.com/${entry.repository}/${entry.commit}/${path}`);
}

function contributions(entry: Entry): string[] {
  return CONTRIBUTIONS.filter(([key]) => (entry[key] as number) > 0).map(([key, one, many]) => {
    const count = entry[key] as number;
    return `${count} ${count === 1 ? one : many}`;
  });
}

function platforms(entry: Entry): string {
  return entry.platforms.length === 0
    ? "All platforms"
    : entry.platforms.map((platform) => PLATFORM_NAMES[platform] ?? platform).join(" · ");
}

function updated(entry: Entry): string {
  const date = new Date(entry.updated_at);
  return Number.isNaN(date.getTime())
    ? ""
    : date.toLocaleDateString("en", { year: "numeric", month: "short", day: "numeric" });
}

/** The homepage, unless it is only the repository again. */
function homepage(entry: Entry): string | null {
  const repository = `https://github.com/${entry.repository}`;
  return entry.homepage && entry.homepage.replace(/\/$/, "") !== repository ? entry.homepage : null;
}

function installCommand(entry: Entry): string {
  return `rozi extensions install https://github.com/${entry.repository}.git`;
}

async function copy(entry: Entry) {
  try {
    await navigator.clipboard.writeText(installCommand(entry));
    copied.value = entry.repository;
    setTimeout(() => {
      if (copied.value === entry.repository) copied.value = null;
    }, 1600);
  } catch {
    // Without clipboard access the command stays selectable in the card.
  }
}
</script>

<template>
  <div class="rz-extensions">
    <div class="rz-extensions-controls">
      <input
        v-model="query"
        class="rz-extensions-search"
        type="search"
        placeholder="Filter extensions"
        aria-label="Filter extensions"
      />
      <select v-model="sort" class="rz-extensions-sort" aria-label="Sort extensions">
        <option value="stars">Most stars</option>
        <option value="updated">Recently updated</option>
        <option value="title">Name</option>
      </select>
    </div>

    <p v-if="status === 'loading'" class="rz-extensions-note">Loading the extension index…</p>
    <p v-else-if="status === 'failed'" class="rz-extensions-note">
      The extension index could not be loaded. It is also available as
      <a :href="index.url">JSON</a>.
    </p>
    <p v-else-if="visible.length === 0" class="rz-extensions-note">
      No extension matches “{{ query }}”.
    </p>

    <div v-else class="rz-extensions-grid">
      <article v-for="entry in visible" :key="entry.repository" class="rz-extension">
        <div class="rz-extension-media">
          <template v-if="entry.screenshots?.length">
            <a
              :href="screenshotUrl(entry, entry.screenshots[shown[entry.repository] ?? 0])"
              target="_blank"
              rel="noopener"
            >
              <img
                :src="screenshotUrl(entry, entry.screenshots[shown[entry.repository] ?? 0])"
                :alt="`${entry.title} screenshot`"
                loading="lazy"
                decoding="async"
              />
            </a>
            <div v-if="entry.screenshots.length > 1" class="rz-extension-dots">
              <button
                v-for="(path, i) in entry.screenshots"
                :key="path"
                type="button"
                :class="{ active: (shown[entry.repository] ?? 0) === i }"
                :aria-label="`Screenshot ${i + 1} of ${entry.screenshots.length}`"
                @click="shown[entry.repository] = i"
              />
            </div>
          </template>
          <div v-else class="rz-extension-placeholder" aria-hidden="true">
            <span class="rz-extension-prompt">❯</span> {{ entry.id }}
          </div>
        </div>

        <div class="rz-extension-body">
          <header class="rz-extension-head">
            <!-- Not an <h3>: the site styles doc headings with `!important`. -->
            <div class="rz-extension-title" role="heading" aria-level="3">
              <a :href="`https://github.com/${entry.repository}`" target="_blank" rel="noopener">
                {{ entry.title }}
              </a>
            </div>
            <span class="rz-extension-version">{{ entry.version }}</span>
          </header>
          <div class="rz-extension-byline">
            <span>{{ entry.repository }}</span>
            <span v-if="trusted(entry)" class="rz-extension-trusted">Trusted</span>
          </div>
          <p class="rz-extension-description">{{ entry.description }}</p>
          <ul v-if="contributions(entry).length" class="rz-extension-chips">
            <li v-for="chip in contributions(entry)" :key="chip">{{ chip }}</li>
          </ul>
          <div class="rz-extension-meta">
            <span>★ {{ entry.stars }}</span>
            <span>{{ platforms(entry) }}</span>
            <span v-if="updated(entry)">Updated {{ updated(entry) }}</span>
            <a v-if="homepage(entry)" :href="homepage(entry)!" target="_blank" rel="noopener">
              Homepage
            </a>
          </div>
          <div class="rz-extension-install">
            <code>{{ installCommand(entry) }}</code>
            <button type="button" @click="copy(entry)">
              {{ copied === entry.repository ? "Copied" : "Copy" }}
            </button>
          </div>
        </div>
      </article>
    </div>
  </div>
</template>
