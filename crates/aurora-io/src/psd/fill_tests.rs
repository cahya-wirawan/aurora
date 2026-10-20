// ---- task 108: fill layers that store no pixels, gradients, artboards ----
//
// Included at the end of `tests.rs` (uses `doc`, `root`, `report_text`,
// `px`, `f`). The corpus comparisons against Photoshop's merged images
// live in `aurora-app`'s `shape_fidelity_tests.rs`.

use super::effects_tests::{V, descriptor_bytes, rgb};
use super::fill::{self, Fill, LeftOut, Style};

/// A `SoCo`/`GdFl`/`artb` block: descriptor version 16 then the body.
fn fill_block(items: &[(&[u8], V<'_>)]) -> Vec<u8> {
    let mut out = 16_u32.to_be_bytes().to_vec();
    out.extend_from_slice(&descriptor_bytes(items));
    out
}

fn solid_block(r: f64, g: f64, b: f64) -> Vec<u8> {
    fill_block(&[(b"Clr ", rgb(r, g, b))])
}

fn colour_stop(r: f64, g: f64, b: f64, location: i32, mid: i32) -> V<'static> {
    V::Obj(vec![
        (b"Clr ", rgb(r, g, b)),
        (b"Lctn", V::Long(location)),
        (b"Mdpn", V::Long(mid)),
    ])
}

fn alpha_stop(opacity: f64, location: i32) -> V<'static> {
    V::Obj(vec![
        (b"Opct", V::Unit(b"#Prc", opacity)),
        (b"Lctn", V::Long(location)),
        (b"Mdpn", V::Long(50)),
    ])
}

