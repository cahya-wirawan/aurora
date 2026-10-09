//! An opened document's tiles, encoded on the background decode thread
//! (0.153.0).
//!
//! Until 0.152.0 the UI thread's install wrote every opened layer's
//! pixels into the live [`aurora_tile::TileStore`] texel by texel
//! (`aurora_io::write_into_store_at`, `aurora_io::write_psd_mask`) — about
//! 0.7 s (dev) for a 4096² four-layer PSD, most of it the `codec::encode`
//! of each tile the store evicts as the write overruns its budget. The
//! decode thread now builds each tile itself, from a blank tile and the
//! same per-tile fill the store writers run, and encodes it
//! ([`aurora_tile::EncodedTile`]); the UI thread only hands each one to
//! [`aurora_tile::TileStore::insert_encoded`], O(1) bookkeeping per tile.
//!
//! What it preserves, and how:
//!
//! - **Bit-identical tiles.** The builders (`aurora_io::encode_image_tiles_at`,
//!   `aurora_io::encode_psd_mask`) share their per-tile body with the
//!   writers, and produce exactly the tiles the writers touch — every tile
//!   a placed image or a mask region overlaps, fully transparent ones
//!   included. They start from a blank tile, which is what the writers
//!   find: the install sweeps the outgoing document first.
//! - **Order.** [`insert_prepared`] runs only after the sweep
//!   (`crate::replace_document_pixels_prepared`), pixels first and then
//!   masks, as before; surfaces are resolved there, on the incoming tree.
//! - **Trust.** Only this process's own encoder makes an `EncodedTile`, so
//!   nothing read from a file reaches the store unvalidated. `.aur` opens
//!   keep the old path (`crate::read_prechecked_aur`): their tile entries
//!   are untrusted and go through `codec::decode`.
//! - **Memory (0.154.0).** A PSD is streamed ([`PreparedPsd::stream`]):
//!   parsed from the file without reading it whole, each layer decoded,
//!   encoded, written into a staging area in the store's scratch
//!   directory ([`aurora_tile::StagingArea`]) and dropped before the next
//!   layer is read. The UI thread adopts the staged files
//!   ([`aurora_tile::TileStore::insert_staged`]) — no bytes in memory.
//!   Measured: a 2 GiB four-layer 16-bit PSD peaks at +1,277 MiB, against
//!   +4,352 MiB for the 0.153.0 path. Every user mask's coverage is still
//!   held until its layer is handed on, and a tile whose staging write
//!   fails stays in memory ([`PreparedTile::Memory`]).

use std::path::Path;

use aurora_doc::LayerId;
use aurora_io::{EncodedTiles, Image, IoError, PsdMaskPixels};
use aurora_tile::{EncodedTile, StagedTile, StagingArea, StagingRoot, TileId};

/// One prepared tile (0.154.0): already on the scratch disk in a staging
/// area the store adopts ([`aurora_tile::TileStore::insert_staged`]), or —
/// when no staging area could be made or a staging write failed — still
/// in memory, inserted as 0.153.0 did
/// ([`aurora_tile::TileStore::insert_encoded`], whose failed-write rule J1
/// then applies).
#[derive(Debug)]
pub(crate) enum PreparedTile {
    Memory(EncodedTile),
    Staged(StagedTile),
}

impl PreparedTile {
    /// The encoded length in bytes.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Memory(tile) => tile.len(),
            Self::Staged(tile) => tile.len(),
        }
    }

    /// The encoded bytes; a staged tile's read back from its file.
    #[cfg(test)]
    pub(crate) fn bytes(&self) -> Vec<u8> {
        match self {
            Self::Memory(tile) => tile.bytes().to_vec(),
            Self::Staged(tile) => tile
                .path()
                .and_then(|path| std::fs::read(path).ok())
                .unwrap_or_default(),
        }
    }
}

/// A layer's or mask's prepared tiles.
pub(crate) type PreparedTiles = Vec<(TileId, PreparedTile)>;

/// `tiles`, kept in memory.
fn in_memory(tiles: EncodedTiles) -> PreparedTiles {
    tiles
        .into_iter()
        .map(|(id, tile)| (id, PreparedTile::Memory(tile)))
        .collect()
}

