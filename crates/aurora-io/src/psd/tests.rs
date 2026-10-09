//! The PSD/PSB reader's own tests: synthetic files from
//! [`super::test_writer`] for every structural case, real psd-tools
//! fixtures (`tests/fixtures/psd/`, see `PROVENANCE.md` there) for what
//! real Photoshop output looks like, deterministic truncation/mutation
//! sweeps, and — when the gitignored corpus is present — a sweep over all
//! of it.

// Samples are compared bit-exactly on purpose: promotion to `f16` is
// deterministic, and "close enough" would hide a /255-vs-/65535 slip.
#![allow(clippy::float_cmp)]

use aurora_doc::{BlendMode, LayerId, LayerKind, LayerTree};
use half::f16;

use super::test_writer::{TestLayer, TestPsd, pack_bits};
use super::{CurvBlock, CurvRefusal, Note, curves_params, parse_curv};
use super::{
    MAX_GROUP_DEPTH, PIXEL_BUDGET, PsdBlend, PsdDocument, PsdFile, PsdNode, blend_for_key, decode,
    read, unpack_bits,
};
use crate::error::IoError;
use crate::image::Image;

fn ok<T>(result: Result<T, IoError>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => unreachable!("expected Ok, got {err:?}"),
    }
}

fn doc(bytes: &[u8]) -> PsdDocument {
    ok(read(bytes))
}

/// The straight RGBA at `(x, y)` of `image`, as `f32`.
fn px(image: &Image, x: u32, y: u32) -> [f32; 4] {
    let i = (y as usize * image.width() as usize + x as usize) * 4;
    let mut out = [f32::NAN; 4];
    for (c, slot) in out.iter_mut().enumerate() {
        *slot = image
            .samples()
            .get(i + c)
            .copied()
            .map_or(f32::NAN, f16::to_f32);
    }
    out
}

fn f(v: f32) -> f32 {
    f16::from_f32(v).to_f32()
}

fn u8px(r: u8, g: u8, b: u8, a: u8) -> [f32; 4] {
    [r, g, b, a].map(|v| f(f32::from(v) / 255.0))
}

fn image_of(document: &PsdDocument, id: LayerId) -> Option<&Image> {
    document
        .pixels
        .iter()
        .find(|placed| placed.layer == id)
        .map(|placed| &placed.image)
}

/// Where `id`'s image sits inside its canvas-anchored surface.
fn offset_of(document: &PsdDocument, id: LayerId) -> Option<(u32, u32)> {
    document
        .pixels
        .iter()
        .find(|placed| placed.layer == id)
        .map(|placed| placed.offset)
}

fn rect(x: i64, y: i64, width: u32, height: u32) -> aurora_core::Rect {
    aurora_core::Rect {
        x,
        y,
        width,
        height,
    }
}

fn root(tree: &LayerTree, index: usize) -> LayerId {
    match tree.roots().get(index) {
        Some(id) => *id,
        None => unreachable!("no root {index} in {:?}", tree.roots()),
    }
}

fn child(tree: &LayerTree, parent: LayerId, index: usize) -> LayerId {
    match tree.children(parent).and_then(|c| c.get(index)) {
        Some(id) => *id,
        None => unreachable!("no child {index} of {parent:?}"),
    }
}

fn two_by_two(depth: u16) -> Vec<[u16; 4]> {
    if depth == 16 {
        vec![
            [0x1234, 0xFFFF, 0x0000, 0xFFFF],
            [0x8000, 0x0101, 0xFEDC, 0x4000],
            [0xFFFF, 0xFFFF, 0xFFFF, 0x0000],
            [0x0001, 0x0002, 0x0003, 0xFFFF],
        ]
    } else {
        vec![
            [10, 20, 30, 255],
            [40, 50, 60, 128],
            [70, 70, 70, 0],
            [255, 0, 1, 255],
        ]
    }
}

fn expected(depth: u16, sample: [u16; 4]) -> [f32; 4] {
    let max = if depth == 16 { 65535.0 } else { 255.0 };
    sample.map(|v| f(f32::from(v) / max))
}

// ---------------------------------------------------------------------
// Pixels: every compression, both depths, both versions
// ---------------------------------------------------------------------

#[test]
fn every_compression_decodes_the_exact_samples_at_both_depths_and_versions() {
    for version in [1, 2] {
        for depth in [8, 16] {
            for compression in 0..=3 {
                let pixels = two_by_two(depth);
                let bytes = TestPsd::new(version, 4, 4, depth)
                    .with(|p| {
                        p.lr16 = depth == 16;
                        p.layers.push(
                            TestLayer::pixels("L", 1, 1, 2, 2, depth, &pixels)
                                .with(|l| l.compression = compression),
                        );
                    })
                    .write();
                let document = doc(&bytes);
                let id = root(&document.layers, 0);
                let Some(image) = image_of(&document, id) else {
                    unreachable!("no pixels for v{version} d{depth} c{compression}");
                };
                for (i, sample) in pixels.iter().enumerate() {
                    let (x, y) = ((i % 2) as u32, (i / 2) as u32);
                    assert_eq!(
                        px(image, x, y),
                        expected(depth, *sample),
                        "v{version} d{depth} c{compression} pixel {i}"
                    );
                }
                // Canvas-anchored bounds, the layer's own 2x2 at (1, 1).
                assert_eq!(offset_of(&document, id), Some((1, 1)));
                assert_eq!(
                    document.layers.bounds(id),
                    Some(aurora_core::Rect {
                        x: 0,
                        y: 0,
                        width: 4,
                        height: 4
                    })
                );
                assert!(document.report.is_empty(), "{:?}", document.report);
            }
        }
    }
}

#[test]
fn sixteen_bit_samples_keep_more_than_the_high_byte() {
    // 0x1234 and 0x12FF share a high byte; a reader that kept only it
    // (or divided by 255) cannot tell them apart.
    let bytes = TestPsd::new(1, 2, 1, 16)
        .with(|p| {
            p.lr16 = true;
            p.layers.push(
                TestLayer::pixels(
                    "L",
                    0,
                    0,
                    2,
                    1,
                    16,
                    &[[0x1234, 0, 0, 0xFFFF], [0x12FF, 0, 0, 0xFFFF]],
                )
                .with(|l| l.compression = 3),
            );
        })
        .write();
    let document = doc(&bytes);
    let Some(image) = image_of(&document, root(&document.layers, 0)) else {
        unreachable!("no pixels");
    };
    let a = px(image, 0, 0)[0];
    let b = px(image, 1, 0)[0];
    assert_eq!(a, f(f32::from(0x1234_u16) / 65535.0));
    assert_eq!(b, f(f32::from(0x12FF_u16) / 65535.0));
    assert!(a < b);
}

#[test]
fn a_missing_transparency_channel_means_opaque() {
    let bytes = TestPsd::new(1, 1, 1, 8)
        .with(|p| {
            p.layers.push(
                TestLayer::pixels("Background", 0, 0, 1, 1, 8, &[[1, 2, 3, 9]])
                    .with(|l| l.channels.retain(|(id, _)| *id >= 0)),
            );
        })
        .write();
    let document = doc(&bytes);
    let Some(image) = image_of(&document, root(&document.layers, 0)) else {
        unreachable!("no pixels");
    };
    assert_eq!(px(image, 0, 0), u8px(1, 2, 3, 255));
}

#[test]
fn the_transparency_channel_is_read_as_alpha() {
    let bytes = TestPsd::new(1, 1, 1, 8)
        .with(|p| {
            p.layers
                .push(TestLayer::pixels("L", 0, 0, 1, 1, 8, &[[1, 2, 3, 77]]));
        })
        .write();
    let document = doc(&bytes);
    let Some(image) = image_of(&document, root(&document.layers, 0)) else {
        unreachable!("no pixels");
    };
    assert_eq!(px(image, 0, 0), u8px(1, 2, 3, 77));
}

// ---------------------------------------------------------------------
// Layer properties and order
// ---------------------------------------------------------------------

#[test]
fn two_layers_keep_order_opacity_fill_blend_and_visibility() {
    let bytes = TestPsd::new(1, 2, 2, 8)
        .with(|p| {
            // Bottom-to-top in the file.
            p.layers.push(TestLayer::pixels(
                "bottom",
                0,
                0,
                1,
                1,
                8,
                &[[0, 0, 0, 255]],
            ));
            p.layers.push(
                TestLayer::pixels("top", 0, 0, 1, 1, 8, &[[0, 0, 0, 255]]).with(|l| {
                    l.opacity = 128;
                    l.fill = Some(51);
                    l.blend = *b"div ";
                    l.flags = 0x02;
                }),
            );
        })
        .write();
    let document = doc(&bytes);
    let tree = &document.layers;
    assert_eq!(tree.roots().len(), 2);
    let top = root(tree, 0);
    let bottom = root(tree, 1);
    assert_eq!(tree.name(top), Some("top"));
    assert_eq!(tree.name(bottom), Some("bottom"));
    assert_eq!(tree.opacity(top), Some(128.0 / 255.0));
    assert_eq!(tree.fill_opacity(top), Some(51.0 / 255.0));
    assert_eq!(tree.blend_mode(top), Some(BlendMode::ColorDodge));
    assert_eq!(tree.visible(top), Some(false));
    assert_eq!(tree.opacity(bottom), Some(1.0));
    assert_eq!(tree.blend_mode(bottom), Some(BlendMode::Normal));
    assert_eq!(tree.visible(bottom), Some(true));
    assert_eq!(document.canvas_size, (2, 2));
}

#[test]
fn the_unicode_name_wins_over_the_pascal_name() {
    let bytes = TestPsd::new(1, 1, 1, 8)
        .with(|p| {
            p.layers.push(
                TestLayer::pixels("ascii", 0, 0, 1, 1, 8, &[[0, 0, 0, 255]])
                    .with(|l| l.luni = Some("Слой 👽".to_owned())),
            );
            p.layers.push(
                TestLayer::pixels("only pascal", 0, 0, 1, 1, 8, &[[0, 0, 0, 255]])
                    .with(|l| l.luni = None),
            );
        })
        .write();
    let document = doc(&bytes);
    assert_eq!(
        document.layers.name(root(&document.layers, 1)),
        Some("Слой 👽")
    );
    assert_eq!(
        document.layers.name(root(&document.layers, 0)),
        Some("only pascal")
    );
}

#[test]
fn a_negative_origin_is_kept() {
    let bytes = TestPsd::new(1, 4, 4, 8)
        .with(|p| {
            p.layers.push(TestLayer::pixels(
                "L",
                -3,
                -2,
                2,
                1,
                8,
                &[[1, 1, 1, 255], [2, 2, 2, 255]],
            ));
        })
        .write();
    let document = doc(&bytes);
    // The union of the layer's own (-3, -2, 2, 1) and the 4x4 canvas;
    // the layer's pixels are at the surface's own top-left.
    let id = root(&document.layers, 0);
    assert_eq!(document.layers.bounds(id), Some(rect(-3, -2, 7, 6)));
    assert_eq!(offset_of(&document, id), Some((0, 0)));
}

#[test]
fn an_empty_layer_gets_the_canvas_as_bounds_and_no_pixels() {
    let bytes = TestPsd::new(1, 7, 5, 8)
        .with(|p| p.layers.push(TestLayer::empty("empty")))
        .write();
    let document = doc(&bytes);
    let id = root(&document.layers, 0);
    assert_eq!(
        document.layers.bounds(id),
        Some(aurora_core::Rect {
            x: 0,
            y: 0,
            width: 7,
            height: 5
        })
    );
    assert!(document.pixels.is_empty());
}

#[test]
fn the_blend_key_table_covers_every_mode_pass_through_and_unknown() {
    let table: [(&[u8; 4], BlendMode); 27] = [
        (b"norm", BlendMode::Normal),
        (b"diss", BlendMode::Dissolve),
        (b"dark", BlendMode::Darken),
        (b"mul ", BlendMode::Multiply),
        (b"idiv", BlendMode::ColorBurn),
        (b"lbrn", BlendMode::LinearBurn),
        (b"dkCl", BlendMode::DarkerColor),
        (b"lite", BlendMode::Lighten),
        (b"scrn", BlendMode::Screen),
        (b"div ", BlendMode::ColorDodge),
        (b"lddg", BlendMode::LinearDodge),
        (b"lgCl", BlendMode::LighterColor),
        (b"over", BlendMode::Overlay),
        (b"sLit", BlendMode::SoftLight),
        (b"hLit", BlendMode::HardLight),
        (b"vLit", BlendMode::VividLight),
        (b"lLit", BlendMode::LinearLight),
        (b"pLit", BlendMode::PinLight),
        (b"hMix", BlendMode::HardMix),
        (b"diff", BlendMode::Difference),
        (b"smud", BlendMode::Exclusion),
        (b"fsub", BlendMode::Subtract),
        (b"fdiv", BlendMode::Divide),
        (b"hue ", BlendMode::Hue),
        (b"sat ", BlendMode::Saturation),
        (b"colr", BlendMode::Color),
        (b"lum ", BlendMode::Luminosity),
    ];
    let mut seen = std::collections::HashSet::new();
    for (key, mode) in table {
        assert_eq!(blend_for_key(*key), PsdBlend::Mode(mode), "{key:?}");
        assert!(seen.insert(format!("{mode:?}")), "{mode:?} twice");
    }
    assert_eq!(blend_for_key(*b"pass"), PsdBlend::PassThrough);
    assert_eq!(blend_for_key(*b"zzzz"), PsdBlend::Unknown);

    // And end to end, through a real record.
    let bytes = TestPsd::new(1, 1, 1, 8)
        .with(|p| {
            for (key, _) in table {
                p.layers.push(
                    TestLayer::pixels("L", 0, 0, 1, 1, 8, &[[0, 0, 0, 255]])
                        .with(|l| l.blend = *key),
                );
            }
            p.layers.push(
                TestLayer::pixels("odd", 0, 0, 1, 1, 8, &[[0, 0, 0, 255]])
                    .with(|l| l.blend = *b"zzzz"),
            );
        })
        .write();
    let document = doc(&bytes);
    let roots = document.layers.roots();
    assert_eq!(roots.len(), 28);
    assert_eq!(
        document.layers.blend_mode(root(&document.layers, 0)),
        Some(BlendMode::Normal)
    );
    for (i, (_, mode)) in table.iter().enumerate() {
        // Roots are top-first; the table was written bottom-first, under
        // the one extra "odd" layer on top.
        let id = root(&document.layers, 27 - i);
        assert_eq!(document.layers.blend_mode(id), Some(*mode));
    }
    assert!(
        document
            .report
            .items
            .iter()
            .any(|item| item.contains("blend mode Aurora doesn't recognise")),
        "{:?}",
        document.report
    );
}

// ---------------------------------------------------------------------
// Groups
// ---------------------------------------------------------------------

#[test]
fn nested_groups_rebuild_the_tree_in_order_with_their_own_properties() {
    let px1 = [[0, 0, 0, 255]];
    let bytes = TestPsd::new(1, 2, 2, 8)
        .with(|p| {
            p.layers = vec![
                TestLayer::pixels("background", 0, 0, 1, 1, 8, &px1),
                TestLayer::divider(),
                TestLayer::pixels("outer low", 0, 0, 1, 1, 8, &px1),
                TestLayer::divider(),
                TestLayer::pixels("inner", 0, 0, 1, 1, 8, &px1),
                TestLayer::group("Inner", *b"mul ").with(|l| l.opacity = 51),
                TestLayer::pixels("outer high", 0, 0, 1, 1, 8, &px1),
                TestLayer::group("Outer", *b"norm").with(|l| l.flags = 0x02),
                TestLayer::pixels("top", 0, 0, 1, 1, 8, &px1),
            ];
        })
        .write();
    let document = doc(&bytes);
    let tree = &document.layers;
    let names: Vec<_> = tree.roots().iter().map(|id| tree.name(*id)).collect();
    assert_eq!(names, [Some("top"), Some("Outer"), Some("background")]);
    let outer = root(tree, 1);
    assert!(matches!(tree.kind(outer), Some(LayerKind::Group { .. })));
    assert_eq!(tree.visible(outer), Some(false));
    let outer_children: Vec<_> = tree
        .children(outer)
        .unwrap_or_default()
        .iter()
        .map(|id| tree.name(*id))
        .collect();
    assert_eq!(
        outer_children,
        [Some("outer high"), Some("Inner"), Some("outer low")]
    );
    let inner = child(tree, outer, 1);
    assert_eq!(tree.blend_mode(inner), Some(BlendMode::Multiply));
    assert_eq!(tree.opacity(inner), Some(51.0 / 255.0));
    assert_eq!(tree.name(child(tree, inner, 0)), Some("inner"));
    assert!(document.report.is_empty(), "{:?}", document.report);
}

#[test]
fn a_pass_through_group_is_normal_and_reported_only_when_it_matters() {
    let px1 = [[0, 0, 0, 255]];
    let plain = TestPsd::new(1, 1, 1, 8)
        .with(|p| {
            p.layers = vec![
                TestLayer::divider(),
                TestLayer::pixels("a", 0, 0, 1, 1, 8, &px1),
                TestLayer::group("G", *b"pass"),
            ];
        })
        .write();
    let document = doc(&plain);
    assert_eq!(
        document.layers.blend_mode(root(&document.layers, 0)),
        Some(BlendMode::Normal)
    );
    assert!(document.report.is_empty(), "{:?}", document.report);

    let blended = TestPsd::new(1, 1, 1, 8)
        .with(|p| {
            p.layers = vec![
                TestLayer::divider(),
                TestLayer::pixels("a", 0, 0, 1, 1, 8, &px1).with(|l| l.blend = *b"scrn"),
                TestLayer::group("G", *b"pass"),
            ];
        })
        .write();
    let document = doc(&blended);
    assert!(
        document
            .report
            .items
            .iter()
            .any(|i| i.contains("Pass Through")),
        "{:?}",
        document.report
    );
}

#[test]
fn unmatched_group_markers_are_lenient_and_reported() {
    let px1 = [[0, 0, 0, 255]];
    // A stray group header with no end marker, and an end marker never
    // closed.
    let bytes = TestPsd::new(1, 1, 1, 8)
        .with(|p| {
            p.layers = vec![
                TestLayer::pixels("a", 0, 0, 1, 1, 8, &px1),
                TestLayer::group("Stray", *b"norm"),
                TestLayer::divider(),
                TestLayer::pixels("b", 0, 0, 1, 1, 8, &px1),
            ];
        })
        .write();
    let document = doc(&bytes);
    let tree = &document.layers;
    let names: Vec<_> = tree.roots().iter().map(|id| tree.name(*id)).collect();
    assert_eq!(names, [Some("b"), Some("Stray"), Some("a")]);
    assert!(
        document
            .report
            .items
            .iter()
            .any(|i| i.contains("2 unmatched group markers")),
        "{:?}",
        document.report
    );
}

#[test]
fn group_nesting_is_capped_without_recursion() {
    let px1 = [[0, 0, 0, 255]];
    let nest = |depth: usize| {
        TestPsd::new(2, 1, 1, 8)
            .with(|p| {
                for _ in 0..depth {
                    p.layers.push(TestLayer::divider());
                }
                p.layers
                    .push(TestLayer::pixels("deep", 0, 0, 1, 1, 8, &px1));
                for _ in 0..depth {
                    p.layers.push(TestLayer::group("G", *b"norm"));
                }
            })
            .write()
    };
    let document = doc(&nest(MAX_GROUP_DEPTH));
    assert_eq!(document.layers.len(), MAX_GROUP_DEPTH + 1);
    match decode(&nest(MAX_GROUP_DEPTH + 1)) {
        Err(IoError::PsdGroupsTooDeep { max }) => assert_eq!(max, MAX_GROUP_DEPTH),
        other => unreachable!("expected PsdGroupsTooDeep, got {other:?}"),
    }
    // Far past the cap, no stack overflow either.
    assert!(matches!(
        decode(&nest(20_000)),
        Err(IoError::PsdGroupsTooDeep { .. })
    ));
}

// ---------------------------------------------------------------------
// Merged image, 16-bit layout, PSB
// ---------------------------------------------------------------------

#[test]
fn a_file_with_no_layers_opens_its_merged_image_as_background() {
    for compression in 0..=3 {
        let planes = vec![vec![1, 2, 3, 4], vec![5, 6, 7, 8], vec![9, 10, 11, 12]];
        let bytes = TestPsd::new(1, 2, 2, 8)
            .with(|p| {
                p.merged = Some(planes.clone());
                p.merged_compression = compression;
            })
            .write();
        let document = doc(&bytes);
        let id = root(&document.layers, 0);
        assert_eq!(document.layers.name(id), Some("Background"));
        let Some(image) = image_of(&document, id) else {
            unreachable!("no pixels");
        };
        assert_eq!(
            px(image, 1, 1),
            u8px(4, 8, 12, 255),
            "compression {compression}"
        );
        assert_eq!(px(image, 0, 1), u8px(3, 7, 11, 255));
    }
}

#[test]
fn a_file_of_only_empty_groups_does_not_read_its_merged_image() {
    // A truncated merged image, as psd-tools' `group-divider-blend-mode.psd`
    // has: it must not be read when the tree is non-empty.
    let bytes = TestPsd::new(1, 4, 4, 8)
        .with(|p| {
            p.layers = vec![TestLayer::divider(), TestLayer::group("G", *b"pass")];
            p.merged = Some(vec![vec![0; 3], Vec::new(), Vec::new()]);
        })
        .write();
    let document = doc(&bytes);
    assert_eq!(document.layers.len(), 1);
    assert!(document.pixels.is_empty());
}

#[test]
fn extra_channels_and_adjustment_layers_are_reported() {
    let bytes = TestPsd::new(1, 1, 1, 8)
        .with(|p| {
            p.channels = 5;
            p.layers = vec![
                TestLayer::pixels("a", 0, 0, 1, 1, 8, &[[0, 0, 0, 255]]).with(|l| {
                    l.clipping = 1;
                    l.blocks.push((*b"lfx2", vec![0; 8]));
                }),
                TestLayer::empty("Levels 1").with(|l| l.blocks.push((*b"levl", vec![0; 4]))),
                TestLayer::empty("Color Fill").with(|l| l.blocks.push((*b"SoCo", vec![0; 4]))),
            ];
        })
        .write();
    let document = doc(&bytes);
    assert_eq!(document.layers.len(), 1);
    let items = document.report.items.join("\n");
    for needle in [
        "1 adjustment layer",
        "1 shape or fill layer",
        "1 clipped layer",
        "layer effects",
        "2 saved channels",
    ] {
        assert!(items.contains(needle), "{needle:?} missing from {items}");
    }
}

