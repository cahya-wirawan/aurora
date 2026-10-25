//! A headless document session for the MCP server (task 159): open a file,
//! describe it, run the commands that make sense without a window, render
//! a preview, save, and check a PSD save, all through the application's own
//! document code. No window, no event loop and no GPU are involved: the
//! composite is the CPU export path.
//!
//! `aurora-cli mcp` wires this into the protocol crate `aurora-mcp`; this
//! module knows nothing of the protocol and nothing of the caller's paths:
//! it is handed private scratch paths only (`aurora-mcp` confines, stages
//! and copies every caller file; see ADR 0014).
//!
//! **Which registry commands run here** ([`HEADLESS_COMMANDS`]): the ones
//! whose whole effect is on the document model: Undo, Redo, New Layer,
//! New Group, Delete Layer, Select All, Deselect, Inverse Selection, the
//! five Select > Modify commands, Check PSD Compatibility. Open and Save As
//! are run by the server (they touch files). Everything else needs a
//! window (tools, panels, the clipboard, the Pen, the crop tool, Quick
//! Mask, colour pickers) or has no headless form yet and is refused with
//! a clear error.

use std::path::Path;

use aurora_commands::{CommandContext, Validated};
use aurora_doc::{LayerId, LayerKind};
use serde_json::{Value, json};

use crate::background_open::DecodedFile;
use crate::prepared_pixels::{PreparedLayer, PreparedPsd};
use crate::{UndoDirection, UndoKind, UndoOrder, selection_view};

/// The registry commands [`HeadlessSession::run_command`] runs itself (the
/// server adds `file.open` and `file.save`).
pub const HEADLESS_COMMANDS: &[&str] = &[
    "edit.undo",
    "edit.redo",
    "layer.new",
    "layer.new_group",
    "layer.delete",
    "select.all",
    "select.deselect",
    "select.inverse",
    "select.feather",
    "select.expand",
    "select.contract",
    "select.smooth",
    "select.border",
    "file.check_psd",
];

/// The most layers `inspect` lists before it says it cut the list.
const MAX_INSPECT_LAYERS: usize = 2000;
/// The deepest group nesting `inspect` follows.
const MAX_INSPECT_DEPTH: usize = 64;
/// The most pixels a preview or an export composites (a guard against a
/// request that would allocate gigabytes).
const MAX_COMPOSITE_PIXELS: u64 = 200_000_000;

/// Why a session operation did not complete.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    NoDocument,
    NotAvailable(String),
    Refused(String),
    Failed(String),
    Cancelled,
}

fn failed(message: impl std::fmt::Display) -> Failure {
    Failure::Failed(message.to_string())
}

/// A rendered preview.
#[derive(Clone, Debug)]
pub struct PreviewImage {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub document_width: u32,
    pub document_height: u32,
}

/// A format [`HeadlessSession::export`] writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportKind {
    Png,
    Jpeg,
    Tiff,
    Psd,
    Aur,
}

/// What an export did.
#[derive(Clone, Debug)]
pub enum ExportResult {
    Written(Value),
    NeedsConfirmation(Value),
}

/// The one open document and everything its editing needs.
struct Doc {
    name: String,
    format: &'static str,
    layers: aurora_doc::LayerTree,
    history: aurora_doc::History,
    pixel_history: aurora_brush::PixelHistory,
    undo_order: UndoOrder,
    selection: aurora_doc::SelectionSet,
    canvas: (u32, u32),
    active: Option<LayerId>,
    import_losses: Vec<String>,
    notes: Vec<String>,
    colour: aurora_io::DocumentColour,
    skipped: aurora_io::SkippedTiles,
    id: crate::document_session::DocumentId,
    store: aurora_tile::TileStore,
    // Dropped last: the store's scratch directory lives inside it.
    _scratch: tempfile::TempDir,
}

/// A headless session: at most one open document.
#[derive(Default)]
pub struct HeadlessSession {
    doc: Option<Doc>,
}

