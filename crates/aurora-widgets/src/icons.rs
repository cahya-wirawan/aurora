//! The **icon set** (0.194.0, round L1 of the Photoshop-style Layers
//! panel): every icon the workspace draws, as plain geometry in a 24 x 24
//! unit box, and nothing else.
//!
//! **Why this is its own module.** The design owner's decision
//! (2026-10-11) is that Aurora draws its own vector icons and that this
//! first set is **provisional** — it will be swapped for a designed one.
//! So the geometry lives here, as data a test can inspect, and the widget
//! that draws it ([`crate::widgets::insert_icon`], `widgets/icon.rs`)
//! knows only [`IconKind`] and [`icon_paths`]. Replacing the set means
//! editing [`icon_parts`] (or the one function behind it); no panel and
//! no widget changes.
//!
//! **Style.** A neutral line icon: one uniform stroke ([`ICON_STROKE`]
//! units wide, drawn with round caps and round joins), no text, a few
//! solid fills ([`IconKind::Adjustment`]'s half disc). Every point lies
//! inside the 24 x 24 box, inset by at least half the stroke so the
//! stroked shape stays in it too (`every_icon_stays_inside_the_box`,
//! which checks the tessellated stroke, not just the control points).
//! Each icon is meant to read at 16 px, where a 1.5-unit stroke is
//! exactly one physical pixel at scale 1.
//!
//! **Drawn on the pixel grid.** The path renderer does not antialias, so
//! the drawings are laid out in *grid* coordinates (`g`): grid step `n` is
//! the centre of pixel `n` of a 16 px icon (`0.75 + 1.5 n` units), so
//! every horizontal and vertical line is exactly one physical pixel at
//! scale 1 and two at scale 2 (pinned by a real-GPU test). Diagonals and
//! curves are as hard-edged as everything the renderer draws; a symmetric
//! drawing is centred on grid step 8, half a pixel right of and below the
//! box centre, because a one-pixel line cannot sit on a pixel boundary.
//!
//! **Not a style value.** Coordinates and the stroke width are *icon
//! geometry*, the unit-box coordinates of a drawing, in the same sense a
//! glyph outline's numbers are; they are not sizes or colours a theme or
//! the token scales govern. The on-screen size of an icon (a token — see
//! [`crate::widgets::icon_side`]) and its colour (an `icon.*` token) are
//! resolved outside this module. [`icon_paths`] scales the unit box to
//! that size and offsets it; nothing here knows either.
//!
//! **`Fx`** is the letters "fx" drawn as strokes (an `f` with its hook
//! and crossbar, an `x`), not text: it needs no font and is drawn by the
//! same path as every other icon.

use aurora_vector::{Path, PathBuilder, Point};

/// The side of the box every icon is drawn in, in icon units.
pub const ICON_BOX: f32 = 24.0;

/// The stroke width of every line in the set, in icon units (a
/// 24-unit-box value: scaled with the icon, so 1.0 px at 16 px).
pub const ICON_STROKE: f32 = 1.5;

/// One icon of the set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IconKind {
    /// An open eye: a layer is visible.
    Eye,
    /// An eye with a slash: a layer is hidden.
    EyeOff,
    /// A closed folder: a collapsed group.
    FolderClosed,
    /// An open folder: an expanded group.
    FolderOpen,
    /// A right-pointing chevron: a collapsed disclosure.
    ChevronRight,
    /// A down-pointing chevron: an expanded disclosure.
    ChevronDown,
    /// A padlock.
    Lock,
    /// The letters "fx": layer effects.
    Fx,
    /// A square with a plus: a new layer.
    NewLayer,
    /// A folder with a plus: a new group.
    Group,
    /// A bin: delete.
    Delete,
    /// Two chain links: link layers.
    Link,
    /// A square with a circle in it: a layer mask.
    Mask,
    /// A half-filled circle: an adjustment layer.
    Adjustment,
    /// A checkerboard square: lock transparent pixels (0.198.0).
    TransparentPixels,
    /// A paint brush: lock image pixels (0.198.0).
    Brush,
    /// Four arrows: lock position (0.198.0).
    Move,
    /// A magnifying glass: search (0.199.0).
    Search,
    /// A picture frame with a hill and a sun: a pixel layer (0.199.0).
    Image,
    /// A corner arrow pointing down: a layer clipped to the one below
    /// (0.201.1).
    Clip,
    /// A dashed rectangle: the Marquee Select tool (0.207.0).
    Marquee,
    /// A magnifying glass with a plus: the Zoom tool (0.207.0).
    ZoomIn,
    /// An open hand: the Pan tool (0.207.0).
    Hand,
    /// A dropper: the Eyedropper tool (0.207.0).
    Eyedropper,
    /// A tilted eraser block: the Eraser tool (0.207.0).
    Eraser,
    /// A bent arrow with a head at each end: swap the foreground and
    /// background colours (0.208.0).
    SwapArrows,
    /// A filled square in front of an outlined one: reset the
    /// foreground and background colours to black and white (0.208.0).
    DefaultColours,
    /// A framed ramp: vertical bars that thicken left to right, the
    /// Gradient tool (0.209.0).
    Gradient,
    /// A tilted bucket with a drop beside it: the Paint Bucket tool
    /// (0.209.0).
    PaintBucket,
}

