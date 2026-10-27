//! Tests for the kind of each open-time report line (task 163): every
//! counted note has a kind, the table below is written out independently of
//! `note_kind`, lines are neither lost nor duplicated, and the colour-mode
//! lines carry theirs.

use super::read_adjust::{ColorLookupReason, GradientReason, Kind};
use super::{
    ImportLossKind, Note, Notes, PsdImportReport, fill, grey_profile, note_kind, note_text,
};
use ImportLossKind::{Changed, FromOpen, NotSaved};

/// Every [`Note`] (parameterised ones once per reason that changes the
/// text) with the kind it must have. Written from the meaning of each line,
/// not copied from `note_kind`.
#[allow(clippy::too_many_lines)] // one row per note
fn table() -> Vec<(Note, ImportLossKind)> {
    vec![
        (Note::AdjustmentSkipped, NotSaved),
        (Note::CurvesUnreadable, NotSaved),
        (Note::CurvesTooManyPoints, NotSaved),
        (Note::CurvesGrayscale, NotSaved),
        (Note::LevelsUnreadable, NotSaved),
        (Note::LevelsGrayscale, NotSaved),
        (Note::HueSaturationUnreadable, NotSaved),
        (Note::HueSaturationGrayscale, NotSaved),
        (Note::Unreadable(Kind::Vibrance), NotSaved),
        (Note::Grayscale(Kind::Vibrance), NotSaved),
        (Note::BrightnessContrastOption, NotSaved),
        (Note::BrightnessContrastApproximate, FromOpen),
        (Note::Unvalidated(Kind::ChannelMixer), FromOpen),
        (Note::Unverified(Kind::SelectiveColor), NotSaved),
        (Note::GradientLeftOut(GradientReason::Method), NotSaved),
        (Note::GradientSmoothness, FromOpen),
        (Note::GradientUnvalidated, FromOpen),
        (Note::ColorLookup(ColorLookupReason::NoTable), NotSaved),
        (Note::ColorLookup(ColorLookupReason::Table), NotSaved),
        (Note::ColorLookup(ColorLookupReason::Unreadable), NotSaved),
        (Note::PhotoFilterColour, NotSaved),
        (Note::GreyExposureDrawn, Changed),
        (
            Note::GreyExposureProfile(grey_profile::GreyReason::NotGray),
            NotSaved,
        ),
        (Note::FillSkipped, NotSaved),
        (Note::FillDrawn, NotSaved),
        (Note::FillGradientApproximate, FromOpen),
        (Note::FillNotDrawn(fill::LeftOut::Pattern), NotSaved),
        (Note::ArtboardBackgroundDrawn, Changed),
        (
            Note::ArtboardBackgroundNotDrawn(fill::ArtboardLeft::Unchecked),
            NotSaved,
        ),
        (Note::FillVectorMaskNotApplied(true), NotSaved),
        (Note::FillVectorMaskNotApplied(false), NotSaved),
        (Note::TextRasterised, NotSaved),
        (Note::TextKept, Changed),
        (Note::TextKeptProfile, FromOpen),
        (Note::SmartObjectRasterised, NotSaved),
        (Note::ShapeRasterised, NotSaved),
        (Note::FillRasterised, NotSaved),
        (Note::MaskUnreadable, NotSaved),
        (Note::MaskParametersNotApplied, Changed),
        (Note::FeatherNotApplied, Changed),
        (Note::RealMaskNotUsed, FromOpen),
        (Note::RenderedMaskUsed, NotSaved),
        (Note::MaskRelativePosition, FromOpen),
        (Note::VectorMaskUnreadable, NotSaved),
        (Note::VectorMaskTooLarge, NotSaved),
        (Note::VectorMaskFromRendering, NotSaved),
        (Note::VectorMaskRasterised, NotSaved),
        (Note::VectorMaskDisabledDropped, NotSaved),
        (Note::ClippingDropped, NotSaved),
        (Note::AdjustmentUnderGroupFill, FromOpen),
        (Note::AdjustmentClipNotApplied, NotSaved),
        (Note::ShadowNoiseNotApplied, FromOpen),
        (Note::ShadowContourNotApplied, FromOpen),
        (Note::EffectScaleNotApplied, FromOpen),
        (Note::ShadowSizeCapped, FromOpen),
        (Note::StrokeSizeCapped, FromOpen),
        (Note::GlowNoiseNotApplied, FromOpen),
        (Note::GlowContourNotApplied, FromOpen),
        (Note::GradientSmoothnessPartial, FromOpen),
        (Note::GlowSizeCapped, FromOpen),
        (Note::DropShadowsNotShown, FromOpen),
        (Note::ColorOverlaysNotShown, FromOpen),
        (Note::StrokesNotShown, FromOpen),
        (Note::OuterGlowsNotShown, FromOpen),
        (Note::OtherEffectsNotShown, NotSaved),
        (Note::PatternOverlaysUnreadable, FromOpen),
        (Note::PatternOverlaysUnplaced, FromOpen),
        (Note::EffectsNotShown, FromOpen),
        (Note::AdjustmentEffectsDropped, NotSaved),
        (Note::EffectsUnreadable, NotSaved),
        (Note::EffectsClamped, Changed),
        (Note::BlendIfNotApplied, NotSaved),
        (Note::KnockoutNotApplied, NotSaved),
        (Note::UnknownBlendMode, Changed),
        (Note::UnknownChannel, NotSaved),
        (Note::MissingColorChannel, FromOpen),
        (Note::LayerWithoutChannels, FromOpen),
        (Note::DuplicateChannel, NotSaved),
        (Note::BrokenGroups, FromOpen),
        (Note::ExtraChannels, NotSaved),
        (Note::AlphaRead, Changed),
        (Note::AlphaInverted, Changed),
        (Note::AlphaSpot, NotSaved),
        (Note::AlphaEmpty, NotSaved),
        (Note::AlphaOverCaps, NotSaved),
        (Note::AlphaUnreadable, NotSaved),
        (Note::ColorProfileIgnored, FromOpen),
        (Note::MergedTransparencyIgnored, FromOpen),
        (Note::ColorProfileTooLarge, FromOpen),
        (Note::AdjustmentLeftOutInMode, NotSaved),
        (Note::AdjustmentOnConvertedCmyk, FromOpen),
        (Note::EffectsLeftOutInMode, NotSaved),
        (Note::GrayscaleHeldAsRgb, Changed),
        (Note::FlattenedFallback, NotSaved),
    ]
}

