//! Click-level pointer routing and focused-widget keyboard routing for
//! the whole widget vocabulary (PLAN.md M1.8, 0.130.0) — the first time
//! a real mouse press or a real key reaches a toolkit widget other than
//! the few the app wires by hand (dialog buttons, layer rows, the
//! command palette).
//!
//! **No new mutation logic lives here, and no selection policy.** Every
//! change goes through a path
//! that already exists and is already tested on its own: a synthesized
//! [`accesskit::ActionRequest`] handed to [`crate::handle_action`] (the
//! same gate an assistive technology goes through — declared actions,
//! disabled state, range and data checks), or, for the three inputs
//! `handle_action` has no vocabulary for, the widget module's own public
//! mutator: [`widgets::set_saturation_value_from_point`]/
//! [`widgets::set_hue_from_point`] (a point inside a colour picker),
//! [`widgets::select_curve_point`]/[`widgets::add_curve_point_from_point`]
//! (a point inside a curve editor), and [`widgets::commit_dropdown_row`]
//! (a dropdown option row, which `handle_action` refuses as
//! `Unsupported` because a `ListRow` click is only routed under a menu).
//!
//! # What a click is
//!
//! A [`ClickTracker`] arms the widget a primary-button `Down` landed on;
//! the matching `Up` **over that same widget** activates it, an `Up`
//! anywhere else cancels it ([`PointerOutcome::Cancelled`]) — the
//! platform-standard "press, change your mind, drag off, release"
//! escape. Activation widgets (button, checkbox, swatch, tree row,
//! dropdown control and option row, tab, menu item) act on that `Up`.
//! Value widgets (slider, scrollbar, colour picker, curve editor) act on
//! the `Down` itself, because the value under the pointer is the whole
//! point of pressing there.
//!
//! # Drags (0.131.0)
//!
//! A value widget's `Down` also **captures** it ([`ClickTracker::captured`]):
//! every later [`PointerPhase::Move`] drives the same mutator the `Down`
//! used, wherever the pointer is — a slider or scrollbar clamps at its
//! ends, a colour picker keeps driving the part the drag began on (an
//! area drag never changes the hue), a curve drag moves the selected
//! point ([`widgets::move_selected_point_from_point`], clamped by
//! `ToneCurve::move_point_to` to `[0, 1]` in `y` and to its neighbours
//! in `x`). The `Up` ends it ([`PointerOutcome::Released`]) without
//! applying its own position. A `Move` with nothing captured does
//! nothing — no hit test. A `Move` never changes focus or the focus
//! ring's visibility. A captured widget removed or disabled mid-drag ends
//! the drag ([`PointerOutcome::Cancelled`]); a new `Down` drops a capture
//! whose `Up` was lost.
//!
//! # Text (0.131.0)
//!
//! A focused `TextField` takes caret motion and the two deletions
//! ([`widgets::TextFieldKey`], `Shift` extending the selection) from
//! [`handle_widget_key`], and typed characters from
//! [`handle_widget_text`].
//!
//! # What this deliberately does not do
//!
//! - **No hover.** Tooltips are never shown from here (the owner drives
//!   them from its own pointer-move handling).
//! - **No escape-to-revert on a drag, and no grab offset**: a scrollbar
//!   drag re-centres the thumb on the pointer at the first `Move`.
//! - **A click does not place a text field's caret**, and word motion,
//!   select-all and the clipboard chords are not routed (a chord is
//!   never a widget's key — below).
//! - **Tree rows expand and collapse from the keyboard only** — a click
//!   on a row *activates* it (`handle_action`'s `Click`, which is all an
//!   assistive technology's `Click` does too); there is no
//!   disclosure-triangle hit area in this toolkit's paint to aim at.
//!   **Selecting** the row is deliberately not done here: single- or
//!   multi-selection is the tree's owner's policy, applied on the
//!   `Activated` outcome so the pointer, keyboard and accessibility paths
//!   share it (0.130.0 review revision — an earlier draft selected here,
//!   before a `Click` that could still be refused, and on the pointer and
//!   keyboard paths only).
//! - **A slider's value mapping is the widget's whole bounds**, not the
//!   painted track inset by the thumb's radius, over `extent - 1` pixels
//!   so the last hittable pixel (hit testing is exclusive at the far
//!   edge) reaches `max`. A scrollbar maps the pointer to the **thumb's
//!   centre** over the track the thumb travels, using the same
//!   `page_size`-proportional thumb length the painter draws.
//! - **Keys held with `Ctrl`, `Alt` or `Meta` are never a widget's** —
//!   [`handle_widget_key`] leaves every such chord to the caller's
//!   shortcuts; `Shift` alone is the colour picker's and curve editor's
//!   coarse step.
//!
//! The caller filters to the primary button, runs layout before routing
//! (hit testing reads last layout's bounds), and re-runs layout after a
//! routed event that changed structure (a dropdown opening, a menu
//! closing).

use accesskit::{Action, ActionData, ActionRequest, Orientation};

use crate::action::{ActionOutcome, ActionRejection, handle_action, payload_disabled};
use crate::error::WidgetError;
use crate::input::{FocusManager, FocusOrigin};
use crate::shortcut::{Modifiers, NamedKey};
use crate::tree::{ACCESSIBILITY_TREE_ID, WidgetId, WidgetTree};
use crate::widgets::{
    self, ColorPickerKey, ColorPickerOutcome, ColorPickerPart, CurveEditorKey, CurveEditorOutcome,
    DropdownKey, DropdownOutcome, MenuKey, MenuOutcome, TabBarKey, TabBarOutcome, TextFieldKey,
    WidgetKind,
};

/// Which half of a primary-button click an event is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerPhase {
    /// The button went down.
    Down,
    /// The button came back up.
    Up,
    /// The pointer moved while the button may be held (0.131.0). Only a
    /// captured drag acts on it; with nothing captured it is ignored
    /// without even a hit test.
    Move,
}

/// One primary-button pointer event in logical pixels, window space —
/// the caller has already filtered out every other button.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointerEvent {
    pub phase: PointerPhase,
    pub position: (f32, f32),
}

/// Which widget, if any, a primary `Down` armed for activation on the
/// matching `Up`, or captured for a drag (0.131.0). The two are
/// exclusive: an activation widget arms, a value widget captures, and
/// every `Down` drops whatever the last one left behind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClickTracker {
    pressed: Option<WidgetId>,
    captured: Option<Capture>,
}

/// What a value widget's `Down` captured: the widget, plus — for a colour
/// picker — which part the drag began on, so a drag that starts in the
/// saturation/value area never changes the hue however far it strays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Capture {
    Range(WidgetId),
    PickerArea {
        picker: WidgetId,
    },
    PickerHue {
        picker: WidgetId,
    },
    /// `point` is the index the `Down` grabbed and `count` the curve's
    /// point count then: a key that deletes or re-selects a point mid-drag
    /// changes one of them, and the drag then ends rather than moving a
    /// different point.
    CurvePoint {
        editor: WidgetId,
        point: usize,
        count: usize,
    },
}

impl Capture {
    fn id(self) -> WidgetId {
        match self {
            Self::Range(id)
            | Self::PickerArea { picker: id }
            | Self::PickerHue { picker: id }
            | Self::CurvePoint { editor: id, .. } => id,
        }
    }
}

impl ClickTracker {
    /// The armed widget, if a `Down` armed one and no `Up` has resolved
    /// it yet.
    #[must_use]
    pub fn pressed(&self) -> Option<WidgetId> {
        self.pressed
    }

    /// The widget a `Down` captured for a drag (a slider, scrollbar,
    /// colour picker or curve editor), if no `Up` has released it yet.
    #[must_use]
    pub fn captured(&self) -> Option<WidgetId> {
        self.captured.map(Capture::id)
    }

    /// Whether a `Down` left anything for a later event to resolve — an
    /// armed widget or a captured one.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.pressed.is_some() || self.captured.is_some()
    }

    /// Ends a drag without routing anything — for a pointer that left the
    /// window, whose `Up` may never arrive. The value the drag last set
    /// stays; an armed (not captured) widget is left alone.
    pub fn release_capture(&mut self) {
        self.captured = None;
    }

    /// Forgets the armed and the captured widget without activating or
    /// un-pressing anything — for an owner removing the subtree either
    /// lived in, so a later `Up` or `Move` cannot be routed to a stale id.
    pub fn reset(&mut self) {
        self.pressed = None;
        self.captured = None;
    }
}

/// What one routed pointer event or key did.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum PointerOutcome {
    /// Nothing interactive was under the pointer (or nothing was armed on
    /// an `Up`); nothing changed.
    Ignored,
    /// A `Down` armed this widget; the matching `Up` decides.
    Pressed(WidgetId),
    /// Focus moved to (or stayed on) this widget and nothing else changed.
    Focused(WidgetId),
    /// The `Up` landed away from the armed widget: nothing activated. Also
    /// a drag whose captured widget was removed or disabled mid-drag, or
    /// that the caller abandoned (0.131.0).
    Cancelled(WidgetId),
    /// The `Up` that ended a drag on this captured widget (0.131.0). The
    /// value is whatever the last `Down`/`Move` set — the `Up`'s own
    /// position is not applied (winit reports a `CursorMoved` before the
    /// release, so it has already been routed as a `Move`).
    Released(WidgetId),
    /// Internal state changed with no meaning beyond the widget itself —
    /// a menu's highlight moving.
    Changed(WidgetId),
    /// A real change, described in [`crate::handle_action`]'s own
    /// vocabulary — including the three mutations that bypass it
    /// (`Dropdown`, `ColorChanged`, `CurveChanged`), so a caller reads one
    /// outcome type whichever path ran.
    Action(ActionOutcome),
    /// The menu closed without activating anything (`Escape`). The menu's
    /// subtree is gone; restoring focus to its opener is the caller's job,
    /// exactly as after `MenuActivated`.
    MenuCancelled { menu: WidgetId },
}

/// What [`handle_widget_key`] did with a key.
#[derive(Debug, Clone, PartialEq)]
pub enum KeyOutcome {
    /// The focused widget has no meaning for this key — the caller should
    /// go on to its own shortcuts.
    Ignored,
    /// The focused widget consumed the key.
    Handled(PointerOutcome),
}

/// The widget families this module routes, resolved from a hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Interactive {
    /// Armed on `Down`, activated through `handle_action`'s `Click` on `Up`.
    Button,
    Clickable,
    TreeItem,
    Tab,
    ListRow,
    /// Set on `Down` from the pointer's position along the widget.
    Range,
    TextField,
    Picker,
    Curve,
}

fn classify(kind: &WidgetKind) -> Option<Interactive> {
    Some(match kind {
        WidgetKind::Button(_) => Interactive::Button,
        WidgetKind::Checkbox(_) | WidgetKind::ColorSwatch(_) | WidgetKind::Dropdown(_) => {
            Interactive::Clickable
        }
        WidgetKind::TreeItem(_) => Interactive::TreeItem,
        WidgetKind::Tab(_) => Interactive::Tab,
        WidgetKind::ListRow(_) => Interactive::ListRow,
        WidgetKind::Slider(_) | WidgetKind::Scrollbar(_) => Interactive::Range,
        WidgetKind::TextField(_) => Interactive::TextField,
        WidgetKind::ColorPicker(_) | WidgetKind::ColorPickerPart(_) => Interactive::Picker,
        WidgetKind::CurveEditor(_) | WidgetKind::CurveEditorPoint(_) => Interactive::Curve,
        _ => return None,
    })
}

