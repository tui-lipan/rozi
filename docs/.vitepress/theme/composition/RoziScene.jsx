/* ─────────────────────────────────────────────────────────────────────
   The rozi composition: a 1920x1080 scene drawn as a pure function of
   authored time `T`. Boot a shell, split it, fill the panes with real agent
   sessions, then fold the whole thing into the mark.

   Ported from the authored React piece. The drawing is deliberately kept as
   close to the original as it can be so the two stay comparable; the changes
   are only the ones needed to make it the site's:

     - the palette, page background and pane plate come from the site tokens
       rather than the piece's near-miss approximations of them,
     - the wordmark is the site's `rozi` in the site's typeface and gradient,
       so the intro can dissolve into the hero without the letters moving,
     - `T` and the cue table arrive as props instead of through a React
       context, and `px()` restores the bare-number style units Vue drops.

   `Tile`, `Mark` and `Name` may be cued past the end of time (see Stage.vue),
   which parks the whole logo resolve at its start frame and leaves the piece
   running as a loopable product demo.
   ───────────────────────────────────────────────────────────────────── */

import { animate, clamp, Easing, px } from "./runtime";

const BG = "#06070f";
const SCREEN = "#0b0d1c";
const EDGE = "#343858";
const TEXT = "#CCD0E6";
const DIM = "#8E93B4";
const ATT = "#F0A830";
const RED = "#FF5F57";
const MONO = "'JetBrains Mono', ui-monospace, SFMono-Regular, monospace";
// VS15 keeps the mark in the text font so it inherits Claude's title color.
const CLAUDE_MARK = "\u2733\uFE0E";

// Terminal roles mirror `rozi_theme()` while the composition keeps its own
// authored content and unchanged background plates.
const C = {
    dir: "#82AAFF",
    md: TEXT,
    toml: ATT,
    sh: "#4ADE80",
    json: DIM,
    green: "#4ADE80",
    run: "#4ADE80",
    pink: "#FD4A80",
    claude: ATT,
    cyan: "#82AAFF",
};

const MOTION = {
    glide: (from, to, start, end) =>
        animate({ from, to, start, end, ease: Easing.easeInOutQuart }),
    snap: (from, to, start, end) =>
        animate({ from, to, start, end, ease: Easing.easeOutBack }),
    fade: (from, to, start, end) =>
        animate({ from, to, start, end, ease: Easing.easeOutQuart }),
};

const SCR = { x: 300, y: 170, w: 1320, h: 760 };
const FULL = { x: 312, y: 228, w: 1296, h: 678 };
const LEFT = { x: 312, y: 228, w: 641, h: 678 };
const LEFT_N = { x: 560, y: 228, w: 393, h: 678 };
const SIDE = { x: 310, y: 178, w: 230, h: 744 };
const RIGHT = { x: 967, y: 228, w: 641, h: 678 };
const RTOP = { x: 967, y: 228, w: 641, h: 332 };
const RBOT = { x: 967, y: 574, w: 641, h: 332 };

/* After the split between the two columns is dragged right. The right column
   stops at 514 because that is the narrowest its longest line still fits in -
   below it the text clips mid-glyph, which reads as a broken layout rather
   than as a terminal that got smaller. */
const LEFT_W = { x: 560, y: 228, w: 520, h: 678 };
const RTOP_W = { x: 1094, y: 228, w: 514, h: 332 };
const RBOT_W = { x: 1094, y: 574, w: 514, h: 332 };

/* Where the pane goes once it leaves the tiling, and where it is dragged to.
   Both straddle the column boundary the tiled panes respect, which is what
   says "floating" without anything having to be labelled. */
const FLOAT_HOME = { x: 880, y: 300, w: 620, h: 420 };
const FLOAT_SIDE = { x: 360, y: 400, w: 620, h: 420 };

const lerp = (a, b, p) => a + (b - a) * p;
const lerpRect = (a, b, p) => ({
    x: lerp(a.x, b.x, p),
    y: lerp(a.y, b.y, p),
    w: lerp(a.w, b.w, p),
    h: lerp(a.h, b.h, p),
});
const typed = (text, T, start, cps) =>
    text.slice(0, Math.floor(clamp((T - start) * (cps || 22), 0, text.length)));

const SIDE_AGENTS = [
    ["Claude Code", "Refactoring layout engine", "1", "run"],
    ["OpenCode", "Idle", "1", "idle"],
    ["OpenCode", "Smoke test · 10s wait", "1", "run"],
    ["Claude Code", "Waiting for approval", "2", "att"],
];
const FILES = [
    ["assets", ""],
    ["benches", ""],
    ["docs", "M"],
    ["src", "M"],
    ["tests", "M"],
    ["README.md", ""],
];

function Cmd(props) {
    const { ps1, text, T, start, cps, caret, color, size } = props;
    if (T < start) return null;
    return (
        <div
            style={px({
                display: "flex",
                gap: 10,
                fontFamily: MONO,
                fontSize: size || 20,
                lineHeight: 1.9,
                whiteSpace: "pre",
                color: color || TEXT,
            })}
        >
            {ps1}
            <span>{typed(text, T, start, cps)}</span>
            {caret ? (
                <span
                    style={px({
                        display: "inline-block",
                        width: 10,
                        height: 20,
                        background: TEXT,
                        transform: "translateY(4px)",
                    })}
                ></span>
            ) : null}
        </div>
    );
}
Cmd.props = ["ps1", "text", "T", "start", "cps", "caret", "color", "size"];

function Out(props, { slots }) {
    const { T, at, color, size, indent } = props;
    return (
        <div
            style={px({
                fontFamily: MONO,
                fontSize: size || 19,
                lineHeight: 1.85,
                whiteSpace: "pre",
                color: color || DIM,
                paddingLeft: indent || 0,
                opacity: MOTION.fade(0, 1, at, at + 0.22)(T),
            })}
        >
            {slots.default?.()}
        </div>
    );
}
Out.props = ["T", "at", "color", "size", "indent"];

