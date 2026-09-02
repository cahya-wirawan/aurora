//! A modal dialog: a title, a message, and a row of labeled actions —
//! the generic mechanism `aurora-app`'s crash-recovery prompt (PLAN.md
//! M1.8's "crash recovery UI" bullet) is built from, kept here because
//! nothing about it is document-specific. A confirmation/alert dialog is
//! exactly the kind of reusable interaction shape this crate already
//! covers — [`super::command_palette`] is the nearest precedent: a real,
//! focusable overlay inserted into the tree on demand, not a
//! hidden-flag widget. **The precedent was followed for layout, not for
//! paint**: the command palette also earned its own `WidgetKind` and a
//! `paint_command_palette` surface, while a dialog's root and message
//! are still `WidgetKind::Container` and draw nothing — see the "what
//! this does not do" paragraph at the end of this comment. Read "nearest
//! precedent" as being about how the overlay is *placed and inserted*,
//! not about how far along it is.
//!
//! `Role::AlertDialog`, not the plainer `Role::Dialog` — every dialog
//! this crate can build today is an urgent, blocking prompt (a crash
//! was detected; something needs a decision before the user continues),
//! which is exactly what ARIA's `alertdialog` role names. A quieter,
//! non-urgent `Role::Dialog` variant is real, separate follow-on work if
//! this crate ever needs one.
//!
//! **Action *handling* is deliberately not this module's job**: it
//! builds the buttons and returns their ids; deciding what happens when
//! one is activated (closing the dialog, taking some action) is the
//! caller's — the same "generic mechanism, caller owns behaviour" split
//! [`super::command_palette`] already draws between its own tree
//! structure and `aurora-app`'s own command matching.
//!
//! **Click routing is the caller's job, same as keyboard**: a dialog's
//! action buttons are real `Button`s with `Action::Click` declared, and
//! this module builds them and returns their ids via
//! [`DialogHandle::action_id`] — deciding what a click (or `Enter` on a
//! focused button) actually *does* stays outside this module, the same
//! split this doc comment's own "action handling" paragraph already
//! draws for the keyboard. `aurora-app`'s `handle_dialog_pointer`
//! (added once that crate had real pointer input at all, PLAN.md M1.9)
//! is the first caller to actually do this.
//!
//! **Layout is this module's job, unlike the command palette's.** A
//! modal alert has no caller-specific placement to make: it is centred
//! over whatever it interrupts, and this module already declares its own
//! modality ([`Role::AlertDialog`] plus `Node::set_modal`) rather than
//! taking it as a parameter. [`super::command_palette`] is the contrast
//! — its popover shape genuinely is the caller's call, which is why its
//! own doc comment leaves placement outside. Pushing this style down
//! here also keeps it under `scripts/check_no_hardcoded_style.py`, which
//! scans `aurora-widgets`/`aurora-ui` and nothing above them. See
//! [`root_style`] for what the styles actually do and why.
//!
//! **What this does not do, stated plainly**: a dialog's root and
//! message are still [`WidgetKind::Container`], and this crate draws no
//! glyphs anywhere, so a dialog on screen today is *only* its action
//! buttons' rounded rects floating over the canvas — no surface behind
//! them, no visible title, no visible message text. The layout work here
//! fixes where the boxes are and therefore which widget a click actually
//! lands on; it is provable headlessly and it is proven that way. It
//! makes no claim about how a dialog looks.
//!
//! The `taffy` mechanics `root_style`/`message_style` rely on (auto-margin
//! overflow behavior, `align_self`'s alignment-safety keywords, the
//! over-constrained-inset stretch, absolute items and `min_size`) are each
//! cited with a source line and a real measured before/after in
//! `docs/taffy-behaviors.md` at the workspace root — check there before
//! re-deriving one from scratch, and add to it rather than only restating
//! a finding in a doc comment here.