#[test]
fn a_layer_mask_is_decoded_and_no_longer_reported() {
    let bytes = TestPsd::new(1, 2, 2, 8)
        .with(|p| {
            p.layers.push(
                TestLayer::pixels("m", 0, 0, 2, 2, 8, &two_by_two(8)).with(|l| {
                    l.mask = Some((0, 1, 2, 2, 0, 0));
                    l.channels.push((-2, vec![255, 0]));
                    l.compression = 1;
                }),
            );
        })
        .write();
    let file = ok(decode(&bytes));
    let Some(PsdNode::Layer(layer)) = file.layers.first() else {
        unreachable!("no layer");
    };
    let Some(mask) = &layer.mask else {
        unreachable!("no mask");
    };
    assert_eq!(mask.bounds.x, 1);
    assert_eq!(mask.bounds.width, 1);
    assert_eq!(
        mask.coverage.as_deref(),
        Some([f16::ONE, f16::ZERO].as_slice())
    );
    assert!(file.report().is_empty(), "{:?}", file.report());
}

// ---------------------------------------------------------------------
// Typed errors
// ---------------------------------------------------------------------

#[test]
fn unsupported_modes_depths_and_compressions_are_typed_errors() {
    let cmyk = TestPsd::new(1, 1, 1, 8).with(|p| p.color_mode = 4).write();
    assert!(matches!(
        decode(&cmyk),
        Err(IoError::UnsupportedPsdColorMode(4))
    ));
    let deep = TestPsd::new(1, 1, 1, 32).write();
    assert!(matches!(
        decode(&deep),
        Err(IoError::UnsupportedPsdDepth(32))
    ));
    let bitmap = TestPsd::new(1, 1, 1, 1).write();
    assert!(matches!(
        decode(&bitmap),
        Err(IoError::UnsupportedPsdDepth(1))
    ));
    let odd = TestPsd::new(1, 1, 1, 8)
        .with(|p| {
            p.layers.push(
                TestLayer::pixels("L", 0, 0, 1, 1, 8, &[[0, 0, 0, 255]])
                    .with(|l| l.compression = 7),
            );
        })
        .write();
    assert!(matches!(
        decode(&odd),
        Err(IoError::UnsupportedPsdCompression(7))
    ));
    let mut v3 = TestPsd::new(1, 1, 1, 8).write();
    if let Some(byte) = v3.get_mut(5) {
        *byte = 3;
    }
    assert!(matches!(
        decode(&v3),
        Err(IoError::UnsupportedPsdVersion(3))
    ));
    assert!(matches!(decode(b"GIF89a....."), Err(IoError::NotPsd)));
    assert!(matches!(decode(b""), Err(IoError::NotPsd)));
}

#[test]
fn canvas_sizes_past_the_format_limit_are_refused() {
    let psb = TestPsd::new(2, 300_001, 1, 8).write();
    assert!(matches!(
        decode(&psb),
        Err(IoError::PsdTooLarge { max: 300_000, .. })
    ));
    let psd = TestPsd::new(1, 1, 30_001, 8).write();
    assert!(matches!(
        decode(&psd),
        Err(IoError::PsdTooLarge { max: 30_000, .. })
    ));
}

#[test]
fn the_pixel_budget_is_checked_before_anything_is_allocated() {
    // A 20,000 x 20,000 layer (4e8 px, past 2^28) whose channels are
    // empty: the file is tiny, and must be refused from its rectangle.
    let bytes = TestPsd::new(2, 20_000, 20_000, 8)
        .with(|p| {
            p.layers.push(TestLayer::empty("huge").with(|l| {
                l.right = 20_000;
                l.bottom = 20_000;
                l.channels = vec![(0, Vec::new())];
            }));
            p.merged = Some(vec![Vec::new(), Vec::new(), Vec::new()]);
        })
        .write();
    assert!(bytes.len() < 1024);
    match decode(&bytes) {
        Err(IoError::PsdPixelBudget { total, max }) => {
            assert_eq!(total, 400_000_000);
            assert_eq!(max, PIXEL_BUDGET);
        }
        other => unreachable!("expected PsdPixelBudget, got {other:?}"),
    }
    // A merged-only file is budgeted too.
    let flat = TestPsd::new(2, 20_000, 20_000, 8)
        .with(|p| p.merged = Some(vec![Vec::new(), Vec::new(), Vec::new()]))
        .write();
    assert!(matches!(decode(&flat), Err(IoError::PsdPixelBudget { .. })));
}

#[test]
fn lying_lengths_are_errors_not_allocations() {
    // A channel claiming u32::MAX bytes.
    let mut good = TestPsd::new(1, 1, 1, 8)
        .with(|p| {
            p.layers.push(
                TestLayer::pixels("L", 0, 0, 1, 1, 8, &[[0, 0, 0, 255]])
                    .with(|l| l.channels.truncate(1)),
            );
        })
        .write();
    // header 26 + 4 + 4 + section len 4 + info len 4 + count 2 + rect 16
    // + channel count 2 + id 2 = offset 64 for the channel length.
    let at = 64;
    if let Some(slot) = good.get_mut(at..at + 4) {
        slot.copy_from_slice(&u32::MAX.to_be_bytes());
    }
    assert!(matches!(decode(&good), Err(IoError::PsdTruncated { .. })));

    // A tagged block claiming u32::MAX bytes.
    let mut lying = TestPsd::new(1, 1, 1, 8)
        .with(|p| {
            p.layers.push(
                TestLayer::pixels("L", 0, 0, 1, 1, 8, &[[0, 0, 0, 255]])
                    .with(|l| l.blocks.push((*b"zzzz", vec![1, 2, 3, 4]))),
            );
        })
        .write();
    let Some(key) = lying.windows(4).position(|w| w == b"zzzz") else {
        unreachable!("no block");
    };
    if let Some(slot) = lying.get_mut(key + 4..key + 8) {
        slot.copy_from_slice(&u32::MAX.to_be_bytes());
    }
    assert!(matches!(decode(&lying), Err(IoError::PsdTruncated { .. })));
}

#[test]
fn inverted_rectangles_are_empty_and_far_away_ones_are_refused() {
    // Real Photoshop output (psd-tools' `vector-mask2.psd`) has
    // `bottom = top - 1` on a fill layer: read as empty, not refused.
    let inverted = TestPsd::new(1, 4, 4, 8)
        .with(|p| {
            p.layers.push(TestLayer::empty("bad").with(|l| {
                l.top = 3;
                l.bottom = 1;
            }));
        })
        .write();
    let document = doc(&inverted);
    assert_eq!(document.layers.len(), 1);
    assert!(document.pixels.is_empty());
    let far = TestPsd::new(1, 4, 4, 8)
        .with(|p| {
            p.layers.push(TestLayer::empty("far").with(|l| {
                l.left = 400_000;
                l.right = 400_000;
            }));
        })
        .write();
    assert!(matches!(decode(&far), Err(IoError::PsdMalformed { .. })));
}

// ---------------------------------------------------------------------
// PackBits
// ---------------------------------------------------------------------

#[test]
fn packbits_literal_repeat_noop_overrun_and_short_rows() {
    let mut out = Vec::new();
    // literal 3, no-op, repeat 4 x 9
    ok(unpack_bits(&[2, 1, 2, 3, 0x80, 0xFD, 9], &mut out, 7));
    assert_eq!(out, [1, 2, 3, 9, 9, 9, 9]);

    let mut out = Vec::new();
    // -1 repeats twice, not once and not three times
    ok(unpack_bits(&[0xFF, 5], &mut out, 2));
    assert_eq!(out, [5, 5]);

    let mut out = Vec::new();
    assert!(matches!(
        unpack_bits(&[0xFD, 9], &mut out, 3),
        Err(IoError::PsdMalformed { .. })
    ));
    let mut out = Vec::new();
    assert!(matches!(
        unpack_bits(&[3, 1, 2, 3, 4], &mut out, 3),
        Err(IoError::PsdMalformed { .. })
    ));
    let mut out = Vec::new();
    assert!(matches!(
        unpack_bits(&[0xFF, 5], &mut out, 3),
        Err(IoError::PsdMalformed { .. })
    ));
    let mut out = Vec::new();
    assert!(matches!(
        unpack_bits(&[0x80, 0x80], &mut out, 1),
        Err(IoError::PsdMalformed { .. })
    ));

    // The writer and reader agree on awkward rows.
    for row in [
        vec![],
        vec![7],
        vec![1, 1],
        vec![1, 2, 2, 3, 3, 3],
        (0..=255).collect::<Vec<u8>>(),
        vec![4; 300],
    ] {
        let mut out = Vec::new();
        ok(unpack_bits(&pack_bits(&row), &mut out, row.len()));
        assert_eq!(out, row);
    }
}

// ---------------------------------------------------------------------
// Robustness sweeps
// ---------------------------------------------------------------------

fn sweep_fixtures() -> Vec<Vec<u8>> {
    let px = two_by_two(8);
    let px16 = two_by_two(16);
    let mut files = Vec::new();
    for compression in 0..=3 {
        files.push(
            TestPsd::new(1, 3, 3, 8)
                .with(|p| {
                    p.layers = vec![
                        TestLayer::divider(),
                        TestLayer::pixels("a", 0, 0, 2, 2, 8, &px).with(|l| {
                            l.compression = compression;
                            l.mask = Some((0, 0, 1, 2, 255, 0));
                            l.channels.push((-2, vec![9, 200]));
                            l.fill = Some(10);
                        }),
                        TestLayer::group("G", *b"pass"),
                    ];
                })
                .write(),
        );
        files.push(
            TestPsd::new(2, 2, 2, 16)
                .with(|p| {
                    p.lr16 = true;
                    p.layers = vec![
                        TestLayer::pixels("b", 0, 0, 2, 2, 16, &px16)
                            .with(|l| l.compression = compression),
                    ];
                })
                .write(),
        );
        files.push(
            TestPsd::new(1, 3, 3, 8)
                .with(|p| {
                    p.color_mode = 1;
                    p.channels = 1;
                    p.layers = vec![
                        TestLayer::gray("g", 0, 0, 2, 2, 8, &[[1, 255], [2, 0], [3, 9], [4, 255]])
                            .with(|l| {
                                l.compression = compression;
                                l.mask = Some((1, 1, 3, 3, 0, 0x15));
                                l.mask_tail = Some(vec![0x03, 9, 0, 0, 0, 0, 0, 0, 0, 1]);
                                l.channels.push((-2, vec![5, 6, 7, 8]));
                            }),
                    ];
                })
                .write(),
        );
        files.push(
            TestPsd::new(1, 2, 2, 8)
                .with(|p| {
                    p.merged = Some(vec![vec![1, 2, 3, 4]; 3]);
                    p.merged_compression = compression;
                })
                .write(),
        );
    }
    // 0.158.0: a Curves layer (legacy curves and `Crv ` extra data, a
    // moved endpoint, a mask) inside a Pass Through group, over pixels.
    files.push(curves_file(&curv_rgb_block(), |_| {}).write());
    files
}

#[test]
fn every_truncation_of_every_sweep_file_is_an_error_or_a_document_never_a_panic() {
    for file in sweep_fixtures() {
        for len in 0..file.len() {
            let _ = read(file.get(..len).unwrap_or(&[]));
        }
        assert!(read(&file).is_ok());
    }
}

#[test]
fn seeded_single_byte_mutations_never_panic() {
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        state >> 33
    };
    for file in sweep_fixtures() {
        for _ in 0..2_000 {
            let mut mutated = file.clone();
            let at = (next() as usize) % mutated.len().max(1);
            let value = next() as u8;
            if let Some(byte) = mutated.get_mut(at) {
                *byte = value;
            }
            let _ = read(&mutated);
        }
    }
}

// ---------------------------------------------------------------------
// Real fixtures (psd-tools' MIT suite; expected values read with
// psd-tools 1.17.4)
// ---------------------------------------------------------------------

macro_rules! fixture {
    ($name:literal) => {
        include_bytes!(concat!("../../tests/fixtures/psd/", $name)).as_slice()
    };
}

fn names(tree: &LayerTree, ids: &[LayerId]) -> Vec<String> {
    ids.iter()
        .map(|id| tree.name(*id).unwrap_or("?").to_owned())
        .collect()
}

#[test]
fn real_1layer_psd_and_psb_open_with_the_right_pixels() {
    for bytes in [fixture!("1layer.psd"), fixture!("1layer.psb")] {
        let document = doc(bytes);
        assert_eq!(document.canvas_size, (101, 55));
        let id = root(&document.layers, 0);
        assert_eq!(document.layers.name(id), Some("Фон"));
        let Some(image) = image_of(&document, id) else {
            unreachable!("no pixels");
        };
        assert_eq!(px(image, 0, 0), u8px(255, 255, 255, 255));
        assert_eq!(px(image, 50, 27), u8px(95, 225, 37, 255));
        assert_eq!(px(image, 100, 54), u8px(255, 255, 255, 255));
        assert!(document.report.is_empty(), "{:?}", document.report);
    }
}

#[test]
fn real_2layers_psd_opens_both_layers_in_order() {
    let document = doc(fixture!("2layers.psd"));
    let tree = &document.layers;
    assert_eq!(names(tree, tree.roots()), ["Слой", "Фон"]);
    let top = root(tree, 0);
    // Photoshop's own (8, 4, 85, 46) content crop, placed inside
    // canvas-anchored bounds.
    assert_eq!(tree.bounds(top), Some(rect(0, 0, 101, 55)));
    assert_eq!(offset_of(&document, top), Some((8, 4)));
    let Some(image) = image_of(&document, top) else {
        unreachable!("no pixels");
    };
    assert_eq!(px(image, 0, 0), u8px(0, 0, 0, 0));
    assert_eq!(px(image, 42, 23), u8px(242, 244, 194, 42));
    let Some(bottom) = image_of(&document, root(tree, 1)) else {
        unreachable!("no pixels");
    };
    assert_eq!(px(bottom, 50, 27), u8px(15, 186, 55, 255));
}

#[test]
fn real_16bit_psd_reads_its_layers_from_lr16() {
    let document = doc(fixture!("16bit5x5.psd"));
    let tree = &document.layers;
    assert_eq!(
        names(tree, tree.roots()),
        ["Background copy 2", "Background copy", "Background"]
    );
    let top = root(tree, 0);
    assert_eq!(tree.bounds(top), Some(rect(0, 0, 5, 5)));
    assert_eq!(offset_of(&document, top), Some((4, 1)));
    let Some(image) = image_of(&document, top) else {
        unreachable!("no pixels");
    };
    assert_eq!(px(image, 0, 2), expected(16, [12020, 50409, 26806, 65535]));
    let Some(bg) = image_of(&document, root(tree, 2)) else {
        unreachable!("no pixels");
    };
    assert_eq!(px(bg, 2, 2), expected(16, [60539, 62141, 64391, 65535]));
}

#[test]
fn real_0layers_psd_opens_its_merged_image() {
    let document = doc(fixture!("0layers.psd"));
    assert_eq!(document.canvas_size, (1600, 1200));
    let id = root(&document.layers, 0);
    assert_eq!(document.layers.name(id), Some("Background"));
    let Some(image) = image_of(&document, id) else {
        unreachable!("no pixels");
    };
    assert_eq!(px(image, 800, 600), u8px(255, 255, 255, 255));
}

#[test]
fn real_group_and_hidden_layer_fixtures() {
    let document = doc(fixture!("group.psd"));
    let tree = &document.layers;
    assert_eq!(names(tree, tree.roots()), ["Group 1", "Background"]);
    let group = root(tree, 0);
    assert_eq!(
        names(tree, tree.children(group).unwrap_or_default()),
        ["Shape 1"]
    );
    let shape = child(tree, group, 0);
    let Some(image) = image_of(&document, shape) else {
        unreachable!("no pixels");
    };
    assert_eq!(px(image, 20, 37), u8px(0, 0, 0, 255));
    assert_eq!(px(image, 0, 0), u8px(0, 0, 0, 0));
    assert!(
        document
            .report
            .items
            .iter()
            .any(|i| i.contains("shape layer")),
        "{:?}",
        document.report
    );

    let document = doc(fixture!("hidden-layer.psd"));
    let tree = &document.layers;
    assert_eq!(
        names(tree, tree.roots()),
        ["Shape 2", "Shape 1", "Background"]
    );
    assert_eq!(tree.visible(root(tree, 0)), Some(false));
    assert_eq!(tree.visible(root(tree, 1)), Some(true));
}

#[test]
fn real_emoji_name_and_linear_dodge_at_half_opacity() {
    let document = doc(fixture!("layer-name-emoji.psd"));
    let tree = &document.layers;
    let id = root(tree, 0);
    assert_eq!(tree.name(id), Some("👽"));
    assert_eq!(tree.blend_mode(id), Some(BlendMode::LinearDodge));
    assert_eq!(tree.opacity(id), Some(128.0 / 255.0));
}

#[test]
fn real_unsupported_fixtures_give_their_specific_errors() {
    assert!(matches!(
        decode(fixture!("32bit5x5.psd")),
        Err(IoError::UnsupportedPsdDepth(32))
    ));
    assert!(matches!(
        decode(fixture!("4x4_8bit_lab.psd")),
        Err(IoError::UnsupportedPsdColorMode(9))
    ));
    // Grayscale opens since 0.147.0 — see
    // `real_grayscale_fixture_matches_psd_tools`.
    assert!(decode(fixture!("4x4_8bit_grayscale.psd")).is_ok());
}

#[test]
fn every_real_fixture_survives_truncation() {
    for bytes in [
        fixture!("1layer.psd"),
        fixture!("2layers.psd"),
        fixture!("16bit5x5.psd"),
        fixture!("group.psd"),
    ] {
        // Every 7th prefix keeps this fast; the synthetic sweep above
        // takes every one.
        for len in (0..bytes.len()).step_by(7) {
            let _ = read(bytes.get(..len).unwrap_or(&[]));
        }
    }
}

// ---------------------------------------------------------------------
// The corpus
// ---------------------------------------------------------------------

/// Opens every `.psd`/`.psb` in the gitignored psd-tools corpus
/// (`corpora/psd/reference/psd-tools-fixtures`, fetched by its own
/// script). Asserts no panic and that every refusal is a typed PSD
/// error; prints the Ok / unsupported / malformed counts. Prints
/// `SKIPPED` when the corpus is absent rather than being `#[ignore]`d.
#[test]
fn corpus_sweep_opens_or_refuses_every_file_with_a_typed_error() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpora/psd/reference/psd-tools-fixtures");
    if !dir.is_dir() {
        println!("SKIPPED: corpus not present at {}", dir.display());
        return;
    }
    let mut pending = vec![dir];
    let mut files = Vec::new();
    while let Some(next) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("psd") || e.eq_ignore_ascii_case("psb"))
            {
                files.push(path);
            }
        }
    }
    files.sort();
    let (mut opened, mut unsupported, mut malformed) = (0, 0, Vec::new());
    for path in &files {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        match read(&bytes) {
            Ok(_) => opened += 1,
            Err(
                IoError::UnsupportedPsdColorMode(_)
                | IoError::UnsupportedPsdDepth(_)
                | IoError::UnsupportedPsdCompression(_)
                | IoError::UnsupportedPsdVersion(_),
            ) => unsupported += 1,
            Err(
                err @ (IoError::PsdTruncated { .. }
                | IoError::PsdMalformed { .. }
                | IoError::PsdTooLarge { .. }
                | IoError::PsdPixelBudget { .. }
                | IoError::PsdGroupsTooDeep { .. }
                | IoError::NotPsd),
            ) => malformed.push(format!("{}: {err}", path.display())),
            Err(other) => unreachable!("{}: untyped error {other:?}", path.display()),
        }
    }
    println!(
        "corpus sweep: {} files, {opened} opened, {unsupported} unsupported, {} refused as \
         damaged",
        files.len(),
        malformed.len()
    );
    for line in &malformed {
        println!("  refused: {line}");
    }
    assert!(opened > 0);
}

#[test]
fn psd_file_report_is_exposed_before_building() {
    let file: PsdFile = ok(decode(fixture!("1layer.psd")));
    assert!(file.report().is_empty());
    assert_eq!(
        (file.version, file.width, file.height, file.depth),
        (1, 101, 55, 8)
    );
}

// ---------------------------------------------------------------------
// 0.144.0 review revision
// ---------------------------------------------------------------------

fn report_text(document: &PsdDocument) -> String {
    document.report.items.join("\n")
}

/// C-01: Photoshop crops a layer to its content, so its rectangle sits
/// inside the canvas. The layer's bounds are canvas-anchored and its
/// pixels land at their real document position once written through
/// `write_into_store_at` — read back from a real tile store.
#[test]
fn an_offset_layer_gets_canvas_anchored_bounds_and_its_pixels_land_in_place() {
    use aurora_tile::{TILE, TileId, TileStore};
    let bytes = TestPsd::new(1, 300, 280, 8)
        .with(|p| {
            p.layers.push(TestLayer::pixels(
                "cropped",
                // Straddles the first tile boundary on both axes.
                i32::try_from(TILE).unwrap_or(256) - 1,
                i32::try_from(TILE).unwrap_or(256) - 1,
                2,
                2,
                8,
                &[
                    [10, 20, 30, 255],
                    [40, 50, 60, 255],
                    [70, 80, 90, 255],
                    [1, 2, 3, 255],
                ],
            ));
        })
        .write();
    let document = doc(&bytes);
    let id = root(&document.layers, 0);
    assert_eq!(document.layers.bounds(id), Some(rect(0, 0, 300, 280)));
    assert_eq!(offset_of(&document, id), Some((TILE - 1, TILE - 1)));

    let dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(err) => unreachable!("{err:?}"),
    };
    let Some(budget) = std::num::NonZeroUsize::new(16) else {
        unreachable!("16 is non-zero");
    };
    let mut store = match TileStore::new(dir.path().to_path_buf(), budget) {
        Ok(store) => store,
        Err(err) => unreachable!("{err:?}"),
    };
    let Some(surface) = document.layers.surface_id(id) else {
        unreachable!("a pixel layer has a surface");
    };
    let Some(placed) = document.pixels.iter().find(|p| p.layer == id) else {
        unreachable!("no pixels");
    };
    let (dx, dy) = placed.offset;
    ok(crate::write_into_store_at(
        &placed.image,
        &mut store,
        surface,
        dx,
        dy,
    ));
    // Exactly the four tiles the 2x2 straddles were written.
    assert_eq!(store.resident_len(), 4);
    let texel = |store: &mut TileStore, x: u32, y: u32| -> [f32; 4] {
        let tile = match store.get(
            surface,
            TileId {
                x: x / TILE,
                y: y / TILE,
            },
        ) {
            Ok(tile) => tile,
            Err(err) => unreachable!("{err:?}"),
        };
        let i = (((y % TILE) * TILE + x % TILE) * 4) as usize;
        let mut out = [f32::NAN; 4];
        for (c, slot) in out.iter_mut().enumerate() {
            *slot = tile
                .texels()
                .get(i + c)
                .copied()
                .map_or(f32::NAN, f16::to_f32);
        }
        out
    };
    // Document position (TILE-1, TILE-1) holds the layer's first pixel,
    // (TILE, TILE) its last; the canvas origin is transparent.
    assert_eq!(texel(&mut store, TILE - 1, TILE - 1), u8px(10, 20, 30, 255));
    assert_eq!(texel(&mut store, TILE, TILE - 1), u8px(40, 50, 60, 255));
    assert_eq!(texel(&mut store, TILE - 1, TILE), u8px(70, 80, 90, 255));
    assert_eq!(texel(&mut store, TILE, TILE), u8px(1, 2, 3, 255));
    assert_eq!(texel(&mut store, 0, 0), [0.0; 4]);
}

