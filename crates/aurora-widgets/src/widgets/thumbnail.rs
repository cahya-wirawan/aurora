//! A thumbnail (0.197.0, round L4 of the Photoshop-style Layers panel): a
//! small, square slot that shows a picture a caller computed — here, a
//! downsampled layer — over a checkerboard where the picture is
//! transparent. Generic: it knows nothing about layers or tiles, only
//! about a [`ThumbnailImage`] it is handed.
//!
//! **The picture** is [`THUMBNAIL_TEXELS`] (48) texels square at most — two
//! per logical pixel of the Layers panel's 24 px slot, so a 2x display shows
//! one texel per physical pixel and a 1x display one per two — holding a
//! rectangle of *content* (an aspect-preserving fit of whatever it
//! pictures, centred; the rest of the slot is left empty and shows the
//! panel). Its texels are **premultiplied, sRGB-encoded** `[f32; 4]` (what
//! the canvas shader's own `over` expects, `canvas.wgsl`), so averaging
//! them is the alpha-weighted average and compositing over the checkerboard
//! is `c + checker * (1 - a)`.
//!
//! **Paint path** (the first picture a widget draws): no texture and no new
//! pipeline. [`paint_thumbnail`](crate::paint) builds a vertex-coloured
//! [`aurora_vector::ColorMesh`] (the colour picker's gradient path) of flat
//! rectangles — one per run of equal-coloured texels on a row, so a
//! transparent or solid area is a handful of quads — already composited over
//! the checkerboard (`aurora_gpu::CHECKERBOARD_SHADES`, the canvas shader's
//! own two greys), at the largest texel grid (48, 24, 12 or 6 wide) whose
//! texels are not smaller than a physical pixel. Flat cells, not bilinear:
//! each cell lands on whole physical pixels at 1x and 2x, so the picture is
//! crisp and the checkerboard keeps hard edges. The mesh is cut to the
//! widget's visible rectangle exactly (every cell is an axis-aligned
//! rectangle), so a half-scrolled thumbnail is drawn, not dropped.
//!
//! **Accessibility**: a hidden `Role::Image`. The row that holds it carries
//! the layer's name and description; the picture adds nothing a screen
//! reader should say.

use std::sync::Arc;

use accesskit::{Node, Role};
use aurora_theme::Scales;
use taffy::style_helpers::length;
use taffy::{Size, Style};

use super::WidgetKind;
use crate::error::WidgetError;
use crate::tree::{WidgetId, WidgetTree};

/// The side of the texel grid a [`ThumbnailImage`] lives on.
pub const THUMBNAIL_TEXELS: u32 = 48;

/// One thumbnail's pixels (see the module docs): a `width` x `height`
/// rectangle at `(x, y)` on the [`THUMBNAIL_TEXELS`]-square grid, premultiplied
/// sRGB-encoded RGBA.
#[derive(Debug, Clone, PartialEq)]
pub struct ThumbnailImage {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    texels: Vec<[f32; 4]>,
}

impl ThumbnailImage {
    /// A picture of `texels` (row-major, `width * height` of them) placed at
    /// `(x, y)`. `None` if the size is zero, the rectangle leaves the
    /// [`THUMBNAIL_TEXELS`] grid, or `texels` is not exactly `width * height`
    /// long.
    #[must_use]
    pub fn new(x: u32, y: u32, width: u32, height: u32, texels: Vec<[f32; 4]>) -> Option<Self> {
        let fits = |origin: u32, extent: u32| {
            extent > 0
                && origin
                    .checked_add(extent)
                    .is_some_and(|end| end <= THUMBNAIL_TEXELS)
        };
        if !fits(x, width) || !fits(y, height) {
            return None;
        }
        let count = (width as usize).checked_mul(height as usize)?;
        (texels.len() == count).then_some(Self {
            x,
            y,
            width,
            height,
            texels,
        })
    }

    /// Left edge of the content on the grid.
    #[must_use]
    pub const fn x(&self) -> u32 {
        self.x
    }

    /// Top edge of the content on the grid.
    #[must_use]
    pub const fn y(&self) -> u32 {
        self.y
    }

