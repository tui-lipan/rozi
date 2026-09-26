use std::time::Duration;

use crate::state::{Pane, State};
use tui_lipan::animation::Easing;
use tui_lipan::prelude::{FloatRect, TransitionConfig};

/// Default durations for [`WindowAnimationConfig`]. Callers read the configured values off that
/// type rather than these, so a user override cannot be bypassed by reaching for the default.
const GEOMETRY_MS: u64 = 220;
const CLOSE_MS: u64 = 120;
const OPEN_DELAY_MS: u64 = 36;
const FOCUS_CHROME_MS: u64 = 160;
const ALERT_PULSE_MS: u64 = 1600;
const ALERT_PULSE_MIN_HALF_MS: u64 = 400;
/// A long, subtle breathe needs fewer samples than short focus feedback. Keeping this separate from
/// the app-wide colour cadence avoids repainting the whole realized tree at 30 fps indefinitely.
pub const ALERT_PULSE_FRAME_RATE: u16 = 10;
/// Alert borders remain recognizably alert-colored at the bottom of their breathe.
pub const ALERT_PULSE_BLEND: f32 = 0.55;

/// How much longer a "calm" alert breathes than an urgent one. A finished agent is good news you
/// have not read yet, not a request for an answer, so it should not compete with a blocked pane for
/// attention. An integer multiple keeps the two in a harmonic relationship: they realign every
/// `ALERT_PULSE_CALM_FACTOR` beats instead of drifting past each other, which is what makes two
/// simultaneous breathes read as one system rather than as noise.
const ALERT_PULSE_CALM_FACTOR: u32 = 2;

/// How far a marked workspace tab's *background* is tinted toward its alert role at the peak of the
/// breathe. Tabs mark on background rather than foreground: a coloured glyph on the panel surface is
/// too quiet to catch peripheral vision in a one-row bar, while a fully saturated cell block is
/// alarm-grade. A partial tint reads as a filled, marked tab without shouting, and the trough is the
/// untinted panel surface, so the tab breathes between neutral and its role colour.
pub const ALERT_TAB_TINT: f32 = 0.72;
const SCRATCH_DURATION_NUMERATOR: u32 = 2;
const SCRATCH_DURATION_DENOMINATOR: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeometryAnimation {
    None,
    Spawn,
    Close,
    Fullscreen,
    TileFloat,
    AxisChange,
}

/// What shape a pane's open and close animation takes. Orthogonal to [`GeometryAnimation`], which
/// says *why* geometry is moving; this says how the arriving or leaving pane itself is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PaneAnimationStyle {
    /// The opening or closing pane appears and disappears at once. Neighbouring panes still
    /// animate their reflow. Not a drawn effect: there is no fade and no reveal.
    Off,
    /// Scale toward the centre of the pane's own rectangle, with a fade riding on top.
    #[default]
    Scale,
    /// Slide in from the edge the pane was split off, clipped to its tile, while the tile that gave
    /// up the space springs into its new size.
    ///
    /// Only tiled panes slide. A floating pane has no tile edge to emerge from and no neighbour to
    /// take space from, so it keeps [`Scale`](Self::Scale).
    Slide,
    /// Materialize the pane radially from its center with a sparse punctuation ring.
    Portal,
    /// Reveal the pane along a fixed, aspect-corrected diagonal.
    Scan,
}

/// How one pane draws itself arriving and leaving: an effect, its timing, its curves, and the
/// geometry parameters that shape it.
///
/// Deliberately compact and `Copy`: the view reads it on every frame of every pane, and a pane
/// snapshots it for the length of a transition. Nothing here is a map, a string, or an allocation.
///
/// The frontier width, ring density, and glyph palette the two paint effects use are *not* here.
/// They are how Portal and Scan are drawn rather than something configuration chooses, so they live
/// as constants next to the drawing code in [`crate::view::pane_reveal`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaneAnimationSpec {
    pub kind: PaneAnimationStyle,
    pub open_duration: Duration,
    pub close_duration: Duration,
    pub open_curve: Easing,
    pub close_curve: Easing,
    /// Curves for the opacity riding the effect. Kept separate because Scale's fade leads its
    /// scale, and Slide is opaque throughout.
    pub visual_open_curve: Easing,
    pub visual_close_curve: Easing,
    pub fade: bool,
    /// Scale only: the inset the pane grows from, as a fraction of its settled rectangle.
    pub scale_from: f32,
    /// Portal only: where the reveal starts, normalized within the pane.
    pub origin: [f32; 2],
    /// Scan only: the corner the reveal sweeps from.
    pub scan_direction: ScanDirection,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScanDirection {
    #[default]
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// The parts of the selected effect a config may override. Each one belongs to a particular style;
/// an override for a style that is not selected sits dormant rather than being an error, so a config
/// can carry settings for all four and switching `pane_style` picks up the matching ones.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PaneAnimationOverrides {
    pub curve: Option<Easing>,
    pub close_curve: Option<Easing>,
    pub fade: Option<bool>,
    pub scale_from: Option<f32>,
    pub portal_origin: Option<[f32; 2]>,
    pub scan_direction: Option<ScanDirection>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaneAnimationSnapshot {
    pub spec: PaneAnimationSpec,
    pub active: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneEventAnimationSnapshot {
    pub duration: Duration,
}

pub(crate) fn builtin_animation(style: PaneAnimationStyle) -> PaneAnimationSpec {
    let (open_curve, close_curve, visual_open_curve, visual_close_curve) = match style {
        PaneAnimationStyle::Off => (
            Easing::Linear,
            Easing::Linear,
            Easing::Linear,
            Easing::Linear,
        ),
        PaneAnimationStyle::Scale => (
            Easing::EaseInOutCubic,
            Easing::EaseOutQuad,
            Easing::EaseOutQuad,
            Easing::EaseOutQuad,
        ),
        PaneAnimationStyle::Slide => (
            Easing::EaseOutQuad,
            Easing::EaseOutQuad,
            Easing::Linear,
            Easing::Linear,
        ),
        PaneAnimationStyle::Portal | PaneAnimationStyle::Scan => (
            Easing::EaseOutQuad,
            Easing::EaseInQuad,
            Easing::EaseOutQuad,
            Easing::EaseInQuad,
        ),
    };
    PaneAnimationSpec {
        kind: style,
        open_duration: Duration::from_millis(GEOMETRY_MS),
        // Only Scale gets its own shorter exit: it is a pop the fade rides on, not motion the
        // surrounding tiles have to keep step with. Slide leaves toward its own edge while the tile
        // taking its place expands in that direction by the same distance, so a shared duration
        // makes the two edges one moving boundary and the pane reads as pushed out rather than
        // dragged. Portal and Scan repaint a fixed rectangle, which has nothing to desynchronize.
        close_duration: if style == PaneAnimationStyle::Scale {
            Duration::from_millis(CLOSE_MS)
        } else {
            Duration::from_millis(GEOMETRY_MS)
        },
        open_curve,
        close_curve,
        visual_open_curve,
        visual_close_curve,
        // Slide is clipped to its tile, so it genuinely emerges; a fade on top would make its
        // leading edge ghostly instead of solid. Off has no effect to fade.
        fade: matches!(
            style,
            PaneAnimationStyle::Scale | PaneAnimationStyle::Portal | PaneAnimationStyle::Scan
        ),
        scale_from: 0.9,
        origin: [0.5, 0.5],
        scan_direction: ScanDirection::TopLeft,
    }
}

pub(crate) fn snapshot_for_open(
    animations: WindowAnimationConfig,
    floating: bool,
) -> PaneAnimationSnapshot {
    let resolved = animations.resolved_animation(floating);
    PaneAnimationSnapshot {
        spec: resolved,
        active: animations.enabled && animations.spawn && resolved.kind != PaneAnimationStyle::Off,
    }
}

pub(crate) fn snapshot_for_close(
    animations: WindowAnimationConfig,
    floating: bool,
) -> PaneAnimationSnapshot {
    let resolved = animations.resolved_animation(floating);
    PaneAnimationSnapshot {
        spec: resolved,
        active: animations.enabled && animations.close && resolved.kind != PaneAnimationStyle::Off,
    }
}

/// How a newly shown session takes over the screen from the previous one.
///
/// Never a geometry animation: pane 2 in one session has no spatial relationship to pane 2 in
/// another, so the incoming session always snaps to its own layout and only its presentation moves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SessionAnimationStyle {
    /// The incoming session appears at once.
    Off,
    /// The incoming session resolves in place from slightly dimmed.
    #[default]
    Fade,
    /// A portal opens from the centre, revealing the incoming session over the outgoing one.
    Portal,
}

impl SessionAnimationStyle {
    /// Cycle order for the Settings row.
    pub fn all() -> &'static [Self] {
        &[Self::Off, Self::Fade, Self::Portal]
    }

    /// Config token and persisted value.
    pub fn id(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Fade => "fade",
            Self::Portal => "portal",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Fade => "Fade",
            Self::Portal => "Portal",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "fade" => Some(Self::Fade),
            "portal" => Some(Self::Portal),
            _ => None,
        }
    }
}

