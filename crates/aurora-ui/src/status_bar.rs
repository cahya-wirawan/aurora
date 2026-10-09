//! The status bar (0.162.0): a one-row strip along the bottom of the
//! canvas column, under the canvas area, showing the canvas zoom and the
//! document's size and sample format — the approved mockup's
//! `100%  Document: 4000 × 3000 px · 16-bit float`
//! (`design/mockups/workspace.html`, `.statusbar`).
//!
//! **Placement.** The mockup draws it across the whole window, under the
//! rail as well. Here it sits in the canvas column only (under the canvas
//! area, between the tools panel and the rail), Photoshop's own
//! per-document convention and the 0.162.0 brief's primary wording: the
//! workspace root is a `Row` that the gallery panel and other floating
//! chrome are inserted into, so a full-width strip would mean wrapping
//! the root's row in a new column — a larger restructuring than this
//! round, raised rather than done.
//!
//! **Composition, not a new widget.** Two plain
//! [`aurora_widgets::widgets::insert_label`] labels in a container, so no
//! gallery entry and no new colour pair: the labels draw in the `Label`
//! widget's own tokens. Height is exactly one `row_height`; horizontal
//! padding is `spacing.sm` and the gap between the two items
//! `spacing.md`, as the mockup's CSS has. Labels are not text-measured
//! (`aurora_widgets::measure` measures checkboxes and toggle buttons
//! only), so the zoom item is `spacing.xxxl` wide — room for the widest
//! whole reading, `12800%` ([`crate::canvas_view::MAX_ZOOM`] on a 2x
//! display) — and the document item takes the rest, ending in "…" when
//! the column is too narrow.
//!
//! **Zoom is physical, as in Photoshop (design-owner decision, Cahya,
//! 2026-10-09).** 100% means one document pixel per *physical* screen
//! pixel: the readout is [`crate::CanvasView::zoom`] (logical — `1.0` is
//! one document pixel per logical pixel) times the window's DPI scale
//! factor ([`physical_zoom`]). So on a 2x Retina display the startup
//! view (logical `1.0`) reads `200%`, and logical `0.5` reads `100%`.
//! The internal zoom, its clamps ([`crate::canvas_view::MIN_ZOOM`],
//! [`crate::canvas_view::MAX_ZOOM`]) and every pointer mapping stay
//! logical; only the readout converts. Rounding is [`zoom_text`]'s one
//! rule at every scale factor, fractional ones included: a whole number
//! when the percentage is within `0.005` of one (`125%` at 1.25x — which
//! also absorbs `f32` noise such as `0.8 × 1.25`), otherwise two
//! decimals (`41.67%`).
//!
//! **Accessibility.** The container is a `Role::Status` node labelled
//! [`STATUS_BAR_LABEL`]; its two labels are `Role::Label` children whose
//! own label is their text, so a screen reader reads the zoom and the
//! document info by navigating into it. ARIA's `status` role implies a
//! polite live region, which would announce every wheel tick of a zoom;
//! the node is therefore explicitly `Live::Off` — the text is current
//! whenever it is read, and nothing is announced unprompted.

use accesskit::{Live, Node, Role};
use aurora_core::SampleFormat;
use aurora_theme::Scales;
use aurora_widgets::widgets::{self, WidgetKind, row_height};
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};
use taffy::style_helpers::{TaffyZero as _, length};
use taffy::{Dimension, FlexDirection, Style};

/// The status bar's accessible label.
pub const STATUS_BAR_LABEL: &str = "Status";

/// The status bar's widgets: the `Role::Status` container and its two
/// labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusBar {
    pub root: WidgetId,
    /// The zoom readout, e.g. `100%`.
    pub zoom: WidgetId,
    /// The document readout, e.g. `Document: 4000 × 3000 px · 16-bit float`.
    pub document: WidgetId,
}

/// What the status bar shows. Built by the app from its live state — the
/// canvas view's zoom, the document's canvas size, and the tile store's
/// sample format — never from literals.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StatusInfo {
    /// The canvas view's own, *logical* zoom factor (`1.0`,
    /// [`crate::canvas_view::DEFAULT_ZOOM`], is one document pixel per
    /// logical screen pixel). Not what the bar shows: see
    /// [`Self::scale_factor`].
    pub zoom: f32,
    /// The window's DPI scale factor (`Window::scale_factor`: `2.0` on a
    /// Retina display). The bar shows [`physical_zoom`]`(zoom,
    /// scale_factor)`, so 100% is one document pixel per physical pixel.
    pub scale_factor: f32,
    /// The document's size in pixels, `(width, height)`.
    pub document_size: (u32, u32),
    /// The document's per-channel sample format.
    pub sample: SampleFormat,
}

