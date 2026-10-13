//! Persisted and saved selections and the selection-history budget
//! (0.218.0), driven through the helpers `App` calls (`App` itself needs a
//! window).

use std::sync::Arc;

use aurora_core::Rect;
use aurora_doc::{Selection, SelectionMask, SelectionSet};
use aurora_widgets::widgets::{command_palette_state, set_command_palette_query};

use crate::selection_history::{
    self, BUDGET_ENV, DEFAULT_BUDGET_BYTES, StepSelection, resolve_budget_bytes,
};
use crate::selection_store::{self, SavedSelectionCommand, StoreOutcome};
use crate::{LockRefusal, UndoDirection, UndoOrder};

fn ok<T, E: std::fmt::Debug>(r: Result<T, E>) -> T {
    match r {
        Ok(v) => v,
        Err(err) => unreachable!("{err:?}"),
    }
}

fn rect(x: i64, y: i64, width: u32, height: u32) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

/// A mask selection of `side` x `side` texels: `noise` makes the bytes
/// incompressible, otherwise they form long runs.
fn mask(side: u32, seed: u64, noise: bool) -> Selection {
    let bounds = rect(i64::from(side) / 2, 3, side, side);
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let data: Vec<u8> = (0..bounds.area_px())
        .map(|i| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            if noise {
                u8::try_from(state & 0xFF).unwrap_or(0)
            } else if (i / 50 + seed).is_multiple_of(2) {
                255
            } else {
                0
            }
        })
        .collect();
    Selection::from_mask(Arc::new(ok(SelectionMask::new(bounds, data))))
}

struct Rig {
    order: UndoOrder,
    history: aurora_doc::History,
    pixel_history: aurora_brush::PixelHistory,
    workspace: aurora_ui::Workspace,
    selection: SelectionSet,
}

impl Rig {
    fn new(budget: u64) -> Self {
        let mut workspace = aurora_ui::build_workspace(&crate::test_workspace_scales());
        workspace.tree.compute_layout(1000.0, 800.0);
        let order = UndoOrder {
            selection_budget: budget,
            ..UndoOrder::default()
        };
        Self {
            order,
            history: aurora_doc::History::new(),
            pixel_history: aurora_brush::PixelHistory::new(),
            workspace,
            selection: SelectionSet::new(),
        }
    }

    /// Makes `next` the selection as one recorded step.
    fn select(&mut self, label: &str, next: Option<Selection>) -> bool {
        let before = self.selection.active();
        match next.clone() {
            Some(s) => self.selection.select(s),
            None => self.selection.deselect(),
        }
        crate::record_selection_step(
            &mut self.workspace,
            &mut self.order,
            &mut self.history,
            &mut self.pixel_history,
            label,
            before,
            next,
        )
    }

    fn step(&mut self, direction: UndoDirection) -> bool {
        let stepped = self.order.apply_selection_step(direction);
        crate::selection_view::apply_pending_selection(&mut self.order, &mut self.selection);
        stepped
    }

    fn labels(&self) -> Vec<String> {
        self.order
            .history_steps()
            .iter()
            .map(|s| format!("{}{}", s.label, if s.undone { " (undone)" } else { "" }))
            .collect()
    }
}

#[test]
fn under_budget_nothing_is_packed_and_a_step_is_always_kept() {
    let mut rig = Rig::new(DEFAULT_BUDGET_BYTES);
    for seed in 0..5 {
        assert!(rig.select("Lasso", Some(mask(40, seed, false))));
    }
    let stats = rig.order.selection_history_stats();
    assert_eq!(stats.steps, 5);
    assert_eq!(stats.packed_bytes, 0);
    assert!(!stats.over_budget);
    assert!(
        rig.order
            .selection_undo
            .iter()
            .all(|s| matches!(s.after, StepSelection::Live(_)))
    );
}

