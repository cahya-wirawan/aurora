//! Routing an assistive technology's `accesskit::ActionRequest` to the
//! widget it names (0.128.0).
//!
//! Every widget in this crate already *declares* what a screen reader may
//! ask of it — `Action::Focus`, `Click`, `SetValue`, `Increment`, ... on
//! its own `accesskit::Node` — but until this module nothing acted on a
//! request: `aurora-app` logged it and dropped it. [`handle_action`] is
//! the one headless entry point that does, so the whole mapping is
//! testable without a window, a platform adapter or a screen reader.
//!
//! # The gate, in order
//!
//! 1. `target_tree` must be [`ACCESSIBILITY_TREE_ID`], the id every
//!    [`WidgetTree::accessibility_update`] is published under, else
//!    [`ActionRejection::WrongTree`].
//! 2. `target_node` must be a live widget, else
//!    [`ActionRejection::UnknownTarget`] — which is also what a request
//!    racing a removal (a closed menu, a collapsed tree row) gets.
//! 3. **The widget's current node must declare the action**, else
//!    [`ActionRejection::Undeclared`]. This is the security gate: a
//!    platform adapter forwards whatever an assistive technology (or any
//!    other process speaking the platform's accessibility API) sends, and
//!    nothing here may do what the node never offered. Every widget's
//!    declared set is *state-dependent* (only the selected tab is
//!    focusable, an open dropdown offers `Collapse` and not `Expand`, a
//!    disabled widget offers nothing), so the check reads the node as it
//!    is now, not as it was when the platform last saw it.
//! 4. A disabled node *or* payload is [`ActionRejection::Disabled`] —
//!    belt and braces, since a disabled widget's node declares nothing and
//!    step 3 already refuses it.
//! 5. The request's `data` must have the shape the action needs:
//!    [`ActionRejection::DataMismatch`] otherwise, and
//!    [`ActionRejection::NonFiniteValue`] for a `NaN`/infinite number.
//! 6. Dispatch, through the same public mutator a pointer or key already
//!    uses (so every change marks its own damage and rebuilds its own
//!    node).
//! 7. [`FocusManager::validate`], always — a dispatch can remove widgets
//!    (a menu activation, a tree collapse), and focus must never be left
//!    naming a dead id.
//!
//! A declared action with no route is [`ActionRejection::Unsupported`],
//! and a standing test asserts that is unreachable for every widget
//! kind this crate builds: a widget declaring an action nothing handles
//! would be announcing a lie.
//!
//! # What is routed
//!
//! | Widget | Action | Route |
//! |---|---|---|
//! | anything declaring it | `Focus` | [`FocusManager::focus`] |
//! | `Button`, `ColorSwatch`, `TreeItem` | `Click` | none — [`ActionOutcome::Activated`]; what activation *means* is the owner's (the app's) |
//! | `Checkbox` | `Click` | [`widgets::toggle_checkbox`] |
//! | `Slider`, `Scrollbar` | `SetValue` (number) | [`widgets::set_slider_value`] / [`widgets::set_scrollbar_value`] (clamped) |
//! | `Slider`, `Scrollbar` | `Increment`/`Decrement` | the value ± the node's `numeric_value_step`, else [`DEFAULT_STEP_FRACTION`] of the range |
//! | `TextField` | `SetValue` (string) | replaces the whole text, one undo step; refuses control characters, text over [`MAX_SET_VALUE_TEXT_BYTES`], and any request while an IME composition is active |
//! | `TreeItem` | `Expand`/`Collapse` | [`widgets::set_tree_item_expanded`] |
//! | `Dropdown` | `Click` | [`widgets::toggle_dropdown`] |
//! | `Dropdown` | `Expand`/`Collapse` | [`widgets::set_dropdown_open`] (closing never commits) |
//! | `Tab` | `Click` | [`widgets::select_tab`] |
//! | menu item (`ListRow` under a `Menu`) | `Click` | [`widgets::activate_menu_item`] |
//! | colour picker channel slider | `SetValue` / `Increment` / `Decrement` | [`widgets::set_color_picker_hsv`] / [`widgets::handle_color_picker_key`] |
//! | curve editor point | `SetValue` / `Increment` / `Decrement` | [`widgets::set_curve_point_output`] / [`widgets::handle_curve_editor_key`] |
//!
//! Everything else — scrolling, `ScrollIntoView`, text selection,
//! `ReplaceSelectedText`, tooltips, context menus, custom actions,
//! `Blur`, choosing a dropdown option by its row — is **not declared by
//! any widget**, so step 3 refuses it. Adding one is a widget change
//! (declare it) *and* a route here, never one without the other.
//!
//! # Focus
//!
//! Only `Focus` moves focus deliberately. Two roving-focus widgets keep it
//! consistent with their selection: a tab `Click`, or a curve-point
//! `SetValue`/`Increment`/`Decrement`, that moves the selection while
//! focus sat on the old selected item moves focus to the new one (the
//! old one no longer declares `Focus`). A `TreeItem` collapse that
//! removes the focused descendant focuses the collapsed row.
//!
//! # What this does not prove
//!
//! The tests drive [`handle_action`] with hand-built requests. Whether a
//! real platform adapter delivers these requests the way they are built
//! here — `VoiceOver`'s `setAccessibilityValue:` sends a *string* when it
//! has one (`accesskit_macos` 0.26.3 `node.rs:556-578`), which a slider
//! refuses as `DataMismatch`; UIA's `RangeValue.SetValue` and AT-SPI's
//! `Value` send a number — has not been checked with a screen reader on
//! any platform.

use accesskit::{Action, ActionData, ActionRequest, Toggled, TreeId};

use crate::error::WidgetError;
use crate::input::{FocusManager, FocusOrigin};
use crate::tree::{ACCESSIBILITY_TREE_ID, WidgetId, WidgetTree};
use crate::widgets::{
    self, ColorPickerKey, ColorPickerOutcome, ColorPickerPart, ColorPickerPartRole, CurveEditorKey,
    DropdownOutcome, Hsv, MenuOutcome, TabBarOutcome, WidgetKind,
};

/// The fraction of a slider's or scrollbar's range one `Increment` or
/// `Decrement` moves it by when its node declares no
/// `numeric_value_step` (neither [`WidgetKind::Slider`] nor
/// [`WidgetKind::Scrollbar`] does today). One percent: fine enough to
/// reach any value a person would pick by ear, and the step a slider with
/// no declared one most commonly gets. Provisional — a design-owner
/// question, not a settled token.
pub const DEFAULT_STEP_FRACTION: f64 = 0.01;

/// What a routed action did.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum ActionOutcome {
    /// Focus moved to (or already was on) this widget.
    Focused(WidgetId),
    /// A `Click` on a widget whose activation is its owner's to define —
    /// a button, a colour swatch, a tree row. Nothing in the tree changed.
    Activated(WidgetId),
    /// A checkbox toggled to `state`.
    Toggled { id: WidgetId, state: Toggled },
    /// A slider, scrollbar, colour-picker channel or curve point now holds
    /// `value` (after clamping), in the units its own node announces.
    ValueChanged { id: WidgetId, value: f64 },
    /// A text field's whole text was replaced.
    TextChanged(WidgetId),
    /// A tree row's expanded state changed.
    ExpandedChanged { id: WidgetId, expanded: bool },
    /// A dropdown opened, closed or committed.
    Dropdown {
        id: WidgetId,
        outcome: DropdownOutcome,
    },
    /// Tab `index` of `bar` is now selected.
    TabSelected { bar: WidgetId, index: usize },
    /// Item `index` of `menu` was activated (an index into the caller's
    /// own items, separators included); the menu's subtree is gone.
    MenuActivated { menu: WidgetId, index: usize },
    /// A colour picker's colour changed.
    ColorChanged {
        picker: WidgetId,
        hsv: Hsv,
        color: aurora_theme::Color,
    },
    /// A curve editor's curve changed (its selection may have moved
    /// too). A request that moved only the selection is `Unchanged`.
    CurveChanged { editor: WidgetId },
    /// The request was valid and routed, and changed nothing (a slider
    /// already at its maximum, the selected tab clicked again).
    Unchanged(WidgetId),
}

