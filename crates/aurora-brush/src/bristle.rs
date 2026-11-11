//! EXPERIMENTAL (task 168 spike): a Painter-style bristle brush cost model.
//!
//! Not exported from the crate, not wired into any tool, no UI, no saving.
//! It exists to answer one question with numbers: what does a brush made of
//! many small, lagging, paint-carrying bristles cost per pen sample on top of
//! the existing dab engine, against the 10 ms brush budget (invariant 5)?
//! See `docs/spikes/bristle-brush-feasibility.md` for the measurements and the
//! model's limits. It is a plausible *cost* model, not a pretty brush.
//!
//! **The model.**
//!
//! * A brush is `count` bristles laid out on the unit disc by a seeded
//!   golden-angle spiral with seeded jitter (`count` is clamped to
//!   [`MIN_BRISTLES`]`..=`[`MAX_BRISTLES`]). Each has an offset from the
//!   brush centre, a stiffness in `0.3..=1.0`, a colour (the brush colour
//!   with a small seeded brightness jitter) and a paint *load* in `0..=1`.
//! * A [`PenSample`] drives it. **Pressure** scales the spread of the layout
//!   (`0.35..=1` of the radius), the bristle dab radius and the flow;
//!   pressure `0` deposits nothing. **Tilt** shifts the footprint centre
//!   along the tilt direction (up to `0.6` radius) and stretches the layout
//!   into an ellipse along it (long axis x `1 + 1.5 m`, short axis x
//!   `1 - 0.3 m`, `m` the tilt magnitude `0..=1`). **Rotation** (barrel
//!   twist, radians) rotates the layout.
//! * Each bristle follows its target with a spring: `pos += (target - pos) *
//!   k`, `k = 1 - lag * (1 - stiffness / 2)`, so a stiff bristle follows
//!   closely and a soft one trails and splays on a curve; with the pen at
//!   rest every bristle converges to its target.
//! * Between two samples a bristle deposits soft dabs along its own path
//!   (spacing [`BRISTLE_STEP_RADII`] of its radius, floored at 1 px, at most
//!   [`MAX_DABS_PER_BRISTLE_SAMPLE`] per sample), each through the existing
//!   [`crate::stamp_dab_amount`] (since task 170 through the batch
//!   [`crate::deposit::deposit_soft_dabs`], the same bits) with the bristle's colour and
//!   `PaintAmount { opacity: 1, flow: flow * pressure * load }`. The load
//!   depletes with distance travelled (`1 / (load_length_radii * radius)` per
//!   pixel), so a stroke dries out.
//!
//! **Limits (also in the findings document).** No pick-up or mixing of
//! canvas paint (the part of a real Painter brush that would cost most), no
//! per-bristle stiffness response to pressure, no bristle collision, bristles
//! deposit independently so the stroke composites through the shared stroke
//! coverage of the soft Brush, not a wet layer.

#![allow(
    dead_code,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use aurora_tile::{SurfaceId, TileStore};

use crate::deposit::{Mode, SoftDab, deposit_soft_dabs_with};
use crate::{PaintAmount, StrokeSnapshot, stamp_dab_amount};

/// Fewest bristles a brush is built with.
pub const MIN_BRISTLES: usize = 16;
/// Most bristles a brush is built with.
pub const MAX_BRISTLES: usize = 1024;
/// A bristle's dab spacing as a fraction of its own radius (floored at 1 px).
pub const BRISTLE_STEP_RADII: f32 = 0.5;
/// Most dabs one bristle places per pen sample: a wild pen jump cannot make
/// a sample cost without bound.
pub const MAX_DABS_PER_BRISTLE_SAMPLE: usize = 64;
/// Pen coordinates are clamped to `±COORD_LIMIT` px.
pub const COORD_LIMIT: f32 = 1.0e6;
/// Smallest spread fraction (pressure just above 0).
const SPREAD_MIN: f32 = 0.35;
/// Shift of the footprint centre at full tilt, in brush radii.
const TILT_SHIFT_RADII: f32 = 0.6;
/// Elongation along the tilt direction at full tilt.
const TILT_STRETCH: f32 = 1.5;
/// Compression across the tilt direction at full tilt.
const TILT_SQUASH: f32 = 0.3;
/// Smallest bristle radius, px.
const MIN_BRISTLE_RADIUS: f32 = 0.75;
/// A bristle with less load than this deposits nothing.
const MIN_LOAD: f32 = 0.01;

/// One pen sample. `tilt_x`/`tilt_y` are normalised to `-1..=1` (+-90 deg),
/// `rotation` is the barrel twist in radians. Out-of-range values are clamped
/// by [`PenSample::sanitised`]; non-finite ones refuse the sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PenSample {
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
    pub tilt_x: f32,
    pub tilt_y: f32,
    pub rotation: f32,
}