#[test]
fn every_note_has_the_kind_the_table_names() {
    for (note, kind) in table() {
        assert_eq!(note_kind(note), kind, "{note:?}");
    }
}

/// The table lists every `Note` variant: the enum's source has as many
/// variant lines as the table has distinct variant names.
#[test]
fn the_table_covers_every_note_variant() {
    let source = include_str!("../psd.rs");
    let start = source.find("enum Note {").unwrap_or(0);
    let body = source.get(start..).unwrap_or_default();
    let end = body.find("\n}\n").unwrap_or(body.len());
    let variants: std::collections::BTreeSet<&str> = body
        .get(..end)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with("//") && l.ends_with(','))
        .filter_map(|l| l.split(['(', ',']).next())
        .collect();
    let named: std::collections::BTreeSet<String> = table()
        .iter()
        .map(|(note, _)| {
            let debug = format!("{note:?}");
            debug.split('(').next().unwrap_or_default().to_owned()
        })
        .collect();
    let named: std::collections::BTreeSet<&str> = named.iter().map(String::as_str).collect();
    assert_eq!(variants, named);
    assert!(variants.len() > 90, "{}", variants.len());
}

/// The kinds per group over the whole table, pinned: a careless move of a
/// note between groups changes a count.
#[test]
fn the_notes_per_kind_are_counted() {
    let count = |kind| {
        table()
            .iter()
            .filter(|(n, _)| note_kind(*n) == kind)
            .count()
    };
    assert_eq!(table().len(), 94);
    assert_eq!(count(NotSaved), 52);
    assert_eq!(count(Changed), 10);
    assert_eq!(count(FromOpen), 32);
}

