//! The Spot Healing (0.237.0) at the app level: the wiring of the tool
//! (shortcut, palette, label, locks, Quick Mask, refusal notices), the drag's
//! painted area, the heal at the end of the stroke into one undo step, and
//! the refusals. The pixel model itself is tested in `aurora-brush`.

use crate::document_session::{DocumentContents, DocumentSession};
use crate::spot_heal_stamp;
use crate::{AppCommand, Drag, LockRefusal, PointerButton, ToolSettings, UndoOrder};
use aurora_ui::Tool;

const LAYER: u32 = 120;

fn rect() -> aurora_core::Rect {
    aurora_core::Rect {
        x: 0,
        y: 0,
        width: LAYER,
        height: LAYER,
    }
}

/// A session with one pixel layer whose content is `f(x, y)` (straight
/// opaque colour), inside `LAYER` x `LAYER` texels.
fn session(
    dir: &tempfile::TempDir,
    size: u32,
    f: impl Fn(u32, u32) -> [f32; 3],
) -> (DocumentSession, aurora_doc::LayerId) {
    let mut tree = aurora_doc::LayerTree::new();
    let Ok(id) = tree.add_pixel_layer("Layer".to_owned(), rect(), None) else {
        unreachable!("layer");
    };
    let Some(budget) = std::num::NonZeroUsize::new(64) else {
        unreachable!("non-zero");
    };
    let Ok(mut store) = aurora_tile::TileStore::new(dir.path().to_path_buf(), budget) else {
        unreachable!("store");
    };
    let surface = crate::surface_id_for(id);
    for y in 0..size {
        for x in 0..size {
            let Ok(tile) = store.get_mut(surface, aurora_tile::TileId { x: 0, y: 0 }) else {
                unreachable!("tile");
            };
            let [r, g, b] = f(x, y);
            let index = ((y * aurora_tile::TILE + x) as usize) * 4;
            for (c, v) in [r, g, b, 1.0].into_iter().enumerate() {
                if let Some(slot) = tile.texels_mut().get_mut(index + c) {
                    *slot = half::f16::from_f32(v);
                }
            }
        }
    }
    let session = DocumentSession::new(DocumentContents {
        layers: tree,
        history: aurora_doc::History::new(),
        canvas_size: (LAYER, LAYER),
        skipped_tiles: aurora_io::SkippedTiles::new(),
        active_layer: Some(id),
        canvas_view: aurora_ui::CanvasView::default(),
        tile_store: Some(store),
    });
    (session, id)
}

fn dir() -> tempfile::TempDir {
    match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(err) => unreachable!("{err:?}"),
    }
}

fn dump(doc: &mut DocumentSession, id: aurora_doc::LayerId) -> Vec<u16> {
    let surface = crate::surface_id_for(id);
    let Some(store) = doc.tile_store.as_mut() else {
        unreachable!("store");
    };
    match store.get(surface, aurora_tile::TileId { x: 0, y: 0 }) {
        Ok(tile) => tile.texels().iter().map(|v| v.to_bits()).collect(),
        Err(err) => unreachable!("{err:?}"),
    }
}

fn stripes(x: u32, y: u32) -> [f32; 3] {
    [
        if x % 6 < 3 { 0.8 } else { 0.2 },
        0.3 + 0.1 * (y % 4) as f32 / 4.0,
        0.5,
    ]
}

fn blemished(x: u32, y: u32) -> [f32; 3] {
    let d2 = (x as f32 - 60.0).powi(2) + (y as f32 - 60.0).powi(2);
    if d2 < 25.0 {
        [1.0, 0.0, 0.0]
    } else {
        stripes(x, y)
    }
}

fn heal_drag(doc: &DocumentSession, id: aurora_doc::LayerId) -> Option<Drag> {
    let view = aurora_ui::CanvasView::new();
    let _ = doc;
    crate::begin_drag(
        Tool::SpotHealing,
        PointerButton::Primary,
        (60.0, 60.0),
        &view,
        Some((id, rect())),
    )
}

fn settings(radius: f32) -> ToolSettings {
    let mut s = ToolSettings::default();
    s.set_radius(Tool::SpotHealing, f64::from(radius));
    s
}

