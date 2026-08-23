<script setup lang="ts">
import { computed, ref } from "vue";
import { withBase } from "vitepress";
import { highlightToml } from "./toml";

/**
 * The customization surfaces, one tab each. Every snippet is real
 * `config.toml` - taken from `examples/config.toml` and `examples/sidebar.toml`
 * rather than composed here - because a reader's next move is to paste it.
 *
 * **Every snippet is exactly thirteen lines.** That is what keeps the panel a
 * fixed height with nothing reserved: an earlier version padded the box out to
 * the tallest snippet, which bought a stable height at the price of a hundred
 * empty pixels under every shorter one. Thirteen is the number the longest
 * useful sample needs; trim a new one to fit rather than raising it.
 */
type Surface = {
  id: string;
  note: string;
  doc: string;
  code: string;
};

const surfaces: Surface[] = [
  {
    id: "theme",
    note: "Configure presets, custom theme files, borders, titlebars, and animation timing.",
    doc: "/themes",
    code: `[theme]
name = "catppuccin-mocha"   # a preset, "system", or your own file

[pane]
border_mode = "merged"      # neighbours share one seam
border_style = "rounded"    # plain, double, thick
titlebar = "integrated"
workbar_powerline = true
padding = [0, 1]

[animations]
pane_style = "slide"        # or "scale"
geometry_ms = 220`,
  },
  {
    id: "keys",
    note: "Bind built-in command IDs or map a key trigger to a user command.",
    doc: "/keybindings",
    code: `# Every command id is in the help overlay, on <prefix> ?
[keys]
copy-mode = "b"                       # replace a default
search = "scheme:ctrl-f"              # still follows the prefix scheme
save-profile = ["ctrl-a z", "alt-z"]
spawn = { add = "super-enter" }       # keep the defaults as well
scratchpad = []                       # unbind it entirely

g = { run = "lazygit", label = "Git UI", keep_open = false }
alt-t = { run = "btop" }
"ctrl-a e" = { send = "ls -la\\n" }
u = { exec = "rozi run-action toggle-float" }
f = { popup = "fzf", label = "Find" }`,
  },
  {
    id: "workbar",
    note: "Add built-in readouts, static text, or commands that Rozi polls on an interval.",
    doc: "/configuration#workbar",
    code: `[workbar]
left = ["title", "workspaces"]
right = [
  "activity",
  { segment = "command:15:git branch --show-current", color = "info" },
  { segment = "clock", color = "accent" },
  "session",
]
clock_format = "%H:%M"

[workbar.alert]
blocked = true
mode = "pulse"   # the workspace tab pulses while a pane waits`,
  },
  {
    id: "sidebar",
    note: "Add built-in tabs, launcher lists, and command tabs to one or two docked panels.",
    doc: "/sidebar",
    code: `[sidebar]
width = 38
position = "left"
tabs = [
  "activity", "panes", "sessions", "git",
  { name = "files", show_hidden = true, explorer = true },
  { name = "dev", label = "Dev", entries = [
    { label = "Run tests", run = "cargo test" },
    { label = "Git UI", popup = "lazygit", keep_open = false },
  ] },
  { name = "branches", command = "git branch", interval = 15 },
]
panels = [["activity", "panes", "files"], ["git", "dev", "branches"]]`,
  },
  {
    id: "rules",
    note: "Rules place command spawns by first match. They can float or size a pane or choose its workspace.",
    doc: "/configuration#rules",
    code: `[[rules]]
match = "btop"
float = true
width = 0.7
height = 0.7
position = "cursor"      # centred on the mouse pointer

[[rules]]
# A regex when a short substring would catch its neighbours -
# a bare "top" rule otherwise fires on htop and btop too.
match_regex = '(^|[^\\w/-])top($|[^\\w-])'
workspace = 9
focus = false`,
  },
  {
    id: "hooks",
    note: "Hooks run on emitted events. Services can subscribe to events in a long-running process.",
    doc: "/hooks",
    code: `[[hooks]]
event = "pane-status-changed"
run = "notify-send \\"pane $ROZI_PANE is now $ROZI_STATUS\\""

[[hooks]]
event = "config-reloaded"
run = "logger rozi reloaded $ROZI_PATH"

# Keep the event stream open instead of spawning per event
[[services]]
name = "pr-watch"
run = "~/.config/rozi/pr-watch.sh"   # calls rozi subscribe
restart = "on-failure"`,
  },
  {
    id: "agents",
    note: "Bundled agent definitions use these tables. A user definition with the same ID replaces the bundled one.",
    doc: "/agents",
    code: `[[agents]]
id = "mytool"
label = "My Tool"
match = { names = ["mytool"], paths = ["@acme/mytool"] }

[[agents.states]]
state = "blocked"
screen = { all_of = ["esc dismiss"], any_of = ["enter submit"] }

[[agents.states]]
state = "working"
scope = "footer"          # only the last 8 non-empty lines
screen = { any_of = ["esc to interrupt"] }`,
  },
];

const active = ref(surfaces[0]);
const highlighted = computed(() => highlightToml(active.value.code));
</script>

<template>
  <div class="lp-snips">
    <div class="lp-snip-tabs" role="tablist" aria-label="Configuration surfaces">
      <button
        v-for="surface in surfaces"
        :key="surface.id"
        type="button"
        role="tab"
        class="lp-snip-tab"
        :class="{ active: surface.id === active.id }"
        :aria-selected="surface.id === active.id"
        @click="active = surface"
      >
        {{ surface.id }}
      </button>
    </div>

    <!-- Constant strings from this file, escaped by the highlighter. -->
    <div class="lp-snip-body">
      <pre><code v-html="highlighted"></code></pre>
    </div>

    <p class="lp-snip-note">
      <span>{{ active.note }}</span>
      <a :href="withBase(active.doc)">Reference →</a>
    </p>
  </div>
</template>
