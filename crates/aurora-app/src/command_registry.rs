//! The app side of the command registry (`aurora-commands`).
//!
//! The registry owns what a command **is** (id, title, docs, parameters,
//! access hint, requirements). This module is where `aurora-app` uses it:
//!
//! * [`palette_entries`] is the command palette's static list, in registry
//!   order. [`menu_label`] gives the menus the same titles.
//! * [`facts`] turns the menu's state into the registry's
//!   [`CommandContext`], and [`enabled`] asks the registry whether a
//!   command can run (the one place the "why is this item disabled" rules
//!   live).
//! * [`activated_from_args`] is the first bridge from a **validated typed
//!   call** to the same [`ActivatedCommand`] the palette and the menus
//!   produce, for the commands whose interactive form is a dialog or a
//!   prompt and whose automation form is a parameter: Open, Save As, Set
//!   Output Profile, Select > Modify, Colour Range, Save Selection and Save
//!   Workspace As.
//!
//! **Running stays here.** [`crate::activate_command`] still resolves an id
//! to an [`ActivatedCommand`] (or does a panel command inline) and
//! `App::run_activated` still performs it; the registry does not execute
//! anything and has no document. A command with no parameters is run by id
//! through `activate_command`, exactly as the palette does.
//!
//! `docs/command-registry.md` says how to add a command.

use aurora_commands::{ArgError, Args, CommandContext, NotEnabled, Registry, Validated};
use aurora_widgets::widgets::CommandEntry;

use crate::ActivatedCommand;
use crate::menu_model::MenuContext;
use crate::selection_refine::RefineKind;

/// The registry the app uses.
pub(crate) fn registry() -> &'static Registry {
    Registry::builtin()
}

/// The palette's static entries, in registry order (the commands the
/// application lists from the start; workspace, document and saved-selection
/// entries are added when the palette opens).
pub(crate) fn palette_entries() -> Vec<CommandEntry> {
    registry()
        .palette()
        .map(|spec| CommandEntry::new(spec.id, spec.title))
        .collect()
}

/// The title the registry gives command `id`: the label a menu shows
/// unless it spells the command differently. `None` for an id that is not
/// a registry command (family members have app-made titles).
pub(crate) fn menu_label(id: &str) -> Option<&'static str> {
    registry().get(id).map(|spec| spec.title)
}

/// The registry's view of the menu's facts.
pub(crate) fn facts(context: &MenuContext) -> CommandContext {
    CommandContext {
        can_undo: context.can_undo,
        can_redo: context.can_redo,
        delete_allowed: context.delete_allowed,
        quick_mask: context.quick_mask,
        clipped: context.clipped,
        clip_refused: context.clip_refused,
    }
}

/// Whether command `id` may run now. An id the registry does not know is
/// reported as enabled, as the menus always did (they show what the model
/// lists and let `activate_command` decide).
pub(crate) fn enabled(id: &str, context: &MenuContext) -> bool {
    !matches!(
        registry().check_enabled(id, &facts(context)),
        Err(NotEnabled::Reason(_))
    )
}

// The typed bridge is exercised by the tests and waits for its first
// caller (the automation front ends of tasks 159 and 160).
#[allow(dead_code)]
/// Why a typed call could not become an [`ActivatedCommand`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TypedError {
    /// The registry refused the arguments (unknown, missing, wrong type or
    /// out of range).
    Invalid(ArgError),
    /// The arguments are well typed but the application's own rule for the
    /// value refuses them (a selection or workspace name it will not take).
    Refused(String),
    /// The command has no typed form here: run it by id through
    /// [`crate::activate_command`].
    NoTypedForm,
}

impl From<ArgError> for TypedError {
    fn from(err: ArgError) -> Self {
        Self::Invalid(err)
    }
}

/// Turns a typed call into the [`ActivatedCommand`] the interactive path
/// would have produced, after the registry has validated it.
///
/// # Errors
/// [`TypedError::Invalid`] when the registry refuses the arguments,
/// [`TypedError::Refused`] when the application's own name rule does, and
/// [`TypedError::NoTypedForm`] for a command that is run by id.
#[allow(dead_code)]
pub(crate) fn activated_from_args(id: &str, args: &Args) -> Result<ActivatedCommand, TypedError> {
    let validated = registry().validate_args(id, args)?;
    typed_form(&validated).unwrap_or(Err(TypedError::NoTypedForm))
}

#[allow(dead_code)]
fn typed_form(call: &Validated) -> Option<Result<ActivatedCommand, TypedError>> {
    let path = || call.text("path").map(std::path::PathBuf::from);
    let made = match call.id.as_str() {
        crate::COMMAND_FILE_OPEN => ActivatedCommand::OpenFile(path()?),
        crate::COMMAND_FILE_SAVE => ActivatedCommand::SaveFile(path()?),
        crate::colour_app::COMMAND_SET_OUTPUT => ActivatedCommand::SetOutputProfile(path()?),
        crate::colour_range_app::COMMAND_ID => {
            let fuzziness = u16::try_from(call.int("fuzziness")?).ok()?;
            ActivatedCommand::ColourRangeApply(fuzziness)
        }
        crate::selection_store::COMMAND_SAVE => {
            let name = call.text("name")?;
            return Some(
                aurora_doc::validate_selection_name(name)
                    .map(|valid| {
                        ActivatedCommand::SavedSelection(
                            crate::selection_store::SavedSelectionCommand::Save(valid),
                        )
                    })
                    .map_err(|err| {
                        TypedError::Refused(format!(
                            "That selection name is not allowed ({err:?})."
                        ))
                    }),
            );
        }
        crate::workspace_presets::COMMAND_WORKSPACE_SAVE_AS => {
            let name = call.text("name")?;
            return Some(
                crate::workspace_presets::validate_workspace_name(name)
                    .map(|valid| {
                        ActivatedCommand::Workspace(
                            crate::workspace_presets::WorkspaceCommand::Save {
                                name: valid,
                                replace: false,
                            },
                        )
                    })
                    .map_err(|err| TypedError::Refused(err.message())),
            );
        }
        other => {
            let kind = RefineKind::for_command(other)?;
            #[allow(clippy::cast_possible_truncation)] // validated to 1..=500
            let amount = match kind {
                RefineKind::Feather => call.number("radius")? as f32,
                _ => call.int("amount")? as f32,
            };
            ActivatedCommand::RefineApply(kind, amount)
        }
    };
    Some(Ok(made))
}
