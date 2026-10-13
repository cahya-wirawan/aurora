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
#[derive(Debug, Clone, PartialEq)]
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
    /// A calm, non-modal notice (0.198.0; a locked layer refusing an
    /// edit): while `Some`, the document label shows it instead of the
    /// document readout. The owner clears it when it no longer applies.
    pub notice: Option<&'static str>,
    /// The active selection (0.212.0), if any: its readout is appended to
    /// the document label ([`selection_text`]) so it is announced with it.
    pub selection: Option<aurora_doc::Selection>,
    /// Quick Mask mode is on (0.217.0): the document label starts with
    /// [`QUICK_MASK_TEXT`].
    pub quick_mask: bool,
    /// The Crop tool's rectangle while it has one (0.226.0): its readout
    /// ([`crop_text`]) is appended to the document label.
    pub crop: Option<aurora_core::Rect>,
}

/// The status bar's Quick Mask indicator (0.217.0).
pub const QUICK_MASK_TEXT: &str = "Quick Mask";

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

/// The selection readout (0.212.0): `Selection: 10, 20, 300 × 200 px` (left,
/// top, width, height in document pixels), `Selection: empty` for a
/// rectangle with no area, `Selection: inverse of 10, 20, 300 × 200 px`
/// when inverted (everything outside the rectangle). A non-rectangular
/// selection (0.213.0) reads its bounds followed by ` (ellipse)` or
/// ` (mask)`.
#[must_use]
pub fn selection_text(selection: &aurora_doc::Selection) -> String {
    let b = selection.bounds;
    let suffix = match selection.shape {
        aurora_doc::SelectionShape::Rect => "",
        aurora_doc::SelectionShape::Ellipse => " (ellipse)",
        aurora_doc::SelectionShape::Mask(_) => " (mask)",
    };
    let rect = format!("{}, {}, {} × {} px{suffix}", b.x, b.y, b.width, b.height);
    match (selection.inverted, b.is_empty()) {
        (false, true) => "Selection: empty".to_owned(),
        (false, false) => format!("Selection: {rect}"),
        (true, true) => "Selection: inverse of empty".to_owned(),
        (true, false) => format!("Selection: inverse of {rect}"),
    }
}

/// The Crop tool's readout (0.226.0): `Crop: 10, 20, 300 × 200 px` (left,
/// top, width, height in document pixels).
#[must_use]
pub fn crop_text(rect: &aurora_core::Rect) -> String {
    format!(
        "Crop: {}, {}, {} × {} px",
        rect.x, rect.y, rect.width, rect.height
    )
}

