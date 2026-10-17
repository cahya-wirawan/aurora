//! The PSD writer's layer effects (task 120): a Photoshop `lfx2` block
//! (object-based effects: version `0`, descriptor version `16`, class
//! `null`) built as the exact inverse of [`super::effects`]'s reader, the
//! document's patterns as a global `Patt` block, the layer's `fxrp`
//! reference point, and the global light angle (image resource 1037).
//!
//! # What is written
//!
//! Drop shadows (`DrSh`), colour overlays (`SoFi`), strokes (`FrFX`, solid,
//! gradient and pattern), outer glows (`OrGl`), gradient overlays (`GrFl`)
//! and pattern overlays (`patternFill`). A kind with one instance uses its
//! single key; two or more use Photoshop's `*Multi` list
//! (`dropShadowMulti`, `solidFillMulti`, `frameFXMulti`,
//! `gradientFillMulti`). Photoshop has no multiple outer glows or pattern
//! overlays: the first is written, the rest are itemised.
//!
//! # What is rounded or left out, always itemised
//!
//! - a non-Linear contour (Aurora keeps only "is it Linear") is written as
//!   Linear;
//! - the global light angle is a whole number of degrees; a shadow that
//!   follows global light at another angle is written with its own angle;
//! - gradient stop positions are whole 1/4096ths and midpoints whole
//!   percents;
//! - every pattern effect of a layer shares the layer's one `fxrp`;
//! - a gradient or pattern stroke Aurora could not draw, a pattern that is
//!   not in the document's pool, extra outer glows and pattern overlays are
//!   not written.
//!
//! Nothing here has been opened in Photoshop.

#![allow(clippy::float_cmp, clippy::cast_possible_truncation)]

use std::collections::{BTreeMap, BTreeSet};

use aurora_core::Rect;
use aurora_core::effects::{
    ColorOverlay, DropShadow, EffectBlendMode, EffectColor, EffectGate, GradientOverlay,
    LayerEffects, OuterGlow, PatternOverlay, Stroke, StrokeFill, StrokePaint, StrokePosition,
};
use aurora_core::gradient_fill::{GradientFill, GradientStyle};
use aurora_core::pattern::Pattern;

use super::descriptor::{Descriptor, MAX_DEPTH, Value};

/// The most bytes one layer's `lfx2` block may take.
const MAX_BLOCK_BYTES: usize = 4 << 20;

/// A value or block that cannot be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Unwritable;

type Done = Result<(), Unwritable>;

// ---------------------------------------------------------------------
// The descriptor writer: the inverse of `descriptor.rs`'s parser.
// ---------------------------------------------------------------------

fn put_len(out: &mut Vec<u8>, n: usize) -> Done {
    out.extend_from_slice(&u32::try_from(n).map_err(|_| Unwritable)?.to_be_bytes());
    Ok(())
}

/// A key or class id: a 4-byte code behind a zero length, any other as a
/// length and its bytes.
fn put_key(out: &mut Vec<u8>, key: &[u8]) -> Done {
    if key.is_empty() {
        return Err(Unwritable);
    }
    if key.len() == 4 {
        out.extend_from_slice(&0u32.to_be_bytes());
    } else {
        put_len(out, key.len())?;
    }
    out.extend_from_slice(key);
    Ok(())
}

fn put_unicode(out: &mut Vec<u8>, text: &str) -> Done {
    let mut units: Vec<u16> = text.encode_utf16().collect();
    units.push(0);
    put_len(out, units.len())?;
    for unit in units {
        out.extend_from_slice(&unit.to_be_bytes());
    }
    Ok(())
}

fn put_descriptor(out: &mut Vec<u8>, d: &Descriptor, depth: usize) -> Done {
    if depth > MAX_DEPTH || out.len() > MAX_BLOCK_BYTES {
        return Err(Unwritable);
    }
    put_unicode(out, "")?;
    put_key(out, &d.class)?;
    put_len(out, d.items.len())?;
    for (key, value) in &d.items {
        put_key(out, key)?;
        put_value(out, value, depth)?;
    }
    Ok(())
}

