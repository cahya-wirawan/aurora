//! A colour picker: a saturation/value square, a hue strip, and a
//! preview of the picked colour — the first consumer of `0.124.0`'s
//! vertex-coloured gradient primitive (`aurora_vector::bilinear_rect`,
//! `aurora_vector::horizontal_strip`).
//!
//! **Scope: HSV only.** The model is hue/saturation/value in straight,
//! gamma-encoded sRGB (the same space the gradient primitive
//! interpolates in, so the square shows the colours the picker reports).
//! There is **no** hex or numeric text entry, **no** alpha channel, **no**
//! eyedropper, **no** colour management, and no glyphs of any kind (this
//! crate draws none). Every one of those is a design-owner decision
//! (`widgets`' own doc comment), not something built half-way here.
//!
//! **Shape: a pure state machine plus a structural reconcile**, the
//! same split `tab_bar.rs` uses. [`ColorPickerState`] owns the [`Hsv`]
//! value; every key, pointer gesture and setter is a transition on that
//! state alone (unit-tested with no tree). After every successful call —
//! a no-op included — the tree is reconciled to match: the picker's
//! persistent children (created once, at insert) are checked
//! structurally, and each one's **whole** payload and accessibility node
//! is compared with what the state says it should be. Only a widget that
//! disagrees is rewritten and dirtied, so a no-op produces no damage and
//! a structure damaged from outside is repaired by the next call.
//!
//! **Part ids are stable only while the structure is intact.** A
//! structural repair (a part removed from outside, a stale snapshot
//! written back) removes every picker-owned child and builds all five
//! parts afresh under **new** ids, so anything a caller attached to an
//! old id — a tooltip on the preview, say — goes with it (a tooltip
//! controller then reports its anchor as unknown on every call). Re-read
//! the ids ([`ColorPickerState::preview_id`] and friends) after any
//! outside change to the picker's subtree.
//!
//! **Do not add children under the picker or its square.** The picker
//! owns its subtree. A foreign child is left alone by an ordinary call
//! (ownership is decided by the tracked ids, plus the picker's own part
//! kind and preview signature for a stray left by a stale snapshot, never
//! by widget kind alone), but it takes part in the column's layout, and
//! one under the square is removed along with the square by the next
//! structural repair. The preview signature is a read-only `ColorWell`
//! swatch carrying a `value` string, so a caller's own swatch whose node
//! was given exactly that shape through `set_accessibility` is
//! indistinguishable from a stray preview and is removed by the next
//! structural repair too.
//!
//! # Structure
//!
//! ```text
//! ColorPicker (Role::Group, labelled, column)
//! ├── ColorPickerPart::Area        (Role::Group)  — paints the S/V gradient + marker
//! │   ├── ColorPickerPart::Saturation (Role::Slider, horizontal, inset 0)
//! │   └── ColorPickerPart::Value      (Role::Slider, vertical, inset 0)
//! ├── ColorPickerPart::Hue         (Role::Slider) — paints the hue strip + marker
//! └── ColorSwatch (preview)        (Role::ColorWell, read-only)
//! ```
//!
//! # Accessibility
//!
//! Checked against the pinned `accesskit` 0.24.1 (whose `Role` has no
//! two-dimensional "colour area" role) and `accesskit_consumer` 0.38.0.
//! The square is therefore exposed the way `react-aria`'s `ColorArea`
//! does it: **two sliders** (saturation, horizontal; value, vertical)
//! inside a labelled `Role::Group`, both covering the square exactly
//! (absolutely positioned, inset 0) and painting nothing. Every slider
//! declares `numeric_value`, `min_numeric_value` and
//! `max_numeric_value` explicitly (the Windows adapter's `RangeValue`
//! pattern needs `numeric_value`, and its min/max default to 0), plus
//! `numeric_value_step` (UIA `SmallChange`) and `numeric_value_jump`
//! (`LargeChange`). Saturation and value are exposed as percentages
//! (`0..=100`, step 1, jump 10), hue in degrees (`0..=360`, step 1,
//! jump 10), each rounded to one decimal place.
//!
//! **Tab stops: exactly two** — the saturation slider and the hue
//! slider declare `Action::Focus`; the value slider does not (one tab
//! stop per two-dimensional area, the APG's own guidance). All three
//! declare `SetValue`, `Increment` and `Decrement` while enabled. The
//! honest consequence: with focus on the saturation slider, `Up`/`Down`
//! change *value*, a sibling node's number, and whether a screen reader
//! announces that change has **not** been checked by a human on any
//! platform.
//!
//! The preview is a real [`WidgetKind::ColorSwatch`] (painted by the same
//! code a standalone swatch uses) whose node the picker builds itself:
//! `Role::ColorWell`, `read_only`, **no actions** (not a tab stop, not
//! clickable), `color_value` set — and, departing from a standalone
//! swatch, a `value` string (`#RRGGBB`, built with `format!`), because
//! no pinned adapter reads `color_value` at all and the Windows adapter
//! exposes the Value pattern whenever `value` is present. A caller that
//! writes the preview through `set_color_swatch_*` is overwriting a
//! picker-owned node; the next picker call repairs it.
//!
//! **Accessibility `ActionRequest`s are not routed** — the same crate-wide
//! gap every widget here has (`aurora-app` logs and drops them). A routed
//! `SetValue`/`Increment`/`Decrement` on a channel slider would call
//! [`set_color_picker_hsv`] or [`handle_color_picker_key`] respectively.
//!
//! Every accessible name below the picker's own label (`"Saturation and
//! value"`, `"Saturation"`, `"Value"`, `"Hue"`, `"Colour"`) is an English
//! string fixed here; there is no localisation layer in this workspace.
//!
//! # Keys
//!
//! [`ColorPickerKey`] is this module's own six-key vocabulary, bridged
//! from `shortcut::NamedKey` by [`ColorPickerKey::from_named_key`]. Keys
//! are addressed to the **picker's** id together with the
//! [`ColorPickerPart`] they act on; a caller routing a key from the
//! focused widget resolves the part with [`color_picker_part_of`].
//! `coarse` selects the large step — the caller's own mapping (`Shift`,
//! typically), since `NamedKey` has no `PageUp`/`PageDown`.
//!
//! | part | key | result (fine step / coarse step) |
//! |---|---|---|
//! | any, disabled | any | `Err(WidgetDisabled)`, nothing changes |
//! | S/V | `Left`/`Right` | saturation −/+ 0.01 / 0.10 |
//! | S/V | `Up`/`Down` | value +/− 0.01 / 0.10 |
//! | S/V | `Home`/`End` | saturation 0 / 1 |
//! | hue | `Left`/`Down` | hue − 1° / 10°, clamped at 0 |
//! | hue | `Right`/`Up` | hue + 1° / 10°, clamped at 360 |
//! | hue | `Home`/`End` | hue 0 / 360 |
//! | any | a key that changes nothing (at a bound) | `Ignored`, no damage |
//!
//! A step **snaps directionally**: the new value is the next grid point
//! strictly past the current one in the key's direction — `(floor(x /
//! step) + 1) * step` up, `(ceil(x / step) - 1) * step` down — clamped.
//! An on-grid value therefore moves exactly one step, and an off-grid
//! one lands on the neighbouring grid point in the key's direction, so no
//! key ever moves further than one step (the advertised
//! `numeric_value_step`/`numeric_value_jump`). The floor/ceil tolerate a
//! grid value's own `f32` representation error (`0.3` stored as
//! `0.30000001` still steps up to `0.31`, not to `0.3` again). Keys **clamp**
//! hue at both ends rather than wrapping (the APG slider pattern, and no
//! marker jumping across the strip); hue is stored in `[0, 360]`
//! inclusive for that reason, `360` and `0` being the same colour.
//! Programmatic setters ([`set_color_picker_hsv`]) instead **wrap** an
//! out-of-range hue (`370` becomes `10`, `-10` becomes `350`) — they take
//! an arbitrary angle, not a gesture. The step sizes are this module's
//! documented defaults, not design tokens (none exist).
//!
//! # Pointer
//!
//! [`set_saturation_value_from_point`] and [`set_hue_from_point`] map a
//! point through the part's **unclipped** layout bounds (clamped to the
//! part, so a drag past the edge pins to it). A zero-width or
//! zero-height part (never laid out) is `Ignored` rather than dividing
//! by zero, and so is a non-finite point. [`color_picker_part_at`]
//! resolves a point through `WidgetTree::hit_test`, which respects the
//! same "not descended into" rule clipping does. The part is resolved
//! *before* the structural repair runs, so the first pointer gesture
//! after a part was removed from outside finds no laid-out part and is
//! `Ignored`; the repair then runs, and the next gesture (after a layout
//! pass) lands normally.
//!
//! # Achromatic colours
//!
//! RGB loses hue when saturation is zero and loses hue *and* saturation
//! when value is zero. [`Hsv::from_color`] therefore keeps the
//! previous hue for any grey, and the previous hue and saturation for
//! black, so dragging value to zero and back does not reset the square.
//!
//! # Size
//!
//! `size` (the square's side and the picker's width) is caller-supplied,
//! the same precedent `open_menu`'s width sets: no "picker size" token
//! exists, and inventing one is the design owner's call. It must be
//! finite and within `1.0..=1.0e6` (`MIN_SIZE..=MAX_SIZE`) logical pixels.
//!
//! # Markers
//!
//! The square's ring and the strip's bar are drawn **inside** their
//! part: the marker's centre is clamped so its whole stroked outline
//! stays within the part's own bounds (which is what the part's damage
//! covers, and what a flush clipping ancestor keeps). At the extremes
//! (`s` or `v` at `0`/`1`, hue at `0`/`360`) the marker therefore sits
//! half its own size in from the edge rather than centred on it; the
//! pointer mapping is unaffected and still uses the full, unclipped part.
//! A part too small to hold its marker (never laid out, or a tiny
//! `size`) paints no marker at all.
//!
//! # What it deliberately does not do
//!
//! No keyboard-focus ring (crate-wide gap), no text, no alpha, no
//! numeric fields, no pointer-capture state (a drag is the caller
//! calling a `*_from_point` function on every move), and no z-layering.

use accesskit::{Action, Node, Orientation, Role};
use aurora_theme::{Color, Scales};
use taffy::style_helpers::{auto, length, percent, zero};
use taffy::{AlignItems, FlexDirection, Position, Rect as LayoutRect, Size, Style};

use super::{WidgetKind, color_swatch, spacing, type_size};
use crate::error::WidgetError;
use crate::shortcut::NamedKey;
use crate::tree::{WidgetId, WidgetTree};

/// The fine saturation/value step (`0.01`, one percent).
const SV_STEP: f64 = 0.01;
/// The coarse saturation/value step (`0.10`, ten percent).
const SV_COARSE_STEP: f64 = 0.1;
/// The fine hue step, in degrees.
const HUE_STEP: f64 = 1.0;
/// The coarse hue step, in degrees.
const HUE_COARSE_STEP: f64 = 10.0;
/// The top of the hue range. `360` and `0` are the same colour.
const HUE_MAX: f32 = 360.0;
/// The smallest `size` [`insert_color_picker`] accepts, in logical
/// pixels: below one pixel the square and strip lay out empty.
const MIN_SIZE: f32 = 1.0;
/// The largest `size` [`insert_color_picker`] accepts, in logical
/// pixels — far past any real screen, and small enough that every
/// position derived from it stays exact in `f32` layout arithmetic.
const MAX_SIZE: f32 = 1.0e6;

/// A colour as hue, saturation and value, in straight gamma-encoded
/// sRGB.
///
/// `hue` is in degrees, `[0, 360]` inclusive (`360` is the same colour as
/// `0` — see this module's own doc comment for why both are kept);
/// `saturation` and `value` are in `[0, 1]`. The fields are public for
/// reading and for building a value to pass to [`set_color_picker_hsv`],
/// which normalises whatever it is given through [`Hsv::new`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hsv {
    pub hue: f32,
    pub saturation: f32,
    pub value: f32,
}

impl Hsv {
    /// A normalised `Hsv`: `hue` wrapped into `[0, 360)` (except that
    /// exactly `360` is kept), `saturation` and `value` clamped to
    /// `[0, 1]`. `None` if any component is not finite.
    #[must_use]
    pub fn new(hue: f32, saturation: f32, value: f32) -> Option<Self> {
        if !(hue.is_finite() && saturation.is_finite() && value.is_finite()) {
            return None;
        }
        let hue = if hue == HUE_MAX {
            HUE_MAX
        } else {
            // `+ 0.0` turns a `-0.0` into `0.0`. `rem_euclid` can round
            // a tiny negative angle up to exactly `360.0`, which is
            // still inside the inclusive range.
            hue.rem_euclid(HUE_MAX) + 0.0
        };
        Some(Self {
            hue,
            saturation: saturation.clamp(0.0, 1.0),
            value: value.clamp(0.0, 1.0),
        })
    }

    /// The colour as straight gamma-encoded sRGB channels in `[0, 1]`,
    /// computed in `f64` (the standard six-sector HSV formula).
    #[must_use]
    fn to_rgb_f64(self) -> [f64; 3] {
        let value = f64::from(self.value).clamp(0.0, 1.0);
        let saturation = f64::from(self.saturation).clamp(0.0, 1.0);
        let hue = f64::from(self.hue);
        // 360 is the same colour as 0; a non-finite hue (only reachable
        // through the public fields) is treated as 0 rather than
        // producing NaN channels.
        let hue = if hue.is_finite() {
            hue.rem_euclid(360.0)
        } else {
            0.0
        };
        let chroma = value * saturation;
        let sector_position = hue / 60.0;
        let middle = chroma * (1.0 - ((sector_position % 2.0) - 1.0).abs());
        let floor = value - chroma;
        #[allow(clippy::cast_sign_loss)]
        let sector = (sector_position.floor() as u32).min(5);
        let (red, green, blue) = match sector {
            0 => (chroma, middle, 0.0),
            1 => (middle, chroma, 0.0),
            2 => (0.0, chroma, middle),
            3 => (0.0, middle, chroma),
            4 => (middle, 0.0, chroma),
            _ => (chroma, 0.0, middle),
        };
        [red + floor, green + floor, blue + floor]
    }

    /// The colour as straight gamma-encoded sRGB channels in `[0, 1]` —
    /// [`aurora_theme::Color::to_srgb_f32`]'s own convention, and what a
    /// gradient vertex carries.
    #[must_use]
    pub fn to_srgb_f32(self) -> [f32; 3] {
        let [r, g, b] = self.to_rgb_f64();
        [r as f32, g as f32, b as f32]
    }

    /// The nearest 8-bit colour — each channel **rounded**, never
    /// truncated.
    #[must_use]
    pub fn to_color(self) -> Color {
        let [r, g, b] = self.to_rgb_f64();
        Color {
            r: to_u8(r),
            g: to_u8(g),
            b: to_u8(b),
        }
    }