#[test]
fn over_budget_packs_the_oldest_first_and_undo_restores_every_mask_exactly() {
    // Each 60 x 60 mask is 3,600 bytes; a budget of three masks.
    let mut rig = Rig::new(12_000);
    let masks: Vec<Selection> = (0..8).map(|seed| mask(60, seed, false)).collect();
    for m in &masks {
        assert!(rig.select("Lasso", Some(m.clone())));
    }
    let stats = rig.order.selection_history_stats();
    assert_eq!(stats.steps, 8, "no step is dropped");
    assert!(stats.packed_bytes > 0, "{stats:?}");
    assert!(stats.total_bytes() <= 12_000, "{stats:?}");
    // Oldest packed, newest still live.
    let first = rig.order.selection_undo.first();
    assert!(first.is_some_and(|s| matches!(s.after, StepSelection::Packed(_))));
    let last = rig.order.selection_undo.last();
    assert!(last.is_some_and(|s| matches!(s.after, StepSelection::Live(_))));
    // Undo walks back through exact masks, packed or not.
    for expected in masks.iter().rev().skip(1) {
        assert!(rig.step(UndoDirection::Undo));
        assert_eq!(rig.selection.active().as_ref(), Some(expected));
    }
    assert!(rig.step(UndoDirection::Undo));
    assert_eq!(rig.selection.active(), None);
    // And redo forwards again, still exact.
    for expected in &masks {
        assert!(rig.step(UndoDirection::Redo));
        assert_eq!(rig.selection.active().as_ref(), Some(expected));
    }
    assert_eq!(rig.labels().len(), 8);
}

#[test]
fn a_mask_shared_by_two_steps_is_counted_and_packed_once() {
    let mut rig = Rig::new(DEFAULT_BUDGET_BYTES);
    let (a, b, c) = (mask(40, 1, false), mask(40, 2, false), mask(40, 3, false));
    rig.select("one", Some(a));
    rig.select("two", Some(b));
    rig.select("three", Some(c));
    // Steps: None->a, a->b, b->c: three distinct masks, six references.
    let stats = rig.order.selection_history_stats();
    assert_eq!(stats.live_bytes, 3 * 1_600);
    // Pack everything with a tiny budget: still one packed copy per mask.
    rig.order.selection_budget = 1;
    let stats = rig.order.trim_selection_history();
    assert_eq!(stats.live_bytes, 0);
    let packed: std::collections::HashSet<_> = rig
        .order
        .selection_undo
        .iter()
        .flat_map(|s| [&s.before, &s.after])
        .filter_map(|side| match side {
            StepSelection::Packed(p) => Some(p.mask_ptr()),
            StepSelection::Live(_) => None,
        })
        .collect();
    assert_eq!(packed.len(), 3, "the shared masks were packed once each");
}

#[test]
fn incompressible_masks_over_budget_keep_every_step_and_say_so() {
    let mut rig = Rig::new(1_000);
    for seed in 0..4 {
        assert!(rig.select("Lasso", Some(mask(64, seed, true))));
    }
    let stats = rig.order.selection_history_stats();
    assert_eq!(stats.steps, 4);
    assert!(stats.over_budget, "{stats:?}");
    assert_eq!(stats.live_bytes, 0, "everything was packed");
    for _ in 0..4 {
        assert!(rig.step(UndoDirection::Undo));
    }
    assert_eq!(rig.selection.active(), None);
}

#[test]
fn the_budget_override_is_parsed_and_bounded() {
    assert_eq!(resolve_budget_bytes(None), DEFAULT_BUDGET_BYTES);
    assert_eq!(resolve_budget_bytes(Some("64")), 64 * 1024 * 1024);
    assert_eq!(resolve_budget_bytes(Some(" 1 ")), 1024 * 1024);
    for bad in ["", "0", "16385", "-3", "lots"] {
        assert_eq!(
            resolve_budget_bytes(Some(bad)),
            DEFAULT_BUDGET_BYTES,
            "{bad:?}"
        );
    }
    assert!(BUDGET_ENV.starts_with("AURORA_"));
    let _ = selection_history::system_budget_bytes();
}