/// C-02: the opened file is the undo baseline — the journal keeps every
/// build step (autosave/recovery replay it), the undo stack keeps none.
#[test]
fn an_opened_psd_cannot_be_undone_but_its_journal_is_kept() {
    let bytes = TestPsd::new(1, 4, 4, 8)
        .with(|p| {
            p.layers.push(
                TestLayer::pixels("a", 0, 0, 1, 1, 8, &[[1, 2, 3, 255]]).with(|l| l.opacity = 128),
            );
        })
        .write();
    let document = doc(&bytes);
    assert!(!document.history.can_undo());
    assert!(!document.history.can_redo());
    assert_eq!(document.history.journal_len(), 2, "add + opacity");
}

/// C-04 / RT-01: a 16384² rectangle with no channels at all used to
/// allocate (and fill) a 2 GiB buffer from a ~150-byte file. It now opens
/// as an empty layer, reported, without sizing anything from the
/// rectangle.
#[test]
fn a_huge_layer_with_no_channels_opens_empty_without_allocating() {
    let bytes = TestPsd::new(1, 16_384, 16_384, 8)
        .with(|p| {
            p.merged = Some(Vec::new());
            p.layers.push(
                TestLayer::pixels("liar", 0, 0, 16_384, 16_384, 8, &[])
                    .with(|l| l.channels.clear()),
            );
        })
        .write();
    assert!(bytes.len() < 512, "{} bytes", bytes.len());
    let start = std::time::Instant::now();
    let document = doc(&bytes);
    let elapsed = start.elapsed();
    assert!(elapsed.as_millis() < 100, "took {elapsed:?}");
    assert!(document.pixels.is_empty());
    assert!(
        report_text(&document).contains("stored no colour or transparency channels"),
        "{}",
        report_text(&document)
    );
}

/// C-04: an RLE channel far too short for its declared rectangle is
/// refused before the rectangle's buffer exists.
#[test]
fn an_rle_channel_too_short_for_its_rectangle_is_refused_before_allocating() {
    let bytes = TestPsd::new(1, 16_384, 16_384, 8)
        .with(|p| {
            p.merged = Some(Vec::new());
            p.layers.push(
                // Empty planes: each channel is its 2-byte compression
                // field and nothing else.
                TestLayer::pixels("short", 0, 0, 16_384, 16_384, 8, &[]).with(|l| {
                    l.compression = 1;
                }),
            );
        })
        .write();
    let start = std::time::Instant::now();
    let result = read(&bytes);
    let elapsed = start.elapsed();
    assert!(
        matches!(result, Err(IoError::PsdMalformed { .. })),
        "{result:?}"
    );
    assert!(elapsed.as_millis() < 100, "took {elapsed:?}");
}

/// C-04: the minimum sizes themselves.
#[test]
fn minimum_channel_sizes_are_sound_lower_bounds() {
    use super::min_channel_len;
    // Raw: exact.
    assert_eq!(ok(min_channel_len(0, 10, 3, false)), 30);
    // RLE: the row table (2 bytes/row in PSD, 4 in PSB) plus 2 bytes per
    // 128-byte run per row.
    assert_eq!(ok(min_channel_len(1, 128, 3, false)), 6 + 6);
    assert_eq!(ok(min_channel_len(1, 129, 3, true)), 12 + 12);
    // A real PackBits encoding is never below the bound.
    for len in [1_usize, 127, 128, 129, 300, 1000] {
        let row = vec![7_u8; len];
        let packed = pack_bits(&row);
        assert!(packed.len() >= len.div_ceil(128) * 2, "{len}");
    }
    // ZIP: half deflate's 1032:1 maximum.
    assert_eq!(ok(min_channel_len(2, 2064, 10, false)), 10);
    let zipped = super::test_writer::zlib(&vec![0_u8; 2064 * 10]);
    assert!(zipped.len() >= 10, "{}", zipped.len());
    assert!(matches!(
        min_channel_len(9, 1, 1, false),
        Err(IoError::UnsupportedPsdCompression(9))
    ));
}

/// C-05 / RT-02: only the first channel of each id is decoded. The
/// duplicate here is a valid zlib stream of the wrong size: decoding it
/// would be an error, so a clean open proves it was never inflated.
#[test]
fn duplicate_channel_ids_use_the_first_and_never_decode_the_rest() {
    let bytes = TestPsd::new(1, 2, 1, 8)
        .with(|p| {
            p.layers.push(
                TestLayer::pixels(
                    "dup",
                    0,
                    0,
                    2,
                    1,
                    8,
                    &[[10, 20, 30, 255], [40, 50, 60, 255]],
                )
                .with(|l| {
                    l.compression = 2;
                    for _ in 0..50 {
                        l.channels.push((0, Vec::new()));
                    }
                }),
            );
        })
        .write();
    let document = doc(&bytes);
    let Some(image) = image_of(&document, root(&document.layers, 0)) else {
        unreachable!("no pixels");
    };
    assert_eq!(px(image, 0, 0), u8px(10, 20, 30, 255));
    assert_eq!(px(image, 1, 0), u8px(40, 50, 60, 255));
    assert!(
        report_text(&document).contains("50 repeated layer channels were ignored"),
        "{}",
        report_text(&document)
    );
}

/// C-06: a damaged user mask is reported and dropped; the file opens.
#[test]
fn a_damaged_mask_is_reported_and_the_file_still_opens() {
    let bytes = TestPsd::new(1, 4, 4, 8)
        .with(|p| {
            p.layers.push(
                TestLayer::pixels("masked", 0, 0, 1, 1, 8, &[[1, 2, 3, 255]]).with(|l| {
                    l.mask = Some((0, 0, 4, 4, 255, 0));
                    // 16 samples declared, none stored.
                    l.channels.push((-2, Vec::new()));
                }),
            );
        })
        .write();
    let document = doc(&bytes);
    assert_eq!(document.pixels.len(), 1);
    assert!(
        report_text(&document).contains("could not be read"),
        "{}",
        report_text(&document)
    );
}

/// C-08: with merged transparency, the merged image's fourth channel is
/// its alpha and its colour is un-matted from white (psd-tools'
/// `_remove_white_background`).
#[test]
fn the_merged_fallback_uses_merged_alpha_and_removes_the_white_matte() {
    let bytes = TestPsd::new(1, 1, 1, 8)
        .with(|p| {
            p.channels = 4;
            p.negative_count = true;
            // Every record left out, so the merged image is used.
            p.layers.push(
                TestLayer::pixels("levels", 0, 0, 1, 1, 8, &[[0, 0, 0, 255]])
                    .with(|l| l.blocks.push((*b"levl", vec![0; 4]))),
            );
            // c' = c*a + (1 - a), a = 128/255: c = 0, 1, ~0.5.
            p.merged = Some(vec![vec![127], vec![255], vec![191], vec![128]]);
        })
        .write();
    let document = doc(&bytes);
    let Some(placed) = document.pixels.first() else {
        unreachable!("no merged pixels");
    };
    let [r, g, b, a] = px(&placed.image, 0, 0);
    assert_eq!(a, f(128.0 / 255.0));
    assert!(r.abs() < 1e-3, "{r}");
    assert!((g - 1.0).abs() < 1e-3, "{g}");
    assert!((b - 0.5).abs() < 4e-3, "{b}");
    // Its alpha is not reported as an unopened extra channel.
    assert!(
        !report_text(&document).contains("saved channel"),
        "{}",
        report_text(&document)
    );
}

/// C-09/C-10: a fill layer with pixels and no vector mask is a fill, not
/// a shape; Blend If and Knockout are reported.
#[test]
fn fill_shape_blend_if_and_knockout_are_reported_distinctly() {
    let bytes = TestPsd::new(1, 2, 2, 8)
        .with(|p| {
            p.layers.push(
                TestLayer::pixels("fill", 0, 0, 1, 1, 8, &[[1, 2, 3, 255]])
                    .with(|l| l.blocks.push((*b"SoCo", vec![0; 4]))),
            );
            p.layers.push(
                TestLayer::pixels("shape", 0, 0, 1, 1, 8, &[[1, 2, 3, 255]]).with(|l| {
                    l.blocks.push((*b"SoCo", vec![0; 4]));
                    l.blocks.push((*b"vmsk", vec![0; 4]));
                }),
            );
            p.layers.push(
                TestLayer::pixels("knock", 0, 0, 1, 1, 8, &[[1, 2, 3, 255]])
                    .with(|l| l.blocks.push((*b"knko", vec![1, 0, 0, 0]))),
            );
        })
        .write();
    let text = report_text(&doc(&bytes));
    assert!(text.contains("1 fill layer (solid colour"), "{text}");
    assert!(text.contains("1 shape layer opened as pixels"), "{text}");
    assert!(text.contains("Knockout"), "{text}");
    assert!(super::blend_if_in_use(&[
        0, 0, 0xFF, 0xFF, 0, 10, 0xFF, 0xFF
    ]));
    assert!(!super::blend_if_in_use(&[
        0, 0, 0xFF, 0xFF, 0, 0, 0xFF, 0xFF
    ]));
    assert!(!super::blend_if_in_use(&[]));
}

/// C-11: a rectangle whose right/bottom edge lies past the document
/// range is refused with a typed error, not silently accepted.
#[test]
fn a_rectangle_whose_far_edge_is_out_of_range_is_refused() {
    use super::rect_from_edges;
    let max = i32::try_from(aurora_core::MAX_DOCUMENT_ORIGIN).unwrap_or(i32::MAX);
    assert!(matches!(
        rect_from_edges(0, max - 10, 10, max + 10),
        Err(IoError::PsdMalformed { .. })
    ));
    assert!(rect_from_edges(0, max - 10, 10, max).is_ok());
}

/// `write_into_store_at` refuses a placement whose far edge overflows.
#[test]
fn write_into_store_at_refuses_an_overflowing_offset() {
    use aurora_tile::{SurfaceId, TileStore};
    let dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(err) => unreachable!("{err:?}"),
    };
    let Some(budget) = std::num::NonZeroUsize::new(4) else {
        unreachable!("4 is non-zero");
    };
    let mut store = match TileStore::new(dir.path().to_path_buf(), budget) {
        Ok(store) => store,
        Err(err) => unreachable!("{err:?}"),
    };
    let image = ok(Image::new(
        2,
        1,
        aurora_color::IccProfile::srgb(),
        vec![f16::ONE; 8],
    ));
    let result =
        crate::write_into_store_at(&image, &mut store, SurfaceId::from_raw(0), u32::MAX, 0);
    assert!(
        matches!(result, Err(IoError::ImagePlacementOutOfRange { .. })),
        "{result:?}"
    );
    assert_eq!(store.resident_len(), 0);
}

// ---------------------------------------------------------------------
// 0.147.0: user masks applied, and Grayscale
// ---------------------------------------------------------------------

/// A mask as `(top, left, bottom, right, default colour, flags)`, the
/// test writer's own tuple.
type MaskSpec = (i32, i32, i32, i32, u8, u8);

/// The coverage the opened document gives `id` at document `(x, y)`,
/// read the way `aurora-app`'s compositor reads it: `0.0` outside the
/// attached mask's bounds, the written value where `masks` covers it,
/// `1.0` (never painted) elsewhere inside. `None` without a mask.
fn effective_mask(document: &PsdDocument, id: LayerId, x: i64, y: i64) -> Option<f32> {
    let mask = document.layers.mask(id)?;
    assert!(
        !mask.inverted,
        "the import bakes inversion into the coverage"
    );
    if !mask.bounds.contains_point(x, y) {
        return Some(0.0);
    }
    let (lx, ly) = (x - mask.bounds.x, y - mask.bounds.y);
    for written in document.masks.iter().filter(|m| m.layer == id) {
        let (ox, oy) = (i64::from(written.offset.0), i64::from(written.offset.1));
        if lx >= ox
            && ly >= oy
            && lx < ox + i64::from(written.width)
            && ly < oy + i64::from(written.height)
        {
            let i = usize::try_from((ly - oy) * i64::from(written.width) + (lx - ox)).ok()?;
            return written.coverage.get(i).map(|v| v.to_f32());
        }
    }
    Some(1.0)
}

/// Photoshop's own reading of a user mask at document `(x, y)`: the
/// stored sample inside the rectangle, the default colour outside, both
/// inverted when flags bit 2 says so.
fn photoshop_mask(spec: MaskSpec, samples: &[f32], x: i64, y: i64) -> f32 {
    let (top, left, bottom, right, default, flags) = spec;
    let (top, left, bottom, right) = (
        i64::from(top),
        i64::from(left),
        i64::from(bottom),
        i64::from(right),
    );
    let value = if x >= left && x < right && y >= top && y < bottom {
        let i = usize::try_from((y - top) * (right - left) + (x - left)).unwrap_or(usize::MAX);
        samples.get(i).copied().unwrap_or(f32::NAN)
    } else if default == 0 {
        0.0
    } else {
        1.0
    };
    if flags & 0x04 != 0 {
        1.0 - value
    } else {
        value
    }
}

/// One opaque 3×3 layer at `(1, 1)` on a 4×4 canvas (so its bounds are
/// the canvas), carrying `spec` and the raw `-2` channel `plane`.
fn masked_file(depth: u16, spec: MaskSpec, plane: Vec<u8>, compression: u16) -> TestPsd {
    TestPsd::new(1, 4, 4, depth).with(|p| {
        p.layers.push(
            TestLayer::pixels("m", 1, 1, 3, 3, depth, &[[200, 100, 50, 255]; 9]).with(|l| {
                l.mask = Some(spec);
                l.channels.push((-2, plane));
                l.compression = compression;
            }),
        );
    })
}

/// Every in-layer point of `document`'s only layer agrees with
/// Photoshop's reading of `spec`.
fn assert_mask_matches_photoshop(document: &PsdDocument, spec: MaskSpec, samples: &[f32]) {
    let id = root(&document.layers, 0);
    for y in 1..4 {
        for x in 1..4 {
            let got = effective_mask(document, id, x, y);
            let want = f(photoshop_mask(spec, samples, x, y));
            assert_eq!(got, Some(want), "({x}, {y}) for {spec:?}");
        }
    }
}

#[test]
fn a_mask_is_applied_with_either_default_colour_and_a_rect_partly_off_the_canvas() {
    // 4×4 rectangle at (2, -1): two columns and one row off the canvas.
    let plane: Vec<u8> = (0..16).map(|i| i * 16).collect();
    let samples: Vec<f32> = plane.iter().map(|v| f32::from(*v) / 255.0).collect();
    for compression in 0..=3 {
        for default in [0, 255] {
            let spec = (-1, 2, 3, 6, default, 0);
            let document = doc(&masked_file(8, spec, plane.clone(), compression).write());
            assert_mask_matches_photoshop(&document, spec, &samples);
            let id = root(&document.layers, 0);
            let Some(mask) = document.layers.mask(id) else {
                unreachable!("mask attached");
            };
            assert!(mask.enabled);
            // Hidden outside: the PSD rectangle itself. Shown outside: the
            // layer's own bounds, with only the overlap written.
            if default == 0 {
                assert_eq!(mask.bounds, rect(2, -1, 4, 4));
            } else {
                assert_eq!(mask.bounds, rect(0, 0, 4, 4));
                let [written] = document.masks.as_slice() else {
                    unreachable!("one mask written");
                };
                assert_eq!(
                    (written.offset, written.width, written.height),
                    ((2, 0), 2, 3)
                );
            }
            assert!(
                report_text(&document).is_empty(),
                "{}",
                report_text(&document)
            );
        }
    }
}

#[test]
fn a_mask_rect_far_outside_the_layer_hides_or_shows_all_of_it() {
    for default in [0, 255] {
        let spec = (100_000, 100_000, 100_002, 100_002, default, 0);
        let document = doc(&masked_file(8, spec, vec![0; 4], 0).write());
        assert_mask_matches_photoshop(&document, spec, &[0.0; 4]);
        if default == 255 {
            assert!(document.masks.is_empty(), "no overlap, nothing written");
        }
    }
}

#[test]
fn a_disabled_mask_is_attached_disabled_with_its_coverage() {
    let spec = (1, 1, 3, 3, 0, 0x02);
    let document = doc(&masked_file(8, spec, vec![0, 64, 128, 255], 0).write());
    let id = root(&document.layers, 0);
    assert_eq!(document.layers.mask(id).map(|m| m.enabled), Some(false));
    let samples = [0.0, 64.0 / 255.0, 128.0 / 255.0, 1.0];
    assert_mask_matches_photoshop(&document, spec, &samples);
    assert!(report_text(&document).is_empty());
}

#[test]
fn an_inverted_mask_flips_its_samples_and_its_default_colour() {
    for default in [0, 255] {
        let spec = (1, 1, 3, 3, default, 0x04);
        let document = doc(&masked_file(8, spec, vec![0, 64, 128, 255], 0).write());
        let samples = [0.0, 64.0 / 255.0, 128.0 / 255.0, 1.0];
        let id = root(&document.layers, 0);
        for y in 1..4 {
            for x in 1..4 {
                let want = f16::from_f32(photoshop_mask(spec, &samples, x, y)).to_f32();
                // Inverted in f16 (1 - v, both f16), so compare loosely by
                // one f16 step.
                let got = effective_mask(&document, id, x, y).unwrap_or(f32::NAN);
                assert!(
                    (got - want).abs() <= 1.0 / 1024.0,
                    "({x}, {y}): {got} vs {want}"
                );
            }
        }
    }
}

#[test]
fn a_sixteen_bit_mask_keeps_its_low_byte() {
    let values: [u16; 4] = [0x0000, 0x8000, 0x0101, 0xFFFF];
    let plane: Vec<u8> = values.iter().flat_map(|v| v.to_be_bytes()).collect();
    let samples: Vec<f32> = values.iter().map(|v| f32::from(*v) / 65535.0).collect();
    for compression in 0..=3 {
        let spec = (1, 1, 3, 3, 0, 0);
        let document = doc(&masked_file(16, spec, plane.clone(), compression).write());
        assert_mask_matches_photoshop(&document, spec, &samples);
    }
}

#[test]
fn a_relative_position_flag_does_not_move_the_mask_and_is_reported_where_it_matters() {
    // Layer at (1, 1): the two readings would differ, so it is reported.
    let spec = (1, 1, 3, 3, 0, 0x01);
    let document = doc(&masked_file(8, spec, vec![255; 4], 0).write());
    assert_mask_matches_photoshop(&document, spec, &[1.0; 4]);
    assert!(report_text(&document).contains("positioned relative"));
    // A layer at the origin: the same place either way, nothing said.
    let at_origin = TestPsd::new(1, 2, 2, 8)
        .with(|p| {
            p.layers.push(
                TestLayer::pixels("o", 0, 0, 2, 2, 8, &two_by_two(8)).with(|l| {
                    l.mask = Some((0, 0, 1, 1, 0, 0x01));
                    l.channels.push((-2, vec![255]));
                }),
            );
        })
        .write();
    assert!(report_text(&doc(&at_origin)).is_empty());
}

// -- 0.149.0: mask density ---------------------------------------------

/// The opened `masked_file` with `tail` as its mask's parameter block
/// (flags carry bit 4, plus `extra_flags`).
fn with_parameters(extra_flags: u8, tail: Vec<u8>) -> PsdDocument {
    let spec = (1, 1, 3, 3, 0, 0x10 | extra_flags);
    let mut file = masked_file(8, spec, vec![0, 64, 128, 255], 0);
    if let Some(layer) = file.layers.first_mut() {
        layer.mask_tail = Some(tail);
    }
    doc(&file.write())
}

/// The density the opened document attached to its only layer's mask.
fn only_density(document: &PsdDocument) -> Option<f32> {
    document
        .layers
        .mask(root(&document.layers, 0))
        .map(|m| m.density)
}

/// psd-tools' effective coverage: `d * c + (1 - d)`, `c` after invert.
fn with_density(coverage: f32, density: f32) -> f32 {
    density * coverage + (1.0 - density)
}

#[test]
fn user_mask_density_is_attached_as_the_masks_density_and_not_baked_or_reported() {
    let samples = [0.0, 64.0 / 255.0, 128.0 / 255.0, 1.0];
    for density in [0_u8, 1, 128, 254, 255] {
        let document = with_parameters(0, vec![0x01, density]);
        assert_eq!(
            only_density(&document),
            Some(f32::from(density) / 255.0),
            "{density}"
        );
        // The coverage itself is the file's, untouched: density stays
        // editable rather than baked in.
        assert_mask_matches_photoshop(&document, (1, 1, 3, 3, 0, 0x10), &samples);
        assert!(
            report_text(&document).is_empty(),
            "{density}: {}",
            report_text(&document)
        );
    }
}

#[test]
fn mask_density_applies_to_the_coverage_after_the_files_invert_flag() {
    // Inverted (bit 2) at density 128: psd-tools inverts the sample and
    // then applies `d * c + (1 - d)`. The import bakes the inversion into
    // the coverage, so the attached density over the attached coverage
    // must give the same number.
    let spec = (1, 1, 3, 3, 0, 0x14);
    let mut file = masked_file(8, spec, vec![0, 64, 128, 255], 0);
    if let Some(layer) = file.layers.first_mut() {
        layer.mask_tail = Some(vec![0x01, 128]);
    }
    let document = doc(&file.write());
    let id = root(&document.layers, 0);
    let density = 128.0 / 255.0;
    assert_eq!(only_density(&document), Some(density));
    let samples = [0.0, 64.0 / 255.0, 128.0 / 255.0, 1.0];
    for y in 1..4 {
        for x in 1..4 {
            let want = with_density(photoshop_mask(spec, &samples, x, y), density);
            let got = with_density(
                effective_mask(&document, id, x, y).unwrap_or(f32::NAN),
                density,
            );
            assert!(
                (got - want).abs() <= 1.0 / 1024.0,
                "({x}, {y}): {got} vs {want}"
            );
        }
    }
}

