use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tui_lipan::prelude::{
    CellEffect, Color, EffectCell, EffectContext, EffectPrepareContext, EffectScope, Element, Key,
    Paint, PreparedCellEffect, Rect, TerminalColor, Theme,
};

use crate::layout::anim::{PaneAnimationSpec, PaneAnimationStyle, PanePaintMotion, ScanDirection};

/// Use the prepared cell masks for picker visibility too; the portal owns its clock and tail.
pub(super) fn picker_reveal_effect(
    style: crate::layout::anim::PickerAnimationStyle,
    progress: f32,
    opening: bool,
) -> tui_lipan::prelude::VisualEffect {
    use crate::layout::anim::PickerAnimationStyle;
    let (pattern, pane_style) = match style {
        PickerAnimationStyle::Portal => (PaneRevealPattern::Portal, PaneAnimationStyle::Portal),
        PickerAnimationStyle::Scan => (PaneRevealPattern::Scan, PaneAnimationStyle::Scan),
        PickerAnimationStyle::Off | PickerAnimationStyle::Fade => unreachable!(),
    };
    tui_lipan::prelude::VisualEffect::Custom(std::sync::Arc::new(TimedRevealEffect::new(
        RevealRecipe::Pane(
            PaneRevealEffect::with_spec(
                pattern,
                progress,
                0,
                crate::layout::anim::builtin_animation(pane_style),
            )
            .with_initial_frontier(opening),
        ),
        PanePaintMotion::Fixed(progress),
    )))
}

/// Apply the optional pane reveal effect while keeping an empty keyed scope mounted at rest.
pub(super) fn pane_reveal_scope(
    pane_tree: Element,
    key: Key,
    spec: PaneAnimationSpec,
    motion: impl Into<PanePaintMotion>,
    seed: u64,
    closing: bool,
) -> Element {
    let motion = motion.into();
    let scope = EffectScope::new();
    let scope = match PaneRevealPattern::from_style(spec.kind)
        .filter(|_| !matches!(motion, PanePaintMotion::Fixed(1.0)))
    {
        Some(pattern) => scope.custom_effect(TimedRevealEffect::new(
            RevealRecipe::Pane(
                PaneRevealEffect::with_spec(pattern, 0.0, seed, spec)
                    .with_initial_frontier(!closing),
            ),
            motion,
        )),
        None => scope,
    };
    let scoped: Element = scope.child(pane_tree).into();
    scoped.key(key)
}

/// Wrap the session content layer in the session portal, keeping the keyed scope mounted at rest.
pub(super) fn session_portal_scope(
    content: Element,
    motion: impl Into<PanePaintMotion>,
    ring: SessionPortalRing,
) -> Element {
    let motion = motion.into();
    let scope = EffectScope::new();
    let scope = if !matches!(motion, PanePaintMotion::Fixed(1.0)) {
        scope.custom_effect(TimedRevealEffect::new(
            RevealRecipe::Session(SessionPortalEffect::new(0.0, ring)),
            motion,
        ))
    } else {
        scope
    };
    let scoped: Element = scope.child(content).into();
    scoped.key("rozi-session-portal")
}

#[derive(Clone, Copy, Debug)]
enum RevealRecipe {
    Pane(PaneRevealEffect),
    Session(SessionPortalEffect),
}

#[derive(Debug)]
struct TimedRevealEffect {
    recipe: RevealRecipe,
    motion: PanePaintMotion,
    finished: AtomicBool,
}
impl TimedRevealEffect {
    fn new(recipe: RevealRecipe, motion: PanePaintMotion) -> Self {
        Self {
            recipe,
            motion,
            finished: AtomicBool::new(false),
        }
    }
}
impl CellEffect for TimedRevealEffect {
    fn apply(&self, _: &mut EffectCell, _: &EffectContext) {}
    fn uses_backdrop(&self) -> bool {
        matches!(self.motion, PanePaintMotion::Fixed(p) if p < 1.0)
            || matches!(self.motion, PanePaintMotion::Timed { closing: true, .. })
            || !self.finished.load(Ordering::Relaxed)
    }
    fn is_animated(&self) -> bool {
        matches!(self.motion, PanePaintMotion::Timed { .. })
            && !self.finished.load(Ordering::Relaxed)
    }
    fn animation_interval(&self) -> Duration {
        match self.motion {
            PanePaintMotion::Timed { interval, .. } => interval,
            _ => Duration::from_millis(16),
        }
    }
    fn prepare(&self, ctx: &EffectPrepareContext) -> Option<Box<dyn PreparedCellEffect>> {
        let (progress, finished) = self.motion.sample(ctx.elapsed);
        self.finished.store(finished, Ordering::Relaxed);
        if progress >= 1.0 {
            return None;
        }
        let mut recipe = self.recipe;
        match &mut recipe {
            RevealRecipe::Pane(effect) => effect.progress = progress,
            RevealRecipe::Session(effect) => effect.progress = progress,
        }
        Some(Box::new(PreparedReveal::new(recipe, ctx.bounds)))
    }
}

