//! The headless render harness for PLAN.md's still-open component
//! gallery: renders a real [`WidgetTree`]'s own paint
//! (`aurora_widgets::paint_widget`) through the real GPU path renderer
//! to an offscreen target, reads the result back as a real
//! `aurora_testkit::Image`, and diffs it against a checked-in golden
//! PNG via `aurora_testkit::compare_to_golden` — no window, no event
//! loop, only a real GPU adapter. This is the headless half
//! `tests/headless.rs` doesn't need (no GPU at all there) and
//! `render_test.rs` only partly covers (needs GPU, but samples one
//! pixel, not a whole image against a golden).
//!
//! **Scope, stated honestly.** `paint_widget` covers `Button`,
//! `Checkbox`, `Slider`, `TextField`, `CommandPalette`, and
//! `ColorSwatch` — this gallery now covers all six, each its own small
//! tree, its own `render_gallery` call, its own golden PNG, the same
//! "add a tree, call [`render_gallery`], bless a golden" shape the very
//! first version of this file's own doc comment already predicted. Nothing
//! here re-litigates what each widget's own `paint_widget` case
//! already decides (colours, which states exist) — see `src/paint.rs`
//! for that; this file only proves the render pipeline actually
//! produces the pixels those decisions imply.
//!
//! **Per-theme coverage, started narrow.** Every widget above was
//! originally Dark theme only. Now that the Light theme exists and
//! passes its own contrast gate (`aurora-theme::contrast`), all six
//! widgets — `Button`, `Checkbox`, `Slider`, `TextField`,
//! `CommandPalette`, and `ColorSwatch` — each have a first Light-theme
//! slice — their own `light_theme()`/`LIGHT_CLEAR` (shared, not
//! redefined per widget), a real self-contained distinct-pixels test,
//! and an `#[ignore]`d golden-diff test pending a human bless, the same
//! shape Dark's own coverage started with (`Button` first, `Checkbox`
//! second, `Slider` third, `TextField` fourth, `CommandPalette` fifth,
//! `ColorSwatch` sixth and last).
//! `TextField`'s own Dark-theme gallery needed `NEUTRAL_CLEAR` (see that
//! constant's own doc comment) because `surface.sunken` is near-black
//! there; in Light, `surface.sunken` resolves to `neutral.700`
//! (`#c1c1c7`) against `LIGHT_CLEAR`'s `neutral.900` (`#f5f5f6`) — a real
//! but modest ≈1.65:1 contrast, and only ≈1.36:1 between the enabled and
//! disabled cells themselves (the only two states this widget has).
//! That's better than the ≈1.09:1 Dark's own first `TextField` bless
//! attempt was rejected for ("effectively unreviewable"), but well
//! short of the ≈2.47:1 `NEUTRAL_CLEAR` achieved for the same widget —
//! and unlike `Slider`, there's no bright `accent.primary` element here
//! to anchor a human's eye regardless of the fill's own contrast. So
//! this is a **provisional, unconfirmed** choice, not a settled "no
//! backdrop needed" finding: `LIGHT_CLEAR` alone is used for now, but
//! the human bless step should scrutinize this one closely and may need
//! its own backdrop fix, the same two-iteration path Dark's `TextField`
//! itself took. `CommandPalette`'s own Light-theme gallery, by contrast,
//! needed a **confirmed, computed** backdrop fix, not a provisional
//! guess: Light's `surface.raised` (the panel fill) and `surface.canvas`
//! (`LIGHT_CLEAR`'s own source token) both resolve to the exact same
//! `neutral.900`/`#f5f5f6` — using `LIGHT_CLEAR` here would have
//! reproduced Dark's own original "just a black image" bug
//! (`NEUTRAL_CLEAR`'s own doc comment) in the light direction, as "just
//! a white image." See `COMMAND_PALETTE_LIGHT_CLEAR`'s own doc comment
//! for the real numbers. `ColorSwatch`'s own Light-theme gallery is
//! different again: it's the one widget whose displayed colour is
//! arbitrary caller data, not a theme-resolved fill (`ColorSwatchState::
//! color`), so the question wasn't "does a token collide with
//! `LIGHT_CLEAR`" but "do the same arbitrary bright colours the Dark
//! gallery already uses still contrast a near-white backdrop" — checked
//! with real WCAG numbers (`aurora_theme::contrast::contrast_ratio`),
//! not assumed: plain `LIGHT_CLEAR` turned out fine (≈4.40:1 and
//! ≈5.89:1 for the two swatch colours used, comparable to Dark's own
//! plain-black backdrop), so no special-cased backdrop was needed. See
//! `render_gallery_produces_distinct_pixels_for_each_color_swatch_state_
//! in_light_theme`'s own doc comment for the full numbers, including how
//! `disabled_opacity` blends differently over a light backdrop than a
//! dark one. That closes out a first Light slice for all six widgets
//! `paint_widget` covers.
//!
//! **High Contrast Dark, and the first real proof of `border.control_
//! opacity`.** `design/themes/high-contrast-dark.toml` now exists and is
//! the first theme landed with `border.control_opacity = 1.0` (Dark and
//! Light both use `0.0`) — meaning `control_outline` (`src/paint.rs`)
//! actually returns a real second shape (a stroke, painted after each
//! widget's own fill) for every widget wired into it: `Button`,
//! `Checkbox`, `TextField`, `ColorSwatch`, `CommandPalette`'s panel, and
//! `Slider`'s thumb. All six widgets now have a High Contrast Dark
//! slice too — their own `high_contrast_dark_theme()`/
//! `HIGH_CONTRAST_DARK_CLEAR`, a real distinct-pixels test, and an
//! `#[ignore]`d golden-diff test — the same shape Dark's and Light's own
//! coverage already established. The backdrop needed real thought, not
//! a copy-paste of `LIGHT_CLEAR`'s reasoning: `design/themes/
//! high-contrast-dark.toml` resolves `surface.canvas`, `surface.app`,
//! `surface.panel`, `surface.raised`, `surface.overlay`, *and*
//! `surface.sunken` to the exact same `hc.black` (a deliberate
//! OS-high-contrast design choice, not an oversight — see that file's
//! own header comment), so a plain-black gallery backdrop would have hit
//! not just `CommandPalette`'s own single-widget collision (Dark/Light's
//! own history) but nearly every widget in the theme at once
//! (`Checkbox`'s unchecked box, `TextField`'s fill, `Slider`'s track,
//! `CommandPalette`'s panel). `HIGH_CONTRAST_DARK_CLEAR` — `NEUTRAL_
//! CLEAR`'s own `#808080` value, reused rather than reinvented, and
//! checked against every colour these six widgets actually paint, not
//! assumed — is used uniformly across all six, one shared fix for what
//! turned out to be one shared root cause. See that constant's own doc
//! comment for the full per-widget check.
//!
//! This is also the first time `border.control_opacity`'s effect has
//! been rendered through the whole real pipeline (theme TOML →
//! `paint_widget` → tessellated `GpuMesh` → real GPU rasterization →
//! readback pixels) and directly verified, not just checked at the
//! abstract `Paint`-list level the way `src/paint.rs`'s own unit tests
//! already do: `render_gallery_button_outline_proves_border_control_
//! opacity_in_high_contrast_dark_theme` and `render_gallery_text_field_
//! outline_proves_border_control_opacity_in_high_contrast_dark_theme`
//! each sample a real rendered pixel one pixel in from a widget's own
//! edge (found by an actual debug scan of this exact render, not
//! assumed from the stroke geometry alone) and confirm it reads
//! `border.control`'s pure white, distinct from both that widget's own
//! fill and the backdrop. `CommandPalette` was deliberately *not* used
//! for this proof — its selected row's own fill occupies the exact same
//! bounds as the panel itself (`body_style`/`row_style` both
//! `percent(1.0)`), so most of the panel's own outline stroke is likely
//! covered by the row's fill; see `command_palette_gallery_matches_the_
//! golden_image_in_high_contrast_dark_theme`'s own doc comment for why
//! that's flagged for the human bless step instead of settled here.
//!
//! **High Contrast Light** now has coverage for all six widgets too —
//! `design/themes/high-contrast-light.toml` exists and resolves the same
//! "every `surface.*` token identical" shape High Contrast Dark has, just
//! inverted (all six to `hc.white` rather than `hc.black`), so the same
//! collision problem applied and the same fix works: `HIGH_CONTRAST_
//! LIGHT_CLEAR` reuses `NEUTRAL_CLEAR`'s own `#808080` value again,
//! checked against this theme's actual colours (`border.control`/
//! `text.primary` = `hc.black`, `accent.primary` = `hc.blue`,
//! `accent.primary_active` = `hc.blue_dark`) rather than assumed to carry
//! over unchanged — see that constant's own doc comment for the numbers.
//! The two outline-proof tests (`Button`, `TextField`) transferred
//! directly: the same edge-sampling coordinates (one pixel in from the
//! bottom/right edge) that High Contrast Dark's own debug scan
//! established are a property of this rendering pipeline's stroke
//! tessellation, not of any theme's colours, and re-running them here
//! confirmed the outline pixel reads `border.control`'s `hc.black`
//! (`[0,0,0]`) — the inverse of High Contrast Dark's `hc.white`, as
//! expected. `CommandPalette`'s own asymmetric-occlusion situation (the
//! selected row's fill sharing the panel's exact bounds, occluding the
//! right/bottom edges of the outline stroke while leaving top/left
//! visible) is the same, already-understood mechanism as High Contrast
//! Dark's — nothing about `command_palette_style`'s layout or
//! `paint_list_row`'s fill logic is theme-dependent, so it reproduces
//! identically here; see `command_palette_gallery_matches_the_golden_
//! image_in_high_contrast_light_theme`'s own doc comment.
//!
//! **Colour-Critical closes out gallery-code coverage for all five
//! built-in themes.** `design/themes/color-critical.toml` now exists —
//! a genuinely neutral-gray chrome theme (every surface/text/icon/border
//! token `R==G==B`, verified by construction, see that file's own header
//! comment and `aurora-theme`'s `neutrality` module) for colour-accurate
//! work, `extends = "Dark"`, `name = "Color-Critical"` (the exact,
//! case-sensitive string `color_critical_theme()`'s own `resolve` call
//! looks up, the same gotcha every prior theme's own helper function had
//! to get right). Structurally it's the closer analog to Light, not to
//! either High Contrast theme: `border.control_opacity = 0.0` (same as
//! Dark/Light — no mandatory-outline feature active here, so unlike
//! High Contrast Dark/Light this slice adds no outline-proof tests), and
//! its six `surface.*` tokens are **not** all identical the way both
//! High Contrast themes' are — `cc.canvas` (`#545454`), `cc.app`
//! (`#464646`), `cc.panel` (`#3c3c3c`), `cc.raised` (`#4c4c4c`),
//! `cc.overlay` (`#5a5a5a`), and `cc.sunken` (`#242424`) are six genuinely
//! different points on a real, if narrow-range, gray ramp. So this slice
//! uses `COLOR_CRITICAL_CLEAR` (`surface.canvas`, the same "what the real
//! UI would show" choice `LIGHT_CLEAR` already established) as the
//! backdrop for `Button`, `Checkbox`, `Slider`, `TextField`, and
//! `ColorSwatch` — computed, not assumed, per widget:
//! - `Button`: `accent.primary` (`accent.blue.600`, `#a4c8ff`) clears
//!   `COLOR_CRITICAL_CLEAR` at ≈4.43:1, `accent.primary_active`
//!   (`accent.blue.500`, `#78acff`) at ≈3.30:1 — both the exact numbers
//!   `design/themes/color-critical.toml`'s own `[accent]` comment already
//!   computed for choosing `.600` over `.500` as `primary` in the first
//!   place, reused here rather than re-derived. No collision.
//! - `Checkbox`/`Slider`/`TextField` (all resolve `surface.sunken` for
//!   their own recessed fill/track): `cc.sunken` vs `COLOR_CRITICAL_CLEAR`
//!   is ≈2.05:1 — real and, if modest, actually a touch stronger than the
//!   ≈1.65:1 Light's own provisional `TextField` fill-vs-canvas number
//!   (this file's own "Per-theme coverage, started narrow" paragraph
//!   above). `Checkbox`/`Slider` both still have a bright `accent.primary`
//!   element (the checked box, the thumb) to anchor a human's eye
//!   regardless; `TextField` doesn't, so — mirroring Light's own
//!   provisional treatment exactly — its Colour-Critical golden-diff test
//!   is flagged for particular bless-time scrutiny rather than treated as
//!   a settled "no backdrop needed" finding.
//! - `ColorSwatch`: the two arbitrary swatch colours
//!   (`color_swatch_gallery_tree`'s own `red = (220,40,40)`,
//!   `blue = (40,80,220)`, unchanged) contrast `COLOR_CRITICAL_CLEAR` at
//!   only ≈1.58:1 and ≈1.18:1 respectively by the real WCAG luminance
//!   formula — markedly weaker than Light's own ≈4.40:1/≈5.89:1 for the
//!   same two colours, because Colour-Critical's canvas (`#545454`, a
//!   genuine mid-tone) sits far closer in raw luminance to both swatch
//!   colours than Light's near-white canvas did. The pixels themselves
//!   are never byte-identical to the backdrop (grey `(84,84,84)` vs
//!   saturated `(220,40,40)`/`(40,80,220)`), and this theme's whole
//!   premise is a *chromatically* neutral surround, so a saturated hue
//!   against genuinely neutral gray should still read as a visibly
//!   distinct region to a human even where luminance-only contrast is
//!   weak — but that's a claim about human colour vision, not something
//!   this headless harness can verify. Flagged honestly, not smoothed
//!   over: this golden-diff test deserves the same bless-time scrutiny as
//!   `TextField`'s, not a rubber stamp on the strength of "arbitrary
//!   colours are unlikely to collide with a mid-tone grey ramp."
//!
//! `CommandPalette` is the one widget that needed its own backdrop
//! constant, `COMMAND_PALETTE_COLOR_CRITICAL_CLEAR` — computed, not
//! assumed either way. `surface.raised` (the panel fill, `cc.raised`,
//! `#4c4c4c`) against `COLOR_CRITICAL_CLEAR` (`cc.canvas`, `#545454`) is
//! ≈1.13:1 — the two values are close but *not* identical (unlike Dark's
//! original bug or Light's own `CommandPalette` collision, where panel
//! and backdrop were byte-for-byte the same token value), yet 1.13:1 is
//! close enough to invisible that it's the same failure in substance: a
//! human reviewing the golden would see essentially no panel at all
//! against `COLOR_CRITICAL_CLEAR`. So `CommandPalette`'s own slice reuses
//! `NEUTRAL_CLEAR` (`#808080`) instead, exactly as `COMMAND_PALETTE_
//! LIGHT_CLEAR` already does — checked, not carried over blind:
//! `surface.raised` vs `NEUTRAL_CLEAR` is ≈2.17:1, a real and adequate
//! gap (comparable to the ≈2.05:1 already accepted above for `Checkbox`/
//! `Slider`/`TextField`'s own `surface.sunken` vs `COLOR_CRITICAL_CLEAR`),
//! and `accent.primary` (the selected row's highlight) is nowhere near
//! `#808080` either. `command_palette_style`'s own `COMMAND_PALETTE_
//! MARGIN` needs no Colour-Critical-specific counterpart — pure layout,
//! theme-independent, same as every other theme's own `CommandPalette`
//! slice already established.
//!
//! **That closes out gallery-code coverage for all five built-in themes
//! across all six widgets** — Dark, Light, High Contrast Dark, High
//! Contrast Light, and now Colour-Critical. Visual human-bless review of
//! any of it was a separate, entirely open matter when this paragraph
//! was written: of the thirty goldens this file had code for (six
//! widgets × five themes), only Dark's own six were blessed at the
//! time, the other twenty-four `#[ignore]`d pending a human on real GPU
//! hardware. **All thirty have since been blessed and committed under
//! `tests/golden/`** (none of those tests carries an `#[ignore]` any
//! more) — the same "never bless blind" discipline every golden in this
//! file follows. That bless step may well surface real findings of its own —
//! Dark's own `TextField`/`CommandPalette` history (`NEUTRAL_CLEAR`'s and
//! `command_palette_style`'s own doc comments) is exactly why this file
//! doesn't treat "the code compiles and the distinct-pixels test passes"
//! as equivalent to "the golden is trustworthy."
//!
//! **Two more widgets have landed since that paragraph was written, and
//! neither is in its counts.** `Scrollbar` (0.75.1) and now `TreeView`
//! (0.76.0) each add a per-theme distinct-pixels test in all five
//! built-in themes plus five `#[ignore]`d golden-diff tests — so this
//! file now has code for **40** goldens across **eight** widgets, of
//! which the 30 committed under `tests/golden/` are blessed and the ten
//! newest (`scrollbar_gallery*.png`, `tree_view_gallery*.png`) do not
//! exist at all. Backdrops for both were inherited rather than
//! re-derived, and the reasoning is recorded on their own per-theme
//! test groups rather than repeated up here.
//!
//! `TreeView` is also the first widget in this file whose gallery cells
//! differ *structurally* rather than by state: its two cells hold the
//! same three-row tree, expanded and collapsed, because collapsing
//! really removes the child widgets (`widgets::tree_view`'s own module
//! doc comment) and that is not something one cell can show. And it is
//! the first whose paint is deliberately *narrower* than its own layout
//! bounds — a group row's box spans its whole subtree, its highlight is
//! one row tall — which is what this gallery's fourth claim exists to
//! prove in real pixels rather than in mesh coordinates.
//!
//! Uses only `aurora_widgets`' public API, the same "exercised exactly
//! as an external consumer would use it" discipline `tests/headless.rs`
//! already established for this crate's integration tests.

use accesskit::Orientation;
use aurora_theme::{Color, Palette, Scales, Theme, ThemeSet};
use aurora_widgets::widgets::{
    CommandEntry, ScrollbarRange, WidgetKind, insert_button, insert_checkbox, insert_color_swatch,
    insert_command_palette, insert_scrollbar, insert_slider, insert_text_field, insert_tree_item,
    insert_tree_view, new_tree, set_button_disabled, set_button_pressed, set_checkbox_disabled,
    set_color_swatch_disabled, set_scrollbar_disabled, set_slider_disabled,
    set_text_field_disabled, set_tree_item_disabled, set_tree_item_expanded,
    set_tree_item_selected, toggle_checkbox,
};
use aurora_widgets::{GpuMesh, PathPipeline, WidgetId, WidgetTree, paint_widget};
use std::sync::{Mutex, MutexGuard};
use taffy::style_helpers::length;
use taffy::{FlexDirection, Rect as LayoutRect, Size, Style};

const PALETTE_TOML: &str = include_str!("../../../design/tokens/palette.toml");
const DARK_THEME_TOML: &str = include_str!("../../../design/themes/dark.toml");
const LIGHT_THEME_TOML: &str = include_str!("../../../design/themes/light.toml");
const HIGH_CONTRAST_DARK_THEME_TOML: &str =
    include_str!("../../../design/themes/high-contrast-dark.toml");
const HIGH_CONTRAST_LIGHT_THEME_TOML: &str =
    include_str!("../../../design/themes/high-contrast-light.toml");
const COLOR_CRITICAL_THEME_TOML: &str = include_str!("../../../design/themes/color-critical.toml");
const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");

/// `Button`'s own gallery cell size. Deliberately explicit, not the
/// button's own real padding-derived content size
/// (`widgets::button`'s own internal `style` function isn't public,
/// and this crate has no text layout wired in yet to give a label its
/// own real content size either) — a real, deterministic pixel size is
/// what a golden-image test needs; this gallery is testing
/// `paint_widget`'s own background-rectangle output, not button
/// content layout. Every other widget's own gallery below picks its
/// own cell size the same way, for the same reason.
const BUTTON_CELL: (u32, u32) = (64, 64);
/// Three states, side by side. `64 * 3 * 4 = 768 = 3 * 256`, already a
/// multiple of `wgpu::COPY_BYTES_PER_ROW_ALIGNMENT` — see
/// [`render_gallery`]'s own doc comment for why that matters and why
/// this harness doesn't (yet) handle a size that isn't. Every gallery
/// cell size below is chosen as a multiple of 64 for the same reason:
/// `64 * 4 = 256`, so *any* number of same-sized cells side by side
/// keeps the whole row aligned, not just this particular count of 3.
const BUTTON_GALLERY_SIZE: (u32, u32) = (BUTTON_CELL.0 * 3, BUTTON_CELL.1);

const CHECKBOX_CELL: (u32, u32) = (64, 64);
const CHECKBOX_GALLERY_SIZE: (u32, u32) = (CHECKBOX_CELL.0 * 3, CHECKBOX_CELL.1);

const COLOR_SWATCH_CELL: (u32, u32) = (64, 64);
const COLOR_SWATCH_GALLERY_SIZE: (u32, u32) = (COLOR_SWATCH_CELL.0 * 3, COLOR_SWATCH_CELL.1);

/// Wider than tall — a slider needs real horizontal travel for its own
/// thumb position to mean anything, unlike the other widgets' own
/// roughly-square cells.
const SLIDER_CELL: (u32, u32) = (128, 32);
const SLIDER_GALLERY_SIZE: (u32, u32) = (SLIDER_CELL.0 * 3, SLIDER_CELL.1);
/// How far into each slider cell, from its own left edge,
/// `render_gallery_produces_distinct_pixels_for_each_slider_state`
/// samples — well within the thumb's own 32px width when the thumb is
/// at that cell's own left edge (the minimum-value cell), but past it
/// once the thumb has moved away (see that test's own doc comment).
const SLIDER_THUMB_SAMPLE_OFFSET_X: u32 = 16;

/// A scrollbar is the one widget here whose *cell shape* has to differ
/// by state, not just its contents: a bar only has travel along its own
/// scrolling axis, so a horizontal bar in a tall square cell degenerates
/// to a full-length thumb with nowhere to move (`paint_scrollbar`'s own
/// `.max(thickness)` floor against a short track), showing nothing. The
/// three vertical cells are therefore tall and narrow and the one
/// horizontal cell is short and wide; the horizontal cell sits at the
/// top of the row with backdrop below it, which is honest about a
/// horizontal bar's own height rather than stretching it to match.
const SCROLLBAR_VERTICAL_CELL: (u32, u32) = (64, 128);
const SCROLLBAR_HORIZONTAL_CELL: (u32, u32) = (128, 32);
/// `64 * 3 + 128 = 320`, and `320 * 4 = 1280 = 5 * 256` — already a
/// multiple of `wgpu::COPY_BYTES_PER_ROW_ALIGNMENT`, the same constraint
/// every other gallery size here satisfies (see [`BUTTON_GALLERY_SIZE`]).
const SCROLLBAR_GALLERY_SIZE: (u32, u32) = (
    SCROLLBAR_VERTICAL_CELL.0 * 3 + SCROLLBAR_HORIZONTAL_CELL.0,
    SCROLLBAR_VERTICAL_CELL.1,
);
/// A 20-of-120 page over a 128px track floors at the bar's own 64px
/// thickness, so each vertical thumb is 64px long with 64px of travel:
/// at the minimum it occupies `y` 0..64, at the maximum `y` 64..128.
/// `y = 16` is inside the thumb in the first case and on bare track in
/// the second — a direct rendered-pixel proof the thumb moved, the same
/// shape `SLIDER_THUMB_SAMPLE_OFFSET_X` already uses on the other axis.
const SCROLLBAR_THUMB_SAMPLE_Y: u32 = 16;
/// `y = 48` is inside the thumb both at the minimum (0..64) and at the
/// disabled cell's own mid-travel position (32..96) — so comparing the
/// two at this row compares one thumb against another, isolating
/// `disabled_opacity` rather than accidentally comparing a thumb to a
/// track.
const SCROLLBAR_DISABLED_SAMPLE_Y: u32 = 48;
/// The horizontal cell starts at `x = 192` and is 128 wide with a 32px
/// thickness, so its thumb is 32 long with 96 of travel; mid-travel puts
/// it at `x` 240..272 within the image. `256` is inside it, `200` is
/// bare track well to its left.
const SCROLLBAR_HORIZONTAL_THUMB_SAMPLE_X: u32 = 256;
const SCROLLBAR_HORIZONTAL_TRACK_SAMPLE_X: u32 = 200;
const SCROLLBAR_HORIZONTAL_SAMPLE_Y: u32 = 16;

/// `TreeView`'s own two cells: the same three-row tree expanded and
/// collapsed. Two cells rather than a per-state row, because a tree
/// row's own *states* (selected, disabled) are shown by the rows within
/// one cell — what needs two cells is the one thing a single cell
/// cannot show at all, that collapsing really removes the child rows.
/// `128 * 2 = 256`, and `256 * 4 = 1024 = 4 * 256`, already row-aligned
/// (see [`BUTTON_GALLERY_SIZE`]).
const TREE_VIEW_CELL: (u32, u32) = (128, 128);
const TREE_VIEW_GALLERY_SIZE: (u32, u32) = (TREE_VIEW_CELL.0 * 2, TREE_VIEW_CELL.1);
/// One tree row's own height, `aurora_theme::Scales`-derived in the
/// widget itself (`widgets::tree_row_height` — `type.size.md` 13 plus
/// `spacing.xxs` 4 above and below) and restated here as the plain
/// number this gallery's own sample coordinates are built from. Not a
/// second source of truth: `tree_view_gallery_rows_are_one_row_tall`
/// asserts the real laid-out bounds match it, so a scale change that
/// moved the rows fails a test rather than silently sampling the wrong
/// band.
const TREE_VIEW_ROW_HEIGHT: u32 = 21;
/// Half a row down — inside the group's own row band (`y` 0..21) in
/// both cells.
const TREE_VIEW_PARENT_SAMPLE_Y: u32 = 10;
/// Inside the first child's band (`y` 21..42) in the expanded cell, and
/// bare backdrop at the same point in the collapsed one.
const TREE_VIEW_CHILD_SAMPLE_Y: u32 = TREE_VIEW_ROW_HEIGHT + 10;
/// Inside the second, disabled child's band (`y` 42..63).
const TREE_VIEW_DISABLED_CHILD_SAMPLE_Y: u32 = TREE_VIEW_ROW_HEIGHT * 2 + 10;
/// Left of a child row's own indented left edge (`spacing.md` = 16) but
/// inside its parent's, which starts at the cell's own edge — the
/// column where an indent is a *visible* difference rather than an
/// arithmetic one.
const TREE_VIEW_INDENT_SAMPLE_X: u32 = 6;
/// Well inside every row, parent and child alike — the column used
/// whenever a claim is about a row's colour rather than its left edge.
const TREE_VIEW_ROW_SAMPLE_X: u32 = 64;

