//! Lab, Indexed Color, Bitmap, Duotone and Multichannel PSD files
//! (task 148): synthetic files built in-test, the tracked psd-tools
//! `colormodes` fixtures (all of them the same 4 x 4 picture, so the RGB
//! twin `4x4_8bit_rgb.psd` is the independent truth for the picture), hostile
//! headers, and a sweep over the local corpus.

// Tests index fixed-size arrays they built and compare exact constants.
#![allow(
    clippy::indexing_slicing,
    clippy::float_cmp,
    clippy::many_single_char_names
)]

use half::f16;

use super::test_writer::{TestLayer, TestPsd};
use super::{PsdDocument, decode, merged_image, merged_image_in_working_space, read};
use crate::error::IoError;

macro_rules! fixture {
    ($name:literal) => {
        include_bytes!(concat!("../../tests/fixtures/psd/", $name)) as &[u8]
    };
}

fn open(bytes: &[u8]) -> PsdDocument {
    match read(bytes) {
        Ok(d) => d,
        Err(e) => unreachable!("{e:?}"),
    }
}

/// Every pixel of an image as `[r, g, b, a]` floats.
fn pixels(image: &crate::image::Image) -> Vec<[f32; 4]> {
    image
        .samples()
        .chunks_exact(4)
        .map(|c| [0, 1, 2, 3].map(|i| f16::to_f32(c[i])))
        .collect()
}

fn levels(p: [f32; 4]) -> [i32; 3] {
    [0, 1, 2].map(|i| (p[i].clamp(0.0, 1.0) * 255.0).round() as i32)
}

fn merged_levels(file: &[u8]) -> Vec<[i32; 3]> {
    match merged_image_in_working_space(file) {
        Ok(i) => pixels(&i).into_iter().map(levels).collect(),
        Err(e) => unreachable!("{e:?}"),
    }
}

fn first_layer(document: &PsdDocument) -> Vec<[f32; 4]> {
    match document.pixels.first() {
        Some(p) => pixels(&p.image),
        None => unreachable!("a layer"),
    }
}

fn report_text(document: &PsdDocument) -> String {
    document.report.items.join("\n")
}

fn near(a: [i32; 3], b: [i32; 3], tol: i32) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tol)
}

fn lab_file(depth: u16, planes: [&[u8]; 3]) -> TestPsd {
    TestPsd::new(1, planes[0].len() as u32 / if depth == 16 { 2 } else { 1 }, 1, depth).with(|p| {
        p.color_mode = 9;
        p.channels = 3;
        p.merged = Some(planes.iter().map(|plane| plane.to_vec()).collect());
    })
}

/// What `lab::lab_to_srgb` gives, as levels.
fn lab_levels(l: f64, a: f64, b: f64) -> [i32; 3] {
    super::lab::lab_to_srgb(l, a, b).map(|v| (f16::from_f32(v).to_f32() * 255.0).round() as i32)
}

// ---------------------------------------------------------------- Lab

#[test]
fn lab_8_bit_scaling_is_l_over_255_and_a_b_offset_128() {
    // Pixels: white, black, a mid grey, a saturated a*, a negative b*.
    let l = [255_u8, 0, 128, 128, 128];
    let a = [128_u8, 128, 128, 255, 128];
    let b = [128_u8, 128, 128, 128, 0];
    let file = lab_file(8, [&l, &a, &b]).write();
    let got = merged_levels(&file);
    assert_eq!(got[0], [255, 255, 255]);
    assert_eq!(got[1], [0, 0, 0]);
    let mid_l = 128.0 / 255.0 * 100.0;
    assert_eq!(got[2], lab_levels(mid_l, 0.0, 0.0));
    assert_eq!(got[3], lab_levels(mid_l, 127.0, 0.0));
    assert_eq!(got[4], lab_levels(mid_l, 0.0, -128.0));
    // The layer-less file opens as one Background layer with the same pixels.
    let document = open(&file);
    let layer: Vec<[i32; 3]> = first_layer(&document).into_iter().map(levels).collect();
    assert_eq!(layer, got);
    assert!(report_text(&document).contains("converted from Lab"));
}

