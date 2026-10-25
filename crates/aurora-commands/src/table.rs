//! The built-in table: every command the application offers.
//!
//! **The order of [`COMMANDS`] is the command palette's order** (the palette
//! lists the [`crate::Presence::Palette`] entries as they stand), so a new command
//! goes where it should appear. Ids are pinned by `tests/command_ids.tsv`;
//! `docs/command-registry.md` says how to add one.
//!
//! Titles are the palette titles. The in-window and native menus take their
//! labels from here too, except where a menu spells a command differently
//! (the Select menu's `Modify: Feather…`), which `aurora-app`'s menu model
//! overrides.

use crate::registry::Registry;
use crate::spec::{
    Access::{Read, Ui, Write},
    Category::{Colour, Edit, File, Layer, Paths, Select, Tool, View, Window},
    CommandSpec, FamilySpec, ParamDefault, ParamKind, ParamSpec, PathUse,
    Requirement::{
        CanRedo, CanUndo, ClipAllowed, Clipped, DeleteAllowed, NotClipped, NotQuickMask,
    },
    TargetKind,
};

pub(crate) static BUILTIN: Registry = Registry::new(COMMANDS, FAMILIES);

/// Name of a selection, saved selection, workspace or path: text the
/// application validates further (it refuses control and bidi characters).
const NAME_MAX_CHARS: usize = 64;

const PATH_TO_OPEN: ParamSpec = ParamSpec::required(
    "path",
    "The file to read. Interactive use picks it with a file dialog instead.",
    ParamKind::Path {
        mode: PathUse::Read,
    },
);

const PATH_TO_SAVE: ParamSpec = ParamSpec::required(
    "path",
    "The file to write; its extension chooses the format. Interactive use picks it with a file \
     dialog instead.",
    ParamKind::Path {
        mode: PathUse::Write,
    },
);

const NOT_IN_QUICK_MASK: &[crate::spec::Requirement] = &[NotQuickMask];

/// `Feather`'s radius is fractional; the others take whole pixels.
const FEATHER_PARAMS: &[ParamSpec] = &[ParamSpec::required(
    "radius",
    "The feather radius in pixels, 1 to 250 (fractions allowed).",
    ParamKind::Number {
        min: 1.0,
        max: 250.0,
    },
)];
const EXPAND_PARAMS: &[ParamSpec] = &[ParamSpec::required(
    "amount",
    "How many pixels to grow the selection by, 1 to 500.",
    ParamKind::Int { min: 1, max: 500 },
)];
const CONTRACT_PARAMS: &[ParamSpec] = &[ParamSpec::required(
    "amount",
    "How many pixels to shrink the selection by, 1 to 500.",
    ParamKind::Int { min: 1, max: 500 },
)];
const SMOOTH_PARAMS: &[ParamSpec] = &[ParamSpec::required(
    "amount",
    "The smoothing radius in pixels, 1 to 100.",
    ParamKind::Int { min: 1, max: 100 },
)];
const BORDER_PARAMS: &[ParamSpec] = &[ParamSpec::required(
    "amount",
    "The width of the border in pixels, 1 to 500.",
    ParamKind::Int { min: 1, max: 500 },
)];
const COLOUR_RANGE_PARAMS: &[ParamSpec] = &[ParamSpec::optional(
    "fuzziness",
    "How far a colour may differ from the foreground colour and still be selected, 0 to 200.",
    ParamKind::Int { min: 0, max: 200 },
    ParamDefault::Int(40),
)];
const SELECTION_NAME_PARAMS: &[ParamSpec] = &[ParamSpec::required(
    "name",
    "The name to save the selection under; an existing name is replaced.",
    ParamKind::Text {
        max_chars: NAME_MAX_CHARS,
    },
)];
const WORKSPACE_NAME_PARAMS: &[ParamSpec] = &[ParamSpec::required(
    "name",
    "The name of the workspace preset to save the current layout as ('Essentials' is reserved).",
    ParamKind::Text {
        max_chars: NAME_MAX_CHARS,
    },
)];
const OPEN_PARAMS: &[ParamSpec] = &[PATH_TO_OPEN];
const SAVE_PARAMS: &[ParamSpec] = &[PATH_TO_SAVE];

