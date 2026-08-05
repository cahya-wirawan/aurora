//! Bridges a decoded [`Image`] into a document's own real pixel storage
//! (`aurora_tile::TileStore`, [ADR
//! 0010](../../../docs/adr/0010-layer-pixel-storage.md)) and back — the
//! missing piece between `png`/`jpeg`/`tiff::decode`/`encode` and a
//! document layer actually showing anything, which `Image`'s own doc
//! comment named as real, separate, still-open work.
//!
//! [`decode_by_extension`]/[`encode_by_extension`] are the single entry
//! points a real "Open File"/"Save/Export" flow needs instead of
//! picking `png`/`jpeg`/`tiff::decode`/`encode` itself — this crate's
//! first, first-party format dispatcher.

use std::path::Path;

use aurora_tile::{CHANNELS, SurfaceId, TILE, TileId, TileStore};

use crate::error::IoError;
use crate::image::Image;

/// Writes `image`'s own samples into `store`'s `surface`, tile by tile,
/// starting at surface-local `(0, 0)` — matches
/// `aurora_doc::LayerTree::surface_id`'s own local-origin convention
/// (ADR 0010): a pixel layer's surface is always addressed from its own
/// `(0, 0)`, not the document's, so the caller is responsible for
/// having already sized the layer's own `bounds` to `image`'s
/// `width`/`height` (e.g. `aurora_app::document_from_image`'s own
/// shape).
///
/// Touches only the tiles `image`'s own extent overlaps; a partial tile
/// at the right/bottom edge is written only in the region the image
/// actually covers — the rest of a freshly allocated tile is already
/// zero. `image` and a [`aurora_tile::Tile`]'s own `texels` share the
/// same row-major RGBA `f16` layout (see [`Image`]'s own doc comment),
/// so each overlapping row is a plain slice copy, not a per-pixel loop.
/// Marks each touched tile's dirty rectangle, the same as
/// `aurora_brush::stamp_dab`, so a later GPU upload picks it up.
///
/// # Errors
///
/// Returns [`IoError::Tile`] if paging a touched tile in from the
/// scratch disk fails.
pub fn write_into_store(
    image: &Image,
    store: &mut TileStore,
    surface: SurfaceId,
) -> Result<(), IoError> {
    let width = image.width();
    let height = image.height();
    if width == 0 || height == 0 {
        return Ok(());
    }
    let samples = image.samples();
    let tiles_x = width.div_ceil(TILE);
    let tiles_y = height.div_ceil(TILE);

    for ty in 0..tiles_y {
        for tx in 0..tiles_x {
            let origin_x = tx * TILE;
            let origin_y = ty * TILE;
            let w = (width - origin_x).min(TILE);
            let h = (height - origin_y).min(TILE);
            let tile = store.get_mut(surface, TileId { x: tx, y: ty })?;
            let texels = tile.texels_mut();

            for ly in 0..h {
                let src_start =
                    ((origin_y + ly) as usize * width as usize + origin_x as usize) * CHANNELS;
                let src_end = src_start + (w as usize) * CHANNELS;
                let dst_start = (ly * TILE) as usize * CHANNELS;
                let dst_end = dst_start + (w as usize) * CHANNELS;
                if let (Some(src), Some(dst)) = (
                    samples.get(src_start..src_end),
                    texels.get_mut(dst_start..dst_end),
                ) {
                    dst.copy_from_slice(src);
                }
            }

            tile.mark_dirty(aurora_core::Rect {
                x: 0,
                y: 0,
                width: w,
                height: h,
            });
        }
    }
    Ok(())
}

/// Decodes `bytes` using whichever format `path`'s own extension names
/// (case-insensitive): `png`, `jpg`/`jpeg`, or `tif`/`tiff`.
///
/// # Errors
///
/// Returns [`IoError::UnsupportedExtension`] for anything else (or no
/// extension at all), or whatever the underlying decoder itself
/// returns.
pub fn decode_by_extension(path: &Path, bytes: &[u8]) -> Result<Image, IoError> {
    let extension = extension_of(path);
    match extension.as_str() {
        "png" => crate::png::decode(bytes),
        "jpg" | "jpeg" => crate::jpeg::decode(bytes),
        "tif" | "tiff" => crate::tiff::decode(bytes),
        _ => Err(IoError::UnsupportedExtension(extension)),
    }
}

