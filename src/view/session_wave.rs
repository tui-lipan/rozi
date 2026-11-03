//! The session Wave: a left-to-right wave that wipes the outgoing session away.
//!
//! A compositor over the outgoing session's retained layer, which is this scope's backdrop. Ahead
//! of a bowed, slowly swelling front the outgoing session still shows, lit by the approaching
//! crest. The front is a bright band in the focused-border colour with a soft halo. Behind it the
//! incoming session resolves through a rippling wake that drifts toward the informational hue and
//! settles back to untouched cells.
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tui_lipan::prelude::{
    CellEffect, EffectCell, EffectContext, EffectPrepareContext, EffectScope, Element,
    PreparedCellEffect, Rect, TerminalColor, Theme,
};

use crate::layout::anim::PanePaintMotion;

type Rgb = (u8, u8, u8);

/// Wrap the session content layer in the wave, keeping the keyed scope mounted at rest.
pub(super) fn session_wave_scope(
    content: Element,
    motion: impl Into<PanePaintMotion>,
    palette: SessionWavePalette,
) -> Element {
    let motion = motion.into();
    let scope = EffectScope::new();
    let scope = if matches!(motion, PanePaintMotion::Fixed(1.0)) {
        scope
    } else {
        scope.custom_effect(SessionWaveEffect::new(motion, palette))
    };
    let scoped: Element = scope.child(content).into();
    // The portal's key: switching style mid-reveal keeps the incoming content mounted.
    scoped.key("rozi-session-portal")
}

/// The colours the wave is lit in, taken from the active theme.
///
/// The crest uses the focused-border colour, the same "this is where you are" signal the portal's
/// ring leads with, and the wake drifts toward the informational hue. Blending needs RGB; a theme
/// that follows the terminal palette lights the crest band in its palette colour and skips the
/// gradients, which cannot be mixed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SessionWavePalette {
    crest: Option<TerminalColor>,
    wake: Option<TerminalColor>,
}

impl SessionWavePalette {
    pub(crate) fn from_theme(theme: &Theme) -> Self {
        let crest = super::pane_reveal::terminal_color(theme.border_active);
        Self {
            crest,
            wake: super::pane_reveal::terminal_color(theme.status.info).or(crest),
        }
    }
}

impl Default for SessionWavePalette {
    fn default() -> Self {
        Self {
            crest: Some(TerminalColor::Cyan),
            wake: Some(TerminalColor::Blue),
        }
    }
}

#[derive(Debug)]
struct SessionWaveEffect {
    motion: PanePaintMotion,
    palette: SessionWavePalette,
    finished: AtomicBool,
}

impl SessionWaveEffect {
    fn new(motion: PanePaintMotion, palette: SessionWavePalette) -> Self {
        Self {
            motion,
            palette,
            finished: AtomicBool::new(false),
        }
    }

    fn frame(&self, ctx: &EffectPrepareContext) -> Option<WaveFrame> {
        let (progress, _) = self.motion.sample(ctx.elapsed);
        (progress < 1.0).then(|| {
            WaveFrame::new(
                progress,
                ctx.bounds,
                self.palette,
                ctx.terminal_bg.and_then(rgb_of),
            )
        })
    }
}

impl CellEffect for SessionWaveEffect {
    fn apply(&self, _: &mut EffectCell, _: &EffectContext) {}
    fn image_mask(&self, ctx: &EffectPrepareContext) -> Option<tui_lipan::CellMask> {
        self.frame(ctx).map(|frame| frame.image_mask())
    }
    fn uses_backdrop(&self) -> bool {
        matches!(self.motion, PanePaintMotion::Fixed(p) if p < 1.0)
            || !self.finished.load(Ordering::Relaxed)
    }
    fn is_animated(&self) -> bool {
        matches!(self.motion, PanePaintMotion::Timed { .. })
            && !self.finished.load(Ordering::Relaxed)
    }
    fn animation_interval(&self) -> Duration {
        match self.motion {
            PanePaintMotion::Timed { interval, .. } => interval,
            PanePaintMotion::Fixed(_) => Duration::from_millis(16),
        }
    }
    fn prepare(&self, ctx: &EffectPrepareContext) -> Option<Box<dyn PreparedCellEffect>> {
        let (_, finished) = self.motion.sample(ctx.elapsed);
        self.finished.store(finished, Ordering::Relaxed);
        self.frame(ctx)
            .map(|frame| Box::new(frame) as Box<dyn PreparedCellEffect>)
    }
}