impl IconKind {
    /// Every icon, in the order the gallery shows them.
    pub const ALL: [IconKind; 29] = [
        IconKind::Eye,
        IconKind::EyeOff,
        IconKind::FolderClosed,
        IconKind::FolderOpen,
        IconKind::ChevronRight,
        IconKind::ChevronDown,
        IconKind::Lock,
        IconKind::Fx,
        IconKind::NewLayer,
        IconKind::Group,
        IconKind::Delete,
        IconKind::Link,
        IconKind::Mask,
        IconKind::Adjustment,
        IconKind::TransparentPixels,
        IconKind::Brush,
        IconKind::Move,
        IconKind::Search,
        IconKind::Image,
        IconKind::Clip,
        IconKind::Marquee,
        IconKind::ZoomIn,
        IconKind::Hand,
        IconKind::Eyedropper,
        IconKind::Eraser,
        IconKind::SwapArrows,
        IconKind::DefaultColours,
        IconKind::Gradient,
        IconKind::PaintBucket,
    ];

    /// The icon's name, as the gallery labels it and a caller can use as
    /// an accessible name when the icon stands alone.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            IconKind::Eye => "Eye",
            IconKind::EyeOff => "Eye Off",
            IconKind::FolderClosed => "Folder Closed",
            IconKind::FolderOpen => "Folder Open",
            IconKind::ChevronRight => "Chevron Right",
            IconKind::ChevronDown => "Chevron Down",
            IconKind::Lock => "Lock",
            IconKind::Fx => "Fx",
            IconKind::NewLayer => "New Layer",
            IconKind::Group => "Group",
            IconKind::Delete => "Delete",
            IconKind::Link => "Link",
            IconKind::Mask => "Mask",
            IconKind::Adjustment => "Adjustment",
            IconKind::TransparentPixels => "Transparent Pixels",
            IconKind::Brush => "Brush",
            IconKind::Move => "Move",
            IconKind::Search => "Search",
            IconKind::Image => "Image",
            IconKind::Clip => "Clip",
            IconKind::Marquee => "Marquee",
            IconKind::ZoomIn => "Zoom In",
            IconKind::Hand => "Hand",
            IconKind::Eyedropper => "Eyedropper",
            IconKind::Eraser => "Eraser",
            IconKind::SwapArrows => "Swap Arrows",
            IconKind::DefaultColours => "Default Colours",
            IconKind::Gradient => "Gradient",
            IconKind::PaintBucket => "Paint Bucket",
        }
    }
}

/// One segment of an icon's outline, in icon units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Seg {
    /// Starts the sub-path.
    Move([f32; 2]),
    /// A straight line to the point.
    Line([f32; 2]),
    /// A cubic Bézier: two control points, then the end point.
    Cubic([f32; 2], [f32; 2], [f32; 2]),
}

impl Seg {
    /// Every point the segment is defined by (control points included —
    /// a Bézier lies inside its control polygon, so a box that holds them
    /// holds the curve).
    #[must_use]
    pub fn points(&self) -> Vec<[f32; 2]> {
        match *self {
            Seg::Move(p) | Seg::Line(p) => vec![p],
            Seg::Cubic(a, b, c) => vec![a, b, c],
        }
    }
}

/// How one part of an icon is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartStyle {
    /// Outlined with the icon stroke.
    Stroke,
    /// Filled solid.
    Fill,
}

/// One sub-path of an icon: it starts with a [`Seg::Move`], is closed or
/// left open, and is stroked or filled.
#[derive(Debug, Clone, PartialEq)]
pub struct IconPart {
    pub style: PartStyle,
    /// Whether the last point joins back to the first.
    pub closed: bool,
    pub segs: Vec<Seg>,
}

/// The pixel grid the drawings are designed on, in icon units per grid
/// step: at the size the set is drawn for (16 px) one step is exactly one
/// pixel, and at scale 2 exactly two physical pixels.
const GRID: f32 = ICON_STROKE;

/// A grid coordinate `n` as an icon-unit coordinate: the **centre** of
/// pixel `n` of the 16 px grid (`0.75 + 1.5 n`). Horizontal and vertical
/// lines drawn at whole `n` are exactly one pixel wide and cover whole
/// pixels at scale 1 and at scale 2, which is what keeps them crisp
/// although the path renderer does not antialias (no widget pipeline
/// does today). Curves and diagonals are drawn on the same grid and are
/// as hard-edged as everything else the renderer draws.
fn g(n: f32) -> f32 {
    GRID / 2.0 + GRID * n
}

impl IconPart {
    fn new(style: PartStyle) -> Self {
        Self {
            style,
            closed: false,
            segs: Vec::new(),
        }
    }

    /// Move to grid point `(x, y)`.
    fn m(mut self, x: f32, y: f32) -> Self {
        self.segs.push(Seg::Move([g(x), g(y)]));
        self
    }

    /// Line to grid point `(x, y)`.
    fn l(mut self, x: f32, y: f32) -> Self {
        self.segs.push(Seg::Line([g(x), g(y)]));
        self
    }