use accesskit::{Node, Role};
use aurora_theme::Scales;
use taffy::style_helpers::{TaffyAuto as _, TaffyZero as _, auto, length, percent};
use taffy::{
    AlignItems, Dimension, FlexDirection, LengthPercentageAuto, Position, Rect as LayoutRect, Size,
    Style,
};

use super::button::insert_button;
use super::{WidgetKind, row_height, spacing};
use crate::error::WidgetError;
use crate::tree::{WidgetId, WidgetTree};

/// The share of the window's own width a dialog spans. A proportion,
/// deliberately, not a pixel count — see [`root_style`].
const WIDTH_FRACTION: f32 = 0.5;

/// The share of the *dialog's* own content width its message spans —
/// all of it. Named rather than written inline for the same reason
/// [`WIDTH_FRACTION`] is: both are structural proportions this module
/// chose, and one convention for both beats a named constant here and a
/// bare literal there.
const MESSAGE_WIDTH_FRACTION: f32 = 1.0;

/// A dialog root's own layout: a centred overlay, **out of its parent's
/// flow entirely**.
///
/// `Position::Absolute` is the load-bearing field. Without it (through
/// `0.77.5`, when this root was given a bare `Style::default()` and
/// nothing styled it afterwards) a dialog is an ordinary in-flow
/// sibling of whatever it was inserted beside — in `aurora-app`'s own
/// workspace, a fourth `Row` child after the canvas, the divider and
/// the rail. Measured on a real `aurora_ui::build_workspace` at
/// 1000x800: opening one shrank the canvas from 750 px wide to 718 px
/// and put the dialog itself at `x: 968, width: 32, height: 800` — a
/// full-height 32 px sliver past the rail, its width coming entirely
/// out of the document. Absolute takes it out of that flow, the same
/// mechanism `aurora-app`'s own command-palette style already uses.
///
/// **Neither vertical inset may be definite.** With both of them
/// definite and `size.height: auto()`, CSS's (and taffy's)
/// over-constrained resolution stretches the box to the full
/// containing-block height, silently reproducing the full-height dialog
/// this style exists to fix. (Measured, and *only* the height is
/// affected: horizontal centring survives it untouched — a definite
/// `top`/`bottom` pair on an 800-wide root still resolves to
/// `Rect { x: 200, width: 400 }`. A previous version of this comment
/// claimed it also collapsed the horizontal auto margins; it does not.)
/// `dialog_lays_out_as_a_centered_overlay_with_real_bounds` asserts
/// against the stretch.
///
/// **The two axes are centred by different mechanisms, and that is
/// deliberate.** Horizontally, two definite insets plus equal `auto`
/// margins — the standard absolute-centring idiom, and the one that
/// works whichever `flex_direction` the parent happens to use, because
/// taffy resolves an `auto` margin against a definite inset the same way
/// on the main and cross axis alike. Vertically that idiom is
/// unavailable (it needs the definite-inset pair the paragraph above
/// forbids), so the dialog instead leaves both vertical insets `auto`
/// and asks for `align_self: Center`.
///
/// Vertical centring is not cosmetic — it is what keeps the dialog
/// *usable in a short window*, which through `0.77.6` it was not. The
/// old style pinned the top edge 15% of the way down the window and let
/// the box grow downward from there; the content height is fixed (~89 px
/// at the default scales, since nothing measures text), so below roughly
/// 72 logical pixels of window height the action button fell past
/// `workspace.root`'s own bottom edge — and
/// [`crate::WidgetTree::hit_test`] refuses to descend into a node whose
/// parent's bounds don't contain the point, so the button became
/// completely mouse-unreachable while the caller's modal routing went on
/// swallowing every click in the window. Centring overflows *symmetrically*
/// instead: the dialog's top is allowed to go negative, which keeps the
/// buttons (the only part that is actually interactive) on screen far
/// longer. Measured with the default scales at 800 px wide: the button's
/// own centre is hit-testable down to a window about 34 px tall, against
/// 73 px before. Note the two `auto` vertical margins are *not* what
/// centres it — taffy floors an auto margin's free space at zero, so
/// they resolve to 0 exactly when the dialog overflows, which is the
/// case that matters; they are there so a parent laid out as a
/// `Column` (where vertical is the *main* axis and `align_self` is
/// ignored) still centres, clamped at the top edge. It is `align_self`
/// that does the work under `aurora-app`'s own `Row` workspace root.
/// `the_dialogs_action_stays_hit_testable_in_a_very_short_window` is the
/// regression test.
///
/// **What it still does not do**: nothing clamps the dialog to the
/// window, so at some small enough size the buttons do leave it anyway
/// (~34 px tall, above). No `min_inner_size` is set on the real window
/// either. Clamping properly needs either a scrollable dialog body or a
/// measured text stack that can reflow the message — neither exists —
/// so this is a documented residue, not a fixed problem.
///
/// The width choice is a **proportion**, not pixels: there is no
/// "dialog dimensions" token in `design/tokens/scales.toml`, and
/// inventing one is a design decision for the design owner (CLAUDE.md:
/// "don't invent tokens ad hoc when implementing a widget"), not a gap
/// to fill here. A proportion needs no token at all — it is a
/// structural layout choice, which is why `check_no_hardcoded_style.py`
/// deliberately scopes `percent(...)` out. Everything that *is* an
/// absolute size here (the padding, the gap, both `min_size` floors)
/// goes through the token scales, per invariant §7.3.10.
///
/// `FlexDirection::Column` because the message belongs *above* the
/// actions, not beside them — `Style::default()` is `Row`, which would
/// lay the message out as a first column next to the buttons. **Its
/// coverage is thin, and worth knowing**: the only test that can fail if
/// this field is dropped is this module's own
/// `the_message_and_every_action_get_a_real_hittable_box`, because it is
/// the only one that builds a *two*-action dialog. Every dialog
/// `aurora-app` actually opens has exactly one action, so that crate's
/// suite — real dialogs, real workspace — cannot see this field at all.
/// Adding an app-level two-action test would be testing a dialog no
/// caller constructs; the honest fix is this note.
fn root_style(scales: &Scales) -> Style {
    Style {
        position: Position::Absolute,
        flex_direction: FlexDirection::Column,
        // Vertical centring; see this function's own doc comment for why
        // the vertical axis cannot use the horizontal axis's idiom.
        align_self: Some(AlignItems::CENTER),
        inset: LayoutRect {
            left: LengthPercentageAuto::ZERO,
            right: LengthPercentageAuto::ZERO,
            top: auto(),
            bottom: auto(),
        },
        // Horizontal centring: equal auto margins against a definite
        // width and two definite horizontal insets, the standard
        // absolute-centring idiom. The vertical pair is the `Column`-
        // parent fallback described in the doc comment.
        margin: LayoutRect {
            left: auto(),
            right: auto(),
            top: auto(),
            bottom: auto(),
        },
        size: Size {
            width: percent(WIDTH_FRACTION),
            height: auto(),
        },
        // A defensive lower bound only. `min_size.height` is already
        // dominated by this style's own vertical padding — taffy raises
        // a `min_size` to at least the padding+border sum before using
        // it, and two `spacing.md` paddings exceed one `row_height` at
        // every built-in scale — so it cannot bind today. It is kept so
        // the floor does not silently become *absent* if the padding
        // ever shrinks; do not read it as load-bearing.
        min_size: Size {
            width: length(spacing(scales.spacing.xxxl)),
            height: length(row_height(scales)),
        },
        padding: LayoutRect {
            left: length(spacing(scales.spacing.md)),
            right: length(spacing(scales.spacing.md)),
            top: length(spacing(scales.spacing.md)),
            bottom: length(spacing(scales.spacing.md)),
        },
        gap: Size {
            width: length(spacing(scales.spacing.sm)),
            height: length(spacing(scales.spacing.sm)),
        },
        ..Default::default()
    }
}

