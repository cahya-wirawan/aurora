//! A gradient bar (task 113): a horizontal preview of a list of colour
//! stops with one marker per stop under it. Generic: it knows nothing
//! about adjustments or documents, only a list of
//! [`GradientBarStop`]s (a position `0..=1` and an sRGB colour) and which
//! one is selected.
//!
//! # What it shows
//!
//! - **The bar** is the gradient itself, a vertex-coloured
//!   [`aurora_vector::ColorMesh`] (the colour picker's gradient path): one
//!   quad per span between two stops, each end flat out to the edge of the
//!   bar, a hard step (two stops at one position) a zero-width span so the
//!   later stop wins from that position on, exactly as the Gradient Map
//!   evaluates it. Stops are opaque; there is no checkerboard.
//! - **A marker per stop**, a square under the bar centred on the stop's
//!   position, filled with the stop's colour (content, not a token) and
//!   outlined with `border.control` like a colour swatch; the selected
//!   stop's marker is outlined in `accent.primary` at
//!   `size.indicator_width`. The track the positions map onto is the
//!   widget's width less one marker, so a marker at `0` or `1` stays
//!   inside the widget.
//!
//! # Interaction
//!
//! Pointer only, routed by [`crate::handle_pointer`]: a press on (within
//! one marker width of) a marker selects it and grabs it, and each move
//! while grabbed sets the selected stop's position from the pointer
//! ([`move_selected_stop_from_point`]): rounded to one tenth of a percent
//! and **clamped between the neighbouring stops**, so a drag never
//! reorders the stops (to reorder, delete and add). When markers are
//! within reach of the pointer together the nearest wins, then the
//! selected one, then the lowest index; stacked stops are told apart with
//! the owner's previous/next controls. **The bar is not a tab stop and
//! its markers have no keyboard model**: keyboard and assistive-technology
//! users select and move a stop with the owner's buttons and sliders, and
//! the bar says what it holds in its description.
//!
//! # Accessibility
//!
//! One `Role::Image` node named by the owner's label, never focusable,
//! no actions, whose description lists every stop in text
//! (`Stop 1 of 3: position 0 percent, colour 0 0 0; ...`, colour as
//! `0..=255` integers) and ends with the selected stop.

use accesskit::{Node, Role};
use aurora_core::Rect;
use aurora_theme::Scales;
use taffy::style_helpers::length;
use taffy::{Dimension, Size, Style};

use super::WidgetKind;
use crate::error::WidgetError;
use crate::tree::{WidgetId, WidgetTree};

/// The most stops a bar holds (a bound on the mesh and the description; the
/// owner's own cap is lower).
pub const GRADIENT_BAR_MAX_STOPS: usize = 256;

/// The finest a drag sets a position: one tenth of a percent.
pub const GRADIENT_BAR_POSITION_STEP: f32 = 0.001;

/// One stop of a [`GradientBarState`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GradientBarStop {
    /// Position along the bar, `0..=1`.
    pub position: f32,
    /// Straight sRGB `[red, green, blue]`, each `0..=1`.
    pub colour: [f32; 3],
}

impl GradientBarStop {
    /// A stop.
    #[must_use]
    pub const fn new(position: f32, colour: [f32; 3]) -> Self {
        Self { position, colour }
    }
}

/// A gradient bar's state.
#[derive(Debug, Clone, PartialEq)]
pub struct GradientBarState {
    label: String,
    stops: Vec<GradientBarStop>,
    selected: usize,
    disabled: bool,
    bar_height: u32,
    marker: u32,
}

impl GradientBarState {
    /// The accessible name.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The stops, in order of position.
    #[must_use]
    pub fn stops(&self) -> &[GradientBarStop] {
        &self.stops
    }

    /// The selected stop's index (`0` when there are no stops).
    #[must_use]
    pub const fn selected(&self) -> usize {
        self.selected
    }

    /// Whether the bar is disabled (dimmed, ignores the pointer).
    #[must_use]
    pub const fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// The bar's height in logical px (a spacing token).
    #[must_use]
    pub const fn bar_height(&self) -> u32 {
        self.bar_height
    }

    /// A marker's side in logical px (a spacing token).
    #[must_use]
    pub const fn marker(&self) -> u32 {
        self.marker
    }
}

/// What a bar mutator did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GradientBarOutcome {
    /// The bar's stops or selection changed.
    Changed,
    /// Nothing changed.
    Ignored,
}

// ---- geometry --------------------------------------------------------------

