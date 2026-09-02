//! A scrollbar: a bounded position along one axis, plus the size of the
//! visible page within that range.
//!
//! **Scope, stated honestly. This is a position *model*, not scrolling.**
//! Nothing in this crate scrolls any content: there is no viewport, no
//! clip rect, no content-offset transform, and no widget observes a
//! [`ScrollbarState`] to move itself. `widgets`' own module doc comment
//! lists "scrolling for scrollbars/trees" as infrastructure that does
//! not exist yet, and it still does not — what landed here is the
//! accessibility node, the layout style, the clamped value arithmetic,
//! and the paint geometry, which is exactly the boundary every other
//! widget in this module already keeps ("layout + content, no
//! behaviour"). A real scrolling container is separate, later work.
//!
//! `tests/gallery.rs` now carries this widget's own component-gallery
//! entry — four cells (a vertical bar at its own minimum, at its own
//! maximum, disabled, and a horizontal bar mid-travel) in all five
//! built-in themes, each with a real rendered-pixel proof and an
//! `#[ignore]`d golden-diff test pending a human bless on real GPU
//! hardware, the same shape every other widget's gallery already
//! follows (CLAUDE.md: "a green test run is not evidence that canvas or
//! UI work is correct"). `paint.rs`'s own scrollbar unit tests cover the
//! shapes, ordering, thumb travel, disabled dimming, and every
//! degenerate-geometry guard without claiming a visual review happened.
//!
//! **Why `Role::ScrollBar` plus the numeric-value vocabulary** — value,
//! min, max, and the `SetValue`/`Increment`/`Decrement` actions — rather
//! than `accesskit`'s `ScrollX`/`ScrollXMin`/`ScrollXMax` and
//! `SetScrollOffset`: the scroll-offset properties are read by no
//! shipping `accesskit` platform adapter, whereas the numeric-value
//! properties are what drive the Windows UIA `RangeValue` pattern and the
//! macOS/AT-SPI Value interfaces. The scroll-offset vocabulary describes
//! a *scrollable container's* own state, which is precisely the thing
//! this crate does not have yet.
//!
//! **Known platform caveat, stated rather than implied away.**
//! `accesskit`'s own Windows UIA adapter maps `Role::ScrollBar` to a
//! `RangeValue` pattern it reports as read-only regardless of the
//! `Action::SetValue` declared below, so on Windows a screen-reader user
//! can read a scrollbar's position but not set it through UIA. That is a
//! property of the `accesskit` role mapping, not of this file, and it is
//! not worked around here — it is recorded so the actions list below
//! isn't read as a promise this crate can keep on every platform.

use accesskit::{Action, Node, Orientation, Role};
use aurora_theme::Scales;
use taffy::style_helpers::{length, percent};
use taffy::{Size, Style};

use super::{WidgetKind, type_size};
use crate::error::WidgetError;
use crate::tree::{WidgetId, WidgetTree};

