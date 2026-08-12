<script setup lang="ts">
/**
 * Fits the 1920x1080 composition into whatever box it is given and drives its
 * clock. Two consumers: the looping hero demo and the one-shot intro. They
 * differ only in the scene list and the fit, which is the whole point of
 * keeping the piece a function of time.
 */
import { computed, onBeforeUnmount, onMounted, ref, shallowRef, watch } from "vue";
import { withBase } from "vitepress";
import RoziScene from "./RoziScene.jsx";
import { clamp, deriveScenes, useClock, warp, type Cues, type Scene } from "./runtime";

const props = withDefaults(
    defineProps<{
        scenes: Scene[];
        /** Cues to pin outside the played range - see ACCENT/HERO_CUES below. */
        cueOverrides?: Cues;
        loop?: boolean;
        /** `width` fills the container; `contain` also fits the height. */
        fit?: "width" | "contain";
        /** Authored frame to hold when motion is off or the screen is small. */
        staticAt?: number;
        pauseOnHover?: boolean;
        /** External pause, e.g. while the intro is on top of the hero. */
        halted?: boolean;
        /** Dip to black across the seam so a looping piece does not jump cut. */
        fadeLoopEdges?: boolean;
        showTagline?: boolean;
    }>(),
    {
        cueOverrides: () => ({}),
        loop: true,
        fit: "width",
        staticAt: 10.5,
        pauseOnHover: false,
        halted: false,
        fadeLoopEdges: false,
        showTagline: true,
    },
);

const emit = defineEmits<{ finished: []; progress: [value: number] }>();

/* The logo gradient, which is also the site's. */
const ACCENT = ["#fd4a80", "#982bf2"];

const host = ref<HTMLElement | null>(null);
const scale = ref(0);
const ready = ref(false);
const hovering = ref(false);
const still = ref(false);

const timeline = shallowRef(deriveScenes(props.scenes, props.cueOverrides));
watch(
    () => [props.scenes, props.cueOverrides],
    () => (timeline.value = deriveScenes(props.scenes, props.cueOverrides)),
);

const paused = computed(
    () => props.halted || still.value || (props.pauseOnHover && hovering.value),
);

const { play } = useClock(timeline, {
    loop: props.loop,
    paused,
    target: host,
    onFinished: () => emit("finished"),
});

const T = computed(() =>
    still.value ? props.staticAt : warp(timeline.value, play.value),
);

watch(play, (v) => emit("progress", timeline.value.total ? v / timeline.value.total : 0));

/* A loop that cuts straight from the finished layout back to an empty pane
   reads as a glitch; a short dip either side of the seam reads as a breath. */
const edge = computed(() => {
    if (!props.fadeLoopEdges || still.value) return 1;
    const total = timeline.value.total;
    return Math.min(
        clamp(play.value / 0.5, 0, 1),
        clamp((total - play.value) / 0.7, 0, 1),
    );
});

function measure() {
    const el = host.value;
    if (!el) return;
    const w = el.clientWidth;
    const h = el.clientHeight;
    if (!w) return;
    scale.value =
        props.fit === "contain" ? Math.min(w / 1920, h / 1080) : w / 1920;
    ready.value = true;
}

let ro: ResizeObserver | undefined;
let reduced: MediaQueryList | undefined;
let narrow: MediaQueryList | undefined;
const syncStill = () =>
    (still.value = !!reduced?.matches || !!narrow?.matches);

onMounted(() => {
    measure();

    ro = new ResizeObserver(measure);
    if (host.value) ro.observe(host.value);

    // Small screens scale the piece past the point where its text means
    // anything, so spending a core animating it buys nothing. Same conclusion,
    // different reason, when the reader has asked for less motion.
    reduced = window.matchMedia("(prefers-reduced-motion: reduce)");
    narrow = window.matchMedia("(max-width: 700px)");
    syncStill();
    reduced.addEventListener("change", syncStill);
    narrow.addEventListener("change", syncStill);
});

onBeforeUnmount(() => {
    ro?.disconnect();
    reduced?.removeEventListener("change", syncStill);
    narrow?.removeEventListener("change", syncStill);
});
</script>

<template>
    <div
        ref="host"
        class="rstage"
        :class="[`fit-${fit}`, { ready }]"
        @mouseenter="hovering = true"
        @mouseleave="hovering = false"
    >
        <div
            class="rstage-frame"
            :style="{
                transform: `translate(-50%, -50%) scale(${scale})`,
                opacity: edge,
            }"
        >
            <RoziScene
                :T="T"
                :cues="timeline.cues"
                :accent="ACCENT"
                :logo-src="withBase('/logo.svg')"
                :show-tagline="showTagline"
            />
        </div>
    </div>
</template>

<style scoped>
.rstage {
    position: relative;
    width: 100%;
    overflow: hidden;
    opacity: 0;
    transition: opacity 0.4s ease;
}
.rstage.ready {
    opacity: 1;
}
.rstage.fit-width {
    aspect-ratio: 1920 / 1080;
}
.rstage.fit-contain {
    height: 100%;
}
.rstage-frame {
    position: absolute;
    left: 50%;
    top: 50%;
    width: 1920px;
    height: 1080px;
    transform-origin: 50% 50%;
    /* The piece paints its own background edge to edge; without this the
       scaled box can show a hairline of the page behind it. */
    will-change: transform;
}
</style>
