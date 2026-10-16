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
    Some(px(&placed.image, ix as u32, iy as u32))
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
    let block = fill_block(&[(b"Clr ", gray(90.64667969942093))]);
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
        let _ = fill::parse(*b"GdFl", &good[..cut]);
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
    // Radial: 0 at the centre, 1 at the corner.
    let g = parsed_gradient(&black_to_white(b"Rdl ", 0.0, 100.0, false));
    near(g.position(16.0, 16.0, b), 0.0);
    near(g.position(32.0, 32.0, b), 1.0);
    // Diamond: 1 at a corner of a square box, 0.5 at an edge midpoint.
    let g = parsed_gradient(&black_to_white(b"Dmnd", 0.0, 100.0, false));
    near(g.position(32.0, 32.0, b), 1.0);
    near(g.position(32.0, 16.0, b), 0.5);
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
    assert_eq!(g.at_for_test(0.0).0, [0.0, 0.0, 1.0]);
    assert_eq!(g.at_for_test(1.0).0, [1.0, 0.0, 0.0]);
    let (mid, alpha) = g.at_for_test(0.5);
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
    let (c, alpha) = g.at_for_test(0.25);
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
}

#[test]
fn transparent_white_and_black_artboard_backgrounds_draw_nothing() {
    let transparent = doc(&artboard_file(3, (0.0, 0.0, 6.0, 8.0)).write());
    assert_eq!(transparent.layers.len(), 1);
    assert!(!report_text(&transparent).contains("background"));
    for kind in [1, 2] {
        let document = doc(&artboard_file(kind, (0.0, 0.0, 6.0, 8.0)).write());
        assert_eq!(document.layers.len(), 1, "type {kind}");
        let text = report_text(&document);
        assert!(text.contains("background") && text.contains("not drawn"), "{text}");
    }
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
    assert!(super::artboard_background(bg, header, &mut budget).is_none());
    assert_eq!(budget.pixels, 11, "a refusal charges nothing");
    let mut budget = super::vector::Budget { pixels: 12, work: 0 };
    assert!(super::artboard_background(bg, header, &mut budget).is_some());
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