/* pinned input box at the bottom of an agent pane */
function InputBox(props) {
    const { op, bar, ps1, caret, status, right, foot, boxed } = props;
    return (
        <div
            style={px({
                position: "absolute",
                left: 0,
                right: 0,
                bottom: 0,
                opacity: op,
            })}
        >
            <div
                style={px({
                    display: "flex",
                    gap: 10,
                    alignItems: "center",
                    margin: "0 16px",
                    padding: "11px 13px",
                    border: "none",
                    borderTop: boxed ? `1px solid ${bar}` : "none",
                    borderBottom: boxed ? `1px solid ${bar}` : "none",
                    borderLeft: boxed ? "none" : `2px solid ${bar}`,
                    background: boxed ? "transparent" : "rgba(255,255,255,0.025)",
                    fontFamily: MONO,
                    fontSize: 17,
                    color: TEXT,
                    whiteSpace: "pre",
                })}
            >
                <span style={px({ color: bar })}>{ps1}</span>
                {caret ? (
                    <span
                        style={px({
                            width: 9,
                            height: 19,
                            background: TEXT,
                            display: "inline-block",
                        })}
                    ></span>
                ) : null}
            </div>
            <div
                style={px({
                    display: "flex",
                    gap: 10,
                    margin: "9px 18px 6px",
                    fontFamily: MONO,
                    fontSize: 15,
                    color: bar,
                    whiteSpace: "pre",
                })}
            >
                <span>{status}</span>
                <span style={px({ flex: 1 })}></span>
                <span style={px({ color: EDGE })}>{right}</span>
            </div>
            {foot ? (
                <div
                    style={px({
                        display: "flex",
                        margin: "0 18px 9px",
                        fontFamily: MONO,
                        fontSize: 14,
                        color: EDGE,
                        whiteSpace: "pre",
                    })}
                >
                    {foot}
                </div>
            ) : null}
        </div>
    );
}
InputBox.props = [
    "op",
    "bar",
    "ps1",
    "caret",
    "status",
    "right",
    "foot",
    "boxed",
];

const PS1_REPO = () => (
    <span style={px({ whiteSpace: "pre" })}>
        <span style={px({ color: C.green, fontWeight: 600 })}>rozi</span>{" "}
        <span style={px({ color: C.toml, fontStyle: "italic" })}>master</span>{" "}
        <span style={px({ color: C.green })}>●</span>{" "}
        <span style={px({ color: C.green })}>❯</span>
    </span>
);

const PS1_HOME = () => (
    <span style={px({ whiteSpace: "pre" })}>
        <span style={px({ color: C.cyan })}>~</span>{" "}
        <span style={px({ color: C.green })}>❯</span>
    </span>
);

function Pane(props, { slots }) {
    const { rect, boxStyle, border, label, labelColor, footer } = props;
    return (
        <div
            style={px({
                position: "absolute",
                left: 0,
                top: 0,
                width: rect.w,
                height: rect.h,
                background: "transparent",
                border: `1px solid ${border}`,
                borderRadius: 0,
                overflow: "hidden",
                transformOrigin: "50% 50%",
                ...boxStyle,
            })}
        >
            <div
                style={px({
                    height: 34,
                    display: "flex",
                    alignItems: "center",
                    gap: 10,
                    padding: "0 14px",
                    color: labelColor || border,
                })}
            >
                <span style={px({ fontFamily: MONO, fontSize: 15 })}>❐</span>
                <span
                    style={px({
                        fontFamily: MONO,
                        fontSize: 16,
                        letterSpacing: "0.04em",
                        whiteSpace: "nowrap",
                    })}
                >
                    {label}
                </span>
            </div>
            <div style={px({ padding: "10px 18px", position: "relative" })}>
                {slots.default?.()}
            </div>
            {footer}
        </div>
    );
}
Pane.props = ["rect", "boxStyle", "border", "label", "labelColor", "footer"];

function Sidebar(props) {
    const { accent, T, cues, op, dx, attention } = props;
    const rose = accent[0];
    const tab = (txt, active, key) => (
        <span
            key={key}
            style={px({
                fontFamily: MONO,
                fontSize: 15,
                padding: "2px 7px",
                color: active ? SCREEN : DIM,
                background: active ? rose : "transparent",
            })}
        >
            {txt}
        </span>
    );
    return (
        <div
            style={px({
                position: "absolute",
                left: SIDE.x,
                top: SIDE.y,
                width: SIDE.w,
                height: SIDE.h,
                border: "none",
                borderRight: `1px solid ${EDGE}`,
                opacity: op,
                transform: `translateX(${dx}px)`,
                overflow: "hidden",
            })}
        >
            <div style={px({ display: "flex", gap: 5, padding: "8px 9px" })}>
                {tab("Agents", true, "a")}
                {tab("Panes", false, "p")}
                {tab("Sessions", false, "s")}
            </div>
            <div
                style={px({
                    display: "flex",
                    justifyContent: "space-between",
                    padding: "10px 11px 6px",
                    fontFamily: MONO,
                    fontSize: 15,
                })}
            >
                <span style={px({ color: C.toml, fontWeight: 600 })}>rozi</span>
                <span style={px({ color: DIM })}>master</span>
            </div>
            {SIDE_AGENTS.map((a, i) => {
                const [name, sub, ws, st] = a;
                const rowOp = MOTION.fade(
                    0,
                    1,
                    cues.Agents + 0.5 + i * 0.16,
                    cues.Agents + 0.75 + i * 0.16,
                )(T);
                const isAtt = st === "att";
                const dot = st === "run" ? C.run : isAtt ? RED : DIM;
                return (
                    <div
                        key={i}
                        style={px({
                            padding: "4px 11px",
                            opacity: rowOp,
                            background: isAtt
                                ? `rgba(255,255,255,${0.05 + 0.05 * attention})`
                                : "transparent",
                        })}
                    >
                        <div
                            style={px({
                                display: "flex",
                                gap: 7,
                                alignItems: "baseline",
                                fontFamily: MONO,
                                fontSize: 15,
                            })}
                        >
                            <span style={px({ color: dot })}>
                                {st === "idle" ? "○" : "●"}
                            </span>
                            <span
                                style={px({
                                    color: TEXT,
                                    flex: 1,
                                    whiteSpace: "nowrap",
                                })}
                            >
                                {name}
                            </span>
                            <span style={px({ color: DIM })}>{ws}</span>
                        </div>
                        <div
                            style={px({
                                fontFamily: MONO,
                                fontSize: 13,
                                color: isAtt ? RED : DIM,
                                paddingLeft: 20,
                                whiteSpace: "nowrap",
                            })}
                        >
                            {sub}
                        </div>
                    </div>
                );
            })}
            <div
                style={px({
                    display: "flex",
                    gap: 5,
                    padding: "20px 9px 8px",
                    borderTop: `1px solid ${EDGE}`,
                    marginTop: 16,
                })}
            >
                {tab("Files", true, "f")}
                {tab("Git", false, "g")}
            </div>
            <div
                style={px({
                    fontFamily: MONO,
                    fontSize: 14,
                    color: C.cyan,
                    padding: "2px 11px 6px",
                })}
            >
                ~/Work/Projects/rozi
            </div>
            {FILES.map((f, i) => (
                <div
                    key={i}
                    style={px({
                        display: "flex",
                        justifyContent: "space-between",
                        padding: "2px 11px",
                        fontFamily: MONO,
                        fontSize: 15,
                        lineHeight: 1.55,
                        whiteSpace: "nowrap",
                        opacity: MOTION.fade(
                            0,
                            1,
                            cues.Agents + 1.25 + i * 0.07,
                            cues.Agents + 1.45 + i * 0.07,
                        )(T),
                    })}
                >
                    <span style={px({ color: f[1] ? TEXT : DIM })}>
                        {"› " + f[0]}
                    </span>
                    <span style={px({ color: ATT })}>{f[1]}</span>
                </div>
            ))}
        </div>
    );
}
Sidebar.props = ["accent", "T", "cues", "op", "dx", "attention"];

