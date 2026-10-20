//! Tests for the PSD writer's layer effects (task 120): the descriptor
//! writer is the inverse of the parser, and every effect kind the model
//! holds reads back equal through Aurora's own reader.

use aurora_core::effects::{
    ColorOverlay, DropShadow, EffectBlendMode, EffectColor, EffectGate, GradientOverlay, OuterGlow,
    PatternOverlay, Stroke, StrokeFill, StrokePaint, StrokePosition,
};
use aurora_core::gradient_fill::{FillStop, GradientFill, GradientStyle};
use aurora_core::pattern::Pattern;
use aurora_core::{LayerEffects, Rect};
use aurora_doc::{LayerId, LayerTree};

use super::descriptor::{self, Descriptor, ItemBudget, Value};
use super::write_effects::{self, Ctx, blend_code, effects_block};
use super::write_tests::{Doc, image, ok, read_back, rect};

fn parse(block: &[u8]) -> Descriptor {
    match descriptor::parse_effects_block(block, &mut ItemBudget::new()) {
        Ok(d) => d,
        Err(err) => unreachable!("{err:?}"),
    }
}

fn block_of(root: &Descriptor) -> Vec<u8> {
    match effects_block(root) {
        Ok(b) => b,
        Err(_) => unreachable!("unwritable"),
    }
}

#[test]
fn descriptor_writer_is_the_parsers_inverse_for_every_value_kind() {
    let inner = Descriptor {
        class: b"RGBC".to_vec(),
        items: vec![(b"Rd  ".to_vec(), Value::Double(12.5))],
    };
    let root = Descriptor {
        class: b"null".to_vec(),
        items: vec![
            (
                b"Scl ".to_vec(),
                Value::Unit {
                    unit: *b"#Prc",
                    value: 208.25,
                },
            ),
            (b"masterFXSwitch".to_vec(), Value::Bool(true)),
            (b"n".to_vec(), Value::Integer(-7)),
            (
                b"Md  ".to_vec(),
                Value::Enum {
                    type_id: b"BlnM".to_vec(),
                    value: b"linearBurn".to_vec(),
                },
            ),
            (b"Nm  ".to_vec(), Value::Text("Wasser 水".to_owned())),
            (
                b"list".to_vec(),
                Value::List(vec![
                    Value::Descriptor(inner.clone()),
                    Value::Double(-0.0),
                    Value::List(vec![Value::Bool(false)]),
                ]),
            ),
            (b"Clr ".to_vec(), Value::Descriptor(inner)),
        ],
    };
    let block = block_of(&root);
    let back = parse(&block);
    // `TEXT` is written NUL-terminated, as Photoshop does.
    let mut want = root.clone();
    for (k, v) in &mut want.items {
        if k == b"Nm  " {
            *v = Value::Text("Wasser 水\0".to_owned());
        }
    }
    assert_eq!(back, want);
    // The header: object version 0, descriptor version 16.
    assert_eq!(block.get(..8), Some(&[0, 0, 0, 0, 0, 0, 0, 16][..]));
}

#[test]
fn keys_use_the_four_byte_form_only_for_four_byte_codes() {
    let root = Descriptor {
        class: b"null".to_vec(),
        items: vec![(b"masterFXSwitch".to_vec(), Value::Bool(true))],
    };
    let block = block_of(&root);
    // name (count 1 + one NUL unit), class: length 0 + 'null', one item.
    assert_eq!(block.get(8..12), Some(&[0, 0, 0, 1][..]));
    let class_at = 8 + 4 + 2;
    assert_eq!(
        block.get(class_at..class_at + 8),
        Some(&b"\0\0\0\0null"[..])
    );
    let key_at = class_at + 8 + 4;
    assert_eq!(block.get(key_at..key_at + 4), Some(&[0, 0, 0, 14][..]));
    assert_eq!(
        block.get(key_at + 4..key_at + 18),
        Some(&b"masterFXSwitch"[..])
    );
}

#[test]
fn depth_is_bounded() {
    let mut v = Value::Bool(true);
    for _ in 0..40 {
        v = Value::List(vec![v]);
    }
    let root = Descriptor {
        class: b"null".to_vec(),
        items: vec![(b"deep".to_vec(), v)],
    };
    assert!(effects_block(&root).is_err());
}

#[test]
fn blend_codes_invert_the_readers_table_for_all_27_modes() {
    for mode in EffectBlendMode::ALL {
        assert_eq!(
            super::effects::blend_for_enum(blend_code(mode)),
            Some(mode),
            "{mode:?}"
        );
    }
}

fn colour(r: f32, g: f32, b: f32) -> EffectColor {
    EffectColor { r, g, b }
}

fn stop<T>(pos: f32, value: T) -> FillStop<T> {
    FillStop {
        pos,
        mid: 0.5,
        value,
    }
}