#[test]
fn feather_and_unused_vector_parameters_are_still_reported_and_density_is_not() {
    let feather = |flag: u8, lead: &[u8], value: f64| {
        let mut tail = vec![flag];
        tail.extend_from_slice(lead);
        tail.extend_from_slice(&value.to_be_bytes());
        tail
    };
    let full = Some(1.0);
    for (what, tail, density, reported) in [
        ("user feather", feather(0x02, &[], 2.5), full, true),
        (
            "density + zero feather",
            feather(0x03, &[128], 0.0),
            Some(128.0 / 255.0),
            false,
        ),
        // psd-tools' fallback: a vector density with no user density is
        // applied to the user mask.
        (
            "vector density only",
            vec![0x04, 64],
            Some(64.0 / 255.0),
            false,
        ),
        (
            "user + vector density",
            vec![0x05, 128, 64],
            Some(128.0 / 255.0),
            true,
        ),
        (
            "user + full vector density",
            vec![0x05, 128, 255],
            Some(128.0 / 255.0),
            false,
        ),
        ("vector feather", feather(0x08, &[], 1.0), full, true),
        ("vector zero feather", feather(0x08, &[], 0.0), full, false),
        ("full density", vec![0x01, 255], full, false),
        ("nothing present", vec![0x00], full, false),
    ] {
        let document = with_parameters(0, tail);
        assert_eq!(only_density(&document), density, "{what}");
        let text = report_text(&document);
        assert_eq!(
            text.contains("feather or a vector-mask density"),
            reported,
            "{what}: {text}"
        );
        assert!(!text.contains("full density"), "{what}: {text}");
    }
}

#[test]
fn a_truncated_or_odd_parameter_block_keeps_what_was_read_and_never_panics() {
    for (what, tail, density) in [
        ("empty", Vec::new(), 1.0),
        ("density byte missing", vec![0x01], 1.0),
        (
            "feather truncated after density",
            vec![0x03, 9, 0, 0],
            9.0 / 255.0,
        ),
        ("every bit, one byte", vec![0xFF, 7], 7.0 / 255.0),
        ("unknown bits only", vec![0xF0], 1.0),
    ] {
        let document = with_parameters(0, tail);
        assert_eq!(only_density(&document), Some(density), "{what}");
        assert!(
            report_text(&document).is_empty(),
            "{what}: {}",
            report_text(&document)
        );
    }
    // Every prefix of a full block, and every single-byte mutation of it,
    // opens without a panic and with a density in range.
    let mut full = vec![0x0F, 128];
    full.extend_from_slice(&2.0_f64.to_be_bytes());
    full.push(64);
    full.extend_from_slice(&f64::NAN.to_be_bytes());
    for len in 0..=full.len() {
        let prefix = full.get(..len).map(<[u8]>::to_vec).unwrap_or_default();
        let d = only_density(&with_parameters(0, prefix)).unwrap_or(f32::NAN);
        assert!((0.0..=1.0).contains(&d), "prefix {len}: {d}");
    }
    for index in 0..full.len() {
        for byte in [0x00, 0x7F, 0xFF] {
            let mut mutated = full.clone();
            if let Some(slot) = mutated.get_mut(index) {
                *slot = byte;
            }
            let d = only_density(&with_parameters(0, mutated)).unwrap_or(f32::NAN);
            assert!((0.0..=1.0).contains(&d), "byte {index} = {byte}: {d}");
        }
    }
}

#[test]
fn a_group_mask_density_is_attached_to_the_group() {
    let bytes = TestPsd::new(1, 4, 4, 8)
        .with(|p| {
            p.layers = vec![
                TestLayer::divider(),
                TestLayer::pixels("in", 0, 0, 2, 2, 8, &two_by_two(8)),
                TestLayer::group("G", *b"norm").with(|l| {
                    l.mask = Some((0, 0, 1, 2, 255, 0x10));
                    l.mask_tail = Some(vec![0x01, 0]);
                    l.channels.push((-2, vec![0, 128]));
                }),
            ];
        })
        .write();
    let document = doc(&bytes);
    let group = root(&document.layers, 0);
    assert_eq!(document.layers.mask(group).map(|m| m.density), Some(0.0));
    // Coverage still the file's own; at density 0 it has no effect.
    assert_eq!(effective_mask(&document, group, 0, 0), Some(0.0));
    assert_eq!(with_density(0.0, 0.0), 1.0);
    assert!(
        report_text(&document).is_empty(),
        "{}",
        report_text(&document)
    );
}

/// Every layer named `name` in `tree`, depth first.
fn find_named(tree: &LayerTree, name: &str) -> Option<LayerId> {
    let mut stack: Vec<LayerId> = tree.roots().to_vec();
    while let Some(id) = stack.pop() {
        if tree.name(id) == Some(name) {
            return Some(id);
        }
        if let Some(children) = tree.children(id) {
            stack.extend_from_slice(children);
        }
    }
    None
}

/// The real psd-tools fixtures that carry a mask parameter block, checked
/// against psd-tools' own reading (`psd_tools` 1.x, `composite.py`
/// `_get_mask`: density = user, else vector, else 255; effective coverage
/// `d * m + (1 - d)` over the user mask). The expected numbers were
/// computed by psd-tools itself over each rectangle (the mask's bbox ∩
/// the layer's record rectangle ∩ the canvas), not by hand.
#[test]
#[allow(clippy::type_complexity, clippy::too_many_lines)] // a data table
fn corpus_mask_densities_and_effective_coverage_match_psd_tools() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpora/psd/reference/psd-tools-fixtures");
    if !dir.is_dir() {
        println!("SKIPPED: corpus not present at {}", dir.display());
        return;
    }
    // (file, layer, density byte, (x0, y0, x1, y1), psd-tools' sum)
    //
    // 0.150.0: `mask-density-layervectormask.psd`'s four layers left this
    // table — they carry both a user mask and a vector mask, whose two
    // densities are now baked into one combined coverage (density 1);
    // `corpus_vector_masks_match_psd_tools_and_photoshops_own_rendering`
    // covers them. The vector-only layers below now open as Aurora's own
    // raster of the vector mask, which matches Photoshop's rendering
    // these numbers were taken from exactly. `mask_parameters.psd`'s
    // "Rectangle 1" left too: its user density (204) now applies only to
    // its (empty, shown) real user mask, and the vector density (250) to
    // the vector raster — psd-tools instead applies 204 to the rendered
    // `-2` channel.
    let cases: &[(&str, &str, u8, (i64, i64, i64, i64), f64)] = &[
        (
            "mask-density-layermask.psd",
            "Rectangle 2 copy",
            64,
            (1, 25, 24, 32),
            145.591_773,
        ),
        (
            "mask-density-layermask.psd",
            "Rectangle 2",
            128,
            (0, 16, 24, 25),
            168.784_191,
        ),
        (
            "mask-density-layermask.psd",
            "Rectangle 2 copy 3",
            191,
            (1, 8, 24, 16),
            130.431_881,
        ),
        (
            "mask-density-layermask.psd",
            "Rectangle 2 copy 2",
            255,
            (0, 0, 24, 8),
            110.505_883,
        ),
        (
            "mask-density-vectormask.psd",
            "Layer 1",
            64,
            (15, 0, 32, 8),
            133.992_157,
        ),
        (
            "mask-density-vectormask.psd",
            "Layer 1 copy",
            128,
            (15, 8, 32, 16),
            131.984_314,
        ),
        (
            "mask-density-vectormask.psd",
            "Layer 1 copy 2",
            191,
            (15, 16, 32, 24),
            130.007_843,
        ),
        (
            "layer_mask_data.psd",
            "1",
            204,
            (12, 17, 68, 179),
            7_104.141_179,
        ),
        (
            "layer_mask_data.psd",
            "2",
            230,
            (22, 11, 179, 59),
            6_064.795_85,
        ),
        (
            "layer_mask_data.psd",
            "4",
            191,
            (12, 141, 188, 191),
            6_541.570_782,
        ),
    ];
    for &(file, layer, byte, (x0, y0, x1, y1), want) in cases {
        let bytes = match std::fs::read(dir.join(file)) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{file}: {err}"),
        };
        let document = doc(&bytes);
        let Some(id) = find_named(&document.layers, layer) else {
            unreachable!("{file}: no layer {layer:?}");
        };
        let density = document.layers.mask(id).map(|m| m.density);
        assert_eq!(density, Some(f32::from(byte) / 255.0), "{file} {layer}");
        let density = density.unwrap_or(f32::NAN);
        let mut sum = 0.0_f64;
        let mut n = 0_u32;
        for y in y0..y1 {
            for x in x0..x1 {
                let c = effective_mask(&document, id, x, y).unwrap_or(f32::NAN);
                sum += f64::from(with_density(c, density));
                n += 1;
            }
        }
        let tolerance = 1e-3 * f64::from(n);
        assert!(
            (sum - want).abs() <= tolerance,
            "{file} {layer}: Aurora {sum} vs psd-tools {want} over {n} px"
        );
        println!("{file} {layer}: Aurora {sum:.4} psd-tools {want:.4} ({n} px)");
    }
}

#[test]
fn a_real_user_mask_is_not_used_and_is_reported_while_the_user_mask_applies() {
    let spec = (1, 1, 3, 3, 0, 0);
    let mut file = masked_file(8, spec, vec![0, 255, 255, 0], 0);
    if let Some(layer) = file.layers.first_mut() {
        // Real flags (bit 4), real background, real rectangle 0,0,4,4.
        let mut tail = vec![0x10, 255];
        for v in [0_i32, 0, 4, 4] {
            tail.extend_from_slice(&v.to_be_bytes());
        }
        layer.mask_tail = Some(tail);
        layer.channels.push((-3, vec![255; 9]));
    }
    let document = doc(&file.write());
    assert_mask_matches_photoshop(&document, spec, &[0.0, 1.0, 1.0, 0.0]);
    assert!(report_text(&document).contains("combined version"));
}

#[test]
fn a_group_mask_is_applied_to_the_group_over_the_canvas() {
    let bytes = TestPsd::new(1, 4, 4, 8)
        .with(|p| {
            p.layers = vec![
                TestLayer::divider(),
                TestLayer::pixels("in", 0, 0, 2, 2, 8, &two_by_two(8)),
                TestLayer::group("G", *b"norm").with(|l| {
                    l.mask = Some((0, 0, 1, 2, 255, 0));
                    l.channels.push((-2, vec![0, 128]));
                }),
            ];
        })
        .write();
    let document = doc(&bytes);
    let group = root(&document.layers, 0);
    assert_eq!(
        document.layers.mask(group).map(|m| m.bounds),
        Some(rect(0, 0, 4, 4))
    );
    assert_eq!(effective_mask(&document, group, 0, 0), Some(0.0));
    assert_eq!(
        effective_mask(&document, group, 1, 0),
        Some(f(128.0 / 255.0))
    );
    assert_eq!(effective_mask(&document, group, 3, 3), Some(1.0));
    assert!(
        report_text(&document).is_empty(),
        "{}",
        report_text(&document)
    );
}

// -- Hostile masks ------------------------------------------------------

#[test]
fn an_empty_or_inverted_mask_rect_is_the_default_colour_everywhere() {
    for (spec, want) in [
        ((0, 0, 0, 0, 0, 0), 0.0),
        ((0, 0, 0, 0, 255, 0), 1.0),
        ((3, 3, 1, 1, 0, 0), 0.0),
        ((3, 3, 1, 1, 255, 0), 1.0),
    ] {
        let document = doc(&masked_file(8, spec, Vec::new(), 0).write());
        let id = root(&document.layers, 0);
        for (x, y) in [(1, 1), (2, 3), (3, 3)] {
            assert_eq!(effective_mask(&document, id, x, y), Some(want), "{spec:?}");
        }
        assert!(document.masks.is_empty());
    }
}

#[test]
fn a_mask_rect_past_the_document_range_is_reported_and_not_applied() {
    let far = i32::try_from(aurora_core::MAX_DOCUMENT_ORIGIN).unwrap_or(i32::MAX) + 10;
    let spec = (0, far, 2, far + 2, 0, 0);
    let document = doc(&masked_file(8, spec, vec![0; 4], 0).write());
    assert_eq!(document.layers.mask(root(&document.layers, 0)), None);
    assert!(report_text(&document).contains("could not be read"));
}

#[test]
fn a_huge_mask_rect_with_a_tiny_channel_is_refused_before_allocating() {
    // Under the pixel budget: the channel is far too short for the
    // rectangle, so the mask is dropped and reported without the
    // rectangle's buffer ever being allocated.
    let spec = (0, 0, 16_000, 16_000, 0, 0);
    for compression in 0..=3 {
        let started = std::time::Instant::now();
        let document = doc(&masked_file(8, spec, vec![0; 4], compression).write());
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        assert_eq!(document.layers.mask(root(&document.layers, 0)), None);
        assert!(report_text(&document).contains("could not be read"));
        assert_eq!(document.pixels.len(), 1, "the layer itself still opens");
    }
    // Over it: the whole file is refused by the budget, not decoded.
    let spec = (0, 0, 30_000, 30_000, 0, 0);
    assert!(matches!(
        read(&masked_file(8, spec, vec![0; 4], 0).write()),
        Err(IoError::PsdPixelBudget { .. })
    ));
}

#[test]
fn a_truncated_or_wrong_depth_mask_channel_is_reported_and_dropped() {
    // A 16-bit file's 2×2 mask stored with 8-bit samples (half the bytes).
    let spec = (1, 1, 3, 3, 0, 0);
    let document = doc(&masked_file(16, spec, vec![0, 64, 128, 255], 0).write());
    assert_eq!(document.layers.mask(root(&document.layers, 0)), None);
    assert!(report_text(&document).contains("could not be read"));
    // An RLE mask whose rows run out.
    let mut file = masked_file(8, spec, vec![1, 2, 3, 4], 1);
    if let Some((_, plane)) = file
        .layers
        .first_mut()
        .and_then(|l| l.channels.iter_mut().find(|(id, _)| *id == -2))
    {
        plane.truncate(1);
    }
    let document = doc(&file.write());
    assert_eq!(document.layers.mask(root(&document.layers, 0)), None);
    assert!(report_text(&document).contains("could not be read"));
}

// -- Grayscale -------------------------------------------------------------

#[test]
fn grayscale_layers_expand_to_rgb_at_both_depths_with_alpha() {
    for depth in [8_u16, 16] {
        let max: u16 = if depth == 16 { 0xFFFF } else { 255 };
        let samples = [[0, max], [max / 3, max / 2], [max, 0], [7, max]];
        for compression in 0..=3 {
            let bytes = TestPsd::new(1, 2, 2, depth)
                .with(|p| {
                    p.color_mode = 1;
                    p.channels = 1;
                    p.lr16 = depth == 16;
                    p.layers.push(
                        TestLayer::gray("g", 0, 0, 2, 2, depth, &samples)
                            .with(|l| l.compression = compression),
                    );
                })
                .write();
            let document = doc(&bytes);
            let Some(image) = image_of(&document, root(&document.layers, 0)) else {
                unreachable!("gray layer has pixels");
            };
            for (i, [v, a]) in samples.iter().enumerate() {
                let (x, y) = (i as u32 % 2, i as u32 / 2);
                let [gv, ga] = [*v, *a].map(|s| f(f32::from(s) / f32::from(max)));
                assert_eq!(
                    px(image, x, y),
                    [gv, gv, gv, ga],
                    "{depth}-bit {compression}"
                );
            }
            assert!(
                report_text(&document).is_empty(),
                "{}",
                report_text(&document)
            );
        }
    }
}

#[test]
fn a_grayscale_layer_with_a_mask_masks_like_rgb() {
    let bytes = TestPsd::new(1, 4, 4, 8)
        .with(|p| {
            p.color_mode = 1;
            p.channels = 1;
            p.layers.push(
                TestLayer::gray("g", 1, 1, 3, 3, 8, &[[90, 255]; 9]).with(|l| {
                    l.mask = Some((1, 1, 3, 3, 0, 0));
                    l.channels.push((-2, vec![0, 64, 128, 255]));
                }),
            );
        })
        .write();
    let document = doc(&bytes);
    let samples = [0.0, 64.0 / 255.0, 128.0 / 255.0, 1.0];
    assert_mask_matches_photoshop(&document, (1, 1, 3, 3, 0, 0), &samples);
}

#[test]
fn grayscale_ignores_colour_channels_one_and_two_and_its_merged_image_is_grey() {
    let bytes = TestPsd::new(1, 1, 1, 8)
        .with(|p| {
            p.color_mode = 1;
            p.channels = 1;
            p.layers.push(
                TestLayer::gray("g", 0, 0, 1, 1, 8, &[[40, 255]])
                    .with(|l| l.channels.push((1, vec![99]))),
            );
        })
        .write();
    let document = doc(&bytes);
    let Some(image) = image_of(&document, root(&document.layers, 0)) else {
        unreachable!("pixels");
    };
    assert_eq!(px(image, 0, 0), u8px(40, 40, 40, 255));
    assert!(report_text(&document).contains("extra layer channel"));

    // No layers: the merged grey plane, plus one more saved channel.
    for compression in 0..=3 {
        let flat = TestPsd::new(1, 2, 1, 8)
            .with(|p| {
                p.color_mode = 1;
                p.channels = 2;
                p.merged = Some(vec![vec![10, 250], vec![1, 2]]);
                p.merged_compression = compression;
            })
            .write();
        let document = doc(&flat);
        let Some(image) = document.pixels.first().map(|p| &p.image) else {
            unreachable!("background");
        };
        assert_eq!(px(image, 0, 0), u8px(10, 10, 10, 255));
        assert_eq!(px(image, 1, 0), u8px(250, 250, 250, 255));
        assert!(report_text(&document).contains("1 saved channel"));
    }
}

#[test]
fn bitmap_indexed_duotone_and_friends_stay_refused_by_name() {
    for mode in [0_u16, 2, 4, 7, 8, 9] {
        let bytes = TestPsd::new(1, 1, 1, 8)
            .with(|p| p.color_mode = mode)
            .write();
        assert!(
            matches!(decode(&bytes), Err(IoError::UnsupportedPsdColorMode(m)) if m == mode),
            "mode {mode}"
        );
    }
}

/// psd-tools 1.17.4 reads this file as a 4×4 Grayscale "Gradient Fill 1"
/// layer over an empty "Layer 1", with an empty, default-255 mask.
#[test]
fn real_grayscale_fixture_matches_psd_tools() {
    let document = doc(fixture!("4x4_8bit_grayscale.psd"));
    let tree = &document.layers;
    assert_eq!(names(tree, tree.roots()), ["Gradient Fill 1", "Layer 1"]);
    let fill = root(tree, 0);
    let Some(image) = image_of(&document, fill) else {
        unreachable!("the fill has pixels");
    };
    let grey = [
        [24, 50, 95, 172],
        [50, 23, 50, 94],
        [95, 51, 24, 50],
        [173, 95, 50, 24],
    ];
    for (y, row) in grey.iter().enumerate() {
        for (x, v) in row.iter().enumerate() {
            assert_eq!(
                px(image, x as u32, y as u32),
                u8px(*v, *v, *v, 255),
                "({x}, {y})"
            );
        }
    }
    // The empty default-255 mask shows everything.
    assert_eq!(effective_mask(&document, fill, 3, 3), Some(1.0));
    // The 16-bit sibling opens too. Its fill layer stores no pixels in
    // `Lr16` (psd-tools' `numpy()` is `None` for it as well), so it is
    // left out and reported, like any pixel-less fill.
    let sixteen = doc(fixture!("4x4_16bit_grayscale.psd"));
    assert_eq!(names(&sixteen.layers, sixteen.layers.roots()), ["Layer 1"]);
    assert!(report_text(&sixteen).contains("no stored pixels"));
}

/// Open time for a document-sized mask (0.147.0): decode, build and
/// write the coverage tile by tile into a real store. 512² by default;
/// `AURORA_PSD_MASK_BENCH=4096` (any side) measures that size instead.
/// Prints the timings; the assertion is only a loose sanity bound.
#[test]
fn a_document_sized_mask_opens_and_writes_through_the_tile_store() {
    let side: u32 = std::env::var("AURORA_PSD_MASK_BENCH")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(512);
    let n = (side * side) as usize;
    let s = i32::try_from(side).unwrap_or(512);
    let bytes = TestPsd::new(1, side, side, 8)
        .with(|p| {
            p.layers
                .push(TestLayer::pixels("big", 0, 0, s, s, 8, &[]).with(|l| {
                    l.channels = vec![
                        (-1, vec![255; n]),
                        (0, vec![10; n]),
                        (1, vec![20; n]),
                        (2, vec![30; n]),
                        (-2, (0..n).map(|i| (i % 251) as u8).collect()),
                    ];
                    l.mask = Some((0, 0, s, s, 0, 0));
                    l.compression = 2;
                }));
        })
        .write();
    let dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(err) => unreachable!("{err:?}"),
    };
    let Some(budget) = std::num::NonZeroUsize::new(1024) else {
        unreachable!("non-zero");
    };
    let mut store = match aurora_tile::TileStore::new(dir.path().to_path_buf(), budget) {
        Ok(store) => store,
        Err(err) => unreachable!("{err:?}"),
    };
    let started = std::time::Instant::now();
    let document = doc(&bytes);
    let decoded = started.elapsed();
    let [mask] = document.masks.as_slice() else {
        unreachable!("one mask");
    };
    let written = std::time::Instant::now();
    ok(super::write_mask_pixels(mask, &document.layers, &mut store));
    let write = written.elapsed();
    println!(
        "{side}x{side} mask: read+build {decoded:?}, coverage write {write:?} ({} file bytes)",
        bytes.len()
    );
    // Spot-check the far corner through the store.
    let id = root(&document.layers, 0);
    let Some(surface) = document.layers.mask_surface_id(id) else {
        unreachable!("mask surface");
    };
    let last = side - 1;
    let tile = ok(store
        .get(
            surface,
            aurora_tile::TileId {
                x: last / aurora_tile::TILE,
                y: last / aurora_tile::TILE,
            },
        )
        .map_err(IoError::from));
    let local = (last % aurora_tile::TILE) as usize;
    let at = (local * aurora_tile::TILE as usize + local) * 4;
    let expected = f(((n - 1) % 251) as f32 / 255.0);
    assert_eq!(tile.texels().get(at).map(|v| v.to_f32()), Some(expected));
    assert!(started.elapsed() < std::time::Duration::from_mins(2));
}

/// A Grayscale file whose only layer is left out falls back to its
/// merged image, and with merged transparency (a negative layer count)
/// the second plane is that image's alpha, not a colour.
#[test]
fn a_grayscale_merged_fallback_reads_its_alpha_plane_as_alpha() {
    let bytes = TestPsd::new(1, 2, 1, 8)
        .with(|p| {
            p.color_mode = 1;
            p.channels = 2;
            p.negative_count = true;
            p.layers
                .push(TestLayer::empty("adj").with(|l| l.blocks.push((*b"levl", vec![0; 4]))));
            p.merged = Some(vec![vec![60, 255], vec![255, 0]]);
        })
        .write();
    let document = doc(&bytes);
    let Some(image) = document.pixels.first().map(|p| &p.image) else {
        unreachable!("background");
    };
    assert_eq!(px(image, 0, 0), u8px(60, 60, 60, 255));
    assert_eq!(px(image, 1, 0)[3], 0.0);
    assert!(!report_text(&document).contains("saved channel"));
}

// ---------------------------------------------------------------------
// Vector masks (0.150.0)
// ---------------------------------------------------------------------

