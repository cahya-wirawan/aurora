//! A scrollbar: a bounded position along one axis, plus the size of the
//! visible page within that range.
//!
//! **Two uses.** A bare bar ([`insert_scrollbar`] alone) is a position
//! *model*: nothing observes its [`ScrollbarState`] to move any content,
//! which is what the Widget Gallery's demo bar is. A **linked** bar
//! (0.146.0, [`link_scrollbar`]) drives a [`WidgetTree::set_scrollable`]
//! container instead: [`set_scrollbar_value`] — and so a pointer drag, a
//! track press and an assistive technology's `SetValue`/`Increment`/
//! `Decrement`, which all go through it — scrolls that container, and
//! [`sync_linked_scrollbars`] (run by `crate::compute_text_layout` after
//! every layout) copies the container's offset, range and height back
//! into the bar and hides the bar (`Display::None`, so it takes no width)
//! while the content fits. The bar must **not** be a descendant of the
//! container it scrolls — a scroll offset moves every descendant, and the
//! bar would scroll away with the content; `aurora-ui`'s docked panels put
//! it beside the body in a row of its own.
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
//! a *scrollable container's* own state — and that is where it lives:
//! since 0.145.0 a [`WidgetTree::set_scrollable`] container reports
//! `scroll_y`/`scroll_y_min`/`scroll_y_max` itself, so a linked bar's
//! numeric value is the bar's own announcement, not a duplicate of them.
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
use taffy::{Display, Size, Style};

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
    /// `aria-controls`-shaped relationship, which a linked bar announces
    /// through [`Self::scrolls`] since 0.146.0) rather than by chrome of
    /// its own — but a bare bar with no such region (the gallery's demo
    /// bar) is otherwise announced as an unnamed "scroll bar", so a caller
    /// must be able to supply one.
    pub label: Option<String>,
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub page_size: f64,
    pub disabled: bool,
    /// The scroll container this bar drives, once [`link_scrollbar`] has
    /// linked it (0.146.0); `None` for a bare position model. Announced as
    /// the node's `controls` relation — the `aria-controls` shape the
    /// [`Self::label`] doc names.
    pub scrolls: Option<WidgetId>,
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
        // A linked bar is not a `Tab` stop (0.146.0), the same as a
        // platform's own scrollbars: a keyboard user reaches the scrolled
        // content by focus, and a focused row scrolls itself into view.
        // Its value actions stay, so an assistive technology can still
        // scroll with it.
        if state.scrolls.is_none() {
            node.add_action(Action::Focus);
        }
        node.add_action(Action::SetValue);
        node.add_action(Action::Increment);
        node.add_action(Action::Decrement);
    }
    if let Some(container) = state.scrolls {
        node.set_controls(vec![container]);
        // Nothing to scroll: the bar is not shown (`sync_scrollbar`), so
        // it is not announced either.
        if state.max <= state.min {
            node.set_hidden();
        }
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
///
/// **The other edge `flex_shrink: 0.0` doesn't cover**: a sized parent
/// that also flexes *along the scrolling axis* — a vertical bar sharing
/// a `Column` with other growing siblings, or a horizontal bar sharing
/// a `Row` — is not this function's scenario (its own scrolling axis is
/// always the parent's *cross* axis, by design), so `percent(1.0)`
/// there resolves against the cross size as intended and nothing
/// overflows. Only a caller who orients the scrollbar to run *along*
/// its parent's main axis hits it: `flex_shrink: 0.0` refuses to give
/// that percentage back up, and the bar can overflow a crowded parent
/// rather than yield space to its siblings. Not exercised by anything
/// in this crate today.
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
        scrolls: None,
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
/// **A linked bar ([`link_scrollbar`]) scrolls its container too**
/// (0.146.0), through [`WidgetTree::set_scroll_y`], and then stores — and
/// returns — the offset the container actually took, so the two can never
/// disagree after a call.
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
    let mut linked = None;
    with_scrollbar_mut(tree, id, |state| {
        if state.disabled {
            return Err(WidgetError::WidgetDisabled(id));
        }
        checked_range(state.min, state.max)?;
        state.value = clamped_value(value, state.min, state.max);
        result = state.value;
        linked = state.scrolls;
        Ok(())
    })?;
    if let Some(container) = linked {
        #[allow(clippy::cast_possible_truncation)]
        tree.set_scroll_y(container, result as f32)?;
        let taken = tree.scroll_y(container).map_or(result, f64::from);
        with_scrollbar_mut(tree, id, |state| {
            state.value = clamped_value(taken, state.min, state.max);
            result = state.value;
            Ok(())
        })?;
    }
    Ok(result)
}