fn gradient() -> GradientFill {
    GradientFill {
        colours: vec![
            stop(0.0, [1.0, 0.0, 0.0]),
            FillStop {
                pos: 0.25,
                mid: 0.3,
                value: [0.0, 0.501_960_8, 0.0],
            },
            stop(1.0, [0.0, 0.0, 1.0]),
        ],
        alphas: vec![stop(0.0, 1.0), stop(1.0, 0.5)],
        style: GradientStyle::Radial,
        angle: 45.0,
        scale: 0.8,
        reverse: true,
        align: true,
        offset: (10.0, -5.0),
        smooth_ramp: true,
    }
}

fn pattern(id: &str) -> Pattern {
    let texels: Vec<u16> = (0..4 * 3 * 2)
        .map(|i| u16::try_from(i % 4).unwrap_or(0) * 257 * 20 + 257)
        .collect();
    ok(Pattern::new(
        id.to_owned(),
        "Stripes".to_owned(),
        3,
        2,
        texels,
    ))
}

fn sixteen_bit_pattern(id: &str) -> Pattern {
    let texels: Vec<u16> = (0..4 * 2 * 2).map(|i| 1000 + i * 3).collect();
    ok(Pattern::new(id.to_owned(), "Deep".to_owned(), 2, 2, texels))
}

fn pattern_overlay(id: &str, origin: (f64, f64)) -> PatternOverlay {
    PatternOverlay {
        gate: EffectGate::default(),
        blend_mode: EffectBlendMode::Normal,
        opacity: 1.0,
        pattern: id.to_owned(),
        scale: 1.0,
        origin,
    }
}

fn gradient_overlay(g: GradientFill) -> GradientOverlay {
    GradientOverlay {
        gate: EffectGate::default(),
        blend_mode: EffectBlendMode::Normal,
        opacity: 1.0,
        gradient: g,
        dither: false,
        reference: None,
    }
}

/// A rich set of effects, every kind and every paint.
fn rich_effects() -> LayerEffects {
    let mut fx = LayerEffects {
        scale: 150.0,
        ..LayerEffects::default()
    };
    fx.drop_shadows.push(DropShadow {
        opacity: 0.35,
        angle: 90.0,
        local_angle: 30.0,
        distance: 18.0,
        spread: 0.2,
        size: 41.0,
        noise: 0.1,
        anti_aliased: true,
        ..DropShadow::default()
    });
    fx.drop_shadows.push(DropShadow {
        blend_mode: EffectBlendMode::LinearBurn,
        color: colour(1.0, 0.501_960_8, 0.0),
        use_global_light: false,
        angle: -45.0,
        local_angle: -45.0,
        knocks_out: false,
        gate: EffectGate {
            present: true,
            enabled: false,
        },
        ..DropShadow::default()
    });
    fx.color_overlays.push(ColorOverlay {
        blend_mode: EffectBlendMode::Overlay,
        color: colour(0.2, 0.4, 0.6),
        opacity: 0.6,
        ..ColorOverlay::default()
    });
    fx.strokes.push(Stroke {
        position: StrokePosition::Inside,
        size: 6.5,
        color: colour(0.0, 1.0, 0.25),
        opacity: 0.8,
        blend_mode: EffectBlendMode::Screen,
        ..Stroke::default()
    });
    fx.strokes.push(Stroke {
        position: StrokePosition::Center,
        fill: StrokeFill::Gradient,
        paint: Some(StrokePaint::Gradient {
            gradient: gradient(),
            reference: None,
        }),
        ..Stroke::default()
    });
    fx.strokes.push(Stroke {
        fill: StrokeFill::Pattern,
        paint: Some(StrokePaint::Pattern {
            pattern: "pat-a".to_owned(),
            scale: 0.5,
            origin: (4.5, 2.5),
        }),
        ..Stroke::default()
    });
    fx.outer_glows.push(OuterGlow {
        blend_mode: EffectBlendMode::Screen,
        color: colour(1.0, 1.0, 0.501_960_8),
        opacity: 0.5,
        precise: true,
        spread: 0.1,
        size: 20.0,
        noise: 0.05,
        ..OuterGlow::default()
    });
    fx.gradient_overlays.push(GradientOverlay {
        blend_mode: EffectBlendMode::SoftLight,
        opacity: 0.9,
        dither: true,
        ..gradient_overlay(gradient())
    });
    fx.pattern_overlays.push(PatternOverlay {
        blend_mode: EffectBlendMode::Multiply,
        opacity: 0.7,
        scale: 2.0,
        ..pattern_overlay("pat-a", (4.5, 2.5))
    });
    fx
}