impl PenSample {
    /// The sample with every field clamped to its range, or `None` when any
    /// field is NaN or infinite (a broken device report is dropped, not
    /// guessed at).
    #[must_use]
    pub fn sanitised(self) -> Option<Self> {
        let all = [
            self.x,
            self.y,
            self.pressure,
            self.tilt_x,
            self.tilt_y,
            self.rotation,
        ];
        if !all.iter().all(|v| v.is_finite()) {
            return None;
        }
        Some(Self {
            x: self.x.clamp(-COORD_LIMIT, COORD_LIMIT),
            y: self.y.clamp(-COORD_LIMIT, COORD_LIMIT),
            pressure: self.pressure.clamp(0.0, 1.0),
            tilt_x: self.tilt_x.clamp(-1.0, 1.0),
            tilt_y: self.tilt_y.clamp(-1.0, 1.0),
            rotation: self.rotation.rem_euclid(std::f32::consts::TAU),
        })
    }
}

/// Brush settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BristleConfig {
    /// Bristle count, clamped to `MIN_BRISTLES..=MAX_BRISTLES`.
    pub count: usize,
    /// Brush radius in px (layout spread at full pressure), clamped >= 1.
    pub radius: f32,
    /// Seed of the layout, stiffness and colour jitter.
    pub seed: u64,
    /// Straight sRGB paint colour.
    pub colour: [f32; 3],
    /// Dab flow before pressure and load, `0.01..=1`.
    pub flow: f32,
    /// How much bristles trail the pen, `0` rigid .. `0.95`.
    pub lag: f32,
    /// Stroke length (in brush radii) over which a full load runs out.
    pub load_length_radii: f32,
}

impl BristleConfig {
    /// A config with the defaults the spike measured: red paint, flow 0.6,
    /// lag 0.6, a load that lasts 60 radii.
    #[must_use]
    pub fn new(count: usize, radius: f32) -> Self {
        Self {
            count,
            radius,
            seed: 0x05ee_db15_71e5,
            colour: [0.8, 0.1, 0.1],
            flow: 0.6,
            lag: 0.6,
            load_length_radii: 60.0,
        }
    }
}

/// splitmix64: a tiny deterministic generator (no wall clock, no OS entropy).
struct SplitMix(u64);

