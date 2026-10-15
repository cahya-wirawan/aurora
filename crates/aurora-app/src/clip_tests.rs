//! Clipping masks (0.201.0): the CPU compositor against hand-computed
//! references on small synthetic trees, culling bit-identity, and the GPU
//! predicate. Pixel `(x, y)` of tile `(0, 0)`; layers are 10 x 10 unless
//! noted, the backdrop is an opaque white layer.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::indexing_slicing,
    clippy::many_single_char_names,
    clippy::float_cmp
)]

use super::*;

const TILE0: aurora_tile::TileId = aurora_tile::TileId { x: 0, y: 0 };
const TOL: f32 = 2.0e-3;

fn ok<T, E: std::fmt::Debug>(r: Result<T, E>) -> T {
    match r {
        Ok(v) => v,
        Err(err) => unreachable!("{err:?}"),
    }
}

fn store() -> (tempfile::TempDir, aurora_tile::TileStore) {
    let dir = ok(tempfile::tempdir());
    let Some(budget) = std::num::NonZeroUsize::new(16) else {
        unreachable!("non-zero");
    };
    let store = ok(aurora_tile::TileStore::new(
        dir.path().to_path_buf(),
        budget,
    ));
    (dir, store)
}

fn rect(side: u32) -> aurora_core::Rect {
    aurora_core::Rect {
        x: 0,
        y: 0,
        width: side,
        height: side,
    }
}

/// A pixel layer on top of `parent` painted by `paint(x, y) -> rgba`.
fn layer(
    layers: &mut aurora_doc::LayerTree,
    store: &mut aurora_tile::TileStore,
    parent: Option<aurora_doc::LayerId>,
    side: u32,
    paint: impl Fn(usize, usize) -> [f32; 4],
) -> aurora_doc::LayerId {
    let id = ok(layers.add_pixel_layer("l", rect(side), parent));
    let Some(surface) = layers.surface_id(id) else {
        unreachable!("pixel layer");
    };
    let tile = ok(store.get_mut(surface, TILE0));
    let t = aurora_tile::TILE as usize;
    for y in 0..side as usize {
        for x in 0..side as usize {
            let rgba = paint(x, y);
            for (k, v) in rgba.iter().enumerate() {
                if let Some(s) = tile.texels_mut().get_mut((y * t + x) * 4 + k) {
                    *s = half::f16::from_f32(*v);
                }
            }
        }
    }
    tile.mark_dirty(rect(aurora_tile::TILE));
    id
}

fn solid(
    layers: &mut aurora_doc::LayerTree,
    store: &mut aurora_tile::TileStore,
    rgba: [f32; 4],
) -> aurora_doc::LayerId {
    layer(layers, store, None, 10, move |_, _| rgba)
}

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

/// The tile's straight composite through the shipped root fold.
fn composite(layers: &aurora_doc::LayerTree, store: &mut aurora_tile::TileStore) -> Vec<half::f16> {
    let mut budget = CompositeBudget::for_pass(layers);
    composite_roots_into_tile(layers, store, TILE0, (0, 0), (0, 0), &mut budget).0
}

fn px(acc: &[half::f16], x: usize, y: usize) -> [f32; 4] {
    let i = (y * aurora_tile::TILE as usize + x) * 4;
    let get = |k: usize| acc.get(i + k).map_or(f32::NAN, |v| v.to_f32());
    [get(0), get(1), get(2), get(3)]
}

#[track_caller]
fn near(got: [f32; 4], want: [f32; 4]) {
    assert!(
        got.iter().zip(want).all(|(g, w)| (g - w).abs() <= TOL),
        "got {got:?}, want {want:?}"
    );
}

fn clip(layers: &mut aurora_doc::LayerTree, id: aurora_doc::LayerId) {
    ok(layers.set_clipped(id, true));
}

/// Base red on the left half (`x < 5`), with a half-alpha pixel at (2, 2).
fn half_red(x: usize, y: usize) -> [f32; 4] {
    if (x, y) == (2, 2) {
        [1.0, 0.0, 0.0, 0.5]
    } else if x < 5 {
        RED
    } else {
        [0.0; 4]
    }
}