/// One pixel layer's encoded tiles, or why its placement was refused
/// (`aurora_io::IoError::ImagePlacementOutOfRange`, the one error the
/// old writer could return without a paging failure).
#[derive(Debug)]
pub(crate) struct PreparedLayer {
    pub(crate) layer: LayerId,
    pub(crate) tiles: Result<PreparedTiles, IoError>,
}

/// One layer mask's encoded coverage tiles.
#[derive(Debug)]
pub(crate) struct PreparedMask {
    pub(crate) layer: LayerId,
    pub(crate) tiles: PreparedTiles,
}

/// A decoded flat image (PNG/JPEG/TIFF), already encoded. The image
/// itself is dropped: the install needs only its size.
#[derive(Debug)]
pub(crate) struct PreparedImage {
    pub(crate) size: (u32, u32),
    pub(crate) tiles: Result<PreparedTiles, IoError>,
}

impl PreparedImage {
    /// Encodes `image`'s tiles at surface-local `(0, 0)`, the flat
    /// image's one layer, and drops it.
    pub(crate) fn new(image: Image) -> Self {
        let size = (image.width(), image.height());
        let tiles = aurora_io::encode_image_tiles_at(&image, 0, 0).map(in_memory);
        drop(image);
        Self { size, tiles }
    }
}

/// A decoded PSD with its pixels already encoded: everything
/// `aurora_io::PsdDocument` carries, its images replaced by tiles.
#[derive(Debug)]
pub(crate) struct PreparedPsd {
    pub(crate) layers: aurora_doc::LayerTree,
    pub(crate) history: aurora_doc::History,
    pub(crate) canvas_size: (u32, u32),
    pub(crate) pixels: Vec<PreparedLayer>,
    pub(crate) masks: Vec<PreparedMask>,
    pub(crate) report: aurora_io::PsdImportReport,
}

impl PreparedPsd {
    /// Encodes every layer's and mask's tiles, dropping each layer's image
    /// as soon as its tiles exist — the 0.153.0 in-memory path, kept for
    /// the tests that compare against it ([`Self::stream`] is the open).
    #[cfg(test)]
    pub(crate) fn new(document: aurora_io::PsdDocument) -> Self {
        let aurora_io::PsdDocument {
            layers,
            history,
            canvas_size,
            pixels,
            masks,
            report,
        } = document;
        let pixels = pixels
            .into_iter()
            .map(|placed| {
                let (x, y) = placed.offset;
                let tiles = aurora_io::encode_image_tiles_at(&placed.image, x, y).map(in_memory);
                PreparedLayer {
                    layer: placed.layer,
                    tiles,
                }
            })
            .collect();
        Self {
            layers,
            history,
            canvas_size,
            pixels,
            masks: prepare_masks(&masks),
            report,
        }
    }
}

/// Encodes each `(layer, image, offset)` entry — the shape the pre-0.153.0
/// install took, kept for the tests that drive it.
#[cfg(test)]
pub(crate) fn prepare_pixels(pixels: &[(LayerId, &Image, (u32, u32))]) -> Vec<PreparedLayer> {
    pixels
        .iter()
        .map(|(layer, image, (x, y))| PreparedLayer {
            layer: *layer,
            tiles: aurora_io::encode_image_tiles_at(image, *x, *y).map(in_memory),
        })
        .collect()
}

/// Encodes each mask's coverage tiles.
#[cfg(test)]
pub(crate) fn prepare_masks(masks: &[PsdMaskPixels]) -> Vec<PreparedMask> {
    masks
        .iter()
        .map(|mask| PreparedMask {
            layer: mask.layer,
            tiles: in_memory(aurora_io::encode_psd_mask(mask)),
        })
        .collect()
}