    /// Cubic to grid point `to` through grid control points `c1`, `c2`.
    fn c(mut self, c1: [f32; 2], c2: [f32; 2], to: [f32; 2]) -> Self {
        let at = |p: [f32; 2]| [g(p[0]), g(p[1])];
        self.segs.push(Seg::Cubic(at(c1), at(c2), at(to)));
        self
    }

    fn closed(mut self) -> Self {
        self.closed = true;
        self
    }
}

/// The control-point offset that makes a cubic approximate a quarter
/// circle: `4/3 * (sqrt(2) - 1)`.
const QUARTER: f32 = 0.552_284_8;

fn stroke_part() -> IconPart {
    IconPart::new(PartStyle::Stroke)
}

fn fill_part() -> IconPart {
    IconPart::new(PartStyle::Fill)
}

/// A circle (grid centre and radius) as four cubics, starting at its
/// right-most point.
fn circle(style: PartStyle, cx: f32, cy: f32, r: f32) -> IconPart {
    let k = QUARTER * r;
    IconPart::new(style)
        .m(cx + r, cy)
        .c([cx + r, cy + k], [cx + k, cy + r], [cx, cy + r])
        .c([cx - k, cy + r], [cx - r, cy + k], [cx - r, cy])
        .c([cx - r, cy - k], [cx - k, cy - r], [cx, cy - r])
        .c([cx + k, cy - r], [cx + r, cy - k], [cx + r, cy])
        .closed()
}

/// A rounded rectangle (grid corner, size, radius), stroked, starting at
/// the top edge's left end.
#[allow(clippy::many_single_char_names)] // x, y, w, h, r, k: a rectangle's own letters
fn rounded_box(x: f32, y: f32, w: f32, h: f32, r: f32) -> IconPart {
    let k = QUARTER * r;
    let (x1, y1) = (x + w, y + h);
    stroke_part()
        .m(x + r, y)
        .l(x1 - r, y)
        .c([x1 - r + k, y], [x1, y + r - k], [x1, y + r])
        .l(x1, y1 - r)
        .c([x1, y1 - r + k], [x1 - r + k, y1], [x1 - r, y1])
        .l(x + r, y1)
        .c([x + r - k, y1], [x, y1 - r + k], [x, y1 - r])
        .l(x, y + r)
        .c([x, y + r - k], [x + r - k, y], [x + r, y])
        .closed()
}

/// A plus sign as two open strokes, centred on grid point `(cx, cy)`.
fn plus(cx: f32, cy: f32, arm: f32) -> [IconPart; 2] {
    [
        stroke_part().m(cx, cy - arm).l(cx, cy + arm),
        stroke_part().m(cx - arm, cy).l(cx + arm, cy),
    ]
}

/// The eye's lens outline (shared by [`IconKind::Eye`] and
/// [`IconKind::EyeOff`]).
fn eye_lens() -> IconPart {
    stroke_part()
        .m(2.0, 8.0)
        .c([4.0, 5.0], [6.0, 4.0], [8.0, 4.0])
        .c([10.0, 4.0], [12.0, 5.0], [14.0, 8.0])
        .c([12.0, 11.0], [10.0, 12.0], [8.0, 12.0])
        .c([6.0, 12.0], [4.0, 11.0], [2.0, 8.0])
        .closed()
}

/// The closed folder's outline: a tab on the left, the body below it.
fn folder_outline() -> IconPart {
    stroke_part()
        .m(2.0, 4.0)
        .l(6.0, 4.0)
        .l(7.0, 6.0)
        .l(14.0, 6.0)
        .l(14.0, 13.0)
        .l(2.0, 13.0)
        .closed()
}

/// One link of the chain: a closed capsule, in a frame whose `u` axis runs
/// along the chain and `v` across it, centred on grid point `(cx, cy)`.
/// The chain runs from the lower left to the upper right.
fn chain_link(cx: f32, cy: f32) -> IconPart {
    const HALF_LENGTH: f32 = 2.0;
    const RADIUS: f32 = 2.0;
    let s = std::f32::consts::FRAC_1_SQRT_2;
    // Local (u, v) -> grid (x, y): u along (s, -s), v along (s, s).
    let at = |u: f32, v: f32| -> [f32; 2] { [g(cx + u * s + v * s), g(cy - u * s + v * s)] };
    let k = QUARTER * RADIUS;
    let (hl, r) = (HALF_LENGTH, RADIUS);
    let mut part = stroke_part();
    part.segs.push(Seg::Move(at(hl, -r)));
    part.segs.push(Seg::Line(at(-hl, -r)));
    part.segs.push(Seg::Cubic(
        at(-hl - k, -r),
        at(-hl - r, -k),
        at(-hl - r, 0.0),
    ));
    part.segs
        .push(Seg::Cubic(at(-hl - r, k), at(-hl - k, r), at(-hl, r)));
    part.segs.push(Seg::Line(at(hl, r)));
    part.segs
        .push(Seg::Cubic(at(hl + k, r), at(hl + r, k), at(hl + r, 0.0)));
    part.segs
        .push(Seg::Cubic(at(hl + r, -k), at(hl + k, -r), at(hl, -r)));
    part.closed = true;
    part
}