/// One value, its `OSType` first, at container depth `depth` (the same
/// depth convention as the parser).
fn put_value(out: &mut Vec<u8>, value: &Value, depth: usize) -> Done {
    match value {
        Value::Descriptor(d) => {
            out.extend_from_slice(b"Objc");
            put_descriptor(out, d, depth + 1)
        }
        Value::List(items) => {
            if depth + 1 > MAX_DEPTH {
                return Err(Unwritable);
            }
            out.extend_from_slice(b"VlLs");
            put_len(out, items.len())?;
            for item in items {
                put_value(out, item, depth + 1)?;
            }
            Ok(())
        }
        Value::Double(v) => {
            out.extend_from_slice(b"doub");
            out.extend_from_slice(&v.to_be_bytes());
            Ok(())
        }
        Value::Unit { unit, value } => {
            out.extend_from_slice(b"UntF");
            out.extend_from_slice(unit);
            out.extend_from_slice(&value.to_be_bytes());
            Ok(())
        }
        Value::Integer(v) => {
            out.extend_from_slice(b"long");
            out.extend_from_slice(&v.to_be_bytes());
            Ok(())
        }
        Value::Bool(v) => {
            out.extend_from_slice(b"bool");
            out.push(u8::from(*v));
            Ok(())
        }
        Value::Enum { type_id, value } => {
            out.extend_from_slice(b"enum");
            put_key(out, type_id)?;
            put_key(out, value)
        }
        Value::Text(text) => {
            out.extend_from_slice(b"TEXT");
            put_unicode(out, text)
        }
        _ => Err(Unwritable),
    }
}

/// A descriptor as the body of an `lfx2` block: object version `0`,
/// descriptor version `16`, then the descriptor.
pub(super) fn effects_block(root: &Descriptor) -> Result<Vec<u8>, Unwritable> {
    let mut out = Vec::new();
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(&16u32.to_be_bytes());
    put_descriptor(&mut out, root, 0)?;
    if out.len() > MAX_BLOCK_BYTES {
        return Err(Unwritable);
    }
    Ok(out)
}

// ---------------------------------------------------------------------
// Value builders.
// ---------------------------------------------------------------------

type Items = Vec<(&'static str, Value)>;

fn obj(class: &str, items: Items) -> Value {
    Value::Descriptor(Descriptor {
        class: class.as_bytes().to_vec(),
        items: items
            .into_iter()
            .map(|(k, v)| (k.as_bytes().to_vec(), v))
            .collect(),
    })
}

fn unit(code: &[u8; 4], value: f64) -> Value {
    Value::Unit { unit: *code, value }
}

fn percent(value: f64) -> Value {
    unit(b"#Prc", value)
}

fn pixels(value: f64) -> Value {
    unit(b"#Pxl", value)
}

fn angle(value: f64) -> Value {
    unit(b"#Ang", value)
}

fn enum_of(type_id: &str, value: &[u8]) -> Value {
    Value::Enum {
        type_id: type_id.as_bytes().to_vec(),
        value: value.to_vec(),
    }
}

/// The file number `v` such that the reader's `v * mult` gives `x` again
/// as an `f32`: a whole number when that works (Photoshop's panels show
/// whole percents), else the exact quotient.
fn by_mul(x: f32, mult: f64) -> f64 {
    let raw = f64::from(x) / mult;
    let whole = raw.round();
    if ((whole * mult) as f32) == x {
        whole
    } else {
        raw
    }
}

/// As [`by_mul`] for a reader that divides: `v / div` gives `x`.
fn by_div(x: f32, div: f64) -> f64 {
    let raw = f64::from(x) * div;
    let whole = raw.round();
    if ((whole / div) as f32) == x {
        whole
    } else {
        raw
    }
}

/// As [`by_div`] for an `f64` model value.
fn by_div64(x: f64, div: f64) -> f64 {
    let raw = x * div;
    let whole = raw.round();
    if whole / div == x { whole } else { raw }
}

fn opacity(x: f32) -> Value {
    percent(by_mul(x, 0.01))
}

fn colour(c: EffectColor) -> Value {
    let m = 1.0 / 255.0;
    obj(
        "RGBC",
        vec![
            ("Rd  ", Value::Double(by_mul(c.r, m))),
            ("Grn ", Value::Double(by_mul(c.g, m))),
            ("Bl  ", Value::Double(by_mul(c.b, m))),
        ],
    )
}

fn gate(g: EffectGate) -> Items {
    vec![
        ("enab", Value::Bool(g.enabled)),
        ("present", Value::Bool(g.present)),
        ("showInDialog", Value::Bool(true)),
    ]
}

/// The inverse of `effects::blend_for_enum`.
pub(super) fn blend_code(mode: EffectBlendMode) -> &'static [u8] {
    use EffectBlendMode as M;
    match mode {
        M::Normal => b"Nrml",
        M::Dissolve => b"Dslv",
        M::Darken => b"Drkn",
        M::Multiply => b"Mltp",
        M::ColorBurn => b"CBrn",
        M::LinearBurn => b"linearBurn",
        M::DarkerColor => b"darkerColor",
        M::Lighten => b"Lghn",
        M::Screen => b"Scrn",
        M::ColorDodge => b"CDdg",
        M::LinearDodge => b"linearDodge",
        M::LighterColor => b"lighterColor",
        M::Overlay => b"Ovrl",
        M::SoftLight => b"SftL",
        M::HardLight => b"HrdL",
        M::VividLight => b"vividLight",
        M::LinearLight => b"linearLight",
        M::PinLight => b"pinLight",
        M::HardMix => b"hardMix",
        M::Difference => b"Dfrn",
        M::Exclusion => b"Xclu",
        M::Subtract => b"blendSubtraction",
        M::Divide => b"blendDivide",
        M::Hue => b"H   ",
        M::Saturation => b"Strt",
        M::Color => b"Clr ",
        M::Luminosity => b"Lmns",
    }
}