/// The three numbers that travel together whenever a scrollbar's own
/// extent is described: the bounds of its position, and how much of the
/// scrolled content is visible at once.
///
/// Grouped into a struct rather than passed as three more parameters
/// only because the workspace's own `too_many_arguments` lint is a real
/// bound and [`insert_scrollbar`] would otherwise sit past it — the same
/// reasoning `aurora_io`'s own `WritePolicy` already records.
///
/// **The convention, stated explicitly** (it is the one thing a caller
/// can get subtly wrong and never see an error for): `min`/`max` bound
/// the scroll *offset*, not the content. For content of `content_size`
/// scrolled through a viewport of `page_size`, the correct range is
/// `min = 0.0`, `max = content_size - page_size` — `max` is where the
/// *top* (or left) of the page sits when scrolled all the way to the
/// end, so `max` is **not** the content size. The whole scrollable
/// extent is therefore `(max - min) + page_size`, which is what
/// `paint_scrollbar` divides `page_size` by to get the thumb's own
/// proportional length. Passing `max = content_size` paints a thumb one
/// page too short and lets the value run one page past the content's
/// own end.
///
/// No `Eq`: the fields are `f64`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollbarRange {
    /// The smallest scroll offset, normally `0.0`. Must be finite, and
    /// `<= max` — [`insert_scrollbar`] returns
    /// [`WidgetError::InvalidRange`] otherwise rather than panicking
    /// inside `f64::clamp`.
    pub min: f64,
    /// The largest scroll offset — `content_size - page_size`, not
    /// `content_size`. Must be finite, and `>= min`.
    pub max: f64,
    /// How much of the scrolled content is visible at once, in the same
    /// units as `min`/`max` — what sets the thumb's own *length*
    /// relative to its track, the one thing that distinguishes a
    /// scrollbar's paint from a slider's. `0.0` is legal and means "no
    /// proportional information," which paints a minimum-length thumb.
    /// A negative or non-finite `page_size` is meaningless rather than
    /// merely unusual (it would make the thumb's own length run
    /// backwards against the value), so it is clamped to `0.0` at
    /// construction instead of being stored as given.
    pub page_size: f64,
}

/// A scrollbar's own state. No `Eq` (the value/range fields are `f64`).
#[derive(Debug, Clone, PartialEq)]
pub struct ScrollbarState {
    /// `accesskit::Orientation` directly, not a second, parallel enum —
    /// the same "reuse `accesskit`'s own vocabulary" discipline
    /// [`super::CheckboxState`]'s own `accesskit::Toggled` already
    /// established.
    pub orientation: Orientation,
    /// The scrollbar's own accessible name, when it has one. `Option`
    /// rather than [`super::SliderState`]'s bare `String`, because a
    /// scrollbar is usually named by the region it scrolls (an
    /// `aria-controls`-shaped relationship this crate cannot express
    /// yet) rather than by chrome of its own — but a standalone bar
    /// with no such region, which is every scrollbar this crate can
    /// build today, is otherwise announced as an unnamed "scroll bar",
    /// so a caller must be able to supply one.
    pub label: Option<String>,
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub page_size: f64,
    pub disabled: bool,
}

fn node(state: &ScrollbarState) -> Node {
    let mut node = Node::new(Role::ScrollBar);
    node.set_orientation(state.orientation);
    if let Some(label) = &state.label {
        node.set_label(label.clone());
    }
    node.set_numeric_value(state.value);
    node.set_min_numeric_value(state.min);
    node.set_max_numeric_value(state.max);
    // The "large change" a Page Up/Page Down lands: one page. Without
    // it a screen reader has the position and the bounds but no idea
    // what a page-jump moves by, which is the one quantity a scrollbar
    // has that a slider does not.
    node.set_numeric_value_jump(state.page_size);
    if state.disabled {
        node.set_disabled();
    } else {
        node.add_action(Action::Focus);
        node.add_action(Action::SetValue);
        node.add_action(Action::Increment);
        node.add_action(Action::Decrement);
    }
    node
}