/// Each scope computes its radius, corner distance, ring width and scan thresholds once per paint.
/// The prepared mask holds decisions only: live backdrop colours are still composed per cell.
#[derive(Debug)]
struct PreparedReveal {
    bounds: Rect,
    recipe: RevealRecipe,
    progress: f32,
    center: (f32, f32),
    radius: f32,
    outer_radius: f32,
    frontier: f32,
    quantized: u32,
    scan_maximum: f32,
}
impl PreparedReveal {
    fn new(recipe: RevealRecipe, bounds: Rect) -> Self {
        let (progress, origin) = match recipe {
            RevealRecipe::Pane(effect) => (effect.progress, effect.spec.origin),
            RevealRecipe::Session(effect) => (effect.progress, SessionPortalEffect::ORIGIN),
        };
        let (_, maximum) = portal_distance(0, 0, i32::from(bounds.w), i32::from(bounds.h), origin);
        let radius = progress * maximum;
        Self {
            bounds,
            recipe,
            progress,
            center: (
                (i32::from(bounds.w) - 1) as f32 * origin[0],
                (i32::from(bounds.h) - 1) as f32 * origin[1],
            ),
            radius,
            outer_radius: radius + portal_ring_width(maximum, progress),
            frontier: frontier_width(progress),
            quantized: quantized_progress(progress),
            scan_maximum: (i32::from(bounds.w) - 1) as f32 + (i32::from(bounds.h) - 1) as f32 * 2.0,
        }
    }
    fn decision(&self, x: i32, y: i32) -> RevealCell {
        if self.progress >= 1.0 {
            return RevealCell::Content;
        }
        if let RevealRecipe::Pane(effect) = self.recipe {
            if self.progress <= 0.0 && !effect.initial_frontier {
                return RevealCell::Backdrop;
            }
            if effect.pattern == PaneRevealPattern::Scan {
                let x = match effect.spec.scan_direction {
                    ScanDirection::TopLeft | ScanDirection::BottomLeft => x,
                    _ => i32::from(self.bounds.w) - 1 - x,
                };
                let y = match effect.spec.scan_direction {
                    ScanDirection::TopLeft | ScanDirection::TopRight => y,
                    _ => i32::from(self.bounds.h) - 1 - y,
                };
                let scan = if self.scan_maximum <= 0.0 {
                    0.0
                } else {
                    (x as f32 + y as f32 * 2.0) / self.scan_maximum
                };
                if scan < self.progress - self.frontier {
                    return RevealCell::Content;
                }
                if scan <= self.progress {
                    // Hash the original cell coordinates, not the mirrored scan coordinates.
                    let px = match effect.spec.scan_direction {
                        ScanDirection::TopLeft | ScanDirection::BottomLeft => x,
                        _ => i32::from(self.bounds.w) - 1 - x,
                    };
                    let py = match effect.spec.scan_direction {
                        ScanDirection::TopLeft | ScanDirection::TopRight => y,
                        _ => i32::from(self.bounds.h) - 1 - y,
                    };
                    return RevealCell::Frontier(frontier_symbol(
                        pane_spatial_hash(px, py, effect.seed),
                        self.quantized,
                    ));
                }
                return RevealCell::Backdrop;
            }
        }
        if self.progress <= 0.0 {
            return RevealCell::Backdrop;
        }
        let distance = (x as f32 - self.center.0).hypot((y as f32 - self.center.1) * 2.0);
        if distance <= self.radius {
            return RevealCell::Content;
        }
        if distance <= self.outer_radius {
            let seed = match self.recipe {
                RevealRecipe::Pane(effect) => effect.seed,
                _ => SessionPortalEffect::SEED,
            };
            let hash = pane_spatial_hash(x, y, seed);
            if hash & 1 == 0 {
                return RevealCell::Frontier(portal_symbol(hash));
            }
        }
        RevealCell::Backdrop
    }
}
impl PreparedCellEffect for PreparedReveal {
    fn apply(&self, _: &mut EffectCell, _: &EffectContext) {}
    fn apply_with_backdrop(
        &self,
        cell: &mut EffectCell,
        backdrop: &EffectCell,
        ctx: &EffectContext,
    ) {
        let x = i32::from(ctx.x) - i32::from(self.bounds.x);
        let y = i32::from(ctx.y) - i32::from(self.bounds.y);
        if x < 0 || y < 0 || x >= i32::from(self.bounds.w) || y >= i32::from(self.bounds.h) {
            return;
        }
        let decision = self.decision(x, y);
        let foreground = if matches!(decision, RevealCell::Frontier(_)) {
            match self.recipe {
                RevealRecipe::Session(effect) => {
                    effect
                        .ring
                        .color_for(pane_spatial_hash(x, y, SessionPortalEffect::SEED))
                }
                _ => None,
            }
        } else {
            None
        };
        decision.composite(cell, backdrop);
        if let Some(fg) = foreground {
            cell.set_fg(fg);
        }
    }
}

/// The colours the session portal's ring is drawn in: the active theme's own accents, so the ring
/// reads as part of the theme rather than as bare terminal-white punctuation.
///
/// Led by the focused-border colour - the portal is the same "this is where you are" signal - and
/// joined by the theme accent and its informational hue. A colour the terminal cannot paint as a
/// glyph (reset, backdrop, transparent) is left out; with none left, the ring falls back to the
/// incoming session's own foreground.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SessionPortalRing {
    colors: [Option<TerminalColor>; 3],
}

impl SessionPortalRing {
    pub(crate) fn from_theme(theme: &Theme) -> Self {
        let accent = match theme.accent.fg {
            Some(Paint::Solid(color)) => Some(color),
            Some(Paint::Alpha { color, .. }) => Some(color),
            _ => None,
        };
        Self {
            colors: [
                terminal_color(theme.border_active),
                accent.and_then(terminal_color),
                terminal_color(theme.status.info),
            ],
        }
    }

    /// The ring colour for a cell, chosen by its spatial hash so the mix holds still as the
    /// portal grows instead of shimmering frame to frame.
    fn color_for(&self, hash: u64) -> Option<TerminalColor> {
        let available = self.colors.iter().flatten().count();
        if available == 0 {
            return None;
        }
        let pick = ((hash >> 8) % available as u64) as usize;
        self.colors.iter().flatten().nth(pick).copied()
    }
}