/// Why a request was refused. Nothing in the tree or the focus changed —
/// with one narrow exception: a curve-point `Increment`/`Decrement`
/// selects the point before stepping it, so a mutator-level
/// [`ActionRejection::Widget`] error from the step (unreachable today)
/// would leave that selection moved.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum ActionRejection {
    /// `target_tree` is not [`ACCESSIBILITY_TREE_ID`].
    #[error("action addressed to unknown accessibility tree {0:?}")]
    WrongTree(TreeId),
    /// No live widget has this id.
    #[error("action addressed to widget {0:?}, which does not exist")]
    UnknownTarget(WidgetId),
    /// The widget's current node does not declare `action`.
    #[error("widget {id:?} does not declare {action:?}")]
    Undeclared { id: WidgetId, action: Action },
    /// The widget is disabled.
    #[error("widget {0:?} is disabled")]
    Disabled(WidgetId),
    /// The request's `data` is not what `action` needs on this widget.
    #[error("{action:?} on widget {id:?} carried the wrong data")]
    DataMismatch { id: WidgetId, action: Action },
    /// A `SetValue` number was `NaN` or infinite.
    #[error("non-finite value for widget {0:?}")]
    NonFiniteValue(WidgetId),
    /// A text field's `SetValue` arrived while an IME composition is in
    /// progress there; nothing changed, composition included.
    #[error("text field {0:?} has an IME composition in progress")]
    CompositionActive(WidgetId),
    /// The widget declares `action` but nothing routes it — a bug in this
    /// module, which a standing test keeps unreachable.
    #[error("widget {id:?} declares {action:?} but nothing routes it")]
    Unsupported { id: WidgetId, action: Action },
    /// The mutator itself refused.
    #[error(transparent)]
    Widget(WidgetError),
}

impl From<WidgetError> for ActionRejection {
    fn from(err: WidgetError) -> Self {
        match err {
            WidgetError::WidgetDisabled(id) => Self::Disabled(id),
            other => Self::Widget(other),
        }
    }
}

/// Every `accesskit::Action`, in [`action_ordinal`] order. Backed by an
/// exhaustive `match`, so a new `accesskit` action fails to compile here
/// rather than going silently unconsidered by the standing tests.
pub const ALL_ACTIONS: [Action; 22] = [
    Action::Click,
    Action::Focus,
    Action::Blur,
    Action::Collapse,
    Action::Expand,
    Action::CustomAction,
    Action::Decrement,
    Action::Increment,
    Action::HideTooltip,
    Action::ShowTooltip,
    Action::ReplaceSelectedText,
    Action::ScrollDown,
    Action::ScrollLeft,
    Action::ScrollRight,
    Action::ScrollUp,
    Action::ScrollIntoView,
    Action::ScrollToPoint,
    Action::SetScrollOffset,
    Action::SetTextSelection,
    Action::SetSequentialFocusNavigationStartingPoint,
    Action::SetValue,
    Action::ShowContextMenu,
];

/// `action`'s position in [`ALL_ACTIONS`]. Exhaustive on purpose.
#[must_use]
pub const fn action_ordinal(action: Action) -> usize {
    match action {
        Action::Click => 0,
        Action::Focus => 1,
        Action::Blur => 2,
        Action::Collapse => 3,
        Action::Expand => 4,
        Action::CustomAction => 5,
        Action::Decrement => 6,
        Action::Increment => 7,
        Action::HideTooltip => 8,
        Action::ShowTooltip => 9,
        Action::ReplaceSelectedText => 10,
        Action::ScrollDown => 11,
        Action::ScrollLeft => 12,
        Action::ScrollRight => 13,
        Action::ScrollUp => 14,
        Action::ScrollIntoView => 15,
        Action::ScrollToPoint => 16,
        Action::SetScrollOffset => 17,
        Action::SetTextSelection => 18,
        Action::SetSequentialFocusNavigationStartingPoint => 19,
        Action::SetValue => 20,
        Action::ShowContextMenu => 21,
    }
}

/// Routes one `request` — see this module's own doc comment for the gate
/// and the mapping.
///
/// # Errors
///
/// An [`ActionRejection`] for any request the gate refuses, or that the
/// widget's own mutator refuses. A refused request changes nothing.
pub fn handle_action(
    tree: &mut WidgetTree<WidgetKind>,
    focus: &mut FocusManager,
    request: &ActionRequest,
) -> Result<ActionOutcome, ActionRejection> {
    if request.target_tree != ACCESSIBILITY_TREE_ID {
        return Err(ActionRejection::WrongTree(request.target_tree));
    }
    let id = request.target_node;
    let action = request.action;
    let node = tree
        .accessibility(id)
        .ok_or(ActionRejection::UnknownTarget(id))?;
    if !node.supports_action(action) {
        return Err(ActionRejection::Undeclared { id, action });
    }
    let node_disabled = node.is_disabled();
    let payload = tree.payload(id).ok_or(ActionRejection::UnknownTarget(id))?;
    if node_disabled || payload_disabled(payload) {
        return Err(ActionRejection::Disabled(id));
    }
    let value = checked_data(payload, id, action, request.data.as_ref())?;
    let result = dispatch(tree, focus, id, action, value);
    focus.validate(tree);
    result
}

/// The payload of a request, once its shape is known to fit.
#[derive(Clone, Copy)]
enum Value<'a> {
    None,
    Number(f64),
    Text(&'a str),
}

/// Whether `kind` takes a string (`true`) or a number (`false`) for
/// `SetValue`.
fn takes_text(kind: &WidgetKind) -> bool {
    matches!(kind, WidgetKind::TextField(_))
}

fn checked_data<'a>(
    kind: &WidgetKind,
    id: WidgetId,
    action: Action,
    data: Option<&'a ActionData>,
) -> Result<Value<'a>, ActionRejection> {
    let mismatch = ActionRejection::DataMismatch { id, action };
    if action != Action::SetValue {
        return match data {
            None => Ok(Value::None),
            Some(_) => Err(mismatch),
        };
    }
    match (data, takes_text(kind)) {
        (Some(ActionData::NumericValue(number)), false) => {
            if number.is_finite() {
                Ok(Value::Number(*number))
            } else {
                Err(ActionRejection::NonFiniteValue(id))
            }
        }
        (Some(ActionData::Value(text)), true) if acceptable_text(text) => Ok(Value::Text(text)),
        _ => Err(mismatch),
    }
}

/// The longest text, in bytes, an assistive technology's `SetValue` may
/// put into a text field: 64 KiB. `TextField` has no length policy of
/// its own (typing is bounded by how fast a person types; a `SetValue`
/// is not), and every field in this app is a short, single-line name or
/// query, so this is far above any real use and far below a payload
/// that could stall layout or shaping on the UI thread. Longer text is
/// refused as `DataMismatch`, not truncated — a silently shortened value
/// is a value the user never asked for.
pub const MAX_SET_VALUE_TEXT_BYTES: usize = 64 * 1024;

/// Whether `text` may become a text field's whole content through
/// `SetValue`: at most [`MAX_SET_VALUE_TEXT_BYTES`], and free of every
/// control character (`char::is_control`: C0, `DEL` and C1 — newline,
/// tab, `NUL`, `ESC` among them). `TextField` is single-line (its own
/// module doc) and the keyboard path never inserts a control character,
/// so an assistive technology cannot put one there either. Bidi
/// formatting characters such as `U+202E` are `Cf`, not `Cc`, and are
/// accepted: they are legitimate in right-to-left names, and typing (or
/// an IME commit) can already insert them.
fn acceptable_text(text: &str) -> bool {
    text.len() <= MAX_SET_VALUE_TEXT_BYTES && !text.chars().any(char::is_control)
}

/// Whether `kind`'s own state says it is disabled — the payload half of
/// step 4 (the node half is `Node::is_disabled`).
pub(crate) fn payload_disabled(kind: &WidgetKind) -> bool {
    match kind {
        WidgetKind::Button(state) => state.disabled,
        WidgetKind::Checkbox(state) => state.disabled,
        WidgetKind::Slider(state) => state.disabled,
        WidgetKind::Scrollbar(state) => state.disabled,
        WidgetKind::TextField(state) => state.disabled,
        WidgetKind::ColorSwatch(state) => state.disabled,
        WidgetKind::ListRow(state) => state.disabled,
        WidgetKind::TreeItem(state) => state.disabled,
        WidgetKind::Dropdown(state) => state.is_disabled(),
        WidgetKind::TabBar(state) => state.is_disabled(),
        WidgetKind::Tab(state) => state.is_disabled(),
        WidgetKind::ColorPicker(state) => state.disabled(),
        WidgetKind::ColorPickerPart(state) => state.is_disabled(),
        WidgetKind::CurveEditor(state) => state.disabled(),
        WidgetKind::CurveEditorPoint(state) => state.is_disabled(),
        WidgetKind::Container
        | WidgetKind::CommandPalette(_)
        | WidgetKind::Panel
        | WidgetKind::Dialog
        | WidgetKind::DropdownList
        | WidgetKind::Tooltip
        | WidgetKind::Menu(_)
        | WidgetKind::MenuSeparator => false,
    }
}

/// Which widget family a request lands on — read once, so `dispatch` can
/// take `tree` mutably afterwards.
#[derive(Clone, Copy)]
enum Target {
    Owned,
    Checkbox,
    Slider { value: f64, min: f64, max: f64 },
    Scrollbar { value: f64, min: f64, max: f64 },
    TextField,
    TreeItem,
    Dropdown,
    Tab,
    MenuItem,
    ColorPickerPart(ColorPickerPartRole),
    CurveEditorPoint,
    Other,
}