/// The document label's full text: the document readout, then the
/// selection readout after ` · ` while one is active.
#[must_use]
pub fn document_label_text(info: &StatusInfo) -> String {
    let document = document_text(info.document_size, info.sample);
    let text = match info.selection.as_ref() {
        Some(selection) => format!("{document} · {}", selection_text(selection)),
        None => document,
    };
    let text = match info.crop.as_ref() {
        Some(rect) => format!("{text} · {}", crop_text(rect)),
        None => text,
    };
    if info.quick_mask {
        format!("{QUICK_MASK_TEXT} · {text}")
    } else {
        text
    }
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
        let document = widgets::insert_label(tree, root, scales, document_label_text(info))?;
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
        &info
            .notice
            .map_or_else(|| document_label_text(info), str::to_owned),
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
            notice: None,
            selection: None,
            quick_mask: false,
            crop: None,
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
            ..zoomed.clone()
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

    /// 0.217.0: Quick Mask mode prefixes the document label with
    /// "Quick Mask" and reaches the bar, and leaving removes it.
    #[test]
    fn quick_mask_mode_is_announced_in_the_document_label() {
        use super::{QUICK_MASK_TEXT, document_label_text};
        let off = StatusInfo {
            zoom: 1.0,
            scale_factor: 1.0,
            document_size: (10, 10),
            sample: aurora_tile::SAMPLE_FORMAT,
            notice: None,
            selection: None,
            quick_mask: false,
            crop: None,
        };
        let on = StatusInfo {
            quick_mask: true,
            crop: None,
            ..off.clone()
        };
        assert_eq!(QUICK_MASK_TEXT, "Quick Mask");
        assert!(!document_label_text(&off).contains("Quick Mask"));
        assert_eq!(
            document_label_text(&on),
            format!("Quick Mask · {}", document_label_text(&off))
        );
        let scales = test_scales();
        let (mut tree, root) = aurora_widgets::widgets::new_tree(taffy::Style::default());
        let bar = match insert_status_bar(&mut tree, root, &scales, &off) {
            Ok(bar) => bar,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(sync_status_bar(&mut tree, bar, &on).ok(), Some(true));
        assert!(status_bar_text(&tree, bar).is_some_and(|(_, d)| d.starts_with("Quick Mask")));
        assert_eq!(sync_status_bar(&mut tree, bar, &off).ok(), Some(true));
        assert!(status_bar_text(&tree, bar).is_some_and(|(_, d)| !d.contains("Quick Mask")));
    }

    /// 0.212.0: the selection readout for none, a rectangle, an empty
    /// rectangle and an inverted selection, and that it reaches the bar.
    #[test]
    fn the_selection_readout_covers_none_rect_empty_and_inverted() {
        use super::{document_label_text, selection_text};
        use aurora_core::Rect;
        use aurora_doc::Selection;
        let rect = |x, y, width, height| Rect {
            x,
            y,
            width,
            height,
        };
        let info = StatusInfo {
            zoom: 1.0,
            scale_factor: 1.0,
            document_size: (640, 480),
            sample: SampleFormat::F16,
            notice: None,
            selection: None,
            quick_mask: false,
            crop: None,
        };
        let base = "Document: 640 × 480 px · 16-bit float";
        assert_eq!(document_label_text(&info), base);
        let some = |selection| StatusInfo {
            selection: Some(selection),
            quick_mask: false,
            crop: None,
            ..info
        };
        let plain = Selection::new(rect(10, 20, 300, 200));
        assert_eq!(selection_text(&plain), "Selection: 10, 20, 300 × 200 px");
        assert_eq!(
            document_label_text(&some(plain.clone())),
            format!("{base} · Selection: 10, 20, 300 × 200 px")
        );
        let inverted = Selection {
            inverted: true,
            ..plain.clone()
        };
        assert_eq!(
            selection_text(&inverted),
            "Selection: inverse of 10, 20, 300 × 200 px"
        );
        // 0.213.0: the shape is named after the bounds.
        let ellipse = Selection::ellipse(rect(10, 20, 300, 200));
        assert_eq!(
            selection_text(&ellipse),
            "Selection: 10, 20, 300 × 200 px (ellipse)"
        );
        assert_eq!(
            selection_text(&Selection {
                inverted: true,
                ..ellipse
            }),
            "Selection: inverse of 10, 20, 300 × 200 px (ellipse)"
        );
        if let Ok(mask) = aurora_doc::SelectionMask::new(rect(1, 2, 2, 1), vec![255, 0]) {
            assert_eq!(
                selection_text(&Selection::from_mask(std::sync::Arc::new(mask))),
                "Selection: 1, 2, 2 × 1 px (mask)"
            );
        }
        assert_eq!(
            selection_text(&Selection::new(rect(5, 5, 0, 9))),
            "Selection: empty"
        );
        // Through the bar: the label follows the selection and back.
        let scales = test_scales();
        let (mut tree, root) = aurora_widgets::widgets::new_tree(taffy::Style::default());
        let bar = match insert_status_bar(&mut tree, root, &scales, &info) {
            Ok(bar) => bar,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(
            sync_status_bar(&mut tree, bar, &some(plain.clone())).ok(),
            Some(true)
        );
        assert!(status_bar_text(&tree, bar).is_some_and(|(_, d)| d.ends_with("300 × 200 px")));
        assert_eq!(sync_status_bar(&mut tree, bar, &info).ok(), Some(true));
        assert_eq!(
            status_bar_text(&tree, bar).map(|(_, d)| d),
            Some(base.to_owned())
        );
    }

    #[test]
    fn the_crop_readout_follows_the_document_and_selection_text() {
        let rect = aurora_core::Rect {
            x: 7,
            y: 3,
            width: 40,
            height: 30,
        };
        assert_eq!(super::crop_text(&rect), "Crop: 7, 3, 40 × 30 px");
        let info = StatusInfo {
            zoom: 1.0,
            scale_factor: 1.0,
            document_size: (100, 80),
            sample: aurora_tile::SAMPLE_FORMAT,
            notice: None,
            selection: None,
            quick_mask: false,
            crop: Some(rect),
        };
        let text = super::document_label_text(&info);
        assert!(text.ends_with(" · Crop: 7, 3, 40 × 30 px"), "{text}");
        let none = StatusInfo { crop: None, ..info };
        assert!(!super::document_label_text(&none).contains("Crop"));
    }
}