fn blend(mode: EffectBlendMode) -> Value {
    enum_of("BlnM", blend_code(mode))
}

/// The two-point Linear contour Photoshop writes.
fn linear_contour() -> Value {
    let point = |v: f64| {
        obj(
            "CrPt",
            vec![("Hrzn", Value::Double(v)), ("Vrtc", Value::Double(v))],
        )
    };
    obj(
        "ShpC",
        vec![
            ("Nm  ", Value::Text("Linear".to_owned())),
            ("Crv ", Value::List(vec![point(0.0), point(255.0)])),
        ],
    )
}

// ---------------------------------------------------------------------
// What was rounded or left out.
// ---------------------------------------------------------------------

/// Counts of effects the file holds differently from the model, for the
/// save report.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct EffectLosses {
    /// Layers with at least one effect written.
    pub layers: usize,
    /// Effects whose contour was not Linear (written Linear).
    pub contours: usize,
    /// Outer glows past the first on a layer.
    pub extra_glows: usize,
    /// Pattern overlays past the first on a layer.
    pub extra_patterns: usize,
    /// Shadows following global light at an angle other than the file's
    /// (written with their own angle).
    pub detached_angles: usize,
    /// Gradient or pattern strokes Aurora could not draw.
    pub strokes_left_out: usize,
    /// Pattern effects whose pattern is not in the document's pool.
    pub patterns_missing: usize,
    /// Layers whose pattern effects have different reference points.
    pub mixed_origins: usize,
    /// Gradients whose stop positions or midpoints were rounded.
    pub gradients_rounded: usize,
}

impl EffectLosses {
    pub(super) fn add(&mut self, other: Self) {
        self.layers += other.layers;
        self.contours += other.contours;
        self.extra_glows += other.extra_glows;
        self.extra_patterns += other.extra_patterns;
        self.detached_angles += other.detached_angles;
        self.strokes_left_out += other.strokes_left_out;
        self.patterns_missing += other.patterns_missing;
        self.mixed_origins += other.mixed_origins;
        self.gradients_rounded += other.gradients_rounded;
    }