fn target_of(kind: &WidgetKind) -> Target {
    match kind {
        WidgetKind::Button(_) | WidgetKind::ColorSwatch(_) => Target::Owned,
        WidgetKind::Checkbox(_) => Target::Checkbox,
        WidgetKind::Slider(state) => Target::Slider {
            value: state.value,
            min: state.min,
            max: state.max,
        },
        WidgetKind::Scrollbar(state) => Target::Scrollbar {
            value: state.value,
            min: state.min,
            max: state.max,
        },
        WidgetKind::TextField(_) => Target::TextField,
        WidgetKind::TreeItem(_) => Target::TreeItem,
        WidgetKind::Dropdown(_) => Target::Dropdown,
        WidgetKind::Tab(_) => Target::Tab,
        WidgetKind::ListRow(_) => Target::MenuItem,
        WidgetKind::ColorPickerPart(state) => Target::ColorPickerPart(state.role()),
        WidgetKind::CurveEditorPoint(_) => Target::CurveEditorPoint,
        _ => Target::Other,
    }
}

fn dispatch(
    tree: &mut WidgetTree<WidgetKind>,
    focus: &mut FocusManager,
    id: WidgetId,
    action: Action,
    value: Value<'_>,
) -> Result<ActionOutcome, ActionRejection> {
    let unsupported = ActionRejection::Unsupported { id, action };
    if action == Action::Focus {
        focus.focus_with(tree, id, FocusOrigin::Accessibility)?;
        return Ok(ActionOutcome::Focused(id));
    }
    let target = tree
        .payload(id)
        .map(target_of)
        .ok_or(ActionRejection::UnknownTarget(id))?;
    // `SliderState`/`ScrollbarRange` have public `min`/`max` fields, so
    // construction does not forbid a `NaN` or inverted range, and
    // `f64::clamp` (here and in both setters) panics on one. Refuse it
    // before any arithmetic, the same error the scrollbar's own setter
    // already uses.
    if let Target::Slider { min, max, .. } | Target::Scrollbar { min, max, .. } = target
        && !(min.is_finite() && max.is_finite() && min <= max)
    {
        return Err(WidgetError::InvalidRange { min, max }.into());
    }
    match (target, action) {
        (Target::Owned | Target::TreeItem, Action::Click) => Ok(ActionOutcome::Activated(id)),
        (Target::Checkbox, Action::Click) => {
            let state = widgets::toggle_checkbox(tree, id)?;
            Ok(ActionOutcome::Toggled { id, state })
        }
        (
            Target::Slider {
                value: old,
                min,
                max,
            },
            _,
        ) => {
            let Some(next) = numeric_target(tree, id, action, &value, old, min, max) else {
                return Err(unsupported);
            };
            let stored = widgets::set_slider_value(tree, id, next)?;
            Ok(value_outcome(id, old, stored))
        }
        (
            Target::Scrollbar {
                value: old,
                min,
                max,
            },
            _,
        ) => {
            let Some(next) = numeric_target(tree, id, action, &value, old, min, max) else {
                return Err(unsupported);
            };
            let stored = widgets::set_scrollbar_value(tree, id, next)?;
            Ok(value_outcome(id, old, stored))
        }
        (Target::TextField, Action::SetValue) => {
            let Value::Text(text) = value else {
                return Err(unsupported);
            };
            set_text_field_value(tree, id, text)
        }
        (Target::TreeItem, Action::Expand | Action::Collapse) => {
            let expanded = action == Action::Expand;
            let focused = focus.focused();
            widgets::set_tree_item_expanded(tree, id, expanded)?;
            if focused.is_some_and(|f| !tree.contains(f)) {
                // The focused row was inside what just collapsed: the
                // collapsed row is the nearest surviving place for it.
                focus.focus(tree, id)?;
            }
            Ok(ActionOutcome::ExpandedChanged { id, expanded })
        }
        (Target::Dropdown, Action::Click) => {
            let outcome = widgets::toggle_dropdown(tree, id)?;
            Ok(ActionOutcome::Dropdown { id, outcome })
        }
        (Target::Dropdown, Action::Expand | Action::Collapse) => {
            let outcome = widgets::set_dropdown_open(tree, id, action == Action::Expand)?;
            Ok(ActionOutcome::Dropdown { id, outcome })
        }
        (Target::Tab, Action::Click) => click_tab(tree, focus, id),
        (Target::MenuItem, Action::Click) => {
            let Some(menu) = tree
                .parent(id)
                .filter(|&parent| matches!(tree.payload(parent), Some(WidgetKind::Menu(_))))
            else {
                return Err(unsupported);
            };
            Ok(match widgets::activate_menu_item(tree, menu, id)? {
                MenuOutcome::Activated(index) => ActionOutcome::MenuActivated { menu, index },
                _ => ActionOutcome::Unchanged(id),
            })
        }
        (Target::ColorPickerPart(role), _) => color_picker_action(tree, id, role, action, &value),
        (Target::CurveEditorPoint, _) => curve_point_action(tree, focus, id, action, &value),
        _ => Err(unsupported),
    }
}

/// A text field's `SetValue`: replaces the whole text as one undo step.
/// The text itself was already vetted by [`acceptable_text`].
///
/// An IME composition in progress is refused, not cleared: the platform
/// input method still holds it, and dropping the widget-side copy alone
/// would desynchronise the two (the next `Ime::Preedit`/`Commit` would
/// land on text the user never saw). The AT can retry once the
/// composition ends. Equal text is `Unchanged`, with no undo step.
fn set_text_field_value(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    text: &str,
) -> Result<ActionOutcome, ActionRejection> {
    if widgets::text_field_state(tree, id)?.composition.is_some() {
        return Err(ActionRejection::CompositionActive(id));
    }
    let changed = widgets::with_text_field_mut(tree, id, |state| {
        if state.content == text {
            return false;
        }
        state.select_all();
        state.insert_str(text);
        true
    })?;
    Ok(if changed {
        ActionOutcome::TextChanged(id)
    } else {
        ActionOutcome::Unchanged(id)
    })
}

fn value_outcome(id: WidgetId, old: f64, stored: f64) -> ActionOutcome {
    if stored.to_bits() == old.to_bits() {
        ActionOutcome::Unchanged(id)
    } else {
        ActionOutcome::ValueChanged { id, value: stored }
    }
}

/// The value a `SetValue`/`Increment`/`Decrement` asks a slider or
/// scrollbar for, before its own setter clamps it. `None` for any other
/// action.
fn numeric_target(
    tree: &WidgetTree<WidgetKind>,
    id: WidgetId,
    action: Action,
    value: &Value<'_>,
    current: f64,
    min: f64,
    max: f64,
) -> Option<f64> {
    match (action, value) {
        (Action::SetValue, Value::Number(number)) => Some(*number),
        (Action::Increment | Action::Decrement, Value::None) => {
            let step = tree
                .accessibility(id)
                .and_then(accesskit::Node::numeric_value_step)
                .filter(|step| step.is_finite() && *step > 0.0)
                .unwrap_or((max - min) * DEFAULT_STEP_FRACTION);
            let signed = if action == Action::Increment {
                step
            } else {
                -step
            };
            // The setter clamps; `min`/`max` bound the sum here only so
            // a pathological range cannot overflow to infinity. `dispatch`
            // has already refused a non-finite or inverted range, so this
            // `clamp` cannot panic.
            Some((current + signed).clamp(min, max))
        }
        _ => None,
    }
}

/// A tab `Click`: selects it, and moves focus with the selection if focus
/// sat on the previously selected tab (roving focus — the old tab no
/// longer declares `Focus`).
fn click_tab(
    tree: &mut WidgetTree<WidgetKind>,
    focus: &mut FocusManager,
    id: WidgetId,
) -> Result<ActionOutcome, ActionRejection> {
    let bar = tree.parent(id).ok_or(ActionRejection::UnknownTarget(id))?;
    let state = widgets::tab_bar_state(tree, bar)?;
    let index = state
        .index_of(id)
        .ok_or(ActionRejection::UnknownTarget(id))?;
    let focus_was_on_old = focus.focused().is_some() && focus.focused() == state.selected_tab();
    match widgets::select_tab(tree, bar, index)? {
        TabBarOutcome::Selected(index) => {
            if focus_was_on_old && let Some(new) = widgets::tab_bar_state(tree, bar)?.selected_tab()
            {
                focus.focus(tree, new)?;
            }
            Ok(ActionOutcome::TabSelected { bar, index })
        }
        TabBarOutcome::Ignored => Ok(ActionOutcome::Unchanged(id)),
    }
}