/// The header and layer records of `bytes`, read the way [`decode`]
/// reads them (layer-info section, else an `Lr16`/`Layr` block).
/// The streaming source [`decode`] runs over, for `bytes` (0.154.0).
fn source(bytes: &[u8]) -> super::StreamSource<std::io::Cursor<&[u8]>> {
    ok(super::StreamSource::new(std::io::Cursor::new(bytes)))
}

fn records_of(bytes: &[u8]) -> (super::Header, Vec<super::Record>) {
    let mut src = ok(super::StreamSource::new(std::io::Cursor::new(bytes)));
    let mut r = super::Reader::new(bytes);
    let header = ok(super::read_header(&mut r));
    let psb = header.psb();
    let len = ok(r.length(false, "colour mode data"));
    ok(r.skip(len, "colour mode data"));
    let len = ok(r.length(false, "resources"));
    ok(r.skip(len, "resources"));
    let len = ok(r.length(psb, "layer and mask section"));
    let mut section = super::Window {
        pos: r.pos as u64,
        end: (r.pos + len) as u64,
    };
    let info_len = ok(section.length(&mut src, psb, "layer info"));
    let mut body = ok(section.sub(info_len, "layer info"));
    let mut records = ok(super::read_layer_info(&mut src, &mut body, header)).records;
    if records.is_empty() && section.remaining() >= 4 {
        let mask_len = ok(section.length(&mut src, false, "global mask"));
        ok(section.advance(mask_len, "global mask"));
        for (key, mut data) in ok(super::read_block_windows(&mut src, &mut section, psb, 4)) {
            if (&key == b"Lr16" || &key == b"Layr") && records.is_empty() {
                records = ok(super::read_layer_info(&mut src, &mut data, header)).records;
            }
        }
    }
    (header, records)
}

/// Aurora's raster of the named layer's vector mask over the whole
/// canvas (outside the raster rectangle: its constant), row-major.
#[allow(clippy::many_single_char_names, clippy::cast_sign_loss)] // pixel loops
fn vector_plane(bytes: &[u8], name: &str) -> Option<(u32, u32, Vec<f32>)> {
    let (header, records) = records_of(bytes);
    let record = records.iter().find(|r| r.name == name)?;
    let path = super::vector::parse(record.vector.as_deref()?).ok()?;
    let mut budget = unlimited();
    let raster = super::vector::rasterize(&path, header.width, header.height, &mut budget).ok()?;
    let (w, h) = (header.width, header.height);
    let outside = if raster.outside { 1.0 } else { 0.0 };
    let mut plane = vec![outside; (w as usize) * (h as usize)];
    let b = raster.bounds;
    for (i, v) in raster.coverage.iter().enumerate() {
        let x = b.x as usize + i % b.width as usize;
        let y = b.y as usize + i / b.width as usize;
        if let Some(slot) = plane.get_mut(y * w as usize + x) {
            *slot = v.to_f32();
        }
    }
    Some((w, h, plane))
}

// -- Synthetic vector masks ---------------------------------------------

/// A `vmsk` block: version 3, `flags`, then the records.
fn vmsk(flags: u32, records: &[[u8; 26]]) -> Vec<u8> {
    let mut out = 3_u32.to_be_bytes().to_vec();
    out.extend_from_slice(&flags.to_be_bytes());
    for record in records {
        out.extend_from_slice(record);
    }
    out
}

fn record(selector: u16, body: &[u8]) -> [u8; 26] {
    let mut out = [0_u8; 26];
    let bytes = selector.to_be_bytes();
    for (slot, v) in out.iter_mut().zip(bytes.iter().chain(body)) {
        *slot = *v;
    }
    out
}

/// A subpath-length record: closed (`0`) or open (`3`), `knots`, `op`.
fn subpath(closed: bool, knots: u16, op: i16) -> [u8; 26] {
    let mut body = knots.to_be_bytes().to_vec();
    body.extend_from_slice(&op.to_be_bytes());
    record(if closed { 0 } else { 3 }, &body)
}

fn fixed(v: f64) -> [u8; 4] {
    ((v * f64::from(1_u32 << 24)).round() as i32).to_be_bytes()
}

/// A knot from `(x, y)` points, normalised; written vertical first.
fn knot(closed: bool, pre: (f64, f64), anchor: (f64, f64), leave: (f64, f64)) -> [u8; 26] {
    let mut body = Vec::new();
    for (x, y) in [pre, anchor, leave] {
        body.extend_from_slice(&fixed(y));
        body.extend_from_slice(&fixed(x));
    }
    record(if closed { 1 } else { 4 }, &body)
}

fn corner(x: f64, y: f64) -> [u8; 26] {
    knot(true, (x, y), (x, y), (x, y))
}

/// A closed axis-aligned rectangle subpath, normalised.
fn rect_path(x0: f64, y0: f64, x1: f64, y1: f64, op: i16) -> Vec<[u8; 26]> {
    vec![
        subpath(true, 4, op),
        corner(x0, y0),
        corner(x1, y0),
        corner(x1, y1),
        corner(x0, y1),
    ]
}

/// A closed four-knot Bézier ellipse (the usual `0.5523` handles).
fn ellipse_path(cx: f64, cy: f64, rx: f64, ry: f64, op: i16) -> Vec<[u8; 26]> {
    let k = 0.552_284_749_8;
    vec![
        subpath(true, 4, op),
        knot(
            true,
            (cx - k * rx, cy - ry),
            (cx, cy - ry),
            (cx + k * rx, cy - ry),
        ),
        knot(
            true,
            (cx + rx, cy - k * ry),
            (cx + rx, cy),
            (cx + rx, cy + k * ry),
        ),
        knot(
            true,
            (cx + k * rx, cy + ry),
            (cx, cy + ry),
            (cx - k * rx, cy + ry),
        ),
        knot(
            true,
            (cx - rx, cy + k * ry),
            (cx - rx, cy),
            (cx - rx, cy - k * ry),
        ),
    ]
}

fn initial_fill(value: u16) -> [u8; 26] {
    record(8, &value.to_be_bytes())
}

/// One opaque full-canvas pixel layer carrying `block` as its `vmsk`.
#[allow(clippy::cast_possible_wrap)] // tiny test canvases
fn vector_file(width: u32, height: u32, block: Vec<u8>) -> TestPsd {
    let (w, h) = (width as i32, height as i32);
    let n = (width * height) as usize;
    TestPsd::new(1, width, height, 8).with(|p| {
        p.layers.push(
            TestLayer::pixels("v", 0, 0, w, h, 8, &vec![[200, 100, 50, 255]; n])
                .with(|l| l.blocks.push((*b"vmsk", block))),
        );
    })
}

fn vector_doc(width: u32, height: u32, records: &[[u8; 26]]) -> PsdDocument {
    doc(&vector_file(width, height, vmsk(0, records)).write())
}

/// The only layer's effective mask (`None` when it has no mask), with
/// its density applied.
fn vmask(document: &PsdDocument, x: i64, y: i64) -> Option<f32> {
    let id = root(&document.layers, 0);
    let density = document.layers.mask(id)?.density;
    effective_mask(document, id, x, y).map(|c| with_density(c, density))
}

fn mask_sum(document: &PsdDocument, width: i64, height: i64) -> f64 {
    let mut sum = 0.0;
    for y in 0..height {
        for x in 0..width {
            sum += f64::from(vmask(document, x, y).unwrap_or(f32::NAN));
        }
    }
    sum
}

#[test]
fn a_rectangle_vector_mask_becomes_the_layers_mask_and_is_reported_as_converted() {
    // x 2.5..6, y 2..6 on 8×8: a half-covered left column.
    let document = vector_doc(8, 8, &rect_path(0.3125, 0.25, 0.75, 0.75, 1));
    for (x, y, want) in [
        (0, 0, 0.0),
        (1, 3, 0.0),
        (2, 3, 0.5),
        (3, 2, 1.0),
        (5, 5, 1.0),
        (6, 5, 0.0),
        (4, 6, 0.0),
        (7, 7, 0.0),
    ] {
        assert_eq!(vmask(&document, x, y), Some(want), "({x}, {y})");
    }
    let text = report_text(&document);
    assert!(
        text.contains("1 vector mask was converted to a pixel mask"),
        "{text}"
    );
    assert!(!text.contains("not applied"), "{text}");
}

#[test]
fn x_is_scaled_by_the_width_and_y_by_the_height() {
    // A 16×8 canvas, rectangle x 0..0.25 (4 px), y 0..0.75 (6 px).
    let document = vector_doc(16, 8, &rect_path(0.0, 0.0, 0.25, 0.75, 1));
    assert_eq!(vmask(&document, 3, 5), Some(1.0));
    assert_eq!(vmask(&document, 4, 5), Some(0.0));
    assert_eq!(vmask(&document, 3, 6), Some(0.0));
    assert_eq!(mask_sum(&document, 16, 8), 24.0);
}

#[test]
fn a_bezier_ellipse_is_filled_as_a_curve_not_as_its_knot_polygon() {
    // r = 16 px on 64×64: area π r² ≈ 804.25 (the four-knot Bézier is
    // within 0.03% of the circle); the knot diamond would be 2 r² = 512.
    let document = vector_doc(64, 64, &ellipse_path(0.5, 0.5, 0.25, 0.25, 1));
    // Chords lie inside the curve: at most `(2/3) × FLATTEN_TOLERANCE ×
    // perimeter` ≈ 3.4 px² is lost to flattening.
    let sum = mask_sum(&document, 64, 64);
    let area = std::f64::consts::PI * 256.0;
    assert!((sum - area).abs() < 4.0, "{sum} vs {area}");
    assert_eq!(vmask(&document, 32, 32), Some(1.0));
    // On the 45° diagonal, just inside the circle but outside the
    // diamond: (32 + 10.5, 32 + 10.5) is 14.85 px from the centre.
    assert_eq!(vmask(&document, 42, 42), Some(1.0));
    assert_eq!(vmask(&document, 0, 0), Some(0.0));
    // Anti-aliased: some edge pixel is partial.
    let partial = (0..64).any(|x| vmask(&document, x, 20).is_some_and(|v| v > 0.0 && v < 1.0));
    assert!(partial);
}

#[test]
fn an_inverted_vector_mask_shows_outside_and_hides_inside() {
    let file = vector_file(8, 8, vmsk(1, &rect_path(0.25, 0.25, 0.75, 0.75, 1)));
    let document = doc(&file.write());
    assert_eq!(vmask(&document, 3, 3), Some(0.0));
    assert_eq!(vmask(&document, 0, 0), Some(1.0));
    assert_eq!(vmask(&document, 7, 7), Some(1.0));
    let id = root(&document.layers, 0);
    assert_eq!(
        document.layers.mask(id).map(|m| m.bounds),
        Some(rect(0, 0, 8, 8))
    );
}

#[test]
fn a_disabled_vector_mask_is_ignored_and_reported_as_left_out() {
    let file = vector_file(8, 8, vmsk(4, &rect_path(0.25, 0.25, 0.75, 0.75, 1)));
    let document = doc(&file.write());
    let id = root(&document.layers, 0);
    assert!(document.layers.mask(id).is_none());
    let text = report_text(&document);
    assert!(
        text.contains("1 turned-off vector mask was left out"),
        "{text}"
    );
    assert!(!text.contains("converted"), "{text}");
}

#[test]
fn subpath_operations_combine_as_psd_tools_combines_them() {
    // A = x 0..6 (op 1); B = x 2..8 with `op`; probes at x = 1 (A only),
    // 3 (both) and 7 (B only) on 8×8.
    for (op, want) in [
        (0_i16, [1.0, 0.0, 1.0]),
        (1, [1.0, 1.0, 1.0]),
        (2, [1.0, 0.0, 0.0]),
        (3, [0.0, 1.0, 0.0]),
        (-1, [1.0, 1.0, 1.0]),
        (7, [1.0, 1.0, 0.0]),
    ] {
        let mut records = rect_path(0.0, 0.0, 0.75, 1.0, 1);
        records.extend(rect_path(0.25, 0.0, 1.0, 1.0, op));
        let document = vector_doc(8, 8, &records);
        let got = [1, 3, 7].map(|x| vmask(&document, x, 4).unwrap_or(f32::NAN));
        assert_eq!(got, want, "op {op}");
    }
}

#[test]
fn a_first_subtract_or_intersect_starts_from_everything() {
    // Subtract first: everything minus the shape. Intersect first:
    // everything ∩ the shape = the shape.
    for (op, inside, outside) in [(2_i16, 0.0, 1.0), (3, 1.0, 0.0), (0, 1.0, 0.0)] {
        let document = vector_doc(8, 8, &rect_path(0.25, 0.25, 0.75, 0.75, op));
        assert_eq!(vmask(&document, 3, 3), Some(inside), "op {op}");
        assert_eq!(vmask(&document, 0, 0), Some(outside), "op {op}");
        assert_eq!(vmask(&document, 7, 7), Some(outside), "op {op}");
    }
}

#[test]
fn the_initial_fill_shows_everything_only_without_subpaths() {
    let all = vector_doc(4, 4, &[initial_fill(1)]);
    assert_eq!(mask_sum(&all, 4, 4), 16.0);
    let none = vector_doc(4, 4, &[initial_fill(0)]);
    assert_eq!(mask_sum(&none, 4, 4), 0.0);
    // psd-tools' rule: with a subpath the initial fill is not used.
    let mut records = vec![initial_fill(1)];
    records.extend(rect_path(0.0, 0.0, 0.5, 0.5, 1));
    let shape = vector_doc(4, 4, &records);
    assert_eq!(mask_sum(&shape, 4, 4), 4.0);
}

#[test]
fn a_self_overlapping_subpath_is_filled_with_the_nonzero_rule() {
    // The same rectangle traced twice in one subpath: winding 2 inside,
    // which the non-zero rule fills and even-odd would leave empty.
    let mut records = vec![subpath(true, 8, 1)];
    for _ in 0..2 {
        records.extend([
            corner(0.25, 0.25),
            corner(0.75, 0.25),
            corner(0.75, 0.75),
            corner(0.25, 0.75),
        ]);
    }
    let document = vector_doc(8, 8, &records);
    assert_eq!(vmask(&document, 3, 3), Some(1.0));
    assert_eq!(mask_sum(&document, 8, 8), 16.0);
}

#[test]
fn an_open_subpath_is_filled_as_if_closed_by_a_straight_line() {
    // Three corners of the 8×8 square, open: the triangle below the
    // diagonal (0,0)-(8,8) — area 32.
    let records = vec![
        subpath(false, 3, 1),
        knot(false, (0.0, 0.0), (0.0, 0.0), (0.0, 0.0)),
        knot(false, (1.0, 1.0), (1.0, 1.0), (1.0, 1.0)),
        knot(false, (0.0, 1.0), (0.0, 1.0), (0.0, 1.0)),
    ];
    let document = vector_doc(8, 8, &records);
    assert!((mask_sum(&document, 8, 8) - 32.0).abs() < 0.1);
    assert_eq!(vmask(&document, 1, 6), Some(1.0));
    assert_eq!(vmask(&document, 6, 1), Some(0.0));
}

#[test]
fn a_vector_density_is_the_masks_density_when_there_is_no_pixel_mask() {
    let mut file = vector_file(8, 8, vmsk(0, &rect_path(0.25, 0.25, 0.75, 0.75, 1)));
    if let Some(layer) = file.layers.first_mut() {
        // Mask data with no `-2` channel: flags bit 4, a parameter block
        // naming only a vector density of 128.
        layer.mask = Some((0, 0, 0, 0, 0, 0x10));
        layer.mask_tail = Some(vec![0x04, 128]);
    }
    let document = doc(&file.write());
    assert_eq!(only_density(&document), Some(128.0 / 255.0));
    assert_eq!(vmask(&document, 3, 3), Some(1.0));
    assert_eq!(
        vmask(&document, 0, 0),
        Some(with_density(0.0, 128.0 / 255.0))
    );
}

#[test]
fn a_pixel_mask_and_a_vector_mask_intersect() {
    // Pixel mask: the left half (x 0..4) shown, hidden elsewhere;
    // vector: x 2..6, y 2..6.
    let mut file = vector_file(8, 8, vmsk(0, &rect_path(0.25, 0.25, 0.75, 0.75, 1)));
    if let Some(layer) = file.layers.first_mut() {
        layer.mask = Some((0, 0, 8, 4, 0, 0));
        layer.channels.push((-2, vec![255; 32]));
    }
    let document = doc(&file.write());
    for (x, y, want) in [
        (3, 3, 1.0),
        (5, 3, 0.0),
        (1, 1, 0.0),
        (2, 5, 1.0),
        (6, 6, 0.0),
    ] {
        assert_eq!(vmask(&document, x, y), Some(want), "({x}, {y})");
    }
    let text = report_text(&document);
    assert!(!text.contains("combined version"), "{text}");
}

#[test]
fn a_rendered_user_mask_is_replaced_by_the_real_mask_times_the_vector() {
    // Flags bit 3: `-2` is Photoshop's rendering (here, a deliberately
    // wrong all-shown plane), the real user mask `-3` (x 0..4, real
    // default hidden) is the pixel part, with densities 128 (user) and
    // 64 (vector) each baked into its own part.
    let mut file = vector_file(8, 8, vmsk(0, &rect_path(0.25, 0.25, 0.75, 0.75, 1)));
    if let Some(layer) = file.layers.first_mut() {
        layer.mask = Some((0, 0, 8, 8, 0, 0x18));
        let mut tail = vec![0x00, 0];
        for v in [0_i32, 0, 8, 4] {
            tail.extend_from_slice(&v.to_be_bytes());
        }
        tail.extend_from_slice(&[0x05, 128, 64]);
        layer.mask_tail = Some(tail);
        layer.channels.push((-2, vec![255; 64]));
        layer.channels.push((-3, vec![255; 32]));
    }
    let document = doc(&file.write());
    let (du, dv) = (128.0 / 255.0, 64.0 / 255.0);
    let both = |u: f32, v: f32| f16::from_f32(with_density(u, du) * with_density(v, dv)).to_f32();
    assert_eq!(only_density(&document), Some(1.0));
    for (x, y, u, v) in [
        (3, 3, 1.0, 1.0),
        (5, 3, 0.0, 1.0),
        (1, 1, 1.0, 0.0),
        (7, 7, 0.0, 0.0),
    ] {
        assert_eq!(vmask(&document, x, y), Some(both(u, v)), "({x}, {y})");
    }
    let text = report_text(&document);
    assert!(!text.contains("combined version"), "{text}");
    assert!(text.contains("converted"), "{text}");
}

#[test]
fn a_group_vector_mask_is_applied_to_the_group() {
    let bytes = TestPsd::new(1, 8, 8, 8)
        .with(|p| {
            p.layers = vec![
                TestLayer::divider(),
                TestLayer::pixels("in", 0, 0, 2, 2, 8, &two_by_two(8)),
                TestLayer::group("G", *b"norm").with(|l| {
                    l.blocks
                        .push((*b"vsms", vmsk(0, &rect_path(0.0, 0.0, 0.5, 1.0, 1))));
                }),
            ];
        })
        .write();
    let document = doc(&bytes);
    let group = root(&document.layers, 0);
    assert_eq!(effective_mask(&document, group, 3, 7), Some(1.0));
    assert_eq!(effective_mask(&document, group, 4, 0), Some(0.0));
}

#[test]
fn a_shape_layers_vector_mask_is_not_applied_again() {
    // A fill + vector mask is a shape layer: its pixels are the rendered
    // shape, so the vector mask is neither applied nor reported.
    let mut file = vector_file(8, 8, vmsk(0, &rect_path(0.25, 0.25, 0.75, 0.75, 1)));
    if let Some(layer) = file.layers.first_mut() {
        layer.blocks.push((*b"SoCo", vec![0; 4]));
    }
    let document = doc(&file.write());
    assert!(document.layers.mask(root(&document.layers, 0)).is_none());
    let text = report_text(&document);
    assert!(text.contains("shape layer opened as pixels"), "{text}");
    assert!(!text.contains("vector mask"), "{text}");
}

#[test]
fn a_malformed_vector_mask_is_reported_and_the_file_still_opens_unmasked() {
    let rect = rect_path(0.25, 0.25, 0.75, 0.75, 1);
    let mut bad_version = vmsk(0, &rect);
    if let Some(v) = bad_version.get_mut(3) {
        *v = 2;
    }
    let mut short_subpath = vec![subpath(true, 5, 1)];
    short_subpath.extend(rect.iter().skip(1).copied());
    let mut unknown = rect.clone();
    unknown.push(record(9, &[]));
    let mut cases = vec![
        ("version", bad_version),
        ("short subpath", vmsk(0, &short_subpath)),
        ("unknown record", vmsk(0, &unknown)),
        ("no flags", vec![0, 0, 0, 3]),
        ("empty", Vec::new()),
    ];
    // Truncated mid-record: whole records only, so this one *parses*
    // (the last knot is gone, so the subpath is short: unreadable).
    let mut cut = vmsk(0, &rect);
    cut.truncate(cut.len() - 10);
    cases.push(("truncated", cut));
    for (what, block) in cases {
        let document = doc(&vector_file(8, 8, block).write());
        assert!(
            document.layers.mask(root(&document.layers, 0)).is_none(),
            "{what}"
        );
        let text = report_text(&document);
        assert!(
            text.contains("vector mask could not be read"),
            "{what}: {text}"
        );
    }
}

#[test]
fn too_many_records_or_segments_are_bounded_and_reported() {
    // One record past the cap.
    let over = vec![record(6, &[]); super::vector::MAX_VECTOR_RECORDS + 1];
    let document = doc(&vector_file(4, 4, vmsk(0, &over)).write());
    assert!(report_text(&document).contains("could not be read"));
    // 65,535 knots whose control points fling far off the canvas: each
    // cubic would want the full step cap, so the total passes
    // `MAX_FLATTENED_SEGMENTS` and the mask is refused before any
    // flattening allocation of that size.
    let mut records = vec![subpath(true, u16::MAX, 1)];
    for i in 0..u16::MAX {
        let x = f64::from(i % 2);
        records.push(knot(true, (x, 100.0), (x, 0.5), (x, -100.0)));
    }
    let started = std::time::Instant::now();
    let document = doc(&vector_file(64, 64, vmsk(0, &records)).write());
    assert!(started.elapsed().as_secs() < 20);
    let text = report_text(&document);
    assert!(text.contains("too large or complex"), "{text}");
    assert!(document.layers.mask(root(&document.layers, 0)).is_none());
    // The segment cap on its own: 8,000 knots wholly above the canvas
    // (so the raster is empty and no work bound applies), each cubic
    // wanting the full 256 steps — 2 Mi segments, past the cap.
    let mut records = vec![subpath(true, 8000, 1)];
    for i in 0..8000_u32 {
        let x = f64::from(i % 2);
        records.push(knot(true, (x, -10.0), (x, -50.0), (1.0 - x, -90.0)));
    }
    let path = ok_vector(&vmsk(0, &records));
    assert_eq!(
        super::vector::rasterize(&path, 64, 64, &mut unlimited()),
        Err(super::vector::VectorFailure::TooLarge)
    );
}