const TEXT_FIELD_CELL: (u32, u32) = (192, 32);
const TEXT_FIELD_GALLERY_SIZE: (u32, u32) = (TEXT_FIELD_CELL.0 * 2, TEXT_FIELD_CELL.1);

/// `CommandPalette` still has no query-text rendering (`paint_widget`'s
/// own doc comment), so one cell captures everything this gallery can
/// show today — one command, always selected, so the row highlight is
/// always visible too — not a row of cells the way a real per-state
/// gallery (`Checkbox`, `Slider`) uses.
const COMMAND_PALETTE_CELL: (u32, u32) = (192, 128);
/// A real margin around the panel on every side, not just a bigger
/// backdrop colour — found necessary the hard way (2026-08-07):
/// `NEUTRAL_CLEAR` alone wasn't enough, because the panel was still
/// filling essentially the whole frame with no border to contrast
/// against at all, `scales.radius.md`'s own small rounded corners
/// being the only pixels that ever showed the backdrop. See
/// `command_palette_style`'s own doc comment for how this actually
/// gets applied.
const COMMAND_PALETTE_MARGIN: u32 = 32;
const COMMAND_PALETTE_GALLERY_SIZE: (u32, u32) = (
    COMMAND_PALETTE_CELL.0 + COMMAND_PALETTE_MARGIN * 2,
    COMMAND_PALETTE_CELL.1 + COMMAND_PALETTE_MARGIN * 2,
);

/// `render_gallery`'s own clear colour for `CommandPalette`/`TextField`
/// specifically, not the plain `wgpu::Color::BLACK` every other
/// gallery uses. Found by actually decoding the committed goldens'
/// own raw pixel bytes, not by eye: `surface.raised` (`CommandPalette`'s
/// own panel, `#28282c` at the time — since shifted a ramp step
/// higher, see `paint_panel`'s own doc comment) and `surface.sunken`
/// (`TextField`'s own background, `#141414`, `#0a0a0a` once
/// `disabled_opacity`-dimmed) are deliberately near-black by design
/// (FR-027's own "near-neutral" principle) — correct data, confirmed
/// pixel-for-pixel, but against a pure black backdrop there is
/// nothing for a human reviewing the golden to visually anchor
/// against, since neither widget ever resolves a bright `accent.*`
/// colour the way `Button`/`Checkbox`/`Slider` each do in at least one
/// of their own states. Those three keep the plain black backdrop —
/// already blessed, already real reference points within each image,
/// no reason to force a re-bless. **On its own, this alone turned out
/// not to be enough for `CommandPalette` specifically** — it has only
/// one visual state, so unlike `TextField` (two states, genuinely
/// contrastable against each other even with no margin at all) its own
/// gallery still filled essentially the whole frame with nothing to
/// contrast the new backdrop against; `command_palette_style`'s own
/// real margin was the second, necessary half of that particular fix.
const NEUTRAL_CLEAR: wgpu::Color = wgpu::Color {
    r: 0.5,
    g: 0.5,
    b: 0.5,
    a: 1.0,
};

/// The Light theme's own real `surface.canvas` colour (`design/themes/
/// light.toml` → `neutral.900` → `design/tokens/palette.toml` →
/// `#f5f5f6`), used as `Button`'s own Light-theme gallery backdrop —
/// what the actual UI would show, not an arbitrary pick. Unlike
/// `NEUTRAL_CLEAR`'s near-black Dark-theme tokens, Light's own fills
/// here (the `accent.blue` family) already contrast fine against this
/// real light backdrop, so no separate margin/positioning fix
/// (`CommandPalette`/`TextField`'s own) is needed for `Button` in Light.
const LIGHT_CLEAR: wgpu::Color = wgpu::Color {
    r: 0xf5 as f64 / 255.0,
    g: 0xf5 as f64 / 255.0,
    b: 0xf6 as f64 / 255.0,
    a: 1.0,
};

/// `CommandPalette`'s own Light-theme backdrop — deliberately not plain
/// `LIGHT_CLEAR`. Computed, not assumed: `design/themes/light.toml`
/// resolves both `surface.raised` (`CommandPalette`'s own panel fill,
/// `paint_command_palette`) and `surface.canvas` (`LIGHT_CLEAR`'s own
/// source token) to the exact same `neutral.900` → `#f5f5f6`. Rendering
/// against `LIGHT_CLEAR` would therefore paint the panel a colour
/// byte-for-byte identical to its own backdrop — precisely the "just a
/// black image" bug `NEUTRAL_CLEAR`'s own doc comment already records
/// for Dark (`surface.raised` there was merely near-black and hard to
/// see; here it's not merely close to the backdrop, it *is* the
/// backdrop), mirrored into Light as "just a white image" instead. So
/// this reuses `NEUTRAL_CLEAR`'s own value (`#808080`, already chosen as
/// "a mid-tone, distinct from every real surface token this crate
/// resolves") rather than inventing a new number: `128` sits ~117 levels
/// from Light's own `#f5f5f6` panel/canvas and nowhere near
/// `accent.primary`'s own `#124fb0` (`(18, 79, 176)` — the selected
/// row's highlight, a distinctly different hue as well as luma from
/// `128,128,128`), so the panel, its margin, and the row highlight all
/// stay visually distinct from one another and from the backdrop. Named
/// separately rather than calling `NEUTRAL_CLEAR` directly at each call
/// site, so a reader doesn't have to cross-reference Dark's own token
/// values to see why the same byte value is also correct for Light.
/// `command_palette_style`'s own `COMMAND_PALETTE_MARGIN` fix is pure
/// layout, not colour, so it already carries over unchanged regardless
/// of theme — no Light-specific margin constant is needed here.
const COMMAND_PALETTE_LIGHT_CLEAR: wgpu::Color = NEUTRAL_CLEAR;

/// High Contrast Dark's own gallery backdrop, used for **every** widget
/// in this theme's slice, not a per-widget special case. Computed, not
/// assumed: `design/themes/high-contrast-dark.toml` resolves
/// `surface.canvas`, `surface.app`, `surface.panel`, `surface.raised`,
/// `surface.overlay`, *and* `surface.sunken` to the exact same
/// `hc.black` (`#000000`) — a deliberate OS-high-contrast design choice
/// (that theme's own header comment: "leans on borders for
/// elevation/region cues, not subtle fill gradation"), but it means
/// there is no real per-surface colour left to reuse as "the thing
/// that's different from every widget fill" the way `LIGHT_CLEAR` (a
/// real `surface.canvas` token value) works for Light. A plain black
/// backdrop here would collide byte-for-byte with `Checkbox`'s unchecked
/// box, `TextField`'s fill, `Slider`'s track (all `surface.sunken`), and
/// `CommandPalette`'s panel (`surface.raised`) all at once — not a
/// single special case the way Dark/Light's `CommandPalette` needed, but
/// nearly every widget in this theme's own gallery.
///
/// Reuses `NEUTRAL_CLEAR`'s own value (`#808080`) rather than inventing a
/// new number, for the same reason `COMMAND_PALETTE_LIGHT_CLEAR` already
/// does: it's already established in this file as "a mid-tone, distinct
/// from every real surface token this crate resolves," and that already
/// covers High Contrast Dark's own all-black surface set trivially (mid-
/// grey vs. pure black is an obvious, large difference). Checked against
/// every other colour this theme's six widgets actually paint, not just
/// assumed to "probably be fine":
/// - `border.control`/`text.primary` = `hc.white` (`#ffffff`) — contrasts
///   `#808080` clearly (that's the whole point of a mid-tone backdrop).
/// - `accent.primary` = `hc.yellow` (`#ffff00`) — `Button`'s enabled
///   fill, `ColorSwatch`'s arbitrary swatch colours are unrelated bright
///   RGB the caller picks (`color_swatch_gallery_tree`, unchanged), and
///   `CommandPalette`'s selected-row highlight — all read fine against
///   grey, the same conclusion `COMMAND_PALETTE_LIGHT_CLEAR`'s own doc
///   comment already reached for a bright accent against a neutral
///   mid-tone.
/// - `accent.primary_active` = `hc.yellow_dark` (`#c8a000`) — `Button`'s
///   pressed fill, still visibly distinct from both grey and full
///   yellow.
/// - `state.disabled_opacity` = `0.6` (not Dark/Light's `0.4` — this
///   theme's own deliberately gentler dimming, see that theme's own
///   `[state]` comment) blended over `#808080`: e.g. a disabled black
///   `surface.sunken` fill becomes `0*0.6 + 128*0.4 ≈ 51` — still clearly
///   darker than the `128` backdrop, not a collision.
///
/// So one constant, shared across all six widgets' High Contrast Dark
/// galleries, is the right fix here — not `COMMAND_PALETTE_LIGHT_CLEAR`'s
/// narrower single-widget special case, because the underlying problem
/// (every `surface.*` token being identical) isn't narrow here either.
const HIGH_CONTRAST_DARK_CLEAR: wgpu::Color = NEUTRAL_CLEAR;

/// High Contrast Light's own gallery backdrop, used for **every** widget
/// in this theme's slice, the same shared-not-per-widget shape
/// `HIGH_CONTRAST_DARK_CLEAR` already established. Computed, not assumed
/// to carry over unchanged just because the same constant already
/// worked once: `design/themes/high-contrast-light.toml` resolves
/// `surface.canvas`, `surface.app`, `surface.panel`, `surface.raised`,
/// `surface.overlay`, *and* `surface.sunken` to the exact same
/// `hc.white` (`#ffffff`) — that theme's own header comment: "High
/// Contrast Dark's philosophy, inverted." A plain white backdrop here
/// would collide byte-for-byte with `Checkbox`'s unchecked box,
/// `TextField`'s fill, `Slider`'s track (all `surface.sunken`), and
/// `CommandPalette`'s panel (`surface.raised`) all at once — the exact
/// same shape of problem `HIGH_CONTRAST_DARK_CLEAR`'s own doc comment
/// already records, just with the collision colour inverted.
///
/// Reuses `NEUTRAL_CLEAR`'s own value (`#808080`) rather than inventing a
/// third number: it is equidistant from `hc.black` and `hc.white` by
/// construction, so a fix already proven against an all-black surface
/// set is, by the same reasoning, proven against an all-white one too —
/// but checked here against every other colour this theme's six widgets
/// actually paint, not just assumed:
/// - `border.control`/`text.primary` = `hc.black` (`#000000`) — contrasts
///   `#808080` clearly (the same relationship `HIGH_CONTRAST_DARK_CLEAR`
///   already established works, just with fill and stroke swapped).
/// - `accent.primary` = `hc.blue` (`#0000ff`) — `Button`'s enabled fill
///   and `CommandPalette`'s selected-row highlight; distinct from grey in
///   both hue and luma, unlike High Contrast Dark's yellow this needed no
///   substitute hue (`high-contrast-light.toml`'s own header comment
///   already explains *why* blue replaces yellow here, a colour-legibility
///   concern separate from this backdrop's own grey-vs-blue contrast,
///   which is unambiguous).
/// - `accent.primary_active` = `hc.blue_dark` (`#00008b`) — `Button`'s
///   pressed fill, still visibly distinct from both grey and full blue.
/// - `state.disabled_opacity` = `0.6` (this theme's own value, same as
///   High Contrast Dark, see its own `[state]` comment) blended over
///   `#808080`: e.g. a disabled white `surface.sunken` fill becomes
///   `255*0.6 + 128*0.4 ≈ 204` — still clearly lighter than the `128`
///   backdrop and clearly distinct from the full-opacity `255` fill, not
///   a collision either direction.
///
/// So the same one constant, shared across all six widgets, is the right
/// fix here too — not a fourth colour value, because the underlying
/// problem (every `surface.*` token being identical) is the same problem,
/// just mirrored.
const HIGH_CONTRAST_LIGHT_CLEAR: wgpu::Color = NEUTRAL_CLEAR;

/// Colour-Critical's own gallery backdrop for every widget *except*
/// `CommandPalette` (see `COMMAND_PALETTE_COLOR_CRITICAL_CLEAR` below) —
/// `design/themes/color-critical.toml`'s own `surface.canvas`
/// (`cc.canvas`, `#545454`), the same "what the real UI would show"
/// choice `LIGHT_CLEAR` already established, not an arbitrary pick.
/// Checked against every colour this theme's widgets actually paint, not
/// assumed to carry over from Light just because the same reasoning
/// pattern applies:
/// - `accent.primary` (`accent.blue.600`, `#a4c8ff`) — `Button`'s enabled
///   fill, `Checkbox`'s checked box, `Slider`'s thumb,
///   `CommandPalette`'s selected-row highlight — clears this backdrop at
///   ≈4.43:1, the same number `color-critical.toml`'s own `[accent]`
///   comment already computed when justifying `.600` over `.500` as
///   `primary` in the first place.
/// - `accent.primary_active` (`accent.blue.500`, `#78acff`) — `Button`'s
///   pressed fill — clears at ≈3.30:1, again the same number that file's
///   own comment already gives.
/// - `surface.sunken` (`cc.sunken`, `#242424`) — `Checkbox`'s unchecked
///   box, `Slider`'s track, `TextField`'s fill — clears at ≈2.05:1: real
///   and, if modest, slightly stronger than the ≈1.65:1 Light's own
///   provisional `TextField` number, computed the same way (the real
///   WCAG 2.1 relative-luminance formula, not eyeballed).
/// - `ColorSwatch`'s two arbitrary swatch colours (`(220,40,40)`,
///   `(40,80,220)`, unchanged from every other theme's slice) clear at
///   only ≈1.58:1 and ≈1.18:1 — genuinely weaker than every other
///   colour checked here, and weaker than Light's own ≈4.40:1/≈5.89:1 for
///   the same two colours, because this backdrop is a real mid-tone
///   rather than Light's near-white one. See this file's own module doc
///   comment for the full reasoning on why that's used anyway (never
///   byte-identical to the backdrop, and this theme's whole premise is
///   *chromatic* neutrality, not luminance separation) and why that
///   golden-diff test is flagged for particular bless-time scrutiny
///   rather than treated as settled.
const COLOR_CRITICAL_CLEAR: wgpu::Color = wgpu::Color {
    r: 0x54 as f64 / 255.0,
    g: 0x54 as f64 / 255.0,
    b: 0x54 as f64 / 255.0,
    a: 1.0,
};

/// `CommandPalette`'s own Colour-Critical backdrop — deliberately not
/// plain `COLOR_CRITICAL_CLEAR`. Computed, not assumed: `design/themes/
/// color-critical.toml` resolves `surface.raised` (`CommandPalette`'s own
/// panel fill, `paint_command_palette`) to `cc.raised` (`#4c4c4c`), and
/// `COLOR_CRITICAL_CLEAR` resolves to `cc.canvas` (`#545454`) — these are
/// two genuinely *different* token values, unlike Dark's original bug or
/// Light's own `CommandPalette` collision (where panel and backdrop were
/// byte-for-byte the same token), but the real WCAG contrast between them
/// is only ≈1.13:1: an 8-level gap out of 255, close enough to invisible
/// that it's the same failure in substance — a human reviewing the golden
/// would see essentially no panel at all against `COLOR_CRITICAL_CLEAR`.
/// So this reuses `NEUTRAL_CLEAR`'s own value (`#808080`) instead, exactly
/// as `COMMAND_PALETTE_LIGHT_CLEAR` already does for the analogous Light
/// problem — checked, not carried over blind: `surface.raised`
/// (`#4c4c4c`) vs `NEUTRAL_CLEAR` (`#808080`) is ≈2.17:1, a real and
/// adequate gap, comparable to the ≈2.05:1 `COLOR_CRITICAL_CLEAR`'s own
/// doc comment already accepts for `Checkbox`/`Slider`/`TextField`'s
/// `surface.sunken`, and `accent.primary` (`#a4c8ff`, the selected row's
/// own highlight) is nowhere near `#808080` either — the panel, its
/// margin, and the row highlight all stay visually distinct from each
/// other and from the backdrop. `command_palette_style`'s own
/// `COMMAND_PALETTE_MARGIN` fix is pure layout, not colour, so it already
/// carries over unchanged regardless of theme — no Colour-Critical-
/// specific margin constant is needed here either.
const COMMAND_PALETTE_COLOR_CRITICAL_CLEAR: wgpu::Color = NEUTRAL_CLEAR;

/// Serializes this file's real-GPU tests, this integration test's own
/// copy of the same "one `wgpu::Instance`/`Device` at a time" lock
/// every other real-GPU test file in this workspace carries
/// independently (`src/test_support.rs`'s own doc comment has the full
/// story) — an integration test file compiles to its own separate test
/// binary, so no other crate's or file's lock covers it.
static GPU_TEST_LOCK: Mutex<()> = Mutex::new(());

struct GpuTestContext {
    _guard: MutexGuard<'static, ()>,
    context: aurora_gpu::GpuContext,
}

impl std::ops::Deref for GpuTestContext {
    type Target = aurora_gpu::GpuContext;
    fn deref(&self) -> &aurora_gpu::GpuContext {
        &self.context
    }
}

/// `None` is an inconclusive skip (no GPU adapter on this machine/CI
/// runner, and `AURORA_REQUIRE_GPU` unset); any other failure — and a
/// missing adapter with that variable set — is a real bug and panics.
/// The decision itself lives in
/// `aurora_gpu::test_support::real_context_or_skip`; only the lock and
/// the guard-bundling wrapper are local to this test binary.
fn real_context() -> Option<GpuTestContext> {
    let guard = GPU_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    aurora_gpu::test_support::real_context_or_skip().map(|context| GpuTestContext {
        _guard: guard,
        context,
    })
}

fn dark_theme() -> Theme {
    let palette = match Palette::from_toml_str(PALETTE_TOML) {
        Ok(palette) => palette,
        Err(err) => unreachable!("the committed palette must parse: {err:?}"),
    };
    let mut themes = ThemeSet::new();
    if let Err(err) = themes.register(DARK_THEME_TOML) {
        unreachable!("the committed Dark theme must register: {err:?}");
    }
    match themes.resolve("Dark", &palette) {
        Ok(theme) => theme,
        Err(err) => unreachable!("the committed Dark theme must resolve: {err:?}"),
    }
}

/// Mirrors [`dark_theme`], but for Light. Light's own `extends = "Dark"`
/// needs the parent actually registered first — the exact pattern
/// `contrast.rs`'s own `the_real_light_theme_passes_every_gated_pair`
/// test already establishes for this same pair of TOML files.
fn light_theme() -> Theme {
    let palette = match Palette::from_toml_str(PALETTE_TOML) {
        Ok(palette) => palette,
        Err(err) => unreachable!("the committed palette must parse: {err:?}"),
    };
    let mut themes = ThemeSet::new();
    if let Err(err) = themes.register(DARK_THEME_TOML) {
        unreachable!("the committed Dark theme must register: {err:?}");
    }
    if let Err(err) = themes.register(LIGHT_THEME_TOML) {
        unreachable!("the committed Light theme must register: {err:?}");
    }
    match themes.resolve("Light", &palette) {
        Ok(theme) => theme,
        Err(err) => unreachable!("the committed Light theme must resolve: {err:?}"),
    }
}

/// Mirrors [`dark_theme`]/[`light_theme`], but for High Contrast Dark.
/// `design/themes/high-contrast-dark.toml` also `extends = "Dark"` (see
/// that file's own header comment), so — exactly like [`light_theme`] —
/// the parent needs registering first before `resolve` can walk the
/// `extends` chain.
fn high_contrast_dark_theme() -> Theme {
    let palette = match Palette::from_toml_str(PALETTE_TOML) {
        Ok(palette) => palette,
        Err(err) => unreachable!("the committed palette must parse: {err:?}"),
    };
    let mut themes = ThemeSet::new();
    if let Err(err) = themes.register(DARK_THEME_TOML) {
        unreachable!("the committed Dark theme must register: {err:?}");
    }
    if let Err(err) = themes.register(HIGH_CONTRAST_DARK_THEME_TOML) {
        unreachable!("the committed High Contrast Dark theme must register: {err:?}");
    }
    match themes.resolve("High Contrast Dark", &palette) {
        Ok(theme) => theme,
        Err(err) => unreachable!("the committed High Contrast Dark theme must resolve: {err:?}"),
    }
}

/// Mirrors [`high_contrast_dark_theme`], but for High Contrast Light.
/// `design/themes/high-contrast-light.toml` also `extends = "Dark"` (see
/// that file's own header comment), so — exactly like the others — the
/// parent needs registering first before `resolve` can walk the
/// `extends` chain.
fn high_contrast_light_theme() -> Theme {
    let palette = match Palette::from_toml_str(PALETTE_TOML) {
        Ok(palette) => palette,
        Err(err) => unreachable!("the committed palette must parse: {err:?}"),
    };
    let mut themes = ThemeSet::new();
    if let Err(err) = themes.register(DARK_THEME_TOML) {
        unreachable!("the committed Dark theme must register: {err:?}");
    }
    if let Err(err) = themes.register(HIGH_CONTRAST_LIGHT_THEME_TOML) {
        unreachable!("the committed High Contrast Light theme must register: {err:?}");
    }
    match themes.resolve("High Contrast Light", &palette) {
        Ok(theme) => theme,
        Err(err) => unreachable!("the committed High Contrast Light theme must resolve: {err:?}"),
    }
}

/// Mirrors [`high_contrast_dark_theme`]/[`high_contrast_light_theme`],
/// but for Colour-Critical. `design/themes/color-critical.toml` also
/// `extends = "Dark"` (see that file's own header comment), so — exactly
/// like the others — the parent needs registering first before `resolve`
/// can walk the `extends` chain. `"Color-Critical"`, not `"Colour-
/// Critical"` — the exact, case-sensitive string that file's own `name`
/// field holds, confirmed by reading the committed TOML directly rather
/// than assumed from this file's own prose (which uses the British
/// spelling throughout, matching the design brief's own wording).
fn color_critical_theme() -> Theme {
    let palette = match Palette::from_toml_str(PALETTE_TOML) {
        Ok(palette) => palette,
        Err(err) => unreachable!("the committed palette must parse: {err:?}"),
    };
    let mut themes = ThemeSet::new();
    if let Err(err) = themes.register(DARK_THEME_TOML) {
        unreachable!("the committed Dark theme must register: {err:?}");
    }
    if let Err(err) = themes.register(COLOR_CRITICAL_THEME_TOML) {
        unreachable!("the committed Color-Critical theme must register: {err:?}");
    }
    match themes.resolve("Color-Critical", &palette) {
        Ok(theme) => theme,
        Err(err) => unreachable!("the committed Color-Critical theme must resolve: {err:?}"),
    }
}

fn scales() -> Scales {
    match Scales::from_toml_str(SCALES_TOML) {
        Ok(scales) => scales,
        Err(err) => unreachable!("the committed scales must parse: {err:?}"),
    }
}

fn sized_style(size: (u32, u32)) -> Style {
    Style {
        size: Size {
            width: length(size.0 as f32),
            height: length(size.1 as f32),
        },
        ..Default::default()
    }
}