/// A theme colour as the terminal colour an effect paints with. Palette colours stay palette
/// colours, so a theme that follows the terminal's own palette keeps it.
pub(super) fn terminal_color(color: Color) -> Option<TerminalColor> {
    Some(match color {
        Color::Reset | Color::Backdrop | Color::Transparent => return None,
        Color::Black => TerminalColor::Black,
        Color::Red => TerminalColor::Red,
        Color::Green => TerminalColor::Green,
        Color::Yellow => TerminalColor::Yellow,
        Color::Blue => TerminalColor::Blue,
        Color::Magenta => TerminalColor::Magenta,
        Color::Cyan => TerminalColor::Cyan,
        Color::Gray => TerminalColor::Gray,
        Color::DarkGray => TerminalColor::DarkGray,
        Color::LightRed => TerminalColor::LightRed,
        Color::LightGreen => TerminalColor::LightGreen,
        Color::LightYellow => TerminalColor::LightYellow,
        Color::LightBlue => TerminalColor::LightBlue,
        Color::LightMagenta => TerminalColor::LightMagenta,
        Color::LightCyan => TerminalColor::LightCyan,
        Color::White => TerminalColor::White,
        Color::Indexed(index) => TerminalColor::Indexed(index),
        Color::Rgb(r, g, b) => TerminalColor::Rgb(r, g, b),
    })
}

/// A portal opening from the centre of the screen onto the incoming session.
///
/// The pane Portal's geometry and ring, drawn as a compositor rather than a mask: inside the
/// radius the incoming session shows, beyond it the outgoing session's retained layer - this
/// scope's backdrop - and on the ring sparse portal glyphs, in the theme's colours, over the
/// outgoing session.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SessionPortalEffect {
    progress: f32,
    ring: SessionPortalRing,
}

impl SessionPortalEffect {
    /// Fixed so the ring pattern is the same on every switch, as a pane's is for its own id.
    const SEED: u64 = 0x5E55_1011;
    const ORIGIN: [f32; 2] = [0.5, 0.5];

    pub(crate) fn new(progress: f32, ring: SessionPortalRing) -> Self {
        Self {
            progress: progress.clamp(0.0, 1.0),
            ring,
        }
    }
}

impl CellEffect for SessionPortalEffect {
    fn apply(&self, _cell: &mut EffectCell, _ctx: &EffectContext) {}

    fn uses_backdrop(&self) -> bool {
        self.progress < 1.0
    }

