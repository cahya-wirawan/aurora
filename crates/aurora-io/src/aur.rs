//! `.aur` document format: read/write ([ADR 0009](../../../docs/adr/0009-aur-document-format.md)).
//! A ZIP archive (the `zip` crate, trimmed to `STORE`/`DEFLATE`) holding:
//!
//! - `mimetype`: `MIME_TYPE`, stored uncompressed — the same
//!   magic-byte-sniff trick ODF/EPUB/`OpenRaster` (`.ora`) use, so a
//!   reader can identify the format without parsing a full ZIP central
//!   directory. Not read back by [`read`] itself (its only job is
//!   external sniffing, before a caller has even decided to call
//!   [`read`]).
//! - `manifest`: canvas size, colour space, and the whole
//!   `aurora_doc::LayerTree`, `postcard`-encoded (`ManifestWrite`/
//!   `ManifestRead`). The manifest's own `version` field is this
//!   format's forward-compatibility hook (ADR
//!   0009): every past version this project has ever written must keep
//!   reading.
//! - `history`: `aurora_doc::History::save_journal`'s own bytes —
//!   finally gives the crash-recovery journal (deferred since M1.4/M1.8)
//!   a real, permanent home, not just the temp-dir autosave file
//!   `aurora-app` writes today.
//! - One entry per non-blank pixel-layer tile, named by `tile_entry_name`
//!   from its own `(SurfaceId, TileId)` pair, holding
//!   `aurora_tile::codec::encode`'s own output **verbatim** — stored,
//!   not deflated, since that output is already `lz4_flex`-compressed
//!   and compressing compressed bytes again wastes CPU for no size
//!   benefit.
//!
//! **Scope, stated honestly.** Only a pixel layer's own `bounds` extent
//! is persisted — the same "no document-extent clamp" limitation
//! `aurora_brush::stamp_dab`'s own doc comment already names (nothing
//! in this pipeline clips painting to a layer's own bounds, so pixels
//! painted past them were never on documented, reliable ground to begin
//! with). A fully blank (all-zero) tile is skipped when writing — most
//! of a freshly created layer's own tile range is never actually
//! painted, and writing every one of those out would be real, avoidable
//! file bloat; a missing tile entry on read simply leaves that tile at
//! the store's own default (blank), not an error. Colour space is a
//! single document-level tag (`ColorSpaceTag`), not a real embedded
//! ICC profile — every image this crate decodes or creates is already
//! always tagged `aurora_color::IccProfile::srgb()` (no decoder here
//! reads an embedded profile yet either), so a richer per-profile
//! encoding has no real caller to serve today.

use std::io::{Read, Seek, Write};

use aurora_doc::{History, LayerId, LayerKind, LayerTree};
use aurora_tile::{SurfaceId, TILE, TileId, TileStore};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::error::IoError;

/// [`MIME_ENTRY`]'s own contents — a real, chosen media type for the
/// format, not borrowed from anything else (`.aur` isn't registered
/// with IANA; this is the same "good enough to sniff by" convention
/// `OpenRaster`'s own `image/openraster` follows).
const MIME_TYPE: &str = "application/vnd.aurora.document";
const MIME_ENTRY: &str = "mimetype";
const MANIFEST_ENTRY: &str = "manifest";
const HISTORY_ENTRY: &str = "history";

/// The manifest's own current schema version (`ManifestWrite`/
/// `ManifestRead`) — bump this whenever their shape changes, and keep
/// every past version [`read`] has ever shipped readable (ADR 0009's
/// own backward-compatibility policy: unconditional, never a hard
/// cutoff).
const MANIFEST_VERSION: u32 = 1;

/// The manifest entry's own real shape, written by reference (avoids
/// needing `LayerTree: Clone`, which nothing else has a real reason for
/// yet) — see [`ManifestRead`] for the owned counterpart [`read`]
/// deserializes into. Both must stay field-for-field identical in order
/// and type: `postcard`'s wire format is positional, not name-matched,
/// so the two are two views of the same shape, not independent types
/// that happen to look similar.
#[derive(serde::Serialize)]
struct ManifestWrite<'a> {
    version: u32,
    canvas_width: u32,
    canvas_height: u32,
    color_space: ColorSpaceTag,
    layers: &'a LayerTree,
}