/// Encodes `image` using whichever format `path`'s own extension names
/// (case-insensitive) — [`decode_by_extension`]'s own encode-side
/// counterpart, for a real "Save/Export" flow.
///
/// # Errors
///
/// Returns [`IoError::UnsupportedExtension`] for anything else (or no
/// extension at all), or whatever the underlying encoder itself
/// returns.
pub fn encode_by_extension(path: &Path, image: &Image) -> Result<Vec<u8>, IoError> {
    let extension = extension_of(path);
    match extension.as_str() {
        "png" => crate::png::encode(image),
        "jpg" | "jpeg" => crate::jpeg::encode(image),
        "tif" | "tiff" => crate::tiff::encode(image),
        _ => Err(IoError::UnsupportedExtension(extension)),
    }
}

/// `path`'s own extension, lower-cased, or an empty string if it has
/// none — shared by [`decode_by_extension`]/[`encode_by_extension`] so
/// both dispatch on exactly the same normalization.
fn extension_of(path: &Path) -> String {
    path.extension()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// Reads `width`x`height` samples back out of `store`'s `surface`,
/// starting at surface-local `(0, 0)` — [`write_into_store`]'s own
/// inverse, needed for a real "Save/Export" flow to turn a document
/// layer's live pixels back into an encodable [`Image`]. Tile by tile,
/// the same row-major layout on both sides as `write_into_store`, so
/// each overlapping row is a plain slice copy. The resulting `Image` is
/// always tagged `IccProfile::srgb()` — the same default every decoder
/// in this crate already assumes (none of them read an embedded ICC
/// profile yet either), not a new gap this function introduces.
///
/// A tile the store has never touched (fully outside whatever's been
/// painted) reads back as all-zero (transparent black) — a freshly
/// allocated [`aurora_tile::Tile`]'s own documented default — not an
/// error.
///
/// # Errors
///
/// Returns [`IoError::Tile`] if paging a touched tile in from the
/// scratch disk fails.
pub fn read_from_store(
    store: &mut TileStore,
    surface: SurfaceId,
    width: u32,
    height: u32,
) -> Result<Image, IoError> {
    let mut samples = vec![half::f16::from_f32(0.0); width as usize * height as usize * CHANNELS];
    if width > 0 && height > 0 {
        let tiles_x = width.div_ceil(TILE);
        let tiles_y = height.div_ceil(TILE);

        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                let origin_x = tx * TILE;
                let origin_y = ty * TILE;
                let w = (width - origin_x).min(TILE);
                let h = (height - origin_y).min(TILE);
                let tile = store.get(surface, TileId { x: tx, y: ty })?;
                let texels = tile.texels();

                for ly in 0..h {
                    let dst_start =
                        ((origin_y + ly) as usize * width as usize + origin_x as usize) * CHANNELS;
                    let dst_end = dst_start + (w as usize) * CHANNELS;
                    let src_start = (ly * TILE) as usize * CHANNELS;
                    let src_end = src_start + (w as usize) * CHANNELS;
                    if let (Some(dst), Some(src)) = (
                        samples.get_mut(dst_start..dst_end),
                        texels.get(src_start..src_end),
                    ) {
                        dst.copy_from_slice(src);
                    }
                }
            }
        }
    }
    Image::new(width, height, aurora_color::IccProfile::srgb(), samples)
}

#[cfg(test)]
mod tests {
    use super::{decode_by_extension, encode_by_extension, read_from_store, write_into_store};
    use crate::Image;
    use aurora_color::IccProfile;
    use aurora_tile::{SurfaceId, TILE, TileId, TileStore};
    use half::f16;
    use std::num::NonZeroUsize;
    use std::path::Path;

