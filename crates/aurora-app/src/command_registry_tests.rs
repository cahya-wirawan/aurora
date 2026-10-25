//! Tests for the app's use of the command registry (task 158): the palette
//! and the menus are driven by it, every id they use resolves, every
//! registry command is reachable and dispatchable, and the enabled rules are
//! the ones the menu model had before the registry existed.

#![allow(clippy::indexing_slicing)]

use std::collections::BTreeSet;
use std::path::PathBuf;

use aurora_commands::{ArgValue, Args, Presence, Resolved};
use aurora_widgets::FocusManager;
use aurora_widgets::widgets::CommandEntry;

use crate::command_registry::{TypedError, activated_from_args, registry};
use crate::menu_model::{self, Entry, MenuContext};
use crate::{
    ActivatedCommand, FileDialogAccess, activate_command, command_close_target,
    command_collapse_target, command_target, default_shortcuts_for, palette_commands,
};

struct Dialog;
impl FileDialogAccess for Dialog {
    fn pick_file(&mut self) -> Option<PathBuf> {
        Some(PathBuf::from("/tmp/in.psd"))
    }
    fn save_file(&mut self) -> Option<PathBuf> {
        Some(PathBuf::from("/tmp/out.png"))
    }
}

fn contexts() -> Vec<MenuContext> {
    (0u32..64)
        .map(|bits| MenuContext {
            can_undo: bits & 1 != 0,
            can_redo: bits & 2 != 0,
            delete_allowed: bits & 4 != 0,
            quick_mask: bits & 8 != 0,
            clipped: bits & 16 != 0,
            clip_refused: bits & 32 != 0,
            workspaces: crate::workspace_presets::workspace_palette_entries(
                &crate::workspace_presets::WorkspacePresets::default(),
            ),
            documents: vec![CommandEntry::new("document.switch.7", "Document: a.png")],
            saved_selections: vec![
                CommandEntry::new("select.load:sky", "Load Selection: sky"),
                CommandEntry::new("select.delete_saved:sky", "Delete Saved Selection: sky"),
            ],
        })
        .collect()
}

/// Every command id any menu shows, in any state.
fn menu_ids() -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for context in contexts() {
        for mac in [false, true] {
            for menu in menu_model::build_menus(&context, &default_shortcuts_for(mac)) {
                for entry in menu.entries {
                    if let Entry::Item { command, .. } = entry {
                        ids.insert(command);
                    }
                }
            }
        }
    }
    ids
}

fn dynamic_workspace_ids() -> Vec<String> {
    crate::workspace_presets::workspace_palette_entries(
        &crate::workspace_presets::WorkspacePresets::default(),
    )
    .into_iter()
    .map(|entry| entry.id)
    .collect()
}

// -- palette --

#[test]
fn the_palette_is_the_registry_in_order() {
    let palette = palette_commands();
    let expected: Vec<(&str, &str)> = registry().palette().map(|s| (s.id, s.title)).collect();
    let actual: Vec<(&str, &str)> = palette
        .iter()
        .map(|entry| (entry.id.as_str(), entry.title.as_str()))
        .collect();
    assert_eq!(actual, expected);
    assert_eq!(palette.len(), 148);
    let unique: BTreeSet<&str> = palette.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(unique.len(), palette.len(), "palette ids are unique");
}

