//! The first-run welcome (0.257.0): one calm, dismissible alert on the
//! very first start, never again.
//!
//! **What counts as a first run.** None of the three traces of an earlier
//! run exists: the welcome's own marker file (`welcome-seen`, beside the
//! workspace layout file), the workspace layout file itself and the
//! workspace presets file ([`is_first_run`]). A user who upgrades from a
//! build without this welcome has a layout file and so never sees it; a
//! user who deletes the marker alone and has a layout does not either.
//! `AURORA_FIRST_RUN=1` (or `true`/`on`/`yes`) forces it on, `0` (or
//! `false`/`off`/`no`) forces it off, anything else detects
//! ([`first_run_override`]), like `AURORA_MENU_BAR`.
//!
//! **Priority.** The welcome is an ordinary modal dialog, and the App has
//! one dialog slot: crash recovery's dialog (opened first) always wins,
//! and a welcome is not even attempted while a recovery is pending
//! ([`should_show_welcome`]). Aurora has no command-line or OS "open this
//! file" start-up path yet (`run` reads no arguments), so there is no such
//! case to exclude; when one is added it must pass `opening_file: true`.
//!
//! **The marker is written only after the user acts**: "Get Started",
//! `Escape` (both the same action) or "Open Command Palette" write it
//! ([`write_marker`], a unique temporary file, `sync_all`, rename). A quit
//! with the dialog still open writes no marker and, because the layout
//! file is itself a first-run trace, the exit does not save the layout
//! either while the welcome is open ([`welcome_pending`], used by
//! `save_layout_on_exit`): the welcome comes back. (A crash or kill
//! cannot save the layout, so it comes back then too.) A marker write
//! failure is logged and otherwise ignored: the welcome shows again next
//! time, nothing breaks.
//!
//! Nothing here is sent anywhere; the text says so because it is true.

use std::path::{Path, PathBuf};

use aurora_theme::Scales;
use aurora_widgets::widgets::DialogAction;

use super::{DialogPurpose, FocusManager, OpenDialog};

/// The dialog's title (also its accessible name).
pub(crate) const WELCOME_TITLE: &str = "Welcome to Aurora";
/// The default and cancel action: close the welcome.
pub(crate) const WELCOME_START: &str = "welcome.start";
/// The second action: close the welcome and open the command palette.
pub(crate) const WELCOME_PALETTE: &str = "welcome.palette";
/// The label of the default action.
pub(crate) const WELCOME_START_LABEL: &str = "Get Started";
/// The label of the second action.
pub(crate) const WELCOME_PALETTE_LABEL: &str = "Open Command Palette";
/// The marker file's name, beside the workspace layout file.
pub(crate) const MARKER_FILE_NAME: &str = "welcome-seen";
/// The environment variable that forces the welcome on or off.
pub(crate) const FIRST_RUN_ENV: &str = "AURORA_FIRST_RUN";

/// What `AURORA_FIRST_RUN` asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FirstRunOverride {
    /// `1`/`true`/`on`/`yes`: show the welcome.
    Force,
    /// `0`/`false`/`off`/`no`: never show it.
    Suppress,
    /// Unset, empty or anything else: decide from the files.
    Detect,
}

/// Reads the variable's value (trimmed, case-insensitive).
pub(crate) fn first_run_override(env: Option<&str>) -> FirstRunOverride {
    match env.map(|value| value.trim().to_ascii_lowercase()) {
        Some(value) if matches!(value.as_str(), "1" | "true" | "on" | "yes") => {
            FirstRunOverride::Force
        }
        Some(value) if matches!(value.as_str(), "0" | "false" | "off" | "no") => {
            FirstRunOverride::Suppress
        }
        _ => FirstRunOverride::Detect,
    }
}

/// The marker file beside `layout_path`.
pub(crate) fn marker_path_for(layout_path: &Path) -> PathBuf {
    layout_path.with_file_name(MARKER_FILE_NAME)
}