impl PaneAnimationStyle {
    /// Cycle order for the Settings row.
    pub fn all() -> &'static [Self] {
        &[
            Self::Off,
            Self::Scale,
            Self::Slide,
            Self::Portal,
            Self::Scan,
        ]
    }

    /// Config token and persisted value.
    pub fn id(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Scale => "scale",
            Self::Slide => "slide",
            Self::Portal => "portal",
            Self::Scan => "scan",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Scale => "Scale",
            Self::Slide => "Slide",
            Self::Portal => "Portal",
            Self::Scan => "Scan",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "scale" => Some(Self::Scale),
            "slide" => Some(Self::Slide),
            "portal" => Some(Self::Portal),
            "scan" => Some(Self::Scan),
            _ => None,
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::Scale,
            Self::Scale => Self::Slide,
            Self::Slide => Self::Portal,
            Self::Portal => Self::Scan,
            Self::Scan => Self::Off,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Off => Self::Scan,
            Self::Scale => Self::Off,
            Self::Slide => Self::Scale,
            Self::Portal => Self::Slide,
            Self::Scan => Self::Portal,
        }
    }
}

impl PaneAnimationSpec {
    pub(crate) fn transition(self, closing: bool) -> TransitionConfig {
        TransitionConfig {
            duration: if closing {
                self.close_duration
            } else {
                self.open_duration
            },
            easing: if closing {
                self.close_curve
            } else {
                self.open_curve
            },
        }
    }

    pub(crate) fn visual_transition(self, closing: bool) -> TransitionConfig {
        TransitionConfig {
            duration: if closing {
                self.close_duration
            } else {
                self.open_duration
            },
            easing: if closing {
                self.visual_close_curve
            } else {
                self.visual_open_curve
            },
        }
    }
}

/// Whether a pane style paints a full-size reveal instead of changing pane geometry.
pub fn pane_reveal_effects(animations: WindowAnimationConfig) -> bool {
    matches!(
        animations.selected_animation().kind,
        PaneAnimationStyle::Portal | PaneAnimationStyle::Scan
    )
}

pub fn pane_animation_for_pane(
    animations: WindowAnimationConfig,
    pane: &crate::state::Pane,
) -> PaneAnimationSpec {
    // A pane that started its transition before a config reload keeps the recipe it started with;
    // one without a snapshot (a fixture, or a pane that predates the reload) reads the selection.
    let snapshot = if pane.closing {
        pane.closing_animation
    } else {
        pane.opening_animation
    };
    match snapshot {
        // Resolved against the pane's floating state at the moment the transition began, which is
        // the state the effect it is drawing was chosen for.
        Some(snapshot) => snapshot.spec,
        None => animations.resolved_animation(pane.floating),
    }
}

/// Whether a pane is anywhere inside its open transition, from the spawn until the terminal goes
/// live. This is the question the *mounting* asks: keep the clip, the effect scope, and the pane's
/// own transition config alive for as long as it is true.
///
/// It is not the question the animation *target* asks. A pane parks at its starting value while
/// `Pane::opening` is set and travels once the spawn timer clears it - and that happens partway
/// through this window, not at the end of it. Anything choosing a target reads `pane.opening`.
pub fn pane_opening_transition(pane: &crate::state::Pane) -> bool {
    pane.opening || pane.opening_animation.is_some()
}

pub fn pane_reveal_effects_for_pane(
    animations: WindowAnimationConfig,
    pane: &crate::state::Pane,
) -> bool {
    matches!(
        pane_animation_for_pane(animations, pane).kind,
        PaneAnimationStyle::Portal | PaneAnimationStyle::Scan
    )
}

/// The tile edge a sliding pane enters from and leaves toward.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SlideEdge {
    Left,
    Right,
    Top,
    /// Also the fallback for panes that never went through a split - the first pane in a workspace,
    /// a restored layout, a follower reconciling a shared layout - matching the scratchpad, which
    /// deploys upward from the bottom of the screen.
    #[default]
    Bottom,
}

/// Whether this pane's open and close animation is the clipped slide rather than the centre scale.
///
/// A floating pane never slides: it has no tile edge to emerge from and no neighbour to take space
/// from, so it keeps the scale whatever the style says.
///
/// Deliberately independent of whether animation is *enabled*. The wrapper that clips a sliding pane
/// stays mounted for the pane's whole life, so a pane is never remounted - and its terminal never
/// re-laid out - at the moment it finishes arriving. With animation off the slide simply snaps
/// straight to deployed.
pub fn pane_slides(animations: WindowAnimationConfig, pane: &crate::state::Pane) -> bool {
    pane_animation_for_pane(animations, pane).kind == PaneAnimationStyle::Slide && !pane.floating
}

/// Whether an open or close effect is timed, as opposed to snapping.
///
/// A snapshot taken while the master switch (or the spawn/close sub-flag) was off stays inactive
/// for the rest of that transition. A pane that never took one — a launcher seed, a fixture —
/// follows the live config, so disabling Animations cannot leave it interpolating on the default
/// duration.
pub fn lifecycle_motion_enabled(
    animations: WindowAnimationConfig,
    pane: &crate::state::Pane,
) -> bool {
    if pane.closing {
        pane.closing_animation
            .map(|snapshot| snapshot.active)
            .unwrap_or(
                animations.enabled
                    && animations.close
                    && animations.pane_style != PaneAnimationStyle::Off,
            )
    } else {
        pane.opening_animation
            .map(|snapshot| snapshot.active)
            .unwrap_or(
                animations.enabled
                    && animations.spawn
                    && animations.pane_style != PaneAnimationStyle::Off,
            )
    }
}

/// Whether a pane should fade during its current open or close transition.
pub fn pane_opacity_animates(animations: WindowAnimationConfig, pane: &crate::state::Pane) -> bool {
    let spec = pane_animation_for_pane(animations, pane);
    spec.fade
        && !pane_slides(animations, pane)
        && spec.kind != PaneAnimationStyle::Off
        && spec.kind != PaneAnimationStyle::Slide
        && lifecycle_motion_enabled(animations, pane)
        && (pane_opening_transition(pane) || pane.closing)
}

/// Visibility target for a pane's open/close opacity animation.
///
/// Animation gates choose whether the transition is timed, not whether a retained closing pane is
/// visible. A pane that is opening or closing must stay at the hidden target until its lifecycle
/// state settles; otherwise disabling close animation can make it reappear before pruning.
///
/// [`PaneAnimationStyle::Off`] draws no effect. An opening pane is fully visible on the first
/// frame, and a retained closing pane (the last scratch pane, held while the dropdown retracts)
/// is fully hidden on the first frame. Slide also has `fade == false`, and it stays opaque
/// because a clip reveals it.
///
/// `pane.opening`, not [`pane_opening_transition`]: the fade has to *travel* once the spawn timer
/// clears that flag, and the snapshot outlives it by design.
pub fn pane_opacity_target(animations: WindowAnimationConfig, pane: &crate::state::Pane) -> f32 {
    let spec = pane_animation_for_pane(animations, pane);
    if spec.kind == PaneAnimationStyle::Off {
        return if pane.closing { 0.0 } else { 1.0 };
    }
    if !spec.fade || pane_slides(animations, pane) || (!pane.opening && !pane.closing) {
        1.0
    } else {
        0.0
    }
}

/// Rigid offset for a pane part-way through its slide, in canvas cells.
///
/// `progress` is `0.0` for fully outside its tile and `1.0` for fully deployed. The pane travels
/// exactly its own extent along the slide axis, so at `0.0` it sits flush outside the edge it
/// entered from and the clip to its tile leaves nothing of it on screen. Applied *after* the pane's
/// own geometry transition, like the scratchpad slide: the pane keeps its final size the whole way,
/// which is what stops the terminal grid reflowing on every frame of the animation.
pub fn slide_offset(rect: FloatRect, edge: SlideEdge, progress: f32) -> (f32, f32) {
    let remaining = 1.0 - progress.clamp(0.0, 1.0);
    match edge {
        SlideEdge::Left => (-rect.w * remaining, 0.0),
        SlideEdge::Right => (rect.w * remaining, 0.0),
        SlideEdge::Top => (0.0, -rect.h * remaining),
        SlideEdge::Bottom => (0.0, rect.h * remaining),
    }
}