/// The physical zoom the status bar shows: the logical `zoom` times the
/// window's `scale_factor`, so `1.0` is one document pixel per
/// *physical* screen pixel — Photoshop's 100% (this module's doc
/// comment). Computed in `f64`. A non-finite, zero or negative
/// `scale_factor` (which `winit` should never report) counts as `1.0`,
/// the same fold `aurora-app`'s own `guarded_scale_factor` applies.
#[must_use]
pub fn physical_zoom(zoom: f32, scale_factor: f32) -> f64 {
    let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
        f64::from(scale_factor)
    } else {
        1.0
    };
    f64::from(zoom) * scale
}

/// The logical `zoom` at `scale_factor` as a physical percentage
/// ([`physical_zoom`]): a whole number when it is one (`100%`, `25%`,
/// `125%`) — within `0.005`, so `f32` noise such as `0.8 × 1.25` still
/// reads `100%` — otherwise two decimals (`33.33%`), the way Photoshop's
/// own status bar reads. One rule at every scale factor. A non-finite
/// zoom (structurally unreachable — [`crate::CanvasView`] clamps) reads
/// as `–%` rather than `NaN%`.
#[must_use]
pub fn zoom_text(zoom: f32, scale_factor: f32) -> String {
    let percent = physical_zoom(zoom, scale_factor) * 100.0;
    if !percent.is_finite() {
        return "–%".to_owned();
    }
    if (percent - percent.round()).abs() < 0.005 {
        format!("{percent:.0}%")
    } else {
        format!("{percent:.2}%")
    }
}

/// A sample format as the status bar names it: `16-bit float`,
/// `32-bit float`, `8-bit integer`.
#[must_use]
pub fn sample_format_text(sample: SampleFormat) -> String {
    let kind = if sample.is_float() {
        "float"
    } else {
        "integer"
    };
    format!("{}-bit {kind}", sample.bits())
}

/// The document readout: `Document: 4000 × 3000 px · 16-bit float`.
#[must_use]
pub fn document_text(size: (u32, u32), sample: SampleFormat) -> String {
    format!(
        "Document: {} × {} px · {}",
        size.0,
        size.1,
        sample_format_text(sample)
    )
}

/// The status bar's style: a row exactly one `row_height` tall that never
/// shrinks on its column's main axis, padded `spacing.sm` left and right,
/// its two items `spacing.md` apart and vertically centred. `min_size`
/// width zero, so a narrow window squeezes it with the canvas column.
#[must_use]
pub fn status_bar_style(scales: &Scales) -> Style {
    #[allow(clippy::cast_precision_loss)]
    let (sm, md) = (scales.spacing.sm as f32, scales.spacing.md as f32);
    let row = row_height(scales);
    Style {
        flex_direction: FlexDirection::Row,
        flex_shrink: 0.0,
        align_items: Some(taffy::AlignItems::CENTER),
        gap: taffy::Size {
            width: length(md),
            height: length(md),
        },
        padding: taffy::Rect {
            left: length(sm),
            right: length(sm),
            top: taffy::LengthPercentage::ZERO,
            bottom: taffy::LengthPercentage::ZERO,
        },
        size: taffy::Size {
            width: Dimension::auto(),
            height: length(row),
        },
        min_size: taffy::Size {
            width: Dimension::ZERO,
            height: length(row),
        },
        overflow: taffy::Point {
            x: taffy::Overflow::Hidden,
            y: taffy::Overflow::Hidden,
        },
        ..Default::default()
    }
}

/// The status bar's accessibility node: `Role::Status`, labelled, and
/// explicitly not a live region (see this module's doc comment).
#[must_use]
pub fn status_bar_node() -> Node {
    let mut node = Node::new(Role::Status);
    node.set_label(STATUS_BAR_LABEL);
    node.set_live(Live::Off);
    node
}