    /// Content width in texels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Content height in texels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// The texels, row-major.
    #[must_use]
    pub fn texels(&self) -> &[[f32; 4]] {
        &self.texels
    }

    /// The texel at content coordinates `(cx, cy)`.
    #[must_use]
    pub fn texel(&self, cx: u32, cy: u32) -> Option<[f32; 4]> {
        if cx >= self.width || cy >= self.height {
            return None;
        }
        self.texels
            .get(cy as usize * self.width as usize + cx as usize)
            .copied()
    }

    /// Bytes this picture holds (for a cache's cap).
    #[must_use]
    pub fn byte_size(&self) -> usize {
        self.texels.len() * std::mem::size_of::<[f32; 4]>() + std::mem::size_of::<Self>()
    }

    /// This picture on a coarser grid, `grid` texels square (`grid` divides
    /// into [`THUMBNAIL_TEXELS`] by halving: 48, 24, 12, 6): the content
    /// rectangle scaled, every output texel the mean of the source texels
    /// it covers. The picture itself when `grid` is not smaller.
    #[must_use]
    pub fn on_grid(&self, grid: u32) -> Self {
        if grid >= THUMBNAIL_TEXELS || grid == 0 {
            return self.clone();
        }
        let scale = |v: u32| (u64::from(v) * u64::from(grid)).div_ceil(u64::from(THUMBNAIL_TEXELS));
        #[allow(clippy::cast_possible_truncation)]
        let (ox0, oy0) = (scale(self.x) as u32, scale(self.y) as u32);
        #[allow(clippy::cast_possible_truncation)]
        let (ox1, oy1) = (
            scale(self.x + self.width) as u32,
            scale(self.y + self.height) as u32,
        );
        let (ow, oh) = ((ox1 - ox0).max(1), (oy1 - oy0).max(1));
        let mut texels = Vec::with_capacity(ow as usize * oh as usize);
        for row in 0..oh {
            // Source rows/columns covered by output row/column `row`, in
            // content coordinates.
            let sy0 = (u64::from(row) * u64::from(self.height) / u64::from(oh)) as u32;
            let sy1 = ((u64::from(row) + 1) * u64::from(self.height))
                .div_ceil(u64::from(oh))
                .max(u64::from(sy0) + 1) as u32;
            for column in 0..ow {
                let sx0 = (u64::from(column) * u64::from(self.width) / u64::from(ow)) as u32;
                let sx1 = ((u64::from(column) + 1) * u64::from(self.width))
                    .div_ceil(u64::from(ow))
                    .max(u64::from(sx0) + 1) as u32;
                let mut sum = [0.0_f32; 4];
                let mut n = 0.0_f32;
                for sy in sy0..sy1.min(self.height) {
                    for sx in sx0..sx1.min(self.width) {
                        if let Some(t) = self.texel(sx, sy) {
                            for (acc, v) in sum.iter_mut().zip(t) {
                                *acc += v;
                            }
                            n += 1.0;
                        }
                    }
                }
                if n > 0.0 {
                    for v in &mut sum {
                        *v /= n;
                    }
                }
                texels.push(sum);
            }
        }
        Self {
            x: ox0,
            y: oy0,
            width: ow,
            height: oh,
            texels,
        }
    }
}

/// A thumbnail's state: the slot's side and the picture, if one has arrived.
#[derive(Debug, Clone)]
pub struct ThumbnailState {
    /// The slot's side in logical px (a spacing token, chosen by the owner).
    pub side: u32,
    /// The picture; `None` until one is computed (the slot is then blank).
    pub image: Option<Arc<ThumbnailImage>>,
}

impl PartialEq for ThumbnailState {
    fn eq(&self, other: &Self) -> bool {
        self.side == other.side
            && match (&self.image, &other.image) {
                (None, None) => true,
                (Some(a), Some(b)) => Arc::ptr_eq(a, b) || **a == **b,
                _ => false,
            }
    }
}

fn node() -> Node {
    let mut node = Node::new(Role::Image);
    node.set_hidden();
    node
}

