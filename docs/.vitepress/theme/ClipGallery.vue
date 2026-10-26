<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";

/**
 * Recorded clips of rozi in the same window frame as `CaptureGallery`, with a tab per clip when
 * there are several.
 *
 * A clip plays, muted and looping, only while the frame is on screen. A reader who prefers reduced
 * motion gets the poster and the player's controls instead, so nothing moves unless they start it.
 * Everyone else never sees the poster: it is the clip's last frame, so it would give the ending
 * away before the clip starts.
 *
 * With several clips, each has its own video stacked in the stage, preloaded once the frame comes
 * into view. Switching rewinds the next one to its first frame and crossfades to it, so a tab
 * never flashes a poster or a black frame while a new source loads.
 *
 * Every clip is a recording rozi made of itself, and the title bar says so, out of the picture's
 * way.
 *
 * `v-model:active` shares the selected clip's index, so a page can show its config beside the frame.
 *
 * `bare` drops the window frame and tabs, for a single clip that was recorded with its own window.
 *
 * `advance` plays each clip once and moves on to the next, like a slideshow, and reports how far
 * the current one has played through `progress`. `tabs: false` hides the tab row, for a page that
 * draws its own. `sheen` sweeps a light across the stage on each switch, for the one showcase
 * that should feel like a show.
 */
type Clip = { src: string; poster: string; label: string; alt: string };

const props = withDefaults(
  defineProps<{
    clips: Clip[];
    title?: string;
    bare?: boolean;
    advance?: boolean;
    tabs?: boolean;
    sheen?: boolean;
  }>(),
  {
    title: "rozi",
    bare: false,
    advance: false,
    tabs: true,
    sheen: false,
  },
);

const emit = defineEmits<{ progress: [fraction: number] }>();

const active = defineModel<number>("active", { default: 0 });
const current = computed(() => props.clips[active.value]);

const root = ref<HTMLElement>();
const video = ref<HTMLVideoElement>();
const tabEls = ref<HTMLButtonElement[]>([]);
/* One per clip, by index. Not reactive: only the handlers read it. */
const videos: HTMLVideoElement[] = [];
const still = ref(true);
const inView = ref(false);
/* Set the first time the frame is on screen; until then only the first clip loads anything. */
const warm = ref(false);
/* The sheen across the stage marks a switch, so it waits for the first one. */
const switched = ref(false);

function playing(el: HTMLVideoElement | undefined, on: boolean) {
  if (!el) return;
  if (on) {
    el.play().catch(() => (still.value = true));
  } else {
    el.pause();
  }
}

function sync() {
  const on = inView.value && !still.value;
  if (props.bare) {
    playing(video.value, on);
    return;
  }
  videos.forEach((el, index) => playing(el, on && index === active.value));
}

function select(index: number) {
  const count = props.clips.length;
  active.value = (index + count) % count;
}

function onTabKey(event: KeyboardEvent, index: number) {
  const step = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[event.key];
  if (event.key === "Home" || event.key === "End" || step) {
    event.preventDefault();
    select(
      event.key === "Home" ? 0 : event.key === "End" ? props.clips.length - 1 : index + step!,
    );
    nextTick(() => tabEls.value[active.value]?.focus());
  }
}

/* A recording at phone width is too small to read; full screen lets the reader
   turn the phone and see it at its own size. iOS only offers it on the video. */
function fullscreen() {
  const el = (props.bare ? video.value : videos[active.value]) as
    | (HTMLVideoElement & { webkitEnterFullscreen?: () => void })
    | undefined;
  if (!el) return;
  if (el.requestFullscreen) el.requestFullscreen().catch(() => {});
  else el.webkitEnterFullscreen?.();
}

function onTime(index: number) {
  const el = videos[index];
  if (index === active.value && el?.duration) emit("progress", el.currentTime / el.duration);
}

function onEnded(index: number) {
  if (props.advance && index === active.value) select(active.value + 1);
}