/// Where the sidebar panel sits inside its clip window while it is part-way in, in canvas columns.
///
/// `window_width` is how much of the panel's `deployed_width` the layout has handed over so far -
/// the animated quantity. The panel is laid out at its full deployed width whatever that is, and
/// anchored to its dock edge inside the window, so what shows is the part of it nearest the screen
/// edge and the rest waits off-screen. Laying it out at `window_width` instead would re-wrap its
/// tabs and rows on every frame of the slide.
///
/// Only the panel needs carrying. The pane column beside it is genuinely resized to whatever the
/// sidebar has not reserved, which is what keeps both of its edges where they belong - the near one
/// travelling with the panel, the far one pinned to the far edge of the screen.
pub fn sidebar_slide_offset(window_width: u16, deployed_width: u16, docked_right: bool) -> f32 {
    if docked_right {
        // Anchored by its left edge, which is the one the pane column meets: the overhang runs off
        // the far side of the window on its own.
        0.0
    } else {
        // Anchored by its right edge, so the overhang runs off the near side.
        f32::from(window_width) - f32::from(deployed_width)
    }
}

/// How much wider a terminal cell is than it is tall, near enough. Lets the two axes of a tile be
/// compared: 10 rows covers about as much screen as 20 columns.
const CELL_ASPECT: f32 = 2.0;

/// A single characteristic extent for a tile, in column units, averaging its two axes.
///
/// The spring amplitude needs a rough size for the tile, not an exact travel distance: which axis a
/// tile is resizing along is not knowable from the tile alone, and being out by a factor of under two
/// only moves the nudge inside the range that looks right anyway.
pub fn spring_extent(rect: FloatRect) -> f32 {
    (rect.w + rect.h * CELL_ASPECT) / 2.0
}

/// Peak spring overshoot for a tile making room, in thousandths of the distance it travels.
///
/// The framework curve's overshoot is a fraction of the distance travelled, so a *fixed* amplitude
/// throws a big tile proportionally further - a pane halving from 240 columns overshot 24 of them, and
/// whipped through that throw in the same tail of the animation a small tile uses for three. Sizing
/// the request by the tile keeps the nudge at `SPRING_OVERSHOOT_CELLS` whatever the tile's size, which
/// is what makes it read as a settle rather than a throw.
fn spring_overshoot_permille(extent: f32) -> u16 {
    /// Three columns: what the unscaled curve happened to produce on a ~30x20 tile, which is the size
    /// the spring was tuned by eye against.
    const SPRING_OVERSHOOT_CELLS: f32 = 3.0;
    /// Never exceed the standard curve, however tiny the tile.
    const MAX_PERMILLE: f32 = 100.0;

    if extent <= 1.0 {
        return 0;
    }
    let permille = 1000.0 * SPRING_OVERSHOOT_CELLS / extent;
    permille.clamp(0.0, MAX_PERMILLE).round() as u16
}

#[derive(Clone, Copy, Debug)]
pub struct WindowAnimationConfig {
    pub enabled: bool,
    pub spawn: bool,
    pub close: bool,
    pub fullscreen: bool,
    pub tile_float: bool,
    pub axis_change: bool,
    pub sidebar: bool,
    pub workspace: bool,
    pub workspace_duration: Duration,
    pub session: SessionAnimationStyle,
    pub focus_chrome: bool,
    pub pane_style: PaneAnimationStyle,
    pub pane_overrides: PaneAnimationOverrides,
    pub geometry_duration: Duration,
    pub close_duration: Duration,
    pub focus_chrome_duration: Duration,
    pub alert_pulse_duration: Duration,
    pub open_delay: Duration,
}

impl Default for WindowAnimationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            spawn: true,
            close: true,
            fullscreen: true,
            tile_float: true,
            axis_change: true,
            sidebar: true,
            workspace: true,
            workspace_duration: Duration::from_millis(GEOMETRY_MS),
            session: SessionAnimationStyle::Fade,
            focus_chrome: true,
            pane_style: PaneAnimationStyle::Scale,
            pane_overrides: PaneAnimationOverrides::default(),
            geometry_duration: Duration::from_millis(GEOMETRY_MS),
            close_duration: Duration::from_millis(CLOSE_MS),
            focus_chrome_duration: Duration::from_millis(FOCUS_CHROME_MS),
            alert_pulse_duration: Duration::from_millis(ALERT_PULSE_MS),
            open_delay: Duration::from_millis(OPEN_DELAY_MS),
        }
    }
}

impl WindowAnimationConfig {
    pub(crate) fn selected_animation(self) -> PaneAnimationSpec {
        self.resolved_animation(false)
    }

    /// The selected effect as one particular pane will actually draw it: the builtin for
    /// `pane_style`, on the configured durations, with any override that belongs to that style.
    ///
    /// A floating pane has no tile edge to emerge from and no neighbour to take space from, so
    /// Slide is not something it can perform - it resolves to Scale before anything else is
    /// applied, and therefore picks up Scale's timing and Scale's overrides. Off stays Off:
    /// disappearing at once does not need a tile edge.
    pub(crate) fn resolved_animation(self, floating: bool) -> PaneAnimationSpec {
        let kind = if floating && self.pane_style == PaneAnimationStyle::Slide {
            PaneAnimationStyle::Scale
        } else {
            self.pane_style
        };
        let mut spec = builtin_animation(kind);
        spec.open_duration = self.geometry_duration;
        spec.close_duration = if kind == PaneAnimationStyle::Scale {
            self.close_duration
        } else {
            self.geometry_duration
        };
        self.pane_overrides.apply(&mut spec);
        spec
    }
}

impl PaneAnimationOverrides {
    /// Layer the configured overrides onto a builtin spec.
    ///
    /// Each geometry override is read only by the style it belongs to, so a config can carry all of
    /// them at once and switching `pane_style` picks up the matching one. Nothing here reports an
    /// error for an override the selected style ignores - `scan_direction` sitting unused under
    /// `pane_style = "portal"` is a config someone can switch between, not a mistake.
    fn apply(self, spec: &mut PaneAnimationSpec) {
        if let Some(curve) = self.curve {
            spec.open_curve = curve;
            spec.visual_open_curve = curve;
            // Without an explicit closing curve, the open curve runs backwards on the way out.
            let close = self.close_curve.unwrap_or_else(|| reverse_curve(curve));
            spec.close_curve = close;
            spec.visual_close_curve = close;
        } else if let Some(close) = self.close_curve {
            spec.close_curve = close;
            spec.visual_close_curve = close;
        }
        if let Some(fade) = self.fade
            && spec.kind != PaneAnimationStyle::Off
        {
            spec.fade = fade;
        }
        match spec.kind {
            PaneAnimationStyle::Scale => {
                if let Some(scale_from) = self.scale_from {
                    spec.scale_from = scale_from;
                }
            }
            PaneAnimationStyle::Portal => {
                if let Some(origin) = self.portal_origin {
                    spec.origin = origin;
                }
            }
            PaneAnimationStyle::Scan => {
                if let Some(direction) = self.scan_direction {
                    spec.scan_direction = direction;
                }
            }
            // A slide enters from the edge the split placed it on; there is nothing to aim.
            // Off draws neither an effect nor a fade, so a dormant override cannot turn one on.
            PaneAnimationStyle::Slide | PaneAnimationStyle::Off => {}
        }
    }
}

/// The temporal complement of a curve, for a close that was given no curve of its own.
///
/// A custom Bézier reverses mathematically. The builtin easings use their in/out partner where they
/// have one, and the symmetric ones are their own reverse.
fn reverse_curve(curve: Easing) -> Easing {
    match curve {
        Easing::CubicBezier(curve) => Easing::CubicBezier(curve.reversed()),
        Easing::EaseInQuad => Easing::EaseOutQuad,
        Easing::EaseOutQuad => Easing::EaseInQuad,
        other => other,
    }
}

/// Half the configured breathe period, floored to prevent alert colors becoming a strobe.
pub fn alert_pulse_half_period(animations: WindowAnimationConfig) -> Duration {
    (animations.alert_pulse_duration / 2).max(Duration::from_millis(ALERT_PULSE_MIN_HALF_MS))
}

/// Half period for calm alerts. Derived from the urgent half period rather than configured
/// separately, so the two stay an exact multiple apart however `alert_pulse_ms` is set - the tick
/// chain runs at the urgent rate and calm alerts simply flip on every `ALERT_PULSE_CALM_FACTOR`th
/// beat.
pub fn alert_pulse_calm_half_period(animations: WindowAnimationConfig) -> Duration {
    alert_pulse_half_period(animations) * ALERT_PULSE_CALM_FACTOR
}

pub fn geometry_transition(duration: Duration) -> TransitionConfig {
    TransitionConfig {
        duration,
        easing: Easing::EaseInOutCubic,
    }
}

/// Geometry for a pane that is closing. `EaseInOutCubic` ramps in slowly, which is right for a
/// pane settling into a new tile but wrong here: the fade riding on top of the scale is
/// `EaseOutQuad`, so a slow-starting scale is still near full size when the pane has already gone
/// transparent, and the shrink is never actually seen. Match the fade instead.
pub fn close_geometry_transition(duration: Duration) -> TransitionConfig {
    TransitionConfig {
        duration,
        easing: Easing::EaseOutQuad,
    }
}

