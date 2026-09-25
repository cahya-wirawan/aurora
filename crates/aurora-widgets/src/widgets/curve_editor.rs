//! A curve editor: a square plot of an `aurora_core::ToneCurve` — the
//! input level along `x`, the output level up `y` — whose control points
//! can be selected, moved, added and removed from the keyboard and the
//! pointer. The eleventh of the 12 widgets PLAN.md's gallery list names;
//! only the number field remains.
//!
//! **Scope: one curve, one channel.** There is **no** channel selector
//! (RGB / red / green / blue), **no** histogram behind the plot, **no**
//! input/output numeric fields, **no** presets, and no glyphs of any kind
//! (this crate draws none). The curve model itself — validation,
//! Fritsch–Carlson interpolation, editing — is [`ToneCurve`] in
//! `aurora-core`, so the adjustment that will one day *apply* a curve
//! shares it without depending on this crate; this module owns only the
//! interaction and the tree.
//!
//! **Shape: a pure state machine plus a structural reconcile**, the
//! same split `tab_bar.rs` and `color_picker.rs` use.
//! [`CurveEditorState`] owns the curve and the selection; every key,
//! pointer gesture and setter is a transition on that state alone. After
//! every successful call — a no-op included — the tree is reconciled:
//! if the editor's point children are not exactly the tracked ids, one
//! per curve point, they are all rebuilt; otherwise each point's
//! **whole** payload and accessibility node is compared with what the
//! state says and only a disagreeing one is rewritten and dirtied. The
//! root is dirtied whenever its painted state (curve, selection,
//! disabled) changed or its node disagrees. So moving a point damages
//! that point and the root, and echoing the current points back damages
//! nothing.
//!
//! **Point ids are not stable across adding or removing a point.** A
//! point count change (an insert, a delete, an owner setter with a
//! different number of points) and any structural repair remove every
//! editor-owned point child and build them all afresh under **new** ids.
//! Moving a point, or changing the selection, keeps them. Re-read
//! [`CurveEditorState::focus_target`] (and [`CurveEditorState::point_id`])
//! after any add, delete, owner update or outside change to the subtree —
//! a caller's `FocusManager` holding the old id is holding a removed
//! widget.
//!
//! **Do not add children under the editor.** A foreign child is left
//! alone (ownership is decided by the tracked ids plus the editor's own
//! point kind, never by any other kind), but it takes part in layout and
//! is painted over by nothing.
//!
//! # Structure
//!
//! ```text
//! CurveEditor (Role::Group, labelled, size x size) — paints everything
//! └── CurveEditorPoint x n (Role::Slider, vertical, inset 0) — paints nothing
//! ```
//!
//! # Accessibility
//!
//! Checked against the pinned `accesskit` 0.24.1 (no curve or
//! two-dimensional role) and `accesskit_consumer` 0.38.0. Each control
//! point is a vertical **slider** over the output level, named
//! `"Point k"` (1-based), with `numeric_value` = the output on a
//! `0..=255` scale rounded to one decimal, `min` `0`, `max` `255`, step
//! `1`, jump `10`, and a `value` string `"Input X, output Y"` (both on
//! the same scale, one decimal, built with `format!`) so the input level
//! reaches a screen reader at all. The `0..=255` scale follows the 8-bit
//! levels a Curves dialog conventionally shows; the colour picker uses
//! `0..=100` percentages instead — which one Aurora standardises on is a
//! design-owner question, flagged. **Whether any platform adapter
//! announces a slider's `value` string alongside its numeric value has
//! not been checked** (it reaches `accesskit_consumer`, which is all the
//! tests here prove).
//!
//! **Every point slider's accessible bounds are the whole editor**
//! (each child is an absolutely positioned full-size overlay), so an
//! assistive technology that locates or highlights elements spatially
//! cannot tell the points apart by position — the `"Input X, output
//! Y"` value string is the only positional information it gets.
//! Per-point bounds (a small rect around each marker) would fix that and
//! are a named follow-on, not done here.
//!
//! **Roving focus, one tab stop**: only the **selected** point declares
//! `Action::Focus` (the `tab_bar.rs` precedent), so the selection *is*
//! the focus. Every enabled point declares `SetValue`, `Increment` and
//! `Decrement`; a disabled editor's points declare none and are marked
//! disabled. **Accessibility `ActionRequest`s are not routed** — the same
//! crate-wide gap every widget here has.
//!
//! # Keys
//!
//! [`CurveEditorKey`] is this module's own vocabulary. The arrows,
//! `Delete` **and `Backspace`** (both for [`CurveEditorKey::Delete`] —
//! a Mac keyboard's "delete" key sends `Backspace`) and `Enter` (for
//! [`CurveEditorKey::Insert`]) are bridged from `shortcut::NamedKey` by
//! [`CurveEditorKey::from_named_key`]. `Enter` conventionally activates
//! a dialog's default button, so an editor inside a Curves dialog
//! shadows that while it holds focus — which key should add a point is a
//! **design-owner question**, flagged, not settled here;
//! **[`CurveEditorKey::PreviousPoint`] and [`CurveEditorKey::NextPoint`]
//! are not** — `NamedKey` has no key this crate could claim for them
//! without taking one from focus traversal, so the caller maps its own
//! (the same division `coarse` has). `coarse` selects the large step.
//!
//! | key | condition | result |
//! |---|---|---|
//! | any | disabled | `Err(WidgetDisabled)`, nothing changes |
//! | `PreviousPoint`/`NextPoint` | not at the first/last point | selection −/+ 1 (the focus target moves) |
//! | `PreviousPoint`/`NextPoint` | at the first/last point | `Ignored` (no wrap) |
//! | `Up`/`Down` | output not at `1`/`0` | output +/− 1/255 (coarse 10/255), snapped, clamped to `[0, 1]` |
//! | `Left`/`Right` | interior point | input −/+ 1/255 (coarse 10/255), snapped, clamped between its neighbours |
//! | `Left`/`Right` | an endpoint | `Ignored` (endpoints never move horizontally) |
//! | `Delete` | interior point selected | removed; the previous point is selected |
//! | `Delete` | an endpoint selected | `Ignored` |
//! | `Insert` | the segment after the selection has room | a point at that segment's midpoint input, **on** the curve, selected |
//! | `Insert` | that segment is under `2/256` wide | the same, in the **widest** segment of the curve (lowest index on a tie) |
//! | `Insert` | 16 points | `Ignored` |
//! | any | a key that changes nothing | `Ignored`, no damage |
//!
//! `Insert` with the **last** point selected uses the segment before it
//! (there is none after). The widest-segment fallback exists because
//! "the segment after the selection" alone stalls: repeated `Insert`
//! from the identity selects each new point and halves the segment after
//! it, reaching a `1/256`-wide segment at only 10 points. The fallback
//! was chosen over "the wider of the selected point's two neighbouring
//! segments" because that one stalls identically (both neighbours are
//! the same halved width); its cost is that the new point, and so the
//! selection and focus, may land away from the old selection.
//!
//! A step **snaps directionally**, exactly as the
//! colour picker's does: to the next `1/255` (or `10/255`) grid point
//! strictly past the current value in the key's direction, tolerating an
//! `f32` grid value's own representation error — so an off-grid point
//! lands on the grid and no key moves further than one step. The step
//! sizes are this module's documented defaults, not design tokens (none
//! exist). A horizontal step is then clamped to stay
//! `aurora_core::MIN_POINT_SEPARATION` (`1/256`) from each neighbour —
//! a separation **smaller** than the `1/255` step, so a clamped point can
//! sit off the `1/255` grid; its next key press snaps it back on.
//!
//! # Pointer
//!
//! The **plot rectangle** is the root's **unclipped** layout bounds
//! inset on every side by the marker's reach (`spacing.xs` plus the
//! ring width), so a marker at an extreme stays inside the widget. The
//! curve, the grid, the diagonal, the markers, the pointer mapping and
//! the hit test all use that one rectangle — a marker *is* the data it
//! draws, so drawing and mapping must agree (the colour picker instead
//! maps through the full part and clamps its marker; a different
//! choice for a different job). Input is `(px - left) / width`, output
//! `1 - (py - top) / height`, clamped to `[0, 1]` for gestures.
//! [`curve_editor_point_at`] resolves a point **geometrically** — the
//! nearest control point within `spacing.sm` pixels — rather than through
//! `WidgetTree::hit_test`, because every point slider covers the whole
//! editor and a tree hit test would pick one arbitrarily. A non-finite
//! point, or a plot with no area (a tiny `size`, or never laid out), is
//! `Ignored` / `None`. There is no pointer-capture state: a drag is the
//! caller calling [`move_selected_point_from_point`] on every move.
//! A drag puts the point's **centre** under the pointer — there is no
//! grab offset, so pressing a marker off-centre makes it jump by up to
//! the hit radius on the first move; a caller that wants the offset
//! kept records it at the press and subtracts it from each move point.
//!
//! **Clipped editors:** painting drops every stroke (the curve, the
//! markers, the well's border) once a clipping ancestor cuts the
//! editor's rect at all, and the surviving well fill is re-rounded at the
//! cut edge (the same as every other clipped fill in `paint.rs`) — but
//! the pointer mapping and [`curve_editor_point_at`] still use the
//! **unclipped** bounds, so a point hidden by the clip can still be hit
//! and dragged. A caller must gate pointer input on the point being
//! visible (e.g. its own clip rect) before calling these functions.
//!
//! # Size
//!
//! `size` (the plot's side) is caller-supplied, the precedent
//! `insert_color_picker` sets: no "curve editor size" token exists. It
//! must be finite and within `1.0..=1.0e6` logical pixels.
//!
//! # Robustness
//!
//! Two misuse paths are **unsupported**, the same stance every other
//! widget in this crate takes towards writing its payloads by hand:
//! writing a `WidgetKind::CurveEditor` payload onto one of the editor's
//! point children (driving that "nested editor" grows point sliders
//! under a point; the outer editor's next reconcile rewrites the point's
//! payload back **and removes any children found under a point**, so
//! nothing is left orphaned, but whatever the caller did through the
//! nested editor is lost), and writing another editor's
//! `CurveEditorState` snapshot onto this editor's root (its label, size
//! and tracked ids replace this editor's own, and the next reconcile
//! rebuilds the points from it). Neither can happen through this
//! module's functions.
//!
//! # What it deliberately does not do
//!
//! No keyboard-focus ring (crate-wide gap), no text, no histogram, no
//! channel selection, no pointer capture, no undo (the owner's history
//! records curve changes), and no hover state.

use accesskit::{Action, Node, Orientation, Role};
use aurora_core::{CurvePoint, MIN_POINT_SEPARATION, Rect, ToneCurve};
use aurora_theme::Scales;
use taffy::style_helpers::{length, zero};
use taffy::{AlignItems, Position, Rect as LayoutRect, Size, Style};

use super::{WidgetKind, spacing};
use crate::error::WidgetError;
use crate::shortcut::NamedKey;
use crate::tree::{WidgetId, WidgetTree};

