/* ─────────────────────────────────────────────────────────────────────
   Two cuts of one composition.

   `nat` is authored time and is not free to change. The piece's opening is
   pinned to absolute authored seconds - `rozi` is typed at 0.9, the accent
   border traces the pane between 1.5 and 2.32, and the client snaps in at 2.3
   - so Boot must always cover 2.4 authored seconds no matter how fast it
   plays. Everything after Boot hangs off the cue table instead, which is what
   makes the later scenes safe to stretch and squeeze.
   ───────────────────────────────────────────────────────────────────── */

import type { Cues, Scene } from "./runtime";

/** The hero loop: boot, split, fill with work, and hold there. */
export const HERO_SCENES: Scene[] = [
    { name: "Boot", dur: 2.0, nat: 2.4 },
    { name: "Split", dur: 3.0, nat: 3.2 },
    { name: "Agents", dur: 6.4, nat: 6.4 },
];

/**
 * The hero stops before the logo resolve - the page already has a wordmark
 * two inches above it. Parking the three remaining cues past the end of time
 * holds that whole act on its first frame, where it is invisible and free,
 * without the piece needing a mode flag.
 */
export const HERO_CUES: Cues = { Tile: 9000, Mark: 9000, Name: 9000 };

/**
 * The full arc, for the first-visit intro: the same demo, then the panes fold
 * into the mark and the wordmark lands. It stops on that title rather than
 * running into the piece's own fade-out at 18.0, because the overlay hands
 * off to the hero instead of ending.
 */
export const CINEMATIC_SCENES: Scene[] = [
    { name: "Boot", dur: 1.5, nat: 2.4 },
    { name: "Split", dur: 2.3, nat: 3.2 },
    { name: "Agents", dur: 4.4, nat: 4.4 },
    { name: "Tile", dur: 1.4, nat: 2.0 },
    { name: "Mark", dur: 0.7, nat: 1.8 },
    { name: "Name", dur: 1.6, nat: 1.7 },
    { name: "Hold", dur: 0.9, nat: 1.0 },
];

/** Wall-clock length of the intro, for the progress bar and the failsafe. */
export const CINEMATIC_SECONDS = CINEMATIC_SCENES.reduce(
    (sum, s) => sum + s.dur,
    0,
);