fn style(side: u32) -> Style {
    #[allow(clippy::cast_precision_loss)]
    let side = side as f32;
    Style {
        size: Size {
            width: length(side),
            height: length(side),
        },
        flex_shrink: 0.0,
        ..Default::default()
    }
}

/// Adds an empty thumbnail slot `side` logical px square as the last child
/// of `parent`.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] if `parent` doesn't exist.
pub fn insert_thumbnail(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    _scales: &Scales,
    side: u32,
) -> Result<WidgetId, WidgetError> {
    tree.insert(
        parent,
        style(side),
        node(),
        WidgetKind::Thumbnail(ThumbnailState { side, image: None }),
    )
}

/// `id`'s thumbnail state.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] / [`WidgetError::WrongWidgetKind`].
pub fn thumbnail_state(
    tree: &WidgetTree<WidgetKind>,
    id: WidgetId,
) -> Result<&ThumbnailState, WidgetError> {
    match tree.payload(id) {
        None => Err(WidgetError::UnknownWidget(id)),
        Some(WidgetKind::Thumbnail(state)) => Ok(state),
        Some(_) => Err(WidgetError::WrongWidgetKind(id)),
    }
}

/// Shows `image` (or blanks the slot with `None`). Returns whether
/// anything changed — the same `Arc`, or an equal picture, touches nothing.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] / [`WidgetError::WrongWidgetKind`].
pub fn set_thumbnail_image(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    image: Option<Arc<ThumbnailImage>>,
) -> Result<bool, WidgetError> {
    {
        let state = thumbnail_state(tree, id)?;
        let same = match (&state.image, &image) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b) || **a == **b,
            _ => false,
        };
        if same {
            return Ok(false);
        }
    }
    let Some(WidgetKind::Thumbnail(state)) = tree.payload_mut(id) else {
        return Err(WidgetError::WrongWidgetKind(id));
    };
    state.image = image;
    tree.mark_dirty(id)?;
    Ok(true)
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::float_cmp,
    clippy::many_single_char_names
)]
mod tests {
    use super::*;
    use crate::widgets::{new_tree, test_scales};

    fn solid(x: u32, y: u32, w: u32, h: u32, c: [f32; 4]) -> ThumbnailImage {
        match ThumbnailImage::new(x, y, w, h, vec![c; (w * h) as usize]) {
            Some(image) => image,
            None => unreachable!("fixture fits the grid"),
        }
    }

    fn one() -> (WidgetTree<WidgetKind>, WidgetId) {
        let (mut tree, root) = new_tree(Style::default());
        let id = match insert_thumbnail(&mut tree, root, &test_scales(), 24) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        (tree, id)
    }

    #[test]
    fn a_picture_must_fit_the_grid_and_match_its_texel_count() {
        assert!(ThumbnailImage::new(0, 0, 0, 4, vec![]).is_none());
        assert!(ThumbnailImage::new(40, 0, 16, 1, vec![[0.0; 4]; 16]).is_none());
        assert!(ThumbnailImage::new(0, 0, 2, 2, vec![[0.0; 4]; 3]).is_none());
        assert!(ThumbnailImage::new(u32::MAX, 0, 2, 2, vec![[0.0; 4]; 4]).is_none());
        assert!(ThumbnailImage::new(0, 0, 48, 48, vec![[0.0; 4]; 48 * 48]).is_some());
    }

    #[test]
    fn halving_a_solid_picture_keeps_its_colour_and_halves_its_rectangle() {
        let image = solid(8, 12, 32, 24, [0.2, 0.4, 0.6, 1.0]);
        let half = image.on_grid(24);
        assert_eq!(
            (half.x(), half.y(), half.width(), half.height()),
            (4, 6, 16, 12)
        );
        assert!(half.texels().iter().all(|t| *t == [0.2, 0.4, 0.6, 1.0]));
        let quarter = image.on_grid(12);
        assert_eq!((quarter.width(), quarter.height()), (8, 6));
    }

