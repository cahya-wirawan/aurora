//! A modal dialog: a title, a message, and a row of labeled actions —
//! the generic mechanism `aurora-app`'s crash-recovery prompt (PLAN.md
//! M1.8's "crash recovery UI" bullet) is built from, kept here because
//! nothing about it is document-specific. A confirmation/alert dialog is
//! exactly the kind of reusable interaction shape this crate already
//! covers — [`super::command_palette`] is the nearest precedent: a real,
//! focusable overlay inserted into the tree on demand, not a
//! hidden-flag widget. **The precedent is now followed for paint as
//! well as layout**: as of `0.79.0` a dialog's root carries its own
//! [`WidgetKind::Dialog`] and `paint::paint_dialog` draws a real
//! rounded-rect surface for it — `surface.overlay` rather than the
//! command palette's `surface.raised`, because
//! `design/tokens/vocabulary.md` names the former "Elevation 2: modals,
//! dialogs" and reserves the latter for "Elevation 1: popovers" — plus
//! an unconditional `border.default` outline over it, added in `0.79.1`
//! and the part that actually makes the surface visible (see the "what
//! this does not do" paragraph below for what `0.79.0` alone got
//! wrong). The dialog's *title* slot and *message* node stay plain
//! `WidgetKind::Container`s and paint no fill; their text is drawn by
//! `crate::text` (below).
//!
//! `Role::AlertDialog`, not the plainer `Role::Dialog` — every dialog
//! this crate can build today is an urgent, blocking prompt (a crash
//! was detected; something needs a decision before the user continues),
//! which is exactly what ARIA's `alertdialog` role names. A quieter,
//! non-urgent `Role::Dialog` variant is real, separate follow-on work if
//! this crate ever needs one.
//!
//! **Choosing is this module's job, acting is not (0.169.0).** Every
//! input path resolves to one [`DialogChoice`] — "action `id` chosen on
//! dialog `root`": [`DialogHandle::key`] for the keyboard (`Enter`,
//! `Space`, `Escape` for the cancel action, `Tab`/`Shift+Tab` trapped in
//! the buttons) and [`DialogHandle::choice_for`] for a pointer click or an
//! assistive technology's `Click` on a button. The caller maps the root to
//! what it opened the dialog for and decides what happens. Before 0.169.0
//! the choice reached no one: the caller only logged it.
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
//! **What this does not do, stated plainly.** The root paints a
//! `surface.overlay` panel behind its buttons *and* a `border.default`
//! outline around it. The outline is not decoration and the claim it
//! replaces was false: `0.79.0` shipped the fill alone and said "the
//! root now paints a real `surface.overlay` panel behind its buttons"
//! without disclosing that in the **Light theme that panel is
//! invisible** — `design/themes/light.toml` resolves
//! `surface.overlay`, `surface.raised`, `surface.panel` and
//! `surface.canvas` all to `neutral.900` `#f5f5f6` and sets
//! `border.control_opacity = 0.0`, so a dialog was byte-identical to
//! the chrome behind it at 1.000:1, with no border and (this crate
//! draws none anywhere) no shadow. It was reachable in the shipping
//! app, at `aurora-app`'s own enforced 640x480 minimum window size,
//! overlapping real `WidgetKind::Panel` chrome. `0.79.1` closed it with
//! the same unconditional border `paint::paint_panel` was already given
//! for the same bug on real hardware; `paint::paint_dialog` carries the
//! numbers, and the **honest residual** — in Colour-Critical that
//! border clears the surfaces around it by only ≈1.35-1.49:1, faint but
//! real, and identical to what a `Panel` already gets in that
//! deliberately close-valued theme.
//!
//! The **title is drawn** since 0.141.0: it has its own one-row layout
//! slot, the root's first child (`DialogHandle::title`), an unlabelled
//! `Role::GenericContainer` into which `crate::text::text_runs` draws
//! the root's own label — so the title stays in the accessibility tree
//! exactly once, on the root, and the slot itself is pruned from the
//! platform tree by `accesskit_consumer`'s common filter. It is Regular
//! weight in `text.primary`, like the message: a heavier title weight
//! is not in the type tokens and is the design owner's call. The
//! **message is drawn** since 0.133.0 — `crate::text::text_runs` draws a
//! `Role::Label` container's label under a `Dialog` in `text.primary`,
//! on one line, clipped, not wrapped (wrapping is still deferred; so is
//! an ellipsis for a title or message too long for its row). There is also **no scrim/backdrop**: nothing dims or
//! covers the rest of the window behind a modal dialog. That is
//! explicitly out of scope here rather than forgotten — a scrim is a
//! window-sized surface owned by whoever hosts the dialog, and giving
//! this module one would mean inventing both a layout it cannot see
//! (the window) and an overlay-opacity token nobody has designed
//! (CLAUDE.md: "don't invent tokens ad hoc"). Named future work, not
//! attempted this round. The layout work here still does what it always
//! did — it fixes where the boxes are and therefore which widget a
//! click actually lands on, provable headlessly and proven that way.
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
    AlignItems, Dimension, FlexDirection, LengthPercentageAuto, Overflow, Position,
    Rect as LayoutRect, Size, Style,
};

use super::button::insert_button;
use super::popup_list::{insert_parts, scrollbar_thickness};
use super::{ListRowState, WidgetKind, row_height, spacing};
use crate::error::WidgetError;
use crate::shortcut::{Key, KeyChord, NamedKey};
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

/// The share of the dialog's own content width its title spans — all
/// of it, for the same reason as [`MESSAGE_WIDTH_FRACTION`].
const TITLE_WIDTH_FRACTION: f32 = 1.0;

