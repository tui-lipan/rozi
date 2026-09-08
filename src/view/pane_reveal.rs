use tui_lipan::prelude::{CellEffect, EffectCell, EffectContext, EffectScope, Element, Key};

use crate::layout::anim::{PaneAnimationSpec, PaneAnimationStyle, ScanDirection};

/// Apply the optional pane reveal effect while keeping an empty keyed scope mounted at rest.
pub(super) fn pane_reveal_scope(
    pane_tree: Element,
    key: Key,
    spec: PaneAnimationSpec,
    progress: f32,
    seed: u64,
) -> Element {
    let scope = EffectScope::new();
    let scope = match (spec.kind, progress < 1.0) {
        (PaneAnimationStyle::Portal, true) => scope.custom_effect(PaneRevealEffect::with_spec(
            PaneRevealPattern::Portal,
            progress,
            seed,
            spec,
        )),
        (PaneAnimationStyle::Scan, true) => scope.custom_effect(PaneRevealEffect::with_spec(
            PaneRevealPattern::Scan,
            progress,
            seed,
            spec,
        )),
        _ => scope,
    };
    let scoped: Element = scope.child(pane_tree).into();
    scoped.key(key)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PaneRevealPattern {
    Portal,
    Scan,
}

#[derive(Clone, Copy, Debug)]
struct PaneRevealEffect {
    pattern: PaneRevealPattern,
    progress: f32,
    seed: u64,
    spec: PaneAnimationSpec,
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
        }
    }

    fn apply_portal(&self, cell: &mut EffectCell, position: RevealPosition) {
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
            return;
        }
        if distance <= radius + ring {
            // Half the ring's cells, chosen by position rather than by frame, so the ring reads as
            // a sparse edge that the reveal moves through rather than as static noise.
            let hash = pane_spatial_hash(position.x, position.y, self.seed);
            if hash & 1 == 0 {
                cell.set_symbol(portal_symbol(hash));
            } else {
                cell.set_symbol(" ");
            }
        } else {
            cell.set_symbol(" ");
        }
    }

    fn apply_scan(&self, cell: &mut EffectCell, position: RevealPosition) {
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
        if scan <= (self.progress - frontier).max(0.0) {
            return;
        }
        if scan <= self.progress {
            cell.set_symbol(frontier_symbol(hash, quantized));
        } else {
            cell.set_symbol(" ");
        }
    }
}

impl CellEffect for PaneRevealEffect {
    fn apply(&self, cell: &mut EffectCell, ctx: &EffectContext) {
        if self.progress >= 1.0 {
            return;
        }
        if self.progress <= 0.0 {
            cell.set_symbol(" ");
            return;
        }
        let position = reveal_position(ctx);
        if !position.is_valid() {
            cell.set_symbol(" ");
            return;
        }
        match self.pattern {
            PaneRevealPattern::Portal => self.apply_portal(cell, position),
            PaneRevealPattern::Scan => self.apply_scan(cell, position),
        }
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
    fn pane_reveal_effects_are_stable_distinct_and_safe_at_the_edges() {
        let bounds = Rect {
            x: 4,
            y: 3,
            w: 9,
            h: 4,
        };
        let context = |x, y| EffectContext {
            x,
            y,
            bounds,
            phase: 99,
            terminal_bg: None,
        };
        let render = |pattern| {
            (0..bounds.h)
                .flat_map(|y| {
                    (0..bounds.w).map(move |x| {
                        let mut cell = EffectCell::new("X");
                        PaneRevealEffect::new(pattern, 0.5, 17).apply(
                            &mut cell,
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
            PaneRevealEffect::new(pattern, 1.0, 17)
                .apply(&mut settled, &context(bounds.x, bounds.y));
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
            low.apply(&mut low_cell, &context(bounds.x, bounds.y));
            assert_eq!(low_cell.symbol(), " ");
            let mut high_cell = EffectCell::new("X");
            high.apply(&mut high_cell, &context(bounds.x, bounds.y));
            assert_eq!(high_cell.symbol(), "X");
        }

        for pattern in [PaneRevealPattern::Portal, PaneRevealPattern::Scan] {
            for (w, h) in [(0, 0), (1, 1), (1, 2), (2, 1)] {
                let bounds = Rect { x: 0, y: 0, w, h };
                let mut cell = EffectCell::new("X");
                PaneRevealEffect::new(pattern, 0.5, 0).apply(
                    &mut cell,
                    &EffectContext {
                        x: 0,
                        y: 0,
                        bounds,
                        phase: 0,
                        terminal_bg: None,
                    },
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
                    PaneRevealEffect::new(pattern, progress, seed).apply(
                        &mut cell,
                        &EffectContext {
                            x: bounds.x + x as i16,
                            y: bounds.y + y as i16,
                            bounds,
                            phase: 99,
                            terminal_bg: None,
                        },
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
            PaneRevealEffect::with_spec(pattern, 0.25, 17, spec).apply(
                &mut cell,
                &EffectContext {
                    x,
                    y,
                    bounds,
                    phase: 99,
                    terminal_bg: None,
                },
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
}