/// Effects with everything position-dependent made absolute or removed,
/// so a read-back compares equal: gradient boxes (the reader sets them
/// from the record) are dropped, pattern origins are made absolute.
#[allow(clippy::cast_precision_loss)]
fn normalised(tree: &LayerTree, id: LayerId) -> LayerEffects {
    let mut fx = tree.effects(id).cloned().unwrap_or_default();
    let (bx, by) = tree
        .bounds(id)
        .map_or((0.0, 0.0), |b| (b.x as f64, b.y as f64));
    for o in &mut fx.gradient_overlays {
        o.reference = None;
    }
    for o in &mut fx.pattern_overlays {
        o.origin = (o.origin.0 + bx, o.origin.1 + by);
    }
    for s in &mut fx.strokes {
        match &mut s.paint {
            Some(StrokePaint::Gradient { reference, .. }) => *reference = None,
            Some(StrokePaint::Pattern { origin, .. }) => *origin = (origin.0 + bx, origin.1 + by),
            None => {}
        }
    }
    fx
}

fn doc_with(effects: LayerEffects, patterns: &[Pattern]) -> (Doc, LayerId) {
    let mut doc = Doc::new((16, 12));
    let img = image(16, 12, |x, y| [x as f32 / 16.0, y as f32 / 16.0, 0.5, 1.0]);
    let id = doc.layer("Styled", rect(0, 0, 16, 12), Some((&img, 0, 0)));
    for p in patterns {
        ok(doc.tree.add_pattern(p.clone()));
    }
    ok(doc.tree.set_effects(id, Some(effects)));
    (doc, id)
}

fn read_ids(bytes: &[u8]) -> (super::PsdDocument, LayerId) {
    let back = read_back(bytes);
    let id = back
        .layers
        .roots()
        .iter()
        .copied()
        .find(|&id| back.layers.name(id) == Some("Styled"))
        .unwrap_or_else(|| unreachable!("layer"));
    (back, id)
}

fn lfx2_of(bytes: &[u8]) -> Vec<u8> {
    let at = bytes
        .windows(8)
        .position(|w| w == b"8BIMlfx2")
        .unwrap_or_else(|| unreachable!("no lfx2"));
    let len = bytes
        .get(at + 8..at + 12)
        .and_then(|b| <[u8; 4]>::try_from(b).ok())
        .map_or(0, u32::from_be_bytes) as usize;
    bytes
        .get(at + 12..at + 12 + len)
        .unwrap_or_default()
        .to_vec()
}

/// Writes `doc` with the unverified kinds (gradient and pattern effects,
/// `Patt`, `fxrp`) switched ON.
fn write_on(doc: &mut Doc) -> (Vec<u8>, super::write::PsdWriteOutcome) {
    let mut out = std::io::Cursor::new(Vec::new());
    let mut source = &doc.composite;
    let outcome = ok(super::write::write_psd_limited(
        &mut out,
        &doc.tree,
        doc.canvas,
        &mut source,
        &mut doc.store,
        super::write::PsdSaveOptions::default(),
        super::write::WriteLimits {
            unverified_effects: true,
            ..Default::default()
        },
    ));
    (out.into_inner(), outcome)
}

fn verify(bytes: &[u8], outcome: &super::write::PsdWriteOutcome) {
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("fx.psd");
    ok(std::fs::write(&path, bytes));
    ok(super::write::verify_psd_file(&path, &outcome.expected));
}

#[test]
fn every_effect_kind_reads_back_equal() {
    let (mut doc, id) = doc_with(rich_effects(), &[pattern("pat-a")]);
    let want = normalised(&doc.tree, id);
    let (bytes, outcome) = write_on(&mut doc);
    verify(&bytes, &outcome);
    let (back, rid) = read_ids(&bytes);
    assert_eq!(normalised(&back.layers, rid), want);
    // The pattern came back too, pixel for pixel.
    assert_eq!(
        back.layers.patterns().get("pat-a"),
        doc.tree.patterns().get("pat-a")
    );
    // Reported as unchecked in Photoshop, and nothing lost.
    let items = outcome.report.items.join("\n");
    assert!(
        items.contains("1 layer's effects are saved in the file"),
        "{items}"
    );
    assert!(!items.contains("couldn't"), "{items}");
}