/// Where the parts sit in a bar's layout bounds (logical px).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BarGeometry {
    /// The track's left edge (position `0`).
    pub left: f32,
    /// The track's width (position `0` to `1`).
    pub width: f32,
    /// The bar's top edge.
    pub bar_top: f32,
    /// The bar's height.
    pub bar_height: f32,
    /// A marker's side.
    pub marker: f32,
}

impl BarGeometry {
    /// The x of `position`.
    pub(crate) fn x_of(&self, position: f32) -> f32 {
        self.left + position * self.width
    }

    /// The position at `x`, clamped to `0..=1`.
    pub(crate) fn position_at(&self, x: f32) -> f32 {
        // `max` then `min`: `f32::clamp` panics on crossed bounds and a
        // `NaN` must not get through as a position.
        ((x - self.left) / self.width).max(0.0).min(1.0)
    }

    /// A marker's rectangle `(x, y, side, side)` for `position`.
    pub(crate) fn marker_rect(&self, position: f32) -> (f32, f32, f32, f32) {
        (
            self.x_of(position) - self.marker / 2.0,
            self.bar_top + self.bar_height,
            self.marker,
            self.marker,
        )
    }
}

/// The geometry of a bar laid out at `bounds`; `None` when the track has
/// no usable width.
pub(crate) fn geometry(bounds: Rect, state: &GradientBarState) -> Option<BarGeometry> {
    let marker = state.marker as f32;
    let width = bounds.width as f32 - marker;
    (width >= 1.0).then_some(BarGeometry {
        left: bounds.x as f32 + marker / 2.0,
        width,
        bar_top: bounds.y as f32,
        bar_height: state.bar_height as f32,
        marker,
    })
}

/// One span of the bar's mesh: from `from` to `to` (positions), the colour
/// running from `from_colour` to `to_colour`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Span {
    pub from: f32,
    pub to: f32,
    pub from_colour: [f32; 3],
    pub to_colour: [f32; 3],
}

/// The spans that draw `stops` over `0..=1`: a flat span from `0` to the
/// first stop and from the last stop to `1` when they do not already touch
/// the ends, and a graded span between each pair of stops that are apart.
/// Two stops at one position leave no span between them, so the later
/// stop's colour starts there.
pub(crate) fn spans(stops: &[GradientBarStop]) -> Vec<Span> {
    let (Some(first), Some(last)) = (stops.first(), stops.last()) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(stops.len() + 1);
    if first.position > 0.0 {
        out.push(Span {
            from: 0.0,
            to: first.position,
            from_colour: first.colour,
            to_colour: first.colour,
        });
    }
    for pair in stops.windows(2) {
        if let [a, b] = pair
            && b.position > a.position
        {
            out.push(Span {
                from: a.position,
                to: b.position,
                from_colour: a.colour,
                to_colour: b.colour,
            });
        }
    }
    if last.position < 1.0 {
        out.push(Span {
            from: last.position,
            to: 1.0,
            from_colour: last.colour,
            to_colour: last.colour,
        });
    }
    out
}

/// The colour the bar shows at position `t`: the model [`spans`] draws
/// (the later of two stops at one position wins, the ends held flat).
#[must_use]
pub fn gradient_bar_colour_at(stops: &[GradientBarStop], t: f32) -> Option<[f32; 3]> {
    let mut colour = stops.first()?.colour;
    for (i, stop) in stops.iter().enumerate() {
        if t >= stop.position {
            colour = stop.colour;
            if let Some(next) = stops.get(i + 1)
                && t < next.position
            {
                let f = (t - stop.position) / (next.position - stop.position);
                colour = [0, 1, 2].map(|c| {
                    let (x, y) = (
                        stop.colour.get(c).copied().unwrap_or(0.0),
                        next.colour.get(c).copied().unwrap_or(0.0),
                    );
                    x + (y - x) * f
                });
            }
        }
    }
    Some(colour)
}

// ---- text ------------------------------------------------------------------

/// `position` (`0..=1`) as percent text: whole numbers plain, otherwise
/// one decimal (`12.5`).
fn percent_text(position: f32) -> String {
    #[allow(clippy::cast_possible_truncation)]
    let tenths = (position.max(0.0).min(1.0) * 1000.0).round() as i64;
    if tenths % 10 == 0 {
        format!("{}", tenths / 10)
    } else {
        format!("{}.{}", tenths / 10, tenths % 10)
    }
}

/// A `0..=1` channel as the `0..=255` integer a person reads.
fn level(channel: f32) -> u32 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let v = (channel.max(0.0).min(1.0) * 255.0).round() as u32;
    v
}