#[test]
fn a_clipped_layer_shows_only_where_its_base_has_alpha_and_atop_at_partial_alpha() {
    let (_dir, mut store) = store();
    let mut layers = aurora_doc::LayerTree::new();
    solid(&mut layers, &mut store, WHITE);
    layer(&mut layers, &mut store, None, 10, half_red);
    let blue = solid(&mut layers, &mut store, BLUE);
    clip(&mut layers, blue);
    let acc = composite(&layers, &mut store);
    near(px(&acc, 1, 1), BLUE);
    near(px(&acc, 8, 1), WHITE);
    // Atop at base alpha 0.5: 0.5 * blue + 0.5 * white, not blue over it.
    near(px(&acc, 2, 2), [0.5, 0.5, 1.0, 1.0]);
}

#[test]
fn unclipped_the_same_tree_paints_blue_everywhere() {
    let (_dir, mut store) = store();
    let mut layers = aurora_doc::LayerTree::new();
    solid(&mut layers, &mut store, WHITE);
    layer(&mut layers, &mut store, None, 10, half_red);
    solid(&mut layers, &mut store, BLUE);
    let acc = composite(&layers, &mut store);
    near(px(&acc, 8, 1), BLUE);
}

#[test]
fn the_base_mask_limits_the_whole_clip_group() {
    let (_dir, mut store) = store();
    let mut layers = aurora_doc::LayerTree::new();
    solid(&mut layers, &mut store, WHITE);
    let base = solid(&mut layers, &mut store, RED);
    ok(layers.add_mask(base, rect(10)));
    let Some(mask) = layers.mask_surface_id(base) else {
        unreachable!("mask surface");
    };
    for y in 0..10 {
        for x in 0..10 {
            ok(aurora_doc::write_mask_coverage(
                &mut store,
                mask,
                TILE0,
                x,
                y,
                if x < 5 { 1.0 } else { 0.0 },
            ));
        }
    }
    let blue = solid(&mut layers, &mut store, BLUE);
    clip(&mut layers, blue);
    let acc = composite(&layers, &mut store);
    near(px(&acc, 2, 5), BLUE);
    near(px(&acc, 7, 5), WHITE);
}

#[test]
fn the_base_opacity_applies_to_the_whole_clip_group() {
    let (_dir, mut store) = store();
    let mut layers = aurora_doc::LayerTree::new();
    solid(&mut layers, &mut store, WHITE);
    let base = solid(&mut layers, &mut store, RED);
    ok(layers.set_opacity(base, 0.5));
    let blue = solid(&mut layers, &mut store, BLUE);
    clip(&mut layers, blue);
    let acc = composite(&layers, &mut store);
    // The group (blue, from the opaque base's shape) at 50 % over white.
    near(px(&acc, 4, 4), [0.5, 0.5, 1.0, 1.0]);
}

#[test]
fn a_clipped_layer_uses_its_own_blend_mode_against_the_base() {
    let (_dir, mut store) = store();
    let mut layers = aurora_doc::LayerTree::new();
    solid(&mut layers, &mut store, WHITE);
    solid(&mut layers, &mut store, [1.0, 0.5, 0.5, 1.0]);
    let top = solid(&mut layers, &mut store, [0.5, 1.0, 1.0, 1.0]);
    ok(layers.set_blend_mode(top, aurora_doc::BlendMode::Multiply));
    clip(&mut layers, top);
    let acc = composite(&layers, &mut store);
    near(px(&acc, 3, 3), [0.5, 0.5, 0.5, 1.0]);
}

#[test]
fn two_stacked_clipped_layers_composite_in_order_inside_the_base() {
    let (_dir, mut store) = store();
    let mut layers = aurora_doc::LayerTree::new();
    solid(&mut layers, &mut store, WHITE);
    layer(&mut layers, &mut store, None, 10, half_red);
    let g = solid(&mut layers, &mut store, [0.0, 1.0, 0.0, 0.5]);
    let b = solid(&mut layers, &mut store, [0.0, 0.0, 1.0, 0.5]);
    clip(&mut layers, g);
    clip(&mut layers, b);
    let acc = composite(&layers, &mut store);
    near(px(&acc, 1, 1), [0.25, 0.25, 0.5, 1.0]);
    near(px(&acc, 8, 1), WHITE);
}