#[test]
fn the_tool_is_wired_into_the_shortcuts_the_palette_and_the_tools_panel() {
    use aurora_widgets::shortcut::{Key, KeyChord, Modifiers};
    let resolved = crate::default_shortcuts()
        .resolve(KeyChord::new(Modifiers::none(), Key::Character('j')))
        .copied();
    assert_eq!(resolved, Some(AppCommand::SelectTool(Tool::SpotHealing)));
    assert_eq!(Tool::SpotHealing.shortcut_text(), "J");
    assert_eq!(
        crate::tool_command_id(Tool::SpotHealing),
        "tool.spot_healing"
    );
    assert!(
        crate::palette_commands()
            .iter()
            .any(|c| c.id == "tool.spot_healing" && c.title == "Select Spot Healing Tool")
    );
    assert_eq!(
        aurora_ui::tool_icon(Tool::SpotHealing),
        aurora_widgets::icons::IconKind::SpotHealing
    );
    assert_eq!(
        aurora_ui::tool_description(Tool::SpotHealing),
        "Spot Healing tool, shortcut J"
    );
    assert_eq!(spot_heal_stamp::SPOT_HEALING_LABEL, "Spot Healing");
    assert_eq!(
        LockRefusal::HealNotEnoughSurroundings.message(),
        "Not enough surrounding texture to heal here."
    );
    assert_eq!(
        LockRefusal::HealTooLarge.message(),
        "That area is too large to heal."
    );
    assert_eq!(
        LockRefusal::SpotHealNeedsPixelLayer.message(),
        "Select a pixel layer to heal. A group, an adjustment layer or no layer can't be healed."
    );
    // The radius setting is the Brush's.
    let mut s = ToolSettings::default();
    assert!(s.radius(Tool::SpotHealing).is_some());
    assert!(s.set_radius(Tool::SpotHealing, 33.0));
    assert_eq!(s.radius(Tool::Brush), Some(33.0));
}

#[test]
fn a_press_begins_a_brush_drag_with_an_empty_painted_area() {
    let view = aurora_ui::CanvasView::new();
    let drag = crate::begin_drag(
        Tool::SpotHealing,
        PointerButton::Primary,
        (5.0, 5.0),
        &view,
        None,
    );
    assert!(spot_heal_stamp::is_heal(drag.as_ref()));
    assert!(spot_heal_stamp::drag_stroke(drag.as_ref()).is_some_and(|s| s.mask().is_empty()));
    for tool in [Tool::Brush, Tool::Pencil, Tool::Smudge, Tool::CloneStamp] {
        let other = crate::begin_drag(tool, PointerButton::Primary, (5.0, 5.0), &view, None);
        assert!(!spot_heal_stamp::is_heal(other.as_ref()), "{tool:?}");
    }
    assert!(!spot_heal_stamp::is_heal(None));
    assert!(
        crate::begin_drag(
            Tool::SpotHealing,
            PointerButton::Secondary,
            (5.0, 5.0),
            &view,
            None
        )
        .is_none()
    );
}

#[test]
fn presses_and_locks_follow_the_brush_rules() {
    assert_eq!(
        spot_heal_stamp::press(false, None),
        Some(LockRefusal::SpotHealNeedsPixelLayer)
    );
    assert_eq!(spot_heal_stamp::press(true, None), None);
    let pixels = aurora_doc::LayerLock {
        pixels: true,
        ..aurora_doc::LayerLock::default()
    };
    let transparency = aurora_doc::LayerLock {
        transparency: true,
        ..aurora_doc::LayerLock::default()
    };
    assert_eq!(
        crate::press_refusal(Tool::SpotHealing, pixels),
        Some(LockRefusal::PixelsLocked)
    );
    assert_eq!(crate::press_refusal(Tool::SpotHealing, transparency), None);
    assert_eq!(
        spot_heal_stamp::press(true, Some(LockRefusal::PixelsLocked)),
        Some(LockRefusal::PixelsLocked)
    );
    for refusal in [
        LockRefusal::SpotHealNeedsPixelLayer,
        LockRefusal::HealNotEnoughSurroundings,
        LockRefusal::HealTooLarge,
    ] {
        assert!(crate::lock_refusal_applies(
            refusal,
            Tool::SpotHealing,
            aurora_doc::LayerLock::default()
        ));
        assert!(!crate::lock_refusal_applies(
            refusal,
            Tool::Brush,
            aurora_doc::LayerLock::default()
        ));
    }
    assert_eq!(
        crate::quick_mask::tool_refusal(Tool::SpotHealing),
        Some(LockRefusal::QuickMaskToolUnavailable)
    );
}