/// Fills the available space along its own scrolling axis (a scrollbar
/// is sized by the region it sits beside, not by its own content), with
/// a fixed cross-axis thickness grounded in the type scale — see
/// `type_size`'s own doc comment on why, the same reasoning
/// `checkbox::style`/`slider::style` already use. Which axis gets the
/// fixed thickness is the whole point of branching here: a vertical
/// scrollbar that took `slider::style` unchanged would be a 13px-tall
/// horizontal bar.
///
/// **Both axes are stated outright, and `flex_grow` is deliberately
/// `0.0`.** The first version of this function set `flex_grow: 1.0` with
/// `auto()` on the scrolling axis, borrowed from `slider::style` — which
/// is wrong for a widget whose axis is its own property rather than its
/// parent's. `flex_grow` grows the *parent's* main axis, so it inflated
/// a vertical scrollbar's width to fill the whole `Row`, and the
/// default `align_items: Stretch` inflated its `auto()` height to
/// match; measured (`a_vertical_scrollbar_fills_its_parents_height_
/// at_a_fixed_width` below, before the fix, and reproduced
/// independently against a standalone `taffy` program with no Aurora
/// code at all) that resolved to `300 x 200` in a 300x200 `Row` — the
/// bar swallowed its entire parent. Filling the scrolling axis with
/// `percent(1.0)` says what is actually meant regardless of which
/// direction the parent happens to flex, and `flex_shrink: 0.0` keeps a
/// crowded parent from squeezing the fixed thickness away.
///
/// Measured against the alternatives rather than assumed: `flex_grow:
/// 1.0` + `align_self: STRETCH` also gives `13 x 200` inside a 300x200
/// `Column`, but `300 x 200` inside a 300x200 `Row` — the bar swallows
/// the whole parent. `percent(1.0)` is the only one of the three that
/// resolves to `13 x 200` in *both*.
///
/// **What it still cannot do**: give a bar length inside a parent whose
/// own size along that axis is content-derived (`auto`). A percentage
/// resolves against a definite parent size, and there isn't one, so the
/// bar comes out zero-length — as it does under `flex_grow`/`stretch`
/// too, for the same reason. That is not a bug this style can fix: a
/// scrollbar's length comes from the region it scrolls, and a region
/// with no size of its own has none to give. Callers put scrollbars in
/// sized containers.
fn style(scales: &Scales, orientation: Orientation) -> Style {
    let thickness = length(type_size(scales.typography.size.md));
    // `1.0_f32`, not a bare `1.0`: `percent` is generic and the literal
    // would otherwise land on a deprecated integer-fallback path.
    let full = percent(1.0_f32);
    let size = match orientation {
        Orientation::Vertical => Size {
            width: thickness,
            height: full,
        },
        Orientation::Horizontal => Size {
            width: full,
            height: thickness,
        },
    };
    Style {
        flex_grow: 0.0,
        flex_shrink: 0.0,
        size,
        ..Default::default()
    }
}

/// A scrollbar's own range, validated once so nothing downstream has to
/// re-check it: `f64::clamp` panics (a `core` assertion this workspace's
/// own `panic = "deny"` lint cannot see) unless `min <= max`, which
/// `NaN` on either side also violates, and `paint_scrollbar`'s own
/// arithmetic produces `NaN` geometry from infinite bounds even though
/// `NEG_INFINITY <= INFINITY` is perfectly true.
fn checked_range(min: f64, max: f64) -> Result<(), WidgetError> {
    if min.is_finite() && max.is_finite() && min <= max {
        Ok(())
    } else {
        Err(WidgetError::InvalidRange { min, max })
    }
}

/// A starting/incoming position, made safe to `clamp`. A non-finite
/// value is a caller bug, but a scrollbar's position is updated
/// continuously from pointer input, so parking it at the range's own
/// start — the same "degenerate input parks at the start" convention
/// `paint_scrollbar` already uses — keeps the widget total rather than
/// erroring once per frame. `f64::clamp` propagates `NaN` silently
/// instead of clamping it, so this cannot be left to `clamp` alone.
fn clamped_value(value: f64, min: f64, max: f64) -> f64 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        min
    }
}