/// A custom-stops gradient block of the given style, angle and scale.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn gradient_block(
    style: &'static [u8; 4],
    angle: f64,
    scale: f64,
    reverse: bool,
    colours: Vec<V<'static>>,
    alphas: Vec<V<'static>>,
) -> Vec<u8> {
    fill_block(&[
        (b"Angl", V::Unit(b"#Ang", angle)),
        (b"Type", V::Enum(b"GrdT", style)),
        (b"Scl ", V::Unit(b"#Prc", scale)),
        (b"Rvrs", V::Bool(reverse)),
        (
            b"Grad",
            V::Obj(vec![
                (b"GrdF", V::Enum(b"GrdF", b"CstS")),
                (b"Clrs", V::List(colours)),
                (b"Trns", V::List(alphas)),
            ]),
        ),
    ])
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn black_to_white(style: &'static [u8; 4], angle: f64, scale: f64, reverse: bool) -> Vec<u8> {
    gradient_block(
        style,
        angle,
        scale,
        reverse,
        vec![
            colour_stop(0.0, 0.0, 0.0, 0, 50),
            colour_stop(255.0, 255.0, 255.0, 4096, 50),
        ],
        vec![alpha_stop(100.0, 0), alpha_stop(100.0, 4096)],
    )
}

fn parsed_gradient(block: &[u8]) -> fill::Gradient {
    match fill::parse(*b"GdFl", block) {
        Ok(Fill::Gradient(g)) => g,
        other => unreachable!("not a gradient: {other:?}"),
    }
}

fn square(side: u32) -> aurora_core::Rect {
    aurora_core::Rect { x: 0, y: 0, width: side, height: side }
}

/// The straight RGBA the opened document holds at document `(x, y)` for
/// the only pixel-bearing layer (`None` outside its image).
fn fill_px(document: &PsdDocument, x: i64, y: i64) -> Option<[f32; 4]> {
    let placed = document.pixels.first()?;
    let origin = document.layers.bounds(placed.layer)?;
    let ix = x - origin.x - i64::from(placed.offset.0);
    let iy = y - origin.y - i64::from(placed.offset.1);
    if ix < 0 || iy < 0 || ix >= i64::from(placed.image.width()) || iy >= i64::from(placed.image.height()) {
        return None;
    }
    Some(px(&placed.image, u32::try_from(ix).ok()?, u32::try_from(iy).ok()?))
}

fn empty_fill_file(width: u32, height: u32, key: [u8; 4], block: Vec<u8>, vector: Option<Vec<u8>>) -> TestPsd {
    TestPsd::new(1, width, height, 8).with(|p| {
        p.layers.push(TestLayer::empty("fill").with(|l| {
            l.blocks.push((key, block));
            if let Some(v) = vector {
                l.blocks.push((*b"vmsk", v));
            }
        }));
    })
}

#[test]
fn a_solid_fill_without_pixels_covers_its_vector_shape_with_the_fill_colour() {
    let file = empty_fill_file(
        8,
        8,
        *b"SoCo",
        solid_block(255.0, 0.0, 51.0),
        Some(vmsk(0, &rect_path(0.25, 0.25, 0.75, 0.75, 1))),
    );
    let document = doc(&file.write());
    assert_eq!(document.layers.len(), 1);
    let want = u8px(255, 0, 51, 255);
    assert_eq!(fill_px(&document, 4, 4), Some(want), "inside the shape");
    // Outside the shape the layer is transparent, or has no pixel at all.
    assert!(fill_px(&document, 0, 0).is_none_or(|p| p[3] == 0.0));
    assert!(fill_px(&document, 7, 7).is_none_or(|p| p[3] == 0.0));
    // The shape's anti-aliased edge: a half-covered column has partial alpha.
    let edge = fill_px(&document, 1, 4).map_or(0.0, |p| p[3]);
    assert!(edge < 1.0, "edge alpha {edge}");
    let text = report_text(&document);
    assert!(text.contains("stored no pixels"), "{text}");
    assert!(text.contains("opened as ordinary pixels"), "{text}");
    assert!(!text.contains("left out"), "{text}");
}

#[test]
fn a_solid_fill_with_no_vector_mask_covers_the_canvas() {
    let file = empty_fill_file(4, 3, *b"SoCo", solid_block(0.0, 255.0, 0.0), None);
    let document = doc(&file.write());
    for (x, y) in [(0, 0), (3, 2), (2, 1)] {
        assert_eq!(fill_px(&document, x, y), Some(u8px(0, 255, 0, 255)), "{x},{y}");
    }
}

#[test]
fn a_disabled_vector_mask_fills_the_whole_canvas() {
    // Flags bit 2: disabled.
    let file = empty_fill_file(
        8,
        8,
        *b"SoCo",
        solid_block(10.0, 20.0, 30.0),
        Some(vmsk(4, &rect_path(0.25, 0.25, 0.75, 0.75, 1))),
    );
    let document = doc(&file.write());
    assert_eq!(fill_px(&document, 0, 0), Some(u8px(10, 20, 30, 255)));
    assert_eq!(fill_px(&document, 7, 7), Some(u8px(10, 20, 30, 255)));
    let text = report_text(&document);
    assert!(text.contains("vector mask that was turned off"), "{text}");
}

#[test]
fn an_unreadable_vector_mask_fills_the_canvas_and_says_so() {
    let file = empty_fill_file(
        4,
        4,
        *b"SoCo",
        solid_block(1.0, 2.0, 3.0),
        Some(vec![0; 4]),
    );
    let document = doc(&file.write());
    assert_eq!(fill_px(&document, 3, 3), Some(u8px(1, 2, 3, 255)));
    let text = report_text(&document);
    assert!(text.contains("vector mask that could not be read"), "{text}");
}

#[test]
fn a_vector_mask_density_lets_the_hidden_area_show_through_the_baked_fill() {
    // Flags bit 4 with a parameter block naming only a vector density of
    // 128: coverage becomes `d * c + (1 - d)`, so outside the shape the
    // fill keeps `1 - d` of its opacity and the layer spans the canvas.
    let mut file = empty_fill_file(
        8,
        8,
        *b"SoCo",
        solid_block(255.0, 0.0, 0.0),
        Some(vmsk(0, &rect_path(0.25, 0.25, 0.75, 0.75, 1))),
    );
    if let Some(layer) = file.layers.first_mut() {
        layer.mask = Some((0, 0, 0, 0, 0, 0x10));
        layer.mask_tail = Some(vec![0x04, 128]);
    }
    let document = doc(&file.write());
    let d = 128.0_f32 / 255.0;
    let inside = fill_px(&document, 4, 4).map_or(f32::NAN, |p| p[3]);
    let outside = fill_px(&document, 0, 0).map_or(f32::NAN, |p| p[3]);
    assert_eq!(inside, f(1.0));
    assert_eq!(outside, f(1.0 - d));
    let text = report_text(&document);
    assert!(!text.contains("density"), "applied, so not reported: {text}");
}

#[test]
fn a_vector_mask_feather_on_a_baked_fill_is_reported() {
    let mut file = empty_fill_file(
        8,
        8,
        *b"SoCo",
        solid_block(255.0, 0.0, 0.0),
        Some(vmsk(0, &rect_path(0.25, 0.25, 0.75, 0.75, 1))),
    );
    if let Some(layer) = file.layers.first_mut() {
        layer.mask = Some((0, 0, 0, 0, 0, 0x10));
        let mut tail = vec![0x08];
        tail.extend_from_slice(&1.5_f64.to_be_bytes());
        layer.mask_tail = Some(tail);
    }
    let text = report_text(&doc(&file.write()));
    assert!(text.contains("feather"), "{text}");
}

#[test]
fn a_rendered_mask_channel_is_not_applied_a_second_time_to_a_baked_fill() {
    // Flags bit 3: `-2` is Photoshop's rendering of the vector mask (here a
    // deliberately all-hidden plane). The vector mask is baked into the
    // pixels, so applying the plane again would hide the whole fill.
    let mut file = empty_fill_file(
        8,
        8,
        *b"SoCo",
        solid_block(255.0, 0.0, 0.0),
        Some(vmsk(0, &rect_path(0.25, 0.25, 0.75, 0.75, 1))),
    );
    if let Some(layer) = file.layers.first_mut() {
        layer.mask = Some((0, 0, 8, 8, 0, 0x08));
        layer.channels.push((-2, vec![0; 64]));
    }
    let document = doc(&file.write());
    let id = root(&document.layers, 0);
    assert!(document.layers.mask(id).is_none(), "no mask on top of the bake");
    assert_eq!(fill_px(&document, 4, 4), Some(u8px(255, 0, 0, 255)));
    // The genuine user mask (`-3`, shown for x < 4 only) still applies.
    let mut file = empty_fill_file(
        8,
        8,
        *b"SoCo",
        solid_block(255.0, 0.0, 0.0),
        Some(vmsk(0, &rect_path(0.25, 0.25, 0.75, 0.75, 1))),
    );
    if let Some(layer) = file.layers.first_mut() {
        layer.mask = Some((0, 0, 8, 8, 0, 0x18));
        let mut tail = vec![0x00, 0];
        for v in [0_i32, 0, 8, 4] {
            tail.extend_from_slice(&v.to_be_bytes());
        }
        tail.extend_from_slice(&[0x00, 0]);
        layer.mask_tail = Some(tail);
        layer.channels.push((-2, vec![0; 64]));
        layer.channels.push((-3, vec![255; 32]));
    }
    let document = doc(&file.write());
    let id = root(&document.layers, 0);
    assert_eq!(effective_mask(&document, id, 3, 3), Some(1.0));
    assert_eq!(effective_mask(&document, id, 5, 3), Some(0.0));
}

#[test]
fn a_fill_layer_that_stores_pixels_keeps_them() {
    let file = TestPsd::new(1, 2, 2, 8).with(|p| {
        p.layers.push(
            TestLayer::pixels("kept", 0, 0, 1, 1, 8, &[[9, 8, 7, 255]]).with(|l| {
                l.blocks.push((*b"SoCo", solid_block(255.0, 0.0, 0.0)));
            }),
        );
    });
    let document = doc(&file.write());
    assert_eq!(fill_px(&document, 0, 0), Some(u8px(9, 8, 7, 255)));
    assert!(report_text(&document).contains("fill layer"), "still a fill layer");
}

#[test]
fn a_gradient_fill_without_pixels_is_drawn_and_called_approximate() {
    // Black to white, left to right (angle 0), over a 10 px canvas.
    let file = empty_fill_file(10, 2, *b"GdFl", black_to_white(b"Lnr ", 0.0, 100.0, false), None);
    let document = doc(&file.write());
    let left = fill_px(&document, 0, 0).map_or(f32::NAN, |p| p[0]);
    let right = fill_px(&document, 9, 0).map_or(f32::NAN, |p| p[0]);
    assert!(left < 0.1 && right > 0.85, "{left} {right}");
    let text = report_text(&document);
    assert!(text.contains("gradient fill layer"), "{text}");
    assert!(text.contains("compared with only two"), "{text}");
}

#[test]
fn pattern_noise_and_unknown_style_fills_stay_left_out_with_their_reason() {
    let noise = fill_block(&[
        (b"Angl", V::Unit(b"#Ang", 0.0)),
        (b"Grad", V::Obj(vec![(b"GrdF", V::Enum(b"GrdF", b"ClNs"))])),
    ]);
    let shapeburst = gradient_block(
        b"Shpb",
        0.0,
        100.0,
        false,
        vec![colour_stop(0.0, 0.0, 0.0, 0, 50)],
        vec![],
    );
    for (key, block, why) in [
        (*b"PtFl", vec![0_u8; 8], "pattern fill"),
        (*b"GdFl", noise, "Noise gradient"),
        (*b"GdFl", shapeburst, "gradient style"),
        (*b"SoCo", vec![0_u8; 4], "could not be read"),
    ] {
        let document = doc(&empty_fill_file(4, 4, key, block, None).write());
        let text = report_text(&document);
        assert!(!text.contains("stored no pixels"), "{why}: drawn after all: {text}");
        assert!(text.contains("left out") && text.contains(why), "{why}: {text}");
    }
}

#[test]
fn fill_blocks_parse_exactly() {
    assert_eq!(
        fill::parse(*b"SoCo", &solid_block(255.0, 51.0, 0.0)),
        Ok(Fill::Solid([1.0, 0.2, 0.0]))
    );
    let g = parsed_gradient(&black_to_white(b"Rflc", 30.0, 200.0, true));
    assert_eq!(g.style, Style::Reflected);
    assert_eq!((g.angle, g.scale, g.reverse, g.align), (30.0, 2.0, true, true));
    assert_eq!(g.colours.len(), 2);
    assert_eq!(g.colours.get(1).map(|s| (s.pos, s.value)), Some((1.0, [1.0; 3])));
    assert_eq!(g.alphas.len(), 2);
    // Every style enum.
    for (key, style) in [
        (b"Lnr ", Style::Linear),
        (b"Rdl ", Style::Radial),
        (b"Angl", Style::Angle),
        (b"Rflc", Style::Reflected),
        (b"Dmnd", Style::Diamond),
    ] {
        assert_eq!(parsed_gradient(&black_to_white(key, 0.0, 100.0, false)).style, style);
    }
}

#[test]
fn a_grayscale_stop_is_ink_percent_so_100_is_black() {
    let gray = |v: f64| V::Obj(vec![(b"Gry ", V::Doub(v))]);
    let block = fill_block(&[(b"Clr ", gray(90.646_679_699_420_93))]);
    let Ok(Fill::Solid(c)) = fill::parse(*b"SoCo", &block) else {
        unreachable!("grey solid")
    };
    // `4x4_16bit_grayscale.psd`: 90.6 % ink shows as level 24.
    assert_eq!((c[0] * 255.0).round() as i32, 24);
    assert!(c.iter().all(|v| (*v - c[0]).abs() < 1e-6));
}

#[test]
fn hostile_fill_blocks_are_refused_without_a_panic() {
    assert_eq!(fill::parse(*b"SoCo", &[]), Err(LeftOut::Unreadable));
    assert_eq!(
        fill::parse(*b"GdFl", &vec![0; fill::MAX_FILL_BLOCK_BYTES + 1]),
        Err(LeftOut::TooLarge)
    );
    let many: Vec<V<'static>> = (0..=fill::MAX_STOPS).map(|_| colour_stop(0.0, 0.0, 0.0, 0, 50)).collect();
    assert_eq!(
        fill::parse(*b"GdFl", &gradient_block(b"Lnr ", 0.0, 100.0, false, many, vec![])),
        Err(LeftOut::TooLarge)
    );
    assert_eq!(
        fill::parse(*b"GdFl", &black_to_white(b"Lnr ", 0.0, 0.0, false)),
        Err(LeftOut::Unreadable),
        "a zero scale"
    );
    assert_eq!(
        fill::parse(*b"GdFl", &gradient_block(b"Lnr ", 0.0, 100.0, false, vec![], vec![])),
        Err(LeftOut::Unreadable),
        "no colour stops"
    );
    // Truncations of a good block never panic.
    let good = black_to_white(b"Lnr ", 0.0, 100.0, false);
    for cut in 0..good.len() {
        let _ = fill::parse(*b"GdFl", good.get(..cut).unwrap_or(&[]));
    }
}

#[test]
fn gradient_positions_follow_the_geometry() {
    let b = square(32);
    let near = |a: f32, want: f32| assert!((a - want).abs() < 1e-5, "{a} != {want}");
    // Linear, angle 0: left to right through the box centre (16, 16).
    let g = parsed_gradient(&black_to_white(b"Lnr ", 0.0, 100.0, false));
    near(g.position(16.0, 16.0, b), 0.5);
    near(g.position(0.0, 16.0, b), 0.0);
    near(g.position(32.0, 5.0, b), 1.0);
    near(g.position(8.0, 0.0, b), 0.25);
    // Angle 180 runs the other way; the picture is vertical-invariant.
    let g = parsed_gradient(&black_to_white(b"Lnr ", 180.0, 100.0, false));
    near(g.position(8.0, 3.0, b), 0.75);
    // Angle 90 is bottom to top (image rows grow downwards).
    let g = parsed_gradient(&black_to_white(b"Lnr ", 90.0, 100.0, false));
    near(g.position(16.0, 24.0, b), 0.25);
    // Scale 200 % doubles the length.
    let g = parsed_gradient(&black_to_white(b"Lnr ", 0.0, 200.0, false));
    near(g.position(8.0, 0.0, b), 0.375);
    // Reverse flips.
    let g = parsed_gradient(&black_to_white(b"Lnr ", 0.0, 100.0, true));
    near(g.position(8.0, 0.0, b), 0.75);
    // Reflected: symmetric about the centre line, 0 on it.
    let g = parsed_gradient(&black_to_white(b"Rflc", 0.0, 100.0, false));
    near(g.position(16.0, 3.0, b), 0.0);
    near(g.position(8.0, 3.0, b), 0.5);
    near(g.position(24.0, 3.0, b), 0.5);
    // Radial (fitted to Photoshop's pixels, task 147): `EDGE_OFFSET` at the
    // centre, 1 at the corner.
    let g = parsed_gradient(&black_to_white(b"Rdl ", 0.0, 100.0, false));
    near(g.position(16.0, 16.0, b), aurora_core::gradient_fill::EDGE_OFFSET as f32);
    near(g.position(32.0, 32.0, b), 1.0);
    // An edge midpoint: the fitted radius is `RADIAL_FACTOR` of the nominal.
    near(g.position(32.0, 16.0, b), 0.806_570);
    // Diamond (same radius as Radial times its factor): 1 at a corner of a
    // square box, `EDGE_OFFSET + 16 / (DIAMOND_FACTOR 16 sqrt 2)` at an
    // edge midpoint.
    let g = parsed_gradient(&black_to_white(b"Dmnd", 0.0, 100.0, false));
    near(g.position(32.0, 32.0, b), 1.0);
    near(g.position(32.0, 16.0, b), 0.838_594);
    // The box offset moves the centre (percent of the box).
    let mut moved = parsed_gradient(&black_to_white(b"Lnr ", 0.0, 100.0, false));
    moved.offset = (25.0, 0.0);
    near(moved.position(24.0, 0.0, b), 0.5);
}

#[test]
fn colour_and_opacity_ramps_clamp_sort_and_bend_at_the_midpoint() {
    // Stops given out of order are sorted; outside them the end values hold.
    let g = parsed_gradient(&gradient_block(
        b"Lnr ",
        0.0,
        100.0,
        false,
        vec![
            colour_stop(255.0, 0.0, 0.0, 3072, 50),
            colour_stop(0.0, 0.0, 255.0, 1024, 50),
        ],
        vec![alpha_stop(100.0, 0), alpha_stop(0.0, 4096)],
    ));
    assert_eq!(g.at(0.0).0, [0.0, 0.0, 1.0]);
    assert_eq!(g.at(1.0).0, [1.0, 0.0, 0.0]);
    let (mid, alpha) = g.at(0.5);
    assert!((mid[0] - 0.5).abs() < 1e-6 && (mid[2] - 0.5).abs() < 1e-6, "{mid:?}");
    assert!((alpha - 0.5).abs() < 1e-6, "{alpha}");
    // A midpoint of 25 % puts the half-way colour a quarter along.
    let g = parsed_gradient(&gradient_block(
        b"Lnr ",
        0.0,
        100.0,
        false,
        vec![
            colour_stop(0.0, 0.0, 0.0, 0, 25),
            colour_stop(255.0, 255.0, 255.0, 4096, 50),
        ],
        vec![],
    ));
    let (c, alpha) = g.at(0.25);
    assert!((c[0] - 0.5).abs() < 1e-5, "{c:?}");
    assert_eq!(alpha, 1.0, "no transparency stops means opaque");
}

#[test]
fn a_gradient_fill_multiplies_its_opacity_stops_by_the_shape_coverage() {
    // Opacity 100 % -> 0 % left to right over a full-canvas fill.
    let block = gradient_block(
        b"Lnr ",
        0.0,
        100.0,
        false,
        vec![
            colour_stop(255.0, 0.0, 0.0, 0, 50),
            colour_stop(255.0, 0.0, 0.0, 4096, 50),
        ],
        vec![alpha_stop(100.0, 0), alpha_stop(0.0, 4096)],
    );
    let document = doc(&empty_fill_file(8, 2, *b"GdFl", block, None).write());
    let a0 = fill_px(&document, 0, 0).map_or(f32::NAN, |p| p[3]);
    let a7 = fill_px(&document, 7, 0).map_or(f32::NAN, |p| p[3]);
    assert!(a0 > 0.9 && a7 < 0.2 && a0 > a7, "{a0} {a7}");
    assert_eq!(fill_px(&document, 3, 0).map(|p| p[0]), Some(1.0));
}

// -- Artboard background colours ------------------------------------------

fn artboard_block(kind: i32, rect: (f64, f64, f64, f64)) -> Vec<u8> {
    fill_block(&[
        (b"Clr ", rgb(18.0, 108.0, 200.0)),
        (b"artboardBackgroundType", V::Long(kind)),
        (
            b"artboardRect",
            V::Obj(vec![
                (b"Top ", V::Doub(rect.0)),
                (b"Left", V::Doub(rect.1)),
                (b"Btom", V::Doub(rect.2)),
                (b"Rght", V::Doub(rect.3)),
            ]),
        ),
    ])
}

fn artboard_file(kind: i32, rect: (f64, f64, f64, f64)) -> TestPsd {
    TestPsd::new(1, 8, 6, 8).with(|p| {
        p.layers.push(TestLayer::divider());
        p.layers.push(TestLayer::group("Artboard 1", *b"pass").with(|l| {
            l.blocks.push((*b"artb", artboard_block(kind, rect)));
        }));
    })
}

#[test]
fn an_artboards_custom_background_colour_is_drawn_inside_its_rectangle() {
    // Photoshop's merged image of `artboard-bgcolor.psd` shows exactly the
    // `Clr ` colour inside `artboardRect` and nothing outside it.
    let document = doc(&artboard_file(4, (0.0, 2.0, 4.0, 6.0)).write());
    assert_eq!(document.layers.len(), 2, "the group and its background layer");
    assert_eq!(fill_px(&document, 2, 0), Some(u8px(18, 108, 200, 255)));
    assert_eq!(fill_px(&document, 5, 3), Some(u8px(18, 108, 200, 255)));
    assert!(fill_px(&document, 1, 0).is_none(), "left of the rectangle");
    assert!(fill_px(&document, 6, 0).is_none(), "right of the rectangle");
    assert!(fill_px(&document, 2, 4).is_none(), "below the rectangle");
    let text = report_text(&document);
    assert!(text.contains("custom background colour"), "{text}");
}

#[test]
fn an_artboard_rectangle_past_the_canvas_is_clipped_to_it() {
    let document = doc(&artboard_file(4, (-3.0, -3.0, 100.0, 100.0)).write());
    assert_eq!(fill_px(&document, 0, 0), Some(u8px(18, 108, 200, 255)));
    assert_eq!(fill_px(&document, 7, 5), Some(u8px(18, 108, 200, 255)));
    let image = &document.pixels.first().map(|p| (p.image.width(), p.image.height()));
    assert_eq!(*image, Some((8, 6)), "the layer is the canvas, not the rectangle");
}

#[test]
fn transparent_and_black_artboard_backgrounds_draw_nothing_and_white_is_drawn() {
    let transparent = doc(&artboard_file(3, (0.0, 0.0, 6.0, 8.0)).write());
    assert_eq!(transparent.layers.len(), 1);
    assert!(!report_text(&transparent).contains("background"));
    let black = doc(&artboard_file(2, (0.0, 0.0, 6.0, 8.0)).write());
    assert_eq!(black.layers.len(), 1);
    let text = report_text(&black);
    assert!(text.contains("black background") && text.contains("not drawn"), "{text}");
    // White (type 1) is drawn as white inside the rectangle: Photoshop's
    // merged image of `advanced-blending.psd` blends its layers over it
    // (task 147, mean error 58 -> see the audit).
    let white = doc(&artboard_file(1, (0.0, 2.0, 4.0, 6.0)).write());
    assert_eq!(white.layers.len(), 2, "the group and its background layer");
    assert_eq!(fill_px(&white, 2, 0), Some(u8px(255, 255, 255, 255)));
    assert!(fill_px(&white, 6, 0).is_none(), "right of the rectangle");
}

#[test]
fn artboard_blocks_parse_and_refuse_as_specified() {
    let bg = fill::parse_artboard(&artboard_block(4, (1.0, 2.0, 5.0, 9.0)));
    assert_eq!(
        bg,
        Ok(Some(fill::ArtboardBackground {
            rect: aurora_core::Rect { x: 2, y: 1, width: 7, height: 4 },
            colour: [18.0 / 255.0, 108.0 / 255.0, 200.0 / 255.0],
        }))
    );
    assert_eq!(fill::parse_artboard(&artboard_block(3, (0.0, 0.0, 1.0, 1.0))), Ok(None));
    assert_eq!(
        fill::parse_artboard(&artboard_block(2, (0.0, 0.0, 1.0, 1.0))),
        Err(fill::ArtboardLeft::Unchecked)
    );
    assert_eq!(fill::parse_artboard(&[1, 2, 3]), Err(fill::ArtboardLeft::Unreadable));
    assert_eq!(
        fill::parse_artboard(&vec![0; fill::MAX_ARTBOARD_BLOCK_BYTES + 1]),
        Err(fill::ArtboardLeft::Unreadable)
    );
}

#[test]
fn an_artboard_background_is_charged_to_the_pixel_budget_and_refused_over_it() {
    let bg = fill::ArtboardBackground {
        rect: aurora_core::Rect { x: 0, y: 0, width: 4, height: 3 },
        colour: [0.0, 0.0, 0.0],
    };
    let header = super::Header { version: 1, channels: 3, color_mode: 3, width: 8, height: 8, depth: 8 };
    let mut budget = super::vector::Budget { pixels: 11, work: 0 };
    assert!(matches!(
        super::artboard_background(bg, header, &mut budget),
        Err(fill::ArtboardLeft::TooLarge)
    ));
    assert_eq!(budget.pixels, 11, "a refusal charges nothing");
    let mut budget = super::vector::Budget { pixels: 12, work: 0 };
    assert!(super::artboard_background(bg, header, &mut budget).is_ok());
    assert_eq!(budget.pixels, 0, "4 x 3 pixels were charged");
}

#[test]
fn a_gradients_reference_box_is_the_shape_only_when_aligned_and_non_empty() {
    let canvas = square(32);
    let shape = aurora_core::Rect { x: 4, y: 5, width: 10, height: 6 };
    let mut g = parsed_gradient(&black_to_white(b"Lnr ", 0.0, 100.0, false));
    assert_eq!(fill::reference_box(&g, Some(shape), canvas), shape);
    assert_eq!(fill::reference_box(&g, None, canvas), canvas);
    let flat = aurora_core::Rect { height: 0, ..shape };
    assert_eq!(fill::reference_box(&g, Some(flat), canvas), canvas);
    g.align = false;
    assert_eq!(fill::reference_box(&g, Some(shape), canvas), canvas);
}

#[test]
fn an_inverted_vector_mask_draws_the_fill_outside_the_shape_across_the_canvas() {
    // Flags bit 0: invert. The shape becomes a hole in a canvas-wide fill.
    let file = empty_fill_file(
        8,
        8,
        *b"SoCo",
        solid_block(255.0, 0.0, 0.0),
        Some(vmsk(1, &rect_path(0.25, 0.25, 0.75, 0.75, 1))),
    );
    let document = doc(&file.write());
    assert_eq!(fill_px(&document, 0, 0), Some(u8px(255, 0, 0, 255)), "outside the shape");
    assert_eq!(fill_px(&document, 7, 7), Some(u8px(255, 0, 0, 255)));
    assert!(fill_px(&document, 4, 4).is_some_and(|p| p[3] == 0.0), "the hole");
}

#[test]
fn an_artboard_with_an_empty_or_off_canvas_rectangle_says_so() {
    for rect in [(2.0, 2.0, 2.0, 2.0), (100.0, 100.0, 200.0, 200.0), (-9.0, -9.0, -1.0, -1.0)] {
        let document = doc(&artboard_file(4, rect).write());
        assert_eq!(document.layers.len(), 1, "{rect:?}: no background layer");
        let text = report_text(&document);
        assert!(text.contains("empty rectangle or one entirely off the canvas"), "{rect:?}: {text}");
        assert!(!text.contains("could not be read"), "{rect:?}: {text}");
    }
}

#[test]
fn a_drawn_fills_buffer_is_charged_unless_it_is_the_shapes_own_charged_raster() {
    use half::f16;
    let region = square(10);
    let raster = |outside| super::vector::Raster {
        bounds: square(10),
        coverage: vec![f16::ONE; 100],
        outside,
        charged: 200,
    };
    assert_eq!(super::fill_pixel_charge(Some(&raster(false)), 1.0, region), 0);
    assert_eq!(super::fill_pixel_charge(Some(&raster(false)), 0.5, region), 100);
    assert_eq!(super::fill_pixel_charge(Some(&raster(true)), 1.0, region), 100);
    assert_eq!(super::fill_pixel_charge(None, 1.0, region), 100);
}

/// Photoshop's own pixels (`gradient-styles.psd`, shapes of 32 x 32 px
/// filled red to white, Smoothness 100 %, G channel = the ramp position
/// through the smoothed transition, dithered by about 1-2 levels), sampled
/// by hand, against the drawn fill's maths. Bounds are the fit's own mean
/// error (Linear about 1, Radial about 1-2 levels) plus margin; the plain
/// straight ramp and the old Diamond radius both fail them.
#[test]
fn drawn_gradients_follow_photoshops_pixels() {
    let red_white = |style: &'static [u8; 4], angle: f64, reverse: bool| {
        let block = gradient_block(
            style,
            angle,
            100.0,
            reverse,
            vec![colour_stop(255.0, 0.0, 0.0, 0, 50), colour_stop(255.0, 255.0, 255.0, 4096, 50)],
            vec![],
        );
        parsed_gradient(&block)
    };
    let level = |g: &fill::Gradient, b: aurora_core::Rect, x: f64, y: f64| {
        f64::from(g.at(g.position(x, y, b)).0[1]) * 255.0
    };
    // Linear 30 degrees, box (0, 0, 32, 32): row y = 11, x = 2, 5, ..., 29.
    let b = square(32);
    let g = red_white(b"Lnr ", 30.0, false);
    let photoshop = [48.0, 68.0, 88.0, 111.0, 133.0, 155.0, 176.0, 196.0, 215.0, 231.0];
    for (i, want) in photoshop.iter().enumerate() {
        let got = level(&g, b, 2.0 + 3.0 * i as f64, 11.0);
        assert!((got - want).abs() < 6.0, "linear x {}: {got} vs {want}", 2 + 3 * i);
    }
    // Radial, box (32, 0, 32, 32), centre pixel (48, 16): the row to the
    // right of the centre.
    let b = aurora_core::Rect { x: 32, y: 0, width: 32, height: 32 };
    let g = red_white(b"Rdl ", 30.0, false);
    let row = [
        0.0, 8.0, 17.0, 29.0, 42.0, 56.0, 71.0, 87.0, 104.0, 120.0, 137.0, 153.0, 170.0, 186.0,
        201.0, 215.0, 228.0,
    ];
    for (k, want) in row.iter().enumerate().skip(1) {
        let got = level(&g, b, 48.0 + k as f64, 16.0);
        assert!((got - want).abs() < 8.0, "radial k {k}: {got} vs {want}");
    }
    // Diamond, 30 degrees (rotated square), box (128, 0, 32, 32): the
    // anti-diagonal is not on the axes, so use the sums along the first
    // axis instead -- Photoshop's pixel at the diamond's own corner
    // distance is saturated white (255) and the centre is near 0.
    let b = aurora_core::Rect { x: 128, y: 0, width: 32, height: 32 };
    let g = red_white(b"Dmnd", 0.0, false);
    assert!(level(&g, b, 144.0, 16.0) < 12.0, "diamond centre");
    assert!(level(&g, b, 144.0 + 20.0, 16.0) > 250.0, "diamond edge 20 px out is white");
}