function Workbar(props) {
    const { accent, op, dy, shift } = props;
    const rose = accent[0];
    const pill = (txt, active, key) => (
        <div
            key={key}
            style={px({
                fontFamily: MONO,
                fontSize: 17,
                padding: "3px 12px",
                borderRadius: 0,
                color: active ? SCREEN : DIM,
                background: active ? rose : "transparent",
                fontWeight: active ? 600 : 400,
                whiteSpace: "pre",
            })}
        >
            {txt}
        </div>
    );
    return (
        <div
            style={px({
                position: "absolute",
                left: SCR.x + 10 + shift,
                top: SCR.y + 8,
                width: SCR.w - 20 - shift,
                height: 38,
                display: "flex",
                alignItems: "center",
                gap: 8,
                opacity: op,
                transform: `translateY(${dy}px)`,
            })}
        >
            <div
                style={px({
                    fontFamily: MONO,
                    fontSize: 17,
                    fontWeight: 600,
                    color: SCREEN,
                    padding: "3px 12px",
                    background: rose,
                    borderRadius: 0,
                })}
            >
                rozi
            </div>
            {pill("1 ·3", false, 1)}
            {pill("2 ·3", true, 2)}
            {pill("3", false, 3)}
            {pill("4", false, 4)}
            {pill("5", false, 5)}
            <div style={px({ flex: 1 })}></div>
            <div
                style={px({
                    fontFamily: MONO,
                    fontSize: 17,
                    fontWeight: 600,
                    color: SCREEN,
                    padding: "3px 12px",
                    background: rose,
                    whiteSpace: "nowrap",
                })}
            >
                ∞ dev
            </div>
        </div>
    );
}
Workbar.props = ["accent", "op", "dy", "shift"];