    /// The report lines (without the "not checked in Photoshop" one).
    pub(super) fn items(&self) -> Vec<String> {
        let many = |n: usize, one: &str, rest: &str| -> String {
            if n == 1 {
                format!("1 {one}{rest}")
            } else {
                format!("{n} {one}s{rest}")
            }
        };
        let mut items = Vec::new();
        if self.contours > 0 {
            items.push(many(
                self.contours,
                "effect",
                " had a custom contour; it is saved with the Linear contour.",
            ));
        }
        if self.extra_glows > 0 {
            items.push(many(
                self.extra_glows,
                "extra outer glow",
                " isn't saved (the file holds one per layer).",
            ));
        }
        if self.extra_patterns > 0 {
            items.push(many(
                self.extra_patterns,
                "extra pattern overlay",
                " isn't saved (the file holds one per layer).",
            ));
        }
        if self.detached_angles > 0 {
            items.push(many(
                self.detached_angles,
                "drop shadow",
                " follows global light at an angle other than the file's, so it is saved with its own angle.",
            ));
        }
        if self.strokes_left_out > 0 {
            items.push(many(
                self.strokes_left_out,
                "gradient or pattern stroke",
                " that Aurora couldn't draw isn't saved.",
            ));
        }
        if self.patterns_missing > 0 {
            items.push(many(
                self.patterns_missing,
                "pattern effect",
                " names a pattern the document doesn't hold and isn't saved.",
            ));
        }
        if self.mixed_origins > 0 {
            items.push(many(
                self.mixed_origins,
                "layer",
                " has pattern effects placed differently; the file holds one placement per layer, so they share the first.",
            ));
        }
        if self.gradients_rounded > 0 {
            items.push(many(
                self.gradients_rounded,
                "gradient",
                " has stop positions or midpoints finer than the file; they are rounded to 1/4096 and 1 %.",
            ));
        }
        items
    }
}

/// What the encoder needs from the document.
#[derive(Clone, Copy)]
pub(super) struct Ctx<'a> {
    /// The global light angle (resource 1037), whole degrees.
    pub global_angle: f32,
    pub patterns: &'a BTreeMap<String, Pattern>,
}

/// One layer's encoded effects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Encoded {
    pub lfx2: Vec<u8>,
    /// The layer's reference point, absolute document coordinates.
    pub fxrp: Option<[u8; 16]>,
    /// The patterns the block names.
    pub patterns: Vec<String>,
    pub losses: EffectLosses,
}

/// The global light angle for the file and whether it had to be rounded:
/// the first drop shadow that follows global light gives it.
pub(super) fn global_angle_of<'a>(
    effects: impl Iterator<Item = &'a LayerEffects>,
) -> (Option<i32>, bool) {
    for fx in effects {
        if let Some(shadow) = fx.drop_shadows.iter().find(|s| s.use_global_light) {
            let rounded = shadow.angle.round().clamp(-360.0, 360.0);
            return (Some(rounded as i32), rounded != shadow.angle);
        }
    }
    (None, false)
}

// ---------------------------------------------------------------------
// The effects.
// ---------------------------------------------------------------------

fn drop_shadow(s: &DropShadow, ctx: &Ctx<'_>, loss: &mut EffectLosses) -> Value {
    let follows = s.use_global_light && s.angle == ctx.global_angle;
    if s.use_global_light && !follows {
        loss.detached_angles += 1;
    }
    if !s.linear_contour {
        loss.contours += 1;
    }
    let lagl = if follows { s.local_angle } else { s.angle };
    let mut items = gate(s.gate);
    items.extend([
        ("Md  ", blend(s.blend_mode)),
        ("Clr ", colour(s.color)),
        ("Opct", opacity(s.opacity)),
        ("uglg", Value::Bool(follows)),
        ("lagl", angle(f64::from(lagl))),
        ("Dstn", pixels(f64::from(s.distance))),
        ("Ckmt", pixels(by_mul(s.spread, 0.01))),
        ("blur", pixels(f64::from(s.size))),
        ("Nose", percent(by_mul(s.noise, 0.01))),
        ("AntA", Value::Bool(s.anti_aliased)),
        ("TrnS", linear_contour()),
        ("layerConceals", Value::Bool(s.knocks_out)),
    ]);
    obj("DrSh", items)
}