/// Adds a new, enabled scrollbar as the last child of `parent`, with
/// `value` (already clamped to `range.min..=range.max`) as its starting
/// position and `label` as its accessible name, when it has one of its
/// own — see [`ScrollbarState::label`] for when it should.
///
/// A non-finite `value` is parked at `range.min` rather than rejected,
/// and a negative or non-finite `range.page_size` is clamped to `0.0`;
/// see `clamped_value` and [`ScrollbarRange::page_size`] for why each
/// is sanitized rather than refused.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `parent` doesn't exist, or
/// [`WidgetError::InvalidRange`] if `range.min`/`range.max` are not both
/// finite with `min <= max`. Unlike [`super::insert_slider`], which
/// documents `min <= max` as an unchecked caller precondition, this is
/// checked: `f64::clamp` asserts it internally, and a `core` assertion
/// firing is a panic in a crate that denies panics.
pub fn insert_scrollbar(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
    orientation: Orientation,
    label: Option<&str>,
    value: f64,
    range: ScrollbarRange,
) -> Result<WidgetId, WidgetError> {
    checked_range(range.min, range.max)?;
    let page_size = if range.page_size.is_finite() {
        range.page_size.max(0.0)
    } else {
        0.0
    };
    let state = ScrollbarState {
        orientation,
        label: label.map(ToOwned::to_owned),
        value: clamped_value(value, range.min, range.max),
        min: range.min,
        max: range.max,
        page_size,
        disabled: false,
    };
    tree.insert(
        parent,
        style(scales, orientation),
        node(&state),
        WidgetKind::Scrollbar(state),
    )
}

/// Sets `id` (a scrollbar) to `value`, clamped to its own
/// `min..=max`. Returns the clamped value actually stored. A non-finite
/// `value` is parked at `min` — see `clamped_value`.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist,
/// [`WidgetError::WrongWidgetKind`] if it exists but isn't a scrollbar,
/// [`WidgetError::WidgetDisabled`] if it's disabled, or
/// [`WidgetError::InvalidRange`] if the stored `min`/`max` are no longer
/// a usable range. [`insert_scrollbar`] guarantees they start out one,
/// so that last case only arises when a caller has reached past this
/// module through [`WidgetTree::payload_mut`] — which is public, so the
/// check is re-done here rather than assumed away.
pub fn set_scrollbar_value(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    value: f64,
) -> Result<f64, WidgetError> {
    let mut result = 0.0;
    with_scrollbar_mut(tree, id, |state| {
        if state.disabled {
            return Err(WidgetError::WidgetDisabled(id));
        }
        checked_range(state.min, state.max)?;
        state.value = clamped_value(value, state.min, state.max);
        result = state.value;
        Ok(())
    })?;
    Ok(result)
}

/// Sets whether `id` (a scrollbar) is disabled.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if it exists but isn't a scrollbar.
pub fn set_scrollbar_disabled(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    disabled: bool,
) -> Result<(), WidgetError> {
    with_scrollbar_mut(tree, id, |state| {
        state.disabled = disabled;
        Ok(())
    })
}

