//! Tests for the save report's structure (task 157): every writer source
//! lands in the category the report model documents, the layer names travel
//! with the entries, `items` keeps meaning "what makes the save lossy", and
//! Kept lines never count as a change.

use aurora_core::effects::{LayerEffects, Stroke};
use aurora_doc::BlendMode;

use super::write::{PsdSaveOptions, psd_save_preflight};
use super::write_effects_tests::{gradient, gradient_overlay};
use super::write_report::{PsdWriteReport, ReportCategory, ReportEntry};
use super::write_tests::{Doc, ok, rect};

fn entry_with<'a>(report: &'a PsdWriteReport, needle: &str) -> &'a ReportEntry {
    let found: Vec<&ReportEntry> = report
        .entries
        .iter()
        .filter(|e| e.text.contains(needle))
        .collect();
    assert_eq!(found.len(), 1, "{needle}: {:?}", report.entries);
    found.first().copied().unwrap_or_else(|| unreachable!())
}

#[test]
fn from_entries_keeps_items_for_asking_categories_only_and_in_order() {
    let report = PsdWriteReport::from_entries(vec![
        ReportEntry::new(ReportCategory::Changed, "a"),
        ReportEntry::new(ReportCategory::Kept, "kept"),
        ReportEntry::new(ReportCategory::NotSaved, "b"),
    ]);
    assert_eq!(report.items, ["a", "b"]);
    assert_eq!(report.entries.len(), 3);
    assert!(!report.is_empty());
    let only_kept =
        PsdWriteReport::from_entries(vec![ReportEntry::new(ReportCategory::Kept, "kept")]);
    assert!(only_kept.is_empty(), "a Kept-only report is lossless");
    assert_eq!(only_kept.entries.len(), 1);
}

#[test]
fn extend_entries_keeps_items_in_step() {
    let mut report = PsdWriteReport::default();
    report.extend_entries([
        ReportEntry::new(ReportCategory::Kept, "kept"),
        ReportEntry::new(ReportCategory::FromOpen, "open"),
        ReportEntry::new(ReportCategory::NeedsAttention, "attn"),
    ]);
    assert_eq!(report.items, ["open", "attn"]);
    assert_eq!(report.entries.len(), 3);
}

#[test]
fn only_kept_does_not_ask() {
    for category in ReportCategory::ALL {
        assert_eq!(
            category.asks(),
            category != ReportCategory::Kept,
            "{category:?}"
        );
    }
}

#[test]
fn with_affected_sorts_and_dedupes() {
    let e = ReportEntry::new(ReportCategory::Changed, "x").with_affected(&["b", "a", "b"]);
    assert_eq!(e.affected, ["a", "b"]);
}

#[test]
fn rounding_and_shortening_are_changed_and_dissolve_needs_attention() {
    let long: String = "名".repeat(300);
    let mut doc = Doc::new((4, 4));
    let a = doc.layer(&long, rect(0, 0, 4, 4), None);
    let b = doc.layer("B", rect(0, 0, 4, 4), None);
    ok(doc.tree.set_opacity(b, 0.3333));
    ok(doc.tree.set_blend_mode(a, BlendMode::Dissolve));
    let report = ok(psd_save_preflight(
        &doc.tree,
        (4, 4),
        PsdSaveOptions::default(),
    ));
    assert_eq!(
        entry_with(&report, "will be shortened").category,
        ReportCategory::Changed
    );
    assert_eq!(
        entry_with(&report, "rounded to Photoshop's 255 steps").category,
        ReportCategory::Changed
    );
    assert_eq!(
        entry_with(&report, "Dissolve layers").category,
        ReportCategory::NeedsAttention
    );
    assert_eq!(report.items.len(), report.entries.len());
}

#[test]
fn a_written_effect_needs_attention_and_an_unwritten_kind_is_not_saved_with_names() {
    let mut doc = Doc::new((4, 4));
    let plain = doc.layer("Stroked", rect(0, 0, 4, 4), None);
    let grad = doc.layer("Graded", rect(0, 0, 4, 4), None);
    ok(doc.tree.set_effects(
        plain,
        Some(LayerEffects {
            strokes: vec![Stroke::default()],
            ..LayerEffects::default()
        }),
    ));
    ok(doc.tree.set_effects(
        grad,
        Some(LayerEffects {
            gradient_overlays: vec![gradient_overlay(gradient())],
            ..LayerEffects::default()
        }),
    ));
    let report = ok(psd_save_preflight(
        &doc.tree,
        (4, 4),
        PsdSaveOptions::default(),
    ));
    let checked = entry_with(&report, "Aurora hasn't checked them in Photoshop");
    assert_eq!(checked.category, ReportCategory::NeedsAttention);
    let unverified = entry_with(&report, "unverified in Photoshop): gradient overlay");
    assert_eq!(unverified.category, ReportCategory::NotSaved);
    assert_eq!(unverified.affected, ["Graded"]);
}

#[test]
fn the_vector_outline_line_is_not_saved_and_names_its_layers() {
    let mut doc = Doc::new((4, 4));
    let shape = doc.layer("Shape", rect(0, 0, 4, 4), None);
    ok(doc.tree.set_effects(
        shape,
        Some(LayerEffects {
            strokes: vec![Stroke::default()],
            ..LayerEffects::default()
        }),
    ));
    let outline = ok(aurora_doc::EffectShape::new(
        rect(0, 0, 2, 2),
        vec![255; 4],
        1,
    ));
    ok(doc
        .tree
        .set_effect_shape(shape, Some(std::sync::Arc::new(outline))));
    let report = ok(psd_save_preflight(
        &doc.tree,
        (4, 4),
        PsdSaveOptions::default(),
    ));
    let line = entry_with(&report, "vector outline");
    assert_eq!(line.category, ReportCategory::NotSaved);
    assert_eq!(line.affected, ["Shape"]);
}

#[test]
fn a_real_write_of_a_plain_document_has_no_entries() {
    let mut doc = Doc::new((4, 4));
    doc.layer("A", rect(0, 0, 4, 4), None);
    let (_, outcome) = ok(doc.write());
    assert!(outcome.report.entries.is_empty());
    assert!(outcome.report.is_empty());
}