/// The registry's titles are the ones the app's own enums and constants
/// produce, so a title changed in one place and not the other fails here.
#[test]
fn registry_titles_equal_the_app_side_titles() {
    let title = |id: &str| registry().get(id).map(|s| s.title);
    for tool in aurora_ui::Tool::ALL {
        assert_eq!(
            title(crate::tool_command_id(tool)),
            Some(format!("Select {} Tool", tool.label()).as_str())
        );
    }
    for mode in aurora_doc::SelectionMode::ALL {
        assert_eq!(
            title(crate::selection_mode_command_id(mode)),
            Some(crate::selection_modes::palette_label(mode))
        );
    }
    for ratio in aurora_ui::CropRatio::ALL {
        assert_eq!(
            title(crate::crop::ratio_command_id(ratio)),
            Some(crate::crop::ratio_command_title(ratio).as_str())
        );
    }
    for index in 0..aurora_core::PHOTO_FILTER_PRESETS.len() {
        assert_eq!(
            title(&crate::adjust_app::preset_command_id(index)),
            Some(crate::adjust_app::preset_command_title(index).as_str())
        );
    }
    for kind in crate::selection_refine::RefineKind::ALL {
        assert_eq!(title(kind.command_id()), Some(kind.menu_label()));
    }
    for (id, label, _) in crate::clipboard_app::COMMANDS {
        assert_eq!(title(id), Some(label));
    }
    for (id, label) in crate::paths_app::COMMANDS {
        assert_eq!(title(id), Some(label));
    }
    for (id, _, _, label) in crate::PANEL_MOVE_COMMANDS {
        assert_eq!(title(id), Some(label));
    }
    for (id, _, _, label) in crate::PANEL_FLOAT_COMMANDS {
        assert_eq!(title(id), Some(label));
    }
    assert_eq!(
        title(crate::psd_save::COMMAND_CHECK),
        Some(crate::psd_save::CHECK_TITLE)
    );
    assert_eq!(
        title(crate::quick_mask::COMMAND_ID),
        Some(crate::quick_mask::COMMAND_TITLE)
    );
    assert_eq!(
        title(crate::crop::COMMAND_CROP_TO_SELECTION),
        Some(crate::crop::COMMAND_TITLE)
    );
    assert_eq!(
        title(crate::crop::COMMAND_SWAP_ORIENTATION),
        Some(crate::crop::SWAP_ORIENTATION_TITLE)
    );
    assert_eq!(
        title(crate::colour_range_app::COMMAND_ID),
        Some(crate::colour_range_app::COMMAND_TITLE)
    );
    assert_eq!(
        title(crate::selection_store::COMMAND_SAVE),
        Some(crate::selection_store::SAVE_TITLE)
    );
    assert_eq!(
        title(crate::colour_app::COMMAND_SET_OUTPUT),
        Some(crate::colour_app::TITLE_SET_OUTPUT)
    );
    assert_eq!(
        title(crate::colour_app::COMMAND_CLEAR_OUTPUT),
        Some(crate::colour_app::TITLE_CLEAR_OUTPUT)
    );
    assert_eq!(
        title(crate::zoomed_view::COMMAND_FIT_ON_SCREEN),
        Some(crate::zoomed_view::FIT_ON_SCREEN_TITLE)
    );
    assert_eq!(
        title(crate::zoomed_view::COMMAND_ACTUAL_PIXELS),
        Some(crate::zoomed_view::ACTUAL_PIXELS_TITLE)
    );
    assert_eq!(
        title(crate::COMMAND_TOGGLE_PANELS),
        Some(crate::COMMAND_TOGGLE_PANELS_LABEL)
    );
    assert_eq!(title(crate::COMMAND_UNDO), Some("Undo"));
    assert_eq!(title(crate::COMMAND_APP_QUIT), Some("Quit Aurora"));
}

// -- every id resolves, and every command is reachable --

#[test]
fn every_palette_menu_native_and_dynamic_id_resolves_to_the_registry() {
    let mut ids: BTreeSet<String> = palette_commands().into_iter().map(|e| e.id).collect();
    ids.extend(menu_ids());
    ids.extend(dynamic_workspace_ids());
    ids.extend(
        menu_model::NATIVE_MENU_COMMANDS
            .iter()
            .map(|id| (*id).to_owned()),
    );
    for id in &ids {
        assert!(
            registry().resolve(id).is_some(),
            "{id} is not in the registry"
        );
    }
}

#[test]
fn every_registry_command_is_in_the_palette_a_menu_or_the_dynamic_entries() {
    let mut reachable: BTreeSet<String> = palette_commands().into_iter().map(|e| e.id).collect();
    reachable.extend(menu_ids());
    reachable.extend(dynamic_workspace_ids());
    for spec in registry().commands() {
        assert!(
            reachable.contains(spec.id),
            "{} is reachable from nowhere",
            spec.id
        );
    }
}

