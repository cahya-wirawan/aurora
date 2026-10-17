use aurora_color::{IccProfile, Transform};
use aurora_io::{ColourImport, Image, SourceProfile};
use half::f16;

use super::{
    NOTICE_NOT_FOUND, NOTICE_NOT_RGB, export_image, judge_output_profile, load_output_profile,
};

const SRGB_ICC: &[u8] = include_bytes!("../../../corpora/icc/sRGB.icc");

fn adobe_bytes() -> Vec<u8> {
    let Ok(p) = IccProfile::synthetic_rgb(
        [0.3127, 0.3290],
        [[0.64, 0.33], [0.21, 0.71], [0.15, 0.06]],
        2.199_218_75,
    ) else {
        unreachable!()
    };
    let Ok(b) = p.to_bytes() else { unreachable!() };
    b
}

fn adobe() -> SourceProfile {
    match judge_output_profile(adobe_bytes()) {
        Ok(p) => p,
        Err(e) => unreachable!("{e}"),
    }
}

fn srgb_image() -> Image {
    let v = |n: u8| f16::from_f32(f32::from(n) / 255.0);
    match Image::new(
        2,
        1,
        IccProfile::srgb(),
        vec![v(51), v(153), v(102), v(204), v(255), v(0), v(0), v(128)],
    ) {
        Ok(i) => i,
        Err(e) => unreachable!("{e:?}"),
    }
}

#[test]
fn no_output_profile_exports_the_image_unchanged_as_srgb() {
    let image = srgb_image();
    let before: Vec<u16> = image.samples().iter().map(|s| s.to_bits()).collect();
    let Ok(out) = export_image(image, None) else {
        unreachable!()
    };
    assert_eq!(
        out.samples()
            .iter()
            .map(|s| s.to_bits())
            .collect::<Vec<_>>(),
        before
    );
    assert!(matches!(out.color_space().is_exactly_srgb(), Ok(true)));
}

#[test]
fn an_output_profile_converts_the_pixels_and_tags_the_image_with_it() {
    let Ok(out) = export_image(srgb_image(), Some(&adobe())) else {
        unreachable!()
    };
    // Tagged with the output profile, never sRGB.
    assert!(matches!(out.color_space().is_exactly_srgb(), Ok(false)));
    // The numbers are sRGB -> output, computed straight from lcms2.
    let Ok(dst) = IccProfile::from_bytes(&adobe_bytes()) else {
        unreachable!()
    };
    let Ok(t) = Transform::from_working(&dst) else {
        unreachable!()
    };
    let mut want = [0.0_f32; 3];
    assert!(
        t.transform(&[51.0 / 255.0, 153.0 / 255.0, 102.0 / 255.0], &mut want)
            .is_ok()
    );
    for (got, w) in out.samples().iter().zip(want) {
        assert!((got.to_f32() - w).abs() < 1e-3);
    }
    // Alpha untouched.
    assert_eq!(
        out.samples().get(3).map(|a| a.to_bits()),
        Some(f16::from_f32(204.0 / 255.0).to_bits())
    );
}

#[test]
fn the_exported_file_embeds_exactly_the_profile_its_pixels_are_in() {
    let Ok(out) = export_image(srgb_image(), Some(&adobe())) else {
        unreachable!()
    };
    for ext in ["a.png", "a.jpg", "a.tif"] {
        let Ok(bytes) = aurora_io::encode_by_extension(std::path::Path::new(ext), &out) else {
            unreachable!()
        };
        // Reading it back with colour management lands on the original
        // sRGB values (within 8-bit quantisation, and JPEG loss).
        let Ok((back, imported)) = aurora_io::decode_to_working(std::path::Path::new(ext), &bytes)
        else {
            unreachable!()
        };
        assert_eq!(
            imported.source.as_ref().map(|s| s.bytes.clone()),
            Some(adobe_bytes()),
            "{ext}"
        );
        let tolerance = if ext == "a.jpg" { 0.03 } else { 0.01 };
        for (got, want) in back.samples().iter().zip(srgb_image().samples()).take(3) {
            assert!(
                (got.to_f32() - want.to_f32()).abs() < tolerance,
                "{ext}: {got:?} vs {want:?}"
            );
        }
    }
    // And with no output profile the PNG still says plain sRGB.
    let Ok(plain) = aurora_io::encode_by_extension(std::path::Path::new("a.png"), &srgb_image())
    else {
        unreachable!()
    };
    let Ok((_, imported)) = aurora_io::decode_to_working(std::path::Path::new("a.png"), &plain)
    else {
        unreachable!()
    };
    assert_eq!(imported, ColourImport::default());
}