/// The fine key step: one 8-bit level.
const STEP: f64 = 1.0 / 255.0;
/// The coarse key step: ten 8-bit levels.
const COARSE_STEP: f64 = 10.0 / 255.0;
/// The smallest `size` [`insert_curve_editor`] accepts, in logical pixels.
const MIN_SIZE: f32 = 1.0;
/// The largest `size` [`insert_curve_editor`] accepts, in logical pixels
/// — the colour picker's own bound, for the same `f32`-exactness reason.
const MAX_SIZE: f32 = 1.0e6;
/// The accessibility scale a point's levels are exposed on (`0..=255`).
const LEVELS: f64 = 255.0;

/// The width of each of a marker's two rings, in logical pixels — the
/// colour picker's own marker ring width. **Not a token**:
/// `design/tokens/scales.toml` has no stroke-weight scale, the same gap
/// every `BORDER_WIDTH` in `paint.rs` records. Shared with `paint.rs`
/// because the plot inset (and so the pointer mapping) depends on it.
pub(crate) const MARKER_RING_WIDTH: f32 = 1.0;

/// The keys a curve editor responds to — see this module's own doc
/// comment for the transition table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CurveEditorKey {
    /// Select the previous point (no wrap). Not bridged from `NamedKey`.
    PreviousPoint,
    /// Select the next point (no wrap). Not bridged from `NamedKey`.
    NextPoint,
    Left,
    Right,
    Up,
    Down,
    /// Remove the selected interior point (bridged from `Delete` and
    /// `Backspace`).
    Delete,
    /// Add a point after the selected one — or, when that segment has
    /// no room, in the widest segment (bridged from `Enter`).
    Insert,
}

impl CurveEditorKey {
    /// The editor key `key` stands for, if any: the four arrows,
    /// `Delete` and `Backspace` as [`Self::Delete`], and `Enter` as
    /// [`Self::Insert`]. `None` for everything
    /// else — including every key the caller might choose for
    /// [`Self::PreviousPoint`]/[`Self::NextPoint`].
    #[must_use]
    pub fn from_named_key(key: NamedKey) -> Option<Self> {
        match key {
            NamedKey::ArrowLeft => Some(Self::Left),
            NamedKey::ArrowRight => Some(Self::Right),
            NamedKey::ArrowUp => Some(Self::Up),
            NamedKey::ArrowDown => Some(Self::Down),
            NamedKey::Delete | NamedKey::Backspace => Some(Self::Delete),
            NamedKey::Enter => Some(Self::Insert),
            _ => None,
        }
    }
}

/// What a transition did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurveEditorOutcome {
    /// The curve, the selection or the disabled state changed.
    Changed,
    /// Nothing changed at all.
    Ignored,
}

/// Layout and geometry resolved at insert and carried in the state,
/// because the pointer functions and a lazy repair take no `&Scales` —
/// and paint must use exactly the numbers the pointer mapping does.
#[derive(Debug, Clone, Copy, PartialEq)]
struct EditorMetrics {
    /// The caller's `size`: the editor's side.
    size: f32,
    /// `spacing.xs`: a marker's outer radius.
    marker_radius: f32,
    /// `spacing.sm`: how far from a marker's centre a pointer still hits it.
    hit_radius: f32,
}

/// A curve editor's own state — the payload of its root
/// ([`WidgetKind::CurveEditor`]).
///
/// Kept in lockstep with the real point children. `Clone`, so a caller
/// can snapshot it; writing a stale snapshot back is tolerated (the next
/// successful call repairs the structure) but is not a supported way to
/// change an editor.
#[derive(Debug, Clone, PartialEq)]
pub struct CurveEditorState {
    curve: ToneCurve,
    selected: usize,
    disabled: bool,
    label: String,
    metrics: EditorMetrics,
    point_ids: Vec<WidgetId>,
}

impl CurveEditorState {
    /// The curve being edited.
    #[must_use]
    pub fn curve(&self) -> &ToneCurve {
        &self.curve
    }

    /// The selected point's index — always a valid index into
    /// `curve().points()`.
    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    #[must_use]
    pub fn disabled(&self) -> bool {
        self.disabled
    }

    /// The editor's own accessible name.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The caller's `size`, in logical pixels.
    #[must_use]
    pub fn size(&self) -> f32 {
        self.metrics.size
    }

    /// Point `index`'s slider. **Not stable across an add, a delete or a
    /// structural repair** — see this module's own doc comment.
    #[must_use]
    pub fn point_id(&self, index: usize) -> Option<WidgetId> {
        self.point_ids.get(index).copied()
    }

    /// The editor's one tab stop: the selected point's slider. Re-read it
    /// after any add, delete or outside change.
    #[must_use]
    pub fn focus_target(&self) -> Option<WidgetId> {
        self.point_id(self.selected)
    }

    /// A marker's outer radius (`spacing.xs` at insert), for `paint.rs`.
    pub(crate) fn marker_radius(&self) -> f32 {
        self.metrics.marker_radius
    }

    fn set_selected(&mut self, index: usize) -> CurveEditorOutcome {
        if index == self.selected {
            return CurveEditorOutcome::Ignored;
        }
        self.selected = index;
        CurveEditorOutcome::Changed
    }

    fn set_curve(&mut self, curve: ToneCurve, selected: usize) -> CurveEditorOutcome {
        if curve == self.curve && selected == self.selected {
            return CurveEditorOutcome::Ignored;
        }
        self.curve = curve;
        self.selected = selected;
        CurveEditorOutcome::Changed
    }

    fn apply_key(
        &mut self,
        id: WidgetId,
        key: CurveEditorKey,
        coarse: bool,
    ) -> Result<CurveEditorOutcome, WidgetError> {
        if self.disabled {
            return Err(WidgetError::WidgetDisabled(id));
        }
        let (curve, selected) = keyed(&self.curve, self.selected, key, coarse);
        Ok(self.set_curve(curve, selected))
    }
}

/// How far (in grid steps) a value may sit from a grid point and still
/// count as on it — the colour picker's own tolerance: far above an
/// `f32` grid value's representation error, far below any real off-grid
/// distance worth honouring.
const GRID_TOLERANCE: f64 = 1e-4;

/// `x` moved to the next grid point strictly past it in `direction`
/// (`-1.0` or `+1.0`), clamped to `[0, 1]` — see "Keys" in this module's
/// own doc comment. Never more than one `step`.
fn step(x: f32, step: f64, direction: f64) -> f32 {
    let units = f64::from(x) / step;
    let index = if direction > 0.0 {
        (units + GRID_TOLERANCE).floor() + 1.0
    } else {
        (units - GRID_TOLERANCE).ceil() - 1.0
    };
    ((index * step) as f32).clamp(0.0, 1.0)
}

/// The transition table for one key — pure, no tree. Returns the next
/// curve and selection (equal to the inputs when the key is `Ignored`).
fn keyed(
    curve: &ToneCurve,
    selected: usize,
    key: CurveEditorKey,
    coarse: bool,
) -> (ToneCurve, usize) {
    let by = if coarse { COARSE_STEP } else { STEP };
    let points = curve.points();
    let last = points.len().saturating_sub(1);
    let unchanged = (curve.clone(), selected);
    let Some(&point) = points.get(selected) else {
        return unchanged;
    };
    let mut next = curve.clone();
    match key {
        CurveEditorKey::PreviousPoint => (curve.clone(), selected.saturating_sub(1)),
        CurveEditorKey::NextPoint => (curve.clone(), (selected + 1).min(last)),
        CurveEditorKey::Up | CurveEditorKey::Down => {
            let direction = if key == CurveEditorKey::Up { 1.0 } else { -1.0 };
            let y = step(point.y, by, direction);
            match next.move_point_to(selected, point.x, y) {
                Ok(true) => (next, selected),
                _ => unchanged,
            }
        }
        CurveEditorKey::Left | CurveEditorKey::Right => {
            if selected == 0 || selected == last {
                return unchanged;
            }
            let direction = if key == CurveEditorKey::Right {
                1.0
            } else {
                -1.0
            };
            let x = step(point.x, by, direction);
            match next.move_point_to(selected, x, point.y) {
                Ok(true) => (next, selected),
                _ => unchanged,
            }
        }
        CurveEditorKey::Delete => match next.remove_point(selected) {
            Ok(_) => (next, selected - 1),
            Err(_) => unchanged,
        },
        CurveEditorKey::Insert => {
            let Some((a, b)) = insert_segment(points, selected) else {
                return unchanged;
            };
            let mid = f64::midpoint(f64::from(a.x), f64::from(b.x)) as f32;
            match next.add_point(mid) {
                Ok(index) => (next, index),
                Err(_) => unchanged,
            }
        }
    }
}

/// Whether the segment from `a` to `b` can take a midpoint that stays
/// [`MIN_POINT_SEPARATION`] from both ends.
fn has_room(a: CurvePoint, b: CurvePoint) -> bool {
    b.x - a.x >= 2.0 * MIN_POINT_SEPARATION
}

/// The segment [`CurveEditorKey::Insert`] splits, as its two end
/// points: the one after `selected` (before it, for the last point) if
/// it has room, otherwise the **widest** segment of the whole curve
/// (the lowest-indexed one on a tie). Without the fallback, repeated
/// `Insert` from the identity halves the same run of segments until the
/// next one is `1/256` wide and stalls at 10 points; with it, `Insert`
/// is refused only at [`aurora_core::MAX_POINTS`] — below the limit
/// there are at most fourteen segments spanning `[0, 1]`, so the widest
/// is at least `1/14` wide, far above `2/256`. `None`
/// when no segment has room (only reachable at the point limit).
fn insert_segment(points: &[CurvePoint], selected: usize) -> Option<(CurvePoint, CurvePoint)> {
    let last = points.len().checked_sub(1)?;
    let from = if selected >= last {
        last.checked_sub(1)?
    } else {
        selected
    };
    let (&a, &b) = (points.get(from)?, points.get(from + 1)?);
    if has_room(a, b) {
        return Some((a, b));
    }
    points
        .windows(2)
        .filter_map(|w| match w {
            [a, b] => Some((*a, *b)),
            _ => None,
        })
        .fold(
            None,
            |widest: Option<(CurvePoint, CurvePoint)>, (a, b)| match widest {
                Some((wa, wb)) if wb.x - wa.x >= b.x - a.x => Some((wa, wb)),
                _ => Some((a, b)),
            },
        )
        .filter(|&(a, b)| has_room(a, b))
}

/// `x` rounded to one decimal place, for an accessibility number.
fn one_decimal(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

fn root_node(state: &CurveEditorState) -> Node {
    let mut node = Node::new(Role::Group);
    node.set_label(state.label.clone());
    if state.disabled {
        node.set_disabled();
    }
    node
}

/// One control point's slider — see "Accessibility" in this module's own
/// doc comment.
fn point_node(index: usize, point: CurvePoint, selected: bool, disabled: bool) -> Node {
    let input = one_decimal(f64::from(point.x) * LEVELS);
    let output = one_decimal(f64::from(point.y) * LEVELS);
    let mut node = Node::new(Role::Slider);
    node.set_label(format!("Point {}", index.saturating_add(1)));
    node.set_orientation(Orientation::Vertical);
    node.set_numeric_value(output);
    node.set_min_numeric_value(0.0);
    node.set_max_numeric_value(LEVELS);
    node.set_numeric_value_step(1.0);
    node.set_numeric_value_jump(10.0);
    node.set_value(format!("Input {input:.1}, output {output:.1}"));
    if disabled {
        node.set_disabled();
    } else {
        if selected {
            // Roving focus: only the selected point is a tab stop.
            node.add_action(Action::Focus);
        }
        node.add_action(Action::SetValue);
        node.add_action(Action::Increment);
        node.add_action(Action::Decrement);
    }
    node
}

/// The payload of one of an editor's own point sliders
/// ([`WidgetKind::CurveEditorPoint`]). Created only by this module; it
/// paints nothing (the editor's root draws every marker).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurveEditorPointState {
    index: usize,
    count: usize,
    point: CurvePoint,
    selected: bool,
    endpoint: bool,
    disabled: bool,
}

