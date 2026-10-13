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
//! and tested here. Every other variant is real too, but its actual
//! work (`aurora_brush::stamp_dab`/`erase_dab`,
//! `aurora_doc::LayerTree::set_bounds`, sampling a pixel for Eyedropper)
//! needs a live `aurora_tile::TileStore`/`LayerTree` this crate has no
//! reason to own — that lives in `aurora-app`, the one place a live
//! document and a live store both exist.

use aurora_core::Rect;

/// Which canvas tool is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    /// Repositions a layer's bounds — real, via
    /// `aurora_doc::LayerTree::set_bounds`, against `aurora-app`'s own
    /// live document and active layer. See this module's own doc
    /// comment for why the actual mutation lives there, not here.
    Move,
    /// Drags out a rectangular selection ([`aurora_doc::SelectionSet`]).
    /// The `#[default]` variant: a real image editor's usual default is
    /// `Move`, but defaulting to a tool that repositions the active
    /// layer on a plain click-drag felt like a worse first-run surprise
    /// than defaulting to a selection tool, which never mutates
    /// anything just by dragging.
    #[default]
    MarqueeSelect,
    /// Drags out an elliptical selection, the ellipse inscribed in the
    /// dragged rectangle (0.213.0): the first tool to make a
    /// non-rectangular [`aurora_doc::Selection`].
    EllipticalMarquee,
    /// Draws a freehand selection by dragging (0.214.0): the path the
    /// pointer travels, closed on release, becomes a raster selection.
    Lasso,
    /// Builds a selection from clicked corners (0.214.0): a click adds a
    /// vertex, a click near the first one, `Enter` or a double click
    /// closes the polygon.
    PolygonalLasso,
    /// Selects the pixels of the active pixel layer similar to the
    /// clicked one (0.214.0), with the Paint Bucket's tolerance and
    /// Contiguous settings.
    MagicWand,
    /// Zooms [`crate::CanvasView`] — real, via
    /// [`crate::CanvasView::zoom_at`]; this variant exists so a caller
    /// can offer it as an explicit tool choice (a click-to-zoom marquee,
    /// say) even though plain scroll-to-zoom works regardless of which
    /// tool is active.
    Zoom,
    /// Pans [`crate::CanvasView`] by a drag — real, via
    /// [`crate::CanvasView::pan_by`].
    Pan,
    /// Samples a pixel's colour — real, via `aurora-app`'s own
    /// `sample_pixel` against the active layer's live
    /// `aurora_tile::TileStore` surface, setting it as the new colour
    /// `Brush` paints with. See this module's own doc comment for why
    /// the actual sampling lives there, not here.
    Eyedropper,
    /// Paints — real, via `aurora_brush::stamp_dab`
    /// against `aurora-app`'s own live `aurora_tile::TileStore` and
    /// active layer. See this module's own doc comment for why the
    /// actual stamping logic lives there, not here.
    Brush,
    /// Erases — real, via `aurora_brush::erase_dab`
    /// against the same live `aurora_tile::TileStore` and active layer
    /// `Brush` uses, subtractive instead of blended. Same reason the
    /// actual erasing logic lives in `aurora-app`, not here.
    Eraser,
    /// Drags out a gradient from the foreground to the background colour
    /// over the active pixel layer (0.209.0): real, via
    /// `aurora_brush::gradient_fill` in `aurora-app`.
    Gradient,
    /// Fills a region of similar colour on the active pixel layer with
    /// the foreground colour (0.209.0): real, via
    /// `aurora_brush::flood_fill` in `aurora-app`.
    PaintBucket,
    /// Paints the foreground colour with a hard, aliased dab (0.220.0):
    /// real, via `aurora_brush::stamp_dab_hard` in `aurora-app`, with the
    /// Brush's radius and spacing and no opacity.
    Pencil,
    /// Copies pixels of the active pixel layer from a source point to
    /// where it is dragged, aligned (0.221.0): real, via
    /// `aurora_brush::clone_dab_covered` in `aurora-app`. `Alt`+click sets
    /// the source.
    CloneStamp,
    /// Drags the active pixel layer's pixels along the stroke (0.222.0):
    /// real, via `aurora_brush::smudge_dab_covered` in `aurora-app`.
    Smudge,
    /// Softens the active pixel layer's pixels under the brush (0.223.0):
    /// real, via `aurora_brush::filter_dab_covered` in `aurora-app`.
    Blur,
    /// Sharpens (unsharp mask) the active pixel layer's pixels under the
    /// brush (0.223.0): the same engine as the Blur.
    Sharpen,
    /// Lightens the shadows, midtones or highlights of the active pixel
    /// layer under the brush (0.224.0): real, via
    /// `aurora_brush::tone_dab_covered` in `aurora-app`.
    Dodge,
    /// Darkens the shadows, midtones or highlights of the active pixel
    /// layer under the brush (0.224.0): the Dodge's engine, reversed.
    Burn,
    /// Saturates or desaturates the active pixel layer under the brush
    /// (0.224.0): the Dodge's engine, on colourfulness.
    Sponge,
}