/// The most of its parent's height a scrolling dialog may take
/// ([`insert_dialog_scrolling`]). A proportion, like [`WIDTH_FRACTION`]: it
/// needs no token and follows a resized window with no recompute, which a
/// pixel cap measured at open time would not. The message viewport gives
/// way first, the title and the buttons never shrink.
const SCROLLING_MAX_HEIGHT_FRACTION: f32 = 0.9;

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
/// at the default scales back then, since nothing measures text; 122 px
/// since 0.141.0 gave the title its own row), so below roughly
/// 72 logical pixels of window height (≈111 with today's 122 px) the
/// action button fell past
/// `workspace.root`'s own bottom edge — and
/// [`crate::WidgetTree::hit_test`] refuses to descend into a node whose
/// parent's bounds don't contain the point, so the button became
/// completely mouse-unreachable while the caller's modal routing went on
/// swallowing every click in the window. Centring overflows *symmetrically*
/// instead: the dialog's top is allowed to go negative, which keeps the
/// buttons (the only part that is actually interactive) on screen far
/// longer. Measured with the default scales at 800 px wide: the button's
/// own centre is hit-testable down to a window about 34 px tall, against
/// 73 px before — and, since 0.141.0 added the title row (33 px: one
/// `row_height` plus one `spacing.sm` gap), down to 68 px, against the
/// ≈111 px the top-pinned style would now need. The threshold is roughly
/// twice the distance from the dialog's centre to the button's.
///
/// Note the two `auto` vertical margins are *not* what centres it — taffy floors an auto margin's free space at zero, so
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
/// (~68 px tall since 0.141.0, above). No `min_inner_size` is set on the real window
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
/// `FlexDirection::Column` because the title and message belong *above*
/// the actions, not beside them — `Style::default()` is `Row`, which
/// would lay them out as columns next to the buttons. Through 0.140.0
/// its coverage was thin (one two-action test here). **Re-measured in
/// 0.141.0**, with the title slot in place: switching this field to
/// `Row` fails `the_title_the_message_and_every_action_get_a_real_hittable_box`
/// and `crate::text`'s
/// `a_dialog_draws_its_title_in_its_own_row_above_the_message` here,
/// and two `aurora-app` tests on real dialogs
/// (`clicking_the_dialogs_action_button_closes_it_under_real_layout`,
/// `each_real_dialogs_own_message_and_actions_lay_out_as_a_centered_overlay`).
/// Whether those app tests would also have caught it *before* the title
/// row existed was not re-measured.
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

/// A dialog title's own layout (0.141.0): the full dialog width and
/// exactly one [`row_height`] tall — a definite height rather than the
/// message's `min_size` floor, because a title is one line by design
/// and never wraps. It is the root's first child, so it sits above the
/// message in the root's `Column`.
///
/// It can never be squeezed below that row: the root's own height is
/// `auto`, so the `Column` sizes to its content and never has negative
/// free space to distribute through `flex_shrink`.
fn title_style(scales: &Scales) -> Style {
    Style {
        size: Size {
            width: percent(TITLE_WIDTH_FRACTION),
            height: length(row_height(scales)),
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
///
/// **Default and cancel (0.169.0).** A dialog may mark one action as its
/// *default* ([`Self::as_default`]: initial focus, and `Enter` when no
/// action button is focused) and one as its *cancel*
/// ([`Self::as_cancel`]: `Escape`). One action may be both — an
/// acknowledgement-only alert's single "OK" is. A dialog with no cancel
/// action is not dismissable by `Escape`: the key does nothing, so the
/// user must pick an action. When more than one action is marked, the
/// first marked one wins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogAction {
    /// Opaque to this crate — the caller defines what its own ids mean,
    /// the same "id is caller-defined" convention
    /// `super::command_palette::CommandEntry::id` already uses.
    pub id: String,
    pub label: String,
    /// The dialog's default action (see the type's own doc comment).
    pub is_default: bool,
    /// The dialog's cancel action, chosen by `Escape`.
    pub is_cancel: bool,
}

impl DialogAction {
    #[must_use]
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            is_default: false,
            is_cancel: false,
        }
    }

    /// Marks this action as the dialog's default.
    #[must_use]
    pub fn as_default(mut self) -> Self {
        self.is_default = true;
        self
    }

    /// Marks this action as the dialog's cancel (`Escape`).
    #[must_use]
    pub fn as_cancel(mut self) -> Self {
        self.is_cancel = true;
        self
    }
}

/// "Action `action` was chosen on dialog `dialog`" (0.169.0) — the one
/// result every input path produces: a key ([`DialogHandle::key`]), a
/// pointer click or an assistive technology's `Click` on a button
/// ([`DialogHandle::choice_for`]). Generic: `action` is the caller's own
/// id, `dialog` the dialog's root, and the caller maps `dialog` to
/// whatever the dialog was opened *for*.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogChoice {
    /// The dialog's own root ([`DialogHandle::root`]).
    pub dialog: WidgetId,
    /// The chosen action's id, as given to [`insert_dialog`].
    pub action: String,
}

/// What one key press means to an open dialog ([`DialogHandle::key`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogKeyOutcome {
    /// An action was chosen; the caller closes the dialog and acts on it.
    Chosen(DialogChoice),
    /// `Tab`/`Shift+Tab`: move focus to this action button. Focus is
    /// trapped in the dialog's own buttons and wraps around.
    MoveFocus(WidgetId),
    /// Nothing happens — including `Escape` on a dialog with no cancel
    /// action. The key is still the dialog's (it is modal).
    Ignored,
}

/// One inserted dialog's own widget ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogHandle {
    /// The dialog's own root — a labeled, modal `Role::AlertDialog`.
    pub root: WidgetId,
    /// The title's own one-row layout slot, the root's first child — an
    /// unlabelled `Role::GenericContainer`, since the title text lives
    /// in the root's own label (drawn here by `crate::text`).
    pub title: WidgetId,
    /// A plain text node holding the dialog's own message body.
    pub message: WidgetId,
    /// Each action's own id (as passed to [`insert_dialog`]) paired with
    /// the real, focusable button widget built for it, in the same
    /// order they were given.
    pub actions: Vec<(String, WidgetId)>,
    /// The default action's button, if one was marked
    /// ([`DialogAction::as_default`]).
    pub default_action: Option<WidgetId>,
    /// The cancel action's button, if one was marked
    /// ([`DialogAction::as_cancel`]).
    pub cancel_action: Option<WidgetId>,
    /// The scroll container holding the message's lines, for a dialog built
    /// by [`insert_dialog_scrolling`] (task 161); `None` for a plain one.
    pub body: Option<WidgetId>,
    /// The scrollbar linked to [`Self::body`] (hidden while every line
    /// fits); `None` for a plain dialog.
    pub scrollbar: Option<WidgetId>,
}