/// Re-styles a label as a status-bar item: the zoom item `spacing.xxxl`
/// wide, the document item growing into the rest from a zero basis.
fn style_item(
    tree: &mut WidgetTree<WidgetKind>,
    scales: &Scales,
    id: WidgetId,
    grows: bool,
) -> Result<(), WidgetError> {
    let mut style = tree
        .style(id)
        .cloned()
        .ok_or(WidgetError::UnknownWidget(id))?;
    style.min_size.width = Dimension::ZERO;
    style.flex_shrink = 1.0;
    if grows {
        style.flex_grow = 1.0;
        style.flex_basis = Dimension::ZERO;
    } else {
        #[allow(clippy::cast_precision_loss)]
        let width = length(scales.spacing.xxxl as f32);
        style.flex_grow = 0.0;
        style.size.width = width;
    }
    tree.set_style(id, style)
}

/// Inserts the status bar as the last child of `parent` (the workspace's
/// canvas column), showing `info`.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `parent` doesn't exist. On
/// any failure the partly built bar is removed again.
pub fn insert_status_bar(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
    info: &StatusInfo,
) -> Result<StatusBar, WidgetError> {
    let root = tree.insert(
        parent,
        status_bar_style(scales),
        status_bar_node(),
        WidgetKind::Container,
    )?;
    let built = (|| {
        let zoom =
            widgets::insert_label(tree, root, scales, zoom_text(info.zoom, info.scale_factor))?;
        let document = widgets::insert_label(
            tree,
            root,
            scales,
            document_text(info.document_size, info.sample),
        )?;
        style_item(tree, scales, zoom, false)?;
        style_item(tree, scales, document, true)?;
        Ok(StatusBar {
            root,
            zoom,
            document,
        })
    })();
    if built.is_err() {
        let _ = tree.remove(root);
    }
    built
}

/// Makes the status bar show `info`. Returns whether any text changed:
/// unchanged text touches nothing ([`widgets::set_label_text`]), so a
/// caller can run this every event-loop iteration.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`]/[`WidgetError::WrongWidgetKind`]
/// if `bar`'s labels are missing or not labels.
pub fn sync_status_bar(
    tree: &mut WidgetTree<WidgetKind>,
    bar: StatusBar,
    info: &StatusInfo,
) -> Result<bool, WidgetError> {
    let zoom = widgets::set_label_text(tree, bar.zoom, &zoom_text(info.zoom, info.scale_factor))?;
    let document = widgets::set_label_text(
        tree,
        bar.document,
        &document_text(info.document_size, info.sample),
    )?;
    Ok(zoom || document)
}

/// The text `bar` currently shows, `(zoom, document)` — `None` if either
/// label is missing.
#[must_use]
pub fn status_bar_text(tree: &WidgetTree<WidgetKind>, bar: StatusBar) -> Option<(String, String)> {
    let text = |id| match tree.payload(id) {
        Some(WidgetKind::Label(state)) => Some(state.text.clone()),
        _ => None,
    };
    Some((text(bar.zoom)?, text(bar.document)?))
}

#[cfg(test)]
mod tests {
    use super::{
        STATUS_BAR_LABEL, StatusInfo, document_text, insert_status_bar, physical_zoom,
        sample_format_text, status_bar_text, sync_status_bar, zoom_text,
    };
    use accesskit::{Live, Role};
    use aurora_core::SampleFormat;