    #[test]
    fn halving_averages_the_texels_it_covers() {
        // 2x2 content, one opaque white texel among transparent ones.
        let mut texels = vec![[0.0; 4]; 4];
        if let Some(first) = texels.first_mut() {
            *first = [1.0, 1.0, 1.0, 1.0];
        }
        let Some(image) = ThumbnailImage::new(0, 0, 2, 2, texels) else {
            unreachable!("fits");
        };
        let coarse = image.on_grid(24);
        assert_eq!((coarse.width(), coarse.height()), (1, 1));
        assert_eq!(coarse.texel(0, 0), Some([0.25, 0.25, 0.25, 0.25]));
    }

    #[test]
    fn on_the_full_grid_a_picture_is_unchanged() {
        let image = solid(0, 0, 48, 48, [0.5; 4]);
        assert_eq!(image.on_grid(48), image);
        assert_eq!(image.on_grid(96), image);
    }

    #[test]
    fn setting_the_same_picture_touches_nothing() {
        let (mut tree, id) = one();
        let image = Arc::new(solid(0, 0, 4, 4, [0.1, 0.2, 0.3, 1.0]));
        assert!(matches!(
            set_thumbnail_image(&mut tree, id, Some(Arc::clone(&image))),
            Ok(true)
        ));
        assert!(matches!(
            set_thumbnail_image(&mut tree, id, Some(Arc::clone(&image))),
            Ok(false)
        ));
        // An equal picture in a different allocation is the same picture.
        assert!(matches!(
            set_thumbnail_image(&mut tree, id, Some(Arc::new((*image).clone()))),
            Ok(false)
        ));
        assert!(matches!(set_thumbnail_image(&mut tree, id, None), Ok(true)));
        assert!(matches!(
            set_thumbnail_image(&mut tree, id, None),
            Ok(false)
        ));
    }

    #[test]
    fn the_node_is_a_hidden_image_with_no_actions() {
        let (tree, id) = one();
        let Some(node) = tree.accessibility(id) else {
            unreachable!("just inserted");
        };
        assert_eq!(node.role(), Role::Image);
        assert!(node.is_hidden());
        assert!(!node.supports_action(accesskit::Action::Click));
        assert!(!node.supports_action(accesskit::Action::Focus));
    }

    // ---- paint ----

    use crate::{PaintOp, paint_widget_ops};
    use aurora_theme::{Palette, Theme, ThemeSet};
    use aurora_vector::ColorMesh;
    use taffy::{Overflow, Point};

