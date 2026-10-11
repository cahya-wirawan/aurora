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

enum V<'a> {
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

fn descriptor_bytes(items: &[(&[u8], V<'_>)]) -> Vec<u8> {
    let mut o = Vec::new();
    put_body(&mut o, items);
    o
}

/// An `lfx2`-style block: object version 0, descriptor version 16.
fn effects_block(items: &[(&[u8], V<'_>)]) -> Vec<u8> {
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

fn rgb<'a>(r: f64, g: f64, b: f64) -> V<'a> {
    V::Obj(vec![
        (b"Rd  ", V::Doub(r)),
        (b"Grn ", V::Doub(g)),
        (b"Bl  ", V::Doub(b)),
    ])
}

fn shadow<'a>(enab: bool, present: bool, distance: f64) -> V<'a> {
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
fn raw_blocks(file: &[u8], key: &[u8; 4]) -> Vec<Vec<u8>> {
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
        (b"OrGl", V::Obj(vec![])),
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
        "1 outer glow isn't shown yet",
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
    // Turned off still counts: the effects are lost from the .psd either way.
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
        items.contains("2 layers' effects aren't saved to .psd yet (they are kept in .aur)."),
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

/// Review of 0.181.0: a clipped layer's colour overlay is drawn, but the
/// layer is shown unclipped, so the open report says so specifically.
#[test]
fn overlays_on_clipped_layers_are_reported_as_drawn_unclipped() {
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
    assert!(
        items.contains(
            "1 colour overlay, drop shadow or stroke on clipped layers is drawn unclipped"
        ),
        "only the clipped layer's: {items}"
    );
    assert!(!items.contains("isn't shown yet"), "{items}");
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
/// stroke on a group (E9). A clipped layer's stroke is reported as drawn
/// unclipped. The PSD-save report still itemises a layer whose only
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
    assert!(
        items.contains(
            "1 colour overlay, drop shadow or stroke on clipped layers is drawn unclipped"
        ),
        "{items}"
    );
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
    assert!(
        items.contains("1 layer's effects aren't saved to .psd yet"),
        "{items}"
    );
}
