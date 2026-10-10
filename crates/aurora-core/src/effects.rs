//! Layer effects (layer styles), 0.180.0: Photoshop's drop shadow, colour
//! overlay, stroke and outer glow as typed, non-destructive parameters,
//! plus a count of every other kind so it can be reported.
//!
//! It lives in `aurora-core`, beside [`crate::CurvesParams`], for the same
//! layering reason (`scripts/layering.json`): `aurora-doc` stores it on a
//! layer, `aurora-io` reads it from a PSD, and the effect maths (from
//! round E2 on) belongs in `aurora-filters`, which may not depend on
//! `aurora-doc`. That is also why an effect's blend mode is its own
//! [`EffectBlendMode`] rather than `aurora_doc::BlendMode`: the doc crate
//! maps between them (`From<EffectBlendMode> for BlendMode`).
//!
//! **Data only, never baked** (invariant §7.3.2): nothing here touches
//! pixels. 0.180.0 renders none of it; rendering starts in E2.
//!
//! **Gating.** Photoshop shows an effect only when the layer's master
//! switch ([`LayerEffects::enabled`], `masterFXSwitch`), the effect's
//! `present` and its `enab` are all on. All three are kept as data
//! ([`EffectGate`]), never collapsed, so turning the master switch back
//! on restores exactly what was there.
//!
//! **Wire format** (`.aur`'s `layer-effects` entry and the history
//! journal's `SetEffects` op, both positional `postcard`): the fields in
//! declaration order. Positional means a field can never be added to
//! these types without breaking every file that holds one: a later
//! version needs new types and a new entry/op version, which the
//! `layer-effects` entry's own version number exists for.
//!
//! **Validation** ([`LayerEffects::validate`]): at most
//! [`MAX_EFFECT_INSTANCES`] per kind, every number finite and inside
//! Photoshop's own range. Every public path that stores one into a
//! document checks it (`aurora_doc::LayerTree::set_effects`), so an
//! out-of-range value decoded from a hostile file is refused, never
//! stored.

/// The most instances of one kind a layer may carry — Photoshop's own
/// limit for the kinds that allow several (drop shadow, inner shadow,
/// colour/gradient overlay, stroke).
pub const MAX_EFFECT_INSTANCES: usize = 10;

/// The largest effect size (blur radius, stroke width) in pixels —
/// Photoshop's own slider maximum.
pub const MAX_EFFECT_SIZE: f32 = 250.0;

/// The largest drop-shadow distance in pixels — Photoshop's own maximum.
pub const MAX_EFFECT_DISTANCE: f32 = 30_000.0;

/// The largest layer-style scale, in percent (Photoshop: 1–1000 %).
pub const MAX_EFFECT_SCALE: f32 = 1000.0;

/// Photoshop's global light angle when a file stores none (image resource
/// 1037 absent), in degrees. Photoshop's own new-document default.
/// **Unverified** against a Photoshop file that lacks the resource: every
/// file in the corpus stores one.
pub const DEFAULT_GLOBAL_ANGLE: f32 = 120.0;

