//! The Healing Brush (0.314.0) at the app level: the wiring of the tool
//! (shortcut, palette, label, locks, Quick Mask, notices), the Clone
//! Stamp-style source and aligned offset it keeps, the heal at the end of
//! the stroke into one undo step, and the refusals. The pixel model is
//! tested in `aurora-brush`.

use crate::clone_stamp::{CloneStamp, Press};
use crate::document_session::{DocumentContents, DocumentSession};
use crate::{Drag, LockRefusal, PointerButton, ToolSettings, UndoOrder, spot_heal_stamp};
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

fn session(
    dir: &tempfile::TempDir,
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
    for y in 0..LAYER {
        for x in 0..LAYER {
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

/// Period-6 stripes: a copy shifted by 12 columns is identical.
fn stripes(x: u32, y: u32) -> [f32; 3] {
    [
        if x % 6 < 3 { 0.8 } else { 0.2 },
        0.3 + 0.1 * (y % 4) as f32 / 4.0,
        0.5,
    ]
}

fn blemished(x: u32, y: u32) -> [f32; 3] {
    let d2 = (x as f32 - 40.0).powi(2) + (y as f32 - 60.0).powi(2);
    if d2 < 25.0 {
        [1.0, 0.0, 0.0]
    } else {
        stripes(x, y)
    }
}

fn drag_for(id: aurora_doc::LayerId) -> Option<Drag> {
    crate::begin_drag(
        Tool::HealingBrush,
        PointerButton::Primary,
        (40.0, 60.0),
        &aurora_ui::CanvasView::new(),
        Some((id, rect())),
    )
}

fn settings(radius: f32) -> ToolSettings {
    let mut s = ToolSettings::default();
    s.set_radius(Tool::HealingBrush, f64::from(radius));
    s
}

fn press(state: &mut CloneStamp, alt: bool, id: aurora_doc::LayerId, at: (f32, f32)) -> Press {
    spot_heal_stamp::healing_press(state, alt, Some(id), None, at)
}

fn layer_id() -> aurora_doc::LayerId {
    let mut tree = aurora_doc::LayerTree::new();
    match tree.add_pixel_layer("L".to_owned(), rect(), None) {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    }
}

#[test]
fn the_tool_is_wired_into_the_shortcuts_the_palette_and_the_tools_panel() {
    use aurora_widgets::shortcut::{Key, KeyChord, Modifiers};
    let shift = Modifiers {
        shift: true,
        ..Modifiers::none()
    };
    let table = crate::default_shortcuts();
    assert_eq!(
        table
            .resolve(KeyChord::new(shift, Key::Character('j')))
            .copied(),
        Some(crate::AppCommand::SelectTool(Tool::HealingBrush))
    );
    // Bare J is still the Spot Healing.
    assert_eq!(
        table
            .resolve(KeyChord::new(Modifiers::none(), Key::Character('j')))
            .copied(),
        Some(crate::AppCommand::SelectTool(Tool::SpotHealing))
    );
    assert_eq!(Tool::HealingBrush.shortcut_text(), "Shift+J");
    assert_eq!(
        crate::tool_command_id(Tool::HealingBrush),
        "tool.healing_brush"
    );
    assert!(
        crate::palette_commands()
            .iter()
            .any(|c| c.id == "tool.healing_brush" && c.title == "Select Healing Brush Tool")
    );
    assert_eq!(
        aurora_ui::tool_icon(Tool::HealingBrush),
        aurora_widgets::icons::IconKind::HealingBrush
    );
    assert_eq!(
        aurora_ui::tool_description(Tool::HealingBrush),
        "Healing Brush tool, shortcut Shift+J"
    );
    assert_eq!(spot_heal_stamp::HEALING_BRUSH_LABEL, "Healing Brush");
    assert_eq!(
        LockRefusal::HealNeedsSource.message(),
        "Alt-click to set a healing source."
    );
    // The radius setting is the Brush's.
    let mut s = ToolSettings::default();
    assert!(s.set_radius(Tool::HealingBrush, 33.0));
    assert_eq!(s.radius(Tool::Brush), Some(33.0));
    assert_eq!(s.radius(Tool::HealingBrush), Some(33.0));
}

#[test]
fn a_press_begins_a_heal_drag_and_the_label_follows_the_offset() {
    let drag = drag_for(layer_id());
    assert!(spot_heal_stamp::is_heal(drag.as_ref()));
    let Some(Drag::Brush { heal, .. }) = &drag else {
        unreachable!("brush drag");
    };
    assert_eq!(spot_heal_stamp::label(heal.as_deref()), "Spot Healing");
    let mut drag = drag;
    spot_heal_stamp::set_drag_offset(&mut drag, (24, 0));
    let Some(Drag::Brush { heal, .. }) = &drag else {
        unreachable!("brush drag");
    };
    assert_eq!(spot_heal_stamp::label(heal.as_deref()), "Healing Brush");
    // Setting an offset on a drag that is not a heal does nothing.
    let mut other = crate::begin_drag(
        Tool::Brush,
        PointerButton::Primary,
        (1.0, 1.0),
        &aurora_ui::CanvasView::new(),
        None,
    );
    spot_heal_stamp::set_drag_offset(&mut other, (1, 1));
    assert!(!spot_heal_stamp::is_heal(other.as_ref()));
}

#[test]
fn alt_click_sets_the_source_the_first_press_fixes_the_offset_and_it_persists() {
    let id = layer_id();
    let mut state = CloneStamp::default();
    // No source yet: the calm notice, and the press fixes nothing.
    assert_eq!(
        press(&mut state, false, id, (40.0, 60.0)),
        Press::Refused(LockRefusal::HealNeedsSource)
    );
    // Alt+click sets the source and starts no stroke.
    assert_eq!(press(&mut state, true, id, (64.0, 60.0)), Press::SourceSet);
    // The first plain press fixes round(source - press).
    assert_eq!(
        press(&mut state, false, id, (40.0, 60.0)),
        Press::Stroke((24, 0))
    );
    // Later strokes keep it wherever they start.
    assert_eq!(
        press(&mut state, false, id, (90.0, 20.0)),
        Press::Stroke((24, 0))
    );
    // A new Alt+click forgets it.
    assert_eq!(press(&mut state, true, id, (50.5, 70.0)), Press::SourceSet);
    assert_eq!(
        press(&mut state, false, id, (10.0, 10.0)),
        Press::Stroke((41, 60))
    );
    // A refused press (a lock) does not fix the offset of a fresh source.
    assert_eq!(press(&mut state, true, id, (0.0, 0.0)), Press::SourceSet);
    assert_eq!(
        spot_heal_stamp::healing_press(
            &mut state,
            false,
            Some(id),
            Some(LockRefusal::PixelsLocked),
            (5.0, 5.0)
        ),
        Press::Refused(LockRefusal::PixelsLocked)
    );
    assert_eq!(
        press(&mut state, false, id, (3.0, 4.0)),
        Press::Stroke((-3, -4))
    );
    // No pixel layer: the Spot Healing's notice.
    assert_eq!(
        spot_heal_stamp::healing_press(&mut state, true, None, None, (1.0, 1.0)),
        Press::Refused(LockRefusal::SpotHealNeedsPixelLayer)
    );
}

#[test]
fn the_source_is_reset_by_a_layer_change_and_kept_apart_from_the_clone_stamps() {
    let (a, b) = (layer_id(), {
        let mut tree = aurora_doc::LayerTree::new();
        let _ = tree.add_pixel_layer("A".to_owned(), rect(), None);
        match tree.add_pixel_layer("B".to_owned(), rect(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        }
    });
    let mut state = CloneStamp::default();
    assert_eq!(press(&mut state, true, a, (10.0, 10.0)), Press::SourceSet);
    state.observe_layer(Some(a));
    assert!(matches!(
        press(&mut state, false, a, (0.0, 0.0)),
        Press::Stroke(_)
    ));
    // The active layer changes: the source is forgotten.
    state.observe_layer(Some(b));
    assert_eq!(
        press(&mut state, false, b, (0.0, 0.0)),
        Press::Refused(LockRefusal::HealNeedsSource)
    );
    // A Clone Stamp source does not feed the Healing Brush.
    let d = dir();
    let (mut doc, id) = session(&d, stripes);
    doc.clone_stamp.set_source(id, (5.0, 5.0));
    assert_eq!(
        press(&mut doc.healing_source, false, id, (0.0, 0.0)),
        Press::Refused(LockRefusal::HealNeedsSource)
    );
    // The glue clears it with the Clone Stamp's on a document switch and
    // observes the layer every turn.
    let tabs = include_str!("document_tabs.rs").replace("\r\n", "\n");
    assert!(tabs.contains("cx.doc.healing_source.clear();"));
    let lib = include_str!("lib.rs").replace("\r\n", "\n");
    assert!(lib.contains("self.doc.healing_source.observe_layer(self.doc.active_layer);"));
    assert!(lib.contains("spot_heal_stamp::healing_press("));
    assert!(lib.contains("spot_heal_stamp::set_drag_offset(&mut self.drag, offset);"));
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
fn the_release_heals_from_the_offset_as_one_step_named_healing_brush() {
    let d = dir();
    let (mut doc, id) = session(&d, blemished);
    let before = dump(&mut doc, id);
    let mut state = CloneStamp::default();
    assert_eq!(press(&mut state, true, id, (64.0, 60.0)), Press::SourceSet);
    let Press::Stroke(offset) = press(&mut state, false, id, (40.0, 60.0)) else {
        unreachable!("stroke");
    };
    let mut drag = drag_for(id);
    spot_heal_stamp::set_drag_offset(&mut drag, offset);
    spot_heal_stamp::add_dab(rect(), (40.0, 60.0), &settings(8.0), &mut drag);
    assert_eq!(
        dump(&mut doc, id),
        before,
        "nothing is written while painting"
    );
    assert_eq!(spot_heal_stamp::finish(&mut drag, &mut doc), None);
    let healed = dump(&mut doc, id);
    assert_ne!(healed, before);
    // The source is the same stripes 12 columns away, so the blemish is
    // replaced by exactly the stripes' colour at the centre.
    let truth = stripes(40, 60);
    let index = ((60 * aurora_tile::TILE + 40) as usize) * 4;
    let got = half::f16::from_bits(healed.get(index).copied().unwrap_or(0)).to_f32();
    let row: Vec<f32> = (30..52)
        .map(|x| {
            half::f16::from_bits(
                healed
                    .get(((60 * aurora_tile::TILE + x) as usize) * 4)
                    .copied()
                    .unwrap_or(0),
            )
            .to_f32()
        })
        .collect();
    assert!(
        (got - truth[0]).abs() < 0.03,
        "{got} vs {truth:?} row {row:?}"
    );
    let (recorded, order) = end(drag, &mut doc);
    assert!(recorded);
    assert_eq!(order.undo_labels, vec!["Healing Brush".to_owned()]);
    assert!(matches!(
        doc.pixel_history.undo(
            doc.tile_store
                .as_mut()
                .unwrap_or_else(|| unreachable!("store"))
        ),
        Ok(true)
    ));
    assert_eq!(dump(&mut doc, id), before);
    assert!(matches!(
        doc.pixel_history.redo(
            doc.tile_store
                .as_mut()
                .unwrap_or_else(|| unreachable!("store"))
        ),
        Ok(true)
    ));
    assert_eq!(dump(&mut doc, id), healed);
}

#[test]
fn a_different_offset_gives_a_different_result() {
    let run = |offset: (i64, i64)| {
        let d = dir();
        let (mut doc, id) = session(&d, blemished);
        let mut drag = drag_for(id);
        spot_heal_stamp::set_drag_offset(&mut drag, offset);
        spot_heal_stamp::add_dab(rect(), (40.0, 60.0), &settings(8.0), &mut drag);
        assert_eq!(spot_heal_stamp::finish(&mut drag, &mut doc), None);
        dump(&mut doc, id)
    };
    assert_ne!(run((24, 0)), run((15, 0)));
    assert_eq!(run((24, 0)), run((24, 0)));
}

#[test]
fn a_source_outside_the_layer_is_refused_with_its_notice_and_records_nothing() {
    let d = dir();
    let (mut doc, id) = session(&d, blemished);
    let before = dump(&mut doc, id);
    let mut drag = drag_for(id);
    spot_heal_stamp::set_drag_offset(&mut drag, (-200, 0));
    spot_heal_stamp::add_dab(rect(), (40.0, 60.0), &settings(8.0), &mut drag);
    assert_eq!(
        spot_heal_stamp::finish(&mut drag, &mut doc),
        Some(LockRefusal::HealSourceOutside)
    );
    assert_eq!(dump(&mut doc, id), before);
    let (recorded, _) = end(drag, &mut doc);
    assert!(!recorded);
    // Too large still says so.
    let mut big = drag_for(id);
    spot_heal_stamp::set_drag_offset(&mut big, (24, 0));
    let s = settings(6.0);
    spot_heal_stamp::add_dab(rect(), (10.0, 10.0), &s, &mut big);
    spot_heal_stamp::add_dab(rect(), (9000.0, 10.0), &s, &mut big);
    assert_eq!(
        spot_heal_stamp::finish(&mut big, &mut doc),
        Some(LockRefusal::HealTooLarge)
    );
}

#[test]
fn locks_notices_and_quick_mask_follow_the_spot_healing() {
    let pixels = aurora_doc::LayerLock {
        pixels: true,
        ..aurora_doc::LayerLock::default()
    };
    let transparency = aurora_doc::LayerLock {
        transparency: true,
        ..aurora_doc::LayerLock::default()
    };
    assert_eq!(
        crate::press_refusal(Tool::HealingBrush, pixels),
        Some(LockRefusal::PixelsLocked)
    );
    assert_eq!(crate::press_refusal(Tool::HealingBrush, transparency), None);
    for refusal in [
        LockRefusal::SpotHealNeedsPixelLayer,
        LockRefusal::HealNotEnoughSurroundings,
        LockRefusal::HealTooLarge,
        LockRefusal::HealNeedsSource,
        LockRefusal::HealSourceOutside,
    ] {
        assert!(crate::lock_refusal_applies(
            refusal,
            Tool::HealingBrush,
            aurora_doc::LayerLock::default()
        ));
        assert!(!crate::lock_refusal_applies(
            refusal,
            Tool::Brush,
            aurora_doc::LayerLock::default()
        ));
    }
    // The source notices belong to the Healing Brush alone.
    assert!(!crate::lock_refusal_applies(
        LockRefusal::HealNeedsSource,
        Tool::SpotHealing,
        aurora_doc::LayerLock::default()
    ));
    assert_eq!(
        crate::quick_mask::tool_refusal(Tool::HealingBrush),
        Some(LockRefusal::QuickMaskToolUnavailable)
    );
    // A transparency lock keeps every alpha bit.
    let d = dir();
    let (mut doc, id) = session(&d, blemished);
    let before = dump(&mut doc, id);
    assert!(
        doc.history
            .set_lock(&mut doc.layers, id, transparency)
            .is_ok()
    );
    let mut drag = drag_for(id);
    spot_heal_stamp::set_drag_offset(&mut drag, (24, 0));
    spot_heal_stamp::add_dab(rect(), (40.0, 60.0), &settings(8.0), &mut drag);
    assert_eq!(spot_heal_stamp::finish(&mut drag, &mut doc), None);
    let after = dump(&mut doc, id);
    assert_ne!(after, before);
    for (a, b) in after.chunks(4).zip(before.chunks(4)) {
        assert_eq!(a.get(3), b.get(3));
    }
}

#[test]
fn the_app_wires_the_press_the_offset_and_the_label() {
    let source = include_str!("lib.rs").replace("\r\n", "\n");
    let production = source.split("\nmod tests {").next().unwrap_or(&source);
    assert!(production.contains("if self.tool == aurora_ui::Tool::HealingBrush && button =="));
    assert!(production.contains("spot_heal_stamp::label(heal.as_deref())"));
    assert!(production.contains("aurora_ui::Tool::SpotHealing | aurora_ui::Tool::HealingBrush"));
}
