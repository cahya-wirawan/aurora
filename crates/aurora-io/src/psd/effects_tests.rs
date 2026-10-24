//! Tests for the descriptor parser and the layer-effects mapping
//! (0.180.0).
#![allow(
    clippy::trivially_copy_pass_by_ref,
    clippy::single_match_else,
    clippy::range_plus_one,
    clippy::cast_possible_truncation,
    clippy::cast_lossless,
    clippy::too_many_lines,
    clippy::match_wildcard_for_single_variants,
    clippy::field_reassign_with_default,
    clippy::needless_pass_by_value,
    clippy::semicolon_if_nothing_returned
)]

use aurora_core::LayerEffects;
use aurora_core::effects::{EffectBlendMode, StrokePosition};
use aurora_doc::{LayerId, LayerKind, LayerTree};

use super::descriptor::{
    self, Descriptor, DescriptorError, ItemBudget, MAX_DEPTH, Value, parse_descriptor,
    parse_effects_block,
};
use super::effects::{map_effects, read_effects};
use super::test_writer::{TestLayer, TestPsd};

// ---------------------------------------------------------------------
// A tiny descriptor encoder, for fixtures
// ---------------------------------------------------------------------

pub(super) enum V<'a> {
    Obj(Vec<(&'a [u8], V<'a>)>),
    Glob(Vec<(&'a [u8], V<'a>)>),
    ObjArray(Vec<(&'a [u8], V<'a>)>),
    List(Vec<V<'a>>),
    Doub(f64),
    Unit(&'a [u8; 4], f64),
    Units(&'a [u8; 4], Vec<f64>),
    Long(i32),
    Comp(i64),
    Bool(bool),
    Enum(&'a [u8], &'a [u8]),
    Text(&'a str),
    Class(&'a [u8; 4], &'a [u8]),
    Reference,
    Raw(&'a [u8; 4], &'a [u8]),
}

fn put_u32(o: &mut Vec<u8>, v: u32) {
    o.extend_from_slice(&v.to_be_bytes());
}

fn put_key(o: &mut Vec<u8>, k: &[u8]) {
    if k.len() == 4 {
        put_u32(o, 0);
    } else {
        put_u32(o, u32::try_from(k.len()).unwrap_or(0));
    }
    o.extend_from_slice(k);
}

fn put_unicode(o: &mut Vec<u8>, s: &str) {
    let units: Vec<u16> = s.encode_utf16().collect();
    put_u32(o, u32::try_from(units.len()).unwrap_or(0));
    for u in units {
        o.extend_from_slice(&u.to_be_bytes());
    }
}

fn put_body(o: &mut Vec<u8>, items: &[(&[u8], V<'_>)]) {
    put_unicode(o, "");
    put_key(o, b"null");
    put_u32(o, u32::try_from(items.len()).unwrap_or(0));
    for (k, v) in items {
        put_key(o, k);
        put_value(o, v);
    }
}

fn put_value(o: &mut Vec<u8>, v: &V<'_>) {
    match v {
        V::Obj(items) => {
            o.extend_from_slice(b"Objc");
            put_body(o, items);
        }
        V::Glob(items) => {
            o.extend_from_slice(b"GlbO");
            put_body(o, items);
        }
        V::ObjArray(items) => {
            o.extend_from_slice(b"ObAr");
            put_u32(o, 1);
            put_body(o, items);
        }
        V::List(items) => {
            o.extend_from_slice(b"VlLs");
            put_u32(o, u32::try_from(items.len()).unwrap_or(0));
            for item in items {
                put_value(o, item);
            }
        }
        V::Doub(x) => {
            o.extend_from_slice(b"doub");
            o.extend_from_slice(&x.to_be_bytes());
        }
        V::Unit(unit, x) => {
            o.extend_from_slice(b"UntF");
            o.extend_from_slice(*unit);
            o.extend_from_slice(&x.to_be_bytes());
        }
        V::Units(unit, xs) => {
            o.extend_from_slice(b"UnFl");
            o.extend_from_slice(*unit);
            put_u32(o, u32::try_from(xs.len()).unwrap_or(0));
            for x in xs {
                o.extend_from_slice(&x.to_be_bytes());
            }
        }
        V::Long(x) => {
            o.extend_from_slice(b"long");
            o.extend_from_slice(&x.to_be_bytes());
        }
        V::Comp(x) => {
            o.extend_from_slice(b"comp");
            o.extend_from_slice(&x.to_be_bytes());
        }
        V::Bool(b) => {
            o.extend_from_slice(b"bool");
            o.push(u8::from(*b));
        }
        V::Enum(t, e) => {
            o.extend_from_slice(b"enum");
            put_key(o, t);
            put_key(o, e);
        }
        V::Text(s) => {
            o.extend_from_slice(b"TEXT");
            put_unicode(o, s);
        }
        V::Class(os, class) => {
            o.extend_from_slice(*os);
            put_unicode(o, "c");
            put_key(o, class);
        }
        V::Reference => {
            o.extend_from_slice(b"obj ");
            put_u32(o, 7);
            o.extend_from_slice(b"prop");
            put_unicode(o, "");
            put_key(o, b"Lyr ");
            put_key(o, b"Nm  ");
            o.extend_from_slice(b"Clss");
            put_unicode(o, "");
            put_key(o, b"Lyr ");
            o.extend_from_slice(b"Enmr");
            put_unicode(o, "");
            put_key(o, b"Lyr ");
            put_key(o, b"Ordn");
            put_key(o, b"Trgt");
            o.extend_from_slice(b"rele");
            put_unicode(o, "");
            put_key(o, b"Lyr ");
            put_u32(o, 3);
            o.extend_from_slice(b"Idnt");
            put_u32(o, 9);
            o.extend_from_slice(b"indx");
            put_u32(o, 2);
            o.extend_from_slice(b"name");
            put_unicode(o, "");
            put_key(o, b"Lyr ");
            put_unicode(o, "Layer 1");
        }
        V::Raw(os, bytes) => {
            o.extend_from_slice(*os);
            put_u32(o, u32::try_from(bytes.len()).unwrap_or(0));
            o.extend_from_slice(bytes);
        }
    }
}

pub(super) fn descriptor_bytes(items: &[(&[u8], V<'_>)]) -> Vec<u8> {
    let mut o = Vec::new();
    put_body(&mut o, items);
    o
}

/// An `lfx2`-style block: object version 0, descriptor version 16.
pub(super) fn effects_block(items: &[(&[u8], V<'_>)]) -> Vec<u8> {
    let mut o = Vec::new();
    put_u32(&mut o, 0);
    put_u32(&mut o, 16);
    put_body(&mut o, items);
    o
}

fn parsed(bytes: &[u8]) -> Descriptor {
    match parse_descriptor(bytes, &mut ItemBudget::new()) {
        Ok(d) => d,
        Err(err) => unreachable!("{err:?}"),
    }
}

pub(super) fn rgb<'a>(r: f64, g: f64, b: f64) -> V<'a> {
    V::Obj(vec![
        (b"Rd  ", V::Doub(r)),
        (b"Grn ", V::Doub(g)),
        (b"Bl  ", V::Doub(b)),
    ])
}

pub(super) fn shadow<'a>(enab: bool, present: bool, distance: f64) -> V<'a> {
    V::Obj(vec![
        (b"enab", V::Bool(enab)),
        (b"present", V::Bool(present)),
        (b"Md  ", V::Enum(b"BlnM", b"Mltp")),
        (b"Clr ", rgb(0.0, 0.0, 0.0)),
        (b"Opct", V::Unit(b"#Prc", 50.0)),
        (b"uglg", V::Bool(false)),
        (b"lagl", V::Unit(b"#Ang", 45.0)),
        (b"Dstn", V::Unit(b"#Pxl", distance)),
        (b"Ckmt", V::Unit(b"#Prc", 10.0)),
        (b"blur", V::Unit(b"#Pxl", 7.0)),
    ])
}

fn mapped(block: &[u8]) -> super::effects::ReadEffects {
    match read_effects(block, 120.0, &mut ItemBudget::new()) {
        Ok(read) => read,
        Err(err) => unreachable!("{err:?}"),
    }
}

// ---------------------------------------------------------------------
// The descriptor parser
// ---------------------------------------------------------------------

#[test]
#[allow(clippy::float_cmp)]
fn descriptor_parses_every_ostype_fixture() {
    let bytes = descriptor_bytes(&[
        (b"obj1", V::Obj(vec![(b"in  ", V::Long(1))])),
        (b"glob", V::Glob(vec![(b"in  ", V::Long(2))])),
        (b"arry", V::ObjArray(vec![(b"in  ", V::Long(3))])),
        (
            b"list",
            V::List(vec![V::Doub(1.5), V::Bool(true), V::Text("x")]),
        ),
        (b"doub", V::Doub(-2.25)),
        (b"untf", V::Unit(b"#Pxl", 18.0)),
        (b"unfl", V::Units(b"#Pxl", vec![1.0, 2.0, 3.0])),
        (b"long", V::Long(-7)),
        (b"comp", V::Comp(1 << 40)),
        (b"bool", V::Bool(true)),
        (b"enum", V::Enum(b"BlnM", b"linearBurn")),
        (b"text", V::Text("線形")),
        (b"typ1", V::Class(b"type", b"Lyr ")),
        (b"typ2", V::Class(b"GlbC", b"Chnl")),
        (b"refs", V::Reference),
        (b"alis", V::Raw(b"alis", b"abc")),
        (b"tdta", V::Raw(b"tdta", b"")),
        (b"path", V::Raw(b"Pth ", b"\x00\x01")),
        (b"a long key name", V::Bool(false)),
    ]);
    let d = parsed(&bytes);
    assert_eq!(d.class, b"null");
    assert_eq!(d.items.len(), 19);
    let inner = |key: &[u8]| {
        d.get(key)
            .and_then(Value::descriptor)
            .and_then(|i| i.get(b"in  "))
            .and_then(Value::number)
    };
    assert_eq!(inner(b"obj1"), Some(1.0));
    assert_eq!(inner(b"glob"), Some(2.0));
    assert_eq!(inner(b"arry"), Some(3.0));
    assert_eq!(
        d.get(b"list").and_then(Value::list).map(<[Value]>::len),
        Some(3)
    );
    assert_eq!(d.get(b"doub").and_then(Value::number), Some(-2.25));
    assert_eq!(
        d.get(b"untf"),
        Some(&Value::Unit {
            unit: *b"#Pxl",
            value: 18.0
        })
    );
    assert_eq!(
        d.get(b"unfl"),
        Some(&Value::Units {
            unit: *b"#Pxl",
            values: vec![1.0, 2.0, 3.0]
        })
    );
    assert_eq!(d.get(b"long").and_then(Value::number), Some(-7.0));
    assert_eq!(d.get(b"comp"), Some(&Value::LargeInteger(1 << 40)));
    assert_eq!(d.get(b"bool").and_then(Value::bool), Some(true));
    assert_eq!(
        d.get(b"enum").and_then(Value::enum_value),
        Some(&b"linearBurn"[..])
    );
    assert_eq!(d.get(b"text"), Some(&Value::Text("線形".to_owned())));
    assert!(matches!(d.get(b"typ1"), Some(Value::Class { class, .. }) if class == b"Lyr "));
    assert!(matches!(d.get(b"typ2"), Some(Value::Class { class, .. }) if class == b"Chnl"));
    assert_eq!(
        d.get(b"refs"),
        Some(&Value::Reference(vec![
            *b"prop", *b"Clss", *b"Enmr", *b"rele", *b"Idnt", *b"indx", *b"name"
        ]))
    );
    assert_eq!(d.get(b"alis"), Some(&Value::Raw(b"abc".to_vec())));
    assert_eq!(d.get(b"tdta"), Some(&Value::Raw(Vec::new())));
    assert_eq!(d.get(b"path"), Some(&Value::Raw(vec![0, 1])));
    assert_eq!(d.get(b"a long key name").and_then(Value::bool), Some(false));

    // An unknown OSType fails the block, with its code.
    let mut bad = descriptor_bytes(&[(b"x   ", V::Long(1))]);
    let at = bad.len() - 8;
    if let Some(slot) = bad.get_mut(at..at + 4) {
        slot.copy_from_slice(b"Zzzz");
    }
    assert_eq!(
        parse_descriptor(&bad, &mut ItemBudget::new()),
        Err(DescriptorError::UnknownType(*b"Zzzz"))
    );
}

/// `depth` descriptors nested inside each other, innermost holding a
/// `long`.
fn nested(depth: usize) -> Vec<u8> {
    // Descriptors carry no length, so a nesting is one repeated prefix
    // (body head, one item `next` of type `Objc`) and a leaf: linear to
    // build however deep.
    let mut level = Vec::new();
    put_unicode(&mut level, "");
    put_key(&mut level, b"null");
    put_u32(&mut level, 1);
    put_key(&mut level, b"next");
    level.extend_from_slice(b"Objc");
    let mut out = level.repeat(depth);
    put_unicode(&mut out, "");
    put_key(&mut out, b"null");
    put_u32(&mut out, 1);
    put_key(&mut out, b"leaf");
    out.extend_from_slice(b"long");
    out.extend_from_slice(&1i32.to_be_bytes());
    out
}

#[test]
fn descriptor_refuses_deep_nesting_without_panic() {
    assert!(parse_descriptor(&nested(MAX_DEPTH), &mut ItemBudget::new()).is_ok());
    assert_eq!(
        parse_descriptor(&nested(MAX_DEPTH + 1), &mut ItemBudget::new()),
        Err(DescriptorError::TooDeep)
    );
    // Far deeper than any stack would survive unbounded recursion of.
    assert_eq!(
        parse_descriptor(&nested(200_000), &mut ItemBudget::new()),
        Err(DescriptorError::TooDeep)
    );
    // Lists count as containers too.
    let mut list = V::Long(1);
    for _ in 0..=MAX_DEPTH {
        list = V::List(vec![list]);
    }
    assert_eq!(
        parse_descriptor(
            &descriptor_bytes(&[(b"l   ", list)]),
            &mut ItemBudget::new()
        ),
        Err(DescriptorError::TooDeep)
    );
}

#[test]
fn descriptor_huge_count_does_not_allocate() {
    // Item count 0xFFFF_FFFF in an 8-byte block (after name and class).
    let mut item_count = Vec::new();
    put_unicode(&mut item_count, "");
    put_key(&mut item_count, b"null");
    let head = item_count.len();
    put_u32(&mut item_count, u32::MAX);
    item_count.extend_from_slice(&[0; 4]);
    assert_eq!(item_count.len() - head, 8);
    // A name of 0xFFFF_FFFF code units; a list, `UnFl` and raw block of
    // 0xFFFF_FFFF items or bytes; an `lfx2` header followed by one.
    let mut name = Vec::new();
    put_u32(&mut name, u32::MAX);
    name.extend_from_slice(&[0; 4]);
    let wrap = |ostype: &[u8; 4], extra: &[u8]| {
        let mut o = Vec::new();
        put_unicode(&mut o, "");
        put_key(&mut o, b"null");
        put_u32(&mut o, 1);
        put_key(&mut o, b"k   ");
        o.extend_from_slice(ostype);
        o.extend_from_slice(extra);
        put_u32(&mut o, u32::MAX);
        o.extend_from_slice(&[0; 4]);
        o
    };
    for bytes in [
        item_count.clone(),
        name,
        wrap(b"VlLs", &[]),
        wrap(b"UnFl", b"#Pxl"),
        wrap(b"tdta", &[]),
        wrap(b"obj ", &[]),
        wrap(b"ObAr", &[0, 0, 0, 1, 0xFF, 0xFF, 0xFF, 0xFF]),
    ] {
        // Exactly the count refusal: a parser that skipped the check
        // (and so reserved nothing, see `PREALLOC_CAP`) would run out of
        // bytes instead and report `Truncated`.
        assert_eq!(
            parse_descriptor(&bytes, &mut ItemBudget::new()),
            Err(DescriptorError::CountTooLarge),
            "{bytes:?}"
        );
    }
    let mut block = vec![0, 0, 0, 0, 0, 0, 0, 16];
    block.extend_from_slice(&item_count);
    assert_eq!(
        parse_effects_block(&block, &mut ItemBudget::new()),
        Err(DescriptorError::CountTooLarge)
    );
}

#[test]
fn descriptor_budget_is_shared_and_spent() {
    let bytes = descriptor_bytes(&[(b"l   ", V::List((0..100).map(V::Long).collect()))]);
    let mut budget = ItemBudget { remaining: 150 };
    assert!(parse_descriptor(&bytes, &mut budget).is_ok());
    assert_eq!(budget.remaining, 150 - 101);
    assert_eq!(
        parse_descriptor(&bytes, &mut budget),
        Err(DescriptorError::BudgetSpent)
    );
    assert_eq!(descriptor::MAX_FILE_ITEMS, 1_000_000);
}

/// Every `key` effects block in a PSD's raw bytes, found by its `8BIM`
/// signature (both are 4-byte-length keys).
pub(super) fn raw_blocks(file: &[u8], key: &[u8; 4]) -> Vec<Vec<u8>> {
    let mut sig = b"8BIM".to_vec();
    sig.extend_from_slice(key);
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(found) = file
        .get(at..)
        .and_then(|rest| rest.windows(8).position(|w| w == sig.as_slice()))
    {
        let start = at + found + 8;
        let len = file
            .get(start..start + 4)
            .and_then(|b| <[u8; 4]>::try_from(b).ok())
            .map(|b| u32::from_be_bytes(b) as usize);
        if let Some(block) = len.and_then(|len| file.get(start + 4..start + 4 + len)) {
            out.push(block.to_vec());
        }
        at = start;
    }
    out
}

fn corpus(name: &str) -> Option<Vec<u8>> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpora/psd/reference/psd-tools-fixtures")
        .join(name);
    match std::fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(_) => {
            println!("SKIPPED: corpus file {} not present", path.display());
            None
        }
    }
}

#[test]
fn descriptor_fuzz_corpus_never_panics() {
    let mut blocks = Vec::new();
    for (file, key) in [
        ("layer_effects.psd", b"lfx2"),
        ("effects/double-stroke-effects.psd", b"lmfx"),
        ("effects/shape-fx.psd", b"lfxs"),
    ] {
        if let Some(bytes) = corpus(file) {
            blocks.extend(raw_blocks(&bytes, key));
        }
    }
    if blocks.is_empty() {
        return;
    }
    // Every real block reads cleanly first.
    for block in &blocks {
        assert!(read_effects(block, 120.0, &mut ItemBudget::new()).is_ok());
    }
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut runs = 0u64;
    for block in blocks.iter().take(6) {
        // Every truncation.
        for len in 0..block.len() {
            let _ = read_effects(
                block.get(..len).unwrap_or_default(),
                120.0,
                &mut ItemBudget::new(),
            );
            runs += 1;
        }
        // One bit flipped at every byte, then random multi-bit damage.
        for i in 0..block.len() {
            let mut damaged = block.clone();
            if let Some(b) = damaged.get_mut(i) {
                *b ^= 1 << (i % 8);
            }
            let _ = read_effects(&damaged, 120.0, &mut ItemBudget::new());
            runs += 1;
        }
        for _ in 0..2000 {
            let mut damaged = block.clone();
            for _ in 0..(next() % 8 + 1) {
                let len = damaged.len().max(1) as u64;
                let i = (next() % len) as usize;
                if let Some(b) = damaged.get_mut(i) {
                    *b = (next() & 0xFF) as u8;
                }
            }
            let _ = read_effects(&damaged, 120.0, &mut ItemBudget::new());
            runs += 1;
        }
    }
    assert!(runs > 10_000, "{runs}");
}

// ---------------------------------------------------------------------
// Mapping
// ---------------------------------------------------------------------

fn find(layers: &LayerTree, name: &str) -> Option<LayerId> {
    let mut stack: Vec<LayerId> = layers.roots().to_vec();
    while let Some(id) = stack.pop() {
        if layers.name(id) == Some(name) {
            return Some(id);
        }
        if let Some(LayerKind::Group { children }) = layers.kind(id) {
            stack.extend(children.iter().copied());
        }
    }
    None
}

fn every_layer(layers: &LayerTree) -> Vec<LayerId> {
    let mut out = Vec::new();
    let mut stack: Vec<LayerId> = layers.roots().to_vec();
    while let Some(id) = stack.pop() {
        out.push(id);
        if let Some(LayerKind::Group { children }) = layers.kind(id) {
            stack.extend(children.iter().copied());
        }
    }
    out
}

fn open(bytes: &[u8]) -> super::PsdDocument {
    match super::read(bytes) {
        Ok(document) => document,
        Err(err) => unreachable!("{err:?}"),
    }
}

#[test]
#[allow(clippy::float_cmp)]
fn lfx2_drop_shadow_fields_match_psd_tools() {
    let Some(bytes) = corpus("layer_effects.psd") else {
        return;
    };
    let document = open(&bytes);
    let Some(id) = find(&document.layers, "Drop Shadow") else {
        unreachable!("no Drop Shadow layer")
    };
    let Some(fx) = document.layers.effects(id) else {
        unreachable!("no effects")
    };
    // psd-tools 1.17.4, `layer.effects` / the raw `lfx2` descriptor:
    // Md Mltp, Clr (0, 0, 0), Opct 35 %, uglg True, lagl 90°, Dstn 18 px,
    // Ckmt 0, blur 41 px, Nose 0 %, AntA False, TrnS linear,
    // layerConceals True, Scl 208.333…%, masterFXSwitch True; the file's
    // global angle (resource 1037) is 90°.
    assert!(fx.enabled);
    assert!((fx.scale - 208.333_33).abs() < 1e-3, "{}", fx.scale);
    assert_eq!(fx.drop_shadows.len(), 1);
    let Some(s) = fx.drop_shadows.first() else {
        return;
    };
    assert!(s.gate.present && s.gate.enabled);
    assert_eq!(s.blend_mode, EffectBlendMode::Multiply);
    assert_eq!((s.color.r, s.color.g, s.color.b), (0.0, 0.0, 0.0));
    assert!((s.opacity - 0.35).abs() < 1e-6);
    assert!(s.use_global_light);
    assert_eq!(s.angle, 90.0);
    assert_eq!(s.local_angle, 90.0);
    assert_eq!(s.distance, 18.0);
    assert_eq!(s.spread, 0.0);
    assert_eq!(s.size, 41.0);
    assert_eq!(s.noise, 0.0);
    assert!(!s.anti_aliased);
    assert!(s.linear_contour);
    assert!(s.knocks_out);
    assert_eq!(fx.shown_drop_shadows(), 1);
    assert!(fx.shows_any());

    // Its "Stroke" layer: FrFX, and the colour overlay on "Color Fill".
    let stroke = find(&document.layers, "Stroke").and_then(|id| document.layers.effects(id));
    assert_eq!(stroke.map(LayerEffects::shown_strokes), Some(1));
    let fill = find(&document.layers, "Color Fill").and_then(|id| document.layers.effects(id));
    assert_eq!(fill.map(LayerEffects::shown_color_overlays), Some(1));
}

#[test]
#[allow(clippy::float_cmp)]
fn lmfx_multiple_instances_are_all_kept() {
    // Synthetic: three drop shadows and two strokes.
    let block = effects_block(&[
        (b"masterFXSwitch", V::Bool(true)),
        (
            b"dropShadowMulti",
            V::List(vec![
                shadow(true, true, 1.0),
                shadow(true, true, 2.0),
                shadow(false, true, 3.0),
            ]),
        ),
        (
            b"frameFXMulti",
            V::List(vec![
                V::Obj(vec![(b"Styl", V::Enum(b"FStl", b"InsF"))]),
                V::Obj(vec![(b"Styl", V::Enum(b"FStl", b"CtrF"))]),
            ]),
        ),
    ]);
    let read = mapped(&block);
    let d: Vec<f32> = read
        .effects
        .drop_shadows
        .iter()
        .map(|s| s.distance)
        .collect();
    assert_eq!(d, vec![1.0, 2.0, 3.0]);
    assert_eq!(read.effects.shown_drop_shadows(), 2);
    let pos: Vec<StrokePosition> = read.effects.strokes.iter().map(|s| s.position).collect();
    assert_eq!(pos, vec![StrokePosition::Inside, StrokePosition::Center]);

    // The real file: two strokes in one `lmfx`, preferred over `lfx2`.
    let Some(bytes) = corpus("effects/double-stroke-effects.psd") else {
        return;
    };
    let document = open(&bytes);
    let strokes: usize = every_layer(&document.layers)
        .into_iter()
        .filter_map(|id| document.layers.effects(id))
        .map(|fx| fx.strokes.len())
        .sum();
    assert_eq!(strokes, 2, "psd-tools: frameFXMulti holds two strokes");
    let fx = every_layer(&document.layers)
        .into_iter()
        .find_map(|id| document.layers.effects(id));
    // psd-tools: Clr (255, 0.00389…, 0.00389…) and (0, 0, 255).
    let colors: Vec<(f32, f32)> = fx
        .map(|fx| fx.strokes.iter().map(|s| (s.color.r, s.color.b)).collect())
        .unwrap_or_default();
    let want = [(1.0, 0.003_891_050_6 / 255.0), (0.0, 1.0)];
    assert_eq!(colors.len(), 2);
    for (got, want) in colors.iter().zip(want) {
        assert!(
            (got.0 - want.0).abs() < 1e-7 && (got.1 - want.1).abs() < 1e-9,
            "{got:?}"
        );
    }

    // A record holding both prefers `lmfx`.
    let lfx2 = effects_block(&[(b"DrSh", shadow(true, true, 9.0))]);
    let doc = open(
        &TestPsd::new(1, 2, 2, 8)
            .with(|p| {
                p.layers = vec![
                    TestLayer::pixels("a", 0, 0, 1, 1, 8, &[[1, 2, 3, 255]]).with(|l| {
                        l.blocks.push((*b"lfx2", lfx2.clone()));
                        l.blocks.push((*b"lmfx", block.clone()));
                    }),
                ];
            })
            .write(),
    );
    let fx = find(&doc.layers, "a").and_then(|id| doc.layers.effects(id));
    assert_eq!(fx.map(|fx| fx.drop_shadows.len()), Some(3));
}

#[test]
fn master_switch_and_enab_and_present_gate_effects() {
    let gated = |master: bool, enab: bool, present: bool| {
        mapped(&effects_block(&[
            (b"masterFXSwitch", V::Bool(master)),
            (b"DrSh", shadow(enab, present, 4.0)),
            (
                b"IrSh",
                V::Obj(vec![
                    (b"enab", V::Bool(enab)),
                    (b"present", V::Bool(present)),
                ]),
            ),
        ]))
        .effects
    };
    let on = gated(true, true, true);
    assert!(on.shows_any());
    assert_eq!(on.shown_drop_shadows(), 1);
    assert_eq!(on.other.inner_shadows, 1);
    for (master, enab, present) in [
        (false, true, true),
        (true, false, true),
        (true, true, false),
    ] {
        let fx = gated(master, enab, present);
        assert!(!fx.shows_any(), "{master} {enab} {present}");
        // Kept as data, not collapsed.
        assert_eq!(fx.enabled, master);
        let Some(s) = fx.drop_shadows.first() else {
            unreachable!("the shadow is kept");
        };
        assert_eq!((s.gate.enabled, s.gate.present), (enab, present));
    }
    // Through an open: a turned-off shadow is stored, but no note.
    let block = effects_block(&[
        (b"masterFXSwitch", V::Bool(false)),
        (b"DrSh", shadow(true, true, 4.0)),
    ]);
    let doc = open(
        &TestPsd::new(1, 2, 2, 8)
            .with(|p| {
                p.layers = vec![
                    TestLayer::pixels("a", 0, 0, 1, 1, 8, &[[1, 2, 3, 255]])
                        .with(|l| l.blocks.push((*b"lfx2", block.clone()))),
                ];
            })
            .write(),
    );
    let fx = find(&doc.layers, "a").and_then(|id| doc.layers.effects(id));
    assert_eq!(
        fx.map(|fx| (fx.enabled, fx.drop_shadows.len())),
        Some((false, 1))
    );
    assert!(
        doc.report.items.iter().all(|i| !i.contains("drop shadow")),
        "{:?}",
        doc.report.items
    );
}

fn global_angle_resource(degrees: i32) -> Vec<u8> {
    let mut o = b"8BIM".to_vec();
    o.extend_from_slice(&1037u16.to_be_bytes());
    o.extend_from_slice(&[0, 0]);
    put_u32(&mut o, 4);
    o.extend_from_slice(&degrees.to_be_bytes());
    o
}

#[test]
#[allow(clippy::float_cmp)]
fn global_angle_resource_is_read_and_defaults_to_120() {
    let mut global = match shadow(true, true, 4.0) {
        V::Obj(items) => items,
        _ => Vec::new(),
    };
    global.retain(|(k, _)| *k != b"uglg");
    global.push((b"uglg", V::Bool(true)));
    let block = effects_block(&[(b"DrSh", V::Obj(global))]);
    let angle_of = |resources: Vec<u8>| {
        let doc = open(
            &TestPsd::new(1, 2, 2, 8)
                .with(|p| {
                    p.resources = resources;
                    p.layers = vec![
                        TestLayer::pixels("a", 0, 0, 1, 1, 8, &[[1, 2, 3, 255]])
                            .with(|l| l.blocks.push((*b"lfx2", block.clone()))),
                    ];
                })
                .write(),
        );
        find(&doc.layers, "a")
            .and_then(|id| doc.layers.effects(id))
            .and_then(|fx| fx.drop_shadows.first().map(|s| (s.angle, s.local_angle)))
    };
    assert_eq!(angle_of(Vec::new()), Some((120.0, 45.0)), "default");
    assert_eq!(angle_of(global_angle_resource(30)), Some((30.0, 45.0)));
    assert_eq!(
        angle_of(global_angle_resource(-100_000)),
        Some((-360.0, 45.0))
    );
    // A shadow not using global light keeps its own angle.
    let local = mapped(&effects_block(&[(b"DrSh", shadow(true, true, 4.0))]));
    assert_eq!(
        local.effects.drop_shadows.first().map(|s| s.angle),
        Some(45.0)
    );
}

#[test]
fn out_of_range_values_are_clamped_and_non_finite_ones_unreadable() {
    let clamped = mapped(&effects_block(&[(
        b"SoFi",
        V::Obj(vec![
            (b"Opct", V::Unit(b"#Prc", 140.0)),
            (b"Clr ", rgb(300.0, 0.0, -5.0)),
        ]),
    )]));
    assert!(clamped.clamped);
    let o = clamped
        .effects
        .color_overlays
        .first()
        .cloned()
        .unwrap_or_default();
    assert!((o.opacity - 1.0).abs() < f32::EPSILON);
    assert!((o.color.r - 1.0).abs() < f32::EPSILON && o.color.b.abs() < f32::EPSILON);
    let many = mapped(&effects_block(&[(
        b"dropShadowMulti",
        V::List((0..12).map(|_| shadow(true, true, 1.0)).collect()),
    )]));
    assert!(many.clamped);
    assert_eq!(many.effects.drop_shadows.len(), 10);
    for bad in [
        effects_block(&[(b"DrSh", V::Obj(vec![(b"blur", V::Doub(f64::NAN))]))]),
        effects_block(&[(b"DrSh", V::Obj(vec![(b"Md  ", V::Enum(b"BlnM", b"what"))]))]),
        effects_block(&[(
            b"SoFi",
            V::Obj(vec![(b"Clr ", V::Obj(vec![(b"Gry ", V::Doub(1.0))]))]),
        )]),
        effects_block(&[(b"DrSh", V::Long(3))]),
    ] {
        assert!(read_effects(&bad, 120.0, &mut ItemBudget::new()).is_err());
    }
    let parsed = parsed(&descriptor_bytes(&[(b"DrSh", shadow(true, true, 1.0))]));
    assert!(map_effects(&parsed, 120.0).is_ok());
}

// ---------------------------------------------------------------------
// Open and save
// ---------------------------------------------------------------------

#[test]
fn per_kind_effect_notes_on_open() {
    let block = effects_block(&[
        (
            b"dropShadowMulti",
            V::List(vec![shadow(true, true, 1.0), shadow(true, true, 2.0)]),
        ),
        (b"SoFi", V::Obj(vec![])),
        (b"FrFX", V::Obj(vec![(b"PntT", V::Enum(b"FrFl", b"GrFl"))])),
        (b"OrGl", V::Obj(vec![(b"GlwT", V::Enum(b"BETE", b"PrBL"))])),
        (b"ebbl", V::Obj(vec![])),
        (b"IrGl", V::Obj(vec![(b"enab", V::Bool(false))])),
    ]);
    let doc = open(
        &TestPsd::new(1, 2, 2, 8)
            .with(|p| {
                p.layers = vec![
                    TestLayer::pixels("a", 0, 0, 1, 1, 8, &[[1, 2, 3, 255]])
                        .with(|l| l.blocks.push((*b"lfx2", block.clone()))),
                    TestLayer::pixels("legacy", 0, 0, 1, 1, 8, &[[1, 2, 3, 255]])
                        .with(|l| l.blocks.push((*b"lrFX", vec![0; 8]))),
                    TestLayer::pixels("bad", 0, 0, 1, 1, 8, &[[1, 2, 3, 255]]).with(|l| {
                        l.blocks
                            .push((*b"lfx2", vec![0, 0, 0, 0, 0, 0, 0, 16, 0xFF]))
                    }),
                ];
            })
            .write(),
    );
    let items = doc.report.items.join("\n");
    for needle in [
        "1 gradient, pattern or other non-solid stroke effect isn't shown",
        "1 outer glow isn't shown (Aurora draws Softer, solid-colour glows",
        "1 other effect (inner shadow, inner glow, bevel",
        "1 layer with layer effects (shadows, strokes, glows): the effects aren't shown.",
        "1 layer has layer effects that couldn't be read",
    ] {
        assert!(items.contains(needle), "{needle:?} missing from\n{items}");
    }
    // 0.181.0/0.182.0: a pixel layer's colour overlay and drop shadows are
    // drawn, so not reported as not shown.
    assert!(!items.contains("colour overlay"), "{items}");
    assert!(!items.contains("drop shadow"), "{items}");
    // The effects are on the layer, as data; nothing on the other two.
    let fx = find(&doc.layers, "a").and_then(|id| doc.layers.effects(id));
    assert_eq!(fx.map(|fx| fx.drop_shadows.len()), Some(2));
    for name in ["legacy", "bad"] {
        assert_eq!(
            find(&doc.layers, name).and_then(|id| doc.layers.effects(id)),
            None
        );
    }
}

#[test]
fn psd_save_itemises_unsaved_effects() {
    let mut layers = LayerTree::new();
    let mut history = aurora_doc::History::new();
    let bounds = aurora_core::Rect {
        x: 0,
        y: 0,
        width: 4,
        height: 4,
    };
    let mut ids = Vec::new();
    for name in ["a", "b", "c"] {
        match history.add_pixel_layer(&mut layers, name, bounds, None) {
            Ok(id) => ids.push(id),
            Err(err) => unreachable!("{err:?}"),
        }
    }
    let opts = super::write::PsdSaveOptions::default();
    let report = |layers: &LayerTree| match super::write::psd_save_preflight(layers, (4, 4), opts) {
        Ok(report) => report.items.join("\n"),
        Err(err) => unreachable!("{err:?}"),
    };
    assert!(!report(&layers).contains("effects"));
    // Turned off still counts: the switch is written with the effects.
    let mut off = LayerEffects::default();
    off.enabled = false;
    off.color_overlays
        .push(aurora_core::effects::ColorOverlay::default());
    for (id, fx) in ids.iter().zip([LayerEffects::default(), off]) {
        let mut fx = fx;
        fx.drop_shadows
            .push(aurora_core::effects::DropShadow::default());
        if let Err(err) = layers.set_effects(*id, Some(fx)) {
            unreachable!("{err:?}");
        }
    }
    let items = report(&layers);
    assert!(
        items.contains("2 layers' effects are saved in the file"),
        "{items}"
    );
}

/// The user's real file (never committed): run with
/// `AURORA_EFFECTS_PSD=images/flat-design-nyepi-template/6739968.psd
/// cargo test -p aurora-io -- --ignored nyepi`.
#[test]
#[ignore = "needs AURORA_EFFECTS_PSD, a file outside the repository"]
fn nyepi_effects_parse_38_drop_shadows_and_9_colour_overlays() {
    let Ok(path) = std::env::var("AURORA_EFFECTS_PSD") else {
        unreachable!("set AURORA_EFFECTS_PSD");
    };
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) => unreachable!("{path}: {err}"),
    };
    let doc = open(&bytes);
    let all: Vec<&LayerEffects> = every_layer(&doc.layers)
        .into_iter()
        .filter_map(|id| doc.layers.effects(id))
        .collect();
    let shadows: usize = all.iter().map(|fx| fx.shown_drop_shadows()).sum();
    let overlays: usize = all.iter().map(|fx| fx.shown_color_overlays()).sum();
    println!(
        "{} layers with effects; {shadows} drop shadows, {overlays} colour overlays shown; report:\n{}",
        all.len(),
        doc.report.items.join("\n")
    );
    assert_eq!(shadows, 38);
    assert_eq!(overlays, 9);
}

#[test]
fn a_spread_tagged_in_pixels_is_still_a_percentage() {
    // `layer_effects.psd` tags `Ckmt` `#Pxl` although Photoshop's dialog
    // shows it as a percentage; the value is read as a percentage either
    // way, so 20 is a spread of 0.2 (not clamped to 1.0).
    let read = mapped(&effects_block(&[
        (b"DrSh", V::Obj(vec![(b"Ckmt", V::Unit(b"#Pxl", 20.0))])),
        (b"OrGl", V::Obj(vec![(b"Ckmt", V::Unit(b"#Pxl", 20.0))])),
    ]));
    assert!(!read.clamped);
    let spread = read.effects.drop_shadows.first().map(|s| s.spread);
    assert!(spread.is_some_and(|s| (s - 0.2).abs() < 1e-6), "{spread:?}");
    let glow = read.effects.outer_glows.first().map(|g| g.spread);
    assert!(glow.is_some_and(|s| (s - 0.2).abs() < 1e-6), "{glow:?}");
}

#[test]
fn a_damaged_lmfx_falls_back_to_lfx2_before_unreadable() {
    let lfx2 = effects_block(&[(b"DrSh", shadow(true, true, 9.0))]);
    let lfxs = effects_block(&[(b"DrSh", shadow(true, true, 5.0))]);
    let damaged = vec![0, 0, 0, 0, 0, 0, 0, 16, 0xFF];
    let open_with = |blocks: Vec<([u8; 4], Vec<u8>)>| {
        let doc = open(
            &TestPsd::new(1, 2, 2, 8)
                .with(|p| {
                    p.layers = vec![
                        TestLayer::pixels("a", 0, 0, 1, 1, 8, &[[1, 2, 3, 255]])
                            .with(|l| l.blocks.extend(blocks)),
                    ];
                })
                .write(),
        );
        let distance = find(&doc.layers, "a")
            .and_then(|id| doc.layers.effects(id))
            .and_then(|fx| fx.drop_shadows.first().map(|s| s.distance));
        (distance, doc.report.items.join("\n"))
    };
    // lmfx damaged: lfx2 is used, nothing reported unreadable.
    let (distance, items) = open_with(vec![
        (*b"lmfx", damaged.clone()),
        (*b"lfx2", lfx2.clone()),
        (*b"lfxs", lfxs.clone()),
    ]);
    assert_eq!(distance, Some(9.0));
    assert!(!items.contains("couldn't be read"), "{items}");
    // lmfx and lfx2 damaged: lfxs.
    let (distance, _) = open_with(vec![
        (*b"lfx2", damaged.clone()),
        (*b"lmfx", damaged.clone()),
        (*b"lfxs", lfxs),
    ]);
    assert_eq!(distance, Some(5.0));
    // Every block damaged: unreadable, reported once.
    let (distance, items) = open_with(vec![(*b"lmfx", damaged.clone()), (*b"lfx2", damaged)]);
    assert_eq!(distance, None);
    assert!(
        items.contains("1 layer has layer effects that couldn't be read"),
        "{items}"
    );
}

#[test]
fn an_adjustment_layers_effects_are_reported_as_not_kept() {
    let block = effects_block(&[(b"DrSh", shadow(true, true, 4.0))]);
    let file = super::tests::curves_file(&super::tests::curv_rgb_block(), |l| {
        l.blocks.push((*b"lfx2", block.clone()));
    })
    .write();
    let doc = open(&file);
    let items = doc.report.items.join("\n");
    assert!(
        items.contains(
            "1 adjustment layer has layer effects, which Aurora doesn't show or keep on an \
             adjustment layer."
        ),
        "{items}"
    );
    assert!(!items.contains("kept with the layer"), "{items}");
    assert!(
        every_layer(&doc.layers)
            .into_iter()
            .all(|id| doc.layers.effects(id).is_none())
    );
}

/// 0.181.0: colour overlays on a pixel layer are drawn and no longer
/// reported; on a group (effects on groups are E9) they still are.
#[test]
fn colour_overlays_are_reported_only_on_groups() {
    let block = effects_block(&[(b"SoFi", V::Obj(vec![]))]);
    let doc = open(
        &TestPsd::new(1, 2, 2, 8)
            .with(|p| {
                p.layers = vec![
                    TestLayer::divider(),
                    TestLayer::pixels("inside", 0, 0, 1, 1, 8, &[[1, 2, 3, 255]])
                        .with(|l| l.blocks.push((*b"lfx2", block.clone()))),
                    TestLayer::group("g", *b"norm")
                        .with(|l| l.blocks.push((*b"lfx2", block.clone()))),
                ];
            })
            .write(),
    );
    let items = doc.report.items.join("\n");
    assert!(
        items.contains("1 colour overlay isn't shown yet"),
        "only the group's: {items}"
    );
    for name in ["inside", "g"] {
        let fx = find(&doc.layers, name).and_then(|id| doc.layers.effects(id));
        assert_eq!(fx.map(|fx| fx.color_overlays.len()), Some(1), "{name}");
    }
}

/// Review of 0.181.0, revised in 0.201.0: a clipped layer's colour overlay
/// is drawn and the layer is now clipped to its base, so the open report no
/// longer says it is shown unclipped.
#[test]
fn overlays_on_clipped_layers_are_no_longer_reported_as_unclipped() {
    let block = effects_block(&[(b"SoFi", V::Obj(vec![]))]);
    let doc = open(
        &TestPsd::new(1, 2, 2, 8)
            .with(|p| {
                p.layers = vec![
                    TestLayer::pixels("base", 0, 0, 1, 1, 8, &[[1, 2, 3, 255]]),
                    TestLayer::pixels("clipped", 0, 0, 1, 1, 8, &[[1, 2, 3, 255]]).with(|l| {
                        l.clipping = 1;
                        l.blocks.push((*b"lfx2", block.clone()));
                    }),
                    TestLayer::pixels("free", 0, 0, 1, 1, 8, &[[1, 2, 3, 255]])
                        .with(|l| l.blocks.push((*b"lfx2", block.clone()))),
                ];
            })
            .write(),
    );
    let items = doc.report.items.join("\n");
    assert!(!items.contains("unclipped"), "{items}");
    assert!(!items.contains("isn't shown yet"), "{items}");
    let clipped = find(&doc.layers, "clipped");
    assert_eq!(
        clipped.and_then(|id| doc.layers.clip_base(id)),
        find(&doc.layers, "base")
    );
    // The effects data itself is untouched (E7 writes it back).
    let fx = find(&doc.layers, "clipped").and_then(|id| doc.layers.effects(id));
    assert_eq!(
        fx.map(aurora_core::LayerEffects::shown_color_overlays),
        Some(1)
    );
}

/// 0.182.0: a pixel layer's drop shadows are drawn, so not "not shown";
/// noise is reported (a non-Linear contour likewise); a group's shadows
/// are still not shown (E9). A capped size cannot come from a valid file
/// (see `MAX_SHADOW_APRON`).
#[test]
fn drop_shadow_notes_on_open() {
    let mut odd = match shadow(true, true, 4.0) {
        V::Obj(items) => items,
        _ => Vec::new(),
    };
    odd.retain(|(k, _)| *k != b"Nose" && *k != b"blur");
    odd.push((b"Nose", V::Unit(b"#Prc", 10.0)));
    odd.push((b"blur", V::Unit(b"#Pxl", 250.0)));
    let odd = effects_block(&[(b"Scl ", V::Unit(b"#Prc", 400.0)), (b"DrSh", V::Obj(odd))]);
    let plain = effects_block(&[(b"DrSh", shadow(true, true, 4.0))]);
    let doc = open(
        &TestPsd::new(1, 2, 2, 8)
            .with(|p| {
                p.layers = vec![
                    TestLayer::divider(),
                    TestLayer::pixels("inside", 0, 0, 1, 1, 8, &[[1, 2, 3, 255]])
                        .with(|l| l.blocks.push((*b"lfx2", plain.clone()))),
                    TestLayer::group("g", *b"norm")
                        .with(|l| l.blocks.push((*b"lfx2", plain.clone()))),
                    TestLayer::pixels("odd", 0, 0, 1, 1, 8, &[[1, 2, 3, 255]])
                        .with(|l| l.blocks.push((*b"lfx2", odd.clone()))),
                ];
            })
            .write(),
    );
    let items = doc.report.items.join("\n");
    for needle in [
        "1 drop shadow isn't shown yet",
        "1 drop shadow with noise is drawn without the noise.",
        "1 layer style with an effect scale other than 100 % is drawn at the effects' stored sizes",
    ] {
        assert!(items.contains(needle), "{needle:?} missing from\n{items}");
    }
}

/// 0.182.0: a `uglg` shadow is drawn at the document's global angle
/// (resource 1037), not its own `lagl`: with a global 90° and a local 0°
/// it falls straight down.
#[test]
fn uglg_uses_resource_1037_not_local_angle() {
    let mut items = match shadow(true, true, 10.0) {
        V::Obj(items) => items,
        _ => Vec::new(),
    };
    items.retain(|(k, _)| *k != b"uglg" && *k != b"lagl");
    items.push((b"uglg", V::Bool(true)));
    items.push((b"lagl", V::Unit(b"#Ang", 0.0)));
    let block = effects_block(&[(b"DrSh", V::Obj(items))]);
    let doc = open(
        &TestPsd::new(1, 2, 2, 8)
            .with(|p| {
                p.resources = global_angle_resource(90);
                p.layers = vec![
                    TestLayer::pixels("a", 0, 0, 1, 1, 8, &[[1, 2, 3, 255]])
                        .with(|l| l.blocks.push((*b"lfx2", block.clone()))),
                ];
            })
            .write(),
    );
    let fx = find(&doc.layers, "a").and_then(|id| doc.layers.effects(id));
    let shadow = fx.and_then(|fx| fx.drop_shadows.first());
    assert_eq!(shadow.map(|s| (s.angle, s.local_angle)), Some((90.0, 0.0)));
    assert_eq!(shadow.map(|s| s.geometry().offset), Some((0, 10)));
}

/// 0.190.0: a pixel layer's solid-colour stroke is drawn, so it is not
/// "not shown"; a gradient or pattern stroke never is, and neither is any
/// stroke on a group (E9). A clipped layer's stroke is drawn within its clip
/// (0.201.0). The PSD-save report still itemises a layer whose only
/// effect is an undrawn stroke.
#[test]
fn stroke_notes_on_open_and_psd_save() {
    let solid = effects_block(&[(b"FrFX", V::Obj(vec![]))]);
    let gradient = effects_block(&[(b"FrFX", V::Obj(vec![(b"PntT", V::Enum(b"FrFl", b"GrFl"))]))]);
    let pattern = effects_block(&[(b"FrFX", V::Obj(vec![(b"PntT", V::Enum(b"FrFl", b"Ptrn"))]))]);
    let doc = open(
        &TestPsd::new(1, 2, 2, 8)
            .with(|p| {
                p.layers = vec![
                    TestLayer::pixels("solid", 0, 0, 1, 1, 8, &[[1, 2, 3, 255]])
                        .with(|l| l.blocks.push((*b"lfx2", solid.clone()))),
                    TestLayer::pixels("clipped", 0, 0, 1, 1, 8, &[[1, 2, 3, 255]]).with(|l| {
                        l.clipping = 1;
                        l.blocks.push((*b"lfx2", solid.clone()));
                    }),
                    TestLayer::pixels("grad", 0, 0, 1, 1, 8, &[[1, 2, 3, 255]])
                        .with(|l| l.blocks.push((*b"lfx2", gradient.clone()))),
                    TestLayer::pixels("pat", 0, 0, 1, 1, 8, &[[1, 2, 3, 255]])
                        .with(|l| l.blocks.push((*b"lfx2", pattern.clone()))),
                ];
            })
            .write(),
    );
    let items = doc.report.items.join("\n");
    assert!(
        items.contains("2 gradient, pattern or other non-solid stroke effects aren't shown"),
        "{items}"
    );
    assert!(!items.contains("unclipped"), "{items}");
    // A group's stroke is not shown, whatever its fill.
    let doc = open(
        &TestPsd::new(1, 2, 2, 8)
            .with(|p| {
                p.layers = vec![
                    TestLayer::divider(),
                    TestLayer::pixels("inside", 0, 0, 1, 1, 8, &[[1, 2, 3, 255]]),
                    TestLayer::group("g", *b"norm")
                        .with(|l| l.blocks.push((*b"lfx2", solid.clone()))),
                ];
            })
            .write(),
    );
    let items = doc.report.items.join("\n");
    assert!(
        items.contains("1 gradient, pattern or other non-solid stroke effect isn't shown"),
        "{items}"
    );
    // The kept data: the gradient stroke is stored and still counted.
    let fx = find(&doc.layers, "g").and_then(|id| doc.layers.effects(id));
    assert_eq!(fx.map(LayerEffects::shown_strokes), Some(1));
    // PSD save: a layer with only a gradient stroke is itemised.
    let mut layers = LayerTree::new();
    let mut history = aurora_doc::History::new();
    let bounds = aurora_core::Rect {
        x: 0,
        y: 0,
        width: 4,
        height: 4,
    };
    let id = match history.add_pixel_layer(&mut layers, "a", bounds, None) {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    let mut fx = LayerEffects::default();
    fx.strokes.push(aurora_core::effects::Stroke {
        fill: aurora_core::effects::StrokeFill::Gradient,
        ..aurora_core::effects::Stroke::default()
    });
    if let Err(err) = layers.set_effects(id, Some(fx)) {
        unreachable!("{err:?}");
    }
    let opts = super::write::PsdSaveOptions::default();
    let items = match super::write::psd_save_preflight(&layers, (4, 4), opts) {
        Ok(report) => report.items.join("\n"),
        Err(err) => unreachable!("{err:?}"),
    };
    // Aurora could not draw it, so it is not written, and the report says so.
    assert!(
        items.contains("Not saved (unverified in Photoshop): gradient stroke on"),
        "{items}"
    );
}

// ---------------------------------------------------------------------
// 0.191.0 (E5): outer glows
// ---------------------------------------------------------------------

#[test]
fn a_gradient_outer_glow_is_counted_and_not_stored_and_a_contour_is_counted() {
    let read = mapped(&effects_block(&[(
        b"OrGl",
        V::Obj(vec![(b"Grad", V::Obj(vec![]))]),
    )]));
    assert!(read.effects.outer_glows.is_empty());
    assert_eq!((read.glow_gradients, read.glow_contours), (1, 0));
    // A gradient glow that is switched off is not counted.
    let read = mapped(&effects_block(&[(
        b"OrGl",
        V::Obj(vec![(b"Grad", V::Obj(vec![])), (b"enab", V::Bool(false))]),
    )]));
    assert_eq!(read.glow_gradients, 0);
    // A non-linear contour keeps the glow and is counted; a linear one is not.
    let read = mapped(&effects_block(&[(
        b"OrGl",
        V::Obj(vec![(b"TrnS", V::Obj(vec![(b"Crv ", V::List(vec![]))]))]),
    )]));
    assert_eq!(read.effects.outer_glows.len(), 1);
    assert_eq!((read.glow_gradients, read.glow_contours), (0, 1));
    let read = mapped(&effects_block(&[(b"OrGl", V::Obj(vec![]))]));
    assert_eq!(read.glow_contours, 0);
}

#[test]
fn outer_glow_notes_name_what_is_not_drawn() {
    let block = effects_block(&[(
        b"OrGl",
        V::Obj(vec![
            (b"Nose", V::Unit(b"#Prc", 20.0)),
            (b"TrnS", V::Obj(vec![(b"Crv ", V::List(vec![]))])),
        ]),
    )]);
    let precise = effects_block(&[(b"OrGl", V::Obj(vec![(b"GlwT", V::Enum(b"BETE", b"PrBL"))]))]);
    let gradient = effects_block(&[(b"OrGl", V::Obj(vec![(b"Grad", V::Obj(vec![]))]))]);
    let doc = open(
        &TestPsd::new(1, 2, 2, 8)
            .with(|p| {
                p.layers = vec![
                    TestLayer::pixels("soft", 0, 0, 1, 1, 8, &[[1, 2, 3, 255]])
                        .with(|l| l.blocks.push((*b"lfx2", block.clone()))),
                    TestLayer::pixels("precise", 0, 0, 1, 1, 8, &[[1, 2, 3, 255]])
                        .with(|l| l.blocks.push((*b"lfx2", precise.clone()))),
                    TestLayer::pixels("gradient", 0, 0, 1, 1, 8, &[[1, 2, 3, 255]])
                        .with(|l| l.blocks.push((*b"lfx2", gradient.clone()))),
                ];
            })
            .write(),
    );
    let items = doc.report.items.join("\n");
    for needle in [
        "1 outer glow with noise is drawn without the noise.",
        "1 outer glow with a contour other than Linear is drawn with a Linear one.",
        // The Precise one and the gradient one.
        "2 outer glows aren't shown (Aurora draws Softer, solid-colour glows",
    ] {
        assert!(items.contains(needle), "{needle:?} missing from\n{items}");
    }
}

/// A `GrFl` descriptor: `Grad` of two colour stops, angle 30, reversed.
fn gradient_fill<'a>(enab: bool, extra: Vec<(&'a [u8], V<'a>)>) -> V<'a> {
    let stop = |r: f64, g: f64, b: f64, at: f64| {
        V::Obj(vec![
            (b"Clr ", rgb(r, g, b)),
            (b"Type", V::Enum(b"Clry", b"UsrS")),
            (b"Lctn", V::Long(at as i32)),
            (b"Mdpn", V::Long(50)),
        ])
    };
    let mut items = vec![
        (b"enab" as &[u8], V::Bool(enab)),
        (b"present", V::Bool(true)),
        (b"Md  ", V::Enum(b"BlnM", b"Mltp")),
        (b"Opct", V::Unit(b"#Prc", 40.0)),
        (
            b"Grad",
            V::Obj(vec![
                (b"GrdF", V::Enum(b"GrdF", b"CstS")),
                (
                    b"Clrs",
                    V::List(vec![
                        stop(255.0, 0.0, 0.0, 0.0),
                        stop(0.0, 0.0, 255.0, 4096.0),
                    ]),
                ),
                (
                    b"Trns",
                    V::List(vec![V::Obj(vec![
                        (b"Opct", V::Unit(b"#Prc", 100.0)),
                        (b"Lctn", V::Long(0)),
                        (b"Mdpn", V::Long(50)),
                    ])]),
                ),
            ]),
        ),
        (b"Angl", V::Unit(b"#Ang", 30.0)),
        (b"Type", V::Enum(b"GrdT", b"Lnr ")),
        (b"Rvrs", V::Bool(true)),
        (b"Dthr", V::Bool(true)),
        (b"Algn", V::Bool(true)),
        (b"Scl ", V::Unit(b"#Prc", 80.0)),
    ];
    items.retain(|(k, _)| !extra.iter().any(|(e, _)| e == k));
    items.extend(extra);
    V::Obj(items)
}

/// A two-stop `Grad` descriptor with an optional `Intr` (smoothness).
fn grad_with_intr(intr: Option<f64>) -> V<'static> {
    let stop = |r: f64, at: i32| {
        V::Obj(vec![
            (b"Clr ", rgb(r, 0.0, 0.0)),
            (b"Type", V::Enum(b"Clry", b"UsrS")),
            (b"Lctn", V::Long(at)),
            (b"Mdpn", V::Long(50)),
        ])
    };
    let mut items = vec![
        (b"GrdF" as &[u8], V::Enum(b"GrdF", b"CstS")),
        (b"Clrs", V::List(vec![stop(255.0, 0), stop(0.0, 4096)])),
    ];
    if let Some(v) = intr {
        items.push((b"Intr", V::Doub(v)));
    }
    V::Obj(items)
}

/// Task 147: a shown gradient overlay or gradient stroke whose Smoothness is
/// neither 0 nor 100 % is counted (drawn at 100 %); 0, 100 %, absent and a
/// hidden one are not.
#[test]
fn partial_gradient_smoothness_is_counted_for_overlays_and_strokes() {
    let count = |intr: Option<f64>, enab: bool| {
        let overlay = gradient_fill(enab, vec![(b"Grad", grad_with_intr(intr))]);
        mapped(&effects_block(&[(b"GrFl", overlay)])).partial_smoothness
    };
    assert_eq!(count(Some(2048.0), true), 1);
    assert_eq!(count(Some(1.0), true), 1);
    assert_eq!(count(Some(4096.0), true), 0);
    assert_eq!(count(Some(0.0), true), 0);
    assert_eq!(count(None, true), 0);
    assert_eq!(
        count(Some(2048.0), false),
        0,
        "a hidden overlay says nothing"
    );
    let stroke = |intr: Option<f64>| {
        let frame = V::Obj(vec![
            (b"enab", V::Bool(true)),
            (b"present", V::Bool(true)),
            (b"PntT", V::Enum(b"FrFl", b"GrFl")),
            (b"Grad", grad_with_intr(intr)),
            (b"Angl", V::Unit(b"#Ang", 0.0)),
            (b"Type", V::Enum(b"GrdT", b"Lnr ")),
            (b"Algn", V::Bool(true)),
        ]);
        mapped(&effects_block(&[(b"FrFX", frame)])).partial_smoothness
    };
    assert_eq!(stroke(Some(3000.0)), 1);
    assert_eq!(stroke(Some(4096.0)), 0);
}

#[test]
fn a_gradient_overlay_is_modelled_with_every_field() {
    let read = mapped(&effects_block(&[(b"GrFl", gradient_fill(true, vec![]))]));
    let fx = &read.effects;
    assert_eq!(fx.gradient_overlays.len(), 1);
    assert_eq!(fx.other.gradient_overlays, 0);
    let Some(o) = fx.gradient_overlays.first() else {
        unreachable!("one overlay");
    };
    assert_eq!(o.blend_mode, EffectBlendMode::Multiply);
    assert!((o.opacity - 0.4).abs() < 1e-6);
    assert!(o.dither && o.gate.on() && o.reference.is_none());
    let g = &o.gradient;
    assert!((g.angle - 30.0).abs() < 1e-9 && (g.scale - 0.8).abs() < 1e-9);
    assert!(g.reverse && g.align);
    assert_eq!(g.style, aurora_core::GradientStyle::Linear);
    assert_eq!(g.colours.len(), 2);
    assert_eq!(g.colours.first().map(|s| s.value), Some([1.0, 0.0, 0.0]));
    assert_eq!(g.colours.get(1).map(|s| s.pos), Some(1.0));
    assert_eq!(g.alphas.len(), 1);
}

#[test]
fn a_gradient_multi_list_keeps_every_instance_in_order() {
    let read = mapped(&effects_block(&[(
        b"gradientFillMulti",
        V::List(vec![
            gradient_fill(true, vec![]),
            gradient_fill(false, vec![]),
        ]),
    )]));
    let gates: Vec<bool> = read
        .effects
        .gradient_overlays
        .iter()
        .map(|o| o.gate.on())
        .collect();
    assert_eq!(gates, vec![true, false]);
    assert_eq!(read.effects.shown_gradient_overlays(), 1);
}

#[test]
fn unmodelled_gradient_overlays_are_counted_only_when_shown() {
    let noise = |enab: bool| {
        let mut v = gradient_fill(enab, vec![]);
        if let V::Obj(items) = &mut v {
            items.retain(|(k, _)| *k != b"Grad".as_slice());
            items.push((b"Grad", V::Obj(vec![(b"GrdF", V::Enum(b"GrdF", b"ClNs"))])));
        }
        v
    };
    let read = mapped(&effects_block(&[(b"GrFl", noise(true))]));
    assert!(read.effects.gradient_overlays.is_empty());
    assert_eq!(read.effects.other.gradient_overlays, 1);
    assert_eq!(
        mapped(&effects_block(&[(b"GrFl", noise(false))]))
            .effects
            .other
            .gradient_overlays,
        0
    );
    // "Align with layer" off: not modelled, counted.
    let unaligned = gradient_fill(true, vec![(b"Algn", V::Bool(false))]);
    let read = mapped(&effects_block(&[(b"GrFl", unaligned)]));
    assert!(read.effects.gradient_overlays.is_empty());
    assert_eq!(read.effects.other.gradient_overlays, 1);
    // An unknown style, and a missing Grad, likewise.
    let style = gradient_fill(true, vec![(b"Type", V::Enum(b"GrdT", b"Xxxx"))]);
    assert_eq!(
        mapped(&effects_block(&[(b"GrFl", style)]))
            .effects
            .other
            .gradient_overlays,
        1
    );
    let none = V::Obj(vec![(b"enab", V::Bool(true))]);
    assert_eq!(
        mapped(&effects_block(&[(b"GrFl", none)]))
            .effects
            .other
            .gradient_overlays,
        1
    );
}

#[test]
fn a_hostile_gradient_overlay_never_panics_or_allocates_past_the_stop_cap() {
    let stop = |at: i32| V::Obj(vec![(b"Clr ", rgb(1.0, 2.0, 3.0)), (b"Lctn", V::Long(at))]);
    let many: Vec<V<'_>> = (0..300).map(|i| stop(i * 10)).collect();
    let hostile = V::Obj(vec![
        (b"enab", V::Bool(true)),
        (
            b"Grad",
            V::Obj(vec![
                (b"GrdF", V::Enum(b"GrdF", b"CstS")),
                (b"Clrs", V::List(many)),
            ]),
        ),
    ]);
    let read = mapped(&effects_block(&[(b"GrFl", hostile)]));
    assert!(read.effects.gradient_overlays.is_empty());
    assert_eq!(read.effects.other.gradient_overlays, 1);
    // NaN scale and an out-of-range scale: counted, no panic.
    for scale in [f64::NAN, 0.0, 1.0e9] {
        let g = gradient_fill(true, vec![(b"Scl ", V::Unit(b"#Prc", scale))]);
        let read = mapped(&effects_block(&[(b"GrFl", g)]));
        assert!(read.effects.gradient_overlays.is_empty(), "{scale}");
    }
}

#[test]
fn a_drawn_gradient_overlay_is_not_reported_and_keeps_its_record_rectangle() {
    let block = effects_block(&[(b"GrFl", gradient_fill(true, vec![]))]);
    let doc = open(
        &TestPsd::new(1, 2, 2, 8)
            .with(|p| {
                p.layers = vec![
                    TestLayer::pixels("px", 0, 0, 1, 1, 8, &[[1, 2, 3, 255]])
                        .with(|l| l.blocks.push((*b"lfx2", block.clone()))),
                ];
            })
            .write(),
    );
    let items = doc.report.items.join("\n");
    assert!(!items.contains("other effect"), "{items}");
    let Some(id) = find(&doc.layers, "px") else {
        unreachable!("layer");
    };
    let fx = doc.layers.effects(id);
    assert_eq!(fx.map(|f| f.gradient_overlays.len()), Some(1));
    // The overlay's box is the record rectangle (here 1 x 1), relative to
    // the layer's own bounds.
    let r = fx
        .and_then(|f| f.gradient_overlays.first())
        .and_then(|o| o.reference);
    assert_eq!(
        r.map(|r| (r.dx, r.dy, r.width, r.height)),
        Some((0, 0, 1, 1))
    );
}

#[test]
fn psd_save_still_itemises_effects_for_a_layer_with_only_a_gradient_overlay() {
    let mut layers = LayerTree::new();
    let mut history = aurora_doc::History::new();
    let bounds = aurora_core::Rect {
        x: 0,
        y: 0,
        width: 4,
        height: 4,
    };
    let id = match history.add_pixel_layer(&mut layers, "g", bounds, None) {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    let read = mapped(&effects_block(&[(b"GrFl", gradient_fill(true, vec![]))]));
    assert_eq!(read.effects.gradient_overlays.len(), 1);
    if let Err(err) = layers.set_effects(id, Some(read.effects)) {
        unreachable!("{err:?}");
    }
    let opts = super::write::PsdSaveOptions::default();
    let items = match super::write::psd_save_preflight(&layers, (4, 4), opts) {
        Ok(report) => report.items.join("\n"),
        Err(err) => unreachable!("{err:?}"),
    };
    assert!(
        items.contains("Not saved (unverified in Photoshop): gradient overlay on"),
        "{items}"
    );
}

// ---- task 111: pattern overlays ----

/// A `patternFill` descriptor: Water at 200 %, 50 %, Multiply, linked, no
/// phase; `extra` replaces items by key.
fn pattern_fill<'a>(enab: bool, extra: Vec<(&'a [u8], V<'a>)>) -> V<'a> {
    let mut items = vec![
        (b"enab" as &[u8], V::Bool(enab)),
        (b"present", V::Bool(true)),
        (b"Md  ", V::Enum(b"BlnM", b"Mltp")),
        (b"Opct", V::Unit(b"#Prc", 50.0)),
        (
            b"Ptrn",
            V::Obj(vec![
                (b"Nm  ", V::Text("Water\0")),
                (b"Idnt", V::Text("2ab8ff7f-7c39\0")),
            ]),
        ),
        (b"Scl ", V::Unit(b"#Prc", 200.0)),
        (b"Algn", V::Bool(true)),
        (
            b"phase",
            V::Obj(vec![(b"Hrzn", V::Doub(0.0)), (b"Vrtc", V::Doub(0.0))]),
        ),
    ];
    items.retain(|(k, _)| !extra.iter().any(|(e, _)| e == k));
    items.extend(extra);
    V::Obj(items)
}

#[test]
fn a_pattern_overlay_is_modelled_with_every_field() {
    let read = mapped(&effects_block(&[(
        b"patternFill",
        pattern_fill(true, vec![]),
    )]));
    let fx = &read.effects;
    assert_eq!(fx.pattern_overlays.len(), 1);
    let Some(o) = fx.pattern_overlays.first() else {
        unreachable!();
    };
    // The trailing NUL of the descriptor's text is not part of the ID.
    assert_eq!(o.pattern, "2ab8ff7f-7c39");
    assert_eq!(o.blend_mode, EffectBlendMode::Multiply);
    assert!((o.opacity - 0.5).abs() < 1e-6);
    assert!((o.scale - 2.0).abs() < 1e-6);
    assert!(o.gate.on());
    assert_eq!(fx.other.pattern_overlays, 0);
    assert_eq!((read.pattern_unreadable, read.pattern_unplaced), (0, 0));
    assert_eq!(fx.shown_pattern_overlays(), 1);
    assert!(fx.shows_any());
}

#[test]
fn unplaced_or_unnamed_pattern_overlays_are_counted_only_when_shown() {
    let unlinked = |on| pattern_fill(on, vec![(b"Algn", V::Bool(false))]);
    let phased = |on| {
        pattern_fill(
            on,
            vec![(
                b"phase",
                V::Obj(vec![(b"Hrzn", V::Doub(3.0)), (b"Vrtc", V::Doub(0.0))]),
            )],
        )
    };
    let unnamed = |on| {
        V::Obj(vec![
            (b"enab" as &[u8], V::Bool(on)),
            (b"present", V::Bool(true)),
            (b"Md  ", V::Enum(b"BlnM", b"Nrml")),
        ])
    };
    for (make, unreadable, unplaced) in [
        (Box::new(unlinked) as Box<dyn Fn(bool) -> V<'static>>, 0, 1),
        (Box::new(phased), 0, 1),
        (Box::new(unnamed), 1, 0),
    ] {
        let on = mapped(&effects_block(&[(b"patternFill", make(true))]));
        assert!(on.effects.pattern_overlays.is_empty());
        assert_eq!(on.effects.other.pattern_overlays, 1);
        assert_eq!(
            (on.pattern_unreadable, on.pattern_unplaced),
            (unreadable, unplaced)
        );
        let off = mapped(&effects_block(&[(b"patternFill", make(false))]));
        assert!(off.effects.pattern_overlays.is_empty());
        assert_eq!(off.effects.other.pattern_overlays, 0);
        assert_eq!((off.pattern_unreadable, off.pattern_unplaced), (0, 0));
    }
    // A disabled but well-formed overlay is kept (and not shown).
    let read = mapped(&effects_block(&[(
        b"patternFill",
        pattern_fill(false, vec![]),
    )]));
    assert_eq!(read.effects.pattern_overlays.len(), 1);
    assert_eq!(read.effects.shown_pattern_overlays(), 0);
}

#[test]
fn a_pattern_scale_outside_photoshops_range_is_clamped_and_flagged() {
    let read = mapped(&effects_block(&[(
        b"patternFill",
        pattern_fill(true, vec![(b"Scl ", V::Unit(b"#Prc", 5000.0))]),
    )]));
    assert!(read.clamped);
    assert!(
        read.effects
            .pattern_overlays
            .first()
            .is_some_and(|o| (o.scale - 10.0).abs() < 1e-6)
    );
    let read = mapped(&effects_block(&[(
        b"patternFill",
        pattern_fill(true, vec![(b"Scl ", V::Unit(b"#Prc", 0.0))]),
    )]));
    assert!(
        read.effects
            .pattern_overlays
            .first()
            .is_some_and(|o| (o.scale - 0.01).abs() < 1e-6)
    );
}

// ---- task 118: gradient and pattern strokes ----

/// A `FrFX` descriptor with the paint type `paint` (`GrFl` or `Ptrn`),
/// whose gradient / pattern items come from `source` (a `gradient_fill` or
/// `pattern_fill` descriptor's items); `extra` replaces items by key.
fn painted_stroke<'a>(paint: &'static [u8], source: V<'a>, extra: Vec<(&'a [u8], V<'a>)>) -> V<'a> {
    let mut items = vec![
        (b"enab" as &[u8], V::Bool(true)),
        (b"present", V::Bool(true)),
        (b"PntT", V::Enum(b"FrFl", paint)),
        (b"Styl", V::Enum(b"FStl", b"OutF")),
        (b"Md  ", V::Enum(b"BlnM", b"Nrml")),
        (b"Opct", V::Unit(b"#Prc", 70.0)),
        (b"Sz  ", V::Unit(b"#Pxl", 5.0)),
    ];
    if let V::Obj(source_items) = source {
        for (key, value) in source_items {
            let skip = [
                b"enab".as_slice(),
                b"present".as_slice(),
                b"Md  ".as_slice(),
                b"Opct".as_slice(),
            ];
            if !skip.contains(&key) {
                items.push((key, value));
            }
        }
    }
    for (key, value) in extra {
        items.retain(|(k, _)| *k != key);
        items.push((key, value));
    }
    V::Obj(items)
}

#[test]
fn a_gradient_stroke_is_painted_when_its_gradient_reads() {
    use aurora_core::effects::{StrokeFill, StrokePaint};
    let read = mapped(&effects_block(&[(
        b"FrFX",
        painted_stroke(b"GrFl", gradient_fill(true, vec![]), vec![]),
    )]));
    let Some(stroke) = read.effects.strokes.first() else {
        unreachable!("one stroke");
    };
    assert_eq!(stroke.fill, StrokeFill::Gradient);
    assert!((stroke.opacity - 0.7).abs() < 1e-6 && (stroke.size - 5.0).abs() < 1e-6);
    let Some(StrokePaint::Gradient {
        gradient,
        reference,
    }) = &stroke.paint
    else {
        unreachable!("a gradient paint, got {:?}", stroke.paint);
    };
    assert!(reference.is_none(), "the box is the caller's");
    assert!(gradient.reverse && gradient.align);
    assert!((gradient.angle - 30.0).abs() < 1e-9);
    assert_eq!(gradient.colours.len(), 2);
    assert!(stroke.drawn());
    assert_eq!(read.effects.shown_drawn_strokes(), 1);
    assert_eq!(read.effects.shown_unrendered_strokes(), 0);
}

#[test]
fn a_gradient_stroke_aurora_cannot_paint_stays_undrawn() {
    let noise = {
        let mut v = gradient_fill(true, vec![]);
        if let V::Obj(items) = &mut v {
            items.retain(|(k, _)| *k != b"Grad".as_slice());
            items.push((b"Grad", V::Obj(vec![(b"GrdF", V::Enum(b"GrdF", b"ClNs"))])));
        }
        v
    };
    let unaligned = gradient_fill(true, vec![(b"Algn", V::Bool(false))]);
    for source in [noise, unaligned] {
        let read = mapped(&effects_block(&[(
            b"FrFX",
            painted_stroke(b"GrFl", source, vec![]),
        )]));
        let Some(stroke) = read.effects.strokes.first() else {
            unreachable!("the stroke is kept");
        };
        assert!(stroke.paint.is_none() && !stroke.drawn());
        assert_eq!(read.effects.shown_unrendered_strokes(), 1);
    }
}

#[test]
fn a_pattern_stroke_keeps_its_id_and_scale_and_waits_for_its_origin() {
    use aurora_core::effects::{StrokeFill, StrokePaint};
    let read = mapped(&effects_block(&[(
        b"FrFX",
        painted_stroke(b"Ptrn", pattern_fill(true, vec![]), vec![]),
    )]));
    let Some(stroke) = read.effects.strokes.first() else {
        unreachable!("one stroke");
    };
    assert_eq!(stroke.fill, StrokeFill::Pattern);
    let Some(StrokePaint::Pattern {
        pattern,
        scale,
        origin,
    }) = &stroke.paint
    else {
        unreachable!("a pattern paint, got {:?}", stroke.paint);
    };
    assert_eq!(pattern, "2ab8ff7f-7c39");
    assert!((scale - 2.0).abs() < 1e-6);
    assert_eq!(*origin, (0.0, 0.0));
}

#[test]
fn an_unnamed_unlinked_or_phased_pattern_stroke_stays_undrawn() {
    let unlinked = pattern_fill(true, vec![(b"Algn", V::Bool(false))]);
    let phased = pattern_fill(
        true,
        vec![(
            b"phase",
            V::Obj(vec![(b"Hrzn", V::Doub(3.0)), (b"Vrtc", V::Doub(0.0))]),
        )],
    );
    let unnamed = pattern_fill(true, vec![(b"Ptrn", V::Obj(vec![]))]);
    for source in [unlinked, phased, unnamed] {
        let read = mapped(&effects_block(&[(
            b"FrFX",
            painted_stroke(b"Ptrn", source, vec![]),
        )]));
        let Some(stroke) = read.effects.strokes.first() else {
            unreachable!("the stroke is kept");
        };
        assert!(stroke.paint.is_none(), "{stroke:?}");
        assert_eq!(read.effects.shown_unrendered_strokes(), 1);
    }
}

/// A gradient stroke on a raster layer that starts left of and above the
/// canvas (the only case where Aurora's bounds do not start at the origin):
/// its box is the record rectangle, stored relative to the layer's bounds
/// (the bounds are the union with the canvas, 19 x 18, and the record
/// rectangle is 4 x 4 at their corner, so the offset is zero). Not compared with Photoshop's box (see `StrokePaint`).
#[test]
fn a_gradient_stroke_box_is_made_relative_to_the_layers_bounds_on_open() {
    use aurora_core::effects::StrokePaint;
    let block = effects_block(&[(
        b"FrFX",
        painted_stroke(b"GrFl", gradient_fill(true, vec![]), vec![]),
    )]);
    let doc = open(
        &TestPsd::new(1, 16, 16, 8)
            .with(|p| {
                p.layers = vec![
                    TestLayer::pixels("a", -3, -2, 4, 4, 8, &[[1, 2, 3, 255]; 16])
                        .with(|l| l.blocks.push((*b"lfx2", block.clone()))),
                ];
            })
            .write(),
    );
    let Some(&id) = doc.layers.roots().first() else {
        unreachable!("one layer");
    };
    assert_eq!(
        doc.layers.bounds(id),
        Some(aurora_core::Rect {
            x: -3,
            y: -2,
            width: 19,
            height: 18
        })
    );
    let paint = doc
        .layers
        .effects(id)
        .and_then(|fx| fx.strokes.first())
        .and_then(|s| s.paint.clone());
    let Some(StrokePaint::Gradient {
        reference: Some(reference),
        ..
    }) = paint
    else {
        unreachable!("a painted gradient stroke with a box, got {paint:?}");
    };
    assert_eq!(
        (
            reference.dx,
            reference.dy,
            reference.width,
            reference.height
        ),
        (0, 0, 4, 4)
    );
}

// ---------------------------------------------------------------------
// Task 151: the outline of a shape layer
// ---------------------------------------------------------------------

mod outline {
    use crate::Image;
    use crate::psd::{PsdEffectShape, outline_adds_information, outline_covers_pixels};
    use aurora_color::IccProfile;
    use aurora_core::Rect;

    fn rect(x: i64, y: i64, width: u32, height: u32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    /// A 4 x 4 image at document (10, 20) with alpha `a` at the listed
    /// `(x, y)` pixels and 0 elsewhere.
    fn image_with(alpha: &[((u32, u32), f32)]) -> Image {
        let mut samples = Vec::new();
        for y in 0..4u32 {
            for x in 0..4u32 {
                let a = alpha
                    .iter()
                    .find(|(p, _)| *p == (x, y))
                    .map_or(0.0, |(_, a)| *a);
                samples.extend([1.0, 1.0, 1.0, a].map(half::f16::from_f32));
            }
        }
        match Image::new(4, 4, IccProfile::srgb(), samples) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    /// The outline covers document (11..13, 21..23).
    fn outline() -> PsdEffectShape {
        PsdEffectShape {
            area: rect(11, 21, 2, 2),
            coverage: vec![255, 255, 0, 128],
        }
    }

    #[test]
    fn pixels_under_the_outline_are_covered() {
        let image = image_with(&[((1, 1), 1.0), ((2, 1), 0.9), ((2, 2), 0.7)]);
        // (2, 2) -> document (12, 22) has coverage 128: covered; a faint
        // alpha at the zero-coverage texel is below the threshold.
        assert!(outline_covers_pixels(
            &image,
            rect(10, 20, 4, 4),
            &outline()
        ));
        let faint = image_with(&[((1, 2), 0.2)]);
        assert!(outline_covers_pixels(
            &faint,
            rect(10, 20, 4, 4),
            &outline()
        ));
    }

    #[test]
    fn a_visible_pixel_outside_the_outline_refuses_it() {
        // (1, 2) is document (11, 22): coverage 0 inside the area.
        let image = image_with(&[((1, 1), 1.0), ((1, 2), 0.5)]);
        assert!(!outline_covers_pixels(
            &image,
            rect(10, 20, 4, 4),
            &outline()
        ));
        // A pixel outside the outline's area altogether.
        let outside = image_with(&[((0, 0), 1.0)]);
        assert!(!outline_covers_pixels(
            &outside,
            rect(10, 20, 4, 4),
            &outline()
        ));
    }

    /// An opaque shape keeps no outline (its alpha is Photoshop's own); a
    /// fill that fades, or a hole where the outline is full, keeps it.
    #[test]
    fn only_a_translucent_fill_keeps_an_outline() {
        let outline = PsdEffectShape {
            area: rect(0, 0, 2, 2),
            coverage: vec![255, 255, 255, 255],
        };
        let all = |a: f32| image_with(&[((0, 0), a), ((1, 0), a), ((0, 1), a), ((1, 1), a)]);
        assert!(!outline_adds_information(
            &all(1.0),
            rect(0, 0, 4, 4),
            &outline
        ));
        assert!(!outline_adds_information(
            &all(0.96),
            rect(0, 0, 4, 4),
            &outline
        ));
        assert!(outline_adds_information(
            &all(0.9),
            rect(0, 0, 4, 4),
            &outline
        ));
        // One faded texel is enough.
        let one = image_with(&[((0, 0), 1.0), ((1, 0), 1.0), ((0, 1), 1.0), ((1, 1), 0.2)]);
        assert!(outline_adds_information(&one, rect(0, 0, 4, 4), &outline));
        // Partial coverage on the edge is not counted.
        let edge = PsdEffectShape {
            area: rect(0, 0, 2, 2),
            coverage: vec![255, 255, 255, 100],
        };
        let image = image_with(&[((0, 0), 1.0), ((1, 0), 1.0), ((0, 1), 1.0), ((1, 1), 0.3)]);
        assert!(!outline_adds_information(&image, rect(0, 0, 4, 4), &edge));
    }
}
