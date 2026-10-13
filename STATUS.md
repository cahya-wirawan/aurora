# Aurora status

**As of 2026-10-13, version 0.221.0.** (0.221.0: the Clone Stamp tool exists (`S`), aligned mode only: `Alt`+click sets a source on the active pixel layer, a plain drag copies the layer's own pixels (read as they were when the stroke began) at a fixed integer offset through the Brush's soft dab, composing with selections and the locks, one undo step "Clone Stamp Stroke"; no source marker/overlay (no token), no sample-all-layers, no opacity/flow; the dab rule (move toward the source by the falloff, not the Brush's max-alpha rule) is a choice not compared with Photoshop; a new provisional `IconKind::CloneStamp` (35 icons); not seen live, not on the Mac. 0.220.0: the Pencil tool exists (`Shift+B`): a hard-edged, aliased dab of the foreground colour, 100 % opacity only (no opacity control), sharing the Brush's radius, composing with selections, the transparency/pixel locks and Quick Mask, one undo step "Pencil Stroke"; the footprint rule (centre snapped to its texel, texel centres strictly inside the radius, so radius 1 is one texel) is a choice not compared with Photoshop; a new provisional `IconKind::Pencil` (34 icons); not seen live, not on the Mac. 0.219.0: the design owner's tokens exist: `selection.ants_light`/`selection.ants_dark` (pure white and black in all five themes, gated 3:1 against each other, now the marching-ants colours), `quick_mask.overlay` (`semantic.red.500` at 0.5, the Quick Mask tint), `size.icon_button` (32) and `size.colour_swatch` (24); the values were chosen by Claude on Cahya's delegation, nothing moved on screen except the ants' colours and the Quick Mask tint colour in the High Contrast themes (was their error colour, now the red), not seen by eye, not seen live, not seen on the Mac. 0.218.0: selections are saved in `.aur` and autosaves, Save/Load/Delete Saved Selection exist, and selection undo steps have a 256 MiB packed-history budget (the budget ignores masks shared with live selections, and packing runs on the UI thread); a PSD save names only saved selections, the active one is not written to PSD; not seen live nor on the Mac.) This file is updated every round (every
version bump that changes what Aurora does or what has been verified). It is
the detailed, honest picture of where the project is. The front page is
[README.md](README.md), the per-version list is [CHANGELOG.md](CHANGELOG.md),
and the full account of every round (design decisions, measurements,
disclosed weaknesses) is [PLAN.md](PLAN.md); where this file and PLAN.md
disagree, PLAN.md wins.

## Where Aurora stands

Aurora is an early but real editor: **Phase 1 (document, canvas, layers,
rendering, shell) is in progress**, and the Phase 1 gate (M1.10) is open. You
can open images and layered Photoshop files, paint and erase on real pixels
with undo/redo, work with layers, groups, masks, clipping masks, 27 blend modes
and three adjustment layers (Curves, Levels, Hue/Saturation), see Photoshop-style layer effects
drawn, keep several documents open in tabs, and save back out, including to
`.psd`. It is roughly 200,000 lines of Rust across 20 crates; 17 of them hold
real code and three (`aurora-ai`, `aurora-plugin`, `aurora-cli`) are still
skeletons.

Three things to keep in mind when reading anything below:

- **A green test run is not evidence that UI or canvas work is correct.** The
  first live session on real hardware (2026-08-12/13) found four bugs that
  900+ passing tests had missed. Most recent UI work has been tested
  headlessly (and, where noted, on a real GPU) but never seen by a person.
- **The macOS verification is a thin slice.** The window, VoiceOver
  announcing it, painting and panning on a Retina MacBook, and (after 0.192.1
  and 0.193.0) the canvas of the Nyepi template "looking perfect" were
  checked by the design owner. Almost nothing added after that has been seen
  on the Mac. Linux and Windows have built and passed CI, but no human has
  confirmed a live desktop session there.
- **No PSD written by Aurora has been opened in Photoshop.** Writes are
  checked against Aurora's own reader and psd-tools only.

Test and CI state: 3,890 tests passed at 0.203.0 (0 failed, 78 ignored, run with
`AURORA_REQUIRE_GPU=1` on an RTX 3090); the full CI gate runs on Linux, macOS and Windows.
Several rounds (0.161.1, 0.165.1, 0.165.2, 0.185.1, 0.186.1, 0.195.1, 0.202.1)
fixed timing-dependent or environment-dependent tests that failed on shared CI
runners; 0.202.1 is the latest (a thumbnail test that failed once on the macOS
runner, cause inferred and not reproduced). Measurements, benchmarks and a few
golden-image tests are `#[ignore]`d.

## What works today

"Headless" below means covered by automated tests with no person looking.
"Not seen on the Mac" means exactly that.

### Documents, files and editing

- **Open** PNG, JPEG, TIFF, Aurora's own `.aur` format, and layered 8- or
  16-bit RGB or Grayscale Photoshop files (PSD/PSB). Decoding runs in the
  background; the title bar and screen readers say "Opening file...". A
  refused open or save shows a dialog saying why and that nothing changed.
- **Save** to `.aur` (round-trips everything) and **Save As `.psd`** (see PSD
  below); **export** PNG/JPEG/TIFF composites every visible layer. Exporting
  does not count as saving.
- **Paint and erase** with a Brush and Eraser (radius 1-256 px in the options
  bar; a tool setting, not undoable, reset on launch). Unified undo/redo
  across structural edits and pixel strokes; whole-stroke coalescing; unlimited
  history stored as reversible operations plus dirtied tiles. Since 0.210.0 the
  pixel part is compressed (lossless) and bounded by a memory budget (a quarter
  of the tile budget, 32 MiB to 2 GiB; `AURORA_HISTORY_BUDGET_MB`): the oldest
  steps spill to the scratch disk and page back on undo, none is dropped. A
  1024-tile flat fill costs about 2 MiB instead of 512 MiB. Costs, plainly: a
  stroke's end packs its tiles on the UI thread (17-20 ms for 4 painted tiles,
  up to about 80 ms for 64-256 in the dev test profile; release not measured),
  spilling writes synchronously (at most 64 MiB and 64 files per call, so the
  budget is soft), a temp-file cleaner can remove spill files (that step then
  cannot be undone and a status-bar notice says so), a fill holds its raw tiles until it ends, and the budget is per
  document. Not seen in the live app.
