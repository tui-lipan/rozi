use std::time::Duration;

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

pub(crate) const MAX_ANIMATION_ID_LEN: usize = 32;

/// The four values that affect a pane's own open/close presentation. This is deliberately compact:
/// the view copies it into a transition closure on every frame, so it must not carry maps or strings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaneAnimationSpec {
    pub kind: PaneAnimationStyle,
    pub open_duration: Duration,
    pub close_duration: Duration,
    pub open_curve: Easing,
    pub close_curve: Easing,
    pub visual_open_curve: Easing,
    pub visual_close_curve: Easing,
    pub scale_from: f32,
    pub origin: [f32; 2],
    pub frontier_width: f32,
    pub density: f32,
    pub glyphs: GlyphPalette,
    pub custom_glyphs: bool,
    pub custom_frontier_width: bool,
    pub fade: bool,
    pub custom_density: bool,
    pub custom_recipe: bool,
    pub scan_direction: ScanDirection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GlyphPalette {
    bytes: [u8; 8],
    len: u8,
}

impl GlyphPalette {
    pub const fn defaults() -> Self {
        Self {
            bytes: *b".:+*#%/=",
            len: 8,
        }
    }

    pub fn get(self, index: usize) -> u8 {
        self.bytes[index % usize::from(self.len.max(1))]
    }

    pub const fn len(self) -> usize {
        self.len as usize
    }

    pub const fn is_empty(self) -> bool {
        self.len == 0
    }

    pub(crate) fn set(&mut self, index: usize, byte: u8) {
        self.bytes[index] = byte;
        self.len = self.len.max((index + 1) as u8);
    }