/// The picker a channel slider belongs to — its nearest
/// [`WidgetKind::ColorPicker`] ancestor that also tracks it.
pub(crate) fn owning_picker(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> Option<WidgetId> {
    let mut current = tree.parent(id);
    while let Some(candidate) = current {
        if matches!(tree.payload(candidate), Some(WidgetKind::ColorPicker(_))) {
            return widgets::color_picker_part_of(tree, candidate, id).map(|_| candidate);
        }
        current = tree.parent(candidate);
    }
    None
}

fn color_picker_action(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    role: ColorPickerPartRole,
    action: Action,
    value: &Value<'_>,
) -> Result<ActionOutcome, ActionRejection> {
    let unsupported = ActionRejection::Unsupported { id, action };
    let picker = owning_picker(tree, id).ok_or(ActionRejection::UnknownTarget(id))?;
    let outcome = match (action, value) {
        (Action::SetValue, Value::Number(number)) => {
            // The number is in the unit the slider's own node announces:
            // percent (0..=100) for saturation and value, degrees
            // (0..=360) for hue — read back from that node's own maximum,
            // so the two can never disagree.
            let max = tree
                .accessibility(id)
                .and_then(accesskit::Node::max_numeric_value)
                .filter(|max| max.is_finite() && *max > 0.0)
                .ok_or(unsupported)?;
            let mut hsv = widgets::color_picker_state(tree, picker)?.hsv();
            let clamped = number.clamp(0.0, max);
            match role {
                ColorPickerPartRole::Saturation => hsv.saturation = (clamped / max) as f32,
                ColorPickerPartRole::Value => hsv.value = (clamped / max) as f32,
                ColorPickerPartRole::Hue => hsv.hue = clamped as f32,
                ColorPickerPartRole::Area => {
                    return Err(ActionRejection::Unsupported { id, action });
                }
            }
            widgets::set_color_picker_hsv(tree, picker, hsv)?
        }
        (Action::Increment | Action::Decrement, Value::None) => {
            let up = action == Action::Increment;
            let (part, key) = match role {
                ColorPickerPartRole::Saturation => (
                    ColorPickerPart::SaturationValue,
                    if up {
                        ColorPickerKey::Right
                    } else {
                        ColorPickerKey::Left
                    },
                ),
                ColorPickerPartRole::Value => (
                    ColorPickerPart::SaturationValue,
                    if up {
                        ColorPickerKey::Up
                    } else {
                        ColorPickerKey::Down
                    },
                ),
                ColorPickerPartRole::Hue => (
                    ColorPickerPart::Hue,
                    if up {
                        ColorPickerKey::Right
                    } else {
                        ColorPickerKey::Left
                    },
                ),
                ColorPickerPartRole::Area => {
                    return Err(ActionRejection::Unsupported { id, action });
                }
            };
            widgets::handle_color_picker_key(tree, picker, part, key, false)?
        }
        _ => return Err(unsupported),
    };
    Ok(match outcome {
        ColorPickerOutcome::Changed { hsv, color } => {
            ActionOutcome::ColorChanged { picker, hsv, color }
        }
        ColorPickerOutcome::Ignored => ActionOutcome::Unchanged(id),
    })
}

fn curve_point_action(
    tree: &mut WidgetTree<WidgetKind>,
    focus: &mut FocusManager,
    id: WidgetId,
    action: Action,
    value: &Value<'_>,
) -> Result<ActionOutcome, ActionRejection> {
    let unsupported = ActionRejection::Unsupported { id, action };
    let editor = widgets::curve_editor_of(tree, id).ok_or(ActionRejection::UnknownTarget(id))?;
    let state = widgets::curve_editor_state(tree, editor)?;
    let index = (0..state.curve().points().len())
        .find(|&index| state.point_id(index) == Some(id))
        .ok_or(ActionRejection::UnknownTarget(id))?;
    let old_target = state.focus_target();
    let old_points = state.curve().points().to_vec();
    let focus_was_on_old = focus.focused().is_some() && focus.focused() == old_target;
    match (action, value) {
        (Action::SetValue, Value::Number(number)) => {
            // In the unit the point's own node announces (0..=255),
            // read back from that node's own maximum.
            let max = tree
                .accessibility(id)
                .and_then(accesskit::Node::max_numeric_value)
                .filter(|max| max.is_finite() && *max > 0.0)
                .ok_or(unsupported)?;
            let output = (number.clamp(0.0, max) / max) as f32;
            widgets::set_curve_point_output(tree, editor, index, output)?;
        }
        (Action::Increment | Action::Decrement, Value::None) => {
            widgets::select_curve_point(tree, editor, index)?;
            let key = if action == Action::Increment {
                CurveEditorKey::Up
            } else {
                CurveEditorKey::Down
            };
            widgets::handle_curve_editor_key(tree, editor, key, false)?;
        }
        _ => return Err(unsupported),
    }
    let state = widgets::curve_editor_state(tree, editor)?;
    // `CurveChanged` means the *curve* changed — the owner's document
    // value. A request that only moved the selection (a point already at
    // the asked-for level) changed nothing the owner stores.
    let changed = state.curve().points() != old_points.as_slice();
    let new_target = state.focus_target();
    if focus_was_on_old
        && new_target != old_target
        && let Some(new) = new_target
    {
        focus.focus(tree, new)?;
    }
    Ok(if changed {
        ActionOutcome::CurveChanged { editor }
    } else {
        ActionOutcome::Unchanged(id)
    })
}

#[cfg(test)]
// Exact float equality is the claim under test (a set value stored
// exactly, a clamp landing exactly on a bound).
#[allow(
    clippy::float_cmp,
    clippy::too_many_lines,
    clippy::unnecessary_wraps,
    clippy::manual_let_else
)]
mod tests {
    use super::*;
    use crate::widgets::{
        CommandEntry, DialogAction, DialogHandle, MenuItem, ScrollbarRange, color_picker_state,
        curve_editor_state, dropdown_state, insert_button, insert_checkbox, insert_color_picker,
        insert_color_swatch, insert_command_palette, insert_curve_editor, insert_dialog,
        insert_dropdown, insert_scrollbar, insert_slider, insert_tab_bar, insert_text_field,
        insert_tree_item, insert_tree_view, menu_state, new_tree, open_menu, select_curve_point,
        select_tab, set_checkbox_disabled, set_color_picker_disabled, set_curve_editor_disabled,
        set_dropdown_open, set_slider_disabled, set_text_field_disabled, set_tree_item_disabled,
        set_tree_item_expanded, tab_bar_state, test_scales, text_field_state, with_text_field_mut,
    };
    use accesskit::{Node, Orientation, Role};
    use aurora_core::{CurvePoint, ToneCurve};
    use aurora_theme::Color;
    use taffy::Style;