fn unlimited() -> super::vector::Budget {
    super::vector::Budget {
        pixels: u64::MAX,
        work: u64::MAX,
    }
}

#[test]
fn the_raster_is_charged_against_the_budget_and_refused_past_it() {
    use super::vector::{Budget, RASTER_PIXEL_CHARGE, VectorFailure, rasterize};
    let path = ok_vector(&vmsk(0, &rect_path(0.0, 0.0, 1.0, 1.0, 1)));
    // The bounding box grown by a pixel, clipped to 100×100, at two
    // budget pixels per raster pixel.
    let charge = 10_000 * RASTER_PIXEL_CHARGE;
    let mut budget = Budget {
        pixels: charge,
        work: u64::MAX,
    };
    let raster = rasterize(&path, 100, 100, &mut budget);
    assert_eq!(raster.map(|r| r.charged), Ok(charge));
    assert_eq!(budget.pixels, 0);
    let spent = u64::MAX - budget.work;
    assert!(spent > 0);
    for short in [
        Budget {
            pixels: charge - 1,
            work: u64::MAX,
        },
        Budget {
            pixels: u64::MAX,
            work: spent - 1,
        },
    ] {
        let mut budget = short;
        assert_eq!(
            rasterize(&path, 100, 100, &mut budget),
            Err(VectorFailure::TooLarge)
        );
        // Only the flattening charge (1 + 4 corner-to-corner segments)
        // is kept: flattening was done.
        assert_eq!(budget.pixels, short.pixels, "pixels untouched");
        assert_eq!(
            short.work - budget.work,
            5 * super::vector::SEGMENT_CHARGE,
            "flattening only"
        );
    }
    // Scanline work past `MAX_RASTER_WORK` (a full-canvas box on a
    // 20,000 × 20,000 canvas) is refused before any raster exists.
    let mut budget = unlimited();
    assert_eq!(
        rasterize(&path, 20_000, 20_000, &mut budget),
        Err(VectorFailure::TooLarge)
    );
}

#[test]
fn one_files_vector_masks_share_one_work_budget() {
    use super::vector::{Budget, VectorFailure, rasterize};
    let path = ok_vector(&vmsk(0, &ellipse_path(0.5, 0.5, 0.4, 0.3, 1)));
    let mut probe = unlimited();
    assert!(rasterize(&path, 64, 64, &mut probe).is_ok());
    let one = u64::MAX - probe.work;
    // Enough for one mask and not two: the second is refused.
    let mut budget = Budget {
        pixels: u64::MAX,
        work: one * 2 - 1,
    };
    assert!(rasterize(&path, 64, 64, &mut budget).is_ok());
    assert_eq!(budget.work, one - 1);
    assert_eq!(
        rasterize(&path, 64, 64, &mut budget),
        Err(VectorFailure::TooLarge)
    );
    assert!(budget.work < one - 1, "the flattening charge is kept");
}

/// `work_for`'s count, by hand: the square x, y 2..6 on 8×8 (exact in
/// 8.24 fixed point), one group.
#[test]
fn the_work_estimate_counts_every_pass_the_fill_makes() {
    let path = ok_vector(&vmsk(0, &rect_path(0.25, 0.25, 0.75, 0.75, 1)));
    let mut budget = unlimited();
    assert!(super::vector::rasterize(&path, 8, 8, &mut budget).is_ok());
    // Raster: the box grown by a pixel, 1..7 → 6×6. Corner-to-corner
    // edges are one segment each; the two vertical ones cross local rows
    // 1..5: 4 × 16 + 1 sub-scanlines each.
    let len = 36;
    let crossings = 2 * (4 * 16 + 1);
    let log = 2; // 64 - leading_zeros(2 edges)
    let (box_w, box_h) = (4 + 2, 4 + 1);
    let segments = (1 + 4) * super::vector::SEGMENT_CHARGE;
    let want =
        segments + 3 * len + 3 * len + crossings * (1 + log) + box_h * 16 + 2 * box_w * box_h;
    assert_eq!(u64::MAX - budget.work, want);
}

#[test]
#[allow(clippy::cast_sign_loss, clippy::cast_possible_wrap)]
fn a_flood_of_empty_subpaths_costs_no_per_pixel_pass() {
    // The review's attack: one canvas rectangle, then 65,530 zero-knot
    // subpath records — each its own group — on a 2,048² canvas. Each
    // group is folded into a constant instead of a whole-raster pass
    // (which would be ~2.7e11 operations).
    let mut records = rect_path(0.25, 0.25, 0.75, 0.75, 1);
    for i in 0..65_530_u32 {
        records.push(subpath(true, 0, (i % 3) as i16));
    }
    let started = std::time::Instant::now();
    let path = ok_vector(&vmsk(0, &records));
    let mut budget = unlimited();
    let raster = super::vector::rasterize(&path, 2048, 2048, &mut budget);
    let elapsed = started.elapsed();
    let Ok(raster) = raster else {
        unreachable!("{raster:?}")
    };
    assert!(elapsed.as_secs_f64() < 5.0, "{elapsed:?}");
    // Exclude, combine and a later subtract with an empty plane are all
    // the identity: still just the rectangle.
    let at = |x: usize, y: usize| {
        let b = raster.bounds;
        let i = (y - b.y as usize) * b.width as usize + (x - b.x as usize);
        raster.coverage.get(i).map(|v| v.to_f32())
    };
    assert_eq!(at(1024, 1024), Some(1.0));
    assert_eq!(at(511, 1024), Some(0.0));
    // An empty intersect after a drawn group clears everything.
    let mut records = rect_path(0.25, 0.25, 0.75, 0.75, 1);
    records.push(subpath(true, 0, 3));
    let document = vector_doc(8, 8, &records);
    assert_eq!(mask_sum(&document, 8, 8), 0.0);
    // And an empty first subtract starts from everything.
    let mut records = vec![subpath(true, 0, 2)];
    records.extend(rect_path(0.25, 0.25, 0.75, 0.75, 2));
    let document = vector_doc(8, 8, &records);
    assert_eq!(vmask(&document, 3, 3), Some(0.0));
    assert_eq!(vmask(&document, 0, 0), Some(1.0));
}

#[test]
fn many_drawn_groups_are_refused_by_their_whole_raster_passes() {
    // 200 one-pixel squares, each its own group, two of them at opposite
    // corners so the raster is the whole 1,024² canvas: 200 groups × 3
    // passes × 1 Mi px is past `MAX_RASTER_WORK`.
    let mut records = Vec::new();
    for i in 0..200_u32 {
        let x = f64::from(i % 2) * 0.999;
        let y = f64::from(i) / 200.0;
        records.extend(rect_path(x, y, x + 0.001, y + 0.001, 1));
    }
    let path = ok_vector(&vmsk(0, &records));
    let mut budget = unlimited();
    assert_eq!(
        super::vector::rasterize(&path, 1024, 1024, &mut budget),
        Err(super::vector::VectorFailure::TooLarge)
    );
}

/// The worst case at the caps (G2, 0.150.0 review): a 4,096-knot zigzag
/// whose every edge spans the full height of a 4,096 × 256 canvas — one
/// sub-scanline sorts 4,096 crossings, 4,096 sub-scanlines — just under
/// `MAX_RASTER_WORK`. Its time is printed; the assertion is generous
/// (debug builds are ~20× slower than release).
#[test]
fn the_worst_case_at_the_work_cap_finishes_in_bounded_time() {
    // In order (a zigzag), and shuffled — knots at a multiplicative
    // permutation of the columns, so every sub-scanline's crossings
    // arrive out of order and every sort does real work.
    let n = 4096_u16;
    for shuffled in [false, true] {
        let mut records = vec![subpath(true, n, 1)];
        for i in 0..n {
            let column = if shuffled {
                (u64::from(i) * 2_654_435_761 % u64::from(n)) as u16
            } else {
                i
            };
            let x = f64::from(column) / f64::from(n);
            let y = f64::from(i % 2);
            records.push(corner(x, y));
        }
        let path = ok_vector(&vmsk(0, &records));
        let mut budget = unlimited();
        let started = std::time::Instant::now();
        let raster = super::vector::rasterize(&path, 4096, 256, &mut budget);
        let elapsed = started.elapsed();
        let work = u64::MAX - budget.work;
        assert!(raster.is_ok(), "shuffled {shuffled}");
        assert!(work <= super::vector::MAX_RASTER_WORK);
        assert!(work * 5 > super::vector::MAX_RASTER_WORK * 4, "{work}");
        println!("worst case at the cap (shuffled {shuffled}): work {work}, {elapsed:?}");
        assert!(elapsed.as_secs_f64() < 120.0, "{elapsed:?}");
    }
}

/// The per-file aggregate (G2): five layers each carrying the shuffled
/// worst case — only four fit `MAX_FILE_VECTOR_WORK`; the fifth is
/// reported, and the whole open is timed.
#[test]
fn a_files_vector_masks_stop_at_the_per_file_work_budget() {
    let n = 4096_u16;
    let mut records = vec![subpath(true, n, 1)];
    for i in 0..n {
        let column = (u64::from(i) * 2_654_435_761 % u64::from(n)) as u16;
        records.push(corner(f64::from(column) / f64::from(n), f64::from(i % 2)));
    }
    let block = vmsk(0, &records);
    let bytes = TestPsd::new(1, 4096, 256, 8)
        .with(|p| {
            for i in 0..5 {
                p.layers.push(
                    TestLayer::pixels(&format!("v{i}"), 0, 0, 1, 1, 8, &[[1, 2, 3, 255]])
                        .with(|l| l.blocks.push((*b"vmsk", block.clone()))),
                );
            }
        })
        .write();
    let started = std::time::Instant::now();
    let document = doc(&bytes);
    let elapsed = started.elapsed();
    let text = report_text(&document);
    assert!(text.contains("4 vector masks were converted"), "{text}");
    assert!(
        text.contains("1 vector mask was too large or complex"),
        "{text}"
    );
    println!("five worst-case vector masks: {elapsed:?}");
    assert!(elapsed.as_secs_f64() < 120.0, "{elapsed:?}");
}

fn ok_vector(block: &[u8]) -> super::vector::VectorPath {
    match super::vector::parse(block) {
        Ok(path) => path,
        Err(err) => unreachable!("{err:?}"),
    }
}

#[test]
fn the_parser_reads_flags_knots_and_fixed_point_vertical_first() {
    let mut records = vec![initial_fill(1)];
    records.push(knot(true, (0.5, 0.5), (0.5, 0.5), (0.5, 0.5))); // stray: ignored
    records.extend(rect_path(0.125, 0.25, 0.5, 0.75, -1));
    records.push(record(7, &[]));
    let path = ok_vector(&vmsk(7, &records));
    assert!(path.invert && path.not_link && path.disable && path.initial_fill);
    assert_eq!(path.subpaths.len(), 1);
    let Some(first) = path.subpaths.first() else {
        unreachable!()
    };
    assert!(first.closed);
    assert_eq!(first.operation, -1);
    assert_eq!(first.knots.len(), 4);
    assert_eq!(
        first.knots.first().map(|k| k.anchor),
        Some(super::vector::Point { x: 0.125, y: 0.25 })
    );
    // psd-tools reads records inside a subpath's count as its items: an
    // initial-fill record there counts toward the knots and is not the
    // path's initial fill.
    let mut records = vec![subpath(true, 5, 1), initial_fill(1)];
    records.extend(rect_path(0.0, 0.0, 0.5, 0.5, 1).into_iter().skip(1));
    let path = ok_vector(&vmsk(0, &records));
    assert!(!path.initial_fill);
    assert_eq!(path.subpaths.first().map(|p| p.knots.len()), Some(4));
}

/// The vector-mask files' own truncation and mutation sweeps.
fn vector_sweep_fixtures() -> Vec<Vec<u8>> {
    let mut records = ellipse_path(0.5, 0.5, 0.3, 0.2, 1);
    records.extend(rect_path(0.1, 0.1, 0.6, 0.6, 0));
    records.extend(rect_path(0.2, 0.2, 0.4, 0.9, -1));
    records.push(initial_fill(0));
    let mut both = vector_file(6, 5, vmsk(1, &records));
    if let Some(layer) = both.layers.first_mut() {
        layer.mask = Some((0, 0, 3, 3, 0, 0x18));
        let mut tail = vec![0x00, 255];
        for v in [1_i32, 1, 3, 4] {
            tail.extend_from_slice(&v.to_be_bytes());
        }
        tail.extend_from_slice(&[
            0x0F, 100, 0, 0, 0, 0, 0, 0, 0, 0, 50, 0, 0, 0, 0, 0, 0, 0, 0,
        ]);
        layer.mask_tail = Some(tail);
        layer.channels.push((-2, vec![255; 9]));
        layer.channels.push((-3, vec![7, 8, 9, 10, 11, 12]));
    }
    vec![vector_file(5, 4, vmsk(0, &records)).write(), both.write()]
}

#[test]
fn every_truncation_and_seeded_mutation_of_a_vector_masked_file_never_panics() {
    let mut state: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        state >> 33
    };
    for file in vector_sweep_fixtures() {
        let document = ok(read(&file));
        assert!(report_text(&document).contains("converted"));
        for len in 0..file.len() {
            let _ = read(file.get(..len).unwrap_or(&[]));
        }
        for _ in 0..3_000 {
            let mut mutated = file.clone();
            let at = (next() as usize) % mutated.len().max(1);
            let value = next() as u8;
            if let Some(byte) = mutated.get_mut(at) {
                *byte = value;
            }
            let _ = read(&mutated);
        }
    }
}

/// The corpus differential (0.150.0): every vector-masked layer Aurora
/// opens from psd-tools' fixtures, Aurora's raster against
///
/// - **psd-tools' own** `draw_vector_mask` (psd-tools 1.17.4 with
///   `aggdraw` 1.4.1), by sum: the committed numbers are psd-tools' sum
///   and its count of partial (edge) pixels. `aggdraw` paints a
///   systematic 63/255 outward band on every edge pixel (measured: the
///   per-pixel maximum |Aurora − psd-tools| is 0.2471 = 63/255 on every
///   layer with a non-trivial edge, and Aurora's sum is lower by about a
///   quarter per edge pixel), so the tolerance is `63/255` per psd-tools
///   edge pixel plus half a pixel;
/// - **Photoshop's own rendering**, pixel by pixel, wherever the file
///   carries one: on a vector-only layer the `-2` channel flagged
///   "from rendering" *is* Photoshop's raster of the same path. The
///   tolerance is `1/16` per pixel: 16 sub-scanlines put a single edge
///   within `1/32` of its exact area, and two edges can share a pixel.
///
/// Expected values were computed by psd-tools itself, not by hand.
#[test]
#[allow(
    clippy::too_many_lines,
    clippy::cast_sign_loss,
    clippy::format_push_string,
    clippy::nonminimal_bool
)] // a data table and a measurement log
fn corpus_vector_masks_match_psd_tools_and_photoshops_own_rendering() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpora/psd/reference/psd-tools-fixtures");
    if !dir.is_dir() {
        println!("SKIPPED: corpus not present at {}", dir.display());
        return;
    }
    // (file, layer, psd-tools sum, psd-tools edge pixels, `-2` is a
    // rendering of the vector mask alone)
    let cases: &[(&str, &str, f64, u32, bool)] = &[
        (
            "clipping-mask2.psd",
            "Rounded Rectangle 1",
            190_429.020,
            1748,
            true,
        ),
        (
            "mask-density-layervectormask.psd",
            "Layer 1",
            133.988,
            25,
            false,
        ),
        (
            "mask-density-layervectormask.psd",
            "Layer 1 copy",
            138.000,
            42,
            false,
        ),
        (
            "mask-density-layervectormask.psd",
            "Layer 1 copy 2",
            138.000,
            42,
            false,
        ),
        (
            "mask-density-layervectormask.psd",
            "Layer 1 copy 3",
            133.988,
            25,
            false,
        ),
        ("mask-density-vectormask.psd", "Layer 1", 133.988, 25, true),
        (
            "mask-density-vectormask.psd",
            "Layer 1 copy",
            138.000,
            42,
            true,
        ),
        (
            "mask-density-vectormask.psd",
            "Layer 1 copy 2",
            138.000,
            42,
            true,
        ),
        (
            "mask-density-vectormask.psd",
            "Layer 1 copy 3",
            133.988,
            25,
            true,
        ),
        ("mask-vector-density.psd", "Layer 1", 133.988, 25, true),
        ("mask-vector-density.psd", "Layer 1 copy", 138.000, 42, true),
        (
            "mask-vector-density.psd",
            "Layer 1 copy 2",
            138.000,
            42,
            true,
        ),
        (
            "mask-vector-density.psd",
            "Layer 1 copy 3",
            133.988,
            25,
            true,
        ),
        ("mask_parameters.psd", "Rectangle 1", 25_757.725, 661, true),
        ("passthrough_vector_mask.psd", "Group 1", 263.965, 33, true),
        ("vector-mask2.psd", "Masked Rectangle 1", 90.129, 40, false),
        ("vector-mask2.psd", "Color Fill 1", 13.241, 20, true),
        ("vector-mask3.psd", "Group 1", 65_536.000, 0, false),
    ];
    let band = 63.0 / 255.0;
    for &(file, layer, want, edge, rendered) in cases {
        let bytes = match std::fs::read(dir.join(file)) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{file}: {err}"),
        };
        let Some((w, h, plane)) = vector_plane(&bytes, layer) else {
            unreachable!("{file}: no vector mask on {layer:?}");
        };
        let sum: f64 = plane.iter().map(|v| f64::from(*v)).sum();
        let tolerance = band * f64::from(edge) + 0.5;
        assert!(
            (sum - want).abs() <= tolerance,
            "{file} {layer}: Aurora {sum} vs psd-tools {want} (±{tolerance})"
        );
        // The document really applies it, and says so.
        let document = doc(&bytes);
        let Some(id) = find_named(&document.layers, layer) else {
            unreachable!("{file}: no layer {layer:?}");
        };
        assert!(document.layers.mask(id).is_some(), "{file} {layer}");
        assert!(report_text(&document).contains("converted"), "{file}");
        let mut line = format!(
            "{file} {layer}: Aurora {sum:.3}, psd-tools {want:.3} (Δ {:.3}, {edge} edge px)",
            sum - want
        );
        if rendered {
            let (header, records) = records_of(&bytes);
            let Some(record) = records.iter().find(|r| r.name == layer) else {
                unreachable!()
            };
            let Ok(Some(mut render)) = super::decode_mask(&mut source(&bytes), record, header)
            else {
                unreachable!("{file} {layer}: no rendered -2");
            };
            render.density = u8::MAX;
            let mut worst = 0.0_f32;
            let mut ps_sum = 0.0_f64;
            for y in 0..i64::from(h) {
                for x in 0..i64::from(w) {
                    let ps = super::effective_at(&render, x, y);
                    let a = plane
                        .get((y * i64::from(w) + x) as usize)
                        .copied()
                        .unwrap_or(f32::NAN);
                    let d = (a - ps).abs();
                    worst = if d.is_nan() {
                        f32::INFINITY
                    } else {
                        worst.max(d)
                    };
                    ps_sum += f64::from(ps);
                }
            }
            assert!(
                worst <= 1.0 / 16.0,
                "{file} {layer}: max |Aurora - Photoshop| {worst}"
            );
            line += &format!("; Photoshop's own render {ps_sum:.3}, max per-pixel |Δ| {worst:.4}");
        } else {
            // Both a user mask and a vector mask: the `-2` channel is
            // Photoshop's rendering of the two together (density not
            // applied), against Aurora's `real user mask × vector`.
            let (header, records) = records_of(&bytes);
            let Some(record) = records.iter().find(|r| r.name == layer) else {
                unreachable!()
            };
            let Ok(Some(mut render)) = super::decode_mask(&mut source(&bytes), record, header)
            else {
                unreachable!("{file} {layer}: no rendered -2");
            };
            render.density = u8::MAX;
            let density = document.layers.mask(id).map_or(f32::NAN, |m| m.density);
            let (mut worst, mut ps_sum, mut a_sum) = (0.0_f32, 0.0_f64, 0.0_f64);
            for y in 0..i64::from(h) {
                for x in 0..i64::from(w) {
                    let ps = super::effective_at(&render, x, y);
                    let a = with_density(
                        effective_mask(&document, id, x, y).unwrap_or(f32::NAN),
                        density,
                    );
                    let d = (a - ps).abs();
                    worst = if d.is_nan() {
                        f32::INFINITY
                    } else {
                        worst.max(d)
                    };
                    ps_sum += f64::from(ps);
                    a_sum += f64::from(a);
                }
            }
            line += &format!(
                "; combined: Aurora {a_sum:.3} vs Photoshop's render {ps_sum:.3}, max |Δ| {worst:.4}"
            );
            // Photoshop's rendering carries no density, so it is the
            // reference only where both densities are full.
            if density == 1.0
                && !record
                    .mask
                    .is_some_and(|m| m.parameters.vector_density.is_some_and(|d| d != u8::MAX))
            {
                assert!(
                    worst <= 1.0 / 16.0,
                    "{file} {layer}: combined max |Δ| {worst}"
                );
                line += " (asserted)";
            }
        }
        println!("{line}");
    }
}

#[test]
fn applied_densities_are_not_reported_and_a_vector_feather_is() {
    // Both masks, both densities baked (the rendered-mask fixture): no
    // parameter line at all.
    let mut file = vector_file(8, 8, vmsk(0, &rect_path(0.25, 0.25, 0.75, 0.75, 1)));
    if let Some(layer) = file.layers.first_mut() {
        layer.mask = Some((0, 0, 8, 8, 0, 0x18));
        let mut tail = vec![0x00, 0];
        for v in [0_i32, 0, 8, 4] {
            tail.extend_from_slice(&v.to_be_bytes());
        }
        tail.extend_from_slice(&[0x05, 128, 64]);
        layer.mask_tail = Some(tail);
        layer.channels.push((-2, vec![255; 64]));
        layer.channels.push((-3, vec![255; 32]));
    }
    let text = report_text(&doc(&file.write()));
    assert!(!text.contains("feather"), "{text}");
    assert!(!text.contains("density"), "{text}");
    // A vector-only layer (mask data, no `-2`) with a vector feather.
    let mut file = vector_file(8, 8, vmsk(0, &rect_path(0.25, 0.25, 0.75, 0.75, 1)));
    if let Some(layer) = file.layers.first_mut() {
        layer.mask = Some((0, 0, 0, 0, 0, 0x10));
        let mut tail = vec![0x08];
        tail.extend_from_slice(&1.5_f64.to_be_bytes());
        layer.mask_tail = Some(tail);
    }
    let text = report_text(&doc(&file.write()));
    assert!(
        text.contains("1 layer mask uses a feather, which"),
        "{text}"
    );
    assert!(!text.contains("density"), "{text}");
}