/// Contributions below this are invisible. Leaving those cells untouched keeps default-coloured
/// (`Reset`) terminal cells intact once the wave has passed.
const EPSILON: f32 = 0.004;

/// One frame of the wave: where the front is on each row, and the colours it lights.
#[derive(Debug)]
struct WaveFrame {
    bounds: Rect,
    /// The front's column, relative to the scope, on each row.
    fronts: Vec<f32>,
    trail: f32,
    t: f32,
    crest: Option<TerminalColor>,
    crest_rgb: Option<Rgb>,
    wake_rgb: Option<Rgb>,
    peak_rgb: Option<Rgb>,
    terminal_bg: Option<Rgb>,
}

impl WaveFrame {
    /// `progress` is linear; the wave applies its own forward-leaning ease so it enters already
    /// moving and glides to rest.
    fn new(
        progress: f32,
        bounds: Rect,
        palette: SessionWavePalette,
        terminal_bg: Option<Rgb>,
    ) -> Self {
        let t = progress.clamp(0.0, 1.0);
        let w = f32::from(bounds.w.max(1));
        let h = f32::from(bounds.h.max(1));
        let trail = (w * 0.42).max(16.0);
        let amp = (h * 0.1).clamp(1.5, 3.5);
        let bow = (w * 0.06).clamp(2.0, 8.0);
        let lead = bow + amp + 2.0;
        let sine = 0.5 - 0.5 * (std::f32::consts::PI * t).cos();
        let eased = 0.45 * sine + 0.55 * (1.0 - (1.0 - t).powf(2.2));
        let base = -lead + eased * (w + trail + lead + amp + 4.0);
        let fronts = (0..bounds.h)
            .map(|row| {
                let row = f32::from(row);
                let ny = ((row + 0.5) / h) * 2.0 - 1.0;
                // A long swell with a faint counter-moving harmonic: organic, never wiggly.
                let swell =
                    0.75 * (row * 0.2 + t * 4.0).sin() + 0.25 * (row * 0.47 - t * 7.0).sin();
                base + bow * (1.0 - ny * ny) + amp * swell
            })
            .collect();
        let crest_rgb = palette.crest.and_then(rgb_of);
        Self {
            bounds,
            fronts,
            trail,
            t,
            crest: palette.crest,
            crest_rgb,
            wake_rgb: palette.wake.and_then(rgb_of).or(crest_rgb),
            peak_rgb: crest_rgb.map(|crest| mix(crest, (255, 255, 255), 0.55)),
            terminal_bg,
        }
    }

    /// Distance behind the front for the scope-relative cell; negative is not yet reached.
    fn depth(&self, x: i32, y: i32) -> f32 {
        let front = usize::try_from(y)
            .ok()
            .and_then(|row| self.fronts.get(row))
            .copied()
            .unwrap_or(f32::NEG_INFINITY);
        front - (x as f32 + 0.5)
    }

    fn crest(&self, depth: f32, y: i32) -> f32 {
        let sparkle = 0.88 + 0.12 * (y as f32 * 1.7 + self.t * 23.0).sin();
        gauss(depth - 0.5, 0.95) * sparkle
    }

    fn halo(depth: f32) -> f32 {
        0.42 * gauss(depth - 0.3, 3.4)
    }

