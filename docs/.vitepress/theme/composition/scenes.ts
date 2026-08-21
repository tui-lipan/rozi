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

/**
 * The hero loop: boot, split, fill with work, then keep going - drag the
 * column split, and trade the two right-hand panes. Those last two acts are
 * the window-manager half of the pitch, so the loop is where they belong
 * rather than the title sequence.
 */
export const HERO_SCENES: Scene[] = [
    { name: "Boot", dur: 2.0, nat: 2.4 },
    { name: "Split", dur: 3.0, nat: 3.2 },
    // Ends 0.7s after the last agent line lands. Any longer and the loop
    // visibly waits for its next move.
    { name: "Agents", dur: 4.6, nat: 4.6 },
    // Long enough for the split to travel out, sit for a beat, and come back.
    { name: "Resize", dur: 2.2, nat: 2.2 },
    { name: "Swap", dur: 1.2, nat: 1.2 },
    // Keep a quarter-second beat between the compact closing acts. Longer
    // sections made the loop appear to drop to half speed after the sidebar
    // left, even though the movement tweens themselves had not changed.
    { name: "Hide", dur: 0.85, nat: 0.85 },
    { name: "Float", dur: 1.0, nat: 1.0 },
    { name: "Drag", dur: 1.05, nat: 1.05 },
    // In, held, and back out.
    { name: "Full", dur: 2.6, nat: 2.6 },
];

/**
 * The hero stops before the logo resolve - the page already has a wordmark
 * two inches above it. Parking the three remaining cues past the end of time
 * holds that whole act on its first frame, where it is invisible and free,
 * without the piece needing a mode flag.
 */
export const HERO_CUES: Cues = { Tile: 9000, Mark: 9000, Name: 9000 };

/**
 * The mirror image: the intro keeps the logo resolve and parks the two layout
 * acts, which would only delay the title it exists to deliver.
 */
export const CINEMATIC_CUES: Cues = {
    Resize: 9000,
    Swap: 9000,
    Hide: 9000,
    Float: 9000,
    Drag: 9000,
    Full: 9000,
};

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
    // Move straight from the folded panes into the wordmark. The old sections
    // held an almost static logo for over a second between those two moves.
    { name: "Tile", dur: 1.0, nat: 1.35 },
    { name: "Mark", dur: 0.1, nat: 0.15 },
    { name: "Name", dur: 1.6, nat: 1.7 },
    { name: "Hold", dur: 0.9, nat: 1.0 },
];

/** Wall-clock length of the intro, for the progress bar and the failsafe. */
export const CINEMATIC_SECONDS = CINEMATIC_SCENES.reduce(
    (sum, s) => sum + s.dur,
    0,
);
