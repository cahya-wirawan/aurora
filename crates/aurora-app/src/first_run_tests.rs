//! Tests for the first-run welcome (0.257.0), through the free helpers
//! (`App` needs a window).

use crate::first_run::{
    FIRST_RUN_ENV, FirstRunOverride, MARKER_FILE_NAME, WELCOME_PALETTE, WELCOME_PALETTE_LABEL,
    WELCOME_START, WELCOME_START_LABEL, WELCOME_TITLE, WelcomeChoice, first_run_from_disk,
    first_run_override, is_first_run, marker_path_for, open_welcome_dialog, should_show_welcome,
    welcome_actions, welcome_choice, welcome_message, welcome_pending, write_marker,
};
use crate::{
    DialogEffect, DialogPurpose, DialogResult, FocusManager, default_shortcuts, dialog_effect,
    palette_chord_text,
};

fn temp() -> tempfile::TempDir {
    match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(err) => unreachable!("{err}"),
    }
}

#[test]
fn the_override_reads_one_and_zero_spellings_and_detects_otherwise() {
    for on in ["1", "true", "TRUE", " on ", "Yes"] {
        assert_eq!(
            first_run_override(Some(on)),
            FirstRunOverride::Force,
            "{on}"
        );
    }
    for off in ["0", "false", "OFF", " no "] {
        assert_eq!(
            first_run_override(Some(off)),
            FirstRunOverride::Suppress,
            "{off}"
        );
    }
    for other in [None, Some(""), Some("maybe"), Some("2")] {
        assert_eq!(
            first_run_override(other),
            FirstRunOverride::Detect,
            "{other:?}"
        );
    }
    assert_eq!(FIRST_RUN_ENV, "AURORA_FIRST_RUN");
}

#[test]
fn a_first_run_means_none_of_the_three_traces_exists() {
    let detect = FirstRunOverride::Detect;
    assert!(is_first_run(detect, false, false, false));
    assert!(!is_first_run(detect, true, false, false), "marker");
    assert!(!is_first_run(detect, false, true, false), "layout file");
    assert!(!is_first_run(detect, false, false, true), "presets file");
    // The override wins both ways.
    assert!(is_first_run(FirstRunOverride::Force, true, true, true));
    assert!(!is_first_run(
        FirstRunOverride::Suppress,
        false,
        false,
        false
    ));
}

#[test]
fn crash_recovery_and_an_outside_open_take_priority() {
    assert!(should_show_welcome(true, false, false));
    assert!(!should_show_welcome(false, false, false), "not a first run");
    assert!(!should_show_welcome(true, true, false), "recovery pending");
    assert!(!should_show_welcome(true, false, true), "opening a file");
}

#[test]
fn the_decision_from_disk_follows_the_files() {
    let dir = temp();
    let layout = dir.path().join("workspace-layout.postcard");
    let presets = dir.path().join("workspace-presets.postcard");
    let detect = FirstRunOverride::Detect;
    assert!(first_run_from_disk(detect, Some(&layout), Some(&presets)));
    // The marker sits beside the layout file.
    assert_eq!(marker_path_for(&layout), dir.path().join(MARKER_FILE_NAME));
    if let Err(err) = write_marker(&marker_path_for(&layout)) {
        unreachable!("{err}");
    }
    assert!(!first_run_from_disk(detect, Some(&layout), Some(&presets)));
    // The forced run shows it anyway.
    assert!(first_run_from_disk(
        FirstRunOverride::Force,
        Some(&layout),
        Some(&presets)
    ));
    // A layout file alone (an upgrade) is not a first run.
    let other = temp();
    let layout = other.path().join("workspace-layout.postcard");
    if let Err(err) = std::fs::write(&layout, b"x") {
        unreachable!("{err}");
    }
    assert!(!first_run_from_disk(detect, Some(&layout), None));
    // A presets file alone is not one either.
    let third = temp();
    let layout = third.path().join("workspace-layout.postcard");
    let presets = third.path().join("workspace-presets.postcard");
    if let Err(err) = std::fs::write(&presets, b"x") {
        unreachable!("{err}");
    }
    assert!(!first_run_from_disk(detect, Some(&layout), Some(&presets)));
    // No config directory: nowhere to remember it, so only when forced.
    assert!(!first_run_from_disk(detect, None, None));
    assert!(first_run_from_disk(FirstRunOverride::Force, None, None));
    assert!(!first_run_from_disk(
        FirstRunOverride::Suppress,
        Some(&layout),
        None
    ));
}

#[test]
fn the_marker_is_written_atomically_and_a_failure_is_an_error_not_a_panic() {
    let dir = temp();
    let path = dir.path().join("nested").join(MARKER_FILE_NAME);
    assert!(write_marker(&path).is_ok());
    assert!(path.exists());
    let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap_or(dir.path()))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(leftovers, vec![MARKER_FILE_NAME.to_owned()], "no temp left");
    // Writing again is fine (idempotent).
    assert!(write_marker(&path).is_ok());
    // A directory in the marker's place makes the rename fail: an error,
    // and the temporary is removed.
    let blocked = dir.path().join("blocked");
    if let Err(err) = std::fs::create_dir_all(blocked.join("child")) {
        unreachable!("{err}");
    }
    assert!(write_marker(&blocked).is_err());
    let names: Vec<String> = std::fs::read_dir(dir.path())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        names.iter().all(|n| !n.contains(".tmp")),
        "temporary removed: {names:?}"
    );
}