#[test]
fn a_non_clipped_layer_breaks_the_chain_and_the_next_clipped_layer_clips_to_it() {
    let (_dir, mut store) = store();
    let mut layers = aurora_doc::LayerTree::new();
    solid(&mut layers, &mut store, WHITE);
    layer(&mut layers, &mut store, None, 10, half_red);
    let blue = solid(&mut layers, &mut store, BLUE);
    clip(&mut layers, blue);
    layer(&mut layers, &mut store, None, 10, |x, _| {
        if x < 7 {
            [0.0, 1.0, 0.0, 1.0]
        } else {
            [0.0; 4]
        }
    });
    let magenta = solid(&mut layers, &mut store, [1.0, 0.0, 1.0, 1.0]);
    clip(&mut layers, magenta);
    assert_eq!(layers.clip_base(magenta), layers.roots().get(1).copied());
    let acc = composite(&layers, &mut store);
    // Clipped to the green layer (x < 7), not to the red one beneath it.
    near(px(&acc, 6, 1), [1.0, 0.0, 1.0, 1.0]);
    near(px(&acc, 8, 1), WHITE);
}

#[test]
fn a_group_boundary_stops_the_clip_and_a_hidden_base_hides_its_clip_group() {
    let (_dir, mut store) = store();
    let mut layers = aurora_doc::LayerTree::new();
    solid(&mut layers, &mut store, WHITE);
    let group = ok(layers.add_group("g", None));
    let base = layer(&mut layers, &mut store, Some(group), 10, half_red);
    let blue = layer(&mut layers, &mut store, Some(group), 10, |_, _| BLUE);
    clip(&mut layers, blue);
    // The group's content is clipped to the in-group base only.
    let acc = composite(&layers, &mut store);
    near(px(&acc, 1, 1), BLUE);
    near(px(&acc, 8, 1), WHITE);
    // Hiding the base hides the clipped layer with it.
    ok(layers.set_visible(base, false));
    let acc = composite(&layers, &mut store);
    near(px(&acc, 1, 1), WHITE);
}

#[test]
fn a_clipped_curves_adjustment_changes_only_the_base() {
    let (_dir, mut store) = store();
    let mut layers = aurora_doc::LayerTree::new();
    solid(&mut layers, &mut store, [0.25, 0.25, 0.25, 1.0]);
    layer(&mut layers, &mut store, None, 10, half_red);
    let invert = aurora_core::CurvesParams {
        composite: curve_invert(),
        ..aurora_core::CurvesParams::default()
    };
    let adj =
        ok(layers.add_adjustment_layer_at("c", aurora_doc::Adjustment::Curves(invert), None, 0));
    clip(&mut layers, adj);
    let acc = composite(&layers, &mut store);
    // Inside the base: red inverted to cyan; outside: untouched grey.
    near(px(&acc, 1, 1), [0.0, 1.0, 1.0, 1.0]);
    near(px(&acc, 8, 1), [0.25, 0.25, 0.25, 1.0]);
}

fn curve_invert() -> aurora_core::ToneCurve {
    let pts = [
        aurora_core::CurvePoint::new(0.0, 1.0),
        aurora_core::CurvePoint::new(1.0, 0.0),
    ];
    ok(aurora_core::ToneCurve::new(&pts))
}

#[test]
fn a_flagged_layer_with_a_group_base_or_nothing_beneath_composites_plain() {
    let (_dir, mut store) = store();
    let mut layers = aurora_doc::LayerTree::new();
    let bottom = solid(&mut layers, &mut store, RED);
    // The model refuses to clip the bottom layer, and a group.
    assert!(matches!(
        layers.set_clipped(bottom, true),
        Err(aurora_doc::DocError::NoClipBase(_))
    ));
    let group = ok(layers.add_group("g", None));
    assert!(matches!(
        layers.set_clipped(group, true),
        Err(aurora_doc::DocError::GroupCannotBeClipped(_))
    ));
    // A pixel layer above an (empty) group base clips to it since 0.243.0:
    // the group has no content, so the clipped layer shows nothing.
    let top = solid(&mut layers, &mut store, BLUE);
    ok(layers.set_clipped(top, true));
    assert_eq!(layers.clip_base(top), Some(group));
    assert_eq!(clip_base_of(&layers, top), Some(group));
    let acc = composite(&layers, &mut store);
    near(px(&acc, 1, 1), RED);
}

// ---- 0.243.0: groups as clip bases, Pass Through Fill ----

const CLEAR: [f32; 4] = [0.0; 4];