    pub(crate) fn custom(bytes: &[u8]) -> Self {
        let mut palette = Self {
            bytes: [0; 8],
            len: 0,
        };
        for (index, byte) in bytes.iter().copied().enumerate() {
            palette.set(index, byte);
        }
        palette
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScanDirection {
    #[default]
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AnimationId {
    bytes: [u8; MAX_ANIMATION_ID_LEN],
    len: u8,
    custom: bool,
    index: u8,
}

impl AnimationId {
    pub(crate) fn builtin(style: PaneAnimationStyle) -> Self {
        Self {
            bytes: [0; MAX_ANIMATION_ID_LEN],
            len: 0,
            custom: false,
            index: style as u8,
        }
    }

    pub(crate) fn custom(id: &str) -> Self {
        let mut bytes = [0; MAX_ANIMATION_ID_LEN];
        bytes[..id.len()].copy_from_slice(id.as_bytes());
        Self {
            bytes,
            len: id.len() as u8,
            custom: true,
            index: 0,
        }
    }

    pub(crate) fn is_custom(self) -> bool {
        self.custom
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AnimationChoice {
    pub(crate) id: AnimationId,
    pub(crate) name: String,
    pub(crate) spec: PaneAnimationSpec,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AnimationCatalog {
    pub(crate) choices: Vec<AnimationChoice>,
}

impl AnimationCatalog {
    pub(crate) fn builtin() -> Self {
        Self {
            choices: PaneAnimationStyle::all()
                .iter()
                .copied()
                .map(|style| AnimationChoice {
                    id: AnimationId::builtin(style),
                    name: style.id().to_string(),
                    spec: builtin_animation(style),
                })
                .collect(),
        }
    }

    pub(crate) fn index_of(&self, id: AnimationId) -> usize {
        self.choices
            .iter()
            .position(|choice| choice.id == id)
            .unwrap_or(0)
    }
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
        scale_from: 0.9,
        origin: [0.5, 0.5],
        frontier_width: 0.09,
        density: 0.5,
        glyphs: GlyphPalette::defaults(),
        custom_glyphs: false,
        custom_frontier_width: false,
        fade: !matches!(style, PaneAnimationStyle::Slide),
        custom_density: false,
        custom_recipe: false,
        scan_direction: ScanDirection::TopLeft,
    }
}

pub(crate) fn snapshot_for_open(
    animations: WindowAnimationConfig,
    floating: bool,
) -> PaneAnimationSnapshot {
    let spec = floating_slide_fallback(animations, animations.selected_animation(), floating);
    PaneAnimationSnapshot {
        spec,
        active: animations.enabled && animations.spawn,
    }
}

pub(crate) fn snapshot_for_close(
    animations: WindowAnimationConfig,
    floating: bool,
) -> PaneAnimationSnapshot {
    let spec = floating_slide_fallback(animations, animations.selected_animation(), floating);
    PaneAnimationSnapshot {
        spec,
        active: animations.enabled && animations.close,
    }
}

impl PaneAnimationStyle {
    /// Cycle order for the Settings row.
    pub fn all() -> &'static [Self] {
        &[Self::Scale, Self::Slide, Self::Portal, Self::Scan]
    }

    /// Config token and persisted value.
    pub fn id(self) -> &'static str {
        match self {
            Self::Scale => "scale",
            Self::Slide => "slide",
            Self::Portal => "portal",
            Self::Scan => "scan",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Scale => "Scale",
            Self::Slide => "Slide",
            Self::Portal => "Portal",
            Self::Scan => "Scan",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "scale" => Some(Self::Scale),
            "slide" => Some(Self::Slide),
            "portal" => Some(Self::Portal),
            "scan" => Some(Self::Scan),
            _ => None,
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Scale => Self::Slide,
            Self::Slide => Self::Portal,
            Self::Portal => Self::Scan,
            Self::Scan => Self::Scale,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Scale => Self::Scan,
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
    let spec = snapshot
        .map(|snapshot| snapshot.spec)
        .unwrap_or_else(|| animations.selected_animation());
    floating_slide_fallback(animations, spec, pane.floating)
}

pub fn pane_opening_transition(pane: &crate::state::Pane) -> bool {
    pane.opening || pane.opening_animation.is_some()
}

fn floating_slide_fallback(
    animations: WindowAnimationConfig,
    spec: PaneAnimationSpec,
    floating: bool,
) -> PaneAnimationSpec {
    if !floating || spec.kind != PaneAnimationStyle::Slide {
        return spec;
    }
    if spec.custom_recipe {
        PaneAnimationSpec {
            kind: PaneAnimationStyle::Scale,
            ..spec
        }
    } else {
        let mut fallback = builtin_animation(PaneAnimationStyle::Scale);
        fallback.open_duration = animations.geometry_duration;
        fallback.close_duration = animations.close_duration;
        fallback
    }
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

/// Whether a pane should fade during its current open or close transition.
pub fn pane_opacity_animates(animations: WindowAnimationConfig, pane: &crate::state::Pane) -> bool {
    let spec = pane_animation_for_pane(animations, pane);
    let opening = pane_opening_transition(pane);
    spec.fade
        && !pane_slides(animations, pane)
        && ((opening
            && pane
                .opening_animation
                .map(|snapshot| snapshot.active)
                .unwrap_or(animations.enabled && animations.spawn)
            && spec.kind != PaneAnimationStyle::Slide)
            || (pane.closing
                && pane
                    .closing_animation
                    .map(|snapshot| snapshot.active)
                    .unwrap_or(animations.enabled && animations.close)
                && spec.kind != PaneAnimationStyle::Slide))
}

/// Visibility target for a pane's open/close opacity animation.
///
/// Animation gates choose whether the transition is timed, not whether a retained closing pane is
/// visible. A pane that is opening or closing must stay at the hidden target until its lifecycle
/// state settles; otherwise disabling close animation can make it reappear before pruning.
pub fn pane_opacity_target(animations: WindowAnimationConfig, pane: &crate::state::Pane) -> f32 {
    let spec = pane_animation_for_pane(animations, pane);
    let opening = pane_opening_transition(pane);
    if !spec.fade || pane_slides(animations, pane) || (!opening && !pane.closing) {
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
    pub focus_chrome: bool,
    pub pane_style: PaneAnimationStyle,
    pub(crate) pane_animation_id: AnimationId,
    pub(crate) pane_animation: PaneAnimationSpec,
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
            focus_chrome: true,
            pane_style: PaneAnimationStyle::Scale,
            pane_animation_id: AnimationId::builtin(PaneAnimationStyle::Scale),
            pane_animation: builtin_animation(PaneAnimationStyle::Scale),
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
        if self.pane_animation_id.is_custom() {
            self.pane_animation
        } else {
            let mut spec = builtin_animation(self.pane_style);
            spec.open_duration = self.geometry_duration;
            spec.close_duration = if self.pane_style == PaneAnimationStyle::Scale {
                self.close_duration
            } else {
                self.geometry_duration
            };
            spec
        }
    }

    pub(crate) fn selected_id(self) -> AnimationId {
        self.pane_animation_id
    }

    pub(crate) fn set_selection(&mut self, choice: &AnimationChoice) {
        self.pane_style = choice.spec.kind;
        self.pane_animation_id = choice.id;
        self.pane_animation = choice.spec;
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

pub fn instant_transition() -> TransitionConfig {
    TransitionConfig {
        duration: Duration::ZERO,
        easing: Easing::Linear,
    }
}

pub fn open_delay(animations: WindowAnimationConfig) -> Duration {
    if animations.enabled && animations.spawn {
        animations.open_delay
    } else {
        Duration::ZERO
    }
}

pub fn activation_delay(animations: WindowAnimationConfig) -> Duration {
    if animations.enabled && animations.spawn {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppRoot;
    use crate::state::Pane;

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
        let tile_making_room = AppRoot::geometry_transition_for_pane(
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
    fn builtin_scale_close_neighbors_keep_the_geometry_duration() {
        let mut state = spawning_state(PaneAnimationStyle::Scale, GeometryAnimation::Close);
        state.config.animations.geometry_duration = Duration::from_millis(300);
        state.config.animations.close_duration = Duration::from_millis(80);
        state.begin_pane_event(GeometryAnimation::Close);
        let pane = &state.current().workspaces[0].panes[0];
        let transition = AppRoot::geometry_transition_for_pane(
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

    #[test]
    fn custom_event_duration_survives_selection_change_and_other_events_ignore_it() {
        let mut state = spawning_state(PaneAnimationStyle::Scale, GeometryAnimation::Spawn);
        let mut custom = builtin_animation(PaneAnimationStyle::Scale);
        custom.custom_recipe = true;
        custom.open_duration = Duration::from_millis(480);
        state.config.animations.pane_animation_id = AnimationId::custom("long-scale");
        state.config.animations.pane_animation = custom;
        state.begin_pane_event(GeometryAnimation::Spawn);
        let scan = AnimationCatalog::builtin()
            .choices
            .into_iter()
            .find(|choice| choice.spec.kind == PaneAnimationStyle::Scan)
            .expect("builtin Scan choice");
        state.config.animations.set_selection(&scan);
        assert_eq!(state.config.animations.selected_id(), scan.id);
        assert_eq!(
            AppRoot::geometry_transition_for_pane(
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
            Duration::from_millis(480)
        );
        state.animation = GeometryAnimation::Fullscreen;
        let pane = &state.current().workspaces[0].panes[0];
        assert_eq!(
            AppRoot::geometry_transition_for_pane(&state, pane, false, None).duration,
            state.config.animations.geometry_duration
        );
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
}