impl CurveEditorPointState {
    /// This point's index in the curve.
    #[must_use]
    pub fn index(&self) -> usize {
        self.index
    }

    /// How many points the curve has.
    #[must_use]
    pub fn count(&self) -> usize {
        self.count
    }

    /// The point itself.
    #[must_use]
    pub fn point(&self) -> CurvePoint {
        self.point
    }

    #[must_use]
    pub fn is_selected(&self) -> bool {
        self.selected
    }

    /// Whether this is the first or last point (which never moves
    /// horizontally and cannot be removed).
    #[must_use]
    pub fn is_endpoint(&self) -> bool {
        self.endpoint
    }

    #[must_use]
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }
}

fn point_payload(state: &CurveEditorState, index: usize, point: CurvePoint) -> WidgetKind {
    let count = state.curve.points().len();
    WidgetKind::CurveEditorPoint(CurveEditorPointState {
        index,
        count,
        point,
        selected: index == state.selected,
        endpoint: index == 0 || index + 1 == count,
        disabled: state.disabled,
    })
}

/// The editor: `size` by `size`, never stretched or shrunk by its
/// parent.
fn root_style(metrics: &EditorMetrics) -> Style {
    Style {
        flex_shrink: 0.0,
        align_self: Some(AlignItems::FLEX_START),
        size: Size {
            width: length(metrics.size),
            height: length(metrics.size),
        },
        ..Default::default()
    }
}

/// A point slider: absolutely positioned, inset 0 — the editor's box.
fn point_style() -> Style {
    Style {
        position: Position::Absolute,
        inset: LayoutRect {
            left: zero(),
            right: zero(),
            top: zero(),
            bottom: zero(),
        },
        ..Default::default()
    }
}

/// The plot rectangle `(left, top, width, height)` for an editor whose
/// unclipped bounds are `bounds` — see "Pointer" in this module's own doc
/// comment. `None` when it has no area. `paint.rs` draws through exactly
/// this function.
pub(crate) fn plot_rect(bounds: Rect, marker_radius: f32) -> Option<(f32, f32, f32, f32)> {
    let reach = marker_radius + MARKER_RING_WIDTH;
    let width = bounds.width as f32 - 2.0 * reach;
    let height = bounds.height as f32 - 2.0 * reach;
    if !(width > 0.0 && height > 0.0 && reach.is_finite()) {
        return None;
    }
    Some((
        bounds.x as f32 + reach,
        bounds.y as f32 + reach,
        width,
        height,
    ))
}

/// `(x, y)` in curve space for the pointer at `(px, py)`, **unclamped**
/// — `None` for a non-finite point or an editor with no plot.
fn curve_space(
    tree: &WidgetTree<WidgetKind>,
    editor: WidgetId,
    state: &CurveEditorState,
    px: f32,
    py: f32,
) -> Option<(f64, f64)> {
    if !(px.is_finite() && py.is_finite()) {
        return None;
    }
    let (left, top, width, height) = plot_rect(tree.bounds(editor)?, state.metrics.marker_radius)?;
    let x = (f64::from(px) - f64::from(left)) / f64::from(width);
    let y = 1.0 - (f64::from(py) - f64::from(top)) / f64::from(height);
    Some((x, y))
}

/// Adds a new, enabled curve editor showing `curve` (its first point
/// selected) as the last child of `parent`. `size` is the editor's side
/// — see "Size" in this module's own doc comment. `scales` supplies the
/// marker radius (`spacing.xs`) and hit radius (`spacing.sm`), fixed at
/// insert so paint and the pointer mapping always agree.
///
/// # Errors
///
/// Returns [`WidgetError::InvalidRange`] (carrying `1.0..=1.0e6`) if
/// `size` is not finite or lies outside it, or
/// [`WidgetError::UnknownWidget`] if `parent` doesn't exist. Nothing is
/// added when either happens.
pub fn insert_curve_editor(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
    label: &str,
    size: f32,
    curve: ToneCurve,
) -> Result<WidgetId, WidgetError> {
    if !(size.is_finite() && (MIN_SIZE..=MAX_SIZE).contains(&size)) {
        return Err(WidgetError::InvalidRange {
            min: f64::from(MIN_SIZE),
            max: f64::from(MAX_SIZE),
        });
    }
    let state = CurveEditorState {
        curve,
        selected: 0,
        disabled: false,
        label: label.to_owned(),
        metrics: EditorMetrics {
            size,
            marker_radius: spacing(scales.spacing.xs),
            hit_radius: spacing(scales.spacing.sm),
        },
        point_ids: Vec::new(),
    };
    let editor = tree.insert(
        parent,
        root_style(&state.metrics),
        root_node(&state),
        WidgetKind::CurveEditor(state),
    )?;
    if let Err(err) = rebuild_points(tree, editor) {
        // Unreachable (`editor` was just inserted), but never leave a
        // half-built editor behind.
        let _ = tree.remove(editor);
        return Err(err);
    }
    Ok(editor)
}