/// White backdrop, a group holding a layer painted by `paint`, and a full
/// blue layer above the group clipped to it. The group's mode is `mode`.
fn group_clip_tree(
    store: &mut aurora_tile::TileStore,
    mode: aurora_doc::BlendMode,
    paint: impl Fn(usize, usize) -> [f32; 4],
) -> (aurora_doc::LayerTree, aurora_doc::LayerId) {
    let mut layers = aurora_doc::LayerTree::new();
    solid(&mut layers, store, WHITE);
    let group = ok(layers.add_group("g", None));
    layer(&mut layers, store, Some(group), 10, paint);
    if mode != aurora_doc::BlendMode::Normal {
        ok(layers.set_blend_mode(group, mode));
    }
    let top = solid(&mut layers, store, BLUE);
    clip(&mut layers, top);
    (layers, group)
}

fn red_corner(x: usize, y: usize) -> [f32; 4] {
    if x < 4 && y < 4 { RED } else { CLEAR }
}

#[test]
fn a_layer_clipped_to_a_group_is_cut_to_the_groups_shape() {
    for mode in [
        aurora_doc::BlendMode::Normal,
        aurora_doc::BlendMode::PassThrough,
    ] {
        let (_dir, mut store) = store();
        let (layers, _group) = group_clip_tree(&mut store, mode, red_corner);
        let acc = composite(&layers, &mut store);
        near(px(&acc, 1, 1), BLUE);
        near(px(&acc, 6, 1), WHITE);
        near(px(&acc, 1, 6), WHITE);
        near(px(&acc, 8, 8), WHITE);
    }
}

#[test]
fn a_group_clip_base_keeps_its_soft_edge_and_opacity() {
    for mode in [
        aurora_doc::BlendMode::Normal,
        aurora_doc::BlendMode::PassThrough,
    ] {
        let (_dir, mut store) = store();
        let (mut layers, group) = group_clip_tree(&mut store, mode, |x, _| {
            if x < 4 { [0.5, 0.0, 0.0, 0.5] } else { CLEAR }
        });
        // Half-covered texel: the clipped blue is limited to alpha 0.5.
        near(
            px(&composite(&layers, &mut store), 1, 1),
            [0.5, 0.5, 1.0, 1.0],
        );
        // The group's own opacity fades the whole clip group.
        ok(layers.set_opacity(group, 0.5));
        near(
            px(&composite(&layers, &mut store), 1, 1),
            [0.75, 0.75, 1.0, 1.0],
        );
        near(px(&composite(&layers, &mut store), 6, 1), WHITE);
    }
}

#[test]
fn a_pass_through_clip_base_keeps_what_its_children_blend_with_the_backdrop() {
    let (_dir, mut store) = store();
    let mut layers = aurora_doc::LayerTree::new();
    solid(&mut layers, &mut store, [0.5, 0.5, 0.5, 1.0]);
    let group = ok(layers.add_group("g", None));
    ok(layers.set_blend_mode(group, aurora_doc::BlendMode::PassThrough));
    let child = layer(&mut layers, &mut store, Some(group), 10, red_corner);
    ok(layers.set_blend_mode(child, aurora_doc::BlendMode::Multiply));
    // A fully transparent clipped layer changes nothing.
    let clipped = layer(&mut layers, &mut store, None, 10, |_, _| CLEAR);
    clip(&mut layers, clipped);
    let acc = composite(&layers, &mut store);
    // Multiply of grey 0.5 and red, not the isolated red.
    near(px(&acc, 1, 1), [0.5, 0.0, 0.0, 1.0]);
    near(px(&acc, 6, 6), [0.5, 0.5, 0.5, 1.0]);
}

/// A partly covered texel is where the backdrop has to be unmixed from the
/// group's content: Multiply of a half-covered red over grey 0.5 gives
/// (0.5, 0.25, 0.25) with or without a (transparent) clipped layer.
#[test]
fn a_pass_through_clip_base_unmixes_the_backdrop_at_partial_coverage() {
    let (_dir, mut store) = store();
    let mut layers = aurora_doc::LayerTree::new();
    solid(&mut layers, &mut store, [0.5, 0.5, 0.5, 1.0]);
    let group = ok(layers.add_group("g", None));
    ok(layers.set_blend_mode(group, aurora_doc::BlendMode::PassThrough));
    let child = layer(&mut layers, &mut store, Some(group), 10, |x, _| {
        if x < 4 { [1.0, 0.0, 0.0, 0.5] } else { CLEAR }
    });
    ok(layers.set_blend_mode(child, aurora_doc::BlendMode::Multiply));
    let plain = px(&composite(&layers, &mut store), 1, 1);
    near(plain, [0.5, 0.25, 0.25, 1.0]);
    let clipped = layer(&mut layers, &mut store, None, 10, |_, _| CLEAR);
    clip(&mut layers, clipped);
    near(px(&composite(&layers, &mut store), 1, 1), plain);
}