// The 4 x 4 gradient-only fixtures of psd-tools (`colormodes/4x4_*bit_*.psd`,
// a 30 degree Reflected gradient on a 4 x 4 canvas) cannot be matched: their
// merged image depends only on |x - y| (a 45 degree direction), while the
// same Photoshop gradient with the same stored angle on the 34 x 34 shapes of
// `gradient-styles.psd` really runs at 30 degrees (checked pixel by pixel,
// task 147). They are kept as fixtures and score 17-28 levels; moving the
// smoothing or the Radial/Diamond constants shifted them by 1-1.3 levels
// either way, which is noise on an unexplained picture, not a signal.

/// A two-stop black-to-white `GdFl` block with an explicit `Intr`
/// (smoothness, 4096 = 100 %).
fn gradient_block_with_smoothness(smoothness: Option<f64>) -> Vec<u8> {
    gradient_block_with_alpha(smoothness, 100.0)
}

/// [`gradient_block_with_smoothness`] with the end opacity (percent).
fn gradient_block_with_alpha(smoothness: Option<f64>, alpha_end: f64) -> Vec<u8> {
    let mut grad = vec![
        (&b"GrdF"[..], V::Enum(b"GrdF", b"CstS")),
        (
            b"Clrs",
            V::List(vec![
                colour_stop(0.0, 0.0, 0.0, 0, 50),
                colour_stop(255.0, 255.0, 255.0, 4096, 50),
            ]),
        ),
        (b"Trns", V::List(vec![alpha_stop(100.0, 0), alpha_stop(alpha_end, 4096)])),
    ];
    if let Some(s) = smoothness {
        grad.push((b"Intr", V::Doub(s)));
    }
    fill_block(&[
        (b"Angl", V::Unit(b"#Ang", 0.0)),
        (b"Type", V::Enum(b"GrdT", b"Lnr ")),
        (b"Grad", V::Obj(grad)),
    ])
}