- **History panel** lists the user's own steps ("Brush Stroke", layer edits,
  and so on) under an "Open" or "New Document" row; the current step is
  highlighted, undone steps are dimmed, and clicking a row jumps there
  (screen readers can click rows; there is no keyboard way to jump yet).
- **Canvas**: pan and zoom, pixel-accurate at any zoom and DPI scale. The status
  bar shows Photoshop-style physical zoom (100 % is one image pixel per
  physical pixel, so a Retina display opens at 200 %) and the document size
  and pixel format. Panning stops at the document's own top-left corner (no
  infinite pasteboard on purpose). Since 0.193.0 only the document is drawn
  (layer content outside the canvas is clipped), since 0.192.1 the canvas no
  longer shows documents washed out on an sRGB surface.
- **Tools panel and options bar**: a left panel (0.207.0: rebuilt Photoshop
  style) of flat icon-only toggle buttons, two to a row in a 72 px strip (the
  text-labelled one was 145 px), in five groups with a rule between them (Move
  and Marquee Select; Zoom and Pan; Eyedropper; Brush and Eraser; and, since
  0.209.0, Gradient and Paint Bucket). The selected
  tool is a filled accent square, hover and press tint it, each button keeps
  the tool's full name for assistive technology and a description with its
  shortcut letter, and each is its own Tab stop. Below the tools sit
  Photoshop's foreground/background swatches (0.208.0), a swap arrow and a
  default-colours icon: the Brush paints with the foreground, the Eyedropper
  sets it (Alt/Option: the background), `X` swaps, `D` resets, and a swatch
  opens an HSV colour picker popover (no hex entry, alpha or colour
  management; the colours are not saved across sessions). No tooltip yet. **Never seen by a human, nor on the Mac; the five new icons
  (marquee, zoom in, hand, eyedropper, eraser) are provisional.** The options
  bar shows the active tool's options: the radius for Brush and Eraser;
  since 0.209.0 Radial, Reverse and Opacity for the Gradient (`G`) and
  Tolerance and Contiguous for the Paint Bucket (`Shift+G`). Both fill the
  active pixel layer with the foreground/background colours, as one undo step,
  through the active selection since 0.211.0 (it was ignored before), the Gradient committing on release
  with no live preview, a fill over 64 Mpx refused with a calm notice; never
  seen by a human, nor on the Mac; the two new icons are provisional. Since 0.212.0 the
  Marquee's selection is drawn as marching ants (two-tone, 1 physical px, inverted
  selections also outline the document; steady under reduced motion, paused in a
  background window), Select All / Deselect / Inverse exist (`Ctrl/Cmd+A`, `+D`,
  `+Shift+I`, palette, macOS Edit menu), Escape cancels a Marquee drag and the status
  bar reads the selection out; a Marquee click deselects. Since 0.213.0 the selection
  can be a rectangle, an ellipse (the **Elliptical Marquee**, `Shift+M`, antialiased
  edge, ants along its curve) or a raster mask (the data model and outline exist, no
  tool makes one yet), and **every selection change is an undo step** in the History
  panel ("Rectangular Marquee", "Elliptical Marquee", "Select All", "Deselect",
  "Inverse Selection") that never marks the document unsaved. Selections are
  still not saved (`.aur`/PSD), are lost when the document closes, and the outline is
  not clipped to the document; nothing of this has been seen live or on the Mac.
  Since 0.217.0 the selection can be painted: **Quick Mask** (`Q`, the palette or
  the macOS Edit menu) turns it into an editable coverage grid over the document, the
  unselected area tinted (`state.error`, up to 50 %); the Brush paints toward the
  foreground's luminance and the Eraser toward the background's, never touching
  layers; Gradient, Paint Bucket, the selection tools and the Select commands are
  unavailable meanwhile; leaving is one undo step "Quick Mask" (all selected = no
  selection, nothing selected = no selection with a notice). Mask strokes are not
  individually undoable, the mode is not saved, and nothing of this has been seen
  live or on the Mac. (A first version turned the mode off every event-loop turn; fixed.)
  Since 0.216.0 the selection can be reshaped: **Feather, Expand, Contract, Smooth
  and Border Selection** (palette and the macOS Edit menu), each asking for a number
  of pixels in the palette's prompt (the last value is remembered for the session),
  each one undo step; Expand/Contract are exact Euclidean distances, Feather is a
  Gaussian (sigma = radius / 2), inverted selections are handled exactly; results over
  64 Mpx or too slow (about 24 Mpx) are refused with a notice. Combining Marquee drags
  back off when slow and say so when the 64 Mpx cap refuses a combination. Not seen
  live nor on the Mac.
  Since 0.215.0 every selection tool has **selection modes**: New (default), Add
  (`Shift` held at the press), Subtract (`Alt`), Intersect (`Shift+Alt`), or a mode
  chosen with the options bar's New/Add/Subtract/Intersect checkboxes (which override
  the modifiers) or the palette ("Selection Mode: ..."). The result is the union
  (`max`), intersection (`min`) or difference (saturating subtract) of the two
  coverages, exact for inverted selections (stored inverted, not clipped to the
  document); the selection stays visible during a combining gesture and the History row
  reads "Add to Selection" / "Subtract from Selection" / "Intersect Selection". A result
  over 64 Mpx is refused (the selection is kept; the Wand shows its notice). No cursor
  badge, no mode shortcut and no menu item; nothing seen live or on the Mac.
  Since 0.214.0 the **Lasso** (`L`, drag), the **Polygonal Lasso** (`Shift+L`, click
  vertices, close near the first one or with `Enter`, `Backspace` removes a vertex) and
  the **Magic Wand** (`W`, tolerance/contiguous shared with the Paint Bucket, active
  layer only) make raster selections, each one undo step; the lassos are clipped to the
  document, show no polyline preview (the live shape is the filled mask) and the first
  outline of a big mask costs a boundary trace on the UI thread; nothing seen live. The Wand's mask is clipped to the document; a tool switch records a live polygon. **Costs (dev profile, UI thread):** a 64 Mpx wand region 2.1 s plus a 351 ms first trace; one live rebuild of a 50k-point scribble in a small box about 0.9 s (live rebuilds thin out and back off after slow ones; the release's exact rebuild still pays it). **Silent behaviours:** a freehand path whose mask would exceed 64 Mpx keeps its last smaller shape without a notice; lasso/wand steps keep their masks in History with no byte budget. Workspace tests at 0.214.0: 4,221 passed, 0 failed, 82 ignored.