impl PreparedPsd {
    /// Opens the PSD/PSB at `path` **streaming** (0.154.0): the file is
    /// parsed from a `BufReader` (never read whole,
    /// `aurora_io::read_psd_streaming`), and each layer's image, as the
    /// reader hands it over, is encoded, written into a staging area made
    /// from `staging` and dropped before the next layer is read — so the
    /// decode thread holds about one layer, never the file, the decoded
    /// document or its encoded tiles. Without `staging` (or when a staging
    /// area or write fails) the encoded tiles stay in memory, as in 0.153.0.
    ///
    /// Nothing reaches the live store here: on any failure every staged
    /// file is deleted as the prepared tiles drop, and the install stays
    /// one atomic step on the UI thread.
    pub(crate) fn stream(
        path: &Path,
        staging: Option<&StagingRoot>,
    ) -> Result<Self, crate::OpenFailure> {
        let file = std::fs::File::open(path).map_err(|err| {
            tracing::warn!(path = %path.display(), %err, "failed to open the chosen file");
            crate::OpenFailure::Read(err)
        })?;
        // Not once the session is ending (0.151.0 review C1's flag): the
        // shutdown cleanup is about to remove the scratch directory, and a
        // detached decode must not put a staging directory back into it.
        let staging = staging.filter(|_| !session_ending());
        let area = staging.and_then(|root| match root.area() {
            Ok(area) => Some(area),
            Err(err) => {
                tracing::warn!(%err, "no tile staging area; the opened tiles stay in memory");
                None
            }
        });
        let mut sink = StreamSink {
            area,
            pixels: Vec::new(),
            masks: Vec::new(),
        };
        let reader = std::io::BufReader::with_capacity(STREAM_BUFFER, file);
        let document =
            aurora_io::read_psd_streaming(reader, &mut sink).map_err(crate::OpenFailure::Decode)?;
        let StreamSink { pixels, masks, .. } = sink;
        Ok(Self {
            layers: document.layers,
            history: document.history,
            canvas_size: document.canvas_size,
            pixels,
            masks,
            report: document.report,
        })
    }
}

/// Whether the session is ending (`crate::SESSION_ENDING`, 0.151.0).
fn session_ending() -> bool {
    crate::SESSION_ENDING.load(std::sync::atomic::Ordering::SeqCst)
}

/// The streaming open's read buffer: the metadata is read in small pieces.
const STREAM_BUFFER: usize = 1 << 16;

/// [`PreparedPsd::stream`]'s sink: encodes and stages one layer at a time.
struct StreamSink {
    area: Option<StagingArea>,
    pixels: Vec<PreparedLayer>,
    masks: Vec<PreparedMask>,
}

impl StreamSink {
    /// Stages `tiles` one by one, keeping in memory any the area cannot
    /// take (and every one without an area).
    fn stage(&mut self, tiles: EncodedTiles) -> PreparedTiles {
        // A detached decode still running at quit (0.154.0 review I3)
        // stops staging: its result is never installed, and the shutdown
        // cleanup is removing the scratch directory. A write already under
        // way as the flag flips can still race that cleanup — then
        // `create_new` fails inside the removed directory and the tile
        // stays in memory, or, if it wins, one stray file is left for the
        // next session's orphaned-scratch sweep.
        if session_ending() {
            self.area = None;
            return in_memory(tiles);
        }
        let Some(area) = self.area.as_mut() else {
            return in_memory(tiles);
        };
        tiles
            .into_iter()
            .map(|(id, tile)| match area.stage(&tile) {
                Ok(staged) => (id, PreparedTile::Staged(staged)),
                Err(err) => {
                    tracing::warn!(%err, "a staging write failed; the tile stays in memory");
                    (id, PreparedTile::Memory(tile))
                }
            })
            .collect()
    }
}

impl aurora_io::PsdPixelSink for StreamSink {
    fn layer(&mut self, placed: aurora_io::PsdPixels) -> Result<(), IoError> {
        let (x, y) = placed.offset;
        let encoded = aurora_io::encode_image_tiles_at(&placed.image, x, y);
        // The layer's image is not needed past its tiles (the point).
        drop(placed.image);
        let tiles = encoded.map(|tiles| self.stage(tiles));
        self.pixels.push(PreparedLayer {
            layer: placed.layer,
            tiles,
        });
        Ok(())
    }

    fn mask(&mut self, mask: PsdMaskPixels) -> Result<(), IoError> {
        let encoded = aurora_io::encode_psd_mask(&mask);
        drop(mask.coverage);
        let tiles = self.stage(encoded);
        self.masks.push(PreparedMask {
            layer: mask.layer,
            tiles,
        });
        Ok(())
    }
}