impl Tool {
    /// Every tool this enum has today, in the fixed order they're
    /// offered to the user (matches PLAN.md's own bullets: Move, Marquee
    /// Select, Zoom, Pan, Eyedropper, then Brush, Eraser, and the 0.209.0 fills Gradient and Paint Bucket).
    pub const ALL: [Self; 21] = [
        Self::Move,
        Self::MarqueeSelect,
        Self::EllipticalMarquee,
        Self::Lasso,
        Self::PolygonalLasso,
        Self::MagicWand,
        Self::Zoom,
        Self::Pan,
        Self::Eyedropper,
        Self::Brush,
        Self::Pencil,
        Self::Eraser,
        Self::CloneStamp,
        Self::Smudge,
        Self::Blur,
        Self::Sharpen,
        Self::Dodge,
        Self::Burn,
        Self::Sponge,
        Self::Gradient,
        Self::PaintBucket,
    ];

    /// Whether this tool makes a selection and so has the New / Add /
    /// Subtract / Intersect modes (0.215.0).
    #[must_use]
    pub const fn is_selection_tool(self) -> bool {
        matches!(
            self,
            Self::MarqueeSelect
                | Self::EllipticalMarquee
                | Self::Lasso
                | Self::PolygonalLasso
                | Self::MagicWand
        )
    }

