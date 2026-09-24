<script setup lang="ts">
import {
  Comment,
  Fragment,
  computed,
  nextTick,
  onBeforeUnmount,
  onMounted,
  ref,
  useSlots,
  type VNode,
} from "vue";
import { highlightToml } from "./toml";

/**
 * Screenshots of rozi, shown as a switcher on the site and as plain images on GitHub.
 *
 * The markdown writes ordinary `<img>` tags inside `<CaptureGallery>`. GitHub drops the unknown
 * wrapper and renders the images one after another; here the component reads them out of its slot
 * instead, so a page has one source for both. Each image may carry:
 *
 * - `data-label`   the tab or step name (falls back to the alt text)
 * - `data-caption` a sentence shown under the frame
 * - `data-code`    TOML shown under the caption, with `\n` for line breaks
 *
 * `mode="tabs"` compares variants of one screen: themes, layouts, styles. `mode="steps"` walks a
 * flow in order, advancing on its own while in view unless the reader prefers reduced motion or
 * has taken over.
 */
const props = withDefaults(
  defineProps<{
    mode?: "tabs" | "steps";
    title?: string;
    interval?: number;
  }>(),
  { mode: "tabs", title: "rozi", interval: 4200 },
);

type Shot = {
  src: string;
  alt: string;
  label: string;
  caption: string;
  code: string;
};

const slots = useSlots();

function images(nodes: VNode[]): VNode[] {
  return nodes.flatMap((node) => {
    if (node.type === Comment) return [];
    if (node.type === Fragment && Array.isArray(node.children)) {
      return images(node.children as VNode[]);
    }
    if (node.type === "img") return [node];
    if (Array.isArray(node.children)) return images(node.children as VNode[]);
    return [];
  });
}

const shots = computed<Shot[]>(() =>
  images(slots.default?.() ?? []).map((node) => {
    const p = (node.props ?? {}) as Record<string, string>;
    return {
      src: p.src,
      alt: p.alt ?? "",
      label: p["data-label"] ?? p.alt ?? "",
      caption: p["data-caption"] ?? "",
      code: (p["data-code"] ?? "").replace(/\\n/g, "\n"),
    };
  }),
);

const active = ref(0);
const current = computed(() => shots.value[active.value]);
const highlighted = computed(() =>
  current.value?.code ? highlightToml(current.value.code) : "",
);

const root = ref<HTMLElement>();
const tabs = ref<HTMLButtonElement[]>([]);
const zoomed = ref(false);
const dialog = ref<HTMLDialogElement>();

/** Set once the reader picks a step, so autoplay never fights them for it. */
const tookOver = ref(false);
const inView = ref(false);
const hovering = ref(false);
const reducedMotion = ref(true);
const playing = computed(
  () =>
    props.mode === "steps" &&
    !tookOver.value &&
    !reducedMotion.value &&
    inView.value &&
    !hovering.value &&
    !zoomed.value,
);

function select(index: number, byReader = true) {
  const count = shots.value.length;
  active.value = (index + count) % count;
  if (byReader) tookOver.value = true;
}

function onTabKey(event: KeyboardEvent, index: number) {
  const step = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[event.key];
  if (event.key === "Home" || event.key === "End" || step) {
    event.preventDefault();
    const next =
      event.key === "Home" ? 0 : event.key === "End" ? shots.value.length - 1 : index + step!;
    select(next);
    nextTick(() => tabs.value[active.value]?.focus());
  }
}

function openZoom() {
  zoomed.value = true;
  dialog.value?.showModal();
}

function closeZoom() {
  dialog.value?.close();
}

function onDialogKey(event: KeyboardEvent) {
  if (event.key === "ArrowRight") select(active.value + 1);
  if (event.key === "ArrowLeft") select(active.value - 1);
}

let timer: ReturnType<typeof setInterval> | undefined;
let observer: IntersectionObserver | undefined;
let motion: MediaQueryList | undefined;
const onMotion = () => (reducedMotion.value = motion!.matches);