#[test]
fn lab_16_bit_scaling_is_l_over_65535_and_a_b_minus_32768_over_256() {
    let be = |v: &[u16]| -> Vec<u8> { v.iter().flat_map(|x| x.to_be_bytes()).collect() };
    let l = be(&[65_535, 0, 32_768, 32_768, 32_768]);
    let a = be(&[32_768, 32_768, 32_768, 65_535, 32_768 + 256 * 20]);
    let b = be(&[32_768, 32_768, 32_768, 32_768, 0]);
    let file = lab_file(16, [&l, &a, &b]).write();
    let got = merged_levels(&file);
    assert_eq!(got[0], [255, 255, 255]);
    assert_eq!(got[1], [0, 0, 0]);
    let mid_l = 32_768.0 / 65_535.0 * 100.0;
    assert_eq!(got[2], lab_levels(mid_l, 0.0, 0.0));
    assert_eq!(got[3], lab_levels(mid_l, 32_767.0 / 256.0, 0.0));
    assert_eq!(got[4], lab_levels(mid_l, 20.0, -128.0));
}

#[test]
fn a_lab_layer_converts_its_planes_and_keeps_alpha() {
    // Layer samples are [L, a, b, alpha] per pixel in the writer's slots.
    let layer = TestLayer::pixels(
        "Lab layer",
        0,
        0,
        3,
        1,
        8,
        &[[255, 128, 128, 255], [128, 200, 100, 128], [90, 128, 128, 0]],
    );
    let file = TestPsd::new(1, 3, 1, 8)
        .with(|p| {
            p.color_mode = 9;
            p.layers.push(layer);
        })
        .write();
    let document = open(&file);
    let got = first_layer(&document);
    assert_eq!(levels(got[0]), [255, 255, 255]);
    assert_eq!(got[0][3], 1.0);
    assert_eq!(
        levels(got[1]),
        lab_levels(128.0 / 255.0 * 100.0, 72.0, -28.0)
    );
    assert_eq!(f16::from_f32(got[1][3]), f16::from_f32(128.0 / 255.0));
    // A fully transparent pixel is not converted.
    assert_eq!(got[2], [0.0, 0.0, 0.0, 0.0]);
}

#[test]
fn a_lab_layer_without_a_lightness_channel_is_white_not_black() {
    let layer = TestLayer::pixels("No L", 0, 0, 1, 1, 8, &[[0, 128, 128, 255]])
        .with(|l| l.channels.retain(|(id, _)| *id != 0));
    let file = TestPsd::new(1, 1, 1, 8)
        .with(|p| {
            p.color_mode = 9;
            p.layers.push(layer);
        })
        .write();
    assert_eq!(levels(first_layer(&open(&file))[0]), [255, 255, 255]);
}

#[test]
fn lab_headers_are_gated() {
    // Fewer than three channels, 32-bit and 1-bit Lab do not exist.
    let thin = TestPsd::new(1, 1, 1, 8)
        .with(|p| {
            p.color_mode = 9;
            p.channels = 2;
        })
        .write();
    assert!(matches!(decode(&thin), Err(IoError::PsdMalformed { .. })));
    for depth in [1_u16, 32] {
        let file = TestPsd::new(1, 1, 1, depth)
            .with(|p| p.color_mode = 9)
            .write();
        assert!(
            matches!(decode(&file), Err(IoError::UnsupportedPsdDepth(d)) if d == depth),
            "depth {depth}"
        );
    }
}

#[test]
fn a_truncated_lab_file_is_an_error_not_a_panic() {
    let l = [10_u8, 20, 30, 40];
    let a = [128_u8; 4];
    let b = [128_u8; 4];
    let file = lab_file(8, [&l, &a, &b]).write();
    for cut in [file.len() - 1, file.len() - 6, file.len() - 12] {
        let short = file.get(..cut).unwrap_or_default();
        assert!(merged_image(short).is_err(), "cut {cut}");
        assert!(read(short).is_err(), "cut {cut}");
    }
}