impl std::fmt::Debug for HeadlessSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeadlessSession")
            .field("open", &self.doc.is_some())
            .finish()
    }
}

fn rect_json(r: &aurora_core::Rect) -> Value {
    json!({"x": r.x, "y": r.y, "width": r.width, "height": r.height})
}

fn selection_json(selection: &aurora_doc::Selection) -> Value {
    let shape = match selection.shape {
        aurora_doc::SelectionShape::Rect => "rectangle",
        aurora_doc::SelectionShape::Ellipse => "ellipse",
        aurora_doc::SelectionShape::Mask(_) => "mask",
    };
    json!({
        "bounds": rect_json(&selection.bounds),
        "inverted": selection.inverted,
        "shape": shape,
    })
}

fn kind_name(kind: &LayerKind) -> &'static str {
    match kind {
        LayerKind::Pixel { .. } => "pixel",
        LayerKind::Group { .. } => "group",
        LayerKind::Adjustment(_) => "adjustment",
    }
}

fn node_json(
    doc: &Doc,
    id: LayerId,
    depth: usize,
    count: &mut usize,
    truncated: &mut bool,
) -> Option<Value> {
    if *count >= MAX_INSPECT_LAYERS || depth > MAX_INSPECT_DEPTH {
        *truncated = true;
        return None;
    }
    *count += 1;
    let layers = &doc.layers;
    let kind = layers.kind(id)?;
    let mut node = json!({
        "id": id.to_raw(),
        "name": aurora_doc::sanitize_display_name(layers.name(id).unwrap_or("")),
        "kind": kind_name(kind),
        "visible": layers.visible(id),
        "opacity": layers.opacity(id),
        "fill": layers.fill_opacity(id),
        "blend_mode": layers.blend_mode(id).map(|m| format!("{m:?}")),
        "bounds": layers.bounds(id).map(|r| rect_json(&r)),
        "clipped": layers.clipped(id),
        "has_mask": layers.mask(id).is_some(),
        "active": doc.active == Some(id),
    });
    if let Some(children) = layers.children(id) {
        let kids: Vec<Value> = children
            .iter()
            .filter_map(|&child| node_json(doc, child, depth + 1, count, truncated))
            .collect();
        if let Some(object) = node.as_object_mut() {
            object.insert("children".into(), Value::Array(kids));
        }
    }
    Some(node)
}

fn category_name(category: aurora_io::ReportCategory) -> &'static str {
    match category {
        aurora_io::ReportCategory::NeedsAttention => "needs_attention",
        aurora_io::ReportCategory::NotSaved => "not_saved",
        aurora_io::ReportCategory::Changed => "changed",
        aurora_io::ReportCategory::FromOpen => "from_open",
        aurora_io::ReportCategory::Kept => "kept",
    }
}

