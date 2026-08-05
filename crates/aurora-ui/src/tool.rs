//! Which interactive canvas tool is active, and the pure geometry each
//! one needs. PLAN.md M1.9's "basic tools" bullet: Move, Marquee Select,
//! Zoom, Pan, Eyedropper — plus `Brush`/`Eraser` (M1.9's separate "basic
//! brush and eraser" bullet), included here since they're canvas tools
//! like any other.
//!
//! **Scope, stated honestly.** This module is deliberately just the tool
//! identity and the coordinate math a caller (`aurora-app`, which owns
//! the live pointer events, `aurora_doc::LayerTree`/`SelectionSet`, and
//! [`crate::CanvasView`]) drives — the same "generic mechanism, caller
//! owns behaviour" split [`crate::panel`]/`aurora_widgets::widgets::dialog`
//! already draw. Zoom and Pan are pure view-transform operations
//! ([`crate::CanvasView`] itself) and need nothing from this module
//! beyond the enum variant. Marquee Select needs [`marquee_rect`], real
//! and tested here. `Brush`/`Eraser` are real too, but their actual
//! pixel math (`aurora_brush::stamp_dab`/`erase_dab`) needs a live
//! `aurora_tile::TileStore` this crate has no reason to own — that lives
//! in `aurora-app`, the one place a live document and a live store both
//! exist. **Move and Eyedropper are not wired to anything yet**: Move
//! needs a notion of "the active layer" a user can *change* —
//! click-to-select routing now exists (`aurora_ui::layers_panel`'s own
//! rows are real, clickable widgets, and
//! `aurora_widgets::WidgetTree::hit_test` plus `aurora-app`'s own
//! `select_layer` turn a click into "this is now the active layer") —
//! but Move's own drag-to-reposition-bounds logic still isn't built on
//! top of it; Eyedropper needs to sample a real pixel, which is real now
//! (`aurora_tile::TileStore`, ADR 0010) but has no sampling function
//! built yet. Both are real, selectable tool variants — so a user (or
//! test) can switch to them — but their pointer handling is deliberately
//! a no-op today, not a faked partial behaviour.

use aurora_core::Rect;

/// Which canvas tool is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    /// Repositions a layer's bounds. **Not wired yet** — see this
    /// module's own doc comment (active-layer selection now exists;
    /// Move's own drag-to-reposition logic on top of it doesn't).
    Move,
    /// Drags out a rectangular selection ([`aurora_doc::SelectionSet`]).
    /// The only tool with real, working pointer logic so far —
    /// [`marquee_rect`]. The `#[default]` variant: a real image editor's
    /// usual default is `Move`, but `Move` doesn't do anything yet (see
    /// this module's own doc comment), and defaulting to a tool that
    /// silently does nothing would be worse than defaulting to one that
    /// actually works.
    #[default]
    MarqueeSelect,
    /// Zooms [`crate::CanvasView`] — real, via
    /// [`crate::CanvasView::zoom_at`]; this variant exists so a caller
    /// can offer it as an explicit tool choice (a click-to-zoom marquee,
    /// say) even though plain scroll-to-zoom works regardless of which
    /// tool is active.
    Zoom,
    /// Pans [`crate::CanvasView`] by a drag — real, via
    /// [`crate::CanvasView::pan_by`].
    Pan,
    /// Samples a pixel's colour. **Not wired yet** — see this module's
    /// own doc comment (no sampling function built on top of
    /// `aurora_tile::TileStore` yet).
    Eyedropper,
    /// Paints — real, via `aurora_brush::stamp_dab`/`stamp_stroke`
    /// against `aurora-app`'s own live `aurora_tile::TileStore` and
    /// active layer. See this module's own doc comment for why the
    /// actual stamping logic lives there, not here.
    Brush,
    /// Erases — real, via `aurora_brush::erase_dab`/`erase_stroke`
    /// against the same live `aurora_tile::TileStore` and active layer
    /// `Brush` uses, subtractive instead of blended. Same reason the
    /// actual erasing logic lives in `aurora-app`, not here.
    Eraser,
}

