//! Flow and Opacity for the paint tools (0.228.0).
//!
//! **The model (a choice, not compared with Photoshop).** A paint dab has a
//! per-dab alpha `a` (the soft falloff, or 1 for the Pencil and the Clone
//! Stamp's source weight). The two settings are two different things:
//!
//! * **Flow** scales the dab's amount: `a' = a * flow`. How marks add up
//!   along a stroke is the tool's own dab rule. Since 0.230.0 the soft Brush
//!   accumulates them (stroke compositing, `stroke_cover`: `A' = A +
//!   (opacity - A) * a'`), so a faint Flow builds up toward the Opacity
//!   ceiling dab by dab (before 0.230.0 its max-alpha rule made Flow only a
//!   per-dab alpha ceiling). The transparency-locked Brush and the Clone
//!   Stamp also build up.
//! * **Opacity** is a per-**stroke** ceiling: the stroke never takes a texel
//!   further than `opacity` of the way from the content it had when the
//!   stroke began (`start`, the stroke snapshot's raw copy; a tile the
//!   stroke has not written yet is still its own start). With `painted` the
//!   texel the stroke would have produced at opacity 100 %,
//!   `stored = start + (painted - start) * opacity`, per premultiplied
//!   channel. A stroke that crosses itself therefore cannot darken beyond
//!   the cap. For the soft Brush the ceiling is the maximum of the stroke
//!   coverage `A` itself, and `stored = start * (1 - A) + colour * A`.
//!
//! The Pencil (a hard dab) and the Clone Stamp keep the pre-0.230.0 rules:
//! the Pencil evaluates the limited texel `start + (painted - start) *
//! opacity`, and for rules that move the stored texel toward a target by
//! `a'` (Clone Stamp), moving toward the limited target `start + (target -
//! start) * opacity` is exactly the limit of moving the painted value
//! toward `target` (the two interpolations commute).
//!
//! **Opacity 100 % and Flow 100 % take the pre-0.228.0 code for the Pencil
//! and the Clone Stamp, bit for bit** (pinned by hashes taken from the
//! 0.229.0 code). With no stroke snapshot the stroke start is the live texel
//! and the coverage starts at 0, so every dab is its own stroke.

/// The smallest Opacity or Flow the engine accepts (1 %). Anything lower,
/// and a non-finite value, is clamped.
pub const MIN_AMOUNT: f32 = 0.01;

/// A stroke's Flow and Opacity, each in `[MIN_AMOUNT, 1]` (1 = 100 %).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaintAmount {
    opacity: f32,
    flow: f32,
}

fn sanitised(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(MIN_AMOUNT, 1.0)
    } else {
        1.0
    }
}

impl PaintAmount {
    /// Opacity 100 %, Flow 100 %: the unchanged pre-0.228.0 behaviour.
    pub const FULL: Self = Self {
        opacity: 1.0,
        flow: 1.0,
    };

    /// `opacity` and `flow` as fractions (0.01..=1; clamped, a non-finite
    /// value reads as 1).
    #[must_use]
    pub fn new(opacity: f32, flow: f32) -> Self {
        Self {
            opacity: sanitised(opacity),
            flow: sanitised(flow),
        }
    }

    /// The per-stroke opacity ceiling, 0.01..=1.
    #[must_use]
    pub const fn opacity(self) -> f32 {
        self.opacity
    }

    /// The per-dab flow, 0.01..=1.
    #[must_use]
    pub const fn flow(self) -> f32 {
        self.flow
    }

    /// The opacity when it actually limits (below 100 %), else `None`.
    pub(crate) fn cap(self) -> Option<f32> {
        (self.opacity < 1.0).then_some(self.opacity)
    }
}

impl Default for PaintAmount {
    fn default() -> Self {
        Self::FULL
    }
}
