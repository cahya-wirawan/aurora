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
            "mask-density-layervectormask.psd",
            "Layer 1",
            64,
            (15, 0, 32, 8),
            123.166_536,
        ),
        (
            "mask-density-layervectormask.psd",
            "Layer 1 copy",
            128,
            (15, 8, 32, 16),
            110.478_739,
        ),
        (
            "mask-density-layervectormask.psd",
            "Layer 1 copy 2",
            191,
            (15, 16, 32, 24),
            97.914_557,
        ),
        (
            "mask-density-layervectormask.psd",
            "Layer 1 copy 3",
            255,
            (15, 24, 32, 32),
            88.337_256,
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
            "mask_parameters.psd",
            "Rectangle 1",
            204,
            (23, 19, 185, 181),
            25_728.8,
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