/* Matches the crossfade in gallery.css: the outgoing clip keeps its frame while it fades. */
const FADE_MS = 700;

watch(active, (now, before) => {
  emit("progress", 0);
  switched.value = true;
  const next = videos[now];
  if (next) next.currentTime = 0;
  sync();
  setTimeout(() => {
    const old = videos[before];
    if (old && before !== active.value) old.currentTime = 0;
  }, FADE_MS);
});

let observer: IntersectionObserver | undefined;

onMounted(() => {
  still.value = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  observer = new IntersectionObserver(
    ([entry]) => {
      inView.value = entry.isIntersecting;
      if (entry.isIntersecting) warm.value = true;
      sync();
    },
    { threshold: 0.35 },
  );
  if (root.value) observer.observe(root.value);
});

onBeforeUnmount(() => observer?.disconnect());
</script>

<template>
  <figure v-if="bare" ref="root" class="rz-clips-bare">
    <button
      type="button"
      class="rz-gallery-zoom rz-clips-full"
      aria-label="Play full screen"
      title="Play full screen"
      @click="fullscreen"
    >
      <svg viewBox="0 0 16 16" aria-hidden="true">
        <path d="M2 6V2h4M14 6V2h-4M2 10v4h4M14 10v4h-4" />
      </svg>
    </button>
    <video
      ref="video"
      :key="current.src"
      :src="current.src"
      :poster="current.poster"
      :aria-label="current.alt"
      :controls="still"
      muted
      loop
      playsinline
      preload="metadata"
      @loadeddata="sync"
    ></video>
  </figure>
  <figure v-else ref="root" class="rz-gallery rz-clips">
    <div v-if="tabs && clips.length > 1" class="rz-gallery-tabs" role="tablist" :aria-label="title">
      <button
        v-for="(clip, index) in clips"
        :key="clip.src"
        ref="tabEls"
        type="button"
        role="tab"
        class="rz-gallery-tab"
        :class="{ active: index === active }"
        :aria-selected="index === active"
        :tabindex="index === active ? 0 : -1"
        @click="select(index)"
        @keydown="onTabKey($event, index)"
      >
        {{ clip.label }}
      </button>
    </div>

    <div class="rz-gallery-window">
      <div class="rz-gallery-bar">
        <span class="rz-gallery-dots" aria-hidden="true"><i /><i /><i /></span>
        <Transition name="rz-clips-title" mode="out-in">
          <span :key="title" class="rz-gallery-title">{{ title }}</span>
        </Transition>
        <span class="rz-gallery-end">
          <span class="rz-gallery-badge"><i aria-hidden="true" />recorded by rozi</span>
          <button
            type="button"
            class="rz-gallery-zoom"
            aria-label="Play full screen"
            title="Play full screen"
            @click="fullscreen"
          >
            <svg viewBox="0 0 16 16" aria-hidden="true">
              <path d="M2 6V2h4M14 6V2h-4M2 10v4h4M14 10v4h-4" />
            </svg>
          </button>
        </span>
      </div>
      <div class="rz-gallery-stage rz-clips-stage">
        <video
          v-for="(clip, index) in clips"
          :key="clip.src"
          :ref="(el) => (videos[index] = el as HTMLVideoElement)"
          :class="{ active: index === active }"
          :src="clip.src"
          :poster="still ? clip.poster : undefined"
          :aria-label="clip.alt"
          :aria-hidden="index !== active"
          :controls="still && index === active"
          :preload="warm || index === active ? 'auto' : 'none'"
          width="1920"
          height="1088"
          muted
          :loop="!advance"
          playsinline
          @loadeddata="sync"
          @timeupdate="onTime(index)"
          @ended="onEnded(index)"
        ></video>
        <span v-if="sheen && switched" :key="active" class="rz-stage-sheen" aria-hidden="true"></span>
      </div>
    </div>
  </figure>
</template>