/// A dialog message's own layout — the full dialog width, and **at
/// least one row height tall**, mirroring `aurora_ui::panel`'s own
/// shared `row_style`.
///
/// The floor is the whole point. Nothing in this codebase measures
/// text: [`crate::WidgetTree::compute_layout`] builds a bare `taffy`
/// tree with no measure function anywhere, so a `Role::Label` node
/// resolves to zero height however much text it actually holds. Fixing
/// the root's own position does not fix that — a zero-height message
/// would still leave the buttons sitting directly under the dialog's
/// top padding, and a zero-*size* box is not hit-testable at all. One
/// [`row_height`] is the honest "at least one line of UI text" floor
/// until real text shaping exists to measure the real thing.
///
/// **Only the height gets a floor.** Through `0.77.6` the width was
/// floored at one `row_height` too — a text *line height* reused as a
/// width, which measures nothing and meant nothing. It could never bind
/// (the width is already [`MESSAGE_WIDTH_FRACTION`] of the dialog's own
/// content box, which is far wider at every scale), so removing it
/// changes no layout; it is gone because a floor nobody can justify is
/// worse than no floor.
fn message_style(scales: &Scales) -> Style {
    Style {
        size: Size {
            width: percent(MESSAGE_WIDTH_FRACTION),
            height: auto(),
        },
        min_size: Size {
            width: Dimension::AUTO,
            height: length(row_height(scales)),
        },
        ..Default::default()
    }
}