#[test]
fn load_selection_is_one_undo_step_that_undoes_to_the_previous_selection() {
    let mut rig = Rig::new(DEFAULT_BUDGET_BYTES);
    let kept = Selection::new(rect(5, 5, 20, 20));
    rig.select("Rectangular Marquee", Some(kept.clone()));
    assert_eq!(
        selection_store::save(&mut rig.order, &mut rig.selection, "Kept"),
        StoreOutcome::MetadataChanged
    );
    let other = Selection::ellipse(rect(40, 40, 30, 30));
    rig.select("Elliptical Marquee", Some(other.clone()));
    let steps_before = rig.order.undo.len();
    let outcome = selection_store::load(
        &mut rig.workspace,
        &mut rig.order,
        &mut rig.history,
        &mut rig.pixel_history,
        &mut rig.selection,
        "Kept",
    );
    assert_eq!(outcome, StoreOutcome::Loaded);
    assert_eq!(rig.order.undo.len(), steps_before + 1, "exactly one step");
    assert_eq!(rig.selection.active(), Some(kept.clone()));
    assert_eq!(
        rig.labels().last().map(String::as_str),
        Some("Load Selection")
    );
    // Undo goes back to the selection before the load, redo forward again.
    assert!(rig.step(UndoDirection::Undo));
    assert_eq!(rig.selection.active(), Some(other));
    assert!(rig.step(UndoDirection::Redo));
    assert_eq!(rig.selection.active(), Some(kept));
    // The saved selection survives undo and redo.
    assert_eq!(rig.selection.saved_count(), 1);
    // Loading what is already active records nothing; an unknown name too.
    let again = selection_store::load(
        &mut rig.workspace,
        &mut rig.order,
        &mut rig.history,
        &mut rig.pixel_history,
        &mut rig.selection,
        "Kept",
    );
    assert_eq!(again, StoreOutcome::Unchanged);
    let unknown = selection_store::load(
        &mut rig.workspace,
        &mut rig.order,
        &mut rig.history,
        &mut rig.pixel_history,
        &mut rig.selection,
        "Nope",
    );
    assert_eq!(unknown, StoreOutcome::Unchanged);
}

#[test]
fn saving_and_deleting_are_metadata_never_a_step_and_never_dirty() {
    let mut rig = Rig::new(DEFAULT_BUDGET_BYTES);
    rig.select("Select All", Some(Selection::new(rect(0, 0, 10, 10))));
    let (content, revision, steps) = (
        rig.order.content_revision,
        rig.order.revision,
        rig.order.undo.len(),
    );
    assert_eq!(
        selection_store::save(&mut rig.order, &mut rig.selection, "A"),
        StoreOutcome::MetadataChanged
    );
    assert_eq!(rig.order.content_revision, content, "not dirty");
    assert_ne!(rig.order.revision, revision, "but autosaved");
    assert_eq!(rig.order.undo.len(), steps, "no History row");
    let revision = rig.order.revision;
    assert_eq!(
        selection_store::delete(&mut rig.order, &mut rig.selection, "A"),
        StoreOutcome::MetadataChanged
    );
    assert_eq!(rig.order.content_revision, content);
    assert_ne!(rig.order.revision, revision);
    assert_eq!(
        selection_store::delete(&mut rig.order, &mut rig.selection, "A"),
        StoreOutcome::Unchanged
    );
    // A selection step itself is dirty-neutral too.
    rig.select("Deselect", None);
    assert_eq!(rig.order.content_revision, content);
}

#[test]
fn save_refusals_are_calm_and_a_replaced_name_does_not_count_twice() {
    let mut rig = Rig::new(DEFAULT_BUDGET_BYTES);
    assert_eq!(
        selection_store::save(&mut rig.order, &mut rig.selection, "A"),
        StoreOutcome::Refused(LockRefusal::SavedSelectionNeedsSelection)
    );
    rig.select("Select All", Some(Selection::new(rect(0, 0, 10, 10))));
    for i in 0..aurora_doc::MAX_SAVED_SELECTIONS {
        assert_eq!(
            selection_store::save(&mut rig.order, &mut rig.selection, &format!("s{i}")),
            StoreOutcome::MetadataChanged
        );
    }
    assert_eq!(
        selection_store::save(&mut rig.order, &mut rig.selection, "one too many"),
        StoreOutcome::Refused(LockRefusal::SavedSelectionsFull)
    );
    // Replacing an existing name at the cap is allowed.
    assert_eq!(
        selection_store::save(&mut rig.order, &mut rig.selection, "s0"),
        StoreOutcome::MetadataChanged
    );
    assert_eq!(
        rig.selection.saved_count(),
        aurora_doc::MAX_SAVED_SELECTIONS
    );
}