#[test]
fn a_profile_that_cannot_convert_exports_srgb_untouched() {
    let broken = SourceProfile {
        description: "x".to_owned(),
        bytes: b"junk".to_vec(),
    };
    let Err(image) = export_image(srgb_image(), Some(&broken)) else {
        unreachable!()
    };
    assert!(matches!(image.color_space().is_exactly_srgb(), Ok(true)));
}

#[test]
fn only_valid_rgb_profiles_can_be_the_output_profile() {
    assert!(judge_output_profile(adobe_bytes()).is_ok());
    assert!(judge_output_profile(SRGB_ICC.to_vec()).is_ok());
    let Ok(gray) = IccProfile::synthetic_gray(2.2).and_then(|p| p.to_bytes()) else {
        unreachable!()
    };
    for bad in [
        gray,
        b"junk".to_vec(),
        vec![0; aurora_color::MAX_PROFILE_BYTES + 1],
    ] {
        assert_eq!(judge_output_profile(bad).err(), Some(NOTICE_NOT_RGB));
    }
}

#[test]
fn a_profile_file_is_read_with_a_bound_and_a_missing_file_is_a_calm_refusal() {
    let dir = match tempfile::tempdir() {
        Ok(d) => d,
        Err(e) => unreachable!("{e}"),
    };
    let good = dir.path().join("a.icc");
    assert!(std::fs::write(&good, adobe_bytes()).is_ok());
    assert!(load_output_profile(&good).is_ok());
    assert_eq!(
        load_output_profile(&dir.path().join("none.icc")).err(),
        Some(NOTICE_NOT_FOUND)
    );
    let big = dir.path().join("big.icc");
    assert!(std::fs::write(&big, vec![0_u8; aurora_color::MAX_PROFILE_BYTES + 100]).is_ok());
    assert_eq!(load_output_profile(&big).err(), Some(NOTICE_NOT_RGB));
}

#[test]
fn opening_a_profiled_png_converts_it_and_names_the_notice() {
    let dir = match tempfile::tempdir() {
        Ok(d) => d,
        Err(e) => unreachable!("{e}"),
    };
    // A file tagged Adobe-like, written by the exporter under test.
    let Ok(tagged) = Image::new(
        1,
        1,
        match IccProfile::from_bytes(&adobe_bytes()) {
            Ok(p) => p,
            Err(_) => unreachable!(),
        },
        vec![
            f16::from_f32(0.2),
            f16::from_f32(0.6),
            f16::from_f32(0.4),
            f16::from_f32(1.0),
        ],
    ) else {
        unreachable!()
    };
    let Ok(bytes) = aurora_io::encode_by_extension(std::path::Path::new("a.png"), &tagged) else {
        unreachable!()
    };
    let path = dir.path().join("a.png");
    assert!(std::fs::write(&path, bytes).is_ok());
    let Ok((image, colour)) = crate::open_image_with_colour(&path) else {
        unreachable!()
    };
    assert!(matches!(image.color_space().is_exactly_srgb(), Ok(true)));
    assert!(colour.source.is_some());
    assert_eq!(
        crate::colour_open_notice(&colour),
        Some("Converted from the file's colour profile to sRGB.")
    );
    // The full background-open path carries it into the prepared image.
    match crate::decode_chosen_file(&path) {
        Ok(crate::DecodedFile::Image(prepared)) => assert!(prepared.colour.source.is_some()),
        other => unreachable!("{:?}", other.is_ok()),
    }
    // An untagged PNG gets the assumption notice and no source profile.
    let untagged = ColourImport {
        source: None,
        report: aurora_io::ColourReport {
            info: vec!["No colour profile: assumed sRGB.".to_owned()],
            warnings: Vec::new(),
        },
    };
    assert_eq!(
        crate::colour_open_notice(&untagged),
        Some("No colour profile in this file: assumed sRGB.")
    );
    assert_eq!(crate::colour_open_notice(&ColourImport::default()), None);
}

