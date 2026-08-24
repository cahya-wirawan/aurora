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
//! the store's own default (blank), not an error.
//!
//! **Reading is hardened against a hostile or corrupt container**
//! (2026-08-24). [`read`] runs on `aurora-app`'s own pre-window startup
//! path (crash-recovery autosave) and on its ordinary "open the `.aur`
//! file a user was sent" path, so neither an unfinishable loop nor an
//! unbounded allocation is acceptable here: the manifest's declared
//! layer bounds are checked against `aurora_core::MAX_DOCUMENT_EXTENT`
//! before any tile grid is derived from them (`tile_grid`), and every
//! entry is read through a per-entry size cap (`read_capped`) rather
//! than a bare `read_to_end`. Both reject with an [`IoError`]; neither
//! panics.
//!
//! **A second hardening pass** (also 2026-08-24) closed three more holes
//! an independent review found in that same untrusted path: the
//! per-layer bounds check said nothing about *how many* layers a
//! manifest may declare, so the tile scan now also carries a
//! whole-document budget (`MAX_TOTAL_TILES_PER_DOCUMENT`); the
//! manifest's own `canvas_width`/`canvas_height` were handed back
//! unchecked and became an allocation size downstream, so they now get
//! the same document-ceiling check the layer bounds already got; and
//! this module's pixel-layer walk was recursive over a `LayerTree` that
//! nothing validated the shape of, so a manifest declaring a group
//! inside itself aborted the process on a stack overflow. That last one
//! is fixed at its root in `aurora_doc::LayerTree`'s own `Deserialize`
//! (a tree from bytes is now checked for cycles and depth before any
//! caller walks it) with this module's own walk made iterative as well.
//!
//! **Colour space, real as of 2026-08-06**: [`write()`]/[`read`] carry a
//! real `Option<&aurora_color::IccProfile>`/`Option<IccProfile>` now,
//! not just a bare tag — `None` (the compact, common case, matching
//! every past `.aur` file this crate has ever written) keeps writing
//! `ColorSpaceTag::Srgb`; `Some(profile)` embeds the profile's own real
//! bytes (`IccProfile::to_bytes`, ADR 0008's `lcms2`) as a new
//! `ColorSpaceTag::Icc(Vec<u8>)` variant, restored via
//! `IccProfile::from_bytes` on read. `Srgb` stays index `0` — an old
//! file only ever contains that variant, so it keeps opening unchanged
//! (ADR 0009's own backward-compatibility policy); `Icc` is purely
//! additive. `aurora-app` doesn't have a live "current document
//! profile" to pass yet (no colour-management UI exists), so every
//! real caller today still passes `None` in practice — the mechanism is
//! real and tested against a real non-sRGB profile
//! (`corpora/icc/ECI-RGBv2.icc`), even though nothing yet drives it to
//! something other than `None` end to end.

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

/// The largest declared uncompressed size [`read`] will accept for the
/// `manifest`/`history` entries. Both are `postcard`-encoded and
/// DEFLATE-compressed, so a hostile container can claim (and really
/// deliver) orders of magnitude more bytes than it occupies on disk —
/// the classic zip-bomb shape, and one that matters here because
/// `aurora-app` reads an autosave container on its own pre-window
/// startup path. 64 MiB is deliberately generous rather than tight:
/// this project promises unlimited layers and unlimited history (PRD
/// §6), so a real document's manifest/journal has no small, principled
/// bound — but it does have a *finite* one, and turning "unbounded" into
/// "large" is the whole point.
const MAX_METADATA_ENTRY_BYTES: u64 = 64 * 1024 * 1024;

/// The largest size [`read`] will accept for one tile entry, unlike the
/// metadata cap above a genuinely tight one: a tile entry holds
/// `aurora_tile::codec::encode`'s own output for exactly one
/// `TILE * TILE` f16 RGBA tile, so its real ceiling is that tile's own
/// uncompressed size (`SAMPLES` samples, two bytes each) plus slack for
/// an `lz4_flex` frame that failed to compress at all. Nothing
/// legitimate can exceed it.
const MAX_TILE_ENTRY_BYTES: u64 = (aurora_tile::SAMPLES as u64) * 2 + 64 * 1024;

/// The most tiles [`read`]'s own tile scan will visit across *all* of a
/// manifest's pixel layers put together.
///
/// `tile_grid` already refuses any single layer larger than the document
/// ceiling, but nothing bounds how many layers a manifest may declare —
/// this project promises unlimited layers (PRD §6), and a `LayerEntry`
/// costs only tens of bytes on the wire, so a manifest well under
/// [`MAX_METADATA_ENTRY_BYTES`] can hold hundreds of thousands of them.
/// Each individually-legal ceiling-sized layer contributes another
/// ~1.37 million grid positions, each costing a `format!` and a ZIP
/// central-directory lookup, so the per-layer check alone still leaves a
/// kilobyte-scale file able to spin for hours on `aurora-app`'s own
/// pre-window startup path.
///
/// The budget is exactly *one* layer at the documented ceiling: the
/// largest single document PRD §7.3.1 says can exist still loads
/// untouched, while a manifest that only reaches a bigger number by
/// stacking many large layers is refused. It is expressed in terms of
/// the ceiling and the tile size rather than a bare literal, so it
/// follows either if they ever change.
const MAX_TOTAL_TILES_PER_DOCUMENT: u64 = {
    let side = (aurora_core::MAX_DOCUMENT_EXTENT as u64).div_ceil(TILE as u64);
    side * side
};