/// Why a [`LayerEffects`] value is refused.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum EffectsError {
    #[error("{count} {kind} effects on one layer; at most {MAX_EFFECT_INSTANCES} are allowed")]
    TooManyInstances { kind: &'static str, count: usize },
    #[error("effect setting {field} is not a finite number within its range")]
    OutOfRange { field: &'static str },
}

/// The blend mode of one effect: Photoshop's 27 layer modes, in the same
/// order as `aurora_doc::BlendMode` (which adds the group-only Pass
/// Through). Appended last if ever extended, like every `postcard` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum EffectBlendMode {
    #[default]
    Normal,
    Dissolve,
    Darken,
    Multiply,
    ColorBurn,
    LinearBurn,
    DarkerColor,
    Lighten,
    Screen,
    ColorDodge,
    LinearDodge,
    LighterColor,
    Overlay,
    SoftLight,
    HardLight,
    VividLight,
    LinearLight,
    PinLight,
    HardMix,
    Difference,
    Exclusion,
    Subtract,
    Divide,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl EffectBlendMode {
    /// Every mode, in declaration order.
    pub const ALL: [Self; 27] = [
        Self::Normal,
        Self::Dissolve,
        Self::Darken,
        Self::Multiply,
        Self::ColorBurn,
        Self::LinearBurn,
        Self::DarkerColor,
        Self::Lighten,
        Self::Screen,
        Self::ColorDodge,
        Self::LinearDodge,
        Self::LighterColor,
        Self::Overlay,
        Self::SoftLight,
        Self::HardLight,
        Self::VividLight,
        Self::LinearLight,
        Self::PinLight,
        Self::HardMix,
        Self::Difference,
        Self::Exclusion,
        Self::Subtract,
        Self::Divide,
        Self::Hue,
        Self::Saturation,
        Self::Color,
        Self::Luminosity,
    ];
}

/// An effect colour, straight sRGB-encoded, each channel `0.0..=1.0`.
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct EffectColor {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

/// The two per-effect switches Photoshop stores: `present` (the effect
/// is in the layer's style at all) and `enab` (its checkbox).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EffectGate {
    pub present: bool,
    pub enabled: bool,
}

impl Default for EffectGate {
    fn default() -> Self {
        Self {
            present: true,
            enabled: true,
        }
    }
}

impl EffectGate {
    /// Both switches on. The master switch is the layer's, see
    /// [`LayerEffects::shows`].
    #[must_use]
    pub fn on(self) -> bool {
        self.present && self.enabled
    }
}

/// A drop shadow (`DrSh`).
// Each bool is a separate Photoshop field, kept one-to-one for E7's write-back.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DropShadow {
    pub gate: EffectGate,
    pub blend_mode: EffectBlendMode,
    pub color: EffectColor,
    /// `0.0..=1.0` (the file's percent / 100).
    pub opacity: f32,
    /// `uglg`: the shadow follows the document's global light.
    pub use_global_light: bool,
    /// The angle the shadow uses, in degrees, `-360.0..=360.0`: the
    /// global light angle when [`Self::use_global_light`] (resolved when
    /// the file was read), else the layer's own `lagl`.
    pub angle: f32,
    /// The layer's own angle (`lagl`), kept for when the shadow stops
    /// following the global light.
    pub local_angle: f32,
    /// `Dstn`, pixels, `0.0..=MAX_EFFECT_DISTANCE`.
    pub distance: f32,
    /// `Ckmt` (spread), `0.0..=1.0`.
    pub spread: f32,
    /// `blur` (size), pixels, `0.0..=MAX_EFFECT_SIZE`.
    pub size: f32,
    /// `Nose`, `0.0..=1.0`.
    pub noise: f32,
    pub anti_aliased: bool,
    /// The contour (`TrnS`) is the two-point linear curve.
    pub linear_contour: bool,
    /// `layerConceals`: the layer knocks out its own shadow.
    pub knocks_out: bool,
}

impl Default for DropShadow {
    fn default() -> Self {
        Self {
            gate: EffectGate::default(),
            blend_mode: EffectBlendMode::Multiply,
            color: EffectColor::default(),
            opacity: 0.75,
            use_global_light: true,
            angle: DEFAULT_GLOBAL_ANGLE,
            local_angle: DEFAULT_GLOBAL_ANGLE,
            distance: 5.0,
            spread: 0.0,
            size: 5.0,
            noise: 0.0,
            anti_aliased: false,
            linear_contour: true,
            knocks_out: true,
        }
    }
}

/// A colour overlay (`SoFi`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ColorOverlay {
    pub gate: EffectGate,
    pub blend_mode: EffectBlendMode,
    pub color: EffectColor,
    /// `0.0..=1.0`.
    pub opacity: f32,
}

impl Default for ColorOverlay {
    fn default() -> Self {
        Self {
            gate: EffectGate::default(),
            blend_mode: EffectBlendMode::Normal,
            color: EffectColor {
                r: 1.0,
                g: 0.0,
                b: 0.0,
            },
            opacity: 1.0,
        }
    }
}

/// Where a stroke sits relative to the layer's edge (`Styl`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum StrokePosition {
    #[default]
    Outside,
    Inside,
    Center,
}

/// What a stroke is filled with (`PntT`). Only `Color` is ever rendered
/// (from E4); the other two are kept so they can be reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum StrokeFill {
    #[default]
    Color,
    Gradient,
    Pattern,
}

/// A stroke (`FrFX`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Stroke {
    pub gate: EffectGate,
    pub position: StrokePosition,
    pub fill: StrokeFill,
    pub blend_mode: EffectBlendMode,
    /// `0.0..=1.0`.
    pub opacity: f32,
    /// `Sz  `, pixels, `0.0..=MAX_EFFECT_SIZE`.
    pub size: f32,
    pub color: EffectColor,
}

impl Default for Stroke {
    fn default() -> Self {
        Self {
            gate: EffectGate::default(),
            position: StrokePosition::Outside,
            fill: StrokeFill::Color,
            blend_mode: EffectBlendMode::Normal,
            opacity: 1.0,
            size: 3.0,
            color: EffectColor::default(),
        }
    }
}