/// Slide progress for an opening or closing pane.
///
/// Ease-out with no overshoot, deliberately. A sliding pane is clipped to its destination tile, so
/// carrying it past its resting place would not read as a bounce - it would open a gap at the edge
/// it entered from, for as long as the overshoot lasted. The spring belongs on the tile making room
/// instead; see [`spring_geometry_transition`].
pub fn slide_transition(duration: Duration) -> TransitionConfig {
    TransitionConfig {
        duration,
        easing: Easing::EaseOutQuad,
    }
}

/// Geometry for a tile making room for an arriving pane, or closing the gap a leaving one left.
///
/// `EaseOutBack` overshoots once and settles, so the tile that gave up the space springs into its new
/// size rather than gliding into it. The amplitude is sized from `distance` - the extent the tile
/// itself covers - so the nudge stays a couple of cells whether the tile is 30 columns or 200; see
/// [`spring_overshoot_permille`].
///
/// Reserved for the panes *around* the one animating, and only under [`PaneAnimationStyle::Slide`].
pub fn spring_geometry_transition(duration: Duration, distance: f32) -> TransitionConfig {
    TransitionConfig {
        duration,
        easing: Easing::EaseOutBack {
            overshoot_permille: spring_overshoot_permille(distance),
        },
    }
}

/// Curve for the sidebar sliding in and out.
///
/// Shares the scratchpad's shortened duration rather than the full geometry one: both are a surface
/// deploying over the workspace rather than tiles rearranging, and a drawer that takes as long as a
/// tiling reflow feels slow. Keeping them on one duration also keeps the two in step when
/// `geometry_ms` is retuned.
pub fn sidebar_transition(animations: WindowAnimationConfig) -> TransitionConfig {
    if !animations.enabled || !animations.sidebar {
        return instant_transition();
    }
    slide_transition(scratch_transition_duration(animations.geometry_duration))
}

/// Opacity the incoming session's content starts from when it replaces another session.
///
/// Deliberately high: the reveal is a delimiter between two unrelated screens, felt more than
/// watched. The attachment itself has already swapped, so there is nothing to crossfade from.
pub const SESSION_REVEAL_FROM: f32 = 0.8;

/// Curve for the incoming session's reveal, or `None` when the switch should snap.
///
/// Only presentation moves, never geometry: see [`SessionAnimationStyle`]. Both styles are tied
/// to `geometry_ms`, so retuning it keeps the whole motion vocabulary in proportion.
///
/// The fade must read as its own beat. A switch made from the Sessions picker starts it on the
/// same frame the picker's backdrop begins to undim, over the scratchpad's two-thirds of
/// `geometry_ms`; a fade on that duration and an ease-out curve finished alongside it and was
/// indistinguishable from it, so switching looked animated even with the fade off. It takes one
/// and a half times `geometry_ms` instead, on an ease-in-out curve: when the backdrop has settled
/// the incoming session is still visibly resolving, and it finishes on its own.
///
/// The portal has a whole screen to cross, so it takes the full geometry duration on the pane
/// Portal's own curve.
pub fn session_reveal_transition(animations: WindowAnimationConfig) -> Option<TransitionConfig> {
    let config = match animations.session {
        SessionAnimationStyle::Off => return None,
        SessionAnimationStyle::Fade => TransitionConfig {
            duration: animations.geometry_duration * 3 / 2,
            easing: Easing::EaseInOutCubic,
        },
        SessionAnimationStyle::Portal => TransitionConfig {
            duration: animations.geometry_duration,
            easing: builtin_animation(PaneAnimationStyle::Portal).open_curve,
        },
    };
    (animations.enabled && !config.duration.is_zero()).then_some(config)
}

/// The portal is open when [`session_reveal_transition`] runs for a portal.
pub fn session_portal_enabled(animations: WindowAnimationConfig) -> bool {
    animations.session == SessionAnimationStyle::Portal
        && session_reveal_transition(animations).is_some()
}

/// Opacity the outgoing session gives way to beneath an opening portal: it recedes rather than
/// vanishing, so the portal reads as opening onto somewhere new instead of out of nothing.
pub const SESSION_PORTAL_RECEDE: f32 = 0.4;

/// How long a screenshot flash takes to ease back from its peak tint.
pub const SCREENSHOT_FLASH: Duration = Duration::from_millis(220);
/// How far a screenshot flash tints what it photographed toward the accent at its peak.
pub const SCREENSHOT_FLASH_PEAK: f32 = 0.35;

/// Whether a screenshot action flashes what it photographed. The flash is chrome feedback, so it
/// follows the same switches as the focus chrome fades; the toast is shown either way.
pub fn screenshot_flash_enabled(animations: WindowAnimationConfig) -> bool {
    animations.enabled && animations.focus_chrome
}

pub fn screenshot_flash_transition() -> TransitionConfig {
    TransitionConfig {
        duration: SCREENSHOT_FLASH,
        easing: Easing::EaseOutQuad,
    }
}

pub fn instant_transition() -> TransitionConfig {
    TransitionConfig {
        duration: Duration::ZERO,
        easing: Easing::Linear,
    }
}

fn pane_open_waits(animations: WindowAnimationConfig) -> bool {
    animations.enabled && animations.spawn && animations.pane_style != PaneAnimationStyle::Off
}

pub fn open_delay(animations: WindowAnimationConfig) -> Duration {
    if pane_open_waits(animations) {
        animations.open_delay
    } else {
        Duration::ZERO
    }
}

pub fn activation_delay(animations: WindowAnimationConfig) -> Duration {
    if pane_open_waits(animations) {
        animations.open_delay + animations.selected_animation().open_duration
    } else {
        Duration::ZERO
    }
}

/// How long a closing pane stays described before `Msg::PruneClosed` drops it. The margin
/// covers the frame the animation finishes on.
pub fn retained_pane_timeout(animations: WindowAnimationConfig) -> Duration {
    if !animations.enabled || !animations.close {
        return Duration::ZERO;
    }
    let spec = animations.selected_animation();
    let motion = match spec.kind {
        // The pane is already gone. Neighbours keep `geometry_duration` from the event snapshot.
        PaneAnimationStyle::Off => return Duration::ZERO,
        // A short pop the fade rides on.
        PaneAnimationStyle::Scale => spec.close_duration,
        // A whole tile to cross, which `close_ms` is far too short for - it would prune the pane
        // part-way out.
        PaneAnimationStyle::Slide => spec.close_duration,
        // Both paint effects run on the same geometry duration in either direction.
        PaneAnimationStyle::Portal | PaneAnimationStyle::Scan => spec.close_duration,
    };
    motion + Duration::from_millis(20)
}

pub fn retained_pane_timeout_for_pane(
    animations: WindowAnimationConfig,
    pane: &crate::state::Pane,
) -> Duration {
    if let Some(snapshot) = pane.closing_animation {
        return if snapshot.active {
            snapshot.spec.close_duration + Duration::from_millis(20)
        } else {
            Duration::ZERO
        };
    }
    retained_pane_timeout(animations)
}

pub fn scratch_transition_duration(geometry_duration: Duration) -> Duration {
    (geometry_duration / SCRATCH_DURATION_DENOMINATOR) * SCRATCH_DURATION_NUMERATOR
}

/// Whether this pane's rectangle may animate to its new position.
///
/// Followers animate too. A layout revision is an authoritative *destination*, not a path, so
/// the transition between the geometry a follower holds and the geometry that arrives is a
/// local presentation choice - the same one the controller makes, from the same
/// [`GeometryAnimation`] the reconciler arms in `apply_shared_layout`.
///
/// A pane under continuous manipulation is the exception, whoever is manipulating it. Its
/// rectangle is being reported, not derived, so easing toward each reported position would
/// leave it trailing the pointer by one relay for the whole gesture. The tiles *around* it
/// still animate: they move once when the pane is lifted and once when it lands, which is an
/// ordinary discrete transition on both the controller and every follower.
pub(crate) fn geometry_animation_enabled(
    state: &State,
    pane: &Pane,
    viewport_changed: bool,
) -> bool {
    if viewport_changed
        || state
            .moving_pane
            .is_some_and(|session| session.id == pane.id)
        || state
            .current()
            .remote_drag
            .is_some_and(|drag| drag.pane_id == pane.id)
        || state.current().remote_drag_snap.get() == Some(pane.id)
        || state
            .resizing_pane
            .as_ref()
            .is_some_and(|session| session.id == pane.id)
    {
        return false;
    }
    let animations = state.config.animations;
    let opening = pane_opening_transition(pane);
    if !animations.enabled && !opening && !pane.closing {
        return false;
    }
    if opening || pane.closing {
        return lifecycle_motion_enabled(animations, pane);
    }
    match state.animation {
        GeometryAnimation::None => false,
        GeometryAnimation::Spawn => animations.spawn,
        GeometryAnimation::Close => animations.close,
        GeometryAnimation::Fullscreen => animations.fullscreen,
        GeometryAnimation::TileFloat => animations.tile_float,
        GeometryAnimation::AxisChange => animations.axis_change,
    }
}