fn state(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> Result<&CurveEditorState, WidgetError> {
    match tree.payload(id).ok_or(WidgetError::UnknownWidget(id))? {
        WidgetKind::CurveEditor(state) => Ok(state),
        _ => Err(WidgetError::WrongWidgetKind(id)),
    }
}

/// A read-only view of `editor`'s own [`CurveEditorState`].
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `editor` doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if it isn't a curve editor.
pub fn curve_editor_state(
    tree: &WidgetTree<WidgetKind>,
    editor: WidgetId,
) -> Result<&CurveEditorState, WidgetError> {
    state(tree, editor)
}

/// Feeds one key to `editor` — see "Keys" in this module's own doc
/// comment. `coarse` selects the large step.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`]/[`WidgetError::WrongWidgetKind`]
/// for an id that isn't a curve editor, or [`WidgetError::WidgetDisabled`]
/// if it is disabled. Nothing changes when any of these happens.
pub fn handle_curve_editor_key(
    tree: &mut WidgetTree<WidgetKind>,
    editor: WidgetId,
    key: CurveEditorKey,
    coarse: bool,
) -> Result<CurveEditorOutcome, WidgetError> {
    with_curve_editor_mut(tree, editor, |state| state.apply_key(editor, key, coarse))
}

/// Selects point `index` (a pointer press on its marker, typically — see
/// [`curve_editor_point_at`]). Selecting the selected point is `Ignored`.
///
/// # Errors
///
/// As [`handle_curve_editor_key`], plus [`WidgetError::IndexOutOfRange`]
/// for an index past the last point.
pub fn select_curve_point(
    tree: &mut WidgetTree<WidgetKind>,
    editor: WidgetId,
    index: usize,
) -> Result<CurveEditorOutcome, WidgetError> {
    with_curve_editor_mut(tree, editor, |state| {
        if state.disabled {
            return Err(WidgetError::WidgetDisabled(editor));
        }
        let len = state.curve.points().len();
        if index >= len {
            return Err(WidgetError::IndexOutOfRange { index, len });
        }
        Ok(state.set_selected(index))
    })
}

/// The control point whose marker is under `(px, py)`: the nearest one
/// within the hit radius (`spacing.sm` at insert, in pixels), the lower
/// index on a tie — `None` over empty plot, outside the reach of every
/// marker, for a non-finite point, or for an editor with no plot. See
/// "Pointer" in this module's own doc comment for why this is geometric.
///
/// # Errors
///
/// As [`handle_curve_editor_key`] (a disabled editor has no pointer
/// interaction at all).
pub fn curve_editor_point_at(
    tree: &WidgetTree<WidgetKind>,
    editor: WidgetId,
    px: f32,
    py: f32,
) -> Result<Option<usize>, WidgetError> {
    let current = state(tree, editor)?;
    if current.disabled {
        return Err(WidgetError::WidgetDisabled(editor));
    }
    let Some((x, y)) = curve_space(tree, editor, current, px, py) else {
        return Ok(None);
    };
    let Some((_, _, width, height)) = tree
        .bounds(editor)
        .and_then(|bounds| plot_rect(bounds, current.metrics.marker_radius))
    else {
        return Ok(None);
    };
    let radius = current.metrics.hit_radius;
    Ok(current
        .curve
        .nearest_point_within(x as f32, y as f32, radius / width, radius / height))
}

/// A pointer press on empty plot: adds a point at the pointer's input
/// level **and** output level (both clamped to `[0, 1]`) and selects it.
/// `Ignored` if the curve refuses the point (16 points already, too close
/// to a neighbour, at an endpoint's input), for a non-finite point, or
/// for an editor with no plot.
///
/// # Errors
///
/// As [`handle_curve_editor_key`].
pub fn add_curve_point_from_point(
    tree: &mut WidgetTree<WidgetKind>,
    editor: WidgetId,
    px: f32,
    py: f32,
) -> Result<CurveEditorOutcome, WidgetError> {
    let at = curve_space(tree, editor, state(tree, editor)?, px, py);
    with_curve_editor_mut(tree, editor, |state| {
        if state.disabled {
            return Err(WidgetError::WidgetDisabled(editor));
        }
        let Some((x, y)) = at else {
            return Ok(CurveEditorOutcome::Ignored);
        };
        let (x, y) = (x.clamp(0.0, 1.0) as f32, y.clamp(0.0, 1.0) as f32);
        let mut next = state.curve.clone();
        let Ok(index) = next.add_point(x) else {
            return Ok(CurveEditorOutcome::Ignored);
        };
        if next.move_point_to(index, x, y).is_err() {
            return Ok(CurveEditorOutcome::Ignored);
        }
        Ok(state.set_curve(next, index))
    })
}

/// A pointer drag: moves the selected point to the pointer's levels,
/// clamped exactly as a key move is (an endpoint keeps its input level).
///
/// # Errors
///
/// As [`handle_curve_editor_key`].
pub fn move_selected_point_from_point(
    tree: &mut WidgetTree<WidgetKind>,
    editor: WidgetId,
    px: f32,
    py: f32,
) -> Result<CurveEditorOutcome, WidgetError> {
    let at = curve_space(tree, editor, state(tree, editor)?, px, py);
    with_curve_editor_mut(tree, editor, |state| {
        if state.disabled {
            return Err(WidgetError::WidgetDisabled(editor));
        }
        let Some((x, y)) = at else {
            return Ok(CurveEditorOutcome::Ignored);
        };
        let mut next = state.curve.clone();
        match next.move_point_to(
            state.selected,
            x.clamp(-1.0, 2.0) as f32,
            y.clamp(-1.0, 2.0) as f32,
        ) {
            Ok(true) => {
                let selected = state.selected;
                Ok(state.set_curve(next, selected))
            }
            _ => Ok(CurveEditorOutcome::Ignored),
        }
    })
}

/// Replaces the curve with one through `points` — an owner-driven change
/// (an undo, a preset, a document load), so it works on a disabled editor
/// too. The selection is kept, clamped to the new last point. Points
/// equal to the current ones are `Ignored` and cost no damage, so a
/// two-way binding may echo the editor's own points straight back.
///
/// # Errors
///
/// Returns [`WidgetError::InvalidCurve`] for points [`ToneCurve::new`]
/// refuses, or [`WidgetError::UnknownWidget`]/
/// [`WidgetError::WrongWidgetKind`] for an id that isn't a curve editor.
/// Nothing changes when any of these happens.
pub fn set_curve_editor_points(
    tree: &mut WidgetTree<WidgetKind>,
    editor: WidgetId,
    points: &[CurvePoint],
) -> Result<CurveEditorOutcome, WidgetError> {
    state(tree, editor)?;
    let curve = ToneCurve::new(points).map_err(WidgetError::InvalidCurve)?;
    with_curve_editor_mut(tree, editor, |state| {
        let selected = state.selected.min(curve.points().len().saturating_sub(1));
        Ok(state.set_curve(curve, selected))
    })
}

/// Enables or disables `editor` and every point in it. Owner-driven, and
/// a request matching the current state is `Ignored` — no damage.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `editor` doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if it isn't a curve editor.
pub fn set_curve_editor_disabled(
    tree: &mut WidgetTree<WidgetKind>,
    editor: WidgetId,
    disabled: bool,
) -> Result<CurveEditorOutcome, WidgetError> {
    with_curve_editor_mut(tree, editor, |state| {
        if state.disabled == disabled {
            return Ok(CurveEditorOutcome::Ignored);
        }
        state.disabled = disabled;
        Ok(CurveEditorOutcome::Changed)
    })
}

/// The curve editor `id` belongs to: `id` itself for an editor, its
/// editor for one of that editor's **tracked** point sliders, `None` for
/// anything else — so a caller routing a key from the focused widget can
/// find the editor to address it to.
#[must_use]
pub fn curve_editor_of(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> Option<WidgetId> {
    match tree.payload(id)? {
        WidgetKind::CurveEditor(_) => Some(id),
        WidgetKind::CurveEditorPoint(_) => {
            let parent = tree.parent(id)?;
            state(tree, parent)
                .ok()?
                .point_ids
                .contains(&id)
                .then_some(parent)
        }
        _ => None,
    }
}

/// The one path every mutator here goes through: run the pure
/// transition on the payload and then — **whether or not it changed
/// anything** — reconcile the points with the state and bring the root
/// in line. A transition that returns an error changes nothing and
/// reconciles nothing. The root is dirtied when its painted state (the
/// curve, the selection, `disabled`) changed or its node disagrees with
/// [`root_node`]; a no-op produces no damage at all.
fn with_curve_editor_mut<T>(
    tree: &mut WidgetTree<WidgetKind>,
    editor: WidgetId,
    f: impl FnOnce(&mut CurveEditorState) -> Result<T, WidgetError>,
) -> Result<T, WidgetError> {
    let (result, repaint) = {
        let kind = tree
            .payload_mut(editor)
            .ok_or(WidgetError::UnknownWidget(editor))?;
        let WidgetKind::CurveEditor(state) = kind else {
            return Err(WidgetError::WrongWidgetKind(editor));
        };
        let before = (state.curve.clone(), state.selected, state.disabled);
        let result = f(state)?;
        let repaint = before != (state.curve.clone(), state.selected, state.disabled);
        (result, repaint)
    };
    reconcile(tree, editor)?;
    let expected = root_node(state(tree, editor)?);
    if tree.accessibility(editor) != Some(&expected) {
        tree.set_accessibility(editor, expected)?;
        tree.mark_dirty(editor)?;
    } else if repaint {
        tree.mark_dirty(editor)?;
    }
    Ok(result)
}

/// The editor-owned children of `editor`, in order: a tracked point id,
/// or any [`WidgetKind::CurveEditorPoint`] (only this module creates
/// one, so an untracked one is a stray from a stale snapshot). **Never
/// any other kind**: a caller's own child is neither counted nor removed.
fn owned_children(
    tree: &WidgetTree<WidgetKind>,
    tracked: &[WidgetId],
    editor: WidgetId,
) -> Vec<WidgetId> {
    tree.children(editor)
        .unwrap_or_default()
        .iter()
        .copied()
        .filter(|child| {
            tracked.contains(child)
                || matches!(tree.payload(*child), Some(WidgetKind::CurveEditorPoint(_)))
        })
        .collect()
}

/// Makes the tree match `editor`'s state. **Trusts nothing it did not
/// just check**: if the owned children are not exactly the tracked ids,
/// one per curve point, every point is rebuilt ([`rebuild_points`]);
/// otherwise each point's whole payload and whole accessibility node is
/// compared with what the state says, and only a point that disagrees is
/// rewritten and dirtied.
fn reconcile(tree: &mut WidgetTree<WidgetKind>, editor: WidgetId) -> Result<(), WidgetError> {
    let current = state(tree, editor)?;
    let intact = current.point_ids.len() == current.curve.points().len()
        && owned_children(tree, &current.point_ids, editor) == current.point_ids;
    if !intact {
        return rebuild_points(tree, editor);
    }
    let stale: Vec<(WidgetId, WidgetKind, Node)> = current
        .point_ids
        .iter()
        .zip(current.curve.points())
        .enumerate()
        .filter_map(|(index, (&id, &point))| {
            let payload = point_payload(current, index, point);
            let node = point_node(index, point, index == current.selected, current.disabled);
            let ok = tree.payload(id) == Some(&payload) && tree.accessibility(id) == Some(&node);
            (!ok).then_some((id, payload, node))
        })
        .collect();
    // A point slider never has children; any found under one (a nested
    // editor written onto it by hand, see "Robustness") would otherwise
    // outlive the payload rewrite below as orphans.
    let strays: Vec<WidgetId> = current
        .point_ids
        .iter()
        .flat_map(|&id| tree.children(id).unwrap_or_default().to_vec())
        .collect();
    for stray in strays {
        tree.remove(stray)?;
    }
    for (id, payload, node) in stale {
        if let Some(slot) = tree.payload_mut(id) {
            *slot = payload;
        }
        tree.set_accessibility(id, node)?;
        tree.mark_dirty(id)?;
    }
    Ok(())
}

/// Removes every editor-owned child ([`owned_children`]) and builds one
/// fresh point slider per curve point under new ids, then records them.
/// A caller's own child is left in place. `WidgetTree::remove` marks each
/// removed widget's old bounds dirty.
fn rebuild_points(tree: &mut WidgetTree<WidgetKind>, editor: WidgetId) -> Result<(), WidgetError> {
    let tracked = state(tree, editor)?.point_ids.clone();
    for child in owned_children(tree, &tracked, editor) {
        tree.remove(child)?;
    }
    let current = state(tree, editor)?.clone();
    let mut ids = Vec::with_capacity(current.curve.points().len());
    for (index, &point) in current.curve.points().iter().enumerate() {
        ids.push(tree.insert(
            editor,
            point_style(),
            point_node(index, point, index == current.selected, current.disabled),
            point_payload(&current, index, point),
        )?);
    }
    let Some(WidgetKind::CurveEditor(state)) = tree.payload_mut(editor) else {
        return Err(WidgetError::WrongWidgetKind(editor));
    };
    state.point_ids = ids;
    // The root repaints too: the markers it draws are the points.
    tree.mark_dirty(editor)?;
    Ok(())
}

#[cfg(test)]
// Exact float equality is the claim under test (grid values, knots,
// clamped bounds); geometry tests name `x`/`y`/`r` as the formulas do.
#[allow(clippy::float_cmp, clippy::many_single_char_names)]
mod tests {
    use super::{
        CurveEditorKey, CurveEditorOutcome, CurveEditorState, MARKER_RING_WIDTH,
        add_curve_point_from_point, curve_editor_of, curve_editor_point_at, curve_editor_state,
        handle_curve_editor_key, insert_curve_editor, keyed, move_selected_point_from_point,
        select_curve_point, set_curve_editor_disabled, set_curve_editor_points,
    };
    use crate::shortcut::NamedKey;
    use crate::tree::{WidgetId, WidgetTree};
    use crate::widgets::{WidgetKind, insert_button, insert_container, new_tree, test_scales};
    use crate::{PaintOp, WidgetError, paint_widget, paint_widget_ops};
    use accesskit::{Action, Orientation, Role};
    use aurora_core::{CurvePoint, MAX_POINTS, MIN_POINT_SEPARATION, Rect, ToneCurve};
    use aurora_theme::{Palette, Theme, ThemeSet};
    use taffy::style_helpers::length;
    use taffy::{FlexDirection, Overflow, Size, Style};

    const SIZE: f32 = 128.0;
    /// `spacing.xs` (8) plus the ring width (1): the plot inset.
    const REACH: f32 = 9.0;
    /// The plot's side at `SIZE`.
    const PLOT: f32 = SIZE - 2.0 * REACH;
    const ID: WidgetId = accesskit::NodeId(7);
    const ALL_KEYS: [CurveEditorKey; 8] = [
        CurveEditorKey::PreviousPoint,
        CurveEditorKey::NextPoint,
        CurveEditorKey::Left,
        CurveEditorKey::Right,
        CurveEditorKey::Up,
        CurveEditorKey::Down,
        CurveEditorKey::Delete,
        CurveEditorKey::Insert,
    ];

    fn ok<T>(result: Result<T, WidgetError>) -> T {
        match result {
            Ok(value) => value,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn p(x: f32, y: f32) -> CurvePoint {
        CurvePoint::new(x, y)
    }

    fn curve(points: &[CurvePoint]) -> ToneCurve {
        match ToneCurve::new(points) {
            Ok(curve) => curve,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn three() -> ToneCurve {
        curve(&[p(0.0, 0.0), p(0.5, 0.5), p(1.0, 1.0)])
    }

    fn level(n: f32) -> f32 {
        (f64::from(n) / 255.0) as f32
    }

    // ---- Pure transitions ----------------------------------------------

    #[test]
    fn named_keys_map_onto_arrows_delete_and_enter_only() {
        let mapped = [
            (NamedKey::ArrowLeft, CurveEditorKey::Left),
            (NamedKey::ArrowRight, CurveEditorKey::Right),
            (NamedKey::ArrowUp, CurveEditorKey::Up),
            (NamedKey::ArrowDown, CurveEditorKey::Down),
            (NamedKey::Delete, CurveEditorKey::Delete),
            (NamedKey::Backspace, CurveEditorKey::Delete),
            (NamedKey::Enter, CurveEditorKey::Insert),
        ];
        for (named, key) in mapped {
            assert_eq!(CurveEditorKey::from_named_key(named), Some(key));
        }
        for named in [
            NamedKey::Home,
            NamedKey::End,
            NamedKey::Tab,
            NamedKey::Escape,
            NamedKey::Space,
        ] {
            assert_eq!(CurveEditorKey::from_named_key(named), None, "{named:?}");
        }
    }

    /// Review C3: "the segment after the selection" alone stalls at 10
    /// points from the identity; the widest-segment fallback carries
    /// repeated `Insert` all the way to `MAX_POINTS`, and only then is
    /// it `Ignored`.
    #[test]
    fn repeated_insert_from_the_identity_reaches_max_points() {
        let (mut c, mut sel) = (ToneCurve::identity(), 0);
        for n in 3..=MAX_POINTS {
            let (next, next_sel) = keyed(&c, sel, CurveEditorKey::Insert, false);
            assert_eq!(next.points().len(), n, "press {} stalled", n - 2);
            (c, sel) = (next, next_sel);
        }
        assert_eq!(
            keyed(&c, sel, CurveEditorKey::Insert, false),
            (c.clone(), sel)
        );
    }

    #[test]
    fn previous_and_next_move_the_selection_without_wrapping() {
        let c = three();
        assert_eq!(
            keyed(&c, 0, CurveEditorKey::PreviousPoint, false),
            (c.clone(), 0)
        );
        assert_eq!(
            keyed(&c, 0, CurveEditorKey::NextPoint, false),
            (c.clone(), 1)
        );
        assert_eq!(
            keyed(&c, 1, CurveEditorKey::PreviousPoint, true),
            (c.clone(), 0)
        );
        assert_eq!(
            keyed(&c, 2, CurveEditorKey::NextPoint, false),
            (c.clone(), 2)
        );
    }

    #[test]
    fn up_and_down_snap_directionally_and_stop_at_the_bounds() {
        let c = curve(&[p(0.0, 0.0), p(0.5, 0.5), p(1.0, 1.0)]);
        let y_after = |c: &ToneCurve, i: usize, key, coarse| {
            let (next, _) = keyed(c, i, key, coarse);
            next.points().get(i).map(|q| q.y)
        };
        // 0.5 is 127.5 levels: off the grid, so each direction lands on
        // the neighbouring grid point, never further than one step.
        assert_eq!(
            y_after(&c, 1, CurveEditorKey::Up, false),
            Some(level(128.0))
        );
        assert_eq!(
            y_after(&c, 1, CurveEditorKey::Down, false),
            Some(level(127.0))
        );
        assert_eq!(y_after(&c, 1, CurveEditorKey::Up, true), Some(level(130.0)));
        assert_eq!(
            y_after(&c, 1, CurveEditorKey::Down, true),
            Some(level(120.0))
        );
        // On the grid: exactly one step.
        let on = curve(&[p(0.0, level(3.0)), p(1.0, level(250.0))]);
        assert_eq!(y_after(&on, 0, CurveEditorKey::Up, false), Some(level(4.0)));
        assert_eq!(
            y_after(&on, 0, CurveEditorKey::Down, false),
            Some(level(2.0))
        );
        assert_eq!(y_after(&on, 0, CurveEditorKey::Down, true), Some(0.0));
        assert_eq!(y_after(&on, 1, CurveEditorKey::Up, true), Some(1.0));
        // At a bound: Ignored, the curve unchanged.
        assert_eq!(keyed(&c, 0, CurveEditorKey::Down, false), (c.clone(), 0));
        assert_eq!(keyed(&c, 2, CurveEditorKey::Up, true), (c.clone(), 2));
        // Endpoints move vertically and keep their x.
        let (next, _) = keyed(&c, 2, CurveEditorKey::Down, false);
        assert_eq!(next.points().last(), Some(&p(1.0, level(254.0))));
    }

    #[test]
    fn left_and_right_move_interior_points_only_clamped_by_the_neighbours() {
        let c = curve(&[p(0.0, 0.0), p(0.5, 0.5), p(0.505, 0.6), p(1.0, 1.0)]);
        let (next, sel) = keyed(&c, 1, CurveEditorKey::Left, false);
        assert_eq!(
            (next.points().get(1).map(|q| q.x), sel),
            (Some(level(127.0)), 1)
        );
        let (next, _) = keyed(&c, 1, CurveEditorKey::Left, true);
        assert_eq!(next.points().get(1).map(|q| q.x), Some(level(120.0)));
        // Right would reach 128/255 ≈ 0.50196, within 1/256 of 0.505:
        // clamped to exactly the separation.
        let (next, _) = keyed(&c, 1, CurveEditorKey::Right, false);
        let x = next.points().get(1).map_or(f32::NAN, |q| q.x);
        assert!(x > 0.5 && 0.505 - x >= MIN_POINT_SEPARATION, "{x}");
        assert!(
            0.505 - x.next_up() < MIN_POINT_SEPARATION,
            "{x} is the tightest legal x"
        );
        // Pinned against the neighbour, a further press is Ignored.
        assert_eq!(
            keyed(&next, 1, CurveEditorKey::Right, false),
            (next.clone(), 1)
        );
        for key in [CurveEditorKey::Left, CurveEditorKey::Right] {
            assert_eq!(keyed(&c, 0, key, false), (c.clone(), 0));
            assert_eq!(keyed(&c, 3, key, true), (c.clone(), 3));
        }
    }

    #[test]
    fn delete_removes_an_interior_point_and_selects_the_previous_one() {
        let c = curve(&[p(0.0, 0.0), p(0.25, 0.4), p(0.5, 0.5), p(1.0, 1.0)]);
        let (next, sel) = keyed(&c, 2, CurveEditorKey::Delete, false);
        assert_eq!(next.points(), [p(0.0, 0.0), p(0.25, 0.4), p(1.0, 1.0)]);
        assert_eq!(sel, 1);
        let (next, sel) = keyed(&c, 1, CurveEditorKey::Delete, false);
        assert_eq!((next.points().len(), sel), (3, 0));
        assert_eq!(keyed(&c, 0, CurveEditorKey::Delete, false), (c.clone(), 0));
        assert_eq!(keyed(&c, 3, CurveEditorKey::Delete, false), (c.clone(), 3));
    }

    #[test]
    fn insert_adds_the_segments_midpoint_on_the_curve_and_selects_it() {
        let c = curve(&[p(0.0, 0.0), p(0.5, 0.8), p(1.0, 1.0)]);
        let (next, sel) = keyed(&c, 0, CurveEditorKey::Insert, false);
        assert_eq!(sel, 1);
        assert_eq!(next.points().get(1), Some(&p(0.25, c.evaluate(0.25))));
        // The last point selected: the segment *before* it.
        let (next, sel) = keyed(&c, 2, CurveEditorKey::Insert, false);
        assert_eq!(sel, 2);
        assert_eq!(next.points().get(2), Some(&p(0.75, c.evaluate(0.75))));
        assert_eq!(next.points().len(), 4);
        // A segment narrower than two separations has no room: the
        // widest segment ([0, 0.5], just wider than [0.5+, 1]) is split.
        let narrow = curve(&[
            p(0.0, 0.0),
            p(0.5, 0.5),
            p(0.5 + 2.0 * MIN_POINT_SEPARATION - 1e-6, 0.6),
            p(1.0, 1.0),
        ]);
        let (next, sel) = keyed(&narrow, 1, CurveEditorKey::Insert, false);
        assert_eq!((next.points().len(), sel), (5, 1));
        assert_eq!(next.points().get(1), Some(&p(0.25, narrow.evaluate(0.25))));
        // Exactly two separations: room for one point, exactly between.
        let tight = curve(&[
            p(0.0, 0.0),
            p(0.5, 0.5),
            p(0.5 + 2.0 * MIN_POINT_SEPARATION, 0.6),
            p(1.0, 1.0),
        ]);
        let (next, sel) = keyed(&tight, 1, CurveEditorKey::Insert, false);
        assert_eq!((next.points().len(), sel), (5, 2));
        // Sixteen points: full.
        let full: Vec<CurvePoint> = (0..MAX_POINTS)
            .map(|i| p(i as f32 / (MAX_POINTS - 1) as f32, 0.5))
            .collect();
        let full = curve(&full);
        assert_eq!(
            keyed(&full, 3, CurveEditorKey::Insert, false),
            (full.clone(), 3)
        );
    }

    // ---- Tree ------------------------------------------------------------

    fn sized_root() -> Style {
        Style {
            flex_direction: FlexDirection::Column,
            size: Size {
                width: length(320.0_f32),
                height: length(320.0_f32),
            },
            ..Default::default()
        }
    }

    fn inserted_with(c: ToneCurve) -> (WidgetTree<WidgetKind>, WidgetId) {
        let (mut tree, root) = new_tree(sized_root());
        let editor = ok(insert_curve_editor(
            &mut tree,
            root,
            &test_scales(),
            "Curve",
            SIZE,
            c,
        ));
        tree.compute_layout(320.0, 320.0);
        let _ = tree.take_damage();
        (tree, editor)
    }

    fn inserted() -> (WidgetTree<WidgetKind>, WidgetId) {
        inserted_with(three())
    }

    fn snapshot(tree: &WidgetTree<WidgetKind>, editor: WidgetId) -> CurveEditorState {
        ok(curve_editor_state(tree, editor)).clone()
    }

    fn point_ids(tree: &WidgetTree<WidgetKind>, editor: WidgetId) -> Vec<WidgetId> {
        let s = snapshot(tree, editor);
        (0..s.curve().points().len())
            .filter_map(|i| s.point_id(i))
            .collect()
    }

    fn dirty(tree: &WidgetTree<WidgetKind>, ids: &[WidgetId]) -> Vec<bool> {
        ids.iter()
            .map(|&id| tree.is_dirty(id) == Some(true))
            .collect()
    }

    /// Every structural and per-node claim, from scratch.
    fn assert_sound(tree: &WidgetTree<WidgetKind>, editor: WidgetId) {
        let s = snapshot(tree, editor);
        let ids = point_ids(tree, editor);
        let n = s.curve().points().len();
        assert_eq!(ids.len(), n);
        let owned: Vec<WidgetId> = tree
            .children(editor)
            .unwrap_or_default()
            .iter()
            .copied()
            .filter(|&c| matches!(tree.payload(c), Some(WidgetKind::CurveEditorPoint(_))))
            .collect();
        assert_eq!(owned, ids);
        assert!(s.selected() < n);
        assert_eq!(s.focus_target(), ids.get(s.selected()).copied());
        for (i, (&id, &point)) in ids.iter().zip(s.curve().points()).enumerate() {
            let Some(WidgetKind::CurveEditorPoint(ps)) = tree.payload(id) else {
                unreachable!("a point slider");
            };
            assert_eq!(
                (
                    ps.index(),
                    ps.count(),
                    ps.point(),
                    ps.is_selected(),
                    ps.is_endpoint(),
                    ps.is_disabled()
                ),
                (
                    i,
                    n,
                    point,
                    i == s.selected(),
                    i == 0 || i + 1 == n,
                    s.disabled()
                )
            );
            let Some(node) = tree.accessibility(id) else {
                unreachable!("every widget has a node");
            };
            assert_eq!(node.role(), Role::Slider);
            assert_eq!(node.label(), Some(format!("Point {}", i + 1).as_str()));
            assert_eq!(node.orientation(), Some(Orientation::Vertical));
            let out = (f64::from(point.y) * 2550.0).round() / 10.0;
            let inp = (f64::from(point.x) * 2550.0).round() / 10.0;
            assert_eq!(node.numeric_value(), Some(out));
            assert_eq!(node.min_numeric_value(), Some(0.0));
            assert_eq!(node.max_numeric_value(), Some(255.0));
            assert_eq!(node.numeric_value_step(), Some(1.0));
            assert_eq!(node.numeric_value_jump(), Some(10.0));
            assert_eq!(
                node.value(),
                Some(format!("Input {inp:.1}, output {out:.1}").as_str())
            );
            assert_eq!(node.is_disabled(), s.disabled());
            assert_eq!(
                node.supports_action(Action::Focus),
                !s.disabled() && i == s.selected(),
                "roving focus, point {i}"
            );
            for action in [Action::SetValue, Action::Increment, Action::Decrement] {
                assert_eq!(node.supports_action(action), !s.disabled());
            }
        }
        let Some(root) = tree.accessibility(editor) else {
            unreachable!("root node");
        };
        assert_eq!(root.role(), Role::Group);
        assert_eq!(root.label(), Some(s.label()));
        assert_eq!(root.is_disabled(), s.disabled());
    }

    #[test]
    fn insert_builds_one_slider_per_point_and_lays_out_size_square() {
        let (tree, editor) = inserted();
        assert_sound(&tree, editor);
        let s = snapshot(&tree, editor);
        assert_eq!(
            (s.selected(), s.disabled(), s.label(), s.size()),
            (0, false, "Curve", SIZE)
        );
        assert_eq!(s.curve(), &three());
        let full = Rect {
            x: 0,
            y: 0,
            width: 128,
            height: 128,
        };
        assert_eq!(tree.bounds(editor), Some(full));
        for id in point_ids(&tree, editor) {
            assert_eq!(tree.bounds(id), Some(full), "inset 0");
        }
    }

    #[test]
    fn insert_rejects_a_bad_size_and_an_unknown_parent_and_adds_nothing() {
        let (mut tree, root) = new_tree(sized_root());
        for size in [0.0, 0.5, f32::NAN, f32::INFINITY, 2.0e6, -1.0] {
            match insert_curve_editor(&mut tree, root, &test_scales(), "C", size, three()) {
                Err(WidgetError::InvalidRange { min, max }) => assert_eq!((min, max), (1.0, 1.0e6)),
                other => unreachable!("{size}: {other:?}"),
            }
        }
        assert!(matches!(
            insert_curve_editor(&mut tree, ID, &test_scales(), "C", SIZE, three()),
            Err(WidgetError::UnknownWidget(_))
        ));
        assert_eq!(tree.len(), 1);
    }

    #[test]
    fn a_move_dirties_exactly_that_point_and_the_root() {
        let (mut tree, editor) = inserted();
        ok(select_curve_point(&mut tree, editor, 1));
        let _ = tree.take_damage();
        let ids = point_ids(&tree, editor);
        let out = ok(handle_curve_editor_key(
            &mut tree,
            editor,
            CurveEditorKey::Up,
            false,
        ));
        assert_eq!(out, CurveEditorOutcome::Changed);
        assert_eq!(point_ids(&tree, editor), ids, "a move keeps every id");
        assert_eq!(dirty(&tree, &ids), vec![false, true, false]);
        assert_eq!(tree.is_dirty(editor), Some(true));
        assert_sound(&tree, editor);
        // A selection change: the two points involved and the root.
        let _ = tree.take_damage();
        ok(handle_curve_editor_key(
            &mut tree,
            editor,
            CurveEditorKey::NextPoint,
            false,
        ));
        assert_eq!(dirty(&tree, &ids), vec![false, true, true]);
        assert_eq!(tree.is_dirty(editor), Some(true));
        assert_eq!(snapshot(&tree, editor).focus_target(), ids.get(2).copied());
    }

    #[test]
    fn ignored_keys_and_echoed_points_cost_no_damage() {
        let (mut tree, editor) = inserted();
        let ids = point_ids(&tree, editor);
        for key in [
            CurveEditorKey::PreviousPoint,
            CurveEditorKey::Left,
            CurveEditorKey::Down,
            CurveEditorKey::Delete,
        ] {
            let out = ok(handle_curve_editor_key(&mut tree, editor, key, false));
            assert_eq!(out, CurveEditorOutcome::Ignored, "{key:?}");
        }
        let echo = snapshot(&tree, editor).curve().points().to_vec();
        assert_eq!(
            ok(set_curve_editor_points(&mut tree, editor, &echo)),
            CurveEditorOutcome::Ignored
        );
        assert_eq!(
            ok(set_curve_editor_disabled(&mut tree, editor, false)),
            CurveEditorOutcome::Ignored
        );
        assert_eq!(
            ok(select_curve_point(&mut tree, editor, 0)),
            CurveEditorOutcome::Ignored
        );
        assert_eq!(tree.take_damage(), None);
        assert_eq!(point_ids(&tree, editor), ids);
    }

    #[test]
    fn add_and_delete_rebuild_every_point_under_new_ids() {
        let (mut tree, editor) = inserted();
        let before = point_ids(&tree, editor);
        ok(handle_curve_editor_key(
            &mut tree,
            editor,
            CurveEditorKey::Insert,
            false,
        ));
        let after = point_ids(&tree, editor);
        assert_eq!(after.len(), 4);
        assert!(after.iter().all(|id| !before.contains(id)));
        assert!(before.iter().all(|&id| !tree.contains(id)));
        let s = snapshot(&tree, editor);
        assert_eq!(s.selected(), 1);
        assert_eq!(s.focus_target(), after.get(1).copied());
        assert_sound(&tree, editor);
        ok(handle_curve_editor_key(
            &mut tree,
            editor,
            CurveEditorKey::Delete,
            false,
        ));
        let s = snapshot(&tree, editor);
        assert_eq!((s.curve(), s.selected()), (&three(), 0));
        assert_sound(&tree, editor);
    }

    #[test]
    fn a_disabled_editor_refuses_every_gesture_but_takes_owner_updates() {
        let (mut tree, editor) = inserted();
        assert_eq!(
            ok(set_curve_editor_disabled(&mut tree, editor, true)),
            CurveEditorOutcome::Changed
        );
        assert_sound(&tree, editor);
        let _ = tree.take_damage();
        let before = snapshot(&tree, editor);
        for key in ALL_KEYS {
            for coarse in [false, true] {
                assert!(matches!(
                    handle_curve_editor_key(&mut tree, editor, key, coarse),
                    Err(WidgetError::WidgetDisabled(id)) if id == editor
                ));
            }
        }
        assert!(matches!(
            select_curve_point(&mut tree, editor, 1),
            Err(WidgetError::WidgetDisabled(_))
        ));
        assert!(matches!(
            curve_editor_point_at(&tree, editor, 64.0, 64.0),
            Err(WidgetError::WidgetDisabled(_))
        ));
        assert!(matches!(
            add_curve_point_from_point(&mut tree, editor, 30.0, 30.0),
            Err(WidgetError::WidgetDisabled(_))
        ));
        assert!(matches!(
            move_selected_point_from_point(&mut tree, editor, 30.0, 30.0),
            Err(WidgetError::WidgetDisabled(_))
        ));
        assert_eq!(snapshot(&tree, editor), before);
        assert_eq!(tree.take_damage(), None);
        let points = [p(0.0, 0.2), p(1.0, 0.8)];
        assert_eq!(
            ok(set_curve_editor_points(&mut tree, editor, &points)),
            CurveEditorOutcome::Changed
        );
        assert_sound(&tree, editor);
    }

    #[test]
    fn owner_updates_validate_and_clamp_the_selection() {
        let (mut tree, editor) = inserted();
        ok(select_curve_point(&mut tree, editor, 2));
        assert!(matches!(
            set_curve_editor_points(&mut tree, editor, &[p(0.0, 0.0), p(0.5, 2.0), p(1.0, 1.0)]),
            Err(WidgetError::InvalidCurve(
                aurora_core::ToneCurveError::OutOfRange
            ))
        ));
        assert!(matches!(
            set_curve_editor_points(&mut tree, editor, &[p(0.0, 0.0)]),
            Err(WidgetError::InvalidCurve(
                aurora_core::ToneCurveError::TooFewPoints
            ))
        ));
        assert_eq!(snapshot(&tree, editor).curve(), &three());
        ok(set_curve_editor_points(
            &mut tree,
            editor,
            &[p(0.0, 0.3), p(1.0, 0.6)],
        ));
        assert_eq!(
            snapshot(&tree, editor).selected(),
            1,
            "clamped to the new last point"
        );
        assert_sound(&tree, editor);
        assert!(matches!(
            select_curve_point(&mut tree, editor, 2),
            Err(WidgetError::IndexOutOfRange { index: 2, len: 2 })
        ));
    }

    #[test]
    fn mutators_reject_a_wrong_widget_kind_and_an_unknown_widget() {
        let (mut tree, editor) = inserted();
        let root = tree.root();
        let button = ok(insert_button(&mut tree, root, &test_scales(), "OK"));
        for id in [button, ID] {
            let key = handle_curve_editor_key(&mut tree, id, CurveEditorKey::Up, false);
            assert!(matches!(
                key,
                Err(WidgetError::WrongWidgetKind(_) | WidgetError::UnknownWidget(_))
            ));
            assert!(set_curve_editor_points(&mut tree, id, &[p(0.0, 0.0), p(1.0, 1.0)]).is_err());
            assert!(set_curve_editor_disabled(&mut tree, id, true).is_err());
            assert!(curve_editor_state(&tree, id).is_err());
        }
        let ids = point_ids(&tree, editor);
        assert_eq!(curve_editor_of(&tree, editor), Some(editor));
        assert_eq!(
            ids.iter()
                .map(|&id| curve_editor_of(&tree, id))
                .collect::<Vec<_>>(),
            vec![Some(editor); 3]
        );
        assert_eq!(curve_editor_of(&tree, button), None);
        assert_eq!(curve_editor_of(&tree, ID), None);
    }

    #[test]
    fn outside_damage_is_repaired_by_the_next_call() {
        // A point removed from outside: every point rebuilt, new ids.
        let (mut tree, editor) = inserted();
        let ids = point_ids(&tree, editor);
        let Some(&middle) = ids.get(1) else {
            unreachable!()
        };
        ok(tree.remove(middle));
        ok(handle_curve_editor_key(
            &mut tree,
            editor,
            CurveEditorKey::Down,
            false,
        ));
        assert_sound(&tree, editor);
        assert!(point_ids(&tree, editor).iter().all(|id| !ids.contains(id)));
        // A stale payload and an overwritten point node: repaired in place
        // by a no-op, ids kept.
        let ids = point_ids(&tree, editor);
        let Some(&first) = ids.first() else {
            unreachable!()
        };
        if let Some(WidgetKind::CurveEditorPoint(ps)) = tree.payload_mut(first) {
            ps.point = p(0.0, 0.9);
        }
        ok(tree.set_accessibility(
            ids.get(2).copied().unwrap_or(first),
            accesskit::Node::new(Role::Slider),
        ));
        let _ = tree.take_damage();
        assert_eq!(
            ok(select_curve_point(&mut tree, editor, 0)),
            CurveEditorOutcome::Ignored
        );
        assert_sound(&tree, editor);
        assert_eq!(point_ids(&tree, editor), ids);
        assert_eq!(dirty(&tree, &ids), vec![true, false, true]);
        // A tampered root node: repaired by a no-op, and dirtied.
        let _ = tree.take_damage();
        ok(tree.set_accessibility(editor, accesskit::Node::new(Role::Group)));
        let _ = tree.take_damage();
        ok(select_curve_point(&mut tree, editor, 0));
        assert_sound(&tree, editor);
        assert_eq!(tree.is_dirty(editor), Some(true));
    }

    #[test]
    fn a_stale_snapshot_and_a_callers_own_child_are_handled() {
        let (mut tree, editor) = inserted();
        let stale = snapshot(&tree, editor);
        ok(handle_curve_editor_key(
            &mut tree,
            editor,
            CurveEditorKey::Insert,
            false,
        ));
        let button = ok(insert_button(&mut tree, editor, &test_scales(), "Mine"));
        if let Some(slot) = tree.payload_mut(editor) {
            *slot = WidgetKind::CurveEditor(stale);
        }
        ok(select_curve_point(&mut tree, editor, 0));
        assert_sound(&tree, editor);
        assert_eq!(
            point_ids(&tree, editor).len(),
            3,
            "no duplicates left behind"
        );
        assert!(tree.contains(button));
        assert!(tree.children(editor).is_some_and(|c| c.contains(&button)));
        // Many more calls later, the caller's child is still there.
        ok(handle_curve_editor_key(
            &mut tree,
            editor,
            CurveEditorKey::Insert,
            false,
        ));
        ok(handle_curve_editor_key(
            &mut tree,
            editor,
            CurveEditorKey::Delete,
            false,
        ));
        assert!(tree.contains(button));
        assert_sound(&tree, editor);
    }

    #[test]
    fn the_tab_order_visits_only_the_selected_point() {
        let (mut tree, editor) = inserted();
        let root = tree.root();
        let button = ok(insert_button(&mut tree, root, &test_scales(), "OK"));
        ok(select_curve_point(&mut tree, editor, 1));
        let ids = point_ids(&tree, editor);
        let mut focus = crate::FocusManager::new();
        let order: Vec<_> = (0..4).filter_map(|_| focus.focus_next(&mut tree)).collect();
        let Some(&selected) = ids.get(1) else {
            unreachable!()
        };
        assert_eq!(order, vec![selected, button, selected, button]);
        ok(set_curve_editor_disabled(&mut tree, editor, true));
        let mut focus = crate::FocusManager::new();
        let order: Vec<_> = (0..2).filter_map(|_| focus.focus_next(&mut tree)).collect();
        assert_eq!(order, vec![button, button]);
    }

    fn descendants<'a>(
        node: accesskit_consumer::Node<'a>,
        out: &mut Vec<accesskit_consumer::Node<'a>>,
    ) {
        out.push(node);
        for child in node.children() {
            descendants(child, out);
        }
    }

    #[test]
    fn accesskit_consumer_reads_the_editor_as_documented() {
        let (mut tree, editor) = inserted_with(curve(&[p(0.0, 0.0), p(0.25, 0.6), p(1.0, 1.0)]));
        ok(select_curve_point(&mut tree, editor, 1));
        let focus = ok(curve_editor_state(&tree, editor)).focus_target();
        let Some(focus) = focus else { unreachable!() };
        let filter = accesskit_consumer::common_filter;
        let consumer = accesskit_consumer::Tree::new(tree.accessibility_update(focus), true);
        let mut nodes = Vec::new();
        descendants(consumer.state().root(), &mut nodes);
        let by_label = |label: &str| {
            let Some(node) = nodes.iter().find(|n| n.label().as_deref() == Some(label)) else {
                unreachable!("{label} reaches the consumer");
            };
            *node
        };
        assert_eq!(by_label("Curve").role(), Role::Group);
        let point = by_label("Point 2");
        assert_eq!(point.role(), Role::Slider);
        assert_eq!(point.numeric_value(), Some(153.0));
        assert_eq!(point.min_numeric_value(), Some(0.0));
        assert_eq!(point.max_numeric_value(), Some(255.0));
        assert_eq!(point.value().as_deref(), Some("Input 63.8, output 153.0"));
        assert!(point.is_focused());
        assert!(point.is_focusable(&filter));
        assert!(point.supports_increment(&filter));
        assert!(!by_label("Point 1").is_focusable(&filter));
        assert!(!by_label("Point 3").is_focusable(&filter));
        // After Insert and Delete the focus target is a live, focusable node.
        for key in [CurveEditorKey::Insert, CurveEditorKey::Delete] {
            ok(handle_curve_editor_key(&mut tree, editor, key, false));
            let Some(target) = ok(curve_editor_state(&tree, editor)).focus_target() else {
                unreachable!()
            };
            let consumer = accesskit_consumer::Tree::new(tree.accessibility_update(target), true);
            let Some(node) = consumer.state().node_by_id(
                consumer
                    .state()
                    .focus_id()
                    .unwrap_or(consumer.state().root_id()),
            ) else {
                unreachable!()
            };
            assert!(node.is_focusable(&filter), "{key:?}");
            assert_eq!(node.role(), Role::Slider);
        }
    }

    // ---- Pointer ---------------------------------------------------------

    #[test]
    fn the_plot_maps_corners_centre_and_clamps_outside_points() {
        let (mut tree, editor) = inserted();
        ok(select_curve_point(&mut tree, editor, 1));
        let at =
            |tree: &WidgetTree<WidgetKind>| snapshot(tree, editor).curve().points().get(1).copied();
        ok(move_selected_point_from_point(
            &mut tree,
            editor,
            REACH + PLOT / 4.0,
            REACH,
        ));
        assert_eq!(at(&tree), Some(p(0.25, 1.0)));
        ok(move_selected_point_from_point(
            &mut tree,
            editor,
            REACH + PLOT * 0.75,
            REACH + PLOT,
        ));
        assert_eq!(
            at(&tree),
            Some(p(0.75, 0.0)),
            "y is inverted: the bottom is 0"
        );
        ok(move_selected_point_from_point(
            &mut tree, editor, 64.0, 64.0,
        ));
        assert_eq!(at(&tree), Some(p(0.5, 0.5)));
        // Outside the plot: clamped (the interior x by its neighbours).
        ok(move_selected_point_from_point(
            &mut tree, editor, -500.0, -500.0,
        ));
        assert_eq!(at(&tree).map(|q| q.y), Some(1.0));
        assert!(at(&tree).is_some_and(|q| q.x > 0.0 && q.x - 0.0 >= MIN_POINT_SEPARATION));
        // An endpoint dragged keeps its x.
        ok(select_curve_point(&mut tree, editor, 2));
        ok(move_selected_point_from_point(
            &mut tree,
            editor,
            REACH,
            REACH + PLOT / 2.0,
        ));
        assert_eq!(
            snapshot(&tree, editor).curve().points().last(),
            Some(&p(1.0, 0.5))
        );
        for (x, y) in [(f32::NAN, 10.0), (10.0, f32::INFINITY)] {
            assert_eq!(
                ok(move_selected_point_from_point(&mut tree, editor, x, y)),
                CurveEditorOutcome::Ignored
            );
            assert_eq!(
                ok(add_curve_point_from_point(&mut tree, editor, x, y)),
                CurveEditorOutcome::Ignored
            );
            assert_eq!(ok(curve_editor_point_at(&tree, editor, x, y)), None);
        }
    }

    #[test]
    fn adding_from_the_pointer_places_and_selects_the_point() {
        let (mut tree, editor) = inserted();
        let out = ok(add_curve_point_from_point(
            &mut tree,
            editor,
            REACH + PLOT / 4.0,
            REACH + PLOT / 4.0,
        ));
        assert_eq!(out, CurveEditorOutcome::Changed);
        let s = snapshot(&tree, editor);
        assert_eq!(s.curve().points().get(1), Some(&p(0.25, 0.75)));
        assert_eq!(s.selected(), 1);
        assert_sound(&tree, editor);
        // On an existing point's input, or at an endpoint's: refused.
        let out = ok(add_curve_point_from_point(
            &mut tree,
            editor,
            REACH + PLOT / 4.0,
            20.0,
        ));
        assert_eq!(out, CurveEditorOutcome::Ignored);
        let out = ok(add_curve_point_from_point(&mut tree, editor, 0.0, 20.0));
        assert_eq!(out, CurveEditorOutcome::Ignored);
    }

    #[test]
    fn a_plot_with_no_area_ignores_the_pointer() {
        let (mut tree, root) = new_tree(sized_root());
        let editor = ok(insert_curve_editor(
            &mut tree,
            root,
            &test_scales(),
            "C",
            2.0 * REACH,
            three(),
        ));
        // Never laid out, then laid out with a zero-area plot.
        for layout in [false, true] {
            if layout {
                tree.compute_layout(320.0, 320.0);
            }
            assert_eq!(ok(curve_editor_point_at(&tree, editor, 5.0, 5.0)), None);
            let out = ok(add_curve_point_from_point(&mut tree, editor, 5.0, 5.0));
            assert_eq!(out, CurveEditorOutcome::Ignored);
            let out = ok(move_selected_point_from_point(&mut tree, editor, 5.0, 5.0));
            assert_eq!(out, CurveEditorOutcome::Ignored);
        }
    }

    #[test]
    fn point_at_finds_the_nearest_marker_within_the_hit_radius() {
        let (mut tree, editor) = inserted_with(curve(&[
            p(0.0, 0.0),
            p(0.5, 0.5),
            p(0.625, 0.5),
            p(1.0, 1.0),
        ]));
        let screen = |x: f32, y: f32| (REACH + x * PLOT, REACH + (1.0 - y) * PLOT);
        let (x, y) = screen(0.5, 0.5);
        assert_eq!(
            ok(curve_editor_point_at(&tree, editor, x + 3.0, y - 3.0)),
            Some(1)
        );
        // Just inside `spacing.sm` (12) px away horizontally: a hit;
        // just outside: not.
        let (x, y) = screen(1.0, 1.0);
        assert_eq!(
            ok(curve_editor_point_at(&tree, editor, x + 11.9, y)),
            Some(3)
        );
        assert_eq!(ok(curve_editor_point_at(&tree, editor, x + 12.5, y)), None);
        // Between points 1 and 2 (13.75 px apart): the nearer one, the
        // lower index on an exact tie.
        let (x1, y1) = screen(0.5, 0.5);
        let (x2, _) = screen(0.625, 0.5);
        assert_eq!(
            ok(curve_editor_point_at(&tree, editor, x2 - 2.0, y1)),
            Some(2)
        );
        assert_eq!(
            ok(curve_editor_point_at(
                &tree,
                editor,
                f32::midpoint(x1, x2),
                y1
            )),
            Some(1)
        );
        assert_eq!(
            ok(curve_editor_point_at(&tree, editor, 64.0, 20.0)),
            None,
            "empty plot"
        );
        ok(select_curve_point(&mut tree, editor, 3));
    }

    // ---- Paint -----------------------------------------------------------

    const PALETTE_TOML: &str = include_str!("../../../../design/tokens/palette.toml");
    const DARK_THEME_TOML: &str = include_str!("../../../../design/themes/dark.toml");

    fn dark_theme() -> Theme {
        let Ok(palette) = Palette::from_toml_str(PALETTE_TOML) else {
            unreachable!("the committed palette parses");
        };
        let mut themes = ThemeSet::new();
        if let Err(err) = themes.register(DARK_THEME_TOML) {
            unreachable!("{err:?}");
        }
        match themes.resolve("Dark", &palette) {
            Ok(theme) => theme,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn solids(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> Vec<(aurora_vector::Mesh, [f32; 4])> {
        ok(paint_widget_ops(
            tree,
            id,
            &dark_theme(),
            &test_scales(),
            1.0,
        ))
        .into_iter()
        .map(|op| match op {
            PaintOp::Solid(paint) => paint,
            PaintOp::Gradient(_) => unreachable!("the editor paints no gradient"),
        })
        .collect()
    }

    fn rgba(color: aurora_theme::Color, alpha: f32) -> [f32; 4] {
        let [r, g, b] = color.to_srgb_f32();
        [r, g, b, alpha]
    }

    fn extent(mesh: &aurora_vector::Mesh) -> (f32, f32, f32, f32) {
        mesh.vertices.iter().fold(
            (f32::MAX, f32::MAX, f32::MIN, f32::MIN),
            |(a, b, c, d), q| (a.min(q.x), b.min(q.y), c.max(q.x), d.max(q.y)),
        )
    }

    /// Well fill, border, six grid bands, diagonal, curve, then two rings
    /// per unselected point and disc + two rings for the selected one,
    /// drawn last — every colour a token.
    #[test]
    fn the_editor_paints_well_grid_diagonal_curve_then_markers_selected_last() {
        let theme = dark_theme();
        let (mut tree, editor) = inserted();
        ok(select_curve_point(&mut tree, editor, 1));
        let paints = solids(&tree, editor);
        let colours: Vec<[f32; 4]> = paints.iter().map(|(_, c)| *c).collect();
        let border = rgba(theme.border.default, 1.0);
        let text = rgba(theme.text.primary, 1.0);
        let panel = rgba(theme.surface.panel, 1.0);
        let mut want = vec![rgba(theme.surface.sunken, 1.0), border];
        want.extend([border; 6]);
        want.extend([border, text]);
        want.extend([text, panel, text, panel]);
        want.extend([rgba(theme.accent.primary, 1.0), text, panel]);
        assert_eq!(colours, want);
        // The selected point's disc is centred on its own data position.
        let Some((disc, _)) = paints.get(paints.len() - 3) else {
            unreachable!()
        };
        let (x0, y0, x1, y1) = extent(disc);
        let (cx, cy) = (f32::midpoint(x0, x1), f32::midpoint(y0, y1));
        assert!(
            (cx - 64.0).abs() < 0.05 && (cy - 64.0).abs() < 0.05,
            "{cx},{cy}"
        );
        // The grid's first vertical band sits at a quarter of the plot.
        let Some((band, _)) = paints.get(2) else {
            unreachable!()
        };
        let (bx0, by0, bx1, by1) = extent(band);
        assert_eq!(
            (bx0, by0, bx1, by1),
            (
                REACH + PLOT / 4.0 - 0.5,
                REACH,
                REACH + PLOT / 4.0 + 0.5,
                REACH + PLOT
            )
        );
        // The curve passes through each knot: its extent is the plot.
        let Some((line, _)) = paints.get(9) else {
            unreachable!()
        };
        let (lx0, ly0, lx1, ly1) = extent(line);
        assert!((lx0 - (REACH - 1.0)).abs() < 0.5 && (lx1 - (REACH + PLOT + 1.0)).abs() < 0.5);
        assert!((ly0 - (REACH - 1.0)).abs() < 0.5 && (ly1 - (REACH + PLOT + 1.0)).abs() < 0.5);
        // paint_widget is exactly the solid subset.
        assert_eq!(
            ok(paint_widget(&tree, editor, &theme, &test_scales(), 1.0)),
            paints
        );
        // The point sliders paint nothing.
        for id in point_ids(&tree, editor) {
            assert!(solids(&tree, id).is_empty());
        }
    }

    #[test]
    fn every_marker_stays_inside_the_editor_at_every_extreme() {
        let (mut tree, editor) = inserted_with(curve(&[p(0.0, 1.0), p(0.5, 0.0), p(1.0, 0.0)]));
        ok(select_curve_point(&mut tree, editor, 2));
        // Everything after the well's fill and border (which straddles
        // the edge, as every bordered surface in this crate does).
        for (i, (mesh, _)) in solids(&tree, editor).iter().enumerate().skip(2) {
            let (x0, y0, x1, y1) = extent(mesh);
            assert!(
                x0 >= -0.05 && y0 >= -0.05 && x1 <= SIZE + 0.05 && y1 <= SIZE + 0.05,
                "shape {i}: {:?}",
                (x0, y0, x1, y1)
            );
        }
        // Rings: outer radius spacing.xs, stroked MARKER_RING_WIDTH wide.
        let paints = solids(&tree, editor);
        let Some((outer, _)) = paints.get(paints.len() - 2) else {
            unreachable!()
        };
        let (x0, _, x1, _) = extent(outer);
        assert!((x1 - x0 - (16.0 + MARKER_RING_WIDTH)).abs() < 0.05);
        assert!(
            (x1 - (SIZE - REACH + 8.0 + 0.5)).abs() < 0.05,
            "flush at the right edge"
        );
    }

    #[test]
    fn a_disabled_editor_dims_every_shape() {
        let theme = dark_theme();
        let (mut tree, editor) = inserted();
        ok(set_curve_editor_disabled(&mut tree, editor, true));
        let paints = solids(&tree, editor);
        assert_eq!(paints.len(), 17);
        assert!(
            paints
                .iter()
                .all(|(_, c)| c[3] == theme.state.disabled_opacity)
        );
    }

    #[test]
    fn a_clipped_editor_keeps_only_the_clipped_fills() {
        let theme = dark_theme();
        let (mut tree, root) = new_tree(sized_root());
        let clip = ok(insert_container(
            &mut tree,
            root,
            Style {
                size: Size {
                    width: length(96.0_f32),
                    height: length(200.0_f32),
                },
                overflow: taffy::Point {
                    x: Overflow::Hidden,
                    y: Overflow::Hidden,
                },
                ..Default::default()
            },
        ));
        let editor = ok(insert_curve_editor(
            &mut tree,
            clip,
            &test_scales(),
            "C",
            SIZE,
            three(),
        ));
        tree.compute_layout(320.0, 320.0);
        let paints = solids(&tree, editor);
        let colours: Vec<[f32; 4]> = paints.iter().map(|(_, c)| *c).collect();
        // Well fill plus the five bands left of x = 96 (the vertical band
        // at three quarters, 91.5..92.5, survives; all three horizontal
        // ones are cut to width).
        let mut want = vec![rgba(theme.surface.sunken, 1.0)];
        want.extend([rgba(theme.border.default, 1.0); 6]);
        assert_eq!(colours, want);
        for (mesh, _) in &paints {
            let (_, _, x1, _) = extent(mesh);
            assert!(x1 <= 96.0 + 0.05, "{x1}");
        }
    }

    /// Review C1: a narrow peak between two uniform samples. The knot
    /// at `y = 1` must be drawn at `y = 1` — the pre-fix polyline,
    /// sampled on the `1/256` grid only, drew its top at `y = 0.5`.
    #[test]
    fn a_narrow_peak_is_drawn_to_its_knot() {
        let step = MIN_POINT_SEPARATION;
        let peak = curve(&[
            p(0.0, 0.0),
            p(0.5 - 0.5 * step, 0.0),
            p(0.5 + 0.5 * step, 1.0),
            p(0.5 + 1.5 * step, 0.0),
            p(1.0, 0.0),
        ]);
        let (tree, editor) = inserted_with(peak);
        let paints = solids(&tree, editor);
        let Some((line, colour)) = paints.get(9) else {
            unreachable!()
        };
        assert_eq!(*colour, rgba(dark_theme().text.primary, 1.0));
        let (_, top, _, _) = extent(line);
        // y = 1 is the plot's top edge (`REACH`). The 2 px stroke's join
        // at a spike this sharp sits within its half-width of the vertex.
        assert!(
            (REACH - 1.5..=REACH + 0.5).contains(&top),
            "curve top at {top}, want ~{REACH}"
        );
    }

    /// Every knot is a polyline vertex at its exact output; the grid is
    /// kept; the count is bounded; a curve whose knots all sit on the
    /// grid (the gallery's) adds nothing, so its paint is unchanged.
    #[test]
    fn the_polyline_samples_include_every_knot() {
        let step = MIN_POINT_SEPARATION;
        let zig: Vec<CurvePoint> = (0..MAX_POINTS)
            .map(|i| {
                let x = if i + 1 == MAX_POINTS {
                    1.0
                } else {
                    0.3 + i as f32 * 1.5 * step
                };
                p(if i == 0 { 0.0 } else { x }, (i % 2) as f32)
            })
            .collect();
        for c in [three(), curve(&zig), ToneCurve::identity()] {
            let samples = crate::paint::curve_polyline_samples(&c);
            assert!(samples.len() <= 257 + MAX_POINTS - 2);
            assert!(samples.windows(2).all(|w| matches!(w, [a, b] if a.x < b.x)));
            for knot in c.points() {
                assert!(samples.contains(knot), "{knot:?} missing");
            }
            for i in 0..=256_u16 {
                let x = f32::from(i) / 256.0;
                assert!(samples.iter().any(|s| s.x == x), "grid {x} missing");
            }
        }
        let on_grid = curve(&[p(0.0, 0.0), p(0.25, 0.15), p(0.75, 0.85), p(1.0, 1.0)]);
        assert_eq!(crate::paint::curve_polyline_samples(&on_grid).len(), 257);
    }

    /// Review C9: children found under a point slider are removed by
    /// the next reconcile rather than left orphaned.
    #[test]
    fn children_under_a_point_are_removed_on_reconcile() {
        let (mut tree, editor) = inserted();
        let Some(point) = snapshot(&tree, editor).point_id(1) else {
            unreachable!()
        };
        let stray = ok(insert_button(&mut tree, point, &test_scales(), "Stray"));
        ok(select_curve_point(&mut tree, editor, 0));
        assert!(!tree.contains(stray));
        assert!(tree.contains(point));
        assert_sound(&tree, editor);
    }

    #[test]
    fn a_tiny_editor_paints_the_well_only() {
        let (mut tree, root) = new_tree(sized_root());
        let editor = ok(insert_curve_editor(
            &mut tree,
            root,
            &test_scales(),
            "C",
            2.0 * REACH,
            three(),
        ));
        tree.compute_layout(320.0, 320.0);
        assert_eq!(solids(&tree, editor).len(), 2);
    }
}