#[test]
fn the_name_prompt_validates_and_the_palette_lists_loads_and_deletes() {
    let mut rig = Rig::new(DEFAULT_BUDGET_BYTES);
    let mut focus = aurora_widgets::FocusManager::default();
    let mut palette = None;
    assert!(selection_store::open_prompt(
        &mut rig.workspace,
        &mut focus,
        &mut palette
    ));
    let Some(root) = palette else {
        unreachable!("opened");
    };
    // Empty: refused, prompt stays, the reason is described.
    assert_eq!(
        selection_store::prompt_enter(&mut rig.workspace.tree, root),
        Some(Err(()))
    );
    let state = ok(command_palette_state(&rig.workspace.tree, root));
    assert_eq!(state.message(), Some("Type a name for the selection."));
    // Typing withdraws the refusal and the row follows the text.
    ok(set_command_palette_query(
        &mut rig.workspace.tree,
        root,
        "  Sky  ",
    ));
    assert!(selection_store::follow_prompt_query(
        &mut rig.workspace.tree,
        root
    ));
    let state = ok(command_palette_state(&rig.workspace.tree, root));
    assert_eq!(state.message(), None);
    assert_eq!(
        selection_store::prompt_enter(&mut rig.workspace.tree, root),
        Some(Ok("Sky".to_owned()))
    );
    for bad in [
        "x".repeat(65),
        "tab\there".to_owned(),
        "\u{202e}x".to_owned(),
    ] {
        ok(set_command_palette_query(
            &mut rig.workspace.tree,
            root,
            &bad,
        ));
        assert_eq!(
            selection_store::prompt_enter(&mut rig.workspace.tree, root),
            Some(Err(()))
        );
    }
    // The ids.
    assert_eq!(
        selection_store::command_for("select.save"),
        Some(SavedSelectionCommand::Prompt)
    );
    assert_eq!(
        selection_store::command_for("select.load:Sky"),
        Some(SavedSelectionCommand::Load("Sky".to_owned()))
    );
    assert_eq!(
        selection_store::command_for("select.delete_saved:a:b"),
        Some(SavedSelectionCommand::Delete("a:b".to_owned()))
    );
    assert_eq!(selection_store::command_for("select.all"), None);
    // Dynamic entries, sorted.
    rig.select("Select All", Some(Selection::new(rect(0, 0, 4, 4))));
    for name in ["Zebra", "Apple"] {
        selection_store::save(&mut rig.order, &mut rig.selection, name);
    }
    let titles: Vec<String> = selection_store::palette_entries(&rig.selection)
        .into_iter()
        .map(|e| e.title)
        .collect();
    assert_eq!(
        titles,
        [
            "Load Selection: Apple",
            "Load Selection: Zebra",
            "Delete Saved Selection: Apple",
            "Delete Saved Selection: Zebra"
        ]
    );
}

#[test]
fn the_psd_report_names_the_selection_only_when_there_is_one() {
    let mut set = SelectionSet::new();
    assert_eq!(crate::psd_save::selection_loss_line(&set), None);
    set.select(Selection::new(rect(0, 0, 3, 3)));
    let line = crate::psd_save::selection_loss_line(&set).unwrap_or_default();
    assert!(line.contains("not saved"), "{line}");
    ok(set.save_active("x"));
    set.deselect();
    let saved_only = crate::psd_save::selection_loss_line(&set).unwrap_or_default();
    assert!(saved_only.contains("Saved selections"), "{saved_only}");
}

// -- Autosave and recovery -------------------------------------------------

fn new_store() -> (tempfile::TempDir, aurora_tile::TileStore) {
    let dir = ok(tempfile::tempdir());
    let Some(budget) = std::num::NonZeroUsize::new(16) else {
        unreachable!("non-zero")
    };
    let store = ok(aurora_tile::TileStore::new(
        dir.path().to_path_buf(),
        budget,
    ));
    (dir, store)
}