fn with_scrollbar_mut(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    f: impl FnOnce(&mut ScrollbarState) -> Result<(), WidgetError>,
) -> Result<(), WidgetError> {
    {
        let kind = tree.payload_mut(id).ok_or(WidgetError::UnknownWidget(id))?;
        let WidgetKind::Scrollbar(state) = kind else {
            return Err(WidgetError::WrongWidgetKind(id));
        };
        f(state)?;
    }
    let Some(WidgetKind::Scrollbar(state)) = tree.payload(id) else {
        unreachable!("id was just confirmed to be a Scrollbar above");
    };
    let accessibility = node(state);
    // Two calls, not one, and deliberately so. `set_accessibility` sets
    // only the per-widget `dirty` flag; `mark_dirty` is what unions the
    // widget's own bounds into the tree-wide damage region
    // `take_damage` hands a renderer. A scrollbar whose value moved has
    // *new pixels*, so it needs both -- the first version of this
    // function called only `set_accessibility` ("the same as
    // `with_slider_mut`") and so moved a thumb that never got
    // repainted. The identical gap still exists in `with_slider_mut`
    // and `text_field`'s own mutators; fixing those is a separate
    // change to a separate widget, not a drive-by here.
    tree.set_accessibility(id, accessibility)?;
    tree.mark_dirty(id)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ScrollbarRange, insert_scrollbar, set_scrollbar_disabled, set_scrollbar_value};
    use crate::WidgetError;
    use crate::widgets::{WidgetKind, new_tree, test_scales};
    use accesskit::{Action, Orientation};
    use aurora_core::Rect;
    use taffy::style_helpers::length;
    use taffy::{FlexDirection, Size, Style};

    fn range() -> ScrollbarRange {
        ScrollbarRange {
            min: 0.0,
            max: 100.0,
            page_size: 20.0,
        }
    }

    #[test]
    // Exact-literal round-trip, no arithmetic -- same reasoning
    // `slider::tests`/`tree::tests` already document for their own
    // float_cmp allows.
    #[allow(clippy::float_cmp)]
    fn insert_scrollbar_clamps_an_out_of_range_starting_value() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_scrollbar(
            &mut tree,
            root,
            &scales,
            Orientation::Vertical,
            None,
            150.0,
            range(),
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        match tree.payload(id) {
            Some(WidgetKind::Scrollbar(state)) => assert_eq!(state.value, 100.0),
            other => unreachable!("expected Scrollbar, got {other:?}"),
        }
        let Some(accessibility) = tree.accessibility(id) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.numeric_value(), Some(100.0));
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn insert_scrollbar_declares_its_own_orientation_and_actions() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_scrollbar(
            &mut tree,
            root,
            &scales,
            Orientation::Vertical,
            None,
            25.0,
            range(),
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(accessibility) = tree.accessibility(id) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.role(), accesskit::Role::ScrollBar);
        // `orientation()` is an `Option<Orientation>`, not a bare value.
        assert_eq!(accessibility.orientation(), Some(Orientation::Vertical));
        assert_eq!(accessibility.numeric_value(), Some(25.0));
        assert_eq!(accessibility.min_numeric_value(), Some(0.0));
        assert_eq!(accessibility.max_numeric_value(), Some(100.0));
        assert_eq!(
            accessibility.numeric_value_jump(),
            Some(20.0),
            "the page size must reach the accessibility node as the numeric value jump -- it is \
             what a Page Up/Page Down actually moves by, and the one quantity a scrollbar has \
             that a slider does not"
        );
        assert!(accessibility.supports_action(Action::Focus));
        assert!(accessibility.supports_action(Action::SetValue));
        assert!(accessibility.supports_action(Action::Increment));
        assert!(accessibility.supports_action(Action::Decrement));
        assert!(!accessibility.is_disabled());
    }

    #[test]
    fn a_horizontal_scrollbar_declares_horizontal_orientation() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_scrollbar(
            &mut tree,
            root,
            &scales,
            Orientation::Horizontal,
            None,
            0.0,
            range(),
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(accessibility) = tree.accessibility(id) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.orientation(), Some(Orientation::Horizontal));
    }

    #[test]
    fn insert_scrollbar_rejects_an_unknown_parent() {
        let (mut tree, _root) = new_tree(Style::default());
        let scales = test_scales();
        // Same bogus-id precedent `tree`'s own tests use -- never
        // inserted into this tree.
        let bogus = accesskit::NodeId(999);
        match insert_scrollbar(
            &mut tree,
            bogus,
            &scales,
            Orientation::Vertical,
            None,
            0.0,
            range(),
        ) {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn set_scrollbar_value_clamps_to_the_scrollbars_own_range() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_scrollbar(
            &mut tree,
            root,
            &scales,
            Orientation::Vertical,
            None,
            50.0,
            range(),
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        match set_scrollbar_value(&mut tree, id, 75.0) {
            Ok(v) => assert_eq!(v, 75.0),
            Err(err) => unreachable!("{err:?}"),
        }
        match set_scrollbar_value(&mut tree, id, -10.0) {
            Ok(v) => assert_eq!(v, 0.0),
            Err(err) => unreachable!("{err:?}"),
        }
        match set_scrollbar_value(&mut tree, id, 1000.0) {
            Ok(v) => assert_eq!(v, 100.0),
            Err(err) => unreachable!("{err:?}"),
        }
        let Some(accessibility) = tree.accessibility(id) else {
            unreachable!("just inserted");
        };
        assert_eq!(
            accessibility.numeric_value(),
            Some(100.0),
            "the accessibility node must carry the clamped value, not the raw one"
        );
    }

    /// Both halves of "dirty", not just the boolean. `set_accessibility`
    /// alone sets the per-widget flag but never unions the widget's own
    /// bounds into the tree-wide damage region a renderer actually reads
    /// through `take_damage` — so an assertion on `is_dirty` alone
    /// passed while a moved thumb went unrepainted.
    #[test]
    fn set_scrollbar_value_marks_the_widget_dirty() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_scrollbar(
            &mut tree,
            root,
            &scales,
            Orientation::Vertical,
            None,
            0.0,
            range(),
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.set_bounds(
            id,
            Rect {
                x: 4,
                y: 8,
                width: 13,
                height: 200,
            },
        ) {
            unreachable!("{err:?}");
        }
        tree.take_damage();
        if let Err(err) = set_scrollbar_value(&mut tree, id, 50.0) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.is_dirty(id), Some(true));
        assert_eq!(
            tree.take_damage(),
            Some(Rect {
                x: 4,
                y: 8,
                width: 13,
                height: 200,
            }),
            "a value change must widen the tree's own damage region to the scrollbar's own \
             bounds, not only set its per-widget dirty flag"
        );
    }

    #[test]
    fn set_scrollbar_value_rejects_a_disabled_scrollbar() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_scrollbar(
            &mut tree,
            root,
            &scales,
            Orientation::Vertical,
            None,
            0.0,
            range(),
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_scrollbar_disabled(&mut tree, id, true) {
            unreachable!("{err:?}");
        }
        match set_scrollbar_value(&mut tree, id, 50.0) {
            Err(WidgetError::WidgetDisabled(got)) => assert_eq!(got, id),
            other => unreachable!("expected WidgetDisabled, got {other:?}"),
        }
    }

    #[test]
    fn set_scrollbar_disabled_clears_the_accesskit_actions() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_scrollbar(
            &mut tree,
            root,
            &scales,
            Orientation::Vertical,
            None,
            0.0,
            range(),
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_scrollbar_disabled(&mut tree, id, true) {
            unreachable!("{err:?}");
        }
        let Some(accessibility) = tree.accessibility(id) else {
            unreachable!("just inserted");
        };
        assert!(accessibility.is_disabled());
        assert!(!accessibility.supports_action(Action::Focus));
        assert!(!accessibility.supports_action(Action::SetValue));
        assert!(!accessibility.supports_action(Action::Increment));
        assert!(!accessibility.supports_action(Action::Decrement));
    }

    #[test]
    fn scrollbar_mutators_reject_a_wrong_widget_kind() {
        let (mut tree, root) = new_tree(Style::default());
        match set_scrollbar_value(&mut tree, root, 1.0) {
            Err(WidgetError::WrongWidgetKind(id)) => assert_eq!(id, root),
            other => unreachable!("expected WrongWidgetKind, got {other:?}"),
        }
        match set_scrollbar_disabled(&mut tree, root, true) {
            Err(WidgetError::WrongWidgetKind(id)) => assert_eq!(id, root),
            other => unreachable!("expected WrongWidgetKind, got {other:?}"),
        }
    }

    #[test]
    fn scrollbar_mutators_reject_an_unknown_widget() {
        let (mut tree, _root) = new_tree(Style::default());
        let bogus = accesskit::NodeId(999);
        match set_scrollbar_value(&mut tree, bogus, 1.0) {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
        match set_scrollbar_disabled(&mut tree, bogus, true) {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }

    /// Every range `f64::clamp`'s own `assert!(min <= max)` would have
    /// panicked on, and the one it would *not* have — infinite bounds,
    /// which satisfy `min <= max` perfectly well while making every
    /// downstream fraction `inf / inf = NaN`. All five must come back as
    /// a `Result`, because this crate denies `panic` precisely so that
    /// a caller's bad number can't cost a user their unsaved work.
    #[test]
    #[allow(clippy::float_cmp)]
    fn insert_scrollbar_rejects_a_range_it_cannot_clamp_against() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        for (min, max) in [
            (100.0, 0.0),
            (f64::NAN, 100.0),
            (0.0, f64::NAN),
            (f64::NEG_INFINITY, f64::INFINITY),
            (0.0, f64::INFINITY),
        ] {
            match insert_scrollbar(
                &mut tree,
                root,
                &scales,
                Orientation::Vertical,
                None,
                0.0,
                ScrollbarRange {
                    min,
                    max,
                    page_size: 20.0,
                },
            ) {
                Err(WidgetError::InvalidRange {
                    min: got_min,
                    max: got_max,
                }) => {
                    // `to_bits`, not `==`: `NaN != NaN`, and the
                    // point is that the error reports back exactly
                    // the bounds it was handed.
                    assert_eq!(got_min.to_bits(), min.to_bits());
                    assert_eq!(got_max.to_bits(), max.to_bits());
                }
                other => unreachable!("expected InvalidRange for ({min}, {max}), got {other:?}"),
            }
        }
        assert_eq!(tree.len(), 1, "no rejected scrollbar may reach the tree");
    }

    /// `WidgetTree::payload_mut` is public, so a caller can put a
    /// scrollbar into a state `insert_scrollbar` would have refused.
    /// `set_scrollbar_value` must still return rather than panic inside
    /// `f64::clamp` — the exact reproduction the review round found.
    #[test]
    fn set_scrollbar_value_rejects_a_range_corrupted_through_payload_mut() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_scrollbar(
            &mut tree,
            root,
            &scales,
            Orientation::Vertical,
            None,
            0.0,
            range(),
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        match tree.payload_mut(id) {
            Some(WidgetKind::Scrollbar(state)) => {
                state.min = 100.0;
                state.max = 0.0;
            }
            other => unreachable!("expected Scrollbar, got {other:?}"),
        }
        match set_scrollbar_value(&mut tree, id, 50.0) {
            Err(WidgetError::InvalidRange { .. }) => {}
            other => unreachable!("expected InvalidRange, got {other:?}"),
        }
    }

    /// `f64::clamp` propagates `NaN` instead of clamping it, so a
    /// non-finite position would otherwise be stored verbatim, reported
    /// to a screen reader verbatim, and tessellated verbatim.
    #[test]
    #[allow(clippy::float_cmp)]
    fn a_non_finite_value_is_parked_at_the_scrollbars_own_minimum() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_scrollbar(
            &mut tree,
            root,
            &scales,
            Orientation::Vertical,
            None,
            f64::NAN,
            ScrollbarRange {
                min: 10.0,
                max: 100.0,
                page_size: 20.0,
            },
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        match tree.payload(id) {
            Some(WidgetKind::Scrollbar(state)) => assert_eq!(state.value, 10.0),
            other => unreachable!("expected Scrollbar, got {other:?}"),
        }
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            match set_scrollbar_value(&mut tree, id, bad) {
                Ok(stored) => assert_eq!(stored, 10.0, "{bad} must park at the minimum"),
                Err(err) => unreachable!("{err:?}"),
            }
        }
    }

    /// A negative page is not merely unusual, it is backwards: it would
    /// shrink the scrollable span below the travel it contains, so the
    /// thumb's own proportional length stops being monotonic in the
    /// content size.
    #[test]
    #[allow(clippy::float_cmp)]
    fn insert_scrollbar_clamps_a_negative_or_non_finite_page_size_to_zero() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        for page_size in [-5.0, f64::NAN, f64::INFINITY] {
            let id = match insert_scrollbar(
                &mut tree,
                root,
                &scales,
                Orientation::Vertical,
                None,
                0.0,
                ScrollbarRange {
                    min: 0.0,
                    max: 100.0,
                    page_size,
                },
            ) {
                Ok(id) => id,
                Err(err) => unreachable!("{err:?}"),
            };
            match tree.payload(id) {
                Some(WidgetKind::Scrollbar(state)) => {
                    assert_eq!(state.page_size, 0.0, "{page_size} must be clamped to 0.0");
                }
                other => unreachable!("expected Scrollbar, got {other:?}"),
            }
            let Some(accessibility) = tree.accessibility(id) else {
                unreachable!("just inserted");
            };
            assert_eq!(accessibility.numeric_value_jump(), Some(0.0));
        }
    }

    #[test]
    fn a_scrollbars_label_reaches_its_accessibility_node() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let named = match insert_scrollbar(
            &mut tree,
            root,
            &scales,
            Orientation::Vertical,
            Some("Layers"),
            0.0,
            range(),
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let unnamed = match insert_scrollbar(
            &mut tree,
            root,
            &scales,
            Orientation::Vertical,
            None,
            0.0,
            range(),
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(accessibility) = tree.accessibility(named) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.label(), Some("Layers"));
        let Some(accessibility) = tree.accessibility(unnamed) else {
            unreachable!("just inserted");
        };
        assert_eq!(
            accessibility.label(),
            None,
            "an unnamed scrollbar must carry no label at all, not an empty one"
        );
        // A label survives a mutation -- `node()` is rebuilt from state
        // on every change, so a field it forgets is silently dropped.
        if let Err(err) = set_scrollbar_value(&mut tree, named, 50.0) {
            unreachable!("{err:?}");
        }
        let Some(accessibility) = tree.accessibility(named) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.label(), Some("Layers"));
    }

    /// A parent with a real size of its own, the way any container that
    /// could actually hold a scrollbar has one — a percentage resolves
    /// against a definite parent size, and `Style::default()`'s `auto`
    /// isn't one (see `style()`'s own doc comment).
    fn sized_row(direction: FlexDirection) -> Style {
        Style {
            flex_direction: direction,
            size: Size {
                width: length(300.0_f32),
                height: length(200.0_f32),
            },
            ..Default::default()
        }
    }

    /// The real resolved-layout proof, run through `compute_layout`
    /// rather than read off `style()`. Before the fix this resolved to
    /// `300 x 200` — the bar swallowed its entire parent, because
    /// `flex_grow: 1.0` inflates the *parent's* main axis (width, in
    /// this `Row`) and the default `align_items: Stretch` inflates
    /// `auto()`'s cross axis (height) to match.
    #[test]
    fn a_vertical_scrollbar_fills_its_parents_height_at_a_fixed_width() {
        let (mut tree, root) = new_tree(sized_row(FlexDirection::Row));
        let scales = test_scales();
        let id = match insert_scrollbar(
            &mut tree,
            root,
            &scales,
            Orientation::Vertical,
            None,
            0.0,
            range(),
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.compute_layout(300.0, 200.0);
        assert_eq!(
            tree.bounds(id),
            Some(Rect {
                x: 0,
                y: 0,
                width: 13,
                height: 200,
            }),
            "a vertical scrollbar is one type-scale step wide and as tall as the region it \
             sits beside"
        );
    }

    /// The mirror of the test above, and the whole point of `style()`
    /// branching on orientation at all: the same widget in the same
    /// sized parent must resolve to a *different* rectangle depending on
    /// which way it scrolls. Deliberately a `Column` root, so neither
    /// case can be passing by accident of the parent's flex direction.
    #[test]
    fn a_horizontal_scrollbar_fills_its_parents_width_at_a_fixed_height() {
        let (mut tree, root) = new_tree(sized_row(FlexDirection::Column));
        let scales = test_scales();
        let id = match insert_scrollbar(
            &mut tree,
            root,
            &scales,
            Orientation::Horizontal,
            None,
            0.0,
            range(),
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.compute_layout(300.0, 200.0);
        assert_eq!(
            tree.bounds(id),
            Some(Rect {
                x: 0,
                y: 0,
                width: 300,
                height: 13,
            }),
            "a horizontal scrollbar is one type-scale step tall and as wide as the region it \
             sits beside"
        );
    }
}