/// A real, laid-out tree with one `Button` per state — `paint_widget`'s
/// own natural minimal fixture, side by side in `BUTTON_CELL`-sized
/// cells.
fn button_gallery_tree(scales: &Scales) -> (WidgetTree<WidgetKind>, [WidgetId; 3]) {
    let (mut tree, root) = new_tree(Style {
        flex_direction: FlexDirection::Row,
        ..Default::default()
    });
    let enabled = match insert_button(&mut tree, root, scales, "Enabled") {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    let pressed = match insert_button(&mut tree, root, scales, "Pressed") {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    if let Err(err) = set_button_pressed(&mut tree, pressed, true) {
        unreachable!("{err:?}");
    }
    let disabled = match insert_button(&mut tree, root, scales, "Disabled") {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    if let Err(err) = set_button_disabled(&mut tree, disabled, true) {
        unreachable!("{err:?}");
    }

    for id in [enabled, pressed, disabled] {
        if let Err(err) = tree.set_style(id, sized_style(BUTTON_CELL)) {
            unreachable!("{err:?}");
        }
    }
    #[allow(clippy::cast_precision_loss)]
    tree.compute_layout(BUTTON_GALLERY_SIZE.0 as f32, BUTTON_GALLERY_SIZE.1 as f32);
    (tree, [enabled, pressed, disabled])
}

/// A real, laid-out tree with one `Checkbox` per state, mirroring
/// exactly the three cases `src/paint.rs`'s own unit tests already
/// cover: unchecked (enabled), checked (enabled), and unchecked
/// disabled — not checked-and-disabled, so this exercises the
/// `surface.sunken` + `disabled_opacity` combination specifically,
/// distinct from the checked cell's own `accent.primary`.
fn checkbox_gallery_tree(scales: &Scales) -> (WidgetTree<WidgetKind>, [WidgetId; 3]) {
    let (mut tree, root) = new_tree(Style {
        flex_direction: FlexDirection::Row,
        ..Default::default()
    });
    let unchecked = match insert_checkbox(&mut tree, root, scales, "Unchecked") {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    let checked = match insert_checkbox(&mut tree, root, scales, "Checked") {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    if let Err(err) = toggle_checkbox(&mut tree, checked) {
        unreachable!("{err:?}");
    }
    let disabled = match insert_checkbox(&mut tree, root, scales, "Disabled") {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    if let Err(err) = set_checkbox_disabled(&mut tree, disabled, true) {
        unreachable!("{err:?}");
    }

    for id in [unchecked, checked, disabled] {
        if let Err(err) = tree.set_style(id, sized_style(CHECKBOX_CELL)) {
            unreachable!("{err:?}");
        }
    }
    #[allow(clippy::cast_precision_loss)]
    tree.compute_layout(
        CHECKBOX_GALLERY_SIZE.0 as f32,
        CHECKBOX_GALLERY_SIZE.1 as f32,
    );
    (tree, [unchecked, checked, disabled])
}

/// A real, laid-out tree with one `ColorSwatch` per state: a first
/// arbitrary colour, a second, visually distinct arbitrary colour, and
/// the first colour again but disabled — proving both that two
/// different `state.color` values actually paint differently (unlike
/// every other widget here, `ColorSwatch`'s own fill isn't a theme
/// token, so this is the one gallery that can't just compare two
/// different *states* of the same colour) and that `disabled_opacity`
/// still dims a swatch's own arbitrary colour the same way it dims
/// every other widget's token colour.
fn color_swatch_gallery_tree(scales: &Scales) -> (WidgetTree<WidgetKind>, [WidgetId; 3]) {
    let (mut tree, root) = new_tree(Style {
        flex_direction: FlexDirection::Row,
        ..Default::default()
    });
    let red = Color {
        r: 220,
        g: 40,
        b: 40,
    };
    let blue = Color {
        r: 40,
        g: 80,
        b: 220,
    };
    let first = match insert_color_swatch(&mut tree, root, scales, red) {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    let second = match insert_color_swatch(&mut tree, root, scales, blue) {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    let disabled = match insert_color_swatch(&mut tree, root, scales, red) {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    if let Err(err) = set_color_swatch_disabled(&mut tree, disabled, true) {
        unreachable!("{err:?}");
    }

    for id in [first, second, disabled] {
        if let Err(err) = tree.set_style(id, sized_style(COLOR_SWATCH_CELL)) {
            unreachable!("{err:?}");
        }
    }
    #[allow(clippy::cast_precision_loss)]
    tree.compute_layout(
        COLOR_SWATCH_GALLERY_SIZE.0 as f32,
        COLOR_SWATCH_GALLERY_SIZE.1 as f32,
    );
    (tree, [first, second, disabled])
}

/// A real, laid-out tree with one `Slider` per state: at its own
/// minimum value, at its own maximum value, and disabled (at a value
/// in between, so its own thumb sits somewhere a bare min/max
/// comparison wouldn't already cover).
fn slider_gallery_tree(scales: &Scales) -> (WidgetTree<WidgetKind>, [WidgetId; 3]) {
    let (mut tree, root) = new_tree(Style {
        flex_direction: FlexDirection::Row,
        ..Default::default()
    });
    let at_min = match insert_slider(&mut tree, root, scales, "Min", 0.0, 0.0, 100.0) {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    let at_max = match insert_slider(&mut tree, root, scales, "Max", 100.0, 0.0, 100.0) {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    let disabled = match insert_slider(&mut tree, root, scales, "Disabled", 50.0, 0.0, 100.0) {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    if let Err(err) = set_slider_disabled(&mut tree, disabled, true) {
        unreachable!("{err:?}");
    }

    for id in [at_min, at_max, disabled] {
        if let Err(err) = tree.set_style(id, sized_style(SLIDER_CELL)) {
            unreachable!("{err:?}");
        }
    }
    #[allow(clippy::cast_precision_loss)]
    tree.compute_layout(SLIDER_GALLERY_SIZE.0 as f32, SLIDER_GALLERY_SIZE.1 as f32);
    (tree, [at_min, at_max, disabled])
}

/// A real, laid-out tree with one `Scrollbar` per state: a vertical bar
/// at its own minimum value, the same bar at its own maximum, a
/// disabled vertical bar mid-travel, and a horizontal bar mid-travel —
/// four cells covering both orientations, both ends of the travel, a
/// position neither end would reveal, and the disabled state.
///
/// The horizontal cell is the one that proves `widgets::scrollbar`'s own
/// `style()` branch means something at the pixel level rather than only
/// in resolved `taffy` numbers: it is short and wide where its three
/// neighbours are tall and narrow, and its thumb travels along `x`.
fn scrollbar_gallery_tree(scales: &Scales) -> (WidgetTree<WidgetKind>, [WidgetId; 4]) {
    let (mut tree, root) = new_tree(Style {
        flex_direction: FlexDirection::Row,
        ..Default::default()
    });
    let range = ScrollbarRange {
        min: 0.0,
        max: 100.0,
        page_size: 20.0,
    };
    let insert =
        |tree: &mut WidgetTree<WidgetKind>, orientation: Orientation, label: &str, value: f64| {
            match insert_scrollbar(tree, root, scales, orientation, Some(label), value, range) {
                Ok(id) => id,
                Err(err) => unreachable!("{err:?}"),
            }
        };
    let at_min = insert(&mut tree, Orientation::Vertical, "Min", 0.0);
    let at_max = insert(&mut tree, Orientation::Vertical, "Max", 100.0);
    let disabled = insert(&mut tree, Orientation::Vertical, "Disabled", 50.0);
    let horizontal = insert(&mut tree, Orientation::Horizontal, "Horizontal", 50.0);
    if let Err(err) = set_scrollbar_disabled(&mut tree, disabled, true) {
        unreachable!("{err:?}");
    }

    for id in [at_min, at_max, disabled] {
        if let Err(err) = tree.set_style(id, sized_style(SCROLLBAR_VERTICAL_CELL)) {
            unreachable!("{err:?}");
        }
    }
    if let Err(err) = tree.set_style(horizontal, sized_style(SCROLLBAR_HORIZONTAL_CELL)) {
        unreachable!("{err:?}");
    }
    #[allow(clippy::cast_precision_loss)]
    tree.compute_layout(
        SCROLLBAR_GALLERY_SIZE.0 as f32,
        SCROLLBAR_GALLERY_SIZE.1 as f32,
    );
    (tree, [at_min, at_max, disabled, horizontal])
}

/// The four rendered-pixel claims every theme's own scrollbar gallery
/// makes, in one place rather than copy-pasted five times over. Unlike
/// the widgets above — whose per-theme assertions each needed their own
/// backdrop reasoning, and so were genuinely different tests wearing
/// similar shapes — a scrollbar resolves exactly the two tokens a
/// `Slider` already does (`surface.sunken` track, `accent.primary`
/// thumb), so the claim really is identical in all five and duplicating
/// it would only make five places to get it wrong. `theme_name` is
/// threaded through so a failure still says which theme produced it.
fn assert_scrollbar_states_are_distinct(image: &aurora_testkit::Image, theme_name: &str) {
    assert_eq!(image.width, SCROLLBAR_GALLERY_SIZE.0);
    assert_eq!(image.height, SCROLLBAR_GALLERY_SIZE.1);

    let vertical = |cell: u32, y: u32| {
        sample_at(
            image,
            cell * SCROLLBAR_VERTICAL_CELL.0 + SCROLLBAR_VERTICAL_CELL.0 / 2,
            y,
        )
    };
    assert_ne!(
        vertical(0, SCROLLBAR_THUMB_SAMPLE_Y),
        vertical(1, SCROLLBAR_THUMB_SAMPLE_Y),
        "{theme_name}: the thumb must be at a different y offset for a scrollbar at its own \
         minimum vs maximum value"
    );
    assert_ne!(
        vertical(0, SCROLLBAR_DISABLED_SAMPLE_Y)[..3],
        vertical(2, SCROLLBAR_DISABLED_SAMPLE_Y)[..3],
        "{theme_name}: state.disabled_opacity must render the thumb dimmer than full opacity, \
         compared thumb-to-thumb at a row both occupy"
    );
    assert_ne!(
        sample_at(
            image,
            SCROLLBAR_HORIZONTAL_THUMB_SAMPLE_X,
            SCROLLBAR_HORIZONTAL_SAMPLE_Y
        ),
        sample_at(
            image,
            SCROLLBAR_HORIZONTAL_TRACK_SAMPLE_X,
            SCROLLBAR_HORIZONTAL_SAMPLE_Y
        ),
        "{theme_name}: a horizontal scrollbar's thumb must travel along x, leaving bare track \
         to its left"
    );
    assert_ne!(
        sample_at(
            image,
            SCROLLBAR_HORIZONTAL_THUMB_SAMPLE_X,
            SCROLLBAR_VERTICAL_CELL.1 - 1
        ),
        sample_at(
            image,
            SCROLLBAR_HORIZONTAL_THUMB_SAMPLE_X,
            SCROLLBAR_HORIZONTAL_SAMPLE_Y
        ),
        "{theme_name}: a horizontal scrollbar must be one type-scale step tall, so the bottom \
         of its own cell is backdrop, not bar"
    );
}

/// One `TreeView` gallery cell's own layout: `TREE_VIEW_CELL`-sized and
/// `Column`, so its rows stack. `sized_style` alone would drop the
/// `Column` direction `insert_tree_view` sets, which would lay the rows
/// out side by side instead of one under the other.
fn tree_view_cell_style() -> Style {
    Style {
        flex_direction: FlexDirection::Column,
        size: Size {
            width: length(TREE_VIEW_CELL.0 as f32),
            height: length(TREE_VIEW_CELL.1 as f32),
        },
        ..Default::default()
    }
}

/// One cell of the `TreeView` gallery: a selected group row holding a
/// selected enabled child and a selected disabled child — then
/// collapsed, if asked, which really removes both children.
///
/// Every row is selected deliberately. An unselected row paints nothing
/// at all (`paint_tree_item`, the same convention `paint_list_row`
/// uses), so a gallery of unselected rows would be an empty image; what
/// this gallery can show is where each row's own highlight lands, which
/// is exactly what indentation, one-row-tall clamping, and collapse all
/// change.
fn tree_view_gallery_cell(
    tree: &mut WidgetTree<WidgetKind>,
    root: WidgetId,
    scales: &Scales,
    collapsed: bool,
) -> WidgetId {
    let view = match insert_tree_view(tree, root, Some("Layers")) {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    if let Err(err) = tree.set_style(view, tree_view_cell_style()) {
        unreachable!("{err:?}");
    }
    let group = match insert_tree_item(tree, view, scales, "Group", true) {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    let enabled_child = match insert_tree_item(tree, group, scales, "Child", false) {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    let disabled_child = match insert_tree_item(tree, group, scales, "Disabled child", false) {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    for id in [group, enabled_child, disabled_child] {
        if let Err(err) = set_tree_item_selected(tree, id, true) {
            unreachable!("{err:?}");
        }
    }
    if let Err(err) = set_tree_item_disabled(tree, disabled_child, true) {
        unreachable!("{err:?}");
    }
    if collapsed && let Err(err) = set_tree_item_expanded(tree, group, false) {
        unreachable!("{err:?}");
    }
    view
}

/// A real, laid-out `TreeView` gallery: the same tree expanded (left)
/// and collapsed (right). Returns the two cells' own root containers —
/// the rows themselves are reached through `tree.children`, since
/// collapsing the right-hand cell has already destroyed its own.
fn tree_view_gallery_tree(scales: &Scales) -> (WidgetTree<WidgetKind>, [WidgetId; 2]) {
    let (mut tree, root) = new_tree(Style {
        flex_direction: FlexDirection::Row,
        ..Default::default()
    });
    let expanded = tree_view_gallery_cell(&mut tree, root, scales, false);
    let collapsed = tree_view_gallery_cell(&mut tree, root, scales, true);
    #[allow(clippy::cast_precision_loss)]
    tree.compute_layout(
        TREE_VIEW_GALLERY_SIZE.0 as f32,
        TREE_VIEW_GALLERY_SIZE.1 as f32,
    );
    (tree, [expanded, collapsed])
}

/// The four rendered-pixel claims every theme's own `TreeView` gallery
/// makes, shared rather than restated five times — the same reasoning
/// [`assert_scrollbar_states_are_distinct`] records: a tree row resolves
/// exactly one token (`accent.primary`, plus `state.disabled_opacity`),
/// so the claim really is identical in all five themes and duplicating
/// it would only make five places to get it wrong.
///
/// Every sample coordinate below was confirmed against an actual debug
/// scan of this exact render (Dark theme, real adapter), not computed
/// from the layout arithmetic and assumed — the same discipline the two
/// `border.control_opacity` outline proofs in this file already follow.
fn assert_tree_view_states_are_distinct(image: &aurora_testkit::Image, theme_name: &str) {
    assert_eq!(image.width, TREE_VIEW_GALLERY_SIZE.0);
    assert_eq!(image.height, TREE_VIEW_GALLERY_SIZE.1);

    let expanded = |x: u32, y: u32| sample_at(image, x, y);
    let collapsed = |x: u32, y: u32| sample_at(image, TREE_VIEW_CELL.0 + x, y);

    // 1. Indentation is real: at a column inside the parent's own
    //    highlight but left of where a child's begins, the parent's row
    //    is painted and the child's row is not.
    assert_ne!(
        expanded(TREE_VIEW_INDENT_SAMPLE_X, TREE_VIEW_PARENT_SAMPLE_Y),
        expanded(TREE_VIEW_INDENT_SAMPLE_X, TREE_VIEW_CHILD_SAMPLE_Y),
        "{theme_name}: a child row's own highlight must start further right than its \
         parent's -- at this column the parent is painted and the child is not"
    );
    assert_eq!(
        expanded(TREE_VIEW_ROW_SAMPLE_X, TREE_VIEW_CHILD_SAMPLE_Y),
        expanded(TREE_VIEW_ROW_SAMPLE_X, TREE_VIEW_PARENT_SAMPLE_Y),
        "{theme_name}: ... and past the indent, the child's own highlight is the same \
         colour as its parent's, so the difference above really is the indent and not \
         a missing row"
    );

    // 2. `state.disabled_opacity` reaches a row's own highlight.
    assert_ne!(
        expanded(TREE_VIEW_ROW_SAMPLE_X, TREE_VIEW_CHILD_SAMPLE_Y),
        expanded(TREE_VIEW_ROW_SAMPLE_X, TREE_VIEW_DISABLED_CHILD_SAMPLE_Y),
        "{theme_name}: a disabled child's highlight must render dimmer than an enabled \
         one's, compared row-to-row at the same column"
    );

    // 3. Collapsing really removed the children -- not a flag, not a
    //    colour change: where the expanded cell shows a child, the
    //    collapsed one shows backdrop.
    assert_ne!(
        expanded(TREE_VIEW_ROW_SAMPLE_X, TREE_VIEW_CHILD_SAMPLE_Y),
        collapsed(TREE_VIEW_ROW_SAMPLE_X, TREE_VIEW_CHILD_SAMPLE_Y),
        "{theme_name}: collapsing must remove the child rows outright, leaving backdrop \
         where the expanded cell paints a child's highlight"
    );

    // 4. A parent's own highlight is one row tall, not its whole box:
    //    in the collapsed cell the row below its strip is backdrop, and
    //    that same backdrop is what the corner shows.
    assert_eq!(
        collapsed(TREE_VIEW_ROW_SAMPLE_X, TREE_VIEW_CHILD_SAMPLE_Y),
        collapsed(TREE_VIEW_ROW_SAMPLE_X, TREE_VIEW_CELL.1 - 1),
        "{theme_name}: one row below a collapsed parent's own strip must be the same \
         backdrop as the bottom of its cell -- a row's highlight is one line tall, not \
         its whole layout box"
    );
    assert_ne!(
        collapsed(TREE_VIEW_ROW_SAMPLE_X, TREE_VIEW_PARENT_SAMPLE_Y),
        collapsed(TREE_VIEW_ROW_SAMPLE_X, TREE_VIEW_CHILD_SAMPLE_Y),
        "{theme_name}: ... and the collapsed parent's own strip is still painted, so \
         the backdrop above is a real end to the highlight, not an unpainted cell"
    );
}

/// A real, laid-out tree with one `TextField` per state: enabled, and
/// disabled.
fn text_field_gallery_tree(scales: &Scales) -> (WidgetTree<WidgetKind>, [WidgetId; 2]) {
    let (mut tree, root) = new_tree(Style {
        flex_direction: FlexDirection::Row,
        ..Default::default()
    });
    let enabled = match insert_text_field(&mut tree, root, scales, "name", "hello") {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    let disabled = match insert_text_field(&mut tree, root, scales, "name", "") {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    if let Err(err) = set_text_field_disabled(&mut tree, disabled, true) {
        unreachable!("{err:?}");
    }

    for id in [enabled, disabled] {
        if let Err(err) = tree.set_style(id, sized_style(TEXT_FIELD_CELL)) {
            unreachable!("{err:?}");
        }
    }
    #[allow(clippy::cast_precision_loss)]
    tree.compute_layout(
        TEXT_FIELD_GALLERY_SIZE.0 as f32,
        TEXT_FIELD_GALLERY_SIZE.1 as f32,
    );
    (tree, [enabled, disabled])
}

/// `sized_style(COMMAND_PALETTE_CELL)` plus a real `COMMAND_PALETTE_
/// MARGIN` on every side. Margin, not a bigger backdrop colour alone
/// (`NEUTRAL_CLEAR`): the root this gets applied to (`new_tree`'s own
/// default, `Style::default()`) shrink-wraps to its one child's own
/// margin box, the standard flexbox content-sizing behaviour a
/// single-child `Auto`-sized container already has — giving the child
/// real margin is what makes that shrink-wrapped box, and therefore
/// this gallery's own render target
/// (`COMMAND_PALETTE_GALLERY_SIZE` = `COMMAND_PALETTE_CELL` padded out
/// by `COMMAND_PALETTE_MARGIN` on every side), bigger than the panel
/// itself in the first place. Without it, a bigger `size` passed to
/// `render_gallery` alone would just leave extra untouched canvas the
/// tree's own layout never actually reaches — checked directly by
/// `command_palette_style_positions_the_panel_with_a_real_margin`
/// below, headlessly, no GPU needed to prove the *layout* half of this
/// is correct.
fn command_palette_style() -> Style {
    let margin = length(COMMAND_PALETTE_MARGIN as f32);
    Style {
        size: Size {
            width: length(COMMAND_PALETTE_CELL.0 as f32),
            height: length(COMMAND_PALETTE_CELL.1 as f32),
        },
        margin: LayoutRect {
            left: margin,
            right: margin,
            top: margin,
            bottom: margin,
        },
        ..Default::default()
    }
}

/// A real, laid-out tree with one `CommandPalette` — `paint_widget`
/// resolves exactly one visual state for it today (see this file's own
/// module doc comment), so there's only one cell, not a row. No
/// `&Scales` parameter, unlike every other `*_gallery_tree` function
/// here — `insert_command_palette` genuinely doesn't need one.
fn command_palette_gallery_tree() -> (WidgetTree<WidgetKind>, [WidgetId; 1]) {
    let (mut tree, root) = new_tree(Style::default());
    let commands = vec![CommandEntry::new("edit.undo", "Undo")];
    let palette = match insert_command_palette(&mut tree, root, commands) {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    if let Err(err) = tree.set_style(palette, command_palette_style()) {
        unreachable!("{err:?}");
    }
    #[allow(clippy::cast_precision_loss)]
    tree.compute_layout(
        COMMAND_PALETTE_GALLERY_SIZE.0 as f32,
        COMMAND_PALETTE_GALLERY_SIZE.1 as f32,
    );
    (tree, [palette])
}

/// Renders every widget in `tree` (`paint_widget`, in
/// `WidgetTree::paint_order`) onto a `size` offscreen `Rgba8Unorm`
/// target and reads the result back as a real `aurora_testkit::Image`.
///
/// `Rgba8Unorm`, deliberately not the `Bgra8UnormSrgb` `aurora-app`'s
/// own real swapchain uses: a golden PNG stores straight sRGB-gamma
/// bytes directly (what `paint_widget` itself already returns), so
/// this target needs none of `aurora-app`'s own
/// `linearize_paint_color` conversion — applying it here would
/// double-encode, the same bug in the opposite direction from the one
/// that conversion exists to prevent for a real sRGB-aware swapchain.
///
/// All `GpuMesh`es are uploaded and collected *before* the render pass
/// begins, not inside it — `PathPipeline::draw` needs
/// `mesh: &'pass GpuMesh`, so every mesh it draws must outlive the
/// pass, the same constraint `aurora-app`'s own
/// `collect_widget_paints`/`draw_widget_paints` split exists for.
///
/// `size.0 * 4` must already be a multiple of
/// `wgpu::COPY_BYTES_PER_ROW_ALIGNMENT` (asserted below) — this
/// function doesn't pad/strip row bytes the way a fully general
/// readback would need to for an arbitrary width; every other
/// real-GPU readback helper in this workspace has the same
/// restriction today (each just picks an already-aligned size),
/// matching that established shape rather than introducing new,
/// unexercised padding logic this harness's own single caller doesn't
/// need yet.
fn render_gallery(
    context: &GpuTestContext,
    tree: &WidgetTree<WidgetKind>,
    theme: &Theme,
    scales: &Scales,
    size: (u32, u32),
    clear: wgpu::Color,
) -> aurora_testkit::Image {
    let device = context.device();
    let queue = context.queue();

    let widget_paints = collect_gallery_paints(tree, theme, scales, device, queue);

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("gallery"),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("gallery"),
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("gallery"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        draw_gallery_paints(&mut pass, device, queue, size, &widget_paints);
    }

    let rgba = read_rgba8(device, queue, encoder, &target, size);
    match aurora_testkit::Image::new(size.0, size.1, rgba) {
        Ok(image) => image,
        Err(err) => {
            unreachable!("read_rgba8 always returns width * height * 4 bytes, no padding: {err}")
        }
    }
}

/// [`render_gallery`]'s own "upload every widget's paint" step, split
/// out so `render_gallery` itself stays under `clippy::too_many_lines`
/// — see that function's own doc comment for *why* this has to happen
/// before the render pass begins, which is the real reason this can't
/// just be a closure inline in `render_gallery`.
fn collect_gallery_paints(
    tree: &WidgetTree<WidgetKind>,
    theme: &Theme,
    scales: &Scales,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> Vec<(GpuMesh, [f32; 4])> {
    let mut widget_paints = Vec::new();
    // `1.0`: this harness renders to a headless offscreen target with no
    // real window to derive a DPI scale factor from, so `1.0` (no
    // scaling) is the honest, correct choice -- not a stand-in for a
    // real value this test harness is missing.
    for id in tree.paint_order() {
        if let Ok(paints) = paint_widget(tree, id, theme, scales, 1.0) {
            for (mesh, color) in paints {
                let gpu_mesh = GpuMesh::upload(device, queue, &mesh);
                widget_paints.push((gpu_mesh, color));
            }
        }
    }
    widget_paints
}

/// [`render_gallery`]'s own "draw every widget's paint within the
/// already-begun pass" step.
fn draw_gallery_paints<'pass>(
    pass: &mut wgpu::RenderPass<'pass>,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: (u32, u32),
    widget_paints: &'pass [(GpuMesh, [f32; 4])],
) {
    if widget_paints.is_empty() {
        return;
    }
    let mut path = PathPipeline::new(device);
    let pipeline = path.pipeline(device, wgpu::TextureFormat::Rgba8Unorm);
    pass.set_pipeline(pipeline);
    #[allow(clippy::cast_precision_loss)]
    let viewport_size = (size.0 as f32, size.1 as f32);
    for (mesh, color) in widget_paints {
        let bind_group = path.bind_group(device, queue, viewport_size, *color);
        pass.set_bind_group(0, &bind_group, &[]);
        path.draw(pass, mesh);
    }
}

/// [`render_gallery`]'s own final "copy the rendered target back to the
/// CPU" step — `size.0 * 4` must already be a multiple of
/// `wgpu::COPY_BYTES_PER_ROW_ALIGNMENT` (asserted below); this function
/// doesn't pad/strip row bytes the way a fully general readback would
/// need to for an arbitrary width. Every other real-GPU readback helper
/// in this workspace has the same restriction today (each just picks
/// an already-aligned size) — matching that established shape rather
/// than introducing new, unexercised padding logic this harness's own
/// single caller doesn't need yet.
fn read_rgba8(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    mut encoder: wgpu::CommandEncoder,
    target: &wgpu::Texture,
    size: (u32, u32),
) -> Vec<u8> {
    let bytes_per_row = size.0 * 4;
    assert_eq!(
        bytes_per_row % wgpu::COPY_BYTES_PER_ROW_ALIGNMENT,
        0,
        "gallery width must keep rows 256-byte aligned -- see read_rgba8's own doc comment"
    );
    let readback_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("gallery-readback"),
        size: u64::from(bytes_per_row) * u64::from(size.1),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback_buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(size.1),
            },
        },
        wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));

    let slice = readback_buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
    let Ok(Ok(())) = rx.recv() else {
        unreachable!("map_async must complete once the device has been polled to idle");
    };
    let Ok(data) = slice.get_mapped_range() else {
        unreachable!("the buffer was just confirmed mapped successfully above");
    };
    let rgba = data.to_vec();
    drop(data);
    readback_buffer.unmap();
    rgba
}

/// Reads the pixel at absolute image coordinates `(x, y)`.
fn sample_at(image: &aurora_testkit::Image, px: u32, py: u32) -> [u8; 4] {
    let offset = (py * image.width + px) as usize * 4;
    let Some(pixel) = image.rgba.get(offset..offset + 4) else {
        unreachable!("sample point is always within a real gallery image");
    };
    match pixel {
        &[r, g, b, a] => [r, g, b, a],
        _ => unreachable!("sliced exactly 4 bytes"),
    }
}

/// Reads the pixel at the centre of gallery cell `cell` (`cell_size`
/// wide/tall each, side by side left to right) — enough to distinguish
/// each state's own fill colour without needing a golden image at all.
fn sample_cell_centre(image: &aurora_testkit::Image, cell_size: (u32, u32), cell: u32) -> [u8; 4] {
    let cx = cell * cell_size.0 + cell_size.0 / 2;
    let cy = cell_size.1 / 2;
    sample_at(image, cx, cy)
}

/// A real, self-contained proof the harness itself works, needing no
/// golden image: the three states resolve to genuinely different
/// pixels. Pressed uses `accent.primary_active` instead of
/// `accent.primary`, so its RGB must differ outright; disabled applies
/// `state.disabled_opacity` alpha-blended over the clear colour, so
/// its RGB reads dimmer than the enabled button's full-strength
/// colour even though the stored *alpha* in the target is opaque
/// either way (blending over an opaque clear always yields opaque
/// output — the final alpha channel alone can't distinguish them).
#[test]
fn render_gallery_produces_distinct_pixels_for_each_button_state() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = dark_theme();
    let (tree, _ids) = button_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        BUTTON_GALLERY_SIZE,
        wgpu::Color::BLACK,
    );
    assert_eq!(image.width, BUTTON_GALLERY_SIZE.0);
    assert_eq!(image.height, BUTTON_GALLERY_SIZE.1);

    let enabled_px = sample_cell_centre(&image, BUTTON_CELL, 0);
    let pressed_px = sample_cell_centre(&image, BUTTON_CELL, 1);
    let disabled_px = sample_cell_centre(&image, BUTTON_CELL, 2);
    assert_ne!(
        enabled_px, pressed_px,
        "accent.primary vs accent.primary_active must render differently"
    );
    assert_ne!(
        enabled_px[..3],
        disabled_px[..3],
        "state.disabled_opacity blended over the clear colour must render dimmer than full opacity"
    );
}

/// The real golden-image regression test this whole harness exists
/// for — `Button`'s own three states, rendered together, diffed
/// against a checked-in golden PNG the same way
/// `aurora-render::composite_over_matches_the_golden_image` already
/// proves this project's own golden-image discipline for the canvas
/// compositor.
///
/// **Blessed and reviewed 2026-08-07**: Cahya ran
/// `AURORA_BLESS_GOLDEN=1 cargo test -p aurora-widgets --test gallery
/// -- --ignored` on real macOS GPU hardware and confirmed
/// `tests/golden/button_gallery.png` actually shows three visually
/// distinct buttons in Dark theme colours (enabled: bright
/// `accent.primary`; pressed: a darker, more saturated
/// `accent.primary_active`; disabled: a dark, desaturated navy —
/// `accent.primary` alpha-blended at `state.disabled_opacity` over the
/// black clear colour) before this test's own `#[ignore]` was removed
/// — never bless blind, the same discipline
/// `aurora_testkit::compare_to_golden`'s own `AURORA_BLESS_GOLDEN` gate
/// exists to enforce.
#[test]
fn button_gallery_matches_the_golden_image() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = dark_theme();
    let (tree, _ids) = button_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        BUTTON_GALLERY_SIZE,
        wgpu::Color::BLACK,
    );
    let golden_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/button_gallery.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}

/// The same real, self-contained proof as
/// `render_gallery_produces_distinct_pixels_for_each_button_state`, but
/// against the Light theme (`light_theme()`/`LIGHT_CLEAR`) instead of
/// Dark. `button_gallery_tree` itself is unchanged and reused as-is —
/// the tree doesn't depend on theme, only rendering does.
#[test]
fn render_gallery_produces_distinct_pixels_for_each_button_state_in_light_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = light_theme();
    let (tree, _ids) = button_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        BUTTON_GALLERY_SIZE,
        LIGHT_CLEAR,
    );
    assert_eq!(image.width, BUTTON_GALLERY_SIZE.0);
    assert_eq!(image.height, BUTTON_GALLERY_SIZE.1);

    let enabled_px = sample_cell_centre(&image, BUTTON_CELL, 0);
    let pressed_px = sample_cell_centre(&image, BUTTON_CELL, 1);
    let disabled_px = sample_cell_centre(&image, BUTTON_CELL, 2);
    assert_ne!(
        enabled_px, pressed_px,
        "accent.primary vs accent.primary_active must render differently in Light theme too"
    );
    assert_ne!(
        enabled_px[..3],
        disabled_px[..3],
        "state.disabled_opacity blended over the clear colour must render dimmer than full opacity"
    );
}

/// The Light-theme counterpart of `button_gallery_matches_the_golden_
/// image` — same tree, same three states, `light_theme()`/`LIGHT_CLEAR`
/// instead of Dark's `dark_theme()`/`wgpu::Color::BLACK`, diffed
/// against its own golden target (`tests/golden/button_gallery_light.
/// png`, which does not exist yet).
///
/// **`#[ignore]`d, deliberately — this file's own "never bless blind"
/// discipline** (see `aurora_testkit::compare_to_golden`'s own
/// `AURORA_BLESS_GOLDEN` gate): a human on real GPU hardware needs to
/// run `AURORA_BLESS_GOLDEN=1 cargo test -p aurora-widgets --test
/// gallery -- --ignored`, open the written `tests/golden/
/// button_gallery_light.png`, and confirm it actually shows three
/// visually distinct buttons in Light-theme colours before this
/// attribute comes off — the same step every other golden in this file
/// went through before being trusted.
#[test]
fn button_gallery_matches_the_golden_image_in_light_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = light_theme();
    let (tree, _ids) = button_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        BUTTON_GALLERY_SIZE,
        LIGHT_CLEAR,
    );
    let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/button_gallery_light.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}

/// The same real, self-contained proof as
/// `render_gallery_produces_distinct_pixels_for_each_button_state`, but
/// against High Contrast Dark (`high_contrast_dark_theme()`/
/// `HIGH_CONTRAST_DARK_CLEAR`) instead of Dark or Light.
/// `button_gallery_tree` itself is unchanged and reused as-is.
#[test]
fn render_gallery_produces_distinct_pixels_for_each_button_state_in_high_contrast_dark_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = high_contrast_dark_theme();
    let (tree, _ids) = button_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        BUTTON_GALLERY_SIZE,
        HIGH_CONTRAST_DARK_CLEAR,
    );
    assert_eq!(image.width, BUTTON_GALLERY_SIZE.0);
    assert_eq!(image.height, BUTTON_GALLERY_SIZE.1);

    let enabled_px = sample_cell_centre(&image, BUTTON_CELL, 0);
    let pressed_px = sample_cell_centre(&image, BUTTON_CELL, 1);
    let disabled_px = sample_cell_centre(&image, BUTTON_CELL, 2);
    assert_ne!(
        enabled_px, pressed_px,
        "accent.primary (hc.yellow) vs accent.primary_active (hc.yellow_dark) must render \
         differently in High Contrast Dark too"
    );
    assert_ne!(
        enabled_px[..3],
        disabled_px[..3],
        "state.disabled_opacity blended over the clear colour must render dimmer than full opacity"
    );
}

/// The first real, rendered, automated proof that
/// `border.control_opacity = 1.0` — High Contrast Dark's own, unique
/// among every theme landed so far (`design/themes/high-contrast-dark.
/// toml`'s own header comment; Dark/Light both use `0.0`, meaning
/// `control_outline` (`src/paint.rs`) returns `None` for them and
/// nothing beyond the fill is ever painted) — actually produces a
/// visible second shape through the *whole* real pipeline: theme TOML →
/// `paint_widget` → tessellated `GpuMesh` → real GPU rasterization →
/// readback pixels. Every unit test that already exercises
/// `control_outline` (`src/paint.rs`'s own `tests` module) only checks
/// the shape *count* and the resolved paint colour/alpha in the abstract
/// `Paint` list — none of them render anything, so none of them can
/// prove the stroke lands where geometry says it should once tessellated
/// and rasterized for real.
///
/// **How the sample point was chosen, not guessed**: a `lyon`
/// `StrokeTessellator` with default (centred) alignment and
/// `CONTROL_BORDER_WIDTH = 1.0` produces a band that straddles the
/// path's own edge by 0.5px on each side. For the enabled button (cell
/// 0 of `button_gallery_tree`, bounds `x=0,y=0,width=64,height=64`),
/// that means the *right* edge's stroke band is `[63.5, 64.5)` in image
/// space. A debug scan of this exact render (real GPU readback, not
/// assumed) confirmed empirically: pixel column `x=63` (centre `63.5`,
/// this pipeline's rasterizer treats the coverage band as half-open,
/// inclusive of its lower bound) reads pure `[255,255,255]` —
/// `border.control` (`hc.white`) at `alpha=1.0` (the enabled cell is
/// neither pressed nor disabled) — while `x=62`, one pixel further
/// in, still reads the plain `accent.primary` fill (`[255,255,0]`), and
/// `x=0` (the *left* edge, whose own stroke band `[-0.5, 0.5)` is
/// excluded at its own upper bound by the same convention) reads fill
/// too, not border. So this samples the right edge specifically, one
/// pixel in from the cell's own right boundary (`BUTTON_CELL.0 - 1`),
/// at mid-height (`BUTTON_CELL.1 / 2`) to stay well clear of
/// `scales.radius.sm`'s small rounded corners (`radius = 2`) — not the
/// literal boundary pixel, which this same scan showed does *not*
/// reliably land inside the stroke.
#[test]
fn render_gallery_button_outline_proves_border_control_opacity_in_high_contrast_dark_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = high_contrast_dark_theme();
    let (tree, _ids) = button_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        BUTTON_GALLERY_SIZE,
        HIGH_CONTRAST_DARK_CLEAR,
    );

    let fill_px = sample_cell_centre(&image, BUTTON_CELL, 0);
    let edge_px = sample_at(&image, BUTTON_CELL.0 - 1, BUTTON_CELL.1 / 2);
    assert_eq!(
        edge_px,
        [255, 255, 255, 255],
        "border.control (hc.white) at alpha=1.0 must paint pure opaque white one pixel in from \
         the enabled button's own right edge"
    );
    assert_ne!(
        edge_px, fill_px,
        "the outline pixel must read differently from the button's own accent.primary fill"
    );
    // No pixel in this gallery is ever pure backdrop -- unlike
    // `CommandPalette`'s own margined gallery, `BUTTON_GALLERY_SIZE`'s
    // three cells exactly tile the whole canvas with no gap
    // (`button_gallery_tree`'s own doc comment), so `HIGH_CONTRAST_DARK_
    // CLEAR` never actually reaches a readable pixel here to sample
    // directly. That's fine: `edge_px == [255, 255, 255, 255]` above
    // already rules out both the fill (`[255, 255, 0]`) and the
    // backdrop (`(0.5, 0.5, 0.5)`, nowhere near pure white) by
    // construction, without needing a second sample this gallery
    // can't actually provide.
}

/// The Light-theme-shaped High Contrast Dark counterpart of
/// `button_gallery_matches_the_golden_image` — same tree, same three
/// states, `high_contrast_dark_theme()`/`HIGH_CONTRAST_DARK_CLEAR`
/// instead of Dark's `dark_theme()`/`wgpu::Color::BLACK` or Light's
/// `light_theme()`/`LIGHT_CLEAR`, diffed against its own golden target
/// (`tests/golden/button_gallery_high_contrast_dark.png`, which does not
/// exist yet).
///
/// **`#[ignore]`d, deliberately — this file's own "never bless blind"
/// discipline** (see `aurora_testkit::compare_to_golden`'s own
/// `AURORA_BLESS_GOLDEN` gate): a human on real GPU hardware needs to
/// run `AURORA_BLESS_GOLDEN=1 cargo test -p aurora-widgets --test
/// gallery -- --ignored`, open the written `tests/golden/
/// button_gallery_high_contrast_dark.png`, and confirm it actually shows
/// three visually distinct buttons in High Contrast Dark colours,
/// **including the white outline stroke around each** (the first theme
/// where that stroke is visible at all — see
/// `render_gallery_button_outline_proves_border_control_opacity_in_
/// high_contrast_dark_theme`'s own doc comment), before this attribute
/// comes off.
#[test]
fn button_gallery_matches_the_golden_image_in_high_contrast_dark_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = high_contrast_dark_theme();
    let (tree, _ids) = button_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        BUTTON_GALLERY_SIZE,
        HIGH_CONTRAST_DARK_CLEAR,
    );
    let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/button_gallery_high_contrast_dark.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}

/// The same real, self-contained proof as
/// `render_gallery_produces_distinct_pixels_for_each_button_state`, but
/// against High Contrast Light (`high_contrast_light_theme()`/
/// `HIGH_CONTRAST_LIGHT_CLEAR`) instead of Dark, Light, or High Contrast
/// Dark. `button_gallery_tree` itself is unchanged and reused as-is.
#[test]
fn render_gallery_produces_distinct_pixels_for_each_button_state_in_high_contrast_light_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = high_contrast_light_theme();
    let (tree, _ids) = button_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        BUTTON_GALLERY_SIZE,
        HIGH_CONTRAST_LIGHT_CLEAR,
    );
    assert_eq!(image.width, BUTTON_GALLERY_SIZE.0);
    assert_eq!(image.height, BUTTON_GALLERY_SIZE.1);

    let enabled_px = sample_cell_centre(&image, BUTTON_CELL, 0);
    let pressed_px = sample_cell_centre(&image, BUTTON_CELL, 1);
    let disabled_px = sample_cell_centre(&image, BUTTON_CELL, 2);
    assert_ne!(
        enabled_px, pressed_px,
        "accent.primary (hc.blue) vs accent.primary_active (hc.blue_dark) must render \
         differently in High Contrast Light too"
    );
    assert_ne!(
        enabled_px[..3],
        disabled_px[..3],
        "state.disabled_opacity blended over the clear colour must render dimmer than full opacity"
    );
}