/// Task 147: a straight ramp is written as Smoothness 0 (`Intr` 0) and a
/// smoothed one as 100 % (4096), so both read back as they were and
/// Photoshop shows the picture Aurora drew.
#[test]
fn a_straight_ramp_is_written_as_smoothness_zero_and_reads_back_straight() {
    for smooth in [false, true] {
        let mut fx = rich_effects();
        for o in &mut fx.gradient_overlays {
            o.gradient.smooth_ramp = smooth;
        }
        for stroke in &mut fx.strokes {
            if let Some(aurora_core::effects::StrokePaint::Gradient { gradient, .. }) =
                &mut stroke.paint
            {
                gradient.smooth_ramp = smooth;
            }
        }
        assert_eq!(fx.gradient_overlay_ramps(), vec![smooth]);
        assert!(fx.stroke_gradient_ramps().contains(&smooth));
        let (mut doc, id) = doc_with(fx, &[pattern("pat-a")]);
        let want = normalised(&doc.tree, id);
        let (bytes, _) = write_on(&mut doc);
        let (back, rid) = read_ids(&bytes);
        assert_eq!(normalised(&back.layers, rid), want, "smooth {smooth}");
        let got = back.layers.effects(rid);
        assert_eq!(
            got.map(aurora_core::LayerEffects::gradient_overlay_ramps),
            Some(vec![smooth]),
            "smooth {smooth}"
        );
        // The descriptor itself.
        let d = parse(&lfx2_of(&bytes));
        let intr = d
            .get(b"GrFl")
            .and_then(Value::descriptor)
            .and_then(|g| g.get(b"Grad"))
            .and_then(Value::descriptor)
            .and_then(|g| g.get(b"Intr"))
            .and_then(Value::number);
        assert_eq!(
            intr,
            Some(if smooth { 4096.0 } else { 0.0 }),
            "smooth {smooth}"
        );
    }
}

#[test]
fn single_instances_use_single_keys_and_two_use_the_multi_list() {
    let (mut doc, _) = doc_with(rich_effects(), &[pattern("pat-a")]);
    let (bytes, _) = write_on(&mut doc);
    let d = parse(&lfx2_of(&bytes));
    assert!(d.get(b"dropShadowMulti").is_some() && d.get(b"DrSh").is_none());
    assert!(d.get(b"frameFXMulti").is_some() && d.get(b"FrFX").is_none());
    assert!(d.get(b"solidFillMulti").is_none() && d.get(b"SoFi").is_some());
    assert!(d.get(b"gradientFillMulti").is_none() && d.get(b"GrFl").is_some());
    assert!(d.get(b"OrGl").is_some() && d.get(b"patternFill").is_some());
    assert_eq!(d.get(b"masterFXSwitch").and_then(Value::bool), Some(true));
    assert_eq!(d.get(b"Scl ").and_then(Value::number), Some(150.0));
}

#[test]
fn photoshops_field_names_and_units_are_used() {
    let (mut doc, _) = doc_with(rich_effects(), &[pattern("pat-a")]);
    let (bytes, _) = write_on(&mut doc);
    let d = parse(&lfx2_of(&bytes));
    let shadow = d
        .get(b"dropShadowMulti")
        .and_then(Value::list)
        .and_then(|l| l.first())
        .and_then(Value::descriptor)
        .unwrap_or_else(|| unreachable!("shadow"));
    let unit_of = |key: &[u8]| match shadow.get(key) {
        Some(Value::Unit { unit, .. }) => Some(*unit),
        _ => None,
    };
    assert_eq!(unit_of(b"Opct"), Some(*b"#Prc"));
    assert_eq!(unit_of(b"lagl"), Some(*b"#Ang"));
    assert_eq!(unit_of(b"Dstn"), Some(*b"#Pxl"));
    assert_eq!(unit_of(b"blur"), Some(*b"#Pxl"));
    assert_eq!(unit_of(b"Nose"), Some(*b"#Prc"));
    assert_eq!(shadow.get(b"Opct").and_then(Value::number), Some(35.0));
    assert_eq!(shadow.class, b"DrSh".to_vec());
    let clr = shadow
        .get(b"Clr ")
        .and_then(Value::descriptor)
        .unwrap_or_else(|| unreachable!("colour"));
    assert_eq!(clr.class, b"RGBC".to_vec());
    assert_eq!(clr.get(b"Rd  ").and_then(Value::number), Some(0.0));
    let stroke = d
        .get(b"frameFXMulti")
        .and_then(Value::list)
        .and_then(|l| l.first())
        .and_then(Value::descriptor)
        .unwrap_or_else(|| unreachable!("stroke"));
    assert_eq!(
        stroke.get(b"Styl").and_then(Value::enum_value),
        Some(&b"InsF"[..])
    );
    assert_eq!(
        stroke.get(b"PntT").and_then(Value::enum_value),
        Some(&b"SClr"[..])
    );
    assert_eq!(stroke.get(b"Sz  ").and_then(Value::number), Some(6.5));
    assert_eq!(
        stroke
            .get(b"Clr ")
            .and_then(Value::descriptor)
            .and_then(|c| c.get(b"Grn "))
            .and_then(Value::number),
        Some(255.0)
    );
}

