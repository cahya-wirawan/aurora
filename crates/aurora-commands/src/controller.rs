//! Commands a bridge **controller** (an automation client driving the
//! running app) may never run, with the reason each is refused. One table
//! for every place that refuses them: the proxy (so the tool list and the
//! error say why), the app's command gate and its menu route.
//!
//! This is metadata only; the app still gates the actions themselves (a
//! controller's key press or menu click reaches the same code).

/// `(id, reason)` for each command a controller is refused.
pub const CONTROLLER_REFUSED: &[(&str, &str)] = &[
    ("app.quit", "quitting is for the person at the window"),
    (
        "automation.disconnect",
        "switching the bridge off is for the person at the window",
    ),
    (
        "edit.copy",
        "it writes to the operating system clipboard, which is shared with the person's other applications",
    ),
    (
        "edit.cut",
        "it writes to the operating system clipboard, which is shared with the person's other applications",
    ),
    (
        "edit.copy_merged",
        "it writes to the operating system clipboard, which is shared with the person's other applications",
    ),
    (
        "edit.paste",
        "it reads the operating system clipboard, which can hold anything the person copied elsewhere",
    ),
    (
        "edit.paste_in_place",
        "it reads the operating system clipboard, which can hold anything the person copied elsewhere",
    ),
];

/// Why a controller is refused command `id`, if it is.
#[must_use]
pub fn controller_refusal(id: &str) -> Option<&'static str> {
    CONTROLLER_REFUSED
        .iter()
        .find(|(refused, _)| *refused == id)
        .map(|(_, reason)| *reason)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Registry;

    #[test]
    fn the_clipboard_and_quit_commands_are_refused_with_a_reason() {
        for id in [
            "app.quit",
            "automation.disconnect",
            "edit.copy",
            "edit.cut",
            "edit.copy_merged",
            "edit.paste",
            "edit.paste_in_place",
        ] {
            let reason = controller_refusal(id).unwrap_or_default();
            assert!(!reason.is_empty(), "{id}");
        }
        // The internal-only layer commands do not touch the clipboard.
        assert_eq!(controller_refusal("edit.layer_via_copy"), None);
        assert_eq!(controller_refusal("edit.undo"), None);
    }

    #[test]
    fn every_refused_id_is_a_real_command() {
        let registry = Registry::builtin();
        for (id, _) in CONTROLLER_REFUSED {
            assert!(registry.get(id).is_some(), "{id} is not in the registry");
        }
    }
}