#[test]
fn a_pass_through_groups_fill_scales_its_interpolation() {
    let (_dir, mut store) = store();
    let mut layers = aurora_doc::LayerTree::new();
    solid(&mut layers, &mut store, [0.5, 0.5, 0.5, 1.0]);
    let group = ok(layers.add_group("g", None));
    ok(layers.set_blend_mode(group, aurora_doc::BlendMode::PassThrough));
    let child = layer(&mut layers, &mut store, Some(group), 10, red_corner);
    ok(layers.set_blend_mode(child, aurora_doc::BlendMode::Multiply));
    ok(layers.set_fill_opacity(group, 0.5));
    // B + (C - B) * 0.5 with C = Multiply(grey, red) = (0.5, 0, 0).
    near(
        px(&composite(&layers, &mut store), 1, 1),
        [0.5, 0.25, 0.25, 1.0],
    );
    near(
        px(&composite(&layers, &mut store), 6, 6),
        [0.5, 0.5, 0.5, 1.0],
    );
    // Opacity and Fill multiply.
    ok(layers.set_opacity(group, 0.5));
    near(
        px(&composite(&layers, &mut store), 1, 1),
        [0.5, 0.375, 0.375, 1.0],
    );
    // Fill 0 leaves the backdrop alone.
    ok(layers.set_fill_opacity(group, 0.0));
    near(
        px(&composite(&layers, &mut store), 1, 1),
        [0.5, 0.5, 0.5, 1.0],
    );
}

/// Under a Pass Through group's Fill below 100 %, an adjustment child
/// adjusts only where the group's own content is.
#[test]
fn an_adjustment_in_a_reduced_fill_pass_through_group_skips_the_backdrop() {
    let build = |fill: f32| {
        let (dir, mut store) = store();
        let mut layers = aurora_doc::LayerTree::new();
        solid(&mut layers, &mut store, [0.25, 0.25, 0.25, 1.0]);
        let group = ok(layers.add_group("g", None));
        ok(layers.set_blend_mode(group, aurora_doc::BlendMode::PassThrough));
        layer(&mut layers, &mut store, Some(group), 10, |x, _| {
            if x < 5 { RED } else { CLEAR }
        });
        let invert = aurora_core::CurvesParams {
            composite: curve_invert(),
            ..aurora_core::CurvesParams::default()
        };
        ok(layers.add_adjustment_layer_at(
            "c",
            aurora_doc::Adjustment::Curves(invert),
            Some(group),
            0,
        ));
        ok(layers.set_fill_opacity(group, fill));
        let acc = composite(&layers, &mut store);
        drop(dir);
        acc
    };
    let half = build(0.5);
    // Content: red inverted is cyan, mixed 50 % into the grey backdrop.
    near(px(&half, 1, 1), [0.125, 0.625, 0.625, 1.0]);
    // No content: the adjustment leaves the backdrop alone (the unisolated
    // fold would invert it to 0.75 and mix to 0.5).
    near(px(&half, 8, 1), [0.25, 0.25, 0.25, 1.0]);
    // Fill 100 %: a Pass Through adjustment reaches the backdrop, as ever.
    let full = build(1.0);
    near(px(&full, 1, 1), [0.0, 1.0, 1.0, 1.0]);
    near(px(&full, 8, 1), [0.75, 0.75, 0.75, 1.0]);
}

#[test]
fn culling_stays_bit_identical_with_group_clip_bases_and_reduced_fill_groups() {
    let (_dir, mut store) = store();
    let mut layers = aurora_doc::LayerTree::new();
    layer(&mut layers, &mut store, None, 256, |_, _| RED);
    // An opaque Pass Through group whose child covers the tile: a cover
    // candidate, and a clip base.
    let group = ok(layers.add_group("g", None));
    ok(layers.set_blend_mode(group, aurora_doc::BlendMode::PassThrough));
    layer(&mut layers, &mut store, Some(group), 256, |_, _| WHITE);
    let c1 = layer(&mut layers, &mut store, None, 256, |x, _| {
        [0.0, 0.5, 0.0, if x % 2 == 0 { 0.5 } else { 1.0 }]
    });
    clip(&mut layers, c1);
    // A second Pass Through group with Fill below 100 %: never a cover.
    let g2 = ok(layers.add_group("g2", None));
    ok(layers.set_blend_mode(g2, aurora_doc::BlendMode::PassThrough));
    layer(&mut layers, &mut store, Some(g2), 256, |_, _| BLUE);
    ok(layers.set_fill_opacity(g2, 0.5));
    set_culling(false);
    let plain = composite(&layers, &mut store);
    set_culling(true);
    let culled = composite(&layers, &mut store);
    assert!(same_bits(&plain, &culled));
    // The reduced-Fill group blends 50 % blue over what is below: not a
    // cover, so the layers below still show through.
    let p = px(&culled, 4, 4);
    assert!(p[0] > 0.0 && p[2] > 0.4 && p[2] < 1.0, "{p:?}");
}