- **Text fields and dialogs**: caret, selection, IME composition, standard
  editing chords through the real OS clipboard (`Cmd` on macOS, `Ctrl`
  elsewhere), click/drag selection (double-click word selection was not done as of 0.138.0).
  Dialog buttons really act: `Enter`/`Space`/click/AT-click, `Enter` is the
  default, `Escape` cancels, `Tab` stays inside the dialog.
- **Command palette** (`Ctrl+Shift+P`) and a native macOS menu bar.

### Several documents, autosave and recovery

- Several documents can be open at once as a row of tabs; `Ctrl+Tab` /
  `Ctrl+Shift+Tab`, the tab bar, arrow keys on a tab or a screen reader's
  click switch between them. Each keeps its own layers, history, active layer
  and view.
- A tab with unsaved changes shows a bullet and an AT description "unsaved".
  **Close Document** (`Ctrl+W`, the tab's "x") asks Save / Don't Save / Cancel;
  closing the window or macOS Quit (`Cmd+Q`) reviews every unsaved document.
  Dock Quit and logout do not ask, but keep unsaved work recoverable.
- Crash-recovery autosave is per document, written off the UI thread, and
  indexed per run; recovery tells you if a document was missing or damaged, and
  a second running Aurora is never mistaken for a crash. Tested headlessly, not
  with a real crash on real hardware.

### Panels and workspaces

- A dockable right rail (Layers; Properties and History as tabs), collapsible
  to a narrow label strip; panels scroll with a visible scrollbar; panels can
  be dragged to re-dock, reordered, joined as tabs, or **floated** inside the
  window; every move is also a command-palette command; the arrangement is
  remembered between launches (damaged layouts are repaired).
- **Workspaces**: built-in "Essentials", "Save Workspace As...", switch, reset
  and delete by name (command palette only, not in the macOS menus).
- All of this is headless-tested; none of the panel dragging, floating or
  workspace work has been tried by a human on real hardware or with VoiceOver.

### Layers panel (Photoshop-style)

- Rows: an eye toggle, a disclosure chevron, folder icons, the name, a
  right-aligned blend-mode label, a padlock for locked layers, a clip glyph for
  clipped layers, nested indent; **thumbnails** for pixel layers (lazy,
  computed in the background for visible rows only, at most 16 tiles per layer
  read).
- Controls above the list: blend mode (all 27 modes; "Pass Through" for groups
  only), **Opacity** and **Fill** sliders (a drag is one undo step), and a
  **Lock** row (transparent pixels, image pixels, position, all).
- Bottom toolbar: New Layer, New Group, New Curves adjustment and Delete work;
  Link, Fx and Mask are disabled placeholders. **New Group** makes Pass Through
  groups, as Photoshop does. A name search and three kind toggles (pixel layers,
  adjustment layers, groups) filter the list.
- New Layer, Delete Layer (never the last pixel layer), New Group, New Curves
  Layer, New Levels Layer, New Hue/Saturation Layer and Create/Release Clipping Mask are one undo step
  each.
- Not seen in the live window or on the Mac: the rebuilt rows (0.195.0), the
  toolbar (0.196.0), thumbnails (0.197.0), locks (0.198.0), the filter
  (0.199.0), the clip glyph (0.201.1) and the vector icons (0.194.0, a
  provisional set no person has looked at).
- Layer locks are enforced: the brush, eraser, Move and Delete Layer refuse a
  locked layer with a quiet status-bar notice, and a transparency lock lets the
  brush recolour without changing alpha. A PSD's Background layer opens locked
  as in Photoshop.
- Filter matching lowercases only: no case folding or Unicode normalisation
  ("ß" does not match "ss", a decomposed name does not match composed input).

### Compositing and blend modes

- All 27 PSD-compatible blend modes composite for real, on flat layers and
  inside groups, including Dissolve (deterministic, seeded from absolute
  document position). Groups composite into an isolated buffer first; nested
  groups blend correctly. **Pass Through** groups composite as Photoshop does
  (0.179.0), which moved the Nyepi template's mean error against Photoshop's
  image from about 20.7-26.6 to about 2.7 levels.
- A **GPU fast path** handles non-grouped documents whose layers sit at one of
  20 blend modes (Normal, Multiply, Darken, Lighten, Screen, Difference,
  LinearDodge, LinearBurn, ColorBurn, ColorDodge, Overlay, HardLight,
  LinearLight, VividLight, HardMix, PinLight, SoftLight, Subtract, Divide,
  Dissolve) and an unmasked root-level Curves, Levels or Hue/Saturation layer (0.206.0 for
  the last two; Hue/Saturation with active colour ranges excepted); everything
  else (groups, Exclusion, the six non-separable modes, masked adjustment
  layers, ranged Hue/Saturation, layers with drawn effects, clipped layers) falls
  back to the CPU path transparently
  with the same result. GPU compositing is verified on Vulkan/NVIDIA only;
  Metal and DX12 are unverified for these mechanisms.
- The CPU whole-view recomposite runs on a **background worker** (so hiding a
  layer or changing opacity no longer freezes the window), folds tiles in
  parallel on all but one core, culls layers hidden under an opaque layer and
  skips empty parts of layers. A brush dab still repaints its own few tiles
  at once.
- The tile store's resident budget is RAM / 4 clamped to [128 MiB, 8 GiB]
  shared by all open documents, with the active document getting at least
  three quarters of it (`AURORA_TILE_BUDGET_MB` overrides). Machines with 4 or
  fewer cores or 8 GiB or less of RAM use a smaller effects cache (64 MiB
  instead of 256 MiB) and open a PSD without overlapping encode and decode;
  that has not been measured on a real small machine.
- Full-document export composites every visible layer, not just the active
  one.

### Masks, clipping masks, Fill

- Layer and group masks have real per-pixel grayscale coverage (feathered
  edges and partial alpha), with bounds, enabled and inverted respected;
  coverage survives `.aur` save/load. PSD mask **density** is applied
  (non-destructively, editable in the model; no control in the app yet), and
  **vector masks** are rasterised into pixel masks and reported as converted.
  Mask feather is not applied and is listed when you open such a file.
- **Fill** (Photoshop's second opacity) is applied everywhere content
  composites.
- **Clipping masks** (0.201.0): Create/Release Clipping Mask (palette, macOS
  Layer menu, `Alt+Ctrl+G`; `Cmd+Alt+G` on macOS). A clipped layer composites
  only where its base has alpha; the base's blend mode, opacity and Fill apply
  to the whole clip group (Photoshop's default). PSD clipping is read and
  written. Against Photoshop's merged images it matches closely on several
  corpus files (for example `adjustment_clipping.psd` mean error 30.3/10.7/16.1
  -> 0.19/0.29/0.23) and not at all on others (`clipping-mask3/4/5.psd` and
  five more are unchanged, the cause not investigated; no file got worse).
  Known differences from Photoshop: "Blend Clipped Layers as Group" turned off
  is not modelled; a base with drawn effects is approximated for non-Normal
  modes; a clipped layer whose base is a group or an adjustment layer
  composites as an ordinary layer (reported on PSD open); when a base is
  removed its clipped layer clips to the next layer down (Photoshop releases
  the first clipped layer instead).

### Adjustment layers

- **Curves** (0.155.0-0.159.0): New Curves Layer, a Properties editor with
  RGB/Red/Green/Blue channel tabs and draggable points (a drag is one undo
  step; screen readers can move points), a muted histogram of the image below
  behind the curve (computed incrementally from at most 16 sampled tiles), up
  to 19 points, a natural cubic spline matching Photoshop's within one 8-bit
  level on `curves_rgb.psd`. A PSD's `curv` layers open as real, editable Curves
  layers (mean 0.14-0.24 levels from Photoshop's merged image, at most 11
  levels on 2 pixels of a very steep segment). GPU path for unmasked root
  Curves.
- **Levels** (0.202.0): New Levels Layer, a Properties editor with channel
  tabs and five sliders (input black, gamma, input white, output black, output
  white), per-channel then master order, `levl` layers imported. **GPU
  compositing for an unmasked root layer at Normal or one of 18 blend modes
  (0.206.0, verified on an RTX 3090/Vulkan only, within 4.9e-4 of the CPU on
  opaque backdrops; Metal/DX12 unverified); masked layers and the other modes
  stay on the CPU.** On
  `adjustments/levels_rgb.psd` against Photoshop's merged image: max error
  (2, 2, 6) levels for R, G, B, mean (0.266, 0.247, 0.429); the maximum of 6 in
  blue is not investigated. **PSD save of Levels layers works
  (0.205.0)**. Not seen in the live window or on the Mac.
- **Hue/Saturation** (0.203.0): New Hue/Saturation Layer, a Properties editor
  (Hue, Saturation, Lightness sliders, a Colorize checkbox with its own three
  sliders; whole numbers only), per-pixel HSL math on encoded RGB (a non-identity
  setting clamps the backdrop to [0, 1], so float values above 1 are clipped), `hue2`
  layers imported. **Per-colour-range adjustments (0.204.0)**: a range selector
  (Master, Reds ... Magentas) re-binds the three sliders; ranges render by a
  fitted model (see PLAN.md) and PSD layers using them import, with the edges
  the file stores (edges are not editable in the UI). **GPU compositing for the master and Colorize
  settings of an unmasked root layer (0.206.0, RTX 3090/Vulkan only, within
  4.9e-4 of the CPU on opaque backdrops; Metal/DX12 unverified); a layer with
  active colour ranges, a mask or an unadmitted blend mode stays on the CPU**; **PSD save of Hue/Saturation layers works
  (0.205.0)**: written as `hue2` adjustment layers, values rounded to whole numbers and itemised. Against Photoshop's merged image: Colorize layers match to a mean
  of about 0.4-0.5 levels (max 3); a master-only layer is about 1-1.4 levels on
  saturated pixels but tens of levels off on near-grey pixels at Saturation
  +100 (possibly the reconstructed backdrop, not the model); positive
  saturation between 0 and 100 uses `s/(1-k)`, a choice not told apart from
  the alternative `s + (1-s)k`. Not seen in the live window or on the Mac.
- Other adjustment kinds and Curves in Grayscale files are listed as "not
  shown" when a PSD is opened.

### Layer effects (pixel layers; solid colour)

- Read and kept with their layers (and saved in `.aur`): drop shadow, colour
  overlay, stroke, outer glow. Drawn on the CPU: **colour overlay** (Fill fades
  the layer's pixels and not its overlay, as in Photoshop), **drop shadow**
  (offset, spread, three-box blur; knocked out by the shape; also hidden under
  a layer's outside stroke), **stroke** (solid; outside, inside or centred),
  and **Softer outer glow**. Gradient and pattern strokes, gradient glows,
  Precise glows, noise, non-Linear contours and any effect on groups are kept or
  reported but not drawn.
- Accuracy (against Photoshop's own merged images in the corpus): Nyepi's error
  fell from mean 2.65 / max 105 to 0.32-0.45 / 25 with shadows; stroke pixels on
  `stroke-effects.psd` match within one level; the glow's error around the test
  text halved from 4.35 to 2.16 levels. The corpus got better on seven files
  and worse on four when strokes landed in 0.190.0 (PLAN.md has the table). The
  `masks.psd` Stroke residual was explained and fixed in 0.191.1 (a shadow that
  knocks out is hidden under the layer's outside stroke too: mean error 0.2257
  -> 0.1970 red; `masks.psb` 0.1433 -> 0.1146), and no other effects-corpus
  file moved; the remaining per-file differences were not investigated.
- Layers with drawn effects use the CPU path. Glow knockout under the layer is
  not applied ("per the brief", unverified against Photoshop).

### PSD / PSB

- **Read**: Aurora's own reader, built for untrusted files (checked against 272
  psd-tools test files, 235 of which open, and 46,000+ deliberately broken
  ones). Keeps groups (including Pass Through), names, opacity, fill, blend
  modes, visibility, layer masks, vector masks (as pixel masks), clipping, locks,
  Curves and Levels layers, effects data and smart objects. Opened streamed, one
  layer at a time, so the file and document are never held whole (a 2 GiB test
  file peaked at about 1.3 GiB instead of 4.3 GiB; still about 7 s). What it
  cannot show is listed in an "Opened With Changes" dialog. Unsupported:
  CMYK, Lab, Bitmap, Indexed, Duotone, 32-bit.
- **Write**: Save As `.psd` writes a 16-bit RGB `.psd` with an sRGB profile, layers,
  nested groups, and layer and group masks (with density; inverted masks are
  written already inverted), locks, clipping bytes and preserved smart objects.
  It writes beside the target, verifies by reopening, then renames; a
  "Save With Changes?" list is shown before anything lossy, and a lossy save
  is a copy (the document stays unsaved). **Refused**: PSB and canvases
  over 30,000 px (an adjustment layer only if its rounded values would not read
  back). Curves, Levels and Hue/Saturation layers are written as adjustment layers
  (0.205.0); values finer than the file (curve points off the 8-bit grid, a Levels
  gamma beyond hundredths, fractional Hue/Saturation values, custom edges on a
  neutral colour range) are rounded and itemised in the list, and so are colour-range
  adjustments under Colorize, which Aurora itself does not keep on reopen.
- **Smart objects** (0.200.0): a smart object layer opens as ordinary pixels
  (Aurora neither renders nor edits the embedded file), but its embedded file
  and placement blocks are held byte for byte and written back on a `.psd`
  save, so the layer reopens in Photoshop as a smart object, **only while the
  layer is untouched**: painting, erasing or moving it makes the save write an
  ordinary pixel layer, and the "Save With Changes?" dialog names the layers.
  Checked with Aurora's own reader and psd-tools on the 35 smart objects of the
  Nyepi template. The embedded files live in memory (about 100 MB for that
  template) and are written into every `.aur` save and autosave.
- Verified against psd-tools 1.17.4 (`scripts/verify_psd_write.py`, not in CI).
  **Never opened in Photoshop.**

### Widget toolkit, themes and accessibility

- `aurora-widgets` and `aurora-theme` build the app's own UI: real layout,
  focus (visible `Tab` focus ring), hit-testing, IME composition, popovers,
  tooltips, menus, scroll containers and real GPU-rendered text (bundled Inter
  Regular, a provisional stand-in for the undecided typeface). A **Widget
  Gallery** panel (command palette) shows every toolkit widget live.
- Five built-in themes exist (Dark, Light, High Contrast Dark, High Contrast
  Light, Colour-Critical), each checked against the same WCAG 2.1 AA contrast
  pairs. 30 widget goldens are blessed; 45 more `#[ignore]`d golden-diff tests
  still wait for a human to review them on real GPU hardware.
- Every widget carries an `accesskit` node and screen-reader actions reach the
  widgets that declare them. A screen reader announcing the window and
  speaking a custom text field was verified on macOS (9 of 10 checks; see
  [spike/a11y-ime/FINDINGS.md](spike/a11y-ime/FINDINGS.md)); no screen reader
  has driven the newer panels, and Linux (AT-SPI) and Windows (UIA) are
  unverified.

## Measured numbers (and their caveats)

### The throwaway vertical slice (2026-07-26, one GPU, one OS, single-threaded)

A window -> `wgpu` -> a 100,000 x 100,000 px half-float tiled document (80 GB)
-> per-tile composite -> brush stroke -> save/reload, UI drawn in the same
frame as the canvas.

| | Budget | p50 | p99 |
|---|---|---|---|
| Stroke latency (input -> frame) | 10 ms | 4.1 ms | 9.1 ms |
| Idle frame | 16.7 ms | 0.6 ms | 0.8 ms |
| Pan with page-in from disk | 16.7 ms | 7.0 ms | 16.7 ms |

An 80 GB document edits comfortably in a 64 MB memory budget, and half-float
save/reload is bit-exact. It corrected the main assumption: the bottleneck is
CPU compositing, **not** disk I/O. Re-run at the real 300,000 x 300,000 px PSB
ceiling, paging behaviour came back identical to the byte (same tiles
created/evicted, same scratch I/O), so the sparse tile store's cost is a
function of what is touched, not the document's nominal size. Full results:
[spike/FINDINGS.md](spike/FINDINGS.md). Under 1 ms of stroke-latency margin
remains; a latency regression test runs in CI.

### The live app: 60 FPS is still missed

A GPU-gated, headless end-to-end test drives `aurora-app`'s own compositing
path for a pan-while-painting sequence at the 300,000 px ceiling. The
0.39.0 figures were p50 35.4 ms / p99 98.8 ms on the GPU path and p50 22.6 ms
/ p99 54.1 ms on the CPU fallback, against a 16.7 ms budget. Later work has
been re-measured on a real RTX 3090: on an idle machine the GPU path's mean
frame is about 7.5-8.6 ms with a worst-of-40 of 21-30 ms, **but with 8
competing CPU-bound threads (ordinary desktop multitasking) the same mean
roughly doubles to about 15-18 ms, at or over the budget.** Quote the
contended figure with the idle one. A "p99" from these 40-sample benchmarks
is a single order statistic and should not be quoted as a ratio. The gate is
still missed; see PLAN.md's 0.96.2 entry for the current record for the two
standing gate fixtures.

### Large-template performance (the "Nyepi" template: 145 MB PSD, about 189 megapixels, 277 Layers rows, 35 smart objects)

| Round | Result |
|---|---|
| 0.183.0 culling of occluded layers | warm full composite 10.1 s -> 8.6 s |
| 0.184.0 skip empty source | 8.6 s -> 5.4 s (tile budget 256) |
| 0.185.0 RAM-sized tile budget | 5.4 s -> 3.34 s on a 62 GiB dev box (paging nothing in); open + first composite 10.88 s; both targets missed |
| 0.186.0 background composite | a structural edit costs 0.93 ms on the UI thread (about 3.3 s before); the image appears about 3 s later |
| 0.188.0 parallel fold (7 threads) | warm structural edit 2.97 s -> 0.62 s, bit-identical |
| 0.189.0 open time | decode-thread work 2.7-2.8 s -> 1.3-1.5 s (release, 8 cores, a loaded box); UI-thread install about 1.7 ms; the PRD's 2 GiB-in-5 s target was not re-measured |
| Layers panel, Nyepi | 277-row populate 0.74 ms; 8 visible thumbnails cost 0.5 ms and 32 tile shares; one filter keystroke 0.07 ms ("post"), 1.5 ms (empty search) |

The streamed open of a 2 GiB, 8192^2 four-layer 16-bit noisy PSD: +1,277 MiB
peak against +4,352 MiB before, UI-thread install 1.3 ms, open 7.3 s (over the
PRD's 5 s target at 0.154.0). Shadow-cache and PSD-open work on
small machines has not been measured on a real small machine.

### Budgets at a glance

| Budget | Target | State |
|---|---|---|
| Brush latency | < 10 ms | spike p99 9.1 ms; under 1 ms of margin |
| Canvas interaction | 60 FPS | **failing** on the live app at the 300,000 px ceiling (see above) |
| Pan with page-in | 16.7 ms | passing in the spike (p50 7.0 ms) |
| Startup | < 3 s | unmeasured |
| Open 2 GB PSD | < 5 s | 7.3 s at 0.154.0; not re-measured since |
| Export 1 GB doc | < 10 s | unmeasured |

## Verification state by platform

- **macOS (Retina MacBook, Apple M2 Pro / Metal)**: the window, VoiceOver
  announcing it, a human painting and panning, the canvas colour fix and the
  Nyepi template ("looks perfect", a visual check, 2026-10-11). The first live
  session found and fixed four real bugs (slow painting, paint offset on
  Retina, sub-tile pan, pan freezing at the edge). The canvas sRGB double
  encode that 0.192.1 fixed was live while earlier checks on that Mac looked
  right; why is unexplained. One saturated-red residual in a user screenshot is
  an unmeasured hypothesis (likely macOS screenshot colour management).
  Everything since 0.194.0 is **not seen on the Mac**.
- **Linux**: builds and passes CI; GPU tests run on Vulkan (NVIDIA RTX 3090 and
  Mesa llvmpipe in software). The AT-SPI/Orca leg of the accessibility
  checklist is unverified. Tests that read a local PSD corpus skip when it is
  absent.
- **Windows**: builds and passes CI; DX12 and UIA are unverified; macOS/Windows
  LGPL packaging needs hardware and a human.
- **Accessibility and IME**: macOS 9 of 10 passes
  ([spike/a11y-ime/FINDINGS.md](spike/a11y-ime/FINDINGS.md)), with one real,
  non-structural bug (live value-change announcements can fail to reach
  VoiceOver). Linux and Windows human legs are open; Cahya accepted that as a
  risk on 2026-07-28 rather than a blocker. [ADR 0001](docs/adr/0001-custom-wgpu-ui.md)
  (the custom `wgpu` UI) remains the most durable open risk.

## What is partial or not done

**Hardware- or human-gated**
- Windows/DX12 validation, the Linux and Windows accessibility/IME legs, and
  macOS/Windows LGPL packaging.
- Everything listed above as "not seen on the Mac" or "not seen in the live
  window"; trackpad feel, drag feel and screen-reader behaviour of the newer
  panels have only been tested headlessly.
- 45 widget golden images await a human bless on real GPU hardware.

**Compositing and performance**
- The 60 FPS gate is missed; `recomposite` is about 73 % of the GPU-path frame.
- GPU fast path does not cover groups, `Exclusion`, the six non-separable
  modes, masked adjustment layers, ranged Hue/Saturation, drawn effects or
  clipped layers (Curves, Levels and master/Colorize Hue/Saturation are covered
  since 0.206.0).
- Re-anchoring the composite grid to the document (so structural undo can
  invalidate narrowly and `composite_reference_origin` becomes constant) is the
  named follow-on to 0.193.0; the far pan edge is not bounded and a document
  smaller than the view is not centred.
- Open time is better but not at the PRD target; the decode thread slows from
  0.75 s alone to 1.1-1.25 s beside the pool (contention, not investigated).

**Adjustments, filters, tools**
- Adjustments: Curves, Levels and Hue/Saturation (Master, Colorize and the six colour ranges as a fitted CPU model; range edges are not editable in the UI). No filters,
  selection tools beyond a raw data model, smart-object editing, RAW import,
  plugins, scripting or AI (`aurora-ai`, `aurora-plugin`, `aurora-cli` are
  skeletons).
- No brush or tool for painting a mask, and no mask-pixel undo or history
  proper; there is no control for mask density in the app.
- The Layers toolbar's Link, Fx and Mask buttons are disabled placeholders;
  there is no visible tooltip yet (the description carries the text); no
  Channels or Paths tabs; no keyboard way to move a floating panel or to jump
  in History.

**PSD**
- Not written: PSB, text layers, editable vector
  paths, CMYK/Lab/32-bit. Not read: CMYK/Lab/Bitmap/Indexed/Duotone/32-bit.
- Smart objects are not rendered or edited and survive a save only while
  untouched.
- Photoshop has never opened a PSD Aurora wrote.
- Mask feather is not applied; vector masks become pixel masks.
- Clipping and effect differences listed above.

**Smaller disclosed items**
- Layers-filter matching is `to_lowercase` only.
- A Photoshop Pass Through group's Fill is reported, not applied.
- The icon set is provisional and unseen by a person; Inter Regular is a
  stand-in for the undecided typeface.
- Dock Quit and logout do not ask about unsaved documents.
- A flaky-test sweep (0.165.2) found one real race; further timing-dependent
  tests can still appear on shared CI runners.

## Known gaps and next steps

- **Adjustment queue approved by the design owner is done** (Levels 0.202.0,
  Hue/Saturation 0.203.0, its per-colour-range adjustments 0.204.0). Open
  follow-ups: the master Lightness slider should blend towards white in RGB
  (PLAN.md, 0.204.0), range edge editing, and a histogram behind the Levels sliders (the GPU
  path for Levels and Hue/Saturation is done, 0.206.0).
- **Next in the performance line**: anchor the composite grid to the document.
- **Phase 1 gate (M1.10)** remains open on hardware (accessibility and IME
  audits on all three platforms), on the missed 60 FPS budget, and on a
  design-owner decision about which remaining gallery component to build next.
- Roadmap beyond Phase 1 (selections, brushes, filters, adjustments, smart
  objects, Camera RAW, colour management, full PSD/PSB) is in
  [README.md](README.md) and [PRD.md](PRD.md) §9; task-level state is in
  [PLAN.md](PLAN.md).