/// The interactive widget `point` lands on: the hit widget itself or its
/// nearest interactive ancestor. A colour picker's or curve editor's
/// inner parts resolve to the picker/editor itself, which then resolves
/// the part from the point. The walk stops at a menu or dropdown list
/// (their padding is not a click on whatever owns them).
fn target_at(tree: &WidgetTree<WidgetKind>, point: (f32, f32)) -> Option<(WidgetId, Interactive)> {
    let mut current = tree.hit_test(point);
    while let Some(id) = current {
        let kind = tree.payload(id)?;
        if matches!(kind, WidgetKind::Menu(_) | WidgetKind::DropdownList) {
            return None;
        }
        if let Some(interactive) = classify(kind) {
            return Some(match interactive {
                Interactive::Picker => (enclosing(tree, id, is_picker)?, interactive),
                Interactive::Curve => (widgets::curve_editor_of(tree, id)?, interactive),
                _ => (id, interactive),
            });
        }
        current = tree.parent(id);
    }
    None
}

fn is_picker(kind: &WidgetKind) -> bool {
    matches!(kind, WidgetKind::ColorPicker(_))
}

/// `id` or its nearest ancestor whose payload satisfies `pred`.
fn enclosing(
    tree: &WidgetTree<WidgetKind>,
    id: WidgetId,
    pred: impl Fn(&WidgetKind) -> bool,
) -> Option<WidgetId> {
    let mut current = Some(id);
    while let Some(candidate) = current {
        if tree.payload(candidate).is_some_and(&pred) {
            return Some(candidate);
        }
        current = tree.parent(candidate);
    }
    None
}