    fn ok<T>(result: Result<T, WidgetError>) -> T {
        match result {
            Ok(value) => value,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    /// One of every widget kind this crate builds, laid out and with its
    /// damage cleared.
    struct Fixture {
        tree: WidgetTree<WidgetKind>,
        focus: FocusManager,
        root: WidgetId,
        button: WidgetId,
        checkbox: WidgetId,
        slider: WidgetId,
        scrollbar: WidgetId,
        text: WidgetId,
        swatch: WidgetId,
        group_row: WidgetId,
        child_row: WidgetId,
        leaf_row: WidgetId,
        panel: WidgetId,
        tooltip: WidgetId,
        dialog: DialogHandle,
        dropdown: WidgetId,
        open_dropdown: WidgetId,
        tab_bar: WidgetId,
        menu: WidgetId,
        picker: WidgetId,
        editor: WidgetId,
    }

    fn fixture() -> Fixture {
        let scales = test_scales();
        let (mut tree, root) = new_tree(Style::default());
        let button = ok(insert_button(&mut tree, root, &scales, "Apply"));
        let checkbox = ok(insert_checkbox(&mut tree, root, &scales, "Snap"));
        let slider = ok(insert_slider(
            &mut tree, root, &scales, "Size", 50.0, 0.0, 100.0,
        ));
        let scrollbar = ok(insert_scrollbar(
            &mut tree,
            root,
            &scales,
            Orientation::Vertical,
            Some("Scroll"),
            10.0,
            ScrollbarRange {
                min: 0.0,
                max: 200.0,
                page_size: 20.0,
            },
        ));
        let text = ok(insert_text_field(&mut tree, root, &scales, "Name", "Layer"));
        ok(insert_command_palette(
            &mut tree,
            root,
            vec![CommandEntry {
                id: "undo".to_owned(),
                title: "Undo".to_owned(),
                shortcut: None,
            }],
        ));
        let swatch = ok(insert_color_swatch(
            &mut tree,
            root,
            &scales,
            Color {
                r: 10,
                g: 20,
                b: 30,
            },
        ));
        let view = ok(insert_tree_view(&mut tree, root, Some("Layers")));
        let group_row = ok(insert_tree_item(&mut tree, view, &scales, "Group", true));
        let child_row = ok(insert_tree_item(
            &mut tree, group_row, &scales, "Child", false,
        ));
        let leaf_row = ok(insert_tree_item(&mut tree, view, &scales, "Leaf", false));
        let panel = ok(tree.insert(
            root,
            Style::default(),
            Node::new(Role::Pane),
            WidgetKind::Panel,
        ));
        let tooltip = ok(tree.insert(
            button,
            Style::default(),
            Node::new(Role::Tooltip),
            WidgetKind::Tooltip,
        ));
        let dialog = ok(insert_dialog(
            &mut tree,
            root,
            &scales,
            "Title",
            "Message",
            vec![DialogAction::new("ok", "OK")],
        ));
        let options = vec![
            "Normal".to_owned(),
            "Multiply".to_owned(),
            "Screen".to_owned(),
        ];
        let dropdown = ok(insert_dropdown(
            &mut tree,
            root,
            &scales,
            "Blend",
            options.clone(),
            Some(0),
        ));
        let open_dropdown = ok(insert_dropdown(
            &mut tree,
            root,
            &scales,
            "Mode",
            options,
            Some(1),
        ));
        ok(set_dropdown_open(&mut tree, open_dropdown, true));
        let tabs = vec!["A".to_owned(), "B".to_owned(), "C".to_owned()];
        let tab_bar = ok(insert_tab_bar(&mut tree, root, &scales, "Tabs", tabs, 0));
        let mut disabled = MenuItem::action("Paste");
        disabled.enabled = false;
        let menu = ok(open_menu(
            &mut tree,
            root,
            &scales,
            "Edit",
            (0.0, 0.0),
            120.0,
            vec![
                MenuItem::action("Cut"),
                MenuItem::separator(),
                MenuItem::action("Copy"),
                disabled,
            ],
        ));
        let picker = ok(insert_color_picker(
            &mut tree,
            root,
            &scales,
            "Foreground",
            Color { r: 255, g: 0, b: 0 },
            100.0,
        ));
        let curve = match ToneCurve::new(&[
            CurvePoint::new(0.0, 0.0),
            CurvePoint::new(0.5, 0.5),
            CurvePoint::new(1.0, 1.0),
        ]) {
            Ok(curve) => curve,
            Err(err) => unreachable!("{err:?}"),
        };
        let editor = ok(insert_curve_editor(
            &mut tree, root, &scales, "Curve", 100.0, curve,
        ));
        tree.compute_layout(800.0, 600.0);
        let _ = tree.take_damage();
        Fixture {
            tree,
            focus: FocusManager::new(),
            root,
            button,
            checkbox,
            slider,
            scrollbar,
            text,
            swatch,
            group_row,
            child_row,
            leaf_row,
            panel,
            tooltip,
            dialog,
            dropdown,
            open_dropdown,
            tab_bar,
            menu,
            picker,
            editor,
        }
    }

    fn request(id: WidgetId, action: Action, data: Option<ActionData>) -> ActionRequest {
        ActionRequest {
            action,
            target_tree: ACCESSIBILITY_TREE_ID,
            target_node: id,
            data,
        }
    }

    fn act(
        id: WidgetId,
        f: &mut Fixture,
        action: Action,
        data: Option<ActionData>,
    ) -> ActionOutcome {
        match handle_action(&mut f.tree, &mut f.focus, &request(id, action, data)) {
            Ok(outcome) => outcome,
            Err(err) => unreachable!("{action:?} on {id:?} was refused: {err:?}"),
        }
    }

    fn refuse(
        id: WidgetId,
        f: &mut Fixture,
        action: Action,
        data: Option<ActionData>,
    ) -> ActionRejection {
        let before_payload = f.tree.payload(id).cloned();
        let before_focus = f.focus.focused();
        let _ = f.tree.take_damage();
        match handle_action(&mut f.tree, &mut f.focus, &request(id, action, data)) {
            Ok(outcome) => unreachable!("{action:?} on {id:?} was routed: {outcome:?}"),
            Err(err) => {
                assert_eq!(
                    f.tree.payload(id).cloned(),
                    before_payload,
                    "payload changed"
                );
                assert_eq!(f.focus.focused(), before_focus, "focus changed");
                assert!(nothing_dirty(f), "a refusal marked a widget dirty");
                err
            }
        }
    }

    /// Whether no live widget is dirty — per-widget, so it is not blind
    /// to a widget whose laid-out bounds happen to be empty.
    fn nothing_dirty(f: &Fixture) -> bool {
        fixture_ids(f)
            .into_iter()
            .all(|id| f.tree.is_dirty(id) == Some(false))
    }

    fn number(value: f64) -> Option<ActionData> {
        Some(ActionData::NumericValue(value))
    }

    fn slider_value(f: &Fixture) -> f64 {
        match f.tree.payload(f.slider) {
            Some(WidgetKind::Slider(state)) => state.value,
            _ => unreachable!("slider"),
        }
    }

    fn scrollbar_value(f: &Fixture) -> f64 {
        match f.tree.payload(f.scrollbar) {
            Some(WidgetKind::Scrollbar(state)) => state.value,
            _ => unreachable!("scrollbar"),
        }
    }

    fn tab(f: &Fixture, index: usize) -> WidgetId {
        match ok(tab_bar_state(&f.tree, f.tab_bar)).tabs().get(index) {
            Some(&id) => id,
            None => unreachable!("tab {index}"),
        }
    }

    fn menu_item(f: &Fixture, index: usize) -> WidgetId {
        match ok(menu_state(&f.tree, f.menu)).item_ids().get(index) {
            Some(&id) => id,
            None => unreachable!("item {index}"),
        }
    }

    fn picker_part(f: &Fixture, role: ColorPickerPartRole) -> WidgetId {
        let found = f
            .tree
            .children(f.picker)
            .into_iter()
            .flatten()
            .copied()
            .chain(
                f.tree
                    .children(f.picker)
                    .into_iter()
                    .flatten()
                    .flat_map(|&c| f.tree.children(c).into_iter().flatten().copied()),
            );
        for id in found.collect::<Vec<_>>() {
            if let Some(WidgetKind::ColorPickerPart(state)) = f.tree.payload(id)
                && state.role() == role
            {
                return id;
            }
        }
        unreachable!("no {role:?} part")
    }

    fn curve_point(f: &Fixture, index: usize) -> WidgetId {
        match ok(curve_editor_state(&f.tree, f.editor)).point_id(index) {
            Some(id) => id,
            None => unreachable!("point {index}"),
        }
    }

    // ---- The gate --------------------------------------------------

    #[test]
    fn every_action_has_its_own_ordinal() {
        for (index, &action) in ALL_ACTIONS.iter().enumerate() {
            assert_eq!(action_ordinal(action), index, "{action:?}");
        }
    }

    #[test]
    fn an_unknown_or_removed_target_is_refused() {
        let mut f = fixture();
        let gone = ok(insert_button(&mut f.tree, f.root, &test_scales(), "Gone"));
        ok(f.tree.remove(gone));
        assert!(matches!(
            refuse(gone, &mut f, Action::Click, None),
            ActionRejection::UnknownTarget(id) if id == gone
        ));
        let bogus = accesskit::NodeId(u64::MAX);
        assert!(matches!(
            refuse(bogus, &mut f, Action::Focus, None),
            ActionRejection::UnknownTarget(_)
        ));
    }

    #[test]
    fn a_request_for_another_tree_is_refused() {
        let mut f = fixture();
        let _ = f.tree.take_damage();
        let mut wrong = request(f.checkbox, Action::Click, None);
        wrong.target_tree = TreeId(accesskit::Uuid::from_u128(7));
        let result = handle_action(&mut f.tree, &mut f.focus, &wrong);
        assert!(matches!(result, Err(ActionRejection::WrongTree(_))));
        assert!(nothing_dirty(&f));
    }

    #[test]
    fn an_undeclared_action_is_refused_whatever_the_widget() {
        let mut f = fixture();
        let unselected_tab = tab(&f, 1);
        let separator = menu_item(&f, 1);
        let option_row = match ok(dropdown_state(&f.tree, f.open_dropdown)).rows().first() {
            Some(&row) => row,
            None => unreachable!("an open dropdown has rows"),
        };
        let cases = [
            (f.slider, Action::Click, None),
            (f.button, Action::SetValue, number(1.0)),
            (f.panel, Action::Focus, None),
            (separator, Action::Focus, None),
            (separator, Action::Click, None),
            (f.tooltip, Action::Focus, None),
            (unselected_tab, Action::Focus, None),
            (f.leaf_row, Action::Expand, None),
            (f.leaf_row, Action::Collapse, None),
            (f.open_dropdown, Action::Expand, None),
            (f.dropdown, Action::Collapse, None),
            (option_row, Action::Click, None),
            (f.button, Action::Blur, None),
            (f.button, Action::ShowTooltip, None),
            (
                f.button,
                Action::CustomAction,
                Some(ActionData::CustomAction(1)),
            ),
            (
                f.text,
                Action::ReplaceSelectedText,
                Some(ActionData::Value("x".into())),
            ),
            (f.button, Action::ScrollIntoView, None),
            (f.dialog.root, Action::Click, None),
        ];
        for (id, action, data) in cases {
            let err = refuse(id, &mut f, action, data);
            assert!(
                matches!(err, ActionRejection::Undeclared { id: got, action: a } if got == id && a == action),
                "{action:?} on {id:?}: {err:?}"
            );
        }
    }

    #[test]
    fn a_collapsed_row_refuses_collapse_and_an_expanded_one_refuses_expand() {
        let mut f = fixture();
        let err = refuse(f.group_row, &mut f, Action::Expand, None);
        assert!(matches!(err, ActionRejection::Undeclared { .. }), "{err:?}");
        act(f.group_row, &mut f, Action::Collapse, None);
        let err = refuse(f.group_row, &mut f, Action::Collapse, None);
        assert!(matches!(err, ActionRejection::Undeclared { .. }), "{err:?}");
    }

    #[test]
    fn a_disabled_widget_is_refused() {
        let mut f = fixture();
        ok(set_checkbox_disabled(&mut f.tree, f.checkbox, true));
        let err = refuse(f.checkbox, &mut f, Action::Click, None);
        assert!(matches!(err, ActionRejection::Undeclared { .. }), "{err:?}");
        // A node that lies about a disabled payload is still refused.
        let mut lying = Node::new(Role::CheckBox);
        lying.add_action(Action::Click);
        ok(f.tree.set_accessibility(f.checkbox, lying));
        let err = refuse(f.checkbox, &mut f, Action::Click, None);
        assert!(
            matches!(err, ActionRejection::Disabled(id) if id == f.checkbox),
            "{err:?}"
        );
        // And for a widget whose `Click` calls no mutator that would
        // refuse it on its own (a button's activation is the owner's), so
        // only this module's own check stands between it and `Activated`.
        ok(crate::widgets::set_button_disabled(
            &mut f.tree,
            f.button,
            true,
        ));
        let mut lying = Node::new(Role::Button);
        lying.add_action(Action::Click);
        ok(f.tree.set_accessibility(f.button, lying));
        let err = refuse(f.button, &mut f, Action::Click, None);
        assert!(
            matches!(err, ActionRejection::Disabled(id) if id == f.button),
            "{err:?}"
        );
    }

    #[test]
    fn a_declared_action_nothing_routes_is_unsupported_and_changes_nothing() {
        let mut f = fixture();
        let mut node = Node::new(Role::GenericContainer);
        node.add_action(Action::Click);
        ok(f.tree.set_accessibility(f.panel, node));
        let err = refuse(f.panel, &mut f, Action::Click, None);
        assert!(
            matches!(err, ActionRejection::Unsupported { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn the_wrong_data_is_refused() {
        let mut f = fixture();
        let cases = [
            (f.slider, Action::SetValue, None),
            (
                f.slider,
                Action::SetValue,
                Some(ActionData::Value("50".into())),
            ),
            (
                f.slider,
                Action::SetValue,
                Some(ActionData::CustomAction(3)),
            ),
            (f.checkbox, Action::Click, number(1.0)),
            (f.slider, Action::Increment, number(1.0)),
            (f.text, Action::SetValue, number(1.0)),
        ];
        for (id, action, data) in cases {
            let err = refuse(id, &mut f, action, data);
            assert!(
                matches!(err, ActionRejection::DataMismatch { .. }),
                "{action:?} on {id:?}: {err:?}"
            );
        }
    }

    #[test]
    fn a_non_finite_value_is_refused_and_a_huge_one_clamps() {
        let mut f = fixture();
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let err = refuse(f.slider, &mut f, Action::SetValue, number(bad));
            assert!(matches!(err, ActionRejection::NonFiniteValue(_)), "{err:?}");
            assert_eq!(slider_value(&f), 50.0);
        }
        assert_eq!(
            act(f.slider, &mut f, Action::SetValue, number(1e308)),
            ActionOutcome::ValueChanged {
                id: f.slider,
                value: 100.0
            }
        );
    }

    // ---- Routes ----------------------------------------------------

    #[test]
    fn focus_moves_focus() {
        let mut f = fixture();
        assert_eq!(
            act(f.button, &mut f, Action::Focus, None),
            ActionOutcome::Focused(f.button)
        );
        assert_eq!(f.focus.focused(), Some(f.button));
    }

    /// An assistive technology's focus request is its own modality
    /// (`FocusOrigin::Accessibility`): it shows the focus ring even when
    /// the last real input was a pointer click, which had hidden it — a
    /// screen-magnifier user follows that ring.
    #[test]
    fn an_accessibility_focus_request_shows_the_focus_ring_after_a_click() {
        let mut f = fixture();
        f.focus
            .note_input(&mut f.tree, crate::input::FocusOrigin::Pointer);
        assert_eq!(
            act(f.button, &mut f, Action::Focus, None),
            ActionOutcome::Focused(f.button)
        );
        assert!(f.focus.focus_visible());
    }

    #[test]
    fn a_button_click_is_reported_not_performed() {
        let mut f = fixture();
        let before = f.tree.payload(f.button).cloned();
        assert_eq!(
            act(f.button, &mut f, Action::Click, None),
            ActionOutcome::Activated(f.button)
        );
        assert_eq!(f.tree.payload(f.button).cloned(), before);
        assert_eq!(
            act(f.swatch, &mut f, Action::Click, None),
            ActionOutcome::Activated(f.swatch)
        );
        assert_eq!(
            act(f.leaf_row, &mut f, Action::Click, None),
            ActionOutcome::Activated(f.leaf_row)
        );
    }

    #[test]
    fn a_checkbox_click_toggles_it_and_marks_damage() {
        let mut f = fixture();
        assert_eq!(
            act(f.checkbox, &mut f, Action::Click, None),
            ActionOutcome::Toggled {
                id: f.checkbox,
                state: Toggled::True
            }
        );
        assert!(!nothing_dirty(&f), "a real change marks damage");
        assert_eq!(
            act(f.checkbox, &mut f, Action::Click, None),
            ActionOutcome::Toggled {
                id: f.checkbox,
                state: Toggled::False
            }
        );
    }

    #[test]
    fn a_slider_takes_set_value_increment_and_decrement() {
        let mut f = fixture();
        assert_eq!(
            act(f.slider, &mut f, Action::SetValue, number(0.25)),
            ActionOutcome::ValueChanged {
                id: f.slider,
                value: 0.25
            }
        );
        assert_eq!(slider_value(&f), 0.25);
        assert!(!nothing_dirty(&f), "a real change marks damage");
        act(f.slider, &mut f, Action::SetValue, number(40.0));
        assert_eq!(
            act(f.slider, &mut f, Action::Increment, None),
            ActionOutcome::ValueChanged {
                id: f.slider,
                value: 41.0
            }
        );
        assert_eq!(
            act(f.slider, &mut f, Action::Decrement, None),
            ActionOutcome::ValueChanged {
                id: f.slider,
                value: 40.0
            }
        );
        act(f.slider, &mut f, Action::SetValue, number(100.0));
        assert_eq!(
            act(f.slider, &mut f, Action::Increment, None),
            ActionOutcome::Unchanged(f.slider)
        );
    }

    #[test]
    fn a_scrollbar_takes_set_value_increment_and_decrement() {
        let mut f = fixture();
        assert_eq!(
            act(f.scrollbar, &mut f, Action::SetValue, number(20.0)),
            ActionOutcome::ValueChanged {
                id: f.scrollbar,
                value: 20.0
            }
        );
        let max = match f.tree.payload(f.scrollbar) {
            Some(WidgetKind::Scrollbar(state)) => state.max,
            _ => unreachable!("scrollbar"),
        };
        let step = max * DEFAULT_STEP_FRACTION;
        assert_eq!(
            act(f.scrollbar, &mut f, Action::Increment, None),
            ActionOutcome::ValueChanged {
                id: f.scrollbar,
                value: 20.0 + step
            }
        );
        assert_eq!(
            act(f.scrollbar, &mut f, Action::Decrement, None),
            ActionOutcome::ValueChanged {
                id: f.scrollbar,
                value: 20.0
            }
        );
        act(f.scrollbar, &mut f, Action::SetValue, number(-5.0));
        assert_eq!(scrollbar_value(&f), 0.0);
        assert_eq!(
            act(f.scrollbar, &mut f, Action::Decrement, None),
            ActionOutcome::Unchanged(f.scrollbar)
        );
    }

    #[test]
    fn a_text_field_set_value_replaces_the_text_as_one_undo_step() {
        let mut f = fixture();
        assert_eq!(
            act(
                f.text,
                &mut f,
                Action::SetValue,
                Some(ActionData::Value("Sky".into()))
            ),
            ActionOutcome::TextChanged(f.text)
        );
        assert_eq!(ok(text_field_state(&f.tree, f.text)).content, "Sky");
        assert_eq!(
            act(
                f.text,
                &mut f,
                Action::SetValue,
                Some(ActionData::Value("Sky".into()))
            ),
            ActionOutcome::Unchanged(f.text)
        );
        let undone = ok(crate::widgets::with_text_field_mut(
            &mut f.tree,
            f.text,
            crate::widgets::TextFieldState::undo,
        ));
        assert!(undone);
        assert_eq!(ok(text_field_state(&f.tree, f.text)).content, "Layer");
    }

    fn text(value: &str) -> Option<ActionData> {
        Some(ActionData::Value(value.into()))
    }

    #[test]
    fn a_text_field_set_value_refuses_control_characters_and_oversized_text() {
        let mut f = fixture();
        let oversized = "a".repeat(MAX_SET_VALUE_TEXT_BYTES + 1);
        for bad in [
            "two\nlines",
            "nul\0",
            "esc\u{1b}[2J",
            "tab\t",
            "c1\u{85}",
            &oversized,
        ] {
            let err = refuse(f.text, &mut f, Action::SetValue, text(bad));
            assert!(
                matches!(err, ActionRejection::DataMismatch { id, action: Action::SetValue } if id == f.text),
                "{err:?}"
            );
            assert_eq!(ok(text_field_state(&f.tree, f.text)).content, "Layer");
        }
        let largest = "b".repeat(MAX_SET_VALUE_TEXT_BYTES);
        assert_eq!(
            act(f.text, &mut f, Action::SetValue, text(&largest)),
            ActionOutcome::TextChanged(f.text)
        );
        assert_eq!(
            act(f.text, &mut f, Action::SetValue, text("\u{202e}txt.exe")),
            ActionOutcome::TextChanged(f.text),
            "a bidi override is a format character, not a control one"
        );
    }

    #[test]
    fn a_text_field_set_value_is_refused_while_a_composition_is_active() {
        let mut f = fixture();
        ok(with_text_field_mut(&mut f.tree, f.text, |state| {
            state.set_composition("ni", None);
        }));
        let before = ok(text_field_state(&f.tree, f.text)).clone();
        // Equal text too (critic C3): no undo step, no `TextChanged`.
        for value in ["Other", "Layer"] {
            let err = refuse(f.text, &mut f, Action::SetValue, text(value));
            assert!(
                matches!(err, ActionRejection::CompositionActive(id) if id == f.text),
                "{err:?}"
            );
            let after = ok(text_field_state(&f.tree, f.text));
            assert_eq!(after.content, before.content);
            assert_eq!(after.composition, before.composition, "composition kept");
        }
        let undone = ok(with_text_field_mut(
            &mut f.tree,
            f.text,
            crate::widgets::TextFieldState::undo,
        ));
        assert!(!undone, "a refused SetValue recorded an undo step");
    }

    #[test]
    fn a_slider_with_a_nan_or_inverted_range_refuses_instead_of_panicking() {
        for (min, max) in [
            (f64::NAN, 100.0),
            (0.0, f64::NAN),
            (0.0, f64::INFINITY),
            (5.0, 1.0),
        ] {
            let mut f = fixture();
            if let Some(WidgetKind::Slider(state)) = f.tree.payload_mut(f.slider) {
                state.min = min;
                state.max = max;
            }
            for (action, data) in [
                (Action::Increment, None),
                (Action::Decrement, None),
                (Action::SetValue, number(1.0)),
            ] {
                // Not `refuse`: its payload equality cannot hold with a NaN
                // bound (`NaN != NaN`), so the value is checked directly.
                let result =
                    handle_action(&mut f.tree, &mut f.focus, &request(f.slider, action, data));
                assert!(
                    matches!(
                        result,
                        Err(ActionRejection::Widget(WidgetError::InvalidRange { .. }))
                    ),
                    "({min}, {max}) {action:?}: {result:?}"
                );
                assert_eq!(slider_value(&f), 50.0, "the value moved");
            }
        }
    }

    #[test]
    fn a_curve_set_value_that_only_moves_the_selection_is_unchanged() {
        let mut f = fixture();
        let last = curve_point(&f, 2);
        let level = ok(curve_editor_state(&f.tree, f.editor))
            .curve()
            .points()
            .get(2)
            .map_or(0.0, |p| f64::from(p.y) * 255.0);
        assert_eq!(
            act(last, &mut f, Action::SetValue, number(level)),
            ActionOutcome::Unchanged(last)
        );
        assert_eq!(
            ok(curve_editor_state(&f.tree, f.editor)).selected(),
            2,
            "the selection still moves"
        );
    }

    #[test]
    fn a_tree_row_expands_and_collapses_and_keeps_focus_alive() {
        let mut f = fixture();
        ok(f.focus.focus(&mut f.tree, f.child_row));
        assert_eq!(
            act(f.group_row, &mut f, Action::Collapse, None),
            ActionOutcome::ExpandedChanged {
                id: f.group_row,
                expanded: false
            }
        );
        assert!(!f.tree.contains(f.child_row));
        assert_eq!(f.focus.focused(), Some(f.group_row));
        let node = f.tree.accessibility(f.group_row);
        assert!(node.is_some_and(|n| n.supports_action(Action::Expand)));
        assert!(node.is_some_and(|n| !n.supports_action(Action::Collapse)));
        assert_eq!(
            act(f.group_row, &mut f, Action::Expand, None),
            ActionOutcome::ExpandedChanged {
                id: f.group_row,
                expanded: true
            }
        );
    }

    #[test]
    fn a_collapse_leaves_focus_elsewhere_alone() {
        let mut f = fixture();
        ok(f.focus.focus(&mut f.tree, f.button));
        act(f.group_row, &mut f, Action::Collapse, None);
        assert_eq!(f.focus.focused(), Some(f.button));
    }

    #[test]
    fn a_dropdown_opens_and_closes_without_committing() {
        let mut f = fixture();
        assert_eq!(
            act(f.dropdown, &mut f, Action::Click, None),
            ActionOutcome::Dropdown {
                id: f.dropdown,
                outcome: DropdownOutcome::Opened
            }
        );
        // Move the highlight so a commit would be visible.
        ok(crate::widgets::handle_dropdown_key(
            &mut f.tree,
            f.dropdown,
            crate::widgets::DropdownKey::Down,
        ));
        assert_eq!(
            act(f.dropdown, &mut f, Action::Collapse, None),
            ActionOutcome::Dropdown {
                id: f.dropdown,
                outcome: DropdownOutcome::Cancelled
            }
        );
        let state = ok(dropdown_state(&f.tree, f.dropdown));
        assert!(!state.is_open());
        assert_eq!(state.selected(), Some(0));
        assert_eq!(
            act(f.dropdown, &mut f, Action::Expand, None),
            ActionOutcome::Dropdown {
                id: f.dropdown,
                outcome: DropdownOutcome::Opened
            }
        );
    }

    #[test]
    fn a_tab_click_selects_it_and_focus_follows_only_from_the_old_tab() {
        let mut f = fixture();
        let old = tab(&f, 0);
        ok(f.focus.focus(&mut f.tree, old));
        let third = tab(&f, 2);
        assert_eq!(
            act(third, &mut f, Action::Click, None),
            ActionOutcome::TabSelected {
                bar: f.tab_bar,
                index: 2
            }
        );
        assert_eq!(ok(tab_bar_state(&f.tree, f.tab_bar)).selected(), 2);
        assert_eq!(f.focus.focused(), Some(third));

        ok(f.focus.focus(&mut f.tree, f.button));
        let second = tab(&f, 1);
        act(second, &mut f, Action::Click, None);
        assert_eq!(ok(tab_bar_state(&f.tree, f.tab_bar)).selected(), 1);
        assert_eq!(f.focus.focused(), Some(f.button));
        assert_eq!(
            act(second, &mut f, Action::Click, None),
            ActionOutcome::Unchanged(second)
        );
    }

    #[test]
    fn a_menu_item_click_activates_its_own_index_counting_separators() {
        let mut f = fixture();
        let copy = menu_item(&f, 2);
        ok(f.focus.focus(&mut f.tree, f.menu));
        assert_eq!(
            act(copy, &mut f, Action::Click, None),
            ActionOutcome::MenuActivated {
                menu: f.menu,
                index: 2
            }
        );
        assert!(!f.tree.contains(f.menu));
        assert_eq!(f.focus.focused(), None, "focus on a closed menu is dropped");
    }

    #[test]
    fn a_disabled_menu_item_is_refused() {
        let mut f = fixture();
        let paste = menu_item(&f, 3);
        let err = refuse(paste, &mut f, Action::Click, None);
        assert!(matches!(err, ActionRejection::Undeclared { .. }), "{err:?}");
        assert!(f.tree.contains(f.menu));
    }

    #[test]
    fn colour_picker_channels_take_values_in_their_own_units() {
        let mut f = fixture();
        let hue = picker_part(&f, ColorPickerPartRole::Hue);
        let saturation = picker_part(&f, ColorPickerPartRole::Saturation);
        let value = picker_part(&f, ColorPickerPartRole::Value);
        assert!(matches!(
            act(hue, &mut f, Action::SetValue, number(180.0)),
            ActionOutcome::ColorChanged { picker, .. } if picker == f.picker
        ));
        assert_eq!(ok(color_picker_state(&f.tree, f.picker)).hsv().hue, 180.0);
        act(saturation, &mut f, Action::SetValue, number(50.0));
        let hsv = ok(color_picker_state(&f.tree, f.picker)).hsv();
        assert_eq!(hsv.saturation, 0.5);
        assert_eq!(hsv.hue, 180.0);
        act(hue, &mut f, Action::Increment, None);
        assert_eq!(ok(color_picker_state(&f.tree, f.picker)).hsv().hue, 181.0);
        act(saturation, &mut f, Action::Decrement, None);
        assert_eq!(
            ok(color_picker_state(&f.tree, f.picker)).hsv().saturation,
            0.49
        );
        act(value, &mut f, Action::SetValue, number(25.0));
        act(value, &mut f, Action::Increment, None);
        let hsv = ok(color_picker_state(&f.tree, f.picker)).hsv();
        assert_eq!(hsv.value, 0.26);
        assert_eq!(hsv.saturation, 0.49);
    }

    #[test]
    fn a_curve_point_takes_values_in_levels_and_moves_the_selection() {
        let mut f = fixture();
        let middle = curve_point(&f, 1);
        let first = curve_point(&f, 0);
        assert_eq!(
            ok(curve_editor_state(&f.tree, f.editor)).focus_target(),
            Some(first)
        );
        ok(f.focus.focus(&mut f.tree, first));
        assert_eq!(
            act(middle, &mut f, Action::SetValue, number(204.0)),
            ActionOutcome::CurveChanged { editor: f.editor }
        );
        let state = ok(curve_editor_state(&f.tree, f.editor));
        assert_eq!(state.selected(), 1);
        let point = state.curve().points().get(1).copied();
        assert_eq!(point, Some(CurvePoint::new(0.5, 0.8)));
        assert_eq!(
            f.focus.focused(),
            Some(middle),
            "focus follows the selection"
        );
        act(middle, &mut f, Action::Increment, None);
        let y = ok(curve_editor_state(&f.tree, f.editor))
            .curve()
            .points()
            .get(1)
            .map(|p| p.y);
        assert!(
            y.is_some_and(|y| (f64::from(y) * 255.0 - 205.0).abs() < 1e-3),
            "{y:?}"
        );
    }

    // ---- The standing guard -----------------------------------------

    /// Every widget in [`fixture`], by id — and a check that the fixture
    /// really holds one of every [`WidgetKind`], through an exhaustive
    /// `match` a new kind cannot slip past.
    fn fixture_ids(f: &Fixture) -> Vec<WidgetId> {
        let mut ids = Vec::new();
        let mut pending = vec![f.root];
        while let Some(id) = pending.pop() {
            ids.push(id);
            pending.extend(f.tree.children(id).into_iter().flatten().copied());
        }
        ids.sort_unstable();
        ids
    }

    fn kind_ordinal(kind: &WidgetKind) -> usize {
        match kind {
            WidgetKind::Container => 0,
            WidgetKind::Button(_) => 1,
            WidgetKind::Checkbox(_) => 2,
            WidgetKind::Slider(_) => 3,
            WidgetKind::Scrollbar(_) => 4,
            WidgetKind::TextField(_) => 5,
            WidgetKind::CommandPalette(_) => 6,
            WidgetKind::ColorSwatch(_) => 7,
            WidgetKind::ListRow(_) => 8,
            WidgetKind::TreeItem(_) => 9,
            WidgetKind::Panel => 10,
            WidgetKind::Dialog => 11,
            WidgetKind::Dropdown(_) => 12,
            WidgetKind::DropdownList => 13,
            WidgetKind::TabBar(_) => 14,
            WidgetKind::Tab(_) => 15,
            WidgetKind::Tooltip => 16,
            WidgetKind::Menu(_) => 17,
            WidgetKind::MenuSeparator => 18,
            WidgetKind::ColorPicker(_) => 19,
            WidgetKind::ColorPickerPart(_) => 20,
            WidgetKind::CurveEditor(_) => 21,
            WidgetKind::CurveEditorPoint(_) => 22,
        }
    }
    const KIND_COUNT: usize = 23;

    /// The data a well-formed request for `action` on `id` would carry.
    fn well_formed(f: &Fixture, id: WidgetId, action: Action) -> Option<ActionData> {
        match action {
            Action::SetValue => match f.tree.payload(id) {
                Some(WidgetKind::TextField(_)) => Some(ActionData::Value("v".into())),
                _ => number(1.0),
            },
            Action::CustomAction => Some(ActionData::CustomAction(0)),
            _ => None,
        }
    }

    #[test]
    fn the_fixture_holds_every_widget_kind() {
        let f = fixture();
        let mut seen = [false; KIND_COUNT];
        for id in fixture_ids(&f) {
            if let Some(kind) = f.tree.payload(id)
                && let Some(slot) = seen.get_mut(kind_ordinal(kind))
            {
                *slot = true;
            }
        }
        assert_eq!(seen, [true; KIND_COUNT], "a widget kind is missing");
    }

    #[test]
    fn every_declared_action_is_routed_and_nothing_undeclared_is() {
        assert!(sweep(fixture) > 50, "too few routes exercised");
    }

    /// The same sweep after every state-dependent declaration has flipped
    /// — declarations depend on state (a collapsed row declares `Expand`,
    /// the selected tab stops declaring `Focus`, a disabled widget is
    /// refused), so the initial fixture alone does not cover them.
    #[test]
    fn the_sweep_holds_in_every_toggled_state_too() {
        let variants: [fn() -> Fixture; 5] = [
            || {
                let mut f = fixture();
                ok(set_dropdown_open(&mut f.tree, f.dropdown, true));
                ok(set_dropdown_open(&mut f.tree, f.open_dropdown, false));
                f
            },
            || {
                let mut f = fixture();
                ok(set_tree_item_expanded(&mut f.tree, f.group_row, false));
                f
            },
            || {
                let mut f = fixture();
                ok(select_tab(&mut f.tree, f.tab_bar, 1));
                f
            },
            || {
                let mut f = fixture();
                ok(select_curve_point(&mut f.tree, f.editor, 2));
                f
            },
            || {
                let mut f = fixture();
                ok(set_slider_disabled(&mut f.tree, f.slider, true));
                ok(set_text_field_disabled(&mut f.tree, f.text, true));
                ok(set_tree_item_disabled(&mut f.tree, f.leaf_row, true));
                ok(set_curve_editor_disabled(&mut f.tree, f.editor, true));
                ok(set_color_picker_disabled(&mut f.tree, f.picker, true));
                f
            },
        ];
        for make in variants {
            assert!(sweep(make) > 30, "too few routes exercised");
        }
    }

    /// Sends every [`ALL_ACTIONS`] action, well-formed, to every widget of
    /// a fresh `make()` fixture and checks declared == routed. Returns how
    /// many requests were routed.
    fn sweep(make: fn() -> Fixture) -> usize {
        let ids = fixture_ids(&make());
        let mut routed = 0_usize;
        for &id in &ids {
            for action in ALL_ACTIONS {
                let mut f = make();
                assert_eq!(fixture_ids(&f), ids, "the fixture is not deterministic");
                let declared = f
                    .tree
                    .accessibility(id)
                    .is_some_and(|node| node.supports_action(action));
                let data = well_formed(&f, id, action);
                let result = handle_action(&mut f.tree, &mut f.focus, &request(id, action, data));
                match result {
                    Ok(_) => {
                        assert!(
                            declared,
                            "{action:?} on {id:?} was routed but never declared"
                        );
                        routed += 1;
                    }
                    Err(ActionRejection::Unsupported { .. }) => {
                        unreachable!("{action:?} on {id:?} is declared but nothing routes it")
                    }
                    Err(ActionRejection::Undeclared { .. }) => {
                        assert!(!declared, "{action:?} on {id:?} is declared yet refused");
                    }
                    Err(ActionRejection::Disabled(_)) => {
                        let disabled = f.tree.accessibility(id).is_some_and(Node::is_disabled)
                            || f.tree.payload(id).is_some_and(payload_disabled);
                        assert!(disabled, "{action:?} on enabled {id:?} refused as disabled");
                    }
                    Err(err) => {
                        assert!(declared, "{action:?} on {id:?}: {err:?}");
                        unreachable!(
                            "a declared, well-formed {action:?} on {id:?} failed: {err:?}"
                        );
                    }
                }
            }
        }
        routed
    }
}
