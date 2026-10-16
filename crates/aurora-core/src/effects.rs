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

/// The largest apron, in pixels, a drop shadow may need around a
/// composite tile (0.182.0): dilation radius plus three box-blur radii.
/// Past it the shadow is drawn smaller ([`ShadowGeometry::capped`]). With
/// sizes held to [`MAX_EFFECT_SIZE`] and [`SHADOW_BLUR_RADIUS_PER_PX`] at
/// 0.45 the largest apron a valid shadow needs is 339 px (size 250, no
/// spread), so the cap is a bound that holds whatever the mapping, not one
/// a valid file reaches today. At
/// 384 the source window is `256 + 2 * 384 = 1024` px a side, which spans
/// at most five 256-px tiles per axis, so one shadow on one tile reads at
/// most 25 source tiles.
pub const MAX_SHADOW_APRON: u32 = 384;

/// What the fitted size → blur mapping multiplies the blur part of a
/// shadow's size (pixels) by to get one box-blur pass's radius: three
/// passes of radius `r` approximate a Gaussian of sigma `sqrt(r(r + 1))`,
/// so this is roughly sigma per pixel of size. Fitted on
/// `layer_effects.psd`'s drop shadow (size 41, distance 18) against
/// Photoshop's merged image, over the shadow's region: mean error per
/// channel 9.53/6.99/8.15 levels with no blur, 5.38/2.69/3.93 at 0.40,
/// **5.34/2.69/3.91 at 0.45**, 5.55/2.94/4.14 at 0.50 and 5.77/3.22/4.39
/// at 0.60 (0.182.0; PLAN.md has the table).
pub const SHADOW_BLUR_RADIUS_PER_PX: f32 = 0.45;

/// The one size → blur mapping (0.182.0): the radius, in pixels, of each
/// of the three box-blur passes a shadow's blur of `blur_px` uses.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn shadow_blur_box_radius(blur_px: f32) -> u32 {
    if blur_px.is_finite() && blur_px > 0.0 {
        (blur_px * SHADOW_BLUR_RADIUS_PER_PX)
            .round()
            .min(f32::from(u16::MAX)) as u32
    } else {
        0
    }
}

/// A drop shadow resolved to pixels for drawing (0.182.0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShadowGeometry {
    /// Where the shadow lands relative to the content, document pixels,
    /// y down: `(-d cos θ, d sin θ)`, rounded.
    pub offset: (i64, i64),
    /// The spread's dilation radius, pixels.
    pub choke: u32,
    /// Each of the three box-blur passes' radius, pixels.
    pub blur_radius: u32,
    /// `choke + 3 * blur_radius`: how far the shadow reaches past the
    /// source, never above [`MAX_SHADOW_APRON`].
    pub apron: u32,
    /// The size had to be reduced to fit [`MAX_SHADOW_APRON`].
    pub capped: bool,
}

impl DropShadow {
    /// This shadow in pixels. Size and distance are used as stored: the
    /// style's scale (`Scl `, [`LayerEffects::scale`]) is **not** applied,
    /// measured rather than assumed — `layer_effects.psd` stores `Scl `
    /// 208.33 %, and Photoshop's merged image shows its shadow reaching
    /// about 63 px below the text (distance 18 + size 41), not the ~123 px
    /// a 208 % scale would give; the user's Nyepi file stores 416.67 %
    /// with sizes up to 144. Spread is a fraction of size.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn geometry(&self) -> ShadowGeometry {
        let distance = f64::from(self.distance.max(0.0));
        let theta = f64::from(self.angle).to_radians();
        let offset = (
            (-distance * theta.cos()).round() as i64,
            (distance * theta.sin()).round() as i64,
        );
        spread_blur_geometry(self.size, self.spread, SHADOW_BLUR_RADIUS_PER_PX, offset)
    }
}