/// One dialog action button — e.g. `("recover", "Recover Document")`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogAction {
    /// Opaque to this crate — the caller defines what its own ids mean,
    /// the same "id is caller-defined" convention
    /// `super::command_palette::CommandEntry::id` already uses.
    pub id: String,
    pub label: String,
}

impl DialogAction {
    #[must_use]
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
        }
    }
}

/// One inserted dialog's own widget ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogHandle {
    /// The dialog's own root — a labeled, modal `Role::AlertDialog`.
    pub root: WidgetId,
    /// A plain text node holding the dialog's own message body.
    pub message: WidgetId,
    /// Each action's own id (as passed to [`insert_dialog`]) paired with
    /// the real, focusable button widget built for it, in the same
    /// order they were given.
    pub actions: Vec<(String, WidgetId)>,
}

impl DialogHandle {
    /// The first action's own button, if any — the usual place to move
    /// focus to when a dialog opens.
    #[must_use]
    pub fn first_action(&self) -> Option<WidgetId> {
        self.actions.first().map(|(_, id)| *id)
    }

    /// The action id `button` belongs to, if `button` is one of this
    /// dialog's own action buttons — `None` for any other widget,
    /// including this dialog's own root or message.
    #[must_use]
    pub fn action_id(&self, button: WidgetId) -> Option<&str> {
        self.actions
            .iter()
            .find(|(_, id)| *id == button)
            .map(|(action_id, _)| action_id.as_str())
    }
}