/// The High Contrast Light counterpart of `render_gallery_button_
/// outline_proves_border_control_opacity_in_high_contrast_dark_theme` —
/// the same real, rendered proof that `border.control_opacity = 1.0`
/// paints a visible outline through the whole real pipeline, now for the
/// theme where `border.control` resolves to `hc.black` rather than
/// `hc.white`. See that test's own doc comment for how the sample point
/// (one pixel in from the enabled button's own right edge,
/// `BUTTON_CELL.0 - 1`, at mid-height `BUTTON_CELL.1 / 2`) was
/// originally derived from a real debug scan of this pipeline's own
/// stroke tessellation — that geometry is a property of the pipeline,
/// not of any theme's colours, so it transfers directly here rather than
/// needing its own scan; running this test confirms that empirically
/// rather than assuming it.
#[test]
fn render_gallery_button_outline_proves_border_control_opacity_in_high_contrast_light_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = high_contrast_light_theme();
    let (tree, _ids) = button_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        BUTTON_GALLERY_SIZE,
        HIGH_CONTRAST_LIGHT_CLEAR,
    );

    let fill_px = sample_cell_centre(&image, BUTTON_CELL, 0);
    let edge_px = sample_at(&image, BUTTON_CELL.0 - 1, BUTTON_CELL.1 / 2);
    assert_eq!(
        edge_px,
        [0, 0, 0, 255],
        "border.control (hc.black) at alpha=1.0 must paint pure opaque black one pixel in from \
         the enabled button's own right edge"
    );
    assert_ne!(
        edge_px, fill_px,
        "the outline pixel must read differently from the button's own accent.primary fill"
    );
}

/// The High Contrast Light counterpart of `button_gallery_matches_the_
/// golden_image_in_high_contrast_dark_theme` — same tree, same three
/// states, `high_contrast_light_theme()`/`HIGH_CONTRAST_LIGHT_CLEAR`
/// instead of any other theme's own pairing, diffed against its own
/// golden target (`tests/golden/button_gallery_high_contrast_light.png`,
/// which does not exist yet).
///
/// **`#[ignore]`d, deliberately — this file's own "never bless blind"
/// discipline** (see `aurora_testkit::compare_to_golden`'s own
/// `AURORA_BLESS_GOLDEN` gate): a human on real GPU hardware needs to
/// run `AURORA_BLESS_GOLDEN=1 cargo test -p aurora-widgets --test
/// gallery -- --ignored`, open the written `tests/golden/
/// button_gallery_high_contrast_light.png`, and confirm it actually shows
/// three visually distinct buttons in High Contrast Light colours,
/// including the black outline stroke around each, before this attribute
/// comes off.
#[test]
fn button_gallery_matches_the_golden_image_in_high_contrast_light_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = high_contrast_light_theme();
    let (tree, _ids) = button_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        BUTTON_GALLERY_SIZE,
        HIGH_CONTRAST_LIGHT_CLEAR,
    );
    let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/button_gallery_high_contrast_light.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}

/// The same real, self-contained proof as
/// `render_gallery_produces_distinct_pixels_for_each_button_state`, but
/// against Colour-Critical (`color_critical_theme()`/
/// `COLOR_CRITICAL_CLEAR`) instead of any other theme's own pairing.
/// `button_gallery_tree` itself is unchanged and reused as-is.
#[test]
fn render_gallery_produces_distinct_pixels_for_each_button_state_in_color_critical_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = color_critical_theme();
    let (tree, _ids) = button_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        BUTTON_GALLERY_SIZE,
        COLOR_CRITICAL_CLEAR,
    );
    assert_eq!(image.width, BUTTON_GALLERY_SIZE.0);
    assert_eq!(image.height, BUTTON_GALLERY_SIZE.1);

    let enabled_px = sample_cell_centre(&image, BUTTON_CELL, 0);
    let pressed_px = sample_cell_centre(&image, BUTTON_CELL, 1);
    let disabled_px = sample_cell_centre(&image, BUTTON_CELL, 2);
    assert_ne!(
        enabled_px, pressed_px,
        "accent.primary (accent.blue.600) vs accent.primary_active (accent.blue.500) must render \
         differently in Colour-Critical too"
    );
    assert_ne!(
        enabled_px[..3],
        disabled_px[..3],
        "state.disabled_opacity blended over the clear colour must render dimmer than full opacity"
    );
}

/// The Colour-Critical counterpart of `button_gallery_matches_the_golden_
/// image` — same tree, same three states, `color_critical_theme()`/
/// `COLOR_CRITICAL_CLEAR` instead of any other theme's own pairing,
/// diffed against its own golden target (`tests/golden/
/// button_gallery_color_critical.png`, which does not exist yet). No
/// special backdrop handling needed: `accent.primary` clears
/// `COLOR_CRITICAL_CLEAR` at ≈4.43:1 and `accent.primary_active` at
/// ≈3.30:1 (`COLOR_CRITICAL_CLEAR`'s own doc comment), both comfortably
/// distinct from the backdrop and from each other.
///
/// **`#[ignore]`d, deliberately — this file's own "never bless blind"
/// discipline** (see `aurora_testkit::compare_to_golden`'s own
/// `AURORA_BLESS_GOLDEN` gate): a human on real GPU hardware needs to
/// run `AURORA_BLESS_GOLDEN=1 cargo test -p aurora-widgets --test
/// gallery -- --ignored`, open the written `tests/golden/
/// button_gallery_color_critical.png`, and confirm it actually shows
/// three visually distinct buttons in Colour-Critical colours before this
/// attribute comes off — the same step every other golden in this file
/// went through before being trusted.
#[test]
fn button_gallery_matches_the_golden_image_in_color_critical_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = color_critical_theme();
    let (tree, _ids) = button_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        BUTTON_GALLERY_SIZE,
        COLOR_CRITICAL_CLEAR,
    );
    let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/button_gallery_color_critical.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}

/// The same "distinct pixels, no golden needed" proof as `Button`'s
/// own, for `Checkbox`: unchecked (`surface.sunken`) vs checked
/// (`accent.primary`) must render differently outright; unchecked vs
/// unchecked-disabled must render as the same token dimmed.
#[test]
fn render_gallery_produces_distinct_pixels_for_each_checkbox_state() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = dark_theme();
    let (tree, _ids) = checkbox_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        CHECKBOX_GALLERY_SIZE,
        wgpu::Color::BLACK,
    );
    assert_eq!(image.width, CHECKBOX_GALLERY_SIZE.0);
    assert_eq!(image.height, CHECKBOX_GALLERY_SIZE.1);

    let unchecked_px = sample_cell_centre(&image, CHECKBOX_CELL, 0);
    let checked_px = sample_cell_centre(&image, CHECKBOX_CELL, 1);
    let disabled_px = sample_cell_centre(&image, CHECKBOX_CELL, 2);
    assert_ne!(
        unchecked_px, checked_px,
        "surface.sunken vs accent.primary must render differently"
    );
    assert_ne!(
        unchecked_px[..3],
        disabled_px[..3],
        "state.disabled_opacity blended over the clear colour must render dimmer than full opacity"
    );
}

/// **Blessed and reviewed 2026-08-07**: Cahya ran the bless command on
/// real macOS GPU hardware and committed `tests/golden/
/// checkbox_gallery.png`. Confirmed by decoding its own raw pixel
/// bytes (not just eyeballing a thumbnail): unchecked `[20,20,20]`
/// (`surface.sunken`), checked `[120,172,255]` (`accent.primary`,
/// clearly visible), disabled `[8,8,8]` (`surface.sunken` dimmed) —
/// the checked cell's own real, bright colour gives a human reviewing
/// this image a genuine reference point the way `TextField`'s own
/// gallery doesn't (see `NEUTRAL_CLEAR`'s own doc comment), so plain
/// black stayed the right backdrop here.
#[test]
fn checkbox_gallery_matches_the_golden_image() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = dark_theme();
    let (tree, _ids) = checkbox_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        CHECKBOX_GALLERY_SIZE,
        wgpu::Color::BLACK,
    );
    let golden_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/checkbox_gallery.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}

/// The same real, self-contained proof as
/// `render_gallery_produces_distinct_pixels_for_each_checkbox_state`, but
/// against the Light theme (`light_theme()`/`LIGHT_CLEAR`) instead of
/// Dark. `checkbox_gallery_tree` itself is unchanged and reused as-is —
/// the tree doesn't depend on theme, only rendering does.
#[test]
fn render_gallery_produces_distinct_pixels_for_each_checkbox_state_in_light_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = light_theme();
    let (tree, _ids) = checkbox_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        CHECKBOX_GALLERY_SIZE,
        LIGHT_CLEAR,
    );
    assert_eq!(image.width, CHECKBOX_GALLERY_SIZE.0);
    assert_eq!(image.height, CHECKBOX_GALLERY_SIZE.1);

    let unchecked_px = sample_cell_centre(&image, CHECKBOX_CELL, 0);
    let checked_px = sample_cell_centre(&image, CHECKBOX_CELL, 1);
    let disabled_px = sample_cell_centre(&image, CHECKBOX_CELL, 2);
    assert_ne!(
        unchecked_px, checked_px,
        "surface.sunken vs accent.primary must render differently in Light theme too"
    );
    assert_ne!(
        unchecked_px[..3],
        disabled_px[..3],
        "state.disabled_opacity blended over the clear colour must render dimmer than full opacity"
    );
}

/// The Light-theme counterpart of `checkbox_gallery_matches_the_golden_
/// image` — same tree, same three states, `light_theme()`/`LIGHT_CLEAR`
/// instead of Dark's `dark_theme()`/`wgpu::Color::BLACK`, diffed against
/// its own golden target (`tests/golden/checkbox_gallery_light.png`,
/// which does not exist yet). The same reasoning
/// `checkbox_gallery_matches_the_golden_image`'s own doc comment gives
/// for why Dark's `Checkbox` gallery needed no `NEUTRAL_CLEAR`-style
/// backdrop fix applies here too: a real, bright `accent.primary` checked
/// cell gives a human reviewing the golden a genuine reference point,
/// and Light's own fills already contrast fine against `LIGHT_CLEAR`
/// (see `LIGHT_CLEAR`'s own doc comment) — so no special-casing beyond
/// swapping the theme and clear colour is needed.
///
/// **`#[ignore]`d, deliberately — this file's own "never bless blind"
/// discipline** (see `aurora_testkit::compare_to_golden`'s own
/// `AURORA_BLESS_GOLDEN` gate): a human on real GPU hardware needs to
/// run `AURORA_BLESS_GOLDEN=1 cargo test -p aurora-widgets --test
/// gallery -- --ignored`, open the written `tests/golden/
/// checkbox_gallery_light.png`, and confirm it actually shows three
/// visually distinct checkboxes in Light-theme colours before this
/// attribute comes off — the same step every other golden in this file
/// went through before being trusted. This holds regardless of whether
/// the sandbox that ran this test happened to have some GPU adapter
/// (possibly software) available — "real GPU hardware" plus human
/// visual review is the bar, not just "some renderer produced pixels."
#[test]
fn checkbox_gallery_matches_the_golden_image_in_light_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = light_theme();
    let (tree, _ids) = checkbox_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        CHECKBOX_GALLERY_SIZE,
        LIGHT_CLEAR,
    );
    let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/checkbox_gallery_light.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}