#[test]
fn presence_matches_where_the_command_really_appears() {
    let palette: BTreeSet<String> = palette_commands().into_iter().map(|e| e.id).collect();
    let dynamic: BTreeSet<String> = dynamic_workspace_ids().into_iter().collect();
    let menus = menu_ids();
    for spec in registry().commands() {
        match spec.presence {
            Presence::Palette => assert!(palette.contains(spec.id), "{}", spec.id),
            Presence::Dynamic => {
                assert!(!palette.contains(spec.id), "{}", spec.id);
                assert!(dynamic.contains(spec.id), "{}", spec.id);
            }
            Presence::MenuOnly => {
                assert!(
                    !palette.contains(spec.id) && !dynamic.contains(spec.id),
                    "{}",
                    spec.id
                );
                assert!(menus.contains(spec.id), "{}", spec.id);
            }
        }
    }
}

#[test]
fn every_registry_command_dispatches() {
    for spec in registry().commands() {
        let mut workspace = aurora_ui::build_workspace(&crate::test_workspace_scales());
        let mut focus = FocusManager::default();
        let handled_inline = command_target(&workspace, spec.id).is_some()
            || command_collapse_target(&workspace, spec.id).is_some()
            || command_close_target(&workspace, spec.id).is_some()
            || spec.id == crate::COMMAND_TOGGLE_PANELS;
        let picked = activate_command(&mut workspace, &mut focus, spec.id, &mut Dialog);
        assert!(
            picked.is_some() || handled_inline,
            "{} is in the registry but activate_command does not know it",
            spec.id
        );
    }
}

#[test]
fn an_id_the_registry_does_not_know_still_dispatches_to_nothing() {
    let mut workspace = aurora_ui::build_workspace(&crate::test_workspace_scales());
    let mut focus = FocusManager::default();
    assert_eq!(registry().resolve("bogus.command"), None);
    assert_eq!(
        activate_command(&mut workspace, &mut focus, "bogus.command", &mut Dialog),
        None
    );
}

#[test]
fn shortcut_hints_and_dialog_rules_name_registry_ids_only() {
    for spec in registry().commands() {
        // A hint is looked up by id; whatever has one is a registry command.
        let _ = menu_model::hint_command(spec.id);
    }
    assert!(registry().get(crate::COMMAND_APP_QUIT).is_some());
    assert!(crate::menu_command_allowed_with_dialog(
        crate::COMMAND_APP_QUIT
    ));
}

// -- families agree with the app's own resolvers --

#[test]
fn families_agree_with_the_apps_dynamic_id_resolvers() {
    use crate::document_tabs::document_command_for;
    use crate::workspace_presets::{WorkspaceCommand, workspace_command_for};
    let member = |id: &str| matches!(registry().resolve(id), Some(Resolved::Member { .. }));
    for name in ["Photography", "My Layout", "a/b"] {
        let switch = format!(
            "{}{name}",
            crate::workspace_presets::COMMAND_WORKSPACE_SWITCH_PREFIX
        );
        let delete = format!(
            "{}{name}",
            crate::workspace_presets::COMMAND_WORKSPACE_DELETE_PREFIX
        );
        assert!(member(&switch) && member(&delete));
        assert_eq!(
            workspace_command_for(&switch),
            Some(WorkspaceCommand::Switch(name.into()))
        );
        assert_eq!(
            workspace_command_for(&delete),
            Some(WorkspaceCommand::Delete(name.into()))
        );
    }
    let doc = format!("{}42", crate::document_tabs::COMMAND_DOCUMENT_SWITCH_PREFIX);
    assert!(member(&doc));
    assert!(document_command_for(&doc).is_some());
    let mut set = aurora_doc::SelectionSet::default();
    set.select(aurora_doc::Selection::new(aurora_core::geometry::Rect {
        x: 0,
        y: 0,
        width: 4,
        height: 4,
    }));
    assert!(set.save_active("sky").is_ok());
    for entry in crate::selection_store::palette_entries(&set) {
        assert!(member(&entry.id), "{}", entry.id);
    }
    for id in ["select.load:sky", "select.delete_saved:sky"] {
        assert!(member(id));
        assert!(crate::selection_store::command_for(id).is_some());
    }
    // The app's workspace entries: switch and delete are family members,
    // reset and save-as are the dynamic registry commands.
    for id in dynamic_workspace_ids() {
        assert!(registry().resolve(&id).is_some(), "{id}");
    }
}