#[test]
fn master_switch_off_and_gates_survive() {
    let mut fx = LayerEffects {
        enabled: false,
        ..LayerEffects::default()
    };
    fx.color_overlays.push(ColorOverlay {
        gate: EffectGate {
            present: false,
            enabled: true,
        },
        ..ColorOverlay::default()
    });
    let (mut doc, id) = doc_with(fx, &[]);
    let want = normalised(&doc.tree, id);
    let (bytes, _) = ok(doc.write());
    let (back, rid) = read_ids(&bytes);
    assert_eq!(normalised(&back.layers, rid), want);
}

#[test]
fn a_layer_without_effects_writes_no_block_and_no_resource() {
    let mut doc = Doc::new((8, 8));
    doc.layer("Plain", rect(0, 0, 8, 8), None);
    let (bytes, outcome) = ok(doc.write());
    assert!(!bytes.windows(8).any(|w| w == b"8BIMlfx2"));
    assert!(!bytes.windows(8).any(|w| w == b"8BIMPatt"));
    assert_eq!(outcome.expected.global_angle, None);
    assert!(outcome.report.is_empty());
}

#[test]
fn global_light_angle_is_written_as_resource_1037_and_followed() {
    let mut fx = LayerEffects::default();
    fx.drop_shadows.push(DropShadow {
        angle: 135.0,
        local_angle: 20.0,
        ..DropShadow::default()
    });
    let (mut doc, _) = doc_with(fx, &[]);
    let (bytes, outcome) = ok(doc.write());
    assert_eq!(outcome.expected.global_angle, Some(135));
    assert!(!outcome.report.items.join(" ").contains("whole number"));
    let (back, rid) = read_ids(&bytes);
    let s = back
        .layers
        .effects(rid)
        .map(|f| f.drop_shadows.clone())
        .unwrap_or_default();
    assert_eq!(s.len(), 1);
    let first = s.first().unwrap_or_else(|| unreachable!("shadow"));
    assert_eq!(
        (first.angle, first.local_angle, first.use_global_light),
        (135.0, 20.0, true)
    );
}

#[test]
fn a_shadow_at_another_global_angle_is_detached_and_itemised() {
    let mut fx = LayerEffects::default();
    for angle in [135.0, 60.0] {
        fx.drop_shadows.push(DropShadow {
            angle,
            local_angle: angle,
            ..DropShadow::default()
        });
    }
    let (mut doc, _) = doc_with(fx, &[]);
    let (bytes, outcome) = ok(doc.write());
    let items = outcome.report.items.join("\n");
    assert!(
        items.contains("1 drop shadow follows global light at an angle"),
        "{items}"
    );
    let (back, rid) = read_ids(&bytes);
    let s = back
        .layers
        .effects(rid)
        .map(|f| f.drop_shadows.clone())
        .unwrap_or_default();
    let got: Vec<(bool, f32)> = s.iter().map(|s| (s.use_global_light, s.angle)).collect();
    assert_eq!(got, [(true, 135.0), (false, 60.0)]);
}

#[test]
fn a_fractional_global_angle_is_rounded_and_itemised() {
    let mut fx = LayerEffects::default();
    fx.drop_shadows.push(DropShadow {
        angle: 100.4,
        local_angle: 100.4,
        ..DropShadow::default()
    });
    let (mut doc, _) = doc_with(fx, &[]);
    let (_, outcome) = ok(doc.write());
    assert_eq!(outcome.expected.global_angle, Some(100));
    let items = outcome.report.items.join("\n");
    assert!(
        items.contains("global light angle is saved as a whole number"),
        "{items}"
    );
    assert!(
        items.contains("drop shadow follows global light"),
        "{items}"
    );
}

#[test]
fn contours_extra_glows_and_extra_patterns_are_itemised() {
    let mut fx = LayerEffects::default();
    fx.drop_shadows.push(DropShadow {
        linear_contour: false,
        ..DropShadow::default()
    });
    fx.outer_glows.push(OuterGlow::default());
    fx.outer_glows.push(OuterGlow::default());
    fx.pattern_overlays
        .push(pattern_overlay("pat-a", (0.0, 0.0)));
    fx.pattern_overlays
        .push(pattern_overlay("pat-a", (0.0, 0.0)));
    let (mut doc, _) = doc_with(fx, &[pattern("pat-a")]);
    let (bytes, outcome) = write_on(&mut doc);
    let items = outcome.report.items.join("\n");
    assert!(items.contains("1 effect had a custom contour"), "{items}");
    assert!(items.contains("1 extra outer glow isn't saved"), "{items}");
    assert!(
        items.contains("1 extra pattern overlay isn't saved"),
        "{items}"
    );
    let (back, rid) = read_ids(&bytes);
    let got = back.layers.effects(rid).cloned().unwrap_or_default();
    assert_eq!(got.outer_glows.len(), 1);
    assert_eq!(got.pattern_overlays.len(), 1);
    assert!(got.drop_shadows.first().is_some_and(|s| s.linear_contour));
}

