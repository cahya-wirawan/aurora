//! Quick Mask mode (0.217.0), the app half: Photoshop's `Q`.
//!
//! **Semantics.**
//!
//! * **Entering** ([`enter`]) turns the document's selection (or "everything"
//!   when there is none) into an editable 8-bit coverage grid over the whole
//!   document rectangle (`aurora_doc::QuickMask`; at most 64 Mpx, a larger
//!   document is refused with a calm notice and stays out of the mode). A
//!   selection reaching past the document is clipped to it. Entering records
//!   nothing and changes neither the selection nor the document.
//! * **While on**, the Brush and the Eraser edit the *grid*, never a layer:
//!   no pixel changes, no stroke snapshot, no pixel-history step, whatever
//!   the layer's locks or even with no active layer. The Brush moves
//!   coverage toward the [`luma`] of the foreground colour (white selects,
//!   black deselects, greys partly) and the Eraser toward the luma of the
//!   **background** colour (Photoshop's swap: the Eraser paints the other
//!   colour), by the layer brush's own dab footprint and falloff
//!   (`aurora_brush::stamp_toward`, `dab_falloff`): a dab pulls each texel
//!   `falloff` of the way to the target, so overlapping dabs accumulate
//!   like flow. The selection itself is untouched until the mode ends, and
//!   it is not applied to layers (nothing paints through it; the grid is
//!   the selection being edited).
//! * **Unavailable while on** ([`tool_refusal`], [`COMMAND_REFUSAL`]): the
//!   Gradient, the Paint Bucket and every selection tool (Marquees, Lassos,
//!   Magic Wand), and the Select commands (All, Deselect, Inverse, Modify):
//!   a press or command says so in the status bar and does nothing. Pan,
//!   Zoom, the Eyedropper and Move work normally.
//! * **The overlay** is the unselected area tinted
//!   (`aurora_ui::quick_mask_tint`: the `quick_mask.overlay` token, up to its 50 % alpha),
//!   rebuilt only when the grid, the view or the sizes change
//!   ([`QuickMaskMode::tint_changed`]); the status bar reads
//!   "Quick Mask · Document: ...".
//! * **Leaving** ([`leave`]; `Q` again or the palette command, a document
//!   switch, New or Open) turns the grid back into a selection
//!   (`QuickMask::outcome`): all `255` is no selection, all `0` is no
//!   selection plus a notice (an empty selection is never stored), anything
//!   else the tight crop as a rectangle or a raster mask. It is recorded as
//!   ONE undo step "Quick Mask" (before = the selection in force when
//!   leaving, so an undo made meanwhile cannot corrupt the step), dirty
//!   neutral. A grid nothing edited changes and records nothing (the
//!   original selection, even one reaching past the document, is kept).
//!   Closing the active document drops the mode without applying it.
//! * **Not covered:** individual mask strokes are not undoable (Ctrl+Z
//!   walks the ordinary history while the mode is on); the mode is not saved
//!   or restored; Escape does not leave it.

use aurora_doc::{QuickMask, QuickMaskOutcome, Selection, SelectionSet};

use crate::colour_state::ColourState;
use crate::{DabTool, LockRefusal, ToolSettings, UndoOrder};

/// The History row of the step recorded on leaving.
pub(crate) const STEP_LABEL: &str = "Quick Mask";

/// The palette and menu command id.
pub(crate) const COMMAND_ID: &str = "select.quick_mask";

/// The palette and menu title.
pub(crate) const COMMAND_TITLE: &str = "Toggle Quick Mask Mode";

/// What a Select command meets while the mode is on.
pub(crate) const COMMAND_REFUSAL: LockRefusal = LockRefusal::QuickMaskLeaveFirst;

/// The mode while it is on: the grid and what the overlay was last built for.
#[derive(Debug)]
pub(crate) struct QuickMaskMode {
    pub(crate) mask: QuickMask,
    tint_key: Option<TintKey>,
}

/// Everything the overlay depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TintKey {
    version: u64,
    zoom: u32,
    pan: (u32, u32),
    canvas: (u32, u32),
    scale: u32,
}