#[test]
fn smoothness_zero_is_a_straight_ramp_and_anything_else_is_smoothed() {
    // Photoshop's default (Smoothness 100 %, also when `Intr` is absent): a
    // smoothstep per segment, 3 * 0.25^2 - 2 * 0.25^3 = 0.15625 at a quarter.
    for smoothness in [None, Some(4096.0)] {
        let g = parsed_gradient(&gradient_block_with_smoothness(smoothness));
        assert!(g.smooth_ramp, "{smoothness:?}");
        near_f32(g.at(0.25).0[0], 0.156_25);
        near_f32(g.at(0.5).0[0], 0.5);
    }
    // Smoothness 0 %: straight.
    let g = parsed_gradient(&gradient_block_with_smoothness(Some(0.0)));
    assert!(!g.smooth_ramp);
    near_f32(g.at(0.25).0[0], 0.25);
}

/// Opacity stops are smoothed like colour stops: a 100 % to 0 % ramp is
/// `1 - smoothstep(t)`, so at a quarter `1 - 0.15625 = 0.84375` (derived
/// from `3 * 0.25^2 - 2 * 0.25^3`, not read off the code), and straight
/// (`0.75`) at Smoothness 0.
#[test]
fn opacity_stops_are_smoothed_like_colour_stops() {
    let smooth = parsed_gradient(&gradient_block_with_alpha(None, 0.0));
    near_f32(smooth.at(0.25).1, 0.843_75);
    near_f32(smooth.at(0.5).1, 0.5);
    let straight = parsed_gradient(&gradient_block_with_alpha(Some(0.0), 0.0));
    near_f32(straight.at(0.25).1, 0.75);
}