    fn apply_with_backdrop(
        &self,
        cell: &mut EffectCell,
        backdrop: &EffectCell,
        ctx: &EffectContext,
    ) {
        let position = reveal_position(ctx);
        if !position.is_valid() {
            return;
        }
        let (distance, maximum) = portal_distance(
            position.x,
            position.y,
            position.width,
            position.height,
            Self::ORIGIN,
        );
        let radius = self.progress * maximum;
        // A closed portal shows nothing of the incoming session, not even the one centre cell a
        // zero radius would otherwise contain.
        if self.progress > 0.0 && distance <= radius {
            return;
        }
        let ring = portal_ring_width(maximum, self.progress);
        let hash = pane_spatial_hash(position.x, position.y, Self::SEED);
        let glyph_fg = self.ring.color_for(hash).unwrap_or(cell.fg);
        *cell = backdrop.clone();
        if self.progress > 0.0 && distance <= radius + ring && hash & 1 == 0 {
            cell.set_symbol(portal_symbol(hash));
            cell.set_fg(glyph_fg);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PaneRevealPattern {
    Portal,
    Scan,
}

impl PaneRevealPattern {
    fn from_style(style: PaneAnimationStyle) -> Option<Self> {
        match style {
            PaneAnimationStyle::Portal => Some(Self::Portal),
            PaneAnimationStyle::Scan => Some(Self::Scan),
            PaneAnimationStyle::Off
            | PaneAnimationStyle::Scale
            | PaneAnimationStyle::Slide
            | PaneAnimationStyle::Particles => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct PaneRevealEffect {
    pattern: PaneRevealPattern,
    progress: f32,
    seed: u64,
    spec: PaneAnimationSpec,
    initial_frontier: bool,
}

impl PaneRevealEffect {
    #[cfg(test)]
    fn new(pattern: PaneRevealPattern, progress: f32, seed: u64) -> Self {
        Self::with_spec(
            pattern,
            progress,
            seed,
            PaneAnimationSpec {
                kind: match pattern {
                    PaneRevealPattern::Portal => PaneAnimationStyle::Portal,
                    PaneRevealPattern::Scan => PaneAnimationStyle::Scan,
                },
                ..crate::layout::anim::builtin_animation(match pattern {
                    PaneRevealPattern::Portal => PaneAnimationStyle::Portal,
                    PaneRevealPattern::Scan => PaneAnimationStyle::Scan,
                })
            },
        )
    }

    fn with_spec(
        pattern: PaneRevealPattern,
        progress: f32,
        seed: u64,
        spec: PaneAnimationSpec,
    ) -> Self {
        Self {
            pattern,
            progress: progress.clamp(0.0, 1.0),
            seed,
            spec,
            initial_frontier: false,
        }
    }

    fn with_initial_frontier(mut self, enabled: bool) -> Self {
        self.initial_frontier = enabled;
        self
    }

    fn initial_cell(&self, position: RevealPosition) -> RevealCell {
        if self.initial_frontier && self.pattern == PaneRevealPattern::Scan {
            self.scan_cell(position)
        } else {
            RevealCell::Backdrop
        }
    }

    fn portal_cell(&self, position: RevealPosition) -> RevealCell {
        let (distance, maximum) = portal_distance(
            position.x,
            position.y,
            position.width,
            position.height,
            self.spec.origin,
        );
        let radius = self.progress * maximum;
        let ring = portal_ring_width(maximum, self.progress);
        if distance <= radius {
            return RevealCell::Content;
        }
        if distance <= radius + ring {
            // Half the ring's cells, chosen by position rather than by frame, so the ring reads as
            // a sparse edge that the reveal moves through rather than as static noise.
            let hash = pane_spatial_hash(position.x, position.y, self.seed);
            if hash & 1 == 0 {
                RevealCell::Frontier(portal_symbol(hash))
            } else {
                RevealCell::Backdrop
            }
        } else {
            RevealCell::Backdrop
        }
    }

    fn scan_cell(&self, position: RevealPosition) -> RevealCell {
        let hash = pane_spatial_hash(position.x, position.y, self.seed);
        let quantized = quantized_progress(self.progress);
        let scan = scan_position(
            position.x,
            position.y,
            position.width,
            position.height,
            self.spec.scan_direction,
        );
        let frontier = frontier_width(self.progress);
        // Keep the first visible corner inside the frontier band. Clamping the content threshold
        // to zero exposed the pane's corner before the scan line had passed it.
        if scan < self.progress - frontier {
            return RevealCell::Content;
        }
        if scan <= self.progress {
            RevealCell::Frontier(frontier_symbol(hash, quantized))
        } else {
            RevealCell::Backdrop
        }
    }

    fn reveal_cell(&self, position: RevealPosition) -> RevealCell {
        if self.progress >= 1.0 {
            return RevealCell::Content;
        }
        if !position.is_valid() {
            return RevealCell::Backdrop;
        }
        if self.progress <= 0.0 {
            return self.initial_cell(position);
        }
        match self.pattern {
            PaneRevealPattern::Portal => self.portal_cell(position),
            PaneRevealPattern::Scan => self.scan_cell(position),
        }
    }
}

/// Patterns choose cell coverage; compositing and backdrop restoration are shared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RevealCell {
    Content,
    Backdrop,
    Frontier(&'static str),
}

impl RevealCell {
    fn composite(self, cell: &mut EffectCell, backdrop: &EffectCell) {
        match self {
            Self::Content => {}
            Self::Backdrop => *cell = backdrop.clone(),
            Self::Frontier(symbol) => {
                let foreground = cell.fg;
                *cell = backdrop.clone();
                cell.set_symbol(symbol);
                cell.set_fg(foreground);
            }
        }
    }
}

impl CellEffect for PaneRevealEffect {
    fn apply(&self, _cell: &mut EffectCell, _ctx: &EffectContext) {}

    fn uses_backdrop(&self) -> bool {
        self.progress < 1.0
    }

    fn apply_with_backdrop(
        &self,
        cell: &mut EffectCell,
        backdrop: &EffectCell,
        ctx: &EffectContext,
    ) {
        self.reveal_cell(reveal_position(ctx))
            .composite(cell, backdrop);
    }
}

#[derive(Clone, Copy)]
struct RevealPosition {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl RevealPosition {
    fn is_valid(self) -> bool {
        self.width > 0
            && self.height > 0
            && self.x >= 0
            && self.y >= 0
            && self.x < self.width
            && self.y < self.height
    }
}

fn reveal_position(ctx: &EffectContext) -> RevealPosition {
    let x = i32::from(ctx.x.saturating_sub(ctx.bounds.x));
    let y = i32::from(ctx.y.saturating_sub(ctx.bounds.y));
    let width = i32::from(ctx.bounds.w);
    let height = i32::from(ctx.bounds.h);
    RevealPosition {
        x,
        y,
        width,
        height,
    }
}

fn portal_distance(x: i32, y: i32, width: i32, height: i32, origin: [f32; 2]) -> (f32, f32) {
    let cx = (width - 1) as f32 * origin[0];
    let cy = (height - 1) as f32 * origin[1];
    let distance = (x as f32 - cx).hypot((y as f32 - cy) * 2.0);
    let maximum = [
        (0.0_f32 - cx).hypot((0.0_f32 - cy) * 2.0),
        ((width - 1) as f32 - cx).hypot((0.0_f32 - cy) * 2.0),
        (0.0_f32 - cx).hypot(((height - 1) as f32 - cy) * 2.0),
        ((width - 1) as f32 - cx).hypot(((height - 1) as f32 - cy) * 2.0),
    ]
    .into_iter()
    .fold(0.0, f32::max);
    (distance, maximum)
}

fn portal_ring_width(maximum: f32, progress: f32) -> f32 {
    let base = (maximum * 0.08).clamp(1.0, 2.0).min(maximum);
    base * ((1.0 - progress) / 0.12).clamp(0.0, 1.0)
}

fn frontier_width(progress: f32) -> f32 {
    const MAX_FRONTIER: f32 = 0.09;
    const SETTLE_WINDOW: f32 = 0.1;
    MAX_FRONTIER * ((1.0 - progress) / SETTLE_WINDOW).clamp(0.0, 1.0)
}

fn quantized_progress(progress: f32) -> u32 {
    (progress.clamp(0.0, 1.0) * 32.0).floor() as u32
}

fn pane_spatial_hash(x: i32, y: i32, seed: u64) -> u64 {
    let mut value = seed
        ^ (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (y as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value ^= value >> 30;
    value = value.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

fn scan_position(x: i32, y: i32, width: i32, height: i32, direction: ScanDirection) -> f32 {
    // Terminal rows are about twice as tall as columns are wide, so correct the diagonal in cell
    // space rather than making the reveal look nearly horizontal.
    const CELL_ASPECT: f32 = 2.0;
    let x = match direction {
        ScanDirection::TopLeft | ScanDirection::BottomLeft => x,
        ScanDirection::TopRight | ScanDirection::BottomRight => width - 1 - x,
    };
    let y = match direction {
        ScanDirection::TopLeft | ScanDirection::TopRight => y,
        ScanDirection::BottomLeft | ScanDirection::BottomRight => height - 1 - y,
    };
    let farthest = (width - 1) as f32 + (height - 1) as f32 * CELL_ASPECT;
    if farthest <= 0.0 {
        0.0
    } else {
        (x as f32 + y as f32 * CELL_ASPECT) / farthest
    }
}

fn frontier_symbol(hash: u64, quantized: u32) -> &'static str {
    const SYMBOLS: [&str; 8] = [".", ":", "+", "*", "#", "%", "/", "="];
    let index = (hash.wrapping_add(u64::from(quantized) * 0x9E37_79B9) as usize) % SYMBOLS.len();
    SYMBOLS[index]
}

fn portal_symbol(hash: u64) -> &'static str {
    match hash >> 60 {
        0 => "+",
        1..=3 => ":",
        _ => ".",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui_lipan::prelude::Rect;

    #[test]
    fn picker_scan_starts_at_the_corner_and_advances_through_overlapping_frontiers() {
        use crate::layout::anim::PickerAnimationStyle;
        let bounds = Rect {
            x: 0,
            y: 0,
            w: 60,
            h: 28,
        };
        let transition = PickerAnimationStyle::Scan.enter_transition();
        let content = EffectCell::new("picker");
        let backdrop = EffectCell::new("underneath");
        for millis in [0, 16, 33] {
            let progress = transition
                .easing
                .apply(millis as f32 / transition.duration.as_millis() as f32);
            let tui_lipan::prelude::VisualEffect::Custom(effect) =
                picker_reveal_effect(PickerAnimationStyle::Scan, progress, true)
            else {
                panic!("scan must use a cell effect")
            };
            let prepared = effect.prepare(&EffectPrepareContext::new(bounds)).unwrap();
            let mut frontier = 0;
            for y in 0..bounds.h {
                for x in 0..bounds.w {
                    let mut cell = content.clone();
                    prepared.apply_with_backdrop(
                        &mut cell,
                        &backdrop,
                        &EffectContext::new(x as i16, y as i16, bounds),
                    );
                    if cell != backdrop {
                        frontier += 1;
                        assert_ne!(
                            cell, content,
                            "the first frames should show the frontier before text"
                        );
                        assert!(
                            x < 7 && y < 4,
                            "the initial line must stay near the top-left corner"
                        );
                    }
                }
            }
            assert!(
                frontier > 0,
                "even the opening frame must show the scan line"
            );
        }
        let tui_lipan::prelude::VisualEffect::Custom(effect) =
            picker_reveal_effect(PickerAnimationStyle::Scan, 0.0, false)
        else {
            unreachable!()
        };
        let prepared = effect.prepare(&EffectPrepareContext::new(bounds)).unwrap();
        let mut cell = content.clone();
        prepared.apply_with_backdrop(&mut cell, &backdrop, &EffectContext::new(0, 0, bounds));
        assert_eq!(
            cell, backdrop,
            "closing endpoint must fully restore the backdrop"
        );
    }

    #[test]
    fn scan_first_visible_frames_contain_only_the_frontier() {
        let bounds = Rect {
            x: 0,
            y: 0,
            w: 41,
            h: 13,
        };
        let content = EffectCell::new("pane-corner");
        let backdrop = EffectCell::new("neighbor");
        for progress in [0.0, 0.001, 0.025, 0.05] {
            let effect = PaneRevealEffect::new(PaneRevealPattern::Scan, progress, 17)
                .with_initial_frontier(true);
            let frame = reveal_frame(&effect, bounds, &content, &backdrop);
            assert!(
                !frame.contains(&content),
                "the starting corner must wait for the frontier to pass"
            );
            assert!(
                frame.iter().any(|cell| cell.symbol() != backdrop.symbol()),
                "the scan line must be visible"
            );
        }
        let effect = PaneRevealEffect::new(PaneRevealPattern::Scan, 0.2, 17);
        assert!(reveal_frame(&effect, bounds, &content, &backdrop).contains(&content));
    }

    #[test]
    fn pane_reveals_restore_live_backdrop_cells_and_colors() {
        let bounds = Rect {
            x: 0,
            y: 0,
            w: 41,
            h: 13,
        };
        let mut backdrop = EffectCell::new("neighbor");
        backdrop.set_fg(TerminalColor::Green);
        backdrop.set_bg(TerminalColor::Blue);
        let mut content = EffectCell::new("pane");
        content.set_fg(TerminalColor::White);
        content.set_bg(TerminalColor::Red);
        for pattern in [PaneRevealPattern::Portal, PaneRevealPattern::Scan] {
            for progress in [0.0, 0.5, 1.0] {
                let effect = PaneRevealEffect::new(pattern, progress, 17);
                assert_eq!(effect.uses_backdrop(), progress < 1.0);
                let frame = reveal_frame(&effect, bounds, &content, &backdrop);
                for cell in &frame {
                    assert_reveal_cell(cell, &content, &backdrop);
                }
                let restored = frame.iter().filter(|cell| **cell == backdrop).count();
                let retained = frame.iter().filter(|cell| **cell == content).count();
                let area = usize::from(bounds.w) * usize::from(bounds.h);
                match progress {
                    0.0 => assert_eq!(restored, area),
                    1.0 => assert_eq!(retained, area),
                    _ => assert!(restored > 0 && retained > 0),
                }
            }
        }
    }

    fn reveal_frame(
        effect: &PaneRevealEffect,
        bounds: Rect,
        content: &EffectCell,
        backdrop: &EffectCell,
    ) -> Vec<EffectCell> {
        (0..bounds.h)
            .flat_map(|y| {
                (0..bounds.w).map(move |x| {
                    let mut cell = content.clone();
                    effect.apply_with_backdrop(
                        &mut cell,
                        backdrop,
                        &EffectContext::new(bounds.x + x as i16, bounds.y + y as i16, bounds),
                    );
                    cell
                })
            })
            .collect()
    }

    fn assert_reveal_cell(cell: &EffectCell, content: &EffectCell, backdrop: &EffectCell) {
        if cell.symbol() == backdrop.symbol() {
            assert_eq!(cell, backdrop);
        } else if cell.symbol() == content.symbol() {
            assert_eq!(cell, content);
        } else {
            assert_eq!(
                cell.bg, backdrop.bg,
                "the frontier must not leave a pane-colored box"
            );
            assert_eq!(cell.fg, content.fg);
        }
    }

    /// Composite a `new` screen over an `old` one through a session portal at `progress`.
    fn session_portal_frame(progress: f32, bounds: Rect) -> Vec<String> {
        let effect = SessionPortalEffect::new(progress, SessionPortalRing::default());
        let old = &EffectCell::new("o");
        (0..bounds.h)
            .flat_map(|y| {
                (0..bounds.w).map(move |x| {
                    let mut cell = EffectCell::new("n");
                    let ctx = EffectContext::new(bounds.x + x as i16, bounds.y + y as i16, bounds);
                    if effect.uses_backdrop() {
                        effect.apply_with_backdrop(&mut cell, old, &ctx);
                    } else {
                        effect.apply(&mut cell, &ctx);
                    }
                    cell.symbol().to_string()
                })
            })
            .collect()
    }

    #[test]
    fn a_session_portal_opens_the_new_screen_over_the_old_one() {
        let bounds = Rect {
            x: 2,
            y: 1,
            w: 41,
            h: 13,
        };
        let at = |frame: &[String], x: u16, y: u16| frame[usize::from(y * bounds.w + x)].clone();
        let (cx, cy) = (bounds.w / 2, bounds.h / 2);

        let closed = session_portal_frame(0.0, bounds);
        assert!(closed.iter().all(|symbol| symbol == "o"), "{closed:?}");

        let half = session_portal_frame(0.5, bounds);
        assert_eq!(at(&half, cx, cy), "n", "the centre opens first");
        assert_eq!(at(&half, 0, 0), "o", "a corner still shows the old screen");
        assert!(
            half.iter()
                .any(|symbol| !matches!(symbol.as_str(), "n" | "o")),
            "the portal's edge carries a ring: {half:?}"
        );
        assert_eq!(
            half,
            session_portal_frame(0.5, bounds),
            "the ring is stable"
        );

        let open = session_portal_frame(1.0, bounds);
        assert!(open.iter().all(|symbol| symbol == "n"), "{open:?}");
        assert!(!SessionPortalEffect::new(1.0, SessionPortalRing::default()).uses_backdrop());
    }

    /// Ring glyphs are drawn in the theme's accents, never the incoming cell's plain foreground.
    #[test]
    fn the_session_portal_ring_takes_the_theme_palette() {
        let theme = Theme::default();
        let ring = SessionPortalRing::from_theme(&theme);
        let palette: Vec<TerminalColor> = ring.colors.iter().flatten().copied().collect();
        assert!(
            !palette.is_empty(),
            "the default theme has accents to draw with"
        );

        let bounds = Rect {
            x: 0,
            y: 0,
            w: 41,
            h: 13,
        };
        let effect = SessionPortalEffect::new(0.5, ring);
        let old = EffectCell::new("o");
        let mut glyphs = 0;
        for y in 0..bounds.h {
            for x in 0..bounds.w {
                let mut cell = EffectCell::new("n");
                cell.set_fg(TerminalColor::White);
                let ctx = EffectContext::new(x as i16, y as i16, bounds);
                effect.apply_with_backdrop(&mut cell, &old, &ctx);
                if !matches!(cell.symbol(), "n" | "o") {
                    glyphs += 1;
                    assert!(palette.contains(&cell.fg), "ring glyph in {:?}", cell.fg);
                }
            }
        }
        assert!(glyphs > 0, "the half-open portal draws a ring");
        assert_eq!(
            SessionPortalRing::default().color_for(7),
            None,
            "no palette leaves the incoming foreground in charge"
        );
    }

    #[test]
    fn pane_reveal_effects_are_stable_distinct_and_safe_at_the_edges() {
        let bounds = Rect {
            x: 4,
            y: 3,
            w: 9,
            h: 4,
        };
        let context = |x, y| EffectContext::new(x, y, bounds).with_phase(99);
        let render = |pattern| {
            (0..bounds.h)
                .flat_map(|y| {
                    (0..bounds.w).map(move |x| {
                        let mut cell = EffectCell::new("X");
                        PaneRevealEffect::new(pattern, 0.5, 17).apply_with_backdrop(
                            &mut cell,
                            &EffectCell::new(" "),
                            &context(bounds.x + x as i16, bounds.y + y as i16),
                        );
                        cell.symbol().to_string()
                    })
                })
                .collect::<Vec<_>>()
        };

        let portal = render(PaneRevealPattern::Portal);
        assert_eq!(portal, render(PaneRevealPattern::Portal));
        assert_ne!(portal, render(PaneRevealPattern::Scan));

        for pattern in [PaneRevealPattern::Portal, PaneRevealPattern::Scan] {
            let original = EffectCell::new("original");
            let mut settled = original.clone();
            PaneRevealEffect::new(pattern, 1.0, 17).apply_with_backdrop(
                &mut settled,
                &EffectCell::new(" "),
                &context(bounds.x, bounds.y),
            );
            assert_eq!(settled, original);
        }

        for pattern in [PaneRevealPattern::Portal, PaneRevealPattern::Scan] {
            let altered = render_at(pattern, bounds, 0.99, 17)
                .into_iter()
                .filter(|symbol| symbol != "X")
                .count();
            assert!(altered <= 4, "near-settled frontier too large: {altered}");
        }

        for pattern in [PaneRevealPattern::Portal, PaneRevealPattern::Scan] {
            let low = PaneRevealEffect::new(pattern, -1.0, 17);
            let high = PaneRevealEffect::new(pattern, 2.0, 17);
            assert_eq!(low.progress, 0.0);
            assert_eq!(high.progress, 1.0);
            let mut low_cell = EffectCell::new("X");
            low.apply_with_backdrop(
                &mut low_cell,
                &EffectCell::new(" "),
                &context(bounds.x, bounds.y),
            );
            assert_eq!(low_cell.symbol(), " ");
            let mut high_cell = EffectCell::new("X");
            high.apply_with_backdrop(
                &mut high_cell,
                &EffectCell::new(" "),
                &context(bounds.x, bounds.y),
            );
            assert_eq!(high_cell.symbol(), "X");
        }

        for pattern in [PaneRevealPattern::Portal, PaneRevealPattern::Scan] {
            for (w, h) in [(0, 0), (1, 1), (1, 2), (2, 1)] {
                let bounds = Rect { x: 0, y: 0, w, h };
                let mut cell = EffectCell::new("X");
                PaneRevealEffect::new(pattern, 0.5, 0).apply_with_backdrop(
                    &mut cell,
                    &EffectCell::new(" "),
                    &EffectContext::new(0, 0, bounds),
                );
            }
        }
    }

    #[test]
    fn portal_reveals_from_center_with_aspect_corrected_monotonic_materialization() {
        let bounds = Rect {
            x: 0,
            y: 0,
            w: 9,
            h: 9,
        };
        let early = render_at(PaneRevealPattern::Portal, bounds, 0.25, 17);
        assert_eq!(early[4 * bounds.w as usize + 4], "X");
        assert_eq!(early[0], " ");

        // Two cells with equal physical distance from the center land on the same side of the
        // reveal boundary even though one moves two columns and the other moves one row.
        assert_eq!(early[4 * bounds.w as usize + 2], "X");
        assert_eq!(early[3 * bounds.w as usize + 4], "X");

        let late = render_at(PaneRevealPattern::Portal, bounds, 0.65, 17);
        for (before, after) in early.iter().zip(&late) {
            if before == "X" {
                assert_eq!(after, "X", "materialized cells must not regress");
            }
        }

        let wide = Rect {
            x: 0,
            y: 0,
            w: 128,
            h: 1,
        };
        let wide_mask = render_at(PaneRevealPattern::Portal, wide, 0.5, 17);
        assert_eq!(wide_mask.len(), 128);
        assert_eq!(wide_mask[64], "X");
    }

    #[test]
    fn portal_ring_is_sparse_deterministic_and_progress_independent_while_overlapping() {
        let bounds = Rect {
            x: 0,
            y: 0,
            w: 61,
            h: 31,
        };
        let first = render_at(PaneRevealPattern::Portal, bounds, 0.5, 17);
        assert_eq!(first, render_at(PaneRevealPattern::Portal, bounds, 0.5, 17));

        let later = render_at(PaneRevealPattern::Portal, bounds, 0.52, 17);
        let stable_ring = first
            .iter()
            .zip(&later)
            .find(|(before, after)| is_portal_glyph(before) && is_portal_glyph(after));
        let (before, after) = stable_ring.expect("the adjacent portal rings should overlap");
        assert_eq!(
            before, after,
            "a ring cell must not flicker as progress changes"
        );

        let glyphs = first.iter().filter(|symbol| is_portal_glyph(symbol));
        let mut dots = 0;
        let mut colons = 0;
        let mut pluses = 0;
        for glyph in glyphs {
            match glyph.as_str() {
                "." => dots += 1,
                ":" => colons += 1,
                "+" => pluses += 1,
                _ => unreachable!(),
            }
        }
        assert!(
            dots > colons && colons >= pluses && dots > 0,
            "portal glyph distribution: dots={dots}, colons={colons}, pluses={pluses}"
        );
    }

    fn is_portal_glyph(symbol: &str) -> bool {
        matches!(symbol, "." | ":" | "+")
    }

    fn render_at(
        pattern: PaneRevealPattern,
        bounds: Rect,
        progress: f32,
        seed: u64,
    ) -> Vec<String> {
        (0..bounds.h)
            .flat_map(|y| {
                (0..bounds.w).map(move |x| {
                    let mut cell = EffectCell::new("X");
                    PaneRevealEffect::new(pattern, progress, seed).apply_with_backdrop(
                        &mut cell,
                        &EffectCell::new(" "),
                        &EffectContext::new(bounds.x + x as i16, bounds.y + y as i16, bounds)
                            .with_phase(99),
                    );
                    cell.symbol().to_string()
                })
            })
            .collect()
    }

    #[test]
    fn pane_reveal_frontier_uses_only_single_width_ascii_punctuation() {
        let bounds = Rect {
            x: 0,
            y: 0,
            w: 20,
            h: 8,
        };
        for pattern in [PaneRevealPattern::Portal, PaneRevealPattern::Scan] {
            for symbol in render_at(pattern, bounds, 0.5, 3) {
                if symbol != "X" && symbol != " " {
                    assert_eq!(symbol.len(), 1);
                    assert!(symbol.as_bytes()[0].is_ascii_punctuation());
                }
            }
        }
    }

    /// The two knobs a recipe still has over the paint effects move *where* the reveal starts.
    /// Whatever they are set to, the cell nearest the origin is revealed before the one furthest
    /// from it - that is what makes the effect read as coming from somewhere.
    #[test]
    fn origin_and_direction_decide_which_corner_arrives_first() {
        let bounds = Rect {
            x: 0,
            y: 0,
            w: 12,
            h: 6,
        };
        // A cell the reveal has reached keeps the content underneath it; one it has not is blanked
        // or wearing a frontier glyph.
        let sample = |spec: PaneAnimationSpec, pattern, x: i16, y: i16| {
            let mut cell = EffectCell::new("X");
            PaneRevealEffect::with_spec(pattern, 0.25, 17, spec).apply_with_backdrop(
                &mut cell,
                &EffectCell::new(" "),
                &EffectContext::new(x, y, bounds).with_phase(99),
            );
            cell.symbol() == "X"
        };

        let mut top_left = crate::layout::anim::builtin_animation(PaneAnimationStyle::Portal);
        top_left.origin = [0.0, 0.0];
        assert!(
            sample(top_left, PaneRevealPattern::Portal, 0, 0),
            "a portal origin of [0, 0] reveals the top-left corner first"
        );
        assert!(
            !sample(top_left, PaneRevealPattern::Portal, 11, 5),
            "and leaves the far corner for later"
        );

        let mut bottom_right = crate::layout::anim::builtin_animation(PaneAnimationStyle::Scan);
        bottom_right.scan_direction = ScanDirection::BottomRight;
        assert!(
            sample(bottom_right, PaneRevealPattern::Scan, 11, 5),
            "a bottom-right scan reveals that corner first"
        );
        assert!(
            !sample(bottom_right, PaneRevealPattern::Scan, 0, 0),
            "and leaves the opposite corner for later"
        );
    }
    #[test]
    fn prepared_reveals_match_the_original_compositor_cell_for_cell() {
        for (width, height) in [(1, 1), (41, 13)] {
            let bounds = Rect {
                x: 7,
                y: 3,
                w: width,
                h: height,
            };
            for progress in [0.0, 0.01, 0.25, 0.6, 0.94, 1.0] {
                for direction in [
                    ScanDirection::TopLeft,
                    ScanDirection::TopRight,
                    ScanDirection::BottomLeft,
                    ScanDirection::BottomRight,
                ] {
                    for pattern in [PaneRevealPattern::Portal, PaneRevealPattern::Scan] {
                        for initial_frontier in [false, true] {
                            let mut effect = PaneRevealEffect::new(pattern, progress, 17)
                                .with_initial_frontier(initial_frontier);
                            effect.spec.scan_direction = direction;
                            effect.spec.origin = [0.2, 0.8];
                            compare_prepared(RevealRecipe::Pane(effect), bounds);
                        }
                    }
                }
                compare_prepared(
                    RevealRecipe::Session(SessionPortalEffect::new(
                        progress,
                        SessionPortalRing::from_theme(&Theme::default()),
                    )),
                    bounds,
                );
            }
        }
    }

    fn compare_prepared(recipe: RevealRecipe, bounds: Rect) {
        let prepared = PreparedReveal::new(recipe, bounds);
        let backdrop = EffectCell::new("b");
        for y in bounds.y..bounds.y + bounds.h as i16 {
            for x in bounds.x..bounds.x + bounds.w as i16 {
                let ctx = EffectContext::new(x, y, bounds);
                let mut reference = EffectCell::new("A");
                reference.set_fg(TerminalColor::Rgb(40, 150, 80));
                reference.set_bg(TerminalColor::Rgb(12, 18, 30));
                let mut optimized = reference.clone();
                match recipe {
                    RevealRecipe::Pane(effect) => {
                        effect.apply_with_backdrop(&mut reference, &backdrop, &ctx)
                    }
                    RevealRecipe::Session(effect) => {
                        effect.apply_with_backdrop(&mut reference, &backdrop, &ctx)
                    }
                }
                prepared.apply_with_backdrop(&mut optimized, &backdrop, &ctx);
                assert_eq!(optimized, reference, "cell ({x}, {y}), {recipe:?}");
            }
        }
    }

    struct RevealPaintProbe {
        views: std::rc::Rc<std::cell::Cell<usize>>,
        recipe: RevealRecipe,
        closing: bool,
    }
    impl tui_lipan::prelude::Component for RevealPaintProbe {
        type State = ();
        type Properties = ();
        type Message = ();
        fn create_state(&self, _: &()) {}
        fn update(&mut self, _: (), _: &mut tui_lipan::Context<Self>) -> tui_lipan::Update {
            tui_lipan::Update::none()
        }
        fn view(&self, _: &tui_lipan::Context<Self>) -> Element {
            self.views.set(self.views.get() + 1);
            EffectScope::new()
                .custom_effect(TimedRevealEffect::new(
                    self.recipe,
                    PanePaintMotion::Timed {
                        started_at: Duration::ZERO,
                        transition: tui_lipan::prelude::TransitionConfig {
                            duration: Duration::from_millis(220),
                            easing: tui_lipan::prelude::Easing::Linear,
                        },
                        closing: self.closing,
                        interval: Duration::from_nanos(8_333_333),
                    },
                ))
                .child(tui_lipan::prelude::Text::new("REVEALED CONTENT"))
                .into()
        }
    }

    #[test]
    fn pane_and_session_masks_advance_without_view_rebuilds() {
        let recipes = [
            RevealRecipe::Pane(PaneRevealEffect::new(PaneRevealPattern::Portal, 0.0, 17)),
            RevealRecipe::Pane(PaneRevealEffect::new(PaneRevealPattern::Scan, 0.0, 17)),
            RevealRecipe::Session(SessionPortalEffect::new(0.0, SessionPortalRing::default())),
        ];
        for recipe in recipes {
            for closing in [false, true] {
                let views = std::rc::Rc::new(std::cell::Cell::new(0));
                let mut backend = tui_lipan::TestBackend::new(RevealPaintProbe {
                    views: views.clone(),
                    recipe,
                    closing,
                });
                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 40,
                    h: 10,
                });
                backend.render();
                let initial_views = views.get();
                let first = backend.capture_frame().to_fixed_grid_lines();
                for _ in 0..11 {
                    backend.advance_frame(Duration::from_millis(10));
                }
                let midway = backend.capture_frame().to_fixed_grid_lines();
                for _ in 0..11 {
                    backend.advance_frame(Duration::from_millis(10));
                }
                let final_frame = backend.capture_frame().to_fixed_grid_lines();
                assert_ne!(first, midway);
                assert_eq!(
                    final_frame.join("\n").contains("REVEALED CONTENT"),
                    !closing
                );
                assert_eq!(views.get(), initial_views);
            }
        }
    }

    /// The two paths process the same cells, glyphs and backdrop at the same progress samples.
    #[test]
    #[ignore = "local performance measurement"]
    fn reveal_frame_cost() {
        let bounds = Rect {
            x: 0,
            y: 0,
            w: 200,
            h: 60,
        };
        for pattern in [PaneRevealPattern::Portal, PaneRevealPattern::Scan] {
            for prepared in [false, true] {
                let started = std::time::Instant::now();
                for frame in 0..100 {
                    let effect = PaneRevealEffect::new(pattern, frame as f32 / 100.0, 17);
                    let prepared_effect = PreparedReveal::new(RevealRecipe::Pane(effect), bounds);
                    let backdrop = EffectCell::new("b");
                    for y in 0..bounds.h {
                        for x in 0..bounds.w {
                            let ctx = EffectContext::new(x as i16, y as i16, bounds);
                            let mut cell = EffectCell::new("A");
                            if prepared {
                                prepared_effect.apply_with_backdrop(&mut cell, &backdrop, &ctx);
                            } else {
                                effect.apply_with_backdrop(&mut cell, &backdrop, &ctx);
                            }
                            std::hint::black_box(cell);
                        }
                    }
                }
                eprintln!(
                    "{pattern:?}, prepared={prepared}: {:?}/frame",
                    started.elapsed() / 100
                );
            }
        }
    }
}