fn is_disabled(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> bool {
    tree.accessibility(id)
        .is_some_and(accesskit::Node::is_disabled)
        || tree.payload(id).is_some_and(payload_disabled)
}

/// A request addressed to `id` in this toolkit's own accessibility tree —
/// exactly what an assistive technology would send.
fn request(id: WidgetId, action: Action, data: Option<ActionData>) -> ActionRequest {
    ActionRequest {
        action,
        target_tree: ACCESSIBILITY_TREE_ID,
        target_node: id,
        data,
    }
}

fn act(
    tree: &mut WidgetTree<WidgetKind>,
    focus: &mut FocusManager,
    id: WidgetId,
    action: Action,
    data: Option<ActionData>,
) -> Result<PointerOutcome, ActionRejection> {
    handle_action(tree, focus, &request(id, action, data)).map(PointerOutcome::Action)
}

/// Un-presses `id` if it is a button. A button that has since been
/// disabled or removed has no pressed state worth restoring, so those two
/// refusals are deliberately not errors here.
fn release_button(tree: &mut WidgetTree<WidgetKind>, id: WidgetId) -> Result<(), WidgetError> {
    if !matches!(tree.payload(id), Some(WidgetKind::Button(_))) {
        return Ok(());
    }
    match widgets::set_button_pressed(tree, id, false) {
        Ok(()) | Err(WidgetError::WidgetDisabled(_) | WidgetError::UnknownWidget(_)) => Ok(()),
        Err(err) => Err(err),
    }
}

/// Routes one primary-button pointer event — see this module's own doc
/// comment for what each widget does on `Down` and on `Up`.
/// `focus.validate` runs after every event, so a click that removed the
/// focused widget (a menu item's activation) never leaves focus dangling.
///
/// # Errors
///
/// [`ActionRejection::Disabled`] for a `Down` on a disabled widget (the
/// tree is left untouched), and whatever `handle_action` or a widget
/// mutator refuses on the way.
pub fn handle_pointer(
    tree: &mut WidgetTree<WidgetKind>,
    focus: &mut FocusManager,
    click: &mut ClickTracker,
    event: PointerEvent,
) -> Result<PointerOutcome, ActionRejection> {
    let result = match event.phase {
        PointerPhase::Down => pointer_down(tree, focus, click, event.position),
        PointerPhase::Up => pointer_up(tree, focus, click, event.position),
        PointerPhase::Move => pointer_move(tree, focus, click, event.position),
    };
    focus.validate(tree);
    result
}

fn pointer_down(
    tree: &mut WidgetTree<WidgetKind>,
    focus: &mut FocusManager,
    click: &mut ClickTracker,
    point: (f32, f32),
) -> Result<PointerOutcome, ActionRejection> {
    focus.note_input(tree, FocusOrigin::Pointer);
    // A `Down` without an `Up` in between (the release happened outside
    // the window): whatever was armed is abandoned, not activated.
    if let Some(stale) = click.pressed.take() {
        release_button(tree, stale)?;
    }
    // Likewise a drag whose `Up` never arrived: it simply ends where the
    // last `Move` left it.
    click.captured = None;
    let Some((id, interactive)) = target_at(tree, point) else {
        return Ok(PointerOutcome::Ignored);
    };
    if is_disabled(tree, id) {
        return Err(ActionRejection::Disabled(id));
    }
    match interactive {
        Interactive::Button => {
            focus_pointer(tree, focus, id)?;
            widgets::set_button_pressed(tree, id, true)?;
            click.pressed = Some(id);
            Ok(PointerOutcome::Pressed(id))
        }
        Interactive::Clickable | Interactive::TreeItem => {
            focus_pointer(tree, focus, id)?;
            click.pressed = Some(id);
            Ok(PointerOutcome::Pressed(id))
        }
        // Neither takes focus on `Down`: an inactive tab is not focusable
        // (roving focus) and focus moves to the new tab on `Up`; an
        // option row or menu item is never focused itself.
        Interactive::Tab | Interactive::ListRow => {
            click.pressed = Some(id);
            Ok(PointerOutcome::Pressed(id))
        }
        Interactive::TextField => {
            focus_pointer(tree, focus, id)?;
            Ok(PointerOutcome::Focused(id))
        }
        Interactive::Range => range_down(tree, focus, click, id, point),
        Interactive::Picker => picker_down(tree, focus, click, id, point),
        Interactive::Curve => curve_down(tree, focus, click, id, point),
    }
}

fn focus_pointer(
    tree: &mut WidgetTree<WidgetKind>,
    focus: &mut FocusManager,
    id: WidgetId,
) -> Result<(), WidgetError> {
    focus.focus_with(tree, id, FocusOrigin::Pointer)
}

/// The value under `point` along a slider or scrollbar, `None` for a
/// zero-sized widget or a non-finite point.
///
/// - **Slider**: the fraction of the widget's own width over
///   `width - 1` pixels (the last hittable pixel, since hit testing is
///   exclusive at the far edge), clamped to `[0, 1]`, onto `[min, max]`.
/// - **Scrollbar**: the pointer names where the thumb's **centre** should
///   go. The thumb travels `track - thumb` pixels (its length is
///   `page_size`-proportional, exactly as `paint_scrollbar` draws it), so
///   the fraction is `(at - thumb / 2) / (track - thumb)`, clamped — a
///   click on the thumb's current centre leaves the value where it is,
///   and anywhere within half a thumb of either end reaches `min`/`max`.
fn value_at(tree: &WidgetTree<WidgetKind>, id: WidgetId, point: (f32, f32)) -> Option<f64> {
    let bounds = tree.bounds(id)?;
    #[allow(clippy::cast_precision_loss)]
    let fraction = match tree.payload(id)? {
        WidgetKind::Slider(state) => {
            let extent = f64::from(bounds.width);
            let at = f64::from(point.0) - bounds.x as f64;
            let fraction = at / (extent - 1.0).max(1.0);
            (extent > 0.0).then_some((state.min, state.max, fraction))
        }
        WidgetKind::Scrollbar(state) => {
            let vertical = state.orientation == Orientation::Vertical;
            let (_, _, thumb_w, thumb_h) = crate::paint::scrollbar_thumb_rect(state, bounds);
            let (origin, extent, at, thumb) = if vertical {
                (
                    bounds.y as f64,
                    f64::from(bounds.height),
                    f64::from(point.1),
                    f64::from(thumb_h),
                )
            } else {
                (
                    bounds.x as f64,
                    f64::from(bounds.width),
                    f64::from(point.0),
                    f64::from(thumb_w),
                )
            };
            let travel = extent - thumb;
            let fraction = if travel > 0.0 {
                (at - origin - thumb / 2.0) / travel
            } else {
                // The thumb fills the track: every point is the start.
                0.0
            };
            (extent > 0.0).then_some((state.min, state.max, fraction))
        }
        _ => None,
    };
    let (min, max, fraction) = fraction?;
    if !fraction.is_finite() {
        return None;
    }
    Some(min + fraction.clamp(0.0, 1.0) * (max - min))
}

fn range_down(
    tree: &mut WidgetTree<WidgetKind>,
    focus: &mut FocusManager,
    click: &mut ClickTracker,
    id: WidgetId,
    point: (f32, f32),
) -> Result<PointerOutcome, ActionRejection> {
    focus_pointer(tree, focus, id)?;
    // Captured even when this point maps to no value: a later `Move` can.
    click.captured = Some(Capture::Range(id));
    let Some(value) = value_at(tree, id, point) else {
        return Ok(PointerOutcome::Focused(id));
    };
    act(
        tree,
        focus,
        id,
        Action::SetValue,
        Some(ActionData::NumericValue(value)),
    )
}

fn picker_down(
    tree: &mut WidgetTree<WidgetKind>,
    focus: &mut FocusManager,
    click: &mut ClickTracker,
    picker: WidgetId,
    point: (f32, f32),
) -> Result<PointerOutcome, ActionRejection> {
    let part = widgets::color_picker_part_at(tree, picker, point);
    let state = widgets::color_picker_state(tree, picker)?;
    // The clicked part's own tab stop, or the picker's first one for a
    // click on the preview (which is no part).
    let target = part
        .and_then(|part| state.part_id(part))
        .or_else(|| state.focus_target());
    if let Some(target) = target {
        focus_pointer(tree, focus, target)?;
    }
    // The part the drag began on is the part it drives; the preview is
    // no part and captures nothing.
    click.captured = match part {
        Some(ColorPickerPart::SaturationValue) => Some(Capture::PickerArea { picker }),
        Some(ColorPickerPart::Hue) => Some(Capture::PickerHue { picker }),
        None => None,
    };
    let outcome = match part {
        Some(ColorPickerPart::SaturationValue) => {
            widgets::set_saturation_value_from_point(tree, picker, point)?
        }
        Some(ColorPickerPart::Hue) => widgets::set_hue_from_point(tree, picker, point)?,
        None => ColorPickerOutcome::Ignored,
    };
    Ok(color_outcome(picker, outcome, target))
}

fn color_outcome(
    picker: WidgetId,
    outcome: ColorPickerOutcome,
    focused: Option<WidgetId>,
) -> PointerOutcome {
    match outcome {
        ColorPickerOutcome::Changed { hsv, color } => {
            PointerOutcome::Action(ActionOutcome::ColorChanged { picker, hsv, color })
        }
        ColorPickerOutcome::Ignored => PointerOutcome::Focused(focused.unwrap_or(picker)),
    }
}

fn curve_down(
    tree: &mut WidgetTree<WidgetKind>,
    focus: &mut FocusManager,
    click: &mut ClickTracker,
    editor: WidgetId,
    point: (f32, f32),
) -> Result<PointerOutcome, ActionRejection> {
    let (outcome, grabbed) =
        if let Some(index) = widgets::curve_editor_point_at(tree, editor, point.0, point.1)? {
            (widgets::select_curve_point(tree, editor, index)?, true)
        } else {
            let added = widgets::add_curve_point_from_point(tree, editor, point.0, point.1)?;
            // Add-then-drag: the new point is selected, so a drag moves
            // it. A refused add (too close to a neighbour) grabs nothing.
            (added, added == CurveEditorOutcome::Changed)
        };
    if grabbed {
        let state = widgets::curve_editor_state(tree, editor)?;
        click.captured = Some(Capture::CurvePoint {
            editor,
            point: state.selected(),
            count: state.curve().points().len(),
        });
    }
    // Re-read after the mutation: selecting or adding a point moves the
    // editor's one tab stop (roving focus).
    let target = widgets::curve_editor_state(tree, editor)?.focus_target();
    if let Some(target) = target {
        focus_pointer(tree, focus, target)?;
    }
    Ok(match outcome {
        CurveEditorOutcome::Changed => {
            PointerOutcome::Action(ActionOutcome::CurveChanged { editor })
        }
        CurveEditorOutcome::Ignored => PointerOutcome::Focused(target.unwrap_or(editor)),
    })
}

/// A `Move`: only a captured drag acts on it, through the same mutators
/// its `Down` used. Never a hit test, never a focus change, never
/// `note_input` — moving the mouse is not a modality switch.
fn pointer_move(
    tree: &mut WidgetTree<WidgetKind>,
    focus: &mut FocusManager,
    click: &mut ClickTracker,
    point: (f32, f32),
) -> Result<PointerOutcome, ActionRejection> {
    let Some(capture) = click.captured else {
        return Ok(PointerOutcome::Ignored);
    };
    let id = capture.id();
    if tree.payload(id).is_none() || is_disabled(tree, id) {
        // Removed or disabled mid-drag: the drag ends with whatever value
        // it last set, and nothing more is routed to it.
        click.captured = None;
        return Ok(PointerOutcome::Cancelled(id));
    }
    match capture {
        Capture::Range(id) => match value_at(tree, id, point) {
            Some(value) => act(
                tree,
                focus,
                id,
                Action::SetValue,
                Some(ActionData::NumericValue(value)),
            ),
            None => Ok(PointerOutcome::Ignored),
        },
        Capture::PickerArea { picker } => {
            let outcome = widgets::set_saturation_value_from_point(tree, picker, point)?;
            Ok(drag_color_outcome(picker, outcome))
        }
        Capture::PickerHue { picker } => {
            let outcome = widgets::set_hue_from_point(tree, picker, point)?;
            Ok(drag_color_outcome(picker, outcome))
        }
        Capture::CurvePoint {
            editor,
            point: grabbed,
            count,
        } => {
            let state = widgets::curve_editor_state(tree, editor)?;
            if state.selected() != grabbed || state.curve().points().len() != count {
                // A key deleted or re-selected the grabbed point mid-drag:
                // the drag ends rather than moving another point.
                click.captured = None;
                return Ok(PointerOutcome::Cancelled(editor));
            }
            Ok(
                match widgets::move_selected_point_from_point(tree, editor, point.0, point.1)? {
                    CurveEditorOutcome::Changed => {
                        PointerOutcome::Action(ActionOutcome::CurveChanged { editor })
                    }
                    CurveEditorOutcome::Ignored => PointerOutcome::Ignored,
                },
            )
        }
    }
}

/// A drag's colour outcome: `Ignored` (not `Focused`) when the point
/// changed nothing, since a `Move` never moves focus.
fn drag_color_outcome(picker: WidgetId, outcome: ColorPickerOutcome) -> PointerOutcome {
    match outcome {
        ColorPickerOutcome::Ignored => PointerOutcome::Ignored,
        changed @ ColorPickerOutcome::Changed { .. } => color_outcome(picker, changed, None),
    }
}

fn pointer_up(
    tree: &mut WidgetTree<WidgetKind>,
    focus: &mut FocusManager,
    click: &mut ClickTracker,
    point: (f32, f32),
) -> Result<PointerOutcome, ActionRejection> {
    if let Some(capture) = click.captured.take() {
        return Ok(PointerOutcome::Released(capture.id()));
    }
    let Some(armed) = click.pressed.take() else {
        return Ok(PointerOutcome::Ignored);
    };
    release_button(tree, armed)?;
    let Some((id, interactive)) = target_at(tree, point).filter(|&(id, _)| id == armed) else {
        return Ok(PointerOutcome::Cancelled(armed));
    };
    match interactive {
        Interactive::Button | Interactive::Clickable | Interactive::TreeItem => {
            act(tree, focus, id, Action::Click, None)
        }
        Interactive::Tab => {
            let outcome = act(tree, focus, id, Action::Click, None)?;
            focus_selected_tab(tree, focus, id, FocusOrigin::Pointer)?;
            Ok(outcome)
        }
        Interactive::ListRow => match widgets::dropdown_of_row(tree, id) {
            Some(dropdown) => {
                let outcome = widgets::commit_dropdown_row(tree, dropdown, id)?;
                focus_pointer(tree, focus, dropdown)?;
                Ok(PointerOutcome::Action(ActionOutcome::Dropdown {
                    id: dropdown,
                    outcome,
                }))
            }
            None => act(tree, focus, id, Action::Click, None),
        },
        // Never armed — a value widget *captures* on `Down` instead, and a
        // captured `Up` returned `Released` above — so an `Up` matching
        // the armed id cannot be one of these.
        Interactive::Range | Interactive::TextField | Interactive::Picker | Interactive::Curve => {
            Ok(PointerOutcome::Cancelled(armed))
        }
    }
}

/// Moves focus to `tab`'s bar's selected tab — the one tab stop under
/// roving focus — after a selection change.
fn focus_selected_tab(
    tree: &mut WidgetTree<WidgetKind>,
    focus: &mut FocusManager,
    tab: WidgetId,
    origin: FocusOrigin,
) -> Result<(), WidgetError> {
    let Some(bar) = tree.parent(tab) else {
        return Ok(());
    };
    if let Some(selected) = widgets::tab_bar_state(tree, bar)?.selected_tab() {
        focus.focus_with(tree, selected, origin)?;
    }
    Ok(())
}

/// Routes one key to the **focused** widget — see the table in this
/// module's own doc comment. [`KeyOutcome::Ignored`] whenever the focused
/// widget has no meaning for `key` (a text field's meaning is
/// [`widgets::TextFieldKey`]'s table), so the caller can fall through to its own
/// shortcuts. `Shift` is the colour picker's and curve editor's "coarse"
/// step; a key held with `Ctrl`, `Alt` or `Meta` is always
/// [`KeyOutcome::Ignored`] and changes nothing.
///
/// # Errors
///
/// Whatever `handle_action` or a widget's own key handler refuses.
pub fn handle_widget_key(
    tree: &mut WidgetTree<WidgetKind>,
    focus: &mut FocusManager,
    key: NamedKey,
    modifiers: Modifiers,
) -> Result<KeyOutcome, ActionRejection> {
    // A chord is a shortcut, never a widget key: `Ctrl+Home` must not
    // move a focused slider to its minimum and swallow the shortcut.
    if modifiers.control || modifiers.alt || modifiers.meta {
        return Ok(KeyOutcome::Ignored);
    }
    let result = widget_key(tree, focus, key, modifiers);
    focus.validate(tree);
    result
}

fn widget_key(
    tree: &mut WidgetTree<WidgetKind>,
    focus: &mut FocusManager,
    key: NamedKey,
    modifiers: Modifiers,
) -> Result<KeyOutcome, ActionRejection> {
    let Some(id) = focus.focused() else {
        return Ok(KeyOutcome::Ignored);
    };
    let Some(kind) = tree.payload(id) else {
        return Ok(KeyOutcome::Ignored);
    };
    let activate = matches!(key, NamedKey::Space | NamedKey::Enter);
    match kind {
        WidgetKind::Dropdown(_) => {
            let Some(key) = DropdownKey::from_named_key(key) else {
                return Ok(KeyOutcome::Ignored);
            };
            let outcome = widgets::handle_dropdown_key(tree, id, key)?;
            Ok(if outcome == DropdownOutcome::Ignored {
                KeyOutcome::Ignored
            } else {
                KeyOutcome::Handled(PointerOutcome::Action(ActionOutcome::Dropdown {
                    id,
                    outcome,
                }))
            })
        }
        WidgetKind::Tab(_) => tab_key(tree, focus, id, key),
        WidgetKind::Menu(_) => menu_key(tree, id, key),
        WidgetKind::ColorPickerPart(_) => picker_key(tree, id, key, modifiers),
        WidgetKind::CurveEditor(_) | WidgetKind::CurveEditorPoint(_) => {
            curve_key(tree, focus, id, key, modifiers)
        }
        WidgetKind::Slider(state) => {
            let (min, max) = (state.min, state.max);
            range_key(tree, focus, id, key, min, max)
        }
        WidgetKind::Scrollbar(state) => {
            let (min, max) = (state.min, state.max);
            range_key(tree, focus, id, key, min, max)
        }
        WidgetKind::Checkbox(_) if key == NamedKey::Space => {
            handled(act(tree, focus, id, Action::Click, None))
        }
        WidgetKind::Button(_) | WidgetKind::ColorSwatch(_) if activate => {
            handled(act(tree, focus, id, Action::Click, None))
        }
        WidgetKind::TreeItem(_) => tree_item_key(tree, focus, id, key),
        WidgetKind::TextField(_) => {
            // Outside the table (`Tab`, `Enter`, `Escape`, ...) the key
            // falls through, so focus traversal and dialogs still work.
            let Some(key) = TextFieldKey::from_named_key(key) else {
                return Ok(KeyOutcome::Ignored);
            };
            widgets::handle_text_field_key(tree, id, key, modifiers.shift)?;
            Ok(KeyOutcome::Handled(PointerOutcome::Changed(id)))
        }
        _ => Ok(KeyOutcome::Ignored),
    }
}

/// Routes typed `text` (a platform's committed characters for one key
/// press — winit's `KeyEvent::text`) to the **focused** text field
/// (0.131.0). Anything else focused, or nothing, is
/// [`KeyOutcome::Ignored`].
///
/// Which modifiers still type: `Alt`/`Option` and `Ctrl+Alt` (`AltGr` on
/// Windows reports both) produce real characters on many layouts, so
/// they type; `Meta`/`Cmd`, and `Ctrl` without `Alt`, are shortcut
/// chords and are refused so the caller's shortcuts see them. Control
/// characters are dropped ([`widgets::insert_text_field_text`]); text
/// that inserts nothing is [`KeyOutcome::Ignored`].
///
/// Whether a key press with `modifiers` is a shortcut chord rather than
/// typing: `Meta`, or `Ctrl` without `Alt`. `Ctrl+Alt` is typing, since
/// Windows reports `AltGr` as that pair (`AltGr+Q` is `@` on a German
/// layout). The single predicate both [`handle_widget_text`] and the
/// app's own "consume every character key while typing" rule use, so the
/// two can never disagree about which presses are text.
#[must_use]
pub fn is_shortcut_chord(modifiers: Modifiers) -> bool {
    modifiers.meta || (modifiers.control && !modifiers.alt)
}

/// # Errors
///
/// Whatever the text field's own mutator refuses — a disabled field
/// included.
pub fn handle_widget_text(
    tree: &mut WidgetTree<WidgetKind>,
    focus: &FocusManager,
    text: &str,
    modifiers: Modifiers,
) -> Result<KeyOutcome, ActionRejection> {
    if is_shortcut_chord(modifiers) {
        return Ok(KeyOutcome::Ignored);
    }
    let Some(id) = focus.focused() else {
        return Ok(KeyOutcome::Ignored);
    };
    if !matches!(tree.payload(id), Some(WidgetKind::TextField(_))) {
        return Ok(KeyOutcome::Ignored);
    }
    Ok(if widgets::insert_text_field_text(tree, id, text)? {
        KeyOutcome::Handled(PointerOutcome::Changed(id))
    } else {
        KeyOutcome::Ignored
    })
}

fn handled(result: Result<PointerOutcome, ActionRejection>) -> Result<KeyOutcome, ActionRejection> {
    result.map(KeyOutcome::Handled)
}

fn tab_key(
    tree: &mut WidgetTree<WidgetKind>,
    focus: &mut FocusManager,
    tab: WidgetId,
    key: NamedKey,
) -> Result<KeyOutcome, ActionRejection> {
    let (Some(key), Some(bar)) = (TabBarKey::from_named_key(key), tree.parent(tab)) else {
        return Ok(KeyOutcome::Ignored);
    };
    match widgets::handle_tab_bar_key(tree, bar, key)? {
        TabBarOutcome::Selected(index) => {
            focus_selected_tab(tree, focus, tab, FocusOrigin::Keyboard)?;
            Ok(KeyOutcome::Handled(PointerOutcome::Action(
                ActionOutcome::TabSelected { bar, index },
            )))
        }
        TabBarOutcome::Ignored => Ok(KeyOutcome::Ignored),
    }
}

fn menu_key(
    tree: &mut WidgetTree<WidgetKind>,
    menu: WidgetId,
    key: NamedKey,
) -> Result<KeyOutcome, ActionRejection> {
    let Some(key) = MenuKey::from_named_key(key) else {
        return Ok(KeyOutcome::Ignored);
    };
    Ok(match widgets::handle_menu_key(tree, menu, key)? {
        MenuOutcome::Ignored => KeyOutcome::Ignored,
        MenuOutcome::Moved(_) => KeyOutcome::Handled(PointerOutcome::Changed(menu)),
        MenuOutcome::Activated(index) => {
            KeyOutcome::Handled(PointerOutcome::Action(ActionOutcome::MenuActivated {
                menu,
                index,
            }))
        }
        MenuOutcome::Cancelled => KeyOutcome::Handled(PointerOutcome::MenuCancelled { menu }),
    })
}

fn picker_key(
    tree: &mut WidgetTree<WidgetKind>,
    part_id: WidgetId,
    key: NamedKey,
    modifiers: Modifiers,
) -> Result<KeyOutcome, ActionRejection> {
    let Some(key) = ColorPickerKey::from_named_key(key) else {
        return Ok(KeyOutcome::Ignored);
    };
    let Some(picker) = crate::action::owning_picker(tree, part_id) else {
        return Ok(KeyOutcome::Ignored);
    };
    let Some(part) = widgets::color_picker_part_of(tree, picker, part_id) else {
        return Ok(KeyOutcome::Ignored);
    };
    let outcome = widgets::handle_color_picker_key(tree, picker, part, key, modifiers.shift)?;
    Ok(match outcome {
        ColorPickerOutcome::Ignored => KeyOutcome::Ignored,
        changed @ ColorPickerOutcome::Changed { .. } => {
            KeyOutcome::Handled(color_outcome(picker, changed, Some(part_id)))
        }
    })
}

fn curve_key(
    tree: &mut WidgetTree<WidgetKind>,
    focus: &mut FocusManager,
    id: WidgetId,
    key: NamedKey,
    modifiers: Modifiers,
) -> Result<KeyOutcome, ActionRejection> {
    let (Some(key), Some(editor)) = (
        CurveEditorKey::from_named_key(key),
        widgets::curve_editor_of(tree, id),
    ) else {
        return Ok(KeyOutcome::Ignored);
    };
    match widgets::handle_curve_editor_key(tree, editor, key, modifiers.shift)? {
        CurveEditorOutcome::Ignored => Ok(KeyOutcome::Ignored),
        CurveEditorOutcome::Changed => {
            // Roving focus: a selection change, an insert or a delete can
            // move the one tab stop — follow it.
            if let Some(target) = widgets::curve_editor_state(tree, editor)?.focus_target() {
                focus.focus_with(tree, target, FocusOrigin::Keyboard)?;
            }
            Ok(KeyOutcome::Handled(PointerOutcome::Action(
                ActionOutcome::CurveChanged { editor },
            )))
        }
    }
}

fn range_key(
    tree: &mut WidgetTree<WidgetKind>,
    focus: &mut FocusManager,
    id: WidgetId,
    key: NamedKey,
    min: f64,
    max: f64,
) -> Result<KeyOutcome, ActionRejection> {
    let (action, data) = match key {
        NamedKey::ArrowRight | NamedKey::ArrowUp => (Action::Increment, None),
        NamedKey::ArrowLeft | NamedKey::ArrowDown => (Action::Decrement, None),
        NamedKey::Home => (Action::SetValue, Some(ActionData::NumericValue(min))),
        NamedKey::End => (Action::SetValue, Some(ActionData::NumericValue(max))),
        _ => return Ok(KeyOutcome::Ignored),
    };
    handled(act(tree, focus, id, action, data))
}

fn tree_item_key(
    tree: &mut WidgetTree<WidgetKind>,
    focus: &mut FocusManager,
    id: WidgetId,
    key: NamedKey,
) -> Result<KeyOutcome, ActionRejection> {
    let action = match key {
        NamedKey::Space | NamedKey::Enter => Action::Click,
        NamedKey::ArrowRight => Action::Expand,
        NamedKey::ArrowLeft => Action::Collapse,
        _ => return Ok(KeyOutcome::Ignored),
    };
    // A leaf row declares neither `Expand` nor `Collapse`: the key means
    // nothing to it, which is `Ignored`, not a refusal.
    let declared = tree
        .accessibility(id)
        .is_some_and(|node| node.supports_action(action));
    if !declared {
        return Ok(KeyOutcome::Ignored);
    }
    handled(act(tree, focus, id, action, None))
}

#[cfg(test)]
mod tests {
    use accesskit::{Orientation, Toggled};
    use aurora_core::ToneCurve;
    use aurora_theme::Color;
    use taffy::style_helpers::length;
    use taffy::{FlexDirection, Size, Style};

    use super::{
        ClickTracker, KeyOutcome, PointerEvent, PointerOutcome, PointerPhase, handle_pointer,
        handle_widget_key, handle_widget_text,
    };
    use crate::action::{ActionOutcome, ActionRejection};
    use crate::input::{FocusManager, FocusOrigin};
    use crate::shortcut::{Modifiers, NamedKey};
    use crate::tree::{WidgetId, WidgetTree};
    use crate::widgets::{
        self, DropdownOutcome, MenuItem, ScrollbarRange, WidgetKind, new_tree, test_scales,
    };

    const WIDTH: f32 = 400.0;
    const HEIGHT: f32 = 1400.0;
    const EDITOR: f32 = 160.0;

    struct Fixture {
        tree: WidgetTree<WidgetKind>,
        root: WidgetId,
        focus: FocusManager,
        click: ClickTracker,
        button: WidgetId,
        checkbox: WidgetId,
        slider: WidgetId,
        scrollbar: WidgetId,
        text_field: WidgetId,
        swatch: WidgetId,
        dropdown: WidgetId,
        tab_bar: WidgetId,
        rows: [WidgetId; 3],
        picker: WidgetId,
        curve: WidgetId,
    }

    fn ok<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    // One widget of every routed kind, built in one straight line; splitting
    // it would only scatter the inserts.
    #[allow(clippy::too_many_lines)]
    fn fixture() -> Fixture {
        let scales = test_scales();
        let (mut tree, root) = new_tree(Style {
            flex_direction: FlexDirection::Column,
            size: Size {
                width: length(WIDTH),
                height: length(HEIGHT),
            },
            ..Default::default()
        });
        let t = &mut tree;
        let button = ok(widgets::insert_button(t, root, &scales, "Button"));
        let checkbox = ok(widgets::insert_checkbox(t, root, &scales, "Checkbox"));
        let slider = ok(widgets::insert_slider(
            t, root, &scales, "Slider", 50.0, 0.0, 100.0,
        ));
        let scrollbar = ok(widgets::insert_scrollbar(
            t,
            root,
            &scales,
            Orientation::Horizontal,
            Some("Scrollbar"),
            0.0,
            ScrollbarRange {
                min: 0.0,
                max: 100.0,
                page_size: 0.0,
            },
        ));
        let text_field = ok(widgets::insert_text_field(t, root, &scales, "Text", ""));
        let swatch = ok(widgets::insert_color_swatch(
            t,
            root,
            &scales,
            Color { r: 0, g: 0, b: 0 },
        ));
        let dropdown = ok(widgets::insert_dropdown(
            t,
            root,
            &scales,
            "Dropdown",
            vec!["One".into(), "Two".into(), "Three".into()],
            Some(0),
        ));
        let tab_bar = ok(widgets::insert_tab_bar(
            t,
            root,
            &scales,
            "Tabs",
            vec!["A".into(), "B".into(), "C".into()],
            0,
        ));
        let holder = ok(widgets::insert_container(
            t,
            root,
            Style {
                flex_shrink: 0.0,
                size: Size {
                    width: length(WIDTH),
                    height: length(widgets::row_height(&scales) * 4.0),
                },
                ..Default::default()
            },
        ));
        let view = ok(widgets::insert_tree_view(t, holder, Some("Tree")));
        let parent = ok(widgets::insert_tree_item(t, view, &scales, "Parent", true));
        let first = ok(widgets::insert_tree_item(
            t, parent, &scales, "First", false,
        ));
        let second = ok(widgets::insert_tree_item(
            t, parent, &scales, "Second", false,
        ));
        ok(widgets::set_tree_item_expanded(t, parent, true));
        let picker = ok(widgets::insert_color_picker(
            t,
            root,
            &scales,
            "Picker",
            Color { r: 255, g: 0, b: 0 },
            EDITOR,
        ));
        let curve = ok(widgets::insert_curve_editor(
            t,
            root,
            &scales,
            "Curve",
            EDITOR,
            ToneCurve::identity(),
        ));
        tree.compute_layout(WIDTH, HEIGHT);
        Fixture {
            tree,
            root,
            focus: FocusManager::new(),
            click: ClickTracker::default(),
            button,
            checkbox,
            slider,
            scrollbar,
            text_field,
            swatch,
            dropdown,
            tab_bar,
            rows: [parent, first, second],
            picker,
            curve,
        }
    }

    /// `(fx, fy)` of the way across `id`'s bounds.
    fn at(tree: &WidgetTree<WidgetKind>, id: WidgetId, fx: f32, fy: f32) -> (f32, f32) {
        let Some(b) = tree.bounds(id) else {
            unreachable!("{id:?} is laid out");
        };
        #[allow(clippy::cast_precision_loss)]
        let point = (
            b.x as f32 + fx * b.width as f32,
            b.y as f32 + fy * b.height as f32,
        );
        point
    }

    fn centre(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> (f32, f32) {
        at(tree, id, 0.5, 0.5)
    }

    impl Fixture {
        fn event(&mut self, phase: PointerPhase, position: (f32, f32)) -> PointerOutcome {
            ok(self.try_event(phase, position))
        }

        fn try_event(
            &mut self,
            phase: PointerPhase,
            position: (f32, f32),
        ) -> Result<PointerOutcome, ActionRejection> {
            handle_pointer(
                &mut self.tree,
                &mut self.focus,
                &mut self.click,
                PointerEvent { phase, position },
            )
        }

        fn click(&mut self, position: (f32, f32)) -> (PointerOutcome, PointerOutcome) {
            let down = self.event(PointerPhase::Down, position);
            let up = self.event(PointerPhase::Up, position);
            self.tree.compute_layout(WIDTH, HEIGHT);
            (down, up)
        }

        fn key(&mut self, key: NamedKey, shift: bool) -> KeyOutcome {
            let modifiers = Modifiers {
                shift,
                ..Modifiers::none()
            };
            let outcome = ok(handle_widget_key(
                &mut self.tree,
                &mut self.focus,
                key,
                modifiers,
            ));
            self.tree.compute_layout(WIDTH, HEIGHT);
            outcome
        }

        fn pressed(&self, id: WidgetId) -> bool {
            matches!(self.tree.payload(id), Some(WidgetKind::Button(s)) if s.pressed)
        }

        fn checked(&self) -> Toggled {
            match self.tree.payload(self.checkbox) {
                Some(WidgetKind::Checkbox(s)) => s.checked,
                other => unreachable!("{other:?}"),
            }
        }

        fn number(&self, id: WidgetId) -> f64 {
            match self.tree.payload(id) {
                Some(WidgetKind::Slider(s)) => s.value,
                Some(WidgetKind::Scrollbar(s)) => s.value,
                other => unreachable!("{other:?}"),
            }
        }

        fn tree_item(&self, index: usize) -> (bool, bool) {
            let id = self.rows.get(index).copied();
            match id.and_then(|id| self.tree.payload(id)) {
                Some(WidgetKind::TreeItem(s)) => (s.selected, s.expanded),
                other => unreachable!("{other:?}"),
            }
        }
    }

    #[test]
    fn a_button_is_pressed_on_down_and_activated_on_up_over_it() {
        let mut f = fixture();
        let point = centre(&f.tree, f.button);
        assert_eq!(
            f.event(PointerPhase::Down, point),
            PointerOutcome::Pressed(f.button)
        );
        assert!(f.pressed(f.button), "pressed while held");
        assert_eq!(f.click.pressed(), Some(f.button));
        assert_eq!(f.focus.focused(), Some(f.button));
        assert!(!f.focus.focus_visible(), "a pointer press hides the ring");
        assert_eq!(
            f.event(PointerPhase::Up, point),
            PointerOutcome::Action(ActionOutcome::Activated(f.button))
        );
        assert!(!f.pressed(f.button), "released on up");
        assert_eq!(f.click.pressed(), None);
    }

    #[test]
    fn an_up_away_from_the_armed_widget_cancels_and_unpresses_it() {
        let mut f = fixture();
        f.event(PointerPhase::Down, centre(&f.tree, f.button));
        let checkbox = centre(&f.tree, f.checkbox);
        assert_eq!(
            f.event(PointerPhase::Up, checkbox),
            PointerOutcome::Cancelled(f.button)
        );
        assert!(!f.pressed(f.button));
        assert_eq!(f.checked(), Toggled::False, "the other widget got nothing");
        assert_eq!(
            f.event(PointerPhase::Up, checkbox),
            PointerOutcome::Ignored,
            "nothing armed any more"
        );
    }

    #[test]
    fn a_checkbox_toggles_on_up_only() {
        let mut f = fixture();
        let point = centre(&f.tree, f.checkbox);
        f.event(PointerPhase::Down, point);
        assert_eq!(f.checked(), Toggled::False, "down alone does nothing");
        f.event(PointerPhase::Up, point);
        assert_eq!(f.checked(), Toggled::True);
        f.click(point);
        assert_eq!(f.checked(), Toggled::False);
    }

    #[test]
    fn a_slider_takes_the_value_under_the_pointer_on_down_and_clamps() {
        let mut f = fixture();
        let quarter = at(&f.tree, f.slider, 0.25, 0.5);
        // A quarter of the width over `width - 1` pixels: a hair past 25.
        let down = f.event(PointerPhase::Down, quarter);
        assert!(
            matches!(
                down,
                PointerOutcome::Action(ActionOutcome::ValueChanged { id, value })
                    if id == f.slider && (value - 25.0).abs() < 0.1
            ),
            "{down:?}"
        );
        assert_eq!(f.focus.focused(), Some(f.slider));
        assert_eq!(f.click.pressed(), None, "a value widget is never armed");
        let _ = f.event(PointerPhase::Up, quarter);
        // Past the right edge the fraction clamps -- but a point outside
        // the slider hits something else, so aim at the last pixel.
        let Some(b) = f.tree.bounds(f.slider) else {
            unreachable!()
        };
        #[allow(clippy::cast_precision_loss)]
        let end = (b.right() as f32 - 0.01, centre(&f.tree, f.slider).1);
        f.event(PointerPhase::Down, end);
        assert!(f.number(f.slider) > 99.9 && f.number(f.slider) <= 100.0);
    }

    /// Red-team RT-2: hit testing is exclusive at the far edge, so the
    /// last *hittable* pixel starts at `right - 1` — mapping over the
    /// whole width left `max` unreachable (99.75 of 100 on a 400 px bar).
    #[test]
    fn a_press_on_the_first_or_last_hittable_pixel_reaches_min_or_max() {
        let mut f = fixture();
        for id in [f.slider, f.scrollbar] {
            let Some(b) = f.tree.bounds(id) else {
                unreachable!()
            };
            let y = centre(&f.tree, id).1;
            #[allow(clippy::cast_precision_loss)]
            let (first, last) = (b.x as f32, b.right() as f32 - 1.0);
            assert_eq!(f.tree.hit_test((last, y)), Some(id), "still over {id:?}");
            f.event(PointerPhase::Down, (last, y));
            assert!((f.number(id) - 100.0).abs() < 1e-9, "{}", f.number(id));
            f.event(PointerPhase::Down, (first, y));
            assert!(f.number(id).abs() < 1e-9, "{}", f.number(id));
        }
    }

    #[test]
    fn a_scrollbar_takes_the_value_under_the_pointer_on_down() {
        let mut f = fixture();
        f.event(PointerPhase::Down, at(&f.tree, f.scrollbar, 0.75, 0.5));
        // The fixture's `page_size` is 0: the thumb is its minimum length
        // (the bar's thickness), so 3/4 of the way along is a little
        // under 75 once the thumb's half-length is taken off each end.
        let value = f.number(f.scrollbar);
        assert!(value > 70.0 && value < 80.0, "{value}");
    }

    /// Critic C10: the scrollbar maps the pointer to the **thumb's
    /// centre**, using the painter's `page_size`-proportional thumb —
    /// a press on the thumb's own centre leaves the value where it is.
    #[test]
    fn a_press_on_a_scrollbars_thumb_centre_keeps_its_value() {
        let mut f = fixture();
        let bar = ok(widgets::insert_scrollbar(
            &mut f.tree,
            f.root,
            &test_scales(),
            Orientation::Horizontal,
            Some("Paged"),
            40.0,
            ScrollbarRange {
                min: 0.0,
                max: 100.0,
                page_size: 50.0,
            },
        ));
        f.tree.compute_layout(WIDTH, HEIGHT);
        let Some(b) = f.tree.bounds(bar) else {
            unreachable!()
        };
        let Some(WidgetKind::Scrollbar(state)) = f.tree.payload(bar) else {
            unreachable!()
        };
        let (thumb_x, thumb_y, thumb_w, thumb_h) = crate::paint::scrollbar_thumb_rect(state, b);
        assert!(
            thumb_w > 0.3 * WIDTH,
            "a page-proportional thumb: {thumb_w}"
        );
        let mid_y = thumb_y + thumb_h / 2.0;
        f.event(PointerPhase::Down, (thumb_x + thumb_w / 2.0, mid_y));
        assert!((f.number(bar) - 40.0).abs() < 0.5, "{}", f.number(bar));
        let Some(b) = f.tree.bounds(bar) else {
            unreachable!()
        };
        #[allow(clippy::cast_precision_loss)]
        let last = (b.right() as f32 - 1.0, mid_y);
        f.event(PointerPhase::Down, last);
        assert!((f.number(bar) - 100.0).abs() < 1e-9, "{}", f.number(bar));
    }

    /// Critic C7: a `Ctrl`/`Alt`/`Meta` chord is the caller's shortcut,
    /// never a focused widget's key.
    #[test]
    fn a_key_held_with_ctrl_alt_or_meta_is_never_a_widgets() {
        let mut f = fixture();
        ok(f.focus.focus(&mut f.tree, f.slider));
        for modifiers in [
            Modifiers {
                control: true,
                ..Modifiers::none()
            },
            Modifiers {
                alt: true,
                ..Modifiers::none()
            },
            Modifiers {
                meta: true,
                ..Modifiers::none()
            },
        ] {
            let outcome = ok(handle_widget_key(
                &mut f.tree,
                &mut f.focus,
                NamedKey::Home,
                modifiers,
            ));
            assert_eq!(outcome, KeyOutcome::Ignored, "{modifiers:?}");
            assert!((f.number(f.slider) - 50.0).abs() < f64::EPSILON);
        }
        assert!(matches!(
            f.key(NamedKey::Home, true),
            KeyOutcome::Handled(_)
        ));
        assert!(
            f.number(f.slider).abs() < f64::EPSILON,
            "Shift alone is fine"
        );
    }

    #[test]
    fn a_text_field_is_focused_and_nothing_else() {
        let mut f = fixture();
        let point = centre(&f.tree, f.text_field);
        assert_eq!(
            f.event(PointerPhase::Down, point),
            PointerOutcome::Focused(f.text_field)
        );
        for key in [NamedKey::Enter, NamedKey::Tab, NamedKey::Escape] {
            assert_eq!(
                f.key(key, false),
                KeyOutcome::Ignored,
                "{key:?} is outside the text field's table and falls through"
            );
        }
        assert_eq!(f.click.captured(), None, "a text field captures nothing");
    }

    #[test]
    fn a_swatch_click_activates_it() {
        let mut f = fixture();
        let (_, up) = f.click(centre(&f.tree, f.swatch));
        assert_eq!(
            up,
            PointerOutcome::Action(ActionOutcome::Activated(f.swatch))
        );
    }

    #[test]
    fn a_tree_row_click_activates_it_without_selecting_and_keys_expand_and_collapse() {
        let mut f = fixture();
        let [parent, first, _] = f.rows;
        let (_, up) = f.click(centre(&f.tree, first));
        assert_eq!(up, PointerOutcome::Action(ActionOutcome::Activated(first)));
        assert!(
            !f.tree_item(1).0,
            "selection is the owner's policy, applied on `Activated`"
        );
        let (_, up) = f.click(at(&f.tree, parent, 0.5, 0.1));
        assert_eq!(up, PointerOutcome::Action(ActionOutcome::Activated(parent)));
        assert!(!f.tree_item(0).0);
        assert_eq!(f.focus.focused(), Some(parent));
        assert_eq!(
            f.key(NamedKey::Enter, false),
            KeyOutcome::Handled(PointerOutcome::Action(ActionOutcome::Activated(parent)))
        );
        assert!(
            !f.tree_item(0).0,
            "the keyboard path selects nothing either"
        );
        assert!(f.tree_item(0).1, "starts expanded");
        assert!(matches!(
            f.key(NamedKey::ArrowLeft, false),
            KeyOutcome::Handled(PointerOutcome::Action(ActionOutcome::ExpandedChanged {
                expanded: false,
                ..
            }))
        ));
        assert!(!f.tree_item(0).1);
        f.key(NamedKey::ArrowRight, false);
        assert!(f.tree_item(0).1);
    }

    #[test]
    fn a_dropdown_opens_on_click_and_a_row_click_commits_and_closes() {
        let mut f = fixture();
        let (_, up) = f.click(centre(&f.tree, f.dropdown));
        assert_eq!(
            up,
            PointerOutcome::Action(ActionOutcome::Dropdown {
                id: f.dropdown,
                outcome: DropdownOutcome::Opened
            })
        );
        let rows = ok(widgets::dropdown_state(&f.tree, f.dropdown))
            .rows()
            .to_vec();
        let Some(&row) = rows.get(1) else {
            unreachable!("three rows")
        };
        let (down, up) = f.click(centre(&f.tree, row));
        assert_eq!(down, PointerOutcome::Pressed(row));
        assert_eq!(
            up,
            PointerOutcome::Action(ActionOutcome::Dropdown {
                id: f.dropdown,
                outcome: DropdownOutcome::Committed {
                    index: 1,
                    changed: true
                }
            })
        );
        let state = ok(widgets::dropdown_state(&f.tree, f.dropdown));
        assert_eq!(state.selected(), Some(1));
        assert!(!state.is_open());
        assert_eq!(f.focus.focused(), Some(f.dropdown));
    }

    #[test]
    fn dropdown_keys_open_move_and_commit() {
        let mut f = fixture();
        ok(f.focus.focus(&mut f.tree, f.dropdown));
        f.key(NamedKey::ArrowDown, false);
        f.key(NamedKey::ArrowDown, false);
        f.key(NamedKey::ArrowDown, false);
        f.key(NamedKey::Enter, false);
        assert_eq!(
            ok(widgets::dropdown_state(&f.tree, f.dropdown)).selected(),
            Some(2)
        );
        assert_eq!(
            f.key(NamedKey::Escape, false),
            KeyOutcome::Ignored,
            "a closed dropdown's Escape falls through"
        );
    }

    #[test]
    fn a_tab_click_selects_that_tab_and_focuses_it() {
        let mut f = fixture();
        let tabs = ok(widgets::tab_bar_state(&f.tree, f.tab_bar))
            .tabs()
            .to_vec();
        let Some(&second) = tabs.get(1) else {
            unreachable!()
        };
        let (_, up) = f.click(centre(&f.tree, second));
        assert_eq!(
            up,
            PointerOutcome::Action(ActionOutcome::TabSelected {
                bar: f.tab_bar,
                index: 1
            })
        );
        assert_eq!(f.focus.focused(), Some(second));
        f.key(NamedKey::ArrowRight, false);
        assert_eq!(ok(widgets::tab_bar_state(&f.tree, f.tab_bar)).selected(), 2);
        assert_eq!(f.focus.focused(), tabs.get(2).copied());
    }

    #[test]
    fn a_menu_item_click_activates_it_and_removes_the_menu() {
        let mut f = fixture();
        let scales = test_scales();
        let menu = ok(widgets::open_menu(
            &mut f.tree,
            f.root,
            &scales,
            "Menu",
            (0.0, 0.0),
            EDITOR,
            vec![MenuItem::action("First"), MenuItem::action("Second")],
        ));
        f.tree.compute_layout(WIDTH, HEIGHT);
        let items = ok(widgets::menu_state(&f.tree, menu)).item_ids().to_vec();
        let Some(&second) = items.get(1) else {
            unreachable!()
        };
        let (_, up) = f.click(centre(&f.tree, second));
        assert_eq!(
            up,
            PointerOutcome::Action(ActionOutcome::MenuActivated { menu, index: 1 })
        );
        assert!(!f.tree.contains(menu));
    }

    #[test]
    fn menu_keys_move_activate_and_cancel() {
        let mut f = fixture();
        let scales = test_scales();
        let open = |f: &mut Fixture| {
            let menu = ok(widgets::open_menu(
                &mut f.tree,
                f.root,
                &scales,
                "Menu",
                (0.0, 0.0),
                EDITOR,
                vec![MenuItem::action("First"), MenuItem::action("Second")],
            ));
            ok(f.focus.focus(&mut f.tree, menu));
            menu
        };
        let menu = open(&mut f);
        assert_eq!(
            f.key(NamedKey::ArrowDown, false),
            KeyOutcome::Handled(PointerOutcome::Changed(menu))
        );
        assert_eq!(
            f.key(NamedKey::Enter, false),
            KeyOutcome::Handled(PointerOutcome::Action(ActionOutcome::MenuActivated {
                menu,
                index: 1
            }))
        );
        assert_eq!(f.focus.focused(), None, "validated: the menu is gone");
        let menu = open(&mut f);
        assert_eq!(
            f.key(NamedKey::Escape, false),
            KeyOutcome::Handled(PointerOutcome::MenuCancelled { menu })
        );
    }

    fn hsv(f: &Fixture) -> widgets::Hsv {
        ok(widgets::color_picker_state(&f.tree, f.picker)).hsv()
    }

    #[test]
    fn a_picker_area_click_sets_saturation_and_value_and_a_hue_click_the_hue() {
        let mut f = fixture();
        let state = ok(widgets::color_picker_state(&f.tree, f.picker));
        let (Some(area), Some(hue)) = (
            state.area_id(),
            state.part_id(widgets::ColorPickerPart::Hue),
        ) else {
            unreachable!()
        };
        ok(widgets::set_color_picker_hsv(
            &mut f.tree,
            f.picker,
            widgets::Hsv {
                hue: 0.0,
                saturation: 0.5,
                value: 0.5,
            },
        ));
        let outcome = f.event(PointerPhase::Down, at(&f.tree, area, 0.999, 0.001));
        assert!(matches!(
            outcome,
            PointerOutcome::Action(ActionOutcome::ColorChanged { picker, .. }) if picker == f.picker
        ));
        let after = hsv(&f);
        assert!(after.saturation > 0.99 && after.value > 0.99, "{after:?}");
        let before = after.hue;
        f.event(PointerPhase::Down, at(&f.tree, hue, 0.5, 0.5));
        assert!((hsv(&f).hue - before).abs() > 1.0, "hue moved");
        assert_eq!(
            f.focus.focused(),
            Some(hue),
            "the clicked part's tab stop takes focus"
        );
    }

    #[test]
    fn shift_is_the_pickers_coarse_step() {
        let fine = {
            let mut f = fixture();
            let state = ok(widgets::color_picker_state(&f.tree, f.picker));
            let Some(target) = state.focus_target() else {
                unreachable!()
            };
            ok(f.focus.focus(&mut f.tree, target));
            ok(widgets::set_color_picker_hsv(
                &mut f.tree,
                f.picker,
                widgets::Hsv {
                    hue: 0.0,
                    saturation: 0.5,
                    value: 0.5,
                },
            ));
            f.key(NamedKey::ArrowRight, false);
            let fine = hsv(&f).saturation - 0.5;
            ok(widgets::set_color_picker_hsv(
                &mut f.tree,
                f.picker,
                widgets::Hsv {
                    hue: 0.0,
                    saturation: 0.5,
                    value: 0.5,
                },
            ));
            f.key(NamedKey::ArrowRight, true);
            let coarse = hsv(&f).saturation - 0.5;
            (fine, coarse)
        };
        assert!(fine.0 > 0.0, "a plain arrow steps: {fine:?}");
        assert!(fine.1 > fine.0, "shift steps further: {fine:?}");
    }

    /// Red-team RT-5 (mutation 6b survived): `Shift` must reach the curve
    /// editor as its coarse step, not only the colour picker's.
    #[test]
    fn shift_is_the_curve_editors_coarse_step() {
        let mut f = fixture();
        let Some(target) = ok(widgets::curve_editor_state(&f.tree, f.curve)).focus_target() else {
            unreachable!()
        };
        ok(f.focus.focus(&mut f.tree, target));
        let y = |f: &Fixture| {
            ok(widgets::curve_editor_state(&f.tree, f.curve))
                .curve()
                .points()
                .first()
                .map_or(f32::NAN, |p| p.y)
        };
        let start = y(&f);
        f.key(NamedKey::ArrowUp, false);
        let fine = y(&f) - start;
        let before = y(&f);
        f.key(NamedKey::ArrowUp, true);
        let coarse = y(&f) - before;
        assert!(fine > 0.0, "a plain arrow steps: {fine}");
        assert!(coarse > fine, "shift steps further: {fine} vs {coarse}");
    }

    fn points(f: &Fixture) -> usize {
        ok(widgets::curve_editor_state(&f.tree, f.curve))
            .curve()
            .points()
            .len()
    }

    #[test]
    fn a_curve_click_on_a_point_selects_it_and_on_empty_space_adds_one() {
        let mut f = fixture();
        let selected = ok(widgets::curve_editor_state(&f.tree, f.curve)).selected();
        assert_eq!(selected, 0, "an editor starts on its first point");
        // The identity's last point, (1, 1), sits at the plot's top-right.
        #[allow(clippy::cast_precision_loss)]
        let marker = test_scales().spacing.xs as f32;
        let (Some((left, top, width, _)), count) = (
            f.tree
                .bounds(f.curve)
                .and_then(|b| widgets::plot_rect(b, marker)),
            points(&f),
        ) else {
            unreachable!("laid out")
        };
        f.event(PointerPhase::Down, (left + width, top));
        assert_eq!(points(&f), count, "selecting adds nothing");
        assert_ne!(
            ok(widgets::curve_editor_state(&f.tree, f.curve)).selected(),
            selected
        );
        let outcome = f.event(PointerPhase::Down, at(&f.tree, f.curve, 0.5, 0.2));
        assert_eq!(
            outcome,
            PointerOutcome::Action(ActionOutcome::CurveChanged { editor: f.curve })
        );
        assert_eq!(points(&f), count + 1, "empty space adds a point");
        assert_eq!(
            f.focus.focused(),
            ok(widgets::curve_editor_state(&f.tree, f.curve)).focus_target()
        );
    }

    #[test]
    fn a_disabled_widget_refuses_the_press_and_nothing_changes() {
        let mut f = fixture();
        ok(widgets::set_checkbox_disabled(
            &mut f.tree,
            f.checkbox,
            true,
        ));
        f.tree.compute_layout(WIDTH, HEIGHT);
        match f.try_event(PointerPhase::Down, centre(&f.tree, f.checkbox)) {
            Err(ActionRejection::Disabled(id)) => assert_eq!(id, f.checkbox),
            other => unreachable!("expected Disabled, got {other:?}"),
        }
        assert_eq!(f.click.pressed(), None);
        assert_eq!(f.focus.focused(), None);
        assert_eq!(f.checked(), Toggled::False);
    }

    #[test]
    fn slider_and_checkbox_keys() {
        let mut f = fixture();
        ok(f.focus.focus(&mut f.tree, f.slider));
        f.key(NamedKey::ArrowRight, false);
        assert!(f.number(f.slider) > 50.0);
        f.key(NamedKey::Home, false);
        assert!(f.number(f.slider).abs() < f64::EPSILON);
        f.key(NamedKey::End, false);
        assert!((f.number(f.slider) - 100.0).abs() < f64::EPSILON);
        ok(f.focus.focus(&mut f.tree, f.checkbox));
        f.key(NamedKey::Space, false);
        assert_eq!(f.checked(), Toggled::True);
        assert_eq!(f.key(NamedKey::Enter, false), KeyOutcome::Ignored);
        ok(f.focus.focus(&mut f.tree, f.button));
        assert_eq!(
            f.key(NamedKey::Tab, false),
            KeyOutcome::Ignored,
            "a key the button has no meaning for falls through"
        );
    }

    #[test]
    fn a_reset_tracker_routes_no_stale_up() {
        let mut f = fixture();
        let point = centre(&f.tree, f.checkbox);
        f.event(PointerPhase::Down, point);
        f.click.reset();
        assert_eq!(f.event(PointerPhase::Up, point), PointerOutcome::Ignored);
        assert_eq!(f.checked(), Toggled::False);
    }

    // -- drags (0.131.0) --

    fn below_and_left_of_everything() -> (f32, f32) {
        (-500.0, HEIGHT + 500.0)
    }

    #[test]
    fn a_slider_drag_follows_moves_and_clamps_outside_its_bounds() {
        let mut f = fixture();
        f.event(PointerPhase::Down, at(&f.tree, f.slider, 0.25, 0.5));
        assert_eq!(f.click.captured(), Some(f.slider));
        let moved = f.event(PointerPhase::Move, at(&f.tree, f.slider, 0.75, 0.5));
        assert!(
            matches!(
                moved,
                PointerOutcome::Action(ActionOutcome::ValueChanged { id, value })
                    if id == f.slider && (value - 75.0).abs() < 0.3
            ),
            "{moved:?}"
        );
        // Far outside the slider (over other widgets or nothing at all):
        // the drag still drives the slider, clamped.
        f.event(PointerPhase::Move, below_and_left_of_everything());
        assert!(f.number(f.slider).abs() < 1e-9, "{}", f.number(f.slider));
        f.event(PointerPhase::Move, (WIDTH + 500.0, -500.0));
        assert!((f.number(f.slider) - 100.0).abs() < 1e-9);
        f.event(PointerPhase::Move, centre(&f.tree, f.checkbox));
        assert_eq!(
            f.checked(),
            Toggled::False,
            "the widget under it got nothing"
        );
    }

    #[test]
    fn a_move_without_capture_changes_nothing() {
        let mut f = fixture();
        let over = at(&f.tree, f.slider, 0.9, 0.5);
        assert_eq!(f.event(PointerPhase::Move, over), PointerOutcome::Ignored);
        assert!((f.number(f.slider) - 50.0).abs() < f64::EPSILON);
        // An armed (not captured) button does not make a move a drag.
        f.event(PointerPhase::Down, centre(&f.tree, f.button));
        assert_eq!(f.click.captured(), None);
        assert_eq!(f.event(PointerPhase::Move, over), PointerOutcome::Ignored);
        assert!((f.number(f.slider) - 50.0).abs() < f64::EPSILON);
        assert_eq!(f.click.pressed(), Some(f.button), "still armed");
    }

    #[test]
    fn up_releases_the_capture_and_later_moves_do_nothing() {
        let mut f = fixture();
        f.event(PointerPhase::Down, at(&f.tree, f.slider, 0.25, 0.5));
        let before = f.number(f.slider);
        assert_eq!(
            f.event(PointerPhase::Up, at(&f.tree, f.slider, 0.9, 0.5)),
            PointerOutcome::Released(f.slider)
        );
        assert!(
            (f.number(f.slider) - before).abs() < f64::EPSILON,
            "the Up's own position is not applied"
        );
        assert_eq!(f.click.captured(), None);
        assert!(!f.click.is_active());
        assert_eq!(
            f.event(PointerPhase::Move, at(&f.tree, f.slider, 0.9, 0.5)),
            PointerOutcome::Ignored
        );
        assert!((f.number(f.slider) - before).abs() < f64::EPSILON);
        assert_eq!(
            f.event(PointerPhase::Up, at(&f.tree, f.slider, 0.9, 0.5)),
            PointerOutcome::Ignored,
            "a second Up has nothing to release"
        );
    }

    #[test]
    fn a_scrollbar_drag_maps_the_thumb_centre() {
        let mut f = fixture();
        f.event(PointerPhase::Down, at(&f.tree, f.scrollbar, 0.1, 0.5));
        assert_eq!(f.click.captured(), Some(f.scrollbar));
        let three_quarters = at(&f.tree, f.scrollbar, 0.75, 0.5);
        f.event(PointerPhase::Move, three_quarters);
        let dragged = f.number(f.scrollbar);
        // The same mapping a press there gives.
        let mut g = fixture();
        g.event(PointerPhase::Down, three_quarters);
        assert!(
            (dragged - g.number(g.scrollbar)).abs() < 1e-9,
            "{dragged} vs {}",
            g.number(g.scrollbar)
        );
        f.event(PointerPhase::Move, (WIDTH + 500.0, 0.0));
        assert!((f.number(f.scrollbar) - 100.0).abs() < 1e-9);
    }

    fn picker_parts(f: &Fixture) -> (WidgetId, WidgetId) {
        let state = ok(widgets::color_picker_state(&f.tree, f.picker));
        let (Some(area), Some(hue)) = (
            state.area_id(),
            state.part_id(widgets::ColorPickerPart::Hue),
        ) else {
            unreachable!()
        };
        (area, hue)
    }

    fn set_hsv(f: &mut Fixture, hue: f32, saturation: f32, value: f32) {
        ok(widgets::set_color_picker_hsv(
            &mut f.tree,
            f.picker,
            widgets::Hsv {
                hue,
                saturation,
                value,
            },
        ));
    }

    #[test]
    fn a_picker_area_drag_clamps_and_never_changes_hue() {
        let mut f = fixture();
        let (area, hue) = picker_parts(&f);
        set_hsv(&mut f, 120.0, 0.5, 0.5);
        f.event(PointerPhase::Down, at(&f.tree, area, 0.5, 0.5));
        assert_eq!(f.click.captured(), Some(f.picker));
        // Onto the hue strip: an area drag stays an area drag.
        f.event(PointerPhase::Move, at(&f.tree, hue, 0.1, 0.1));
        assert!((hsv(&f).hue - 120.0).abs() < 1e-3, "{:?}", hsv(&f));
        // Far past the area's top-right: saturation and value clamp at 1.
        let moved = f.event(PointerPhase::Move, (WIDTH + 500.0, -500.0));
        assert!(matches!(
            moved,
            PointerOutcome::Action(ActionOutcome::ColorChanged { picker, .. }) if picker == f.picker
        ));
        let after = hsv(&f);
        assert!(after.saturation > 0.999 && after.value > 0.999, "{after:?}");
        assert!((after.hue - 120.0).abs() < 1e-3, "{after:?}");
    }

    #[test]
    fn a_hue_drag_stays_on_hue() {
        let mut f = fixture();
        let (area, hue) = picker_parts(&f);
        set_hsv(&mut f, 0.0, 0.25, 0.75);
        f.event(PointerPhase::Down, at(&f.tree, hue, 0.5, 0.5));
        let start = hsv(&f).hue;
        // Into the area: the drag keeps driving the hue, never s/v.
        f.event(PointerPhase::Move, at(&f.tree, area, 0.9, 0.9));
        f.event(PointerPhase::Move, at(&f.tree, hue, 0.9, 0.9));
        let after = hsv(&f);
        assert!((after.hue - start).abs() > 1.0, "hue moved: {after:?}");
        assert!(
            (after.saturation - 0.25).abs() < 1e-6 && (after.value - 0.75).abs() < 1e-6,
            "{after:?}"
        );
    }

    fn plot(f: &Fixture) -> (f32, f32, f32, f32) {
        #[allow(clippy::cast_precision_loss)]
        let marker = test_scales().spacing.xs as f32;
        let Some(rect) = f
            .tree
            .bounds(f.curve)
            .and_then(|b| widgets::plot_rect(b, marker))
        else {
            unreachable!("laid out")
        };
        rect
    }

    fn selected_point(f: &Fixture) -> (f32, f32) {
        let state = ok(widgets::curve_editor_state(&f.tree, f.curve));
        state
            .curve()
            .points()
            .get(state.selected())
            .map_or((f32::NAN, f32::NAN), |p| (p.x, p.y))
    }

    #[test]
    fn a_curve_point_drag_moves_the_selected_point_without_adding() {
        let mut f = fixture();
        let (left, top, width, height) = plot(&f);
        let count = points(&f);
        f.event(PointerPhase::Down, (left + width, top));
        assert_eq!(f.click.captured(), Some(f.curve));
        let moved = f.event(PointerPhase::Move, (left + width * 0.5, top + height * 0.5));
        assert_eq!(
            moved,
            PointerOutcome::Action(ActionOutcome::CurveChanged { editor: f.curve })
        );
        let (x, y) = selected_point(&f);
        assert!((x - 1.0).abs() < 1e-6, "an endpoint keeps its input: {x}");
        assert!((y - 0.5).abs() < 0.02, "{y}");
        assert_eq!(points(&f), count, "a drag adds nothing");
        // Far below the plot: the output clamps at 0.
        f.event(PointerPhase::Move, (left, top + height * 10.0));
        assert!(selected_point(&f).1.abs() < 1e-6);
        assert_eq!(points(&f), count);
    }

    #[test]
    fn a_curve_add_then_drag_moves_the_new_point() {
        let mut f = fixture();
        let (left, top, width, height) = plot(&f);
        let count = points(&f);
        f.event(PointerPhase::Down, (left + width * 0.5, top + height * 0.2));
        assert_eq!(points(&f), count + 1, "added");
        assert_eq!(f.click.captured(), Some(f.curve));
        f.event(PointerPhase::Move, (left + width * 0.5, top + height * 0.9));
        let (x, y) = selected_point(&f);
        assert!(
            (x - 0.5).abs() < 0.02 && (y - 0.1).abs() < 0.02,
            "({x}, {y})"
        );
        // Far above and right: y clamps to 1, x stays inside its
        // neighbours (the endpoint at 1.0).
        f.event(
            PointerPhase::Move,
            (left + width * 10.0, top - height * 10.0),
        );
        let (x, y) = selected_point(&f);
        assert!(x < 1.0 && (y - 1.0).abs() < 1e-6, "({x}, {y})");
        assert_eq!(points(&f), count + 1, "the drag added nothing more");
    }

    #[test]
    fn a_curve_drag_ends_when_a_key_deletes_its_point_mid_drag() {
        let mut f = fixture();
        let (left, top, width, height) = plot(&f);
        f.event(PointerPhase::Down, (left + width * 0.5, top + height * 0.2));
        let count = points(&f);
        assert_eq!(f.click.captured(), Some(f.curve));
        // `Delete` removes the grabbed point; the selection moves to a
        // neighbour, which the drag must not now move.
        ok(widgets::handle_curve_editor_key(
            &mut f.tree,
            f.curve,
            widgets::CurveEditorKey::Delete,
            false,
        ));
        assert_eq!(points(&f), count - 1, "deleted");
        let before = selected_point(&f);
        let moved = f.event(PointerPhase::Move, (left + width * 0.5, top + height * 0.9));
        assert_eq!(moved, PointerOutcome::Cancelled(f.curve));
        assert_eq!(f.click.captured(), None, "the drag ended");
        assert_eq!(selected_point(&f), before, "the neighbour did not move");
    }

    #[test]
    fn a_down_on_the_preview_or_a_button_captures_nothing() {
        let mut f = fixture();
        f.event(PointerPhase::Down, centre(&f.tree, f.button));
        assert_eq!(f.click.captured(), None);
        assert!(f.click.is_active(), "armed, not captured");
        f.event(PointerPhase::Up, centre(&f.tree, f.button));
        let Some(b) = f.tree.bounds(f.picker) else {
            unreachable!()
        };
        // The preview: inside the picker, on neither part.
        #[allow(clippy::cast_precision_loss)]
        let preview = (0..40)
            .flat_map(|i| (0..40).map(move |j| (i, j)))
            .map(|(i, j)| {
                (
                    b.x as f32 + (i as f32 + 0.5) * b.width as f32 / 40.0,
                    b.y as f32 + (j as f32 + 0.5) * b.height as f32 / 40.0,
                )
            })
            .find(|&p| {
                super::target_at(&f.tree, p).map(|(id, _)| id) == Some(f.picker)
                    && widgets::color_picker_part_at(&f.tree, f.picker, p).is_none()
            });
        let Some(preview) = preview else {
            unreachable!("the picker has a preview")
        };
        let before = hsv(&f);
        f.event(PointerPhase::Down, preview);
        assert_eq!(f.click.captured(), None);
        f.event(
            PointerPhase::Move,
            at(&f.tree, picker_parts(&f).0, 0.1, 0.1),
        );
        assert_eq!(hsv(&f), before);
    }

    #[test]
    fn a_captured_widget_disabled_mid_drag_is_released_unchanged() {
        let mut f = fixture();
        f.event(PointerPhase::Down, at(&f.tree, f.slider, 0.25, 0.5));
        let before = f.number(f.slider);
        ok(widgets::set_slider_disabled(&mut f.tree, f.slider, true));
        assert_eq!(
            f.event(PointerPhase::Move, at(&f.tree, f.slider, 0.75, 0.5)),
            PointerOutcome::Cancelled(f.slider)
        );
        assert!((f.number(f.slider) - before).abs() < f64::EPSILON);
        assert_eq!(f.click.captured(), None);
        ok(widgets::set_slider_disabled(&mut f.tree, f.slider, false));
        assert_eq!(
            f.event(PointerPhase::Move, at(&f.tree, f.slider, 0.75, 0.5)),
            PointerOutcome::Ignored,
            "re-enabling does not resume the drag"
        );
        assert!((f.number(f.slider) - before).abs() < f64::EPSILON);
    }

    #[test]
    fn a_move_does_not_touch_focus_or_the_focus_ring() {
        let mut f = fixture();
        f.event(PointerPhase::Down, at(&f.tree, f.slider, 0.25, 0.5));
        ok(f.focus
            .focus_with(&mut f.tree, f.checkbox, FocusOrigin::Keyboard));
        assert!(f.focus.focus_visible());
        f.event(PointerPhase::Move, at(&f.tree, f.slider, 0.75, 0.5));
        assert!(f.number(f.slider) > 70.0, "the drag still ran");
        assert_eq!(f.focus.focused(), Some(f.checkbox));
        assert!(
            f.focus.focus_visible(),
            "a move is not a pointer modality switch"
        );
    }

    #[test]
    fn a_new_down_replaces_a_lost_capture() {
        let mut f = fixture();
        f.event(PointerPhase::Down, at(&f.tree, f.slider, 0.25, 0.5));
        let slider = f.number(f.slider);
        // The Up was lost (released outside the window).
        f.event(PointerPhase::Down, at(&f.tree, f.scrollbar, 0.1, 0.5));
        assert_eq!(f.click.captured(), Some(f.scrollbar));
        f.event(PointerPhase::Move, at(&f.tree, f.scrollbar, 0.9, 0.5));
        assert!((f.number(f.slider) - slider).abs() < f64::EPSILON);
        assert!(f.number(f.scrollbar) > 80.0);
        f.event(PointerPhase::Down, centre(&f.tree, f.button));
        assert_eq!(f.click.captured(), None, "a button's Down drops it too");
    }

    #[test]
    fn reset_and_release_capture_clear_the_capture() {
        let mut f = fixture();
        f.event(PointerPhase::Down, at(&f.tree, f.slider, 0.25, 0.5));
        f.click.reset();
        assert!(!f.click.is_active());
        let far = at(&f.tree, f.slider, 0.9, 0.5);
        assert_eq!(f.event(PointerPhase::Move, far), PointerOutcome::Ignored);
        assert_eq!(f.event(PointerPhase::Up, far), PointerOutcome::Ignored);
        f.event(PointerPhase::Down, at(&f.tree, f.slider, 0.25, 0.5));
        f.click.release_capture();
        assert_eq!(f.click.captured(), None);
        assert_eq!(f.event(PointerPhase::Move, far), PointerOutcome::Ignored);
        assert!(f.number(f.slider) < 30.0);
    }

    // -- text (0.131.0) --

    fn content(f: &Fixture) -> String {
        ok(widgets::text_field_state(&f.tree, f.text_field))
            .content
            .clone()
    }

    fn type_text(f: &mut Fixture, text: &str, modifiers: Modifiers) -> KeyOutcome {
        ok(handle_widget_text(&mut f.tree, &f.focus, text, modifiers))
    }

    fn focused_field(initial: &str) -> Fixture {
        let mut f = fixture();
        f.event(PointerPhase::Down, centre(&f.tree, f.text_field));
        f.event(PointerPhase::Up, centre(&f.tree, f.text_field));
        if !initial.is_empty() {
            type_text(&mut f, initial, Modifiers::none());
        }
        f
    }

    #[test]
    fn typing_inserts_at_the_caret_preserving_case() {
        let mut f = focused_field("");
        let shift = Modifiers {
            shift: true,
            ..Modifiers::none()
        };
        assert_eq!(
            type_text(&mut f, "H", shift),
            KeyOutcome::Handled(PointerOutcome::Changed(f.text_field))
        );
        type_text(&mut f, "ello", Modifiers::none());
        f.key(NamedKey::ArrowLeft, false);
        f.key(NamedKey::ArrowLeft, false);
        type_text(&mut f, "X", shift);
        assert_eq!(content(&f), "HelXlo");
    }

    #[test]
    fn shift_arrows_extend_the_selection_and_typing_replaces_it() {
        let mut f = focused_field("abcd");
        f.key(NamedKey::ArrowLeft, true);
        f.key(NamedKey::ArrowLeft, true);
        assert_eq!(
            ok(widgets::text_field_state(&f.tree, f.text_field)).selected_text(),
            "cd"
        );
        type_text(&mut f, "Z", Modifiers::none());
        assert_eq!(content(&f), "abZ");
        f.key(NamedKey::Home, true);
        assert_eq!(
            ok(widgets::text_field_state(&f.tree, f.text_field)).selected_text(),
            "abZ"
        );
        f.key(NamedKey::End, false);
        assert_eq!(
            ok(widgets::text_field_state(&f.tree, f.text_field)).selected_text(),
            "",
            "an unshifted move collapses the selection"
        );
    }

    #[test]
    fn home_end_backspace_and_delete() {
        let mut f = focused_field("abc");
        assert_eq!(
            f.key(NamedKey::Home, false),
            KeyOutcome::Handled(PointerOutcome::Changed(f.text_field))
        );
        f.key(NamedKey::Delete, false);
        assert_eq!(content(&f), "bc");
        f.key(NamedKey::End, false);
        f.key(NamedKey::Backspace, false);
        assert_eq!(content(&f), "b");
        f.key(NamedKey::ArrowRight, false);
        f.key(NamedKey::Delete, false);
        assert_eq!(content(&f), "b", "delete at the end is a no-op");
    }

    #[test]
    fn control_characters_are_filtered() {
        let mut f = focused_field("x");
        for text in ["\r", "\t", "\u{8}", "\u{7f}", "\u{1b}", ""] {
            assert_eq!(
                type_text(&mut f, text, Modifiers::none()),
                KeyOutcome::Ignored,
                "{text:?}"
            );
            assert_eq!(content(&f), "x", "{text:?}");
        }
        type_text(&mut f, "a\tb\r", Modifiers::none());
        assert_eq!(content(&f), "xab");
    }

    #[test]
    fn ctrl_and_meta_text_is_refused_but_alt_and_altgr_type() {
        let mut f = focused_field("");
        let control = Modifiers {
            control: true,
            ..Modifiers::none()
        };
        let meta = Modifiers {
            meta: true,
            ..Modifiers::none()
        };
        let alt = Modifiers {
            alt: true,
            ..Modifiers::none()
        };
        let alt_gr = Modifiers {
            control: true,
            alt: true,
            ..Modifiers::none()
        };
        let meta_alt = Modifiers {
            meta: true,
            alt: true,
            ..Modifiers::none()
        };
        for refused in [control, meta, meta_alt] {
            assert_eq!(type_text(&mut f, "s", refused), KeyOutcome::Ignored);
        }
        assert_eq!(content(&f), "");
        type_text(&mut f, "\u{e5}", alt);
        type_text(&mut f, "@", alt_gr);
        assert_eq!(content(&f), "\u{e5}@");
    }

    #[test]
    fn text_goes_only_to_a_focused_text_field() {
        let mut f = fixture();
        assert_eq!(
            type_text(&mut f, "a", Modifiers::none()),
            KeyOutcome::Ignored
        );
        ok(f.focus.focus(&mut f.tree, f.slider));
        assert_eq!(
            type_text(&mut f, "a", Modifiers::none()),
            KeyOutcome::Ignored
        );
        assert_eq!(content(&f), "");
    }

    #[test]
    fn a_disabled_field_refuses() {
        let mut f = focused_field("ab");
        ok(widgets::set_text_field_disabled(
            &mut f.tree,
            f.text_field,
            true,
        ));
        assert!(handle_widget_text(&mut f.tree, &f.focus, "c", Modifiers::none()).is_err());
        assert!(
            handle_widget_key(
                &mut f.tree,
                &mut f.focus,
                NamedKey::Backspace,
                Modifiers::none()
            )
            .is_err()
                || f.focus.focused() != Some(f.text_field)
        );
        assert_eq!(content(&f), "ab");
    }

    #[test]
    fn the_accessibility_value_follows_edits() {
        let mut f = focused_field("abc");
        let value = |f: &Fixture| {
            f.tree
                .accessibility(f.text_field)
                .and_then(|node| node.value().map(str::to_owned))
        };
        assert_eq!(value(&f).as_deref(), Some("abc"));
        f.key(NamedKey::Backspace, false);
        assert_eq!(value(&f).as_deref(), Some("ab"));
        type_text(&mut f, "Z", Modifiers::none());
        assert_eq!(value(&f).as_deref(), Some("abZ"));
    }
}