fn near_f32(a: f32, want: f32) {
    assert!((a - want).abs() < 1e-5, "{a} != {want}");
}

/// A CMYK file's gradient is blended in ink space through sub-stops that
/// carry the smoothing themselves, and the ramp between them must stay
/// straight (a second smoothing would flatten each tiny segment).
#[test]
fn a_cmyk_gradient_is_smoothed_once_through_its_ink_space_sub_stops() {
    let ink = |c: f64| {
        V::Obj(vec![
            (b"Cyn ", V::Doub(c)),
            (b"Mgnt", V::Doub(0.0)),
            (b"Ylw ", V::Doub(0.0)),
            (b"Blck", V::Doub(0.0)),
        ])
    };
    let stop = |c: f64, at: i32| {
        V::Obj(vec![(b"Clr ", ink(c)), (b"Lctn", V::Long(at)), (b"Mdpn", V::Long(50))])
    };
    let block = fill_block(&[
        (b"Angl", V::Unit(b"#Ang", 0.0)),
        (b"Type", V::Enum(b"GrdT", b"Lnr ")),
        (
            b"Grad",
            V::Obj(vec![
                (b"GrdF", V::Enum(b"GrdF", b"CstS")),
                (b"Clrs", V::List(vec![stop(0.0, 0), stop(100.0, 4096)])),
                (b"Trns", V::List(vec![alpha_stop(100.0, 0), alpha_stop(100.0, 4096)])),
            ]),
        ),
    ]);
    // No profile: the naive formula, red = 1 - cyan, so red = 1 - smoothstep(t).
    let converter = crate::colour_cmyk::CmykConverter::new(None);
    let Ok(Fill::Gradient(g)) = fill::parse_in(*b"GdFl", &block, Some(&converter)) else {
        unreachable!("a CMYK gradient")
    };
    for t in [0.1_f32, 0.27, 0.5, 0.8] {
        let want = 1.0 - f64::from(t) * f64::from(t) * (3.0 - 2.0 * f64::from(t));
        let got = f64::from(g.at(t).0[0]);
        assert!((got - want).abs() < 0.005, "t {t}: {got} vs {want}");
    }
}

