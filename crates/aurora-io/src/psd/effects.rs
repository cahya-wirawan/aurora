//! A PSD layer's object-based effects (`lmfx`, `lfx2`, `lfxs`) mapped to
//! typed [`LayerEffects`] (0.180.0).
//!
//! **Which block.** `lmfx` (Photoshop's multi-instance block) when the
//! record has one, else `lfx2`, else `lfxs`. Each kind is read from its
//! `*Multi` list when the descriptor has one (`dropShadowMulti`, …, which
//! even `lfx2` blocks carry), else from its single key (`DrSh`, …).
//!
//! **Validation: clamp and note, or unreadable.** A *finite* number past
//! Photoshop's own range (an opacity of 104 %, a size of 300 px), or more
//! than [`MAX_EFFECT_INSTANCES`] of one kind, is clamped (the extra
//! instances dropped) and the open says so
//! ([`ReadEffects::clamped`]): the rest of the layer's style is still
//! meaningful, and a clamped value is the nearest one Photoshop itself
//! could have stored. A value that is not a finite number, the wrong
//! type, a colour that is not RGB, or an unknown blend mode has no
//! nearest value, so the whole layer's effects are reported unreadable
//! instead — never guessed.

use aurora_core::LayerEffects;
use aurora_core::effects::{
    ColorOverlay, DropShadow, EffectBlendMode, EffectColor, EffectGate, MAX_EFFECT_DISTANCE,
    MAX_EFFECT_INSTANCES, MAX_EFFECT_SCALE, MAX_EFFECT_SIZE, OtherEffects, OuterGlow, Stroke,
    StrokeFill, StrokePosition,
};

use super::descriptor::{self, Descriptor, ItemBudget, Value};

/// One layer's effects, read.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ReadEffects {
    pub effects: LayerEffects,
    /// Some value was out of range and clamped, or instances past the
    /// cap were dropped.
    pub clamped: bool,
    /// Shown outer glows with a gradient colour (`Grad`), which are not
    /// stored (0.191.0): the positional `.aur` format has no place for a
    /// gradient, and drawing one in a single colour would be wrong.
    pub glow_gradients: u32,
    /// Shown outer glows with a contour other than Linear, stored and
    /// drawn with a Linear one (0.191.0).
    pub glow_contours: u32,
}

/// Why a layer's effects were not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Unreadable;

/// Parses the effects `block` and maps it, charging `budget`.
pub(super) fn read_effects(
    block: &[u8],
    global_angle: f32,
    budget: &mut ItemBudget,
) -> Result<ReadEffects, Unreadable> {
    let descriptor = descriptor::parse_effects_block(block, budget).map_err(|_| Unreadable)?;
    map_effects(&descriptor, global_angle)
}

struct Ctx {
    clamped: bool,
}

impl Ctx {
    /// The number at `key` (or `default` when absent), times `scale`,
    /// clamped to `lo..=hi`.
    fn num(
        &mut self,
        d: &Descriptor,
        key: &[u8],
        default: f32,
        scale: f64,
        lo: f32,
        hi: f32,
    ) -> Result<f32, Unreadable> {
        let Some(value) = d.get(key) else {
            return Ok(default);
        };
        let v = value.number().ok_or(Unreadable)? * scale;
        if !v.is_finite() {
            return Err(Unreadable);
        }
        #[allow(clippy::cast_possible_truncation)] // clamped to an f32 range just below
        let clamped = v.clamp(f64::from(lo), f64::from(hi)) as f32;
        if v < f64::from(lo) || v > f64::from(hi) {
            self.clamped = true;
        }
        Ok(clamped)
    }

    fn color(&mut self, d: &Descriptor) -> Result<EffectColor, Unreadable> {
        let Some(value) = d.get(b"Clr ") else {
            return Ok(EffectColor::default());
        };
        let c = value.descriptor().ok_or(Unreadable)?;
        // RGB only (`RGBC`); Grayscale, HSB, CMYK and Lab colours are not
        // converted yet.
        if c.get(b"Rd  ").is_none() {
            return Err(Unreadable);
        }
        let scale = 1.0 / 255.0;
        Ok(EffectColor {
            r: self.num(c, b"Rd  ", 0.0, scale, 0.0, 1.0)?,
            g: self.num(c, b"Grn ", 0.0, scale, 0.0, 1.0)?,
            b: self.num(c, b"Bl  ", 0.0, scale, 0.0, 1.0)?,
        })
    }

    fn opacity(&mut self, d: &Descriptor) -> Result<f32, Unreadable> {
        self.num(d, b"Opct", 1.0, 0.01, 0.0, 1.0)
    }

    fn percent(&mut self, d: &Descriptor, key: &[u8]) -> Result<f32, Unreadable> {
        self.num(d, key, 0.0, 0.01, 0.0, 1.0)
    }

    fn size(&mut self, d: &Descriptor, key: &[u8], default: f32) -> Result<f32, Unreadable> {
        self.num(d, key, default, 1.0, 0.0, MAX_EFFECT_SIZE)
    }
}