#[test]
fn an_unconvertible_vector_mask_falls_back_to_photoshops_rendering_and_says_so() {
    // Flags bit 3, `-2` the rendering (left half), `-3` a real user mask,
    // and a vector mask that does not parse: `-2` is applied, and the
    // report says it is Photoshop's combination, not "areas are visible".
    let mut file = vector_file(8, 8, vec![0, 0, 0, 2, 0, 0, 0, 0]);
    if let Some(layer) = file.layers.first_mut() {
        layer.mask = Some((0, 0, 8, 4, 0, 0x08));
        let mut tail = vec![0x00, 0];
        for v in [0_i32, 0, 8, 8] {
            tail.extend_from_slice(&v.to_be_bytes());
        }
        layer.mask_tail = Some(tail);
        layer.channels.push((-2, vec![255; 32]));
        layer.channels.push((-3, vec![255; 64]));
    }
    let document = doc(&file.write());
    assert_eq!(vmask(&document, 1, 1), Some(1.0));
    assert_eq!(vmask(&document, 5, 1), Some(0.0));
    let text = report_text(&document);
    assert!(
        text.contains("the mask Photoshop saved already rendered"),
        "{text}"
    );
    assert!(
        text.contains("Photoshop's own saved rendering of it was applied"),
        "{text}"
    );
    assert!(!text.contains("areas the mask hides are visible"), "{text}");
    assert!(!text.contains("isn't used"), "{text}");
}

#[test]
fn a_layer_that_falls_back_gets_its_pixel_charges_back() {
    // Pixel mask (no rendering flag) + vector mask; a budget that fits
    // the raster but not the combined rectangle.
    let mut file = vector_file(8, 8, vmsk(0, &rect_path(0.25, 0.25, 0.75, 0.75, 1)));
    if let Some(layer) = file.layers.first_mut() {
        layer.mask = Some((0, 0, 8, 4, 0, 0));
        layer.channels.push((-2, vec![255; 32]));
    }
    let bytes = file.write();
    let (header, records) = records_of(&bytes);
    let Some(record) = records.first() else {
        unreachable!()
    };
    // The raster is 6×6 (the box grown by a pixel), charged twice.
    let start = 36 * super::vector::RASTER_PIXEL_CHARGE;
    let mut budget = super::vector::Budget {
        pixels: start,
        work: u64::MAX,
    };
    let mut notes = super::Notes::default();
    let canvas = super::canvas_of(header);
    let outcome = super::masks_for(
        &mut source(&bytes),
        record,
        header,
        canvas,
        &mut notes,
        &mut budget,
    );
    assert!(!outcome.vector_applied);
    assert_eq!(budget.pixels, start, "refunded");
    assert!(outcome.mask.is_some(), "the pixel mask alone, as 0.149.0");
    assert!(
        notes
            .report()
            .items
            .join("\n")
            .contains("too large or complex")
    );
}

/// G5 (0.150.0 review): the partial-density layers with both masks,
/// against `(d_u·U + 1 − d_u)(d_v·V + 1 − d_v)` computed here from the
/// decoded real user mask `U` (`-3`) and Aurora's vector plane `V`.
/// The oracle reuses Aurora's own raster (`vector_plane`), so this is a
/// self-consistency check of `combine_masks`' density arithmetic and
/// rectangle choice — not an independent check of the raster, which
/// the psd-tools/Photoshop differential above is. A `NaN` anywhere
/// fails it.
#[test]
#[allow(clippy::cast_sign_loss, clippy::many_single_char_names)]
fn corpus_masks_with_both_densities_are_the_product_of_each_part() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpora/psd/reference/psd-tools-fixtures");
    if !dir.is_dir() {
        println!("SKIPPED: corpus not present at {}", dir.display());
        return;
    }
    let Ok(bytes) = std::fs::read(dir.join("mask-density-layervectormask.psd")) else {
        unreachable!()
    };
    let document = doc(&bytes);
    let (header, records) = records_of(&bytes);
    for (layer, density) in [
        ("Layer 1", 64_u8),
        ("Layer 1 copy", 128),
        ("Layer 1 copy 2", 191),
    ] {
        let Some(record) = records.iter().find(|r| r.name == layer) else {
            unreachable!()
        };
        let Some(info) = record.mask else {
            unreachable!()
        };
        assert_eq!(info.parameters.user_density, Some(density));
        assert_eq!(info.parameters.vector_density, Some(density));
        let Some(real) = info.real else {
            unreachable!()
        };
        let Ok(u) =
            super::decode_mask_channel(&mut source(&bytes), record, header, -3, real, u8::MAX)
        else {
            unreachable!()
        };
        let Some((w, _, v)) = vector_plane(&bytes, layer) else {
            unreachable!()
        };
        let Some(id) = find_named(&document.layers, layer) else {
            unreachable!()
        };
        let d = f32::from(density) / 255.0;
        let (mut worst, mut sum) = (0.0_f32, 0.0_f64);
        for y in 0..i64::from(header.height) {
            for x in 0..i64::from(header.width) {
                let b = u.bounds;
                let inside = x >= b.x && y >= b.y && x < b.right() && y < b.bottom();
                let mut uv = if inside {
                    let i = ((y - b.y) * i64::from(b.width) + (x - b.x)) as usize;
                    u.coverage
                        .as_ref()
                        .and_then(|c| c.get(i))
                        .map_or(f32::NAN, |c| c.to_f32())
                } else if u.default_color != 0 {
                    1.0
                } else {
                    0.0
                };
                if u.flags & 0x04 != 0 {
                    uv = 1.0 - uv;
                }
                let vv = v
                    .get((y * i64::from(w) + x) as usize)
                    .copied()
                    .unwrap_or(f32::NAN);
                let want = (d * uv + 1.0 - d) * (d * vv + 1.0 - d);
                let got = effective_mask(&document, id, x, y).unwrap_or(f32::NAN);
                let d = (got - want).abs();
                worst = if d.is_nan() {
                    f32::INFINITY
                } else {
                    worst.max(d)
                };
                sum += f64::from(got);
            }
        }
        assert_eq!(document.layers.mask(id).map(|m| m.density), Some(1.0));
        assert!(worst <= 1e-3, "{layer}: {worst}");
        println!(
            "{layer}: density {density}, Aurora sum {sum:.3}, max |Δ| vs the product {worst:.5}"
        );
    }
}

/// One full-size vector-masked layer per block, 1×1 pixels each.
fn many_vector_layers(width: u32, height: u32, block: &[u8], layers: usize) -> Vec<u8> {
    TestPsd::new(1, width, height, 8)
        .with(|p| {
            for i in 0..layers {
                p.layers.push(
                    TestLayer::pixels(&format!("v{i}"), 0, 0, 1, 1, 8, &[[1, 2, 3, 255]])
                        .with(|l| l.blocks.push((*b"vmsk", block.to_vec()))),
                );
            }
        })
        .write()
}

/// Round 2 of the review: parsing and flattening are charged to the
/// file's work budget too, and an off-canvas path (an empty raster) is
/// not free. 50 layers each carrying a 4,000-knot path wholly above
/// the canvas; a budget for exactly three: three convert, the other 47
/// are refused before they are even parsed.
#[test]
fn parsing_and_flattening_draw_on_the_files_work_budget() {
    let knots = 4000_u16;
    let mut records = vec![subpath(true, knots, 1)];
    for i in 0..knots {
        records.push(corner(f64::from(i % 2), -0.5));
    }
    let block = vmsk(0, &records);
    // Parse: one per record plus one; flatten: `SEGMENT_CHARGE` per segment.
    let parse = (block.len() / 26) as u64 + 1;
    let flatten = (1 + u64::from(knots)) * super::vector::SEGMENT_CHARGE;
    let one = parse + flatten;
    let bytes = many_vector_layers(8, 8, &block, 50);
    let started = std::time::Instant::now();
    let file = ok(super::decode_with_vector_work(&bytes, 3 * one + 10));
    let elapsed = started.elapsed();
    let text = file.report().items.join("\n");
    assert!(text.contains("3 vector masks were converted"), "{text}");
    assert!(
        text.contains("47 vector masks were too large or complex"),
        "{text}"
    );
    assert!(elapsed.as_secs_f64() < 5.0, "{elapsed:?}");
}

/// The hostile ceiling of parsing and flattening, at the real per-file
/// budget: 1,100 layers, each a 4,095-knot path whose handles fling far
/// off the canvas (≈ 2^20 segments, just under the per-mask cap) and
/// whose every point lies above it. Heavy (a ~117 MB file), so it is
/// `#[ignore]`d rather than printing `SKIPPED` — that word is the
/// workspace's "a real-GPU test did not run" signal under
/// `AURORA_REQUIRE_GPU`, and an always-printed one would dilute it. Run it
/// with `cargo test -p aurora-io --release -- --ignored
/// the_worst_case_of_parsing`; measured in release and recorded in PLAN.md.
#[test]
#[ignore = "heavy (~117 MB file); run with --ignored, ideally in release"]
fn the_worst_case_of_parsing_and_flattening_at_the_file_budget() {
    let knots = 4095_u16;
    let mut records = vec![subpath(true, knots, 1)];
    for i in 0..knots {
        let x = f64::from(i % 2);
        records.push(knot(true, (x, -10.0), (x, -50.0), (1.0 - x, -90.0)));
    }
    let block = vmsk(0, &records);
    let bytes = many_vector_layers(64, 64, &block, 1100);
    let started = std::time::Instant::now();
    let file = ok(decode(&bytes));
    let elapsed = started.elapsed();
    let text = file.report().items.join("\n");
    println!("1,100 flatten-heavy vector masks: {elapsed:?}\n{text}");
    assert!(text.contains("too large or complex"), "{text}");
}

/// The exact parse and flattening charges of one off-canvas mask (its
/// raster is empty, so nothing else is charged), and a budget one unit
/// short of the flattening refusing it with only the parse charge kept.
#[test]
fn an_off_canvas_mask_is_charged_exactly_its_parse_and_flattening() {
    let knots = 100_u16;
    let mut records = vec![subpath(true, knots, 1)];
    for i in 0..knots {
        records.push(corner(f64::from(i % 2), -0.5));
    }
    let block = vmsk(0, &records);
    let parse = (block.len() / 26) as u64 + 1;
    let flatten = (1 + u64::from(knots)) * super::vector::SEGMENT_CHARGE;
    let bytes = many_vector_layers(8, 8, &block, 1);
    let (header, layers) = records_of(&bytes);
    let Some(record) = layers.first() else {
        unreachable!()
    };
    let start = parse + flatten;
    let mut budget = super::vector::Budget {
        pixels: u64::MAX,
        work: start,
    };
    let raster = super::vector_for(record, header, &mut budget);
    assert!(
        matches!(raster, Ok(Some(ref r)) if r.coverage.is_empty()),
        "{raster:?}"
    );
    assert_eq!(budget.work, 0, "parse + flatten, nothing else");
    let mut budget = super::vector::Budget {
        pixels: u64::MAX,
        work: start - 1,
    };
    assert_eq!(
        super::vector_for(record, header, &mut budget).map(|r| r.is_some()),
        Err(super::VectorSkip::TooLarge)
    );
    assert_eq!(budget.work, flatten - 1, "only the parse charge is kept");
}

// ---------------------------------------------------------------------
// Streaming (0.154.0)
// ---------------------------------------------------------------------

/// A seekable reader over `data` that records how far into the file it
/// has ever read (shared, so a sink can look mid-read), and can fail with
/// an I/O error past a byte or claim to be longer than it is.
struct Probe {
    inner: std::io::Cursor<Vec<u8>>,
    /// Every read, as `(offset, length)`.
    log: std::rc::Rc<std::cell::RefCell<Vec<(u64, u64)>>>,
    fail_at: Option<u64>,
    /// Fails the first read that touches this byte range, once; every
    /// read after it succeeds.
    fail_once: Option<(u64, u64)>,
    claimed_len: Option<u64>,
}

impl Probe {
    fn new(data: Vec<u8>) -> Self {
        Self {
            inner: std::io::Cursor::new(data),
            log: std::rc::Rc::default(),
            fail_at: None,
            fail_once: None,
            claimed_len: None,
        }
    }
}

impl std::io::Read for Probe {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let pos = self.inner.position();
        if let Some(fail_at) = self.fail_at
            && pos + buf.len() as u64 > fail_at
        {
            return Err(std::io::Error::other("probe: the disk went away"));
        }
        if let Some((from, to)) = self.fail_once
            && pos < to
            && pos + buf.len() as u64 > from
        {
            self.fail_once = None;
            return Err(std::io::Error::other("probe: one transient read error"));
        }
        let n = self.inner.read(buf)?;
        self.log.borrow_mut().push((pos, n as u64));
        Ok(n)
    }
}

impl std::io::Seek for Probe {
    fn seek(&mut self, to: std::io::SeekFrom) -> std::io::Result<u64> {
        match (to, self.claimed_len) {
            (std::io::SeekFrom::End(0), Some(len)) => {
                self.inner.set_position(len);
                Ok(len)
            }
            _ => self.inner.seek(to),
        }
    }
}

/// Records what a streaming read hands over, and how far into the file
/// the reader had got at each hand-over.
#[derive(Default)]
struct Recording {
    log: std::rc::Rc<std::cell::RefCell<Vec<(u64, u64)>>>,
    /// How many reads had happened at each layer's hand-over.
    reached: Vec<usize>,
    pixels: Vec<super::PsdPixels>,
    masks: Vec<super::PsdMaskPixels>,
    fail_on_layer: Option<usize>,
}

impl super::PsdPixelSink for Recording {
    fn layer(&mut self, pixels: super::PsdPixels) -> Result<(), IoError> {
        if self.fail_on_layer == Some(self.pixels.len()) {
            return Err(IoError::PsdMalformed {
                what: "sink refused",
            });
        }
        self.reached.push(self.log.borrow().len());
        self.pixels.push(pixels);
        Ok(())
    }

    fn mask(&mut self, mask: super::PsdMaskPixels) -> Result<(), IoError> {
        self.masks.push(mask);
        Ok(())
    }
}

/// Four noisy 64×64 layers, `PackBits` or raw.
fn four_noisy_layers(compression: u16) -> Vec<u8> {
    let mut seed = 0x2545_F491_4F6C_DD1D_u64;
    TestPsd::new(1, 64, 64, 8)
        .with(|f| {
            for k in 0..4 {
                let rgba: Vec<[u16; 4]> = (0..64 * 64)
                    .map(|_| {
                        seed ^= seed << 13;
                        seed ^= seed >> 7;
                        seed ^= seed << 17;
                        let v = seed.to_le_bytes();
                        [v[0], v[1], v[2], v[3] | 1].map(u16::from)
                    })
                    .collect();
                let name = format!("noise {k}");
                let layer = TestLayer::pixels(&name, 0, 0, 64, 64, 8, &rgba).with(|l| {
                    l.compression = compression;
                });
                f.layers.push(layer);
            }
        })
        .write()
}

/// A [`PsdDocument`] reduced to comparable data: tree, canvas, report,
/// and every pixel and mask sample's bits.
fn summary(document: &PsdDocument) -> (String, u64) {
    use std::fmt::Write as _;
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut out = format!(
        "{:?}\n{:?}\n{:?}\n",
        document.layers, document.canvas_size, document.report
    );
    for p in &document.pixels {
        let _ = writeln!(
            out,
            "{:?} {:?} {}x{}",
            p.layer,
            p.offset,
            p.image.width(),
            p.image.height()
        );
        for sample in p.image.samples() {
            sample.to_bits().hash(&mut hasher);
        }
    }
    for m in &document.masks {
        let _ = writeln!(out, "{:?} {:?} {}x{}", m.layer, m.offset, m.width, m.height);
        for sample in &m.coverage {
            sample.to_bits().hash(&mut hasher);
        }
    }
    (out, hasher.finish())
}

fn streamed(reader: impl std::io::Read + std::io::Seek) -> Result<PsdDocument, IoError> {
    let mut collect = super::Collect::default();
    super::read_streaming(reader, &mut collect).map(|document| collect.into_document(document))
}

/// AC-2: the streaming read of a real file on disk (`BufReader<File>`) is
/// the in-memory read, sample for sample, for every fixture and — when
/// present — every corpus file, refusals included.
#[test]
fn streaming_a_file_from_disk_matches_the_in_memory_read_for_every_fixture_and_corpus_file() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut pending = vec![
        root.join("tests/fixtures/psd"),
        root.join("../../corpora/psd"),
    ];
    let mut files = Vec::new();
    while let Some(next) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("psd") || e.eq_ignore_ascii_case("psb"))
            {
                files.push(path);
            }
        }
    }
    assert!(files.len() >= 10, "the checked-in fixtures are there");
    for path in &files {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let Ok(file) = std::fs::File::open(path) else {
            continue;
        };
        let from_disk = streamed(std::io::BufReader::new(file));
        let in_memory = read(&bytes);
        match (from_disk, in_memory) {
            (Ok(a), Ok(b)) => assert_eq!(summary(&a), summary(&b), "{}", path.display()),
            (Err(a), Err(b)) => {
                assert_eq!(format!("{a:?}"), format!("{b:?}"), "{}", path.display());
            }
            (a, b) => unreachable!("{}: {a:?} vs {b:?}", path.display()),
        }
        // And the eager `decode` + `build_document` path agrees too.
        if let Ok(a) = read(&bytes) {
            let b = ok(super::build_document(ok(decode(&bytes))));
            assert_eq!(summary(&a), summary(&b), "{}", path.display());
        }
    }
}

/// AC-1/AC-5: each layer is handed to the sink before the next layer's
/// channel data is read, no single read is bigger than the largest
/// channel, and the whole file is read about once — never whole.
#[test]
fn the_streaming_read_hands_each_layer_on_before_reading_the_next() {
    for compression in [0, 1] {
        let bytes = four_noisy_layers(compression);
        let (_, records) = records_of(&bytes);
        // Each layer's channel data, as one byte range (they are laid out
        // layer after layer).
        let ranges: Vec<(u64, u64)> = records
            .iter()
            .map(|r| {
                let start = r.data.iter().map(|(_, c)| c.offset).min().unwrap_or(0);
                let end = r
                    .data
                    .iter()
                    .map(|(_, c)| c.offset + c.len as u64)
                    .max()
                    .unwrap_or(0);
                (start, end)
            })
            .collect();
        let largest_channel = records
            .iter()
            .flat_map(|r| r.data.iter().map(|(_, c)| c.len))
            .max()
            .unwrap_or(0);
        let len = bytes.len() as u64;
        let probe = Probe::new(bytes);
        let probe_log = std::rc::Rc::clone(&probe.log);
        let mut sink = Recording {
            log: std::rc::Rc::clone(&probe.log),
            ..Recording::default()
        };
        let mut src = ok(super::StreamSource::new(probe));
        let file = ok(super::decode_source(
            &mut src,
            super::vector::MAX_FILE_VECTOR_WORK,
        ));
        ok(super::build_streamed(file, Some(&mut src), &mut sink));
        assert_eq!(sink.pixels.len(), 4);
        let log = probe_log.borrow();
        for (k, reached) in sink.reached.iter().enumerate() {
            for (start, len) in log.iter().take(*reached) {
                for (later, (from, to)) in ranges.iter().enumerate().skip(k + 1) {
                    assert!(
                        start + len <= *from || start >= to,
                        "layer {k} was handed on after a read of layer {later}'s channel data"
                    );
                }
            }
        }
        assert_eq!(sink.reached.len(), 4);
        assert!(
            src.largest_read <= largest_channel,
            "largest single read {} > largest channel {largest_channel}",
            src.largest_read
        );
        // Each channel's 2-byte compression field is read twice (checked,
        // then decoded), and the header probe re-reads up to 64 bytes.
        let slack = 64 + 2 * 16;
        assert!(
            src.total_read <= len + slack,
            "read {} bytes of a {len}-byte file",
            src.total_read
        );
        let channels: u64 = records
            .iter()
            .flat_map(|r| r.data.iter().map(|(_, c)| c.len as u64))
            .sum();
        assert!(src.total_read >= channels, "every channel is read once");
    }
}

/// AC-3: an I/O error mid-layer is `IoError::Io`, and a reader that ends
/// before the length it reported is `PsdTruncated` — never a panic, never
/// a silently zero-filled layer.
#[test]
fn a_reader_failing_or_ending_mid_layer_is_a_typed_error() {
    let bytes = four_noisy_layers(0);
    let (_, records) = records_of(&bytes);
    let Some(third) = records.get(2).and_then(|r| r.data.first()) else {
        unreachable!("four layers");
    };
    let mid = third.1.offset + 100;
    let mut failing = Probe::new(bytes.clone());
    failing.fail_at = Some(mid);
    assert!(matches!(streamed(failing), Err(IoError::Io(_))));

    let mut short = Probe::new(bytes.get(..mid as usize).unwrap_or(&[]).to_vec());
    short.claimed_len = Some(bytes.len() as u64);
    assert!(matches!(streamed(short), Err(IoError::PsdTruncated { .. })));

    // A failure inside a mask channel fails the open too — it is the file
    // that failed, not the mask (the tree build would report a damaged
    // mask and open the layer unmasked).
    let masked = TestPsd::new(1, 8, 8, 8)
        .with(|f| {
            let rgba = vec![[200, 100, 50, 255]; 64];
            let layer = TestLayer::pixels("masked", 0, 0, 8, 8, 8, &rgba).with(|l| {
                l.mask = Some((0, 0, 8, 8, 0, 0));
                l.channels.push((-2, vec![128; 64]));
            });
            f.layers.push(layer);
        })
        .write();
    assert_eq!(doc(&masked).masks.len(), 1, "the fixture's mask decodes");
    let (_, records) = records_of(&masked);
    let Some(mask) = records
        .first()
        .and_then(|r| r.data.iter().find(|(id, _)| *id == -2))
    else {
        unreachable!("a -2 channel");
    };
    let mut failing = Probe::new(masked);
    failing.fail_at = Some(mask.1.offset + 1);
    assert!(matches!(streamed(failing), Err(IoError::Io(_))));
}

/// AC-3/AC-5: every truncation of a multi-layer file read through a real
/// `File` gives exactly what the in-memory read of the same prefix gives.
#[test]
fn every_truncation_read_from_a_real_file_matches_the_in_memory_result() {
    let bytes = four_noisy_layers(1);
    let dir = ok(tempfile::tempdir().map_err(IoError::Io));
    let path = dir.path().join("cut.psd");
    for len in (0..bytes.len()).step_by(37) {
        let prefix = bytes.get(..len).unwrap_or(&[]);
        ok(std::fs::write(&path, prefix).map_err(IoError::Io));
        let file = ok(std::fs::File::open(&path).map_err(IoError::Io));
        let a = streamed(std::io::BufReader::new(file)).map(|d| summary(&d));
        let b = read(prefix).map(|d| summary(&d));
        assert_eq!(format!("{a:?}"), format!("{b:?}"), "prefix {len}");
    }
}