/// An outer glow (`OrGl`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OuterGlow {
    pub gate: EffectGate,
    pub blend_mode: EffectBlendMode,
    pub color: EffectColor,
    /// `0.0..=1.0`.
    pub opacity: f32,
    /// `GlwT` is `PrBL` (Precise) rather than Softer.
    pub precise: bool,
    /// `Ckmt` (spread), `0.0..=1.0`.
    pub spread: f32,
    /// `blur` (size), pixels, `0.0..=MAX_EFFECT_SIZE`.
    pub size: f32,
    /// `Nose`, `0.0..=1.0`.
    pub noise: f32,
}

impl Default for OuterGlow {
    fn default() -> Self {
        Self {
            gate: EffectGate::default(),
            blend_mode: EffectBlendMode::Screen,
            color: EffectColor {
                r: 1.0,
                g: 1.0,
                b: 190.0 / 255.0,
            },
            opacity: 0.75,
            precise: false,
            spread: 0.0,
            size: 5.0,
            noise: 0.0,
        }
    }
}

/// How many effects of each kind Aurora does not model yet, counting
/// only instances whose own `present` and `enab` are both on. Kept so a
/// report can name them; their settings are not kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct OtherEffects {
    pub inner_shadows: u32,
    pub inner_glows: u32,
    pub bevels: u32,
    pub satins: u32,
    pub gradient_overlays: u32,
    pub pattern_overlays: u32,
}

impl OtherEffects {
    /// All of them together.
    #[must_use]
    pub fn total(&self) -> u64 {
        [
            self.inner_shadows,
            self.inner_glows,
            self.bevels,
            self.satins,
            self.gradient_overlays,
            self.pattern_overlays,
        ]
        .iter()
        .map(|n| u64::from(*n))
        .sum()
    }
}

/// One layer's effects. See the module docs.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LayerEffects {
    /// The master switch (`masterFXSwitch`): off hides every effect.
    pub enabled: bool,
    /// The style's scale (`Scl `), percent, `0.0..=MAX_EFFECT_SCALE`.
    pub scale: f32,
    pub drop_shadows: Vec<DropShadow>,
    pub color_overlays: Vec<ColorOverlay>,
    pub strokes: Vec<Stroke>,
    pub outer_glows: Vec<OuterGlow>,
    pub other: OtherEffects,
}

impl Default for LayerEffects {
    fn default() -> Self {
        Self {
            enabled: true,
            scale: 100.0,
            drop_shadows: Vec::new(),
            color_overlays: Vec::new(),
            strokes: Vec::new(),
            outer_glows: Vec::new(),
            other: OtherEffects::default(),
        }
    }
}

fn in_range(value: f32, lo: f32, hi: f32, field: &'static str) -> Result<(), EffectsError> {
    if value.is_finite() && (lo..=hi).contains(&value) {
        Ok(())
    } else {
        Err(EffectsError::OutOfRange { field })
    }
}

fn unit(value: f32, field: &'static str) -> Result<(), EffectsError> {
    in_range(value, 0.0, 1.0, field)
}

fn color(c: EffectColor) -> Result<(), EffectsError> {
    unit(c.r, "colour")?;
    unit(c.g, "colour")?;
    unit(c.b, "colour")
}

fn count<T>(items: &[T], kind: &'static str) -> Result<(), EffectsError> {
    if items.len() > MAX_EFFECT_INSTANCES {
        Err(EffectsError::TooManyInstances {
            kind,
            count: items.len(),
        })
    } else {
        Ok(())
    }
}

impl LayerEffects {
    /// Checks every cap and range in the module docs.
    ///
    /// # Errors
    ///
    /// The first [`EffectsError`] found.
    pub fn validate(&self) -> Result<(), EffectsError> {
        in_range(self.scale, 0.0, MAX_EFFECT_SCALE, "scale")?;
        count(&self.drop_shadows, "drop shadow")?;
        count(&self.color_overlays, "colour overlay")?;
        count(&self.strokes, "stroke")?;
        count(&self.outer_glows, "outer glow")?;
        for s in &self.drop_shadows {
            color(s.color)?;
            unit(s.opacity, "opacity")?;
            in_range(s.angle, -360.0, 360.0, "angle")?;
            in_range(s.local_angle, -360.0, 360.0, "angle")?;
            in_range(s.distance, 0.0, MAX_EFFECT_DISTANCE, "distance")?;
            unit(s.spread, "spread")?;
            in_range(s.size, 0.0, MAX_EFFECT_SIZE, "size")?;
            unit(s.noise, "noise")?;
        }
        for o in &self.color_overlays {
            color(o.color)?;
            unit(o.opacity, "opacity")?;
        }
        for s in &self.strokes {
            color(s.color)?;
            unit(s.opacity, "opacity")?;
            in_range(s.size, 0.0, MAX_EFFECT_SIZE, "size")?;
        }
        for g in &self.outer_glows {
            color(g.color)?;
            unit(g.opacity, "opacity")?;
            unit(g.spread, "spread")?;
            in_range(g.size, 0.0, MAX_EFFECT_SIZE, "size")?;
            unit(g.noise, "noise")?;
        }
        Ok(())
    }