impl DialogHandle {
    /// The first action's own button, if any.
    #[must_use]
    pub fn first_action(&self) -> Option<WidgetId> {
        self.actions.first().map(|(_, id)| *id)
    }

    /// Where focus goes when the dialog opens: the default action's
    /// button, else the first action's.
    #[must_use]
    pub fn initial_focus(&self) -> Option<WidgetId> {
        self.default_action.or_else(|| self.first_action())
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

    /// The choice an activation of `button` (a pointer click, an
    /// assistive technology's `Click`, `Enter`/`Space` while focused)
    /// makes — `None` when `button` is none of this dialog's actions.
    #[must_use]
    pub fn choice_for(&self, button: WidgetId) -> Option<DialogChoice> {
        self.action_id(button).map(|action| DialogChoice {
            dialog: self.root,
            action: action.to_owned(),
        })
    }

    /// What `chord` means while this dialog is open and `focused` holds
    /// keyboard focus:
    ///
    /// - `Enter` chooses the focused action button, or the default action
    ///   when focus is on none of them (the focused button wins, as on
    ///   Windows and GTK; focus starts on the default, so an untouched
    ///   dialog's `Enter` is the default either way);
    /// - `Space` chooses the focused action button only;
    /// - `Escape` chooses the cancel action, and does nothing when there
    ///   is none;
    /// - `Tab`/`Shift+Tab` move focus to the next/previous action button,
    ///   wrapping around.
    ///
    /// A chord with `Ctrl`, `Alt` or `Meta` held is ignored, as is
    /// `Shift` with anything but `Tab`.
    #[must_use]
    pub fn key(&self, chord: &KeyChord, focused: Option<WidgetId>) -> DialogKeyOutcome {
        let mods = chord.modifiers;
        if mods.control || mods.alt || mods.meta {
            return DialogKeyOutcome::Ignored;
        }
        let focused_action = focused.filter(|id| self.action_id(*id).is_some());
        let chosen = |button: Option<WidgetId>| {
            button
                .and_then(|id| self.choice_for(id))
                .map_or(DialogKeyOutcome::Ignored, DialogKeyOutcome::Chosen)
        };
        match chord.key {
            Key::Named(NamedKey::Tab) => self
                .next_action(focused_action, mods.shift)
                .map_or(DialogKeyOutcome::Ignored, DialogKeyOutcome::MoveFocus),
            _ if mods.shift => DialogKeyOutcome::Ignored,
            Key::Named(NamedKey::Enter) => chosen(focused_action.or(self.default_action)),
            Key::Named(NamedKey::Space) => chosen(focused_action),
            Key::Named(NamedKey::Escape) => chosen(self.cancel_action),
            _ => DialogKeyOutcome::Ignored,
        }
    }

    /// The action button after (or, `backwards`, before) `from`, wrapping;
    /// from no action button, the first (or last) one.
    fn next_action(&self, from: Option<WidgetId>, backwards: bool) -> Option<WidgetId> {
        let count = self.actions.len();
        if count == 0 {
            return None;
        }
        let index = match from.and_then(|id| self.actions.iter().position(|(_, b)| *b == id)) {
            Some(at) if backwards => (at + count - 1) % count,
            Some(at) => (at + 1) % count,
            None if backwards => count - 1,
            None => 0,
        };
        self.actions.get(index).map(|(_, id)| *id)
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
    build_dialog(
        tree,
        parent,
        scales,
        title.into(),
        message.into(),
        None,
        actions,
    )
}

/// [`insert_dialog`] for a message of many lines (task 161): `message` is
/// the whole text, as a screen reader should hear it (it stays the message
/// node's single accessibility label, exactly as in a plain dialog), and
/// `lines` is what is drawn, one row each, in a scroll container with a
/// linked scrollbar.
///
/// Why a separate call: a plain dialog draws its message as ONE line (a
/// newline is drawn as a space and the rest is clipped: there is no
/// wrapping), which is right for the short sentences every other dialog
/// holds and useless for a list. The caller wraps `lines` itself to the
/// width [`dialog_text_width`] gives (the text engine is not available
/// here).
///
/// **Capped to the window.** The dialog may take at most 90 % of its
/// parent's height ([`SCROLLING_MAX_HEIGHT_FRACTION`], a proportion, so a
/// resized window needs no recompute); the message viewport shrinks to fit
/// (never below one row) and scrolls, while the title and every button
/// keep their height, so the buttons stay on screen and clickable. The
/// scrollbar is hidden while all the lines fit. Titles, buttons, default
/// and cancel actions, and the announcement are as in [`insert_dialog`].
///
/// # Errors
///
/// As [`insert_dialog`].
pub fn insert_dialog_scrolling(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
    title: impl Into<String>,
    message: impl Into<String>,
    lines: &[String],
    actions: Vec<DialogAction>,
) -> Result<DialogHandle, WidgetError> {
    build_dialog(
        tree,
        parent,
        scales,
        title.into(),
        message.into(),
        Some(lines),
        actions,
    )
}

/// The width in logical px a scrolling dialog's text lines have when the
/// window (really: the dialog's parent) is `window_width` wide: the dialog
/// is [`WIDTH_FRACTION`] of it (never under its `spacing.xxxl` floor), less
/// its padding, the always-reserved scrollbar and a row's own text inset.
/// What [`insert_dialog_scrolling`]'s caller should wrap its lines to.
#[must_use]
pub fn dialog_text_width(window_width: f32, scales: &Scales) -> f32 {
    let floor = spacing(scales.spacing.xxxl);
    let dialog = (window_width * WIDTH_FRACTION).max(floor);
    let inset = 2.0 * spacing(scales.spacing.md)
        + scrollbar_thickness(scales)
        + 2.0 * spacing(scales.spacing.sm);
    (dialog - inset).max(0.0)
}

/// The most lines [`wrap_dialog_lines`] returns; the last one then says how
/// many were cut. A bound on what a hostile report can make a dialog build.
pub const MAX_DIALOG_LINES: usize = 10_000;

/// Wraps `text` (lines separated by `\n`) to `max_width` logical px for
/// [`insert_dialog_scrolling`], breaking at spaces and, for a word wider
/// than the line, between characters. A wrapped line continues indented by
/// two spaces beyond the original's own indent (a bullet's text lines up
/// roughly under its first word). Text that already fits is returned
/// unchanged, line for line; a non-positive or non-finite `max_width` wraps
/// nothing. At most [`MAX_DIALOG_LINES`] lines come back.
///
/// The width is measured with the engine and scale the dialog's text is
/// drawn with, once, when the dialog opens: a window narrowed afterwards
/// clips the ends of lines (there is still no re-wrap on resize).
pub fn wrap_dialog_lines(
    engine: &mut aurora_text::TextEngine,
    scales: &Scales,
    scale_factor: f32,
    text: &str,
    max_width: f32,
) -> Vec<String> {
    let style = crate::text::label_style(scales);
    let mut out: Vec<String> = Vec::new();
    let mut width_of = |candidate: &str| -> f32 {
        engine
            .shape(
                &crate::text::sanitize_label(candidate),
                &style,
                scale_factor,
            )
            .width
    };
    let wrap = max_width.is_finite() && max_width > 0.0;
    let mut total = 0_usize;
    'lines: for line in text.split('\n') {
        if !wrap || width_of(line) <= max_width {
            total += 1;
            if total > MAX_DIALOG_LINES {
                break 'lines;
            }
            out.push(line.to_owned());
            continue;
        }
        let indent: String = line.chars().take_while(|c| *c == ' ').collect();
        let body = line.trim_start_matches(' ');
        let continuation = format!("{indent}  ");
        let mut current = indent.clone();
        let mut has_word = false;
        let mut pieces: Vec<String> = Vec::new();
        for word in body.split(' ').filter(|w| !w.is_empty()) {
            let candidate = if has_word {
                format!("{current} {word}")
            } else {
                format!("{current}{word}")
            };
            if width_of(&candidate) <= max_width {
                current = candidate;
                has_word = true;
                continue;
            }
            if has_word {
                pieces.push(std::mem::replace(&mut current, continuation.clone()));
            }
            // A word wider than a whole line: cut it between characters.
            let mut rest = word;
            loop {
                let mut take = 0;
                for (at, ch) in rest.char_indices() {
                    let end = at + ch.len_utf8();
                    let probe = format!("{current}{}", rest.get(..end).unwrap_or(rest));
                    if width_of(&probe) > max_width && take > 0 {
                        break;
                    }
                    take = end;
                }
                let (head, tail) =
                    rest.split_at(take.max(rest.chars().next().map_or(0, char::len_utf8)));
                current.push_str(head);
                if tail.is_empty() {
                    has_word = true;
                    break;
                }
                pieces.push(std::mem::replace(&mut current, continuation.clone()));
                rest = tail;
            }
        }
        pieces.push(current);
        for piece in pieces {
            total += 1;
            if total > MAX_DIALOG_LINES {
                break 'lines;
            }
            out.push(piece);
        }
    }
    if total > MAX_DIALOG_LINES {
        out.push("\u{2026} more lines; the full list is in Aurora's log.".to_owned());
    }
    out
}

/// A scrolling dialog's message viewport: a clipping `Row` that holds the
/// scroll container and its bar, and gives way when the dialog hits its
/// height cap.
fn viewport_style(scales: &Scales) -> Style {
    Style {
        flex_direction: FlexDirection::Row,
        flex_shrink: 1.0,
        overflow: taffy::Point {
            x: Overflow::Hidden,
            y: Overflow::Hidden,
        },
        ..message_style(scales)
    }
}

/// One drawn line of a scrolling dialog: full width, exactly one row, never
/// shrunk.
fn line_style(scales: &Scales) -> Style {
    Style {
        size: Size {
            width: percent(MESSAGE_WIDTH_FRACTION),
            height: length(row_height(scales)),
        },
        flex_shrink: 0.0,
        ..Default::default()
    }
}

/// Makes `id` keep its size when the dialog is squeezed.
fn never_shrink(tree: &mut WidgetTree<WidgetKind>, id: WidgetId) -> Result<(), WidgetError> {
    let Some(mut style) = tree.style(id).cloned() else {
        return Err(WidgetError::UnknownWidget(id));
    };
    style.flex_shrink = 0.0;
    tree.set_style(id, style)
}

fn build_dialog(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
    title: String,
    message: String,
    lines: Option<&[String]>,
    actions: Vec<DialogAction>,
) -> Result<DialogHandle, WidgetError> {
    let mut root_node = Node::new(Role::AlertDialog);
    root_node.set_label(title);
    root_node.set_modal();
    let mut root_layout = root_style(scales);
    if lines.is_some() {
        root_layout.max_size.height = percent(SCROLLING_MAX_HEIGHT_FRACTION);
    }
    let root = tree.insert(parent, root_layout, root_node, WidgetKind::Dialog)?;

    // The title's own layout slot (0.141.0), first so it sits above
    // the message. Presentational: the root's own label already *is*
    // the title, so a labelled node here would announce it twice; an
    // unlabelled `GenericContainer` is dropped from the platform tree
    // by `accesskit_consumer`'s common filter. `crate::text` draws the
    // parent's label into it.
    let title_id = tree.insert(
        root,
        title_style(scales),
        Node::new(Role::GenericContainer),
        WidgetKind::Container,
    )?;

    let mut message_node = Node::new(Role::Label);
    message_node.set_label(message);
    let message_layout = if lines.is_some() {
        viewport_style(scales)
    } else {
        message_style(scales)
    };
    let message_id = tree.insert(root, message_layout, message_node, WidgetKind::Container)?;

    let mut parts = None;
    if let Some(lines) = lines {
        never_shrink(tree, title_id)?;
        let (body, scrollbar) = insert_parts(tree, message_id, scrollbar_thickness(scales))?;
        for line in lines {
            // A drawn line is presentational: the message node above holds
            // the one announced copy of the text. `draws_label` is the
            // existing opt-in that makes a plain list row draw its own
            // accessible label.
            let mut node = Node::new(Role::GenericContainer);
            node.set_label(line.as_str());
            node.set_hidden();
            tree.insert(
                body,
                line_style(scales),
                node,
                WidgetKind::ListRow(ListRowState {
                    draws_label: true,
                    ..ListRowState::default()
                }),
            )?;
        }
        parts = Some((body, scrollbar));
    }

    let mut action_ids = Vec::with_capacity(actions.len());
    let mut default_action = None;
    let mut cancel_action = None;
    for action in actions {
        let button = insert_button(tree, root, scales, action.label)?;
        if lines.is_some() {
            never_shrink(tree, button)?;
        }
        if action.is_default && default_action.is_none() {
            default_action = Some(button);
        }
        if action.is_cancel && cancel_action.is_none() {
            cancel_action = Some(button);
        }
        action_ids.push((action.id, button));
    }

    Ok(DialogHandle {
        root,
        title: title_id,
        message: message_id,
        actions: action_ids,
        default_action,
        cancel_action,
        body: parts.map(|(body, _)| body),
        scrollbar: parts.map(|(_, scrollbar)| scrollbar),
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

    /// The title's slot (0.141.0) is the root's first child, ahead of
    /// the message and every action, and it is presentational: an
    /// unlabelled `GenericContainer`, so the title is announced once, by
    /// the root, and never a second time by its own slot.
    #[test]
    fn the_title_slot_is_the_first_child_and_carries_no_label_of_its_own() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let handle = match insert_dialog(
            &mut tree,
            root,
            &scales,
            "Unsaved Changes",
            "Save before closing?",
            vec![DialogAction::new("ok", "OK")],
        ) {
            Ok(handle) => handle,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some((_, button)) = handle.actions.first() else {
            unreachable!("one action was inserted");
        };
        assert_eq!(
            tree.children(handle.root),
            Some(&[handle.title, handle.message, *button][..]),
            "title, then message, then actions"
        );
        let Some(title) = tree.accessibility(handle.title) else {
            unreachable!("just inserted");
        };
        assert_eq!(title.role(), accesskit::Role::GenericContainer);
        assert_eq!(
            title.label(),
            None,
            "a labelled slot would announce the title twice"
        );
        let Some(root_accessibility) = tree.accessibility(handle.root) else {
            unreachable!("just inserted");
        };
        assert_eq!(root_accessibility.label(), Some("Unsaved Changes"));
        assert_eq!(handle.action_id(handle.title), None);
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
        // did hit it here through 0.140.0 (600 - 89 is odd). Since the
        // title row made the content 122 px, 600 - 122 is even and it
        // no longer does -- which is exactly why an exact-equality
        // assertion would be an accident of the numbers rather than a
        // statement about centring.
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
    fn the_title_the_message_and_every_action_get_a_real_hittable_box() {
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

        // The title (0.141.0): the message's own width, exactly one row
        // tall, and wholly above the message.
        let Some(title) = tree.bounds(handle.title) else {
            unreachable!("just laid out");
        };
        assert_eq!(
            (title.x, title.width),
            (message.x, message.width),
            "the title spans the message's own width: {title:?}"
        );
        assert_eq!(title.height, row, "the title is exactly one row: {title:?}");
        assert!(
            title.y + i64::from(title.height) <= message.y,
            "the title sits above the message: {title:?} vs {message:?}"
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
    /// in a 68 px window.
    ///
    /// **Re-bracketed in 0.141.0.** The title row (one `row_height` plus
    /// one `spacing.sm` gap, 33 px at the default scales) grew the
    /// content from 89 px to 122 px and pushed the button 33 px further
    /// below the dialog's centre, so the centred layout's own floor
    /// moved from ~34 px to 68 px (measured: 67 fails, 68 passes) and
    /// the old 40 px and 58 px rows now genuinely fail — that is the
    /// residue `root_style` documents, not a regression in centring.
    /// The heights are now 68 (the new floor itself), 80, 96 and 108;
    /// every one of them is below the ≈111 px the top-pinned 0.77.6
    /// style would need for today's content (`0.15 * h + 94 < h`), so
    /// the test still separates the two layouts.
    #[test]
    fn the_dialogs_action_stays_hit_testable_in_a_very_short_window() {
        for height in [68.0_f32, 80.0, 96.0, 108.0] {
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

    // -- 0.169.0: choosing an action --

    fn chord(key: crate::shortcut::Key, shift: bool) -> crate::shortcut::KeyChord {
        let modifiers = crate::shortcut::Modifiers {
            shift,
            ..crate::shortcut::Modifiers::none()
        };
        crate::shortcut::KeyChord::new(modifiers, key)
    }

    fn named(key: crate::shortcut::NamedKey) -> crate::shortcut::Key {
        crate::shortcut::Key::Named(key)
    }

    /// A laid-out "Replace / Cancel / Other" dialog: Replace is the
    /// default, Cancel the cancel.
    fn choice_dialog() -> (
        crate::WidgetTree<crate::widgets::WidgetKind>,
        super::DialogHandle,
    ) {
        let (mut tree, root) = sized_tree();
        let handle = match insert_dialog(
            &mut tree,
            root,
            &test_scales(),
            "Replace?",
            "Message",
            vec![
                DialogAction::new("other", "Other"),
                DialogAction::new("replace", "Replace").as_default(),
                DialogAction::new("cancel", "Cancel").as_cancel(),
            ],
        ) {
            Ok(handle) => handle,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.compute_layout(WINDOW.0, WINDOW.1);
        (tree, handle)
    }

    fn button(handle: &super::DialogHandle, id: &str) -> WidgetId {
        match handle.actions.iter().find(|(a, _)| a == id) {
            Some((_, button)) => *button,
            None => unreachable!("no action {id}"),
        }
    }

    fn chosen(handle: &super::DialogHandle, action: &str) -> super::DialogKeyOutcome {
        super::DialogKeyOutcome::Chosen(super::DialogChoice {
            dialog: handle.root,
            action: action.to_owned(),
        })
    }

    #[test]
    fn default_and_cancel_are_recorded_and_initial_focus_is_the_default() {
        let (_, handle) = choice_dialog();
        assert_eq!(handle.default_action, Some(button(&handle, "replace")));
        assert_eq!(handle.cancel_action, Some(button(&handle, "cancel")));
        assert_eq!(handle.initial_focus(), Some(button(&handle, "replace")));
        assert_ne!(handle.initial_focus(), handle.first_action());
    }

    #[test]
    fn without_a_default_initial_focus_is_the_first_action_and_the_first_marked_wins() {
        let (mut tree, root) = sized_tree();
        let handle = match insert_dialog(
            &mut tree,
            root,
            &test_scales(),
            "T",
            "M",
            vec![
                DialogAction::new("a", "A"),
                DialogAction::new("b", "B").as_cancel(),
                DialogAction::new("c", "C").as_cancel(),
            ],
        ) {
            Ok(handle) => handle,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(handle.default_action, None);
        assert_eq!(handle.initial_focus(), handle.first_action());
        assert_eq!(handle.cancel_action, Some(button(&handle, "b")));
    }

    #[test]
    fn enter_space_and_escape_choose_the_right_action() {
        use crate::shortcut::NamedKey;
        let (_, handle) = choice_dialog();
        let replace = button(&handle, "replace");
        let other = button(&handle, "other");
        // Enter: the focused button wins; with focus on no button, the default.
        assert_eq!(
            handle.key(&chord(named(NamedKey::Enter), false), Some(replace)),
            chosen(&handle, "replace")
        );
        assert_eq!(
            handle.key(&chord(named(NamedKey::Enter), false), Some(other)),
            chosen(&handle, "other")
        );
        assert_eq!(
            handle.key(&chord(named(NamedKey::Enter), false), Some(handle.message)),
            chosen(&handle, "replace")
        );
        assert_eq!(
            handle.key(&chord(named(NamedKey::Enter), false), None),
            chosen(&handle, "replace")
        );
        // Space: the focused button only.
        assert_eq!(
            handle.key(&chord(named(NamedKey::Space), false), Some(other)),
            chosen(&handle, "other")
        );
        assert_eq!(
            handle.key(&chord(named(NamedKey::Space), false), None),
            super::DialogKeyOutcome::Ignored
        );
        // Escape: the cancel action, wherever focus is — never the default.
        for focused in [Some(replace), Some(other), None] {
            assert_eq!(
                handle.key(&chord(named(NamedKey::Escape), false), focused),
                chosen(&handle, "cancel")
            );
        }
        // Modified chords are not the dialog's.
        let ctrl_enter = crate::shortcut::KeyChord::new(
            crate::shortcut::Modifiers {
                control: true,
                ..crate::shortcut::Modifiers::none()
            },
            named(NamedKey::Enter),
        );
        assert_eq!(
            handle.key(&ctrl_enter, Some(replace)),
            super::DialogKeyOutcome::Ignored
        );
        assert_eq!(
            handle.key(&chord(named(NamedKey::Enter), true), Some(replace)),
            super::DialogKeyOutcome::Ignored
        );
    }

    #[test]
    fn escape_does_nothing_without_a_cancel_action() {
        use crate::shortcut::NamedKey;
        let (mut tree, root) = sized_tree();
        let handle = match insert_dialog(
            &mut tree,
            root,
            &test_scales(),
            "T",
            "M",
            vec![DialogAction::new("go", "Go").as_default()],
        ) {
            Ok(handle) => handle,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(
            handle.key(
                &chord(named(NamedKey::Escape), false),
                handle.initial_focus()
            ),
            super::DialogKeyOutcome::Ignored
        );
        // An action that is both default and cancel (an "OK"): Escape is it.
        let ok = match insert_dialog(
            &mut tree,
            root,
            &test_scales(),
            "T",
            "M",
            vec![DialogAction::new("ok", "OK").as_default().as_cancel()],
        ) {
            Ok(handle) => handle,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(
            ok.key(&chord(named(NamedKey::Escape), false), None),
            chosen(&ok, "ok")
        );
    }

    #[test]
    fn tab_and_shift_tab_cycle_the_action_buttons_and_wrap() {
        use crate::shortcut::NamedKey;
        let (mut tree, handle) = choice_dialog();
        let mut focus = crate::FocusManager::default();
        let order: Vec<WidgetId> = handle.actions.iter().map(|(_, b)| *b).collect();
        let tab = chord(named(NamedKey::Tab), false);
        let back = chord(named(NamedKey::Tab), true);
        let Some(start) = handle.initial_focus() else {
            unreachable!("has actions");
        };
        if let Err(err) = focus.focus(&mut tree, start) {
            unreachable!("{err:?}");
        }
        let mut seen = Vec::new();
        for _ in 0..order.len() {
            match handle.key(&tab, focus.focused()) {
                super::DialogKeyOutcome::MoveFocus(next) => {
                    if let Err(err) = focus.focus(&mut tree, next) {
                        unreachable!("{err:?}");
                    }
                    seen.push(next);
                }
                other => unreachable!("{other:?}"),
            }
        }
        let &[other, replace, cancel] = order.as_slice() else {
            unreachable!("three actions");
        };
        // replace -> cancel -> other (wrapped) -> replace.
        assert_eq!(seen, vec![cancel, other, replace]);
        assert_eq!(
            handle.key(&back, Some(other)),
            super::DialogKeyOutcome::MoveFocus(cancel),
            "Shift+Tab wraps backwards"
        );
        assert_eq!(
            handle.key(&back, None),
            super::DialogKeyOutcome::MoveFocus(cancel)
        );
        assert_eq!(
            handle.key(&tab, None),
            super::DialogKeyOutcome::MoveFocus(other)
        );
    }

    /// AC-1 at the widget level: a pointer click (hit test) and an
    /// assistive technology's `Click` (`handle_action`) on the same button
    /// produce the same choice `Enter`/`Space` on it does.
    #[test]
    fn pointer_and_accessibility_clicks_make_the_same_choice_as_the_keyboard() {
        use crate::shortcut::NamedKey;
        let (mut tree, handle) = choice_dialog();
        let cancel = button(&handle, "cancel");
        let Some(bounds) = tree.bounds(cancel) else {
            unreachable!("laid out");
        };
        #[allow(clippy::cast_precision_loss)]
        let center = (
            bounds.x as f32 + bounds.width as f32 / 2.0,
            bounds.y as f32 + bounds.height as f32 / 2.0,
        );
        let by_pointer = tree.hit_test(center).and_then(|hit| handle.choice_for(hit));
        let mut focus = crate::FocusManager::default();
        let request = accesskit::ActionRequest {
            action: Action::Click,
            target_tree: crate::ACCESSIBILITY_TREE_ID,
            target_node: cancel,
            data: None,
        };
        let by_at = match crate::handle_action(&mut tree, &mut focus, &request) {
            Ok(crate::ActionOutcome::Activated(id)) => handle.choice_for(id),
            other => unreachable!("{other:?}"),
        };
        let by_key = match handle.key(&chord(named(NamedKey::Space), false), Some(cancel)) {
            super::DialogKeyOutcome::Chosen(choice) => Some(choice),
            other => unreachable!("{other:?}"),
        };
        let expected = Some(super::DialogChoice {
            dialog: handle.root,
            action: "cancel".to_owned(),
        });
        assert_eq!(by_pointer, expected);
        assert_eq!(by_at, expected);
        assert_eq!(by_key, expected);
        assert_eq!(handle.choice_for(handle.message), None);
        assert_eq!(handle.choice_for(handle.root), None);
    }

    // -- task 161: the scrolling dialog --

    fn scrolling(
        line_count: usize,
        window: (f32, f32),
    ) -> (
        crate::WidgetTree<crate::widgets::WidgetKind>,
        super::DialogHandle,
    ) {
        let (mut tree, root) = new_tree(Style {
            size: taffy::Size {
                width: taffy::style_helpers::length(window.0),
                height: taffy::style_helpers::length(window.1),
            },
            ..Default::default()
        });
        let lines: Vec<String> = (0..line_count).map(|i| format!("line {i}")).collect();
        let handle = match super::insert_dialog_scrolling(
            &mut tree,
            root,
            &test_scales(),
            "Save With Changes?",
            lines.join("\n"),
            &lines,
            vec![
                DialogAction::new("save", "Save Anyway"),
                DialogAction::new("cancel", "Cancel")
                    .as_default()
                    .as_cancel(),
            ],
        ) {
            Ok(handle) => handle,
            Err(err) => unreachable!("{err:?}"),
        };
        crate::compute_text_layout(&mut tree, window.0, window.1, None);
        (tree, handle)
    }

    fn parts(handle: &super::DialogHandle) -> (WidgetId, WidgetId) {
        match (handle.body, handle.scrollbar) {
            (Some(body), Some(bar)) => (body, bar),
            _ => unreachable!("a scrolling dialog has a body and a bar"),
        }
    }

    #[test]
    fn a_long_message_scrolls_inside_a_dialog_capped_to_the_window() {
        let window = (800.0, 300.0);
        let (tree, handle) = scrolling(200, window);
        let (body, bar) = parts(&handle);
        let Some(dialog) = tree.bounds(handle.root) else {
            unreachable!("laid out");
        };
        assert!(
            f32::from(u16::try_from(dialog.height).unwrap_or(u16::MAX)) <= window.1 * 0.9 + 1.0,
            "capped at 90 % of the window: {dialog:?}"
        );
        assert!(dialog.y >= 0 && dialog.y + i64::from(dialog.height) <= 300);
        assert!(tree.scroll_range(body).is_some_and(|r| r > 0.0), "scrolls");
        assert_ne!(
            tree.style(bar).map(|s| s.display),
            Some(taffy::Display::None),
            "the bar is shown when the lines overflow"
        );
        // Every button is on screen and reachable.
        for (_, button) in &handle.actions {
            let Some(b) = tree.bounds(*button) else {
                unreachable!("laid out");
            };
            assert!(
                b.height > 0 && b.y >= 0 && b.y + i64::from(b.height) <= 300,
                "{b:?}"
            );
            #[allow(clippy::cast_precision_loss)]
            let centre = (
                b.x as f32 + b.width as f32 / 2.0,
                b.y as f32 + b.height as f32 / 2.0,
            );
            assert_eq!(tree.hit_test(centre), Some(*button));
        }
    }

    #[test]
    fn scrolling_reaches_the_last_line_and_a_short_message_does_not_scroll() {
        let (mut tree, handle) = scrolling(200, (800.0, 300.0));
        let (body, _) = parts(&handle);
        let range = tree.scroll_range(body).unwrap_or(0.0);
        assert!(matches!(tree.set_scroll_y(body, range), Ok(true)));
        let Some(rows) = tree.children(body) else {
            unreachable!("rows");
        };
        let Some(last) = rows.last().copied() else {
            unreachable!("rows");
        };
        let (Some(row), Some(view)) = (tree.bounds(last), tree.bounds(body)) else {
            unreachable!("laid out");
        };
        assert!(
            row.y + i64::from(row.height) <= view.y + i64::from(view.height),
            "the last line is in view at the end: {row:?} in {view:?}"
        );
        let (short, short_handle) = scrolling(3, (800.0, 600.0));
        let (short_body, short_bar) = parts(&short_handle);
        assert_eq!(short.scroll_range(short_body), Some(0.0));
        assert_eq!(
            short.style(short_bar).map(|s| s.display),
            Some(taffy::Display::None),
            "no bar when everything fits"
        );
    }

    #[test]
    fn the_scrolling_dialog_announces_the_whole_message_once_and_keeps_its_choices() {
        let (tree, handle) = scrolling(5, (800.0, 600.0));
        let Some(message) = tree.accessibility(handle.message) else {
            unreachable!("inserted");
        };
        assert_eq!(
            message.label(),
            Some("line 0\nline 1\nline 2\nline 3\nline 4")
        );
        let (body, _) = parts(&handle);
        for row in tree.children(body).unwrap_or_default() {
            assert!(
                tree.accessibility(*row)
                    .is_some_and(accesskit::Node::is_hidden),
                "drawn lines are not announced a second time"
            );
        }
        let Some(root) = tree.accessibility(handle.root) else {
            unreachable!("inserted");
        };
        assert_eq!(root.label(), Some("Save With Changes?"));
        assert!(root.is_modal());
        // Default and cancel are unchanged: Enter and Escape.
        let cancel = handle.cancel_action;
        assert_eq!(handle.initial_focus(), cancel);
        let escape = chord(named(crate::shortcut::NamedKey::Escape), false);
        assert_eq!(
            handle.key(&escape, None),
            super::DialogKeyOutcome::Chosen(super::DialogChoice {
                dialog: handle.root,
                action: "cancel".into()
            })
        );
        let enter = chord(named(crate::shortcut::NamedKey::Enter), false);
        assert!(matches!(
            handle.key(&enter, None),
            super::DialogKeyOutcome::Chosen(c) if c.action == "cancel"
        ));
    }

    #[test]
    fn a_plain_dialog_has_no_scroll_parts_and_the_text_width_follows_the_window() {
        let (_, handle) = choice_dialog();
        assert_eq!((handle.body, handle.scrollbar), (None, None));
        let scales = test_scales();
        let narrow = super::dialog_text_width(600.0, &scales);
        let wide = super::dialog_text_width(1200.0, &scales);
        assert!(narrow > 0.0 && wide > narrow);
        assert!(wide < 1200.0 * 0.5, "less than the dialog itself");
    }

    // -- task 161: wrapping --

    fn engine() -> aurora_text::TextEngine {
        match aurora_text::TextEngine::new() {
            Ok(engine) => engine,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn width(engine: &mut aurora_text::TextEngine, text: &str) -> f32 {
        engine
            .shape(text, &crate::text::label_style(&test_scales()), 1.0)
            .width
    }

    #[test]
    fn text_that_fits_is_returned_line_for_line_and_long_text_wraps_losslessly() {
        let scales = test_scales();
        let mut engine = engine();
        let short = "Changed:\n\u{2022} one\n\u{2022} two";
        assert_eq!(
            super::wrap_dialog_lines(&mut engine, &scales, 1.0, short, 400.0),
            ["Changed:", "\u{2022} one", "\u{2022} two"]
        );
        let long = format!(
            "\u{2022} {}",
            (0..60)
                .map(|i| format!("word{i}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        let lines = super::wrap_dialog_lines(&mut engine, &scales, 1.0, &long, 200.0);
        assert!(lines.len() > 3, "{lines:?}");
        for line in &lines {
            assert!(width(&mut engine, line) <= 200.0, "{line:?}");
        }
        let words: Vec<&str> = lines
            .iter()
            .flat_map(|l| l.split(' '))
            .filter(|w| !w.is_empty() && *w != "\u{2022}")
            .collect();
        let expected: Vec<String> = (0..60).map(|i| format!("word{i}")).collect();
        assert_eq!(
            words,
            expected.iter().map(String::as_str).collect::<Vec<_>>()
        );
        assert!(
            lines.iter().skip(1).all(|l| l.starts_with("  ")),
            "continuations are indented"
        );
    }

    #[test]
    fn a_word_wider_than_the_line_is_cut_between_characters_without_losing_any() {
        let scales = test_scales();
        let mut engine = engine();
        let word = "W".repeat(80);
        let lines = super::wrap_dialog_lines(&mut engine, &scales, 1.0, &word, 120.0);
        assert!(lines.len() > 1);
        let joined: String = lines.iter().map(|l| l.trim_start()).collect();
        assert_eq!(joined, word);
        for line in &lines {
            assert!(width(&mut engine, line) <= 120.0, "{line:?}");
        }
    }

    #[test]
    fn a_degenerate_width_wraps_nothing_and_the_line_count_is_bounded() {
        let scales = test_scales();
        let mut engine = engine();
        for bad in [0.0_f32, -4.0, f32::NAN] {
            assert_eq!(
                super::wrap_dialog_lines(&mut engine, &scales, 1.0, "a b c d", bad),
                ["a b c d"]
            );
        }
        let many = vec!["x"; super::MAX_DIALOG_LINES + 50].join("\n");
        let lines = super::wrap_dialog_lines(&mut engine, &scales, 1.0, &many, 400.0);
        assert_eq!(lines.len(), super::MAX_DIALOG_LINES + 1);
        assert!(
            lines
                .last()
                .is_some_and(|l| l.contains("full list is in Aurora's log"))
        );
    }
}