fn rich() -> SelectionSet {
    let mut inverted = mask(30, 9, false);
    inverted.inverted = true;
    let mut set = SelectionSet::new();
    set.select(inverted.clone());
    ok(set.save_active("Hair"));
    set.select(Selection::ellipse(rect(2, 2, 9, 5)));
    ok(set.save_active("Oval"));
    set.select(inverted);
    set
}

fn tree() -> aurora_doc::LayerTree {
    let mut tree = aurora_doc::LayerTree::new();
    ok(tree.add_pixel_layer("A", rect(0, 0, 8, 8), None));
    tree
}

#[test]
fn a_synchronous_autosave_restores_the_selection_on_recovery() {
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("aurora-autosave-test.aur");
    let (_s, mut store) = new_store();
    let set = rich();
    crate::write_autosave(
        &path,
        &tree(),
        &aurora_doc::History::new(),
        (8, 8),
        &mut aurora_io::SkippedTiles::new(),
        &mut store,
        None,
        Some(&set),
    );
    let (_s2, mut fresh) = new_store();
    let Some(recovered) = crate::recover_document(&path, &mut fresh) else {
        unreachable!("recovers");
    };
    assert_eq!(recovered.selection.active(), set.active());
    assert_eq!(recovered.selection.saved_sorted(), set.saved_sorted());
}

#[test]
fn a_snapshot_autosave_carries_the_selection_and_counts_its_bytes() {
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("aurora-autosave-snap.aur");
    let set = rich();
    let bytes = set.active().map_or(0, |s| s.mask_bytes()) + set.saved_mask_bytes();
    assert!(bytes > 0);
    let layers = tree();
    let take =
        |budget: usize, selection: Option<&SelectionSet>, store: &mut aurora_tile::TileStore| {
            crate::snapshot_autosave(
                &path,
                &layers,
                &aurora_doc::History::new(),
                (8, 8),
                &mut aurora_io::SkippedTiles::new(),
                budget,
                store,
                None,
                selection,
            )
        };
    let (_s, mut store) = new_store();
    // One real tile, so the tiles alone have a non-zero bound.
    let layer_surface = layers
        .roots()
        .first()
        .and_then(|id| layers.surface_id(*id))
        .unwrap_or_else(|| unreachable!("a pixel layer"));
    if let Some(texel) = ok(store.get_mut(layer_surface, aurora_tile::TileId { x: 0, y: 0 }))
        .texels_mut()
        .first_mut()
    {
        *texel = half::f16::from_f32(1.0);
    }
    // The smallest bound the tiles alone pass (found, not assumed).
    let (mut lo, mut hi) = (0_usize, 1_usize << 24);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if matches!(
            take(mid, None, &mut store),
            crate::SnapshotOutcome::Taken(_)
        ) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    let base = lo;
    assert!(base > 0, "the tile counts against the bound");
    let budget = base + usize::try_from(bytes).unwrap_or(usize::MAX);
    let crate::SnapshotOutcome::Taken(job) = take(budget, Some(&set), &mut store) else {
        unreachable!("the tiles plus exactly the selection's bytes fit");
    };
    assert!(
        matches!(
            take(budget - 1, Some(&set), &mut store),
            crate::SnapshotOutcome::OverBudget
        ),
        "one byte less does not: the selection is counted against the bound"
    );
    let mut out = std::io::Cursor::new(Vec::new());
    ok(aurora_io::write_aur_snapshot(&mut out, &job.snapshot));
    let (_s2, mut fresh) = new_store();
    let document = ok(aurora_io::read_aur(
        std::io::Cursor::new(out.into_inner()),
        &mut fresh,
    ));
    assert_eq!(document.selection.active(), set.active());
    assert_eq!(document.selection.saved_sorted(), set.saved_sorted());
}

#[test]
fn quick_mask_does_not_change_what_is_persisted() {
    // While the mode is on the grid is separate from the selection in
    // force, which is what a save and an autosave write.
    let set = rich();
    let before = set.saved_sorted().len();
    let active = set.active();
    let mode = ok(crate::quick_mask::enter(active.as_ref(), (64, 64)));
    drop(mode);
    assert_eq!(set.saved_sorted().len(), before);
    assert_eq!(set.active(), active);
}