impl QuickMaskMode {
    /// Whether the overlay must be rebuilt for `key` inputs; remembers
    /// them, so a second call with the same inputs says `false`.
    pub(crate) fn tint_changed(
        &mut self,
        view: &aurora_ui::CanvasView,
        canvas: (f32, f32),
        scale: f32,
    ) -> bool {
        let key = TintKey {
            version: self.mask.version(),
            zoom: view.zoom().to_bits(),
            pan: (view.pan().0.to_bits(), view.pan().1.to_bits()),
            canvas: (canvas.0.to_bits(), canvas.1.to_bits()),
            scale: scale.to_bits(),
        };
        let changed = self.tint_key != Some(key);
        self.tint_key = Some(key);
        changed
    }
}

/// The Rec.709 luma of a straight sRGB colour as a coverage byte: `0.2126 r +
/// 0.7152 g + 0.0722 b`, each channel clamped to `0..=1` (a NaN counts as
/// `0`), rounded to the nearest byte.
#[must_use]
pub(crate) fn luma(rgb: [f32; 3]) -> u8 {
    let [r, g, b] = rgb.map(|c| if c.is_nan() { 0.0 } else { c.clamp(0.0, 1.0) });
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    // `y` is in 0..=1 (the weights sum to 1), so this is in 0..=255.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let byte = (y * 255.0).round().clamp(0.0, 255.0) as u8;
    byte
}

/// The coverage a dab of `tool` pulls toward: the Brush the foreground's
/// luma, the Eraser the **background**'s.
#[must_use]
pub(crate) fn target_byte(tool: DabTool, colours: &ColourState) -> u8 {
    match tool {
        DabTool::Brush | DabTool::Pencil => luma(colours.foreground()),
        DabTool::Eraser => luma(colours.background()),
    }
}

/// One Brush or Eraser dab at document point `doc_point` on the grid: the
/// one call `App::paint_dab`/`erase_dab` make in the mode. Returns whether
/// any texel changed.
pub(crate) fn paint_dab(
    mode: &mut QuickMaskMode,
    tool: DabTool,
    doc_point: (f32, f32),
    settings: &ToolSettings,
    colours: &ColourState,
) -> bool {
    let radius = settings.dab_radius(tool);
    let target = target_byte(tool, colours);
    mode.mask
        .edit(|data, w, h| {
            // The Pencil's hard footprint (0.220.0), the others' soft one.
            if tool == DabTool::Pencil {
                aurora_brush::stamp_toward_hard(data, w, h, doc_point, radius, target)
            } else {
                aurora_brush::stamp_toward(data, w, h, doc_point, radius, target)
            }
        })
        .is_some()
}

/// Enters the mode for a document of `size` with `selection`.
///
/// # Errors
///
/// [`LockRefusal::QuickMaskTooLarge`] when the document is over the 64 Mpx
/// cap (or empty): the mode is not entered.
pub(crate) fn enter(
    selection: Option<&Selection>,
    size: (u32, u32),
) -> Result<QuickMaskMode, LockRefusal> {
    QuickMask::from_selection(selection, size)
        .map(|mask| QuickMaskMode {
            mask,
            tint_key: None,
        })
        .map_err(|_| LockRefusal::QuickMaskTooLarge)
}

/// What leaving did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LeaveOutcome {
    /// The grid was never edited: the selection is as it was.
    Unchanged,
    /// A selection (a region or none) was set as one step.
    Applied,
    /// The grid selected nothing: no selection, as one step, with a notice.
    Emptied,
}

/// Leaves the mode, turning `mode`'s grid into `selection` and recording
/// ONE "Quick Mask" step (nothing when the selection ends up as it was).
pub(crate) fn leave(
    mode: &QuickMaskMode,
    workspace: &mut aurora_ui::Workspace,
    undo_order: &mut UndoOrder,
    history: &mut aurora_doc::History,
    pixel_history: &mut aurora_brush::PixelHistory,
    selection: &mut SelectionSet,
) -> LeaveOutcome {
    if !mode.mask.is_edited() {
        return LeaveOutcome::Unchanged;
    }
    let before = selection.active();
    let (after, emptied) = match mode.mask.outcome() {
        QuickMaskOutcome::Everything => (None, false),
        QuickMaskOutcome::Nothing => (None, true),
        QuickMaskOutcome::Region(region) => (Some(region), false),
    };
    match after.clone() {
        Some(region) => selection.select(region),
        None => selection.deselect(),
    }
    crate::record_selection_step(
        workspace,
        undo_order,
        history,
        pixel_history,
        STEP_LABEL,
        before,
        after,
    );
    if emptied {
        LeaveOutcome::Emptied
    } else {
        LeaveOutcome::Applied
    }
}