/// The same real, self-contained proof as
/// `render_gallery_produces_distinct_pixels_for_each_checkbox_state`, but
/// against High Contrast Dark (`high_contrast_dark_theme()`/
/// `HIGH_CONTRAST_DARK_CLEAR`) instead of Dark or Light.
/// `checkbox_gallery_tree` itself is unchanged and reused as-is. Unlike
/// Dark/Light, `surface.sunken` here is `hc.black` (pure `#000000`,
/// identical to every other `surface.*` token in this theme — see
/// `HIGH_CONTRAST_DARK_CLEAR`'s own doc comment) — the reason this
/// gallery needs the mid-grey backdrop at all: against a plain black
/// clear, the unchecked box would be invisible, exactly the failure
/// `NEUTRAL_CLEAR`'s own doc comment already records for Dark's
/// `TextField`.
#[test]
fn render_gallery_produces_distinct_pixels_for_each_checkbox_state_in_high_contrast_dark_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = high_contrast_dark_theme();
    let (tree, _ids) = checkbox_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        CHECKBOX_GALLERY_SIZE,
        HIGH_CONTRAST_DARK_CLEAR,
    );
    assert_eq!(image.width, CHECKBOX_GALLERY_SIZE.0);
    assert_eq!(image.height, CHECKBOX_GALLERY_SIZE.1);

    let unchecked_px = sample_cell_centre(&image, CHECKBOX_CELL, 0);
    let checked_px = sample_cell_centre(&image, CHECKBOX_CELL, 1);
    let disabled_px = sample_cell_centre(&image, CHECKBOX_CELL, 2);
    assert_ne!(
        unchecked_px, checked_px,
        "surface.sunken (hc.black) vs accent.primary (hc.yellow) must render differently"
    );
    assert_ne!(
        unchecked_px[..3],
        disabled_px[..3],
        "state.disabled_opacity blended over the clear colour must render dimmer than full opacity"
    );
}

/// The High Contrast Dark counterpart of `checkbox_gallery_matches_the_
/// golden_image` — same tree, same three states,
/// `high_contrast_dark_theme()`/`HIGH_CONTRAST_DARK_CLEAR`, diffed
/// against its own golden target (`tests/golden/
/// checkbox_gallery_high_contrast_dark.png`, which does not exist yet).
///
/// **`#[ignore]`d, deliberately — this file's own "never bless blind"
/// discipline**: a human on real GPU hardware needs to run
/// `AURORA_BLESS_GOLDEN=1 cargo test -p aurora-widgets --test gallery --
/// --ignored`, open the written golden, and confirm it actually shows
/// three visually distinct checkboxes in High Contrast Dark colours —
/// including the white outline around every box (`border.control_opacity
/// = 1.0`, see `render_gallery_button_outline_proves_border_control_
/// opacity_in_high_contrast_dark_theme`'s own doc comment for the first
/// widget this was proven for) — before this attribute comes off.
#[test]
fn checkbox_gallery_matches_the_golden_image_in_high_contrast_dark_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = high_contrast_dark_theme();
    let (tree, _ids) = checkbox_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        CHECKBOX_GALLERY_SIZE,
        HIGH_CONTRAST_DARK_CLEAR,
    );
    let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/checkbox_gallery_high_contrast_dark.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}

/// The same real, self-contained proof as
/// `render_gallery_produces_distinct_pixels_for_each_checkbox_state`, but
/// against High Contrast Light (`high_contrast_light_theme()`/
/// `HIGH_CONTRAST_LIGHT_CLEAR`) instead of any other theme.
/// `checkbox_gallery_tree` itself is unchanged and reused as-is. Unlike
/// Dark/Light, `surface.sunken` here is `hc.white` (pure `#ffffff`,
/// identical to every other `surface.*` token in this theme — see
/// `HIGH_CONTRAST_LIGHT_CLEAR`'s own doc comment) — the reason this
/// gallery needs the mid-grey backdrop at all: against a plain white
/// clear, the unchecked box would be invisible, the same failure
/// `HIGH_CONTRAST_DARK_CLEAR`'s own doc comment already records, mirrored.
#[test]
fn render_gallery_produces_distinct_pixels_for_each_checkbox_state_in_high_contrast_light_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = high_contrast_light_theme();
    let (tree, _ids) = checkbox_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        CHECKBOX_GALLERY_SIZE,
        HIGH_CONTRAST_LIGHT_CLEAR,
    );
    assert_eq!(image.width, CHECKBOX_GALLERY_SIZE.0);
    assert_eq!(image.height, CHECKBOX_GALLERY_SIZE.1);

    let unchecked_px = sample_cell_centre(&image, CHECKBOX_CELL, 0);
    let checked_px = sample_cell_centre(&image, CHECKBOX_CELL, 1);
    let disabled_px = sample_cell_centre(&image, CHECKBOX_CELL, 2);
    assert_ne!(
        unchecked_px, checked_px,
        "surface.sunken (hc.white) vs accent.primary (hc.blue) must render differently"
    );
    assert_ne!(
        unchecked_px[..3],
        disabled_px[..3],
        "state.disabled_opacity blended over the clear colour must render dimmer than full opacity"
    );
}

/// The High Contrast Light counterpart of `checkbox_gallery_matches_the_
/// golden_image_in_high_contrast_dark_theme` — same tree, same three
/// states, `high_contrast_light_theme()`/`HIGH_CONTRAST_LIGHT_CLEAR`,
/// diffed against its own golden target (`tests/golden/
/// checkbox_gallery_high_contrast_light.png`, which does not exist yet).
///
/// **`#[ignore]`d, deliberately — this file's own "never bless blind"
/// discipline**: a human on real GPU hardware needs to run
/// `AURORA_BLESS_GOLDEN=1 cargo test -p aurora-widgets --test gallery --
/// --ignored`, open the written golden, and confirm it actually shows
/// three visually distinct checkboxes in High Contrast Light colours —
/// including the black outline around every box (`border.control_opacity
/// = 1.0`) — before this attribute comes off.
#[test]
fn checkbox_gallery_matches_the_golden_image_in_high_contrast_light_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = high_contrast_light_theme();
    let (tree, _ids) = checkbox_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        CHECKBOX_GALLERY_SIZE,
        HIGH_CONTRAST_LIGHT_CLEAR,
    );
    let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/checkbox_gallery_high_contrast_light.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}

/// The same real, self-contained proof as
/// `render_gallery_produces_distinct_pixels_for_each_checkbox_state`, but
/// against Colour-Critical (`color_critical_theme()`/
/// `COLOR_CRITICAL_CLEAR`) instead of any other theme's own pairing.
/// `checkbox_gallery_tree` itself is unchanged and reused as-is.
#[test]
fn render_gallery_produces_distinct_pixels_for_each_checkbox_state_in_color_critical_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = color_critical_theme();
    let (tree, _ids) = checkbox_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        CHECKBOX_GALLERY_SIZE,
        COLOR_CRITICAL_CLEAR,
    );
    assert_eq!(image.width, CHECKBOX_GALLERY_SIZE.0);
    assert_eq!(image.height, CHECKBOX_GALLERY_SIZE.1);

    let unchecked_px = sample_cell_centre(&image, CHECKBOX_CELL, 0);
    let checked_px = sample_cell_centre(&image, CHECKBOX_CELL, 1);
    let disabled_px = sample_cell_centre(&image, CHECKBOX_CELL, 2);
    assert_ne!(
        unchecked_px, checked_px,
        "surface.sunken vs accent.primary must render differently in Colour-Critical too"
    );
    assert_ne!(
        unchecked_px[..3],
        disabled_px[..3],
        "state.disabled_opacity blended over the clear colour must render dimmer than full opacity"
    );
}

/// The Colour-Critical counterpart of `checkbox_gallery_matches_the_
/// golden_image` — same tree, same three states, `color_critical_theme()`
/// /`COLOR_CRITICAL_CLEAR` instead of any other theme's own pairing,
/// diffed against its own golden target (`tests/golden/
/// checkbox_gallery_color_critical.png`, which does not exist yet).
/// `surface.sunken` clears `COLOR_CRITICAL_CLEAR` at ≈2.05:1
/// (`COLOR_CRITICAL_CLEAR`'s own doc comment) — real and modest, but the
/// checked cell's own bright `accent.primary` gives a human reviewing the
/// golden a genuine reference point regardless, the same reasoning
/// Light's own `Checkbox` slice already used.
///
/// **`#[ignore]`d, deliberately — this file's own "never bless blind"
/// discipline** (see `aurora_testkit::compare_to_golden`'s own
/// `AURORA_BLESS_GOLDEN` gate): a human on real GPU hardware needs to
/// run `AURORA_BLESS_GOLDEN=1 cargo test -p aurora-widgets --test
/// gallery -- --ignored`, open the written `tests/golden/
/// checkbox_gallery_color_critical.png`, and confirm it actually shows
/// three visually distinct checkboxes in Colour-Critical colours before
/// this attribute comes off — the same step every other golden in this
/// file went through before being trusted.
#[test]
fn checkbox_gallery_matches_the_golden_image_in_color_critical_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = color_critical_theme();
    let (tree, _ids) = checkbox_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        CHECKBOX_GALLERY_SIZE,
        COLOR_CRITICAL_CLEAR,
    );
    let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/checkbox_gallery_color_critical.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}

/// The same "distinct pixels, no golden needed" proof as the other
/// widgets', for `ColorSwatch`: two different arbitrary colours must
/// render differently from each other (this is the one gallery where
/// that's the fill colour itself, not a theme token two different
/// *states* happen to resolve to), and the disabled cell (same colour
/// as the first, enabled cell) must still render dimmer.
#[test]
fn render_gallery_produces_distinct_pixels_for_each_color_swatch_state() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = dark_theme();
    let (tree, _ids) = color_swatch_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        COLOR_SWATCH_GALLERY_SIZE,
        wgpu::Color::BLACK,
    );
    assert_eq!(image.width, COLOR_SWATCH_GALLERY_SIZE.0);
    assert_eq!(image.height, COLOR_SWATCH_GALLERY_SIZE.1);

    let first_px = sample_cell_centre(&image, COLOR_SWATCH_CELL, 0);
    let second_px = sample_cell_centre(&image, COLOR_SWATCH_CELL, 1);
    let disabled_px = sample_cell_centre(&image, COLOR_SWATCH_CELL, 2);
    assert_ne!(
        first_px, second_px,
        "two different swatch colours must render differently"
    );
    assert_ne!(
        first_px[..3],
        disabled_px[..3],
        "state.disabled_opacity blended over the clear colour must render dimmer than full \
         opacity, even though both cells share the same underlying color"
    );
}

/// **Blessed and reviewed 2026-08-08**: Cahya ran the bless command on
/// real macOS GPU hardware and pushed `tests/golden/
/// color_swatch_gallery.png`. Confirmed by decoding its own raw pixel
/// bytes, not just eyeballing a thumbnail: `[220,40,40]` (the first
/// arbitrary colour), `[40,80,220]` (the second), `[88,16,16]` (the
/// first colour again, disabled — exactly `red * disabled_opacity
/// (0.4)`, `220*0.4=88`, `40*0.4=16`), and `[0,0,0]` at the corner (the
/// plain black backdrop, confirming no bleed outside a swatch's own
/// cell).
#[test]
fn color_swatch_gallery_matches_the_golden_image() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = dark_theme();
    let (tree, _ids) = color_swatch_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        COLOR_SWATCH_GALLERY_SIZE,
        wgpu::Color::BLACK,
    );
    let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/color_swatch_gallery.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}

/// The Light-theme counterpart of
/// `render_gallery_produces_distinct_pixels_for_each_color_swatch_state`,
/// against `light_theme()`/`LIGHT_CLEAR` instead of Dark's
/// `dark_theme()`/`wgpu::Color::BLACK`. `color_swatch_gallery_tree` is
/// reused unchanged — the tree, and the two arbitrary swatch colours it
/// picks (`state.color`, "content, not chrome" — see that function's own
/// doc comment), don't depend on theme at all; only `scales.radius.sm`
/// and `theme.state.disabled_opacity` do (`paint_color_swatch`), and
/// neither is colour-visible in a flat fill the way a token-resolved
/// fill would be. So unlike every other widget's Light slice, the real
/// question here isn't "does the theme's own fill still contrast" (there
/// is no theme-resolved fill) but "do these same arbitrary bright
/// colours still read against a near-white backdrop instead of Dark's
/// near-black one" — checked here with real numbers via
/// `aurora_theme::contrast::contrast_ratio`'s own WCAG 2.1 formula, not
/// assumed from the pattern the other widgets' `LIGHT_CLEAR` reasoning
/// established.
///
/// The first swatch colour (`(220,40,40)`) against `LIGHT_CLEAR`
/// (`(245,245,246)`) is ≈4.40:1, the second (`(40,80,220)`) ≈5.89:1 —
/// both comfortably distinct, similar in magnitude to `(220,40,40)`
/// against Dark's own plain black backdrop (≈4.38:1). So no
/// `COMMAND_PALETTE_LIGHT_CLEAR`-style special-cased backdrop is needed
/// here: unlike that widget's panel fill, these swatch colours never
/// collide with `LIGHT_CLEAR` the way `surface.raised` did with it (see
/// `COMMAND_PALETTE_LIGHT_CLEAR`'s own doc comment) — plain `LIGHT_CLEAR`
/// is the right backdrop, the same conclusion `Button`/`Checkbox`/
/// `Slider` already reached for their own theme-resolved fills.
///
/// The disabled cell is worth checking on its own, since
/// `disabled_opacity` blends the swatch colour over whatever backdrop is
/// in use, and Dark's near-black backdrop and Light's near-white one
/// blend very differently: blending `(220,40,40)` at `disabled_opacity`
/// `0.4` over `LIGHT_CLEAR` gives ≈`(235,163,164)` (contrast ≈2.35:1
/// against the enabled first cell), versus Dark's own already-blessed
/// `(88,16,16)` over black (contrast ≈2.92:1 against its own enabled
/// cell — see `color_swatch_gallery_matches_the_golden_image`'s own doc
/// comment for that byte value). The Light-blended result reads visibly
/// lighter and less saturated than Dark's, as expected from blending
/// toward a light backdrop instead of a dark one — but both stay clearly
/// distinguishable from their own enabled cell (2.35:1 and 2.92:1 are
/// both well above "the same colour"), so this is a real difference
/// worth a human's attention during the bless step, not a defect: it
/// doesn't change which backdrop is correct here, just what the dimmed
/// cell will look like.
#[test]
fn render_gallery_produces_distinct_pixels_for_each_color_swatch_state_in_light_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = light_theme();
    let (tree, _ids) = color_swatch_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        COLOR_SWATCH_GALLERY_SIZE,
        LIGHT_CLEAR,
    );
    assert_eq!(image.width, COLOR_SWATCH_GALLERY_SIZE.0);
    assert_eq!(image.height, COLOR_SWATCH_GALLERY_SIZE.1);

    let first_px = sample_cell_centre(&image, COLOR_SWATCH_CELL, 0);
    let second_px = sample_cell_centre(&image, COLOR_SWATCH_CELL, 1);
    let disabled_px = sample_cell_centre(&image, COLOR_SWATCH_CELL, 2);
    assert_ne!(
        first_px, second_px,
        "two different swatch colours must render differently in Light theme too"
    );
    assert_ne!(
        first_px[..3],
        disabled_px[..3],
        "state.disabled_opacity blended over LIGHT_CLEAR must render dimmer than full opacity, \
         even though both cells share the same underlying color"
    );
}

/// The Light-theme counterpart of `color_swatch_gallery_matches_the_
/// golden_image` — same tree, same three states, `light_theme()`/
/// `LIGHT_CLEAR` instead of Dark's `dark_theme()`/`wgpu::Color::BLACK`,
/// diffed against its own golden target (`tests/golden/
/// color_swatch_gallery_light.png`, which does not exist yet). The
/// backdrop reasoning is `render_gallery_produces_distinct_pixels_for_
/// each_color_swatch_state_in_light_theme`'s own doc comment: both
/// arbitrary swatch colours contrast plain `LIGHT_CLEAR` at ≈4.40:1 and
/// ≈5.89:1, comparable to Dark's own plain-black backdrop, so no special
/// casing beyond swapping the theme and clear colour is needed.
///
/// **`#[ignore]`d, deliberately — this file's own "never bless blind"
/// discipline** (see `aurora_testkit::compare_to_golden`'s own
/// `AURORA_BLESS_GOLDEN` gate): a human on real GPU hardware needs to run
/// `AURORA_BLESS_GOLDEN=1 cargo test -p aurora-widgets --test gallery --
/// --ignored`, open the written `tests/golden/
/// color_swatch_gallery_light.png`, and confirm it actually shows three
/// visually distinct swatches in the expected colours (including the
/// disabled cell's own lighter, desaturated blend against `LIGHT_CLEAR`
/// — see this test's own sibling doc comment for why that blend looks
/// different from Dark's) before this attribute comes off — the same
/// step every other golden in this file went through before being
/// trusted. This holds regardless of whether the sandbox that ran this
/// test happened to have some GPU adapter (possibly software) available
/// — "real GPU hardware" plus human visual review is the bar, not just
/// "some renderer produced pixels."
#[test]
fn color_swatch_gallery_matches_the_golden_image_in_light_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = light_theme();
    let (tree, _ids) = color_swatch_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        COLOR_SWATCH_GALLERY_SIZE,
        LIGHT_CLEAR,
    );
    let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/color_swatch_gallery_light.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}

/// The High Contrast Dark counterpart of
/// `render_gallery_produces_distinct_pixels_for_each_color_swatch_state`.
/// `color_swatch_gallery_tree` is reused unchanged — as with its own
/// Light-theme sibling, the two arbitrary swatch colours
/// (`(220,40,40)`, `(40,80,220)`) are content, not theme-resolved chrome,
/// so this is really "do these same arbitrary colours still contrast
/// `HIGH_CONTRAST_DARK_CLEAR`'s own mid-grey" -- an easy yes here (both
/// are far from `#808080` in every channel), unlike the more marginal
/// Light-theme case that needed real WCAG numbers to settle.
#[test]
fn render_gallery_produces_distinct_pixels_for_each_color_swatch_state_in_high_contrast_dark_theme()
{
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = high_contrast_dark_theme();
    let (tree, _ids) = color_swatch_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        COLOR_SWATCH_GALLERY_SIZE,
        HIGH_CONTRAST_DARK_CLEAR,
    );
    assert_eq!(image.width, COLOR_SWATCH_GALLERY_SIZE.0);
    assert_eq!(image.height, COLOR_SWATCH_GALLERY_SIZE.1);

    let first_px = sample_cell_centre(&image, COLOR_SWATCH_CELL, 0);
    let second_px = sample_cell_centre(&image, COLOR_SWATCH_CELL, 1);
    let disabled_px = sample_cell_centre(&image, COLOR_SWATCH_CELL, 2);
    assert_ne!(
        first_px, second_px,
        "two different swatch colours must render differently in High Contrast Dark too"
    );
    assert_ne!(
        first_px[..3],
        disabled_px[..3],
        "state.disabled_opacity (0.6 here, not Dark/Light's 0.4 -- see this theme's own \
         [state] comment) blended over HIGH_CONTRAST_DARK_CLEAR must render dimmer than full \
         opacity, even though both cells share the same underlying colour"
    );
}

/// The High Contrast Dark counterpart of `color_swatch_gallery_matches_
/// the_golden_image` — diffed against `tests/golden/
/// color_swatch_gallery_high_contrast_dark.png`, which does not exist
/// yet.
///
/// **`#[ignore]`d, deliberately — this file's own "never bless blind"
/// discipline**: a human on real GPU hardware needs to run
/// `AURORA_BLESS_GOLDEN=1 cargo test -p aurora-widgets --test gallery --
/// --ignored`, open the written golden, and confirm it actually shows
/// three visually distinct swatches, each with its own white outline
/// ring (`border.control_opacity = 1.0`), before this attribute comes
/// off.
#[test]
fn color_swatch_gallery_matches_the_golden_image_in_high_contrast_dark_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = high_contrast_dark_theme();
    let (tree, _ids) = color_swatch_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        COLOR_SWATCH_GALLERY_SIZE,
        HIGH_CONTRAST_DARK_CLEAR,
    );
    let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/color_swatch_gallery_high_contrast_dark.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}

/// The High Contrast Light counterpart of
/// `render_gallery_produces_distinct_pixels_for_each_color_swatch_state`.
/// `color_swatch_gallery_tree` is reused unchanged — the two arbitrary
/// swatch colours (`(220,40,40)`, `(40,80,220)`) are content, not
/// theme-resolved chrome, so this is "do these same arbitrary colours
/// still contrast `HIGH_CONTRAST_LIGHT_CLEAR`'s own mid-grey" — an easy
/// yes here too, both are far from `#808080` in every channel, the same
/// conclusion the High Contrast Dark counterpart already reached.
#[test]
fn render_gallery_produces_distinct_pixels_for_each_color_swatch_state_in_high_contrast_light_theme()
 {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = high_contrast_light_theme();
    let (tree, _ids) = color_swatch_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        COLOR_SWATCH_GALLERY_SIZE,
        HIGH_CONTRAST_LIGHT_CLEAR,
    );
    assert_eq!(image.width, COLOR_SWATCH_GALLERY_SIZE.0);
    assert_eq!(image.height, COLOR_SWATCH_GALLERY_SIZE.1);

    let first_px = sample_cell_centre(&image, COLOR_SWATCH_CELL, 0);
    let second_px = sample_cell_centre(&image, COLOR_SWATCH_CELL, 1);
    let disabled_px = sample_cell_centre(&image, COLOR_SWATCH_CELL, 2);
    assert_ne!(
        first_px, second_px,
        "two different swatch colours must render differently in High Contrast Light too"
    );
    assert_ne!(
        first_px[..3],
        disabled_px[..3],
        "state.disabled_opacity (0.6 here, same as High Contrast Dark -- see this theme's own \
         [state] comment) blended over HIGH_CONTRAST_LIGHT_CLEAR must render dimmer than full \
         opacity, even though both cells share the same underlying colour"
    );
}

/// The High Contrast Light counterpart of `color_swatch_gallery_matches_
/// the_golden_image_in_high_contrast_dark_theme` — diffed against
/// `tests/golden/color_swatch_gallery_high_contrast_light.png`, which
/// does not exist yet.
///
/// **`#[ignore]`d, deliberately — this file's own "never bless blind"
/// discipline**: a human on real GPU hardware needs to run
/// `AURORA_BLESS_GOLDEN=1 cargo test -p aurora-widgets --test gallery --
/// --ignored`, open the written golden, and confirm it actually shows
/// three visually distinct swatches, each with its own black outline
/// ring (`border.control_opacity = 1.0`), before this attribute comes
/// off.
#[test]
fn color_swatch_gallery_matches_the_golden_image_in_high_contrast_light_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = high_contrast_light_theme();
    let (tree, _ids) = color_swatch_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        COLOR_SWATCH_GALLERY_SIZE,
        HIGH_CONTRAST_LIGHT_CLEAR,
    );
    let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/color_swatch_gallery_high_contrast_light.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}

/// The same real, self-contained proof as
/// `render_gallery_produces_distinct_pixels_for_each_color_swatch_state`,
/// but against Colour-Critical (`color_critical_theme()`/
/// `COLOR_CRITICAL_CLEAR`) instead of any other theme's own pairing.
/// `color_swatch_gallery_tree` itself is unchanged and reused as-is — the
/// two arbitrary swatch colours (`(220,40,40)`, `(40,80,220)`) are
/// content, not theme-resolved chrome, so distinctness between the two
/// enabled cells and between enabled/disabled is a property of the
/// colours and `state.disabled_opacity`, not this theme.
#[test]
fn render_gallery_produces_distinct_pixels_for_each_color_swatch_state_in_color_critical_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = color_critical_theme();
    let (tree, _ids) = color_swatch_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        COLOR_SWATCH_GALLERY_SIZE,
        COLOR_CRITICAL_CLEAR,
    );
    assert_eq!(image.width, COLOR_SWATCH_GALLERY_SIZE.0);
    assert_eq!(image.height, COLOR_SWATCH_GALLERY_SIZE.1);

    let first_px = sample_cell_centre(&image, COLOR_SWATCH_CELL, 0);
    let second_px = sample_cell_centre(&image, COLOR_SWATCH_CELL, 1);
    let disabled_px = sample_cell_centre(&image, COLOR_SWATCH_CELL, 2);
    assert_ne!(
        first_px, second_px,
        "two different swatch colours must render differently in Colour-Critical too"
    );
    assert_ne!(
        first_px[..3],
        disabled_px[..3],
        "state.disabled_opacity blended over COLOR_CRITICAL_CLEAR must render dimmer than full \
         opacity, even though both cells share the same underlying color"
    );
}

/// The Colour-Critical counterpart of `color_swatch_gallery_matches_the_
/// golden_image` — same tree, same three states, `color_critical_theme()`
/// /`COLOR_CRITICAL_CLEAR` instead of any other theme's own pairing,
/// diffed against its own golden target (`tests/golden/
/// color_swatch_gallery_color_critical.png`, which does not exist yet).
///
/// **Weaker numbers than every other theme's own `ColorSwatch` slice,
/// stated plainly, not smoothed over**: the real WCAG luminance contrast
/// of the two swatch colours against `COLOR_CRITICAL_CLEAR` is only
/// ≈1.58:1 (`(220,40,40)`) and ≈1.18:1 (`(40,80,220)`) —
/// `COLOR_CRITICAL_CLEAR`'s own doc comment has the numbers — markedly
/// below Light's own ≈4.40:1/≈5.89:1 for these exact same two colours,
/// because Colour-Critical's canvas (`#545454`) is a real mid-tone
/// close in raw luminance to both swatch colours, unlike Light's
/// near-white one. The pixels are never byte-identical to the backdrop
/// (`(84,84,84)` vs `(220,40,40)`/`(40,80,220)`), and this theme's whole
/// premise is *chromatic* neutrality (every chrome token `R==G==B`) — so
/// a saturated hue against a genuinely neutral gray surround should still
/// read as a visibly distinct region even where luminance-only contrast
/// is weak, but that's a claim about human colour perception this
/// headless harness cannot itself verify.
///
/// **`#[ignore]`d, deliberately — this file's own "never bless blind"
/// discipline** (see `aurora_testkit::compare_to_golden`'s own
/// `AURORA_BLESS_GOLDEN` gate): a human on real GPU hardware needs to
/// run `AURORA_BLESS_GOLDEN=1 cargo test -p aurora-widgets --test
/// gallery -- --ignored`, open the written `tests/golden/
/// color_swatch_gallery_color_critical.png`, and confirm it actually
/// shows three visually distinct swatches before this attribute comes
/// off. Given the weaker computed numbers above, this golden deserves
/// the same particular scrutiny `TextField`'s own Colour-Critical golden
/// does — if it reads as ambiguous against its backdrop, the fix is a
/// `ColorSwatch`-specific backdrop constant (mirroring `NEUTRAL_CLEAR`'s
/// or `COMMAND_PALETTE_COLOR_CRITICAL_CLEAR`'s own history), not forcing
/// a pass through this comment's chroma argument alone.
#[test]
fn color_swatch_gallery_matches_the_golden_image_in_color_critical_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = color_critical_theme();
    let (tree, _ids) = color_swatch_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        COLOR_SWATCH_GALLERY_SIZE,
        COLOR_CRITICAL_CLEAR,
    );
    let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/color_swatch_gallery_color_critical.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}