/// `kind`'s geometry: its parts, in drawing order, in a 24 x 24 box.
#[must_use]
#[allow(clippy::too_many_lines)] // one flat table of drawings is the point
pub fn icon_parts(kind: IconKind) -> Vec<IconPart> {
    match kind {
        IconKind::Eye => vec![eye_lens(), circle(PartStyle::Stroke, 8.0, 8.0, 2.0)],
        IconKind::EyeOff => vec![
            eye_lens(),
            circle(PartStyle::Stroke, 8.0, 8.0, 2.0),
            stroke_part().m(3.0, 3.0).l(13.0, 13.0),
        ],
        IconKind::FolderClosed => vec![folder_outline()],
        IconKind::FolderOpen => vec![
            stroke_part()
                .m(2.0, 13.0)
                .l(2.0, 4.0)
                .l(6.0, 4.0)
                .l(7.0, 6.0)
                .l(12.0, 6.0)
                .l(12.0, 8.0),
            stroke_part()
                .m(2.0, 13.0)
                .l(4.0, 8.0)
                .l(14.0, 8.0)
                .l(12.0, 13.0)
                .closed(),
        ],
        IconKind::ChevronRight => vec![stroke_part().m(6.0, 3.0).l(11.0, 8.0).l(6.0, 13.0)],
        IconKind::ChevronDown => vec![stroke_part().m(3.0, 6.0).l(8.0, 11.0).l(13.0, 6.0)],
        IconKind::Lock => vec![
            rounded_box(3.0, 7.0, 10.0, 7.0, 1.0),
            stroke_part()
                .m(5.0, 7.0)
                .l(5.0, 5.0)
                .c([5.0, 3.4], [6.3, 2.0], [8.0, 2.0])
                .c([9.7, 2.0], [11.0, 3.4], [11.0, 5.0])
                .l(11.0, 7.0),
        ],
        IconKind::Fx => vec![
            // The f: a stem with a hook, and a crossbar.
            stroke_part()
                .m(5.0, 14.0)
                .l(5.0, 5.0)
                .c([5.0, 3.2], [6.2, 2.0], [8.0, 2.0]),
            stroke_part().m(3.0, 8.0).l(8.0, 8.0),
            // The x.
            stroke_part().m(9.0, 8.0).l(14.0, 14.0),
            stroke_part().m(14.0, 8.0).l(9.0, 14.0),
        ],
        IconKind::NewLayer => {
            let [vertical, horizontal] = plus(8.0, 8.0, 3.0);
            vec![rounded_box(2.0, 2.0, 12.0, 12.0, 1.5), vertical, horizontal]
        }
        IconKind::Group => {
            let [vertical, horizontal] = plus(8.0, 10.0, 2.0);
            vec![folder_outline(), vertical, horizontal]
        }
        IconKind::Delete => vec![
            stroke_part().m(3.0, 5.0).l(13.0, 5.0),
            stroke_part()
                .m(6.0, 5.0)
                .l(6.0, 3.0)
                .l(10.0, 3.0)
                .l(10.0, 5.0),
            stroke_part()
                .m(4.0, 5.0)
                .l(5.0, 14.0)
                .l(11.0, 14.0)
                .l(12.0, 5.0),
            stroke_part().m(7.0, 7.0).l(7.0, 12.0),
            stroke_part().m(9.0, 7.0).l(9.0, 12.0),
        ],
        IconKind::Link => vec![chain_link(5.5, 10.5), chain_link(10.5, 5.5)],
        IconKind::Mask => vec![
            rounded_box(2.0, 2.0, 12.0, 12.0, 1.5),
            circle(PartStyle::Stroke, 8.0, 8.0, 3.0),
        ],
        IconKind::Adjustment => {
            let k = QUARTER * 6.0;
            vec![
                // The right half, filled; then the whole outline over it.
                fill_part()
                    .m(8.0, 2.0)
                    .c([8.0 + k, 2.0], [14.0, 8.0 - k], [14.0, 8.0])
                    .c([14.0, 8.0 + k], [8.0 + k, 14.0], [8.0, 14.0])
                    .closed(),
                circle(PartStyle::Stroke, 8.0, 8.0, 6.0),
            ]
        }
        IconKind::TransparentPixels => vec![
            // Two filled checker squares inside the outline.
            fill_part()
                .m(2.0, 2.0)
                .l(8.0, 2.0)
                .l(8.0, 8.0)
                .l(2.0, 8.0)
                .closed(),
            fill_part()
                .m(8.0, 8.0)
                .l(14.0, 8.0)
                .l(14.0, 14.0)
                .l(8.0, 14.0)
                .closed(),
            rounded_box(2.0, 2.0, 12.0, 12.0, 1.0),
        ],
        IconKind::Brush => vec![
            // The handle, then the head with its bristles.
            stroke_part().m(14.0, 2.0).l(9.0, 7.0),
            stroke_part()
                .m(7.0, 7.0)
                .l(9.0, 9.0)
                .c([9.0, 12.0], [6.0, 14.0], [2.0, 14.0])
                .c([4.0, 12.0], [3.0, 10.0], [5.0, 8.0])
                .closed(),
        ],
        IconKind::Move => vec![
            stroke_part().m(8.0, 2.0).l(8.0, 14.0),
            stroke_part().m(2.0, 8.0).l(14.0, 8.0),
            stroke_part().m(6.0, 4.0).l(8.0, 2.0).l(10.0, 4.0),
            stroke_part().m(6.0, 12.0).l(8.0, 14.0).l(10.0, 12.0),
            stroke_part().m(4.0, 6.0).l(2.0, 8.0).l(4.0, 10.0),
            stroke_part().m(12.0, 6.0).l(14.0, 8.0).l(12.0, 10.0),
        ],
        IconKind::Search => vec![
            // The lens, then the handle running to the lower right.
            circle(PartStyle::Stroke, 6.0, 6.0, 4.0),
            stroke_part().m(9.0, 9.0).l(13.0, 13.0),
        ],
        IconKind::Image => vec![
            // The frame, a sun filled in its upper right and a hill line.
            rounded_box(2.0, 3.0, 12.0, 10.0, 1.0),
            circle(PartStyle::Fill, 10.0, 6.0, 1.0),
            stroke_part()
                .m(2.0, 11.0)
                .l(6.0, 7.0)
                .l(9.0, 10.0)
                .l(11.0, 8.0)
                .l(14.0, 11.0),
        ],
        IconKind::Clip => vec![
            // The corner: across, then down; and the head pointing down at
            // the layer below.
            stroke_part().m(2.0, 3.0).l(11.0, 3.0).l(11.0, 13.0),
            stroke_part().m(8.0, 10.0).l(11.0, 13.0).l(14.0, 10.0),
        ],
        IconKind::Marquee => marquee_dashes(),
        IconKind::ZoomIn => {
            let [vertical, horizontal] = plus(6.0, 6.0, 2.0);
            vec![
                circle(PartStyle::Stroke, 6.0, 6.0, 4.0),
                stroke_part().m(9.0, 9.0).l(13.0, 13.0),
                vertical,
                horizontal,
            ]
        }
        IconKind::Hand => vec![
            // Four fingers (the middle two taller), the palm under them
            // and the thumb off its left side.
            stroke_part().m(5.0, 4.0).l(5.0, 8.0),
            stroke_part().m(7.0, 2.0).l(7.0, 8.0),
            stroke_part().m(9.0, 2.0).l(9.0, 8.0),
            stroke_part().m(11.0, 4.0).l(11.0, 8.0),
            stroke_part()
                .m(5.0, 8.0)
                .l(5.0, 11.0)
                .l(7.0, 14.0)
                .l(9.0, 14.0)
                .l(11.0, 11.0)
                .l(11.0, 8.0),
            stroke_part().m(5.0, 11.0).l(3.0, 9.0),
        ],
        IconKind::Eyedropper => vec![
            // The glass, a collar, the rubber bulb and the tip.
            stroke_part()
                .m(4.0, 11.0)
                .l(9.0, 6.0)
                .l(11.0, 8.0)
                .l(6.0, 13.0)
                .closed(),
            stroke_part().m(8.0, 4.0).l(12.0, 8.0),
            stroke_part().m(10.0, 2.0).l(14.0, 6.0),
            stroke_part().m(2.0, 14.0).l(4.0, 12.0),
        ],
        IconKind::Eraser => vec![
            // A tilted block and the line between its worn and clean ends.
            stroke_part()
                .m(2.0, 9.0)
                .l(8.0, 3.0)
                .l(13.0, 8.0)
                .l(7.0, 14.0)
                .closed(),
            stroke_part().m(5.0, 6.0).l(10.0, 11.0),
        ],
        IconKind::SwapArrows => vec![
            // A bent line, an arrow head at each end: up-and-over.
            stroke_part().m(3.0, 13.0).l(3.0, 6.0).l(12.0, 6.0),
            stroke_part().m(10.0, 4.0).l(12.0, 6.0).l(10.0, 8.0),
            stroke_part().m(1.0, 11.0).l(3.0, 13.0).l(5.0, 11.0),
        ],
        IconKind::DefaultColours => vec![
            // The front (foreground) square filled, the back one an
            // outline cut where the front covers it.
            fill_part()
                .m(2.0, 2.0)
                .l(8.0, 2.0)
                .l(8.0, 8.0)
                .l(2.0, 8.0)
                .closed(),
            stroke_part()
                .m(9.0, 5.0)
                .l(13.0, 5.0)
                .l(13.0, 13.0)
                .l(5.0, 13.0)
                .l(5.0, 9.0),
        ],
        IconKind::Gradient => vec![
            // The frame, and three bars that grow from left to right.
            rounded_box(2.0, 3.0, 12.0, 10.0, 1.0),
            stroke_part().m(5.0, 7.0).l(5.0, 9.0),
            stroke_part().m(8.0, 6.0).l(8.0, 10.0),
            stroke_part().m(11.0, 5.0).l(11.0, 11.0),
        ],
        IconKind::PaintBucket => vec![
            // A tilted bucket (a diamond), the paint line across it, and
            // the drop pouring from its corner.
            stroke_part()
                .m(2.0, 8.0)
                .l(7.0, 3.0)
                .l(12.0, 8.0)
                .l(7.0, 13.0)
                .closed(),
            stroke_part().m(4.0, 6.0).l(10.0, 12.0),
            fill_part()
                .m(14.0, 10.0)
                .l(15.0, 12.0)
                .l(14.0, 14.0)
                .l(13.0, 12.0)
                .closed(),
        ],
    }
}