/// [`ManifestWrite`]'s owned counterpart — see that type's own doc
/// comment.
#[derive(serde::Deserialize)]
struct ManifestRead {
    version: u32,
    canvas_width: u32,
    canvas_height: u32,
    color_space: ColorSpaceTag,
    layers: LayerTree,
}

/// The manifest's own "colour space" field — see this module's own doc
/// comment for why it's a single tag, not a real ICC profile, today.
#[derive(serde::Serialize, serde::Deserialize)]
enum ColorSpaceTag {
    Srgb,
}

/// Writes a complete `.aur` document to `writer`: `layers`/`history`'s
/// own current state, tagged with `canvas_size`, plus every non-blank
/// pixel-layer tile currently in `store`. `writer` is generic over
/// `Write + Seek` (a real `std::fs::File`, or an in-memory
/// `std::io::Cursor<Vec<u8>>` for round-trip testing) rather than a
/// path, so the caller decides how (and whether) to stage/verify the
/// bytes before they land at a real destination — `aurora-app`'s own
/// `write_verified`-style "write to temp, verify by reopening, then
/// swap" discipline (CLAUDE.md's PSD/PSB round-trip rule) composes with
/// this rather than being duplicated inside it.
///
/// # Errors
///
/// Returns [`IoError::Zip`]/[`IoError::Io`] for a real container/I/O
/// failure, [`IoError::ManifestSerialization`] if the manifest itself
/// somehow fails to `postcard`-encode (a plain, already-checked struct —
/// not expected in practice), [`IoError::Doc`] if `history.save_journal`
/// fails, or [`IoError::Tile`] if paging a touched tile in from the
/// scratch disk fails.
pub fn write<W: Write + Seek>(
    writer: W,
    layers: &LayerTree,
    history: &History,
    canvas_size: (u32, u32),
    store: &mut TileStore,
) -> Result<(), IoError> {
    let mut zip = ZipWriter::new(writer);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file(MIME_ENTRY, stored)?;
    zip.write_all(MIME_TYPE.as_bytes())?;

    let manifest = ManifestWrite {
        version: MANIFEST_VERSION,
        canvas_width: canvas_size.0,
        canvas_height: canvas_size.1,
        color_space: ColorSpaceTag::Srgb,
        layers,
    };
    let manifest_bytes = postcard::to_allocvec(&manifest)
        .map_err(|source| IoError::ManifestSerialization(source.to_string()))?;
    zip.start_file(MANIFEST_ENTRY, deflated)?;
    zip.write_all(&manifest_bytes)?;

    let history_bytes = history.save_journal()?;
    zip.start_file(HISTORY_ENTRY, deflated)?;
    zip.write_all(&history_bytes)?;

    for id in pixel_layer_ids(layers) {
        let Some(LayerKind::Pixel { bounds }) = layers.kind(id) else {
            continue;
        };
        let Some(surface) = layers.surface_id(id) else {
            continue;
        };
        let tiles_x = bounds.width.div_ceil(TILE);
        let tiles_y = bounds.height.div_ceil(TILE);
        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                let tile_id = TileId { x: tx, y: ty };
                let tile = store.get(surface, tile_id)?;
                if tile.texels().iter().all(|sample| sample.to_f32() == 0.0) {
                    continue;
                }
                let bytes = aurora_tile::codec::encode(tile.texels());
                zip.start_file(tile_entry_name(surface, tile_id), stored)?;
                zip.write_all(&bytes)?;
            }
        }
    }

    zip.finish()?;
    Ok(())
}