#[test]
fn lab_fill_layer_colours_are_read_as_lab() {
    // A Lab-mode gradient's stops are `Lmnc`/`A   `/`B   ` descriptors.
    let document = open(fixture!("4x4_16bit_lab.psd"));
    assert!(report_text(&document).contains("gradient"));
    let block = super::effects_tests::raw_blocks(fixture!("4x4_16bit_lab.psd"), b"GdFl")
        .into_iter()
        .next()
        .unwrap_or_default();
    let Ok(super::fill::Fill::Gradient(g)) = super::fill::parse_in(*b"GdFl", &block, None) else {
        unreachable!("a Lab gradient")
    };
    let lv = |c: [f32; 3]| c.map(|v| (f16::from_f32(v).to_f32() * 255.0).round() as i32);
    let first = lv(g.at(0.0).0);
    let last = lv(g.at(1.0).0);
    // Stops: Lab (19.07, 52.29, -85.08) and (96.84, -14.39, 92.87).
    assert_eq!(first, lab_levels(19.07, 52.29, -85.08));
    assert_eq!(last, lab_levels(96.84, -14.39, 92.87));
    // ... and Photoshop's own sRGB twin has the blue at (10, 0, 178).
    assert!(near(first, [10, 0, 178], 2), "{first:?}");
}

/// psd-tools' `4x4_*_lab.psd` are the picture of `4x4_8bit_rgb.psd` with
/// the gradient blended in Lab: the stops (pixels 0 and 2) must agree with
/// the RGB twin, the blended pixels (1, 3) legitimately differ. Pixel 1 is
/// pinned from Pillow's `ImageCms` (lcms2, relative colorimetric, black
/// point compensation) run on the same stored samples: (170, 0, 99) for
/// 8-bit and (170, 0, 97) for 16-bit.
#[test]
fn the_real_lab_fixtures_match_the_rgb_twin_at_its_stops_and_lcms2_inside() {
    let twin = merged_levels(fixture!("4x4_8bit_rgb.psd"));
    assert_eq!(twin[0], [10, 0, 178]);
    let lab16 = merged_levels(fixture!("4x4_16bit_lab.psd"));
    assert!(near(lab16[0], twin[0], 1), "{:?}", lab16[0]);
    assert!(near(lab16[2], twin[2], 1), "{:?}", lab16[2]);
    assert!(near(lab16[1], [170, 0, 97], 1), "{:?}", lab16[1]);
    let lab8 = merged_levels(fixture!("4x4_8bit_lab.psd"));
    // 8-bit samples are coarser and the blue sits on the gamut edge.
    assert!(near(lab8[0], twin[0], 12), "{:?}", lab8[0]);
    assert!(near(lab8[2], twin[2], 8), "{:?}", lab8[2]);
    assert!(near(lab8[1], [170, 0, 99], 1), "{:?}", lab8[1]);
    // The 8-bit file's layer is a pixel layer and equals its merged image.
    let layer: Vec<[i32; 3]> = first_layer(&open(fixture!("4x4_8bit_lab.psd")))
        .into_iter()
        .map(levels)
        .collect();
    assert_eq!(layer, lab8);
}

// ---------------------------------------------------------- Indexed

fn palette_bytes(colours: &[[u8; 3]]) -> Vec<u8> {
    let mut table = vec![0_u8; 768];
    for (i, [r, g, b]) in colours.iter().enumerate() {
        table[i] = *r;
        table[256 + i] = *g;
        table[512 + i] = *b;
    }
    table
}

fn resource(id: u16, body: &[u8]) -> Vec<u8> {
    let mut o = Vec::new();
    o.extend_from_slice(b"8BIM");
    o.extend_from_slice(&id.to_be_bytes());
    o.extend_from_slice(&[0, 0]);
    o.extend_from_slice(&u32::try_from(body.len()).unwrap_or(0).to_be_bytes());
    o.extend_from_slice(body);
    if body.len() % 2 == 1 {
        o.push(0);
    }
    o
}