fn color_overlay(o: &ColorOverlay) -> Value {
    let mut items = gate(o.gate);
    items.extend([
        ("Md  ", blend(o.blend_mode)),
        ("Clr ", colour(o.color)),
        ("Opct", opacity(o.opacity)),
    ]);
    obj("SoFi", items)
}

fn outer_glow(g: &OuterGlow, loss: &mut EffectLosses) -> Value {
    // The model keeps whether the contour was Linear only for shadows; a
    // glow with another contour was already read as Linear.
    let _ = loss;
    let mut items = gate(g.gate);
    items.extend([
        ("Md  ", blend(g.blend_mode)),
        ("Clr ", colour(g.color)),
        ("Opct", opacity(g.opacity)),
        (
            "GlwT",
            enum_of("BETE", if g.precise { b"PrBL" } else { b"SfBL" }),
        ),
        ("Ckmt", pixels(by_mul(g.spread, 0.01))),
        ("blur", pixels(f64::from(g.size))),
        ("Nose", percent(by_mul(g.noise, 0.01))),
        ("ShdN", percent(0.0)),
        ("AntA", Value::Bool(false)),
        ("TrnS", linear_contour()),
        ("Inpr", percent(50.0)),
    ]);
    obj("OrGl", items)
}

/// A gradient as a `Grdn` descriptor, the inverse of `fill::gradient`'s
/// stop readers. Returns whether a position or midpoint had to be rounded.
fn gradient_descriptor(g: &GradientFill) -> (Value, bool) {
    let mut rounded = false;
    let mut stop = |pos: f32, mid: f32| -> (i32, i32) {
        let l = (f64::from(pos) * 4096.0).round();
        let m = (f64::from(mid) * 100.0).round();
        if ((l / 4096.0) as f32) != pos || ((m / 100.0) as f32) != mid {
            rounded = true;
        }
        (l as i32, m as i32)
    };
    let colours: Vec<Value> = g
        .colours
        .iter()
        .map(|s| {
            let (l, m) = stop(s.pos, s.mid);
            let ch = |c: f32| Value::Double(by_div(c, 255.0));
            obj(
                "Clrt",
                vec![
                    (
                        "Clr ",
                        obj(
                            "RGBC",
                            vec![
                                ("Rd  ", ch(s.value[0])),
                                ("Grn ", ch(s.value[1])),
                                ("Bl  ", ch(s.value[2])),
                            ],
                        ),
                    ),
                    ("Type", enum_of("Clry", b"UsrS")),
                    ("Lctn", Value::Integer(l)),
                    ("Mdpn", Value::Integer(m)),
                ],
            )
        })
        .collect();
    let transparency =
        |pos: f32, mid: f32, a: f32, stop: &mut dyn FnMut(f32, f32) -> (i32, i32)| {
            let (l, m) = stop(pos, mid);
            obj(
                "TrnS",
                vec![
                    ("Opct", percent(by_div(a, 100.0))),
                    ("Lctn", Value::Integer(l)),
                    ("Mdpn", Value::Integer(m)),
                ],
            )
        };
    let alphas: Vec<Value> = if g.alphas.is_empty() {
        // Photoshop always holds at least two opacity stops.
        vec![
            transparency(0.0, 0.5, 1.0, &mut stop),
            transparency(1.0, 0.5, 1.0, &mut stop),
        ]
    } else {
        g.alphas
            .iter()
            .map(|s| transparency(s.pos, s.mid, s.value, &mut stop))
            .collect()
    };
    let value = obj(
        "Grdn",
        vec![
            ("Nm  ", Value::Text("Custom".to_owned())),
            ("GrdF", enum_of("GrdF", b"CstS")),
            ("Intr", Value::Double(4096.0)),
            ("Clrs", Value::List(colours)),
            ("Trns", Value::List(alphas)),
        ],
    );
    (value, rounded)
}

