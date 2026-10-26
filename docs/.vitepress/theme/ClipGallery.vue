<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";

/**
 * Recorded clips of rozi in the same window frame as `CaptureGallery`, with a tab per clip when
 * there are several.
 *
 * A clip plays, muted and looping, only while the frame is on screen. A reader who prefers reduced
 * motion gets the poster and the player's controls instead, so nothing moves unless they start it.
 *
 * Every clip is a recording rozi made of itself, and the title bar says so, out of the picture's
 * way.
 *
 * `v-model:active` shares the selected clip's index, so a page can show its config beside the frame.
 *
 * `bare` drops the window frame and tabs, for a single clip that was recorded with its own window.
 */
type Clip = { src: string; poster: string; label: string; alt: string };

const props = withDefaults(defineProps<{ clips: Clip[]; title?: string; bare?: boolean }>(), {
  title: "rozi",
  bare: false,
});

const active = defineModel<number>("active", { default: 0 });
const current = computed(() => props.clips[active.value]);

const root = ref<HTMLElement>();
const video = ref<HTMLVideoElement>();
const tabs = ref<HTMLButtonElement[]>([]);
const still = ref(true);
const inView = ref(false);

function sync() {
  const el = video.value;
  if (!el) return;
  if (inView.value && !still.value) {
    el.play().catch(() => (still.value = true));
  } else {
    el.pause();
  }
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
    nextTick(() => tabs.value[active.value]?.focus());
  }
}

// A new source starts paused at its first frame; resume it if the frame should be playing.
watch(active, () => nextTick(sync));

let observer: IntersectionObserver | undefined;

onMounted(() => {
  still.value = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  observer = new IntersectionObserver(
    ([entry]) => {
      inView.value = entry.isIntersecting;
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
    <div v-if="clips.length > 1" class="rz-gallery-tabs" role="tablist" :aria-label="title">
      <button
        v-for="(clip, index) in clips"
        :key="clip.src"
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
        {{ clip.label }}
      </button>
    </div>

    <div class="rz-gallery-window">
      <div class="rz-gallery-bar">
        <span class="rz-gallery-dots" aria-hidden="true"><i /><i /><i /></span>
        <span class="rz-gallery-title">{{ title }}</span>
        <span class="rz-clips-badge"><i aria-hidden="true" />recorded by rozi</span>
      </div>
      <div class="rz-gallery-stage rz-clips-stage">
        <video
          ref="video"
          :key="current.src"
          :src="current.src"
          :poster="current.poster"
          :aria-label="current.alt"
          :controls="still"
          width="1920"
          height="1088"
          muted
          loop
          playsinline
          preload="metadata"
          @loadeddata="sync"
        ></video>
      </div>
    </div>
  </figure>
</template>
