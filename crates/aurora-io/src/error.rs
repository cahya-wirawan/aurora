//! Errors from format import/export.

use thiserror::Error;

/// `#[non_exhaustive]`: more variants land as this crate grows past
/// PNG/JPEG into other formats.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum IoError {
    #[error("failed to decode PNG: {0}")]
    PngDecode(#[from] png::DecodingError),
    #[error("failed to encode PNG: {0}")]
    PngEncode(#[from] png::EncodingError),
    /// The PNG decoder's own `EXPAND`/`ALPHA` transformations were
    /// requested (see `png` module doc comment) but produced something
    /// other than RGBA — not expected for any real PNG this crate has
    /// seen, but a real, checked condition rather than an assumption:
    /// misreading a different channel layout as RGBA would silently
    /// scramble colour channels, not just fail loudly.
    #[error("decoded PNG has an unexpected colour layout: {0:?}")]
    UnexpectedColorType(png::ColorType),
    #[error("failed to decode JPEG: {0}")]
    JpegDecode(#[from] zune_jpeg::errors::DecodeErrors),
    #[error("failed to encode JPEG: {0}")]
    JpegEncode(#[from] jpeg_encoder::EncodingError),
    /// Requesting RGBA output from the JPEG decoder (see `jpeg` module
    /// doc comment) produced something else — the decoder's own docs
    /// warn it "does not guarantee... can convert to all colorspaces,"
    /// so this is a real, checked condition, not an assumption.
    #[error("decoded JPEG has an unexpected colour layout: {0:?}")]
    UnexpectedJpegColorSpace(zune_jpeg::zune_core::colorspace::ColorSpace),
    /// [`crate::jpeg::encode`] was given an [`crate::Image`] wider or
    /// taller than JPEG's own SOF marker can represent — a real,
    /// permanent format limit (16-bit dimension fields), not a library
    /// shortcoming.
    #[error("image is {width}x{height}, which exceeds JPEG's own 65535x65535 dimension limit")]
    JpegDimensionsTooLarge { width: u32, height: u32 },
    #[error("failed to decode TIFF: {0}")]
    TiffDecode(#[from] tiff::TiffError),
    #[error("failed to encode TIFF: {0}")]
    TiffEncode(tiff::TiffError),
    /// The decoded TIFF's own pixel layout (`tiff::ColorType`) is one
    /// this crate doesn't handle yet — see `tiff` module doc comment
    /// for exactly which layouts are covered (real, checked scope, not
    /// every TIFF variant that exists).
    #[error("decoded TIFF has an unsupported colour layout: {0:?}")]
    UnsupportedTiffColorType(tiff::ColorType),
    /// The decoded TIFF's own sample format (`tiff::decoder::DecodingResult`'s
    /// variant — e.g. 32-bit float) isn't one of the two this crate
    /// promotes from (8-bit or 16-bit unsigned integer samples).
    #[error("decoded TIFF uses an unsupported sample format: {0}")]
    UnsupportedTiffSampleFormat(&'static str),
    /// [`crate::Image::new`] was given a sample buffer whose length
    /// doesn't match `width * height * 4`.
    #[error("image is {width}x{height} (expects {expected} samples) but got {actual} samples")]
    SampleCountMismatch {
        width: u32,
        height: u32,
        expected: usize,
        actual: usize,
    },
    /// A [`aurora_tile::TileError`] surfaced through this crate — most
    /// often paging a tile in from the scratch disk, from either
    /// direction: [`crate::import::write_into_store`] hits it while
    /// *writing* an imported image into the store, and
    /// [`crate::aur::read`] hits it while *reading* a `.aur` file's own
    /// tile entries back (a corrupt entry is rejected by
    /// `aurora_tile::codec::decode` and arrives here). The message is
    /// therefore deliberately direction-neutral: it used to say "failed
    /// to write image into the tile store" and was shown to a user whose
    /// *open* had failed, which is a misleading thing to tell someone
    /// about their own file.
    #[error("tile data error: {0}")]
    Tile(#[from] aurora_tile::TileError),
    /// [`crate::import::decode_by_extension`] was given a path whose
    /// extension names a format this crate doesn't decode (or has no
    /// extension at all) — real, checked scope, not every image format
    /// that exists.
    #[error("unsupported file extension: {0:?}")]
    UnsupportedExtension(String),
    /// A `.aur` file's own ZIP container ([`crate::aur`], ADR 0009)
    /// failed to read or write.
    #[error("failed to read/write the .aur container: {0}")]
    Zip(#[from] zip::result::ZipError),
    /// Raw I/O within a `.aur` container entry (as opposed to
    /// [`IoError::Zip`], which is the ZIP format itself) — e.g. reading
    /// a tile entry's own bytes.
    #[error("I/O error reading/writing a .aur entry: {0}")]
    Io(#[from] std::io::Error),
    /// A `.aur` file's `LayerTree` mutation ([`crate::aur::read`]
    /// rebuilding an active layer's own bounds, or similar) failed.
    #[error("failed to rebuild the document from a .aur file: {0}")]
    Doc(#[from] aurora_doc::DocError),
    /// [`crate::aur::write`] failed to serialize an embedded ICC
    /// profile's own bytes, or [`crate::aur::read`] failed to parse one
    /// back out of the manifest.
    #[error("failed to read/write a .aur file's embedded ICC profile: {0}")]
    Color(#[from] aurora_color::ColorError),
    /// [`crate::aur::write`] failed to serialize the manifest entry.
    #[error("failed to serialize the .aur manifest: {0}")]
    ManifestSerialization(String),
    /// [`crate::aur::read`] failed to deserialize the manifest entry —
    /// corrupted, truncated, or from an incompatible future schema
    /// version (see [`crate::aur`]'s own doc comment for the
    /// forward-compatibility policy ADR 0009 sets).
    #[error("failed to deserialize the .aur manifest: {0}")]
    ManifestDeserialization(String),
    /// [`crate::aur::read`] found a required entry (the manifest or the
    /// history) missing from the ZIP container — not a valid `.aur`
    /// file, or one truncated/corrupted past recovery.
    #[error(".aur file is missing its own required {0:?} entry")]
    MissingEntry(&'static str),
    /// [`crate::aur::read`] found a ZIP entry that declares (or really
    /// does decompress to) more bytes than that entry could legitimately
    /// hold — a `.aur` file's own tile entries have a known, fixed
    /// maximum size, and its manifest/history entries a generous but
    /// finite one. Rejected rather than read, so a hostile or corrupt
    /// container can't turn a few compressed kilobytes into gigabytes of
    /// resident memory (the classic DEFLATE zip-bomb shape) on a path
    /// that runs before the application even has a window.
    #[error(".aur entry {name:?} holds {size} bytes, past this reader's own {cap}-byte cap")]
    EntryTooLarge { name: String, size: u64, cap: u64 },
    /// [`crate::aur::read`] found a manifest declaring a pixel layer
    /// larger than [`aurora_core::MAX_DOCUMENT_EXTENT`], the documented
    /// document ceiling (PRD §7.3.1 / ADR 0002). The tile-scan loop
    /// derives its own iteration count straight from those bounds, so an
    /// unchecked `u32::MAX` there is not a large read but an effectively
    /// unbounded one — rejected up front instead.
    #[error(".aur manifest declares a {width}x{height} layer, past the {max}px document ceiling")]
    LayerBoundsTooLarge { width: u32, height: u32, max: u32 },
    /// [`crate::aur::read`] found a manifest declaring a canvas larger
    /// than [`aurora_core::MAX_DOCUMENT_EXTENT`] — the same document
    /// ceiling [`IoError::LayerBoundsTooLarge`] enforces for a layer's
    /// own bounds, applied to the document-level canvas size the
    /// manifest carries alongside them. `aurora-app` puts that value
    /// straight into its own live canvas size and later allocates
    /// `width * height * 4` samples from it, so an unchecked
    /// `u32::MAX` there is an allocation no machine can serve (and, on a
    /// 32-bit target, an arithmetic overflow first).
    #[error(".aur manifest declares a {width}x{height} canvas, past the {max}px document ceiling")]
    CanvasTooLarge { width: u32, height: u32, max: u32 },
    /// [`crate::aur::read`] found a manifest whose pixel layers add up
    /// to more tiles than any real document has. Each layer's own bounds
    /// are separately checked against the document ceiling
    /// ([`IoError::LayerBoundsTooLarge`]), but *layer count* has no
    /// ceiling of its own — this project promises unlimited layers — so
    /// without this check a manifest could declare many ceiling-sized
    /// layers, each individually legal, and multiply the tile scan out
    /// to an effectively unbounded loop from a file only kilobytes long.
    #[error(
        ".aur manifest's layers add up to {total} tiles, past this reader's own {max}-tile budget"
    )]
    TooManyTiles { total: u64, max: u64 },
    /// A flat, composited export (`aurora-app`'s own `composite_document`)
    /// could not read every tile it needed out of the tile store, so the
    /// image it assembled is missing content — the skipped layers
    /// contributed nothing at all rather than their real pixels.
    ///
    /// This is refused rather than written. The live canvas deliberately
    /// degrades instead: one unreadable tile logs a warning and paints
    /// what it can, because hard-failing every repaint over one corrupt
    /// scratch file would be far worse to use. A *file* is different —
    /// CLAUDE.md's own rule is that silently degrading a professional's
    /// file is the worst failure this project can have — so the export
    /// path turns the same condition into a real `Err` and writes
    /// nothing.
    ///
    /// `first` is the message of the first underlying
    /// [`aurora_tile::TileError`] seen; `skipped` counts every skip
    /// across the whole export, which is why the error carries a count
    /// rather than the error value itself.
    #[error(
        "refusing to export a document with missing content: {skipped} layer tile read(s) failed \
         (first: {first})"
    )]
    IncompleteComposite { skipped: usize, first: String },
}