/// The gradient items shared by overlays and strokes.
fn gradient_items(g: &GradientFill, loss: &mut EffectLosses) -> Items {
    let (grad, rounded) = gradient_descriptor(g);
    loss.gradients_rounded += usize::from(rounded);
    let style: &[u8] = match g.style {
        GradientStyle::Linear => b"Lnr ",
        GradientStyle::Radial => b"Rdl ",
        GradientStyle::Angle => b"Angl",
        GradientStyle::Reflected => b"Rflc",
        GradientStyle::Diamond => b"Dmnd",
    };
    vec![
        ("Grad", grad),
        ("Angl", angle(g.angle)),
        ("Type", enum_of("GrdT", style)),
        ("Rvrs", Value::Bool(g.reverse)),
        ("Dthr", Value::Bool(false)),
        ("Algn", Value::Bool(true)),
        ("Scl ", percent(by_div64(g.scale, 100.0))),
        (
            "Ofst",
            obj(
                "Pnt ",
                vec![("Hrzn", percent(g.offset.0)), ("Vrtc", percent(g.offset.1))],
            ),
        ),
    ]
}

fn gradient_overlay(o: &GradientOverlay, loss: &mut EffectLosses) -> Value {
    let mut items = gate(o.gate);
    items.extend([("Md  ", blend(o.blend_mode)), ("Opct", opacity(o.opacity))]);
    let mut grad = gradient_items(&o.gradient, loss);
    for (key, value) in &mut grad {
        if *key == "Dthr" {
            *value = Value::Bool(o.dither);
        }
    }
    items.extend(grad);
    obj("GrFl", items)
}

/// The pattern reference descriptor and the pattern, when the pool has it.
fn pattern_ref<'a>(id: &str, ctx: &Ctx<'a>) -> Option<(Value, &'a Pattern)> {
    let pattern = ctx.patterns.get(id)?;
    Some((
        obj(
            "Ptrn",
            vec![
                ("Nm  ", Value::Text(pattern.name().to_owned())),
                ("Idnt", Value::Text(id.to_owned())),
            ],
        ),
        pattern,
    ))
}

fn phase(code: fn(f64) -> Value) -> Value {
    obj("Pnt ", vec![("Hrzn", code(0.0)), ("Vrtc", code(0.0))])
}

/// Where a layer's pattern effects sit: one `fxrp` for all of them.
#[derive(Default)]
struct PatternPlace {
    ids: BTreeSet<String>,
    origin: Option<(f64, f64)>,
    mixed: bool,
}

impl PatternPlace {
    fn note(&mut self, id: &str, origin: (f64, f64), bounds: Option<Rect>) {
        self.ids.insert(id.to_owned());
        #[allow(clippy::cast_precision_loss)] // layer origins are far below 2^53
        let abs = (
            origin.0 + bounds.map_or(0.0, |b| b.x as f64),
            origin.1 + bounds.map_or(0.0, |b| b.y as f64),
        );
        match self.origin {
            None => self.origin = Some(abs),
            Some(first) => {
                if (first.0 - abs.0).abs() > 1e-9 || (first.1 - abs.1).abs() > 1e-9 {
                    self.mixed = true;
                }
            }
        }
    }
}

fn pattern_overlay(
    o: &PatternOverlay,
    ctx: &Ctx<'_>,
    bounds: Option<Rect>,
    place: &mut PatternPlace,
    loss: &mut EffectLosses,
) -> Option<Value> {
    let Some((reference, _)) = pattern_ref(&o.pattern, ctx) else {
        loss.patterns_missing += 1;
        return None;
    };
    place.note(&o.pattern, o.origin, bounds);
    let mut items = gate(o.gate);
    items.extend([
        ("Md  ", blend(o.blend_mode)),
        ("Opct", opacity(o.opacity)),
        ("Ptrn", reference),
        ("Scl ", percent(by_mul(o.scale, 0.01))),
        ("Algn", Value::Bool(true)),
        ("phase", phase(Value::Double)),
    ]);
    Some(obj("patternFill", items))
}