/// Every command, in palette order.
#[allow(clippy::too_many_lines)] // one entry per command
pub(crate) static COMMANDS: &[CommandSpec] = &[
    // -- Panels: focus, toggle, close (View menu) --
    CommandSpec::new(
        "view.focus_layers",
        "Focus Layers Panel",
        "Moves keyboard focus to the Layers panel.",
        View,
        Ui,
    ),
    CommandSpec::new(
        "view.focus_properties",
        "Focus Properties Panel",
        "Moves keyboard focus to the Properties panel, showing its tab first.",
        View,
        Ui,
    ),
    CommandSpec::new(
        "view.focus_history",
        "Focus History Panel",
        "Moves keyboard focus to the History panel, showing its tab first.",
        View,
        Ui,
    ),
    CommandSpec::new(
        "view.focus_channels",
        "Focus Channels Panel",
        "Moves keyboard focus to the Channels panel, showing its tab first.",
        View,
        Ui,
    ),
    CommandSpec::new(
        "view.focus_paths",
        "Focus Paths Panel",
        "Moves keyboard focus to the Paths panel, showing its tab first.",
        View,
        Ui,
    ),
    CommandSpec::new(
        "view.toggle_layers",
        "Toggle Layers Panel",
        "Collapses the Layers panel to its title row, or expands it again.",
        View,
        Ui,
    ),
    CommandSpec::new(
        "view.toggle_properties",
        "Toggle Properties Panel",
        "Shows the Properties tab expanded, or collapses its group when it is already shown.",
        View,
        Ui,
    ),
    CommandSpec::new(
        "view.toggle_history",
        "Toggle History Panel",
        "Shows the History tab expanded, or collapses its group when it is already shown.",
        View,
        Ui,
    ),
    CommandSpec::new(
        "view.toggle_channels",
        "Toggle Channels Panel",
        "Shows the Channels tab expanded, or collapses its group when it is already shown.",
        View,
        Ui,
    ),
    CommandSpec::new(
        "view.toggle_paths",
        "Toggle Paths Panel",
        "Shows the Paths tab expanded, or collapses its group when it is already shown.",
        View,
        Ui,
    ),
    CommandSpec::new(
        "view.close_layers",
        "Close Layers Panel",
        "Closes the Layers panel; the toggle command reopens it.",
        View,
        Ui,
    ),
    CommandSpec::new(
        "view.close_properties",
        "Close Properties Panel",
        "Closes the Properties panel; the toggle command reopens it.",
        View,
        Ui,
    ),
    CommandSpec::new(
        "view.close_history",
        "Close History Panel",
        "Closes the History panel; the toggle command reopens it.",
        View,
        Ui,
    ),
    CommandSpec::new(
        "view.close_channels",
        "Close Channels Panel",
        "Closes the Channels panel; the toggle command reopens it.",
        View,
        Ui,
    ),
    CommandSpec::new(
        "view.close_paths",
        "Close Paths Panel",
        "Closes the Paths panel; the toggle command reopens it.",
        View,
        Ui,
    ),
    CommandSpec::new(
        "window.toggle_panels",
        "Collapse or Expand Panels",
        "Collapses the whole right rail to a strip of labels, or expands it back.",
        Window,
        Ui,
    ),
    CommandSpec::new(
        "window.reset_panels",
        "Reset Panel Layout",
        "Applies the Essentials workspace and docks every floating panel.",
        Window,
        Ui,
    ),
    // -- Documents and files --
    CommandSpec::new(
        "file.new",
        "New Document",
        "Creates a new empty document in a new tab; the current one stays open.",
        File,
        Write,
    ),
    CommandSpec::new(
        "file.close",
        "Close Document",
        "Closes the active document, asking first when it has unsaved changes.",
        File,
        Write,
    ),
    CommandSpec::new(
        "file.open",
        "Open File\u{2026}",
        "Opens a PNG, JPEG, TIFF, PSD, PSB or Aurora (.aur) file in a new document tab.",
        File,
        Write,
    )
    .params(OPEN_PARAMS),
    CommandSpec::new(
        "document.next",
        "Next Document",
        "Makes the next document tab active.",
        File,
        Ui,
    ),
    CommandSpec::new(
        "document.previous",
        "Previous Document",
        "Makes the previous document tab active.",
        File,
        Ui,
    ),
    CommandSpec::new(
        "file.save",
        "Save As\u{2026}",
        "Saves the active document to a new file; the extension chooses .aur, .psd or an \
         export format. A PSD save that would lose something asks first.",
        File,
        Write,
    )
    .params(SAVE_PARAMS),
    CommandSpec::new(
        "file.check_psd",
        "Check PSD Compatibility\u{2026}",
        "Reports what saving the active document as a PSD would keep, change or not save, \
         without writing a file.",
        File,
        Read,
    ),
    // -- Edit --
    CommandSpec::new(
        "edit.undo",
        "Undo",
        "Undoes the last undoable step of the active document.",
        Edit,
        Write,
    )
    .requires(&[CanUndo]),
    CommandSpec::new(
        "edit.redo",
        "Redo",
        "Redoes the last undone step of the active document.",
        Edit,
        Write,
    )
    .requires(&[CanRedo]),
    CommandSpec::new(
        "view.toggle_widget_gallery",
        "Toggle Widget Gallery",
        "Opens or closes the developer panel showing every toolkit widget in every state.",
        View,
        Ui,
    ),
    // -- Layers --
    CommandSpec::new(
        "layer.new",
        "New Layer",
        "Adds an empty, document-sized layer above the active layer and makes it active.",
        Layer,
        Write,
    ),
    CommandSpec::new(
        "layer.delete",
        "Delete Layer",
        "Deletes the active layer or group with everything inside it. The last pixel layer \
         and a layer locked with Lock All cannot be deleted.",
        Layer,
        Write,
    )
    .requires(&[DeleteAllowed]),
    CommandSpec::new(
        "layer.new_curves",
        "New Curves Layer",
        "Adds a Curves adjustment layer (identity curves) above the active layer.",
        Layer,
        Write,
    ),
    CommandSpec::new(
        "layer.new_levels",
        "New Levels Layer",
        "Adds a Levels adjustment layer (identity levels) above the active layer.",
        Layer,
        Write,
    ),
    CommandSpec::new(
        "layer.new_hue_saturation",
        "New Hue/Saturation Layer",
        "Adds a Hue/Saturation adjustment layer (no change) above the active layer.",
        Layer,
        Write,
    ),
    CommandSpec::new(
        "layer.new_vibrance",
        "New Vibrance Layer",
        "Adds a Vibrance adjustment layer (no change) above the active layer.",
        Layer,
        Write,
    ),
    CommandSpec::new(
        "layer.new_exposure",
        "New Exposure Layer",
        "Adds an Exposure adjustment layer (no change) above the active layer.",
        Layer,
        Write,
    ),
    CommandSpec::new(
        "layer.new_photo_filter",
        "New Photo Filter Layer",
        "Adds a Photo Filter adjustment layer above the active layer.",
        Layer,
        Write,
    ),
    CommandSpec::new(
        "layer.new_black_white",
        "New Black & White Layer",
        "Adds a Black & White adjustment layer above the active layer.",
        Layer,
        Write,
    ),
    CommandSpec::new(
        "layer.new_color_balance",
        "New Color Balance Layer",
        "Adds a Color Balance adjustment layer (no change) above the active layer.",
        Layer,
        Write,
    ),
    CommandSpec::new(
        "layer.new_invert",
        "New Invert Layer",
        "Adds an Invert adjustment layer above the active layer.",
        Layer,
        Write,
    ),
    CommandSpec::new(
        "layer.new_threshold",
        "New Threshold Layer",
        "Adds a Threshold adjustment layer above the active layer.",
        Layer,
        Write,
    ),
    CommandSpec::new(
        "layer.new_posterize",
        "New Posterize Layer",
        "Adds a Posterize adjustment layer above the active layer.",
        Layer,
        Write,
    ),
    CommandSpec::new(
        "layer.new_brightness_contrast",
        "New Brightness/Contrast Layer",
        "Adds a Brightness/Contrast adjustment layer (no change) above the active layer.",
        Layer,
        Write,
    ),
    CommandSpec::new(
        "layer.new_channel_mixer",
        "New Channel Mixer Layer",
        "Adds a Channel Mixer adjustment layer (identity mix) above the active layer.",
        Layer,
        Write,
    ),
    CommandSpec::new(
        "layer.new_selective_color",
        "New Selective Color Layer",
        "Adds a Selective Color adjustment layer (no change) above the active layer.",
        Layer,
        Write,
    ),
    CommandSpec::new(
        "layer.new_gradient_map",
        "New Gradient Map Layer",
        "Adds a Gradient Map adjustment layer (black to white) above the active layer.",
        Layer,
        Write,
    ),
    CommandSpec::new(
        "layer.new_group",
        "New Group",
        "Adds an empty Pass Through group above the active layer.",
        Layer,
        Write,
    ),
    CommandSpec::new(
        "layer.clip_create",
        "Create Clipping Mask",
        "Clips the active layer to the layer below it. Not available for a group or the \
         bottom layer of its group.",
        Layer,
        Write,
    )
    .requires(&[NotClipped, ClipAllowed]),
    CommandSpec::new(
        "layer.clip_release",
        "Release Clipping Mask",
        "Releases the active layer's clipping mask.",
        Layer,
        Write,
    )
    .requires(&[Clipped]),
    // -- Colours and the view --
    CommandSpec::new(
        "colour.swap",
        "Swap Foreground and Background Colours",
        "Swaps the foreground and the background colour.",
        Colour,
        Ui,
    ),
    CommandSpec::new(
        "view.fit-on-screen",
        "Fit on Screen",
        "Zooms and centres the canvas so the whole document is visible.",
        View,
        Ui,
    ),
    CommandSpec::new(
        "view.actual-pixels",
        "100%",
        "Zooms so one image pixel is one physical screen pixel.",
        View,
        Ui,
    ),
    CommandSpec::new(
        "colour.reset",
        "Default Foreground and Background Colours",
        "Resets the colours to black over white.",
        Colour,
        Ui,
    ),
    // -- Selection --
    CommandSpec::new(
        "select.all",
        "Select All",
        "Selects the whole canvas as one undo step.",
        Select,
        Write,
    )
    .requires(NOT_IN_QUICK_MASK),
    CommandSpec::new(
        "select.deselect",
        "Deselect",
        "Clears the selection as one undo step.",
        Select,
        Write,
    )
    .requires(NOT_IN_QUICK_MASK),
    CommandSpec::new(
        "select.inverse",
        "Inverse Selection",
        "Inverts the selection as one undo step.",
        Select,
        Write,
    )
    .requires(NOT_IN_QUICK_MASK),
    CommandSpec::new(
        "select.quick_mask",
        "Toggle Quick Mask Mode",
        "Turns the selection into an editable mask painted with the Brush and Eraser, or turns \
         the mask back into a selection (one undo step).",
        Select,
        Write,
    ),
    CommandSpec::new(
        "select.crop_to_selection",
        "Crop to Selection",
        "Crops the canvas to the bounds of the selection as one undo step; nothing is deleted, \
         pixels outside stay in their layers.",
        Select,
        Write,
    )
    .requires(NOT_IN_QUICK_MASK),
    CommandSpec::new(
        "crop.swap_orientation",
        "Crop: Swap Orientation",
        "Swaps the Crop tool's ratio between landscape and portrait.",
        Tool,
        Ui,
    ),
    CommandSpec::new(
        "select.save",
        "Save Selection\u{2026}",
        "Saves the current selection under a name (an existing name is replaced).",
        Select,
        Write,
    )
    .params(SELECTION_NAME_PARAMS),
    // -- Clipboard --
    CommandSpec::new(
        "edit.copy",
        "Copy",
        "Copies the active layer's pixels under the selection to the clipboard.",
        Edit,
        Ui,
    )
    .requires(NOT_IN_QUICK_MASK),
    CommandSpec::new(
        "edit.cut",
        "Cut",
        "Copies the active layer's pixels under the selection to the clipboard and clears \
         them (one undo step).",
        Edit,
        Write,
    )
    .requires(NOT_IN_QUICK_MASK),
    CommandSpec::new(
        "edit.copy_merged",
        "Copy Merged",
        "Copies the visible composite under the selection to the clipboard.",
        Edit,
        Ui,
    )
    .requires(NOT_IN_QUICK_MASK),
    CommandSpec::new(
        "edit.paste",
        "Paste",
        "Pastes the clipboard as a new layer above the active layer (one undo step).",
        Edit,
        Write,
    )
    .requires(NOT_IN_QUICK_MASK),
    CommandSpec::new(
        "edit.paste_in_place",
        "Paste in Place",
        "Pastes the clipboard as a new layer at its original position (one undo step).",
        Edit,
        Write,
    )
    .requires(NOT_IN_QUICK_MASK),
    CommandSpec::new(
        "edit.layer_via_copy",
        "Layer via Copy",
        "Copies the selected pixels of the active layer into a new layer above it.",
        Edit,
        Write,
    )
    .requires(NOT_IN_QUICK_MASK),
    CommandSpec::new(
        "edit.layer_via_cut",
        "Layer via Cut",
        "Moves the selected pixels of the active layer into a new layer above it \
         (one undo step).",
        Edit,
        Write,
    )
    .requires(NOT_IN_QUICK_MASK),
    // -- Paths --
    CommandSpec::new(
        "paths.make_selection",
        "Make Selection from Path",
        "Turns the active path into the selection (one undo step).",
        Paths,
        Write,
    )
    .requires(NOT_IN_QUICK_MASK),
    CommandSpec::new(
        "paths.fill",
        "Fill Path",
        "Fills the active path with the foreground colour on the active pixel layer \
         (one undo step).",
        Paths,
        Write,
    )
    .requires(NOT_IN_QUICK_MASK),
    CommandSpec::new(
        "paths.stroke",
        "Stroke Path",
        "Strokes the active path with the Brush on the active pixel layer (one undo step).",
        Paths,
        Write,
    )
    .requires(NOT_IN_QUICK_MASK),
    CommandSpec::new(
        "paths.make_from_selection",
        "Make Work Path from Selection",
        "Replaces the work path with the outline of the selection (one undo step).",
        Paths,
        Write,
    )
    .requires(NOT_IN_QUICK_MASK),
    CommandSpec::new(
        "paths.new",
        "New Path",
        "Adds a new empty path and makes it active.",
        Paths,
        Write,
    ),
    CommandSpec::new(
        "paths.delete",
        "Delete Path",
        "Deletes the active path.",
        Paths,
        Write,
    ),
    CommandSpec::new(
        "paths.rename",
        "Rename Path\u{2026}",
        "Renames the active path (asks for the new name).",
        Paths,
        Write,
    ),
    CommandSpec::new(
        "paths.save_work",
        "Save Work Path\u{2026}",
        "Saves the work path as a named path (asks for the name).",
        Paths,
        Write,
    ),
    // -- Colour management --
    CommandSpec::new(
        "colour.output_profile.set",
        "Set Output Profile\u{2026}",
        "Sets the document's output ICC profile from a profile file; export converts to it \
         and embeds it.",
        Colour,
        Write,
    )
    .params(OPEN_PARAMS),
    CommandSpec::new(
        "colour.output_profile.clear",
        "Clear Output Profile (sRGB)",
        "Clears the document's output profile, so export writes sRGB.",
        Colour,
        Write,
    ),
    CommandSpec::new(
        "select.colour_range",
        "Select Colour Range\u{2026}",
        "Selects the pixels of the active layer whose colour is near the foreground colour.",
        Select,
        Write,
    )
    .requires(NOT_IN_QUICK_MASK)
    .params(COLOUR_RANGE_PARAMS),
    // -- Select > Modify --
    CommandSpec::new(
        "select.feather",
        "Feather Selection\u{2026}",
        "Softens the selection's edge with a Gaussian of the given radius (one undo step).",
        Select,
        Write,
    )
    .requires(NOT_IN_QUICK_MASK)
    .params(FEATHER_PARAMS),
    CommandSpec::new(
        "select.expand",
        "Expand Selection\u{2026}",
        "Grows the selection by the given number of pixels (one undo step).",
        Select,
        Write,
    )
    .requires(NOT_IN_QUICK_MASK)
    .params(EXPAND_PARAMS),
    CommandSpec::new(
        "select.contract",
        "Contract Selection\u{2026}",
        "Shrinks the selection by the given number of pixels (one undo step).",
        Select,
        Write,
    )
    .requires(NOT_IN_QUICK_MASK)
    .params(CONTRACT_PARAMS),
    CommandSpec::new(
        "select.smooth",
        "Smooth Selection\u{2026}",
        "Rounds off the selection's corners with a majority filter of the given radius \
         (one undo step).",
        Select,
        Write,
    )
    .requires(NOT_IN_QUICK_MASK)
    .params(SMOOTH_PARAMS),
    CommandSpec::new(
        "select.border",
        "Border Selection\u{2026}",
        "Replaces the selection by a border of the given width along its edge \
         (one undo step).",
        Select,
        Write,
    )
    .requires(NOT_IN_QUICK_MASK)
    .params(BORDER_PARAMS),
    // -- Photo Filter presets --
    CommandSpec::new(
        "photo_filter.preset.0",
        "Photo Filter: Warming Filter (85)",
        "Sets the active Photo Filter layer's colour to the Warming Filter (85) preset.",
        Layer,
        Write,
    ),
    CommandSpec::new(
        "photo_filter.preset.1",
        "Photo Filter: Warming Filter (LBA)",
        "Sets the active Photo Filter layer's colour to the Warming Filter (LBA) preset.",
        Layer,
        Write,
    ),
    CommandSpec::new(
        "photo_filter.preset.2",
        "Photo Filter: Cooling Filter (80)",
        "Sets the active Photo Filter layer's colour to the Cooling Filter (80) preset.",
        Layer,
        Write,
    ),
    CommandSpec::new(
        "photo_filter.preset.3",
        "Photo Filter: Cooling Filter (LBB)",
        "Sets the active Photo Filter layer's colour to the Cooling Filter (LBB) preset.",
        Layer,
        Write,
    ),
    // -- Crop ratios --
    CommandSpec::new(
        "crop.ratio.free",
        "Crop Ratio: Free",
        "Lets the Crop tool's rectangle take any shape.",
        Tool,
        Ui,
    ),
    CommandSpec::new(
        "crop.ratio.original",
        "Crop Ratio: Original",
        "Keeps the Crop tool's rectangle at the canvas's own aspect ratio.",
        Tool,
        Ui,
    ),
    CommandSpec::new(
        "crop.ratio.1_1",
        "Crop Ratio: 1:1",
        "Keeps the Crop tool's rectangle square.",
        Tool,
        Ui,
    ),
    CommandSpec::new(
        "crop.ratio.3_2",
        "Crop Ratio: 3:2",
        "Keeps the Crop tool's rectangle at 3:2.",
        Tool,
        Ui,
    ),
    CommandSpec::new(
        "crop.ratio.4_3",
        "Crop Ratio: 4:3",
        "Keeps the Crop tool's rectangle at 4:3.",
        Tool,
        Ui,
    ),
    CommandSpec::new(
        "crop.ratio.5_4",
        "Crop Ratio: 5:4",
        "Keeps the Crop tool's rectangle at 5:4.",
        Tool,
        Ui,
    ),
    CommandSpec::new(
        "crop.ratio.16_9",
        "Crop Ratio: 16:9",
        "Keeps the Crop tool's rectangle at 16:9.",
        Tool,
        Ui,
    ),
    CommandSpec::new(
        "crop.ratio.16_10",
        "Crop Ratio: 16:10",
        "Keeps the Crop tool's rectangle at 16:10.",
        Tool,
        Ui,
    ),
    // -- Selection modes --
    CommandSpec::new(
        "selection.mode.new",
        "Selection Mode: New",
        "The selection tools replace the selection.",
        Select,
        Ui,
    ),
    CommandSpec::new(
        "selection.mode.add",
        "Selection Mode: Add",
        "The selection tools add to the selection.",
        Select,
        Ui,
    ),
    CommandSpec::new(
        "selection.mode.subtract",
        "Selection Mode: Subtract",
        "The selection tools subtract from the selection.",
        Select,
        Ui,
    ),
    CommandSpec::new(
        "selection.mode.intersect",
        "Selection Mode: Intersect",
        "The selection tools intersect with the selection.",
        Select,
        Ui,
    ),
    // -- Tools --
    CommandSpec::new(
        "tool.move",
        "Select Move Tool",
        "Makes Move the active tool: drags the active layer.",
        Tool,
        Ui,
    ),
    CommandSpec::new(
        "tool.marquee_select",
        "Select Marquee Select Tool",
        "Makes the rectangular Marquee the active tool.",
        Tool,
        Ui,
    ),
    CommandSpec::new(
        "tool.elliptical_marquee",
        "Select Elliptical Marquee Tool",
        "Makes the Elliptical Marquee the active tool.",
        Tool,
        Ui,
    ),
    CommandSpec::new(
        "tool.lasso",
        "Select Lasso Tool",
        "Makes the freehand Lasso the active tool.",
        Tool,
        Ui,
    ),
    CommandSpec::new(
        "tool.polygonal_lasso",
        "Select Polygonal Lasso Tool",
        "Makes the Polygonal Lasso the active tool.",
        Tool,
        Ui,
    ),
    CommandSpec::new(
        "tool.magic_wand",
        "Select Magic Wand Tool",
        "Makes the Magic Wand the active tool.",
        Tool,
        Ui,
    ),
    CommandSpec::new(
        "tool.quick_select",
        "Select Quick Select Tool",
        "Makes Quick Select the active tool.",
        Tool,
        Ui,
    ),
    CommandSpec::new(
        "tool.crop",
        "Select Crop Tool",
        "Makes Crop the active tool.",
        Tool,
        Ui,
    ),
    CommandSpec::new(
        "tool.zoom",
        "Select Zoom Tool",
        "Makes Zoom the active tool.",
        Tool,
        Ui,
    ),
    CommandSpec::new(
        "tool.pan",
        "Select Pan Tool",
        "Makes Pan (the hand) the active tool.",
        Tool,
        Ui,
    ),
    CommandSpec::new(
        "tool.eyedropper",
        "Select Eyedropper Tool",
        "Makes the Eyedropper the active tool: click to pick the foreground colour.",
        Tool,
        Ui,
    ),
    CommandSpec::new(
        "tool.brush",
        "Select Brush Tool",
        "Makes the Brush the active tool.",
        Tool,
        Ui,
    ),
    CommandSpec::new(
        "tool.pencil",
        "Select Pencil Tool",
        "Makes the hard-edged Pencil the active tool.",
        Tool,
        Ui,
    ),
    CommandSpec::new(
        "tool.eraser",
        "Select Eraser Tool",
        "Makes the Eraser the active tool.",
        Tool,
        Ui,
    ),
    CommandSpec::new(
        "tool.clone_stamp",
        "Select Clone Stamp Tool",
        "Makes the Clone Stamp the active tool (Alt-click sets the source).",
        Tool,
        Ui,
    ),
    CommandSpec::new(
        "tool.smudge",
        "Select Smudge Tool",
        "Makes the Smudge the active tool.",
        Tool,
        Ui,
    ),
    CommandSpec::new(
        "tool.spot_healing",
        "Select Spot Healing Tool",
        "Makes Spot Healing the active tool.",
        Tool,
        Ui,
    ),
    CommandSpec::new(
        "tool.blur",
        "Select Blur Tool",
        "Makes the Blur brush the active tool.",
        Tool,
        Ui,
    ),
    CommandSpec::new(
        "tool.sharpen",
        "Select Sharpen Tool",
        "Makes the Sharpen brush the active tool.",
        Tool,
        Ui,
    ),
    CommandSpec::new(
        "tool.dodge",
        "Select Dodge Tool",
        "Makes the Dodge brush the active tool.",
        Tool,
        Ui,
    ),
    CommandSpec::new(
        "tool.burn",
        "Select Burn Tool",
        "Makes the Burn brush the active tool.",
        Tool,
        Ui,
    ),
    CommandSpec::new(
        "tool.sponge",
        "Select Sponge Tool",
        "Makes the Sponge brush the active tool.",
        Tool,
        Ui,
    ),
    CommandSpec::new(
        "tool.pen",
        "Select Pen Tool",
        "Makes the Pen the active tool.",
        Tool,
        Ui,
    ),
    CommandSpec::new(
        "tool.gradient",
        "Select Gradient Tool",
        "Makes the Gradient the active tool.",
        Tool,
        Ui,
    ),
    CommandSpec::new(
        "tool.paint_bucket",
        "Select Paint Bucket Tool",
        "Makes the Paint Bucket the active tool.",
        Tool,
        Ui,
    ),
    // -- Moving panels (keyboard path of drag-to-redock) --
    CommandSpec::new(
        "view.move_layers_up",
        "Move Layers Panel Up",
        "Moves the Layers panel up one place in the rail.",
        Window,
        Ui,
    ),
    CommandSpec::new(
        "view.move_layers_down",
        "Move Layers Panel Down",
        "Moves the Layers panel down one place in the rail.",
        Window,
        Ui,
    ),
    CommandSpec::new(
        "view.move_layers_next_group",
        "Move Layers Panel to Next Group",
        "Moves the Layers panel into the next tab group of the rail.",
        Window,
        Ui,
    ),
    CommandSpec::new(
        "view.move_properties_up",
        "Move Properties Panel Up",
        "Moves the Properties panel up one place in the rail.",
        Window,
        Ui,
    ),
    CommandSpec::new(
        "view.move_properties_down",
        "Move Properties Panel Down",
        "Moves the Properties panel down one place in the rail.",
        Window,
        Ui,
    ),
    CommandSpec::new(
        "view.move_properties_next_group",
        "Move Properties Panel to Next Group",
        "Moves the Properties panel into the next tab group of the rail.",
        Window,
        Ui,
    ),
    CommandSpec::new(
        "view.move_history_up",
        "Move History Panel Up",
        "Moves the History panel up one place in the rail.",
        Window,
        Ui,
    ),
    CommandSpec::new(
        "view.move_history_down",
        "Move History Panel Down",
        "Moves the History panel down one place in the rail.",
        Window,
        Ui,
    ),
    CommandSpec::new(
        "view.move_history_next_group",
        "Move History Panel to Next Group",
        "Moves the History panel into the next tab group of the rail.",
        Window,
        Ui,
    ),
    CommandSpec::new(
        "view.move_channels_up",
        "Move Channels Panel Up",
        "Moves the Channels panel up one place in the rail.",
        Window,
        Ui,
    ),
    CommandSpec::new(
        "view.move_channels_down",
        "Move Channels Panel Down",
        "Moves the Channels panel down one place in the rail.",
        Window,
        Ui,
    ),
    CommandSpec::new(
        "view.move_channels_next_group",
        "Move Channels Panel to Next Group",
        "Moves the Channels panel into the next tab group of the rail.",
        Window,
        Ui,
    ),
    CommandSpec::new(
        "view.move_paths_up",
        "Move Paths Panel Up",
        "Moves the Paths panel up one place in the rail.",
        Window,
        Ui,
    ),
    CommandSpec::new(
        "view.move_paths_down",
        "Move Paths Panel Down",
        "Moves the Paths panel down one place in the rail.",
        Window,
        Ui,
    ),
    CommandSpec::new(
        "view.move_paths_next_group",
        "Move Paths Panel to Next Group",
        "Moves the Paths panel into the next tab group of the rail.",
        Window,
        Ui,
    ),
    // -- Floating and docking panels --
    CommandSpec::new(
        "view.float_layers",
        "Float Layers Panel",
        "Turns the Layers panel into a floating panel over the canvas.",
        Window,
        Ui,
    ),
    CommandSpec::new(
        "view.dock_layers",
        "Dock Layers Panel",
        "Docks the Layers panel back at the end of the rail.",
        Window,
        Ui,
    ),
    CommandSpec::new(
        "view.float_properties",
        "Float Properties Panel",
        "Turns the Properties panel into a floating panel over the canvas.",
        Window,
        Ui,
    ),
    CommandSpec::new(
        "view.dock_properties",
        "Dock Properties Panel",
        "Docks the Properties panel back at the end of the rail.",
        Window,
        Ui,
    ),
    CommandSpec::new(
        "view.float_history",
        "Float History Panel",
        "Turns the History panel into a floating panel over the canvas.",
        Window,
        Ui,
    ),
    CommandSpec::new(
        "view.dock_history",
        "Dock History Panel",
        "Docks the History panel back at the end of the rail.",
        Window,
        Ui,
    ),
    CommandSpec::new(
        "view.float_channels",
        "Float Channels Panel",
        "Turns the Channels panel into a floating panel over the canvas.",
        Window,
        Ui,
    ),
    CommandSpec::new(
        "view.dock_channels",
        "Dock Channels Panel",
        "Docks the Channels panel back at the end of the rail.",
        Window,
        Ui,
    ),
    CommandSpec::new(
        "view.float_paths",
        "Float Paths Panel",
        "Turns the Paths panel into a floating panel over the canvas.",
        Window,
        Ui,
    ),
    CommandSpec::new(
        "view.dock_paths",
        "Dock Paths Panel",
        "Docks the Paths panel back at the end of the rail.",
        Window,
        Ui,
    ),
    // -- Not in the static palette --
    CommandSpec::new(
        "app.quit",
        "Quit Aurora",
        "Quits the application after reviewing every document with unsaved changes.",
        File,
        Write,
    )
    .menu_only(),
    CommandSpec::new(
        "workspace.reset_active",
        "Reset Workspace",
        "Re-applies the active workspace preset, discarding the live layout changes. The \
         palette titles it 'Reset <name>'.",
        Window,
        Ui,
    )
    .dynamic(),
    CommandSpec::new(
        "workspace.save_as",
        "Save Workspace As\u{2026}",
        "Saves the current panel layout as a named workspace preset.",
        Window,
        Ui,
    )
    .params(WORKSPACE_NAME_PARAMS)
    .dynamic(),
];