/// Inserts a new modal dialog as the last child of `parent`: a labeled,
/// modal `Role::AlertDialog` holding a message and one real, focusable
/// button per entry in `actions`, in order.
///
/// # `parent` must be definitely sized
///
/// The dialog is an absolutely positioned overlay (this module's own
/// private `root_style`), so
/// `parent` is its containing block, and every proportion in that style
/// — the half-of-the-window width, the vertical centring — resolves
/// against `parent`'s own resolved box, **not** against the window.
/// Pass the node that is actually window-sized (`aurora_ui::Workspace`'s
/// own `root`, in this workspace). Violating it does not error; it
/// silently misplaces the dialog:
///
/// - a `parent` that is smaller than the window centres the dialog over
///   *that* box rather than over the window;
/// - a `parent` whose width is `auto` gives `percent` nothing to resolve
///   against, and the dialog collapses to the `min_size` floor in
///   `root_style` — a box far too small for its own content, whose
///   buttons then overflow it and stop being hit-testable, since
///   [`crate::WidgetTree::hit_test`] will not descend into a node whose
///   parent's bounds exclude the point.
///
/// **`parent` must also be a flex container** — the vertical centring
/// specifically relies on taffy's flexbox absolute-item layout path
/// (`align_self`/auto-inset behavior that only exists for flex items).
/// Every node this crate builds is a flex container today, so this is
/// unenforced and untested; it is the next precondition to violate if a
/// grid or block-layout parent is ever introduced.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `parent` doesn't exist.
/// Nothing is added when this happens.
pub fn insert_dialog(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
    title: impl Into<String>,
    message: impl Into<String>,
    actions: Vec<DialogAction>,
) -> Result<DialogHandle, WidgetError> {
    let mut root_node = Node::new(Role::AlertDialog);
    root_node.set_label(title.into());
    root_node.set_modal();
    let root = tree.insert(parent, root_style(scales), root_node, WidgetKind::Container)?;

    let mut message_node = Node::new(Role::Label);
    message_node.set_label(message.into());
    let message_id = tree.insert(
        root,
        message_style(scales),
        message_node,
        WidgetKind::Container,
    )?;

    let mut action_ids = Vec::with_capacity(actions.len());
    for action in actions {
        let button = insert_button(tree, root, scales, action.label)?;
        action_ids.push((action.id, button));
    }

    Ok(DialogHandle {
        root,
        message: message_id,
        actions: action_ids,
    })
}

#[cfg(test)]
mod tests {
    use super::{DialogAction, insert_dialog};
    use crate::WidgetError;
    use crate::tree::WidgetId;
    use crate::widgets::{new_tree, row_height, test_scales};
    use accesskit::Action;
    use taffy::Style;

    #[test]
    fn insert_dialog_builds_a_labeled_modal_alert_with_message_and_ordered_actions() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let handle = match insert_dialog(
            &mut tree,
            root,
            &scales,
            "Aurora Didn't Close Properly",
            "The previous session didn't shut down cleanly.",
            vec![
                DialogAction::new("recover", "Recover Document"),
                DialogAction::new("discard", "Discard"),
            ],
        ) {
            Ok(handle) => handle,
            Err(err) => unreachable!("{err:?}"),
        };