fn stroke(
    s: &Stroke,
    ctx: &Ctx<'_>,
    bounds: Option<Rect>,
    place: &mut PatternPlace,
    loss: &mut EffectLosses,
) -> Option<Value> {
    let mut items = gate(s.gate);
    let position: &[u8] = match s.position {
        StrokePosition::Outside => b"OutF",
        StrokePosition::Inside => b"InsF",
        StrokePosition::Center => b"CtrF",
    };
    let paint_type: &[u8] = match s.fill {
        StrokeFill::Color => b"SClr",
        StrokeFill::Gradient => b"GrFl",
        StrokeFill::Pattern => b"Ptrn",
    };
    items.extend([
        ("Styl", enum_of("FStl", position)),
        ("PntT", enum_of("FrFl", paint_type)),
        ("Md  ", blend(s.blend_mode)),
        ("Opct", opacity(s.opacity)),
        ("Sz  ", pixels(f64::from(s.size))),
        ("Clr ", colour(s.color)),
    ]);
    match (s.fill, &s.paint) {
        (StrokeFill::Color, _) => {}
        (StrokeFill::Gradient, Some(StrokePaint::Gradient { gradient, .. })) => {
            items.extend(gradient_items(gradient, loss));
        }
        (
            StrokeFill::Pattern,
            Some(StrokePaint::Pattern {
                pattern,
                scale,
                origin,
            }),
        ) => {
            let Some((reference, _)) = pattern_ref(pattern, ctx) else {
                loss.patterns_missing += 1;
                return None;
            };
            place.note(pattern, *origin, bounds);
            items.extend([
                ("Ptrn", reference),
                ("Scl ", percent(by_mul(*scale, 0.01))),
                ("Lnkd", Value::Bool(true)),
                ("phase", phase(percent)),
            ]);
        }
        _ => {
            loss.strokes_left_out += 1;
            return None;
        }
    }
    items.push(("overprint", Value::Bool(false)));
    Some(obj("FrFX", items))
}

/// One kind's instances: nothing, the single key, or the `*Multi` list.
fn push_kind(root: &mut Items, single: &'static str, multi: &'static str, mut values: Vec<Value>) {
    match values.len() {
        0 => {}
        1 => {
            if let Some(v) = values.pop() {
                root.push((single, v));
            }
        }
        _ => root.push((multi, Value::List(values))),
    }
}

/// `fx` as an `lfx2` block, its `fxrp` and the patterns it names. `None`
/// when no effect can be written. `bounds` is the layer's bounds (pattern
/// origins are relative to them).
pub(super) fn encode(fx: &LayerEffects, bounds: Option<Rect>, ctx: &Ctx<'_>) -> Option<Encoded> {
    if fx.validate().is_err() {
        return None;
    }
    let mut loss = EffectLosses::default();
    let mut place = PatternPlace::default();
    let mut root: Items = vec![
        ("Scl ", percent(f64::from(fx.scale))),
        ("masterFXSwitch", Value::Bool(fx.enabled)),
    ];
    let shadows: Vec<Value> = fx
        .drop_shadows
        .iter()
        .map(|s| drop_shadow(s, ctx, &mut loss))
        .collect();
    push_kind(&mut root, "DrSh", "dropShadowMulti", shadows);
    let fills: Vec<Value> = fx.color_overlays.iter().map(color_overlay).collect();
    push_kind(&mut root, "SoFi", "solidFillMulti", fills);
    let strokes: Vec<Value> = fx
        .strokes
        .iter()
        .filter_map(|s| stroke(s, ctx, bounds, &mut place, &mut loss))
        .collect();
    push_kind(&mut root, "FrFX", "frameFXMulti", strokes);
    if let Some(glow) = fx.outer_glows.first() {
        root.push(("OrGl", outer_glow(glow, &mut loss)));
    }
    loss.extra_glows += fx.outer_glows.len().saturating_sub(1);
    let gradients: Vec<Value> = fx
        .gradient_overlays
        .iter()
        .map(|o| gradient_overlay(o, &mut loss))
        .collect();
    push_kind(&mut root, "GrFl", "gradientFillMulti", gradients);
    let mut patterns = fx
        .pattern_overlays
        .iter()
        .filter_map(|o| pattern_overlay(o, ctx, bounds, &mut place, &mut loss));
    if let Some(first) = patterns.next() {
        root.push(("patternFill", first));
    }
    drop(patterns);
    // Only the first pattern overlay is written; the others are counted.
    loss.extra_patterns += fx.pattern_overlays.len().saturating_sub(1);
    loss.mixed_origins += usize::from(place.mixed);
    if root.len() <= 2 {
        return None;
    }
    let Value::Descriptor(descriptor) = obj("null", root) else {
        return None;
    };
    let mut lfx2 = effects_block(&descriptor).ok()?;
    // Padded inside the block's length, like the adjustment blocks.
    while lfx2.len() % 4 != 0 {
        lfx2.push(0);
    }
    let fxrp = place.origin.map(|(x, y)| {
        let mut b = [0u8; 16];
        b[..8].copy_from_slice(&x.to_be_bytes());
        b[8..].copy_from_slice(&y.to_be_bytes());
        b
    });
    loss.layers = 1;
    Some(Encoded {
        lfx2,
        fxrp,
        patterns: place.ids.into_iter().collect(),
        losses: loss,
    })
}