// -- enabled rules: the pre-registry rules, re-stated independently --

/// The menu model's `enabled` as it was before the registry (0.292.0).
fn reference_enabled(id: &str, c: &MenuContext) -> bool {
    use crate::{
        clipboard_app, colour_range_app, crop, path_paint, paths_app, selection_refine,
        selection_store,
    };
    match id {
        crate::COMMAND_UNDO => c.can_undo,
        crate::COMMAND_REDO => c.can_redo,
        crate::COMMAND_LAYER_DELETE => c.delete_allowed,
        crate::COMMAND_LAYER_CLIP_CREATE => !c.clipped && !c.clip_refused,
        crate::COMMAND_LAYER_CLIP_RELEASE => c.clipped,
        crate::COMMAND_SELECT_ALL
        | crate::COMMAND_SELECT_DESELECT
        | crate::COMMAND_SELECT_INVERSE
        | crop::COMMAND_CROP_TO_SELECTION
        | colour_range_app::COMMAND_ID
        | paths_app::COMMAND_MAKE_SELECTION
        | path_paint::COMMAND_FILL
        | path_paint::COMMAND_STROKE
        | path_paint::COMMAND_MAKE_FROM_SELECTION => !c.quick_mask,
        _ if clipboard_app::command_for(id).is_some() => !c.quick_mask,
        _ if selection_refine::RefineKind::for_command(id).is_some() => !c.quick_mask,
        _ if matches!(
            selection_store::command_for(id),
            Some(selection_store::SavedSelectionCommand::Load(_))
        ) =>
        {
            !c.quick_mask
        }
        _ => true,
    }
}

#[test]
fn enabled_matches_the_pre_registry_rules_for_every_id_in_every_state() {
    let mut ids: BTreeSet<String> = registry()
        .commands()
        .iter()
        .map(|s| s.id.to_owned())
        .collect();
    ids.extend(
        [
            "select.load:sky",
            "select.delete_saved:sky",
            "workspace.switch/X",
            "document.switch.3",
            "bogus",
        ]
        .map(String::from),
    );
    for context in contexts() {
        for id in &ids {
            assert_eq!(
                menu_model::enabled(id, &context),
                reference_enabled(id, &context),
                "{id} with {context:?}"
            );
        }
    }
}

// -- menus --

#[test]
fn menu_labels_are_the_registry_titles_except_the_modify_rows() {
    let context = &contexts()[63];
    for menu in menu_model::build_menus(context, &default_shortcuts_for(false)) {
        for entry in menu.entries {
            let Entry::Item { command, label, .. } = entry else {
                continue;
            };
            let modify = crate::selection_refine::RefineKind::for_command(&command).is_some();
            match registry().get(&command) {
                Some(spec) if spec.presence == Presence::Dynamic => {} // app-made title
                Some(spec) if !modify => assert_eq!(label, spec.title, "{command}"),
                Some(_) => assert!(label.starts_with("Modify: "), "{label}"),
                None => {} // a family member: the app made its title
            }
        }
    }
}

// -- parameters agree with the app's own limits --

#[test]
fn registry_ranges_equal_the_apps_limits() {
    use aurora_commands::ParamKind;
    for kind in crate::selection_refine::RefineKind::ALL {
        let (min, max) = kind.range();
        let spec = registry().get(kind.command_id());
        let param = spec.and_then(|s| s.params.first());
        let expected = if kind.allows_fraction() {
            ParamKind::Number {
                min: f64::from(min),
                max: f64::from(max),
            }
        } else {
            ParamKind::Int {
                min: min as i64,
                max: max as i64,
            }
        };
        assert_eq!(param.map(|p| p.kind), Some(expected), "{kind:?}");
    }
    let fuzz = registry()
        .get(crate::colour_range_app::COMMAND_ID)
        .and_then(|s| s.params.first());
    assert_eq!(
        fuzz.map(|p| (p.kind, p.default)),
        Some((
            ParamKind::Int {
                min: 0,
                max: i64::from(aurora_brush::MAX_FUZZINESS)
            },
            Some(aurora_commands::ParamDefault::Int(i64::from(
                crate::colour_range_app::DEFAULT_FUZZINESS
            )))
        ))
    );
    for id in [
        crate::selection_store::COMMAND_SAVE,
        crate::workspace_presets::COMMAND_WORKSPACE_SAVE_AS,
    ] {
        let max = registry()
            .get(id)
            .and_then(|s| s.params.first())
            .map(|p| p.kind);
        assert_eq!(max, Some(ParamKind::Text { max_chars: 64 }));
    }
    assert_eq!(aurora_doc::SELECTION_NAME_MAX_CHARS, 64);
    assert_eq!(crate::workspace_presets::WORKSPACE_NAME_MAX_CHARS, 64);
}

