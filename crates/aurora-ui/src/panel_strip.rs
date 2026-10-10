//! The collapsed rail's label strip (0.165.0, workspace round 4's
//! predecessor): when the right rail is collapsed
//! ([`crate::set_rail_collapsed`]) it is replaced by this narrow vertical
//! `Role::Toolbar` labelled [`PANEL_STRIP_LABEL`], one text button per
//! rail panel. Design-owner decision (Cahya, 2026-10-10): short text
//! labels ([`PANEL_SHORT_LABELS`]) until an icon set is chosen — no
//! icons, glyphs or new colour tokens.
//!
//! **One button per panel, not one per dock slot.** The Properties +
//! History group gets two buttons ("Prop", "Hist"), not one: Photoshop's
//! icon-collapsed dock likewise shows one icon per panel, a group's
//! button would need a name that is neither member's, and a per-member
//! button lets "Hist" open the History tab directly rather than whatever
//! tab the group last showed.
//!
//! **What a button does: expand and show.** Activating one (a pointer
//! click, `Space`/`Enter`, an assistive technology's `Click`) expands
//! the whole rail with that panel shown — its tab selected and its slot
//! expanded ([`crate::expand_rail_showing`]). A flyout over the canvas
//! was the alternative; it would need a floating layer, its own hit
//! testing, light dismiss and focus trapping, none of which the dock has
//! yet, so "expand and show" is the simpler and more robust choice. It
//! also means the strip is never visible at the same time as the rail, so
//! "click the button again" has no second state to toggle: the rail is
//! collapsed again with the "Collapse or Expand Panels" command.
//!
//! The buttons are toggle buttons that are always off — the tools
//! panel's outlined look, no accent fill — but their accessibility node
//! carries no `toggled` state (they do not toggle) and instead reports
//! `expanded: false` (the rail they open is collapsed), and its name is
//! the **full** panel name, "Layers", never the visible "Lay". Each is a
//! `Tab` stop, like a tools-panel button. The strip's width is the widest
//! button — its label measured by the text engine plus the button's own
//! `spacing.md` padding — plus `spacing.xs` either side, the tools panel's
//! own style; no literal anywhere.

use accesskit::{Node, Orientation, Role};
use aurora_theme::Scales;
use aurora_widgets::widgets::{self, WidgetKind};
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};
use taffy::Display;

use crate::panel::PanelHandle;

/// The strip's accessible label.
pub const PANEL_STRIP_LABEL: &str = "Panels";

/// Every rail panel's short strip label, `(full name, short label)` —
/// the one place the abbreviations are defined. The Widget Gallery is not
/// here: it is a root-level column beside the rail, not one of the
/// rail's docked panels, and collapsing the rail leaves it as it is.
pub const PANEL_SHORT_LABELS: [(&str, &str); 3] = [
    ("Layers", "Lay"),
    ("Properties", "Prop"),
    ("History", "Hist"),
];

/// The short strip label for a panel's full name, if it has one.
#[must_use]
pub fn panel_short_label(full: &str) -> Option<&'static str> {
    PANEL_SHORT_LABELS
        .iter()
        .find(|(name, _)| *name == full)
        .map(|(_, short)| *short)
}

/// The collapsed rail's strip: its root toolbar and one button per rail
/// panel, in rail order (Layers, Properties, History).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanelStrip {
    pub root: WidgetId,
    /// `(panel, button)` per rail panel, in rail order.
    pub buttons: [(PanelHandle, WidgetId); 3],
}

impl PanelStrip {
    /// The panel a strip button opens.
    #[must_use]
    pub fn panel_for(&self, id: WidgetId) -> Option<PanelHandle> {
        self.buttons
            .iter()
            .find(|(_, button)| *button == id)
            .map(|(panel, _)| *panel)
    }

    /// The strip button that opens `panel`.
    #[must_use]
    pub fn button_for(&self, panel: PanelHandle) -> Option<WidgetId> {
        self.buttons
            .iter()
            .find(|(candidate, _)| *candidate == panel)
            .map(|(_, button)| *button)
    }
}

/// Builds the strip under `parent`, hidden (the rail starts expanded).
/// `panels` is each rail panel with its full name.
pub(crate) fn insert_panel_strip(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
    panels: [(PanelHandle, &str); 3],
) -> Result<PanelStrip, WidgetError> {
    let mut node = Node::new(Role::Toolbar);
    node.set_label(PANEL_STRIP_LABEL);
    node.set_orientation(Orientation::Vertical);
    node.set_hidden();
    let mut style = crate::tools_panel::tools_style(scales);
    style.display = Display::None;
    let root = tree.insert(parent, style, node, WidgetKind::Container)?;
    let mut buttons = panels.map(|(panel, _)| (panel, root));
    for (slot, (panel, full)) in buttons.iter_mut().zip(panels) {
        let short = panel_short_label(full).unwrap_or(full);
        let id = match widgets::insert_toggle_button(tree, root, scales, short, false) {
            Ok(id) => id,
            Err(err) => {
                let _ = tree.remove(root);
                return Err(err);
            }
        };
        *slot = (panel, id);
    }
    let strip = PanelStrip { root, buttons };
    let names = panels.map(|(_, full)| full);
    if let Err(err) = sync_strip_buttons(tree, &strip, names) {
        let _ = tree.remove(root);
        return Err(err);
    }
    Ok(strip)
}