// ---------------------------------------------------------------------
// The pattern pool block (`Patt`).
// ---------------------------------------------------------------------

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

/// One pattern as an item of the `Patt` block (the inverse of
/// `pattern::read_item`): RGB, four channels (colour and alpha) written
/// raw, 8 bits when every sample is a whole `v * 257` (lossless), else 16.
fn pattern_item(p: &Pattern) -> Result<Vec<u8>, Unwritable> {
    let eight = p.texels().iter().all(|v| v % 257 == 0);
    let (width, height) = (p.width(), p.height());
    let n = usize::try_from(Pattern::texel_count(width, height).map_err(|_| Unwritable)?)
        .map_err(|_| Unwritable)?;
    let mut body = Vec::new();
    put_u32(&mut body, 1);
    put_u32(&mut body, 3);
    body.extend_from_slice(&[0; 4]); // the point
    put_unicode(&mut body, p.name())?;
    let id = p.id().as_bytes();
    body.push(u8::try_from(id.len()).map_err(|_| Unwritable)?);
    body.extend_from_slice(id);
    let depth: u32 = if eight { 8 } else { 16 };
    let bytes = n * if eight { 1 } else { 2 };
    let mut array = Vec::new();
    for edge in [0, 0, height, width] {
        put_u32(&mut array, edge);
    }
    put_u32(&mut array, 4); // channels
    for plane in 0..4usize {
        put_u32(&mut array, 1); // written
        put_u32(
            &mut array,
            u32::try_from(23 + bytes).map_err(|_| Unwritable)?,
        );
        put_u32(&mut array, depth);
        for edge in [0, 0, height, width] {
            put_u32(&mut array, edge);
        }
        array.extend_from_slice(&(depth as u16).to_be_bytes());
        array.push(0); // raw
        for texel in 0..n {
            let v = p
                .texels()
                .get(texel * 4 + plane)
                .copied()
                .ok_or(Unwritable)?;
            if eight {
                array.push((v / 257) as u8);
            } else {
                array.extend_from_slice(&v.to_be_bytes());
            }
        }
    }
    put_u32(&mut array, 0); // the two records past the channels: not written
    put_u32(&mut array, 0);
    put_u32(&mut body, 3);
    put_u32(
        &mut body,
        u32::try_from(array.len()).map_err(|_| Unwritable)?,
    );
    body.extend_from_slice(&array);
    let mut item = Vec::new();
    put_u32(
        &mut item,
        u32::try_from(body.len()).map_err(|_| Unwritable)?,
    );
    item.extend_from_slice(&body);
    while item.len() % 4 != 0 {
        item.push(0);
    }
    Ok(item)
}

/// The data of a `Patt` block holding `patterns`.
pub(super) fn pattern_block(patterns: &[&Pattern]) -> Result<Vec<u8>, Unwritable> {
    let mut out = Vec::new();
    for p in patterns {
        out.extend_from_slice(&pattern_item(p)?);
    }
    Ok(out)
}