/// Inserts one prepared tile; `false` if the store refused it.
fn insert_one(
    store: &mut aurora_tile::TileStore,
    surface: aurora_tile::SurfaceId,
    id: TileId,
    tile: PreparedTile,
) -> bool {
    match tile {
        PreparedTile::Memory(tile) => {
            store.insert_encoded(surface, id, tile);
            true
        }
        PreparedTile::Staged(tile) => match store.insert_staged(surface, id, tile) {
            Ok(()) => true,
            Err(err) => {
                tracing::error!(%err, "a staged tile was refused by the tile store");
                false
            }
        },
    }
}

/// Inserts every prepared tile into `store` on its layer's surface (or
/// mask surface) in `incoming_layers`, and returns how many layers or
/// masks could not be written — the count the user is told about
/// (`crate::unwritten_layers_item`), with the same rules as the old
/// writers: a layer with no surface, a refused placement, or a mask whose
/// layer has no mask surface each count once.
///
/// Must run after the outgoing document's sweep (see
/// `crate::replace_document_pixels_prepared`): surfaces are derived from
/// layer ids, which restart at zero, so a sweep after this would erase
/// what it inserted.
pub(crate) fn insert_prepared(
    store: &mut aurora_tile::TileStore,
    incoming_layers: &aurora_doc::LayerTree,
    pixels: Vec<PreparedLayer>,
    masks: Vec<PreparedMask>,
) -> usize {
    let mut failed = 0_usize;
    for PreparedLayer { layer, tiles } in pixels {
        let Some(surface) = incoming_layers.surface_id(layer) else {
            tracing::error!(
                ?layer,
                "an opened document's layer has no surface; its pixels were not written into the \
                 tile store and it will be blank"
            );
            failed += 1;
            continue;
        };
        match tiles {
            Ok(tiles) => {
                let mut refused = false;
                for (id, tile) in tiles {
                    refused |= !insert_one(store, surface, id, tile);
                }
                if refused {
                    failed += 1;
                }
            }
            Err(err) => {
                tracing::warn!(
                    ?err,
                    ?layer,
                    "failed to write an opened layer's pixels into the tile store"
                );
                failed += 1;
            }
        }
    }
    // After every layer's pixels, as `write_psd_mask` ran after every
    // `write_into_store_at` (0.147.0): the order only matters against the
    // sweep, but keeping it costs nothing.
    for PreparedMask { layer, tiles } in masks {
        let Some(surface) = incoming_layers.mask_surface_id(layer) else {
            tracing::warn!(
                ?layer,
                "failed to write an opened layer mask's coverage into the tile store: the layer \
                 has no mask surface"
            );
            failed += 1;
            continue;
        };
        let mut refused = false;
        for (id, tile) in tiles {
            refused |= !insert_one(store, surface, id, tile);
        }
        if refused {
            failed += 1;
        }
    }
    failed
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Mutex, mpsc};
    use std::time::Duration;

    use aurora_core::Rect;
    use aurora_doc::{History, LayerId, LayerTree};
    use aurora_io::{Image, PsdMaskPixels};
    use aurora_tile::{SurfaceId, TileId, TileStore};
    use half::f16;

    use super::{prepare_masks, prepare_pixels};
    use crate::background_autosave::{AutosaveJob, AutosaveWorker, WrittenTemp, test_support};

    const WAIT: Duration = Duration::from_mins(1);

    fn store(budget: usize) -> (tempfile::TempDir, TileStore) {
        let dir = tempdir();
        let Some(budget) = std::num::NonZeroUsize::new(budget) else {
            unreachable!("non-zero literal");
        };
        match TileStore::new(dir.path().to_path_buf(), budget) {
            Ok(store) => (dir, store),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn tempdir() -> tempfile::TempDir {
        match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn rect(width: u32, height: u32) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    /// Partly transparent, premultiplied-looking noise: alpha varies per
    /// texel, every fourth texel fully transparent, colour never above
    /// alpha. Deterministic in `seed`.
    fn noisy_image(width: u32, height: u32, seed: u64) -> Image {
        let mut state = seed | 1;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut samples = Vec::with_capacity(width as usize * height as usize * 4);
        for texel in 0..(width as usize * height as usize) {
            let alpha = if texel % 4 == 0 {
                0.0
            } else {
                (next() % 1000) as f32 / 999.0
            };
            for _ in 0..3 {
                let colour = (next() % 1000) as f32 / 999.0 * alpha;
                samples.push(f16::from_f32(colour));
            }
            samples.push(f16::from_f32(alpha));
        }
        match Image::new(width, height, aurora_color::IccProfile::srgb(), samples) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn transparent_image(width: u32, height: u32) -> Image {
        match Image::new(
            width,
            height,
            aurora_color::IccProfile::srgb(),
            vec![f16::ZERO; width as usize * height as usize * 4],
        ) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn add_layer(
        layers: &mut LayerTree,
        history: &mut History,
        name: &str,
        w: u32,
        h: u32,
    ) -> LayerId {
        match history.add_pixel_layer(layers, name, rect(w, h), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn add_mask(layers: &mut LayerTree, layer: LayerId, w: u32, h: u32) {
        if let Err(err) = layers.add_mask(layer, rect(w, h)) {
            unreachable!("{err:?}");
        }
    }

    /// What the install receives: a multi-layer document with a cropped
    /// layer at an offset that crosses tile seams, a fully transparent
    /// layer (its one tile is still stored), partly transparent noise, and
    /// two masks (one with a `NaN`, clamped and short-buffer coverage).
    struct Incoming {
        layers: LayerTree,
        history: History,
        images: Vec<(LayerId, Image, (u32, u32))>,
        masks: Vec<PsdMaskPixels>,
    }

    fn incoming(seed: u64) -> Incoming {
        let mut layers = LayerTree::new();
        let mut history = History::new();
        let full = add_layer(&mut layers, &mut history, "full", 600, 520);
        let cropped = add_layer(&mut layers, &mut history, "cropped", 600, 520);
        let clear = add_layer(&mut layers, &mut history, "clear", 600, 520);
        add_mask(&mut layers, full, 600, 520);
        add_mask(&mut layers, cropped, 600, 520);
        let images = vec![
            (full, noisy_image(600, 520, seed), (0, 0)),
            (cropped, noisy_image(300, 200, seed ^ 0xABCD), (130, 270)),
            (clear, transparent_image(256, 256), (256, 256)),
        ];
        let mut coverage: Vec<f16> = (0..400 * 300)
            .map(|i| f16::from_f32(((i * 7 + seed as usize) % 255) as f32 / 254.0))
            .collect();
        if let Some(first) = coverage.first_mut() {
            *first = f16::NAN;
        }
        if let Some(second) = coverage.get_mut(1) {
            *second = f16::from_f32(3.0);
        }
        let masks = vec![
            PsdMaskPixels {
                layer: full,
                offset: (10, 20),
                width: 400,
                height: 300,
                coverage,
            },
            PsdMaskPixels {
                layer: cropped,
                offset: (250, 0),
                width: 300,
                height: 300,
                // Short: the tail fails open to full coverage.
                coverage: vec![f16::from_f32(0.25); 300 * 100],
            },
        ];
        Incoming {
            layers,
            history,
            images,
            masks,
        }
    }

    /// An outgoing document whose layer and mask ids alias the incoming
    /// ones, painted across a wider area than the incoming layers cover
    /// and partly evicted (pending writes in flight at the sweep).
    fn outgoing(store: &mut TileStore) -> (LayerTree, History) {
        let mut layers = LayerTree::new();
        let mut history = History::new();
        let mut ids = Vec::new();
        for name in ["o0", "o1", "o2", "o3"] {
            ids.push(add_layer(&mut layers, &mut history, name, 1024, 1024));
        }
        for &id in &ids {
            add_mask(&mut layers, id, 1024, 1024);
        }
        for &id in &ids {
            let surfaces = [layers.surface_id(id), layers.mask_surface_id(id)];
            for surface in surfaces.into_iter().flatten() {
                for ty in 0..4 {
                    for tx in 0..4 {
                        match store.get_mut(surface, TileId { x: tx, y: ty }) {
                            Ok(tile) => tile.texels_mut().fill(f16::from_f32(0.75)),
                            Err(err) => unreachable!("{err:?}"),
                        }
                    }
                }
            }
        }
        (layers, history)
    }

    /// The pre-0.153.0 install, verbatim: sweep, then
    /// `write_into_store_at` per layer, then `write_psd_mask` per mask.
    fn install_old_path(store: &mut TileStore, incoming: &Incoming) -> usize {
        let (out_layers, out_history) = outgoing(store);
        let _freed = aurora_doc::forget_document_surfaces(out_layers, out_history, store)
            + store.forget_surface(crate::composite_surface_id());
        let mut failed = 0;
        for (layer, image, (x, y)) in &incoming.images {
            let Some(surface) = incoming.layers.surface_id(*layer) else {
                unreachable!("a pixel layer");
            };
            if aurora_io::write_into_store_at(image, store, surface, *x, *y).is_err() {
                failed += 1;
            }
        }
        for mask in &incoming.masks {
            if aurora_io::write_psd_mask(mask, &incoming.layers, store).is_err() {
                failed += 1;
            }
        }
        failed
    }

    /// The 0.153.0 install: tiles prepared (on the decode thread in the
    /// app), then the production sweep-and-insert.
    fn install_new_path(store: &mut TileStore, incoming: &Incoming) -> usize {
        let pixels: Vec<_> = incoming
            .images
            .iter()
            .map(|(layer, image, offset)| (*layer, image, *offset))
            .collect();
        let prepared = prepare_pixels(&pixels);
        let masks = prepare_masks(&incoming.masks);
        let (out_layers, out_history) = outgoing(store);
        let (_freed, failed) = crate::replace_document_pixels_prepared(
            store,
            out_layers,
            out_history,
            &incoming.layers,
            prepared,
            masks,
        );
        failed
    }

    /// Every surface the incoming document or the outgoing one could hold:
    /// layer and mask surfaces of ids 0..4, read off an outgoing-shaped
    /// tree (ids are derived, so both documents alias them).
    fn surfaces(_layers: &LayerTree) -> Vec<SurfaceId> {
        let mut layers = LayerTree::new();
        let mut history = History::new();
        let mut surfaces = Vec::new();
        for name in ["s0", "s1", "s2", "s3"] {
            let id = add_layer(&mut layers, &mut history, name, 8, 8);
            add_mask(&mut layers, id, 8, 8);
            surfaces.extend(layers.surface_id(id));
            surfaces.extend(layers.mask_surface_id(id));
        }
        surfaces
    }

    /// Asserts both stores hold the same set of tiles over every surface
    /// and a grid past both documents' extents, with equal texels.
    fn assert_same_tiles(old: &mut TileStore, new: &mut TileStore, layers: &LayerTree) -> usize {
        let mut held = 0;
        for surface in surfaces(layers) {
            for ty in 0..5 {
                for tx in 0..5 {
                    let id = TileId { x: tx, y: ty };
                    let contains = old.contains_tile(surface, id);
                    assert_eq!(
                        contains,
                        new.contains_tile(surface, id),
                        "{surface:?} {id:?}: the same tiles are stored (no blank-tile elision, \
                         nothing of the outgoing document left)"
                    );
                    if !contains {
                        continue;
                    }
                    held += 1;
                    let expected = match old.get(surface, id) {
                        Ok(tile) => tile.texels().to_vec(),
                        Err(err) => unreachable!("{err:?}"),
                    };
                    let actual = match new.get(surface, id) {
                        Ok(tile) => tile.texels().to_vec(),
                        Err(err) => unreachable!("{err:?}"),
                    };
                    assert!(
                        expected
                            .iter()
                            .zip(&actual)
                            .all(|(a, b)| a.to_bits() == b.to_bits()),
                        "{surface:?} {id:?}: texels are bit-identical"
                    );
                }
            }
        }
        held
    }

    #[test]
    fn the_prepared_install_is_bit_identical_to_the_old_writers() {
        let doc = incoming(0x5EED);
        let (_d1, mut old) = store(4);
        let (_d2, mut new) = store(4);
        assert_eq!(install_old_path(&mut old, &doc), 0);
        assert_eq!(install_new_path(&mut new, &doc), 0);
        assert_eq!(
            new.resident_len(),
            0,
            "the install itself makes nothing resident (the budget is not touched)"
        );
        // Every incoming tile owes its GPU upload.
        for surface in surfaces(&doc.layers) {
            for ty in 0..5 {
                for tx in 0..5 {
                    let id = TileId { x: tx, y: ty };
                    if new.contains_tile(surface, id) {
                        assert!(new.take_dirty(surface, id).is_some(), "{surface:?} {id:?}");
                    }
                }
            }
        }
        let held = assert_same_tiles(&mut old, &mut new, &doc.layers);
        // full 3x3 + cropped 2x1 + clear 1 + masks 2x2 and 3x2.
        assert_eq!(held, 9 + 2 + 1 + 4 + 6);
        assert!(
            new.contains_tile(SurfaceId::from_raw(2), TileId { x: 1, y: 1 }),
            "the fully transparent overlapped tile is stored, as the old writer stored it"
        );
        assert!(new.resident_len() <= 4, "the budget holds after reading");
        // Again after the writer has landed everything: from the scratch
        // disk rather than `pending`.
        if let Err(err) = new.flush() {
            unreachable!("{err:?}");
        }
        assert_same_tiles(&mut old, &mut new, &doc.layers);
    }

    #[test]
    fn a_refused_placement_and_a_maskless_layer_are_counted_as_before() {
        let mut doc = incoming(7);
        // An offset whose far edge overflows `u32`.
        if let Some(entry) = doc.images.get_mut(1) {
            entry.2 = (u32::MAX - 10, 0);
        }
        // A mask on the layer that has none.
        if let Some(clear) = doc.images.get(2).map(|entry| entry.0) {
            doc.masks.push(PsdMaskPixels {
                layer: clear,
                offset: (0, 0),
                width: 4,
                height: 4,
                coverage: vec![f16::ONE; 16],
            });
        }
        let (_d1, mut old) = store(4);
        let (_d2, mut new) = store(4);
        let old_failed = install_old_path(&mut old, &doc);
        assert_eq!(install_new_path(&mut new, &doc), old_failed);
        assert!(old_failed >= 1, "the overflowing placement is refused");
        assert_same_tiles(&mut old, &mut new, &doc.layers);
    }

    fn reference(doc: &Incoming) -> (tempfile::TempDir, TileStore) {
        let (dir, mut old) = store(64);
        assert_eq!(install_old_path(&mut old, doc), 0);
        (dir, old)
    }

    fn snapshot(path: &std::path::Path, doc: &Incoming, store: &mut TileStore) -> AutosaveJob {
        match crate::snapshot_autosave(
            path,
            &doc.layers,
            &doc.history,
            (600, 520),
            &mut aurora_io::SkippedTiles::new(),
            crate::AUTOSAVE_SNAPSHOT_BUDGET_BYTES,
            store,
        ) {
            crate::SnapshotOutcome::Taken(job) => job,
            other => unreachable!("{other:?}"),
        }
    }

    /// Reads `path` into a fresh store and asserts every tile of `doc`'s
    /// surfaces equals `expected`'s. Blank tiles are not in an autosave,
    /// so only texels are compared.
    fn assert_recovers(path: &std::path::Path, doc: &Incoming, expected: &mut TileStore) {
        let (_dir, mut recovered) = store(64);
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(err) => unreachable!("{err:?}"),
        };
        let read = match aurora_io::read_aur(std::io::BufReader::new(file), &mut recovered) {
            Ok(read) => read,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(read.layers.len(), doc.layers.len());
        for surface in surfaces(&doc.layers) {
            for ty in 0..5 {
                for tx in 0..5 {
                    let id = TileId { x: tx, y: ty };
                    // Read without materializing: `get` would create a
                    // blank tile and change what the store holds.
                    let want = texels_or_blank(expected, surface, id);
                    let got = texels_or_blank(&mut recovered, surface, id);
                    assert!(
                        want.iter()
                            .zip(&got)
                            .all(|(a, b)| a.to_bits() == b.to_bits()),
                        "{surface:?} {id:?}: the recovered tile equals the installed one"
                    );
                }
            }
        }
    }

    /// `id`'s texels if `store` holds it, else a blank tile's, without
    /// creating one.
    fn texels_or_blank(store: &mut TileStore, surface: SurfaceId, id: TileId) -> Vec<f16> {
        if !store.contains_tile(surface, id) {
            return vec![f16::ZERO; aurora_tile::SAMPLES];
        }
        match store.get(surface, id) {
            Ok(tile) => tile.texels().to_vec(),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn a_background_autosave_right_after_the_install_recovers_the_document() {
        let doc = incoming(0xA570);
        let (_d, mut expected) = reference(&doc);
        let dir = tempdir();
        let (_s, mut live) = store(4);
        assert_eq!(install_new_path(&mut live, &doc), 0);
        let mut worker = AutosaveWorker::new(crate::write_autosave_temp, test_support::never);
        // Straight away: the inserted tiles are still in `pending`.
        let first = dir.path().join("first.aur");
        assert_eq!(worker.submit(snapshot(&first, &doc, &mut live)), Some(1));
        // Let the first write land before submitting the second: the
        // worker coalesces, so a second job submitted while the first is
        // still *waiting* replaces it and `first.aur` is never written —
        // the race CI hit on slower Linux/Windows runners (0.161.1). The
        // snapshot was already taken at submit, so waiting changes nothing
        // about what is tested: the bytes were still in `pending` then.
        assert!(worker.wait_idle(WAIT));
        // And once the store's writer has put them on the scratch disk.
        if let Err(err) = live.flush() {
            unreachable!("{err:?}");
        }
        let second = dir.path().join("second.aur");
        assert_eq!(worker.submit(snapshot(&second, &doc, &mut live)), Some(2));
        assert!(worker.wait_idle(WAIT));
        assert_recovers(&first, &doc, &mut expected);
        assert_recovers(&second, &doc, &mut expected);
        let _ = worker.shutdown(crate::background_autosave::SHUTDOWN_WAIT_BOUND);
    }

    #[test]
    fn an_install_while_the_previous_autosave_is_writing_is_safe() {
        let first_doc = incoming(1);
        let second_doc = incoming(2);
        let (_d1, mut first_expected) = reference(&first_doc);
        let (_d2, mut second_expected) = reference(&second_doc);
        let dir = tempdir();
        let path = dir.path().join("aurora-autosave.aur");
        let side = dir.path().join("first-landed.aur");
        let (started_tx, started) = mpsc::channel();
        let (release, release_rx) = mpsc::channel::<()>();
        let started_tx = Mutex::new(started_tx);
        let release_rx = Mutex::new(release_rx);
        let calls = AtomicUsize::new(0);
        let side_copy = side.clone();
        // The first write blocks until released and keeps a copy of
        // its file; every write is real.
        let write = move |job: &AutosaveJob, cancel: &AtomicBool| -> Option<WrittenTemp> {
            let first = calls.fetch_add(1, Ordering::SeqCst) == 0;
            if first {
                if let Ok(tx) = started_tx.lock() {
                    let _ = tx.send(());
                }
                if let Ok(rx) = release_rx.lock() {
                    let _ = rx.recv_timeout(WAIT);
                }
            }
            let written = crate::write_autosave_temp(job, cancel)?;
            if first {
                let _ = std::fs::copy(&written.temp, &side_copy);
            }
            Some(written)
        };
        let mut worker = AutosaveWorker::new(write, test_support::never);
        let (_s, mut live) = store(4);
        assert_eq!(install_new_path(&mut live, &first_doc), 0);
        assert_eq!(
            worker.submit(snapshot(&path, &first_doc, &mut live)),
            Some(1)
        );
        assert!(
            started.recv_timeout(WAIT).is_ok(),
            "the first autosave is writing"
        );
        // The second open: sweeps the first document (whose tiles the
        // in-flight job snapshotted) and inserts its own on the same,
        // aliased surfaces.
        let pixels: Vec<_> = second_doc
            .images
            .iter()
            .map(|(layer, image, offset)| (*layer, image, *offset))
            .collect();
        // The outgoing tree and history are the first document's (rebuilt:
        // `LayerTree` is not `Clone`); the same ids, so the same surfaces.
        let first_again = incoming(1);
        let (_freed, failed) = crate::replace_document_pixels_prepared(
            &mut live,
            first_again.layers,
            first_again.history,
            &second_doc.layers,
            prepare_pixels(&pixels),
            prepare_masks(&second_doc.masks),
        );
        assert_eq!(failed, 0);
        assert_eq!(
            worker.submit(snapshot(&path, &second_doc, &mut live)),
            Some(2)
        );
        assert!(release.send(()).is_ok());
        assert!(worker.wait_idle(WAIT));
        assert_recovers(&side, &first_doc, &mut first_expected);
        assert_recovers(&path, &second_doc, &mut second_expected);
        let mut fresh = second_expected;
        assert_same_tiles(&mut fresh, &mut live, &second_doc.layers);
        let _ = worker.shutdown(crate::background_autosave::SHUTDOWN_WAIT_BOUND);
    }
}