/// Reads a complete `.aur` document from `reader`, writing every
/// persisted tile it finds directly into `store` (mirroring
/// `crate::import::write_into_store`'s own "the caller already has a
/// live store; write into it" shape rather than returning some
/// intermediate pixel buffer). Returns the reconstructed
/// `LayerTree`/`History` and the manifest's own `(canvas_width,
/// canvas_height)`.
///
/// # Errors
///
/// Returns [`IoError::Zip`]/[`IoError::Io`] for a real container/I/O
/// failure, [`IoError::MissingEntry`] if the manifest or history entry
/// is absent (not a valid `.aur` file, or one truncated past recovery),
/// [`IoError::ManifestDeserialization`]/[`IoError::Doc`] if either
/// fails to decode, or [`IoError::Tile`] if a tile entry fails to
/// decode or doesn't decode to the expected sample count.
pub fn read<R: Read + Seek>(
    reader: R,
    store: &mut TileStore,
) -> Result<(LayerTree, History, (u32, u32)), IoError> {
    let mut zip = ZipArchive::new(reader)?;

    let manifest_bytes = read_entry(&mut zip, MANIFEST_ENTRY)?;
    let manifest: ManifestRead = postcard::from_bytes(&manifest_bytes)
        .map_err(|source| IoError::ManifestDeserialization(source.to_string()))?;
    if manifest.version != MANIFEST_VERSION {
        return Err(IoError::ManifestDeserialization(format!(
            "unsupported manifest version {} (this build understands version {MANIFEST_VERSION})",
            manifest.version
        )));
    }
    // Exhaustive on purpose: `ColorSpaceTag` gaining a real second
    // variant (a step toward this module's own still-open "real ICC
    // profile, not just a tag" gap) must force a deliberate decision
    // here, not silently fall through unread.
    match manifest.color_space {
        ColorSpaceTag::Srgb => {}
    }

    let history_bytes = read_entry(&mut zip, HISTORY_ENTRY)?;
    let history = History::load_journal(&history_bytes)?;

    for id in pixel_layer_ids(&manifest.layers) {
        let Some(LayerKind::Pixel { bounds }) = manifest.layers.kind(id) else {
            continue;
        };
        let Some(surface) = manifest.layers.surface_id(id) else {
            continue;
        };
        let tiles_x = bounds.width.div_ceil(TILE);
        let tiles_y = bounds.height.div_ceil(TILE);
        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                let tile_id = TileId { x: tx, y: ty };
                let bytes = match zip.by_name(&tile_entry_name(surface, tile_id)) {
                    Ok(mut file) => {
                        let mut bytes = Vec::new();
                        file.read_to_end(&mut bytes)?;
                        bytes
                    }
                    // No entry for this tile -- it was blank when
                    // written (see this module's own doc comment) and
                    // stays at the store's own default.
                    Err(zip::result::ZipError::FileNotFound) => continue,
                    Err(err) => return Err(err.into()),
                };
                let decoded = aurora_tile::codec::decode(&bytes)?;
                let tile = store.get_mut(surface, tile_id)?;
                let texels = tile.texels_mut();
                if decoded.len() != texels.len() {
                    return Err(IoError::Tile(aurora_tile::TileError::CorruptFile(format!(
                        "tile {tile_id:?} on surface {surface:?} decoded to {} samples, expected {}",
                        decoded.len(),
                        texels.len()
                    ))));
                }
                texels.copy_from_slice(&decoded);
                tile.mark_dirty(aurora_core::Rect {
                    x: 0,
                    y: 0,
                    width: TILE,
                    height: TILE,
                });
            }
        }
    }

    Ok((
        manifest.layers,
        history,
        (manifest.canvas_width, manifest.canvas_height),
    ))
}