fn flag(d: &Descriptor, key: &[u8], default: bool) -> Result<bool, Unreadable> {
    d.get(key)
        .map_or(Ok(default), |v| v.bool().ok_or(Unreadable))
}

fn gate(d: &Descriptor) -> Result<EffectGate, Unreadable> {
    Ok(EffectGate {
        present: flag(d, b"present", true)?,
        enabled: flag(d, b"enab", true)?,
    })
}

fn blend(d: &Descriptor) -> Result<EffectBlendMode, Unreadable> {
    let Some(value) = d.get(b"Md  ") else {
        return Ok(EffectBlendMode::Normal);
    };
    blend_for_enum(value.enum_value().ok_or(Unreadable)?).ok_or(Unreadable)
}

/// An effect's `BlnM` enum value as a blend mode (psd-tools'
/// `composite/blend.py` descriptor keys).
pub(super) fn blend_for_enum(key: &[u8]) -> Option<EffectBlendMode> {
    use EffectBlendMode as M;
    Some(match key {
        b"Nrml" => M::Normal,
        b"Dslv" => M::Dissolve,
        b"Drkn" => M::Darken,
        b"Mltp" => M::Multiply,
        b"CBrn" => M::ColorBurn,
        b"linearBurn" => M::LinearBurn,
        b"darkerColor" => M::DarkerColor,
        b"Lghn" => M::Lighten,
        b"Scrn" => M::Screen,
        b"CDdg" => M::ColorDodge,
        b"linearDodge" => M::LinearDodge,
        b"lighterColor" => M::LighterColor,
        b"Ovrl" => M::Overlay,
        b"SftL" => M::SoftLight,
        b"HrdL" => M::HardLight,
        b"vividLight" => M::VividLight,
        b"linearLight" => M::LinearLight,
        b"pinLight" => M::PinLight,
        b"hardMix" => M::HardMix,
        b"Dfrn" => M::Difference,
        b"Xclu" => M::Exclusion,
        b"Sbtr" | b"blendSubtraction" => M::Subtract,
        b"blendDivide" => M::Divide,
        b"H   " => M::Hue,
        b"Strt" => M::Saturation,
        b"Clr " => M::Color,
        b"Lmns" => M::Luminosity,
        _ => return None,
    })
}

/// Whether a `TrnS` contour is the two-point linear curve (absent counts
/// as linear). Decided by its points, not its name, which is localised
/// (the corpus has `線形`).
fn linear_contour(d: &Descriptor) -> Result<bool, Unreadable> {
    let Some(value) = d.get(b"TrnS") else {
        return Ok(true);
    };
    let contour = value.descriptor().ok_or(Unreadable)?;
    let Some(points) = contour.get(b"Crv ").and_then(Value::list) else {
        return Ok(false);
    };
    let at = |p: &Value| -> Option<(f64, f64)> {
        let p = p.descriptor()?;
        Some((p.get(b"Hrzn")?.number()?, p.get(b"Vrtc")?.number()?))
    };
    let pts: Vec<Option<(f64, f64)>> = points.iter().map(at).collect();
    #[allow(clippy::float_cmp)] // exact file values
    let linear = matches!(
        pts.as_slice(),
        [Some((x0, y0)), Some((x1, y1))] if *x0 == 0.0 && *y0 == 0.0 && *x1 == 255.0 && *y1 == 255.0
    );
    Ok(linear)
}

/// The instances of one kind: its `multi` list when present, else its
/// `single` key, capped at [`MAX_EFFECT_INSTANCES`].
fn instances<'a>(
    d: &'a Descriptor,
    single: &[u8],
    multi: Option<&[u8]>,
    ctx: &mut Ctx,
) -> Result<Vec<&'a Descriptor>, Unreadable> {
    let mut out = Vec::new();
    if let Some(list) = multi.and_then(|m| d.get(m)) {
        for item in list.list().ok_or(Unreadable)? {
            out.push(item.descriptor().ok_or(Unreadable)?);
        }
    } else if let Some(one) = d.get(single) {
        out.push(one.descriptor().ok_or(Unreadable)?);
    }
    if out.len() > MAX_EFFECT_INSTANCES {
        out.truncate(MAX_EFFECT_INSTANCES);
        ctx.clamped = true;
    }
    Ok(out)
}

/// How many instances of a not-yet-modelled kind are on (`present` and
/// `enab`).
fn count_on(
    d: &Descriptor,
    single: &[u8],
    multi: Option<&[u8]>,
    ctx: &mut Ctx,
) -> Result<u32, Unreadable> {
    let mut n = 0u32;
    for e in instances(d, single, multi, ctx)? {
        if gate(e)?.on() {
            n += 1;
        }
    }
    Ok(n)
}

