<script setup lang="ts">
/**
 * The first-visit intro: the composition plays its full arc over the page,
 * ending on the mark and wordmark, then dissolves into a hero that is already
 * showing the same wordmark in the same typeface.
 *
 * It is a courtesy, not a toll booth. It plays once per tab, never on a deep
 * link, never under reduced motion, and never on a screen too small to read
 * it - and while it plays, a click, a scroll, Escape, or the skip button ends
 * it immediately. The decision to play is taken by the inline script in
 * config.ts before first paint so the page underneath never flashes; this
 * component only honours it.
 */
import { onBeforeUnmount, onMounted, ref } from "vue";
import RoziStage from "./composition/RoziStage.vue";
import { CINEMATIC_SCENES } from "./composition/scenes";

const emit = defineEmits<{ done: [] }>();

const leaving = ref(false);
const progress = ref(0);

function dismiss() {
    if (leaving.value) return;
    leaving.value = true;

    try {
        sessionStorage.setItem("rozi:intro-seen", "1");
    } catch {
        // Private mode. Playing again next navigation is a small price.
    }

    // Uncovering the page and fading the overlay at the same time is what
    // makes the handoff read as one move rather than two.
    document.documentElement.classList.remove("rozi-intro-pending");
    window.setTimeout(() => emit("done"), 700);
}

function onKey(e: KeyboardEvent) {
    if (e.key === "Escape" || e.key === "Enter" || e.key === " ") dismiss();
}

onMounted(() => {
    document.addEventListener("keydown", onKey);
    window.addEventListener("wheel", dismiss, { passive: true });
    window.addEventListener("touchmove", dismiss, { passive: true });
    document.body.style.overflow = "hidden";
});

onBeforeUnmount(() => {
    document.removeEventListener("keydown", onKey);
    window.removeEventListener("wheel", dismiss);
    window.removeEventListener("touchmove", dismiss);
    document.body.style.overflow = "";
});
</script>

<template>
    <div
        class="rintro"
        :class="{ leaving }"
        role="dialog"
        aria-label="rozi intro"
        @click="dismiss"
    >
        <RoziStage
            :scenes="CINEMATIC_SCENES"
            fit="contain"
            :loop="false"
            :halted="leaving"
            @finished="dismiss"
            @progress="progress = $event"
        />

        <button type="button" class="rintro-skip" @click.stop="dismiss">
            Skip <span class="rintro-esc">Esc</span>
        </button>

        <div class="rintro-progress" aria-hidden="true">
            <span :style="{ transform: `scaleX(${progress})` }" />
        </div>
    </div>
</template>

<style scoped>
.rintro {
    position: fixed;
    inset: 0;
    z-index: 500;
    background: var(--vp-c-bg);
    cursor: pointer;
    transition:
        opacity 0.65s ease,
        transform 0.65s ease;
}
.rintro.leaving {
    opacity: 0;
    /* Drifting in slightly as it goes keeps the mark moving toward the hero
       rather than simply switching off. */
    transform: scale(1.04);
    pointer-events: none;
}

.rintro-skip {
    position: absolute;
    right: 22px;
    bottom: 26px;
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 7px 13px;
    font-family: var(--vp-font-family-base);
    font-size: 12px;
    color: var(--vp-c-text-2);
    background: color-mix(in srgb, var(--vp-c-bg-alt) 80%, transparent);
    border: 1px solid var(--vp-c-divider);
    border-radius: 3px;
    cursor: pointer;
    transition:
        color 0.15s,
        border-color 0.15s;
}
.rintro-skip:hover {
    color: var(--vp-c-text-1);
    border-color: var(--vp-c-border);
}
.rintro-esc {
    font-size: 10px;
    color: var(--vp-c-text-3);
    border: 1px solid var(--vp-c-divider);
    border-radius: 2px;
    padding: 1px 4px;
}

.rintro-progress {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 2px;
    background: var(--vp-c-divider);
}
.rintro-progress span {
    display: block;
    height: 100%;
    background: var(--rozi-gradient);
    transform-origin: 0 50%;
}
</style>