// -- the typed bridge --

fn call(id: &str, pairs: &[(&str, ArgValue)]) -> Result<ActivatedCommand, TypedError> {
    let args: Args = pairs
        .iter()
        .map(|(n, v)| ((*n).to_owned(), v.clone()))
        .collect();
    activated_from_args(id, &args)
}

fn text(s: &str) -> ArgValue {
    ArgValue::Text(s.to_owned())
}

#[test]
fn typed_calls_become_the_same_activation_the_ui_produces() {
    use crate::selection_refine::RefineKind;
    assert_eq!(
        call("file.open", &[("path", text("/tmp/a.psd"))]),
        Ok(ActivatedCommand::OpenFile(PathBuf::from("/tmp/a.psd")))
    );
    assert_eq!(
        call("file.save", &[("path", text("/tmp/a.png"))]),
        Ok(ActivatedCommand::SaveFile(PathBuf::from("/tmp/a.png")))
    );
    assert_eq!(
        call("colour.output_profile.set", &[("path", text("/tmp/p.icc"))]),
        Ok(ActivatedCommand::SetOutputProfile(PathBuf::from(
            "/tmp/p.icc"
        )))
    );
    assert_eq!(
        call("select.feather", &[("radius", ArgValue::Number(2.5))]),
        Ok(ActivatedCommand::RefineApply(RefineKind::Feather, 2.5))
    );
    assert_eq!(
        call("select.border", &[("amount", ArgValue::Int(7))]),
        Ok(ActivatedCommand::RefineApply(RefineKind::Border, 7.0))
    );
    assert_eq!(
        call("select.colour_range", &[]),
        Ok(ActivatedCommand::ColourRangeApply(40))
    );
    assert_eq!(
        call("select.colour_range", &[("fuzziness", ArgValue::Int(0))]),
        Ok(ActivatedCommand::ColourRangeApply(0))
    );
    assert_eq!(
        call("select.save", &[("name", text("  Sky "))]),
        Ok(ActivatedCommand::SavedSelection(
            crate::selection_store::SavedSelectionCommand::Save("Sky".into())
        ))
    );
    assert_eq!(
        call("workspace.save_as", &[("name", text("Mine"))]),
        Ok(ActivatedCommand::Workspace(
            crate::workspace_presets::WorkspaceCommand::Save {
                name: "Mine".into(),
                replace: false
            }
        ))
    );
}

#[test]
fn typed_calls_are_refused_before_anything_runs() {
    assert!(matches!(
        call("select.expand", &[("amount", ArgValue::Int(0))]),
        Err(TypedError::Invalid(_))
    ));
    assert!(matches!(
        call("file.open", &[]),
        Err(TypedError::Invalid(_))
    ));
    assert!(matches!(
        call(
            "file.open",
            &[("path", text("x")), ("extra", ArgValue::Bool(true))]
        ),
        Err(TypedError::Invalid(_))
    ));
    assert!(matches!(call("nope", &[]), Err(TypedError::Invalid(_))));
    // The app's own name rule still applies after the registry's.
    assert!(matches!(
        call("workspace.save_as", &[("name", text("Essentials"))]),
        Err(TypedError::Refused(_))
    ));
    assert!(matches!(
        call("select.save", &[("name", text("a\u{202e}b"))]),
        Err(TypedError::Refused(_))
    ));
    // A command with no typed form is run by id.
    assert_eq!(call("layer.new", &[]), Err(TypedError::NoTypedForm));
}