/// A report built from every note keeps every line once, in the notes'
/// order, with its text unchanged and its kind beside it.
#[test]
fn the_report_keeps_every_line_once_with_its_kind() {
    let mut notes = Notes::default();
    let mut expected = Vec::new();
    for (note, kind) in table() {
        // Distinct counts also make one/many wording differ per note.
        notes.add_n(note, 2);
        expected.push((note, kind));
    }
    let report = notes.report();
    // `Notes` is a map ordered by `Note`, so build the expectation the
    // same way (the order is the enum's declaration order).
    expected.sort_by_key(|(note, _)| *note);
    assert_eq!(report.items.len(), expected.len());
    assert_eq!(report.kinds.len(), report.items.len());
    for ((note, kind), (text, got)) in expected.iter().zip(report.items.iter().zip(&report.kinds)) {
        assert_eq!(text, &note_text(*note, 2), "{note:?}");
        assert_eq!(got, kind, "{note:?}");
    }
    let distinct: std::collections::BTreeSet<&String> = report.items.iter().collect();
    assert_eq!(distinct.len(), report.items.len(), "no line twice");
}

#[test]
fn losses_pair_lines_with_kinds_and_default_a_missing_kind_to_from_open() {
    let mut report = PsdImportReport::default();
    report.push(NotSaved, "a");
    report.push(Changed, "b");
    let losses = report.losses();
    assert_eq!(
        losses
            .iter()
            .map(|l| (l.kind, l.text.as_str()))
            .collect::<Vec<_>>(),
        [(NotSaved, "a"), (Changed, "b")]
    );
    // A caller that pushed straight onto `items`.
    report.items.push("c".to_owned());
    let losses = report.losses();
    assert_eq!(losses.len(), 3);
    assert_eq!(losses.last().map(|l| l.kind), Some(FromOpen));
    assert_eq!(report.items, ["a", "b", "c"]);
}

fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/psd")
        .join(name);
    match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) => unreachable!("{}: {err}", path.display()),
    }
}

fn opened(name: &str) -> PsdImportReport {
    match super::read(&fixture(name)) {
        Ok(document) => document.report,
        Err(err) => unreachable!("{name}: {err:?}"),
    }
}

fn kind_of(report: &PsdImportReport, needle: &str) -> ImportLossKind {
    let found: Vec<_> = report
        .losses()
        .into_iter()
        .filter(|l| l.text.contains(needle))
        .collect();
    assert_eq!(found.len(), 1, "{needle}: {:?}", report.items);
    found.first().map_or(FromOpen, |l| l.kind)
}

/// The colour-mode lines carry their kinds, from real files of each mode.
#[test]
fn the_colour_mode_lines_carry_their_kinds() {
    for (file, needle, kind) in [
        ("4x4_8bit_lab.psd", "converted from Lab", Changed),
        ("4x4_8bit_index_color.psd", "indexed colour table", Changed),
        ("4x4_1bit_bitmap.psd", "1-bit Bitmap", Changed),
        ("4x4_8bit_duotone.psd", "is Duotone", NotSaved),
        ("4x4_16bit_multichannel.psd", "is Multichannel", NotSaved),
        ("4x4_8bit_grayscale.psd", "held as RGB", Changed),
        ("32bit5x5.psd", "converted from 32-bit", Changed),
    ] {
        let report = opened(file);
        assert_eq!(kind_of(&report, needle), kind, "{file}: {:?}", report.items);
    }
}

/// Every fixture: one kind per line, lines unchanged, and the sum of the
/// three groups is the line count (nothing lost, nothing duplicated).
#[test]
fn every_fixture_report_has_one_kind_per_line() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/psd");
    let mut seen = 0;
    let mut kinds_seen = std::collections::BTreeSet::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        unreachable!("fixtures present")
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("psd" | "psb")
        ) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(document) = super::read(&bytes) else {
            continue;
        };
        let report = document.report;
        assert_eq!(report.kinds.len(), report.items.len(), "{}", path.display());
        let losses = report.losses();
        let texts: Vec<&str> = losses.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(
            texts,
            report.items.iter().map(String::as_str).collect::<Vec<_>>()
        );
        let by_kind =
            [FromOpen, Changed, NotSaved].map(|k| losses.iter().filter(|l| l.kind == k).count());
        assert_eq!(by_kind.iter().sum::<usize>(), report.items.len());
        kinds_seen.extend(losses.iter().map(|l| l.kind));
        seen += 1;
    }
    assert!(seen > 15, "{seen} fixtures");
    // The corpus really exercises all three groups.
    assert_eq!(kinds_seen.len(), 3, "{kinds_seen:?}");
}