/// Puts every strip button back in its one state: off (outlined), its
/// accessible name the full panel name, no `toggled` state and
/// `expanded: false`. Idempotent; run whenever the strip is shown, so a
/// toggle the widget layer flipped on activation never sticks.
fn sync_strip_buttons(
    tree: &mut WidgetTree<WidgetKind>,
    strip: &PanelStrip,
    names: [&str; 3],
) -> Result<(), WidgetError> {
    for ((_, id), full) in strip.buttons.iter().zip(names) {
        widgets::set_button_toggled(tree, *id, false)?;
        let node = tree
            .accessibility(*id)
            .ok_or(WidgetError::UnknownWidget(*id))?;
        let mut updated = node.clone();
        updated.set_label(full);
        updated.clear_toggled();
        updated.set_expanded(false);
        if updated != *node {
            tree.set_accessibility(*id, updated)?;
        }
    }
    Ok(())
}

/// Shows or hides the strip: its `display` and its AT `hidden` state (a
/// hidden strip leaves the accessibility tree and the `Tab` order).
pub(crate) fn set_panel_strip_shown(
    tree: &mut WidgetTree<WidgetKind>,
    strip: &PanelStrip,
    shown: bool,
) -> Result<(), WidgetError> {
    if shown {
        let names = PANEL_SHORT_LABELS.map(|(full, _)| full);
        sync_strip_buttons(tree, strip, names)?;
    }
    crate::workspace::set_shown(tree, strip.root, shown)
}

/// Whether the strip is shown (the rail is collapsed).
#[must_use]
pub fn panel_strip_shown(tree: &WidgetTree<WidgetKind>, strip: &PanelStrip) -> bool {
    tree.style(strip.root)
        .is_some_and(|style| style.display != Display::None)
}

/// Whether `id` is one of the strip's buttons: a button whose parent is a
/// `Role::Toolbar` labelled [`PANEL_STRIP_LABEL`] — the tree-only form a
/// caller with no [`PanelStrip`] at hand (`aurora-app`'s widget-owner
/// lookup) can ask.
#[must_use]
pub fn is_panel_strip_button(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> bool {
    matches!(tree.payload(id), Some(WidgetKind::Button(_)))
        && tree
            .parent(id)
            .and_then(|parent| tree.accessibility(parent))
            .is_some_and(|node| {
                node.role() == Role::Toolbar && node.label() == Some(PANEL_STRIP_LABEL)
            })
}

#[cfg(test)]
mod tests {
    use super::{PANEL_SHORT_LABELS, PANEL_STRIP_LABEL, is_panel_strip_button, panel_short_label};
    use crate::workspace::build_workspace;

    fn scales() -> aurora_theme::Scales {
        const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");
        match aurora_theme::Scales::from_toml_str(SCALES_TOML) {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn the_short_labels_are_defined_once_for_every_rail_panel() {
        assert_eq!(panel_short_label("Layers"), Some("Lay"));
        assert_eq!(panel_short_label("Properties"), Some("Prop"));
        assert_eq!(panel_short_label("History"), Some("Hist"));
        assert_eq!(panel_short_label("Widget Gallery"), None);
        for (full, short) in PANEL_SHORT_LABELS {
            assert!(short.len() < full.len(), "{short} abbreviates {full}");
        }
    }

    /// AC-3: a vertical toolbar labelled "Panels", one `Tab`-stop button
    /// per rail panel whose accessible name is the full panel name (the
    /// visible text is the short label), reporting `expanded: false` and
    /// no `toggled` state.
    #[test]
    fn the_strip_is_a_panels_toolbar_whose_buttons_carry_the_full_names() {
        let ws = build_workspace(&scales());
        let strip = ws.panel_strip;
        let Some(node) = ws.tree.accessibility(strip.root) else {
            unreachable!("built");
        };
        assert_eq!(node.role(), accesskit::Role::Toolbar);
        assert_eq!(node.label(), Some(PANEL_STRIP_LABEL));
        assert_eq!(node.orientation(), Some(accesskit::Orientation::Vertical));
        assert!(node.is_hidden(), "the rail starts expanded: strip hidden");
        let expected = [
            (ws.layers, "Layers", "Lay"),
            (ws.properties, "Properties", "Prop"),
            (ws.history, "History", "Hist"),
        ];
        assert_eq!(
            ws.tree.children(strip.root).map(<[_]>::len),
            Some(expected.len())
        );
        for ((panel, button), (want, full, short)) in strip.buttons.iter().zip(expected) {
            assert_eq!(*panel, want);
            let Some(node) = ws.tree.accessibility(*button) else {
                unreachable!("built");
            };
            assert_eq!(node.role(), accesskit::Role::Button);
            assert_eq!(node.label(), Some(full), "the AT name is the full name");
            assert_eq!(node.toggled(), None, "{full}: not a toggle");
            assert_eq!(node.is_expanded(), Some(false), "{full}");
            assert!(node.supports_action(accesskit::Action::Focus));
            assert!(node.supports_action(accesskit::Action::Click));
            match ws.tree.payload(*button) {
                Some(aurora_widgets::widgets::WidgetKind::Button(state)) => {
                    assert_eq!(state.label, short, "the visible text is the short label");
                    assert_eq!(state.toggled, Some(false), "outlined, like an off tool");
                }
                other => unreachable!("{other:?}"),
            }
            assert!(is_panel_strip_button(&ws.tree, *button));
            assert_eq!(strip.panel_for(*button), Some(want));
            assert_eq!(strip.button_for(want), Some(*button));
        }
        for (_, tool) in ws.tools.buttons {
            assert!(!is_panel_strip_button(&ws.tree, tool));
        }
        assert_eq!(strip.panel_for(ws.canvas_area), None);
    }
}