impl SplitMix {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)`.
    fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1_u64 << 24) as f32
    }
}

#[derive(Debug, Clone, Copy)]
struct Bristle {
    /// Offset on the unit disc.
    offset: (f32, f32),
    stiffness: f32,
    colour: [f32; 3],
    load: f32,
    pos: (f32, f32),
    /// Distance travelled since the last dab, px.
    carry: f32,
}

/// One dab a bristle wants stamped, produced by [`BristleBrush::advance`] and
/// consumed by [`BristleBrush::deposit`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BristleDab {
    pub center: (f32, f32),
    pub radius: f32,
    pub colour: [f32; 3],
    /// Flow in `0..=1` (already times pressure and load).
    pub flow: f32,
}

/// What a deposit pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DepositStats {
    pub dabs: usize,
    pub failed: usize,
}

/// The bristle brush for one stroke.
#[derive(Debug, Clone)]
pub struct BristleBrush {
    config: BristleConfig,
    bristle_radius: f32,
    bristles: Vec<Bristle>,
    pending: Vec<BristleDab>,
    started: bool,
}

impl BristleBrush {
    /// A brush for a new stroke. `config.count` and `config.radius` are
    /// clamped; non-finite settings fall back to the defaults.
    #[must_use]
    pub fn new(config: BristleConfig) -> Self {
        let defaults = BristleConfig::new(config.count, config.radius);
        let finite = |v: f32, d: f32| if v.is_finite() { v } else { d };
        let config = BristleConfig {
            count: config.count.clamp(MIN_BRISTLES, MAX_BRISTLES),
            radius: finite(config.radius, 12.0).clamp(1.0, 4096.0),
            seed: config.seed,
            colour: [
                finite(config.colour[0], defaults.colour[0]).clamp(0.0, 1.0),
                finite(config.colour[1], defaults.colour[1]).clamp(0.0, 1.0),
                finite(config.colour[2], defaults.colour[2]).clamp(0.0, 1.0),
            ],
            flow: finite(config.flow, defaults.flow).clamp(0.01, 1.0),
            lag: finite(config.lag, defaults.lag).clamp(0.0, 0.95),
            load_length_radii: finite(config.load_length_radii, defaults.load_length_radii)
                .clamp(1.0, 1.0e6),
        };
        let mut rng = SplitMix(config.seed);
        let n = config.count;
        let golden = std::f32::consts::PI * (3.0 - 5.0_f32.sqrt());
        let bristles = (0..n)
            .map(|i| {
                // Golden-angle spiral covers the disc evenly; jitter breaks
                // the regularity so bristles are not on a visible lattice.
                let t = (i as f32 + 0.5) / n as f32;
                let r = t.sqrt() * (0.9 + 0.1 * rng.unit());
                let a = i as f32 * golden + 0.3 * rng.unit();
                let shade = 0.92 + 0.16 * rng.unit();
                Bristle {
                    offset: (r * a.cos(), r * a.sin()),
                    stiffness: 0.3 + 0.7 * rng.unit(),
                    colour: [
                        (config.colour[0] * shade).min(1.0),
                        (config.colour[1] * shade).min(1.0),
                        (config.colour[2] * shade).min(1.0),
                    ],
                    load: 1.0,
                    pos: (0.0, 0.0),
                    carry: 0.0,
                }
            })
            .collect();
        let bristle_radius = (1.6 * config.radius / (n as f32).sqrt())
            .max(MIN_BRISTLE_RADIUS)
            .min(config.radius);
        Self {
            config,
            bristle_radius,
            bristles,
            pending: Vec::new(),
            started: false,
        }
    }

    /// The (clamped) settings in force.
    #[must_use]
    pub const fn config(&self) -> &BristleConfig {
        &self.config
    }

    /// Radius of one bristle's dab at full pressure, px.
    #[must_use]
    pub const fn bristle_radius(&self) -> f32 {
        self.bristle_radius
    }

    /// Current bristle positions.
    pub fn positions(&self) -> impl Iterator<Item = (f32, f32)> + '_ {
        self.bristles.iter().map(|b| b.pos)
    }

    /// Current paint loads.
    pub fn loads(&self) -> impl Iterator<Item = f32> + '_ {
        self.bristles.iter().map(|b| b.load)
    }

    /// The dabs the last [`Self::advance`] produced.
    #[must_use]
    pub fn pending(&self) -> &[BristleDab] {
        &self.pending
    }

    /// The target of every bristle for `sample` (already sanitised).
    fn targets(&self, sample: &PenSample) -> impl Iterator<Item = (f32, f32)> + '_ {
        let radius = self.config.radius;
        let spread = radius * (SPREAD_MIN + (1.0 - SPREAD_MIN) * sample.pressure);
        let (tx, ty) = (sample.tilt_x, sample.tilt_y);
        let mag = tx.hypot(ty).min(1.0);
        // Unit tilt direction; any direction when untilted (m = 0 makes it moot).
        let (dx, dy) = if mag > 1.0e-6 {
            (tx / tx.hypot(ty), ty / tx.hypot(ty))
        } else {
            (1.0, 0.0)
        };
        let long = 1.0 + TILT_STRETCH * mag;
        let short = 1.0 - TILT_SQUASH * mag;
        let shift = TILT_SHIFT_RADII * radius * mag;
        let (cx, cy) = (sample.x + dx * shift, sample.y + dy * shift);
        let (sin_r, cos_r) = sample.rotation.sin_cos();
        self.bristles.iter().map(move |b| {
            // Rotate the layout, then stretch along the tilt direction.
            let rx = b.offset.0 * cos_r - b.offset.1 * sin_r;
            let ry = b.offset.0 * sin_r + b.offset.1 * cos_r;
            let along = rx * dx + ry * dy;
            let across = -rx * dy + ry * dx;
            let (along, across) = (along * long, across * short);
            (
                cx + (along * dx - across * dy) * spread,
                cy + (along * dy + across * dx) * spread,
            )
        })
    }

    /// Moves every bristle toward its target for `sample` and fills the
    /// pending dab list (replacing the last one). Returns `false`, changing
    /// nothing, when the sample is refused (non-finite field). Cheap, no
    /// tile access: this is the part a GPU port would replace.
    pub fn advance(&mut self, sample: PenSample) -> bool {
        let Some(sample) = sample.sanitised() else {
            return false;
        };
        self.pending.clear();
        let targets: Vec<(f32, f32)> = self.targets(&sample).collect();
        let first = !self.started;
        self.started = true;
        let lag = self.config.lag;
        let radius = self.config.radius;
        let bristle_radius = self.bristle_radius * (0.6 + 0.4 * sample.pressure);
        let step = (bristle_radius * BRISTLE_STEP_RADII).max(1.0);
        let load_per_px = 1.0 / (self.config.load_length_radii * radius);
        let flow = self.config.flow * sample.pressure;
        let depositing = sample.pressure > 0.0;
        let pending = &mut self.pending;
        for (b, target) in self.bristles.iter_mut().zip(targets) {
            if first {
                b.pos = target;
                if depositing && b.load >= MIN_LOAD {
                    pending.push(BristleDab {
                        center: b.pos,
                        radius: bristle_radius,
                        colour: b.colour,
                        flow: (flow * b.load).clamp(0.0, 1.0),
                    });
                }
                continue;
            }
            let k = (1.0 - lag * (1.0 - 0.5 * b.stiffness)).clamp(0.05, 1.0);
            let prev = b.pos;
            b.pos = (
                prev.0 + (target.0 - prev.0) * k,
                prev.1 + (target.1 - prev.1) * k,
            );
            let (dx, dy) = (b.pos.0 - prev.0, b.pos.1 - prev.1);
            let len = dx.hypot(dy);
            if len <= f32::EPSILON {
                continue;
            }
            // Dabs along this bristle's own path, spacing carried across
            // samples, at most MAX_DABS_PER_BRISTLE_SAMPLE.
            let (ux, uy) = (dx / len, dy / len);
            let mut travelled = 0.0_f32;
            let mut carry = b.carry;
            let mut placed = 0;
            while carry + (len - travelled) >= step && placed < MAX_DABS_PER_BRISTLE_SAMPLE {
                travelled += (step - carry).max(0.0);
                carry = 0.0;
                placed += 1;
                // The load runs out along the path actually travelled.
                b.load = (b.load - step * load_per_px).max(0.0);
                if depositing && b.load >= MIN_LOAD {
                    pending.push(BristleDab {
                        center: (prev.0 + ux * travelled, prev.1 + uy * travelled),
                        radius: bristle_radius,
                        colour: b.colour,
                        flow: (flow * b.load).clamp(0.0, 1.0),
                    });
                }
            }
            b.carry = if placed >= MAX_DABS_PER_BRISTLE_SAMPLE {
                0.0
            } else {
                carry + (len - travelled)
            };
        }
        true
    }

    /// Stamps the pending dabs onto `surface` of `store` with the batch
    /// deposit ([`crate::deposit::deposit_soft_dabs`], task 170): the same
    /// tile bits, stroke coverage and undo capture as
    /// [`Self::deposit_per_dab`], tile by tile.
    pub fn deposit(
        &mut self,
        store: &mut TileStore,
        surface: SurfaceId,
        snapshot: Option<&mut StrokeSnapshot>,
    ) -> DepositStats {
        self.deposit_with(store, surface, snapshot, Mode::Auto)
    }

    /// [`Self::deposit`] with the batch deposit's scheduling chosen (the
    /// measurement compares [`Mode::Serial`] and [`Mode::Auto`]).
    pub fn deposit_with(
        &mut self,
        store: &mut TileStore,
        surface: SurfaceId,
        snapshot: Option<&mut StrokeSnapshot>,
        mode: Mode,
    ) -> DepositStats {
        let dabs: Vec<SoftDab> = self
            .pending
            .drain(..)
            .map(|dab| SoftDab {
                center: dab.center,
                radius: dab.radius,
                colour: dab.colour,
                amount: PaintAmount::new(1.0, dab.flow),
            })
            .collect();
        let outcome = deposit_soft_dabs_with(store, surface, &dabs, snapshot, mode);
        DepositStats {
            dabs: dabs.len(),
            failed: outcome.failed_dabs,
        }
    }

    /// The task 168 spike's deposit: every pending dab through the existing
    /// soft-Brush path ([`crate::stamp_dab_amount`]) one at a time, capturing
    /// undo and stroke coverage in `snapshot`. Kept as the reference the
    /// batch deposit is compared and measured against.
    pub fn deposit_per_dab(
        &mut self,
        store: &mut TileStore,
        surface: SurfaceId,
        mut snapshot: Option<&mut StrokeSnapshot>,
    ) -> DepositStats {
        let mut stats = DepositStats::default();
        for dab in self.pending.drain(..) {
            let outcome = stamp_dab_amount(
                store,
                surface,
                dab.center,
                dab.radius,
                dab.colour,
                snapshot.as_deref_mut(),
                None,
                false,
                false,
                PaintAmount::new(1.0, dab.flow),
            );
            stats.dabs += 1;
            if !outcome.is_complete() {
                stats.failed += 1;
            }
        }
        stats
    }

    /// [`Self::advance`] then [`Self::deposit`]: one pen sample.
    pub fn step(
        &mut self,
        sample: PenSample,
        store: &mut TileStore,
        surface: SurfaceId,
        snapshot: Option<&mut StrokeSnapshot>,
    ) -> DepositStats {
        if self.advance(sample) {
            self.deposit(store, surface, snapshot)
        } else {
            DepositStats::default()
        }
    }

    /// The largest distance from a bristle to its target for `sample`
    /// (tests: convergence once the pen stops).
    #[must_use]
    pub fn max_lag(&self, sample: PenSample) -> f32 {
        let Some(sample) = sample.sanitised() else {
            return f32::NAN;
        };
        self.targets(&sample)
            .zip(&self.bristles)
            .map(|(t, b)| (t.0 - b.pos.0).hypot(t.1 - b.pos.1))
            .fold(0.0, f32::max)
    }
}