    /// Incoming cells: those the front has passed. Pictures show only there.
    fn image_mask(&self) -> tui_lipan::CellMask {
        let width = usize::from(self.bounds.w);
        let mut bits = vec![0u64; (width * usize::from(self.bounds.h)).div_ceil(64)];
        for y in 0..self.bounds.h {
            for x in 0..self.bounds.w {
                if self.depth(i32::from(x), i32::from(y)) >= 0.0 {
                    let bit = usize::from(y) * width + usize::from(x);
                    bits[bit / 64] |= 1 << (bit % 64);
                }
            }
        }
        tui_lipan::CellMask {
            origin: (0, 0),
            w: self.bounds.w,
            h: self.bounds.h,
            bits: bits.into(),
        }
    }

    /// Light `base` with the wake tint and the crest, for a background or a surface glyph.
    fn light(&self, base: Rgb, tint: f32, hue: Rgb, glow: f32, crest: f32) -> Rgb {
        let (Some(primary), Some(peak)) = (self.crest_rgb, self.peak_rgb) else {
            return base;
        };
        let lit = mix(mix(base, hue, tint), primary, glow * 0.85);
        mix(lit, peak, crest.powi(3) * 0.3)
    }
}

impl PreparedCellEffect for WaveFrame {
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
        let depth = self.depth(x, y);
        let crest = self.crest(depth, y);
        let halo = Self::halo(depth);
        if depth < 0.0 {
            // Not reached yet: the outgoing session, lit by the approaching crest.
            *cell = backdrop.clone();
            let glow = crest.max(halo).max(0.16 * (depth / 4.0).exp());
            self.paint_glow(cell, glow, crest);
            return;
        }
        let wake = 1.0 - smoothstep(0.0, self.trail, depth);
        let fade = smoothstep(0.0, self.trail * 0.45, depth);
        if wake <= EPSILON && halo <= EPSILON && fade >= 1.0 {
            return;
        }
        let (Some(primary), Some(wake_rgb)) = (self.crest_rgb, self.wake_rgb) else {
            // A palette theme cannot be blended: draw the crest as a solid band only.
            if crest.max(halo) > 0.45
                && let Some(color) = self.crest
            {
                cell.set_bg(color);
            }
            return;
        };
        let ripple = 0.5 + 0.5 * (depth * 0.5 - self.t * 5.0).cos();
        let tint = wake.powi(2) * (0.17 + 0.13 * ripple);
        let hue = mix(primary, wake_rgb, smoothstep(0.0, self.trail * 0.7, depth));
        let glow = crest.max(halo);
        // A default background blends from the terminal's own; one in the terminal's ANSI palette
        // has no known RGB, so it keeps its colour and only the crest band is laid over it.
        let content_bg = match cell.bg {
            TerminalColor::Reset => rgb_of(backdrop.bg).or(self.terminal_bg),
            color => rgb_of(color),
        };
        let Some(content_bg) = content_bg else {
            if glow > 0.45
                && let Some(color) = self.crest
            {
                cell.set_bg(color);
            }
            return;
        };
        let bg = self.light(content_bg, tint, hue, glow, crest);
        if let Some(fg) = rgb_of(cell.fg) {
            let fg = if is_surface_glyph(cell.symbol()) {
                // Block and box glyphs paint surfaces (borders, bars): they arrive with the
                // surface they shape instead of fading in like text.
                self.light(fg, tint, hue, glow, crest)
            } else {
                let peak = self.peak_rgb.unwrap_or(primary);
                mix(mix(bg, fg, 0.12 + 0.88 * fade), peak, crest * 0.7)
            };
            cell.set_fg(rgb(fg));
        }
        cell.set_bg(rgb(bg));
    }
}

impl WaveFrame {
    fn paint_glow(&self, cell: &mut EffectCell, glow: f32, crest: f32) {
        if glow <= EPSILON {
            return;
        }
        match (self.crest_rgb, rgb_of(cell.bg).or(self.terminal_bg)) {
            (Some(_), Some(base)) => {
                let lit = self.light(base, 0.0, base, glow, crest);
                cell.set_bg(rgb(lit));
            }
            _ if glow > 0.45 => {
                if let Some(color) = self.crest {
                    cell.set_bg(color);
                }
            }
            _ => {}
        }
    }
}

