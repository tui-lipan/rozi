//! Pane-specific ballistic fragments. The framework owns timing, prepared effects and compositing;
//! this module supplies the art direction and the particle trajectories.
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::layout::anim::PanePaintMotion;
use tui_lipan::prelude::*;

const PAD_X: i16 = 22;
const PAD_Y: i16 = 18;

pub(crate) fn particle_pane(
    child: Element,
    rect: FloatRect,
    motion: PanePaintMotion,
    closing: bool,
    seed: u64,
    key: Key,
    theme: &Theme,
) -> (FloatRect, Element) {
    // Keep this wrapper mounted at rest as well: changing terminal ancestry on the final frame
    // would lose focus and force a terminal remount. Only the effect itself comes and goes.
    let left = (rect.x - f32::from(PAD_X)).max(0.0);
    let top = (rect.y - f32::from(PAD_Y)).max(0.0);
    let inner = Rect {
        x: (rect.x - left).round() as i16,
        y: (rect.y - top).round() as i16,
        w: rect.to_rect().w,
        h: rect.to_rect().h,
    };
    let canvas = Canvas::new().passthrough(true).child_at(inner, child);
    let mut scope = EffectScope::new().child(canvas);
    if !matches!(motion, PanePaintMotion::Fixed(1.0)) {
        scope = scope.custom_effect(ParticleEffect::new(
            inner,
            motion,
            closing,
            seed,
            theme.border_active,
        ));
    }
    let outer = FloatRect {
        x: left,
        y: top,
        w: rect.x + rect.w + f32::from(PAD_X) - left,
        h: rect.y + rect.h + f32::from(PAD_Y) - top,
    };
    (outer, Element::from(scope).key(key))
}

#[derive(Debug)]
struct ParticleEffect {
    inner: Rect,
    motion: PanePaintMotion,
    closing: bool,
    color: Color,
    fragments: Box<[ParticleSource]>,
    finished: AtomicBool,
}

#[derive(Clone, Copy, Debug)]
struct ParticleSource {
    fragment: Fragment,
    start: f32,
    arrival: f32,
    life: f32,
    twinkle: f32,
    emits: bool,
}

impl ParticleEffect {
    fn new(inner: Rect, motion: PanePaintMotion, closing: bool, seed: u64, color: Color) -> Self {
        let fragments = (0..i32::from(inner.h))
            .flat_map(|y| {
                (0..i32::from(inner.w)).map(move |x| {
                    let fragment = Fragment::new(x, y, inner.w, inner.h, seed);
                    let spark_hash = hash(x, y, seed ^ 0x5a17);
                    ParticleSource {
                        fragment,
                        start: unit(fragment.hash, 0) * 0.15,
                        arrival: 0.38 + unit(fragment.hash, 10) * 0.56,
                        life: 0.48 + unit(spark_hash, 0).powf(1.8) * 0.80,
                        twinkle: unit(spark_hash, 20) * 6.0,
                        emits: spark_hash.is_multiple_of(5),
                    }
                })
            })
            .collect();
        Self {
            inner,
            motion,
            closing,
            color,
            fragments,
            finished: AtomicBool::new(false),
        }
    }
}