onMounted(() => {
  motion = window.matchMedia("(prefers-reduced-motion: reduce)");
  onMotion();
  motion.addEventListener("change", onMotion);
  observer = new IntersectionObserver(
    ([entry]) => (inView.value = entry.intersectionRatio >= 0.6),
    { threshold: [0, 0.6, 1] },
  );
  if (root.value) observer.observe(root.value);
  timer = setInterval(() => {
    if (playing.value) select(active.value + 1, false);
  }, props.interval);
});

onBeforeUnmount(() => {
  clearInterval(timer);
  observer?.disconnect();
  motion?.removeEventListener("change", onMotion);
});
</script>

<template>
  <figure
    v-if="shots.length"
    ref="root"
    class="rz-gallery"
    :class="`rz-gallery--${mode}`"
    @mouseenter="hovering = true"
    @mouseleave="hovering = false"
  >
    <div
      v-if="mode === 'tabs' && shots.length > 1"
      class="rz-gallery-tabs"
      role="tablist"
      :aria-label="title"
    >
      <button
        v-for="(shot, index) in shots"
        :key="shot.src"
        ref="tabs"
        type="button"
        role="tab"
        class="rz-gallery-tab"
        :class="{ active: index === active }"
        :aria-selected="index === active"
        :tabindex="index === active ? 0 : -1"
        @click="select(index)"
        @keydown="onTabKey($event, index)"
      >
        {{ shot.label }}
      </button>
    </div>

    <div class="rz-gallery-window">
      <div class="rz-gallery-bar">
        <span class="rz-gallery-dots" aria-hidden="true"><i /><i /><i /></span>
        <span class="rz-gallery-title">{{ title }}</span>
        <button
          type="button"
          class="rz-gallery-zoom"
          aria-label="View full size"
          title="View full size"
          @click="openZoom"
        >
          <svg viewBox="0 0 16 16" aria-hidden="true">
            <path d="M2 6V2h4M14 6V2h-4M2 10v4h4M14 10v4h-4" />
          </svg>
        </button>
      </div>
      <button type="button" class="rz-gallery-stage" aria-label="View full size" @click="openZoom">
        <img
          v-for="(shot, index) in shots"
          :key="shot.src"
          :src="shot.src"
          :alt="index === active ? shot.alt : ''"
          :aria-hidden="index !== active"
          :class="{ active: index === active }"
          :loading="index === 0 ? 'eager' : 'lazy'"
          decoding="async"
        />
      </button>
    </div>

    <ol v-if="mode === 'steps'" class="rz-gallery-steps">
      <li v-for="(shot, index) in shots" :key="shot.src">
        <button
          type="button"
          class="rz-gallery-step"
          :class="{ active: index === active, done: index < active }"
          :aria-current="index === active ? 'step' : undefined"
          @click="select(index)"
        >
          <span class="rz-gallery-step-num">{{ index + 1 }}</span>
          <span class="rz-gallery-step-label">{{ shot.label }}</span>
          <span
            v-if="index === active && playing"
            :key="`${active}-progress`"
            class="rz-gallery-progress"
            :style="{ animationDuration: `${interval}ms` }"
          />
        </button>
      </li>
    </ol>

    <figcaption v-if="current.caption || current.code" class="rz-gallery-caption">
      <p v-if="current.caption">{{ current.caption }}</p>
      <!-- Constant strings from the page's markdown, escaped by the highlighter. -->
      <pre v-if="current.code"><code v-html="highlighted"></code></pre>
    </figcaption>

    <dialog
      ref="dialog"
      class="rz-gallery-dialog"
      aria-label="Screenshot"
      @close="zoomed = false"
      @click.self="closeZoom"
      @keydown="onDialogKey"
    >
      <img :src="current.src" :alt="current.alt" @click="closeZoom" />
      <button type="button" class="rz-gallery-close" aria-label="Close" @click="closeZoom">
        Esc
      </button>
    </dialog>
  </figure>
</template>