#[test]
fn undrawn_strokes_and_missing_patterns_are_left_out_and_itemised() {
    let mut fx = LayerEffects::default();
    fx.color_overlays.push(ColorOverlay::default());
    fx.strokes.push(Stroke {
        fill: StrokeFill::Gradient,
        paint: None,
        ..Stroke::default()
    });
    fx.pattern_overlays
        .push(pattern_overlay("missing", (0.0, 0.0)));
    let (mut doc, _) = doc_with(fx, &[]);
    let (bytes, outcome) = write_on(&mut doc);
    let items = outcome.report.items.join("\n");
    assert!(
        items.contains("1 gradient or pattern stroke that Aurora couldn't draw"),
        "{items}"
    );
    assert!(
        items.contains("1 pattern effect names a pattern the document doesn't hold"),
        "{items}"
    );
    let (back, rid) = read_ids(&bytes);
    let got = back.layers.effects(rid).cloned().unwrap_or_default();
    assert!(got.strokes.is_empty() && got.pattern_overlays.is_empty());
    assert_eq!(got.color_overlays.len(), 1);
}

#[test]
fn gradient_stops_finer_than_the_file_are_rounded_and_itemised() {
    let mut g = gradient();
    if let Some(s) = g.colours.get_mut(1) {
        s.pos = 0.2501;
    }
    let mut fx = LayerEffects::default();
    fx.gradient_overlays.push(gradient_overlay(g));
    let (mut doc, _) = doc_with(fx, &[]);
    let (_, outcome) = write_on(&mut doc);
    let items = outcome.report.items.join("\n");
    assert!(
        items.contains("1 gradient has stop positions or midpoints finer"),
        "{items}"
    );
    // An exact gradient is not reported.
    let mut fx = LayerEffects::default();
    fx.gradient_overlays.push(gradient_overlay(gradient()));
    let (mut doc, _) = doc_with(fx, &[]);
    let (_, outcome) = write_on(&mut doc);
    assert!(!outcome.report.items.join("\n").contains("finer"));
}

#[test]
fn pattern_pool_block_round_trips_8_and_16_bit_patterns() {
    let eight = pattern("p8");
    let deep = sixteen_bit_pattern("p16");
    let block = write_effects::pattern_block(&[&eight, &deep]).unwrap_or_default();
    assert!(!block.is_empty() && block.len().is_multiple_of(4));
    let mut fx = LayerEffects::default();
    fx.pattern_overlays.push(pattern_overlay("p8", (0.0, 0.0)));
    fx.strokes.push(Stroke {
        fill: StrokeFill::Pattern,
        paint: Some(StrokePaint::Pattern {
            pattern: "p16".to_owned(),
            scale: 1.0,
            origin: (0.0, 0.0),
        }),
        ..Stroke::default()
    });
    let (mut doc, _) = doc_with(fx, &[eight.clone(), deep.clone()]);
    let (bytes, outcome) = write_on(&mut doc);
    verify(&bytes, &outcome);
    let (back, _) = read_ids(&bytes);
    assert_eq!(back.layers.patterns().get("p8"), Some(&eight));
    assert_eq!(back.layers.patterns().get("p16"), Some(&deep));
}

#[test]
fn only_the_patterns_a_layer_names_are_written() {
    let mut fx = LayerEffects::default();
    fx.pattern_overlays.push(pattern_overlay("p8", (0.0, 0.0)));
    let (mut doc, _) = doc_with(fx, &[pattern("p8"), pattern("unused")]);
    let (bytes, _) = write_on(&mut doc);
    let (back, _) = read_ids(&bytes);
    assert!(back.layers.patterns().contains_key("p8"));
    assert!(!back.layers.patterns().contains_key("unused"));
}

#[test]
#[allow(clippy::cast_precision_loss)]
fn pattern_origin_is_written_as_the_layers_fxrp_in_document_coordinates() {
    let mut fx = LayerEffects::default();
    fx.pattern_overlays
        .push(pattern_overlay("pat-a", (3.5, 1.5)));
    let mut doc = Doc::new((16, 12));
    let img = image(10, 8, |_, _| [1.0, 0.0, 0.0, 1.0]);
    let id = doc.layer("Styled", rect(5, 4, 10, 8), Some((&img, 0, 0)));
    ok(doc.tree.add_pattern(pattern("pat-a")));
    ok(doc.tree.set_effects(id, Some(fx)));
    let (bytes, _) = write_on(&mut doc);
    let at = bytes
        .windows(8)
        .position(|w| w == b"8BIMfxrp")
        .unwrap_or_else(|| unreachable!("no fxrp"));
    let f = |from: usize| {
        bytes
            .get(from..from + 8)
            .and_then(|b| <[u8; 8]>::try_from(b).ok())
            .map_or(f64::NAN, f64::from_be_bytes)
    };
    // The origin is relative to the layer's bounds; the file's is absolute.
    assert_eq!((f(at + 12), f(at + 20)), (8.5, 5.5));
    let (back, rid) = read_ids(&bytes);
    let o = back
        .layers
        .effects(rid)
        .and_then(|f| f.pattern_overlays.first().map(|p| p.origin))
        .unwrap_or((0.0, 0.0));
    let b = back.layers.bounds(rid).unwrap_or(Rect {
        x: 0,
        y: 0,
        width: 0,
        height: 0,
    });
    assert_eq!((o.0 + b.x as f64, o.1 + b.y as f64), (8.5, 5.5));
}