        let Some(accessibility) = tree.accessibility(handle.root) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.role(), accesskit::Role::AlertDialog);
        assert_eq!(accessibility.label(), Some("Aurora Didn't Close Properly"));
        assert!(accessibility.is_modal());

        let Some(message_accessibility) = tree.accessibility(handle.message) else {
            unreachable!("just inserted");
        };
        assert_eq!(
            message_accessibility.label(),
            Some("The previous session didn't shut down cleanly.")
        );

        assert_eq!(handle.actions.len(), 2);
        let Some((first_id, first_button)) = handle.actions.first() else {
            unreachable!("just asserted len() == 2");
        };
        assert_eq!(first_id, "recover");
        let Some(button_accessibility) = tree.accessibility(*first_button) else {
            unreachable!("just inserted");
        };
        assert_eq!(button_accessibility.label(), Some("Recover Document"));
        assert!(button_accessibility.supports_action(Action::Focus));
        assert!(button_accessibility.supports_action(Action::Click));

        assert_eq!(handle.first_action(), Some(*first_button));
        assert_eq!(handle.action_id(*first_button), Some("recover"));
    }

    #[test]
    fn first_action_is_none_for_a_dialog_with_no_actions() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let handle = match insert_dialog(&mut tree, root, &scales, "Title", "Message", vec![]) {
            Ok(handle) => handle,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(handle.first_action(), None);
    }

    #[test]
    fn action_id_returns_none_for_a_widget_that_is_not_one_of_this_dialogs_buttons() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let handle = match insert_dialog(
            &mut tree,
            root,
            &scales,
            "Title",
            "Message",
            vec![DialogAction::new("ok", "OK")],
        ) {
            Ok(handle) => handle,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(handle.action_id(handle.message), None);
        assert_eq!(handle.action_id(root), None);
    }

    /// A definite-size test root — a `percent`-sized absolutely
    /// positioned child has nothing to resolve against otherwise
    /// (`new_tree(Style::default())`'s own root is auto-sized, and the
    /// dialog would collapse to its `min_size` floor), the same
    /// give-the-root-a-real-size setup `command_palette`'s own layout
    /// test already uses.
    fn sized_tree() -> (crate::WidgetTree<crate::widgets::WidgetKind>, WidgetId) {
        new_tree(Style {
            size: taffy::Size {
                width: taffy::style_helpers::length(WINDOW.0),
                height: taffy::style_helpers::length(WINDOW.1),
            },
            ..Default::default()
        })
    }

    const WINDOW: (f32, f32) = (800.0, 600.0);

    /// Headless (no GPU, no display) proof of the layout half of
    /// `0.77.6`: a dialog is a real, centred overlay box, not the
    /// full-height sliver a bare `Style::default()` root resolved to.
    #[test]
    fn dialog_lays_out_as_a_centered_overlay_with_real_bounds() {
        let (mut tree, root) = sized_tree();
        let scales = test_scales();
        let handle = match insert_dialog(
            &mut tree,
            root,
            &scales,
            "Title",
            "Message",
            vec![DialogAction::new("ok", "OK")],
        ) {
            Ok(handle) => handle,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.compute_layout(WINDOW.0, WINDOW.1);

        let Some(bounds) = tree.bounds(handle.root) else {
            unreachable!("just laid out");
        };
        assert!(
            bounds.width > 0 && bounds.height > 0,
            "the dialog must have a real box: {bounds:?}"
        );
        // `<= 1`, not exact equality: `WidgetTree::apply_taffy_layout`
        // truncates an `f32` origin to an `i64`, so a window whose
        // free space is odd centres to two gaps that differ by a pixel.
        // That is correct behaviour, and the vertical pair below really
        // does hit it here (600 - 89 is odd), which is exactly why an
        // exact-equality assertion would have been an accident of the
        // numbers rather than a statement about centring.
        let right_gap = 800 - (bounds.x + i64::from(bounds.width));
        assert!(
            (bounds.x - right_gap).abs() <= 1,
            "the dialog must be horizontally centred: {bounds:?}"
        );
        let bottom_gap = 600 - (bounds.y + i64::from(bounds.height));
        assert!(
            (bounds.y - bottom_gap).abs() <= 1,
            "the dialog must be vertically centred too: {bounds:?}"
        );
        assert!(
            bounds.y > 0,
            "the dialog must sit below the top edge: {bounds:?}"
        );
        assert!(
            bounds.y + i64::from(bounds.height) < 600,
            "the dialog must be content-height, not stretched to the full window \
             height -- neither vertical inset being definite is what prevents \
             that: {bounds:?}"
        );
    }

    #[test]
    fn the_message_and_every_action_get_a_real_hittable_box() {
        let (mut tree, root) = sized_tree();
        let scales = test_scales();
        let handle = match insert_dialog(
            &mut tree,
            root,
            &scales,
            "Title",
            "Message",
            vec![
                DialogAction::new("recover", "Recover Document"),
                DialogAction::new("discard", "Discard"),
            ],
        ) {
            Ok(handle) => handle,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.compute_layout(WINDOW.0, WINDOW.1);

        let Some(message) = tree.bounds(handle.message) else {
            unreachable!("just laid out");
        };
        assert!(
            message.width > 0,
            "the message must span real width: {message:?}"
        );
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let row = row_height(&scales) as u32;
        assert!(
            message.height >= row,
            "the message must be at least one row of text tall ({row}), since \
             nothing measures its actual text: {message:?}"
        );

        let mut previous_bottom = None;
        for (_, button) in &handle.actions {
            let Some(bounds) = tree.bounds(*button) else {
                unreachable!("just laid out");
            };
            assert!(
                bounds.width > 0 && bounds.height > 0,
                "every action must be a real, hit-testable box: {bounds:?}"
            );
            if let Some(previous_bottom) = previous_bottom {
                assert!(
                    bounds.y >= previous_bottom,
                    "stacked actions must not overlap vertically, or a click \
                     lands on whichever one hit_test happens to reach first: \
                     {bounds:?} after {previous_bottom}"
                );
            }
            previous_bottom = Some(bounds.y + i64::from(bounds.height));
        }
        assert!(previous_bottom.is_some(), "two actions were inserted");
    }

    /// The `0.77.7` boundary fix, and the one test here that would fail
    /// under `0.77.6`'s own style.
    ///
    /// A dialog's content height is fixed (nothing measures text), so a
    /// short window is the case where placement decides whether the
    /// thing is usable at all. Through `0.77.6` the root's top edge sat
    /// at 15% of the window height and the box grew downward from there,
    /// which put the action button past the window's own bottom edge
    /// below roughly 72 logical pixels — and since
    /// [`crate::WidgetTree::hit_test`] will not descend into a node
    /// whose parent's bounds exclude the point, the button became
    /// completely unclickable while the caller's modal routing kept
    /// swallowing every click. Measured then: at 800x58 the root landed
    /// at `y: 9, height: 89` and `hit_test` on the button's own centre
    /// returned `None`. Vertical centring overflows symmetrically
    /// instead, so the button stays on screen.
    ///
    /// The heights are chosen to bracket the old threshold, and the
    /// window is far shorter than any real one — the point is that the
    /// failure mode is gone at the boundary, not that anyone runs Aurora
    /// in a 58 px window.
    #[test]
    fn the_dialogs_action_stays_hit_testable_in_a_very_short_window() {
        for height in [40.0_f32, 58.0, 72.0, 90.0] {
            let (mut tree, root) = new_tree(Style {
                size: taffy::Size {
                    width: taffy::style_helpers::length(WINDOW.0),
                    height: taffy::style_helpers::length(height),
                },
                ..Default::default()
            });
            let scales = test_scales();
            let handle = match insert_dialog(
                &mut tree,
                root,
                &scales,
                "Title",
                "Message",
                vec![DialogAction::new("ok", "OK")],
            ) {
                Ok(handle) => handle,
                Err(err) => unreachable!("{err:?}"),
            };
            tree.compute_layout(WINDOW.0, height);

            let Some(button) = handle.first_action() else {
                unreachable!("one action was inserted");
            };
            let Some(bounds) = tree.bounds(button) else {
                unreachable!("just laid out");
            };
            #[allow(clippy::cast_precision_loss)]
            let center = (
                bounds.x as f32 + bounds.width as f32 / 2.0,
                bounds.y as f32 + bounds.height as f32 / 2.0,
            );
            // Window-relative, not derived from the button: the point we
            // are about to click has to be a point a mouse can actually
            // reach, which is exactly what the old layout failed.
            assert!(
                center.1 >= 0.0 && center.1 < height,
                "at {}x{height} the button's own centre must lie inside the \
                 window at all: {center:?} from {bounds:?}",
                WINDOW.0
            );
            assert_eq!(
                tree.hit_test(center),
                Some(button),
                "at {}x{height} a click on the button's own centre must reach \
                 it: {bounds:?}",
                WINDOW.0
            );
        }
    }

    #[test]
    fn insert_dialog_rejects_an_unknown_parent() {
        let (mut tree, _root) = new_tree(Style::default());
        let scales = test_scales();
        let bogus = accesskit::NodeId(999);
        match insert_dialog(&mut tree, bogus, &scales, "Title", "Message", vec![]) {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }
}