    fn test_scales() -> aurora_theme::Scales {
        const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");
        match aurora_theme::Scales::from_toml_str(SCALES_TOML) {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn zoom_reads_as_a_percentage() {
        assert_eq!(zoom_text(1.0, 1.0), "100%");
        assert_eq!(zoom_text(0.25, 1.0), "25%");
        assert_eq!(zoom_text(64.0, 1.0), "6400%");
        assert_eq!(zoom_text(0.01, 1.0), "1%");
        assert_eq!(zoom_text(1.0 / 3.0, 1.0), "33.33%");
        assert_eq!(zoom_text(f32::NAN, 1.0), "–%");
    }

    /// 0.163.0 (design-owner decision): the readout is physical, as in
    /// Photoshop — logical zoom times the scale factor, one rounding rule
    /// at whole and fractional scales.
    #[test]
    fn zoom_reads_physical_pixels_at_whole_and_fractional_scale_factors() {
        // 2x Retina: the startup view is Photoshop's 200%, and one image
        // pixel per physical pixel is logical 0.5.
        assert_eq!(zoom_text(1.0, 2.0), "200%");
        assert_eq!(zoom_text(0.5, 2.0), "100%");
        assert_eq!(zoom_text(64.0, 2.0), "12800%", "MAX_ZOOM on Retina");
        assert_eq!(zoom_text(0.01, 2.0), "2%", "MIN_ZOOM on Retina");
        assert_eq!(zoom_text(1.0 / 3.0, 2.0), "66.67%");
        // 1.25x and 1.5x: whole where the product is whole, `f32` noise
        // (0.8 and 1/1.5 are inexact) rounded away, two decimals otherwise.
        assert_eq!(zoom_text(1.0, 1.25), "125%");
        assert_eq!(zoom_text(0.8, 1.25), "100%");
        assert_eq!(zoom_text(1.0 / 3.0, 1.25), "41.67%");
        assert_eq!(zoom_text(0.01, 1.25), "1.25%");
        assert_eq!(zoom_text(1.0, 1.5), "150%");
        assert_eq!(zoom_text(1.0 / 1.5, 1.5), "100%");
        assert_eq!(zoom_text(1.0 / 3.0, 1.5), "50%");
        assert_eq!(zoom_text(0.01, 1.5), "1.50%");
        // A degenerate scale factor counts as 1x, never NaN or 0%.
        for bad in [0.0, -2.0, f32::NAN, f32::INFINITY] {
            assert_eq!(zoom_text(1.0, bad), "100%", "scale {bad}");
        }
        assert!((physical_zoom(0.5, 2.0) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn the_document_readout_names_size_and_sample_format() {
        assert_eq!(
            document_text((4000, 3000), SampleFormat::F16),
            "Document: 4000 × 3000 px · 16-bit float"
        );
        assert_eq!(sample_format_text(SampleFormat::F32), "32-bit float");
        assert_eq!(sample_format_text(SampleFormat::U8), "8-bit integer");
    }

    #[test]
    fn the_bar_is_a_labelled_status_region_that_is_not_live_and_syncs_its_text() {
        let scales = test_scales();
        let (mut tree, root) = aurora_widgets::widgets::new_tree(taffy::Style::default());
        let info = StatusInfo {
            zoom: 1.0,
            scale_factor: 1.0,
            document_size: (4000, 3000),
            sample: SampleFormat::F16,
        };
        let bar = match insert_status_bar(&mut tree, root, &scales, &info) {
            Ok(bar) => bar,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(node) = tree.accessibility(bar.root) else {
            unreachable!("inserted");
        };
        assert_eq!(node.role(), Role::Status);
        assert_eq!(node.label(), Some(STATUS_BAR_LABEL));
        assert_eq!(
            node.live(),
            Some(Live::Off),
            "no announcement per zoom tick"
        );
        assert_eq!(
            tree.children(bar.root),
            Some(&[bar.zoom, bar.document][..]),
            "the two readouts are the region's children"
        );
        for (id, text) in [
            (bar.zoom, "100%"),
            (bar.document, "Document: 4000 × 3000 px · 16-bit float"),
        ] {
            let Some(node) = tree.accessibility(id) else {
                unreachable!("inserted");
            };
            assert_eq!(node.role(), Role::Label);
            assert_eq!(node.label(), Some(text), "a screen reader reads the text");
        }
        let unchanged = sync_status_bar(&mut tree, bar, &info);
        assert!(matches!(unchanged, Ok(false)), "same text touches nothing");
        let zoomed = StatusInfo { zoom: 2.0, ..info };
        assert!(matches!(sync_status_bar(&mut tree, bar, &zoomed), Ok(true)));
        assert_eq!(
            status_bar_text(&tree, bar),
            Some((
                "200%".to_owned(),
                "Document: 4000 × 3000 px · 16-bit float".to_owned()
            ))
        );
        let Some(node) = tree.accessibility(bar.zoom) else {
            unreachable!("inserted");
        };
        assert_eq!(node.label(), Some("200%"), "the AT text follows the zoom");
        // 0.163.0: the same logical zoom on a 2x display is 400% physical,
        // and the AT text reads the physical figure; back on a 1x display
        // it reads 200% again.
        let retina = StatusInfo {
            scale_factor: 2.0,
            ..zoomed
        };
        assert!(matches!(sync_status_bar(&mut tree, bar, &retina), Ok(true)));
        let Some(node) = tree.accessibility(bar.zoom) else {
            unreachable!("inserted");
        };
        assert_eq!(node.label(), Some("400%"), "the AT text is physical");
        assert!(matches!(sync_status_bar(&mut tree, bar, &zoomed), Ok(true)));
        assert_eq!(
            status_bar_text(&tree, bar).map(|text| text.0),
            Some("200%".to_owned())
        );
    }
}