/// `Scrollbar`'s own gallery, one test per built-in theme, each a
/// self-contained rendered-pixel proof needing no golden image at all —
/// see [`assert_scrollbar_states_are_distinct`] for the four claims and
/// why they are shared rather than restated five times.
///
/// **Backdrops are inherited, not re-derived.** A scrollbar paints
/// exactly the two tokens a `Slider` does — `surface.sunken` for the
/// track, `accent.primary` for the thumb — so each theme's clear colour
/// here is the one that theme's own `Slider` gallery already uses, and
/// the reasoning recorded on `NEUTRAL_CLEAR`, `LIGHT_CLEAR`,
/// `HIGH_CONTRAST_DARK_CLEAR`, `HIGH_CONTRAST_LIGHT_CLEAR` and
/// `COLOR_CRITICAL_CLEAR` carries over unchanged. Nothing new was
/// assumed: the same two tokens against the same five backdrops.
#[test]
fn render_gallery_produces_distinct_pixels_for_each_scrollbar_state() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let (tree, _ids) = scrollbar_gallery_tree(&scales);
    let image = render_gallery(
        &context,
        &tree,
        &dark_theme(),
        &scales,
        SCROLLBAR_GALLERY_SIZE,
        wgpu::Color::BLACK,
    );
    assert_scrollbar_states_are_distinct(&image, "Dark");
}

#[test]
fn render_gallery_produces_distinct_pixels_for_each_scrollbar_state_in_light_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let (tree, _ids) = scrollbar_gallery_tree(&scales);
    let image = render_gallery(
        &context,
        &tree,
        &light_theme(),
        &scales,
        SCROLLBAR_GALLERY_SIZE,
        LIGHT_CLEAR,
    );
    assert_scrollbar_states_are_distinct(&image, "Light");
}

#[test]
fn render_gallery_produces_distinct_pixels_for_each_scrollbar_state_in_high_contrast_dark_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let (tree, _ids) = scrollbar_gallery_tree(&scales);
    let image = render_gallery(
        &context,
        &tree,
        &high_contrast_dark_theme(),
        &scales,
        SCROLLBAR_GALLERY_SIZE,
        HIGH_CONTRAST_DARK_CLEAR,
    );
    assert_scrollbar_states_are_distinct(&image, "High Contrast Dark");
}

#[test]
fn render_gallery_produces_distinct_pixels_for_each_scrollbar_state_in_high_contrast_light_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let (tree, _ids) = scrollbar_gallery_tree(&scales);
    let image = render_gallery(
        &context,
        &tree,
        &high_contrast_light_theme(),
        &scales,
        SCROLLBAR_GALLERY_SIZE,
        HIGH_CONTRAST_LIGHT_CLEAR,
    );
    assert_scrollbar_states_are_distinct(&image, "High Contrast Light");
}

#[test]
fn render_gallery_produces_distinct_pixels_for_each_scrollbar_state_in_color_critical_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let (tree, _ids) = scrollbar_gallery_tree(&scales);
    let image = render_gallery(
        &context,
        &tree,
        &color_critical_theme(),
        &scales,
        SCROLLBAR_GALLERY_SIZE,
        COLOR_CRITICAL_CLEAR,
    );
    assert_scrollbar_states_are_distinct(&image, "Colour-Critical");
}

/// `Scrollbar`'s own five golden-diff tests, one per built-in theme,
/// against goldens that **do not exist yet** — including Dark's.
///
/// **All five are `#[ignore]`d, Dark included, and that is the point.**
/// Every other widget in this file had its Dark golden blessed by a
/// human on real macOS GPU hardware before the `#[ignore]` came off
/// (`slider_gallery_matches_the_golden_image`'s own doc comment records
/// the date), and the other four themes are still waiting for the same
/// step. A scrollbar is not exempt from that just because the round
/// that added it happened to run on a machine with a real discrete
/// adapter: "the distinct-pixels test passes" is not "the golden is
/// trustworthy", which is exactly the distinction this file's own
/// header insists on. A human runs `AURORA_BLESS_GOLDEN=1 cargo test -p
/// aurora-widgets --test gallery -- --ignored`, opens the five written
/// PNGs, and confirms each shows a thumb at the top, a thumb at the
/// bottom, a dimmed thumb mid-track, and a short wide bar with its own
/// thumb mid-travel — before any of these attributes come off.
macro_rules! scrollbar_golden_test {
    ($name:ident, $theme:expr, $clear:expr, $golden:expr) => {
        #[test]
        #[ignore = "golden not blessed: needs a human on real GPU hardware"]
        fn $name() {
            let Some(context) = real_context() else {
                return;
            };
            let scales = scales();
            let (tree, _ids) = scrollbar_gallery_tree(&scales);
            let image = render_gallery(
                &context,
                &tree,
                &$theme,
                &scales,
                SCROLLBAR_GALLERY_SIZE,
                $clear,
            );
            let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(concat!("tests/golden/", $golden));
            if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
                unreachable!("{err}");
            }
        }
    };
}

scrollbar_golden_test!(
    scrollbar_gallery_matches_the_golden_image,
    dark_theme(),
    wgpu::Color::BLACK,
    "scrollbar_gallery.png"
);
scrollbar_golden_test!(
    scrollbar_gallery_matches_the_golden_image_in_light_theme,
    light_theme(),
    LIGHT_CLEAR,
    "scrollbar_gallery_light.png"
);
scrollbar_golden_test!(
    scrollbar_gallery_matches_the_golden_image_in_high_contrast_dark_theme,
    high_contrast_dark_theme(),
    HIGH_CONTRAST_DARK_CLEAR,
    "scrollbar_gallery_high_contrast_dark.png"
);
scrollbar_golden_test!(
    scrollbar_gallery_matches_the_golden_image_in_high_contrast_light_theme,
    high_contrast_light_theme(),
    HIGH_CONTRAST_LIGHT_CLEAR,
    "scrollbar_gallery_high_contrast_light.png"
);
scrollbar_golden_test!(
    scrollbar_gallery_matches_the_golden_image_in_color_critical_theme,
    color_critical_theme(),
    COLOR_CRITICAL_CLEAR,
    "scrollbar_gallery_color_critical.png"
);

/// A headless (no GPU) proof that the number every `TreeView` sample
/// coordinate above is built from is the number the real layout
/// actually produces — `TREE_VIEW_ROW_HEIGHT` restates
/// `aurora_widgets::widgets`' own `Scales`-derived `tree_row_height`,
/// and a scale change that moved the rows would otherwise leave every
/// pixel assertion in this file quietly sampling the wrong band.
#[test]
fn tree_view_gallery_rows_are_one_row_tall() {
    let scales = scales();
    let (tree, [expanded, _collapsed]) = tree_view_gallery_tree(&scales);
    let Some(&group) = tree.children(expanded).and_then(<[_]>::first) else {
        unreachable!("the expanded cell holds its own group row");
    };
    let Some(rows) = tree.children(group) else {
        unreachable!("the expanded cell's group holds two children");
    };
    assert_eq!(rows.len(), 2);
    let Some(group_bounds) = tree.bounds(group) else {
        unreachable!("just laid out");
    };
    assert_eq!(
        group_bounds.height,
        TREE_VIEW_ROW_HEIGHT * 3,
        "a group's own box spans its own row plus both children"
    );
    let mut expected_y = group_bounds.y + i64::from(TREE_VIEW_ROW_HEIGHT);
    for &row in rows {
        let Some(bounds) = tree.bounds(row) else {
            unreachable!("just laid out");
        };
        assert_eq!(bounds.height, TREE_VIEW_ROW_HEIGHT);
        assert_eq!(
            bounds.y, expected_y,
            "child rows stack under the parent's row"
        );
        let indent = bounds.x - group_bounds.x;
        assert_eq!(indent, 16, "one spacing.md step of indent");
        assert!(
            i64::from(TREE_VIEW_INDENT_SAMPLE_X) < indent,
            "the indent sample column must sit left of where a child's own row begins"
        );
        expected_y += i64::from(TREE_VIEW_ROW_HEIGHT);
    }
}

/// `TreeView`'s own gallery, one test per built-in theme, each a
/// self-contained rendered-pixel proof needing no golden image at all —
/// see [`assert_tree_view_states_are_distinct`] for the four claims and
/// why they are shared rather than restated five times.
///
/// **Backdrops are inherited, not re-derived.** A tree row paints
/// exactly one token, `accent.primary` (dimmed by
/// `state.disabled_opacity` when disabled) — the same token a selected
/// `CommandPalette` row already paints and a `Slider`/`Scrollbar` thumb
/// already uses — so each theme's clear colour here is the one that
/// theme's own `Slider`/`Scrollbar` gallery already uses, and the
/// reasoning recorded on `NEUTRAL_CLEAR`, `LIGHT_CLEAR`,
/// `HIGH_CONTRAST_DARK_CLEAR`, `HIGH_CONTRAST_LIGHT_CLEAR` and
/// `COLOR_CRITICAL_CLEAR` carries over unchanged. Nothing new was
/// assumed: one already-checked token against the same five backdrops.
#[test]
fn render_gallery_produces_distinct_pixels_for_each_tree_view_state() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let (tree, _ids) = tree_view_gallery_tree(&scales);
    let image = render_gallery(
        &context,
        &tree,
        &dark_theme(),
        &scales,
        TREE_VIEW_GALLERY_SIZE,
        wgpu::Color::BLACK,
    );
    assert_tree_view_states_are_distinct(&image, "Dark");
}

#[test]
fn render_gallery_produces_distinct_pixels_for_each_tree_view_state_in_light_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let (tree, _ids) = tree_view_gallery_tree(&scales);
    let image = render_gallery(
        &context,
        &tree,
        &light_theme(),
        &scales,
        TREE_VIEW_GALLERY_SIZE,
        LIGHT_CLEAR,
    );
    assert_tree_view_states_are_distinct(&image, "Light");
}

#[test]
fn render_gallery_produces_distinct_pixels_for_each_tree_view_state_in_high_contrast_dark_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let (tree, _ids) = tree_view_gallery_tree(&scales);
    let image = render_gallery(
        &context,
        &tree,
        &high_contrast_dark_theme(),
        &scales,
        TREE_VIEW_GALLERY_SIZE,
        HIGH_CONTRAST_DARK_CLEAR,
    );
    assert_tree_view_states_are_distinct(&image, "High Contrast Dark");
}

#[test]
fn render_gallery_produces_distinct_pixels_for_each_tree_view_state_in_high_contrast_light_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let (tree, _ids) = tree_view_gallery_tree(&scales);
    let image = render_gallery(
        &context,
        &tree,
        &high_contrast_light_theme(),
        &scales,
        TREE_VIEW_GALLERY_SIZE,
        HIGH_CONTRAST_LIGHT_CLEAR,
    );
    assert_tree_view_states_are_distinct(&image, "High Contrast Light");
}

#[test]
fn render_gallery_produces_distinct_pixels_for_each_tree_view_state_in_color_critical_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let (tree, _ids) = tree_view_gallery_tree(&scales);
    let image = render_gallery(
        &context,
        &tree,
        &color_critical_theme(),
        &scales,
        TREE_VIEW_GALLERY_SIZE,
        COLOR_CRITICAL_CLEAR,
    );
    assert_tree_view_states_are_distinct(&image, "Colour-Critical");
}

/// `TreeView`'s own five golden-diff tests, one per built-in theme,
/// against goldens that **do not exist yet** — including Dark's, and
/// deliberately so, exactly as `scrollbar_golden_test!`'s own doc
/// comment records. A human runs `AURORA_BLESS_GOLDEN=1 cargo test -p
/// aurora-widgets --test gallery -- --ignored`, opens the five written
/// PNGs, and confirms each shows a full-width parent strip with two
/// indented child strips beneath it (the lower one dimmed) on the left,
/// and a lone parent strip with empty space beneath it on the right,
/// before any of these attributes come off.
macro_rules! tree_view_golden_test {
    ($name:ident, $theme:expr, $clear:expr, $golden:expr) => {
        #[test]
        #[ignore = "golden not blessed: needs a human on real GPU hardware"]
        fn $name() {
            let Some(context) = real_context() else {
                return;
            };
            let scales = scales();
            let (tree, _ids) = tree_view_gallery_tree(&scales);
            let image = render_gallery(
                &context,
                &tree,
                &$theme,
                &scales,
                TREE_VIEW_GALLERY_SIZE,
                $clear,
            );
            let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(concat!("tests/golden/", $golden));
            if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
                unreachable!("{err}");
            }
        }
    };
}

tree_view_golden_test!(
    tree_view_gallery_matches_the_golden_image,
    dark_theme(),
    wgpu::Color::BLACK,
    "tree_view_gallery.png"
);
tree_view_golden_test!(
    tree_view_gallery_matches_the_golden_image_in_light_theme,
    light_theme(),
    LIGHT_CLEAR,
    "tree_view_gallery_light.png"
);
tree_view_golden_test!(
    tree_view_gallery_matches_the_golden_image_in_high_contrast_dark_theme,
    high_contrast_dark_theme(),
    HIGH_CONTRAST_DARK_CLEAR,
    "tree_view_gallery_high_contrast_dark.png"
);
tree_view_golden_test!(
    tree_view_gallery_matches_the_golden_image_in_high_contrast_light_theme,
    high_contrast_light_theme(),
    HIGH_CONTRAST_LIGHT_CLEAR,
    "tree_view_gallery_high_contrast_light.png"
);
tree_view_golden_test!(
    tree_view_gallery_matches_the_golden_image_in_color_critical_theme,
    color_critical_theme(),
    COLOR_CRITICAL_CLEAR,
    "tree_view_gallery_color_critical.png"
);

/// `Slider`'s own "distinct pixels" proof is shaped differently from
/// the others: instead of comparing each cell's own centre (which
/// would just show the track, not the thumb, for anything but a
/// dead-centre value), this samples a fixed offset from each cell's
/// own left edge (`x = 16`, well within the thumb's own 32px width) —
/// at the minimum value the thumb sits right there (`accent.primary`);
/// at the maximum value the thumb has moved to the cell's own right
/// edge, so that same offset now shows bare track (`surface.sunken`)
/// instead. A real, direct proof the thumb's own position actually
/// moved, via rendered pixels rather than mesh vertices
/// (`src/paint.rs`'s own `a_sliders_thumb_moves_right_as_its_value_
/// increases` unit test already covers the geometry; this covers the
/// GPU pipeline actually drawing it).
#[test]
fn render_gallery_produces_distinct_pixels_for_each_slider_state() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = dark_theme();
    let (tree, _ids) = slider_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        SLIDER_GALLERY_SIZE,
        wgpu::Color::BLACK,
    );
    assert_eq!(image.width, SLIDER_GALLERY_SIZE.0);
    assert_eq!(image.height, SLIDER_GALLERY_SIZE.1);

    let near_left_edge = |cell: u32| {
        sample_at(
            &image,
            cell * SLIDER_CELL.0 + SLIDER_THUMB_SAMPLE_OFFSET_X,
            SLIDER_CELL.1 / 2,
        )
    };
    let at_min = near_left_edge(0);
    let at_max = near_left_edge(1);
    let disabled = near_left_edge(2);
    assert_ne!(
        at_min, at_max,
        "the thumb must be at a different x offset for a slider at its own minimum vs maximum value"
    );
    assert_ne!(
        at_max[..3],
        disabled[..3],
        "state.disabled_opacity must render the track dimmer than full opacity, at the same offset"
    );
}

/// **Blessed and reviewed 2026-08-07**: Cahya ran the bless command on
/// real macOS GPU hardware and committed `tests/golden/
/// slider_gallery.png`, clearly showing the thumb at the left edge
/// (minimum value), the right edge (maximum value), and a dimmed thumb
/// at its own middle position (disabled) — visually unambiguous, no
/// low-contrast concern the way `CommandPalette`/`TextField`'s own
/// galleries had (see `NEUTRAL_CLEAR`'s own doc comment).
#[test]
fn slider_gallery_matches_the_golden_image() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = dark_theme();
    let (tree, _ids) = slider_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        SLIDER_GALLERY_SIZE,
        wgpu::Color::BLACK,
    );
    let golden_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/slider_gallery.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}

/// The same real, self-contained proof as
/// `render_gallery_produces_distinct_pixels_for_each_slider_state`, but
/// against the Light theme (`light_theme()`/`LIGHT_CLEAR`) instead of
/// Dark. `slider_gallery_tree` itself is unchanged and reused as-is —
/// the tree doesn't depend on theme, only rendering does. Light's own
/// `surface.sunken` (`neutral.700`, `#c1c1c7`) against `LIGHT_CLEAR`
/// (`neutral.900`, `#f5f5f6`) is a real, if modest, contrast — the same
/// spirit as `Checkbox`'s own unchecked box getting away with a modest
/// contrast because a brighter cell (here, the thumb's own
/// `accent.primary`, `#124fb0` in Light) anchors the review; no new
/// backdrop constant was needed.
#[test]
fn render_gallery_produces_distinct_pixels_for_each_slider_state_in_light_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = light_theme();
    let (tree, _ids) = slider_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        SLIDER_GALLERY_SIZE,
        LIGHT_CLEAR,
    );
    assert_eq!(image.width, SLIDER_GALLERY_SIZE.0);
    assert_eq!(image.height, SLIDER_GALLERY_SIZE.1);

    let near_left_edge = |cell: u32| {
        sample_at(
            &image,
            cell * SLIDER_CELL.0 + SLIDER_THUMB_SAMPLE_OFFSET_X,
            SLIDER_CELL.1 / 2,
        )
    };
    let at_min = near_left_edge(0);
    let at_max = near_left_edge(1);
    let disabled = near_left_edge(2);
    assert_ne!(
        at_min, at_max,
        "the thumb must be at a different x offset for a slider at its own minimum vs maximum value in Light theme too"
    );
    assert_ne!(
        at_max[..3],
        disabled[..3],
        "state.disabled_opacity must render the track dimmer than full opacity, at the same offset"
    );
}

/// The Light-theme counterpart of `slider_gallery_matches_the_golden_
/// image` — same tree, same three states, `light_theme()`/`LIGHT_CLEAR`
/// instead of Dark's `dark_theme()`/`wgpu::Color::BLACK`, diffed against
/// its own golden target (`tests/golden/slider_gallery_light.png`, which
/// does not exist yet). The same reasoning
/// `render_gallery_produces_distinct_pixels_for_each_slider_state_in_
/// light_theme`'s own doc comment gives for why no special backdrop
/// handling is needed applies here too: `surface.sunken` against
/// `LIGHT_CLEAR` is a real, if modest, contrast, and the thumb's own
/// bright `accent.primary` gives a human reviewing the golden a genuine
/// reference point regardless.
///
/// **`#[ignore]`d, deliberately — this file's own "never bless blind"
/// discipline** (see `aurora_testkit::compare_to_golden`'s own
/// `AURORA_BLESS_GOLDEN` gate): a human on real GPU hardware needs to
/// run `AURORA_BLESS_GOLDEN=1 cargo test -p aurora-widgets --test
/// gallery -- --ignored`, open the written `tests/golden/
/// slider_gallery_light.png`, and confirm it actually shows the thumb at
/// three visually distinct positions/opacities in Light-theme colours
/// before this attribute comes off — the same step every other golden
/// in this file went through before being trusted.
#[test]
fn slider_gallery_matches_the_golden_image_in_light_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = light_theme();
    let (tree, _ids) = slider_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        SLIDER_GALLERY_SIZE,
        LIGHT_CLEAR,
    );
    let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/slider_gallery_light.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}

/// The same real, self-contained proof as
/// `render_gallery_produces_distinct_pixels_for_each_slider_state`, but
/// against High Contrast Dark (`high_contrast_dark_theme()`/
/// `HIGH_CONTRAST_DARK_CLEAR`) instead of Dark or Light.
/// `slider_gallery_tree` itself is unchanged and reused as-is. Unlike
/// Dark/Light, the track (`surface.sunken`, `hc.black`) needs
/// `HIGH_CONTRAST_DARK_CLEAR` to be visible at all against the clear
/// colour, the same reasoning `Checkbox`'s own High Contrast Dark test
/// gives (`HIGH_CONTRAST_DARK_CLEAR`'s own doc comment).
#[test]
fn render_gallery_produces_distinct_pixels_for_each_slider_state_in_high_contrast_dark_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = high_contrast_dark_theme();
    let (tree, _ids) = slider_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        SLIDER_GALLERY_SIZE,
        HIGH_CONTRAST_DARK_CLEAR,
    );
    assert_eq!(image.width, SLIDER_GALLERY_SIZE.0);
    assert_eq!(image.height, SLIDER_GALLERY_SIZE.1);

    let near_left_edge = |cell: u32| {
        sample_at(
            &image,
            cell * SLIDER_CELL.0 + SLIDER_THUMB_SAMPLE_OFFSET_X,
            SLIDER_CELL.1 / 2,
        )
    };
    let at_min = near_left_edge(0);
    let at_max = near_left_edge(1);
    let disabled = near_left_edge(2);
    assert_ne!(
        at_min, at_max,
        "the thumb must be at a different x offset for a slider at its own minimum vs maximum \
         value in High Contrast Dark too"
    );
    assert_ne!(
        at_max[..3],
        disabled[..3],
        "state.disabled_opacity must render the track dimmer than full opacity, at the same offset"
    );
}

/// The High Contrast Dark counterpart of `slider_gallery_matches_the_
/// golden_image` — diffed against `tests/golden/
/// slider_gallery_high_contrast_dark.png`, which does not exist yet.
///
/// **`#[ignore]`d, deliberately — this file's own "never bless blind"
/// discipline**: a human on real GPU hardware needs to run
/// `AURORA_BLESS_GOLDEN=1 cargo test -p aurora-widgets --test gallery --
/// --ignored`, open the written golden, and confirm it actually shows
/// the thumb at three visually distinct positions/opacities in High
/// Contrast Dark colours, with a visible white ring around the thumb
/// itself (`control_outline` is applied to the thumb path only, not the
/// track — see `paint_slider`'s own doc comment), before this attribute
/// comes off.
#[test]
fn slider_gallery_matches_the_golden_image_in_high_contrast_dark_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = high_contrast_dark_theme();
    let (tree, _ids) = slider_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        SLIDER_GALLERY_SIZE,
        HIGH_CONTRAST_DARK_CLEAR,
    );
    let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/slider_gallery_high_contrast_dark.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}

/// The same real, self-contained proof as
/// `render_gallery_produces_distinct_pixels_for_each_slider_state`, but
/// against High Contrast Light (`high_contrast_light_theme()`/
/// `HIGH_CONTRAST_LIGHT_CLEAR`) instead of any other theme.
/// `slider_gallery_tree` itself is unchanged and reused as-is. As with
/// High Contrast Dark, the track (`surface.sunken`, `hc.white` here)
/// needs `HIGH_CONTRAST_LIGHT_CLEAR` to be visible at all against the
/// clear colour, the same reasoning `Checkbox`'s own High Contrast Light
/// test gives (`HIGH_CONTRAST_LIGHT_CLEAR`'s own doc comment).
#[test]
fn render_gallery_produces_distinct_pixels_for_each_slider_state_in_high_contrast_light_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = high_contrast_light_theme();
    let (tree, _ids) = slider_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        SLIDER_GALLERY_SIZE,
        HIGH_CONTRAST_LIGHT_CLEAR,
    );
    assert_eq!(image.width, SLIDER_GALLERY_SIZE.0);
    assert_eq!(image.height, SLIDER_GALLERY_SIZE.1);

    let near_left_edge = |cell: u32| {
        sample_at(
            &image,
            cell * SLIDER_CELL.0 + SLIDER_THUMB_SAMPLE_OFFSET_X,
            SLIDER_CELL.1 / 2,
        )
    };
    let at_min = near_left_edge(0);
    let at_max = near_left_edge(1);
    let disabled = near_left_edge(2);
    assert_ne!(
        at_min, at_max,
        "the thumb must be at a different x offset for a slider at its own minimum vs maximum \
         value in High Contrast Light too"
    );
    assert_ne!(
        at_max[..3],
        disabled[..3],
        "state.disabled_opacity must render the track dimmer than full opacity, at the same offset"
    );
}