/// The text a screen reader gets for a bar's stops.
#[must_use]
pub fn gradient_bar_description(stops: &[GradientBarStop], selected: usize) -> String {
    let count = stops.len();
    let mut text = stops
        .iter()
        .enumerate()
        .map(|(i, s)| {
            format!(
                "Stop {} of {count}: position {} percent, colour {} {} {}",
                i + 1,
                percent_text(s.position),
                level(s.colour[0]),
                level(s.colour[1]),
                level(s.colour[2]),
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    if count > 0 {
        text.push_str(&format!(". Selected: stop {}", selected + 1));
    }
    text
}

fn node(state: &GradientBarState) -> Node {
    let mut node = Node::new(Role::Image);
    node.set_label(state.label.clone());
    node.set_description(gradient_bar_description(&state.stops, state.selected));
    if state.disabled {
        node.set_disabled();
    }
    node
}

fn style(state: &GradientBarState) -> Style {
    Style {
        size: Size {
            width: Dimension::auto(),
            height: length((state.bar_height + state.marker) as f32),
        },
        flex_shrink: 0.0,
        ..Default::default()
    }
}

/// Stops made safe to hold: at most [`GRADIENT_BAR_MAX_STOPS`], every
/// number finite (a non-finite one becomes `0`), positions and channels in
/// `0..=1`, and positions never decreasing (a stop before its predecessor
/// is raised to it).
fn sanitised(stops: &[GradientBarStop]) -> Vec<GradientBarStop> {
    let unit = |v: f32| {
        if v.is_finite() {
            v.max(0.0).min(1.0)
        } else {
            0.0
        }
    };
    let mut previous = 0.0_f32;
    stops
        .iter()
        .take(GRADIENT_BAR_MAX_STOPS)
        .map(|s| {
            let position = unit(s.position).max(previous);
            previous = position;
            GradientBarStop::new(position, s.colour.map(unit))
        })
        .collect()
}

// ---- tree functions --------------------------------------------------------

/// Adds a gradient bar showing `stops` (made safe, see the module docs),
/// with the first stop selected, as the last child of `parent`. The bar is
/// `spacing.lg` tall with `spacing.sm` markers under it and as wide as its
/// parent lays it out.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] if `parent` doesn't exist.
pub fn insert_gradient_bar(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
    label: impl Into<String>,
    stops: &[GradientBarStop],
) -> Result<WidgetId, WidgetError> {
    let state = GradientBarState {
        label: label.into(),
        stops: sanitised(stops),
        selected: 0,
        disabled: false,
        bar_height: scales.spacing.lg,
        marker: scales.spacing.sm,
    };
    tree.insert(
        parent,
        style(&state),
        node(&state),
        WidgetKind::GradientBar(state),
    )
}

/// `id`'s gradient bar state.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] / [`WidgetError::WrongWidgetKind`].
pub fn gradient_bar_state(
    tree: &WidgetTree<WidgetKind>,
    id: WidgetId,
) -> Result<&GradientBarState, WidgetError> {
    match tree.payload(id) {
        None => Err(WidgetError::UnknownWidget(id)),
        Some(WidgetKind::GradientBar(state)) => Ok(state),
        Some(_) => Err(WidgetError::WrongWidgetKind(id)),
    }
}

/// Runs `f` on the bar's state; if it reports a change, rebuilds the
/// accessibility node and marks the bar dirty.
fn mutate(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    f: impl FnOnce(&mut GradientBarState) -> Result<bool, WidgetError>,
) -> Result<GradientBarOutcome, WidgetError> {
    let (changed, rebuilt) = {
        let kind = tree.payload_mut(id).ok_or(WidgetError::UnknownWidget(id))?;
        let WidgetKind::GradientBar(state) = kind else {
            return Err(WidgetError::WrongWidgetKind(id));
        };
        let changed = f(state)?;
        (changed, node(state))
    };
    if !changed {
        return Ok(GradientBarOutcome::Ignored);
    }
    tree.set_accessibility(id, rebuilt)?;
    tree.mark_dirty(id)?;
    Ok(GradientBarOutcome::Changed)
}

/// Replaces the bar's stops with `stops` (made safe). The selection stays
/// on the same index, moved back to the last stop if there are fewer.
/// Setting what it already holds touches nothing, so an owner can mirror
/// its document every loop turn.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] / [`WidgetError::WrongWidgetKind`].
pub fn set_gradient_bar_stops(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    stops: &[GradientBarStop],
) -> Result<GradientBarOutcome, WidgetError> {
    let stops = sanitised(stops);
    mutate(tree, id, |state| {
        let selected = state.selected.min(stops.len().saturating_sub(1));
        let changed = state.stops != stops || state.selected != selected;
        state.stops = stops;
        state.selected = selected;
        Ok(changed)
    })
}

/// Selects stop `index`.
///
/// # Errors
///
/// [`WidgetError::IndexOutOfRange`] for a missing stop,
/// [`WidgetError::WidgetDisabled`] on a disabled bar, and the usual two.
pub fn select_gradient_stop(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    index: usize,
) -> Result<GradientBarOutcome, WidgetError> {
    mutate(tree, id, |state| {
        if state.disabled {
            return Err(WidgetError::WidgetDisabled(id));
        }
        if index >= state.stops.len() {
            return Err(WidgetError::IndexOutOfRange {
                index,
                len: state.stops.len(),
            });
        }
        let changed = state.selected != index;
        state.selected = index;
        Ok(changed)
    })
}

/// Enables or disables the bar.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] / [`WidgetError::WrongWidgetKind`].
pub fn set_gradient_bar_disabled(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    disabled: bool,
) -> Result<GradientBarOutcome, WidgetError> {
    mutate(tree, id, |state| {
        let changed = state.disabled != disabled;
        state.disabled = disabled;
        Ok(changed)
    })
}

/// The stop whose marker `(x, y)` (window space, logical px) grabs: the
/// nearest centre within one marker width, ties to the selected stop and
/// then the lowest index. `y` is ignored (the whole bar is the target
/// strip) but the point must be inside the bar's vertical extent. `None`
/// for a disabled bar, a track with no width, or a non-finite point.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] / [`WidgetError::WrongWidgetKind`].
pub fn gradient_bar_stop_at(
    tree: &WidgetTree<WidgetKind>,
    id: WidgetId,
    x: f32,
    y: f32,
) -> Result<Option<usize>, WidgetError> {
    let state = gradient_bar_state(tree, id)?;
    let Some(bounds) = tree.bounds(id) else {
        return Ok(None);
    };
    let Some(g) = geometry(bounds, state) else {
        return Ok(None);
    };
    if state.disabled || !x.is_finite() || !y.is_finite() {
        return Ok(None);
    }
    let (top, bottom) = (g.bar_top, g.bar_top + g.bar_height + g.marker);
    if y < top || y >= bottom {
        return Ok(None);
    }
    let reach = g.marker;
    let mut best: Option<(usize, f32)> = None;
    for (i, stop) in state.stops.iter().enumerate() {
        let distance = (g.x_of(stop.position) - x).abs();
        if distance > reach {
            continue;
        }
        let better = match best {
            None => true,
            Some((j, d)) => {
                distance < d || (distance == d && i == state.selected && j != state.selected)
            }
        };
        if better {
            best = Some((i, distance));
        }
    }
    Ok(best.map(|(i, _)| i))
}

/// Moves the selected stop to the position under `x`: rounded to
/// [`GRADIENT_BAR_POSITION_STEP`] and clamped between the neighbouring
/// stops (the first stop's lower bound is `0`, the last's upper bound
/// `1`). `Ignored` when that is where it already is, the point is not
/// finite, or the track has no width.
///
/// # Errors
///
/// [`WidgetError::WidgetDisabled`] on a disabled bar, and the usual two.
pub fn move_selected_stop_from_point(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    x: f32,
    _y: f32,
) -> Result<GradientBarOutcome, WidgetError> {
    let bounds = tree.bounds(id);
    mutate(tree, id, |state| {
        if state.disabled {
            return Err(WidgetError::WidgetDisabled(id));
        }
        let Some(g) = bounds.and_then(|b| geometry(b, state)) else {
            return Ok(false);
        };
        if !x.is_finite() {
            return Ok(false);
        }
        let step = GRADIENT_BAR_POSITION_STEP;
        let wanted = (g.position_at(x) / step).round() * step;
        let i = state.selected;
        let low = i
            .checked_sub(1)
            .and_then(|before| state.stops.get(before))
            .map_or(0.0, |s| s.position);
        let high = state.stops.get(i + 1).map_or(1.0, |s| s.position);
        let position = wanted.max(low).min(high);
        let Some(stop) = state.stops.get_mut(i) else {
            return Ok(false);
        };
        let changed = stop.position.to_bits() != position.to_bits();
        stop.position = position;
        Ok(changed)
    })
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::float_cmp)]
mod tests {
    use super::*;
    use crate::widgets::{new_tree, test_scales};

    fn stops3() -> Vec<GradientBarStop> {
        vec![
            GradientBarStop::new(0.0, [0.0, 0.0, 0.0]),
            GradientBarStop::new(0.5, [1.0, 0.0, 0.0]),
            GradientBarStop::new(1.0, [1.0, 1.0, 1.0]),
        ]
    }

    /// A 200 px wide bar at the origin: track 188 px from x = 6.
    fn bar(stops: &[GradientBarStop]) -> (WidgetTree<WidgetKind>, WidgetId) {
        let (mut tree, root) = new_tree(Style {
            display: taffy::Display::Flex,
            flex_direction: taffy::FlexDirection::Column,
            size: Size {
                width: length(200.0_f32),
                height: length(100.0_f32),
            },
            ..Default::default()
        });
        let scales = test_scales();
        let id = match insert_gradient_bar(&mut tree, root, &scales, "Gradient preview", stops) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.compute_layout(200.0, 100.0);
        (tree, id)
    }

    fn state(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> GradientBarState {
        match gradient_bar_state(tree, id) {
            Ok(s) => s.clone(),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn x_of(tree: &WidgetTree<WidgetKind>, id: WidgetId, position: f32) -> f32 {
        let s = state(tree, id);
        let Some(b) = tree.bounds(id) else {
            unreachable!("laid out")
        };
        match geometry(b, &s) {
            Some(g) => g.x_of(position),
            None => unreachable!("has a track"),
        }
    }

    #[test]
    fn the_bar_is_sized_from_spacing_tokens_and_fills_its_parent() {
        let (tree, id) = bar(&stops3());
        let scales = test_scales();
        let Some(b) = tree.bounds(id) else {
            unreachable!("laid out")
        };
        assert_eq!(b.width, 200);
        assert_eq!(b.height, scales.spacing.lg + scales.spacing.sm);
        let s = state(&tree, id);
        assert_eq!(
            (s.bar_height(), s.marker()),
            (scales.spacing.lg, scales.spacing.sm)
        );
    }

    #[test]
    fn the_track_leaves_room_for_the_end_markers() {
        let (tree, id) = bar(&stops3());
        let s = state(&tree, id);
        let Some(b) = tree.bounds(id) else {
            unreachable!("laid out")
        };
        let Some(g) = geometry(b, &s) else {
            unreachable!("track")
        };
        assert_eq!(g.left, 6.0);
        assert_eq!(g.width, 188.0);
        let (x0, _, side, _) = g.marker_rect(0.0);
        let (x1, _, _, _) = g.marker_rect(1.0);
        assert_eq!(x0, 0.0, "the marker at 0 starts at the widget's edge");
        assert_eq!(x1 + side, 200.0, "and the marker at 1 ends at the other");
        assert_eq!(g.position_at(-50.0), 0.0);
        assert_eq!(g.position_at(500.0), 1.0);
        assert_eq!(g.position_at(f32::NAN), 0.0);
    }

    #[test]
    fn a_track_narrower_than_a_pixel_has_no_geometry() {
        let (mut tree, root) = new_tree(Style::default());
        let id = match insert_gradient_bar(&mut tree, root, &test_scales(), "g", &stops3()) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.compute_layout(0.0, 0.0);
        assert_eq!(gradient_bar_stop_at(&tree, id, 0.0, 1.0).ok(), Some(None));
        assert_eq!(
            move_selected_stop_from_point(&mut tree, id, 5.0, 1.0).ok(),
            Some(GradientBarOutcome::Ignored)
        );
    }

    #[test]
    fn spans_cover_zero_to_one_and_a_hard_step_leaves_no_span() {
        let hard = [
            GradientBarStop::new(0.0, [0.0; 3]),
            GradientBarStop::new(0.5, [0.2; 3]),
            GradientBarStop::new(0.5, [0.8; 3]),
            GradientBarStop::new(1.0, [1.0; 3]),
        ];
        let s = spans(&hard);
        assert_eq!(s.len(), 2, "two graded spans, none across the step");
        assert_eq!((s[0].from, s[0].to), (0.0, 0.5));
        assert_eq!((s[1].from, s[1].to), (0.5, 1.0));
        assert_eq!(s[0].to_colour, [0.2; 3]);
        assert_eq!(
            s[1].from_colour, [0.8; 3],
            "the later stop starts the next span"
        );
        // Flat ends when the first/last stop is inside the bar.
        let inner = [
            GradientBarStop::new(0.25, [0.1; 3]),
            GradientBarStop::new(0.75, [0.9; 3]),
        ];
        let s = spans(&inner);
        assert_eq!(s.len(), 3);
        assert_eq!((s[0].from, s[0].to), (0.0, 0.25));
        assert_eq!(s[0].from_colour, [0.1; 3]);
        assert_eq!((s[2].from, s[2].to), (0.75, 1.0));
        assert_eq!(s[2].to_colour, [0.9; 3]);
        assert!(spans(&[]).is_empty());
    }

    #[test]
    fn the_colour_model_agrees_with_the_spans() {
        let stops = [
            GradientBarStop::new(0.2, [0.0, 0.0, 0.0]),
            GradientBarStop::new(0.6, [1.0, 0.5, 0.0]),
            GradientBarStop::new(0.6, [0.0, 1.0, 0.0]),
            GradientBarStop::new(0.9, [0.0, 0.0, 1.0]),
        ];
        assert_eq!(gradient_bar_colour_at(&stops, 0.0), Some([0.0; 3]));
        let mid = gradient_bar_colour_at(&stops, 0.4).unwrap_or([9.0; 3]);
        assert!((mid[0] - 0.5).abs() < 1e-5 && (mid[1] - 0.25).abs() < 1e-5 && mid[2] == 0.0);
        assert_eq!(
            gradient_bar_colour_at(&stops, 0.6),
            Some([0.0, 1.0, 0.0]),
            "the later stop wins at a shared position"
        );
        assert_eq!(gradient_bar_colour_at(&stops, 1.0), Some([0.0, 0.0, 1.0]));
        assert_eq!(gradient_bar_colour_at(&[], 0.5), None);
    }

    #[test]
    fn a_press_finds_the_nearest_marker_and_misses_far_from_all() {
        let (tree, id) = bar(&stops3());
        let (a, b, c) = (
            x_of(&tree, id, 0.0),
            x_of(&tree, id, 0.5),
            x_of(&tree, id, 1.0),
        );
        let y = 5.0;
        assert_eq!(gradient_bar_stop_at(&tree, id, a, y).ok(), Some(Some(0)));
        assert_eq!(
            gradient_bar_stop_at(&tree, id, b + 3.0, y).ok(),
            Some(Some(1))
        );
        assert_eq!(
            gradient_bar_stop_at(&tree, id, c - 2.0, y).ok(),
            Some(Some(2))
        );
        assert_eq!(
            gradient_bar_stop_at(&tree, id, (a + b) / 2.0, y).ok(),
            Some(None)
        );
        // Within reach (one marker width = 12) but not beyond.
        assert_eq!(
            gradient_bar_stop_at(&tree, id, b + 12.0, y).ok(),
            Some(Some(1))
        );
        assert_eq!(
            gradient_bar_stop_at(&tree, id, b + 12.5, y).ok(),
            Some(None)
        );
        // Outside the bar's vertical extent, or not finite.
        assert_eq!(gradient_bar_stop_at(&tree, id, b, -1.0).ok(), Some(None));
        assert_eq!(gradient_bar_stop_at(&tree, id, b, 36.0).ok(), Some(None));
        assert_eq!(
            gradient_bar_stop_at(&tree, id, f32::NAN, y).ok(),
            Some(None)
        );
    }

    #[test]
    fn stacked_markers_prefer_the_selected_stop_then_the_lowest() {
        let stacked = [
            GradientBarStop::new(0.0, [0.0; 3]),
            GradientBarStop::new(0.5, [0.2; 3]),
            GradientBarStop::new(0.5, [0.8; 3]),
            GradientBarStop::new(1.0, [1.0; 3]),
        ];
        let (mut tree, id) = bar(&stacked);
        let x = x_of(&tree, id, 0.5);
        assert_eq!(
            gradient_bar_stop_at(&tree, id, x, 5.0).ok(),
            Some(Some(1)),
            "lowest"
        );
        assert!(select_gradient_stop(&mut tree, id, 2).is_ok());
        assert_eq!(
            gradient_bar_stop_at(&tree, id, x, 5.0).ok(),
            Some(Some(2)),
            "selected"
        );
        // A nearer marker beats the selected one.
        let near = x_of(&tree, id, 0.0) + 1.0;
        assert_eq!(
            gradient_bar_stop_at(&tree, id, near, 5.0).ok(),
            Some(Some(0))
        );
    }

    #[test]
    fn selecting_changes_the_state_and_the_description_and_refuses_a_missing_stop() {
        let (mut tree, id) = bar(&stops3());
        assert_eq!(state(&tree, id).selected(), 0);
        assert_eq!(
            select_gradient_stop(&mut tree, id, 2).ok(),
            Some(GradientBarOutcome::Changed)
        );
        assert_eq!(state(&tree, id).selected(), 2);
        assert_eq!(
            select_gradient_stop(&mut tree, id, 2).ok(),
            Some(GradientBarOutcome::Ignored)
        );
        assert!(
            matches!(select_gradient_stop(&mut tree, id, 3), Err(e) if format!("{e:?}") == format!("{:?}", WidgetError::IndexOutOfRange { index: 3, len: 3 }))
        );
        let described = tree
            .accessibility(id)
            .and_then(|n| n.description())
            .map(str::to_owned);
        assert!(described.is_some_and(|d| d.ends_with("Selected: stop 3")));
    }

    #[test]
    fn a_drag_moves_the_selected_stop_clamped_between_its_neighbours() {
        let (mut tree, id) = bar(&stops3());
        assert!(select_gradient_stop(&mut tree, id, 1).is_ok());
        let at = |t: &WidgetTree<WidgetKind>, p: f32| x_of(t, id, p);
        let pos = |t: &WidgetTree<WidgetKind>| state(t, id).stops()[1].position;
        let x = at(&tree, 0.25);
        assert_eq!(
            move_selected_stop_from_point(&mut tree, id, x, 5.0).ok(),
            Some(GradientBarOutcome::Changed)
        );
        assert!((pos(&tree) - 0.25).abs() < 0.0006, "{}", pos(&tree));
        // Past either neighbour it stops at the neighbour, never reordering.
        assert_eq!(
            move_selected_stop_from_point(&mut tree, id, 5000.0, 5.0).ok(),
            Some(GradientBarOutcome::Changed)
        );
        assert_eq!(pos(&tree), 1.0);
        assert_eq!(
            move_selected_stop_from_point(&mut tree, id, -5000.0, 5.0).ok(),
            Some(GradientBarOutcome::Changed)
        );
        assert_eq!(pos(&tree), 0.0);
        let positions: Vec<f32> = state(&tree, id)
            .stops()
            .iter()
            .map(|s| s.position)
            .collect();
        assert!(positions.windows(2).all(|w| w[0] <= w[1]));
        // Colours and the other stops are untouched.
        assert_eq!(state(&tree, id).stops()[1].colour, [1.0, 0.0, 0.0]);
        assert_eq!(state(&tree, id).stops()[2].position, 1.0);
        // The same place twice, and a non-finite pointer, change nothing.
        assert_eq!(
            move_selected_stop_from_point(&mut tree, id, -5000.0, 5.0).ok(),
            Some(GradientBarOutcome::Ignored)
        );
        assert_eq!(
            move_selected_stop_from_point(&mut tree, id, f32::NAN, 5.0).ok(),
            Some(GradientBarOutcome::Ignored)
        );
    }

    #[test]
    fn the_end_stops_are_bounded_by_zero_and_one() {
        let (mut tree, id) = bar(&stops3());
        assert_eq!(
            move_selected_stop_from_point(&mut tree, id, 5000.0, 5.0).ok(),
            Some(GradientBarOutcome::Changed)
        );
        assert_eq!(
            state(&tree, id).stops()[0].position,
            0.5,
            "first stop stops at the second"
        );
        assert!(select_gradient_stop(&mut tree, id, 2).is_ok());
        assert_eq!(
            move_selected_stop_from_point(&mut tree, id, -5000.0, 5.0).ok(),
            Some(GradientBarOutcome::Changed)
        );
        assert_eq!(state(&tree, id).stops()[2].position, 0.5);
    }

    #[test]
    fn a_drag_snaps_to_a_tenth_of_a_percent() {
        let (mut tree, id) = bar(&stops3());
        assert!(select_gradient_stop(&mut tree, id, 1).is_ok());
        for px in [1.0_f32, 17.3, 42.9, 77.77, 101.1] {
            let _ = move_selected_stop_from_point(&mut tree, id, px, 5.0);
            let p = state(&tree, id).stops()[1].position;
            let steps = p / GRADIENT_BAR_POSITION_STEP;
            assert!(
                (steps - steps.round()).abs() < 1e-3,
                "{p} is not on the 0.1 % grid"
            );
        }
    }

    #[test]
    fn a_disabled_bar_refuses_selection_and_moves_and_finds_no_marker() {
        let (mut tree, id) = bar(&stops3());
        assert_eq!(
            set_gradient_bar_disabled(&mut tree, id, true).ok(),
            Some(GradientBarOutcome::Changed)
        );
        assert!(
            tree.accessibility(id)
                .is_some_and(accesskit::Node::is_disabled)
        );
        assert!(
            matches!(select_gradient_stop(&mut tree, id, 1), Err(e) if format!("{e:?}") == format!("{:?}", WidgetError::WidgetDisabled(id)))
        );
        assert!(
            matches!(move_selected_stop_from_point(&mut tree, id, 50.0, 5.0), Err(e) if format!("{e:?}") == format!("{:?}", WidgetError::WidgetDisabled(id)))
        );
        let x = x_of(&tree, id, 0.0);
        assert_eq!(gradient_bar_stop_at(&tree, id, x, 5.0).ok(), Some(None));
        assert_eq!(
            set_gradient_bar_disabled(&mut tree, id, false).ok(),
            Some(GradientBarOutcome::Changed)
        );
        assert_eq!(
            set_gradient_bar_disabled(&mut tree, id, false).ok(),
            Some(GradientBarOutcome::Ignored)
        );
    }

    #[test]
    fn setting_stops_keeps_the_selection_in_range_and_is_idempotent() {
        let (mut tree, id) = bar(&stops3());
        assert!(select_gradient_stop(&mut tree, id, 2).is_ok());
        assert_eq!(
            set_gradient_bar_stops(&mut tree, id, &stops3()).ok(),
            Some(GradientBarOutcome::Ignored)
        );
        let two = &stops3()[..2];
        assert_eq!(
            set_gradient_bar_stops(&mut tree, id, two).ok(),
            Some(GradientBarOutcome::Changed)
        );
        assert_eq!(
            state(&tree, id).selected(),
            1,
            "pulled back to the last stop"
        );
        assert_eq!(
            set_gradient_bar_stops(&mut tree, id, &stops3()).ok(),
            Some(GradientBarOutcome::Changed)
        );
        assert_eq!(state(&tree, id).selected(), 1, "and not moved back out");
        assert_eq!(
            set_gradient_bar_stops(&mut tree, id, &[]).ok(),
            Some(GradientBarOutcome::Changed)
        );
        assert_eq!(state(&tree, id).selected(), 0);
        assert_eq!(gradient_bar_stop_at(&tree, id, 10.0, 5.0).ok(), Some(None));
    }

    #[test]
    fn stops_are_made_safe() {
        let wild = [
            GradientBarStop::new(f32::NAN, [2.0, -1.0, f32::INFINITY]),
            GradientBarStop::new(0.7, [0.5; 3]),
            GradientBarStop::new(0.3, [0.5; 3]),
            GradientBarStop::new(9.0, [0.5; 3]),
        ];
        let (tree, id) = bar(&wild);
        let s = state(&tree, id);
        let positions: Vec<f32> = s.stops().iter().map(|s| s.position).collect();
        assert_eq!(positions, vec![0.0, 0.7, 0.7, 1.0]);
        assert_eq!(s.stops()[0].colour, [1.0, 0.0, 0.0]);
        let many = vec![GradientBarStop::new(0.5, [0.0; 3]); GRADIENT_BAR_MAX_STOPS + 10];
        let (tree, id) = bar(&many);
        assert_eq!(state(&tree, id).stops().len(), GRADIENT_BAR_MAX_STOPS);
    }

    #[test]
    fn the_description_lists_every_stop_in_text() {
        let text = gradient_bar_description(&stops3(), 1);
        assert_eq!(
            text,
            "Stop 1 of 3: position 0 percent, colour 0 0 0; \
             Stop 2 of 3: position 50 percent, colour 255 0 0; \
             Stop 3 of 3: position 100 percent, colour 255 255 255. Selected: stop 2"
        );
        let fine = [
            GradientBarStop::new(0.125, [0.5, 0.0, 0.2]),
            GradientBarStop::new(1.0, [0.0; 3]),
        ];
        assert!(
            gradient_bar_description(&fine, 0).contains("position 12.5 percent, colour 128 0 51")
        );
        assert_eq!(gradient_bar_description(&[], 0), "");
    }

    #[test]
    fn the_node_is_a_named_unfocusable_image_with_no_actions() {
        let (tree, id) = bar(&stops3());
        let Some(n) = tree.accessibility(id) else {
            unreachable!("has a node")
        };
        assert_eq!(n.role(), Role::Image);
        assert_eq!(n.label(), Some("Gradient preview"));
        assert!(!n.is_hidden());
        assert!(n.supports_action(accesskit::Action::Focus) == false);
        assert!(!n.is_disabled());
        assert!(
            n.description()
                .is_some_and(|d| d.contains("Stop 2 of 3: position 50 percent, colour 255 0 0"))
        );
    }

    #[test]
    fn the_wrong_kind_and_unknown_ids_are_errors() {
        let (mut tree, root) = new_tree(Style::default());
        assert!(matches!(
            gradient_bar_state(&tree, root),
            Err(WidgetError::WrongWidgetKind(_))
        ));
        assert!(matches!(
            set_gradient_bar_stops(&mut tree, root, &[]),
            Err(WidgetError::WrongWidgetKind(_))
        ));
    }
}