/// A spatial hash gives every fragment its own repeatable trajectory without random state or
/// frame-rate dependence. Neighbouring 3x2 patches depart together, like small pieces of a pane.
fn hash(x: i32, y: i32, seed: u64) -> u64 {
    let mut n = seed
        ^ (x as u64).wrapping_mul(0x9e3779b97f4a7c15)
        ^ (y as u64).wrapping_mul(0xbf58476d1ce4e5b9);
    n = (n ^ (n >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    n = (n ^ (n >> 27)).wrapping_mul(0x94d049bb133111eb);
    n ^ (n >> 31)
}
fn unit(n: u64, shift: u32) -> f32 {
    ((n >> shift) & 1023) as f32 / 1023.0
}

#[derive(Clone, Copy, Debug)]
struct Fragment {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    delay: f32,
    hash: u64,
}
impl Fragment {
    fn new(x: i32, y: i32, width: u16, height: u16, seed: u64) -> Self {
        let n = hash(x / 3, y / 2, seed);
        let dx = (x as f32 - f32::from(width) * 0.5) * 0.5;
        let dy = y as f32 - f32::from(height) * 0.5;
        let distance = dx.hypot(dy).max(1.0);
        let radius = (distance
            / (f32::from(width) * 0.25)
                .hypot(f32::from(height) * 0.5)
                .max(1.0))
        .min(1.0);
        let speed = 3.0 + unit(n, 10) * 5.0;
        Self {
            x: x as f32,
            y: y as f32,
            vx: dx / distance * speed * 2.0 + (unit(n, 20) - 0.5) * 4.0,
            vy: dy / distance * speed * 0.45 - 5.0 - unit(n, 30) * 3.0,
            delay: radius * 0.20 + unit(n, 0) * 0.10,
            hash: n,
        }
    }
    fn age(self, dispersion: f32) -> f32 {
        ((dispersion - self.delay) / (1.0 - self.delay)).max(0.0)
    }
    fn assembly_position(self, age: f32) -> (f32, f32) {
        let distance = (1.0 - age).powi(3);
        let dx = (unit(self.hash, 20) - 0.5) * 14.0;
        let dy = (unit(self.hash, 30) - 0.5) * 6.0;
        // A slight sideways curl guides each mote into place, with zero velocity at arrival.
        let curl = (age * std::f32::consts::PI).sin() * distance;
        (
            self.x + dx * distance + dy * curl,
            self.y + dy * distance - dx * 0.12 * curl,
        )
    }

    fn position(self, age: f32) -> (f32, f32) {
        // Terminal columns are approximately half as wide as rows are tall. Horizontal velocity
        // is doubled above, so the burst is circular in pixels rather than flattened in cells.
        (
            self.x + self.vx * age,
            self.y + self.vy * age + 13.0 * age * age,
        )
    }
}

#[derive(Clone, Copy, Debug)]
enum SparkGlyph {
    Dot,
    Ember,
    Chip,
    Burst,
}
impl SparkGlyph {
    fn symbol(self) -> &'static str {
        match self {
            Self::Dot => "·",
            Self::Ember => "•",
            Self::Chip => "▪",
            Self::Burst => "⠶",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Spark {
    glyph: SparkGlyph,
    alpha: f32,
    heat: f32,
}
#[derive(Debug)]
struct ParticleFrame {
    bounds: Rect,
    inner: Rect,
    intact: Vec<bool>,
    sparks: Vec<Option<Spark>>,
    color: TerminalColor,
}
impl CellEffect for ParticleEffect {
    fn apply(&self, _: &mut EffectCell, _: &EffectContext) {}
    fn uses_backdrop(&self) -> bool {
        self.closing || !self.finished.load(Ordering::Relaxed)
    }
    fn is_animated(&self) -> bool {
        matches!(self.motion, PanePaintMotion::Timed { .. })
            && !self.finished.load(Ordering::Relaxed)
    }
    fn animation_interval(&self) -> Duration {
        match self.motion {
            PanePaintMotion::Timed { interval, .. } => interval,
            PanePaintMotion::Fixed(_) => Duration::from_nanos(33_333_333),
        }
    }
    fn prepare(&self, ctx: &EffectPrepareContext) -> Option<Box<dyn PreparedCellEffect>> {
        let (progress, finished) = self.motion.sample(ctx.elapsed);
        self.finished.store(
            finished && (self.closing || progress >= 1.0),
            Ordering::Relaxed,
        );
        // Settled opens need neither scratch buffers nor backdrop copies. The persistent wrapper
        // and terminal subtree stay mounted, but the custom effect becomes a no-op.
        if progress >= 1.0 {
            return None;
        }
        let width = usize::from(ctx.bounds.w);
        let mut frame = ParticleFrame {
            bounds: ctx.bounds,
            inner: self.inner,
            intact: vec![false; usize::from(self.inner.w) * usize::from(self.inner.h)],
            sparks: vec![None; width * usize::from(ctx.bounds.h)],
            color: super::pane_reveal::terminal_color(self.color).unwrap_or(TerminalColor::Cyan),
        };
        let flight = (1.0 - progress) * (4.0 / 3.0);
        for (index, source) in self.fragments.iter().enumerate() {
            let fragment = source.fragment;
            let intact = if self.closing {
                flight <= fragment.delay
            } else {
                progress >= source.arrival
            };
            frame.intact[index] = intact;
            if intact || !source.emits {
                continue;
            }
            let (age, life) = if self.closing {
                (fragment.age(flight), source.life)
            } else {
                (
                    ((progress - source.start) / (source.arrival - source.start)).clamp(0.0, 1.0),
                    1.0,
                )
            };
            if age <= 0.0 || age >= life {
                continue;
            }
            let remaining = (1.0 - age / life).clamp(0.0, 1.0);
            // A faint twinkle as embers cool. Its phase follows elapsed motion, never
            // the frame counter, so captures and delayed frames follow the same path.
            let twinkle = if self.closing {
                1.0 - (1.0 - remaining) * 0.22 * (age * 46.0 + source.twinkle).sin().abs()
            } else {
                (age * 9.0).min(1.0)
            };
            let alpha = remaining.powf(if self.closing { 0.7 } else { 0.3 }) * twinkle;
            let glyph = if remaining < 0.28 {
                SparkGlyph::Dot
            } else if remaining < 0.60 {
                SparkGlyph::Ember
            } else if fragment.hash & 1 == 0 {
                SparkGlyph::Chip
            } else {
                SparkGlyph::Burst
            };
            // Short dim trails make the initial impulse legible; they shorten near arrival.
            for trail in (0..=2).rev() {
                let sample = (age - trail as f32 * 0.035).max(0.0);
                let (px, py) = if self.closing {
                    fragment.position(sample)
                } else {
                    fragment.assembly_position(sample)
                };
                let col = px.round() as i32 + i32::from(self.inner.x);
                let row = py.round() as i32 + i32::from(self.inner.y);
                if col < 0
                    || row < 0
                    || col >= i32::from(ctx.bounds.w)
                    || row >= i32::from(ctx.bounds.h)
                {
                    continue;
                }
                let trail_alpha = alpha
                    * if trail == 0 {
                        0.95
                    } else {
                        0.24 / trail as f32
                    };
                let glyph = if trail > 0 { SparkGlyph::Dot } else { glyph };
                let index = row as usize * width + col as usize;
                if frame.sparks[index].is_none_or(|old| old.alpha < trail_alpha) {
                    frame.sparks[index] = Some(Spark {
                        glyph,
                        alpha: trail_alpha,
                        heat: if trail == 0 { remaining * 0.3 } else { 0.0 },
                    });
                }
            }
        }
        Some(Box::new(frame))
    }
}
impl PreparedCellEffect for ParticleFrame {
    fn apply(&self, _: &mut EffectCell, _: &EffectContext) {}
    fn apply_with_backdrop(
        &self,
        cell: &mut EffectCell,
        backdrop: &EffectCell,
        ctx: &EffectContext,
    ) {
        let x = i32::from(ctx.x) - i32::from(self.bounds.x);
        let y = i32::from(ctx.y) - i32::from(self.bounds.y);
        let ix = x - i32::from(self.inner.x);
        let iy = y - i32::from(self.inner.y);
        let intact = ix >= 0
            && iy >= 0
            && ix < i32::from(self.inner.w)
            && iy < i32::from(self.inner.h)
            && self.intact[iy as usize * usize::from(self.inner.w) + ix as usize];
        if intact {
            return;
        }
        *cell = backdrop.clone();
        if x < 0 || y < 0 || x >= i32::from(self.bounds.w) || y >= i32::from(self.bounds.h) {
            return;
        }
        if let Some(spark) = self.sparks[y as usize * usize::from(self.bounds.w) + x as usize] {
            cell.set_symbol(spark.glyph.symbol());
            cell.modifier = Default::default();
            cell.set_fg(match (self.color, backdrop.bg) {
                (TerminalColor::Rgb(r, g, b), TerminalColor::Rgb(br, bg, bb)) => {
                    let mix = |a: u8, b: u8| {
                        let hot = f32::from(a) + (255.0 - f32::from(a)) * spark.heat;
                        (f32::from(b) + (hot - f32::from(b)) * spark.alpha) as u8
                    };
                    TerminalColor::Rgb(mix(r, br), mix(g, bg), mix(b, bb))
                }
                _ => self.color,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gravity_turns_an_upward_burst_downward() {
        let fragment = Fragment::new(20, 10, 40, 20, 7);
        let (_, initial) = fragment.position(0.1);
        let (_, apex) = fragment.position(-fragment.vy / 26.0);
        let (_, final_y) = fragment.position(0.9);
        assert!(initial < fragment.y);
        assert!(apex < initial);
        assert!(final_y > apex);
    }
    #[test]
    fn endpoints_restore_backdrop_and_leave_settled_content_intact() {
        let bounds = Rect {
            x: 0,
            y: 0,
            w: 50,
            h: 25,
        };
        let mut effect = ParticleEffect::new(
            Rect {
                x: 14,
                y: 7,
                w: 20,
                h: 10,
            },
            PanePaintMotion::Fixed(0.0),
            true,
            4,
            Color::Cyan,
        );
        let backdrop = EffectCell::new("x");
        for progress in [0.0, 1.0] {
            effect.motion = PanePaintMotion::Fixed(progress);
            let frame = effect.prepare(&EffectPrepareContext::new(bounds));
            for (x, y) in [(18, 10), (0, 0)] {
                let mut cell = EffectCell::new("A");
                if let Some(frame) = &frame {
                    frame.apply_with_backdrop(
                        &mut cell,
                        &backdrop,
                        &EffectContext::new(x, y, bounds),
                    );
                } else if x == 0 {
                    cell = backdrop.clone(); // Canvas padding is never painted by the child.
                }
                assert_eq!(
                    cell.symbol(),
                    if progress == 1.0 && x == 18 { "A" } else { "x" }
                );
            }
        }
    }
    #[test]
    fn assembly_eases_into_its_home_instead_of_reversing_gravity() {
        let fragment = Fragment::new(20, 10, 40, 20, 7);
        let distance = |age| {
            let (x, y) = fragment.assembly_position(age);
            (x - fragment.x).hypot(y - fragment.y)
        };
        assert!(distance(0.5) < distance(0.0) * 0.3);
        assert!(distance(0.9) < distance(0.5) * 0.02);
        assert_eq!(fragment.assembly_position(1.0), (fragment.x, fragment.y));
    }

    #[test]
    fn sparks_die_individually_and_a_small_tail_survives() {
        let bounds = Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 80,
        };
        let mut effect = ParticleEffect::new(
            Rect {
                x: 30,
                y: 25,
                w: 40,
                h: 20,
            },
            PanePaintMotion::Fixed(0.6),
            true,
            4,
            Color::Cyan,
        );
        let mut counts = Vec::new();
        for progress in [0.6, 0.3, 0.1, 0.0] {
            effect.motion = PanePaintMotion::Fixed(progress);
            let frame = effect.prepare(&EffectPrepareContext::new(bounds)).unwrap();
            let mut count = 0;
            for y in 0..bounds.h {
                for x in 0..bounds.w {
                    let mut cell = EffectCell::new(" ");
                    frame.apply_with_backdrop(
                        &mut cell,
                        &EffectCell::new(" "),
                        &EffectContext::new(x as i16, y as i16, bounds),
                    );
                    count += usize::from(cell.symbol() != " ");
                }
            }
            counts.push(count);
        }
        assert!(counts[0] > counts[1] && counts[1] > counts[2], "{counts:?}");
        assert!(counts[2] > 0, "a few embers should linger");
        assert_eq!(counts[3], 0, "the last frame must restore the backdrop");
    }

    struct ParticlePaintProbe {
        views: std::rc::Rc<std::cell::Cell<usize>>,
        closing: bool,
    }
    impl Component for ParticlePaintProbe {
        type State = ();
        type Properties = ();
        type Message = ();
        fn create_state(&self, _: &()) {}
        fn update(&mut self, _: (), _: &mut Context<Self>) -> Update {
            Update::none()
        }
        fn view(&self, ctx: &Context<Self>) -> Element {
            self.views.set(self.views.get() + 1);
            let (rect, element) = particle_pane(
                Text::new("ASSEMBLED CONTENT").into(),
                FloatRect {
                    x: 10.0,
                    y: 5.0,
                    w: 20.0,
                    h: 10.0,
                },
                PanePaintMotion::Timed {
                    started_at: Duration::ZERO,
                    transition: TransitionConfig {
                        duration: Duration::from_millis(880),
                        easing: Easing::Linear,
                    },
                    closing: self.closing,
                    interval: Duration::from_nanos(33_333_333),
                },
                self.closing,
                4,
                "particles".into(),
                &ctx.theme(),
            );
            Canvas::new().child_at(rect.to_rect(), element).into()
        }
    }

    #[test]
    fn particles_advance_with_paints_without_rebuilding_the_view() {
        for closing in [false, true] {
            let views = std::rc::Rc::new(std::cell::Cell::new(0));
            let mut backend = tui_lipan::TestBackend::new(ParticlePaintProbe {
                views: views.clone(),
                closing,
            });
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 60,
                h: 35,
            });
            backend.render();
            let before = backend.capture_frame().to_fixed_grid_lines();
            let initial_views = views.get();
            for _ in 0..44 {
                backend.advance_frame(Duration::from_millis(10));
            }
            let midway = backend.capture_frame().to_fixed_grid_lines();
            assert_ne!(before, midway, "particles must move on a paint-only frame");
            for _ in 0..44 {
                backend.advance_frame(Duration::from_millis(10));
            }
            let settled = backend.capture_frame().to_fixed_grid_lines();
            assert_eq!(settled.join("\n").contains("ASSEMBLED CONTENT"), !closing);
            for _ in 0..40 {
                backend.advance_frame(Duration::from_millis(10));
            }
            assert_eq!(backend.capture_frame().to_fixed_grid_lines(), settled);
            assert_eq!(
                views.get(),
                initial_views,
                "particle ticks must not call view()"
            );
        }
    }

    #[test]
    fn paint_clock_quantizes_paints_and_stops_at_the_endpoint() {
        let bounds = Rect {
            x: 0,
            y: 0,
            w: 60,
            h: 35,
        };
        let motion = PanePaintMotion::Timed {
            started_at: Duration::from_millis(100),
            transition: TransitionConfig {
                duration: Duration::from_millis(220),
                easing: Easing::Linear,
            },
            closing: false,
            interval: Duration::from_millis(66), // A 15 FPS client remains below 30 FPS.
        };
        assert_eq!(
            motion.sample(Duration::from_millis(170)),
            motion.sample(Duration::from_millis(180))
        );
        assert_eq!(motion.sample(Duration::from_millis(320)), (1.0, true));
        let effect = ParticleEffect::new(
            Rect {
                x: 10,
                y: 5,
                w: 20,
                h: 10,
            },
            motion,
            false,
            4,
            Color::Cyan,
        );
        assert!(effect.is_animated());
        assert!(effect.uses_backdrop());
        assert_eq!(effect.animation_interval(), Duration::from_millis(66));
        assert!(
            effect
                .prepare(
                    &EffectPrepareContext::new(bounds).with_elapsed(Duration::from_millis(320))
                )
                .is_none()
        );
        assert!(!effect.is_animated());
        assert!(!effect.uses_backdrop());
    }

    /// Local timing probe; excludes terminal I/O and application view/layout work.
    #[test]
    #[ignore = "local performance measurement"]
    fn particle_frame_cost() {
        let bounds = Rect {
            x: 0,
            y: 0,
            w: 200,
            h: 60,
        };
        let mut effect = ParticleEffect::new(
            Rect {
                x: 22,
                y: 18,
                w: 156,
                h: 24,
            },
            PanePaintMotion::Fixed(0.5),
            true,
            4,
            Color::Cyan,
        );
        let started = std::time::Instant::now();
        for frame in 0..2000 {
            effect.motion = PanePaintMotion::Fixed(1.0 - (frame % 100) as f32 / 100.0);
            std::hint::black_box(effect.prepare(&EffectPrepareContext::new(bounds)));
        }
        eprintln!(
            "particle prepare: {:?}/frame (200x60 scope)",
            started.elapsed() / 2000
        );
    }
}