/// The refusal a canvas press with `tool` meets while the mode is on.
#[must_use]
pub(crate) fn tool_refusal(tool: aurora_ui::Tool) -> Option<LockRefusal> {
    use aurora_ui::Tool;
    match tool {
        Tool::Gradient
        | Tool::PaintBucket
        | Tool::MarqueeSelect
        | Tool::EllipticalMarquee
        | Tool::Lasso
        | Tool::PolygonalLasso
        | Tool::MagicWand
        | Tool::CloneStamp
        | Tool::Smudge
        | Tool::Blur
        | Tool::Sharpen
        | Tool::Dodge
        | Tool::Burn
        | Tool::Sponge => Some(LockRefusal::QuickMaskToolUnavailable),
        Tool::Move
        | Tool::Zoom
        | Tool::Pan
        | Tool::Eyedropper
        | Tool::Brush
        | Tool::Pencil
        | Tool::Eraser => None,
    }
}

/// The layer a press with `tool` hands `begin_drag`: in Quick Mask a Brush
/// or Eraser stroke gets none (so no stroke snapshot is ever made and no
/// pixel-history step can be recorded); everything else is unchanged.
#[must_use]
pub(crate) fn drag_layer<T>(on: bool, tool: aurora_ui::Tool, layer: Option<T>) -> Option<T> {
    if on && paints_mask(tool) { None } else { layer }
}

/// Whether a press skips the layer-lock refusal: only the mask-painting
/// tools in Quick Mask (they never touch the layer). Move still honours a
/// position lock.
#[must_use]
pub(crate) fn skips_lock_gate(on: bool, tool: aurora_ui::Tool) -> bool {
    on && paints_mask(tool)
}

/// Whether `tool` edits the grid in the mode (the three dab tools).
const fn paints_mask(tool: aurora_ui::Tool) -> bool {
    matches!(
        tool,
        aurora_ui::Tool::Brush | aurora_ui::Tool::Pencil | aurora_ui::Tool::Eraser
    )
}

/// Ends a Brush or Eraser drag that has no stroke snapshot: such a drag
/// belongs to Quick Mask (it paints the grid), and once the mode is gone it
/// must not go on painting the layer with no undo.
pub(crate) fn drop_mask_stroke(drag: &mut Option<crate::Drag>) {
    use crate::Drag;
    if matches!(
        drag,
        Some(Drag::Brush { stroke: None, .. } | Drag::Eraser { stroke: None, .. })
    ) {
        *drag = None;
    }
}

/// Whether a drag in progress when the mode is entered must be committed
/// first: a layer stroke (it holds a pixel snapshot) would otherwise carry
/// on into the mode with a stale snapshot.
#[must_use]
pub(crate) fn drag_needs_commit_before_enter(drag: Option<&crate::Drag>) -> bool {
    use crate::Drag;
    matches!(
        drag,
        Some(
            Drag::Brush {
                stroke: Some(_),
                ..
            } | Drag::Eraser {
                stroke: Some(_),
                ..
            }
        )
    )
}

/// Clears the overlay (the mode is over).
pub(crate) fn clear_tint(workspace: &mut aurora_ui::Workspace) {
    if let Err(err) = aurora_ui::sync_quick_mask_tint(
        &mut workspace.tree,
        workspace.selection_outline,
        Vec::new(),
    ) {
        tracing::warn!(?err, "failed to clear the Quick Mask tint");
    }
}

/// Leaves the mode because the document is stopping being the active one
/// (a tab switch, New, Open, Close, quit): the grid becomes **that
/// document's** selection as one "Quick Mask" step, before the document is
/// parked. Called from `document_tabs::commit_live_gestures`, which every
/// such path runs once, only when the change will really happen; it does
/// nothing when the mode is off (so it is safe to call every time). The
/// "selected nothing" notice is not shown (there is no longer a document it
/// would describe).
pub(crate) fn leave_for_switch(
    mode: &mut Option<QuickMaskMode>,
    workspace: &mut aurora_ui::Workspace,
    doc: &mut crate::document_session::DocumentSession,
) -> Option<LeaveOutcome> {
    let mode = mode.take()?;
    // A mask stroke still in `drag` has no snapshot and is ended with the
    // other live gestures right after (`commit_drag_into_history`).
    let outcome = leave(
        &mode,
        workspace,
        &mut doc.undo_order,
        &mut doc.history,
        &mut doc.pixel_history,
        &mut doc.selection,
    );
    clear_tint(workspace);
    Some(outcome)
}