/// Whether this start is a first run: forced either way by `over`, else
/// when no marker, no layout file and no presets file exists.
pub(crate) fn is_first_run(
    over: FirstRunOverride,
    marker_exists: bool,
    layout_exists: bool,
    presets_exists: bool,
) -> bool {
    match over {
        FirstRunOverride::Force => true,
        FirstRunOverride::Suppress => false,
        FirstRunOverride::Detect => !(marker_exists || layout_exists || presets_exists),
    }
}

/// Whether to open the welcome now: a first run, with no crash recovery
/// pending and no file being opened from outside. (A forced run still
/// yields to a pending recovery: that dialog is more important.)
pub(crate) fn should_show_welcome(
    first_run: bool,
    recovery_pending: bool,
    opening_file: bool,
) -> bool {
    first_run && !recovery_pending && !opening_file
}

/// Whether the welcome is open and undismissed: the open dialog is the
/// welcome and the marker does not exist yet.
pub(crate) fn welcome_pending(open: Option<&DialogPurpose>, marker_exists: bool) -> bool {
    matches!(open, Some(DialogPurpose::Welcome)) && !marker_exists
}

/// The first-run decision from the file system: `layout_path` (where the
/// layout lives; `None` when the OS reports no config directory, in which
/// case there is nowhere to remember a dismissal and the welcome is only
/// shown when forced) and `presets_path`.
pub(crate) fn first_run_from_disk(
    over: FirstRunOverride,
    layout_path: Option<&Path>,
    presets_path: Option<&Path>,
) -> bool {
    let Some(layout) = layout_path else {
        return over == FirstRunOverride::Force;
    };
    is_first_run(
        over,
        marker_path_for(layout).exists(),
        layout.exists(),
        presets_path.is_some_and(Path::exists),
    )
}

/// The welcome's message. `palette_chord` is the command palette's chord
/// as the shortcut table spells it (so the text can never disagree with
/// the binding).
pub(crate) fn welcome_message(palette_chord: &str) -> String {
    format!(
        "Aurora opens your PSD files and photos and keeps your edits non-destructive: layers, \
         masks and adjustments can always be changed or undone. Press {palette_chord} to search \
         every command by name. Everything stays on this computer; nothing is sent anywhere."
    )
}

/// The two actions: "Get Started" (default and cancel) and "Open Command
/// Palette".
pub(crate) fn welcome_actions() -> Vec<DialogAction> {
    vec![
        DialogAction::new(WELCOME_START, WELCOME_START_LABEL)
            .as_default()
            .as_cancel(),
        DialogAction::new(WELCOME_PALETTE, WELCOME_PALETTE_LABEL),
    ]
}

/// What a welcome action asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WelcomeChoice {
    /// Close it.
    GetStarted,
    /// Close it and open the command palette.
    OpenPalette,
}

/// The choice an action id means, `None` for any other id.
pub(crate) fn welcome_choice(action: &str) -> Option<WelcomeChoice> {
    match action {
        WELCOME_START => Some(WelcomeChoice::GetStarted),
        WELCOME_PALETTE => Some(WelcomeChoice::OpenPalette),
        _ => None,
    }
}

/// Opens the welcome (a no-op returning `false` when another dialog is
/// open: the one-dialog rule).
pub(crate) fn open_welcome_dialog(
    workspace: &mut aurora_ui::Workspace,
    focus: &mut FocusManager,
    dialog: &mut Option<OpenDialog>,
    scales: &Scales,
    palette_chord: &str,
) -> bool {
    super::open_dialog(
        workspace,
        focus,
        dialog,
        scales,
        WELCOME_TITLE,
        &welcome_message(palette_chord),
        welcome_actions(),
        DialogPurpose::Welcome,
    )
}

/// Records that the welcome was dismissed: writes the marker through a
/// unique temporary file, `sync_all` and a rename.
pub(crate) fn write_marker(path: &Path) -> std::io::Result<()> {
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut name = path
        .file_name()
        .map_or_else(|| MARKER_FILE_NAME.into(), std::ffi::OsStr::to_os_string);
    name.push(format!(".{}.tmp", std::process::id()));
    let temporary = path.with_file_name(name);
    let written = (|| {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(b"1\n")?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}