/// Box-drawing, block-element and shade glyphs (U+2500..=U+259F) draw surfaces, not text.
fn is_surface_glyph(symbol: &str) -> bool {
    symbol
        .chars()
        .next()
        .is_some_and(|c| ('\u{2500}'..='\u{259F}').contains(&c))
}

fn gauss(x: f32, sigma: f32) -> f32 {
    (-(x * x) / (2.0 * sigma * sigma)).exp()
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let channel = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
    (channel(a.0, b.0), channel(a.1, b.1), channel(a.2, b.2))
}

fn rgb((r, g, b): Rgb) -> TerminalColor {
    TerminalColor::Rgb(r, g, b)
}

/// The RGB of a colour whose value does not depend on the terminal: truecolor, and the xterm
/// 256-colour cube and grey ramp. The 16 ANSI colours follow the terminal's palette, so they have
/// none.
fn rgb_of(color: TerminalColor) -> Option<Rgb> {
    match color {
        TerminalColor::Rgb(r, g, b) => Some((r, g, b)),
        TerminalColor::Indexed(index @ 16..=231) => {
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            let i = index - 16;
            Some((level(i / 36), level(i / 6 % 6), level(i % 6)))
        }
        TerminalColor::Indexed(index @ 232..=255) => {
            let grey = 8 + (index - 232) * 10;
            Some((grey, grey, grey))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNDS: Rect = Rect {
        x: 2,
        y: 1,
        w: 80,
        h: 24,
    };
    const DARK: TerminalColor = TerminalColor::Rgb(10, 10, 10);
    const TEXT: TerminalColor = TerminalColor::Rgb(200, 200, 200);

    fn palette() -> SessionWavePalette {
        SessionWavePalette {
            crest: Some(TerminalColor::Rgb(250, 120, 60)),
            wake: Some(TerminalColor::Rgb(90, 140, 250)),
        }
    }

    /// Composite one incoming cell `symbol` over an outgoing `.` at `progress`.
    fn composite(frame: &WaveFrame, symbol: &str, x: i32, y: i32) -> EffectCell {
        let mut backdrop = EffectCell::default();
        backdrop.set_symbol(".").set_bg(DARK);
        let mut cell = EffectCell::default();
        cell.set_symbol(symbol).set_fg(TEXT).set_bg(DARK);
        let ctx = EffectContext::new(BOUNDS.x + x as i16, BOUNDS.y + y as i16, BOUNDS);
        frame.apply_with_backdrop(&mut cell, &backdrop, &ctx);
        cell
    }

    fn frame(progress: f32) -> WaveFrame {
        WaveFrame::new(progress, BOUNDS, palette(), None)
    }

    #[test]
    fn the_wave_sweeps_left_to_right_with_a_lit_crest() {
        assert_eq!(composite(&frame(0.0), "H", 0, 12).symbol(), ".");
        let mid = frame(0.4);
        assert_eq!(composite(&mid, "H", 0, 12).symbol(), "H");
        assert_eq!(composite(&mid, "H", 79, 12).symbol(), ".");
        // The front column carries the crest-coloured glow on its background.
        let crest_x = (0..80).find(|&x| mid.depth(x, 12) < 0.6).unwrap();
        let TerminalColor::Rgb(r, _, b) = composite(&mid, "H", crest_x, 12).bg else {
            panic!("the crest is lit");
        };
        assert!(r > 100 && r > b, "crest glows in the crest hue");
        // Each row's front leads the one a step later in time.
        let later = frame(0.5);
        assert!((0..24).all(|y| later.depth(40, y) > mid.depth(40, y)));
    }

    #[test]
    fn a_settled_cell_and_a_finished_wave_leave_the_incoming_session_untouched() {
        let settled = composite(&frame(0.99), "H", 0, 0);
        assert_eq!((settled.fg, settled.bg), (TEXT, DARK));
        let effect = SessionWaveEffect::new(
            PanePaintMotion::Timed {
                started_at: Duration::ZERO,
                transition: tui_lipan::prelude::TransitionConfig {
                    duration: Duration::from_millis(880),
                    easing: tui_lipan::prelude::Easing::Linear,
                },
                closing: false,
                interval: Duration::from_millis(16),
            },
            palette(),
        );
        let at = |ms| EffectPrepareContext::new(BOUNDS).with_elapsed(Duration::from_millis(ms));
        assert!(effect.prepare(&at(400)).is_some());
        assert!(effect.is_animated() && effect.uses_backdrop());
        assert!(effect.prepare(&at(880)).is_none());
        assert!(!effect.is_animated() && !effect.uses_backdrop());
    }

    #[test]
    fn surface_glyphs_arrive_with_their_surface_while_text_fades_in() {
        let mid = frame(0.4);
        let x = (0..80).rev().find(|&x| mid.depth(x, 12) > 6.0).unwrap();
        let luma = |c: TerminalColor| match c {
            TerminalColor::Rgb(r, g, b) => u32::from(r) + u32::from(g) + u32::from(b),
            _ => 0,
        };
        let block = composite(&mid, "▀", x, 12);
        let text = composite(&mid, "A", x, 12);
        assert!(luma(block.fg) > luma(text.fg), "{block:?} vs {text:?}");
    }

    #[test]
    fn a_palette_theme_draws_a_solid_crest_and_never_blends() {
        let palette = SessionWavePalette {
            crest: Some(TerminalColor::Cyan),
            wake: Some(TerminalColor::Blue),
        };
        let mid = WaveFrame::new(0.4, BOUNDS, palette, None);
        let crest_x = (0..80).find(|&x| mid.depth(x, 12) < 0.6).unwrap();
        assert_eq!(composite(&mid, "H", crest_x, 12).bg, TerminalColor::Cyan);
        let behind = composite(&mid, "H", 0, 12);
        assert_eq!(
            (behind.fg, behind.bg),
            (TEXT, DARK),
            "nothing to blend with"
        );
    }

    #[test]
    fn palette_cells_keep_their_colour_and_cube_cells_blend() {
        assert_eq!(rgb_of(TerminalColor::Indexed(196)), Some((255, 0, 0)));
        assert_eq!(rgb_of(TerminalColor::Indexed(16)), Some((0, 0, 0)));
        assert_eq!(rgb_of(TerminalColor::Indexed(244)), Some((128, 128, 128)));
        assert_eq!(rgb_of(TerminalColor::Indexed(3)), None);
        let mid = frame(0.4);
        let x = (0..80).rev().find(|&x| mid.depth(x, 12) > 6.0).unwrap();
        let shade = |bg| {
            let backdrop = EffectCell::default();
            let mut cell = EffectCell::default();
            cell.set_symbol(" ").set_bg(bg);
            let ctx = EffectContext::new(BOUNDS.x + x as i16, BOUNDS.y + 12, BOUNDS);
            mid.apply_with_backdrop(&mut cell, &backdrop, &ctx);
            cell.bg
        };
        // An ANSI colour is left alone behind the crest rather than replaced by a guess.
        assert_eq!(shade(TerminalColor::Red), TerminalColor::Red);
        // A 256-colour cell stays recognisably its own colour while the wake tints it.
        let TerminalColor::Rgb(r, g, _) = shade(TerminalColor::Indexed(196)) else {
            panic!("a cube colour blends");
        };
        assert!(r > 150 && g < 100, "{r} {g}");
    }

    #[test]
    fn pictures_show_only_where_the_front_has_passed() {
        let mid = frame(0.4);
        let mask = mid.image_mask();
        let bit = |x: u16, y: u16| {
            let i = usize::from(y) * usize::from(BOUNDS.w) + usize::from(x);
            mask.bits[i / 64] & (1 << (i % 64)) != 0
        };
        assert!(bit(0, 12));
        assert!(!bit(79, 12));
    }
}