    /// A short, human-readable label — for a future tool palette/status
    /// bar, and for this crate's own tests.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Move => "Move",
            Self::MarqueeSelect => "Marquee Select",
            Self::EllipticalMarquee => "Elliptical Marquee",
            Self::Lasso => "Lasso",
            Self::PolygonalLasso => "Polygonal Lasso",
            Self::MagicWand => "Magic Wand",
            Self::Zoom => "Zoom",
            Self::Pan => "Pan",
            Self::Eyedropper => "Eyedropper",
            Self::Brush => "Brush",
            Self::Eraser => "Eraser",
            Self::Gradient => "Gradient",
            Self::PaintBucket => "Paint Bucket",
            Self::Pencil => "Pencil",
            Self::CloneStamp => "Clone Stamp",
            Self::Smudge => "Smudge",
            Self::Blur => "Blur",
            Self::Sharpen => "Sharpen",
            Self::Dodge => "Dodge",
            Self::Burn => "Burn",
            Self::Sponge => "Sponge",
        }
    }

    /// The single key that selects the tool, as it is shown to the user
    /// (upper case, Photoshop's own letters). `aurora-app`'s shortcut
    /// table binds the lower-case letter to `SelectTool`; a test there
    /// checks that the two agree for every tool.
    #[must_use]
    pub const fn shortcut_key(self) -> char {
        match self {
            Self::Move => 'V',
            Self::MarqueeSelect | Self::EllipticalMarquee => 'M',
            Self::Lasso | Self::PolygonalLasso => 'L',
            Self::MagicWand => 'W',
            Self::Zoom => 'Z',
            Self::Pan => 'H',
            Self::Eyedropper => 'I',
            Self::Brush | Self::Pencil => 'B',
            Self::Eraser => 'E',
            Self::CloneStamp | Self::Sharpen => 'S',
            Self::Smudge | Self::Blur => 'R',
            Self::Gradient | Self::PaintBucket => 'G',
            Self::Dodge | Self::Burn => 'O',
            Self::Sponge => 'P',
        }
    }

    /// Whether the tool's shortcut needs `Shift` as well as
    /// [`Self::shortcut_key`] (Photoshop's grouped keys: `G` selects the
    /// Gradient and `Shift+G` the Paint Bucket, 0.209.0; `M` the
    /// rectangular and `Shift+M` the Elliptical Marquee, 0.213.0; `L` the
    /// Lasso and `Shift+L` the Polygonal Lasso, 0.214.0; `B` the Brush and
    /// `Shift+B` the Pencil, 0.220.0; `R` the Smudge and `Shift+R` the Blur,
    /// and `Shift+S` the Sharpen, 0.223.0: Photoshop shares R among the three,
    /// Aurora gives each its own chord; `O` the Dodge and `Shift+O` the Burn,
    /// and `Shift+P` the Sponge, 0.224.0: Photoshop shares O among the three,
    /// and `Shift+P` is the free chord Aurora gives the Sponge).
    #[must_use]
    pub const fn shortcut_needs_shift(self) -> bool {
        matches!(
            self,
            Self::PaintBucket
                | Self::EllipticalMarquee
                | Self::PolygonalLasso
                | Self::Pencil
                | Self::Blur
                | Self::Sharpen
                | Self::Burn
                | Self::Sponge
        )
    }

    /// The shortcut as the user reads it: `"B"`, `"Shift+G"`.
    #[must_use]
    pub fn shortcut_text(self) -> String {
        if self.shortcut_needs_shift() {
            format!("Shift+{}", self.shortcut_key())
        } else {
            self.shortcut_key().to_string()
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
        assert_eq!(Tool::ALL.len(), 21);
        let mut seen = Tool::ALL.to_vec();
        seen.sort_by_key(|tool| tool.label());
        seen.dedup();
        assert_eq!(seen.len(), 21, "ALL must not repeat a variant");
    }

    #[test]
    fn shortcut_keys_are_upper_case_letters_and_the_chords_are_distinct() {
        let keys: Vec<char> = Tool::ALL.iter().map(|tool| tool.shortcut_key()).collect();
        assert!(keys.iter().all(char::is_ascii_uppercase), "{keys:?}");
        let mut chords: Vec<(char, bool)> = Tool::ALL
            .iter()
            .map(|tool| (tool.shortcut_key(), tool.shortcut_needs_shift()))
            .collect();
        chords.sort_unstable();
        chords.dedup();
        assert_eq!(chords.len(), Tool::ALL.len());
        // Only the grouped G, M, L, B, R, S and O keys share a letter, and only by Shift.
        let mut letters = keys;
        letters.sort_unstable();
        letters.dedup();
        assert_eq!(letters.len(), Tool::ALL.len() - 7);
        assert_eq!(Tool::Lasso.shortcut_text(), "L");
        assert_eq!(Tool::PolygonalLasso.shortcut_text(), "Shift+L");
        assert_eq!(Tool::MagicWand.shortcut_text(), "W");
        assert_eq!(Tool::EllipticalMarquee.shortcut_text(), "Shift+M");
        assert_eq!(Tool::MarqueeSelect.shortcut_text(), "M");
        assert_eq!(Tool::Gradient.shortcut_text(), "G");
        assert_eq!(Tool::PaintBucket.shortcut_text(), "Shift+G");
        assert_eq!(Tool::Brush.shortcut_text(), "B");
        assert_eq!(Tool::Pencil.shortcut_text(), "Shift+B");
        assert_eq!(Tool::CloneStamp.shortcut_text(), "S");
        assert_eq!(Tool::Smudge.shortcut_text(), "R");
        assert_eq!(Tool::Blur.shortcut_text(), "Shift+R");
        assert_eq!(Tool::Sharpen.shortcut_text(), "Shift+S");
        assert_eq!(Tool::Dodge.shortcut_text(), "O");
        assert_eq!(Tool::Burn.shortcut_text(), "Shift+O");
        assert_eq!(Tool::Sponge.shortcut_text(), "Shift+P");
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