/// A PSD write report as grouped JSON (the dialog's groups, in its order).
fn report_json(name: &str, report: &aurora_io::PsdWriteReport) -> Value {
    let grouped = crate::psd_report::SaveReport::new(report);
    let groups: Vec<Value> = grouped
        .groups
        .iter()
        .map(|g| {
            json!({
                "category": category_name(g.category),
                "heading": crate::psd_report::heading(g.category),
                "entries": g.entries.iter().map(|e| json!({
                    "text": e.text,
                    "affected": e.affected,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "asks": grouped.asks(),
        "summary": grouped.summary(),
        "groups": groups,
        "message": crate::psd_report::check_message(name, report),
        "verified_in_photoshop": false,
    })
}

impl HeadlessSession {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn has_document(&self) -> bool {
        self.doc.is_some()
    }

    #[must_use]
    pub fn headless_commands(&self) -> &'static [&'static str] {
        HEADLESS_COMMANDS
    }

    fn doc(&self) -> Result<&Doc, Failure> {
        self.doc.as_ref().ok_or(Failure::NoDocument)
    }

    fn doc_mut(&mut self) -> Result<&mut Doc, Failure> {
        self.doc.as_mut().ok_or(Failure::NoDocument)
    }

    /// Opens the file at `staged` (a private copy; `name` is the original
    /// file name) as the session's document, replacing any previous one.
    ///
    /// # Errors
    /// The file cannot be decoded or installed.
    pub fn open(&mut self, staged: &Path, name: &str) -> Result<Value, Failure> {
        let scratch = tempfile::Builder::new()
            .prefix("aurora-mcp-doc-")
            .tempdir()
            .map_err(|e| failed(format!("scratch directory: {e}")))?;
        let mut store =
            aurora_tile::TileStore::new_shared(scratch.path().to_path_buf(), crate::tile_pool())
                .map_err(|e| failed(format!("tile store: {e:?}")))?;
        let staging = store.staging_root();
        let stem = Path::new(name)
            .file_stem()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("Image")
            .to_owned();
        let decoded = crate::decode_chosen_file_with(staged, Some(&staging))
            .map_err(|e| Failure::Refused(format!("couldn't open {name}: {e:?}")))?;
        let mut notes: Vec<String> = Vec::new();
        let mut import_losses: Vec<String> = Vec::new();
        let mut skipped = aurora_io::SkippedTiles::default();
        let mut selection = aurora_doc::SelectionSet::new();
        let mut colour = aurora_io::DocumentColour::default();
        let (format, layers, history, canvas) = match decoded {
            DecodedFile::Psd(psd) => {
                let PreparedPsd {
                    layers,
                    history,
                    canvas_size,
                    pixels,
                    masks,
                    report,
                    colour: imported,
                    selections,
                    ..
                } = psd;
                let (_freed, bad) = crate::replace_document_pixels_prepared(
                    &mut store,
                    aurora_doc::LayerTree::new(),
                    aurora_doc::History::new(),
                    &layers,
                    pixels,
                    masks,
                );
                if bad > 0 {
                    return Err(failed(format!("{bad} tiles could not be written")));
                }
                if !selections.is_empty() {
                    match aurora_doc::SelectionSet::from_parts(None, selections) {
                        Ok(set) => selection = set,
                        Err(err) => tracing::warn!(?err, "the file's alpha channels were refused"),
                    }
                }
                notes.extend(report.items.iter().cloned());
                import_losses = report.items;
                colour.source = imported.source;
                ("psd", layers, history, canvas_size)
            }
            DecodedFile::Image(image) => {
                let size = image.size;
                notes.extend(image.colour.report.warnings.iter().cloned());
                notes.extend(image.colour.report.info.iter().cloned());
                colour.source = image.colour.source.clone();
                let (layers, history, layer_id) = crate::document_from_size(stem.clone(), size);
                let (_freed, bad) = crate::replace_document_pixels_prepared(
                    &mut store,
                    aurora_doc::LayerTree::new(),
                    aurora_doc::History::new(),
                    &layers,
                    vec![PreparedLayer {
                        layer: layer_id,
                        tiles: image.tiles,
                    }],
                    Vec::new(),
                );
                if bad > 0 {
                    return Err(failed(format!("{bad} tiles could not be written")));
                }
                ("image", layers, history, size)
            }
            DecodedFile::Aur(bytes) => {
                let document = crate::read_prechecked_aur(&bytes, &mut store)
                    .map_err(|e| Failure::Refused(format!("couldn't open {name}: {e:?}")))?;
                if let Some(losses) = document.import_losses {
                    notes.extend(losses.iter().cloned());
                    import_losses = losses;
                }
                if let Some(reason) = document.selection_dropped {
                    notes.push(reason);
                }
                skipped = document.skipped_tiles;
                selection = document.selection;
                colour = document.colour;
                ("aur", document.layers, document.history, document.canvas_size)
            }
        };
        let active = crate::topmost_pixel_layer(&layers);
        let doc = Doc {
            name: stem,
            format,
            layers,
            history,
            pixel_history: aurora_brush::PixelHistory::new(),
            undo_order: UndoOrder::default(),
            selection,
            canvas,
            active,
            import_losses,
            notes,
            colour,
            skipped,
            id: crate::document_session::DocumentId::next(),
            store,
            _scratch: scratch,
        };
        let info = json!({
            "name": doc.name,
            "format": doc.format,
            "width": doc.canvas.0,
            "height": doc.canvas.1,
            "layers": doc.layers.len(),
            "notes": doc.notes,
        });
        self.doc = Some(doc);
        Ok(info)
    }

    /// The document as JSON.
    ///
    /// # Errors
    /// None today; the result type keeps the host trait uniform.
    pub fn inspect(&mut self) -> Result<Value, Failure> {
        let Some(doc) = self.doc.as_mut() else {
            return Ok(json!({"document": null}));
        };
        doc.undo_order.sync_labels();
        let mut count = 0;
        let mut truncated = false;
        let tree: Vec<Value> = doc
            .layers
            .roots()
            .iter()
            .filter_map(|&id| node_json(doc, id, 0, &mut count, &mut truncated))
            .collect();
        let mut redo: Vec<&String> = doc.undo_order.redo_labels.iter().collect();
        redo.reverse();
        Ok(json!({"document": {
            "name": doc.name,
            "format": doc.format,
            "canvas": {"width": doc.canvas.0, "height": doc.canvas.1},
            "layer_count": doc.layers.len(),
            "layers": tree,
            "layers_truncated": truncated,
            "active_layer": doc.active.map(LayerId::to_raw),
            "history": {
                "origin": doc.undo_order.origin,
                "steps": doc.undo_order.undo_labels,
                "undone": redo,
                "can_undo": !doc.undo_order.undo.is_empty(),
                "can_redo": !doc.undo_order.redo.is_empty(),
            },
            "selection": {
                "active": doc.selection.active().as_ref().map(selection_json),
                "saved": doc.selection.saved_sorted().iter().map(|(n, _)| *n).collect::<Vec<_>>(),
            },
            "paths": doc.selection.paths().len(),
            "notes": doc.notes,
        }}))
    }

    /// The facts the registry's enabled checks need.
    #[must_use]
    pub fn command_context(&self) -> CommandContext {
        let Some(doc) = self.doc.as_ref() else {
            return CommandContext::default();
        };
        CommandContext {
            can_undo: !doc.undo_order.undo.is_empty(),
            can_redo: !doc.undo_order.redo.is_empty(),
            delete_allowed: crate::delete_allowed(&doc.layers, doc.active)
                && doc
                    .active
                    .and_then(|id| doc.layers.lock(id))
                    .is_none_or(|lock| crate::delete_refusal(lock).is_none()),
            quick_mask: false,
            clipped: doc.active.is_some_and(|id| doc.layers.clipped(id)),
            clip_refused: doc
                .active
                .is_none_or(|id| crate::clip_refusal(&doc.layers, id).is_some()),
        }
    }

    /// Runs a validated command.
    ///
    /// # Errors
    /// [`Failure::NotAvailable`] for a command that needs a window.
    pub fn run_command(&mut self, call: &Validated) -> Result<Value, Failure> {
        let id = call.id.as_str();
        match id {
            "file.check_psd" => return self.psd_check(&|| false),
            "edit.undo" => return self.undo_redo(UndoDirection::Undo),
            "edit.redo" => return self.undo_redo(UndoDirection::Redo),
            _ => {}
        }
        let doc = self.doc_mut()?;
        match id {
            "layer.new" => {
                let placement = crate::new_layer_placement(&doc.layers, doc.active);
                let name = crate::next_layer_name(&doc.layers);
                let bounds = crate::new_layer_bounds(&doc.layers, doc.canvas);
                let new = doc
                    .history
                    .add_pixel_layer_at(&mut doc.layers, name, bounds, placement.parent, placement.index)
                    .map_err(|e| Failure::Refused(format!("New Layer was refused: {e:?}")))?;
                doc.undo_order
                    .record(UndoKind::Structural, &mut doc.history, &mut doc.pixel_history);
                doc.active = Some(new);
                Ok(json!({"changed": true, "layer": new.to_raw(), "label": "New Layer"}))
            }
            "layer.new_group" => {
                let placement = crate::new_layer_placement(&doc.layers, doc.active);
                let name = crate::next_numbered_name(&doc.layers, "Group ");
                let new = doc
                    .history
                    .add_group_at(
                        &mut doc.layers,
                        name,
                        placement.parent,
                        placement.index,
                        aurora_doc::BlendMode::PassThrough,
                    )
                    .map_err(|e| Failure::Refused(format!("New Group was refused: {e:?}")))?;
                doc.undo_order
                    .record(UndoKind::Structural, &mut doc.history, &mut doc.pixel_history);
                doc.active = Some(new);
                Ok(json!({"changed": true, "layer": new.to_raw(), "label": "New Group"}))
            }
            "layer.delete" => {
                let target = doc
                    .active
                    .filter(|&id| doc.layers.contains(id))
                    .ok_or_else(|| Failure::Refused("there is no active layer".to_owned()))?;
                if doc
                    .layers
                    .lock(target)
                    .is_some_and(|lock| crate::delete_refusal(lock).is_some())
                {
                    return Err(Failure::Refused("the layer is locked (all)".to_owned()));
                }
                if !crate::delete_allowed(&doc.layers, Some(target)) {
                    return Err(Failure::Refused(
                        "the last pixel layer cannot be deleted".to_owned(),
                    ));
                }
                let position = crate::layer_position(&doc.layers, target);
                doc.history
                    .remove(&mut doc.layers, target)
                    .map_err(|e| Failure::Refused(format!("Delete Layer was refused: {e:?}")))?;
                doc.undo_order
                    .record(UndoKind::Structural, &mut doc.history, &mut doc.pixel_history);
                doc.active = position.and_then(|p| crate::layer_at_or_near(&doc.layers, p));
                Ok(json!({"changed": true, "label": "Delete Layer"}))
            }
            "select.all" | "select.deselect" | "select.inverse" => {
                let command = match id {
                    "select.all" => selection_view::SelectionCommand::SelectAll,
                    "select.deselect" => selection_view::SelectionCommand::Deselect,
                    _ => selection_view::SelectionCommand::Inverse,
                };
                let before = doc.selection.active();
                let changed = selection_view::apply_selection_command(
                    &mut doc.selection,
                    command,
                    doc.canvas,
                );
                if changed {
                    doc.undo_order.record_selection(
                        command.label(),
                        before,
                        doc.selection.active(),
                        &mut doc.history,
                        &mut doc.pixel_history,
                    );
                }
                Ok(json!({"changed": changed, "label": command.label()}))
            }
            "select.feather" | "select.expand" | "select.contract" | "select.smooth"
            | "select.border" => Self::refine(doc, call),
            other => Err(Failure::NotAvailable(format!(
                "`{other}` is not available in the headless server"
            ))),
        }
    }

    fn refine(doc: &mut Doc, call: &Validated) -> Result<Value, Failure> {
        let Some(current) = doc.selection.active() else {
            return Err(Failure::Refused("there is no selection to modify".to_owned()));
        };
        let (label, result) = match call.id.as_str() {
            "select.feather" => {
                let radius = call.number("radius").unwrap_or(0.0) as f32;
                ("Feather Selection", aurora_doc::feather(&current, radius))
            }
            other => {
                let amount = u32::try_from(call.int("amount").unwrap_or(0)).unwrap_or(0);
                match other {
                    "select.expand" => ("Expand Selection", aurora_doc::expand(&current, amount)),
                    "select.contract" => {
                        ("Contract Selection", aurora_doc::contract(&current, amount))
                    }
                    "select.smooth" => ("Smooth Selection", aurora_doc::smooth(&current, amount)),
                    _ => ("Border Selection", aurora_doc::border(&current, amount)),
                }
            }
        };
        let refined = result.map_err(|e| Failure::Refused(format!("{label} was refused: {e:?}")))?;
        match &refined {
            Some(selection) => doc.selection.select(selection.clone()),
            None => doc.selection.deselect(),
        }
        let recorded = doc.undo_order.record_selection(
            label,
            Some(current),
            doc.selection.active(),
            &mut doc.history,
            &mut doc.pixel_history,
        );
        Ok(json!({"changed": recorded, "label": label, "selection_empty": refined.is_none()}))
    }

    fn undo_redo(&mut self, direction: UndoDirection) -> Result<Value, Failure> {
        let doc = self.doc_mut()?;
        let applied = crate::step_undo_order(
            &mut doc.layers,
            &mut doc.history,
            &mut doc.pixel_history,
            Some(&mut doc.store),
            &mut doc.undo_order,
            direction,
        )
        .is_some()
            // A selection step moves its own stacks and leaves the target
            // selection to put in force.
            || doc.undo_order.pending_selection.is_some();
        selection_view::apply_pending_selection(&mut doc.undo_order, &mut doc.selection);
        if doc.active.is_none_or(|id| !doc.layers.contains(id)) {
            doc.active = crate::topmost_pixel_layer(&doc.layers);
        }
        Ok(json!({"changed": applied}))
    }

    /// Composites the whole document on the CPU, one tile at a time, polling
    /// `cancelled` and reporting a fraction.
    fn composite(
        doc: &mut Doc,
        cancelled: &dyn Fn() -> bool,
        progress: &mut dyn FnMut(f64),
    ) -> Result<aurora_io::Image, Failure> {
        let (width, height) = doc.canvas;
        if u64::from(width) * u64::from(height) > MAX_COMPOSITE_PIXELS {
            return Err(Failure::Refused(format!(
                "the document is {width} x {height} px; this server composites at most \
                 {MAX_COMPOSITE_PIXELS} pixels"
            )));
        }
        let mut samples = vec![
            half::f16::from_f32(0.0);
            width as usize * height as usize * aurora_tile::CHANNELS
        ];
        let tiles = crate::headless::document_tiles(width, height);
        let total = tiles.len().max(1);
        let mut budget = crate::CompositeBudget::for_pass(&doc.layers);
        for (index, tile_id) in tiles.into_iter().enumerate() {
            if cancelled() {
                return Err(Failure::Cancelled);
            }
            budget.next_tile(&doc.layers);
            let (texels, _) = crate::composite_roots_into_tile(
                &doc.layers,
                &mut doc.store,
                tile_id,
                crate::composite_tile_doc_origin((0, 0), tile_id),
                (0, 0),
                &mut budget,
            );
            crate::headless::place_tile(&mut samples, (width, height), tile_id, &texels);
            progress((index + 1) as f64 / total as f64);
        }
        if let Some((skipped, _)) = budget.store_error() {
            return Err(failed(format!("{skipped} source tiles could not be read")));
        }
        aurora_io::Image::new(width, height, aurora_color::IccProfile::srgb(), samples)
            .map_err(failed)
    }

    /// A flattened preview no larger than `max_side`.
    ///
    /// # Errors
    /// No document, too large to composite, cancelled, or a failure.
    pub fn preview(
        &mut self,
        max_side: u32,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<PreviewImage, Failure> {
        let doc = self.doc_mut()?;
        let image = Self::composite(doc, cancelled, &mut |_| {})?;
        let (w, h) = (image.width(), image.height());
        let scale = f64::from(max_side) / f64::from(w.max(h).max(1));
        let (tw, th) = if scale >= 1.0 {
            (w, h)
        } else {
            (
                ((f64::from(w) * scale).round() as u32).max(1),
                ((f64::from(h) * scale).round() as u32).max(1),
            )
        };
        let small = if (tw, th) == (w, h) {
            image
        } else {
            box_downscale(&image, tw, th).map_err(failed)?
        };
        let small = match crate::colour_app::export_image(small, None) {
            Ok(image) | Err(image) => image,
        };
        let png = aurora_io::png::encode(&small).map_err(failed)?;
        Ok(PreviewImage {
            png,
            width: tw,
            height: th,
            document_width: w,
            document_height: h,
        })
    }

    /// Writes the document to `out`, a fresh private path.
    ///
    /// # Errors
    /// No document, a refusal, a failure or a cancel.
    #[allow(clippy::too_many_lines)]
    pub fn export(
        &mut self,
        kind: ExportKind,
        accept_losses: bool,
        out: &Path,
        cancelled: &dyn Fn() -> bool,
        progress: &mut dyn FnMut(f64),
    ) -> Result<ExportResult, Failure> {
        let doc = self.doc_mut()?;
        match kind {
            ExportKind::Png | ExportKind::Jpeg | ExportKind::Tiff => {
                let image = Self::composite(doc, cancelled, &mut |f| progress(f * 0.8))?;
                let image = match crate::colour_app::export_image(image, None) {
                    Ok(image) | Err(image) => image,
                };
                let bytes = aurora_io::encode_by_extension(out, &image).map_err(failed)?;
                if cancelled() {
                    return Err(Failure::Cancelled);
                }
                progress(0.9);
                crate::export_save::write_export_verified(
                    out,
                    &bytes,
                    image.width(),
                    image.height(),
                )
                .map_err(failed)?;
                progress(1.0);
                Ok(ExportResult::Written(json!({"lossless": false})))
            }
            ExportKind::Aur => {
                progress(0.1);
                let source = crate::aur_save::AurSaveSource {
                    layers: &doc.layers,
                    history: &doc.history,
                    canvas_size: doc.canvas,
                    skipped_tiles: &doc.skipped,
                    import_losses: (!doc.import_losses.is_empty())
                        .then_some(doc.import_losses.as_slice()),
                    selection: &doc.selection,
                    colour: &doc.colour,
                };
                crate::aur_save::save_aur_document(out, &source, &mut doc.store).map_err(failed)?;
                progress(1.0);
                Ok(ExportResult::Written(json!({"lossless": true})))
            }
            ExportKind::Psd => {
                if cancelled() {
                    return Err(Failure::Cancelled);
                }
                progress(0.05);
                let mut carried =
                    crate::psd_save::carried_losses(Some(&doc.import_losses), &doc.skipped);
                carried.extend(crate::psd_save::paths_loss_entry(&doc.selection));
                carried.extend(crate::psd_save::colour_loss_entries(&doc.colour));
                let name = doc.name.clone();
                let source = crate::psd_save::PsdSaveSource {
                    saved: doc.selection.saved_sorted(),
                    layers: &doc.layers,
                    canvas: doc.canvas,
                    store: &mut doc.store,
                    doc_id: doc.id,
                    revision: crate::UndoRevision::default(),
                    carried,
                };
                let step = crate::psd_save::begin_psd_save(source, out).map_err(|e| match e {
                    crate::psd_save::PsdSaveFailure::Refused(reason) => Failure::Refused(reason),
                    other => failed(format!("{other:?}")),
                })?;
                progress(0.95);
                match step {
                    crate::psd_save::PsdSaveStep::Saved
                    | crate::psd_save::PsdSaveStep::SavedNotConfirmed => {
                        progress(1.0);
                        Ok(ExportResult::Written(json!({"lossless": true})))
                    }
                    crate::psd_save::PsdSaveStep::NeedsConfirmation(pending) => {
                        let report = report_json(&name, &pending.report);
                        if accept_losses {
                            pending.temp.persist().map_err(failed)?;
                            progress(1.0);
                            Ok(ExportResult::Written(report))
                        } else {
                            // Dropping the pending save deletes its temp file.
                            Ok(ExportResult::NeedsConfirmation(report))
                        }
                    }
                }
            }
        }
    }

    /// The grouped report a PSD save would give, writing no file.
    ///
    /// # Errors
    /// No document, a refusal (a canvas over 30,000 px, ...) or a failure.
    pub fn psd_check(&mut self, cancelled: &dyn Fn() -> bool) -> Result<Value, Failure> {
        let doc = self.doc_mut()?;
        if cancelled() {
            return Err(Failure::Cancelled);
        }
        let mut carried = crate::psd_save::carried_losses(Some(&doc.import_losses), &doc.skipped);
        carried.extend(crate::psd_save::paths_loss_entry(&doc.selection));
        carried.extend(crate::psd_save::colour_loss_entries(&doc.colour));
        let name = doc.name.clone();
        let source = crate::psd_save::PsdSaveSource {
            saved: doc.selection.saved_sorted(),
            layers: &doc.layers,
            canvas: doc.canvas,
            store: &mut doc.store,
            doc_id: doc.id,
            revision: crate::UndoRevision::default(),
            carried,
        };
        match crate::psd_save::check_psd_compatibility(source) {
            Ok(report) => Ok(report_json(&name, &report)),
            Err(failure) => Err(Failure::Refused(crate::psd_save::psd_check_failure_message(
                &failure,
            ))),
        }
    }

    /// Whether a document is open (for tests).
    #[must_use]
    pub fn canvas_size(&self) -> Option<(u32, u32)> {
        self.doc().ok().map(|d| d.canvas)
    }
}

/// An alpha-weighted box average to `tw` x `th`.
fn box_downscale(
    image: &aurora_io::Image,
    tw: u32,
    th: u32,
) -> Result<aurora_io::Image, aurora_io::IoError> {
    let (w, h) = (image.width() as usize, image.height() as usize);
    let (tw_us, th_us) = (tw as usize, th as usize);
    let src = image.samples();
    let channels = aurora_tile::CHANNELS;
    let mut out = vec![half::f16::from_f32(0.0); tw_us * th_us * channels];
    for ty in 0..th_us {
        let y0 = ty * h / th_us;
        let y1 = ((ty + 1) * h / th_us).max(y0 + 1).min(h);
        for tx in 0..tw_us {
            let x0 = tx * w / tw_us;
            let x1 = ((tx + 1) * w / tw_us).max(x0 + 1).min(w);
            let (mut r, mut g, mut b, mut a) = (0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32);
            let mut n = 0.0_f32;
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = (y * w + x) * channels;
                    if let Some(px) = src.get(i..i + 4) {
                        let alpha = px.get(3).map_or(0.0, |v| v.to_f32());
                        r += px.first().map_or(0.0, |v| v.to_f32()) * alpha;
                        g += px.get(1).map_or(0.0, |v| v.to_f32()) * alpha;
                        b += px.get(2).map_or(0.0, |v| v.to_f32()) * alpha;
                        a += alpha;
                        n += 1.0;
                    }
                }
            }
            let o = (ty * tw_us + tx) * channels;
            if let Some(dst) = out.get_mut(o..o + 4) {
                let (cr, cg, cb) = if a > 0.0 { (r / a, g / a, b / a) } else { (0.0, 0.0, 0.0) };
                let ca = if n > 0.0 { a / n } else { 0.0 };
                for (slot, v) in dst.iter_mut().zip([cr, cg, cb, ca]) {
                    *slot = half::f16::from_f32(v);
                }
            }
        }
    }
    aurora_io::Image::new(tw, th, aurora_color::IccProfile::srgb(), out)
}