/// Commands whose ids the application makes up at run time.
pub(crate) static FAMILIES: &[FamilySpec] = &[
    FamilySpec {
        prefix: "workspace.switch/",
        title_pattern: "Workspace: <name>",
        docs: "Applies a built-in or saved workspace preset.",
        category: Window,
        access: Ui,
        requires: &[],
        target_name: "name",
        target: TargetKind::Name {
            max_chars: NAME_MAX_CHARS,
        },
    },
    FamilySpec {
        prefix: "workspace.delete/",
        title_pattern: "Delete Workspace: <name>",
        docs: "Deletes a saved workspace preset (a built-in one cannot be deleted).",
        category: Window,
        access: Ui,
        requires: &[],
        target_name: "name",
        target: TargetKind::Name {
            max_chars: NAME_MAX_CHARS,
        },
    },
    FamilySpec {
        prefix: "document.switch.",
        title_pattern: "Document: <name>",
        docs: "Makes an open document the active one; the id is the document's session id.",
        category: File,
        access: Ui,
        requires: &[],
        target_name: "id",
        target: TargetKind::Id,
    },
    FamilySpec {
        prefix: "select.load:",
        title_pattern: "Load Selection: <name>",
        docs: "Loads a saved selection as the active selection (one undo step).",
        category: Select,
        access: Write,
        requires: NOT_IN_QUICK_MASK,
        target_name: "name",
        target: TargetKind::Name {
            max_chars: NAME_MAX_CHARS,
        },
    },
    FamilySpec {
        prefix: "select.delete_saved:",
        title_pattern: "Delete Saved Selection: <name>",
        docs: "Deletes a saved selection (not an undo step; the active selection is untouched).",
        category: Select,
        access: Write,
        requires: &[],
        target_name: "name",
        target: TargetKind::Name {
            max_chars: NAME_MAX_CHARS,
        },
    },
];