/// One entry's whole contents, refusing anything past `cap` — see
/// [`MAX_METADATA_ENTRY_BYTES`]/[`MAX_TILE_ENTRY_BYTES`] for why a cap
/// exists at all. Both halves are real checks, not one belt-and-braces
/// pair: `ZipFile::size()` is the container's own *claim* about the
/// uncompressed size and a crafted archive is free to lie about it, so
/// the actual read is bounded by `Read::take` as well.
fn read_capped(mut file: zip::read::ZipFile<'_>, name: &str, cap: u64) -> Result<Vec<u8>, IoError> {
    let declared = file.size();
    if declared > cap {
        return Err(IoError::EntryTooLarge {
            name: name.to_owned(),
            size: declared,
            cap,
        });
    }
    let mut bytes = Vec::new();
    // `cap + 1`: reading one byte past the cap is what makes an entry
    // that lied about its own size distinguishable from one that sits
    // exactly at the limit.
    let read = file
        .by_ref()
        .take(cap.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if read as u64 > cap {
        return Err(IoError::EntryTooLarge {
            name: name.to_owned(),
            size: read as u64,
            cap,
        });
    }
    Ok(bytes)
}

/// How many tiles wide and tall `bounds` is, rejecting bounds past
/// [`aurora_core::MAX_DOCUMENT_EXTENT`] (PRD §7.3.1's own document
/// ceiling, the same one `aurora_core::Size::new` already enforces).
///
/// This is a real safety check, not a tidiness one. [`read`] derives
/// this grid from a manifest it has just parsed out of an untrusted
/// file, then loops `tiles_y * tiles_x` times — so an unchecked
/// `u32::MAX` extent there is not a big loop but an unfinishable one
/// (~2.8e14 iterations), reached from `aurora-app`'s own pre-window
/// startup recovery *and* from opening any `.aur` file a user was sent.
/// Clamping to the ceiling the format already documents bounds the
/// worst case to something large but finite without newly restricting
/// any legitimate document.
fn tile_grid(bounds: aurora_core::Rect) -> Result<(u32, u32), IoError> {
    if bounds.width > aurora_core::MAX_DOCUMENT_EXTENT
        || bounds.height > aurora_core::MAX_DOCUMENT_EXTENT
    {
        return Err(IoError::LayerBoundsTooLarge {
            width: bounds.width,
            height: bounds.height,
            max: aurora_core::MAX_DOCUMENT_EXTENT,
        });
    }
    Ok((bounds.width.div_ceil(TILE), bounds.height.div_ceil(TILE)))
}

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
/// comment for the real-vs-compact tradeoff behind having two variants.
/// `Srgb` must stay variant `0` (declared first): `postcard`'s wire
/// format encodes an enum variant positionally, and every `.aur` file
/// written before `Icc` existed only ever contains this one, so its own
/// index can never change without breaking ADR 0009's backward-
/// compatibility policy.
#[derive(serde::Serialize, serde::Deserialize)]
enum ColorSpaceTag {
    Srgb,
    /// A real embedded ICC profile's own raw bytes
    /// (`aurora_color::IccProfile::to_bytes`).
    Icc(Vec<u8>),
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
/// `profile`: `None` writes the compact `ColorSpaceTag::Srgb` (every
/// past caller's own behaviour, unchanged); `Some` embeds that
/// profile's own real bytes — see this module's own doc comment.
///
/// # Errors
///
/// Returns [`IoError::Zip`]/[`IoError::Io`] for a real container/I/O
/// failure, [`IoError::ManifestSerialization`] if the manifest itself
/// somehow fails to `postcard`-encode (a plain, already-checked struct —
/// not expected in practice), [`IoError::Doc`] if `history.save_journal`
/// fails, [`IoError::Color`] if `profile.to_bytes()` fails, or
/// [`IoError::Tile`] if paging a touched tile in from the scratch disk
/// fails.
///
/// That last one aborts the whole write, deliberately: an explicit save
/// must refuse rather than quietly produce a document with content
/// missing. A caller whose alternative to an incomplete file is *no*
/// file — crash-recovery autosave, and only that — wants
/// [`write_best_effort`] instead, which skips the unreadable tiles and
/// names them.
pub fn write<W: Write + Seek>(
    writer: W,
    layers: &LayerTree,
    history: &History,
    canvas_size: (u32, u32),
    profile: Option<&aurora_color::IccProfile>,
    store: &mut TileStore,
) -> Result<(), IoError> {
    // The returned list is empty by construction, not by assumption:
    // `UnreadableTile::Refuse` is handled by the single `return
    // Err(err.into())` in `write_with_policy`'s tile loop, which is the
    // only place a `SkippedTile` is ever pushed and the only place that
    // policy is read. It is dropped rather than asserted on because a
    // `debug_assert!` here would be a `panic!` that the workspace's own
    // `panic`-denying lints do not see (they lint the macro, not what it
    // expands to) — a bad trade for restating something the type-level
    // control flow already guarantees.
    let _empty = write_with_policy(
        writer,
        layers,
        history,
        canvas_size,
        profile,
        store,
        UnreadableTile::Refuse,
    )?;
    Ok(())
}

/// One tile [`write_best_effort`] could not read, and therefore left out
/// of the container it wrote.
#[derive(Debug, Clone)]
pub struct SkippedTile {
    pub surface: aurora_tile::SurfaceId,
    pub tile: TileId,
    /// The underlying [`aurora_tile::TileError`]'s own message — kept as
    /// a `String` because that type is not `Clone`, the same reason
    /// `aurora-app`'s own `CompositeBudget` already keeps one.
    pub reason: String,
}

/// [`write()`], except that a tile which cannot be read out of `store` is
/// **left out** of the container instead of aborting the whole write.
/// Returns whatever it had to skip, in the order it hit them, so a
/// caller can say so; an empty vector means the file is complete and
/// identical to what [`write()`] would have produced.
///
/// **This exists for autosave, and deliberately not for Save/Export.**
/// An explicit save is a professional's deliberate action on their own
/// file, and 0.52.1 settled that such a save must *refuse* rather than
/// quietly write a document with content missing — [`write()`] keeps that
/// behaviour, unchanged, and every user-facing save path keeps calling
/// it. A background autosave is the opposite case: it is crash-recovery
/// protection the user never asked for and cannot see fail, and its
/// alternative to an incomplete file is **no file at all**. Before this
/// existed, one unreadable tile aborted every autosave for the rest of
/// the session — so the whole document, every other layer included,
/// silently stopped being protected because of one bad tile. Writing the
/// rest and naming what was dropped is strictly better than that, and it
/// is the same distinction `aurora-app` already draws between its live
/// canvas (degrades and repaints) and `composite_document` (refuses).
///
/// Only a tile read is tolerated. A container/I/O failure, a bad
/// manifest, or a layer whose bounds exceed the document ceiling still
/// fail the write outright — those say the *output* is broken, not that
/// one piece of input is unreadable.
///
/// # Errors
///
/// Same as [`write()`], minus [`IoError::Tile`] for a tile that fails to
/// page in.
pub fn write_best_effort<W: Write + Seek>(
    writer: W,
    layers: &LayerTree,
    history: &History,
    canvas_size: (u32, u32),
    profile: Option<&aurora_color::IccProfile>,
    store: &mut TileStore,
) -> Result<Vec<SkippedTile>, IoError> {
    write_with_policy(
        writer,
        layers,
        history,
        canvas_size,
        profile,
        store,
        UnreadableTile::Skip,
    )
}

/// What [`write_with_policy`] does about a tile it cannot read: the two
/// halves of the deliberate refuse-vs-degrade split
/// [`write_best_effort`]'s own doc comment explains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnreadableTile {
    /// Fail the whole write with that tile's own [`IoError::Tile`].
    Refuse,
    /// Leave the tile out and record it in the returned list.
    Skip,
}

fn write_with_policy<W: Write + Seek>(
    writer: W,
    layers: &LayerTree,
    history: &History,
    canvas_size: (u32, u32),
    profile: Option<&aurora_color::IccProfile>,
    store: &mut TileStore,
    unreadable: UnreadableTile,
) -> Result<Vec<SkippedTile>, IoError> {
    let mut skipped: Vec<SkippedTile> = Vec::new();
    let mut zip = ZipWriter::new(writer);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file(MIME_ENTRY, stored)?;
    zip.write_all(MIME_TYPE.as_bytes())?;

    let color_space = match profile {
        None => ColorSpaceTag::Srgb,
        Some(profile) => ColorSpaceTag::Icc(profile.to_bytes()?),
    };
    let manifest = ManifestWrite {
        version: MANIFEST_VERSION,
        canvas_width: canvas_size.0,
        canvas_height: canvas_size.1,
        color_space,
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
        let (tiles_x, tiles_y) = tile_grid(*bounds)?;
        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                let tile_id = TileId { x: tx, y: ty };
                let tile = match store.get(surface, tile_id) {
                    Ok(tile) => tile,
                    Err(err) => match unreadable {
                        UnreadableTile::Refuse => return Err(err.into()),
                        UnreadableTile::Skip => {
                            tracing::warn!(
                                ?surface,
                                ?tile_id,
                                %err,
                                "leaving an unreadable tile out of a best-effort .aur write"
                            );
                            skipped.push(SkippedTile {
                                surface,
                                tile: tile_id,
                                reason: err.to_string(),
                            });
                            continue;
                        }
                    },
                };
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
    Ok(skipped)
}

/// [`read`]'s own return shape: the reconstructed `LayerTree`/`History`,
/// the manifest's own `(canvas_width, canvas_height)`, and its own
/// colour profile (`None`/`Some` — see [`read`]'s own doc comment).
type AurDocument = (
    LayerTree,
    History,
    (u32, u32),
    Option<aurora_color::IccProfile>,
);

/// Reads a complete `.aur` document from `reader`, writing every
/// persisted tile it finds directly into `store` (mirroring
/// `crate::import::write_into_store`'s own "the caller already has a
/// live store; write into it" shape rather than returning some
/// intermediate pixel buffer). Returns the reconstructed
/// `LayerTree`/`History`, the manifest's own `(canvas_width,
/// canvas_height)`, and its own colour profile — `None` for a file
/// that only ever carried the bare `ColorSpaceTag::Srgb` (every `.aur`
/// file written before [`write()`]'s own `profile` parameter existed, and
/// every one written with `profile: None` since), `Some` for one that
/// embedded a real ICC profile.
///
/// # Errors
///
/// Returns [`IoError::Zip`]/[`IoError::Io`] for a real container/I/O
/// failure, [`IoError::MissingEntry`] if the manifest or history entry
/// is absent (not a valid `.aur` file, or one truncated past recovery),
/// [`IoError::ManifestDeserialization`]/[`IoError::Doc`] if either
/// fails to decode, [`IoError::Color`] if an embedded ICC profile's own
/// bytes fail to parse, [`IoError::Tile`] if a tile entry fails to
/// decode or doesn't decode to the expected sample count,
/// [`IoError::LayerBoundsTooLarge`] if the manifest declares a layer
/// past the document ceiling, [`IoError::CanvasTooLarge`] if it declares
/// a *canvas* past that same ceiling, [`IoError::TooManyTiles`] if its
/// layers together add up to more tiles than any real document has, or
/// [`IoError::EntryTooLarge`] if an entry holds more bytes than it
/// legitimately could — see
/// `tile_grid`/`read_capped`/`MAX_TOTAL_TILES_PER_DOCUMENT` for why an
/// untrusted manifest gets those checks before anything is looped over
/// or allocated. A manifest whose `LayerTree` isn't structurally a tree
/// at all (a group nested inside itself, or nested past
/// `aurora_doc::MAX_LAYER_TREE_DEPTH`) is rejected as
/// [`IoError::ManifestDeserialization`] by that type's own
/// `Deserialize`, before this function ever walks it.
pub fn read<R: Read + Seek>(reader: R, store: &mut TileStore) -> Result<AurDocument, IoError> {
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
    // Exhaustive on purpose: a further `ColorSpaceTag` variant would
    // need a deliberate decision here, not a silent fall-through.
    let profile = match manifest.color_space {
        ColorSpaceTag::Srgb => None,
        ColorSpaceTag::Icc(bytes) => Some(aurora_color::IccProfile::from_bytes(&bytes)?),
    };

    // The manifest's own canvas size gets the same document-ceiling
    // check its layer bounds already get (`tile_grid`). `aurora-app`
    // stores this straight into `App::canvas_size` and later allocates
    // `width * height * 4` samples from it, so leaving it unchecked
    // makes a 200-byte file an allocation request no machine can serve.
    if manifest.canvas_width > aurora_core::MAX_DOCUMENT_EXTENT
        || manifest.canvas_height > aurora_core::MAX_DOCUMENT_EXTENT
    {
        return Err(IoError::CanvasTooLarge {
            width: manifest.canvas_width,
            height: manifest.canvas_height,
            max: aurora_core::MAX_DOCUMENT_EXTENT,
        });
    }

    let history_bytes = read_entry(&mut zip, HISTORY_ENTRY)?;
    let history = History::load_journal(&history_bytes)?;

    let mut total_tiles: u64 = 0;
    for id in pixel_layer_ids(&manifest.layers) {
        let Some(LayerKind::Pixel { bounds }) = manifest.layers.kind(id) else {
            continue;
        };
        let Some(surface) = manifest.layers.surface_id(id) else {
            continue;
        };
        let (tiles_x, tiles_y) = tile_grid(*bounds)?;
        // Charged against the whole-document budget *before* this
        // layer's own grid is walked, so an over-budget manifest costs
        // one addition rather than a scan -- see
        // `MAX_TOTAL_TILES_PER_DOCUMENT`.
        total_tiles = total_tiles.saturating_add(u64::from(tiles_x) * u64::from(tiles_y));
        if total_tiles > MAX_TOTAL_TILES_PER_DOCUMENT {
            return Err(IoError::TooManyTiles {
                total: total_tiles,
                max: MAX_TOTAL_TILES_PER_DOCUMENT,
            });
        }
        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                let tile_id = TileId { x: tx, y: ty };
                let name = tile_entry_name(surface, tile_id);
                let bytes = match zip.by_name(&name) {
                    Ok(file) => read_capped(file, &name, MAX_TILE_ENTRY_BYTES)?,
                    // No entry for this tile -- it was blank when
                    // written (see this module's own doc comment) and
                    // stays at the store's own default.
                    Err(zip::result::ZipError::FileNotFound) => continue,
                    Err(err) => return Err(err.into()),
                };
                let decoded = aurora_tile::codec::decode(&bytes)?;
                let tile = store.get_mut(surface, tile_id)?;
                let texels = tile.texels_mut();
                // Redundant by construction since 0.52.1: `codec::decode`
                // now rejects anything that isn't exactly one whole tile,
                // so `decoded.len()` can only be `SAMPLES` here, which is
                // also what `texels.len()` always is. Kept deliberately --
                // it is this crate's own independent guard on the
                // `copy_from_slice` below (a panic on mismatch), it costs
                // one comparison per tile, and it is what makes this read
                // correct on its own terms rather than by an argument
                // about another crate. It is *not* covered by any test of
                // its own, so do not read it as evidence of anything.
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
        profile,
    ))
}

/// Reads one required entry's whole contents, or
/// [`IoError::MissingEntry`] if it isn't present at all — [`read`]'s own
/// shared "the manifest/history entries are not optional" step. Capped
/// at [`MAX_METADATA_ENTRY_BYTES`] ([`read_capped`]).
fn read_entry<R: Read + Seek>(
    zip: &mut ZipArchive<R>,
    name: &'static str,
) -> Result<Vec<u8>, IoError> {
    let file = match zip.by_name(name) {
        Ok(file) => file,
        Err(zip::result::ZipError::FileNotFound) => return Err(IoError::MissingEntry(name)),
        Err(err) => return Err(err.into()),
    };
    read_capped(file, name, MAX_METADATA_ENTRY_BYTES)
}

/// Every pixel layer in `layers`, at any nesting depth — [`write`]/
/// [`read`]'s own shared "which layers actually have tiles to
/// persist" walk. Order doesn't matter here (unlike
/// `aurora_ui::layers_panel`'s own top-to-bottom paint-order
/// convention) since this only decides which tiles to touch, not how
/// to composite them.
///
/// Iterative on an explicit stack, never recursive. `LayerTree`'s own
/// `Deserialize` already refuses a manifest whose tree isn't really a
/// tree, so a cycle can no longer reach here from a file at all — but
/// this walk runs on `aurora-app`'s own pre-window startup path, where
/// the failure mode of unbounded recursion is a stack overflow, and a
/// stack overflow is a process abort rather than an `Err` anything can
/// report. `budget` bounds the walk at one visit per layer the tree
/// actually holds, which is all a real tree ever needs.
fn pixel_layer_ids(layers: &LayerTree) -> Vec<LayerId> {
    let mut ids = Vec::new();
    // Reversed on the way in so popping yields each sibling list in its
    // own stored order -- the order the recursive walk this replaced
    // produced. (Order isn't load-bearing here, per the doc comment
    // above; keeping it identical just means nothing downstream can
    // quietly depend on a change.)
    let mut stack: Vec<LayerId> = layers.roots().iter().rev().copied().collect();
    let mut budget = layers.len();
    while let Some(id) = stack.pop() {
        let Some(kind) = layers.kind(id) else {
            continue;
        };
        if budget == 0 {
            break;
        }
        budget -= 1;
        match kind {
            LayerKind::Pixel { .. } => ids.push(id),
            LayerKind::Group { children } => stack.extend(children.iter().rev().copied()),
        }
    }
    ids
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
    use super::{read, write, write_best_effort};
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

    /// A store that can hold exactly one tile resident, so touching a
    /// second tile evicts the first to the scratch disk — the setup
    /// [`break_the_only_scratch_file`] needs.
    fn one_tile_store() -> (tempfile::TempDir, TileStore) {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(budget) = NonZeroUsize::new(1) else {
            unreachable!("1 is non-zero");
        };
        let store = match TileStore::new(dir.path().to_path_buf(), budget) {
            Ok(store) => store,
            Err(err) => {
                unreachable!("scratch dir just created by tempfile must be usable: {err:?}")
            }
        };
        (dir, store)
    }

    /// Truncates the one file in `dir` to half its length, leaving a
    /// well-formed-but-short ATIL file — what a crash mid-write really
    /// leaves behind, and what `aurora_tile::codec::decode` rejects on
    /// every read (0.52.2), making the tile permanently unreadable.
    fn break_the_only_scratch_file(dir: &tempfile::TempDir) {
        let Ok(entries) = std::fs::read_dir(dir.path()) else {
            unreachable!("the scratch directory must be readable");
        };
        let files: Vec<std::path::PathBuf> = entries.flatten().map(|e| e.path()).collect();
        let [victim] = files.as_slice() else {
            unreachable!("exactly one tile should have been evicted: {files:?}");
        };
        let Ok(bytes) = std::fs::read(victim) else {
            unreachable!("the evicted tile file must be readable");
        };
        let Some(truncated) = bytes.get(..bytes.len() / 2) else {
            unreachable!("half of a slice's own length is always in range");
        };
        if let Err(err) = std::fs::write(victim, truncated) {
            unreachable!("test-local scratch disk must accept the write: {err:?}");
        }
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
        // A canvas size that deliberately doesn't match the layer's own
        // 10x10 bounds -- proving canvas size round-trips as its own,
        // independent document-level value, not something derived from
        // whichever layer happens to be on top.
        if let Err(err) = write(&mut bytes, &layers, &history, (20, 15), None, &mut store) {
            unreachable!("{err:?}");
        }

        let (_dir2, mut fresh_store) = real_tile_store();
        bytes.set_position(0);
        let (restored_layers, restored_history, canvas_size, profile) =
            match read(bytes, &mut fresh_store) {
                Ok(result) => result,
                Err(err) => unreachable!("{err:?}"),
            };

        assert_eq!(canvas_size, (20, 15));
        assert!(
            profile.is_none(),
            "writing with profile: None must read back as no embedded profile"
        );
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

    // CC0-licensed, from the colord-data Debian package -- see
    // corpora/icc/README.md for full provenance. The same real,
    // deliberately non-sRGB profile `aurora-color`'s own tests already
    // use, so a passing test here proves a real round trip through
    // `.aur`, not just a profile this module invented to be convenient.
    const ECI_RGBV2_ICC: &[u8] = include_bytes!("../../../corpora/icc/ECI-RGBv2.icc");

    #[test]
    fn round_trips_a_real_non_srgb_icc_profile() {
        let (_dir, mut store) = real_tile_store();
        let layers = LayerTree::new();
        let history = History::new();
        let profile = match aurora_color::IccProfile::from_bytes(ECI_RGBV2_ICC) {
            Ok(profile) => profile,
            Err(err) => unreachable!("{err:?}"),
        };

        let mut bytes = Cursor::new(Vec::new());
        if let Err(err) = write(
            &mut bytes,
            &layers,
            &history,
            (1, 1),
            Some(&profile),
            &mut store,
        ) {
            unreachable!("{err:?}");
        }

        let (_dir2, mut fresh_store) = real_tile_store();
        bytes.set_position(0);
        let (_, _, _, restored_profile) = match read(bytes, &mut fresh_store) {
            Ok(result) => result,
            Err(err) => unreachable!("{err:?}"),
        };

        let Some(restored_profile) = restored_profile else {
            unreachable!("writing with Some(profile) must read back as a real embedded profile");
        };
        // Re-serializing the restored profile must itself succeed --
        // real, checked evidence it's a genuinely usable `lcms2`
        // profile, not just bytes that happened to survive the ZIP trip
        // unexamined.
        if let Err(err) = restored_profile.to_bytes() {
            unreachable!("the restored profile must itself be a real, usable profile: {err:?}");
        }
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
        if let Err(err) = write(&mut bytes, &layers, &history, (10, 10), None, &mut store) {
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

    /// A real container holding `manifest_bytes` verbatim, a real
    /// (empty) history entry, and whatever `extra` entries a test needs
    /// on top -- the shared "hand-craft a `.aur` file the writer would
    /// never produce" step behind the hardening tests below.
    fn container_with(manifest_bytes: &[u8], extra: &[(String, Vec<u8>)]) -> Cursor<Vec<u8>> {
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut bytes);
            let deflated = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            if let Err(err) = zip.start_file("manifest", deflated) {
                unreachable!("{err:?}");
            }
            if let Err(err) = std::io::Write::write_all(&mut zip, manifest_bytes) {
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
            for (name, contents) in extra {
                if let Err(err) = zip.start_file(name.clone(), deflated) {
                    unreachable!("{err:?}");
                }
                if let Err(err) = std::io::Write::write_all(&mut zip, contents) {
                    unreachable!("{err:?}");
                }
            }
            if let Err(err) = zip.finish() {
                unreachable!("{err:?}");
            }
        }
        bytes.set_position(0);
        bytes
    }

    #[test]
    fn read_rejects_a_manifest_declaring_a_layer_past_the_document_ceiling() {
        // The tile-scan loop derives its own iteration count from these
        // bounds, so without the check this is not a slow read but an
        // unfinishable one (~2.8e14 iterations for a 376-byte file) --
        // on `aurora-app`'s pre-window startup path, where it looks
        // exactly like a hung launch. The elapsed-time assertion is the
        // point of the test: the answer must come back immediately, not
        // eventually.
        let mut layers = LayerTree::new();
        let huge = aurora_core::Rect {
            x: 0,
            y: 0,
            width: u32::MAX,
            height: u32::MAX,
        };
        if let Err(err) = layers.add_pixel_layer("huge", huge, None) {
            unreachable!("{err:?}");
        }
        let manifest = ManifestReadForTest {
            version: super::MANIFEST_VERSION,
            canvas_width: 1,
            canvas_height: 1,
            color_space: super::ColorSpaceTag::Srgb,
            layers,
        };
        let manifest_bytes = match postcard::to_allocvec(&manifest) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };

        let (_dir, mut store) = real_tile_store();
        let started = std::time::Instant::now();
        match read(container_with(&manifest_bytes, &[]), &mut store) {
            Err(super::IoError::LayerBoundsTooLarge { width, height, max }) => {
                assert_eq!(width, u32::MAX);
                assert_eq!(height, u32::MAX);
                assert_eq!(max, aurora_core::MAX_DOCUMENT_EXTENT);
            }
            other => unreachable!("expected LayerBoundsTooLarge, got {other:?}"),
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "an oversized declared extent must be rejected up front, not looped over"
        );
    }

    #[test]
    fn read_accepts_bounds_exactly_at_the_document_ceiling() {
        // The other side of the same check: the ceiling is documented,
        // legal scope (PRD §7.3.1), so it must not be what gets
        // rejected. Only the tile grid is exercised here -- no tile
        // entries exist, so every lookup misses and leaves the store at
        // its own blank default.
        let mut layers = LayerTree::new();
        let at_ceiling = aurora_core::Rect {
            x: 0,
            y: 0,
            width: aurora_core::MAX_DOCUMENT_EXTENT,
            height: 1,
        };
        if let Err(err) = layers.add_pixel_layer("wide", at_ceiling, None) {
            unreachable!("{err:?}");
        }
        let manifest = ManifestReadForTest {
            version: super::MANIFEST_VERSION,
            canvas_width: aurora_core::MAX_DOCUMENT_EXTENT,
            canvas_height: 1,
            color_space: super::ColorSpaceTag::Srgb,
            layers,
        };
        let manifest_bytes = match postcard::to_allocvec(&manifest) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        let (_dir, mut store) = real_tile_store();
        if let Err(err) = read(container_with(&manifest_bytes, &[]), &mut store) {
            unreachable!("bounds at the documented ceiling must still read: {err:?}");
        }
    }

    #[test]
    fn read_rejects_a_tile_entry_far_larger_than_one_tile_can_hold() {
        // The zip-bomb shape: a few kilobytes on disk that expand to
        // orders of magnitude more in RAM. A tile entry's real ceiling
        // is exactly one tile's worth of f16 RGBA samples, so anything
        // past `MAX_TILE_ENTRY_BYTES` is refused before it is read
        // rather than allocated first and validated after.
        let mut layers = LayerTree::new();
        let id = match layers.add_pixel_layer("Background", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(surface) = layers.surface_id(id) else {
            unreachable!("id was just created as a pixel layer");
        };
        let manifest = ManifestReadForTest {
            version: super::MANIFEST_VERSION,
            canvas_width: 10,
            canvas_height: 10,
            color_space: super::ColorSpaceTag::Srgb,
            layers,
        };
        let manifest_bytes = match postcard::to_allocvec(&manifest) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        let bomb = (
            super::tile_entry_name(surface, TileId { x: 0, y: 0 }),
            vec![0u8; 4 * 1024 * 1024],
        );
        let container = container_with(&manifest_bytes, std::slice::from_ref(&bomb));
        assert!(
            container.get_ref().len() < 64 * 1024,
            "the crafted container must really be small on disk -- that is the whole attack"
        );

        let (_dir, mut store) = real_tile_store();
        match read(container, &mut store) {
            Err(super::IoError::EntryTooLarge { size, cap, .. }) => {
                assert!(size > cap, "{size} must exceed the {cap}-byte cap");
            }
            other => unreachable!("expected EntryTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn read_rejects_a_manifest_whose_layers_add_up_past_the_whole_document_tile_budget() {
        // Each of these layers is individually legal -- exactly at the
        // documented document ceiling, which
        // `read_accepts_bounds_exactly_at_the_document_ceiling` proves
        // must keep working. What is not legal is their sum: the
        // per-layer check says nothing about layer count, and a
        // `LayerEntry` costs tens of bytes on the wire, so without a
        // whole-document budget a small file could stack these until the
        // tile scan never finished. As with the single-layer test above,
        // the elapsed-time assertion is the point.
        let mut layers = LayerTree::new();
        let at_ceiling = aurora_core::Rect {
            x: 0,
            y: 0,
            width: aurora_core::MAX_DOCUMENT_EXTENT,
            height: aurora_core::MAX_DOCUMENT_EXTENT,
        };
        for index in 0..64 {
            if let Err(err) = layers.add_pixel_layer(format!("huge {index}"), at_ceiling, None) {
                unreachable!("{err:?}");
            }
        }
        let manifest = ManifestReadForTest {
            version: super::MANIFEST_VERSION,
            canvas_width: 1,
            canvas_height: 1,
            color_space: super::ColorSpaceTag::Srgb,
            layers,
        };
        let manifest_bytes = match postcard::to_allocvec(&manifest) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        let container = container_with(&manifest_bytes, &[]);
        assert!(
            container.get_ref().len() < 16 * 1024,
            "the crafted container must really be small on disk -- that is the whole attack"
        );

        let (_dir, mut store) = real_tile_store();
        let started = std::time::Instant::now();
        match read(container, &mut store) {
            Err(super::IoError::TooManyTiles { total, max }) => {
                assert!(total > max, "{total} must exceed the {max}-tile budget");
                assert_eq!(max, super::MAX_TOTAL_TILES_PER_DOCUMENT);
            }
            other => unreachable!("expected TooManyTiles, got {other:?}"),
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "an over-budget manifest must be rejected on arithmetic, not by scanning it"
        );
    }

    #[test]
    fn read_rejects_a_manifest_declaring_a_canvas_past_the_document_ceiling() {
        // `aurora-app` stores this straight into its own live canvas
        // size and later allocates `width * height * 4` samples from it,
        // so an unchecked `u32::MAX` here is an allocation request no
        // machine can serve -- from a file a few hundred bytes long. The
        // layer bounds are deliberately tiny: the canvas value alone is
        // what must be refused.
        let mut layers = LayerTree::new();
        if let Err(err) = layers.add_pixel_layer("small", bounds(), None) {
            unreachable!("{err:?}");
        }
        let manifest = ManifestReadForTest {
            version: super::MANIFEST_VERSION,
            canvas_width: u32::MAX,
            canvas_height: u32::MAX,
            color_space: super::ColorSpaceTag::Srgb,
            layers,
        };
        let manifest_bytes = match postcard::to_allocvec(&manifest) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        let (_dir, mut store) = real_tile_store();
        match read(container_with(&manifest_bytes, &[]), &mut store) {
            Err(super::IoError::CanvasTooLarge { width, height, max }) => {
                assert_eq!(width, u32::MAX);
                assert_eq!(height, u32::MAX);
                assert_eq!(max, aurora_core::MAX_DOCUMENT_EXTENT);
            }
            other => unreachable!("expected CanvasTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn read_accepts_a_canvas_exactly_at_the_document_ceiling() {
        // The other side of the same check: the ceiling is documented,
        // legal scope (PRD §7.3.1), so it must not be what gets
        // rejected.
        let mut layers = LayerTree::new();
        if let Err(err) = layers.add_pixel_layer("small", bounds(), None) {
            unreachable!("{err:?}");
        }
        let manifest = ManifestReadForTest {
            version: super::MANIFEST_VERSION,
            canvas_width: aurora_core::MAX_DOCUMENT_EXTENT,
            canvas_height: aurora_core::MAX_DOCUMENT_EXTENT,
            color_space: super::ColorSpaceTag::Srgb,
            layers,
        };
        let manifest_bytes = match postcard::to_allocvec(&manifest) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        let (_dir, mut store) = real_tile_store();
        let (_, _, canvas_size, _) = match read(container_with(&manifest_bytes, &[]), &mut store) {
            Ok(result) => result,
            Err(err) => unreachable!("a canvas at the documented ceiling must still read: {err:?}"),
        };
        assert_eq!(
            canvas_size,
            (
                aurora_core::MAX_DOCUMENT_EXTENT,
                aurora_core::MAX_DOCUMENT_EXTENT
            )
        );
    }

    #[test]
    fn read_rejects_a_manifest_whose_layer_tree_is_cyclic_rather_than_aborting() {
        // Measured before the fix: a 226-byte container carrying exactly
        // this tree killed the process with `fatal runtime error: stack
        // overflow` (exit 134, core dumped) -- on `aurora-app`'s own
        // pre-window startup path. Not a panic anything could catch.
        //
        // The tree is hand-crafted here because every path through
        // `LayerTree`'s own API refuses to build one, which is exactly
        // why nothing had caught this: the type's derived `Deserialize`
        // was the one way in that skipped every one of those checks.
        let manifest_bytes = {
            #[derive(serde::Serialize)]
            enum Kind {
                #[allow(dead_code)]
                Pixel {
                    x: i32,
                    y: i32,
                    w: u32,
                    h: u32,
                },
                Group {
                    children: Vec<u64>,
                },
            }
            #[derive(serde::Serialize)]
            struct Lock {
                transparency: bool,
                pixels: bool,
                position: bool,
            }
            #[derive(serde::Serialize)]
            struct Entry {
                name: String,
                parent: Option<u64>,
                kind: Kind,
                opacity: f32,
                fill_opacity: f32,
                blend_mode: u32,
                visible: bool,
                lock: Lock,
                mask: Option<()>,
            }
            #[derive(serde::Serialize)]
            struct Tree {
                ids: u64,
                layers: std::collections::HashMap<u64, Entry>,
                roots: Vec<u64>,
            }
            #[derive(serde::Serialize)]
            struct Manifest {
                version: u32,
                canvas_width: u32,
                canvas_height: u32,
                color_space: u32,
                layers: Tree,
            }
            let mut tree_layers = std::collections::HashMap::new();
            tree_layers.insert(
                0u64,
                Entry {
                    name: "cycle".to_owned(),
                    parent: None,
                    kind: Kind::Group { children: vec![0] },
                    opacity: 1.0,
                    fill_opacity: 1.0,
                    blend_mode: 0,
                    visible: true,
                    lock: Lock {
                        transparency: false,
                        pixels: false,
                        position: false,
                    },
                    mask: None,
                },
            );
            let manifest = Manifest {
                version: super::MANIFEST_VERSION,
                canvas_width: 1,
                canvas_height: 1,
                color_space: 0,
                layers: Tree {
                    ids: 1,
                    layers: tree_layers,
                    roots: vec![0],
                },
            };
            match postcard::to_allocvec(&manifest) {
                Ok(bytes) => bytes,
                Err(err) => unreachable!("{err:?}"),
            }
        };

        let (_dir, mut store) = real_tile_store();
        match read(container_with(&manifest_bytes, &[]), &mut store) {
            Err(super::IoError::ManifestDeserialization(_)) => {}
            other => unreachable!("expected ManifestDeserialization, got {other:?}"),
        }
    }

    /// One unreadable tile must not cost a background autosave the
    /// *whole* document (0.52.2). `write` refuses — correctly, and
    /// unchanged: an explicit save that quietly dropped content would be
    /// the worst failure this project can have. `write_best_effort`
    /// exists for the other caller, the one whose only alternative to an
    /// incomplete file is no file at all, and whose failure the user
    /// cannot see: before it existed, one permanently unreadable tile
    /// aborted every autosave for the rest of the session, so every other
    /// layer and every subsequent edit silently stopped being protected.
    ///
    /// The unreadable tile is manufactured the way `aurora-app`'s own
    /// `composite_document_refuses_to_export_...` test does it: a
    /// one-tile store budget forces the first layer's tile out to the
    /// scratch disk, `flush` makes that write real, and truncating the
    /// file it landed in leaves a well-formed-but-short ATIL file that
    /// `aurora_tile::codec::decode` rejects on every read.
    #[test]
    fn best_effort_write_skips_an_unreadable_tile_while_write_still_refuses() {
        let (dir, mut store) = one_tile_store();
        let mut layers = LayerTree::new();
        let mut history = History::new();
        let mut layer_ids = Vec::new();
        for name in ["broken", "intact"] {
            let id = match history.add_pixel_layer(&mut layers, name, bounds(), None) {
                Ok(id) => id,
                Err(err) => unreachable!("{err:?}"),
            };
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            // A distinctive, non-blank first texel, so a tile that does
            // survive is provably the real one and not a blank stand-in.
            let tile = match store.get_mut(surface, TileId { x: 0, y: 0 }) {
                Ok(tile) => tile,
                Err(err) => unreachable!("{err:?}"),
            };
            if let Some(sample) = tile.texels_mut().first_mut() {
                *sample = f16::from_f32(0.5);
            }
            layer_ids.push((id, surface));
        }
        // Touching "intact" above evicted "broken"'s tile; `flush` makes
        // that write real so the file truncated below is the one a later
        // page-in will read.
        if let Err(err) = store.flush() {
            unreachable!("test-local scratch disk must accept the write: {err:?}");
        }
        break_the_only_scratch_file(&dir);

        // The explicit-save contract, unchanged.
        let mut refused = Cursor::new(Vec::new());
        match write(&mut refused, &layers, &history, (10, 10), None, &mut store) {
            Err(super::IoError::Tile(_)) => {}
            Ok(()) => unreachable!("an explicit save must not silently drop an unreadable tile"),
            Err(other) => unreachable!("expected IoError::Tile, got {other:?}"),
        }

        // The autosave contract: write what can be written, and say what
        // could not.
        let mut salvaged = Cursor::new(Vec::new());
        let skipped =
            match write_best_effort(&mut salvaged, &layers, &history, (10, 10), None, &mut store) {
                Ok(skipped) => skipped,
                Err(err) => unreachable!("a best-effort write must still produce a file: {err:?}"),
            };
        let [only] = skipped.as_slice() else {
            unreachable!("exactly one tile is unreadable, got {skipped:?}");
        };
        let Some(&(_, broken_surface)) = layer_ids.first() else {
            unreachable!("two layers were just created");
        };
        assert_eq!(only.surface, broken_surface);
        assert_eq!(only.tile, TileId { x: 0, y: 0 });
        assert!(
            only.reason.contains("corrupt tile file"),
            "the skip must carry the real underlying tile error: {}",
            only.reason
        );

        // And the file it produced is a real, readable `.aur` document
        // holding every layer -- the broken one blank, the other one's
        // pixels intact.
        let (_fresh_dir, mut fresh_store) = real_tile_store();
        salvaged.set_position(0);
        let (restored_layers, _history, canvas_size, _profile) =
            match read(salvaged, &mut fresh_store) {
                Ok(result) => result,
                Err(err) => unreachable!("the salvaged autosave must reopen: {err:?}"),
            };
        assert_eq!(canvas_size, (10, 10));
        assert_eq!(restored_layers.len(), 2, "no layer was dropped");
        let Some(&(_, intact_surface)) = layer_ids.get(1) else {
            unreachable!("two layers were just created");
        };
        for (surface, expected) in [(broken_surface, 0.0), (intact_surface, 0.5)] {
            let tile = match fresh_store.get(surface, TileId { x: 0, y: 0 }) {
                Ok(tile) => tile,
                Err(err) => unreachable!("{err:?}"),
            };
            let Some(&first) = tile.texels().first() else {
                unreachable!("a tile's texel buffer is never empty");
            };
            #[allow(clippy::float_cmp)]
            {
                assert_eq!(first.to_f32(), expected);
            }
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