/// The marquee's dashed rectangle: on every side, three dashes with a
/// single-pixel gap between them (12 short strokes).
fn marquee_dashes() -> Vec<IconPart> {
    const DASHES: [(f32, f32); 3] = [(2.0, 5.0), (7.0, 9.0), (11.0, 14.0)];
    let mut parts = Vec::with_capacity(12);
    for (from, to) in DASHES {
        parts.push(stroke_part().m(from, 2.0).l(to, 2.0));
        parts.push(stroke_part().m(from, 14.0).l(to, 14.0));
        parts.push(stroke_part().m(2.0, from).l(2.0, to));
        parts.push(stroke_part().m(14.0, from).l(14.0, to));
    }
    parts
}

/// An icon's parts as tessellation-ready paths, scaled and placed.
#[derive(Debug, Clone)]
pub struct IconPaths {
    /// Every [`PartStyle::Fill`] part as one path, `None` when there is
    /// none.
    pub fill: Option<Path>,
    /// Every [`PartStyle::Stroke`] part as one path, `None` when there is
    /// none.
    pub stroke: Option<Path>,
    /// The stroke width for the placed size.
    pub stroke_width: f32,
}

/// `kind`'s [`IconPaths`] with the 24-unit box scaled to `side` and its
/// top-left corner at `origin` (both in the caller's own units — logical
/// px for a widget).
#[must_use]
pub fn icon_paths(kind: IconKind, origin: (f32, f32), side: f32) -> IconPaths {
    let scale = side / ICON_BOX;
    let at = |p: [f32; 2]| Point::new(origin.0 + p[0] * scale, origin.1 + p[1] * scale);
    let mut fill = PathBuilder::new();
    let mut stroke = PathBuilder::new();
    let (mut has_fill, mut has_stroke) = (false, false);
    for part in icon_parts(kind) {
        let (builder, flag) = match part.style {
            PartStyle::Fill => (&mut fill, &mut has_fill),
            PartStyle::Stroke => (&mut stroke, &mut has_stroke),
        };
        let mut open = false;
        for seg in &part.segs {
            match *seg {
                Seg::Move(p) => {
                    builder.move_to(at(p));
                    open = true;
                }
                Seg::Line(p) if open => {
                    builder.line_to(at(p));
                }
                Seg::Cubic(a, b, c) if open => {
                    builder.cubic_bezier_to(at(a), at(b), at(c));
                }
                // A segment before any Move would be a broken drawing;
                // `every_icon_part_starts_with_a_move` pins that.
                Seg::Line(_) | Seg::Cubic(..) => {}
            }
        }
        if open {
            *flag = true;
            if part.closed || part.style == PartStyle::Fill {
                builder.close();
            } else {
                builder.end();
            }
        }
    }
    IconPaths {
        fill: has_fill.then(|| fill.build()),
        stroke: has_stroke.then(|| stroke.build()),
        stroke_width: ICON_STROKE * scale,
    }
}