/// A gradient fill with a `-2` mask channel of 6 x 6 at (2, 2) on a 10 x 10
/// canvas, black to white left to right; `mask_flags` is the mask's flags
/// byte and `vector` an optional `vmsk` block.
fn masked_gradient_file(mask_flags: u8, vector: Option<Vec<u8>>) -> TestPsd {
    TestPsd::new(1, 10, 10, 8).with(|p| {
        p.layers.push(TestLayer::empty("fill").with(|l| {
            l.blocks.push((*b"GdFl", black_to_white(b"Lnr ", 0.0, 100.0, false)));
            if let Some(v) = vector {
                l.blocks.push((*b"vmsk", v));
            }
            l.mask = Some((2, 2, 8, 8, 0, mask_flags));
            l.channels.push((-2, vec![255; 36]));
        }));
    })
}

/// The red a drawn gradient has at document `(3, 5)`: 0.216 when it spans
/// the 10 px canvas (`t = 0.3`, smoothed), 0.074 when it spans the 6 px mask
/// rectangle (`t = 1/6`).
fn gradient_red_at_3_5(file: &TestPsd) -> f32 {
    let document = doc(&file.write());
    fill_px(&document, 3, 5).map_or(f32::NAN, |p| p[0])
}

/// Task 147: a raster user mask's rectangle is the layer's extent for a
/// drawn gradient (Photoshop: `gradient-fill.psd`); the rendering of a
/// disabled vector mask in the `-2` channel (flags bit 3) is not, and the
/// gradient spans the canvas.
#[test]
fn a_drawn_gradient_spans_a_raster_masks_rectangle_but_not_a_disabled_vector_masks() {
    let raster = gradient_red_at_3_5(&masked_gradient_file(0, None));
    assert!((raster - 0.074).abs() < 0.01, "raster mask: {raster}");
    // Flags bit 3: the mask channel is the vector mask's rendering; the
    // vector mask itself is turned off (flags bit 2).
    let disabled = gradient_red_at_3_5(&masked_gradient_file(
        0x08,
        Some(vmsk(4, &rect_path(0.25, 0.25, 0.75, 0.75, 1))),
    ));
    assert!((disabled - 0.216).abs() < 0.01, "disabled vector mask: {disabled}");
}

/// Task 147: a drawn gradient fill whose Smoothness is neither 0 nor 100 %
/// says so in the open report; 0, 100 % and an absent `Intr` do not.
#[test]
fn a_drawn_fill_with_partial_smoothness_reports_it() {
    for (intr, says) in [
        (Some(2048.0), true),
        (Some(4096.0), false),
        (Some(0.0), false),
        (None, false),
    ] {
        let file = empty_fill_file(10, 2, *b"GdFl", gradient_block_with_smoothness(intr), None);
        let text = report_text(&doc(&file.write()));
        assert_eq!(
            text.contains("Smoothness between 0 and 100 % is drawn at 100 %")
                || text.contains("Smoothness between 0 and 100 % are drawn at 100 %"),
            says,
            "{intr:?}: {text}"
        );
    }
}