#[test]
fn groups_carry_effects_too() {
    let mut doc = Doc::new((16, 12));
    let group = ok(doc.tree.add_group("Group", None));
    ok(doc
        .tree
        .add_pixel_layer("Child", rect(0, 0, 16, 12), Some(group)));
    let mut fx = LayerEffects::default();
    fx.color_overlays.push(ColorOverlay {
        color: colour(0.0, 0.0, 1.0),
        ..ColorOverlay::default()
    });
    ok(doc.tree.set_effects(group, Some(fx.clone())));
    let (bytes, _) = ok(doc.write());
    let back = read_back(&bytes);
    let g = back
        .layers
        .roots()
        .iter()
        .copied()
        .find(|&id| back.layers.name(id) == Some("Group"))
        .unwrap_or_else(|| unreachable!("group"));
    assert_eq!(
        back.layers.effects(g).map(|f| f.color_overlays.clone()),
        Some(fx.color_overlays)
    );
}

#[test]
fn effects_block_is_padded_inside_its_length() {
    let (mut doc, _) = doc_with(rich_effects(), &[pattern("pat-a")]);
    let (bytes, _) = write_on(&mut doc);
    assert_eq!(lfx2_of(&bytes).len() % 4, 0);
}

#[test]
fn nothing_to_write_gives_no_block() {
    let tree = LayerTree::new();
    let ctx = Ctx {
        paints: true,
        global_angle: 120.0,
        patterns: tree.patterns(),
    };
    let (encoded, lost) = write_effects::encode(&LayerEffects::default(), None, &ctx);
    assert!(encoded.is_none());
    assert_eq!(lost, write_effects::EffectLosses::default());
}

#[test]
fn a_pattern_of_whole_8_bit_values_is_written_with_8_bit_planes() {
    let eight = pattern("same");
    // The same size, name and ID, with samples that are not whole 8-bit values.
    let texels: Vec<u16> = eight.texels().iter().map(|v| v + 1).collect();
    let deep = ok(Pattern::new(
        "same".to_owned(),
        "Stripes".to_owned(),
        3,
        2,
        texels,
    ));
    let short = write_effects::pattern_block(&[&eight]).unwrap_or_default();
    let long = write_effects::pattern_block(&[&deep]).unwrap_or_default();
    assert!(!short.is_empty() && short.len() < long.len());
    assert_eq!(long.len() - short.len(), 24);
}