#[cfg(test)]
mod tests {
    use super::{ICON_BOX, ICON_STROKE, IconKind, PartStyle, Seg, icon_parts, icon_paths};
    use aurora_vector::{DEFAULT_TOLERANCE, fill, stroke_rounded};

    #[test]
    fn every_icon_has_parts_and_every_part_starts_with_a_move_and_has_a_segment_after_it() {
        for kind in IconKind::ALL {
            let parts = icon_parts(kind);
            assert!(!parts.is_empty(), "{kind:?} has no parts");
            for part in &parts {
                assert!(
                    matches!(part.segs.first(), Some(Seg::Move(_))),
                    "{kind:?}: a part must start with a Move"
                );
                assert!(
                    part.segs.len() >= 2,
                    "{kind:?}: a part needs at least one drawn segment"
                );
                assert_eq!(
                    part.segs
                        .iter()
                        .filter(|s| matches!(s, Seg::Move(_)))
                        .count(),
                    1,
                    "{kind:?}: one Move per part"
                );
            }
        }
    }

    #[test]
    fn the_set_has_every_named_icon_once_with_a_distinct_name_and_drawing() {
        assert_eq!(IconKind::ALL.len(), 29);
        for kind in [IconKind::SwapArrows, IconKind::DefaultColours] {
            assert!(IconKind::ALL.contains(&kind));
            assert!(!icon_parts(kind).is_empty(), "{kind:?}");
        }
        // The defaults icon is the one with a solid square (black over white).
        assert!(
            icon_parts(IconKind::DefaultColours)
                .iter()
                .any(|p| p.style == PartStyle::Fill)
        );
        assert!(
            icon_parts(IconKind::SwapArrows)
                .iter()
                .all(|p| p.style == PartStyle::Stroke)
        );
        for (i, a) in IconKind::ALL.iter().enumerate() {
            assert!(!a.name().is_empty());
            for b in IconKind::ALL.iter().skip(i + 1) {
                assert_ne!(a, b);
                assert_ne!(a.name(), b.name());
                assert_ne!(icon_parts(*a), icon_parts(*b), "{a:?} and {b:?} look alike");
            }
        }
    }

