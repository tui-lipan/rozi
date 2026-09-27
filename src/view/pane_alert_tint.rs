//! The content half of a pane alert: a faint wash of the alert colour over the terminal, breathing
//! on the same beat as the border.
//!
//! The border breathes through a late-bound chrome paint, so each of its frames is a repaint rather
//! than a rebuild of the window. A tint over the content has to hold to the same bargain, but an
//! effect scope's built-in tint embeds its strength, and moving that strength would re-run `view()`
//! ten times a second for as long as an agent waits. So the strength is not a value here at all: the
//! effect carries the fade (where it starts, where it ends, when it began) and reads the clock once
//! per painted frame. The only rebuilds are the pulse ticks that flip the phase, which the border
//! already pays for.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tui_lipan::prelude::{
    CellEffect, Color, Easing, EffectCell, EffectContext, EffectPrepareContext, PreparedCellEffect,
    TerminalColor, VisualEffect,
};

use crate::state::Pane;

/// One half-beat of the content tint: a fade from `from` to `to`, or a still tint at `to`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AlertTint {
    /// The alert colour, already known to be truecolor. Palette colours never get this far.
    rgb: (u8, u8, u8),
    from: f32,
    to: f32,
    /// When the fade began. `None` holds the tint at `to`, which is how a static alert, or a pulse
    /// before its first tick, draws.
    started: Option<Instant>,
    duration: Duration,
    frame_interval: Duration,
}

impl AlertTint {
    /// A tint that holds still at `strength`.
    pub(crate) fn still(color: Color, strength: f32) -> Option<Self> {
        Self::fading(
            color,
            strength,
            strength,
            None,
            Duration::ZERO,
            Duration::MAX,
        )
    }

    /// A tint fading from `from` to `to` over `duration`, starting at `started`.
    ///
    /// `None` for a colour the terminal palette owns: blending it would go through a fixed xterm
    /// table and repaint the pane in colours that are not the user's.
    pub(crate) fn fading(
        color: Color,
        from: f32,
        to: f32,
        started: Option<Instant>,
        duration: Duration,
        frame_interval: Duration,
    ) -> Option<Self> {
        let Color::Rgb(r, g, b) = color else {
            return None;
        };
        Some(Self {
            rgb: (r, g, b),
            from: from.clamp(0.0, 1.0),
            to: to.clamp(0.0, 1.0),
            started,
            duration,
            frame_interval,
        })
    }

    /// How strongly the tint covers the content at `now`. The fade eases in and out, the same curve
    /// as the border's, so the two surfaces move as one.
    pub(crate) fn strength_at(&self, now: Instant) -> f32 {
        let Some(started) = self.started else {
            return self.to;
        };
        if self.duration.is_zero() {
            return self.to;
        }
        let t = (now.saturating_duration_since(started).as_secs_f32()
            / self.duration.as_secs_f32())
        .clamp(0.0, 1.0);
        self.from + (self.to - self.from) * Easing::EaseInOutCubic.apply(t)
    }

    fn settled_at(&self, now: Instant) -> bool {
        self.started
            .is_none_or(|started| now.saturating_duration_since(started) >= self.duration)
    }
}

/// The effect for `tint`, reusing the one this pane already carries when the tint has not changed.
///
/// Effect scopes compare custom effects by identity, so a fresh `Arc` in every `view()` would make
/// the scope look changed on every rebuild even though it paints exactly the same.
pub(crate) fn alert_tint_effect(pane: &Pane, tint: Option<AlertTint>) -> Option<VisualEffect> {
    let mut cached = pane.alert_tint.borrow_mut();
    let Some(tint) = tint else {
        *cached = None;
        return None;
    };
    if let Some((held, effect)) = cached.as_ref()
        && *held == tint
    {
        return Some(effect.clone());
    }
    let effect = VisualEffect::Custom(Arc::new(AlertTintEffect(tint)));
    *cached = Some((tint, effect.clone()));
    Some(effect)
}

#[derive(Debug)]
struct AlertTintEffect(AlertTint);

impl CellEffect for AlertTintEffect {
    fn apply(&self, cell: &mut EffectCell, ctx: &EffectContext) {
        PreparedAlertTint::new(self.0, Instant::now()).apply(cell, ctx);
    }

    fn prepare(&self, _ctx: &EffectPrepareContext) -> Option<Box<dyn PreparedCellEffect>> {
        Some(Box::new(PreparedAlertTint::new(self.0, Instant::now())))
    }

    /// Animated only while a fade is in flight. A settled tint drops out of the effect clock, and
    /// the next phase flip rebuilds the pane with the next fade.
    fn is_animated(&self) -> bool {
        !self.0.settled_at(Instant::now())
    }

    fn animation_interval(&self) -> Duration {
        self.0.frame_interval
    }
}

/// One frame's tint: the colour and how far toward it every cell moves.
#[derive(Debug)]
struct PreparedAlertTint {
    rgb: (u8, u8, u8),
    strength: f32,
}