fn has(bytes: &[u8], needle: &[u8]) -> bool {
    bytes.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn the_default_writes_only_the_verified_kinds_and_itemises_the_rest() {
    let (mut doc, _) = doc_with(rich_effects(), &[pattern("pat-a")]);
    let (bytes, outcome) = ok(doc.write());
    // None of the unverified layouts reach the file.
    for needle in [
        &b"Grdn"[..],
        b"Clrt",
        b"patternFill",
        b"Ptrn",
        b"8BIMPatt",
        b"8BIMfxrp",
        b"GrFl",
    ] {
        assert!(!has(&bytes, needle), "{}", String::from_utf8_lossy(needle));
    }
    // The verified kinds do: shadows, colour overlay, the solid stroke, the glow.
    let d = parse(&lfx2_of(&bytes));
    assert!(d.get(b"dropShadowMulti").is_some());
    assert!(d.get(b"SoFi").is_some() && d.get(b"OrGl").is_some());
    let strokes = d.get(b"FrFX").and_then(Value::descriptor);
    assert_eq!(
        strokes
            .and_then(|s| s.get(b"PntT"))
            .and_then(Value::enum_value),
        Some(&b"SClr"[..])
    );
    assert!(d.get(b"GrFl").is_none() && d.get(b"patternFill").is_none());
    // The pool is not written either, and the verifier agrees.
    assert_eq!(outcome.expected.patterns, None);
    verify(&bytes, &outcome);
    let items = outcome.report.items.join("\n");
    for kind in [
        "gradient overlay",
        "gradient stroke",
        "pattern overlay",
        "pattern stroke",
    ] {
        assert!(
            items.contains(&format!(
                "Not saved (unverified in Photoshop): {kind} on “Styled”."
            )),
            "{kind}: {items}"
        );
    }
    assert!(
        items.contains("hasn't checked them in Photoshop"),
        "{items}"
    );
    assert!(
        items.contains("how Photoshop draws them is verified")
            || items.contains("neither the layout nor how Photoshop draws"),
        "{items}"
    );
    // The reader sees exactly the verified subset.
    let (back, rid) = read_ids(&bytes);
    let got = back.layers.effects(rid).cloned().unwrap_or_default();
    assert!(got.gradient_overlays.is_empty() && got.pattern_overlays.is_empty());
    assert_eq!(got.strokes.len(), 1);
    assert_eq!(got.strokes.first().map(|s| s.fill), Some(StrokeFill::Color));
    assert!(back.layers.patterns().is_empty());
}

#[test]
fn the_preflight_names_what_the_default_does_not_save() {
    let (doc, _) = doc_with(rich_effects(), &[pattern("pat-a")]);
    let report = ok(super::write::psd_save_preflight(
        &doc.tree,
        doc.canvas,
        super::write::PsdSaveOptions::default(),
    ));
    let items = report.items.join("\n");
    assert!(items.contains("gradient overlay on “Styled”"), "{items}");
    assert!(items.contains("pattern stroke on “Styled”"), "{items}");
}

#[test]
fn outer_glow_fixed_values_are_itemised() {
    let mut fx = LayerEffects::default();
    fx.outer_glows.push(OuterGlow::default());
    let (mut doc, _) = doc_with(fx, &[]);
    let (_, outcome) = ok(doc.write());
    let items = outcome.report.items.join("\n");
    assert!(
        items.contains("1 outer glow is saved with Photoshop's fixed Range 50 %"),
        "{items}"
    );
}

#[test]
fn a_pattern_that_cannot_be_encoded_drops_its_effect_instead_of_the_save() {
    let good = pattern("good");
    let bad = pattern("bad");
    let pool: std::collections::BTreeMap<String, Pattern> = [good.clone(), bad.clone()]
        .into_iter()
        .map(|p| (p.id().to_owned(), p))
        .collect();
    // The encoder fails for one pattern; it is left out of the usable pool.
    let usable = write_effects::usable_patterns(&pool, |p| {
        if p.id() == "bad" {
            Err(write_effects::Unwritable)
        } else {
            Ok(Vec::new())
        }
    });
    assert!(usable.contains_key("good") && !usable.contains_key("bad"));
    let ctx = Ctx {
        paints: true,
        global_angle: 120.0,
        patterns: &usable,
    };
    let mut fx = LayerEffects::default();
    fx.color_overlays.push(ColorOverlay::default());
    fx.pattern_overlays.push(pattern_overlay("bad", (0.0, 0.0)));
    fx.strokes.push(Stroke {
        fill: StrokeFill::Pattern,
        paint: Some(StrokePaint::Pattern {
            pattern: "good".to_owned(),
            scale: 1.0,
            origin: (0.0, 0.0),
        }),
        ..Stroke::default()
    });
    let (encoded, lost) = write_effects::encode(&fx, None, &ctx);
    let encoded = encoded.unwrap_or_else(|| unreachable!("the rest is written"));
    assert_eq!(encoded.patterns, ["good".to_owned()]);
    assert_eq!(lost.patterns_missing, 1);
    let d = parse(&encoded.lfx2);
    assert!(d.get(b"patternFill").is_none() && d.get(b"FrFX").is_some());
}

#[test]
fn with_the_switch_off_no_paint_is_encoded_even_when_the_pool_has_the_pattern() {
    let pool: std::collections::BTreeMap<String, Pattern> =
        std::iter::once(("pat-a".to_owned(), pattern("pat-a"))).collect();
    let ctx = Ctx {
        paints: false,
        global_angle: 120.0,
        patterns: &pool,
    };
    let mut fx = rich_effects();
    fx.color_overlays.clear();
    fx.drop_shadows.clear();
    fx.outer_glows.clear();
    fx.strokes.clear();
    let (encoded, _) = write_effects::encode(&fx, None, &ctx);
    assert!(
        encoded.is_none(),
        "only gradient and pattern overlays were set"
    );
    // And a mixed layer keeps its solid parts only, with no `fxrp` or pool.
    let fx = rich_effects();
    let (encoded, _) = write_effects::encode(&fx, None, &ctx);
    let encoded = encoded.unwrap_or_else(|| unreachable!("solid kinds"));
    assert!(encoded.fxrp.is_none() && encoded.patterns.is_empty());
    let d = parse(&encoded.lfx2);
    assert!(d.get(b"GrFl").is_none() && d.get(b"patternFill").is_none());
}