    /// The clip glyph (0.201.1): an open corner arrow, strokes only, that
    /// points down: its lowest point is the end of the stem and its
    /// arrowhead is wider than the stem is long from the left edge.
    #[test]
    fn the_clip_icon_is_an_open_corner_arrow_pointing_down() {
        let parts = icon_parts(IconKind::Clip);
        assert_eq!(parts.len(), 2, "the corner and the head");
        assert!(
            parts
                .iter()
                .all(|p| !p.closed && p.style == PartStyle::Stroke)
        );
        assert_eq!(IconKind::Clip.name(), "Clip");
        let paths = icon_paths(IconKind::Clip, (0.0, 0.0), ICON_BOX);
        let Some(path) = &paths.stroke else {
            unreachable!("the clip icon strokes");
        };
        let Ok(mesh) = stroke_rounded(path, paths.stroke_width, DEFAULT_TOLERANCE) else {
            unreachable!("tessellates");
        };
        let xs: Vec<f32> = mesh.vertices.iter().map(|v| v.x).collect();
        let ys: Vec<f32> = mesh.vertices.iter().map(|v| v.y).collect();
        let (min_x, max_x) = (
            xs.iter().copied().fold(f32::MAX, f32::min),
            xs.iter().copied().fold(f32::MIN, f32::max),
        );
        let (min_y, max_y) = (
            ys.iter().copied().fold(f32::MAX, f32::min),
            ys.iter().copied().fold(f32::MIN, f32::max),
        );
        assert!(max_x - min_x > 14.0, "wide: {min_x}..{max_x}");
        assert!(max_y - min_y > 12.0, "tall: {min_y}..{max_y}");
        // The arrowhead's apex (the lowest point) is right of the middle:
        // the stem is the right-hand part, the horizontal runs in from the
        // left at the top.
        let lowest_x = mesh
            .vertices
            .iter()
            .fold(None::<&aurora_vector::Point>, |best, v| match best {
                Some(b) if b.y >= v.y => Some(b),
                _ => Some(v),
            })
            .map(|v| v.x);
        assert!(lowest_x.is_some_and(|x| x > 12.0), "{lowest_x:?}");
        assert!(min_y < 6.0 && max_y > 18.0, "{min_y} {max_y}");
    }

    #[test]
    fn closed_and_open_parts_are_as_intended() {
        let closed_count = |kind| icon_parts(kind).iter().filter(|p| p.closed).count();
        let open_count = |kind| icon_parts(kind).iter().filter(|p| !p.closed).count();
        // Open on purpose: chevrons, the eye's slash, the "fx" strokes.
        assert_eq!(closed_count(IconKind::ChevronRight), 0);
        assert_eq!(closed_count(IconKind::ChevronDown), 0);
        assert_eq!(open_count(IconKind::Fx), 4);
        assert_eq!(closed_count(IconKind::Fx), 0);
        assert_eq!(open_count(IconKind::EyeOff), 1, "just the slash");
        // Closed on purpose: outlines.
        assert_eq!(closed_count(IconKind::FolderClosed), 1);
        assert_eq!(closed_count(IconKind::Eye), 2, "lens and pupil");
        assert_eq!(closed_count(IconKind::Mask), 2);
        assert_eq!(closed_count(IconKind::NewLayer), 1);
        // Every fill is a closed shape (a fill of an open one is
        // ambiguous).
        for kind in IconKind::ALL {
            for part in icon_parts(kind) {
                if part.style == PartStyle::Fill {
                    assert!(part.closed, "{kind:?}: a filled part must be closed");
                }
            }
        }
        assert!(
            icon_parts(IconKind::Adjustment)
                .iter()
                .any(|p| p.style == PartStyle::Fill),
            "the adjustment icon's half disc is a fill"
        );
    }

    #[test]
    fn every_icon_stays_inside_the_box_even_stroked() {
        let half = ICON_STROKE / 2.0;
        for kind in IconKind::ALL {
            // Control points: inside the box, inset by half the stroke.
            for part in icon_parts(kind) {
                for seg in &part.segs {
                    for [x, y] in seg.points() {
                        assert!(
                            x >= half && x <= ICON_BOX - half && y >= half && y <= ICON_BOX - half,
                            "{kind:?}: ({x}, {y}) leaves the inset box"
                        );
                    }
                }
            }
            // And the real tessellated shapes, at the box's own size.
            let paths = icon_paths(kind, (0.0, 0.0), ICON_BOX);
            let mut vertices = Vec::new();
            if let Some(path) = &paths.stroke
                && let Ok(mesh) = stroke_rounded(path, paths.stroke_width, DEFAULT_TOLERANCE)
            {
                vertices.extend(mesh.vertices);
            }
            if let Some(path) = &paths.fill
                && let Ok(mesh) = fill(path, DEFAULT_TOLERANCE)
            {
                vertices.extend(mesh.vertices);
            }
            assert!(!vertices.is_empty(), "{kind:?} tessellates to nothing");
            for v in vertices {
                assert!(
                    v.x >= -1e-3
                        && v.x <= ICON_BOX + 1e-3
                        && v.y >= -1e-3
                        && v.y <= ICON_BOX + 1e-3,
                    "{kind:?}: tessellated ({}, {}) leaves the 24 x 24 box",
                    v.x,
                    v.y
                );
            }
        }
    }