    /// `color` as HSV. `previous` supplies what RGB cannot: a grey
    /// (`max == min`) keeps `previous.hue`, and black keeps
    /// `previous.hue` **and** `previous.saturation` — see "Achromatic
    /// colours" in this module's own doc comment. Only a **sane**
    /// `previous` value is carried over: a hue that is not finite or
    /// outside `[0, 360]`, or a saturation that is not finite or outside
    /// `[0, 1]` (both reachable through the public fields), is replaced
    /// by `0.0`, so the result always satisfies this type's own ranges.
    #[must_use]
    // Every comparison here is between exact small integers held in
    // `f64` (`0..=255`), so `==` is exact, not a rounding hazard.
    #[allow(clippy::float_cmp)]
    pub fn from_color(color: Color, previous: Hsv) -> Hsv {
        let r = f64::from(color.r);
        let g = f64::from(color.g);
        let b = f64::from(color.b);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let value = (max / 255.0) as f32;
        let previous_hue = if previous.hue.is_finite() && (0.0..=HUE_MAX).contains(&previous.hue) {
            previous.hue
        } else {
            0.0
        };
        let previous_saturation =
            if previous.saturation.is_finite() && (0.0..=1.0).contains(&previous.saturation) {
                previous.saturation
            } else {
                0.0
            };
        if max == 0.0 {
            return Hsv {
                hue: previous_hue,
                saturation: previous_saturation,
                value: 0.0,
            };
        }
        if max == min {
            return Hsv {
                hue: previous_hue,
                saturation: 0.0,
                value,
            };
        }
        let delta = max - min;
        let saturation = (delta / max) as f32;
        let sector = if max == r {
            ((g - b) / delta).rem_euclid(6.0)
        } else if max == g {
            (b - r) / delta + 2.0
        } else {
            (r - g) / delta + 4.0
        };
        let hue = (sector * 60.0) as f32;
        Hsv {
            hue: hue.clamp(0.0, HUE_MAX),
            saturation,
            value,
        }
    }
}

/// `channel` (`[0, 1]`) as a rounded 8-bit value.
#[allow(clippy::cast_sign_loss)]
fn to_u8(channel: f64) -> u8 {
    (channel.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Which part of a picker a key or gesture acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColorPickerPart {
    /// The saturation/value square (and its two channel sliders).
    SaturationValue,
    /// The hue strip.
    Hue,
}

/// The six keys a colour picker responds to — see this module's own doc
/// comment for the transition table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColorPickerKey {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
}

impl ColorPickerKey {
    /// The picker key `key` stands for, if any — `None` for every
    /// `NamedKey` a picker does not handle.
    #[must_use]
    pub fn from_named_key(key: NamedKey) -> Option<Self> {
        match key {
            NamedKey::ArrowLeft => Some(Self::Left),
            NamedKey::ArrowRight => Some(Self::Right),
            NamedKey::ArrowUp => Some(Self::Up),
            NamedKey::ArrowDown => Some(Self::Down),
            NamedKey::Home => Some(Self::Home),
            NamedKey::End => Some(Self::End),
            _ => None,
        }
    }
}

/// What a transition did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColorPickerOutcome {
    /// The picked colour changed. `color` is [`Hsv::to_color`] of `hsv`;
    /// a hue change at zero saturation is still `Changed` even though
    /// `color` is the same.
    Changed { hsv: Hsv, color: Color },
    /// Nothing changed at all.
    Ignored,
}

/// Which of a picker's own children a [`WidgetKind::ColorPickerPart`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColorPickerPartRole {
    /// The saturation/value square: paints the gradient and the marker.
    Area,
    /// The square's horizontal channel slider; paints nothing.
    Saturation,
    /// The square's vertical channel slider; paints nothing.
    Value,
    /// The hue strip: paints the strip and the marker.
    Hue,
}

/// The payload of one of a picker's own parts
/// ([`WidgetKind::ColorPickerPart`]). Created only by this module.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorPickerPartState {
    role: ColorPickerPartRole,
    hsv: Hsv,
    disabled: bool,
}

impl ColorPickerPartState {
    /// Which part this is.
    #[must_use]
    pub fn role(&self) -> ColorPickerPartRole {
        self.role
    }

    /// What this part displays. The hue strip only depends on the hue,
    /// so its copy carries saturation and value at `1` — a
    /// saturation/value change leaves it untouched and undamaged. The
    /// square **and its two channel sliders** carry the full value, so a
    /// hue-only change rewrites (and dirties) the sliders too. That is a
    /// deliberate simplicity choice, not a damage cost: the sliders are
    /// absolutely positioned exactly over the square, which a hue change
    /// dirties anyway, so the damaged area is the same either way.
    #[must_use]
    pub fn hsv(&self) -> Hsv {
        self.hsv
    }

    /// Whether the owning picker is disabled.
    #[must_use]
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }
}

/// The ids of a picker's persistent children.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PartIds {
    area: WidgetId,
    saturation: WidgetId,
    value: WidgetId,
    hue: WidgetId,
    preview: WidgetId,
}

/// Layout values resolved at insert and carried in the state, because a
/// lazy repair rebuilds children from mutators that take no `&Scales`.
#[derive(Debug, Clone, PartialEq)]
struct PickerMetrics {
    /// The caller's `size`: the square's side and the picker's width.
    size: f32,
    /// `spacing.sm`, between the square, the strip and the preview.
    gap: f32,
    /// `typography.size.md`, the hue strip's height.
    strip_height: f32,
    /// `typography.size.md`, the preview swatch's side — exactly what
    /// `color_swatch::style(scales)` resolves.
    preview_side: f32,
}

/// A colour picker's own state — the payload of its root
/// ([`WidgetKind::ColorPicker`]).
///
/// Only `label` is public; everything else is kept in lockstep with the
/// real children. `Clone`, so a caller can snapshot it; writing a stale
/// snapshot back is tolerated (the next successful call repairs the
/// structure) but is not a supported way to change a picker.
#[derive(Debug, Clone, PartialEq)]
pub struct ColorPickerState {
    /// The picker's own accessible name.
    pub label: String,
    hsv: Hsv,
    disabled: bool,
    parts: Option<PartIds>,
    metrics: PickerMetrics,
}

impl ColorPickerState {
    fn new(label: String, hsv: Hsv, metrics: PickerMetrics) -> Self {
        Self {
            label,
            hsv,
            disabled: false,
            parts: None,
            metrics,
        }
    }

    /// The picked colour, as HSV.
    #[must_use]
    pub fn hsv(&self) -> Hsv {
        self.hsv
    }

    /// The picked colour, rounded to 8 bits ([`Hsv::to_color`]).
    #[must_use]
    pub fn color(&self) -> Color {
        self.hsv.to_color()
    }

    #[must_use]
    pub fn disabled(&self) -> bool {
        self.disabled
    }

    /// The picker's own accessible name.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The widget `part` is addressed through: the saturation slider for
    /// the square (its tab stop), the hue slider for the strip.
    #[must_use]
    pub fn part_id(&self, part: ColorPickerPart) -> Option<WidgetId> {
        self.parts.map(|ids| match part {
            ColorPickerPart::SaturationValue => ids.saturation,
            ColorPickerPart::Hue => ids.hue,
        })
    }

    /// The widget to focus when the picker as a whole receives focus:
    /// the saturation slider, its first tab stop.
    #[must_use]
    pub fn focus_target(&self) -> Option<WidgetId> {
        self.part_id(ColorPickerPart::SaturationValue)
    }

    /// The preview swatch. **Not stable across a structural repair**
    /// (see this module's own doc comment): a repair replaces every part,
    /// the preview included, under a new id, so a tooltip or anything
    /// else anchored to the old one goes with it. Re-read this after any
    /// outside change to the picker's subtree.
    #[must_use]
    pub fn preview_id(&self) -> Option<WidgetId> {
        self.parts.map(|ids| ids.preview)
    }

    /// The saturation/value square (the painted `Area` part).
    #[must_use]
    pub fn area_id(&self) -> Option<WidgetId> {
        self.parts.map(|ids| ids.area)
    }

    /// The square's vertical channel slider.
    #[must_use]
    pub fn value_slider_id(&self) -> Option<WidgetId> {
        self.parts.map(|ids| ids.value)
    }

    /// Replaces the colour; `Ignored` if `next` is exactly the current one.
    fn set(&mut self, next: Hsv) -> ColorPickerOutcome {
        if next == self.hsv {
            return ColorPickerOutcome::Ignored;
        }
        self.hsv = next;
        ColorPickerOutcome::Changed {
            hsv: next,
            color: next.to_color(),
        }
    }

    fn apply_key(
        &mut self,
        id: WidgetId,
        part: ColorPickerPart,
        key: ColorPickerKey,
        coarse: bool,
    ) -> Result<ColorPickerOutcome, WidgetError> {
        if self.disabled {
            return Err(WidgetError::WidgetDisabled(id));
        }
        let next = keyed(self.hsv, part, key, coarse);
        Ok(self.set(next))
    }

    fn apply_disabled(&mut self, disabled: bool) -> bool {
        let changed = self.disabled != disabled;
        self.disabled = disabled;
        changed
    }
}

/// How far (in grid steps) a value may sit from a grid point and still
/// count as on it: far above an `f32` grid value's own representation
/// error in grid units (`360 * 2^-24` at most), far below any real
/// off-grid distance a key or pointer produces worth honouring.
const GRID_TOLERANCE: f64 = 1e-4;

/// `x` moved to the next grid point strictly past it in `direction`
/// (`-1.0` or `+1.0`), clamped to `[lo, hi]` — see "Keys" in this
/// module's own doc comment. Never more than one `step`.
fn step(x: f32, step: f64, direction: f64, lo: f32, hi: f32) -> f32 {
    let units = f64::from(x) / step;
    let index = if direction > 0.0 {
        (units + GRID_TOLERANCE).floor() + 1.0
    } else {
        (units - GRID_TOLERANCE).ceil() - 1.0
    };
    ((index * step) as f32).clamp(lo, hi)
}

/// The transition table for one key — pure, no tree.
fn keyed(hsv: Hsv, part: ColorPickerPart, key: ColorPickerKey, coarse: bool) -> Hsv {
    let mut next = hsv;
    match part {
        ColorPickerPart::SaturationValue => {
            let by = if coarse { SV_COARSE_STEP } else { SV_STEP };
            match key {
                ColorPickerKey::Left => next.saturation = step(hsv.saturation, by, -1.0, 0.0, 1.0),
                ColorPickerKey::Right => next.saturation = step(hsv.saturation, by, 1.0, 0.0, 1.0),
                ColorPickerKey::Up => next.value = step(hsv.value, by, 1.0, 0.0, 1.0),
                ColorPickerKey::Down => next.value = step(hsv.value, by, -1.0, 0.0, 1.0),
                ColorPickerKey::Home => next.saturation = 0.0,
                ColorPickerKey::End => next.saturation = 1.0,
            }
        }
        ColorPickerPart::Hue => {
            let by = if coarse { HUE_COARSE_STEP } else { HUE_STEP };
            match key {
                ColorPickerKey::Left | ColorPickerKey::Down => {
                    next.hue = step(hsv.hue, by, -1.0, 0.0, HUE_MAX);
                }
                ColorPickerKey::Right | ColorPickerKey::Up => {
                    next.hue = step(hsv.hue, by, 1.0, 0.0, HUE_MAX);
                }
                ColorPickerKey::Home => next.hue = 0.0,
                ColorPickerKey::End => next.hue = HUE_MAX,
            }
        }
    }
    next
}