#[test]
fn dabs_collect_the_area_and_write_no_pixel() {
    let d = dir();
    let (mut doc, id) = session(&d, LAYER, blemished);
    let before = dump(&mut doc, id);
    let mut drag = heal_drag(&doc, id);
    let s = settings(6.0);
    for point in [(60.0, 60.0), (63.0, 60.0), (66.0, 60.0)] {
        spot_heal_stamp::add_dab(rect(), point, &s, &mut drag);
    }
    let Some(stroke) = spot_heal_stamp::drag_stroke(drag.as_ref()) else {
        unreachable!("heal drag");
    };
    assert!(stroke.mask().count() > 100);
    assert!(stroke.mask().contains(60, 60) && stroke.mask().contains(70, 60));
    assert!(!stroke.mask().contains(40, 60));
    assert_eq!(
        dump(&mut doc, id),
        before,
        "nothing is written while painting"
    );
    // A drag that is not a heal ignores the call.
    let mut brush = crate::begin_drag(
        Tool::Brush,
        PointerButton::Primary,
        (1.0, 1.0),
        &aurora_ui::CanvasView::new(),
        None,
    );
    spot_heal_stamp::add_dab(rect(), (5.0, 5.0), &s, &mut brush);
    assert!(spot_heal_stamp::drag_stroke(brush.as_ref()).is_none());
}

fn end(drag: Option<Drag>, doc: &mut DocumentSession) -> (bool, UndoOrder) {
    let mut order = UndoOrder::default();
    let mut view = aurora_ui::CanvasView::new();
    let recorded = crate::commit_ending_drag(
        drag,
        &doc.layers,
        &mut doc.history,
        &mut doc.pixel_history,
        &mut order,
        &mut view,
        doc.active_layer,
        Some((1000.0, 800.0)),
    );
    (recorded, order)
}

#[test]
fn the_release_heals_the_area_as_one_undo_step_that_undoes_bit_for_bit() {
    let d = dir();
    let (mut doc, id) = session(&d, LAYER, blemished);
    let before = dump(&mut doc, id);
    let mut drag = heal_drag(&doc, id);
    let s = settings(8.0);
    spot_heal_stamp::add_dab(rect(), (60.0, 60.0), &s, &mut drag);
    assert_eq!(spot_heal_stamp::finish(&mut drag, &mut doc), None);
    let healed = dump(&mut doc, id);
    assert_ne!(healed, before);
    // The blemish is gone: the centre is a stripe colour again.
    let truth = stripes(60, 60);
    let index = ((60 * aurora_tile::TILE + 60) as usize) * 4;
    let got = half::f16::from_bits(healed[index]).to_f32();
    assert!((got - truth[0]).abs() < 0.03, "{got} vs {truth:?}");
    // Finishing again heals nothing more.
    assert_eq!(spot_heal_stamp::finish(&mut drag, &mut doc), None);
    assert_eq!(dump(&mut doc, id), healed);
    let (recorded, order) = end(drag, &mut doc);
    assert!(recorded);
    assert_eq!(order.undo_labels, vec!["Spot Healing".to_owned()]);
    assert!(matches!(
        doc.pixel_history.undo(
            doc.tile_store
                .as_mut()
                .unwrap_or_else(|| unreachable!("store"))
        ),
        Ok(true)
    ));
    assert_eq!(dump(&mut doc, id), before);
}

#[test]
fn a_cancelled_stroke_changes_and_records_nothing() {
    let d = dir();
    let (mut doc, id) = session(&d, LAYER, blemished);
    let before = dump(&mut doc, id);
    let mut drag = heal_drag(&doc, id);
    spot_heal_stamp::add_dab(rect(), (60.0, 60.0), &settings(8.0), &mut drag);
    // Escape drops the drag (`App::escape_cancels_spot_heal`).
    let (recorded, order) = end(None, &mut doc);
    assert!(!recorded && order.undo_labels.is_empty());
    drop(drag);
    assert_eq!(dump(&mut doc, id), before);
    // Ending a heal drag that was never healed records nothing either (an
    // empty snapshot).
    let (recorded, _) = end(heal_drag(&doc, id), &mut doc);
    assert!(!recorded);
}