/// The geometry a shadow and an outer glow share (0.191.0): a spread
/// (choke) of `size * spread` pixels and a blur of the rest, three box
/// passes of `round(per_px * size * (1 - spread))`, reduced to fit
/// [`MAX_SHADOW_APRON`].
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn spread_blur_geometry(size: f32, spread: f32, per_px: f32, offset: (i64, i64)) -> ShadowGeometry {
    let size = size.max(0.0);
    let spread = spread.clamp(0.0, 1.0);
    let mut choke = (size * spread).round().min(f32::from(u16::MAX)) as u32;
    let blur = size * (1.0 - spread);
    let mut blur_radius = if blur.is_finite() && blur > 0.0 {
        (blur * per_px).round().min(f32::from(u16::MAX)) as u32
    } else {
        0
    };
    let wanted = choke.saturating_add(blur_radius.saturating_mul(3));
    let capped = wanted > MAX_SHADOW_APRON;
    if capped {
        let f = f64::from(MAX_SHADOW_APRON) / f64::from(wanted);
        choke = (f64::from(choke) * f).floor() as u32;
        blur_radius = (f64::from(blur_radius) * f).floor() as u32;
    }
    ShadowGeometry {
        offset,
        choke,
        blur_radius,
        apron: choke + 3 * blur_radius,
        capped,
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

/// A stroke resolved to pixels for drawing (0.190.0).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StrokeGeometry {
    /// How far the stroke reaches outside the layer's edge, pixels.
    pub outside: f32,
    /// How far it reaches inside the edge, pixels.
    pub inside: f32,
    /// The window margin a tile needs to draw it exactly: the larger
    /// reach, rounded up, plus one pixel for the edge's anti-aliasing,
    /// never above [`MAX_SHADOW_APRON`].
    pub apron: u32,
    /// The size had to be reduced to fit [`MAX_SHADOW_APRON`].
    pub capped: bool,
}

impl Stroke {
    /// This stroke in pixels. Outside reaches `size` past the edge, Inside
    /// `size` within it, Center half of `size` each way. The style's scale
    /// (`Scl `) is not applied, as for a drop shadow (measured there; not
    /// measured for strokes). With `size` held to [`MAX_EFFECT_SIZE`] the
    /// cap is a bound that holds whatever the limits, not one a valid file
    /// reaches.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn geometry(&self) -> StrokeGeometry {
        let size = if self.size.is_finite() {
            self.size.max(0.0)
        } else {
            0.0
        };
        let (mut outside, mut inside) = match self.position {
            StrokePosition::Outside => (size, 0.0),
            StrokePosition::Inside => (0.0, size),
            StrokePosition::Center => (size / 2.0, size / 2.0),
        };
        let room = (MAX_SHADOW_APRON - 1) as f32;
        let capped = outside > room || inside > room;
        if capped {
            outside = outside.min(room);
            inside = inside.min(room);
        }
        let reach = outside.max(inside).ceil() as u32;
        StrokeGeometry {
            outside,
            inside,
            apron: (reach + 1).min(MAX_SHADOW_APRON),
            capped,
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

/// What the glow's blur size → box radius mapping multiplies by
/// (0.191.0). Fitted on `layer_effects.psd`'s Outer Glow (Softer, size 41,
/// Screen white at 35 %) against Photoshop's merged image; PLAN.md has the
/// table.
pub const GLOW_BLUR_RADIUS_PER_PX: f32 = 0.45;

impl OuterGlow {
    /// This glow in pixels (0.191.0): a [`DropShadow`] without an offset.
    /// Size is used as stored (the style's `Scl ` is not applied, as for
    /// shadows); spread is a fraction of size. The apron is capped at
    /// [`MAX_SHADOW_APRON`] like a shadow's.
    #[must_use]
    pub fn geometry(&self) -> ShadowGeometry {
        spread_blur_geometry(self.size, self.spread, GLOW_BLUR_RADIUS_PER_PX, (0, 0))
    }
}

/// The box an aligned gradient overlay is laid over when it is not the
/// layer's own `bounds` (task 110): the layer's PSD record rectangle,
/// kept as an offset of its top-left from the layer bounds' top-left, so
/// it follows the layer when it moves. Aurora gives a shape layer the
/// canvas's size where Photoshop's box is the shape's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReferenceBox {
    pub dx: i64,
    pub dy: i64,
    pub width: u32,
    pub height: u32,
}

impl ReferenceBox {
    /// The box for a layer whose bounds are `bounds`.
    #[must_use]
    pub fn within(self, bounds: crate::Rect) -> crate::Rect {
        crate::Rect {
            x: bounds.x.saturating_add(self.dx),
            y: bounds.y.saturating_add(self.dy),
            width: self.width,
            height: self.height,
        }
    }
}

/// A gradient overlay (`GrFl`, task 110): a gradient drawn over the layer's
/// interior in its own blend mode and opacity, above the colour overlays.
///
/// **Not on the `.aur` effects wire**: [`LayerEffects`] is positional
/// `postcard`, so these ride in the optional `layer-gradient-overlays`
/// entry (and `LayerOp::SetGradientOverlays`) instead, and
/// `LayerEffects::gradient_overlays` is `#[serde(skip)]`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GradientOverlay {
    pub gate: EffectGate,
    pub blend_mode: EffectBlendMode,
    /// `0.0..=1.0`.
    pub opacity: f32,
    /// The gradient; its `align` is always on (an overlay aligned to the
    /// document is not modelled and is counted in [`OtherEffects`]).
    pub gradient: crate::GradientFill,
    /// `Dthr`, kept; not applied (the interior is `f16`).
    pub dither: bool,
    /// The box the gradient spans; `None` is the layer's `bounds`.
    pub reference: Option<ReferenceBox>,
}

/// A pattern overlay (`patternFill`, task 111): a pattern from the
/// document's pool tiled over the layer's interior, in its own blend mode
/// and opacity, above the gradient overlays.
///
/// **Not on the `.aur` effects wire** (like [`GradientOverlay`]): it rides
/// in the optional `layer-patterns` entry and
/// `LayerOp::SetPatternOverlays`.
///
/// **Placement** (derived from Photoshop's own pictures, see PLAN.md): the
/// pattern's texel `(0, 0)` is centred on pixel `origin` (Photoshop's
/// reference point, `fxrp`), texel `(i, j)` on `origin + (i, j) * scale`
/// (scale `1.0` = 100 %), and the pattern tiles forever from there.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PatternOverlay {
    pub gate: EffectGate,
    pub blend_mode: EffectBlendMode,
    /// `0.0..=1.0`.
    pub opacity: f32,
    /// The pattern's ID in the document's pool
    /// (`aurora_doc::LayerTree::patterns`).
    pub pattern: String,
    /// `0.01..=10.0`; `1.0` is 100 %.
    pub scale: f32,
    /// The origin as an offset from the layer's `bounds` top-left, so it
    /// follows the layer when it moves.
    pub origin: (f64, f64),
}