fn indexed_file(indices: &[u8], transparent: Option<u16>) -> TestPsd {
    TestPsd::new(1, indices.len() as u32, 1, 8).with(|p| {
        p.color_mode = 2;
        p.channels = 1;
        p.color_mode_data = palette_bytes(&[[10, 20, 30], [200, 100, 50], [255, 255, 255]]);
        p.merged = Some(vec![indices.to_vec()]);
        if let Some(t) = transparent {
            p.resources = resource(1047, &t.to_be_bytes());
        }
    })
}

#[test]
fn indexed_samples_index_the_three_plane_table() {
    let file = indexed_file(&[0, 1, 2, 1], None).write();
    let got = merged_levels(&file);
    assert_eq!(
        got,
        vec![[10, 20, 30], [200, 100, 50], [255, 255, 255], [200, 100, 50]]
    );
    let document = open(&file);
    assert!(report_text(&document).contains("indexed colour table"));
    assert!(first_layer(&document).iter().all(|p| p[3] == 1.0));
}

#[test]
fn the_transparency_index_makes_a_pixel_transparent() {
    let file = indexed_file(&[0, 1, 2], Some(1)).write();
    let image = merged_image(&file).map(|i| pixels(&i));
    let Ok(got) = image else {
        unreachable!("opens")
    };
    assert_eq!(got[1], [0.0, 0.0, 0.0, 0.0]);
    assert_eq!(got[0][3], 1.0);
    assert_eq!(got[2][3], 1.0);
    // An index past 255 names nothing.
    let none = indexed_file(&[0, 1, 2], Some(300)).write();
    assert!(
        pixels(&merged_image(&none).unwrap_or_else(|e| unreachable!("{e:?}")))
            .iter()
            .all(|p| p[3] == 1.0)
    );
}

#[test]
fn an_indexed_file_needs_exactly_768_table_bytes_and_no_layers() {
    for len in [0_usize, 767, 769] {
        let file = indexed_file(&[0], None)
            .with(|p| p.color_mode_data = vec![0; len])
            .write();
        assert!(
            matches!(decode(&file), Err(IoError::PsdMalformed { .. })),
            "{len}"
        );
        assert!(
            matches!(merged_image(&file), Err(IoError::PsdMalformed { .. })),
            "{len}"
        );
    }
    let layered = indexed_file(&[0], None)
        .with(|p| {
            p.layers
                .push(TestLayer::pixels("L", 0, 0, 1, 1, 8, &[[0, 0, 0, 255]]));
        })
        .write();
    assert!(matches!(
        decode(&layered),
        Err(IoError::PsdMalformed { .. })
    ));
    for depth in [1_u16, 16, 32] {
        let file = TestPsd::new(1, 1, 1, depth)
            .with(|p| {
                p.color_mode = 2;
                p.channels = 1;
            })
            .write();
        assert!(
            matches!(decode(&file), Err(IoError::UnsupportedPsdDepth(d)) if d == depth),
            "depth {depth}"
        );
    }
}

#[test]
fn a_truncated_indexed_file_is_an_error() {
    let file = indexed_file(&[0, 1, 2, 1], None).write();
    for cut in [file.len() - 1, file.len() - 3] {
        assert!(read(file.get(..cut).unwrap_or_default()).is_err());
    }
}

#[test]
fn the_real_indexed_fixture_equals_its_rgb_twin_exactly() {
    let twin = merged_levels(fixture!("4x4_8bit_rgb.psd"));
    let indexed = merged_levels(fixture!("4x4_8bit_index_color.psd"));
    // The table holds the twin's colours; the 133/132 pair is the twin's
    // own gradient noise, the table has 221 entries for 16 pixels.
    for (i, (a, b)) in indexed.iter().zip(&twin).enumerate() {
        assert!(near(*a, *b, 1), "pixel {i}: {a:?} vs {b:?}");
    }
    assert_eq!(indexed[0], [10, 0, 178]);
    assert_eq!(indexed[3], [255, 126, 0]);
}