/// Reads one required entry's whole contents, or
/// [`IoError::MissingEntry`] if it isn't present at all — [`read`]'s own
/// shared "the manifest/history entries are not optional" step.
fn read_entry<R: Read + Seek>(
    zip: &mut ZipArchive<R>,
    name: &'static str,
) -> Result<Vec<u8>, IoError> {
    let mut file = match zip.by_name(name) {
        Ok(file) => file,
        Err(zip::result::ZipError::FileNotFound) => return Err(IoError::MissingEntry(name)),
        Err(err) => return Err(err.into()),
    };
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Every pixel layer in `layers`, at any nesting depth — [`write`]/
/// [`read`]'s own shared "which layers actually have tiles to
/// persist" walk. Order doesn't matter here (unlike
/// `aurora_ui::layers_panel`'s own top-to-bottom paint-order
/// convention) since this only decides which tiles to touch, not how
/// to composite them.
fn pixel_layer_ids(layers: &LayerTree) -> Vec<LayerId> {
    let mut ids = Vec::new();
    collect_pixel_layers(layers, layers.roots(), &mut ids);
    ids
}

fn collect_pixel_layers(layers: &LayerTree, siblings: &[LayerId], out: &mut Vec<LayerId>) {
    for &id in siblings {
        match layers.kind(id) {
            Some(LayerKind::Pixel { .. }) => out.push(id),
            Some(LayerKind::Group { .. }) => {
                if let Some(children) = layers.children(id) {
                    collect_pixel_layers(layers, children, out);
                }
            }
            None => {}
        }
    }
}

/// The ZIP entry name one tile's own encoded bytes live under —
/// `tiles/<surface>/<x>_<y>.tile`, a real, inspectable path (ADR 0009's
/// own "open format... a user can inspect a `.aur` file's contents with
/// a file manager" goal), not an opaque or flat-namespaced one.
fn tile_entry_name(surface: SurfaceId, id: TileId) -> String {
    format!("tiles/{}/{}_{}.tile", surface.to_raw(), id.x, id.y)
}

#[cfg(test)]
mod tests {
    use super::{read, write};
    use aurora_doc::{BlendMode, History, LayerKind, LayerTree};
    use aurora_tile::{TileId, TileStore};
    use half::f16;
    use std::io::Cursor;
    use std::num::NonZeroUsize;

    fn real_tile_store() -> (tempfile::TempDir, TileStore) {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(budget) = NonZeroUsize::new(16) else {
            unreachable!("16 is non-zero");
        };
        let store = match TileStore::new(dir.path().to_path_buf(), budget) {
            Ok(store) => store,
            Err(err) => {
                unreachable!("scratch dir just created by tempfile must be usable: {err:?}")
            }
        };
        (dir, store)
    }

    fn bounds() -> aurora_core::Rect {
        aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        }
    }

    #[test]
    fn round_trips_a_real_document_with_a_painted_tile() {
        let (_dir, mut store) = real_tile_store();
        let mut layers = LayerTree::new();
        let mut history = History::new();
        let id = match history.add_pixel_layer(&mut layers, "Background", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = history.set_blend_mode(&mut layers, id, BlendMode::Multiply) {
            unreachable!("{err:?}");
        }
        let Some(surface) = layers.surface_id(id) else {
            unreachable!("id was just created as a pixel layer");
        };
        {
            let tile = match store.get_mut(surface, TileId { x: 0, y: 0 }) {
                Ok(tile) => tile,
                Err(err) => unreachable!("{err:?}"),
            };
            let texels = tile.texels_mut();
            if let Some(sample) = texels.get_mut(1) {
                *sample = f16::from_f32(1.0); // green channel of texel (0, 0)
            }
            if let Some(sample) = texels.get_mut(3) {
                *sample = f16::from_f32(1.0); // alpha channel of texel (0, 0)
            }
        }

        let mut bytes = Cursor::new(Vec::new());
        if let Err(err) = write(&mut bytes, &layers, &history, (10, 10), &mut store) {
            unreachable!("{err:?}");
        }

        let (_dir2, mut fresh_store) = real_tile_store();
        bytes.set_position(0);
        let (restored_layers, restored_history, canvas_size) = match read(bytes, &mut fresh_store) {
            Ok(result) => result,
            Err(err) => unreachable!("{err:?}"),
        };

        assert_eq!(canvas_size, (10, 10));
        assert_eq!(restored_layers.roots(), &[id]);
        assert_eq!(restored_layers.name(id), Some("Background"));
        assert_eq!(restored_layers.blend_mode(id), Some(BlendMode::Multiply));
        assert_eq!(
            restored_layers.kind(id),
            Some(&LayerKind::Pixel { bounds: bounds() })
        );
        assert_eq!(
            restored_history.journal_descriptions(),
            history.journal_descriptions()
        );

        let restored_tile = match fresh_store.get(surface, TileId { x: 0, y: 0 }) {
            Ok(tile) => tile,
            Err(err) => unreachable!("{err:?}"),
        };
        let restored_texels = restored_tile.texels();
        let Some(&green) = restored_texels.get(1) else {
            unreachable!("index is in bounds for a full tile");
        };
        let Some(&alpha) = restored_texels.get(3) else {
            unreachable!("index is in bounds for a full tile");
        };
        assert!(
            green.to_f32() > 0.9,
            "green channel must survive: {green:?}"
        );
        assert!(
            alpha.to_f32() > 0.9,
            "alpha channel must survive: {alpha:?}"
        );
    }

    #[test]
    fn write_skips_a_fully_blank_layer_and_read_leaves_it_blank() {
        let (_dir, mut store) = real_tile_store();
        let mut layers = LayerTree::new();
        let history = History::new();
        // Never painted -- add_pixel_layer directly (not through
        // History) so the journal stays empty and this test's own
        // point (blank tiles) isn't muddied by an unrelated assertion.
        let id = match layers.add_pixel_layer("Untouched", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(surface) = layers.surface_id(id) else {
            unreachable!("id was just created as a pixel layer");
        };

        let mut bytes = Cursor::new(Vec::new());
        if let Err(err) = write(&mut bytes, &layers, &history, (10, 10), &mut store) {
            unreachable!("{err:?}");
        }

        // No `tiles/` entry at all -- the one tile this layer's own
        // 10x10 bounds overlap was never painted, so it must have been
        // skipped, not written out all-zero.
        let archive = match zip::ZipArchive::new(Cursor::new(bytes.get_ref().clone())) {
            Ok(archive) => archive,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(
            archive.file_names().all(|name| !name.starts_with("tiles/")),
            "a never-painted layer must not write out any tile entries: {:?}",
            archive.file_names().collect::<Vec<_>>()
        );

        let (_dir2, mut fresh_store) = real_tile_store();
        bytes.set_position(0);
        if let Err(err) = read(bytes, &mut fresh_store) {
            unreachable!("{err:?}");
        }
        let restored_tile = match fresh_store.get(surface, TileId { x: 0, y: 0 }) {
            Ok(tile) => tile,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(
            restored_tile.texels().iter().all(|s| s.to_f32() == 0.0),
            "a layer with no persisted tile entries must read back blank"
        );
    }

    #[test]
    fn read_rejects_a_file_missing_its_manifest() {
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut bytes);
            if let Err(err) = zip.start_file("mimetype", zip::write::SimpleFileOptions::default()) {
                unreachable!("{err:?}");
            }
            if let Err(err) = std::io::Write::write_all(&mut zip, super::MIME_TYPE.as_bytes()) {
                unreachable!("{err:?}");
            }
            if let Err(err) = zip.finish() {
                unreachable!("{err:?}");
            }
        }
        bytes.set_position(0);
        let (_dir, mut store) = real_tile_store();
        match read(bytes, &mut store) {
            Err(super::IoError::MissingEntry("manifest")) => {}
            other => unreachable!("expected MissingEntry(\"manifest\"), got {other:?}"),
        }
    }

    #[test]
    fn read_rejects_an_unsupported_manifest_version() {
        let manifest = ManifestReadForTest {
            version: 999,
            canvas_width: 1,
            canvas_height: 1,
            color_space: super::ColorSpaceTag::Srgb,
            layers: LayerTree::new(),
        };
        let manifest_bytes = match postcard::to_allocvec(&manifest) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut bytes);
            let deflated = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            if let Err(err) = zip.start_file("manifest", deflated) {
                unreachable!("{err:?}");
            }
            if let Err(err) = std::io::Write::write_all(&mut zip, &manifest_bytes) {
                unreachable!("{err:?}");
            }
            let history = History::new();
            let history_bytes = match history.save_journal() {
                Ok(bytes) => bytes,
                Err(err) => unreachable!("{err:?}"),
            };
            if let Err(err) = zip.start_file("history", deflated) {
                unreachable!("{err:?}");
            }
            if let Err(err) = std::io::Write::write_all(&mut zip, &history_bytes) {
                unreachable!("{err:?}");
            }
            if let Err(err) = zip.finish() {
                unreachable!("{err:?}");
            }
        }
        bytes.set_position(0);
        let (_dir, mut store) = real_tile_store();
        match read(bytes, &mut store) {
            Err(super::IoError::ManifestDeserialization(_)) => {}
            other => unreachable!("expected ManifestDeserialization, got {other:?}"),
        }
    }

    /// Field-for-field identical to `super::ManifestRead`, so its own
    /// `postcard` bytes decode identically -- used only to hand-craft a
    /// manifest with an unsupported `version` for
    /// `read_rejects_an_unsupported_manifest_version`, since the real
    /// `ManifestWrite`/`ManifestRead` always write the current,
    /// supported version.
    #[derive(serde::Serialize)]
    struct ManifestReadForTest {
        version: u32,
        canvas_width: u32,
        canvas_height: u32,
        color_space: super::ColorSpaceTag,
        layers: LayerTree,
    }
}