/// The smallest and largest pattern scale (`Scl `, 1 % to 1000 %).
pub const MIN_PATTERN_SCALE: f32 = 0.01;
pub const MAX_PATTERN_SCALE: f32 = 10.0;

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
    /// Gradient overlays (task 110). `#[serde(skip)]`: see
    /// [`GradientOverlay`].
    #[serde(skip)]
    pub gradient_overlays: Vec<GradientOverlay>,
    /// Pattern overlays (task 111). `#[serde(skip)]`: see
    /// [`PatternOverlay`].
    #[serde(skip)]
    pub pattern_overlays: Vec<PatternOverlay>,
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
            gradient_overlays: Vec::new(),
            pattern_overlays: Vec::new(),
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
        count(&self.gradient_overlays, "gradient overlay")?;
        count(&self.pattern_overlays, "pattern overlay")?;
        for p in &self.pattern_overlays {
            unit(p.opacity, "opacity")?;
            if p.pattern.is_empty() || p.pattern.len() > crate::pattern::MAX_PATTERN_ID_BYTES {
                return Err(EffectsError::OutOfRange {
                    field: "pattern ID",
                });
            }
            if !(MIN_PATTERN_SCALE..=MAX_PATTERN_SCALE).contains(&p.scale) {
                return Err(EffectsError::OutOfRange {
                    field: "pattern scale",
                });
            }
            let max = f64::from(crate::MAX_DOCUMENT_EXTENT);
            if !(p.origin.0.is_finite()
                && p.origin.1.is_finite()
                && p.origin.0.abs() <= max
                && p.origin.1.abs() <= max)
            {
                return Err(EffectsError::OutOfRange {
                    field: "pattern origin",
                });
            }
        }
        for g in &self.gradient_overlays {
            unit(g.opacity, "opacity")?;
            g.gradient
                .validate()
                .map_err(|e| EffectsError::OutOfRange { field: e.field })?;
            if let Some(r) = g.reference
                && (r.width == 0
                    || r.height == 0
                    || r.dx.unsigned_abs() > crate::MAX_DOCUMENT_ORIGIN.unsigned_abs()
                    || r.dy.unsigned_abs() > crate::MAX_DOCUMENT_ORIGIN.unsigned_abs())
            {
                return Err(EffectsError::OutOfRange {
                    field: "gradient reference box",
                });
            }
            if !g.gradient.align {
                return Err(EffectsError::OutOfRange {
                    field: "gradient alignment",
                });
            }
        }
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

    /// The solid-colour strokes Photoshop would draw and Aurora draws
    /// (0.190.0).
    #[must_use]
    pub fn shown_color_strokes(&self) -> usize {
        self.strokes
            .iter()
            .filter(|e| self.shows(e.gate) && e.fill == StrokeFill::Color)
            .count()
    }

    /// The gradient and pattern strokes Photoshop would draw and Aurora
    /// does not (0.190.0).
    #[must_use]
    pub fn shown_unrendered_strokes(&self) -> usize {
        self.shown_strokes() - self.shown_color_strokes()
    }

    /// The outer glows Photoshop would draw.
    #[must_use]
    pub fn shown_outer_glows(&self) -> usize {
        self.outer_glows
            .iter()
            .filter(|e| self.shows(e.gate))
            .count()
    }

    /// The outer glows Aurora draws (0.191.0): shown and Softer.
    #[must_use]
    pub fn shown_soft_glows(&self) -> usize {
        self.outer_glows
            .iter()
            .filter(|e| self.shows(e.gate) && !e.precise)
            .count()
    }

    /// The outer glows Photoshop would draw and Aurora does not (0.191.0):
    /// the Precise technique.
    #[must_use]
    pub fn shown_unrendered_glows(&self) -> usize {
        self.shown_outer_glows() - self.shown_soft_glows()
    }

    /// The gradient overlays Photoshop would draw and Aurora draws
    /// (task 110).
    #[must_use]
    pub fn shown_gradient_overlays(&self) -> usize {
        self.gradient_overlays
            .iter()
            .filter(|e| self.shows(e.gate))
            .count()
    }

    /// The pattern overlays Photoshop would draw and Aurora draws
    /// (task 111).
    #[must_use]
    pub fn shown_pattern_overlays(&self) -> usize {
        self.pattern_overlays
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
            || self.shown_gradient_overlays() > 0
            || self.shown_pattern_overlays() > 0
            || self.shown_other() > 0
    }

    /// Whether there is nothing at all to keep (no instance of any kind).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.drop_shadows.is_empty()
            && self.color_overlays.is_empty()
            && self.strokes.is_empty()
            && self.outer_glows.is_empty()
            && self.gradient_overlays.is_empty()
            && self.pattern_overlays.is_empty()
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

    #[test]
    fn stroke_geometry_reaches_by_position() {
        let mut s = Stroke {
            size: 6.0,
            ..Stroke::default()
        };
        let g = s.geometry();
        assert_eq!(
            (g.outside, g.inside, g.apron, g.capped),
            (6.0, 0.0, 7, false)
        );
        s.position = StrokePosition::Inside;
        let g = s.geometry();
        assert_eq!((g.outside, g.inside, g.apron), (0.0, 6.0, 7));
        s.position = StrokePosition::Center;
        let g = s.geometry();
        assert_eq!((g.outside, g.inside, g.apron), (3.0, 3.0, 4));
        // A fractional size rounds the apron up.
        s.size = 2.5;
        assert_eq!(s.geometry().apron, 3);
    }

    #[test]
    fn stroke_geometry_is_capped_and_total() {
        let mut s = Stroke {
            size: MAX_EFFECT_SIZE,
            ..Stroke::default()
        };
        let g = s.geometry();
        assert!(
            !g.capped && g.apron <= MAX_SHADOW_APRON,
            "a valid size is never capped"
        );
        // Not reachable from a file (E1 clamps), but total all the same.
        s.size = 10_000.0;
        let g = s.geometry();
        assert!(g.capped && g.apron == MAX_SHADOW_APRON);
        assert!(g.outside < MAX_SHADOW_APRON as f32);
        for bad in [f32::NAN, f32::INFINITY, -4.0] {
            s.size = bad;
            let g = s.geometry();
            assert!(g.outside <= (MAX_SHADOW_APRON as f32), "{bad}");
            assert!(g.apron <= MAX_SHADOW_APRON);
        }
    }

    #[test]
    fn only_solid_colour_strokes_are_drawn() {
        let mut fx = LayerEffects::default();
        fx.strokes.push(Stroke::default());
        fx.strokes.push(Stroke {
            fill: StrokeFill::Gradient,
            ..Stroke::default()
        });
        fx.strokes.push(Stroke {
            fill: StrokeFill::Pattern,
            ..Stroke::default()
        });
        let mut off = Stroke::default();
        off.gate.enabled = false;
        fx.strokes.push(off);
        assert_eq!(fx.shown_strokes(), 3);
        assert_eq!(fx.shown_color_strokes(), 1);
        assert_eq!(fx.shown_unrendered_strokes(), 2);
    }

    fn ramp() -> crate::GradientFill {
        let stop = |pos: f32, v: f32| crate::FillStop {
            pos,
            mid: 0.5,
            value: [v; 3],
        };
        crate::GradientFill {
            colours: vec![stop(0.0, 0.0), stop(1.0, 1.0)],
            alphas: Vec::new(),
            style: crate::GradientStyle::Linear,
            angle: 0.0,
            scale: 1.0,
            reverse: false,
            align: true,
            offset: (0.0, 0.0),
        }
    }

    fn overlay() -> GradientOverlay {
        GradientOverlay {
            gate: EffectGate::default(),
            blend_mode: EffectBlendMode::Normal,
            opacity: 1.0,
            gradient: ramp(),
            dither: false,
            reference: None,
        }
    }

    #[test]
    #[allow(clippy::indexing_slicing)]
    fn gradient_overlay_validation_refuses_each_bad_field() {
        let with = |o: GradientOverlay| LayerEffects {
            gradient_overlays: vec![o],
            ..LayerEffects::default()
        };
        assert_eq!(with(overlay()).validate(), Ok(()));
        let mut bad = Vec::new();
        for opacity in [-0.1, 1.1, f32::NAN] {
            let mut o = overlay();
            o.opacity = opacity;
            bad.push(o);
        }
        let mut o = overlay();
        o.gradient.colours.clear();
        bad.push(o);
        let mut o = overlay();
        o.gradient.colours = vec![o.gradient.colours[0]; crate::MAX_GRADIENT_STOPS + 1];
        bad.push(o);
        let mut o = overlay();
        o.gradient.scale = 0.0;
        bad.push(o);
        let mut o = overlay();
        o.gradient.angle = f64::NAN;
        bad.push(o);
        let mut o = overlay();
        o.gradient.offset = (2000.0, 0.0);
        bad.push(o);
        let mut o = overlay();
        o.gradient.colours[0].value = [2.0, 0.0, 0.0];
        bad.push(o);
        let mut o = overlay();
        o.gradient.colours.swap(0, 1);
        bad.push(o);
        let mut o = overlay();
        o.gradient.align = false;
        bad.push(o);
        for (zero_w, zero_h, far) in [
            (true, false, false),
            (false, true, false),
            (false, false, true),
        ] {
            let mut o = overlay();
            o.reference = Some(ReferenceBox {
                dx: if far { i64::MAX } else { 0 },
                dy: 0,
                width: u32::from(!zero_w),
                height: u32::from(!zero_h),
            });
            bad.push(o);
        }
        for (n, o) in bad.into_iter().enumerate() {
            assert!(with(o).validate().is_err(), "case {n}");
        }
        let many = LayerEffects {
            gradient_overlays: vec![overlay(); MAX_EFFECT_INSTANCES + 1],
            ..LayerEffects::default()
        };
        assert!(matches!(
            many.validate(),
            Err(EffectsError::TooManyInstances { .. })
        ));
    }

    #[test]
    #[allow(clippy::indexing_slicing)]
    fn gradient_overlays_are_gated_counted_and_not_on_the_effects_wire() {
        let mut fx = LayerEffects {
            gradient_overlays: vec![overlay()],
            ..LayerEffects::default()
        };
        assert!(fx.shows_any());
        assert!(!fx.is_empty());
        assert_eq!(fx.shown_gradient_overlays(), 1);
        fx.gradient_overlays[0].gate.present = false;
        assert_eq!((fx.shown_gradient_overlays(), fx.shows_any()), (0, false));
        fx.gradient_overlays[0].gate.present = true;
        fx.enabled = false;
        assert_eq!(fx.shown_gradient_overlays(), 0);
        // `#[serde(skip)]`: the positional wire is the one before task 110.
        let with = LayerEffects {
            gradient_overlays: vec![overlay()],
            ..LayerEffects::default()
        };
        let a = postcard::to_allocvec(&with).unwrap_or_default();
        let b = postcard::to_allocvec(&LayerEffects::default()).unwrap_or_default();
        assert_eq!(a, b);
        let back: Result<LayerEffects, _> = postcard::from_bytes(&a);
        assert_eq!(back.map(|f| f.gradient_overlays.len()).ok(), Some(0));
    }

    #[test]
    fn a_reference_box_follows_the_layer_bounds() {
        let r = ReferenceBox {
            dx: -3,
            dy: 5,
            width: 7,
            height: 9,
        };
        let moved = crate::Rect {
            x: 100,
            y: 200,
            width: 50,
            height: 50,
        };
        assert_eq!(
            r.within(moved),
            crate::Rect {
                x: 97,
                y: 205,
                width: 7,
                height: 9
            }
        );
    }

    fn pattern_overlay() -> PatternOverlay {
        PatternOverlay {
            gate: EffectGate::default(),
            blend_mode: EffectBlendMode::Normal,
            opacity: 1.0,
            pattern: "p".to_owned(),
            scale: 1.0,
            origin: (0.0, 0.0),
        }
    }

    #[test]
    fn pattern_overlay_validation_refuses_each_bad_field() {
        let with = |o: PatternOverlay| LayerEffects {
            pattern_overlays: vec![o],
            ..LayerEffects::default()
        };
        assert_eq!(with(pattern_overlay()).validate(), Ok(()));
        let mut bad = Vec::new();
        for opacity in [-0.1, 1.1, f32::NAN] {
            let mut o = pattern_overlay();
            o.opacity = opacity;
            bad.push(o);
        }
        for scale in [0.0, 0.009, 10.1, f32::NAN, f32::INFINITY, -1.0] {
            let mut o = pattern_overlay();
            o.scale = scale;
            bad.push(o);
        }
        for origin in [
            (f64::NAN, 0.0),
            (0.0, f64::INFINITY),
            (300_001.0, 0.0),
            (0.0, -300_001.0),
        ] {
            let mut o = pattern_overlay();
            o.origin = origin;
            bad.push(o);
        }
        let mut o = pattern_overlay();
        o.pattern = String::new();
        bad.push(o);
        let mut o = pattern_overlay();
        o.pattern = "x".repeat(crate::pattern::MAX_PATTERN_ID_BYTES + 1);
        bad.push(o);
        for (n, o) in bad.into_iter().enumerate() {
            assert!(with(o).validate().is_err(), "case {n}");
        }
        // The limits themselves are valid.
        for (scale, origin) in [(0.01, (-300_000.0, 300_000.0)), (10.0, (0.0, 0.0))] {
            let mut o = pattern_overlay();
            o.scale = scale;
            o.origin = origin;
            assert_eq!(with(o).validate(), Ok(()));
        }
        let many = LayerEffects {
            pattern_overlays: vec![pattern_overlay(); MAX_EFFECT_INSTANCES + 1],
            ..LayerEffects::default()
        };
        assert!(matches!(
            many.validate(),
            Err(EffectsError::TooManyInstances { .. })
        ));
    }

    #[test]
    fn pattern_overlays_are_gated_counted_and_not_on_the_effects_wire() {
        let mut fx = LayerEffects {
            pattern_overlays: vec![pattern_overlay()],
            ..LayerEffects::default()
        };
        assert!(fx.shows_any());
        assert!(!fx.is_empty());
        assert_eq!(fx.shown_pattern_overlays(), 1);
        fx.pattern_overlays[0].gate.enabled = false;
        assert_eq!((fx.shown_pattern_overlays(), fx.shows_any()), (0, false));
        fx.pattern_overlays[0].gate.enabled = true;
        fx.enabled = false;
        assert_eq!(fx.shown_pattern_overlays(), 0);
        let with = LayerEffects {
            pattern_overlays: vec![pattern_overlay()],
            ..LayerEffects::default()
        };
        let a = postcard::to_allocvec(&with).unwrap_or_default();
        let b = postcard::to_allocvec(&LayerEffects::default()).unwrap_or_default();
        assert_eq!(a, b);
        let back: Result<LayerEffects, _> = postcard::from_bytes(&a);
        assert_eq!(back.map(|f| f.pattern_overlays.len()).ok(), Some(0));
    }
}