/// AC-3: a declared length or offset past the end of the file is refused
/// from the declaration alone — nothing that size is read or allocated.
#[test]
fn a_length_past_the_file_is_refused_before_anything_is_read() {
    let bytes = four_noisy_layers(0);
    let at = |offset: usize, value: u32| {
        let mut out = bytes.clone();
        if let Some(slot) = out.get_mut(offset..offset + 4) {
            slot.copy_from_slice(&value.to_be_bytes());
        }
        out
    };
    // Header (26), colour data length (4) at 26, resources length at 30,
    // section length at 34, layer-info length at 38.
    for (offset, what) in [
        (30, "image resources"),
        (34, "layer and mask section"),
        (38, "layer info"),
    ] {
        let mut src = ok(super::StreamSource::new(std::io::Cursor::new(at(
            offset,
            u32::MAX - 7,
        ))));
        let result = super::decode_source(&mut src, super::vector::MAX_FILE_VECTOR_WORK);
        assert!(
            matches!(result, Err(IoError::PsdTruncated { what: w }) if w == what),
            "{what}: {result:?}"
        );
        assert!(src.largest_read <= 64, "{what}: read {}", src.largest_read);
    }
    // The first record's first channel length, past the file.
    let (_, records) = records_of(&bytes);
    assert!(!records.is_empty());
    // Rectangle (16) + count (2) + id (2): the length is at 38 + 4 + 2 + 20.
    let mut src = ok(super::StreamSource::new(std::io::Cursor::new(at(
        38 + 4 + 2 + 20,
        u32::MAX - 7,
    ))));
    let result = super::decode_source(&mut src, super::vector::MAX_FILE_VECTOR_WORK);
    assert!(
        matches!(
            result,
            Err(IoError::PsdTruncated {
                what: "channel image data"
            })
        ),
        "{result:?}"
    );
    assert!(src.largest_read <= 4096, "read {}", src.largest_read);
    // And a read past the file is refused by the source itself, from the
    // range alone: the reader is never asked for a byte.
    let probe = Probe::new(vec![0_u8; 16]);
    let log = std::rc::Rc::clone(&probe.log);
    let mut src = ok(super::StreamSource::new(probe));
    assert!(matches!(
        super::ByteSource::read_at(&mut src, 10, 7, "probe"),
        Err(IoError::PsdTruncated { what: "probe" })
    ));
    assert!(matches!(
        super::ByteSource::read_at(&mut src, u64::MAX, 1, "probe"),
        Err(IoError::PsdTruncated { what: "probe" })
    ));
    assert_eq!(src.total_read, 0);
    assert!(
        log.borrow().is_empty(),
        "nothing was read: {:?}",
        log.borrow()
    );
}

/// A sink's error stops the read and is what comes back.
#[test]
fn a_sink_error_stops_the_streaming_read() {
    let probe = Probe::new(four_noisy_layers(0));
    let mut sink = Recording {
        fail_on_layer: Some(1),
        ..Recording::default()
    };
    let result = super::read_streaming(probe, &mut sink);
    assert!(matches!(
        result,
        Err(IoError::PsdMalformed {
            what: "sink refused"
        })
    ));
    assert_eq!(sink.pixels.len(), 1);
}

/// 0.154.0 review I1: a read that fails **once**, only on a mask
/// channel's bytes, and would succeed if retried, fails the open with the
/// reader's own `IoError::Io` — through the streaming read and through
/// `decode`'s parser — and is never opened with the mask dropped and
/// reported as damaged (`MaskUnreadable`).
#[test]
fn a_read_failing_once_inside_a_mask_fails_the_open_not_the_mask() {
    let masked = TestPsd::new(1, 8, 8, 8)
        .with(|f| {
            let rgba = vec![[200, 100, 50, 255]; 64];
            let layer = TestLayer::pixels("masked", 0, 0, 8, 8, 8, &rgba).with(|l| {
                l.mask = Some((0, 0, 8, 8, 0, 0));
                l.channels.push((-2, vec![128; 64]));
            });
            f.layers.push(layer);
        })
        .write();
    let opened = doc(&masked);
    assert_eq!(opened.masks.len(), 1, "the fixture's mask decodes");
    assert!(opened.report.is_empty(), "{:?}", opened.report);
    let (_, records) = records_of(&masked);
    let Some((_, mask)) = records
        .first()
        .and_then(|r| r.data.iter().find(|(id, _)| *id == -2))
        .copied()
    else {
        unreachable!("a -2 channel");
    };
    let range = (mask.offset, mask.offset + mask.len as u64);
    // Pixel channels come before the mask's, so only the mask read fails.
    assert!(records.first().is_some_and(|r| {
        r.data
            .iter()
            .all(|(id, c)| *id == -2 || c.offset + c.len as u64 <= range.0)
    }));

    let mut probe = Probe::new(masked.clone());
    probe.fail_once = Some(range);
    let result = streamed(probe);
    assert!(
        matches!(&result, Err(IoError::Io(err)) if err.to_string().contains("transient")),
        "{:?}",
        result.map(|d| d.report)
    );

    let mut probe = Probe::new(masked);
    probe.fail_once = Some(range);
    let mut src = ok(super::StreamSource::new(probe));
    let result = super::decode_source(&mut src, super::vector::MAX_FILE_VECTOR_WORK);
    assert!(
        matches!(&result, Err(IoError::Io(err)) if err.to_string().contains("transient")),
        "{:?}",
        result.map(|f| f.report())
    );
}

// ---------------------------------------------------------------------
// Curves adjustment layers (`curv`, 0.158.0)
// ---------------------------------------------------------------------

/// One `Crv ` item: channel id and `(output, input)` points.
type CrvItem<'a> = (u16, &'a [(u16, u16)]);

/// A `curv` block: the map flag `0`, `version`, `count_map`, the legacy
/// curves, then (when given) the `Crv ` extra data. Points are the file's
/// `(output, input)` pairs.
fn curv(
    version: u16,
    count_map: u32,
    legacy: &[&[(u16, u16)]],
    extra: Option<&[CrvItem<'_>]>,
) -> Vec<u8> {
    let points = |out: &mut Vec<u8>, pts: &[(u16, u16)]| {
        out.extend_from_slice(&u16::try_from(pts.len()).unwrap_or(u16::MAX).to_be_bytes());
        for (o, i) in pts {
            out.extend_from_slice(&o.to_be_bytes());
            out.extend_from_slice(&i.to_be_bytes());
        }
    };
    let mut out = vec![0];
    out.extend_from_slice(&version.to_be_bytes());
    out.extend_from_slice(&count_map.to_be_bytes());
    for pts in legacy {
        points(&mut out, pts);
    }
    if let Some(extra) = extra {
        out.extend_from_slice(b"Crv ");
        out.extend_from_slice(&4_u16.to_be_bytes());
        out.extend_from_slice(&u32::try_from(extra.len()).unwrap_or(0).to_be_bytes());
        for (channel, pts) in extra {
            out.extend_from_slice(&channel.to_be_bytes());
            points(&mut out, pts);
        }
    }
    while out.len() % 4 != 0 {
        out.push(0);
    }
    out
}

/// `curves_rgb.psd`'s `Curves 1`-like block: composite, red (moved
/// endpoints), green, blue, in both layouts.
fn curv_rgb_block() -> Vec<u8> {
    let c: &[(u16, u16)] = &[(0, 0), (37, 49), (118, 95), (232, 197), (255, 255)];
    let r: &[(u16, u16)] = &[(0, 11), (34, 33), (255, 238)];
    let g: &[(u16, u16)] = &[(0, 0), (72, 65), (211, 213), (255, 255)];
    let b: &[(u16, u16)] = &[(15, 0), (95, 73), (128, 122), (255, 255)];
    curv(
        1,
        0b1111,
        &[c, r, g, b],
        Some(&[(0, c), (1, r), (2, g), (3, b)]),
    )
}

/// A 2×2 RGB file: a pixel layer under a Curves layer carrying `block`
/// and a 1-pixel-wide mask, both inside a Pass Through group.
fn curves_file(block: &[u8], tweak: impl FnOnce(&mut TestLayer)) -> TestPsd {
    let block = block.to_vec();
    let curves = TestLayer::empty("Curves 1").with(|l| {
        l.blocks.push((*b"curv", block));
        l.mask = Some((0, 0, 2, 1, 255, 0));
        l.channels.push((-2, vec![0, 255]));
        tweak(l);
    });
    TestPsd::new(1, 2, 2, 8).with(|p| {
        p.layers = vec![
            TestLayer::divider(),
            TestLayer::pixels("p", 0, 0, 2, 2, 8, &two_by_two(8)),
            curves,
            TestLayer::group("G", *b"pass"),
        ];
    })
}

fn points_of(curve: &aurora_core::ToneCurve) -> Vec<(f32, f32)> {
    curve.points().iter().map(|p| (p.x, p.y)).collect()
}

fn levels(pts: &[(u16, u16)]) -> Vec<(f32, f32)> {
    pts.iter()
        .map(|&(o, i)| (f32::from(i) / 255.0, f32::from(o) / 255.0))
        .collect()
}

fn params_of(block: &[u8]) -> Result<aurora_core::CurvesParams, Note> {
    match parse_curv(block) {
        Ok(parsed) => curves_params(&parsed, false),
        Err(CurvRefusal::TooManyPoints) => Err(Note::CurvesTooManyPoints),
        Err(CurvRefusal::Unreadable) => Err(Note::CurvesUnreadable),
    }
}

/// AC-1: the legacy layout alone — version 1, no `Crv ` data — names its
/// curves' channels by the bitmap's set bits, lowest first.
#[test]
fn curv_legacy_layout_maps_the_channel_bitmap_low_bit_first() {
    let comp: &[(u16, u16)] = &[(0, 0), (84, 55), (255, 255)];
    let blue: &[(u16, u16)] = &[(0, 0), (110, 167), (166, 255)];
    let block = curv(1, 0b1001, &[comp, blue], None);
    assert_eq!(
        parse_curv(&block),
        Ok(CurvBlock {
            curves: vec![(0, comp.to_vec()), (3, blue.to_vec())]
        })
    );
    let params = match params_of(&block) {
        Ok(params) => params,
        Err(note) => unreachable!("{note:?}"),
    };
    assert_eq!(points_of(&params.composite), levels(comp));
    assert!(params.red.is_none() && params.green.is_none());
    assert_eq!(params.blue.as_ref().map(points_of), Some(levels(blue)));
    // Bits 1 and 2: red then green.
    let block = curv(1, 0b0110, &[comp, blue], None);
    let params = params_of(&block).unwrap_or_else(|_| aurora_core::CurvesParams::identity());
    assert!(params.composite.is_identity());
    assert_eq!(params.red.as_ref().map(points_of), Some(levels(comp)));
    assert_eq!(params.green.as_ref().map(points_of), Some(levels(blue)));
}

/// AC-1: the extended layout (`Crv ` extra data, which psd-tools'
/// compositor reads exclusively) wins over the legacy curves; a damaged
/// `Crv ` falls back to them, as psd-tools does.
#[test]
fn curv_extended_layout_wins_and_a_damaged_one_falls_back_to_legacy() {
    let a: &[(u16, u16)] = &[(0, 0), (100, 128), (255, 255)];
    let b: &[(u16, u16)] = &[(0, 0), (200, 128), (255, 255)];
    let block = curv(1, 0b0001, &[a], Some(&[(2, b)]));
    let params = match params_of(&block) {
        Ok(params) => params,
        Err(note) => unreachable!("{note:?}"),
    };
    assert!(params.composite.is_identity());
    assert_eq!(params.green.as_ref().map(points_of), Some(levels(b)));
    // The same block with its `Crv ` signature broken, then cut short.
    let mut broken = block.clone();
    let at = broken.windows(4).position(|w| w == b"Crv ").unwrap_or(0);
    if let Some(byte) = broken.get_mut(at) {
        *byte = b'X';
    }
    for damaged in [broken, block.get(..block.len() - 6).unwrap_or(&[]).to_vec()] {
        let params = match params_of(&damaged) {
            Ok(params) => params,
            Err(note) => unreachable!("{note:?}"),
        };
        assert_eq!(points_of(&params.composite), levels(a));
        assert!(params.green.is_none());
    }
}

/// AC-1/AC-2: per-channel curves land in their own slots, channel ids
/// past 3 are ignored, a later curve for the same channel wins, and a
/// `Crv ` item with fewer than two points is skipped.
#[test]
fn curv_per_channel_curves_map_to_curves_params() {
    let params = match params_of(&curv_rgb_block()) {
        Ok(params) => params,
        Err(note) => unreachable!("{note:?}"),
    };
    assert_eq!(
        points_of(&params.composite),
        levels(&[(0, 0), (37, 49), (118, 95), (232, 197), (255, 255)])
    );
    assert_eq!(
        params.red.as_ref().map(points_of),
        Some(levels(&[(0, 11), (34, 33), (255, 238)]))
    );
    assert_eq!(
        params.green.as_ref().map(points_of),
        Some(levels(&[(0, 0), (72, 65), (211, 213), (255, 255)]))
    );
    assert_eq!(
        params.blue.as_ref().map(points_of),
        Some(levels(&[(15, 0), (95, 73), (128, 122), (255, 255)]))
    );
    let first: &[(u16, u16)] = &[(0, 0), (10, 128), (255, 255)];
    let second: &[(u16, u16)] = &[(0, 0), (240, 128), (255, 255)];
    let block = curv(
        1,
        0,
        &[],
        Some(&[(4, first), (1, first), (1, second), (2, &[(0, 0)])]),
    );
    let params = match params_of(&block) {
        Ok(params) => params,
        Err(note) => unreachable!("{note:?}"),
    };
    assert_eq!(params.red.as_ref().map(points_of), Some(levels(second)));
    assert!(params.green.is_none() && params.blue.is_none());
    // No curves at all (psd-tools' `curves.psd`): the identity.
    assert_eq!(
        params_of(&curv(1, 0, &[], Some(&[]))),
        Ok(aurora_core::CurvesParams::identity())
    );
}

/// AC-2: movable endpoints — a first point at input 26 and a last at 238
/// — come in as given, divided by 255, and hold the output flat.
#[test]
fn curv_movable_endpoints_are_kept_and_flat_beyond() {
    let comp: &[(u16, u16)] = &[(3, 26), (84, 55), (170, 171), (255, 255)];
    let red: &[(u16, u16)] = &[(0, 11), (34, 33), (255, 238)];
    let block = curv(1, 0b0011, &[comp, red], Some(&[(0, comp), (1, red)]));
    let params = match params_of(&block) {
        Ok(params) => params,
        Err(note) => unreachable!("{note:?}"),
    };
    assert_eq!(params.composite.input_range(), (26.0 / 255.0, 1.0));
    assert_eq!(params.composite.evaluate(0.0), 3.0 / 255.0);
    assert_eq!(params.composite.evaluate(0.1), 3.0 / 255.0);
    let Some(red) = params.red else {
        unreachable!("a red curve");
    };
    assert_eq!(red.input_range(), (11.0 / 255.0, 238.0 / 255.0));
    assert_eq!(red.evaluate(1.0), 1.0);
    assert_eq!(red.evaluate(0.0), 0.0);
}

/// AC-1 hostile: too many points (either layout), the map form, version
/// 4, an unknown version, a huge count, levels past 255, inputs out of
/// order, a short legacy curve — each a typed refusal, nothing allocated
/// past the caps. And every prefix and seeded mutation of a real block
/// parses to `Ok` or a refusal, never a panic.
#[test]
fn hostile_curv_blocks_are_refused_never_a_panic() {
    let twenty: Vec<(u16, u16)> = (0..20).map(|i| (i * 13, i * 13)).collect();
    let ok3: &[(u16, u16)] = &[(0, 0), (100, 128), (255, 255)];
    assert_eq!(
        parse_curv(&curv(1, 1, &[&twenty], None)),
        Err(CurvRefusal::TooManyPoints)
    );
    assert_eq!(
        parse_curv(&curv(1, 1, &[ok3], Some(&[(0, &twenty)]))),
        Err(CurvRefusal::TooManyPoints)
    );
    // A count claiming 65535 points in a 12-byte block: capped first.
    let mut huge = vec![0, 0, 1, 0, 0, 0, 1, 0xFF, 0xFF];
    huge.extend_from_slice(&[0; 3]);
    assert_eq!(parse_curv(&huge), Err(CurvRefusal::TooManyPoints));
    let mut map = curv(1, 1, &[ok3], None);
    if let Some(flag) = map.first_mut() {
        *flag = 1;
    }
    let mut v4_huge = curv(4, 0, &[], None);
    if let Some(count) = v4_huge.get_mut(3..7) {
        count.copy_from_slice(&u32::MAX.to_be_bytes());
    }
    for (name, block) in [
        ("map form", map),
        ("version 4", curv(4, 1, &[ok3], None)),
        ("version 4, huge count", v4_huge),
        ("version 2", curv(2, 1, &[ok3], None)),
        ("one point", curv(1, 1, &[&[(0, 0)]], None)),
        ("empty", Vec::new()),
    ] {
        assert_eq!(parse_curv(&block), Err(CurvRefusal::Unreadable), "{name}");
    }
    for (name, pts) in [
        ("level 256", &[(0_u16, 0_u16), (256, 128), (255, 255)][..]),
        (
            "inputs out of order",
            &[(0, 0), (100, 200), (200, 100), (255, 255)][..],
        ),
        (
            "repeated input",
            &[(0, 0), (100, 128), (200, 128), (255, 255)][..],
        ),
    ] {
        assert_eq!(
            params_of(&curv(1, 1, &[pts], None)),
            Err(Note::CurvesUnreadable),
            "{name}"
        );
    }
    let block = curv_rgb_block();
    for len in 0..block.len() {
        let _ = parse_curv(block.get(..len).unwrap_or(&[]));
    }
    let mut state: u64 = 0x5851_F42D_4C95_7F2D;
    for _ in 0..20_000 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let mut mutated = block.clone();
        let at = (state >> 33) as usize % mutated.len();
        if let Some(byte) = mutated.get_mut(at) {
            *byte = (state >> 13) as u8;
        }
        if let Ok(parsed) = parse_curv(&mutated) {
            let _ = curves_params(&parsed, false);
        }
    }
}

/// AC-2/AC-3: through the real reader, a Curves layer becomes an
/// `Adjustment::Curves` layer above the layer below it, with its
/// opacity, blend mode, visibility and mask; it is no longer reported as
/// left out, and its Pass Through group is.
#[test]
fn a_curves_layer_opens_as_a_curves_adjustment_and_is_not_reported() {
    let bytes = curves_file(&curv_rgb_block(), |l| {
        l.opacity = 128;
        l.blend = *b"mul ";
        l.flags = 0x02;
    })
    .write();
    let document = doc(&bytes);
    let Some(&group) = document.layers.roots().first() else {
        unreachable!("a group");
    };
    let children = document.layers.children(group).unwrap_or(&[]).to_vec();
    assert_eq!(children.len(), 2);
    let (Some(&top), Some(&bottom)) = (children.first(), children.get(1)) else {
        unreachable!("two children");
    };
    assert!(matches!(
        document.layers.kind(bottom),
        Some(LayerKind::Pixel { .. })
    ));
    let Some(aurora_doc::Adjustment::Curves(params)) = document.layers.adjustment(top) else {
        unreachable!("the top child is the Curves layer");
    };
    assert_eq!(
        params.red.as_ref().map(aurora_core::ToneCurve::input_range),
        Some((11.0 / 255.0, 238.0 / 255.0))
    );
    assert_eq!(document.layers.opacity(top), Some(128.0 / 255.0));
    assert_eq!(document.layers.blend_mode(top), Some(BlendMode::Multiply));
    assert_eq!(document.layers.visible(top), Some(false));
    assert!(document.layers.mask(top).is_some());
    assert!(document.masks.iter().any(|m| m.layer == top));
    let items = document.report.items.join("\n");
    assert!(!items.contains("adjustment layer"), "{items}");
    assert!(
        items.contains("1 Curves layer is inside a Pass Through group"),
        "{items}"
    );
}

/// AC-2/AC-3: the Pass Through note appears only for a Pass Through
/// parent; other adjustment kinds, a `curv` record that also carries one,
/// a Grayscale file's Curves and an unreadable or over-long `curv` stay
/// reported and left out.
#[test]
fn curves_reporting_covers_pass_through_other_kinds_grayscale_and_refusals() {
    let normal_group = curves_file(&curv_rgb_block(), |_| {}).with(|p| {
        if let Some(group) = p.layers.last_mut() {
            *group = TestLayer::group("G", *b"norm");
        }
    });
    let document = doc(&normal_group.write());
    assert!(document.report.is_empty(), "{:?}", document.report);

    let cases: [(&str, TestPsd, &str); 5] = [
        (
            "levels",
            curves_file(&curv_rgb_block(), |l| {
                l.blocks = vec![(*b"levl", vec![0; 4])];
            }),
            "1 adjustment layer (Levels, Hue/Saturation and similar) was left out",
        ),
        (
            "curv and levl",
            curves_file(&curv_rgb_block(), |l| l.blocks.push((*b"levl", vec![0; 4]))),
            "1 adjustment layer",
        ),
        (
            "too many points",
            curves_file(
                &curv(
                    1,
                    1,
                    &[&(0..20).map(|i| (i * 13, i * 13)).collect::<Vec<_>>()],
                    None,
                ),
                |_| {},
            ),
            "more than 19 points",
        ),
        (
            "unreadable",
            curves_file(&curv(4, 1, &[&[(0, 0), (255, 255)]], None), |_| {}),
            "settings could not be read",
        ),
        (
            "grayscale",
            TestPsd::new(1, 2, 2, 8).with(|p| {
                p.color_mode = 1;
                p.channels = 1;
                p.layers = vec![
                    TestLayer::gray(
                        "g",
                        0,
                        0,
                        2,
                        2,
                        8,
                        &[[1, 255], [2, 255], [3, 255], [4, 255]],
                    ),
                    TestLayer::empty("Curves 1").with(|l| {
                        l.channels.retain(|(id, _)| *id < 1);
                        l.blocks.push((
                            *b"curv",
                            curv(1, 2, &[&[(0, 0), (147, 184), (255, 255)]], None),
                        ));
                    }),
                ];
            }),
            "in this Grayscale file was left out",
        ),
    ];
    for (name, file, needle) in cases {
        let document = doc(&file.write());
        let items = document.report.items.join("\n");
        assert!(
            items.contains(needle),
            "{name}: {needle:?} missing from {items}"
        );
        assert!(
            !items.contains("Pass Through group, which Aurora doesn't have; the group"),
            "{name}: a left-out Curves layer is not in the group: {items}"
        );
        let mut pending = document.layers.roots().to_vec();
        let mut adjustments = 0;
        while let Some(id) = pending.pop() {
            adjustments += usize::from(document.layers.adjustment(id).is_some());
            pending.extend(document.layers.children(id).unwrap_or(&[]));
        }
        assert_eq!(adjustments, 0, "{name}");
    }
}