    fn store() -> (tempfile::TempDir, TileStore) {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("tempdir creation must succeed in a test environment: {err}"),
        };
        let Some(budget) = NonZeroUsize::new(16) else {
            unreachable!("16 is non-zero");
        };
        let store = match TileStore::new(dir.path().to_path_buf(), budget) {
            Ok(store) => store,
            Err(err) => unreachable!("scratch dir just created by tempfile must be usable: {err}"),
        };
        (dir, store)
    }

    fn surface() -> SurfaceId {
        SurfaceId::from_raw(0)
    }

    /// A solid-colour image `width`x`height`, every texel the same
    /// straight RGBA value.
    fn solid_image(width: u32, height: u32, rgba: [f32; 4]) -> Image {
        let samples: Vec<f16> = (0..width as usize * height as usize)
            .flat_map(|_| rgba.map(f16::from_f32))
            .collect();
        match Image::new(width, height, IccProfile::srgb(), samples) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn write_into_store_of_an_empty_image_touches_nothing() {
        let (_dir, mut store) = store();
        let image = match Image::new(0, 0, IccProfile::srgb(), Vec::new()) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = write_into_store(&image, &mut store, surface()) {
            unreachable!("{err:?}");
        }
        assert_eq!(store.resident_len(), 0);
    }

    #[test]
    // Every asserted value here is a straight copy of an `f16` an exact
    // literal (`0.0`/`1.0`) built, or a texel `write_into_store` never
    // touched at all (still its `Tile::new` default of exactly `0.0`) --
    // not a computed/accumulated value, so exact equality is correct.
    #[allow(clippy::float_cmp)]
    fn write_into_store_smaller_than_one_tile_lands_at_the_origin() {
        let (_dir, mut store) = store();
        let image = solid_image(10, 10, [0.0, 1.0, 0.0, 1.0]);
        if let Err(err) = write_into_store(&image, &mut store, surface()) {
            unreachable!("{err:?}");
        }
        let tile = match store.get(surface(), TileId { x: 0, y: 0 }) {
            Ok(tile) => tile,
            Err(err) => unreachable!("{err:?}"),
        };
        let index = (5 * TILE + 5) as usize * aurora_tile::CHANNELS;
        let Some(&g) = tile.texels().get(index + 1) else {
            unreachable!("index is in bounds for a full tile");
        };
        assert_eq!(g.to_f32(), 1.0, "a pixel within the image must be green");
        let outside_index = (200 * TILE + 200) as usize * aurora_tile::CHANNELS;
        let Some(&a) = tile.texels().get(outside_index + 3) else {
            unreachable!("index is in bounds for a full tile");
        };
        assert_eq!(
            a.to_f32(),
            0.0,
            "a pixel outside the 10x10 image must be untouched"
        );
    }

    #[test]
    // Same reasoning as `write_into_store_smaller_than_one_tile_lands_at_the_origin`.
    #[allow(clippy::float_cmp)]
    fn write_into_store_spanning_multiple_tiles_touches_exactly_the_overlapping_ones() {
        let (_dir, mut store) = store();
        // TILE is 256; a 300x300 image overlaps a 2x2 tile grid.
        let image = solid_image(300, 300, [1.0, 0.0, 0.0, 1.0]);
        if let Err(err) = write_into_store(&image, &mut store, surface()) {
            unreachable!("{err:?}");
        }
        assert_eq!(store.resident_len(), 4);

        // The bottom-right tile (256..300 in both axes) is only
        // partially covered -- a texel just past the image's own extent
        // within that same tile must stay untouched.
        let tile = match store.get(surface(), TileId { x: 1, y: 1 }) {
            Ok(tile) => tile,
            Err(err) => unreachable!("{err:?}"),
        };
        let covered_index = (10 * TILE + 10) as usize * aurora_tile::CHANNELS; // (266, 266)
        let Some(&r) = tile.texels().get(covered_index) else {
            unreachable!("index is in bounds for a full tile");
        };
        assert_eq!(r.to_f32(), 1.0, "within the image's own 300x300 extent");

        let uncovered_index = (100 * TILE + 100) as usize * aurora_tile::CHANNELS; // (356, 356)
        let Some(&a) = tile.texels().get(uncovered_index + 3) else {
            unreachable!("index is in bounds for a full tile");
        };
        assert_eq!(a.to_f32(), 0.0, "past the image's own 300x300 extent");
    }

    #[test]
    fn decode_by_extension_round_trips_a_real_png() {
        let image = solid_image(4, 4, [0.0, 0.0, 1.0, 1.0]);
        let bytes = match crate::png::encode(&image) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        let decoded = match decode_by_extension(Path::new("photo.PNG"), &bytes) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(decoded.width(), 4);
        assert_eq!(decoded.height(), 4);
    }

    #[test]
    fn decode_by_extension_round_trips_a_real_tiff_case_insensitively() {
        let image = solid_image(4, 4, [0.0, 0.0, 1.0, 1.0]);
        let bytes = match crate::tiff::encode(&image) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        for name in ["scan.tif", "scan.TIFF"] {
            let decoded = match decode_by_extension(Path::new(name), &bytes) {
                Ok(image) => image,
                Err(err) => unreachable!("{name}: {err:?}"),
            };
            assert_eq!(decoded.width(), 4);
            assert_eq!(decoded.height(), 4);
        }
    }

    #[test]
    fn decode_by_extension_rejects_an_unsupported_extension() {
        match decode_by_extension(Path::new("document.psd"), &[]) {
            Err(super::IoError::UnsupportedExtension(ext)) => assert_eq!(ext, "psd"),
            other => unreachable!("expected UnsupportedExtension, got {other:?}"),
        }
    }

    #[test]
    fn decode_by_extension_rejects_a_path_with_no_extension_at_all() {
        match decode_by_extension(Path::new("noextension"), &[]) {
            Err(super::IoError::UnsupportedExtension(ext)) => assert_eq!(ext, ""),
            other => unreachable!("expected UnsupportedExtension, got {other:?}"),
        }
    }

    #[test]
    fn encode_by_extension_round_trips_a_real_jpeg() {
        let image = solid_image(4, 4, [1.0, 0.0, 0.0, 1.0]);
        let bytes = match encode_by_extension(Path::new("photo.jpg"), &image) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        let decoded = match crate::jpeg::decode(&bytes) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(decoded.width(), 4);
        assert_eq!(decoded.height(), 4);
    }

    #[test]
    fn encode_by_extension_rejects_an_unsupported_extension() {
        let image = solid_image(1, 1, [0.0, 0.0, 0.0, 1.0]);
        match encode_by_extension(Path::new("document.psd"), &image) {
            Err(super::IoError::UnsupportedExtension(ext)) => assert_eq!(ext, "psd"),
            other => unreachable!("expected UnsupportedExtension, got {other:?}"),
        }
    }

    #[test]
    // A freshly allocated tile's own exact `0.0` default, never computed
    // -- exact equality is correct here, same reasoning as the
    // `write_into_store` tests above.
    #[allow(clippy::float_cmp)]
    fn read_from_store_of_an_untouched_surface_is_all_zero() {
        let (_dir, mut store) = store();
        let image = match read_from_store(&mut store, surface(), 4, 4) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(image.width(), 4);
        assert_eq!(image.height(), 4);
        assert!(image.samples().iter().all(|s| s.to_f32() == 0.0));
    }

    #[test]
    fn read_from_store_round_trips_write_into_store_within_one_tile() {
        let (_dir, mut store) = store();
        let written = solid_image(10, 10, [0.0, 1.0, 0.0, 1.0]);
        if let Err(err) = write_into_store(&written, &mut store, surface()) {
            unreachable!("{err:?}");
        }
        let read_back = match read_from_store(&mut store, surface(), 10, 10) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(read_back.width(), 10);
        assert_eq!(read_back.height(), 10);
        assert_eq!(read_back.samples(), written.samples());
    }

    #[test]
    fn read_from_store_round_trips_write_into_store_across_multiple_tiles() {
        let (_dir, mut store) = store();
        // TILE is 256; 300x300 spans a 2x2 tile grid, exercising the
        // same partial-edge-tile logic `write_into_store`'s own
        // multi-tile test does.
        let written = solid_image(300, 300, [1.0, 0.0, 1.0, 1.0]);
        if let Err(err) = write_into_store(&written, &mut store, surface()) {
            unreachable!("{err:?}");
        }
        let read_back = match read_from_store(&mut store, surface(), 300, 300) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(read_back.samples(), written.samples());
    }
}