    #[test]
    fn geometry_is_deterministic_and_scales_with_the_requested_side() {
        for kind in IconKind::ALL {
            assert_eq!(icon_parts(kind), icon_parts(kind));
            let tess = |side: f32| {
                let paths = icon_paths(kind, (0.0, 0.0), side);
                let mut out = Vec::new();
                if let Some(p) = &paths.stroke
                    && let Ok(m) = stroke_rounded(p, paths.stroke_width, DEFAULT_TOLERANCE)
                {
                    out.extend(m.vertices);
                }
                (out, paths.stroke_width)
            };
            assert_eq!(tess(16.0), tess(16.0), "{kind:?} is not deterministic");
        }
        let small = icon_paths(IconKind::Eye, (0.0, 0.0), 16.0);
        let big = icon_paths(IconKind::Eye, (0.0, 0.0), 32.0);
        assert!((small.stroke_width - 1.0).abs() < 1e-6, "1.5/24*16 = 1 px");
        assert!((big.stroke_width - 2.0).abs() < 1e-6);
    }

    #[test]
    fn an_origin_offset_moves_every_point() {
        let at_zero = icon_paths(IconKind::ChevronRight, (0.0, 0.0), 24.0);
        let moved = icon_paths(IconKind::ChevronRight, (10.0, 20.0), 24.0);
        let (Some(a), Some(b)) = (at_zero.stroke, moved.stroke) else {
            unreachable!("a chevron strokes");
        };
        let (Ok(ma), Ok(mb)) = (
            stroke_rounded(&a, 1.5, DEFAULT_TOLERANCE),
            stroke_rounded(&b, 1.5, DEFAULT_TOLERANCE),
        ) else {
            unreachable!("tessellates");
        };
        assert_eq!(ma.vertices.len(), mb.vertices.len());
        for (p, q) in ma.vertices.iter().zip(&mb.vertices) {
            assert!((q.x - p.x - 10.0).abs() < 1e-3 && (q.y - p.y - 20.0).abs() < 1e-3);
        }
    }

    /// The icons that have an obvious up and down or left and right must
    /// come out the right way round (the symmetric tests above cannot
    /// tell a flipped drawing from the right one).
    #[test]
    fn icons_come_out_the_right_way_up() {
        let vertices = |kind| {
            let paths = icon_paths(kind, (0.0, 0.0), ICON_BOX);
            let mut out = Vec::new();
            if let Some(path) = &paths.stroke
                && let Ok(mesh) = stroke_rounded(path, paths.stroke_width, DEFAULT_TOLERANCE)
            {
                out.extend(mesh.vertices);
            }
            out
        };
        // A down chevron's lowest point is its apex, near the middle; its
        // highest points are its two ends, far apart.
        let down = vertices(IconKind::ChevronDown);
        let lowest = down
            .iter()
            .fold(None::<&aurora_vector::Point>, |best, v| match best {
                Some(b) if b.y >= v.y => Some(b),
                _ => Some(v),
            });
        let highest = down
            .iter()
            .fold(None::<&aurora_vector::Point>, |best, v| match best {
                Some(b) if b.y <= v.y => Some(b),
                _ => Some(v),
            });
        let (Some(lowest), Some(highest)) = (lowest, highest) else {
            unreachable!("a chevron has vertices");
        };
        assert!(
            (lowest.x - 12.75).abs() < 2.0,
            "apex at the bottom: {lowest:?}"
        );
        assert!(
            (highest.x - 12.75).abs() > 4.0,
            "ends at the top: {highest:?}"
        );
        // A lock's shackle is above its body: more empty box above than
        // below.
        let lock = vertices(IconKind::Lock);
        let top = lock.iter().map(|v| v.y).fold(f32::MAX, f32::min);
        let bottom = lock.iter().map(|v| v.y).fold(f32::MIN, f32::max);
        assert!(
            top > ICON_BOX - bottom,
            "the shackle leaves more room above than the body below: {top} {bottom}"
        );
        assert!(bottom > 20.0, "the body reaches the lower part: {bottom}");
        assert!(top < 4.0, "the shackle reaches the upper part: {top}");
        // A right chevron points right: its rightmost point is the apex.
        let right = vertices(IconKind::ChevronRight);
        let apex = right
            .iter()
            .fold(None::<&aurora_vector::Point>, |best, v| match best {
                Some(b) if b.x >= v.x => Some(b),
                _ => Some(v),
            });
        assert!(apex.is_some_and(|a| (a.y - 12.75).abs() < 2.0), "{apex:?}");
    }
}