function RoziScene(props) {
    const { T, cues, accent, logoSrc, showTagline } = props;
    const [rose, violet] = accent;
    const S = cues.Split;
    const G = cues.Agents;


    const R = cues.Resize;
    const W = cues.Swap;
    const H = cues.Hide;
    const F = cues.Float;
    const D = cues.Drag;
    const U = cues.Full;

    const pA = MOTION.snap(0, 1, S, S + 0.6)(T);
    const pB = MOTION.snap(0, 1, S + 2.45, S + 3.05)(T);

    /* The side panel arrives with the agents and leaves again later, and the
       tiling gives up its width for it and takes it back. One value drives all
       of that: the panel's own travel, the left pane's width, and how far the
       workbar is pushed across. */
    const panel = clamp(
        MOTION.glide(0, 1, G - 0.15, G + 0.5)(T) -
            MOTION.fade(0, 1, H + 0.05, H + 0.62)(T),
        0,
        1,
    );
    const narrow = panel;

    /* Drag the column split right and back again, then trade the two
       right-hand panes. Both acts move at the pace the rest of the piece does
       - out of the gate immediately and decelerating hard - rather than easing
       in from rest, which next to a boot sequence and a typing prompt reads as
       a stall.

       Out and back as two overlapping ramps rather than one tween, so each
       leg gets its own fast-out easing. Neither overshoots: past the target
       the right column is narrower than its longest line, and a backswing
       would clip text for a few frames. rozi does not light the split while
       it moves, so neither does this. */
    const resize =
        MOTION.fade(0, 1, R + 0.05, R + 0.72)(T) -
        MOTION.fade(0, 1, R + 1.05, R + 1.78)(T);

    // Two panes trading places along one axis have to pass through each other.
    // One leads, both pinch in and push apart as they cross: an exchange
    // rather than one pane blinking into the other.
    const swapC = MOTION.snap(0, 1, W + 0.05, W + 0.85)(T);
    const swapB = MOTION.snap(0, 1, W + 0.16, W + 0.96)(T);
    const arc = (p) => Math.sin(Math.PI * clamp(p, 0, 1));
    const scaleB = 1 - 0.1 * arc(swapB);
    const scaleC = 1 - 0.1 * arc(swapC);
    const nudgeB = 34 * arc(swapB);
    const nudgeC = 34 * arc(swapC);
    // Panes are transparent so the screen shows through them, which turns the
    // moment they cross into two transcripts printed on top of each other.
    // Fading the plate in for the crossing keeps each one a solid window.
    const plate = (p) => `rgba(11, 13, 28, ${arc(p)})`;

    /* The last three acts all happen to one pane: it leaves the tiling, gets
       dragged across, and goes fullscreen and back. `reflow` is separate from
       `floatUp` on purpose - the tiling closes the gap immediately and
       smoothly while the pane itself pops out with a little overshoot. */
    const reflow = MOTION.fade(0, 1, F + 0.02, F + 0.7)(T);
    const floatUp = MOTION.snap(0, 1, F + 0.06, F + 0.78)(T);
    const dragged = MOTION.fade(0, 1, D + 0.05, D + 0.85)(T);
    const full =
        MOTION.fade(0, 1, U + 0.05, U + 0.72)(T) -
        MOTION.fade(0, 1, U + 1.5, U + 2.2)(T);

    const slotTop = lerpRect(RTOP, RTOP_W, resize);
    const slotBot = lerpRect(RBOT, RBOT_W, resize);

    const rectA = lerpRect(
        lerpRect(lerpRect(FULL, LEFT, pA), LEFT_N, narrow),
        LEFT_W,
        resize,
    );
    // With the third pane floated out, the one left behind takes the column.
    const rectB = lerpRect(
        lerpRect(RIGHT, lerpRect(slotTop, slotBot, swapB), pB),
        RIGHT,
        reflow,
    );
    const rectC = lerpRect(
        lerpRect(slotBot, slotTop, swapC),
        lerpRect(lerpRect(FLOAT_HOME, FLOAT_SIDE, dragged), FULL, clamp(full, 0, 1)),
        floatUp,
    );

    const inA = MOTION.snap(0.94, 1, 0.15, 1.0)(T);
    const opA = MOTION.fade(0, 1, 0.02, 0.4)(T);
    const inB = MOTION.snap(70, 0, S + 0.12, S + 0.8)(T);
    const inC = MOTION.snap(70, 0, S + 2.5, S + 3.15)(T);
    const opB = MOTION.fade(0, 1, S + 0.12, S + 0.45)(T);
    const opC = MOTION.fade(0, 1, S + 2.5, S + 2.8)(T);

    const barOp = MOTION.fade(0, 1, S - 0.05, S + 0.35)(T);
    // boot: the accent border traces around the pane as `rozi` loads, then the
    // client snaps in
    const trace = animate({
        from: 0,
        to: 1,
        start: 1.5,
        end: 2.32,
        ease: Easing.easeInOutSine,
    })(T);
    const booted = T >= 2.3;
    const tW = FULL.w;
    const tH = FULL.h;
    const tTotal = 2 * (tW + tH);
    const d = trace * tTotal;
    const seg = [
        clamp(d, 0, tW),
        clamp(d - tW, 0, tH),
        clamp(d - tW - tH, 0, tW),
        clamp(d - 2 * tW - tH, 0, tH),
    ];
    const traceOp = (T < 1.5 ? 0 : 1) * MOTION.fade(1, 0, S + 0.15, S + 0.5)(T);
    const barDy = MOTION.snap(-26, 0, S - 0.05, S + 0.55)(T);

    const sideOp =
        MOTION.fade(0, 1, G, G + 0.35)(T) * MOTION.fade(1, 0, H + 0.05, H + 0.5)(T);
    const sideDx = lerp(-240, 0, panel);
    const shellOp = MOTION.fade(1, 0, G, G + 0.35)(T);
    const sessOp = MOTION.fade(0, 1, G + 0.45, G + 0.8)(T);
    /* Attending to a blocked pane is what stops it asking. Focus lands on it
       as it floats out, so the alert stands down and the frame goes from the
       pulsing red to the ordinary focused accent - which is what rozi does,
       since a focused pane does not alert. */
    const attOn =
        clamp((T - (G + 2.45)) / 0.25, 0, 1) * MOTION.fade(1, 0, F, F + 0.4)(T);
    const attPulse = attOn * (0.5 + 0.5 * Math.sin((T - (G + 2.45)) * 5.2));
    const cFocused = T >= F + 0.25;

    const tile = MOTION.glide(0, 1, cues.Tile, cues.Tile + 1.0)(T);
    const paneOut = MOTION.fade(1, 0, cues.Tile + 0.55, cues.Tile + 0.92)(T);
    const targets = [
        { cx: 877, cy: 446, s: 0.34, r: -9 },
        { cx: 1043, cy: 446, s: 0.28, r: 9 },
        { cx: 960, cy: 630, s: 0.28, r: 0 },
    ];
    const xf = (rect, t, extraScale, ox, oy) => {
        const cx = rect.x + rect.w / 2;
        const cy = rect.y + rect.h / 2;
        const dx = lerp(0, t.cx - cx, tile);
        const dy = lerp(0, t.cy - cy, tile);
        return `translate(${rect.x + dx + (ox || 0)}px, ${rect.y + dy + (oy || 0)}px) scale(${
            lerp(1, t.s, tile) * (extraScale == null ? 1 : extraScale)
        }) rotate(${lerp(0, t.r, tile)}deg)`;
    };

    const logoIn = MOTION.snap(0.82, 1, cues.Tile + 0.42, cues.Tile + 1.3)(T);
    const logoOp = MOTION.fade(0, 1, cues.Tile + 0.4, cues.Tile + 0.8)(T);
    const move = MOTION.glide(0, 1, cues.Name, cues.Name + 0.8)(T);
    const logoSize = lerp(460, 300, move);
    const logoCx = lerp(960, 742, move);
    const logoCy = lerp(520, 540, move);
    const breathe = 1 + 0.012 * Math.sin((T - cues.Mark) * 1.5);
    const glow = MOTION.fade(0, 1, cues.Tile + 0.45, cues.Mark + 0.5)(T);

    const wordIn = MOTION.snap(34, 0, cues.Name + 0.35, cues.Name + 1.05)(T);
    const wordOp = MOTION.fade(0, 1, cues.Name + 0.35, cues.Name + 0.7)(T);
    const ruleW = MOTION.glide(0, 300, cues.Name + 0.65, cues.Name + 1.3)(T);
    const tagOp = MOTION.fade(0, 1, cues.Name + 0.95, cues.Name + 1.35)(T);

    const caret = Math.floor(T * 1.9) % 2 === 0 && T < cues.Tile;
    const deskOp = paneOut * opA;
    const agents = T >= G + 0.2;
    const cBorder =
        attOn > 0.02
            ? `rgba(255,95,87,${0.4 + 0.6 * attPulse})`
            : cFocused
              ? rose
              : EDGE;
    // A floating window is the only thing here that casts one, and it stops
    // once it is fullscreen and there is nothing left to cast onto.
    const floatLift = clamp(floatUp, 0, 1) * (1 - clamp(full, 0, 1));

    return (
        <div
            style={px({
                position: "absolute",
                inset: 0,
                background: BG,
                overflow: "hidden",
            })}
        >
            <div
                style={px({
                    position: "absolute",
                    inset: 0,
                    backgroundImage:
                        "radial-gradient(rgba(255,255,255,0.055) 1px, transparent 1px)",
                    backgroundSize: "48px 48px",
                    maskImage:
                        "radial-gradient(circle at 50% 48%, #000 20%, transparent 72%)",
                    "-webkit-mask-image":
                        "radial-gradient(circle at 50% 48%, #000 20%, transparent 72%)",
                    transform: `translate(${(T * 3.2) % 48}px, ${(T * 2) % 48}px)`,
                })}
            ></div>
            <div
                style={px({
                    position: "absolute",
                    left: 260,
                    top: -180,
                    width: 1400,
                    height: 1400,
                    background: `radial-gradient(circle, ${violet}44 0%, ${violet}00 62%)`,
                    opacity: glow * 0.9,
                    pointerEvents: "none",
                })}
            ></div>

            {/* The authored piece pushed in 4% across its run. On a screen made
                almost entirely of small type that resamples every glyph every
                frame, which reads as shimmer rather than as a camera move. */}
            <div style={px({ position: "absolute", inset: 0 })}>
                <div style={px({ position: "absolute", inset: 0, opacity: deskOp })}>
                    <div
                        style={px({
                            position: "absolute",
                            left: SCR.x,
                            top: SCR.y,
                            width: SCR.w,
                            height: SCR.h,
                            background: SCREEN,
                            border: `1px solid ${EDGE}`,
                            borderRadius: 18,
                            opacity: MOTION.fade(0, 1, 0.02, 0.45)(T),
                            boxShadow: "0 50px 120px rgba(0,0,0,0.6)",
                        })}
                    ></div>
                    <Workbar
                        accent={accent}
                        op={barOp}
                        dy={barDy}
                        shift={narrow * 250}
                    />
                    <Sidebar
                        accent={accent}
                        T={T}
                        cues={cues}
                        op={sideOp}
                        dx={sideDx}
                        attention={attPulse}
                    />

                    <div
                        style={px({
                            position: "absolute",
                            left: FULL.x,
                            top: FULL.y,
                            width: tW,
                            height: tH,
                            opacity: traceOp,
                            pointerEvents: "none",
                        })}
                    >
                        <div
                            style={px({
                                position: "absolute",
                                left: 0,
                                top: -1,
                                height: 2,
                                width: seg[0],
                                background: rose,
                                filter: `drop-shadow(0 0 6px ${rose})`,
                            })}
                        ></div>
                        <div
                            style={px({
                                position: "absolute",
                                right: -1,
                                top: 0,
                                width: 2,
                                height: seg[1],
                                background: rose,
                                filter: `drop-shadow(0 0 6px ${rose})`,
                            })}
                        ></div>
                        <div
                            style={px({
                                position: "absolute",
                                right: 0,
                                bottom: -1,
                                height: 2,
                                width: seg[2],
                                background: rose,
                                filter: `drop-shadow(0 0 6px ${rose})`,
                            })}
                        ></div>
                        <div
                            style={px({
                                position: "absolute",
                                left: -1,
                                bottom: 0,
                                width: 2,
                                height: seg[3],
                                background: rose,
                                filter: `drop-shadow(0 0 6px ${rose})`,
                            })}
                        ></div>
                    </div>

                    {/* left pane: shell → Claude Code session */}
                    <Pane
                        rect={rectA}
                        border={booted && !cFocused ? rose : EDGE}
                        labelColor={booted && !cFocused ? rose : DIM}
                        label={
                            agents
                                ? `${CLAUDE_MARK} Claude Code · rozi`
                                : booted
                                  ? "rozi"
                                  : "zsh"
                        }
                        boxStyle={{ transform: xf(rectA, targets[0], inA, 0, 0) }}
                        footer={
                            agents ? (
                                <InputBox
                                    op={sessOp * MOTION.fade(0, 1, G + 1.0, G + 1.3)(T)}
                                    bar={C.claude}
                                    ps1="&gt;"
                                    boxed={true}
                                    /* Stops when focus leaves for the pane that
                                       floats out. Only the focused pane has a
                                       live cursor. */
                                    caret={caret && !cFocused}
                                    status="⏵⏵ accept edits on"
                                    right="? for shortcuts"
                                />
                            ) : null
                        }
                    >
                        <div
                            style={px({
                                position: "absolute",
                                top: 10,
                                left: 18,
                                width: 600,
                                opacity: shellOp * MOTION.fade(1, 0, S, S + 0.18)(T),
                            })}
                        >
                            <Cmd
                                T={T}
                                start={0.9}
                                ps1={PS1_REPO()}
                                text="rozi"
                                cps={8}
                                caret={caret && T < S + 0.3}
                            />
                        </div>
                        <div
                            style={px({
                                position: "absolute",
                                top: 10,
                                left: 18,
                                width: 600,
                                opacity:
                                    shellOp * MOTION.fade(0, 1, S + 0.22, S + 0.34)(T),
                            })}
                        >
                            <Cmd
                                T={T}
                                start={S + 0.5}
                                ps1={PS1_REPO()}
                                text="git log --oneline -3"
                                cps={13}
                            />
                            <Out T={T} at={S + 2.0} size={19} color={C.toml}>
                                a3f19c2{" "}
                                <span style={px({ color: TEXT })}>
                                    layout: cache the tiling tree
                                </span>
                            </Out>
                            <Out T={T} at={S + 2.2} size={19} color={C.toml}>
                                7be04d1{" "}
                                <span style={px({ color: TEXT })}>
                                    workbar: workspace badges
                                </span>
                            </Out>
                            <Out T={T} at={S + 2.4} size={19} color={C.toml}>
                                e2c0b8f{" "}
                                <span style={px({ color: TEXT })}>
                                    agents: detect blocked state
                                </span>
                            </Out>
                        </div>
                        <div
                            style={px({
                                opacity: sessOp,
                                fontFamily: MONO,
                                lineHeight: 1.85,
                            })}
                        >
                            <div
                                style={px({
                                    fontSize: 18,
                                    color: C.claude,
                                    fontWeight: 600,
                                })}
                            >
                                {CLAUDE_MARK} Claude Code{" "}
                                <span style={px({ color: DIM, fontWeight: 400 })}>
                                    v2.1.228
                                </span>
                            </div>
                            <div style={px({ fontSize: 16, color: DIM })}>
                                Opus 5 · high effort · Claude Pro
                            </div>
                            <div style={px({ fontSize: 16, color: DIM })}>
                                ~/Work/Projects/rozi
                            </div>
                            <div style={px({ height: 16 })}></div>
                            <div
                                style={px({
                                    display: "inline-flex",
                                    gap: 9,
                                    background: "rgba(255,255,255,0.05)",
                                    padding: "2px 9px",
                                    fontSize: 16,
                                    color: TEXT,
                                    whiteSpace: "nowrap",
                                })}
                            >
                                <span style={px({ color: DIM })}>❯</span>refactor the
                                layout engine
                            </div>
                            <div style={px({ height: 14 })}></div>
                            <Out T={T} at={G + 1.6} size={16} color={DIM}>
                                ⏺ <span style={px({ color: TEXT })}>Edit</span>(
                                <span style={px({ color: C.cyan })}>
                                    src/layout/mod.rs
                                </span>
                                )
                            </Out>
                            <Out T={T} at={G + 1.8} size={15} indent={16}>
                                ⎿ +42 −18 · tiling pass split out
                            </Out>
                            <Out T={T} at={G + 2.2} size={16} color={DIM}>
                                ⏺ <span style={px({ color: TEXT })}>Bash</span>(
                                <span style={px({ color: C.cyan })}>cargo clippy</span>)
                            </Out>
                            <Out T={T} at={G + 2.4} size={15} indent={16}>
                                ⎿ 0 warnings · 42 suites green
                            </Out>
                            <div style={px({ height: 14 })}></div>
                            <Out T={T} at={G + 2.8} size={15} color={TEXT}>
                                ⏺ <span style={px({ fontWeight: 700 })}>Done</span> —
                                tiling pass is out
                            </Out>
                            <Out T={T} at={G + 2.85} size={15} color={TEXT} indent={14}>
                                of <span style={px({ color: C.cyan })}>mod.rs</span>.
                            </Out>
                            <div style={px({ height: 10 })}></div>
                            <Out T={T} at={G + 3.05} size={15} color={TEXT}>
                                - <span style={px({ color: C.cyan })}>layout/mod.rs</span>
                                : split and
                            </Out>
                            <Out T={T} at={G + 3.1} size={15} color={TEXT} indent={14}>
                                resize share one solve pass.
                            </Out>
                            <Out T={T} at={G + 3.3} size={15} color={TEXT}>
                                -{" "}
                                <span style={px({ color: C.cyan })}>layout/tree.rs</span>:
                                rebalance is
                            </Out>
                            <Out T={T} at={G + 3.35} size={15} color={TEXT} indent={14}>
                                incremental —{" "}
                                <span style={px({ color: C.toml })}>O(depth)</span> now.
                            </Out>
                            <div style={px({ height: 10 })}></div>
                            <Out T={T} at={G + 3.6} size={15} color={TEXT}>
                                Tree is clean;{" "}
                                <span style={px({ color: C.toml })}>cargo test</span>{" "}
                                passes.
                            </Out>
                            <div style={px({ height: 10 })}></div>
                            <Out T={T} at={G + 3.9} size={15} color={C.claude}>
                                {CLAUDE_MARK}{" "}
                                <span style={px({ color: DIM })}>Sautéed for 2m 49s</span>
                            </Out>
                        </div>
                    </Pane>

                    {/* top-right pane: rozi agent list → OpenCode */}
                    <Pane
                        rect={rectB}
                        border={EDGE}
                        labelColor={DIM}
                        label={agents ? "OpenCode · ~" : "~"}
                        boxStyle={{
                            opacity: opB,
                            background: plate(swapB),
                            transform: xf(
                                rectB,
                                targets[1],
                                scaleB,
                                inB + nudgeB,
                                0,
                            ),
                        }}
                        footer={
                            agents ? (
                                <div
                                    style={px({
                                        position: "absolute",
                                        left: 0,
                                        right: 0,
                                        bottom: 0,
                                        opacity:
                                            sessOp *
                                            MOTION.fade(0, 1, G + 1.1, G + 1.4)(T),
                                        fontFamily: MONO,
                                    })}
                                >
                                    <div
                                        style={px({
                                            margin: "0 14px",
                                            padding: "10px 12px 8px",
                                            borderLeft: `2px solid ${violet}`,
                                            background: "rgba(255,255,255,0.03)",
                                        })}
                                    >
                                        <div
                                            style={px({
                                                display: "flex",
                                                gap: 8,
                                                alignItems: "center",
                                                fontSize: 16,
                                                color: TEXT,
                                            })}
                                        >
                                            {/* This pane is never the focused
                                                one, so its cursor sits still
                                                and dim rather than blinking. */}
                                            <span
                                                style={px({
                                                    width: 9,
                                                    height: 18,
                                                    background: DIM,
                                                    display: "inline-block",
                                                })}
                                            ></span>
                                        </div>
                                        <div style={px({ height: 10 })}></div>
                                        <div
                                            style={px({
                                                fontSize: 15,
                                                whiteSpace: "nowrap",
                                            })}
                                        >
                                            <span style={px({ color: violet })}>
                                                Build
                                            </span>
                                            <span style={px({ color: DIM })}>
                                                {" · GPT-5.6 Sol OpenAI · "}
                                            </span>
                                            <span style={px({ color: ATT })}>high</span>
                                        </div>
                                    </div>
                                    <div
                                        style={px({
                                            display: "flex",
                                            margin: "7px 16px 8px",
                                            fontSize: 14,
                                            color: DIM,
                                            whiteSpace: "pre",
                                        })}
                                    >
                                        <span>~/Work/Projects/rozi</span>
                                        <span style={px({ flex: 1 })}></span>
                                        <span>22.5K (4%) · $0.04   ctrl+p commands</span>
                                    </div>
                                </div>
                            ) : null
                        }
                    >
                        <div
                            style={px({
                                position: "absolute",
                                top: 10,
                                left: 18,
                                width: 560,
                                opacity: shellOp,
                            })}
                        >
                            <Cmd
                                T={T}
                                start={S + 1.0}
                                ps1={PS1_HOME()}
                                text="opencode"
                                cps={9}
                            />
                            <div style={px({ height: 8 })}></div>
                            <Out T={T} at={S + 2.05} size={18} color={DIM}>
                                opencode <span style={px({ color: TEXT })}>1.18.16</span>
                            </Out>
                            <Out T={T} at={S + 2.35} size={18} color={DIM}>
                                loading workspace · 2 MCP servers
                            </Out>
                        </div>
                        <div
                            style={px({
                                opacity: sessOp,
                                fontFamily: MONO,
                                lineHeight: 1.7,
                            })}
                        >
                            <div
                                style={px({
                                    borderLeft: `2px solid ${C.claude}`,
                                    background: "rgba(255,255,255,0.03)",
                                    padding: "6px 11px",
                                    marginBottom: 9,
                                })}
                            >
                                <div
                                    style={px({
                                        fontSize: 15,
                                        color: TEXT,
                                        whiteSpace: "nowrap",
                                    })}
                                >
                                    read ../tui-lipan/AGENTS.md
                                </div>
                                <div style={px({ fontSize: 14, color: DIM })}>
                                    21:45 · 8/11/2026
                                </div>
                            </div>
                            <Out T={T} at={G + 1.2} size={15} color={ATT}>
                                + Thought: 190ms
                            </Out>
                            <Out T={T} at={G + 1.5} size={15} color={C.green}>
                                I'll read{" "}
                                <span style={px({ color: TEXT })}>
                                    ../tui-lipan/AGENTS.md
                                </span>{" "}
                                from the workspace.
                            </Out>
                            <Out T={T} at={G + 1.9} size={15} color={TEXT}>
                                → Read ~/Work/Projects/tui-lipan/AGENTS.md
                            </Out>
                            <Out T={T} at={G + 2.8} size={15} color={DIM}>
                                ▣ Skein · Grok 4.5 · 4m 51s · done
                            </Out>
                        </div>
                    </Pane>

                    {/* bottom-right pane: attach → blocked agent */}
                    <Pane
                        rect={rectC}
                        border={cBorder}
                        /* A blocked pane in rozi recolours and pulses its frame,
                           and the titlebar label follows the frame foreground.
                           It does not relabel itself - the pane keeps its own
                           title, so that is all this says. */
                        labelColor={attOn > 0.02 ? RED : cFocused ? rose : DIM}
                        label={agents ? `${CLAUDE_MARK} Claude Code · ~` : "~"}
                        boxStyle={{
                            opacity: opC,
                            transform: xf(
                                rectC,
                                targets[2],
                                scaleC,
                                -nudgeC,
                                inC,
                            ),
                            // Opaque whenever it is over another pane - crossing
                            // during the swap, or floating above the tiling. The
                            // alert wash it covers is 4% red; nobody misses it.
                            background:
                                Math.max(arc(swapC), clamp(floatUp, 0, 1)) > 0.01
                                    ? `rgba(11, 13, 28, ${Math.max(
                                          arc(swapC),
                                          clamp(floatUp, 0, 1),
                                      )})`
                                    : attOn > 0
                                      ? `rgba(255,95,87,${0.04 * attPulse})`
                                      : "transparent",
                            boxShadow: `0 ${26 * floatLift}px ${70 * floatLift}px rgba(0,0,0,${0.66 * floatLift})`,
                        }}
                        footer={
                            agents ? (
                                <div
                                    style={px({
                                        position: "absolute",
                                        left: 0,
                                        right: 0,
                                        bottom: 0,
                                        padding: "9px 16px 7px",
                                        borderTop: `1px solid ${C.cyan}`,
                                        background: "rgba(255,255,255,0.03)",
                                        fontFamily: MONO,
                                        opacity: MOTION.fade(0, 1, G + 2.3, G + 2.6)(T),
                                    })}
                                >
                                    <div
                                        style={px({
                                            fontSize: 15,
                                            color: C.cyan,
                                            fontWeight: 700,
                                        })}
                                    >
                                        Edit file
                                    </div>
                                    <div style={px({ height: 5 })}></div>
                                    <div
                                        style={px({
                                            fontSize: 15,
                                            color: TEXT,
                                            paddingLeft: 14,
                                            whiteSpace: "nowrap",
                                        })}
                                    >
                                        src/layout/tree.rs
                                    </div>
                                    <div
                                        style={px({
                                            fontSize: 14,
                                            color: DIM,
                                            paddingLeft: 14,
                                            whiteSpace: "nowrap",
                                        })}
                                    >
                                        rebalance siblings when a pane closes
                                    </div>
                                    <div style={px({ height: 7 })}></div>
                                    <div style={px({ fontSize: 15, color: TEXT })}>
                                        Do you want to proceed?
                                    </div>
                                    <div
                                        style={px({
                                            fontSize: 15,
                                            color: C.cyan,
                                            whiteSpace: "nowrap",
                                        })}
                                    >
                                        <span style={px({ color: RED })}>❯ </span>1. Yes
                                    </div>
                                    <div
                                        style={px({
                                            fontSize: 15,
                                            color: TEXT,
                                            paddingLeft: 16,
                                            whiteSpace: "nowrap",
                                        })}
                                    >
                                        2. Yes, and always allow edits in{" "}
                                        <span style={px({ fontWeight: 700 })}>src/</span>
                                    </div>
                                    <div
                                        style={px({
                                            fontSize: 15,
                                            color: TEXT,
                                            paddingLeft: 16,
                                            whiteSpace: "nowrap",
                                        })}
                                    >
                                        3. No, tell Claude what to do
                                    </div>
                                    <div style={px({ height: 5 })}></div>
                                    <div
                                        style={px({
                                            fontSize: 14,
                                            color: DIM,
                                            whiteSpace: "nowrap",
                                        })}
                                    >
                                        Esc to cancel · Tab to amend
                                    </div>
                                </div>
                            ) : null
                        }
                    >
                        <div
                            style={px({
                                position: "absolute",
                                top: 10,
                                left: 18,
                                opacity: MOTION.fade(1, 0, G + 0.75, G + 1.0)(T),
                            })}
                        >
                            <Cmd
                                T={T}
                                start={S + 2.5}
                                ps1={PS1_HOME()}
                                text="claude"
                                cps={11}
                            />
                        </div>
                        <div
                            style={px({
                                opacity:
                                    sessOp * MOTION.fade(0, 1, G + 0.95, G + 1.2)(T),
                                fontFamily: MONO,
                            })}
                        >
                            <div
                                style={px({
                                    fontSize: 16,
                                    color: C.claude,
                                    fontWeight: 700,
                                })}
                            >
                                Claude Code{" "}
                                <span style={px({ color: DIM, fontWeight: 400 })}>
                                    v2.1.228
                                </span>
                            </div>
                            <div
                                style={px({
                                    fontSize: 14,
                                    color: DIM,
                                    whiteSpace: "nowrap",
                                })}
                            >
                                Opus 4.6 with high effort · ~/Work/Projects/rozi
                            </div>
                            {/* This pane is the shortest of the three and its
                                approval prompt is the tallest footer, so the
                                transcript above it retires as the prompt slides
                                up - which is also what the real one does. */}
                            <div
                                style={px({
                                    opacity: MOTION.fade(1, 0, G + 2.1, G + 2.4)(T),
                                })}
                            >
                                <div style={px({ height: 7 })}></div>
                                <div
                                    style={px({
                                        display: "inline-flex",
                                        gap: 9,
                                        background: "rgba(255,255,255,0.05)",
                                        padding: "2px 9px",
                                        fontSize: 15,
                                        color: TEXT,
                                        whiteSpace: "nowrap",
                                    })}
                                >
                                    <span style={px({ color: DIM })}>❯</span>fix the pane
                                    focus race in tree.rs
                                </div>
                                <div style={px({ height: 7 })}></div>
                                <div
                                    style={px({
                                        fontSize: 15,
                                        color: TEXT,
                                        whiteSpace: "nowrap",
                                    })}
                                >
                                    <span style={px({ color: DIM })}>⏺ </span>Edit(
                                    <span style={px({ color: C.cyan })}>
                                        src/layout/tree.rs
                                    </span>
                                    )
                                </div>
                            </div>
                        </div>
                    </Pane>
                </div>

                <img
                    src={logoSrc}
                    alt=""
                    style={px({
                        position: "absolute",
                        left: 0,
                        top: 0,
                        width: logoSize,
                        height: logoSize,
                        transform: `translate(${logoCx - logoSize / 2}px, ${
                            logoCy - logoSize / 2
                        }px) scale(${logoIn * breathe})`,
                        transformOrigin: "50% 50%",
                        opacity: logoOp,
                        filter: `drop-shadow(0 40px 90px ${violet}55)`,
                    })}
                />

                <div
                    style={px({
                        position: "absolute",
                        left: 940,
                        top: 400,
                        opacity: wordOp,
                        transform: `translateX(${wordIn}px)`,
                    })}
                >
                    {/* The site's own wordmark, so the intro can hand off to the
                        hero without the letters changing shape. */}
                    <div
                        style={px({
                            fontFamily: MONO,
                            fontWeight: 800,
                            fontSize: 150,
                            lineHeight: 1,
                            letterSpacing: "-0.045em",
                            background: `linear-gradient(100deg, ${rose} 5%, ${violet} 92%)`,
                            "-webkit-background-clip": "text",
                            "background-clip": "text",
                            color: "transparent",
                        })}
                    >
                        rozi
                    </div>
                    <div
                        style={px({
                            height: 3,
                            width: ruleW,
                            background: `linear-gradient(90deg, ${rose}, ${violet})`,
                            margin: "26px 0 22px",
                        })}
                    ></div>
                    {showTagline ? (
                        <div
                            style={px({
                                fontFamily: MONO,
                                fontSize: 27,
                                letterSpacing: "0.1em",
                                color: DIM,
                                opacity: tagOp,
                                whiteSpace: "nowrap",
                            })}
                        >
                            tiling terminal multiplexer
                        </div>
                    ) : null}
                </div>
            </div>
        </div>
    );
}
RoziScene.props = ["T", "cues", "accent", "logoSrc", "showTagline"];

export default RoziScene;