    fn dark_theme() -> Theme {
        let Ok(palette) =
            Palette::from_toml_str(include_str!("../../../../design/tokens/palette.toml"))
        else {
            unreachable!()
        };
        let mut themes = ThemeSet::new();
        if themes
            .register(include_str!("../../../../design/themes/dark.toml"))
            .is_err()
        {
            unreachable!()
        }
        match themes.resolve("Dark", &palette) {
            Ok(theme) => theme,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn meshes(tree: &WidgetTree<WidgetKind>, id: WidgetId, scale: f32) -> Vec<ColorMesh> {
        let ops = match paint_widget_ops(tree, id, &dark_theme(), &test_scales(), scale) {
            Ok(ops) => ops,
            Err(err) => unreachable!("{err:?}"),
        };
        ops.into_iter()
            .filter_map(|op| match op {
                PaintOp::Gradient(mesh) => Some(mesh),
                _ => None,
            })
            .collect()
    }

    /// Total area of a mesh's triangles, and its bounding box.
    fn area_and_box(mesh: &ColorMesh) -> (f32, (f32, f32, f32, f32)) {
        let mut area = 0.0;
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for tri in mesh.indices.chunks_exact(3) {
            let p: Vec<_> = tri
                .iter()
                .filter_map(|&i| mesh.vertices.get(i as usize))
                .collect();
            if let [a, b, c] = p[..] {
                area += ((b.position.x - a.position.x) * (c.position.y - a.position.y)
                    - (c.position.x - a.position.x) * (b.position.y - a.position.y))
                    .abs()
                    / 2.0;
            }
        }
        for v in &mesh.vertices {
            x0 = x0.min(v.position.x);
            y0 = y0.min(v.position.y);
            x1 = x1.max(v.position.x);
            y1 = y1.max(v.position.y);
        }
        (area, (x0, y0, x1, y1))
    }

    fn shown(image: ThumbnailImage) -> (WidgetTree<WidgetKind>, WidgetId) {
        let (mut tree, id) = one();
        if set_thumbnail_image(&mut tree, id, Some(Arc::new(image))).is_err() {
            unreachable!("just inserted");
        }
        tree.compute_layout(100.0, 100.0);
        (tree, id)
    }

    #[test]
    fn a_blank_slot_paints_nothing() {
        let (mut tree, id) = one();
        tree.compute_layout(100.0, 100.0);
        assert!(meshes(&tree, id, 1.0).is_empty());
    }

    #[test]
    fn an_opaque_picture_fills_its_slot_in_its_own_colours() {
        let (tree, id) = shown(solid(0, 0, 48, 48, [0.25, 0.5, 0.75, 1.0]));
        let mesh = &meshes(&tree, id, 2.0)[0];
        let (area, bounds) = area_and_box(mesh);
        assert!((area - 24.0 * 24.0).abs() < 0.01, "area {area}");
        assert_eq!(bounds, (0.0, 0.0, 24.0, 24.0));
        assert!(
            mesh.vertices
                .iter()
                .all(|v| v.color == [0.25, 0.5, 0.75, 1.0])
        );
    }

    #[test]
    fn a_transparent_picture_shows_the_canvass_checkerboard_shades() {
        let (tree, id) = shown(solid(0, 0, 48, 48, [0.0; 4]));
        let mesh = &meshes(&tree, id, 2.0)[0];
        let [dark, light] = aurora_gpu::CHECKERBOARD_SHADES;
        let mut seen: Vec<f32> = mesh.vertices.iter().map(|v| v.color[0]).collect();
        seen.sort_by(f32::total_cmp);
        seen.dedup();
        assert_eq!(seen, vec![dark, light]);
        assert!(mesh.vertices.iter().all(|v| v.color[3] == 1.0));
        // Runs merge: a flat cell of the checkerboard is one rectangle per
        // row, not one per texel.
        assert!(mesh.vertices.len() < 48 * 48 * 4 / 2);
    }

    #[test]
    fn a_translucent_texel_is_composited_over_the_checkerboard() {
        // Premultiplied (0.5, 0, 0, 0.5): straight red at half alpha.
        let (tree, id) = shown(solid(0, 0, 48, 48, [0.5, 0.0, 0.0, 0.5]));
        let mesh = &meshes(&tree, id, 2.0)[0];
        let [dark, light] = aurora_gpu::CHECKERBOARD_SHADES;
        for v in &mesh.vertices {
            let ok_dark = (v.color[0] - (0.5 + dark * 0.5)).abs() < 1e-6;
            let ok_light = (v.color[0] - (0.5 + light * 0.5)).abs() < 1e-6;
            assert!(ok_dark || ok_light, "{:?}", v.color);
            assert!(
                (v.color[1] - dark * 0.5).abs() < 1e-6 || (v.color[1] - light * 0.5).abs() < 1e-6
            );
        }
    }

    #[test]
    fn content_smaller_than_the_slot_leaves_the_rest_unpainted() {
        // 48 x 24 centred: 12 logical px of margin above and below.
        let (tree, id) = shown(solid(0, 12, 48, 24, [1.0; 4]));
        let (area, bounds) = area_and_box(&meshes(&tree, id, 2.0)[0]);
        assert!((area - 24.0 * 12.0).abs() < 0.01);
        assert_eq!(bounds, (0.0, 6.0, 24.0, 18.0));
    }

    #[test]
    fn texels_are_never_smaller_than_a_physical_pixel() {
        let (tree, id) = shown(solid(0, 0, 48, 48, [0.1, 0.2, 0.3, 1.0]));
        // At 1x the 48-texel picture is drawn on a 24-texel grid: one
        // logical (= physical) pixel per texel, so every row is 1 px tall.
        let (_, _) = area_and_box(&meshes(&tree, id, 1.0)[0]);
        let ys: Vec<f32> = meshes(&tree, id, 1.0)[0]
            .vertices
            .iter()
            .map(|v| v.position.y)
            .collect();
        let mut rows: Vec<f32> = ys.clone();
        rows.sort_by(f32::total_cmp);
        rows.dedup();
        assert_eq!(rows.len(), 25, "24 rows of 1 px at 1x");
        let mut rows2: Vec<f32> = meshes(&tree, id, 2.0)[0]
            .vertices
            .iter()
            .map(|v| v.position.y)
            .collect();
        rows2.sort_by(f32::total_cmp);
        rows2.dedup();
        assert_eq!(rows2.len(), 49, "48 rows of 0.5 px (1 physical px) at 2x");
    }

    #[test]
    fn a_thumbnail_an_ancestor_clips_is_cut_exactly_not_dropped() {
        let (mut tree, root) = new_tree(Style::default());
        let clip = match crate::widgets::insert_container(
            &mut tree,
            root,
            Style {
                size: taffy::Size {
                    width: taffy::style_helpers::length(100.0_f32),
                    height: taffy::style_helpers::length(10.0_f32),
                },
                overflow: Point {
                    x: Overflow::Hidden,
                    y: Overflow::Hidden,
                },
                ..Default::default()
            },
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let id = match insert_thumbnail(&mut tree, clip, &test_scales(), 24) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if set_thumbnail_image(
            &mut tree,
            id,
            Some(Arc::new(solid(0, 0, 48, 48, [0.3, 0.3, 0.3, 1.0]))),
        )
        .is_err()
        {
            unreachable!("just inserted");
        }
        tree.compute_layout(100.0, 100.0);
        let (area, bounds) = area_and_box(&meshes(&tree, id, 2.0)[0]);
        assert!((area - 24.0 * 10.0).abs() < 0.01, "area {area}");
        assert_eq!(bounds, (0.0, 0.0, 24.0, 10.0));
    }

    /// The repaint cost of noisy thumbnails (0.197.0 review S3): twelve
    /// 48 x 48 pictures of per-texel noise at 2x, the worst case for run
    /// merging. Prints the geometry and the CPU time of one repaint.
    #[test]
    fn repainting_twelve_noisy_thumbnails_is_cheap_enough() {
        let mut state = 0x9e37_79b9_u32;
        let mut noise = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state >> 8) as f32 / (1_u32 << 24) as f32
        };
        let (mut tree, root) = new_tree(Style::default());
        let mut ids = Vec::new();
        for _ in 0..12 {
            let id = match insert_thumbnail(&mut tree, root, &test_scales(), 24) {
                Ok(id) => id,
                Err(err) => unreachable!("{err:?}"),
            };
            let texels = (0..48 * 48)
                .map(|_| [noise(), noise(), noise(), 1.0])
                .collect();
            let Some(image) = ThumbnailImage::new(0, 0, 48, 48, texels) else {
                unreachable!("fits");
            };
            if set_thumbnail_image(&mut tree, id, Some(Arc::new(image))).is_err() {
                unreachable!("just inserted");
            }
            ids.push(id);
        }
        tree.compute_layout(100.0, 1000.0);
        let theme = dark_theme();
        let scales = test_scales();
        let started = std::time::Instant::now();
        let (mut vertices, mut indices) = (0, 0);
        for _ in 0..20 {
            vertices = 0;
            indices = 0;
            for &id in &ids {
                if let Ok(ops) = paint_widget_ops(&tree, id, &theme, &scales, 2.0) {
                    for op in ops {
                        if let PaintOp::Gradient(mesh) = op {
                            vertices += mesh.vertices.len();
                            indices += mesh.indices.len();
                        }
                    }
                }
            }
        }
        let per_repaint = started.elapsed() / 20;
        println!(
            "NOISY THUMBNAILS x12 at 2x: {vertices} vertices ({} KiB), {indices} indices ({} KiB), \
             CPU build {per_repaint:?} per repaint",
            vertices * 24 / 1024,
            indices * 4 / 1024
        );
        assert!(per_repaint < std::time::Duration::from_millis(50));
    }
}