fn culled_folds_now() -> u64 {
    COMPOSITE_CULLED_FOLDS.with(std::cell::Cell::get)
}

fn set_culling(enabled: bool) {
    COMPOSITE_CULLING_DISABLED.with(|c| c.set(!enabled));
}

fn same_bits(a: &[half::f16], b: &[half::f16]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits())
}

#[test]
fn culling_is_bit_identical_with_clip_groups_and_still_culls_below_an_opaque_base() {
    let (_dir, mut store) = store();
    let mut layers = aurora_doc::LayerTree::new();
    // A full-tile opaque red layer below an opaque Normal base: the base
    // is a cover, the layer under it is skipped.
    layer(&mut layers, &mut store, None, 256, |_, _| RED);
    layer(&mut layers, &mut store, None, 256, |_, _| WHITE);
    let c1 = layer(&mut layers, &mut store, None, 256, |x, _| {
        [0.0, 0.5, 0.0, if x % 2 == 0 { 0.5 } else { 1.0 }]
    });
    clip(&mut layers, c1);
    set_culling(false);
    let plain = composite(&layers, &mut store);
    set_culling(true);
    let before = culled_folds_now();
    let culled = composite(&layers, &mut store);
    assert!(same_bits(&plain, &culled));
    assert!(culled_folds_now() > before, "the opaque base is a cover");
}

#[test]
fn an_opaque_clipped_layer_is_never_a_cover_and_nothing_wrong_is_skipped() {
    let (_dir, mut store) = store();
    let mut layers = aurora_doc::LayerTree::new();
    layer(&mut layers, &mut store, None, 256, |_, _| RED);
    // A translucent base, then an opaque Normal clipped layer above it:
    // the clipped layer covers only the base's shape, so the red below
    // must still show where the base has no alpha.
    layer(&mut layers, &mut store, None, 256, |x, _| {
        [0.0, 1.0, 0.0, if x < 128 { 1.0 } else { 0.0 }]
    });
    let top = layer(&mut layers, &mut store, None, 256, |_, _| BLUE);
    clip(&mut layers, top);
    set_culling(false);
    let plain = composite(&layers, &mut store);
    set_culling(true);
    let before = culled_folds_now();
    let culled = composite(&layers, &mut store);
    assert!(same_bits(&plain, &culled));
    assert_eq!(culled_folds_now(), before, "no cover exists");
    near(px(&culled, 200, 3), RED);
    near(px(&culled, 3, 3), BLUE);
}

#[test]
fn a_visible_clipped_layer_refuses_the_gpu_path_and_releasing_it_restores_it() {
    let (_dir, mut store) = store();
    let mut layers = aurora_doc::LayerTree::new();
    solid(&mut layers, &mut store, WHITE);
    let top = solid(&mut layers, &mut store, BLUE);
    assert!(document_qualifies_for_gpu_compositing(&layers));
    clip(&mut layers, top);
    assert!(!document_qualifies_for_gpu_compositing(&layers));
    ok(layers.set_clipped(top, false));
    assert!(document_qualifies_for_gpu_compositing(&layers));
}

#[test]
fn the_clip_group_blends_with_the_bases_mode_over_the_backdrop() {
    let (_dir, mut store) = store();
    let mut layers = aurora_doc::LayerTree::new();
    solid(&mut layers, &mut store, [0.5, 0.5, 0.5, 1.0]);
    let base = solid(&mut layers, &mut store, RED);
    ok(layers.set_blend_mode(base, aurora_doc::BlendMode::Multiply));
    let blue = solid(&mut layers, &mut store, BLUE);
    clip(&mut layers, blue);
    let acc = composite(&layers, &mut store);
    // Group (blue) multiplied with the grey backdrop, not painted Normal.
    near(px(&acc, 3, 3), [0.0, 0.0, 0.5, 1.0]);
}