/// `x` rounded to one decimal place, for an accessibility number.
fn one_decimal(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

fn root_node(state: &ColorPickerState) -> Node {
    let mut node = Node::new(Role::Group);
    node.set_label(state.label.clone());
    if state.disabled {
        node.set_disabled();
    }
    node
}

fn area_node(disabled: bool) -> Node {
    let mut node = Node::new(Role::Group);
    node.set_label("Saturation and value");
    if disabled {
        node.set_disabled();
    }
    node
}

/// One channel slider — see "Accessibility" in this module's own doc
/// comment. `focusable` is `true` for the saturation and hue sliders
/// only.
fn slider_node(
    label: &str,
    orientation: Orientation,
    value: f64,
    max: f64,
    focusable: bool,
    disabled: bool,
) -> Node {
    let mut node = Node::new(Role::Slider);
    node.set_label(label.to_owned());
    node.set_orientation(orientation);
    node.set_numeric_value(one_decimal(value));
    node.set_min_numeric_value(0.0);
    node.set_max_numeric_value(max);
    node.set_numeric_value_step(1.0);
    node.set_numeric_value_jump(10.0);
    if disabled {
        node.set_disabled();
    } else {
        if focusable {
            node.add_action(Action::Focus);
        }
        node.add_action(Action::SetValue);
        node.add_action(Action::Increment);
        node.add_action(Action::Decrement);
    }
    node
}

fn preview_node(color: Color, disabled: bool) -> Node {
    let mut node = Node::new(Role::ColorWell);
    node.set_label("Colour");
    node.set_color_value(accesskit::Color {
        red: color.r,
        green: color.g,
        blue: color.b,
        alpha: 255,
    });
    node.set_value(format!("#{:02X}{:02X}{:02X}", color.r, color.g, color.b));
    node.set_read_only();
    if disabled {
        node.set_disabled();
    }
    node
}

/// The payload a part should carry for `state` — see
/// [`ColorPickerPartState::hsv`] for the hue strip's projection.
fn part_payload(role: ColorPickerPartRole, state: &ColorPickerState) -> WidgetKind {
    let hsv = match role {
        ColorPickerPartRole::Hue => Hsv {
            hue: state.hsv.hue,
            saturation: 1.0,
            value: 1.0,
        },
        _ => state.hsv,
    };
    WidgetKind::ColorPickerPart(ColorPickerPartState {
        role,
        hsv,
        disabled: state.disabled,
    })
}

fn part_node(role: ColorPickerPartRole, state: &ColorPickerState) -> Node {
    let hsv = state.hsv;
    let disabled = state.disabled;
    match role {
        ColorPickerPartRole::Area => area_node(disabled),
        ColorPickerPartRole::Saturation => slider_node(
            "Saturation",
            Orientation::Horizontal,
            f64::from(hsv.saturation) * 100.0,
            100.0,
            true,
            disabled,
        ),
        ColorPickerPartRole::Value => slider_node(
            "Value",
            Orientation::Vertical,
            f64::from(hsv.value) * 100.0,
            100.0,
            false,
            disabled,
        ),
        ColorPickerPartRole::Hue => slider_node(
            "Hue",
            Orientation::Horizontal,
            f64::from(hsv.hue),
            f64::from(HUE_MAX),
            true,
            disabled,
        ),
    }
}

fn preview_payload(state: &ColorPickerState) -> WidgetKind {
    WidgetKind::ColorSwatch(color_swatch::ColorSwatchState {
        color: state.hsv.to_color(),
        disabled: state.disabled,
    })
}

/// The picker: a column `size` wide, its children `spacing.sm` apart.
/// `align_self: FlexStart` keeps a `Row` parent from stretching it.
fn root_style(metrics: &PickerMetrics) -> Style {
    Style {
        flex_direction: FlexDirection::Column,
        align_self: Some(AlignItems::FLEX_START),
        gap: Size {
            width: zero(),
            height: length(metrics.gap),
        },
        size: Size {
            width: length(metrics.size),
            height: auto(),
        },
        ..Default::default()
    }
}

/// The square: `size` by `size`.
fn area_style(metrics: &PickerMetrics) -> Style {
    Style {
        flex_shrink: 0.0,
        size: Size {
            width: length(metrics.size),
            height: length(metrics.size),
        },
        ..Default::default()
    }
}

/// A channel slider: absolutely positioned, inset 0 — exactly the
/// square's own box.
fn channel_style() -> Style {
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

/// The hue strip: the picker's full width, one line of text tall.
fn hue_style(metrics: &PickerMetrics) -> Style {
    Style {
        flex_shrink: 0.0,
        size: Size {
            width: percent(1.0_f32),
            height: length(metrics.strip_height),
        },
        ..Default::default()
    }
}

/// Adds a new, enabled colour picker showing `initial` as the last
/// child of `parent`. `size` is the square's side and the picker's
/// width, in the same logical pixels layout uses — see "Size" in this
/// module's own doc comment. The initial hue of a grey `initial` is `0`.
///
/// # Errors
///
/// Returns [`WidgetError::InvalidRange`] (carrying the accepted range,
/// `1.0..=1.0e6` (`MIN_SIZE..=MAX_SIZE`)) if `size` is not finite or lies
/// outside it, or [`WidgetError::UnknownWidget`] if `parent` doesn't
/// exist. Nothing is added when either happens.
pub fn insert_color_picker(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
    label: &str,
    initial: Color,
    size: f32,
) -> Result<WidgetId, WidgetError> {
    if !(size.is_finite() && (MIN_SIZE..=MAX_SIZE).contains(&size)) {
        return Err(WidgetError::InvalidRange {
            min: f64::from(MIN_SIZE),
            max: f64::from(MAX_SIZE),
        });
    }
    let metrics = PickerMetrics {
        size,
        gap: spacing(scales.spacing.sm),
        strip_height: type_size(scales.typography.size.md),
        preview_side: type_size(scales.typography.size.md),
    };
    let start = Hsv {
        hue: 0.0,
        saturation: 0.0,
        value: 0.0,
    };
    let state = ColorPickerState::new(label.to_owned(), Hsv::from_color(initial, start), metrics);
    let picker = tree.insert(
        parent,
        root_style(&state.metrics),
        root_node(&state),
        WidgetKind::ColorPicker(state),
    )?;
    if let Err(err) = rebuild_parts(tree, picker) {
        // Unreachable (`picker` was just inserted), but never leave a
        // half-built picker behind.
        let _ = tree.remove(picker);
        return Err(err);
    }
    Ok(picker)
}

fn state(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> Result<&ColorPickerState, WidgetError> {
    match tree.payload(id).ok_or(WidgetError::UnknownWidget(id))? {
        WidgetKind::ColorPicker(state) => Ok(state),
        _ => Err(WidgetError::WrongWidgetKind(id)),
    }
}

/// A read-only view of `picker`'s own [`ColorPickerState`].
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `picker` doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if it isn't a colour picker.
pub fn color_picker_state(
    tree: &WidgetTree<WidgetKind>,
    picker: WidgetId,
) -> Result<&ColorPickerState, WidgetError> {
    state(tree, picker)
}

/// Feeds one key to `part` of `picker` (the picker's id — see "Keys" in
/// this module's own doc comment). `coarse` selects the large step.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`]/[`WidgetError::WrongWidgetKind`]
/// for an id that isn't a colour picker, or
/// [`WidgetError::WidgetDisabled`] if it is disabled. Nothing changes
/// when any of these happens.
pub fn handle_color_picker_key(
    tree: &mut WidgetTree<WidgetKind>,
    picker: WidgetId,
    part: ColorPickerPart,
    key: ColorPickerKey,
    coarse: bool,
) -> Result<ColorPickerOutcome, WidgetError> {
    with_color_picker_mut(tree, picker, |state| {
        state.apply_key(picker, part, key, coarse)
    })
}

/// The fractions of `point` across the tracked part `id`'s own
/// **unclipped** bounds, each clamped to `[0, 1]` — `None` for a part
/// that doesn't exist, has zero width or height, or a non-finite point.
fn fractions(
    tree: &WidgetTree<WidgetKind>,
    id: Option<WidgetId>,
    point: (f32, f32),
) -> Option<(f32, f32)> {
    let bounds = tree.bounds(id?)?;
    if bounds.width == 0 || bounds.height == 0 || !point.0.is_finite() || !point.1.is_finite() {
        return None;
    }
    let fx = (f64::from(point.0) - bounds.x as f64) / f64::from(bounds.width);
    let fy = (f64::from(point.1) - bounds.y as f64) / f64::from(bounds.height);
    Some((fx.clamp(0.0, 1.0) as f32, fy.clamp(0.0, 1.0) as f32))
}

/// A pointer press or drag at `point` on the saturation/value square:
/// saturation from the horizontal position, value from the vertical one
/// (top is `1`), both clamped to the square — see "Pointer" in this
/// module's own doc comment.
///
/// # Errors
///
/// As [`handle_color_picker_key`].
pub fn set_saturation_value_from_point(
    tree: &mut WidgetTree<WidgetKind>,
    picker: WidgetId,
    point: (f32, f32),
) -> Result<ColorPickerOutcome, WidgetError> {
    let area = state(tree, picker)?.area_id();
    let at = fractions(tree, area, point);
    with_color_picker_mut(tree, picker, |state| {
        if state.disabled {
            return Err(WidgetError::WidgetDisabled(picker));
        }
        let Some((fx, fy)) = at else {
            return Ok(ColorPickerOutcome::Ignored);
        };
        let next = Hsv {
            hue: state.hsv.hue,
            saturation: fx,
            value: 1.0 - fy,
        };
        Ok(state.set(next))
    })
}

/// A pointer press or drag at `point` on the hue strip: hue from the
/// horizontal position (`0` at the left edge, `360` at the right),
/// clamped to the strip.
///
/// # Errors
///
/// As [`handle_color_picker_key`].
pub fn set_hue_from_point(
    tree: &mut WidgetTree<WidgetKind>,
    picker: WidgetId,
    point: (f32, f32),
) -> Result<ColorPickerOutcome, WidgetError> {
    let hue = state(tree, picker)?.part_id(ColorPickerPart::Hue);
    let at = fractions(tree, hue, point);
    with_color_picker_mut(tree, picker, |state| {
        if state.disabled {
            return Err(WidgetError::WidgetDisabled(picker));
        }
        let Some((fx, _)) = at else {
            return Ok(ColorPickerOutcome::Ignored);
        };
        let next = Hsv {
            hue: (f64::from(fx) * f64::from(HUE_MAX)) as f32,
            ..state.hsv
        };
        Ok(state.set(next))
    })
}

/// Which part of `picker` the widget `id` belongs to: the square, or
/// either of its channel sliders, is [`ColorPickerPart::SaturationValue`];
/// the hue strip is [`ColorPickerPart::Hue`]; anything else (the preview,
/// the picker itself, an unrelated widget) is `None`.
#[must_use]
pub fn color_picker_part_of(
    tree: &WidgetTree<WidgetKind>,
    picker: WidgetId,
    id: WidgetId,
) -> Option<ColorPickerPart> {
    let ids = state(tree, picker).ok()?.parts?;
    if id == ids.area || id == ids.saturation || id == ids.value {
        Some(ColorPickerPart::SaturationValue)
    } else if id == ids.hue {
        Some(ColorPickerPart::Hue)
    } else {
        None
    }
}

/// Which part of `picker` is under `point`, through
/// `WidgetTree::hit_test` — `None` over the preview, the gap, or
/// anything outside the picker.
#[must_use]
pub fn color_picker_part_at(
    tree: &WidgetTree<WidgetKind>,
    picker: WidgetId,
    point: (f32, f32),
) -> Option<ColorPickerPart> {
    color_picker_part_of(tree, picker, tree.hit_test(point)?)
}

/// Sets the picked colour from `hsv` — an owner-driven change (a
/// document's foreground colour changing elsewhere), so it works on a
/// disabled picker too. `hsv` is normalised through [`Hsv::new`]: an
/// out-of-range hue **wraps**, saturation and value clamp. Setting the
/// current value is `Ignored` and costs no damage.
///
/// # Errors
///
/// Returns [`WidgetError::InvalidRange`] if any component is not
/// finite — `0..=360` for a non-finite hue, else `0..=1` for the
/// non-finite saturation or value — or [`WidgetError::UnknownWidget`]/
/// [`WidgetError::WrongWidgetKind`] for an id that isn't a colour
/// picker. Nothing changes when any of these happens.
pub fn set_color_picker_hsv(
    tree: &mut WidgetTree<WidgetKind>,
    picker: WidgetId,
    hsv: Hsv,
) -> Result<ColorPickerOutcome, WidgetError> {
    // Checked before touching the tree, so a bad value changes nothing.
    state(tree, picker)?;
    let Some(next) = Hsv::new(hsv.hue, hsv.saturation, hsv.value) else {
        // Report the failing channel's own range.
        let max = if hsv.hue.is_finite() {
            1.0
        } else {
            f64::from(HUE_MAX)
        };
        return Err(WidgetError::InvalidRange { min: 0.0, max });
    };
    with_color_picker_mut(tree, picker, |state| Ok(state.set(next)))
}

/// Sets the picked colour from an 8-bit `color` ([`Hsv::from_color`],
/// keeping the current hue for a grey and the current hue and
/// saturation for black). Owner-driven: works on a disabled picker.
///
/// A `color` equal to the picker's own current [`ColorPickerState::color`]
/// is `Ignored` and leaves the HSV value **untouched** — so a two-way
/// binding that echoes the picker's reported colour straight back costs
/// no damage and never snaps the hue or saturation to what the 8-bit
/// colour alone implies (a dark colour's hue, `360` becoming `0`).
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`]/[`WidgetError::WrongWidgetKind`]
/// for an id that isn't a colour picker.
pub fn set_color_picker_color(
    tree: &mut WidgetTree<WidgetKind>,
    picker: WidgetId,
    color: Color,
) -> Result<ColorPickerOutcome, WidgetError> {
    with_color_picker_mut(tree, picker, |state| {
        if color == state.hsv.to_color() {
            return Ok(ColorPickerOutcome::Ignored);
        }
        let next = Hsv::from_color(color, state.hsv);
        Ok(state.set(next))
    })
}

/// Enables or disables `picker` and every part of it. A request that
/// matches the current state changes nothing — no damage.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `picker` doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if it isn't a colour picker.
pub fn set_color_picker_disabled(
    tree: &mut WidgetTree<WidgetKind>,
    picker: WidgetId,
    disabled: bool,
) -> Result<(), WidgetError> {
    with_color_picker_mut(tree, picker, |state| {
        state.apply_disabled(disabled);
        Ok(())
    })
}

/// The one path every mutator here goes through: run the pure
/// transition on the payload and then — **whether or not it changed
/// anything** — reconcile the children with the state and bring the
/// picker's own node in line. A transition that returns an error changes
/// nothing and reconciles nothing. Only a widget whose payload or node
/// disagrees with the state is rewritten and dirtied (both
/// `set_accessibility` and `mark_dirty`), so a no-op produces no damage.
fn with_color_picker_mut<T>(
    tree: &mut WidgetTree<WidgetKind>,
    picker: WidgetId,
    f: impl FnOnce(&mut ColorPickerState) -> Result<T, WidgetError>,
) -> Result<T, WidgetError> {
    let result = {
        let kind = tree
            .payload_mut(picker)
            .ok_or(WidgetError::UnknownWidget(picker))?;
        let WidgetKind::ColorPicker(state) = kind else {
            return Err(WidgetError::WrongWidgetKind(picker));
        };
        f(state)?
    };
    reconcile(tree, picker)?;
    let expected = root_node(state(tree, picker)?);
    if tree.accessibility(picker) != Some(&expected) {
        tree.set_accessibility(picker, expected)?;
        tree.mark_dirty(picker)?;
    }
    Ok(result)
}

/// Whether `child` is picker-owned: one of the ids `parts` tracks, a
/// [`WidgetKind::ColorPickerPart`] (only this module creates one), or a
/// swatch carrying the picker preview's own signature (a read-only
/// `Role::ColorWell` with a `value` string, which a standalone swatch
/// never sets — see [`preview_node`]) left untracked by a stale
/// snapshot. **Never by kind alone**: a caller's own `ColorSwatch` is
/// not picker-owned, so it is neither counted nor removed.
fn picker_owned(tree: &WidgetTree<WidgetKind>, parts: Option<PartIds>, child: WidgetId) -> bool {
    let tracked = parts.is_some_and(|ids| {
        [ids.area, ids.saturation, ids.value, ids.hue, ids.preview].contains(&child)
    });
    tracked
        || match tree.payload(child) {
            Some(WidgetKind::ColorPickerPart(_)) => true,
            Some(WidgetKind::ColorSwatch(_)) => tree.accessibility(child).is_some_and(|node| {
                node.role() == Role::ColorWell && node.is_read_only() && node.value().is_some()
            }),
            _ => false,
        }
}

/// The picker-owned children of `id` ([`picker_owned`]), in order.
fn owned_children(
    tree: &WidgetTree<WidgetKind>,
    parts: Option<PartIds>,
    id: WidgetId,
) -> Vec<WidgetId> {
    tree.children(id)
        .unwrap_or_default()
        .iter()
        .copied()
        .filter(|&child| picker_owned(tree, parts, child))
        .collect()
}

/// Makes the tree match `picker`'s state. **Trusts nothing it did not
/// just check**: if the picker's owned children are not exactly the
/// tracked square, strip and preview, or the square's are not exactly
/// the two tracked channel sliders, every part is rebuilt
/// ([`rebuild_parts`]); otherwise each part's whole payload and whole
/// accessibility node is compared with what the state says, and only a
/// part that disagrees is rewritten and dirtied.
fn reconcile(tree: &mut WidgetTree<WidgetKind>, picker: WidgetId) -> Result<(), WidgetError> {
    let current = state(tree, picker)?;
    let intact = current.parts.is_some_and(|ids| {
        owned_children(tree, Some(ids), picker) == [ids.area, ids.hue, ids.preview]
            && owned_children(tree, Some(ids), ids.area) == [ids.saturation, ids.value]
    });
    if !intact {
        return rebuild_parts(tree, picker);
    }
    let Some(ids) = current.parts else {
        return rebuild_parts(tree, picker);
    };
    let expected = [
        (
            ids.area,
            part_payload(ColorPickerPartRole::Area, current),
            part_node(ColorPickerPartRole::Area, current),
        ),
        (
            ids.saturation,
            part_payload(ColorPickerPartRole::Saturation, current),
            part_node(ColorPickerPartRole::Saturation, current),
        ),
        (
            ids.value,
            part_payload(ColorPickerPartRole::Value, current),
            part_node(ColorPickerPartRole::Value, current),
        ),
        (
            ids.hue,
            part_payload(ColorPickerPartRole::Hue, current),
            part_node(ColorPickerPartRole::Hue, current),
        ),
        (
            ids.preview,
            preview_payload(current),
            preview_node(current.hsv.to_color(), current.disabled),
        ),
    ];
    for (id, payload, node) in expected {
        let payload_ok = tree.payload(id) == Some(&payload);
        let node_ok = tree.accessibility(id) == Some(&node);
        if payload_ok && node_ok {
            continue;
        }
        if let Some(slot) = tree.payload_mut(id) {
            *slot = payload;
        }
        tree.set_accessibility(id, node)?;
        tree.mark_dirty(id)?;
    }
    Ok(())
}

/// Removes every picker-owned child of `picker` ([`picker_owned`], with
/// its subtree) and builds the five parts afresh under new ids, then
/// records them. A caller's own child is left in place.
/// `WidgetTree::remove` marks each removed widget's old bounds dirty.
fn rebuild_parts(tree: &mut WidgetTree<WidgetKind>, picker: WidgetId) -> Result<(), WidgetError> {
    let tracked = state(tree, picker)?.parts;
    for child in owned_children(tree, tracked, picker) {
        tree.remove(child)?;
    }
    let current = state(tree, picker)?.clone();
    let metrics = &current.metrics;
    let area = tree.insert(
        picker,
        area_style(metrics),
        part_node(ColorPickerPartRole::Area, &current),
        part_payload(ColorPickerPartRole::Area, &current),
    )?;
    let saturation = tree.insert(
        area,
        channel_style(),
        part_node(ColorPickerPartRole::Saturation, &current),
        part_payload(ColorPickerPartRole::Saturation, &current),
    )?;
    let value = tree.insert(
        area,
        channel_style(),
        part_node(ColorPickerPartRole::Value, &current),
        part_payload(ColorPickerPartRole::Value, &current),
    )?;
    let hue = tree.insert(
        picker,
        hue_style(metrics),
        part_node(ColorPickerPartRole::Hue, &current),
        part_payload(ColorPickerPartRole::Hue, &current),
    )?;
    let preview = tree.insert(
        picker,
        color_swatch::style_for_side(metrics.preview_side),
        preview_node(current.hsv.to_color(), current.disabled),
        preview_payload(&current),
    )?;
    let Some(WidgetKind::ColorPicker(state)) = tree.payload_mut(picker) else {
        return Err(WidgetError::WrongWidgetKind(picker));
    };
    state.parts = Some(PartIds {
        area,
        saturation,
        value,
        hue,
        preview,
    });
    Ok(())
}

#[cfg(test)]
// Geometry tests name a rect's `x`/`y`/`w`/`h` and HSV's `s`/`v`, as
// the formulas they check do; and exact float equality is the claim
// being tested (bit-exact stops, corners and key-table grid values).
#[allow(clippy::many_single_char_names, clippy::float_cmp)]
mod tests {
    use super::{
        ColorPickerKey, ColorPickerOutcome, ColorPickerPart, ColorPickerPartRole, ColorPickerState,
        Hsv, PickerMetrics, color_picker_part_at, color_picker_part_of, color_picker_state,
        handle_color_picker_key, insert_color_picker, keyed, set_color_picker_color,
        set_color_picker_disabled, set_color_picker_hsv, set_hue_from_point,
        set_saturation_value_from_point,
    };
    use crate::shortcut::NamedKey;
    use crate::tree::{WidgetId, WidgetTree};
    use crate::widgets::{
        WidgetKind, insert_button, insert_container, new_tree, set_color_swatch_color, spacing,
        test_scales, type_size,
    };
    use crate::{PaintOp, WidgetError, paint_widget, paint_widget_ops};
    use accesskit::{Action, Orientation, Role};
    use aurora_core::Rect;
    use aurora_theme::{Color, Palette, Theme, ThemeSet};
    use taffy::style_helpers::length;
    use taffy::{FlexDirection, Overflow, Size, Style};

    const ID: WidgetId = accesskit::NodeId(7);
    const SIZE: f32 = 128.0;
    const KEYS: [ColorPickerKey; 6] = [
        ColorPickerKey::Left,
        ColorPickerKey::Right,
        ColorPickerKey::Up,
        ColorPickerKey::Down,
        ColorPickerKey::Home,
        ColorPickerKey::End,
    ];
    const PARTS: [ColorPickerPart; 2] = [ColorPickerPart::SaturationValue, ColorPickerPart::Hue];

    fn hsv(hue: f32, saturation: f32, value: f32) -> Hsv {
        Hsv {
            hue,
            saturation,
            value,
        }
    }

    fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b }
    }

    fn ok<T>(result: Result<T, WidgetError>) -> T {
        match result {
            Ok(value) => value,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    // ---- Conversions ---------------------------------------------------

    /// Every one of the 16,777,216 8-bit colours survives RGB -> HSV ->
    /// RGB exactly.
    #[test]
    fn every_8bit_colour_round_trips_through_hsv_exactly() {
        let previous = hsv(0.0, 0.0, 0.0);
        let mut failures = 0_u32;
        for r in 0..=255_u8 {
            for g in 0..=255_u8 {
                for b in 0..=255_u8 {
                    let color = rgb(r, g, b);
                    let back = Hsv::from_color(color, previous);
                    if back.to_color() != color
                        || !(0.0..=360.0).contains(&back.hue)
                        || !(0.0..=1.0).contains(&back.saturation)
                        || !(0.0..=1.0).contains(&back.value)
                    {
                        failures += 1;
                    }
                }
            }
        }
        assert_eq!(failures, 0);
    }

    #[test]
    fn the_seven_hue_stops_are_exact_primaries_and_secondaries() {
        let expected = [
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 1.0, 1.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
        ];
        for (i, want) in (0_u8..).zip(expected) {
            assert_eq!(
                hsv(f32::from(i) * 60.0, 1.0, 1.0).to_srgb_f32(),
                want,
                "stop {i}"
            );
        }
        assert_eq!(
            hsv(360.0, 0.5, 0.75),
            hsv(0.0, 0.5, 0.75).with_same_colour()
        );
    }

    impl Hsv {
        /// Test-only: the same colour through a hue of 0 instead of 360.
        fn with_same_colour(self) -> Self {
            if self.hue == 0.0 {
                Self { hue: 360.0, ..self }
            } else {
                self
            }
        }
    }

    #[test]
    fn hue_360_is_the_same_colour_as_0() {
        for (s, v) in [(1.0, 1.0), (0.5, 0.25), (0.3, 0.9)] {
            assert_eq!(hsv(360.0, s, v).to_srgb_f32(), hsv(0.0, s, v).to_srgb_f32());
            assert_eq!(hsv(360.0, s, v).to_color(), hsv(0.0, s, v).to_color());
        }
    }

    #[test]
    fn new_wraps_hue_clamps_the_rest_and_rejects_non_finite() {
        let wrap = |h: f32| Hsv::new(h, 0.5, 0.5).map(|x| x.hue);
        assert_eq!(wrap(370.0), Some(10.0));
        assert_eq!(wrap(-10.0), Some(350.0));
        assert_eq!(wrap(360.0), Some(360.0));
        assert_eq!(wrap(720.0), Some(0.0));
        let zero = wrap(-0.0);
        assert_eq!(zero, Some(0.0));
        assert!(
            zero.is_some_and(f32::is_sign_positive),
            "-0.0 is normalised"
        );
        assert_eq!(Hsv::new(10.0, 2.0, -1.0), Some(hsv(10.0, 1.0, 0.0)));
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(Hsv::new(bad, 0.5, 0.5), None);
            assert_eq!(Hsv::new(10.0, bad, 0.5), None);
            assert_eq!(Hsv::new(10.0, 0.5, bad), None);
        }
    }

    #[test]
    fn a_grey_keeps_the_previous_hue_and_black_also_its_saturation() {
        let previous = hsv(210.0, 0.6, 0.8);
        assert_eq!(
            Hsv::from_color(rgb(128, 128, 128), previous),
            hsv(210.0, 0.0, 128.0 / 255.0)
        );
        assert_eq!(
            Hsv::from_color(rgb(255, 255, 255), previous),
            hsv(210.0, 0.0, 1.0)
        );
        assert_eq!(
            Hsv::from_color(rgb(0, 0, 0), previous),
            hsv(210.0, 0.6, 0.0)
        );
        // A chromatic colour ignores `previous` entirely.
        assert_eq!(
            Hsv::from_color(rgb(255, 0, 0), previous),
            hsv(0.0, 1.0, 1.0)
        );
    }

    /// A non-finite or out-of-range `previous` (reachable through the
    /// public fields) is never carried into the result.
    #[test]
    fn from_color_sanitises_a_bad_previous_hue_and_saturation() {
        let black = rgb(0, 0, 0);
        let grey = rgb(90, 90, 90);
        for bad in [
            hsv(f32::NAN, f32::NAN, 0.5),
            hsv(f32::INFINITY, f32::NEG_INFINITY, 0.5),
            hsv(400.0, 2.0, 0.5),
            hsv(-10.0, -0.5, 0.5),
        ] {
            let got = Hsv::from_color(black, bad);
            assert_eq!(
                (got.hue, got.saturation, got.value),
                (0.0, 0.0, 0.0),
                "{bad:?}"
            );
            let got = Hsv::from_color(grey, bad);
            assert_eq!((got.hue, got.saturation), (0.0, 0.0), "{bad:?}");
            assert!(got.value.is_finite());
        }
        // A sane previous is still carried, 360 included.
        let kept = Hsv::from_color(black, hsv(360.0, 0.25, 0.5));
        assert_eq!((kept.hue, kept.saturation), (360.0, 0.25));
    }

    #[test]
    fn to_color_rounds_rather_than_truncates() {
        // 0.5 * 255 = 127.5, which rounds to 128 (truncation gives 127).
        assert_eq!(hsv(0.0, 0.0, 0.5).to_color(), rgb(128, 128, 128));
        // 30 degrees at full saturation and value: green is exactly 127.5.
        assert_eq!(hsv(30.0, 1.0, 1.0).to_color(), rgb(255, 128, 0));
        assert_eq!(hsv(30.0, 1.0, 1.0).to_srgb_f32(), [1.0, 0.5, 0.0]);
    }

    // ---- Keys (pure) ---------------------------------------------------

    /// The documented key table, restated by **search** over the integer
    /// grid rather than through `keyed`'s own floor/ceil: the nearest
    /// grid point (as the `f32` a key would store) strictly past `x` in
    /// the key's direction, or `x`'s own bound when there is none.
    fn oracle(start: Hsv, part: ColorPickerPart, key: ColorPickerKey, coarse: bool) -> Hsv {
        let grid = |x: f32, units: f64, dir: i64, hi: i64| -> f32 {
            // `units` grid steps per unit of `x`; `hi` is the top, in steps.
            let at = |k: i64| (k as f64 / units) as f32;
            let found = if dir > 0 {
                (0..=hi).map(at).find(|&g| g > x)
            } else {
                (0..=hi).rev().map(at).find(|&g| g < x)
            };
            found.unwrap_or(if dir > 0 { at(hi) } else { 0.0 })
        };
        let mut next = start;
        match part {
            ColorPickerPart::SaturationValue => {
                let (units, hi) = if coarse { (10.0, 10) } else { (100.0, 100) };
                match key {
                    ColorPickerKey::Left => next.saturation = grid(start.saturation, units, -1, hi),
                    ColorPickerKey::Right => next.saturation = grid(start.saturation, units, 1, hi),
                    ColorPickerKey::Up => next.value = grid(start.value, units, 1, hi),
                    ColorPickerKey::Down => next.value = grid(start.value, units, -1, hi),
                    ColorPickerKey::Home => next.saturation = 0.0,
                    ColorPickerKey::End => next.saturation = 1.0,
                }
            }
            ColorPickerPart::Hue => {
                let (units, hi) = if coarse { (0.1, 36) } else { (1.0, 360) };
                match key {
                    ColorPickerKey::Left | ColorPickerKey::Down => {
                        next.hue = grid(start.hue, units, -1, hi);
                    }
                    ColorPickerKey::Right | ColorPickerKey::Up => {
                        next.hue = grid(start.hue, units, 1, hi);
                    }
                    ColorPickerKey::Home => next.hue = 0.0,
                    ColorPickerKey::End => next.hue = 360.0,
                }
            }
        }
        next
    }

    const STARTS: [Hsv; 4] = [
        Hsv {
            hue: 0.0,
            saturation: 0.0,
            value: 0.0,
        },
        Hsv {
            hue: 180.0,
            saturation: 0.5,
            value: 0.5,
        },
        Hsv {
            hue: 360.0,
            saturation: 1.0,
            value: 1.0,
        },
        Hsv {
            hue: 123.4,
            saturation: 0.237,
            value: 0.762,
        },
    ];

    fn pure(start: Hsv) -> ColorPickerState {
        let metrics = PickerMetrics {
            size: SIZE,
            gap: 1.0,
            strip_height: 1.0,
            preview_side: 1.0,
        };
        ColorPickerState::new("Colour".to_owned(), start, metrics)
    }

    #[test]
    fn every_part_key_and_step_follows_the_documented_table() {
        for start in STARTS {
            for part in PARTS {
                for key in KEYS {
                    for coarse in [false, true] {
                        let want = oracle(start, part, key, coarse);
                        let mut state = pure(start);
                        let outcome = ok(state.apply_key(ID, part, key, coarse));
                        let label = format!("{start:?} {part:?} {key:?} coarse={coarse}");
                        assert_eq!(state.hsv(), want, "{label}");
                        assert_eq!(keyed(start, part, key, coarse), want, "{label}");
                        if want == start {
                            assert_eq!(outcome, ColorPickerOutcome::Ignored, "{label}");
                        } else {
                            assert_eq!(
                                outcome,
                                ColorPickerOutcome::Changed {
                                    hsv: want,
                                    color: want.to_color()
                                },
                                "{label}"
                            );
                        }
                        // A part never touches the other part's channels.
                        match part {
                            ColorPickerPart::SaturationValue => {
                                assert_eq!(state.hsv().hue, start.hue, "{label}");
                            }
                            ColorPickerPart::Hue => {
                                assert_eq!(state.hsv().saturation, start.saturation, "{label}");
                                assert_eq!(state.hsv().value, start.value, "{label}");
                            }
                        }
                    }
                }
            }
        }
    }

    /// Spot checks written out by hand, independent of the oracle: the
    /// direction of every arrow, snapping, and clamping (never wrapping)
    /// at both ends of the hue strip.
    #[test]
    fn keys_move_the_documented_way_snap_and_clamp() {
        let sv = ColorPickerPart::SaturationValue;
        let h = ColorPickerPart::Hue;
        let mid = hsv(180.0, 0.5, 0.5);
        assert_eq!(keyed(mid, sv, ColorPickerKey::Up, false).value, 0.51);
        assert_eq!(keyed(mid, sv, ColorPickerKey::Down, false).value, 0.49);
        assert_eq!(
            keyed(mid, sv, ColorPickerKey::Right, false).saturation,
            0.51
        );
        assert_eq!(keyed(mid, sv, ColorPickerKey::Left, false).saturation, 0.49);
        assert_eq!(keyed(mid, sv, ColorPickerKey::Up, true).value, 0.6);
        // Off-grid values land on the neighbouring grid point in the
        // key's direction: 0.237 -> 0.24 / 0.23, 123.4 -> 124 / 120.
        let off = hsv(123.4, 0.237, 0.762);
        assert_eq!(
            keyed(off, sv, ColorPickerKey::Right, false).saturation,
            0.24
        );
        assert_eq!(keyed(off, sv, ColorPickerKey::Left, false).saturation, 0.23);
        assert_eq!(keyed(off, h, ColorPickerKey::Right, false).hue, 124.0);
        assert_eq!(keyed(off, h, ColorPickerKey::Left, true).hue, 120.0);
        // Hue clamps at both ends, whichever arrow is used.
        let top = hsv(360.0, 1.0, 1.0);
        let bottom = hsv(0.0, 1.0, 1.0);
        for key in [ColorPickerKey::Right, ColorPickerKey::Up] {
            assert_eq!(keyed(top, h, key, false), top);
            assert_eq!(keyed(bottom, h, key, false).hue, 1.0);
        }
        for key in [ColorPickerKey::Left, ColorPickerKey::Down] {
            assert_eq!(keyed(bottom, h, key, true), bottom);
            assert_eq!(keyed(top, h, key, false).hue, 359.0);
        }
        assert_eq!(keyed(mid, h, ColorPickerKey::Home, false).hue, 0.0);
        assert_eq!(keyed(mid, h, ColorPickerKey::End, false).hue, 360.0);
        assert_eq!(keyed(mid, sv, ColorPickerKey::Home, false).saturation, 0.0);
        assert_eq!(keyed(mid, sv, ColorPickerKey::End, false).saturation, 1.0);
    }

    /// No key moves further than one step (the advertised
    /// `numeric_value_step`/`numeric_value_jump`), off-grid or on it.
    #[test]
    fn a_step_never_moves_more_than_one_step_either_way() {
        let sv = ColorPickerPart::SaturationValue;
        let h = ColorPickerPart::Hue;
        let (left, right) = (ColorPickerKey::Left, ColorPickerKey::Right);
        let sat = |s: f32, key, coarse| keyed(hsv(0.0, s, 0.5), sv, key, coarse).saturation;
        let hue = |x: f32, key, coarse| keyed(hsv(x, 0.5, 0.5), h, key, coarse).hue;
        // Off-grid, coarse: halfway between two grid points.
        assert_eq!(sat(0.25, right, true), 0.3);
        assert_eq!(sat(0.25, left, true), 0.2);
        assert_eq!(sat(0.55, right, true), 0.6);
        assert_eq!(sat(0.55, left, true), 0.5);
        // On-grid: exactly one step, coarse and fine — including a grid
        // value whose `f32` is a hair off it in either direction.
        assert_eq!(sat(0.3, right, true), 0.4);
        assert_eq!(sat(0.3, left, true), 0.2);
        assert_eq!(sat(0.3, right, false), 0.31);
        assert_eq!(sat(0.3, left, false), 0.29);
        for hair in [
            f32::from_bits(0.3_f32.to_bits() - 1),
            f32::from_bits(0.3_f32.to_bits() + 1),
        ] {
            assert_eq!(sat(hair, right, false), 0.31, "{hair}");
            assert_eq!(sat(hair, left, false), 0.29, "{hair}");
        }
        assert_eq!(hue(5.0, right, true), 10.0);
        assert_eq!(hue(5.0, left, true), 0.0);
        assert_eq!(hue(354.9, right, true), 360.0);
        assert_eq!(hue(354.9, left, true), 350.0);
        // At a bound: nothing moves, so the transition is `Ignored`.
        for (start, part, key) in [
            (hsv(0.0, 1.0, 0.5), sv, right),
            (hsv(0.0, 0.0, 0.5), sv, left),
            (hsv(0.0, 0.5, 1.0), sv, ColorPickerKey::Up),
            (hsv(0.0, 0.5, 0.0), sv, ColorPickerKey::Down),
            (hsv(360.0, 0.5, 0.5), h, right),
            (hsv(0.0, 0.5, 0.5), h, left),
        ] {
            for coarse in [false, true] {
                let mut state = pure(start);
                assert_eq!(
                    ok(state.apply_key(ID, part, key, coarse)),
                    ColorPickerOutcome::Ignored
                );
                assert_eq!(state.hsv(), start);
            }
        }
    }

    /// Fine steps sweep each channel end to end in exactly 100 presses
    /// (hue: 360 fine, 36 coarse), every press `Changed`, the next one
    /// `Ignored` — in both directions.
    #[test]
    fn fine_and_coarse_sweeps_take_exactly_the_advertised_number_of_presses() {
        let sv = ColorPickerPart::SaturationValue;
        let h = ColorPickerPart::Hue;
        let cases = [
            (
                hsv(0.0, 0.0, 0.0),
                sv,
                ColorPickerKey::Right,
                ColorPickerKey::Left,
                false,
                100,
            ),
            (
                hsv(0.0, 0.0, 0.0),
                sv,
                ColorPickerKey::Up,
                ColorPickerKey::Down,
                false,
                100,
            ),
            (
                hsv(0.0, 0.0, 0.0),
                sv,
                ColorPickerKey::Right,
                ColorPickerKey::Left,
                true,
                10,
            ),
            (
                hsv(0.0, 0.0, 0.0),
                h,
                ColorPickerKey::Right,
                ColorPickerKey::Left,
                false,
                360,
            ),
            (
                hsv(0.0, 0.0, 0.0),
                h,
                ColorPickerKey::Up,
                ColorPickerKey::Down,
                true,
                36,
            ),
        ];
        for (start, part, forward, back, coarse, presses) in cases {
            let label = format!("{part:?} {forward:?} coarse={coarse}");
            let mut state = pure(start);
            for key in [forward, back] {
                for press in 0..presses {
                    let outcome = ok(state.apply_key(ID, part, key, coarse));
                    assert!(
                        matches!(outcome, ColorPickerOutcome::Changed { .. }),
                        "{label} {key:?} press {press}"
                    );
                }
                assert_eq!(
                    ok(state.apply_key(ID, part, key, coarse)),
                    ColorPickerOutcome::Ignored,
                    "{label} {key:?}"
                );
            }
            assert_eq!(state.hsv(), start, "{label}: back where it started");
        }
    }

    #[test]
    fn a_disabled_picker_refuses_every_key_and_changes_nothing() {
        for start in STARTS {
            let mut state = pure(start);
            assert!(state.apply_disabled(true));
            assert!(!state.apply_disabled(true));
            for part in PARTS {
                for key in KEYS {
                    match state.apply_key(ID, part, key, false) {
                        Err(WidgetError::WidgetDisabled(id)) => assert_eq!(id, ID),
                        other => unreachable!("expected WidgetDisabled, got {other:?}"),
                    }
                    assert_eq!(state.hsv(), start);
                }
            }
        }
    }

    #[test]
    fn named_keys_map_onto_the_six_picker_keys_and_nothing_else() {
        let all = [
            NamedKey::Enter,
            NamedKey::Escape,
            NamedKey::Tab,
            NamedKey::Backspace,
            NamedKey::Delete,
            NamedKey::Space,
            NamedKey::ArrowUp,
            NamedKey::ArrowDown,
            NamedKey::ArrowLeft,
            NamedKey::ArrowRight,
            NamedKey::Home,
            NamedKey::End,
            NamedKey::F1,
        ];
        let mapped: Vec<_> = all
            .into_iter()
            .filter_map(ColorPickerKey::from_named_key)
            .collect();
        assert_eq!(
            mapped,
            vec![
                ColorPickerKey::Up,
                ColorPickerKey::Down,
                ColorPickerKey::Left,
                ColorPickerKey::Right,
                ColorPickerKey::Home,
                ColorPickerKey::End
            ]
        );
    }

    // ---- Tree ----------------------------------------------------------

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

    const INITIAL: Color = Color {
        r: 64,
        g: 128,
        b: 191,
    };

    fn inserted() -> (WidgetTree<WidgetKind>, WidgetId) {
        let (mut tree, root) = new_tree(sized_root());
        let picker = ok(insert_color_picker(
            &mut tree,
            root,
            &test_scales(),
            "Foreground",
            INITIAL,
            SIZE,
        ));
        tree.compute_layout(320.0, 320.0);
        (tree, picker)
    }

    fn snapshot(tree: &WidgetTree<WidgetKind>, picker: WidgetId) -> ColorPickerState {
        ok(color_picker_state(tree, picker)).clone()
    }

    struct Ids {
        area: WidgetId,
        saturation: WidgetId,
        value: WidgetId,
        hue: WidgetId,
        preview: WidgetId,
    }

    fn ids(tree: &WidgetTree<WidgetKind>, picker: WidgetId) -> Ids {
        let state = snapshot(tree, picker);
        let (Some(area), Some(saturation), Some(value), Some(hue), Some(preview)) = (
            state.area_id(),
            state.part_id(ColorPickerPart::SaturationValue),
            state.value_slider_id(),
            state.part_id(ColorPickerPart::Hue),
            state.preview_id(),
        ) else {
            unreachable!("a built picker tracks every part");
        };
        Ids {
            area,
            saturation,
            value,
            hue,
            preview,
        }
    }

    /// Every structural and per-node claim, from scratch.
    #[allow(clippy::too_many_lines)]
    fn assert_structure_is_sound(tree: &WidgetTree<WidgetKind>, picker: WidgetId) {
        let state = snapshot(tree, picker);
        let ids = ids(tree, picker);
        assert_eq!(
            tree.children(picker),
            Some([ids.area, ids.hue, ids.preview].as_slice())
        );
        assert_eq!(
            tree.children(ids.area),
            Some([ids.saturation, ids.value].as_slice())
        );
        let disabled = state.disabled();
        let hsv = state.hsv();
        let node = |id| {
            let Some(node) = tree.accessibility(id) else {
                unreachable!("live");
            };
            node
        };
        let root = node(picker);
        assert_eq!(root.role(), Role::Group);
        assert_eq!(root.label(), Some("Foreground"));
        assert_eq!(root.is_disabled(), disabled);
        let area = node(ids.area);
        assert_eq!(area.role(), Role::Group);
        assert!(area.label().is_some());
        for (id, role, numeric, max, orientation, focus) in [
            (
                ids.saturation,
                ColorPickerPartRole::Saturation,
                f64::from(hsv.saturation) * 100.0,
                100.0,
                Orientation::Horizontal,
                true,
            ),
            (
                ids.value,
                ColorPickerPartRole::Value,
                f64::from(hsv.value) * 100.0,
                100.0,
                Orientation::Vertical,
                false,
            ),
            (
                ids.hue,
                ColorPickerPartRole::Hue,
                f64::from(hsv.hue),
                360.0,
                Orientation::Horizontal,
                true,
            ),
        ] {
            let n = node(id);
            assert_eq!(n.role(), Role::Slider, "{role:?}");
            assert_eq!(n.orientation(), Some(orientation), "{role:?}");
            assert_eq!(n.numeric_value(), Some((numeric * 10.0).round() / 10.0));
            assert_eq!(n.min_numeric_value(), Some(0.0));
            assert_eq!(n.max_numeric_value(), Some(max));
            assert_eq!(n.numeric_value_step(), Some(1.0));
            assert_eq!(n.numeric_value_jump(), Some(10.0));
            assert_eq!(n.is_disabled(), disabled);
            assert_eq!(n.supports_action(Action::Focus), focus && !disabled);
            for action in [Action::SetValue, Action::Increment, Action::Decrement] {
                assert_eq!(n.supports_action(action), !disabled, "{role:?} {action:?}");
            }
            let Some(WidgetKind::ColorPickerPart(payload)) = tree.payload(id) else {
                unreachable!("a part");
            };
            assert_eq!(payload.role(), role);
            assert_eq!(payload.is_disabled(), disabled);
        }
        let Some(WidgetKind::ColorPickerPart(area_payload)) = tree.payload(ids.area) else {
            unreachable!("the area");
        };
        assert_eq!(area_payload.hsv(), hsv);
        let Some(WidgetKind::ColorPickerPart(hue_payload)) = tree.payload(ids.hue) else {
            unreachable!("the strip");
        };
        assert_eq!(
            hue_payload.hsv(),
            Hsv {
                hue: hsv.hue,
                saturation: 1.0,
                value: 1.0
            }
        );
        let preview = node(ids.preview);
        let color = hsv.to_color();
        assert_eq!(preview.role(), Role::ColorWell);
        assert!(preview.is_read_only());
        assert!(!preview.supports_action(Action::Focus));
        assert!(!preview.supports_action(Action::Click));
        assert_eq!(
            preview.color_value(),
            Some(accesskit::Color {
                red: color.r,
                green: color.g,
                blue: color.b,
                alpha: 255
            })
        );
        assert_eq!(
            preview.value(),
            Some(format!("#{:02X}{:02X}{:02X}", color.r, color.g, color.b).as_str())
        );
        assert_eq!(preview.is_disabled(), disabled);
        match tree.payload(ids.preview) {
            Some(WidgetKind::ColorSwatch(swatch)) => {
                assert_eq!(swatch.color, color);
                assert_eq!(swatch.disabled, disabled);
            }
            other => unreachable!("the preview is a swatch: {other:?}"),
        }
    }

    #[test]
    fn insert_builds_the_documented_structure() {
        let (tree, picker) = inserted();
        assert_structure_is_sound(&tree, picker);
        let state = snapshot(&tree, picker);
        assert_eq!(state.color(), INITIAL);
        assert_eq!(state.label(), "Foreground");
        assert_eq!(
            state.focus_target(),
            state.part_id(ColorPickerPart::SaturationValue)
        );
    }

    #[test]
    fn insert_rejects_a_bad_size_and_an_unknown_parent_and_adds_nothing() {
        let (mut tree, root) = new_tree(sized_root());
        let scales = test_scales();
        for bad in [
            0.0,
            -0.0,
            -1.0,
            0.5,
            1.0e-6,
            f32::MIN_POSITIVE,
            1.0e6 + 1.0,
            1.0e9,
            1.0e20,
            f32::MAX,
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
        ] {
            match insert_color_picker(&mut tree, root, &scales, "C", INITIAL, bad) {
                Err(WidgetError::InvalidRange { min, max }) => {
                    // The accepted range, never the rejected value itself.
                    assert_eq!((min, max), (1.0, 1.0e6), "{bad}");
                }
                other => unreachable!("expected InvalidRange for {bad}, got {other:?}"),
            }
        }
        assert_eq!(tree.len(), 1);
        for good in [super::MIN_SIZE, super::MAX_SIZE] {
            let picker = ok(insert_color_picker(
                &mut tree, root, &scales, "C", INITIAL, good,
            ));
            ok(tree.remove(picker));
        }
        let bogus = accesskit::NodeId(999);
        match insert_color_picker(&mut tree, bogus, &scales, "C", INITIAL, SIZE) {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
        assert_eq!(tree.len(), 1);
    }

    #[test]
    fn layout_uses_the_token_sizes_and_the_channel_sliders_cover_the_square() {
        let (tree, picker) = inserted();
        let scales = test_scales();
        let ids = ids(&tree, picker);
        let bounds = |id| {
            let Some(b) = tree.bounds(id) else {
                unreachable!("live");
            };
            b
        };
        #[allow(clippy::cast_sign_loss)]
        let size = SIZE as u32;
        #[allow(clippy::cast_sign_loss)]
        let gap = spacing(scales.spacing.sm) as i64;
        #[allow(clippy::cast_sign_loss)]
        let line = type_size(scales.typography.size.md) as u32;
        let area = bounds(ids.area);
        assert_eq!((area.width, area.height), (size, size));
        assert_eq!(bounds(ids.saturation), area);
        assert_eq!(bounds(ids.value), area);
        let hue = bounds(ids.hue);
        assert_eq!((hue.x, hue.width, hue.height), (area.x, size, line));
        assert_eq!(hue.y, area.y + i64::from(size) + gap);
        let preview = bounds(ids.preview);
        assert_eq!((preview.width, preview.height), (line, line));
        assert_eq!(preview.y, hue.y + i64::from(line) + gap);
        assert_eq!(bounds(picker).width, size);
    }

    #[test]
    fn a_key_updates_state_payloads_and_nodes() {
        let (mut tree, picker) = inserted();
        let before = snapshot(&tree, picker).hsv();
        let outcome = ok(handle_color_picker_key(
            &mut tree,
            picker,
            ColorPickerPart::SaturationValue,
            ColorPickerKey::Up,
            true,
        ));
        let after = snapshot(&tree, picker).hsv();
        assert_ne!(after, before);
        assert_eq!(
            outcome,
            ColorPickerOutcome::Changed {
                hsv: after,
                color: after.to_color()
            }
        );
        assert_structure_is_sound(&tree, picker);
    }

    fn dirty(tree: &WidgetTree<WidgetKind>, ids: &[WidgetId]) -> Vec<bool> {
        ids.iter()
            .map(|&id| tree.is_dirty(id) == Some(true))
            .collect()
    }

    #[test]
    fn a_saturation_value_change_dirties_only_the_square_its_sliders_and_the_preview() {
        let (mut tree, picker) = inserted();
        let i = ids(&tree, picker);
        let all = [picker, i.area, i.saturation, i.value, i.hue, i.preview];
        tree.take_damage();
        ok(set_saturation_value_from_point(
            &mut tree,
            picker,
            (10.0, 10.0),
        ));
        assert_eq!(
            dirty(&tree, &all),
            vec![false, true, true, true, false, true]
        );
        tree.take_damage();
        // The initial hue is ~209.76, so the first coarse step only snaps
        // it up to 210 (too small to change this pale colour's 8-bit
        // preview); the second is a full 10 degrees.
        for _ in 0..2 {
            ok(handle_color_picker_key(
                &mut tree,
                picker,
                ColorPickerPart::Hue,
                ColorPickerKey::Right,
                true,
            ));
        }
        assert!(dirty(&tree, &[i.area, i.hue, i.preview]).iter().all(|&d| d));
        assert_eq!(tree.is_dirty(picker), Some(false));
    }

    #[test]
    fn no_op_transitions_produce_no_damage() {
        let (mut tree, picker) = inserted();
        tree.take_damage();
        let current = snapshot(&tree, picker).hsv();
        assert_eq!(
            ok(set_color_picker_hsv(&mut tree, picker, current)),
            ColorPickerOutcome::Ignored
        );
        assert_eq!(
            ok(set_color_picker_color(&mut tree, picker, INITIAL)),
            ColorPickerOutcome::Ignored
        );
        ok(set_color_picker_disabled(&mut tree, picker, false));
        ok(handle_color_picker_key(
            &mut tree,
            picker,
            ColorPickerPart::Hue,
            ColorPickerKey::Home,
            false,
        ));
        ok(handle_color_picker_key(
            &mut tree,
            picker,
            ColorPickerPart::Hue,
            ColorPickerKey::Home,
            false,
        ));
        tree.take_damage();
        ok(handle_color_picker_key(
            &mut tree,
            picker,
            ColorPickerPart::Hue,
            ColorPickerKey::Home,
            false,
        ));
        assert_eq!(tree.take_damage(), None);
    }

    #[test]
    fn errors_change_nothing_and_cost_no_damage() {
        let (mut tree, picker) = inserted();
        ok(set_color_picker_disabled(&mut tree, picker, true));
        assert_structure_is_sound(&tree, picker);
        tree.take_damage();
        let before = snapshot(&tree, picker);
        for part in PARTS {
            for key in KEYS {
                match handle_color_picker_key(&mut tree, picker, part, key, false) {
                    Err(WidgetError::WidgetDisabled(id)) => assert_eq!(id, picker),
                    other => unreachable!("expected WidgetDisabled, got {other:?}"),
                }
            }
        }
        for result in [
            set_saturation_value_from_point(&mut tree, picker, (1.0, 1.0)),
            set_hue_from_point(&mut tree, picker, (1.0, 1.0)),
        ] {
            match result {
                Err(WidgetError::WidgetDisabled(id)) => assert_eq!(id, picker),
                other => unreachable!("expected WidgetDisabled, got {other:?}"),
            }
        }
        // Each failure reports the failing channel's own range.
        for (bad, range) in [
            (hsv(f32::NAN, 0.5, 0.5), (0.0, 360.0)),
            (hsv(f32::INFINITY, f32::NAN, 0.5), (0.0, 360.0)),
            (hsv(10.0, f32::NAN, 0.5), (0.0, 1.0)),
            (hsv(10.0, 0.5, f32::NEG_INFINITY), (0.0, 1.0)),
        ] {
            match set_color_picker_hsv(&mut tree, picker, bad) {
                Err(WidgetError::InvalidRange { min, max }) => assert_eq!((min, max), range),
                other => unreachable!("expected InvalidRange, got {other:?}"),
            }
        }
        assert_eq!(snapshot(&tree, picker), before);
        assert_eq!(tree.take_damage(), None);
    }

    #[test]
    fn owner_setters_work_while_disabled_wrap_hue_and_keep_hue_for_a_grey() {
        let (mut tree, picker) = inserted();
        ok(set_color_picker_disabled(&mut tree, picker, true));
        ok(set_color_picker_hsv(
            &mut tree,
            picker,
            hsv(370.0, 0.5, 0.5),
        ));
        assert_eq!(snapshot(&tree, picker).hsv(), hsv(10.0, 0.5, 0.5));
        ok(set_color_picker_hsv(
            &mut tree,
            picker,
            hsv(-10.0, 2.0, -1.0),
        ));
        assert_eq!(snapshot(&tree, picker).hsv(), hsv(350.0, 1.0, 0.0));
        ok(set_color_picker_color(
            &mut tree,
            picker,
            rgb(128, 128, 128),
        ));
        assert_eq!(snapshot(&tree, picker).hsv().hue, 350.0);
        // A hue change at zero saturation is Changed, same 8-bit colour.
        let outcome = ok(set_color_picker_hsv(&mut tree, picker, hsv(20.0, 0.0, 0.5)));
        let ColorPickerOutcome::Changed { color, .. } = outcome else {
            unreachable!("{outcome:?}");
        };
        let changed = ok(set_color_picker_hsv(&mut tree, picker, hsv(90.0, 0.0, 0.5)));
        assert_eq!(
            changed,
            ColorPickerOutcome::Changed {
                hsv: hsv(90.0, 0.0, 0.5),
                color
            }
        );
        assert_structure_is_sound(&tree, picker);
    }

    #[test]
    fn mutators_reject_a_wrong_widget_kind_and_an_unknown_widget() {
        let (mut tree, picker) = inserted();
        let root = tree.root();
        let bogus = accesskit::NodeId(999);
        for (id, want_unknown) in [(root, false), (bogus, true)] {
            let results = [
                handle_color_picker_key(
                    &mut tree,
                    id,
                    ColorPickerPart::Hue,
                    ColorPickerKey::Left,
                    false,
                )
                .map(|_| ()),
                set_color_picker_hsv(&mut tree, id, hsv(1.0, 1.0, 1.0)).map(|_| ()),
                set_color_picker_color(&mut tree, id, INITIAL).map(|_| ()),
                set_color_picker_disabled(&mut tree, id, true),
                set_saturation_value_from_point(&mut tree, id, (1.0, 1.0)).map(|_| ()),
                set_hue_from_point(&mut tree, id, (1.0, 1.0)).map(|_| ()),
                color_picker_state(&tree, id).map(|_| ()),
            ];
            for result in results {
                match (result, want_unknown) {
                    (Err(WidgetError::UnknownWidget(got)), true)
                    | (Err(WidgetError::WrongWidgetKind(got)), false) => assert_eq!(got, id),
                    (other, _) => unreachable!("{other:?}"),
                }
            }
        }
        assert_structure_is_sound(&tree, picker);
    }

    // ---- Lazy repair ---------------------------------------------------

    #[test]
    fn a_part_removed_externally_is_rebuilt_on_the_next_call() {
        for victim in 0..5 {
            let (mut tree, picker) = inserted();
            let i = ids(&tree, picker);
            let target = [i.area, i.saturation, i.value, i.hue, i.preview];
            let Some(&id) = target.get(victim) else {
                unreachable!();
            };
            ok(tree.remove(id));
            ok(set_color_picker_disabled(&mut tree, picker, false));
            assert_structure_is_sound(&tree, picker);
            assert!(!tree.contains(id) || victim == 0, "a fresh id replaced it");
        }
    }

    #[test]
    fn a_stale_payload_or_an_overwritten_node_is_repaired_by_a_no_op() {
        let (mut tree, picker) = inserted();
        let i = ids(&tree, picker);
        if let Some(WidgetKind::ColorPickerPart(part)) = tree.payload_mut(i.area) {
            part.hsv = hsv(1.0, 1.0, 1.0);
        }
        let mut focusable_value = accesskit::Node::new(Role::Slider);
        focusable_value.add_action(Action::Focus);
        ok(tree.set_accessibility(i.value, focusable_value));
        ok(set_color_swatch_color(&mut tree, i.preview, rgb(1, 2, 3)));
        ok(tree.set_accessibility(picker, accesskit::Node::new(Role::Button)));
        tree.take_damage();
        let current = snapshot(&tree, picker).hsv();
        assert_eq!(
            ok(set_color_picker_hsv(&mut tree, picker, current)),
            ColorPickerOutcome::Ignored
        );
        assert_structure_is_sound(&tree, picker);
        assert_eq!(
            dirty(&tree, &[picker, i.area, i.value, i.preview, i.hue]),
            vec![true, true, true, true, false]
        );
        tree.take_damage();
        ok(set_color_picker_hsv(&mut tree, picker, current));
        assert_eq!(
            tree.take_damage(),
            None,
            "repaired once, then a no-op again"
        );
    }

    #[test]
    fn a_stale_snapshot_written_back_is_rebuilt_without_duplicates() {
        let (mut tree, picker) = inserted();
        let mut stale = snapshot(&tree, picker);
        stale.parts = None;
        if let Some(slot) = tree.payload_mut(picker) {
            *slot = WidgetKind::ColorPicker(stale);
        }
        ok(set_color_picker_disabled(&mut tree, picker, false));
        assert_structure_is_sound(&tree, picker);
        assert_eq!(tree.children(picker).map(<[_]>::len), Some(3));
    }

    /// Echoing the picker's own reported colour back (a two-way binding)
    /// is `Ignored`, costs no damage and leaves the HSV exactly as it was
    /// — even where the 8-bit colour alone would imply another hue or
    /// saturation (a near-black, a near-grey, hue `360`).
    #[test]
    fn echoing_the_pickers_own_colour_back_changes_nothing() {
        for start in [
            hsv(200.0, 0.03, 0.1),
            hsv(10.0, 0.5, 0.004),
            // rgb(191, 64, 64) at hue 360 rather than 0.
            hsv(360.0, 127.0 / 191.0, 191.0 / 255.0),
            hsv(123.4, 0.237, 0.762),
        ] {
            let (mut tree, picker) = inserted();
            ok(set_color_picker_hsv(&mut tree, picker, start));
            assert_eq!(snapshot(&tree, picker).hsv(), start);
            tree.take_damage();
            let echo = snapshot(&tree, picker).color();
            assert_eq!(
                ok(set_color_picker_color(&mut tree, picker, echo)),
                ColorPickerOutcome::Ignored,
                "{start:?}"
            );
            assert_eq!(snapshot(&tree, picker).hsv(), start, "{start:?}");
            assert_eq!(tree.take_damage(), None, "{start:?}");
        }
        // A genuinely different colour still changes it.
        let (mut tree, picker) = inserted();
        assert!(matches!(
            ok(set_color_picker_color(&mut tree, picker, rgb(1, 2, 3))),
            ColorPickerOutcome::Changed { .. }
        ));
    }

    /// Ownership is decided by the tracked ids (plus the picker's own
    /// part kind and preview signature for strays), never by kind alone:
    /// a caller's own swatch under the picker or the square survives an
    /// ordinary call, and the one under the picker also survives a
    /// structural repair and a stale snapshot.
    #[test]
    fn a_callers_own_swatch_under_the_picker_or_the_square_is_left_alone() {
        let (mut tree, picker) = inserted();
        let scales = test_scales();
        let i = ids(&tree, picker);
        let under_picker = ok(crate::widgets::insert_color_swatch(
            &mut tree,
            picker,
            &scales,
            rgb(1, 2, 3),
        ));
        let under_square = ok(crate::widgets::insert_color_swatch(
            &mut tree,
            i.area,
            &scales,
            rgb(4, 5, 6),
        ));
        ok(set_color_picker_hsv(&mut tree, picker, hsv(30.0, 0.5, 0.5)));
        ok(set_color_picker_color(&mut tree, picker, rgb(9, 99, 199)));
        assert_eq!(
            tree.children(picker),
            Some([i.area, i.hue, i.preview, under_picker].as_slice())
        );
        assert_eq!(
            tree.children(i.area),
            Some([i.saturation, i.value, under_square].as_slice())
        );
        let swatch_color = |tree: &WidgetTree<WidgetKind>| match tree.payload(under_picker) {
            Some(WidgetKind::ColorSwatch(swatch)) => swatch.color,
            other => unreachable!("{other:?}"),
        };
        assert_eq!(swatch_color(&tree), rgb(1, 2, 3));
        // A structural repair replaces the parts but keeps the caller's
        // swatch under the picker.
        ok(tree.remove(i.hue));
        ok(set_color_picker_disabled(&mut tree, picker, false));
        let j = ids(&tree, picker);
        assert_eq!(
            tree.children(picker),
            Some([under_picker, j.area, j.hue, j.preview].as_slice())
        );
        assert_eq!(swatch_color(&tree), rgb(1, 2, 3));
        // A stale snapshot (no tracked ids): the untracked old preview is
        // recognised by its signature and removed — no duplicate — and
        // the caller's swatch still survives.
        let mut stale = snapshot(&tree, picker);
        stale.parts = None;
        if let Some(slot) = tree.payload_mut(picker) {
            *slot = WidgetKind::ColorPicker(stale);
        }
        ok(set_color_picker_disabled(&mut tree, picker, false));
        let k = ids(&tree, picker);
        assert_eq!(
            tree.children(picker),
            Some([under_picker, k.area, k.hue, k.preview].as_slice())
        );
        assert_eq!(swatch_color(&tree), rgb(1, 2, 3));
    }

    // ---- Pointer -------------------------------------------------------

    fn area_rect(tree: &WidgetTree<WidgetKind>, picker: WidgetId) -> Rect {
        let Some(b) = tree.bounds(ids(tree, picker).area) else {
            unreachable!("live");
        };
        b
    }

    #[test]
    fn the_square_maps_corners_and_clamps_outside_points() {
        let (mut tree, picker) = inserted();
        let a = area_rect(&tree, picker);
        let (x, y, w, h) = (a.x as f32, a.y as f32, a.width as f32, a.height as f32);
        ok(set_color_picker_hsv(
            &mut tree,
            picker,
            hsv(210.0, 0.5, 0.5),
        ));
        for (point, s, v) in [
            ((x, y), 0.0, 1.0),
            ((x + w, y), 1.0, 1.0),
            ((x, y + h), 0.0, 0.0),
            ((x + w, y + h), 1.0, 0.0),
            ((x + w / 4.0, y + h / 4.0), 0.25, 0.75),
            ((x - 50.0, y - 50.0), 0.0, 1.0),
            ((x + w + 50.0, y + h + 50.0), 1.0, 0.0),
        ] {
            ok(set_saturation_value_from_point(&mut tree, picker, point));
            assert_eq!(snapshot(&tree, picker).hsv(), hsv(210.0, s, v), "{point:?}");
        }
        let strip = {
            let Some(b) = tree.bounds(ids(&tree, picker).hue) else {
                unreachable!("live");
            };
            b
        };
        let (sx, sw) = (strip.x as f32, strip.width as f32);
        for (px, hue) in [
            (sx, 0.0),
            (sx + sw, 360.0),
            (sx + sw / 4.0, 90.0),
            (sx - 9.0, 0.0),
        ] {
            ok(set_hue_from_point(&mut tree, picker, (px, strip.y as f32)));
            assert_eq!(snapshot(&tree, picker).hsv().hue, hue, "{px}");
        }
        assert_eq!(snapshot(&tree, picker).hsv().saturation, 1.0);
    }

    #[test]
    fn a_zero_size_part_or_a_non_finite_point_is_ignored_without_nan() {
        let (mut tree, root) = new_tree(sized_root());
        let picker = ok(insert_color_picker(
            &mut tree,
            root,
            &test_scales(),
            "C",
            INITIAL,
            SIZE,
        ));
        // Never laid out: every bound is the zero rect.
        let before = snapshot(&tree, picker).hsv();
        tree.take_damage();
        for point in [(0.0, 0.0), (5.0, 5.0)] {
            assert_eq!(
                ok(set_saturation_value_from_point(&mut tree, picker, point)),
                ColorPickerOutcome::Ignored
            );
            assert_eq!(
                ok(set_hue_from_point(&mut tree, picker, point)),
                ColorPickerOutcome::Ignored
            );
        }
        tree.compute_layout(320.0, 320.0);
        for point in [(f32::NAN, 5.0), (5.0, f32::INFINITY)] {
            assert_eq!(
                ok(set_saturation_value_from_point(&mut tree, picker, point)),
                ColorPickerOutcome::Ignored
            );
        }
        assert_eq!(snapshot(&tree, picker).hsv(), before);
    }

    /// A picker in a short, clipping container: the point maps through
    /// the square's own unclipped box, not the visible part of it.
    #[test]
    fn pointer_mapping_uses_the_unclipped_bounds() {
        let (mut tree, root) = new_tree(sized_root());
        let scales = test_scales();
        let clip = ok(insert_container(
            &mut tree,
            root,
            Style {
                size: Size {
                    width: length(320.0_f32),
                    height: length(64.0_f32),
                },
                overflow: taffy::Point {
                    x: Overflow::Hidden,
                    y: Overflow::Hidden,
                },
                ..Default::default()
            },
        ));
        let picker = ok(insert_color_picker(
            &mut tree, clip, &scales, "C", INITIAL, SIZE,
        ));
        tree.compute_layout(320.0, 320.0);
        let a = area_rect(&tree, picker);
        assert_eq!(a.height, 128, "the square keeps its own size");
        ok(set_saturation_value_from_point(
            &mut tree,
            picker,
            (a.x as f32, a.y as f32 + 32.0),
        ));
        assert_eq!(snapshot(&tree, picker).hsv().value, 0.75);
    }

    #[test]
    fn part_at_and_part_of_resolve_the_square_and_the_strip_only() {
        let (tree, picker) = inserted();
        let i = ids(&tree, picker);
        for id in [i.area, i.saturation, i.value] {
            assert_eq!(
                color_picker_part_of(&tree, picker, id),
                Some(ColorPickerPart::SaturationValue)
            );
        }
        assert_eq!(
            color_picker_part_of(&tree, picker, i.hue),
            Some(ColorPickerPart::Hue)
        );
        for id in [i.preview, picker, tree.root()] {
            assert_eq!(color_picker_part_of(&tree, picker, id), None);
        }
        assert_eq!(color_picker_part_of(&tree, tree.root(), i.hue), None);
        let centre = |id| {
            let Some(b) = tree.bounds(id) else {
                unreachable!("live");
            };
            (
                b.x as f32 + b.width as f32 / 2.0,
                b.y as f32 + b.height as f32 / 2.0,
            )
        };
        assert_eq!(
            color_picker_part_at(&tree, picker, centre(i.area)),
            Some(ColorPickerPart::SaturationValue)
        );
        assert_eq!(
            color_picker_part_at(&tree, picker, centre(i.hue)),
            Some(ColorPickerPart::Hue)
        );
        assert_eq!(color_picker_part_at(&tree, picker, centre(i.preview)), None);
        assert_eq!(color_picker_part_at(&tree, picker, (319.0, 319.0)), None);
    }

    // ---- Accessibility consumer and focus -------------------------------

    /// Depth-first: every consumer node under `node`, itself included.
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
    fn accesskit_consumer_reads_the_picker_as_documented() {
        let (mut tree, picker) = inserted();
        ok(set_color_picker_hsv(
            &mut tree,
            picker,
            hsv(210.0, 0.25, 0.75),
        ));
        let i = ids(&tree, picker);
        let filter = accesskit_consumer::common_filter;
        let consumer = accesskit_consumer::Tree::new(tree.accessibility_update(i.saturation), true);
        let mut nodes = Vec::new();
        descendants(consumer.state().root(), &mut nodes);
        let by_label = |label: &str| {
            let Some(node) = nodes.iter().find(|n| n.label().as_deref() == Some(label)) else {
                unreachable!("{label} reaches the consumer");
            };
            *node
        };
        assert_eq!(by_label("Foreground").role(), Role::Group);
        assert_eq!(by_label("Saturation and value").role(), Role::Group);
        for (label, value, max) in [
            ("Saturation", 25.0, 100.0),
            ("Value", 75.0, 100.0),
            ("Hue", 210.0, 360.0),
        ] {
            let node = by_label(label);
            assert_eq!(node.role(), Role::Slider);
            assert_eq!(node.numeric_value(), Some(value), "{label}");
            assert_eq!(node.min_numeric_value(), Some(0.0));
            assert_eq!(node.max_numeric_value(), Some(max));
            assert_eq!(node.numeric_value_step(), Some(1.0));
            assert_eq!(node.numeric_value_jump(), Some(10.0));
            assert!(node.supports_increment(&filter));
            assert!(node.supports_decrement(&filter));
        }
        let preview = by_label("Colour");
        assert_eq!(preview.role(), Role::ColorWell);
        assert!(preview.is_read_only());
        let color = hsv(210.0, 0.25, 0.75).to_color();
        assert_eq!(
            preview.value(),
            Some(format!("#{:02X}{:02X}{:02X}", color.r, color.g, color.b))
        );
        assert!(!preview.is_clickable(&filter));
        ok(set_color_picker_disabled(&mut tree, picker, true));
        let consumer = accesskit_consumer::Tree::new(tree.accessibility_update(picker), true);
        let mut nodes = Vec::new();
        descendants(consumer.state().root(), &mut nodes);
        let Some(hue) = nodes.iter().find(|n| n.label().as_deref() == Some("Hue")) else {
            unreachable!("live");
        };
        assert!(hue.is_disabled());
        assert!(!hue.supports_increment(&filter));
    }

    #[test]
    fn the_tab_order_visits_exactly_the_saturation_and_hue_sliders() {
        let (mut tree, picker) = inserted();
        let root = tree.root();
        let button = ok(insert_button(&mut tree, root, &test_scales(), "OK"));
        let i = ids(&tree, picker);
        let mut focus = crate::FocusManager::new();
        let order: Vec<_> = (0..6).filter_map(|_| focus.focus_next(&mut tree)).collect();
        assert_eq!(
            order,
            vec![i.saturation, i.hue, button, i.saturation, i.hue, button]
        );
        ok(set_color_picker_disabled(&mut tree, picker, true));
        let mut focus = crate::FocusManager::new();
        let order: Vec<_> = (0..2).filter_map(|_| focus.focus_next(&mut tree)).collect();
        assert_eq!(order, vec![button, button]);
    }

    // ---- Paint ---------------------------------------------------------

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

    fn ops(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> Vec<PaintOp> {
        ok(paint_widget_ops(
            tree,
            id,
            &dark_theme(),
            &test_scales(),
            1.0,
        ))
    }

    fn gradients(ops: &[PaintOp]) -> Vec<&aurora_vector::ColorMesh> {
        ops.iter()
            .filter_map(|op| match op {
                PaintOp::Gradient(mesh) => Some(mesh),
                PaintOp::Solid(_) => None,
            })
            .collect()
    }

    fn close(a: [f32; 4], b: [f32; 4], tolerance: f32) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tolerance)
    }

    fn with_alpha([r, g, b]: [f32; 3], alpha: f32) -> [f32; 4] {
        [r, g, b, alpha]
    }

    /// The square: one gradient then two marker rings; every vertex is
    /// HSV at its own position (exact at the corners, within `1e-6`
    /// everywhere — the red channel's bilinear cross term is zero, and
    /// the others are within the documented subdivision bound only
    /// *between* vertices, not at them).
    #[test]
    fn the_square_paints_hsv_exactly_at_every_vertex_then_its_marker() {
        let (mut tree, picker) = inserted();
        ok(set_color_picker_hsv(
            &mut tree,
            picker,
            hsv(210.0, 0.25, 0.75),
        ));
        let i = ids(&tree, picker);
        let a = area_rect(&tree, picker);
        let all = ops(&tree, i.area);
        assert_eq!(all.len(), 3);
        assert!(matches!(all.first(), Some(PaintOp::Gradient(_))));
        assert!(all.iter().skip(1).all(|op| matches!(op, PaintOp::Solid(_))));
        let meshes = gradients(&all);
        let [mesh] = meshes.as_slice() else {
            unreachable!("one gradient");
        };
        let (x, y, w, h) = (a.x as f32, a.y as f32, a.width as f32, a.height as f32);
        for vertex in &mesh.vertices {
            let s = (vertex.position.x - x) / w;
            let v = 1.0 - (vertex.position.y - y) / h;
            let want = with_alpha(hsv(210.0, s, v).to_srgb_f32(), 1.0);
            assert!(close(vertex.color, want, 1e-6), "{vertex:?} vs {want:?}");
        }
        let corner = |px: f32, py: f32| {
            mesh.vertices
                .iter()
                .find(|vtx| vtx.position.x == px && vtx.position.y == py)
                .map(|vtx| vtx.color)
        };
        assert_eq!(corner(x, y), Some([1.0, 1.0, 1.0, 1.0]));
        assert_eq!(
            corner(x + w, y),
            Some(with_alpha(hsv(210.0, 1.0, 1.0).to_srgb_f32(), 1.0))
        );
        assert_eq!(corner(x, y + h), Some([0.0, 0.0, 0.0, 1.0]));
        assert_eq!(corner(x + w, y + h), Some([0.0, 0.0, 0.0, 1.0]));
        // The marker's ring is centred on (s, 1 - v) of the square.
        let Some(PaintOp::Solid((ring, _))) = all.get(1) else {
            unreachable!("a ring");
        };
        let (min_x, max_x, min_y, max_y) = ring.vertices.iter().fold(
            (f32::MAX, f32::MIN, f32::MAX, f32::MIN),
            |(a, b, c, d), p| (a.min(p.x), b.max(p.x), c.min(p.y), d.max(p.y)),
        );
        let (cx, cy) = (f32::midpoint(min_x, max_x), f32::midpoint(min_y, max_y));
        assert!((cx - (x + 0.25 * w)).abs() < 0.01, "{cx}");
        assert!((cy - (y + 0.25 * h)).abs() < 0.01, "{cy}");
    }

    #[test]
    fn the_strip_paints_seven_exact_stops_that_are_piecewise_hsv() {
        let (mut tree, picker) = inserted();
        ok(set_color_picker_hsv(
            &mut tree,
            picker,
            hsv(90.0, 0.25, 0.75),
        ));
        let i = ids(&tree, picker);
        let all = ops(&tree, i.hue);
        assert_eq!(all.len(), 3);
        let meshes = gradients(&all);
        let [mesh] = meshes.as_slice() else {
            unreachable!("one gradient");
        };
        assert_eq!(mesh.vertices.len(), 14, "seven stops, two vertices each");
        for (stop, pair) in (0_u8..).zip(mesh.vertices.chunks(2)) {
            let want = with_alpha(hsv(f32::from(stop) * 60.0, 1.0, 1.0).to_srgb_f32(), 1.0);
            assert!(pair.iter().all(|vtx| vtx.color == want), "stop {stop}");
        }
        // Linear between stops is exactly HSV at full saturation/value.
        for step in 0..=720_u16 {
            let hue = f32::from(step) / 2.0;
            let segment = (hue / 60.0).floor().min(5.0);
            let t = hue / 60.0 - segment;
            let left = hsv(segment * 60.0, 1.0, 1.0).to_srgb_f32();
            let right = hsv((segment + 1.0) * 60.0, 1.0, 1.0).to_srgb_f32();
            let lerped: Vec<f32> = left
                .iter()
                .zip(right)
                .map(|(l, r)| l * (1.0 - t) + r * t)
                .collect();
            let want = hsv(hue, 1.0, 1.0).to_srgb_f32();
            assert!(
                lerped.iter().zip(want).all(|(a, b)| (a - b).abs() <= 1e-6),
                "{hue}"
            );
        }
    }

    #[test]
    fn a_disabled_picker_dims_every_gradient_vertex_and_marker() {
        let (mut tree, picker) = inserted();
        ok(set_color_picker_disabled(&mut tree, picker, true));
        let alpha = dark_theme().state.disabled_opacity;
        assert!(alpha < 1.0);
        let i = ids(&tree, picker);
        for id in [i.area, i.hue] {
            let all = ops(&tree, id);
            assert_eq!(all.len(), 3);
            for op in &all {
                match op {
                    PaintOp::Gradient(mesh) => {
                        assert!(
                            mesh.vertices
                                .iter()
                                .all(|v| v.color.get(3).is_some_and(|a| (a - alpha).abs() <= 1e-6))
                        );
                    }
                    PaintOp::Solid((_, color)) => assert_eq!(color.get(3), Some(&alpha)),
                }
            }
        }
    }

    /// The channel sliders and the root paint nothing; the preview paints
    /// exactly what a standalone swatch of the same colour paints.
    #[test]
    fn the_sliders_paint_nothing_and_the_preview_paints_a_real_swatch() {
        let (mut tree, picker) = inserted();
        let i = ids(&tree, picker);
        for id in [picker, i.saturation, i.value] {
            assert!(ops(&tree, id).is_empty());
        }
        let root = tree.root();
        let color = snapshot(&tree, picker).color();
        let swatch = ok(crate::widgets::insert_color_swatch(
            &mut tree,
            root,
            &test_scales(),
            color,
        ));
        let Some(bounds) = tree.bounds(i.preview) else {
            unreachable!("live");
        };
        ok(tree.set_bounds(swatch, bounds));
        assert_eq!(ops(&tree, i.preview), ops(&tree, swatch));
        assert_eq!(ops(&tree, i.preview).len(), 1);
    }

    /// A picker in a clipping container that hides the bottom of the
    /// square and all of the strip.
    fn clipped(height: f32) -> (WidgetTree<WidgetKind>, WidgetId) {
        let (mut tree, root) = new_tree(sized_root());
        let clip = ok(insert_container(
            &mut tree,
            root,
            Style {
                size: Size {
                    width: length(96.0_f32),
                    height: length(height),
                },
                overflow: taffy::Point {
                    x: Overflow::Hidden,
                    y: Overflow::Hidden,
                },
                ..Default::default()
            },
        ));
        let picker = ok(insert_color_picker(
            &mut tree,
            clip,
            &test_scales(),
            "C",
            INITIAL,
            SIZE,
        ));
        tree.compute_layout(320.0, 320.0);
        ok(set_color_picker_hsv(
            &mut tree,
            picker,
            hsv(210.0, 0.5, 0.5),
        ));
        (tree, picker)
    }

    #[test]
    fn a_clipped_square_draws_only_the_visible_part_with_the_full_gradients_colours() {
        let (tree, picker) = clipped(64.0);
        let i = ids(&tree, picker);
        let a = area_rect(&tree, picker);
        let all = ops(&tree, i.area);
        let meshes = gradients(&all);
        let [mesh] = meshes.as_slice() else {
            unreachable!("one gradient");
        };
        let (x, y, w, h) = (a.x as f32, a.y as f32, a.width as f32, a.height as f32);
        for vertex in &mesh.vertices {
            let p = vertex.position;
            assert!(
                p.x >= x && p.x <= x + 96.0 && p.y >= y && p.y <= y + 64.0,
                "{p:?}"
            );
            let want = with_alpha(
                hsv(210.0, (p.x - x) / w, 1.0 - (p.y - y) / h).to_srgb_f32(),
                1.0,
            );
            assert!(close(vertex.color, want, 1e-6), "{vertex:?} vs {want:?}");
        }
        // The marker at (0.5, 0.5) sits on the clip edge (x = 64 of a
        // 96-wide clip, y = 64 of a 64-tall one): partly clipped, dropped.
        assert_eq!(all.len(), 1, "gradient only: {all:?}");
        // The strip lies wholly below the clip: nothing at all.
        assert!(ops(&tree, i.hue).is_empty());
    }

    #[test]
    fn a_clipped_strip_draws_one_segment_per_visible_sixth_with_exact_colours() {
        let scales = test_scales();
        // Tall enough to show the strip, narrow enough to cut it at 96 px.
        let strip_bottom = SIZE + spacing(scales.spacing.sm) + type_size(scales.typography.size.md);
        let (tree, picker) = clipped(strip_bottom);
        let i = ids(&tree, picker);
        let Some(strip) = tree.bounds(i.hue) else {
            unreachable!("live");
        };
        let all = ops(&tree, i.hue);
        let meshes = gradients(&all);
        // 96 of 128 px is 0.75 of the strip: sixths 0..=4 overlap it.
        assert_eq!(meshes.len(), 5);
        let (x, w) = (strip.x as f32, strip.width as f32);
        for mesh in &meshes {
            for vertex in &mesh.vertices {
                assert!(vertex.position.x <= x + 96.0);
                let hue = (vertex.position.x - x) / w * 360.0;
                let want = with_alpha(hsv(hue, 1.0, 1.0).to_srgb_f32(), 1.0);
                assert!(close(vertex.color, want, 1e-5), "{vertex:?} vs {want:?}");
            }
        }
        // The hue marker at 210 degrees (x = 74.7) is fully visible.
        assert_eq!(all.len(), 7);
        // Visible edges are exactly the strip's left edge and the clip
        // line, and adjacent segments share their seam exactly.
        let edges: Vec<(f32, f32)> = meshes
            .iter()
            .map(|mesh| {
                mesh.vertices
                    .iter()
                    .fold((f32::MAX, f32::MIN), |(lo, hi), v| {
                        (lo.min(v.position.x), hi.max(v.position.x))
                    })
            })
            .collect();
        assert_eq!(edges.first().map(|e| e.0), Some(x));
        assert_eq!(edges.last().map(|e| e.1), Some(x + 96.0));
        for pair in edges.windows(2) {
            if let [(_, right), (left, _)] = pair {
                assert_eq!(right, left);
            }
        }
    }

    /// The bounding box of every solid vertex in `ops` (the markers).
    fn solid_extent(all: &[PaintOp]) -> (f32, f32, f32, f32) {
        all.iter()
            .filter_map(|op| match op {
                PaintOp::Solid((mesh, _)) => Some(mesh),
                PaintOp::Gradient(_) => None,
            })
            .flat_map(|mesh| mesh.vertices.iter())
            .fold(
                (f32::MAX, f32::MAX, f32::MIN, f32::MIN),
                |(a, b, c, d), p| (a.min(p.x), b.min(p.y), c.max(p.x), d.max(p.y)),
            )
    }

    /// How far a stroked marker's tessellated vertices may stray past
    /// the exact outline: the stroke tolerance at scale factor `1`
    /// (measured at ~0.0045 px), far short of the half pixel that would
    /// reach a neighbouring pixel's centre.
    const SLACK: f32 = 0.05;

    fn inside(extent: (f32, f32, f32, f32), part: Rect) -> bool {
        let (x0, y0, x1, y1) = extent;
        let (x, y, w, h) = (
            part.x as f32,
            part.y as f32,
            part.width as f32,
            part.height as f32,
        );
        x0 >= x - SLACK && y0 >= y - SLACK && x1 <= x + w + SLACK && y1 <= y + h + SLACK
    }

    /// At every extreme each marker is drawn whole and wholly inside its
    /// own part (so the part's damage covers it and it never overpaints
    /// a neighbour), flush with the edge it is clamped against.
    #[test]
    fn markers_stay_inside_their_part_at_every_extreme() {
        let (mut tree, picker) = inserted();
        let i = ids(&tree, picker);
        for (s, v) in [(0.0, 0.0), (0.0, 1.0), (1.0, 0.0), (1.0, 1.0), (0.5, 0.5)] {
            ok(set_color_picker_hsv(&mut tree, picker, hsv(210.0, s, v)));
            let a = area_rect(&tree, picker);
            let all = ops(&tree, i.area);
            assert_eq!(all.len(), 3, "s={s} v={v}");
            let extent = solid_extent(&all);
            assert!(inside(extent, a), "s={s} v={v}: {extent:?} vs {a:?}");
            if s == 0.0 {
                assert!((extent.0 - a.x as f32).abs() < SLACK, "flush left");
            }
            if v == 1.0 {
                assert!((extent.1 - a.y as f32).abs() < SLACK, "flush top");
            }
        }
        for hue in [0.0, 360.0, 180.0] {
            ok(set_color_picker_hsv(&mut tree, picker, hsv(hue, 0.5, 0.5)));
            let Some(strip) = tree.bounds(i.hue) else {
                unreachable!("live");
            };
            let all = ops(&tree, i.hue);
            assert_eq!(all.len(), 3, "hue={hue}");
            let extent = solid_extent(&all);
            assert!(inside(extent, strip), "hue={hue}: {extent:?} vs {strip:?}");
        }
    }

    /// A clipping panel exactly flush with the picker keeps every marker
    /// at every extreme — none is dropped as "partly clipped".
    #[test]
    fn a_flush_clipping_panel_keeps_the_markers_at_the_extremes() {
        let scales = test_scales();
        let strip_bottom = SIZE + spacing(scales.spacing.sm) + type_size(scales.typography.size.md);
        let (mut tree, root) = new_tree(sized_root());
        let clip = ok(insert_container(
            &mut tree,
            root,
            Style {
                size: Size {
                    width: length(SIZE),
                    height: length(strip_bottom),
                },
                overflow: taffy::Point {
                    x: Overflow::Hidden,
                    y: Overflow::Hidden,
                },
                ..Default::default()
            },
        ));
        let picker = ok(insert_color_picker(
            &mut tree, clip, &scales, "C", INITIAL, SIZE,
        ));
        tree.compute_layout(320.0, 320.0);
        let i = ids(&tree, picker);
        for start in [
            hsv(0.0, 0.0, 1.0),
            hsv(360.0, 1.0, 0.0),
            hsv(0.0, 1.0, 1.0),
            hsv(360.0, 0.0, 0.0),
        ] {
            ok(set_color_picker_hsv(&mut tree, picker, start));
            assert_eq!(ops(&tree, i.area).len(), 3, "square {start:?}");
            assert_eq!(ops(&tree, i.hue).len(), 3, "strip {start:?}");
        }
    }

    /// A part that cannot hold its marker paints none: a never-laid-out
    /// (zero-size) part paints nothing at all, and the smallest accepted
    /// picker paints its gradients but no markers.
    #[test]
    fn a_part_too_small_for_its_marker_paints_no_marker() {
        let scales = test_scales();
        let (mut tree, root) = new_tree(sized_root());
        let picker = ok(insert_color_picker(
            &mut tree, root, &scales, "C", INITIAL, SIZE,
        ));
        let i = ids(&tree, picker);
        for id in [i.area, i.hue] {
            assert!(ops(&tree, id).is_empty(), "never laid out");
        }
        let (mut tree, root) = new_tree(sized_root());
        let picker = ok(insert_color_picker(
            &mut tree,
            root,
            &scales,
            "C",
            INITIAL,
            super::MIN_SIZE,
        ));
        tree.compute_layout(320.0, 320.0);
        let i = ids(&tree, picker);
        for id in [i.area, i.hue] {
            let all = ops(&tree, id);
            assert_eq!(gradients(&all).len(), 1);
            assert_eq!(all.len(), 1, "no marker: {all:?}");
        }
    }

    #[test]
    fn paint_widget_is_exactly_the_solid_subset_of_paint_widget_ops() {
        let (mut tree, picker) = inserted();
        let root = tree.root();
        let scales = test_scales();
        ok(insert_button(&mut tree, root, &scales, "OK"));
        let theme = dark_theme();
        let mut stack = vec![root];
        let mut gradient_owners = Vec::new();
        while let Some(id) = stack.pop() {
            stack.extend(tree.children(id).unwrap_or_default().iter().copied());
            let all = ok(paint_widget_ops(&tree, id, &theme, &scales, 1.0));
            let solids: Vec<_> = all
                .iter()
                .filter_map(|op| match op {
                    PaintOp::Solid(paint) => Some(paint.clone()),
                    PaintOp::Gradient(_) => None,
                })
                .collect();
            assert_eq!(solids, ok(paint_widget(&tree, id, &theme, &scales, 1.0)));
            if all.iter().any(|op| matches!(op, PaintOp::Gradient(_))) {
                gradient_owners.push(id);
            }
        }
        gradient_owners.sort_unstable_by_key(|id| id.0);
        let i = ids(&tree, picker);
        let mut want = vec![i.area, i.hue];
        want.sort_unstable_by_key(|id| id.0);
        assert_eq!(gradient_owners, want);
    }
}