// ----------------------------------------------------------- Bitmap

fn bitmap_file(width: u32, rows: &[&[u8]]) -> TestPsd {
    TestPsd::new(1, width, rows.len() as u32, 1).with(|p| {
        p.color_mode = 0;
        p.channels = 1;
        p.merged = Some(vec![rows.concat()]);
    })
}

#[test]
fn a_set_bit_is_black_and_bits_run_most_significant_first() {
    // 10 pixels wide: two bytes a row, the last six bits padding.
    let file = bitmap_file(10, &[&[0b1010_0000, 0b0100_0000], &[0, 0]]).write();
    let got = merged_levels(&file);
    let row0: Vec<i32> = got[..10].iter().map(|p| p[0]).collect();
    assert_eq!(row0, [0, 255, 0, 255, 255, 255, 255, 255, 255, 0]);
    assert!(got[10..].iter().all(|p| *p == [255, 255, 255]));
    assert!(got.iter().all(|p| p[0] == p[1] && p[1] == p[2]));
    // RLE-compressed rows decode the same.
    let rle = bitmap_file(10, &[&[0b1010_0000, 0b0100_0000], &[0, 0]])
        .with(|p| p.merged_compression = 1)
        .write();
    assert_eq!(merged_levels(&rle), got);
    let document = open(&file);
    assert!(report_text(&document).contains("Bitmap"));
}

#[test]
fn bitmap_gates_depth_compression_layers_and_truncation() {
    let ok = bitmap_file(8, &[&[0xF0]]);
    for depth in [8_u16, 16, 32] {
        let file = ok.clone().with(|p| p.depth = depth).write();
        assert!(
            matches!(decode(&file), Err(IoError::UnsupportedPsdDepth(d)) if d == depth),
            "depth {depth}"
        );
    }
    // Prediction is not defined on bits.
    let predicted = ok.clone().with(|p| p.merged_compression = 3).write();
    assert!(matches!(
        decode(&predicted),
        Err(IoError::UnsupportedPsdCompression(3))
    ));
    let layered = ok
        .clone()
        .with(|p| {
            p.layers
                .push(TestLayer::pixels("L", 0, 0, 1, 1, 8, &[[0, 0, 0, 255]]));
        })
        .write();
    assert!(matches!(
        decode(&layered),
        Err(IoError::PsdMalformed { .. })
    ));
    let file = ok.write();
    assert!(read(file.get(..file.len() - 1).unwrap_or_default()).is_err());
}

#[test]
fn the_real_bitmap_fixture_has_psd_tools_bits() {
    // psd-tools 1.17.4 reads rows [192, 240, 112, 48] with a set bit black.
    let got: Vec<i32> = merged_levels(fixture!("4x4_1bit_bitmap.psd"))
        .iter()
        .map(|p| p[0])
        .collect();
    let b = 0;
    let w = 255;
    assert_eq!(
        got,
        [b, b, w, w, b, b, b, b, w, b, b, b, w, w, b, b]
    );
}

// ------------------------------------------------ Duotone, Multichannel

#[test]
fn duotone_and_multichannel_show_their_first_channel_as_grey() {
    for (mode, channels) in [(8_u16, 1_u16), (7, 1), (7, 5)] {
        let planes: Vec<Vec<u8>> = (0..channels).map(|c| vec![0, 64, 255 - c as u8]).collect();
        let file = TestPsd::new(1, 3, 1, 8)
            .with(|p| {
                p.color_mode = mode;
                p.channels = channels;
                p.merged = Some(planes.clone());
            })
            .write();
        let got = merged_levels(&file);
        assert_eq!(got, vec![[0, 0, 0], [64, 64, 64], [255, 255, 255]], "{mode}");
        let document = open(&file);
        let text = report_text(&document);
        assert!(
            text.contains(if mode == 8 { "Duotone" } else { "Multichannel" }),
            "{text}"
        );
    }
}