    /// Whether an effect with this gate is shown: the master switch and
    /// both of its own.
    #[must_use]
    pub fn shows(&self, gate: EffectGate) -> bool {
        self.enabled && gate.on()
    }

    /// The drop shadows Photoshop would draw.
    #[must_use]
    pub fn shown_drop_shadows(&self) -> usize {
        self.drop_shadows
            .iter()
            .filter(|e| self.shows(e.gate))
            .count()
    }

    /// The colour overlays Photoshop would draw.
    #[must_use]
    pub fn shown_color_overlays(&self) -> usize {
        self.color_overlays
            .iter()
            .filter(|e| self.shows(e.gate))
            .count()
    }

    /// The strokes Photoshop would draw.
    #[must_use]
    pub fn shown_strokes(&self) -> usize {
        self.strokes.iter().filter(|e| self.shows(e.gate)).count()
    }

    /// The outer glows Photoshop would draw.
    #[must_use]
    pub fn shown_outer_glows(&self) -> usize {
        self.outer_glows
            .iter()
            .filter(|e| self.shows(e.gate))
            .count()
    }

    /// The not-yet-modelled effects Photoshop would draw.
    #[must_use]
    pub fn shown_other(&self) -> u64 {
        if self.enabled { self.other.total() } else { 0 }
    }

    /// Whether any effect at all would be drawn — what refuses the GPU
    /// fast path (0.180.0, inert until E2 renders effects).
    #[must_use]
    pub fn shows_any(&self) -> bool {
        self.shown_drop_shadows() > 0
            || self.shown_color_overlays() > 0
            || self.shown_strokes() > 0
            || self.shown_outer_glows() > 0
            || self.shown_other() > 0
    }

    /// Whether there is nothing at all to keep (no instance of any kind).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.drop_shadows.is_empty()
            && self.color_overlays.is_empty()
            && self.strokes.is_empty()
            && self.outer_glows.is_empty()
            && self.other.total() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_refuses_each_cap_and_range() {
        assert_eq!(LayerEffects::default().validate(), Ok(()));
        let mut fx = LayerEffects {
            drop_shadows: vec![DropShadow::default(); MAX_EFFECT_INSTANCES],
            ..LayerEffects::default()
        };
        assert_eq!(fx.validate(), Ok(()));
        fx.drop_shadows.push(DropShadow::default());
        assert!(matches!(
            fx.validate(),
            Err(EffectsError::TooManyInstances { .. })
        ));
        for bad in [f32::NAN, f32::INFINITY, -1.0, MAX_EFFECT_SIZE + 1.0] {
            let mut fx = LayerEffects::default();
            fx.drop_shadows.push(DropShadow {
                size: bad,
                ..DropShadow::default()
            });
            assert!(fx.validate().is_err(), "{bad}");
        }
        let mut fx = LayerEffects::default();
        fx.drop_shadows.push(DropShadow {
            distance: MAX_EFFECT_DISTANCE + 1.0,
            ..DropShadow::default()
        });
        assert!(fx.validate().is_err());
    }

    #[test]
    fn gating_needs_the_master_switch_present_and_enabled() {
        let mut fx = LayerEffects::default();
        fx.drop_shadows.push(DropShadow::default());
        assert!(fx.shows_any());
        for gate in [
            EffectGate {
                present: false,
                enabled: true,
            },
            EffectGate {
                present: true,
                enabled: false,
            },
        ] {
            let mut off = fx.clone();
            if let Some(s) = off.drop_shadows.first_mut() {
                s.gate = gate;
            }
            assert!(!off.shows_any());
        }
        fx.enabled = false;
        assert!(!fx.shows_any());
        assert!(!fx.is_empty(), "turned off, but still kept");
    }

    #[test]
    fn layer_effects_round_trip_through_postcard() {
        let mut fx = LayerEffects::default();
        fx.drop_shadows.push(DropShadow::default());
        fx.strokes.push(Stroke::default());
        fx.other.bevels = 2;
        let bytes = postcard::to_allocvec(&fx).unwrap_or_default();
        let back: Result<LayerEffects, _> = postcard::from_bytes(&bytes);
        assert_eq!(back.ok(), Some(fx));
    }
}