/// Maps a parsed effects descriptor. See the module docs.
#[allow(clippy::too_many_lines)]
pub(super) fn map_effects(d: &Descriptor, global_angle: f32) -> Result<ReadEffects, Unreadable> {
    let mut ctx = Ctx { clamped: false };
    let mut fx = LayerEffects {
        enabled: flag(d, b"masterFXSwitch", true)?,
        scale: ctx.num(d, b"Scl ", 100.0, 1.0, 0.0, MAX_EFFECT_SCALE)?,
        ..LayerEffects::default()
    };
    for e in instances(d, b"DrSh", Some(b"dropShadowMulti"), &mut ctx)? {
        let use_global_light = flag(e, b"uglg", false)?;
        let local_angle = ctx.num(e, b"lagl", global_angle, 1.0, -360.0, 360.0)?;
        fx.drop_shadows.push(DropShadow {
            gate: gate(e)?,
            blend_mode: blend(e)?,
            color: ctx.color(e)?,
            opacity: ctx.opacity(e)?,
            use_global_light,
            angle: if use_global_light {
                global_angle
            } else {
                local_angle
            },
            local_angle,
            distance: ctx.num(e, b"Dstn", 0.0, 1.0, 0.0, MAX_EFFECT_DISTANCE)?,
            spread: ctx.percent(e, b"Ckmt")?,
            size: ctx.size(e, b"blur", 0.0)?,
            noise: ctx.percent(e, b"Nose")?,
            anti_aliased: flag(e, b"AntA", false)?,
            linear_contour: linear_contour(e)?,
            knocks_out: flag(e, b"layerConceals", true)?,
        });
    }
    for e in instances(d, b"SoFi", Some(b"solidFillMulti"), &mut ctx)? {
        fx.color_overlays.push(ColorOverlay {
            gate: gate(e)?,
            blend_mode: blend(e)?,
            color: ctx.color(e)?,
            opacity: ctx.opacity(e)?,
        });
    }
    for e in instances(d, b"FrFX", Some(b"frameFXMulti"), &mut ctx)? {
        let position = match e.get(b"Styl").map(|v| v.enum_value().ok_or(Unreadable)) {
            None | Some(Ok(b"OutF")) => StrokePosition::Outside,
            Some(Ok(b"InsF")) => StrokePosition::Inside,
            Some(Ok(b"CtrF")) => StrokePosition::Center,
            Some(_) => return Err(Unreadable),
        };
        let fill = match e.get(b"PntT").map(|v| v.enum_value().ok_or(Unreadable)) {
            None | Some(Ok(b"SClr")) => StrokeFill::Color,
            Some(Ok(b"GrFl")) => StrokeFill::Gradient,
            Some(Ok(b"Ptrn")) => StrokeFill::Pattern,
            Some(_) => return Err(Unreadable),
        };
        fx.strokes.push(Stroke {
            gate: gate(e)?,
            position,
            fill,
            blend_mode: blend(e)?,
            opacity: ctx.opacity(e)?,
            size: ctx.size(e, b"Sz  ", 3.0)?,
            // A gradient or pattern stroke has no `Clr `.
            color: ctx.color(e)?,
        });
    }
    let mut glow_gradients = 0u32;
    let mut glow_contours = 0u32;
    for e in instances(d, b"OrGl", None, &mut ctx)? {
        let shown = fx.enabled && gate(e)?.on();
        if e.get(b"Grad").is_some() {
            // A gradient glow has no `Clr `; keeping it as a solid glow of
            // the default colour would draw the wrong thing.
            glow_gradients += u32::from(shown);
            continue;
        }
        if shown && !linear_contour(e)? {
            glow_contours += 1;
        }
        let precise = match e.get(b"GlwT").map(|v| v.enum_value().ok_or(Unreadable)) {
            None | Some(Ok(b"SfBL")) => false,
            Some(Ok(b"PrBL")) => true,
            Some(_) => return Err(Unreadable),
        };
        fx.outer_glows.push(OuterGlow {
            gate: gate(e)?,
            blend_mode: blend(e)?,
            color: ctx.color(e)?,
            opacity: ctx.opacity(e)?,
            precise,
            spread: ctx.percent(e, b"Ckmt")?,
            size: ctx.size(e, b"blur", 0.0)?,
            noise: ctx.percent(e, b"Nose")?,
        });
    }
    fx.other = OtherEffects {
        inner_shadows: count_on(d, b"IrSh", Some(b"innerShadowMulti"), &mut ctx)?,
        inner_glows: count_on(d, b"IrGl", None, &mut ctx)?,
        bevels: count_on(d, b"ebbl", None, &mut ctx)?,
        satins: count_on(d, b"ChFX", None, &mut ctx)?,
        gradient_overlays: count_on(d, b"GrFl", Some(b"gradientFillMulti"), &mut ctx)?,
        pattern_overlays: count_on(d, b"patternFill", None, &mut ctx)?,
    };
    // Every clamp above keeps the value valid; this is the backstop for a
    // case the mapping missed, so a stored value is always a valid one.
    fx.validate().map_err(|_| Unreadable)?;
    Ok(ReadEffects {
        effects: fx,
        clamped: ctx.clamped,
        glow_gradients,
        glow_contours,
    })
}