impl Tool {
    /// Every tool this enum has today, in the fixed order they're
    /// offered to the user (matches PLAN.md's own bullets: Move, Marquee
    /// Select, Zoom, Pan, Eyedropper, then Brush, Eraser).
    pub const ALL: [Self; 7] = [
        Self::Move,
        Self::MarqueeSelect,
        Self::Zoom,
        Self::Pan,
        Self::Eyedropper,
        Self::Brush,
        Self::Eraser,
    ];

    /// A short, human-readable label — for a future tool palette/status
    /// bar, and for this crate's own tests.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Move => "Move",
            Self::MarqueeSelect => "Marquee Select",
            Self::Zoom => "Zoom",
            Self::Pan => "Pan",
            Self::Eyedropper => "Eyedropper",
            Self::Brush => "Brush",
            Self::Eraser => "Eraser",
        }
    }
}

/// The axis-aligned [`Rect`] a marquee-select drag from `start` to
/// `current` (both document-space points) spans. Handles every drag
/// direction — up-left, up-right, down-left, down-right — not just
/// "down and to the right," by taking the min corner and the absolute
/// size rather than assuming `current` is past `start` on both axes.
///
/// Document space is `f32` (screen-derived, via
/// [`crate::CanvasView::to_document`]) but [`Rect`] is integer — this
/// rounds rather than truncating, so a drag that lands exactly on a
/// pixel boundary doesn't shrink by one pixel from float error.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn marquee_rect(start: (f32, f32), current: (f32, f32)) -> Rect {
    let x = start.0.min(current.0);
    let y = start.1.min(current.1);
    let width = (start.0 - current.0).abs();
    let height = (start.1 - current.1).abs();
    Rect {
        x: x.round() as i64,
        y: y.round() as i64,
        width: width.round() as u32,
        height: height.round() as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::{Rect, Tool, marquee_rect};

    #[test]
    fn default_tool_is_marquee_select_not_move() {
        assert_eq!(Tool::default(), Tool::MarqueeSelect);
    }

    #[test]
    fn all_lists_every_variant_once() {
        assert_eq!(Tool::ALL.len(), 7);
        let mut seen = Tool::ALL.to_vec();
        seen.sort_by_key(|tool| tool.label());
        seen.dedup();
        assert_eq!(seen.len(), 7, "ALL must not repeat a variant");
    }

    #[test]
    fn label_is_distinct_for_every_tool() {
        let labels: Vec<&str> = Tool::ALL.iter().map(|tool| tool.label()).collect();
        let mut sorted = labels.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), labels.len());
    }

    #[test]
    fn marquee_rect_handles_a_down_right_drag() {
        let rect = marquee_rect((10.0, 10.0), (30.0, 25.0));
        assert_eq!(
            rect,
            Rect {
                x: 10,
                y: 10,
                width: 20,
                height: 15
            }
        );
    }

    #[test]
    fn marquee_rect_handles_an_up_left_drag() {
        // Dragged from bottom-right back up to top-left -- must produce
        // the same rect as the equivalent down-right drag, not a
        // negative-size or nonsensical one.
        let rect = marquee_rect((30.0, 25.0), (10.0, 10.0));
        assert_eq!(
            rect,
            Rect {
                x: 10,
                y: 10,
                width: 20,
                height: 15
            }
        );
    }

    #[test]
    fn marquee_rect_handles_up_right_and_down_left_drags() {
        let up_right = marquee_rect((10.0, 25.0), (30.0, 10.0));
        assert_eq!(
            up_right,
            Rect {
                x: 10,
                y: 10,
                width: 20,
                height: 15
            }
        );

        let down_left = marquee_rect((30.0, 10.0), (10.0, 25.0));
        assert_eq!(
            down_left,
            Rect {
                x: 10,
                y: 10,
                width: 20,
                height: 15
            }
        );
    }

    #[test]
    fn marquee_rect_of_a_zero_size_drag_is_empty() {
        let rect = marquee_rect((5.0, 5.0), (5.0, 5.0));
        assert!(rect.is_empty());
    }
}