#[test]
fn the_colour_metadata_survives_a_save_and_a_reopen_through_the_app_writer() {
    let dir = match tempfile::tempdir() {
        Ok(d) => d,
        Err(e) => unreachable!("{e}"),
    };
    let (layers, history, _) = crate::document_from_size("L", (4, 4));
    let colour = aurora_io::DocumentColour {
        source: Some(adobe()),
        output: Some(adobe()),
    };
    let selection = aurora_doc::SelectionSet::new();
    let skipped = aurora_io::SkippedTiles::new();
    let source = crate::aur_save::AurSaveSource {
        layers: &layers,
        history: &history,
        canvas_size: (4, 4),
        skipped_tiles: &skipped,
        import_losses: None,
        selection: &selection,
        colour: &colour,
    };
    let scratch = match tempfile::tempdir() {
        Ok(d) => d,
        Err(e) => unreachable!("{e}"),
    };
    let Some(budget) = std::num::NonZeroUsize::new(8) else {
        unreachable!()
    };
    let Ok(mut store) = aurora_tile::TileStore::new(scratch.path().to_path_buf(), budget) else {
        unreachable!()
    };
    let path = dir.path().join("a.aur");
    assert!(crate::aur_save::save_aur_document_with(&path, &source, &mut store, |_| true).is_ok());
    let Ok(file) = std::fs::File::open(&path) else {
        unreachable!()
    };
    let scratch2 = match tempfile::tempdir() {
        Ok(d) => d,
        Err(e) => unreachable!("{e}"),
    };
    let Ok(mut store2) = aurora_tile::TileStore::new(scratch2.path().to_path_buf(), budget) else {
        unreachable!()
    };
    let Ok(doc) = aurora_io::read_aur(file, &mut store2) else {
        unreachable!()
    };
    assert_eq!(doc.colour, colour);
}

struct PickOnce(Option<std::path::PathBuf>);

impl crate::FileDialogAccess for PickOnce {
    fn pick_file(&mut self) -> Option<std::path::PathBuf> {
        self.0.take()
    }

    fn save_file(&mut self) -> Option<std::path::PathBuf> {
        None
    }
}

#[test]
fn the_output_profile_commands_resolve_and_a_cancelled_dialog_does_nothing() {
    let mut workspace = aurora_ui::build_workspace(&crate::test_workspace_scales());
    let mut focus = crate::FocusManager::default();
    let path = std::path::PathBuf::from("/tmp/p.icc");
    let picked = crate::activate_command(
        &mut workspace,
        &mut focus,
        super::COMMAND_SET_OUTPUT,
        &mut PickOnce(Some(path.clone())),
    );
    assert_eq!(
        picked,
        Some(crate::ActivatedCommand::SetOutputProfile(path))
    );
    let cancelled = crate::activate_command(
        &mut workspace,
        &mut focus,
        super::COMMAND_SET_OUTPUT,
        &mut PickOnce(None),
    );
    assert_eq!(cancelled, None);
    let cleared = crate::activate_command(
        &mut workspace,
        &mut focus,
        super::COMMAND_CLEAR_OUTPUT,
        &mut PickOnce(None),
    );
    assert_eq!(cleared, Some(crate::ActivatedCommand::ClearOutputProfile));
}

#[test]
fn both_commands_are_in_the_palette_and_the_edit_menu() {
    let palette = crate::palette_commands();
    for id in [super::COMMAND_SET_OUTPUT, super::COMMAND_CLEAR_OUTPUT] {
        assert!(palette.iter().any(|entry| entry.id == id), "{id}");
    }
    let menus = crate::menu_model::build_menus(
        &crate::menu_model::MenuContext::default(),
        &crate::default_shortcuts_for(false),
    );
    let Some(edit) = menus.iter().find(|m| m.title == "Edit") else {
        unreachable!()
    };
    for id in [super::COMMAND_SET_OUTPUT, super::COMMAND_CLEAR_OUTPUT] {
        assert!(
            edit.entries.iter().any(|e| matches!(
                e,
                crate::menu_model::Entry::Item { command, enabled: true, .. } if command == id
            )),
            "{id}"
        );
    }
}

#[test]
fn a_chosen_profile_file_becomes_the_output_profile_or_a_notice() {
    // The same judging the App method runs, end to end from a file.
    let dir = match tempfile::tempdir() {
        Ok(d) => d,
        Err(e) => unreachable!("{e}"),
    };
    let ok_path = dir.path().join("a.icc");
    assert!(std::fs::write(&ok_path, adobe_bytes()).is_ok());
    let bad_path = dir.path().join("b.icc");
    assert!(std::fs::write(&bad_path, b"junk").is_ok());
    let mut colour = aurora_io::DocumentColour::default();
    for path in [&ok_path, &bad_path] {
        if let Ok(profile) = load_output_profile(path) {
            colour.output = Some(profile);
        }
        assert_eq!(
            colour.output.is_some(),
            expect_set || colour.output.is_some()
        );
    }
    assert!(colour.output.is_some());
    // The refused one left the earlier choice alone.
    assert_eq!(load_output_profile(&bad_path).err(), Some(NOTICE_NOT_RGB));
}