impl PreparedAlertTint {
    fn new(tint: AlertTint, now: Instant) -> Self {
        Self {
            rgb: tint.rgb,
            strength: tint.strength_at(now),
        }
    }
}

impl PreparedCellEffect for PreparedAlertTint {
    fn apply(&self, cell: &mut EffectCell, ctx: &EffectContext) {
        if self.strength <= 0.0 {
            return;
        }
        // The default background is the terminal's own; tint it only when its colour is known, or
        // blank rows would stay dark while the text on them warmed.
        let bg = match cell.bg {
            TerminalColor::Reset => ctx.terminal_bg.unwrap_or(TerminalColor::Reset),
            bg => bg,
        };
        if let Some(bg) = blend(bg, self.rgb, self.strength) {
            cell.bg = bg;
        }
        if let Some(fg) = blend(cell.fg, self.rgb, self.strength) {
            cell.fg = fg;
        }
    }
}

/// `color` moved `strength` of the way toward `target`, or `None` to leave it alone.
///
/// Only colours with a fixed meaning blend: truecolor, and the 6x6x6 cube and grey ramp above the
/// first sixteen indexed slots, which no terminal remaps. The first sixteen and the named colours
/// belong to the user's palette, which rozi cannot see here, so they keep their colour rather than
/// jump to a stand-in while the tint runs.
fn blend(color: TerminalColor, target: (u8, u8, u8), strength: f32) -> Option<TerminalColor> {
    let (r, g, b) = match color {
        TerminalColor::Rgb(r, g, b) => (r, g, b),
        TerminalColor::Indexed(index) if index >= 16 => Color::Indexed(index).to_rgb()?,
        _ => return None,
    };
    let mix = |from: u8, to: u8| {
        (f32::from(from) + (f32::from(to) - f32::from(from)) * strength)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Some(TerminalColor::Rgb(
        mix(r, target.0),
        mix(g, target.1),
        mix(b, target.2),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Color = Color::Rgb(240, 80, 80);

    #[test]
    fn palette_colours_never_tint() {
        assert_eq!(AlertTint::still(Color::Red, 0.1), None);
        assert_eq!(AlertTint::still(Color::Indexed(1), 0.1), None);
        assert!(AlertTint::still(RED, 0.1).is_some());
    }

    #[test]
    fn a_fade_eases_between_its_ends_and_then_holds() {
        let start = Instant::now();
        let second = Duration::from_secs(1);
        let tint = AlertTint::fading(
            RED,
            0.0,
            0.1,
            Some(start),
            second,
            Duration::from_millis(100),
        )
        .unwrap();
        assert_eq!(tint.strength_at(start), 0.0);
        let middle = tint.strength_at(start + second / 2);
        assert!((middle - 0.05).abs() < 1e-4, "{middle}");
        let early = tint.strength_at(start + second / 10);
        assert!(early < 0.01, "eases in rather than lurching: {early}");
        assert_eq!(tint.strength_at(start + second), 0.1);
        assert_eq!(tint.strength_at(start + second * 3), 0.1);
        assert!(!tint.settled_at(start + second / 2));
        assert!(tint.settled_at(start + second));
        assert_eq!(AlertTint::still(RED, 0.1).unwrap().strength_at(start), 0.1);
    }

    #[test]
    fn cells_blend_toward_the_alert_but_palette_cells_keep_their_colour() {
        let prepared = PreparedAlertTint {
            rgb: (255, 0, 0),
            strength: 0.1,
        };
        let ctx = EffectContext {
            x: 0,
            y: 0,
            bounds: tui_lipan::prelude::Rect {
                x: 0,
                y: 0,
                w: 1,
                h: 1,
            },
            phase: 0,
            terminal_bg: Some(TerminalColor::Rgb(0, 0, 0)),
        };

        let mut cell = EffectCell::default();
        cell.fg = TerminalColor::Rgb(200, 200, 200);
        cell.bg = TerminalColor::Reset;
        prepared.apply(&mut cell, &ctx);
        assert_eq!(cell.fg, TerminalColor::Rgb(206, 180, 180));
        assert_eq!(cell.bg, TerminalColor::Rgb(26, 0, 0));

        let mut cell = EffectCell::default();
        cell.fg = TerminalColor::Red;
        cell.bg = TerminalColor::Indexed(4);
        prepared.apply(&mut cell, &ctx);
        assert_eq!(cell.fg, TerminalColor::Red);
        assert_eq!(cell.bg, TerminalColor::Indexed(4));

        let unknown = EffectContext {
            terminal_bg: None,
            ..ctx
        };
        let mut cell = EffectCell::default();
        cell.bg = TerminalColor::Reset;
        prepared.apply(&mut cell, &unknown);
        assert_eq!(cell.bg, TerminalColor::Reset);
    }
}