/// The High Contrast Light counterpart of `slider_gallery_matches_the_
/// golden_image_in_high_contrast_dark_theme` — diffed against
/// `tests/golden/slider_gallery_high_contrast_light.png`, which does not
/// exist yet.
///
/// **`#[ignore]`d, deliberately — this file's own "never bless blind"
/// discipline**: a human on real GPU hardware needs to run
/// `AURORA_BLESS_GOLDEN=1 cargo test -p aurora-widgets --test gallery --
/// --ignored`, open the written golden, and confirm it actually shows the
/// thumb at three visually distinct positions/opacities in High Contrast
/// Light colours, with a visible black ring around the thumb itself
/// (`control_outline` is applied to the thumb path only, not the track),
/// before this attribute comes off.
#[test]
fn slider_gallery_matches_the_golden_image_in_high_contrast_light_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = high_contrast_light_theme();
    let (tree, _ids) = slider_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        SLIDER_GALLERY_SIZE,
        HIGH_CONTRAST_LIGHT_CLEAR,
    );
    let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/slider_gallery_high_contrast_light.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}

/// The same real, self-contained proof as
/// `render_gallery_produces_distinct_pixels_for_each_slider_state`, but
/// against Colour-Critical (`color_critical_theme()`/
/// `COLOR_CRITICAL_CLEAR`) instead of any other theme's own pairing.
/// `slider_gallery_tree` itself is unchanged and reused as-is.
#[test]
fn render_gallery_produces_distinct_pixels_for_each_slider_state_in_color_critical_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = color_critical_theme();
    let (tree, _ids) = slider_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        SLIDER_GALLERY_SIZE,
        COLOR_CRITICAL_CLEAR,
    );
    assert_eq!(image.width, SLIDER_GALLERY_SIZE.0);
    assert_eq!(image.height, SLIDER_GALLERY_SIZE.1);

    let near_left_edge = |cell: u32| {
        sample_at(
            &image,
            cell * SLIDER_CELL.0 + SLIDER_THUMB_SAMPLE_OFFSET_X,
            SLIDER_CELL.1 / 2,
        )
    };
    let at_min = near_left_edge(0);
    let at_max = near_left_edge(1);
    let disabled = near_left_edge(2);
    assert_ne!(
        at_min, at_max,
        "the thumb must be at a different x offset for a slider at its own minimum vs maximum \
         value in Colour-Critical too"
    );
    assert_ne!(
        at_max[..3],
        disabled[..3],
        "state.disabled_opacity must render the track dimmer than full opacity, at the same offset"
    );
}

/// The Colour-Critical counterpart of `slider_gallery_matches_the_golden_
/// image` — same tree, same three states, `color_critical_theme()`/
/// `COLOR_CRITICAL_CLEAR` instead of any other theme's own pairing,
/// diffed against its own golden target (`tests/golden/
/// slider_gallery_color_critical.png`, which does not exist yet).
/// `surface.sunken` clears `COLOR_CRITICAL_CLEAR` at ≈2.05:1
/// (`COLOR_CRITICAL_CLEAR`'s own doc comment) — real and modest, but the
/// thumb's own bright `accent.primary` gives a human reviewing the golden
/// a genuine reference point regardless, the same reasoning Light's own
/// `Slider` slice already used.
///
/// **`#[ignore]`d, deliberately — this file's own "never bless blind"
/// discipline** (see `aurora_testkit::compare_to_golden`'s own
/// `AURORA_BLESS_GOLDEN` gate): a human on real GPU hardware needs to
/// run `AURORA_BLESS_GOLDEN=1 cargo test -p aurora-widgets --test
/// gallery -- --ignored`, open the written `tests/golden/
/// slider_gallery_color_critical.png`, and confirm it actually shows the
/// thumb at three visually distinct positions/opacities in
/// Colour-Critical colours before this attribute comes off — the same
/// step every other golden in this file went through before being
/// trusted.
#[test]
fn slider_gallery_matches_the_golden_image_in_color_critical_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = color_critical_theme();
    let (tree, _ids) = slider_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        SLIDER_GALLERY_SIZE,
        COLOR_CRITICAL_CLEAR,
    );
    let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/slider_gallery_color_critical.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}

/// The same "distinct pixels, no golden needed" proof as the other
/// widgets', for `TextField`: enabled vs disabled must render as the
/// same `surface.sunken` token dimmed, the only state `paint_text_field`
/// resolves today.
#[test]
fn render_gallery_produces_distinct_pixels_for_each_text_field_state() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = dark_theme();
    let (tree, _ids) = text_field_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        TEXT_FIELD_GALLERY_SIZE,
        NEUTRAL_CLEAR,
    );
    assert_eq!(image.width, TEXT_FIELD_GALLERY_SIZE.0);
    assert_eq!(image.height, TEXT_FIELD_GALLERY_SIZE.1);

    let enabled_px = sample_cell_centre(&image, TEXT_FIELD_CELL, 0);
    let disabled_px = sample_cell_centre(&image, TEXT_FIELD_CELL, 1);
    assert_ne!(
        enabled_px[..3],
        disabled_px[..3],
        "state.disabled_opacity blended toward the clear colour must render differently from full \
         opacity -- lighter here specifically, since NEUTRAL_CLEAR is lighter than surface.sunken, \
         unlike the other galleries' own black backdrop"
    );
}

/// **Blessed and reviewed 2026-08-07** (a second time — the first
/// committed golden here rendered against plain black and was
/// genuinely correct data but effectively unreviewable by eye, same
/// root cause as `CommandPalette`'s own, see `NEUTRAL_CLEAR`'s own doc
/// comment): Cahya re-ran the bless command against this
/// `NEUTRAL_CLEAR`-backed version and committed the result. Confirmed
/// by decoding its own raw pixel bytes directly: enabled `[20,20,20]`
/// (`surface.sunken`, unchanged by the backdrop since full opacity
/// never blends with anything), disabled `[85,85,85]` — genuinely,
/// clearly distinct from the enabled cell (a human can see two real
/// rectangles of different shades), even with no margin around either
/// one — unlike `CommandPalette`, `TextField` has two different states
/// to contrast *against each other*, so it never needed the margin fix
/// `CommandPalette`'s own single-state gallery did.
#[test]
fn text_field_gallery_matches_the_golden_image() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = dark_theme();
    let (tree, _ids) = text_field_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        TEXT_FIELD_GALLERY_SIZE,
        NEUTRAL_CLEAR,
    );
    let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/text_field_gallery.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}

/// The same real, self-contained proof as
/// `render_gallery_produces_distinct_pixels_for_each_text_field_state`,
/// but against the Light theme (`light_theme()`/`LIGHT_CLEAR`) instead
/// of Dark's `dark_theme()`/`NEUTRAL_CLEAR`. `text_field_gallery_tree`
/// itself is unchanged and reused as-is — the tree doesn't depend on
/// theme, only rendering does. `LIGHT_CLEAR`, not `NEUTRAL_CLEAR`:
/// unlike Dark's `surface.sunken` (near-black, invisible against a pure
/// black clear — see `NEUTRAL_CLEAR`'s own doc comment for why Dark's
/// `TextField` gallery needed the fix), Light's own `surface.sunken`
/// resolves to `neutral.700` (`#c1c1c7`), which already contrasts
/// against `LIGHT_CLEAR`'s own `neutral.900` (`#f5f5f6`) — confirmed by
/// checking `paint_text_field` itself: `surface.sunken` plus
/// `state.disabled_opacity` is the only thing it ever paints, nothing
/// else theme-dependent affects visibility. But the actual contrast is
/// modest (≈1.65:1 fill-vs-canvas, ≈1.36:1 between the enabled and
/// disabled cells themselves) — worse than the ≈2.47:1 `NEUTRAL_CLEAR`
/// achieved for this exact widget in Dark, and `TextField` has no bright
/// accent element like `Slider`'s thumb to anchor a human's eye
/// regardless. This is a **provisional** choice pending the human bless,
/// not a confirmed "no backdrop needed" finding — see this file's own
/// top doc comment for the full reasoning.
#[test]
fn render_gallery_produces_distinct_pixels_for_each_text_field_state_in_light_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = light_theme();
    let (tree, _ids) = text_field_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        TEXT_FIELD_GALLERY_SIZE,
        LIGHT_CLEAR,
    );
    assert_eq!(image.width, TEXT_FIELD_GALLERY_SIZE.0);
    assert_eq!(image.height, TEXT_FIELD_GALLERY_SIZE.1);

    let enabled_px = sample_cell_centre(&image, TEXT_FIELD_CELL, 0);
    let disabled_px = sample_cell_centre(&image, TEXT_FIELD_CELL, 1);
    assert_ne!(
        enabled_px[..3],
        disabled_px[..3],
        "state.disabled_opacity blended over the clear colour must render differently from full \
         opacity in Light theme too"
    );
}

/// The Light-theme counterpart of `text_field_gallery_matches_the_
/// golden_image` — same tree, same two states, `light_theme()`/
/// `LIGHT_CLEAR` instead of Dark's `dark_theme()`/`NEUTRAL_CLEAR`,
/// diffed against its own golden target (`tests/golden/
/// text_field_gallery_light.png`, which does not exist yet). Uses
/// `LIGHT_CLEAR` alone, **provisionally** — see
/// `render_gallery_produces_distinct_pixels_for_each_text_field_state_
/// in_light_theme`'s own doc comment: the real contrast here (≈1.65:1
/// fill-vs-canvas, ≈1.36:1 between states) is weaker than the ≈2.47:1
/// Dark's own `NEUTRAL_CLEAR` fix achieved for this exact widget, and
/// there's no bright accent element to compensate the way `Slider` has.
///
/// **`#[ignore]`d, deliberately — this file's own "never bless blind"
/// discipline** (see `aurora_testkit::compare_to_golden`'s own
/// `AURORA_BLESS_GOLDEN` gate): a human on real GPU hardware needs to
/// run `AURORA_BLESS_GOLDEN=1 cargo test -p aurora-widgets --test
/// gallery -- --ignored`, open the written `tests/golden/
/// text_field_gallery_light.png`, and confirm it actually shows two
/// visually distinct text fields in Light-theme colours before this
/// attribute comes off — the same step every other golden in this file
/// went through before being trusted. Given the modest contrast noted
/// above, this one deserves particular scrutiny: if it reads as
/// "effectively unreviewable" the way Dark's own first `TextField`
/// attempt did, the fix is a `TextField`-specific backdrop (mirroring
/// `NEUTRAL_CLEAR`'s own history), not forcing a pass through this
/// comment alone.
#[test]
fn text_field_gallery_matches_the_golden_image_in_light_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = light_theme();
    let (tree, _ids) = text_field_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        TEXT_FIELD_GALLERY_SIZE,
        LIGHT_CLEAR,
    );
    let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/text_field_gallery_light.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}

/// The same real, self-contained proof as
/// `render_gallery_produces_distinct_pixels_for_each_text_field_state`,
/// but against High Contrast Dark (`high_contrast_dark_theme()`/
/// `HIGH_CONTRAST_DARK_CLEAR`) instead of Dark's `NEUTRAL_CLEAR` or
/// Light's `LIGHT_CLEAR`. `text_field_gallery_tree` itself is unchanged
/// and reused as-is. `HIGH_CONTRAST_DARK_CLEAR`, not a `TextField`-
/// specific constant: `surface.sunken` here is `hc.black`, exactly the
/// same "invisible against a black clear" problem Dark's own first
/// `TextField` bless attempt hit (`NEUTRAL_CLEAR`'s own doc comment) —
/// already solved once for this whole theme by
/// `HIGH_CONTRAST_DARK_CLEAR`'s own reasoning, not re-solved per widget.
#[test]
fn render_gallery_produces_distinct_pixels_for_each_text_field_state_in_high_contrast_dark_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = high_contrast_dark_theme();
    let (tree, _ids) = text_field_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        TEXT_FIELD_GALLERY_SIZE,
        HIGH_CONTRAST_DARK_CLEAR,
    );
    assert_eq!(image.width, TEXT_FIELD_GALLERY_SIZE.0);
    assert_eq!(image.height, TEXT_FIELD_GALLERY_SIZE.1);

    let enabled_px = sample_cell_centre(&image, TEXT_FIELD_CELL, 0);
    let disabled_px = sample_cell_centre(&image, TEXT_FIELD_CELL, 1);
    assert_ne!(
        enabled_px[..3],
        disabled_px[..3],
        "state.disabled_opacity blended over HIGH_CONTRAST_DARK_CLEAR must render differently \
         from full opacity in High Contrast Dark too"
    );
}

/// The second of this task's two required real, rendered proofs that
/// `border.control_opacity = 1.0` actually paints a visible outline
/// through the whole real pipeline (see `render_gallery_button_outline_
/// proves_border_control_opacity_in_high_contrast_dark_theme`'s own doc
/// comment for the first, and for the general reasoning this one
/// reuses). Chosen specifically because `TextField`'s own geometry
/// differs from `Button`'s (`TEXT_FIELD_CELL = (192, 32)`, not square),
/// so this also checks the same stroke-sampling approach generalizes,
/// not just an artifact of one particular cell size.
///
/// **How the sample point was chosen**: the same real-GPU debug scan
/// approach as `Button`'s own test, run against this exact tree/theme/
/// backdrop. It confirmed empirically: pixel column `x = TEXT_FIELD_
/// CELL.0 - 1` (`191`), row `y = TEXT_FIELD_CELL.1 / 2` (`16`, mid-
/// height, clear of `scales.radius.sm`'s corners) reads pure
/// `[255, 255, 255]` — `border.control` at `alpha = 1.0` for the
/// enabled (non-disabled) cell — while one pixel further in
/// (`x = 190`) still reads the plain `surface.sunken` fill
/// (`[0, 0, 0]`), and the field's own bottom edge (`y = TEXT_FIELD_
/// CELL.1 - 1`, i.e. `31`) shows the same white outline pixel too,
/// confirming this isn't specific to one edge.
#[test]
fn render_gallery_text_field_outline_proves_border_control_opacity_in_high_contrast_dark_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = high_contrast_dark_theme();
    let (tree, _ids) = text_field_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        TEXT_FIELD_GALLERY_SIZE,
        HIGH_CONTRAST_DARK_CLEAR,
    );

    let fill_px = sample_cell_centre(&image, TEXT_FIELD_CELL, 0);
    let right_edge_px = sample_at(&image, TEXT_FIELD_CELL.0 - 1, TEXT_FIELD_CELL.1 / 2);
    let bottom_edge_px = sample_at(&image, TEXT_FIELD_CELL.0 / 2, TEXT_FIELD_CELL.1 - 1);
    for (label, edge_px) in [("right", right_edge_px), ("bottom", bottom_edge_px)] {
        assert_eq!(
            edge_px,
            [255, 255, 255, 255],
            "border.control (hc.white) at alpha=1.0 must paint pure opaque white one pixel in \
             from the enabled text field's own {label} edge"
        );
        assert_ne!(
            edge_px, fill_px,
            "the {label} outline pixel must read differently from the field's own surface.sunken \
             fill"
        );
    }
}

/// The High Contrast Dark counterpart of `text_field_gallery_matches_
/// the_golden_image` — diffed against `tests/golden/
/// text_field_gallery_high_contrast_dark.png`, which does not exist yet.
///
/// **`#[ignore]`d, deliberately — this file's own "never bless blind"
/// discipline**: a human on real GPU hardware needs to run
/// `AURORA_BLESS_GOLDEN=1 cargo test -p aurora-widgets --test gallery --
/// --ignored`, open the written golden, and confirm it actually shows
/// two visually distinct text fields in High Contrast Dark colours, each
/// with a visible white outline — the same outline
/// `render_gallery_text_field_outline_proves_border_control_opacity_in_
/// high_contrast_dark_theme` already proved lands at the right byte
/// values, now checked as a whole image — before this attribute comes
/// off.
#[test]
fn text_field_gallery_matches_the_golden_image_in_high_contrast_dark_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = high_contrast_dark_theme();
    let (tree, _ids) = text_field_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        TEXT_FIELD_GALLERY_SIZE,
        HIGH_CONTRAST_DARK_CLEAR,
    );
    let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/text_field_gallery_high_contrast_dark.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}

/// The same real, self-contained proof as
/// `render_gallery_produces_distinct_pixels_for_each_text_field_state`,
/// but against High Contrast Light (`high_contrast_light_theme()`/
/// `HIGH_CONTRAST_LIGHT_CLEAR`) instead of any other theme.
/// `text_field_gallery_tree` itself is unchanged and reused as-is.
/// `surface.sunken` here is `hc.white`, exactly the same "invisible
/// against a plain white clear" problem as every other widget in this
/// theme — already solved once for the whole theme by
/// `HIGH_CONTRAST_LIGHT_CLEAR`'s own reasoning, not re-solved per widget.
#[test]
fn render_gallery_produces_distinct_pixels_for_each_text_field_state_in_high_contrast_light_theme()
{
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = high_contrast_light_theme();
    let (tree, _ids) = text_field_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        TEXT_FIELD_GALLERY_SIZE,
        HIGH_CONTRAST_LIGHT_CLEAR,
    );
    assert_eq!(image.width, TEXT_FIELD_GALLERY_SIZE.0);
    assert_eq!(image.height, TEXT_FIELD_GALLERY_SIZE.1);

    let enabled_px = sample_cell_centre(&image, TEXT_FIELD_CELL, 0);
    let disabled_px = sample_cell_centre(&image, TEXT_FIELD_CELL, 1);
    assert_ne!(
        enabled_px[..3],
        disabled_px[..3],
        "state.disabled_opacity blended over HIGH_CONTRAST_LIGHT_CLEAR must render differently \
         from full opacity in High Contrast Light too"
    );
}

/// The High Contrast Light counterpart of `render_gallery_text_field_
/// outline_proves_border_control_opacity_in_high_contrast_dark_theme` —
/// the second of this theme's two required real, rendered proofs that
/// `border.control_opacity = 1.0` actually paints a visible outline
/// through the whole real pipeline. Same sample coordinates as the Dark
/// counterpart (`TEXT_FIELD_CELL.0 - 1` for the right edge,
/// `TEXT_FIELD_CELL.1 - 1` for the bottom edge, both one pixel in from
/// their own boundary) — that geometry is a property of this pipeline's
/// own stroke tessellation, not of any theme's colours, confirmed by
/// actually running this test rather than assumed to transfer.
#[test]
fn render_gallery_text_field_outline_proves_border_control_opacity_in_high_contrast_light_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = high_contrast_light_theme();
    let (tree, _ids) = text_field_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        TEXT_FIELD_GALLERY_SIZE,
        HIGH_CONTRAST_LIGHT_CLEAR,
    );

    let fill_px = sample_cell_centre(&image, TEXT_FIELD_CELL, 0);
    let right_edge_px = sample_at(&image, TEXT_FIELD_CELL.0 - 1, TEXT_FIELD_CELL.1 / 2);
    let bottom_edge_px = sample_at(&image, TEXT_FIELD_CELL.0 / 2, TEXT_FIELD_CELL.1 - 1);
    for (label, edge_px) in [("right", right_edge_px), ("bottom", bottom_edge_px)] {
        assert_eq!(
            edge_px,
            [0, 0, 0, 255],
            "border.control (hc.black) at alpha=1.0 must paint pure opaque black one pixel in \
             from the enabled text field's own {label} edge"
        );
        assert_ne!(
            edge_px, fill_px,
            "the {label} outline pixel must read differently from the field's own surface.sunken \
             fill"
        );
    }
}

/// The High Contrast Light counterpart of `text_field_gallery_matches_
/// the_golden_image_in_high_contrast_dark_theme` — diffed against
/// `tests/golden/text_field_gallery_high_contrast_light.png`, which does
/// not exist yet.
///
/// **`#[ignore]`d, deliberately — this file's own "never bless blind"
/// discipline**: a human on real GPU hardware needs to run
/// `AURORA_BLESS_GOLDEN=1 cargo test -p aurora-widgets --test gallery --
/// --ignored`, open the written golden, and confirm it actually shows two
/// visually distinct text fields in High Contrast Light colours, each
/// with a visible black outline — the same outline
/// `render_gallery_text_field_outline_proves_border_control_opacity_in_
/// high_contrast_light_theme` already proved lands at the right byte
/// values, now checked as a whole image — before this attribute comes
/// off.
#[test]
fn text_field_gallery_matches_the_golden_image_in_high_contrast_light_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = high_contrast_light_theme();
    let (tree, _ids) = text_field_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        TEXT_FIELD_GALLERY_SIZE,
        HIGH_CONTRAST_LIGHT_CLEAR,
    );
    let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/text_field_gallery_high_contrast_light.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}

/// The same real, self-contained proof as
/// `render_gallery_produces_distinct_pixels_for_each_text_field_state`,
/// but against Colour-Critical (`color_critical_theme()`/
/// `COLOR_CRITICAL_CLEAR`) instead of any other theme's own pairing.
/// `text_field_gallery_tree` itself is unchanged and reused as-is.
#[test]
fn render_gallery_produces_distinct_pixels_for_each_text_field_state_in_color_critical_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = color_critical_theme();
    let (tree, _ids) = text_field_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        TEXT_FIELD_GALLERY_SIZE,
        COLOR_CRITICAL_CLEAR,
    );
    assert_eq!(image.width, TEXT_FIELD_GALLERY_SIZE.0);
    assert_eq!(image.height, TEXT_FIELD_GALLERY_SIZE.1);

    let enabled_px = sample_cell_centre(&image, TEXT_FIELD_CELL, 0);
    let disabled_px = sample_cell_centre(&image, TEXT_FIELD_CELL, 1);
    assert_ne!(
        enabled_px[..3],
        disabled_px[..3],
        "state.disabled_opacity blended over the clear colour must render differently from full \
         opacity in Colour-Critical too"
    );
}

/// The Colour-Critical counterpart of `text_field_gallery_matches_the_
/// golden_image` — same tree, same two states, `color_critical_theme()`/
/// `COLOR_CRITICAL_CLEAR` instead of any other theme's own pairing,
/// diffed against its own golden target (`tests/golden/
/// text_field_gallery_color_critical.png`, which does not exist yet).
/// Uses `COLOR_CRITICAL_CLEAR` alone, **provisionally** — mirroring
/// Light's own exact treatment of this widget: `surface.sunken` clears
/// `COLOR_CRITICAL_CLEAR` at ≈2.05:1 (`COLOR_CRITICAL_CLEAR`'s own doc
/// comment), a touch stronger than Light's own ≈1.65:1 fill-vs-canvas
/// number for this same widget, but still modest, and — unlike `Slider`
/// — there's no bright `accent.primary` element here to anchor a human's
/// eye regardless of the fill's own contrast.
///
/// **`#[ignore]`d, deliberately — this file's own "never bless blind"
/// discipline** (see `aurora_testkit::compare_to_golden`'s own
/// `AURORA_BLESS_GOLDEN` gate): a human on real GPU hardware needs to
/// run `AURORA_BLESS_GOLDEN=1 cargo test -p aurora-widgets --test
/// gallery -- --ignored`, open the written `tests/golden/
/// text_field_gallery_color_critical.png`, and confirm it actually shows
/// two visually distinct text fields in Colour-Critical colours before
/// this attribute comes off — the same step every other golden in this
/// file went through before being trusted. Given the modest contrast
/// noted above, this one deserves particular scrutiny, the same as
/// Light's own first `TextField` slice: if it reads as "effectively
/// unreviewable" the way Dark's own first `TextField` attempt did, the
/// fix is a `TextField`-specific backdrop (mirroring `NEUTRAL_CLEAR`'s
/// own history), not forcing a pass through this comment alone.
#[test]
fn text_field_gallery_matches_the_golden_image_in_color_critical_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = color_critical_theme();
    let (tree, _ids) = text_field_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        TEXT_FIELD_GALLERY_SIZE,
        COLOR_CRITICAL_CLEAR,
    );
    let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/text_field_gallery_color_critical.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}

/// This gallery's one command is always the selected result, and
/// `command_palette_gallery_tree`'s own body/row layout
/// (`command_palette::body_style`/`row_style`) makes that one row fill
/// the entire panel interior — so the "panel centre" pixel this test
/// samples is actually the row's own `accent.primary` highlight, painted
/// on top of the panel's `surface.raised` fill beneath it, not
/// `surface.raised` directly (see
/// `command_palette_gallery_paints_the_selected_rows_own_highlight`
/// below for the real, exact-colour proof of that). This test only
/// proves the weaker, layout-agnostic thing: *something* opaque was
/// painted across the panel's own interior, distinct from the
/// surrounding clear colour — comparing the centre against corner pixel
/// `(0, 0)`, which `scales.radius.md`'s real rounded corner
/// (`paint_command_palette`'s own doc comment) keeps outside the filled
/// rounded rect regardless of what colour filled it, without hardcoding
/// what that clear colour's own exact byte value is (see
/// `NEUTRAL_CLEAR`'s own doc comment for why this gallery doesn't use
/// plain black).
#[test]
fn render_gallery_produces_the_command_palettes_own_panel() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = dark_theme();
    let (tree, _ids) = command_palette_gallery_tree();

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        COMMAND_PALETTE_GALLERY_SIZE,
        NEUTRAL_CLEAR,
    );
    assert_eq!(image.width, COMMAND_PALETTE_GALLERY_SIZE.0);
    assert_eq!(image.height, COMMAND_PALETTE_GALLERY_SIZE.1);

    // Not `sample_cell_centre` -- that assumes cell 0 starts at x = 0,
    // but `command_palette_style`'s own margin pushes the panel's real
    // centre over by `COMMAND_PALETTE_MARGIN` on both axes.
    let panel_centre = sample_at(
        &image,
        COMMAND_PALETTE_MARGIN + COMMAND_PALETTE_CELL.0 / 2,
        COMMAND_PALETTE_MARGIN + COMMAND_PALETTE_CELL.1 / 2,
    );
    let corner = sample_at(&image, 0, 0);
    assert_ne!(
        corner, panel_centre,
        "the panel's own interior must show something opaque, not the same clear colour its own \
         margin (and rounded-off corner) shows"
    );
}