/// Links `bar` (a scrollbar) to `container` (0.146.0): from now on
/// [`set_scrollbar_value`] scrolls `container`, and
/// [`sync_linked_scrollbars`] keeps the bar's value, range, page size and
/// visibility in line with it. The bar stops being a `Tab` stop and
/// announces `container` as the region it controls. Syncs once at once,
/// so a bar linked to a container that has not been laid out yet starts
/// hidden.
///
/// `bar` must not be a descendant of `container` — see this module's own
/// doc comment.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] if either id doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if `bar` isn't a scrollbar.
pub fn link_scrollbar(
    tree: &mut WidgetTree<WidgetKind>,
    bar: WidgetId,
    container: WidgetId,
) -> Result<(), WidgetError> {
    if !tree.contains(container) {
        return Err(WidgetError::UnknownWidget(container));
    }
    with_scrollbar_mut(tree, bar, |state| {
        state.scrolls = Some(container);
        Ok(())
    })?;
    sync_scrollbar(tree, bar)?;
    Ok(())
}

/// The scroll container `id` drives, if `id` is a linked scrollbar
/// ([`link_scrollbar`]); `None` for anything else.
#[must_use]
pub fn scrollbar_target(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> Option<WidgetId> {
    match tree.payload(id) {
        Some(WidgetKind::Scrollbar(state)) => state.scrolls,
        _ => None,
    }
}

/// Copies a linked `bar`'s container's state into the bar (0.146.0):
/// `value` = the container's [`WidgetTree::scroll_y`], `max` = its
/// [`WidgetTree::scroll_range`] (`min` is always `0.0`), `page_size` = its
/// laid-out height — [`ScrollbarRange`]'s own `max = content - page`
/// convention, which is exactly what `scroll_range` already is. The bar is
/// shown while `max > 0` and `Display::None` otherwise, so a panel whose
/// content fits gives the bar no width at all. A container that no longer
/// exists hides the bar. Never writes to the container: a collapsed body
/// keeps the offset it was collapsed at, with its bar hidden.
///
/// Returns whether the bar's **visibility** changed — the one change that
/// moves other widgets (the container gains or loses the bar's width), so
/// the caller lays out once more. A state-only change dirties the bar
/// for repaint and returns `false`. An unlinked bar is left alone.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] if `bar` doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if it isn't a scrollbar.
// Exact float equality is the intent: an unchanged value is the same
// stored number.
#[allow(clippy::float_cmp)]
pub fn sync_scrollbar(
    tree: &mut WidgetTree<WidgetKind>,
    bar: WidgetId,
) -> Result<bool, WidgetError> {
    sync_one(tree, bar, true)
}

/// [`sync_scrollbar`], with `may_toggle: false` leaving the bar's
/// visibility as it is and returning whether it *would* have changed.
#[allow(clippy::float_cmp)]
fn sync_one(
    tree: &mut WidgetTree<WidgetKind>,
    bar: WidgetId,
    may_toggle: bool,
) -> Result<bool, WidgetError> {
    let state = match tree.payload(bar) {
        Some(WidgetKind::Scrollbar(state)) => state,
        Some(_) => return Err(WidgetError::WrongWidgetKind(bar)),
        None => return Err(WidgetError::UnknownWidget(bar)),
    };
    let Some(container) = state.scrolls else {
        return Ok(false);
    };
    let (offset, max, page) = match (
        tree.scroll_y(container),
        tree.scroll_range(container),
        tree.bounds(container),
    ) {
        (Some(offset), Some(range), Some(bounds)) => (
            f64::from(offset),
            f64::from(range).max(0.0),
            f64::from(bounds.height),
        ),
        _ => (0.0, 0.0, 0.0),
    };
    let value = clamped_value(offset, 0.0, max);
    if state.min != 0.0 || state.max != max || state.page_size != page || state.value != value {
        with_scrollbar_mut(tree, bar, |state| {
            state.min = 0.0;
            state.max = max;
            state.page_size = page;
            state.value = value;
            Ok(())
        })?;
    }
    let display = if max > 0.0 {
        Display::Flex
    } else {
        Display::None
    };
    let mut style = tree
        .style(bar)
        .cloned()
        .ok_or(WidgetError::UnknownWidget(bar))?;
    if style.display == display {
        return Ok(false);
    }
    if !may_toggle {
        return Ok(true);
    }
    style.display = display;
    tree.set_style(bar, style)?;
    Ok(true)
}

/// [`sync_scrollbar`] for every linked scrollbar in `tree` (0.146.0) —
/// what `crate::compute_text_layout` runs after its first layout. Returns
/// whether any bar was shown or hidden, in which case that function lays
/// out once more and then calls [`settle_linked_scrollbars`], never a
/// third layout.
pub fn sync_linked_scrollbars(tree: &mut WidgetTree<WidgetKind>) -> bool {
    sync_all(tree, true)
}

/// The second, settling pass after [`sync_linked_scrollbars`] toggled a
/// bar and the tree was laid out again (0.146.0): every linked bar takes
/// its container's new value, range and page size, but **no bar is shown
/// or hidden** — a bar the first pass showed is never hidden by this one,
/// and the other way round — so the layout just computed stays the one
/// the bars are drawn in. Returns whether some bar *would* have toggled.
///
/// That happens only for content whose height **shrinks** as it narrows
/// (an aspect-ratio box, say): showing the bar narrows it until it fits,
/// or hiding the bar widens it until it overflows. The bar then stays as
/// the first pass left it — shown over a full-length thumb, or hidden over
/// still-scrollable content (the wheel and focus still scroll it) — and
/// the next layout may decide the other way, so such content can flip per
/// layout. Text, rows and every shipped panel's content only grow as they
/// narrow, for which this never happens.
pub fn settle_linked_scrollbars(tree: &mut WidgetTree<WidgetKind>) -> bool {
    sync_all(tree, false)
}

fn sync_all(tree: &mut WidgetTree<WidgetKind>, may_toggle: bool) -> bool {
    let bars: Vec<WidgetId> = tree
        .ids()
        .filter(|&id| scrollbar_target(tree, id).is_some())
        .collect();
    let mut toggled = false;
    for bar in bars {
        toggled |= matches!(sync_one(tree, bar, may_toggle), Ok(true));
    }
    toggled
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
    use super::{
        ScrollbarRange, insert_scrollbar, link_scrollbar, set_scrollbar_disabled,
        set_scrollbar_value,
    };
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

    // -- linked scrollbars (0.146.0) --

    /// A 200x300 window with a 100 px tall row `[body | bar]`: the body is
    /// a clipping scroll container holding `rows` 20 px rows, the bar is
    /// linked to it and is *not* its descendant.
    fn linked_scene(
        rows: usize,
    ) -> (
        crate::WidgetTree<WidgetKind>,
        crate::WidgetId,
        crate::WidgetId,
        Vec<crate::WidgetId>,
    ) {
        let (mut tree, root) = new_tree(Style {
            flex_direction: FlexDirection::Column,
            size: Size {
                width: length(200.0_f32),
                height: length(300.0_f32),
            },
            ..Default::default()
        });
        let ok = |result: Result<crate::WidgetId, WidgetError>| match result {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let viewport = ok(crate::widgets::insert_container(
            &mut tree,
            root,
            Style {
                flex_direction: FlexDirection::Row,
                flex_shrink: 0.0,
                size: Size {
                    width: length(200.0_f32),
                    height: length(100.0_f32),
                },
                ..Default::default()
            },
        ));
        let body = ok(crate::widgets::insert_container(
            &mut tree,
            viewport,
            Style {
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                flex_basis: length(0.0_f32),
                overflow: taffy::Point {
                    x: taffy::Overflow::Hidden,
                    y: taffy::Overflow::Hidden,
                },
                ..Default::default()
            },
        ));
        if let Err(err) = tree.set_scrollable(body, true) {
            unreachable!("{err:?}");
        }
        let bar = ok(insert_scrollbar(
            &mut tree,
            viewport,
            &test_scales(),
            Orientation::Vertical,
            None,
            0.0,
            ScrollbarRange {
                min: 0.0,
                max: 0.0,
                page_size: 0.0,
            },
        ));
        if let Err(err) = link_scrollbar(&mut tree, bar, body) {
            unreachable!("{err:?}");
        }
        let rows = (0..rows)
            .map(|_| {
                ok(crate::widgets::insert_container(
                    &mut tree,
                    body,
                    Style {
                        flex_shrink: 0.0,
                        size: Size {
                            width: taffy::Dimension::auto(),
                            height: length(20.0_f32),
                        },
                        ..Default::default()
                    },
                ))
            })
            .collect();
        crate::compute_text_layout(&mut tree, 200.0, 300.0, None);
        (tree, body, bar, rows)
    }

    fn bar_state(
        tree: &crate::WidgetTree<WidgetKind>,
        bar: crate::WidgetId,
    ) -> super::ScrollbarState {
        match tree.payload(bar) {
            Some(WidgetKind::Scrollbar(state)) => state.clone(),
            other => unreachable!("not a scrollbar: {other:?}"),
        }
    }

    fn rect(tree: &crate::WidgetTree<WidgetKind>, id: crate::WidgetId) -> Rect {
        match tree.bounds(id) {
            Some(rect) => rect,
            None => unreachable!("laid out"),
        }
    }

    #[test]
    fn a_linked_bar_is_hidden_takes_no_width_and_is_announced_hidden_while_content_fits() {
        let (tree, body, bar, rows) = linked_scene(3);
        assert_eq!(tree.scroll_range(body), Some(0.0));
        assert_eq!(rect(&tree, bar).width, 0, "no width at all");
        assert_eq!(rect(&tree, body).width, 200, "the body takes the whole row");
        let Some(&row) = rows.first() else {
            unreachable!("three rows");
        };
        assert_eq!(rect(&tree, row).width, 200);
        let Some(node) = tree.accessibility(bar) else {
            unreachable!("inserted");
        };
        assert!(node.is_hidden());
        assert_eq!(node.role(), accesskit::Role::ScrollBar);
        assert_eq!(node.controls(), &[body]);
        assert!(!node.supports_action(Action::Focus), "not a Tab stop");
        assert!(node.supports_action(Action::SetValue));
    }

    #[test]
    fn an_overflowing_container_shows_its_bar_with_a_proportional_thumb_and_rows_lose_its_width() {
        let (tree, body, bar, rows) = linked_scene(10);
        let thickness = rect(&tree, bar).width;
        assert!(thickness > 0, "shown");
        let bar_box = rect(&tree, bar);
        assert_eq!(bar_box.height, 100, "the bar spans the body's height");
        assert_eq!(bar_box.x, i64::from(200 - thickness), "at the right edge");
        // Showing the bar narrowed the body: one re-layout, inside
        // `compute_text_layout`, gave the rows their new width.
        assert_eq!(rect(&tree, body).width, 200 - thickness);
        for &row in &rows {
            assert_eq!(rect(&tree, row).width, 200 - thickness);
        }
        let state = bar_state(&tree, bar);
        assert_eq!(tree.scroll_range(body), Some(100.0));
        assert!((state.max - 100.0).abs() < 1e-9);
        assert!((state.page_size - 100.0).abs() < 1e-9);
        assert!(state.value.abs() < 1e-9);
        let (_, top, _, thumb) = crate::paint::scrollbar_thumb_rect(&state, bar_box);
        assert!(
            (thumb - 50.0).abs() < 1.0,
            "page / content = 100 / 200: {thumb}"
        );
        assert!((top - 0.0).abs() < 1.0, "at the top: {top}");
        let Some(node) = tree.accessibility(bar) else {
            unreachable!("inserted");
        };
        assert!(!node.is_hidden());
    }

    #[test]
    fn the_bar_follows_the_containers_offset_after_layout_and_never_moves_itself() {
        let (mut tree, body, bar, _) = linked_scene(10);
        let before = rect(&tree, bar);
        if let Err(err) = tree.set_scroll_y(body, 100.0) {
            unreachable!("{err:?}");
        }
        crate::compute_text_layout(&mut tree, 200.0, 300.0, None);
        let state = bar_state(&tree, bar);
        assert!((state.value - 100.0).abs() < 1e-9, "{}", state.value);
        assert_eq!(rect(&tree, bar), before, "the bar is not scrolled content");
        let (_, top, _, thumb) = crate::paint::scrollbar_thumb_rect(&state, before);
        assert!(
            (top + thumb - 100.0).abs() < 1.0,
            "thumb at the bottom: {top}+{thumb}"
        );
    }

    #[test]
    fn setting_a_linked_bars_value_scrolls_its_container_and_moves_the_rows() {
        let (mut tree, body, bar, rows) = linked_scene(10);
        let Some(&row0) = rows.first() else {
            unreachable!("ten rows");
        };
        let stored = match set_scrollbar_value(&mut tree, bar, 40.0) {
            Ok(stored) => stored,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!((stored - 40.0).abs() < 1e-9);
        assert_eq!(tree.scroll_y(body), Some(40.0));
        assert_eq!(
            rect(&tree, row0).y,
            -40,
            "content moved before any relayout"
        );
        // Past the end: the container's own clamp wins and the bar takes it.
        let stored = match set_scrollbar_value(&mut tree, bar, 1.0e6) {
            Ok(stored) => stored,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!((stored - 100.0).abs() < 1e-9);
        assert_eq!(tree.scroll_y(body), Some(100.0));
    }

    #[test]
    fn accessibility_value_actions_on_a_linked_bar_scroll_its_container() {
        let (mut tree, body, bar, _) = linked_scene(10);
        let mut focus = crate::FocusManager::new();
        let request = |action, data| accesskit::ActionRequest {
            action,
            target_tree: crate::ACCESSIBILITY_TREE_ID,
            target_node: bar,
            data,
        };
        if let Err(err) = crate::handle_action(
            &mut tree,
            &mut focus,
            &request(
                Action::SetValue,
                Some(accesskit::ActionData::NumericValue(30.0)),
            ),
        ) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.scroll_y(body), Some(30.0));
        if let Err(err) =
            crate::handle_action(&mut tree, &mut focus, &request(Action::Increment, None))
        {
            unreachable!("{err:?}");
        }
        assert!(
            tree.scroll_y(body).is_some_and(|y| y > 30.0),
            "{:?}",
            tree.scroll_y(body)
        );
        if let Err(err) =
            crate::handle_action(&mut tree, &mut focus, &request(Action::Decrement, None))
        {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.scroll_y(body), Some(30.0));
        assert!(
            crate::handle_action(&mut tree, &mut focus, &request(Action::Focus, None)).is_err(),
            "a linked bar is not focusable"
        );
    }

    #[test]
    fn a_pointer_press_on_a_linked_bars_track_scrolls_without_taking_focus() {
        let (mut tree, body, bar, _) = linked_scene(10);
        let mut focus = crate::FocusManager::new();
        let mut click = crate::ClickTracker::default();
        let bar_box = rect(&tree, bar);
        #[allow(clippy::cast_precision_loss)]
        let x = bar_box.x as f32 + 1.0;
        let event = |phase, y| crate::PointerEvent {
            phase,
            position: (x, y),
        };
        if let Err(err) = crate::handle_pointer(
            &mut tree,
            &mut focus,
            &mut click,
            event(crate::PointerPhase::Down, 99.0),
        ) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            tree.scroll_y(body),
            Some(100.0),
            "a track press at the bottom"
        );
        assert_eq!(focus.focused(), None);
        if let Err(err) = crate::handle_pointer(
            &mut tree,
            &mut focus,
            &mut click,
            event(crate::PointerPhase::Move, 50.0),
        ) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            tree.scroll_y(body),
            Some(50.0),
            "dragging the thumb to the middle"
        );
    }

    /// Review I-2 (0.146.0): the settling pass never shows or hides a bar,
    /// so the layout it follows stays the one the bar is drawn in.
    #[test]
    fn the_settling_pass_updates_a_bars_state_but_never_its_visibility() {
        let (mut tree, body, bar, rows) = linked_scene(10);
        assert!(rect(&tree, bar).width > 0, "shown");
        for &row in rows.iter().skip(2) {
            if let Err(err) = tree.remove(row) {
                unreachable!("{err:?}");
            }
        }
        tree.compute_layout(200.0, 300.0);
        assert_eq!(tree.scroll_range(body), Some(0.0), "now it fits");
        assert!(super::settle_linked_scrollbars(&mut tree), "it would hide");
        assert_eq!(
            tree.style(bar).map(|style| style.display),
            Some(taffy::Display::Flex),
            "but the settling pass leaves it shown"
        );
        assert!(bar_state(&tree, bar).max.abs() < 1e-9, "its state did sync");
        assert!(
            super::sync_linked_scrollbars(&mut tree),
            "the first pass hides it"
        );
        assert_eq!(
            tree.style(bar).map(|style| style.display),
            Some(taffy::Display::None)
        );
    }
}
