/* ─────────────────────────────────────────────────────────────────────
   Timeline runtime for the rozi composition.

   The composition is a pure function of one number: authored time `T`. Every
   position, opacity and typed character is derived from it, so the same piece
   can be played at different speeds, held on a frame, or looped over a slice
   of itself without touching the drawing code.

   Scenes carry two lengths. `dur` is wall-clock: how long the scene takes to
   watch. `nat` is authored: how much of the composition's own timeline it
   covers. When they differ the scene plays fast or slow, and - crucially -
   the cue table stays put. The piece's internals are pinned to authored
   seconds (`CUES.Agents + 3.9`), so compressing a scene speeds it up rather
   than desynchronising it.

   This is a re-implementation of the parts of the source animation's React
   stage that the piece actually calls, so the port stays faithful.
   ───────────────────────────────────────────────────────────────────── */

import { onBeforeUnmount, onMounted, ref, watch, type Ref } from "vue";

export const clamp = (v: number, min: number, max: number) =>
    Math.max(min, Math.min(max, v));

export const Easing = {
    easeOutQuart: (t: number) => 1 - --t * t * t * t,
    easeInOutQuart: (t: number) =>
        t < 0.5 ? 8 * t * t * t * t : 1 - 8 * --t * t * t * t,
    easeInOutSine: (t: number) => -(Math.cos(Math.PI * t) - 1) / 2,
    easeOutBack: (t: number) => {
        const c1 = 1.70158;
        const c3 = c1 + 1;
        return 1 + c3 * Math.pow(t - 1, 3) + c1 * Math.pow(t - 1, 2);
    },
};

type Tween = {
    from?: number;
    to?: number;
    start?: number;
    end?: number;
    ease?: (t: number) => number;
};

/** Single-segment tween. Holds `from` before `start` and `to` after `end`. */
export function animate({
    from = 0,
    to = 1,
    start = 0,
    end = 1,
    ease = Easing.easeInOutQuart,
}: Tween) {
    return (t: number) => {
        if (t <= start) return from;
        if (t >= end) return to;
        return from + (to - from) * ease((t - start) / (end - start));
    };
}

export type Scene = {
    name: string;
    /** Wall-clock seconds this scene plays for. */
    dur: number;
    /** Authored seconds it covers. Defaults to `dur`. */
    nat?: number;
};

export type Cues = Record<string, number>;

export type Timeline = {
    sections: { playStart: number; dur: number; authStart: number; nat: number }[];
    cues: Cues;
    /** Wall-clock length of one pass. */
    total: number;
    authoredTotal: number;
};

export function deriveScenes(scenes: Scene[], overrides: Cues = {}): Timeline {
    let playStart = 0;
    let authStart = 0;
    const sections: Timeline["sections"] = [];
    const cues: Cues = Object.create(null);

    for (const s of scenes) {
        const nat = typeof s.nat === "number" && s.nat > 0 ? s.nat : s.dur;
        sections.push({ playStart, dur: s.dur, authStart, nat });
        if (!(s.name in cues)) cues[s.name] = Math.round(authStart * 1000) / 1000;
        playStart += s.dur;
        authStart += nat;
    }

    return {
        sections,
        cues: { ...cues, ...overrides },
        total: Math.round(playStart * 1000) / 1000,
        authoredTotal: Math.round(authStart * 1000) / 1000,
    };
}

/** Map wall-clock time into the composition's authored timeline. */
export function warp(tl: Timeline, t: number): number {
    const ss = tl.sections;
    if (ss.length === 0) return 0;

    let idx = ss.length - 1;
    for (let i = 0; i < ss.length; i++) {
        if (t < ss[i].playStart + ss[i].dur) {
            idx = i;
            break;
        }
    }

    const s = ss[idx];
    const local = clamp(t - s.playStart, 0, s.dur);
    const T = s.authStart + (s.dur > 0 ? local * (s.nat / s.dur) : 0);
    return Math.min(T, tl.authoredTotal);
}

/* ── Style helper ─────────────────────────────────────────────────────
   React appends `px` to bare numbers in a style object; Vue does not. The
   piece was written against React, so it sets ~150 lengths as numbers. One
   normaliser at each `style={}` keeps the port a transliteration instead of
   150 hand-edits waiting to go wrong.
   ───────────────────────────────────────────────────────────────────── */

const UNITLESS = new Set([
    "opacity",
    "zIndex",
    "fontWeight",
    "lineHeight",
    "flex",
    "flexGrow",
    "flexShrink",
    "order",
    "zoom",
]);

export function px(style: Record<string, unknown>): Record<string, unknown> {
    const out: Record<string, unknown> = {};
    for (const key in style) {
        const v = style[key];
        out[key] = typeof v === "number" && !UNITLESS.has(key) ? `${v}px` : v;
    }
    return out;
}

/* ── Clock ────────────────────────────────────────────────────────────
   A requestAnimationFrame loop that only runs when the composition is worth
   drawing: on screen, in a visible tab, and not paused. A landing page has no
   business burning a core on an animation nobody is looking at.
   ───────────────────────────────────────────────────────────────────── */

export type ClockOptions = {
    loop: boolean;
    /** External pause switch (hover, an overlay on top, and so on). */
    paused?: Ref<boolean>;
    /** Element to watch; the clock idles while it is off screen. */
    target?: Ref<HTMLElement | null>;
    onFinished?: () => void;
};

export function useClock(tl: Ref<Timeline>, opts: ClockOptions) {
    const play = ref(0);
    const onScreen = ref(true);
    const tabVisible = ref(true);

    let raf = 0;
    let last = 0;
    let finished = false;

    const shouldRun = () =>
        onScreen.value &&
        tabVisible.value &&
        !opts.paused?.value &&
        !(finished && !opts.loop);

    function frame(ts: number) {
        raf = requestAnimationFrame(frame);
        if (!last) last = ts;
        // A long tab-out or a slow frame must not fast-forward the piece.
        const dt = Math.min((ts - last) / 1000, 1 / 20);
        last = ts;

        if (!shouldRun()) return;

        const next = play.value + dt;
        const total = tl.value.total;

        if (next >= total) {
            if (opts.loop) {
                play.value = total > 0 ? next % total : 0;
            } else {
                play.value = total;
                finished = true;
                opts.onFinished?.();
            }
        } else {
            play.value = next;
        }
    }

    function onTabChange() {
        tabVisible.value = !document.hidden;
        last = 0;
    }

    let io: IntersectionObserver | undefined;

    onMounted(() => {
        document.addEventListener("visibilitychange", onTabChange);
        onTabChange();

        const el = opts.target?.value;
        if (el && typeof IntersectionObserver !== "undefined") {
            io = new IntersectionObserver(
                ([entry]) => {
                    onScreen.value = entry.isIntersecting;
                    last = 0;
                },
                { rootMargin: "120px" },
            );
            io.observe(el);
        }

        raf = requestAnimationFrame(frame);
    });

    // A pause must not bank elapsed time and jump on resume.
    if (opts.paused) watch(opts.paused, () => (last = 0));

    onBeforeUnmount(() => {
        cancelAnimationFrame(raf);
        io?.disconnect();
        document.removeEventListener("visibilitychange", onTabChange);
    });

    return { play };
}