/// Geometry transition policy for a pane. Extracted so tests can assert Scrollable resize
/// instant-vs-AxisChange behavior without constructing a live [`tui_lipan::prelude::Context`].
///
/// `target_rect` sizes the Slide spring's amplitude and is only known to the view; without it the
/// spring degrades to the plain geometry curve rather than guessing an amplitude.
pub(crate) fn geometry_transition_for_pane(
    state: &State,
    pane: &Pane,
    viewport_changed: bool,
    target_rect: Option<FloatRect>,
) -> TransitionConfig {
    if !geometry_animation_enabled(state, pane, viewport_changed) {
        return instant_transition();
    }

    let animations = state.config.animations;
    let spec = pane_animation_for_pane(animations, pane);
    // Only read below under Spawn and Close, which is what armed it.
    let event_duration = state.pane_event_animation.map(|snapshot| snapshot.duration);
    // An arriving or leaving pane that slides or uses a paint effect does not animate its
    // rectangle. Slide carries it in, while Portal and Scan repaint its cells, so all three
    // keep their final size the whole way.
    let pane_transition = pane_opening_transition(pane) || pane.closing;
    if pane_transition {
        if matches!(
            spec.kind,
            PaneAnimationStyle::Off
                | PaneAnimationStyle::Slide
                | PaneAnimationStyle::Portal
                | PaneAnimationStyle::Scan
        ) {
            return instant_transition();
        }
        return spec.transition(pane.closing);
    }

    // Every tile moving to make room for - or take back the space of - the pane in transition
    // shares that pane's clock, so their common edges stay one moving boundary. Only the two
    // lifecycle events borrow it: fullscreen, tile/float, and axis changes are not a pane
    // arriving or leaving, so a recipe's `open_ms` must not become the duration of every
    // reflow in the app.
    let neighbour_duration = match state.animation {
        GeometryAnimation::Close => event_duration.unwrap_or(spec.close_duration),
        GeometryAnimation::Spawn => event_duration.unwrap_or(spec.open_duration),
        GeometryAnimation::None
        | GeometryAnimation::Fullscreen
        | GeometryAnimation::TileFloat
        | GeometryAnimation::AxisChange => animations.geometry_duration,
    };
    // Under Slide, the tiles *around* an arriving or leaving pane are where the spring lives:
    // this is the tile that gave up the space, or the one taking it back.
    if spec.kind == PaneAnimationStyle::Slide
        && matches!(
            state.animation,
            GeometryAnimation::Spawn | GeometryAnimation::Close
        )
        && let Some(rect) = target_rect
    {
        return spring_geometry_transition(neighbour_duration, spring_extent(rect));
    }
    geometry_transition(neighbour_duration)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Pane;

    /// A switch from the Sessions picker starts the fade on the frame the picker's backdrop starts
    /// to undim. On the backdrop's own duration the two read as one motion, so switching looked
    /// animated even with the fade off; the fade has to outlast it clearly, however `geometry_ms`
    /// is tuned.
    #[test]
    fn the_session_fade_outlasts_the_picker_backdrop_it_starts_with() {
        for geometry_ms in [220, 90, 600] {
            let animations = WindowAnimationConfig {
                geometry_duration: Duration::from_millis(geometry_ms),
                ..WindowAnimationConfig::default()
            };
            let fade = session_reveal_transition(animations).expect("the fade is on by default");
            let backdrop = scratch_transition_duration(animations.geometry_duration);
            assert!(
                fade.duration >= backdrop * 2,
                "geometry_ms {geometry_ms}: fade {:?} vs backdrop {backdrop:?}",
                fade.duration
            );
        }
    }

    #[test]
    fn scratch_transition_duration_is_two_thirds_of_geometry_duration() {
        assert_eq!(
            scratch_transition_duration(Duration::from_millis(300)),
            Duration::from_millis(200)
        );
    }

    #[test]
    fn retained_pane_timeout_covers_the_close_animation() {
        let animations = WindowAnimationConfig {
            close_duration: Duration::from_millis(80),
            // A longer survivor-expansion duration must not extend how long the closed pane is
            // kept: survivors animate independently of its lifetime.
            geometry_duration: Duration::from_millis(240),
            ..WindowAnimationConfig::default()
        };
        assert_eq!(
            retained_pane_timeout(animations),
            Duration::from_millis(100)
        );
        assert_eq!(
            retained_pane_timeout(WindowAnimationConfig {
                close: false,
                ..animations
            }),
            Duration::ZERO
        );
    }

    #[test]
    fn a_slide_leaves_in_step_with_the_tile_that_displaces_it() {
        let scale = WindowAnimationConfig {
            close_duration: Duration::from_millis(120),
            geometry_duration: Duration::from_millis(200),
            ..WindowAnimationConfig::default()
        };
        assert_eq!(
            retained_pane_timeout(scale),
            Duration::from_millis(140),
            "the scale close is a short pop the fade rides on"
        );

        let slide = WindowAnimationConfig {
            pane_style: PaneAnimationStyle::Slide,
            ..scale
        };
        // The closing pane and the tile expanding into its place both run at `geometry_duration`, so
        // their shared edge is one moving boundary and the pane reads as pushed out rather than
        // dragged behind. A departing pane on its own clock is what broke that.
        let leaving = slide.selected_animation().close_duration;
        assert_eq!(leaving, slide.geometry_duration);
        let state = spawning_state(PaneAnimationStyle::Slide, GeometryAnimation::Close);
        let tile_making_room = geometry_transition_for_pane(
            &state,
            // The settled survivor, not the pane on its way out.
            &state.current().workspaces[0].panes[0],
            false,
            Some(FloatRect {
                x: 0.0,
                y: 0.0,
                w: 30.0,
                h: 20.0,
            }),
        );
        assert_eq!(
            tile_making_room.duration, leaving,
            "the pusher and the pushed have to share a duration"
        );

        assert!(
            retained_pane_timeout(slide) > leaving,
            "the pane must stay described past the end of its slide"
        );
        assert_eq!(retained_pane_timeout(slide), Duration::from_millis(220));
        assert_eq!(
            retained_pane_timeout(WindowAnimationConfig {
                close: false,
                ..slide
            }),
            Duration::ZERO
        );
    }

    #[test]
    fn off_opens_live_with_no_delay_and_draws_no_effect() {
        let animations = WindowAnimationConfig {
            pane_style: PaneAnimationStyle::Off,
            open_delay: Duration::from_millis(36),
            geometry_duration: Duration::from_millis(220),
            ..WindowAnimationConfig::default()
        };
        assert_eq!(open_delay(animations), Duration::ZERO);
        assert_eq!(activation_delay(animations), Duration::ZERO);
        assert_eq!(
            PaneAnimationStyle::parse("off"),
            Some(PaneAnimationStyle::Off)
        );
        assert_eq!(
            PaneAnimationStyle::parse("  OFF "),
            Some(PaneAnimationStyle::Off)
        );
        assert_eq!(
            animations.resolved_animation(true).kind,
            PaneAnimationStyle::Off
        );
        assert_eq!(
            WindowAnimationConfig {
                pane_style: PaneAnimationStyle::Slide,
                ..animations
            }
            .resolved_animation(true)
            .kind,
            PaneAnimationStyle::Scale
        );

        let mut pane = Pane::new(1, 100, FloatRect::default());
        pane.opening = true;
        pane.floating = true;
        pane.begin_open_animation(animations);
        let snapshot = pane.opening_animation.expect("open snapshot");
        assert!(!snapshot.active);
        assert_eq!(snapshot.spec.kind, PaneAnimationStyle::Off);
        assert!(!snapshot.spec.fade);
        assert!(!pane_opacity_animates(animations, &pane));
        assert!(!pane_reveal_effects_for_pane(animations, &pane));
        assert!(!pane_slides(animations, &pane));
        assert_eq!(pane_opacity_target(animations, &pane), 1.0);
        pane.opening = false;
        pane.closing = true;
        pane.opening_animation = None;
        pane.begin_close_animation(animations);
        assert!(!pane_opacity_animates(animations, &pane));
        assert_eq!(pane_opacity_target(animations, &pane), 0.0);

        let mut state = spawning_state(PaneAnimationStyle::Off, GeometryAnimation::Spawn);
        state.config.animations.open_delay = Duration::from_millis(36);
        state.begin_pane_event(GeometryAnimation::Spawn);
        let animations = state.config.animations;
        {
            let opening = &mut state.current_mut().workspaces[0].panes[0];
            opening.opening = true;
            opening.begin_open_animation(animations);
        }
        let opening = &state.current().workspaces[0].panes[0];
        assert_eq!(
            geometry_transition_for_pane(&state, opening, false, None).duration,
            Duration::ZERO
        );
    }

    #[test]
    fn off_closes_with_no_retention_while_neighbours_keep_geometry_duration() {
        let mut state = spawning_state(PaneAnimationStyle::Off, GeometryAnimation::Close);
        state.config.animations.geometry_duration = Duration::from_millis(275);
        state.config.animations.close_duration = Duration::from_millis(80);
        state.begin_pane_event(GeometryAnimation::Close);
        let animations = state.config.animations;

        {
            let closing = &mut state.current_mut().workspaces[0].panes[0];
            closing.opening = false;
            closing.closing = true;
            closing.begin_close_animation(animations);
            assert!(!closing.closing_animation.expect("close snapshot").active);
            assert!(!pane_opacity_animates(animations, closing));
            assert_eq!(pane_opacity_target(animations, closing), 0.0);
            assert_eq!(
                retained_pane_timeout_for_pane(animations, closing),
                Duration::ZERO
            );
        }
        assert_eq!(retained_pane_timeout(animations), Duration::ZERO);

        let mut neighbour = Pane::new(2, 100, FloatRect::default());
        neighbour.opening = false;
        state.current_mut().workspaces[0].panes.push(neighbour);
        let neighbour = &state.current().workspaces[0].panes[1];
        assert_eq!(
            geometry_transition_for_pane(
                &state,
                neighbour,
                false,
                Some(FloatRect {
                    x: 0.0,
                    y: 0.0,
                    w: 40.0,
                    h: 12.0,
                }),
            )
            .duration,
            Duration::from_millis(275)
        );
    }

    #[test]
    fn builtin_scale_close_neighbors_keep_the_geometry_duration() {
        let mut state = spawning_state(PaneAnimationStyle::Scale, GeometryAnimation::Close);
        state.config.animations.geometry_duration = Duration::from_millis(300);
        state.config.animations.close_duration = Duration::from_millis(80);
        state.begin_pane_event(GeometryAnimation::Close);
        let pane = &state.current().workspaces[0].panes[0];
        let transition = geometry_transition_for_pane(
            &state,
            pane,
            false,
            Some(FloatRect {
                x: 0.0,
                y: 0.0,
                w: 30.0,
                h: 20.0,
            }),
        );
        assert_eq!(transition.duration, Duration::from_millis(300));
    }

    /// The tiles moving around a spawning pane are timed when the event is armed, so a config
    /// reload part-way through cannot retime motion that is already running.
    #[test]
    fn the_neighbour_clock_is_captured_when_the_event_is_armed() {
        let mut state = spawning_state(PaneAnimationStyle::Scale, GeometryAnimation::Spawn);
        state.config.animations.geometry_duration = Duration::from_millis(480);
        state.begin_pane_event(GeometryAnimation::Spawn);

        // A reload lands mid-spawn and shortens everything.
        state.config.animations.geometry_duration = Duration::from_millis(120);
        assert_eq!(
            geometry_transition_for_pane(
                &state,
                &state.current().workspaces[0].panes[0],
                false,
                Some(FloatRect {
                    x: 0.0,
                    y: 0.0,
                    w: 30.0,
                    h: 20.0,
                }),
            )
            .duration,
            Duration::from_millis(480),
            "the tiles in flight keep the clock the spawn started on"
        );
    }

    /// The pane lifecycle owns `close_ms`; nothing else does. Fullscreen, tile/float, and axis
    /// changes are reflows the lifecycle has no part in, so `geometry_ms` stays theirs.
    #[test]
    fn pane_close_timing_does_not_retime_reflows_that_are_not_lifecycle_events() {
        for animation in [
            GeometryAnimation::Fullscreen,
            GeometryAnimation::TileFloat,
            GeometryAnimation::AxisChange,
        ] {
            let mut state = spawning_state(PaneAnimationStyle::Scale, animation);
            state.config.animations.geometry_duration = Duration::from_millis(200);
            state.config.animations.close_duration = Duration::from_millis(640);

            let pane = &state.current().workspaces[0].panes[0];
            assert_eq!(
                geometry_transition_for_pane(&state, pane, false, None).duration,
                Duration::from_millis(200),
                "{animation:?} is not a pane opening or closing"
            );
        }
    }

    /// A state with one settled tiled pane, mid spawn or close, for asserting transition policy.
    fn spawning_state(
        style: PaneAnimationStyle,
        animation: GeometryAnimation,
    ) -> crate::state::State {
        let mut state =
            crate::state::State::new(crate::config::Config::default(), Default::default());
        state.config.animations.pane_style = style;
        state.config.animations.geometry_duration = Duration::from_millis(200);
        state.animation = animation;
        let workspace = &mut state.current_mut().workspaces[0];
        workspace.panes.clear();
        let mut pane = Pane::new(1, 100, FloatRect::default());
        pane.opening = false;
        workspace.panes.push(pane);
        state
    }

    #[test]
    fn the_spring_nudge_stays_a_couple_of_cells_at_any_tile_size() {
        // The whole point of sizing the amplitude: overshoot in *cells* has to stay put as the tile
        // grows, or a big split throws its neighbour a tenth of the screen and back.
        let mut previous_cells = f32::MAX;
        for width in [20.0_f32, 40.0, 80.0, 160.0, 320.0] {
            let rect = FloatRect {
                x: 0.0,
                y: 0.0,
                w: width,
                h: width / 2.0,
            };
            let extent = spring_extent(rect);
            let permille = spring_overshoot_permille(extent);
            let cells = extent * f32::from(permille) / 1000.0;
            assert!(
                (1.0..=3.5).contains(&cells),
                "a {width}-column tile should nudge by about three cells, got {cells}"
            );
            // Amplitude only ever shrinks as the tile grows.
            assert!(permille as f32 <= previous_cells);
            previous_cells = permille as f32;
        }

        // A tile too small to nudge inside gets no spring rather than a violent one.
        assert_eq!(spring_overshoot_permille(0.0), 0);
        assert!(spring_overshoot_permille(4.0) <= 100);
    }

    #[test]
    fn slide_offset_starts_flush_outside_its_edge_and_ends_deployed() {
        let rect = FloatRect {
            x: 10.0,
            y: 4.0,
            w: 40.0,
            h: 12.0,
        };

        for edge in [
            SlideEdge::Left,
            SlideEdge::Right,
            SlideEdge::Top,
            SlideEdge::Bottom,
        ] {
            assert_eq!(slide_offset(rect, edge, 1.0), (0.0, 0.0));
        }

        // At rest-minus-everything the pane is exactly its own extent away, so the clip to its tile
        // leaves nothing of it visible.
        assert_eq!(slide_offset(rect, SlideEdge::Right, 0.0), (40.0, 0.0));
        assert_eq!(slide_offset(rect, SlideEdge::Left, 0.0), (-40.0, 0.0));
        assert_eq!(slide_offset(rect, SlideEdge::Bottom, 0.0), (0.0, 12.0));
        assert_eq!(slide_offset(rect, SlideEdge::Top, 0.0), (0.0, -12.0));

        assert_eq!(slide_offset(rect, SlideEdge::Bottom, 0.25), (0.0, 9.0));

        // A curve that overshoots past 1.0 must not carry the pane back out of its tile on the far
        // side; the offset floors at its resting value.
        assert_eq!(slide_offset(rect, SlideEdge::Right, 1.4), (0.0, 0.0));
    }

    #[test]
    fn pane_animation_style_round_trips_its_config_token() {
        for style in PaneAnimationStyle::all().iter().copied() {
            assert_eq!(PaneAnimationStyle::parse(style.id()), Some(style));
            assert_eq!(style.next().prev(), style);
            assert_eq!(style.prev().next(), style);
        }
        assert_eq!(
            PaneAnimationStyle::parse("  SLIDE "),
            Some(PaneAnimationStyle::Slide)
        );
        assert_eq!(PaneAnimationStyle::parse("springy"), None);
        assert_eq!(PaneAnimationStyle::default(), PaneAnimationStyle::Scale);
    }

    #[test]
    fn pane_opacity_target_stays_hidden_until_non_sliding_pane_settles() {
        let mut pane = Pane::new(1, 100, FloatRect::default());
        let mut animations = WindowAnimationConfig {
            pane_style: PaneAnimationStyle::Portal,
            ..WindowAnimationConfig::default()
        };

        for style in [
            PaneAnimationStyle::Scale,
            PaneAnimationStyle::Portal,
            PaneAnimationStyle::Scan,
        ] {
            animations.pane_style = style;
            pane.opening = true;
            animations.spawn = false;
            animations.enabled = false;
            assert_eq!(pane_opacity_target(animations, &pane), 0.0);
            assert!(!pane_opacity_animates(animations, &pane));

            pane.opening = false;
            pane.closing = true;
            animations.close = false;
            animations.enabled = true;
            assert_eq!(pane_opacity_target(animations, &pane), 0.0);
            assert!(!pane_opacity_animates(animations, &pane));

            pane.closing = false;
            assert_eq!(pane_opacity_target(animations, &pane), 1.0);
        }

        pane.closing = true;
        animations.pane_style = PaneAnimationStyle::Slide;
        assert_eq!(pane_opacity_target(animations, &pane), 1.0);
    }

    #[test]
    fn unsnapshotted_lifecycle_motion_follows_the_master_switch() {
        let mut pane = Pane::new(1, 100, FloatRect::default());
        let mut animations = WindowAnimationConfig::default();
        assert!(lifecycle_motion_enabled(animations, &pane));

        animations.enabled = false;
        assert!(!lifecycle_motion_enabled(animations, &pane));
        assert!(!pane_opacity_animates(animations, &pane));

        pane.begin_open_animation(animations);
        assert!(!pane.opening_animation.expect("snapshot").active);
        assert!(!lifecycle_motion_enabled(animations, &pane));

        pane.opening = false;
        pane.opening_animation = None;
        pane.closing = true;
        assert!(!lifecycle_motion_enabled(animations, &pane));
        pane.begin_close_animation(animations);
        assert!(!lifecycle_motion_enabled(animations, &pane));
    }

    /// The snapshot deliberately outlives `Pane::opening` so the effect stays mounted on its
    /// original recipe until the terminal goes live. Reading it to pick the *target* parked every
    /// opening pane at its starting value for the whole transition, so nothing ever animated in.
    #[test]
    fn an_opening_pane_travels_once_its_spawn_timer_clears_the_flag() {
        let animations = WindowAnimationConfig::default();
        let mut pane = Pane::new(1, 100, FloatRect::default());
        pane.opening = true;
        pane.begin_open_animation(animations);

        assert_eq!(
            pane_opacity_target(animations, &pane),
            0.0,
            "parked while the spawn is still in flight"
        );

        // What `finish_open` does: clears the flag, keeps the snapshot for `activate_pane`.
        pane.opening = false;
        assert!(pane.opening_animation.is_some());
        assert_eq!(
            pane_opacity_target(animations, &pane),
            1.0,
            "and travels while the snapshot still holds the effect mounted"
        );
        assert!(pane_opacity_animates(animations, &pane));
    }

    #[test]
    fn fade_defaults_follow_builtin_style_and_can_be_disabled() {
        assert!(builtin_animation(PaneAnimationStyle::Scale).fade);
        assert!(!builtin_animation(PaneAnimationStyle::Slide).fade);
        assert!(builtin_animation(PaneAnimationStyle::Portal).fade);
        assert!(builtin_animation(PaneAnimationStyle::Scan).fade);

        let mut animations = WindowAnimationConfig::default();
        let mut pane = Pane::new(1, 100, FloatRect::default());
        let mut spec = builtin_animation(PaneAnimationStyle::Portal);
        spec.fade = false;
        pane.opening = true;
        pane.opening_animation = Some(PaneAnimationSnapshot { spec, active: true });
        assert!(!pane_opacity_animates(animations, &pane));
        assert_eq!(pane_opacity_target(animations, &pane), 1.0);

        animations.pane_style = PaneAnimationStyle::Slide;
        assert_eq!(pane_opacity_target(animations, &pane), 1.0);
    }

    #[test]
    fn pane_reveal_progress_and_opacity_share_complementary_open_close_policies() {
        let mut animations = WindowAnimationConfig {
            pane_style: PaneAnimationStyle::Portal,
            ..WindowAnimationConfig::default()
        };
        let mut pane = Pane::new(1, 100, FloatRect::default());

        for style in [PaneAnimationStyle::Portal, PaneAnimationStyle::Scan] {
            animations.pane_style = style;
            assert!(pane_reveal_effects(animations));
            assert!(pane_opacity_animates(animations, &pane));

            // The paint effect reads `transition`, the fade over it reads `visual_transition`. They
            // have to agree, or the cells finish arriving before or after the pane is fully opaque.
            pane.closing = false;
            let spec = pane_animation_for_pane(animations, &pane);
            let opening_effect = spec.transition(pane.closing);
            let opening_opacity = spec.visual_transition(pane.closing);
            assert_eq!(opening_effect.duration, opening_opacity.duration);
            assert_eq!(opening_effect.easing, opening_opacity.easing);
            assert_eq!(opening_effect.easing, Easing::EaseOutQuad);

            pane.closing = true;
            let spec = pane_animation_for_pane(animations, &pane);
            let closing_effect = spec.transition(pane.closing);
            let closing_opacity = spec.visual_transition(pane.closing);
            assert_eq!(closing_effect.duration, closing_opacity.duration);
            assert_eq!(closing_effect.easing, closing_opacity.easing);
            assert_eq!(closing_effect.easing, Easing::EaseInQuad);

            assert_eq!(opening_effect.duration, closing_effect.duration);
            assert_ne!(opening_effect.easing, closing_effect.easing);
        }
    }

    #[test]
    fn pane_transition_snapshots_survive_selection_changes_and_drive_retention() {
        let mut animations = WindowAnimationConfig {
            pane_style: PaneAnimationStyle::Portal,
            geometry_duration: Duration::from_millis(300),
            ..WindowAnimationConfig::default()
        };
        let mut pane = Pane::new(1, 100, FloatRect::default());
        pane.begin_open_animation(animations);
        animations.pane_style = PaneAnimationStyle::Scale;
        assert_eq!(
            pane_animation_for_pane(animations, &pane).kind,
            PaneAnimationStyle::Portal
        );

        pane.opening = false;
        pane.opening_animation = None;
        pane.closing = true;
        pane.begin_close_animation(animations);
        animations.pane_style = PaneAnimationStyle::Scan;
        animations.close_duration = Duration::from_millis(800);
        assert_eq!(
            pane_animation_for_pane(animations, &pane).kind,
            PaneAnimationStyle::Scale
        );
        assert_eq!(
            retained_pane_timeout_for_pane(animations, &pane),
            Duration::from_millis(140)
        );
    }

    #[test]
    fn the_panel_is_anchored_to_its_dock_edge_inside_its_clip_window() {
        const SIDEBAR: u16 = 32;

        for window in [0, 1, 15, 31, SIDEBAR] {
            // Docked right the panel's left edge is the one the pane column meets, so it sits at the
            // near side of the window and its overhang runs off the far side on its own.
            assert_eq!(sidebar_slide_offset(window, SIDEBAR, true), 0.0);
            // Docked left it is anchored by its right edge instead, so the offset is exactly the
            // part of it still to arrive - never more, which would leave a gap at the seam.
            let offset = sidebar_slide_offset(window, SIDEBAR, false);
            assert_eq!(offset, f32::from(window) - f32::from(SIDEBAR));
            assert!(offset <= 0.0 && offset >= -f32::from(SIDEBAR));
            // The anchored edge always lands on the edge of the window the pane column meets.
            assert_eq!(offset + f32::from(SIDEBAR), f32::from(window));
        }

        // Fully deployed, the panel fills its window exactly, either dock.
        assert_eq!(sidebar_slide_offset(SIDEBAR, SIDEBAR, false), 0.0);
        assert_eq!(sidebar_slide_offset(SIDEBAR, SIDEBAR, true), 0.0);
    }

    #[test]
    fn the_sidebar_slide_shares_the_scratchpad_curve_and_yields_to_its_toggles() {
        let animations = WindowAnimationConfig::default();
        assert_eq!(
            sidebar_transition(animations).duration,
            scratch_transition_duration(animations.geometry_duration)
        );
        // Ease-out with no overshoot: the pane column is clipped to the viewport, so carrying it
        // past flush would open a gap at the far edge for as long as the overshoot lasted.
        assert_eq!(sidebar_transition(animations).easing, Easing::EaseOutQuad);

        for off in [
            WindowAnimationConfig {
                sidebar: false,
                ..animations
            },
            WindowAnimationConfig {
                enabled: false,
                ..animations
            },
        ] {
            assert_eq!(sidebar_transition(off).duration, Duration::ZERO);
        }
    }

    #[test]
    fn alert_pulse_half_period_has_an_accessible_floor() {
        let mut animations = WindowAnimationConfig::default();
        assert_eq!(
            alert_pulse_half_period(animations),
            Duration::from_millis(800)
        );
        animations.alert_pulse_duration = Duration::from_millis(100);
        assert_eq!(
            alert_pulse_half_period(animations),
            Duration::from_millis(400)
        );
        animations.alert_pulse_duration = Duration::ZERO;
        assert_eq!(
            alert_pulse_half_period(animations),
            Duration::from_millis(400)
        );
    }

    #[test]
    fn slide_springs_the_neighbours_and_leaves_the_travelling_pane_alone() {
        let mut state = State::new(crate::config::Config::default(), Default::default());
        state.config.animations.pane_style = PaneAnimationStyle::Slide;
        state.animation = GeometryAnimation::Spawn;
        let workspace = &mut state.current_mut().workspaces[0];
        workspace.panes.clear();
        for id in [1, 2] {
            let mut pane = Pane::new(id, 100, FloatRect::default());
            pane.opening = id == 2;
            workspace.panes.push(pane);
        }

        let tile = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 30.0,
            h: 20.0,
        };
        let settled = &state.current().workspaces[0].panes[0];
        let arriving = &state.current().workspaces[0].panes[1];

        let neighbour = geometry_transition_for_pane(&state, settled, false, Some(tile));
        assert!(
            matches!(
                neighbour.easing,
                Easing::EaseOutBack { overshoot_permille } if overshoot_permille > 0
            ),
            "the tile making room springs, got {:?}",
            neighbour.easing
        );
        assert_eq!(
            neighbour.duration,
            state.config.animations.geometry_duration
        );

        // A bigger tile asks for a proportionally *smaller* amplitude, which is what keeps the nudge
        // a couple of cells instead of a tenth of the pane.
        let wide = FloatRect { w: 300.0, ..tile };
        let wide_neighbour = geometry_transition_for_pane(&state, settled, false, Some(wide));
        let amplitude = |config: TransitionConfig| match config.easing {
            Easing::EaseOutBack { overshoot_permille } => overshoot_permille,
            other => panic!("expected a spring, got {other:?}"),
        };
        assert!(amplitude(wide_neighbour) < amplitude(neighbour));

        // Without a rect there is no amplitude to size, so the spring degrades rather than guessing.
        let unsized_neighbour = geometry_transition_for_pane(&state, settled, false, None);
        assert_eq!(unsized_neighbour.easing, Easing::EaseInOutCubic);

        // The travelling pane's rectangle does not animate at all - `slide_offset` moves it.
        let travelling = geometry_transition_for_pane(&state, arriving, false, Some(tile));
        assert_eq!(travelling.duration, Duration::ZERO);

        // Scale is untouched: neighbours keep the plain geometry curve.
        state.config.animations.pane_style = PaneAnimationStyle::Scale;
        let settled = &state.current().workspaces[0].panes[0];
        let scale_neighbour = geometry_transition_for_pane(&state, settled, false, Some(tile));
        assert_eq!(scale_neighbour.easing, Easing::EaseInOutCubic);
    }

    /// The spring is scoped to spawn and close. A fullscreen toggle or an axis flip under Slide is
    /// still an ordinary geometry move, and springing those would make the whole layout wobble
    /// whenever anything changed shape.
    #[test]
    fn slide_does_not_spring_unrelated_geometry_animations() {
        let mut state = State::new(crate::config::Config::default(), Default::default());
        state.config.animations.pane_style = PaneAnimationStyle::Slide;
        let workspace = &mut state.current_mut().workspaces[0];
        workspace.panes.clear();
        let mut pane = Pane::new(1, 100, FloatRect::default());
        pane.opening = false;
        workspace.panes.push(pane);

        for animation in [
            GeometryAnimation::Fullscreen,
            GeometryAnimation::TileFloat,
            GeometryAnimation::AxisChange,
        ] {
            state.animation = animation;
            let pane = &state.current().workspaces[0].panes[0];
            let config = geometry_transition_for_pane(
                &state,
                pane,
                false,
                Some(FloatRect {
                    x: 0.0,
                    y: 0.0,
                    w: 30.0,
                    h: 20.0,
                }),
            );
            assert_eq!(
                config.easing,
                Easing::EaseInOutCubic,
                "{animation:?} is not a spawn or close"
            );
        }
    }

    /// A floating pane has no tile edge to emerge from and no neighbour to take space from, so it
    /// keeps the scale whatever the style says - and therefore keeps its fade.
    #[test]
    fn floating_panes_never_slide() {
        let mut state = State::new(crate::config::Config::default(), Default::default());
        state.config.animations.pane_style = PaneAnimationStyle::Slide;
        let workspace = &mut state.current_mut().workspaces[0];
        workspace.panes.clear();
        let mut pane = Pane::new(1, 100, FloatRect::default());
        pane.opening = true;
        pane.floating = true;
        workspace.panes.push(pane);

        let pane = &state.current().workspaces[0].panes[0];
        assert!(!pane_slides(state.config.animations, pane));
    }

    #[test]
    fn paint_effect_panes_keep_geometry_fixed_while_neighbours_use_plain_motion() {
        let mut state = State::new(crate::config::Config::default(), Default::default());
        state.animation = GeometryAnimation::Spawn;
        let workspace = &mut state.current_mut().workspaces[0];
        workspace.panes.clear();
        for id in [1, 2] {
            let mut pane = Pane::new(id, 100, FloatRect::default());
            pane.opening = id == 2;
            workspace.panes.push(pane);
        }
        let tile = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 30.0,
            h: 20.0,
        };

        for style in [PaneAnimationStyle::Portal, PaneAnimationStyle::Scan] {
            state.config.animations.pane_style = style;
            let settled = &state.current().workspaces[0].panes[0];
            let arriving = &state.current().workspaces[0].panes[1];
            assert_eq!(
                geometry_transition_for_pane(&state, arriving, false, Some(tile)).duration,
                Duration::ZERO,
                "{style:?} owns its final rectangle"
            );
            assert_eq!(
                geometry_transition_for_pane(&state, settled, false, Some(tile)).easing,
                Easing::EaseInOutCubic,
                "{style:?} does not spring neighbouring tiles"
            );
            assert_eq!(
                retained_pane_timeout(state.config.animations),
                state.config.animations.geometry_duration + Duration::from_millis(20)
            );
        }
    }

    /// Two tiled panes in a shared session whose lease belongs to `controller`.
    fn shared_state(controller: crate::layout::shared::ClientId) -> State {
        let mut state = State::new(crate::config::Config::default(), Default::default());
        state.current_mut().session_attached = true;
        let mut shared = crate::state::SharedSessionState::new(1);
        shared.controller = Some(controller);
        state.current_mut().shared = Some(shared);
        let workspace = &mut state.current_mut().workspaces[0];
        workspace.panes.clear();
        for id in 1..=2 {
            let mut pane = Pane::new(id, 100, FloatRect::default());
            pane.opening = false;
            workspace.panes.push(pane);
        }
        state
    }

    /// A layout revision is a destination, not a path. Followers reconcile toward it through the
    /// same `GeometryAnimation` the controller uses, so they animate the same way - anything else
    /// makes one client's workspace snap while another's eases.
    #[test]
    fn a_follower_animates_the_geometry_a_layout_revision_brings() {
        let mut state = shared_state(2);
        state.animation = GeometryAnimation::TileFloat;
        let pane = &state.current().workspaces[0].panes[0];

        assert!(
            geometry_animation_enabled(&state, pane, false),
            "a follower reconciling toward a new revision animates like the controller"
        );
    }

    /// Continuous manipulation is direct on every screen. Easing toward each reported position
    /// would leave the carried pane a relay behind the pointer for the whole gesture; the tiles
    /// around it still animate, because they move once at the lift and once at the drop.
    #[test]
    fn a_carried_pane_tracks_directly_while_its_neighbours_still_animate() {
        let mut state = shared_state(2);
        state.current_mut().remote_drag = Some(crate::state::RemoteDrag {
            pane_id: 1,
            rect: crate::layout::shared::FracRect {
                x: 0.1,
                y: 0.1,
                w: 0.4,
                h: 0.4,
            },
        });
        state.animation = GeometryAnimation::TileFloat;

        let carried = &state.current().workspaces[0].panes[0];
        assert!(
            !geometry_animation_enabled(&state, carried, false),
            "the pane being carried is reported, not derived - it must not ease"
        );
        let neighbour = &state.current().workspaces[0].panes[1];
        assert!(
            geometry_animation_enabled(&state, neighbour, false),
            "the tile it vacated makes one discrete move, which animates"
        );
    }

    /// The same rule from the other side: this client's own drag is direct too.
    #[test]
    fn a_locally_dragged_pane_tracks_directly() {
        let mut state = shared_state(1);
        state.animation = GeometryAnimation::TileFloat;
        state.moving_pane = Some(crate::state::MoveSession {
            id: 1,
            was_floating: false,
            drag_rect: FloatRect::default(),
            pointer_x: 0,
            pointer_y: 0,
        });

        let carried = &state.current().workspaces[0].panes[0];
        assert!(!geometry_animation_enabled(&state, carried, false));
        let neighbour = &state.current().workspaces[0].panes[1];
        assert!(geometry_animation_enabled(&state, neighbour, false));
    }
}