#[test]
fn the_text_is_short_honest_and_names_the_palette_chord_from_the_table() {
    let chord = palette_chord_text(&default_shortcuts());
    assert!(chord.contains("Shift"), "{chord}");
    assert!(chord.ends_with('P'), "{chord}");
    let message = welcome_message(&chord);
    assert!(message.contains(&chord));
    assert!(message.contains("PSD"));
    assert!(message.contains("non-destructive"));
    assert!(message.contains("nothing is sent anywhere"));
    assert!(message.len() < 420, "calm: {} bytes", message.len());
    assert_eq!(WELCOME_TITLE, "Welcome to Aurora");
}

#[test]
fn the_dialog_is_an_alert_with_get_started_as_default_and_cancel() {
    let mut workspace = aurora_ui::build_workspace(&crate::test_workspace_scales());
    let mut focus = FocusManager::default();
    let mut dialog = None;
    let scales = crate::test_layout_scales();
    assert!(open_welcome_dialog(
        &mut workspace,
        &mut focus,
        &mut dialog,
        &scales,
        "Ctrl+Shift+P"
    ));
    let Some(open) = dialog.clone() else {
        unreachable!("opened");
    };
    assert_eq!(open.purpose, DialogPurpose::Welcome);
    let node = workspace.tree.accessibility(open.root);
    assert_eq!(
        node.map(accesskit::Node::role),
        Some(accesskit::Role::AlertDialog)
    );
    assert_eq!(node.and_then(|n| n.label()), Some(WELCOME_TITLE));
    // Focus opens on the default action.
    assert_eq!(focus.focused(), open.first_action());
    let labels: Vec<Option<String>> = open
        .actions
        .iter()
        .map(|(_, id)| {
            workspace
                .tree
                .accessibility(*id)
                .and_then(|n| n.label().map(str::to_owned))
        })
        .collect();
    assert_eq!(
        labels,
        vec![
            Some(WELCOME_START_LABEL.to_owned()),
            Some(WELCOME_PALETTE_LABEL.to_owned())
        ]
    );
    // A second dialog is refused (the one-dialog rule), so a welcome
    // cannot stack on top of crash recovery's.
    assert!(!open_welcome_dialog(
        &mut workspace,
        &mut focus,
        &mut dialog,
        &scales,
        "Ctrl+Shift+P"
    ));
}

#[test]
fn the_actions_map_to_choices_and_the_default_is_also_the_cancel() {
    let actions = welcome_actions();
    assert_eq!(actions.len(), 2);
    assert_eq!(
        welcome_choice(WELCOME_START),
        Some(WelcomeChoice::GetStarted)
    );
    assert_eq!(
        welcome_choice(WELCOME_PALETTE),
        Some(WelcomeChoice::OpenPalette)
    );
    assert_eq!(welcome_choice("welcome.other"), None);
    for (action, choice) in [
        (WELCOME_START, WelcomeChoice::GetStarted),
        (WELCOME_PALETTE, WelcomeChoice::OpenPalette),
    ] {
        let result = DialogResult {
            purpose: DialogPurpose::Welcome,
            action: action.to_owned(),
        };
        assert_eq!(dialog_effect(&result), DialogEffect::Welcome(choice));
    }
    let unknown = DialogResult {
        purpose: DialogPurpose::Welcome,
        action: "nope".to_owned(),
    };
    assert_eq!(dialog_effect(&unknown), DialogEffect::Unknown);
}

/// Escape and Enter both end in "Get Started" (the default is the cancel),
/// through the real key path.
#[test]
fn enter_and_escape_both_choose_get_started_and_never_open_the_palette() {
    use crate::{Key, KeyChord, Modifiers, NamedKey};
    for key in [NamedKey::Enter, NamedKey::Escape] {
        let mut workspace = aurora_ui::build_workspace(&crate::test_workspace_scales());
        let mut focus = FocusManager::default();
        let mut dialog = None;
        let scales = crate::test_layout_scales();
        open_welcome_dialog(
            &mut workspace,
            &mut focus,
            &mut dialog,
            &scales,
            "Ctrl+Shift+P",
        );
        let result = crate::handle_dialog_key(
            &mut workspace,
            &mut focus,
            &mut dialog,
            KeyChord::new(Modifiers::none(), Key::Named(key)),
        );
        let Some(result) = result else {
            unreachable!("{key:?} chose nothing");
        };
        assert_eq!(
            dialog_effect(&result),
            DialogEffect::Welcome(WelcomeChoice::GetStarted),
            "{key:?}"
        );
        assert!(dialog.is_none(), "closed");
    }
}

#[test]
fn the_welcome_is_pending_only_while_its_dialog_is_open_and_the_marker_absent() {
    assert!(welcome_pending(Some(&DialogPurpose::Welcome), false));
    assert!(
        !welcome_pending(Some(&DialogPurpose::Welcome), true),
        "dismissed"
    );
    assert!(!welcome_pending(Some(&DialogPurpose::CrashRecovery), false));
    assert!(!welcome_pending(None, false), "no dialog");
}