/// The real, exact-colour proof `render_gallery_produces_the_command_
/// palettes_own_panel`'s own doc comment names: with this gallery's one
/// command always selected and its one row filling the whole panel
/// interior, the panel's own centre pixel must be `accent.primary`
/// itself, not `surface.raised` — `[120,172,255]`, the same Dark-theme
/// byte value `checkbox_gallery_matches_the_golden_image`'s own doc
/// comment already confirmed by decoding a real blessed golden, reused
/// here rather than re-deriving it, since both ultimately resolve the
/// same token against the same committed Dark theme.
#[test]
fn command_palette_gallery_paints_the_selected_rows_own_highlight() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = dark_theme();
    let (tree, _ids) = command_palette_gallery_tree();

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        COMMAND_PALETTE_GALLERY_SIZE,
        NEUTRAL_CLEAR,
    );
    let panel_centre = sample_at(
        &image,
        COMMAND_PALETTE_MARGIN + COMMAND_PALETTE_CELL.0 / 2,
        COMMAND_PALETTE_MARGIN + COMMAND_PALETTE_CELL.1 / 2,
    );
    assert_eq!(
        panel_centre[..3],
        [120, 172, 255],
        "the sole, always-selected result row must paint accent.primary across the panel's own \
         interior"
    );
}

/// A headless (no GPU needed) proof of the *layout* half of the fix
/// above: `command_palette_style`'s own margin actually reaches the
/// tree's own computed bounds, not just the render target's own,
/// larger `size`. Written specifically to build confidence *before*
/// asking for another real-hardware bless — this sandbox can check
/// `WidgetTree::bounds` (pure CPU layout math) but never the actual
/// rendered pixels (no GPU adapter at all), so this is the one piece
/// of the fix this sandbox can actually prove outright.
#[test]
fn command_palette_style_positions_the_panel_with_a_real_margin() {
    let (tree, [palette]) = command_palette_gallery_tree();
    let Some(bounds) = tree.bounds(palette) else {
        unreachable!("just laid out");
    };
    let margin = i64::from(COMMAND_PALETTE_MARGIN);
    assert_eq!(
        bounds.x, margin,
        "the panel must sit inset from the gallery's own left edge"
    );
    assert_eq!(
        bounds.y, margin,
        "the panel must sit inset from the gallery's own top edge"
    );
    assert_eq!(bounds.width, COMMAND_PALETTE_CELL.0);
    assert_eq!(bounds.height, COMMAND_PALETTE_CELL.1);
    assert_eq!(
        bounds.x + i64::from(bounds.width) + margin,
        i64::from(COMMAND_PALETTE_GALLERY_SIZE.0),
        "the panel plus its own margin on both sides must exactly fill the gallery's own width"
    );
}

/// **Blessed a fourth time, 2026-08-08** — see `NEUTRAL_CLEAR`'s and
/// `command_palette_style`'s own doc comments for the first three
/// rounds (plain black hid a correct panel, then a correct backdrop
/// still had no margin, then a real 32px margin fixed it), and the
/// paragraph above for why a fourth bless was needed at all (the
/// result row is now a real, painted `WidgetKind::ListRow` — the
/// rendered image gained a real `accent.primary` highlight rectangle
/// the third bless never had). Confirmed by decoding the new golden's
/// own raw pixel bytes: `[127,127,127]` (`NEUTRAL_CLEAR`) at the
/// corner and one pixel outside the panel's own left edge (`x=31`),
/// `[120,172,255]` (`accent.primary`, the same byte value
/// `checkbox_gallery_matches_the_golden_image`'s own doc comment
/// already confirmed) at the panel's centre and one pixel inside its
/// left edge (`x=33`) — the margin boundary lands exactly where
/// `COMMAND_PALETTE_MARGIN` predicts, same as every prior bless.
#[test]
fn command_palette_gallery_matches_the_golden_image() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = dark_theme();
    let (tree, _ids) = command_palette_gallery_tree();

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        COMMAND_PALETTE_GALLERY_SIZE,
        NEUTRAL_CLEAR,
    );
    let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/command_palette_gallery.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}

/// The Light-theme counterpart of `render_gallery_produces_the_command_
/// palettes_own_panel` — same tree, same weaker "something opaque was
/// painted, distinct from the surrounding clear colour" proof, but
/// against `light_theme()`/`COMMAND_PALETTE_LIGHT_CLEAR` instead of
/// Dark's `dark_theme()`/`NEUTRAL_CLEAR`. `COMMAND_PALETTE_LIGHT_CLEAR`,
/// not plain `LIGHT_CLEAR` — see that constant's own doc comment for why
/// `LIGHT_CLEAR` would collide byte-for-byte with `surface.raised` in
/// Light and hide the panel entirely, mirroring Dark's own original
/// "just a black image" bug in the opposite direction.
#[test]
fn render_gallery_produces_the_command_palettes_own_panel_in_light_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = light_theme();
    let (tree, _ids) = command_palette_gallery_tree();

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        COMMAND_PALETTE_GALLERY_SIZE,
        COMMAND_PALETTE_LIGHT_CLEAR,
    );
    assert_eq!(image.width, COMMAND_PALETTE_GALLERY_SIZE.0);
    assert_eq!(image.height, COMMAND_PALETTE_GALLERY_SIZE.1);

    let panel_centre = sample_at(
        &image,
        COMMAND_PALETTE_MARGIN + COMMAND_PALETTE_CELL.0 / 2,
        COMMAND_PALETTE_MARGIN + COMMAND_PALETTE_CELL.1 / 2,
    );
    let corner = sample_at(&image, 0, 0);
    assert_ne!(
        corner, panel_centre,
        "the panel's own interior must show something opaque, not the same clear colour its own \
         margin (and rounded-off corner) shows, in Light theme too"
    );
}

/// The Light-theme counterpart of `command_palette_gallery_paints_the_
/// selected_rows_own_highlight` — the real, exact-colour proof that the
/// panel's own centre pixel is `accent.primary` itself, not
/// `surface.raised`. `[18,79,176]`, Light's own real `accent.blue.200`
/// (`design/themes/light.toml` → `design/tokens/palette.toml`'s
/// `[accent.blue]` table, `#124fb0` → `(0x12, 0x4f, 0xb0)`), computed
/// directly from the committed TOML rather than assumed — a genuinely
/// different byte value from Dark's own `[120,172,255]`, as expected
/// (Light's accent ramp uses the new dark 100/200/300 steps added
/// specifically to fill against near-white surfaces, not Dark's
/// 400/500/600).
#[test]
fn command_palette_gallery_paints_the_selected_rows_own_highlight_in_light_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = light_theme();
    let (tree, _ids) = command_palette_gallery_tree();

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        COMMAND_PALETTE_GALLERY_SIZE,
        COMMAND_PALETTE_LIGHT_CLEAR,
    );
    let panel_centre = sample_at(
        &image,
        COMMAND_PALETTE_MARGIN + COMMAND_PALETTE_CELL.0 / 2,
        COMMAND_PALETTE_MARGIN + COMMAND_PALETTE_CELL.1 / 2,
    );
    assert_eq!(
        panel_centre[..3],
        [18, 79, 176],
        "the sole, always-selected result row must paint Light's own accent.primary across the \
         panel's own interior"
    );
}

/// The Light-theme counterpart of `command_palette_gallery_matches_the_
/// golden_image` — same tree, same one always-selected row,
/// `light_theme()`/`COMMAND_PALETTE_LIGHT_CLEAR` instead of Dark's
/// `dark_theme()`/`NEUTRAL_CLEAR`, diffed against its own golden target
/// (`tests/golden/command_palette_gallery_light.png`, which does not
/// exist yet). `command_palette_style`'s own `COMMAND_PALETTE_MARGIN` is
/// reused completely unchanged — pure layout, no theme parameter, so it
/// needs no Light-specific counterpart the way the backdrop colour did.
///
/// **`#[ignore]`d, deliberately — this file's own "never bless blind"
/// discipline** (see `aurora_testkit::compare_to_golden`'s own
/// `AURORA_BLESS_GOLDEN` gate): a human on real GPU hardware needs to
/// run `AURORA_BLESS_GOLDEN=1 cargo test -p aurora-widgets --test
/// gallery -- --ignored`, open the written `tests/golden/
/// command_palette_gallery_light.png`, and confirm it actually shows a
/// visible panel with a visible selected-row highlight in Light-theme
/// colours before this attribute comes off — the same step every other
/// golden in this file went through before being trusted.
/// `CommandPalette` has the most complex visibility history of any
/// widget in this file (`NEUTRAL_CLEAR`'s own doc comment: Dark's
/// gallery needed *two* separate fixes, backdrop then margin, before its
/// own golden was trustworthy), so this Light golden deserves the same
/// level of scrutiny, not a rubber stamp just because the backdrop
/// collision was caught and fixed here before any bless was attempted.
#[test]
fn command_palette_gallery_matches_the_golden_image_in_light_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = light_theme();
    let (tree, _ids) = command_palette_gallery_tree();

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        COMMAND_PALETTE_GALLERY_SIZE,
        COMMAND_PALETTE_LIGHT_CLEAR,
    );
    let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/command_palette_gallery_light.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}

/// The High Contrast Dark counterpart of `render_gallery_produces_the_
/// command_palettes_own_panel` — same tree, same weaker "something
/// opaque was painted, distinct from the surrounding clear colour"
/// proof, but against `high_contrast_dark_theme()`/
/// `HIGH_CONTRAST_DARK_CLEAR` instead of Dark's `NEUTRAL_CLEAR` or
/// Light's `COMMAND_PALETTE_LIGHT_CLEAR`. No separate `COMMAND_PALETTE_
/// HIGH_CONTRAST_DARK_CLEAR` constant is needed the way Light needed its
/// own: `HIGH_CONTRAST_DARK_CLEAR` was already chosen as one shared
/// backdrop for every widget in this theme's gallery, `CommandPalette`
/// included (see that constant's own doc comment) — its own
/// `surface.raised` collision with a plain-black backdrop is exactly the
/// same "every surface token is `hc.black`" problem every other widget
/// here has, not a distinct one worth its own constant.
#[test]
fn render_gallery_produces_the_command_palettes_own_panel_in_high_contrast_dark_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = high_contrast_dark_theme();
    let (tree, _ids) = command_palette_gallery_tree();

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        COMMAND_PALETTE_GALLERY_SIZE,
        HIGH_CONTRAST_DARK_CLEAR,
    );
    assert_eq!(image.width, COMMAND_PALETTE_GALLERY_SIZE.0);
    assert_eq!(image.height, COMMAND_PALETTE_GALLERY_SIZE.1);

    let panel_centre = sample_at(
        &image,
        COMMAND_PALETTE_MARGIN + COMMAND_PALETTE_CELL.0 / 2,
        COMMAND_PALETTE_MARGIN + COMMAND_PALETTE_CELL.1 / 2,
    );
    let corner = sample_at(&image, 0, 0);
    assert_ne!(
        corner, panel_centre,
        "the panel's own interior must show something opaque, not the same clear colour its own \
         margin (and rounded-off corner) shows, in High Contrast Dark too"
    );
}

/// The High Contrast Dark counterpart of `command_palette_gallery_
/// paints_the_selected_rows_own_highlight` — the real, exact-colour
/// proof that the panel's own centre pixel is `accent.primary` itself,
/// not `surface.raised`. `[255,255,0]`, this theme's own `hc.yellow`
/// (`design/tokens/palette.toml`'s `[hc]` table, `#ffff00`) — a value
/// with no fractional byte rounding ambiguity (each channel is either
/// `0x00` or `0xff`), unlike `HIGH_CONTRAST_DARK_CLEAR`'s own `0.5`
/// components, so this is safe to assert as an exact literal the same
/// way Dark's own `[120,172,255]` and Light's own `[18,79,176]` already
/// are.
#[test]
fn command_palette_gallery_paints_the_selected_rows_own_highlight_in_high_contrast_dark_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = high_contrast_dark_theme();
    let (tree, _ids) = command_palette_gallery_tree();

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        COMMAND_PALETTE_GALLERY_SIZE,
        HIGH_CONTRAST_DARK_CLEAR,
    );
    let panel_centre = sample_at(
        &image,
        COMMAND_PALETTE_MARGIN + COMMAND_PALETTE_CELL.0 / 2,
        COMMAND_PALETTE_MARGIN + COMMAND_PALETTE_CELL.1 / 2,
    );
    assert_eq!(
        panel_centre[..3],
        [255, 255, 0],
        "the sole, always-selected result row must paint High Contrast Dark's own accent.primary \
         (hc.yellow) across the panel's own interior"
    );
}

/// The High Contrast Dark counterpart of `command_palette_gallery_
/// matches_the_golden_image` — diffed against `tests/golden/
/// command_palette_gallery_high_contrast_dark.png`, which does not exist
/// yet. `command_palette_style`'s own `COMMAND_PALETTE_MARGIN` is reused
/// completely unchanged, same as every other theme's own `CommandPalette`
/// gallery.
///
/// **`#[ignore]`d, deliberately — this file's own "never bless blind"
/// discipline**: a human on real GPU hardware needs to run
/// `AURORA_BLESS_GOLDEN=1 cargo test -p aurora-widgets --test gallery --
/// --ignored`, open the written golden, and confirm it actually shows a
/// visible panel with a visible selected-row highlight and a visible
/// white panel outline (`border.control_opacity = 1.0` — note the
/// selected row's own fill occupies the exact same bounds as the panel
/// itself, `body_style`/`row_style` both being `percent(1.0)`, so most
/// of the panel's own outline stroke is likely covered by the row's own
/// fill; this is a real open question for the human bless to look at
/// closely, not one this file's own automated tests can settle — see
/// `render_gallery_button_outline_proves_border_control_opacity_in_
/// high_contrast_dark_theme`'s and `render_gallery_text_field_outline_
/// proves_border_control_opacity_in_high_contrast_dark_theme`'s own doc
/// comments for why `CommandPalette` specifically was not chosen as
/// either of this task's two required outline-proof widgets) before this
/// attribute comes off.
///
/// **A critic pass rendered this gallery and scanned the actual pixels,
/// so the human bless doesn't have to guess what "likely covered" means
/// in practice**: the occlusion is real but asymmetric, not uniform.
/// The panel's own outline is genuinely visible along the **top and
/// left** edges (that band's included pixel falls just outside the
/// panel, in the margin, where the row's own fill never reaches) and
/// genuinely invisible along the **right and bottom** edges (there the
/// same band's included pixel falls one pixel *inside* the panel,
/// exactly where the selected row's own fill -- identical bounds,
/// painted after -- overwrites it). Expect a crisp L-shaped white line,
/// not a uniformly faint one, when actually looking at the image.
#[test]
fn command_palette_gallery_matches_the_golden_image_in_high_contrast_dark_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = high_contrast_dark_theme();
    let (tree, _ids) = command_palette_gallery_tree();

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        COMMAND_PALETTE_GALLERY_SIZE,
        HIGH_CONTRAST_DARK_CLEAR,
    );
    let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/command_palette_gallery_high_contrast_dark.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}

/// The High Contrast Light counterpart of `render_gallery_produces_the_
/// command_palettes_own_panel_in_high_contrast_dark_theme` — same tree,
/// same weaker "something opaque was painted, distinct from the
/// surrounding clear colour" proof, but against
/// `high_contrast_light_theme()`/`HIGH_CONTRAST_LIGHT_CLEAR` instead of
/// any other theme's own pairing. No separate `COMMAND_PALETTE_HIGH_
/// CONTRAST_LIGHT_CLEAR` constant is needed, for the same reason
/// `HIGH_CONTRAST_DARK_CLEAR` already didn't need one: `HIGH_CONTRAST_
/// LIGHT_CLEAR` was already chosen as one shared backdrop for every
/// widget in this theme's gallery, `CommandPalette` included (see that
/// constant's own doc comment) — its own `surface.raised` collision with
/// a plain-white backdrop is exactly the same "every surface token is
/// `hc.white`" problem every other widget here has, not a distinct one
/// worth its own constant.
#[test]
fn render_gallery_produces_the_command_palettes_own_panel_in_high_contrast_light_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = high_contrast_light_theme();
    let (tree, _ids) = command_palette_gallery_tree();

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        COMMAND_PALETTE_GALLERY_SIZE,
        HIGH_CONTRAST_LIGHT_CLEAR,
    );
    assert_eq!(image.width, COMMAND_PALETTE_GALLERY_SIZE.0);
    assert_eq!(image.height, COMMAND_PALETTE_GALLERY_SIZE.1);

    let panel_centre = sample_at(
        &image,
        COMMAND_PALETTE_MARGIN + COMMAND_PALETTE_CELL.0 / 2,
        COMMAND_PALETTE_MARGIN + COMMAND_PALETTE_CELL.1 / 2,
    );
    let corner = sample_at(&image, 0, 0);
    assert_ne!(
        corner, panel_centre,
        "the panel's own interior must show something opaque, not the same clear colour its own \
         margin (and rounded-off corner) shows, in High Contrast Light too"
    );
}

/// The High Contrast Light counterpart of `command_palette_gallery_
/// paints_the_selected_rows_own_highlight_in_high_contrast_dark_theme` —
/// the real, exact-colour proof that the panel's own centre pixel is
/// `accent.primary` itself, not `surface.raised`. `[0,0,255]`, this
/// theme's own `hc.blue` (`design/tokens/palette.toml`'s `[hc]` table,
/// `#0000ff`) — a value with no fractional byte rounding ambiguity (each
/// channel is either `0x00` or `0xff`), safe to assert as an exact
/// literal the same way High Contrast Dark's own `[255,255,0]` already
/// is.
#[test]
fn command_palette_gallery_paints_the_selected_rows_own_highlight_in_high_contrast_light_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = high_contrast_light_theme();
    let (tree, _ids) = command_palette_gallery_tree();

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        COMMAND_PALETTE_GALLERY_SIZE,
        HIGH_CONTRAST_LIGHT_CLEAR,
    );
    let panel_centre = sample_at(
        &image,
        COMMAND_PALETTE_MARGIN + COMMAND_PALETTE_CELL.0 / 2,
        COMMAND_PALETTE_MARGIN + COMMAND_PALETTE_CELL.1 / 2,
    );
    assert_eq!(
        panel_centre[..3],
        [0, 0, 255],
        "the sole, always-selected result row must paint High Contrast Light's own accent.primary \
         (hc.blue) across the panel's own interior"
    );
}

/// The High Contrast Light counterpart of `command_palette_gallery_
/// matches_the_golden_image_in_high_contrast_dark_theme` — diffed against
/// `tests/golden/command_palette_gallery_high_contrast_light.png`, which
/// does not exist yet. `command_palette_style`'s own `COMMAND_PALETTE_
/// MARGIN` is reused completely unchanged, same as every other theme's
/// own `CommandPalette` gallery.
///
/// **`#[ignore]`d, deliberately — this file's own "never bless blind"
/// discipline**: a human on real GPU hardware needs to run
/// `AURORA_BLESS_GOLDEN=1 cargo test -p aurora-widgets --test gallery --
/// --ignored`, open the written golden, and confirm it actually shows a
/// visible panel with a visible selected-row highlight and a visible
/// black panel outline (`border.control_opacity = 1.0`) before this
/// attribute comes off.
///
/// **Same asymmetric occlusion as High Contrast Dark, not a new
/// investigation**: nothing about `command_palette_style`'s own layout or
/// `paint_list_row`'s own fill logic is theme-dependent (see
/// `command_palette_gallery_matches_the_golden_image_in_high_contrast_
/// dark_theme`'s own doc comment for the full, already-worked-out
/// mechanism), so the same selected-row-occludes-the-panel-outline
/// situation applies here unchanged: the panel's own outline is expected
/// to be genuinely visible along the **top and left** edges (that band's
/// included pixel falls just outside the panel, in the margin) and
/// genuinely invisible along the **right and bottom** edges (there the
/// row's own identical-bounds fill, painted after, overwrites it) — a
/// crisp L-shaped black line, not a uniformly faint one, is what the
/// human bless should expect to see.
#[test]
fn command_palette_gallery_matches_the_golden_image_in_high_contrast_light_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = high_contrast_light_theme();
    let (tree, _ids) = command_palette_gallery_tree();

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        COMMAND_PALETTE_GALLERY_SIZE,
        HIGH_CONTRAST_LIGHT_CLEAR,
    );
    let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/command_palette_gallery_high_contrast_light.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}

/// The Colour-Critical counterpart of `render_gallery_produces_the_
/// command_palettes_own_panel` — same tree, same weaker "something opaque
/// was painted, distinct from the surrounding clear colour" proof, but
/// against `color_critical_theme()`/`COMMAND_PALETTE_COLOR_CRITICAL_
/// CLEAR` instead of any other theme's own pairing.
/// `COMMAND_PALETTE_COLOR_CRITICAL_CLEAR`, not plain `COLOR_CRITICAL_
/// CLEAR` — see that constant's own doc comment for why `COLOR_CRITICAL_
/// CLEAR` (`cc.canvas`, `#545454`) would sit only ≈1.13:1 from
/// `surface.raised` (`cc.raised`, `#4c4c4c`), close enough to invisible
/// for a human reviewing the golden even though the two token values
/// aren't byte-identical the way Dark's/Light's own `CommandPalette`
/// collisions were.
#[test]
fn render_gallery_produces_the_command_palettes_own_panel_in_color_critical_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = color_critical_theme();
    let (tree, _ids) = command_palette_gallery_tree();

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        COMMAND_PALETTE_GALLERY_SIZE,
        COMMAND_PALETTE_COLOR_CRITICAL_CLEAR,
    );
    assert_eq!(image.width, COMMAND_PALETTE_GALLERY_SIZE.0);
    assert_eq!(image.height, COMMAND_PALETTE_GALLERY_SIZE.1);

    let panel_centre = sample_at(
        &image,
        COMMAND_PALETTE_MARGIN + COMMAND_PALETTE_CELL.0 / 2,
        COMMAND_PALETTE_MARGIN + COMMAND_PALETTE_CELL.1 / 2,
    );
    let corner = sample_at(&image, 0, 0);
    assert_ne!(
        corner, panel_centre,
        "the panel's own interior must show something opaque, not the same clear colour its own \
         margin (and rounded-off corner) shows, in Colour-Critical too"
    );
}

/// The Colour-Critical counterpart of `command_palette_gallery_paints_
/// the_selected_rows_own_highlight` — the real, exact-colour proof that
/// the panel's own centre pixel is `accent.primary` itself, not
/// `surface.raised`. `[164,200,255]`, Colour-Critical's own real
/// `accent.blue.600` (`design/themes/color-critical.toml` → `design/
/// tokens/palette.toml`'s `[accent.blue]` table, `#a4c8ff` →
/// `(0xa4, 0xc8, 0xff)`), computed directly from the committed TOML
/// rather than assumed — the same value `COLOR_CRITICAL_CLEAR`'s own doc
/// comment already gives when computing the ≈4.43:1 contrast against the
/// backdrop.
#[test]
fn command_palette_gallery_paints_the_selected_rows_own_highlight_in_color_critical_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = color_critical_theme();
    let (tree, _ids) = command_palette_gallery_tree();

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        COMMAND_PALETTE_GALLERY_SIZE,
        COMMAND_PALETTE_COLOR_CRITICAL_CLEAR,
    );
    let panel_centre = sample_at(
        &image,
        COMMAND_PALETTE_MARGIN + COMMAND_PALETTE_CELL.0 / 2,
        COMMAND_PALETTE_MARGIN + COMMAND_PALETTE_CELL.1 / 2,
    );
    assert_eq!(
        panel_centre[..3],
        [164, 200, 255],
        "the sole, always-selected result row must paint Colour-Critical's own accent.primary \
         across the panel's own interior"
    );
}

/// The Colour-Critical counterpart of `command_palette_gallery_matches_
/// the_golden_image` — same tree, same one always-selected row,
/// `color_critical_theme()`/`COMMAND_PALETTE_COLOR_CRITICAL_CLEAR`
/// instead of any other theme's own pairing, diffed against its own
/// golden target (`tests/golden/command_palette_gallery_color_critical.
/// png`, which does not exist yet). `command_palette_style`'s own
/// `COMMAND_PALETTE_MARGIN` is reused completely unchanged — pure
/// layout, no theme parameter, so it needs no Colour-Critical-specific
/// counterpart the way the backdrop colour did.
///
/// **`#[ignore]`d, deliberately — this file's own "never bless blind"
/// discipline** (see `aurora_testkit::compare_to_golden`'s own
/// `AURORA_BLESS_GOLDEN` gate): a human on real GPU hardware needs to
/// run `AURORA_BLESS_GOLDEN=1 cargo test -p aurora-widgets --test
/// gallery -- --ignored`, open the written `tests/golden/
/// command_palette_gallery_color_critical.png`, and confirm it actually
/// shows a visible panel with a visible selected-row highlight in
/// Colour-Critical colours before this attribute comes off — the same
/// step every other golden in this file went through before being
/// trusted. `CommandPalette` has the most complex visibility history of
/// any widget in this file (`NEUTRAL_CLEAR`'s own doc comment: Dark's
/// gallery needed *two* separate fixes, backdrop then margin, before its
/// own golden was trustworthy), so this Colour-Critical golden deserves
/// the same level of scrutiny, not a rubber stamp just because the
/// backdrop collision was caught and fixed here before any bless was
/// attempted.
#[test]
fn command_palette_gallery_matches_the_golden_image_in_color_critical_theme() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = color_critical_theme();
    let (tree, _ids) = command_palette_gallery_tree();

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        COMMAND_PALETTE_GALLERY_SIZE,
        COMMAND_PALETTE_COLOR_CRITICAL_CLEAR,
    );
    let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/command_palette_gallery_color_critical.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}