#[test]
fn a_duotone_layer_is_read_as_a_grey_plane() {
    let layer = TestLayer::pixels("D", 0, 0, 2, 1, 8, &[[0, 0, 0, 255], [200, 0, 0, 128]]);
    // Only channel 0 (and alpha) are read; the writer's 1 and 2 are unknown.
    let file = TestPsd::new(1, 2, 1, 8)
        .with(|p| {
            p.color_mode = 8;
            p.channels = 1;
            p.layers.push(layer);
        })
        .write();
    let got = first_layer(&open(&file));
    assert_eq!(levels(got[1]), [200, 200, 200]);
    assert_eq!(f16::from_f32(got[1][3]), f16::from_f32(128.0 / 255.0));
}

#[test]
fn duotone_and_multichannel_depth_gates() {
    for mode in [7_u16, 8] {
        for depth in [1_u16, 32] {
            let file = TestPsd::new(1, 1, 1, depth)
                .with(|p| {
                    p.color_mode = mode;
                    p.channels = 1;
                })
                .write();
            assert!(
                matches!(decode(&file), Err(IoError::UnsupportedPsdDepth(d)) if d == depth),
                "mode {mode} depth {depth}"
            );
        }
        let sixteen = TestPsd::new(1, 1, 1, 16)
            .with(|p| {
                p.color_mode = mode;
                p.channels = 1;
            })
            .write();
        assert!(decode(&sixteen).is_ok());
    }
}

#[test]
fn the_real_duotone_and_multichannel_fixtures_open_as_grey() {
    let duo = merged_levels(fixture!("4x4_8bit_duotone.psd"));
    assert_eq!(duo[0], [24, 24, 24]);
    assert_eq!(duo[3], [172, 172, 172]);
    let multi = merged_levels(fixture!("4x4_16bit_multichannel.psd"));
    // 16-bit channel 1: 12498 / 65535 of full scale.
    assert_eq!(multi[0], [49, 49, 49]);
    assert_eq!(multi[3], [198, 198, 198]);
    let text = report_text(&open(fixture!("4x4_16bit_multichannel.psd")));
    assert!(text.contains("Multichannel"), "{text}");
    assert!(text.contains("2 saved channels"), "{text}");
}

// ------------------------------------------------------ Refusals, corpus

#[test]
fn unassigned_colour_modes_are_still_refused_by_name() {
    for mode in [5_u16, 6, 10, 255] {
        let file = TestPsd::new(1, 1, 1, 8)
            .with(|p| p.color_mode = mode)
            .write();
        assert!(
            matches!(decode(&file), Err(IoError::UnsupportedPsdColorMode(m)) if m == mode),
            "mode {mode}"
        );
    }
}

/// Every corpus file of the five modes opens (the local corpus only).
#[test]
fn every_corpus_file_of_the_five_modes_opens() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for dir in ["corpora/psd", "images"] {
        let mut stack = vec![root.join(dir)];
        while let Some(path) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&path) else {
                continue;
            };
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|e| {
                    e.eq_ignore_ascii_case("psd") || e.eq_ignore_ascii_case("psb")
                }) {
                    files.push(p);
                }
            }
        }
    }
    let mut opened = 0;
    for path in &files {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let mode = bytes
            .get(24..26)
            .map(|m| u16::from_be_bytes([m[0], m[1]]));
        if !matches!(mode, Some(0 | 2 | 7 | 8 | 9)) {
            continue;
        }
        assert!(read(&bytes).is_ok(), "{} did not open", path.display());
        assert!(merged_image(&bytes).is_ok(), "{} merged", path.display());
        opened += 1;
    }
    if files.is_empty() {
        println!("SKIPPED: no PSD corpus present");
        assert!(
            std::env::var_os("AURORA_REQUIRE_CORPUS").is_none(),
            "AURORA_REQUIRE_CORPUS is set and the PSD corpus is missing"
        );
    } else {
        println!("{opened} corpus files of modes 0, 2, 7, 8, 9 opened");
        assert!(
            opened == 0 || opened == 7,
            "expected 7 corpus files of those modes, found {opened}"
        );
    }
}