#[test]
fn refusals_change_nothing_and_record_no_step() {
    // Too little around: the layer is a 12 x 12 patch.
    let d = dir();
    let (mut doc, id) = session(&d, 12, |_, _| [0.5, 0.5, 0.5]);
    let before = dump(&mut doc, id);
    let mut drag = heal_drag(&doc, id);
    spot_heal_stamp::add_dab(rect(), (6.0, 6.0), &settings(5.0), &mut drag);
    assert_eq!(
        spot_heal_stamp::finish(&mut drag, &mut doc),
        Some(LockRefusal::HealNotEnoughSurroundings)
    );
    assert_eq!(dump(&mut doc, id), before);
    let (recorded, _) = end(drag, &mut doc);
    assert!(!recorded);
    // Too large: a stroke past the bound.
    let d2 = dir();
    let (mut doc2, id2) = session(&d2, LAYER, blemished);
    let before2 = dump(&mut doc2, id2);
    let mut drag2 = heal_drag(&doc2, id2);
    let s = settings(6.0);
    spot_heal_stamp::add_dab(rect(), (10.0, 10.0), &s, &mut drag2);
    spot_heal_stamp::add_dab(rect(), (9000.0, 10.0), &s, &mut drag2);
    assert_eq!(
        spot_heal_stamp::finish(&mut drag2, &mut doc2),
        Some(LockRefusal::HealTooLarge)
    );
    assert_eq!(dump(&mut doc2, id2), before2);
    let (recorded, _) = end(drag2, &mut doc2);
    assert!(!recorded);
}

#[test]
fn a_pixels_lock_refuses_and_a_transparency_lock_keeps_alpha() {
    let d = dir();
    let (mut doc, id) = session(&d, LAYER, blemished);
    let before = dump(&mut doc, id);
    let pixels = aurora_doc::LayerLock {
        pixels: true,
        ..aurora_doc::LayerLock::default()
    };
    assert!(doc.history.set_lock(&mut doc.layers, id, pixels).is_ok());
    let mut drag = heal_drag(&doc, id);
    spot_heal_stamp::add_dab(rect(), (60.0, 60.0), &settings(8.0), &mut drag);
    assert_eq!(
        spot_heal_stamp::finish(&mut drag, &mut doc),
        Some(LockRefusal::PixelsLocked)
    );
    assert_eq!(dump(&mut doc, id), before);
    // Transparency lock: heals, alpha bits unchanged.
    let transparency = aurora_doc::LayerLock {
        transparency: true,
        ..aurora_doc::LayerLock::default()
    };
    assert!(
        doc.history
            .set_lock(&mut doc.layers, id, transparency)
            .is_ok()
    );
    let mut drag = heal_drag(&doc, id);
    spot_heal_stamp::add_dab(rect(), (60.0, 60.0), &settings(8.0), &mut drag);
    assert_eq!(spot_heal_stamp::finish(&mut drag, &mut doc), None);
    let after = dump(&mut doc, id);
    assert_ne!(after, before);
    for (a, b) in after.chunks(4).zip(before.chunks(4)) {
        assert_eq!(a.get(3), b.get(3));
    }
}

#[test]
fn the_app_wires_the_press_the_dab_the_release_and_escape() {
    // `App` needs a window; these single-line checks pin the glue the free
    // helpers cannot.
    let source = include_str!("lib.rs");
    let production = source.split("\nmod tests {").next().unwrap_or(source);
    assert!(production.contains("&& let Some(why) = spot_heal_stamp::press("));
    assert!(production.contains("if spot_heal_stamp::is_heal(self.drag.as_ref()) {"));
    assert!(production.contains("spot_heal_stamp::add_dab(bounds, doc_point"));
    assert!(production.contains("spot_heal_stamp::finish(&mut drag, &mut self.doc)"));
    assert!(production.contains("} else if heal.is_some() {"));
    assert!(production.contains("if self.escape_cancels_spot_heal(key) {"));
    let tabs = include_str!("document_tabs.rs");
    assert!(tabs.contains("crate::spot_heal_stamp::finish(&mut ending, cx.doc)"));
}
