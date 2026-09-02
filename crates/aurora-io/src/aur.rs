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
//! - `skipped-tiles` (**optional**, since 0.74.0): the tiles a
//!   [`write_best_effort`] writer could not read and therefore left
//!   out, `postcard`-encoded as a `Vec<SkippedTileRecord>`. Written
//!   only when that list is non-empty, so an ordinary [`write()`] —
//!   every user-facing save — still produces byte-identical output to
//!   what it produced before this entry existed. Absent means "nothing
//!   was dropped", which is exactly what every file written before
//!   0.74.0 means too.
//! - One entry per non-blank persisted tile — a pixel layer's own
//!   content **and, since 0.71.0, a layer mask's coverage** — named by
//!   `tile_entry_name` from its own `(SurfaceId, TileId)` pair, holding
//!   `aurora_tile::codec::encode`'s own output **verbatim** — stored,
//!   not deflated, since that output is already `lz4_flex`-compressed
//!   and compressing compressed bytes again wastes CPU for no size
//!   benefit.
//!
//! **Scope, stated honestly.** Two kinds of surface are persisted, and
//! `persisted_surfaces` is the single place that decides so: a pixel
//! layer's own content over its `bounds` extent, and a layer's mask
//! coverage over the *mask's* own `aurora_doc::LayerMask::bounds`
//! extent — a different rectangle, at a possibly different origin, on a
//! different `aurora_tile::SurfaceId`
//! (`aurora_doc::LayerTree::mask_surface_id`). A masked layer's tile
//! range is therefore walked **twice**, once per surface; that is the
//! real cost of the extension, and both the whole-document tile budget
//! (`MAX_TOTAL_TILES_PER_DOCUMENT`) and the wall clock pay it. Group
//! masks are included too: a group has no content surface but certainly
//! can carry a mask.
//!
//! Neither kind is clamped to the document's own extent — the same "no
//! document-extent clamp" limitation `aurora_brush::stamp_dab`'s own
//! doc comment already names (nothing in this pipeline clips painting
//! to a layer's own bounds, so pixels painted past them were never on
//! documented, reliable ground to begin with). A fully blank (all-zero)
//! tile is skipped when writing, as is any tile the store has never
//! held at all — most of a freshly created layer's own tile range is
//! never actually painted, and *no* mask surface is painted by anything
//! in the app today, so writing every one of those out would be real,
//! avoidable file bloat; a missing tile entry on read simply leaves
//! that tile at the store's own default, which for a mask means
//! coverage `1.0`/fully visible (`aurora_doc::mask`'s
//! alpha-as-presence-flag convention) rather than a hidden layer. That
//! is also what makes every `.aur` file written before 0.71.0 keep
//! opening unchanged: no mask entries, so every mask reads back fully
//! visible, exactly as it composited then.
//!
//! **The accepted cost of not bumping `MANIFEST_VERSION`, stated
//! plainly.** Compatibility runs backward but not forward. A build
//! older than 0.71.0 opens a 0.71.0-saved file happily — it simply
//! never probes for mask entries — and if the user then re-saves from
//! that build, **every painted mask pixel in the document is silently
//! gone**, because that build's own surface walk yields only pixel
//! layers. Nothing in the file makes that loud: the manifest carries no
//! signal an older reader could notice, which is precisely what makes
//! the no-bump choice work in the first place. The alternative was a
//! `MANIFEST_VERSION` bump, which this reader answers with a hard
//! refusal ("unsupported manifest version"), so it would make every
//! `.aur` file and every autosave that already exists unopenable — a
//! certain, universal loss traded against a conditional one that
//! requires a user to run two different builds against one file.
//! Deliberate, and recorded in PLAN.md beside the same decision. The
//! honest mitigation, if downgrade ever becomes a real scenario, is a
//! *capability* signal an old reader can be taught to warn on, not a
//! version bump; nothing needs one today, because 0.71.0 is unreleased
//! and no build in a user's hands has ever written a mask entry.
//!
//! **Nothing in the app paints mask coverage yet** — the mask brush/tool
//! UI is still a named follow-on in `aurora_doc::mask` — so this half of
//! the format is exercised by tests only, not end to end through the
//! running editor. It is persisted anyway rather than deferred, because
//! the alternative is a format that silently drops content the moment
//! that tool lands.
//!
//! Persistence is **not** gated on `aurora_doc::LayerMask::enabled`:
//! that flag is a UI toggle, and skipping disabled masks would mean
//! switching one off and saving destroys the painted pixels
//! irrecoverably.
//!
//! **Reading is hardened against a hostile or corrupt container**
//! (2026-08-24). [`read`] runs on `aurora-app`'s own pre-window startup
//! path (crash-recovery autosave) and on its ordinary "open the `.aur`
//! file a user was sent" path, so neither an unfinishable loop nor an
//! unbounded allocation is acceptable here: the manifest's declared
//! layer *extent* is checked against `aurora_core::MAX_DOCUMENT_EXTENT`
//! before any tile grid is derived from it (`tile_grid`), and every
//! entry is read through a per-entry size cap (`read_capped`) rather
//! than a bare `read_to_end`. Both reject with an [`IoError`]; neither
//! panics.
//!
//! That said "layer bounds" until 2026-08-29, which overclaimed: only
//! the extent was checked, and a `Rect`'s own *origin* went through
//! untouched. `tile_grid` now checks that too, against
//! `aurora_core::MAX_DOCUMENT_ORIGIN`
//! ([`IoError::LayerOriginOutOfRange`]) — a different class of defect
//! from the extent one, since an out-of-range origin does not enlarge
//! any loop here but does propagate into arithmetic downstream:
//! `aurora-app`'s own `read_layer_window` subtracts a layer origin from
//! a document origin in `i64` with no check of its own. A negative
//! origin stays legal; a layer may sit off the canvas edge.
//!
//! **A layer's `bounds` is not the only `Rect` a manifest carries**
//! (0.57.13). A `LayerMask` has one of its own, and `tile_grid` could
//! not see it then — it was only ever called on a `LayerKind::Pixel`
//! arm's own `bounds`, so a mask's rectangle went unchecked, and a
//! *group*, which can carry a mask but has no `bounds`, never reached
//! `tile_grid` at all. `validate_persisted_rects` closes both, from [`read`]
//! and from the shared body behind [`write()`] and
//! [`write_best_effort`], reusing [`IoError::LayerOriginOutOfRange`]
//! rather than adding a variant.
//!
//! That walk checked only a mask's *origin* until 0.71.0, and said so:
//! no loop here was derived from a mask's extent, so an oversized one
//! had nothing to make unfinishable. Persisting mask coverage changed
//! exactly that — a mask rectangle now drives a real tile grid in both
//! directions, and `aurora_doc::LayerTree::add_mask` bounds a mask's
//! origin but never its extent — so `validate_persisted_rects` now runs the
//! whole of `tile_grid` over every mask, extent
//! ([`IoError::LayerBoundsTooLarge`]) included. Without it a crafted
//! manifest declaring a `u32::MAX`-wide mask would hang
//! `aurora-app`'s own pre-window startup path, which is the same defect
//! the layer-bounds check already closed.
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
//! **The way *out* was checked less than the way in, until 0.71.1.**
//! Every check above was reached from [`read`]; the writer shared only
//! the mask half. Two consequences, both reachable through ordinary
//! `aurora-doc` API calls rather than a crafted file, and both closed
//! by hoisting the whole pre-flight into `validate_persisted_rects`:
//! the writer had **no whole-document tile budget at all**, so two
//! ordinary layers each carrying an ordinary full-canvas mask wrote a
//! valid container that this module's own [`read`] then refused with
//! [`IoError::TooManyTiles`] — a file a user saved and can never
//! reopen; and an oversized *layer* extent (`aurora-doc` bounds a
//! layer's origin, not its extent) failed from inside the tile loop,
//! after the mimetype, manifest and history entries were already
//! written, leaving a well-formed 3-entry partial container at the
//! destination. Both now refuse before the first byte.
//!
//! **Why `skipped-tiles` is a separate ZIP entry and not a manifest
//! field** (0.74.0). It looks like a manifest field — it is per-file
//! metadata, and the manifest is where per-file metadata lives — but it
//! cannot be one without breaking every `.aur` file and every autosave
//! that already exists. Two independent reasons, both verified against
//! this workspace's own pinned `postcard` rather than recalled:
//!
//! 1. **`postcard`'s wire format is positional.** Field names and tags
//!    are not on the wire at all, so a struct's fields are decoded
//!    strictly in declaration order from a bare byte stream. Adding a
//!    trailing field to `ManifestRead` means the decoder runs off the
//!    end of an old container's manifest bytes and fails with
//!    `DeserializeUnexpectedEnd` — measured directly, with an old-shaped
//!    two-field struct encoded and a new-shaped three-field struct
//!    decoded from those same bytes. `#[serde(default)]` does **not**
//!    rescue it: `default` applies when a self-describing format omits a
//!    *named* field, and `postcard` has no names to omit, so the hard
//!    decode error arrives before serde ever consults the attribute.
//! 2. **A `MANIFEST_VERSION` bump is not the escape hatch either.**
//!    [`read`] answers any version it does not recognise with a hard
//!    refusal, so bumping it would make every existing `.aur` file and
//!    every existing crash-recovery autosave permanently unopenable —
//!    the exact "certain, universal loss" the mask-persistence decision
//!    above already rejected for the same reason.
//!
//! A separate, optional, top-level entry has neither problem, and it is
//! how this format already evolves additively: tile entries are probed
//! by name and a `ZipError::FileNotFound` simply means "not present"
//! (see `read_persisted_tiles`). A reader that predates the entry never
//! looks it up; a reader that knows about it treats absence as an empty
//! list. Recorded here so nobody has to re-derive the `postcard`
//! investigation.
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

/// The optional entry naming the tiles a [`write_best_effort`] writer
/// left out — see this module's own doc comment for why it is an entry
/// of its own rather than a manifest field.
const SKIPPED_TILES_ENTRY: &str = "skipped-tiles";

/// The most [`SkippedTileRecord`]s either side of this format will put
/// in, or take out of, the [`SKIPPED_TILES_ENTRY`] entry.
///
/// Real skips are rare and correlated (a broken scratch file loses the
/// handful of tiles that lived in it), so this is far above anything a
/// genuine best-effort write produces. It exists because the read side
/// parses a file that may be crafted or corrupt: the entry is already
/// bounded in *bytes* by [`MAX_METADATA_ENTRY_BYTES`], but 64 MiB of
/// `postcard`-encoded records is still millions of heap-allocated
/// `String`s, and the whole point of this list is to be summarized in
/// one dialog line. Truncating is the honest answer — the count the
/// user is shown is then a floor, not a fiction.
const MAX_SKIPPED_TILE_RECORDS: usize = 4096;

/// The most characters of one skip's own `reason` this format stores.
/// The reason is an [`aurora_tile::TileError`]'s own message, which is
/// well under this in practice; the bound is here so a pathological
/// error string cannot make the entry itself large.
///
/// **Characters, not bytes, deliberately.** Truncating a `String` by
/// byte offset splits UTF-8 sequences (and would need slicing, which
/// this workspace denies), so the cut is made with `chars().take(..)`
/// and this bound counts what that counts. It still bounds the byte
/// length — at four bytes per `char`, 2 KiB — which is all the storage
/// bound needs.
const MAX_SKIPPED_REASON_CHARS: usize = 512;

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

/// The most tiles a tile scan will visit across *all* of a document's
/// persisted surfaces put together — every pixel layer's own grid *and*
/// every mask's, since 0.71.0 charges both against this one budget
/// ([`persisted_surfaces`]).
///
/// **Charged on the way out as well as on the way in** (0.71.1).
/// Until then only [`read`] carried this budget, and the consequence
/// was reachable without a crafted file at all: two ordinary layers,
/// each given an ordinary full-canvas mask through
/// `aurora_doc::LayerTree::add_mask`, wrote a perfectly valid container
/// that then failed its own reader with [`IoError::TooManyTiles`] — an
/// unopenable file produced by entirely ordinary API use, which is the
/// "silently degrading a professional's file" failure CLAUDE.md names
/// as the worst this project can have. [`validate_persisted_rects`] now
/// runs the same sum before the first byte is written.
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
/// **What this is, stated plainly.** It is a *flat, document-wide
/// total*, sized so that the largest single document PRD §7.3.1 says
/// can exist — one ceiling-sized layer carrying a full-canvas mask —
/// still loads untouched. It is expressed in terms of the ceiling and
/// the tile size rather than a bare literal, so it follows either if
/// they ever change.
///
/// The `2 *` arrived with mask persistence (0.71.0), because a mask's
/// grid is now walked alongside its layer's and a single *legal*
/// ceiling-sized masked layer would otherwise be refused by a check
/// meant for crafted files. **It is not mask-scoped, and does not
/// pretend to be** (corrected 0.71.1, which found the previous wording
/// here claiming exactly that): the check is `total > MAX` over the sum
/// of every persisted grid, so any combination reaching the same total
/// is admitted equally — two ceiling-sized layers carrying *no* masks
/// at all, one ceiling-sized layer plus a second layer's full-canvas
/// mask, and so on. That is an accepted, document-wide loosening: the
/// budget exists to make an unfinishable scan impossible, not to
/// enforce a layer count, and no arrangement under it can cost more
/// than the legal one-masked-ceiling-layer case it is sized for.
///
/// **The real cost is not uniform across that total, and this budget
/// does not distinguish.** A grid position with no matching ZIP entry
/// costs a `format!` and a central-directory miss; one with an entry
/// costs a capped read, an `lz4` decode, and a tile made resident —
/// measured at roughly two orders of magnitude more, with an
/// archive-to-scratch-disk amplification on top. A capped *second*
/// budget on materialized tiles was considered in 0.71.1 and
/// deliberately not added: it would bound the worst case tighter, but
/// it also caps how much real content a legitimate document may hold,
/// which is a product decision (PRD §6 promises unlimited layers) and
/// not one to make silently inside a hardening pass. Disclosed here so
/// the tradeoff is visible rather than assumed away — a file that fills
/// the whole budget with real tiles is a large, slow read, not a
/// refused one.
const MAX_TOTAL_TILES_PER_DOCUMENT: u64 = {
    let side = (aurora_core::MAX_DOCUMENT_EXTENT as u64).div_ceil(TILE as u64);
    2 * side * side
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

/// How many tiles wide and tall `bounds` is, after checking *both*
/// halves of that rectangle against the document ceiling (PRD §7.3.1):
/// its origin against [`aurora_core::MAX_DOCUMENT_ORIGIN`], and its
/// extent against [`aurora_core::MAX_DOCUMENT_EXTENT`] (the same one
/// `aurora_core::Size::new` already enforces).
///
/// Both are real safety checks, not tidiness ones, and they guard
/// different things.
///
/// **Extent.** [`read`] derives this grid from a manifest it has just
/// parsed out of an untrusted file, then loops `tiles_y * tiles_x`
/// times — so an unchecked `u32::MAX` extent there is not a big loop
/// but an unfinishable one (~2.8e14 iterations), reached from
/// `aurora-app`'s own pre-window startup recovery *and* from opening
/// any `.aur` file a user was sent.
///
/// **Origin.** An out-of-range `x`/`y` costs nothing here — the grid
/// size does not depend on it — but it propagates: `aurora-app`'s own
/// `read_layer_window` subtracts a layer origin from a document origin
/// in `i64` with no check of its own, and
/// `aurora_core::Rect::right`/`bottom` add the extent to it. Refusing
/// it at the file boundary is what keeps a crafted origin from ever
/// reaching that arithmetic. A *negative* origin stays accepted: a
/// layer may legitimately sit off the canvas edge.
///
/// The origin is checked first, because an origin defect corrupts
/// downstream arithmetic while an oversized extent only makes a loop
/// too long.
///
/// Placing both here rather than in [`read`] is deliberate: [`read`],
/// [`write()`] and [`write_best_effort`] all derive their tile grids
/// through this one function, so one check covers every path in and
/// out of the format.
///
/// **`bounds` is no longer always a layer's own** (0.71.0). Now that
/// mask coverage is persisted, [`persisted_surfaces`] yields a *mask*'s
/// own `aurora_doc::LayerMask` rectangle here too — including a
/// group's, which has no `bounds` of its own — so this function has
/// become the single check for every rectangle that drives a tile loop
/// in this module. Since 0.71.1 every one of those rectangles reaches
/// this function from [`validate_persisted_rects`] *up front* — before
/// a single byte is written or a single grid position is probed — and
/// then again, per surface, purely for the grid dimensions the loops
/// need.
///
/// Clamping to the ceilings the format already documents bounds the
/// worst case without newly restricting any legitimate document.
fn tile_grid(bounds: aurora_core::Rect) -> Result<(u32, u32), IoError> {
    if !bounds.origin_in_document_range() {
        return Err(IoError::LayerOriginOutOfRange {
            x: bounds.x,
            y: bounds.y,
            max: aurora_core::MAX_DOCUMENT_ORIGIN,
        });
    }
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
/// tile currently in `store` on a surface this format persists — every
/// pixel layer's own content *and* every layer mask's coverage, enabled
/// or not (`persisted_surfaces`). `writer` is generic over
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
/// fails, [`IoError::Color`] if `profile.to_bytes()` fails,
/// [`IoError::LayerBoundsTooLarge`]/[`IoError::LayerOriginOutOfRange`]
/// if some layer's own bounds — **or some layer's mask rectangle** —
/// are past the document ceiling in extent or origin,
/// [`IoError::TooManyTiles`] if its layers and their masks together add
/// up to more tiles than any real document has
/// (`MAX_TOTAL_TILES_PER_DOCUMENT`), or [`IoError::Tile`] if paging a
/// touched tile in from the scratch disk fails.
///
/// All three rectangle/budget refusals come from the one hoisted
/// `validate_persisted_rects` pre-flight shared with [`read`], so the
/// same checks apply on the way out as on the way in and **a refusal
/// for any of them leaves zero bytes at `writer`** rather than a
/// partial container. The budget half is what stops this function from
/// producing a file its own reader would refuse (0.71.1); the extent
/// half is what stops an oversized mask or layer from turning the tile
/// loop into an unfinishable one.
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

/// One [`SkippedTile`] as it is **stored in the container**, so that a
/// tile dropped by [`write_best_effort`] is still distinguishable from
/// a genuinely blank one after a restart.
///
/// Until 0.74.0 a skip was visible only within the session that made it
/// (a `tracing::warn!` line, and `aurora-app`'s own `.partial` autosave
/// filename). The container itself said nothing: a dropped tile and a
/// never-painted one both come back as "no entry for this tile", which
/// is the same silence. This record is what closes that gap — see
/// the `skipped-tiles` entry, and this module's own doc comment.
///
/// **`surface` is a raw `u64`, not a `SurfaceId`, on purpose.** This is
/// a persisted format that has to keep decoding forever, and encoding a
/// field as another crate's type would mean inheriting that type's
/// serde shape as part of this format's wire contract — a change there
/// would silently become a change here. `SurfaceId::to_raw` /
/// `SurfaceId::from_raw` is the conversion, and it is this module's own
/// to make.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SkippedTileRecord {
    /// The skipped tile's own surface, as `SurfaceId::to_raw`.
    pub surface: u64,
    pub tile: TileId,
    /// The underlying `aurora_tile::TileError`'s own message, truncated
    /// to `MAX_SKIPPED_REASON_CHARS` (512) characters.
    ///
    /// **Untrusted on the way back in.** On read this is bytes out of a
    /// file, so anything that renders it — `aurora-app`'s own
    /// "missing content" dialog is the one caller today — must put it
    /// through `aurora_doc::sanitize_display_name` first, exactly as
    /// the layer names out of the same file already are.
    pub reason: String,
}

impl From<&SkippedTile> for SkippedTileRecord {
    fn from(skipped: &SkippedTile) -> Self {
        Self {
            surface: skipped.surface.to_raw(),
            tile: skipped.tile,
            // `chars().take(..)`, never a byte slice: a byte cut can
            // land mid-sequence, and `indexing_slicing` is denied
            // workspace-wide anyway.
            reason: skipped
                .reason
                .chars()
                .take(MAX_SKIPPED_REASON_CHARS)
                .collect(),
        }
    }
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
/// manifest, a layer whose own bounds — or whose mask's bounds —
/// exceed the document ceiling in extent or origin, or a tree whose
/// grids together exceed `MAX_TOTAL_TILES_PER_DOCUMENT` still fail
/// the write outright: those say the *tree* is broken, not that one
/// piece of input is unreadable. Disclosed rather than silently
/// assumed, since it means this writer can refuse a whole autosave over
/// a rectangle, and **that is reachable through ordinary
/// `aurora-doc` API calls, not only from a crafted file** — an earlier
/// version of this paragraph claimed otherwise. `add_mask` bounds a
/// mask's extent as of 0.71.3, precisely so one oversized mask can no
/// longer disable a session's autosave; `add_pixel_layer` still bounds
/// a layer's origin but not its extent, and nothing bounds the
/// whole-document total at all, so an oversized *layer* or two
/// full-canvas-masked ceiling layers still reach this refusal from a
/// live session. Refusing is still the right answer (the
/// alternative is an unfinishable loop, or a file that cannot be
/// reopened) but it means one such layer disables autosave for the rest
/// of the session, which is a real cost. PLAN.md records it.
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
    // Before a single byte is written: every rectangle the tile loop
    // below will derive a grid from, range-checked, and their grids
    // summed against the whole-document budget. A refusal here leaves
    // no half-built container behind -- which, until 0.71.1, an
    // oversized *layer* extent did (it failed from inside the loop,
    // after the mimetype/manifest/history entries were already
    // written), and an over-budget document did not do at all (the
    // writer had no budget check, so it produced files its own reader
    // then refused).
    validate_persisted_rects(layers)?;

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

    for (surface, bounds) in persisted_surfaces(layers) {
        let (tiles_x, tiles_y) = tile_grid(bounds)?;
        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                let tile_id = TileId { x: tx, y: ty };
                // A tile the store has never held cannot hold anything
                // worth writing: `store.get` would materialize it as
                // `Tile::blank()`, and the all-zero check below would
                // then drop it again. Identical output, without paying
                // for the materialization -- and it is a mask surface
                // that makes this worth doing, since a mask nobody has
                // painted is the common case for every document written
                // today.
                if !store.contains_tile(surface, tile_id) {
                    continue;
                }
                let tile = match store.get(surface, tile_id) {
                    Ok(tile) => tile,
                    Err(err) => match unreadable {
                        UnreadableTile::Refuse => return Err(err.into()),
                        UnreadableTile::Skip => {
                            tracing::warn!(
                                ?surface,
                                ?tile_id,
                                // Which half of the surface id space
                                // this is: a lost mask tile and a lost
                                // content tile look identical in a log
                                // otherwise, and they are different
                                // losses to a user.
                                mask = surface.to_raw() & aurora_doc::MASK_SURFACE_BIT != 0,
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

    // The one place the skip list becomes part of the *file* rather
    // than just the return value. The `is_empty` guard is load-bearing
    // and not an optimization: it is what keeps `write()` -- the
    // `Refuse` policy, and so every ordinary user-facing save -- byte-
    // identical to what it produced before this entry existed. Only a
    // best-effort write that really dropped something adds an entry.
    if !skipped.is_empty() {
        let records: Vec<SkippedTileRecord> = skipped
            .iter()
            .take(MAX_SKIPPED_TILE_RECORDS)
            .map(SkippedTileRecord::from)
            .collect();
        let bytes = postcard::to_allocvec(&records)
            .map_err(|source| IoError::ManifestSerialization(source.to_string()))?;
        zip.start_file(SKIPPED_TILES_ENTRY, deflated)?;
        zip.write_all(&bytes)?;
    }

    zip.finish()?;
    Ok(skipped)
}

/// [`read`]'s own return shape: the reconstructed `LayerTree`/`History`,
/// the manifest's own `(canvas_width, canvas_height)`, its own colour
/// profile (`None`/`Some` — see [`read`]'s own doc comment), and
/// whatever the writer had to leave out.
///
/// A named struct rather than the private four-tuple this was until
/// 0.74.0: a fifth positional element that callers must not ignore is
/// exactly the thing a tuple makes easy to ignore.
#[derive(Debug)]
pub struct AurDocument {
    pub layers: LayerTree,
    pub history: History,
    pub canvas_size: (u32, u32),
    pub profile: Option<aurora_color::IccProfile>,
    /// Tiles a [`write_best_effort`] writer could not read and
    /// therefore left out. Empty for every file [`write()`] produced,
    /// and for every file written before the `skipped-tiles` entry
    /// existed — the two are deliberately indistinguishable, since
    /// neither lost anything.
    ///
    /// Non-empty means this document is **missing content that cannot
    /// be recovered from this file**. A caller that shows a document to
    /// a user should say so; `aurora-app`'s own `open_aur_file` does.
    pub skipped_tiles: Vec<SkippedTileRecord>,
}

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
/// embedded a real ICC profile — all as a named [`AurDocument`].
///
/// [`AurDocument::skipped_tiles`] is the fifth field and the one a
/// caller must not ignore: non-empty means the file was written by
/// [`write_best_effort`] with content it could not read, and the
/// document just handed back is missing pixels no reread will restore.
///
/// **A failed read commits nothing** (0.71.2). Tiles go into `store` as
/// they are decoded, so a container whose *later* entries are corrupt
/// has already written its earlier ones by the time it fails; every one
/// of those is dropped again before this returns `Err`
/// (`roll_back_committed_tiles`), so a rejected file cannot leave
/// pixels resident under surface ids the caller's next document is
/// about to claim. See `read_persisted_tiles` for what that fixed and
/// the one case it cannot.
///
/// # Errors
///
/// Returns [`IoError::Zip`]/[`IoError::Io`] for a real container/I/O
/// failure, [`IoError::MissingEntry`] if the manifest or history entry
/// is absent (not a valid `.aur` file, or one truncated past recovery),
/// [`IoError::ManifestDeserialization`]/[`IoError::Doc`] if either
/// fails to decode — or if the optional `skipped-tiles` entry is
/// present but not decodable (its *absence* is never an error; see
/// `read_skipped_tiles`) —, [`IoError::Color`] if an embedded ICC profile's own
/// bytes fail to parse, [`IoError::Tile`] if a tile entry fails to
/// decode or doesn't decode to the expected sample count,
/// [`IoError::LayerBoundsTooLarge`] if the manifest declares a layer —
/// or a layer *mask* — whose extent is past the document ceiling,
/// [`IoError::LayerOriginOutOfRange`] if it declares a layer — or a
/// layer *mask* — whose *origin* is further from the document origin
/// than that same ceiling (a negative origin is still legal — see that
/// variant),
/// [`IoError::CanvasTooLarge`] if it declares a *canvas* past that same
/// ceiling, [`IoError::TooManyTiles`] if its layers **and their masks**
/// together add up to more tiles than any real document has, or
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

    // Every rectangle this manifest declares -- a layer's own bounds
    // and every mask's, origin and extent both -- plus the
    // whole-document tile budget, before any of them is used to derive
    // a grid below. Hoisted out of the tile loop deliberately: an
    // oversized extent must be refused before the loop it would
    // otherwise make unfinishable starts, not part-way through it.
    validate_persisted_rects(&manifest.layers)?;

    let history_bytes = read_entry(&mut zip, HISTORY_ENTRY)?;
    let history = History::load_journal(&history_bytes)?;

    let skipped_tiles = read_skipped_tiles(&mut zip)?;

    // Every tile this scan commits is recorded, and undone if the scan
    // does not finish -- see `read_persisted_tiles` for why a partial
    // read must not survive.
    let mut committed: Vec<(SurfaceId, TileId)> = Vec::new();
    if let Err(err) = read_persisted_tiles(&mut zip, &manifest.layers, store, &mut committed) {
        roll_back_committed_tiles(store, &committed, &err);
        return Err(err);
    }

    Ok(AurDocument {
        layers: manifest.layers,
        history,
        canvas_size: (manifest.canvas_width, manifest.canvas_height),
        profile,
        skipped_tiles,
    })
}

/// Reads [`SKIPPED_TILES_ENTRY`] if the container has one.
///
/// **Absence is not an error, and that is the whole design.** This is a
/// separate, optional top-level entry precisely so that every `.aur`
/// file and every crash-recovery autosave written before 0.74.0 keeps
/// opening unchanged — see this module's own doc comment for why a
/// manifest field could not do that (`postcard`'s positional wire
/// format) and why a `MANIFEST_VERSION` bump could not either ([`read`]
/// hard-refuses an unrecognised version). A missing entry means "the
/// writer dropped nothing", which is also exactly what an ordinary
/// [`write()`] means. The `FileNotFound` arm below is that contract,
/// modelled on the same arm in `read_persisted_tiles`.
///
/// # Errors
///
/// [`IoError::EntryTooLarge`] if the entry declares or holds more than
/// [`MAX_METADATA_ENTRY_BYTES`], [`IoError::ManifestDeserialization`]
/// if its bytes are not a `postcard`-encoded `Vec<SkippedTileRecord>`,
/// or [`IoError::Zip`]/[`IoError::Io`] for a real container failure.
fn read_skipped_tiles<R: Read + Seek>(
    zip: &mut ZipArchive<R>,
) -> Result<Vec<SkippedTileRecord>, IoError> {
    let bytes = match zip.by_name(SKIPPED_TILES_ENTRY) {
        Ok(file) => read_capped(file, SKIPPED_TILES_ENTRY, MAX_METADATA_ENTRY_BYTES)?,
        Err(zip::result::ZipError::FileNotFound) => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };
    let mut records: Vec<SkippedTileRecord> = postcard::from_bytes(&bytes)
        .map_err(|source| IoError::ManifestDeserialization(source.to_string()))?;
    // A real bound on untrusted input, not cosmetic: the byte cap above
    // still admits millions of records, and every one of them carries a
    // heap-allocated `String`. See `MAX_SKIPPED_TILE_RECORDS`.
    records.truncate(MAX_SKIPPED_TILE_RECORDS);
    Ok(records)
}

/// [`read`]'s own tile scan: every grid position of every persisted
/// surface, probed by name, decoded into `store` when an entry exists.
///
/// **Split out of [`read`] so that a failure part-way through is
/// recoverable** (0.71.2). This writes into the *caller's live* store as
/// it goes — deliberately, mirroring `crate::import::write_into_store`,
/// since staging a whole document's pixels somewhere else first would
/// double the memory and the scratch-disk traffic invariant §7.3.1
/// exists to avoid. The cost of that shape is that a container whose
/// *later* entries are corrupt has already committed its earlier ones,
/// and until 0.71.2 nothing undid them: the tiles stayed resident under
/// exactly the `SurfaceId`s the caller's next document was about to
/// claim (a fresh `LayerTree` restarts layer ids, and so surface ids,
/// from the bottom of the space), so a rejected file could show a user
/// pixels from a document that failed to open — or, on `aurora-app`'s
/// own "open a `.aur` file a user was sent" path, silently overwrite
/// tiles of the document they already had open.
///
/// So every `(surface, tile)` this commits is pushed onto `committed`
/// *after* the write succeeds, and [`read`] hands that list to
/// [`roll_back_committed_tiles`] on any error. The list is bounded by
/// the number of tile entries the archive actually holds (a grid
/// position with no entry commits nothing and records nothing), at
/// twelve bytes each.
///
/// 0.71.0 widened the window this closes rather than opening it: a
/// masked layer now has a *second* surface whose entries can fail
/// independently, after its content surface has already been committed
/// in full.
///
/// # Errors
///
/// The tile-scan half of [`read`]'s own list: [`IoError::Zip`] /
/// [`IoError::Io`] for a container failure, [`IoError::EntryTooLarge`]
/// for an entry claiming or holding more bytes than one tile can, and
/// [`IoError::Tile`] for one that fails to decode or decodes to the
/// wrong sample count.
fn read_persisted_tiles<R: Read + Seek>(
    zip: &mut ZipArchive<R>,
    layers: &LayerTree,
    store: &mut TileStore,
    committed: &mut Vec<(SurfaceId, TileId)>,
) -> Result<(), IoError> {
    for (surface, bounds) in persisted_surfaces(layers) {
        // Range-checked and charged against the whole-document budget
        // already, by `validate_persisted_rects`; this call is only
        // here for the grid dimensions it returns.
        let (tiles_x, tiles_y) = tile_grid(bounds)?;
        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                let tile_id = TileId { x: tx, y: ty };
                let name = tile_entry_name(surface, tile_id);
                let bytes = match zip.by_name(&name) {
                    Ok(file) => read_capped(file, &name, MAX_TILE_ENTRY_BYTES)?,
                    // No entry for this tile -- it was blank when
                    // written (see this module's own doc comment) and
                    // stays at the store's own default. Nothing is
                    // committed, so nothing is recorded for rollback.
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
                // Recorded only now, after the write really landed.
                // `store.get_mut` materializing a blank tile and then
                // failing is not a case this can reach -- the `?` above
                // returns first -- but recording after the fact means
                // the list can never name a tile this scan did not
                // actually put content into.
                committed.push((surface, tile_id));
            }
        }
    }
    Ok(())
}

/// Drops every tile [`read_persisted_tiles`] committed before it
/// failed, so a rejected container leaves the caller's store exactly as
/// unpopulated (by this read) as it was before.
///
/// **This deletes pixels, and that is correct here.** Every key in
/// `committed` is one this read itself wrote — the content came out of
/// the container being rejected, not out of the caller's own document.
/// The one case it is lossy is a caller that had already painted the
/// *same* `(surface, tile)` before calling [`read`] and whose tile this
/// read then overwrote: that tile's earlier content was destroyed by the
/// overwrite itself, not by this rollback, and no amount of eviction
/// here brings it back. `aurora-app` never does that — both of its
/// read paths open a document *into* a store, they do not merge one
/// into a live document — and the alternative (leaving the rejected
/// file's pixels resident) is the failure this exists to prevent.
///
/// Failures are logged, never returned: this runs on an error path that
/// already has a real error to report, and replacing that error with a
/// store-eviction one would hide the reason the read failed. `err` is
/// carried in only so the log line says which failure the rollback
/// belongs to.
fn roll_back_committed_tiles(
    store: &mut TileStore,
    committed: &[(SurfaceId, TileId)],
    err: &IoError,
) {
    if committed.is_empty() {
        return;
    }
    for &(surface, tile_id) in committed {
        store.forget_tile(surface, tile_id);
    }
    tracing::warn!(
        tiles = committed.len(),
        %err,
        "a .aur read failed part-way through; dropped every tile it had already committed to the \
         tile store"
    );
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

/// Every surface this format persists, paired with the rectangle its
/// own tile grid is derived from — [`write`]/[`read`]'s **one** shared
/// answer to "what goes in the archive", so the writer and the reader
/// cannot disagree about it.
///
/// Two kinds of surface, and both are real content a user can lose:
///
/// - A pixel layer's own content surface (`LayerTree::surface_id`),
///   addressed relative to that layer's own `LayerKind::Pixel` bounds.
/// - A layer's **mask coverage** surface
///   (`LayerTree::mask_surface_id`), addressed relative to that mask's
///   own `aurora_doc::LayerMask::bounds` origin — *not* the layer's
///   (`aurora_doc::mask`'s own "addressing convention" section is the
///   normative statement of that, and this module is a real caller
///   bound by it). A *group* has no content surface but can carry a
///   mask, so it appears here with exactly one entry.
///
/// A masked pixel layer therefore contributes two entries and has its
/// grid walked twice — once per surface, at two different origins.
/// That is a real, named cost of persisting masks, not a free
/// extension: both the whole-document tile budget
/// ([`MAX_TOTAL_TILES_PER_DOCUMENT`], widened to match) and the wall
/// clock see it.
///
/// Mask coverage is persisted **regardless of
/// `aurora_doc::LayerMask::enabled`**, deliberately. `enabled` is a UI
/// toggle (shift-click a mask thumbnail); gating persistence on it
/// would mean switching a mask off and saving silently destroys the
/// pixels the user painted into it, with no way back — precisely the
/// "silently degrading a professional's file" failure CLAUDE.md names
/// as the worst this project can have. `inverted` is a composite-time
/// interpretation of the same pixels and likewise changes nothing about
/// what is stored.
///
/// Order doesn't matter here (unlike `aurora_ui::layers_panel`'s own
/// top-to-bottom paint-order convention) since this only decides which
/// tiles to touch, not how to composite them.
fn persisted_surfaces(layers: &LayerTree) -> Vec<(SurfaceId, aurora_core::Rect)> {
    let mut surfaces = Vec::new();
    for id in layer_ids(layers) {
        if let Some(LayerKind::Pixel { bounds }) = layers.kind(id)
            && let Some(surface) = layers.surface_id(id)
        {
            surfaces.push((surface, *bounds));
        }
        if let Some(mask) = layers.mask(id) {
            if let Some(surface) = layers.mask_surface_id(id) {
                surfaces.push((surface, mask.bounds));
            } else {
                // Unreachable today and not silently relied on to stay
                // that way: `mask_surface_id` returns `None` only for a
                // layer that is not in the tree, and `mask` above just
                // proved it is. If that ever stops holding, this is a
                // mask whose painted coverage would be dropped from
                // every save with no error anywhere -- the exact
                // silent-degradation failure this format refuses
                // elsewhere -- so it is logged rather than left to be
                // rediscovered from a user's lost work.
                tracing::warn!(
                    ?id,
                    "a layer carries a mask but has no mask surface id; its coverage cannot be \
                     persisted"
                );
            }
        }
    }
    surfaces
}

/// Every layer the tree holds, groups included — the shared walk
/// [`persisted_surfaces`] goes through (and so, by way of it,
/// [`validate_persisted_rects`]).
///
/// Covering *groups* is the whole reason it walks everything rather
/// than only `LayerKind::Pixel` entries. A group carries no `bounds`
/// and has no content surface, but it can carry a mask, and that mask
/// has both a `Rect` reaching the same downstream arithmetic a pixel
/// layer's bounds do and (since 0.71.0) coverage tiles of its own to
/// persist.
///
/// Iterative on an explicit stack, never recursive. `LayerTree`'s own
/// `Deserialize` already refuses a manifest whose tree isn't really a
/// tree, so a cycle can no longer reach here from a file at all — but
/// this walk runs on `aurora-app`'s own pre-window startup path, where
/// the failure mode of unbounded recursion is a stack overflow, and a
/// stack overflow is a process abort rather than an `Err` anything can
/// report. `budget` bounds the walk at one visit per layer the tree
/// actually holds, which is all a real tree ever needs.
///
/// Walking from `roots` rather than over the `layers` map reaches every
/// entry regardless: `aurora_doc`'s own deserialize-time `validate_shape`
/// refuses an orphan (an entry nothing names), so "reachable from
/// `roots`" and "present in the map" are the same set for any tree that
/// got this far.
fn layer_ids(layers: &LayerTree) -> Vec<LayerId> {
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
        ids.push(id);
        if let LayerKind::Group { children } = kind {
            stack.extend(children.iter().rev().copied());
        }
    }
    ids
}

/// The whole pre-flight both directions of the format run **before any
/// tile grid is walked, any byte is written, or any archive entry is
/// probed**: every rectangle [`persisted_surfaces`] will derive a grid
/// from — a pixel layer's own `bounds` *and* every mask's — checked for
/// range, and their grids summed against
/// [`MAX_TOTAL_TILES_PER_DOCUMENT`].
///
/// Three refusals, all of them the same variants `tile_grid` and
/// [`read`] already returned:
///
/// - [`IoError::LayerOriginOutOfRange`] — a rectangle whose origin sits
///   further from the document origin than
///   [`aurora_core::MAX_DOCUMENT_ORIGIN`].
/// - [`IoError::LayerBoundsTooLarge`] — one whose extent is past
///   [`aurora_core::MAX_DOCUMENT_EXTENT`].
/// - [`IoError::TooManyTiles`] — grids that individually pass and
///   together do not.
///
/// A layer's bad rectangle and its mask's are deliberately not told
/// apart: they are the same failure classes reaching the same
/// downstream arithmetic, and a caller has no use for the distinction.
///
/// **Why it is one hoisted walk rather than checks inside the loops.**
/// A grid derived from an unchecked extent is not a big loop but an
/// unfinishable one (~2.8e14 iterations for a `u32::MAX`-wide
/// rectangle), reached from `aurora-app`'s own pre-window startup
/// recovery path; and a refusal discovered *part-way* through a write
/// leaves a well-formed partial container behind at whatever
/// destination the caller passed. Both are only closed by checking
/// everything up front. Until 0.57.13 a crafted manifest could declare
/// a mask at `i64::MIN`, be read back as `Ok`, survive a round trip and
/// reach `apply_mask` -> `aurora_core::Rect::contains_point` in
/// `aurora-app`, which saturates rather than panicking and so renders
/// the *wrong picture* rather than failing loudly.
///
/// **History of what this walk covered**, because the gaps were found
/// one at a time and the pattern matters more than any one of them:
/// mask *origins* only (0.57.13), mask extents as well once mask
/// coverage became real persisted tiles (0.71.0), and finally — 0.71.1,
/// this revision — layer bounds and the whole-document tile budget too,
/// which had been checked only from inside the loops they bound. That
/// last gap was reachable with no crafted input at all: two ordinary
/// layers with ordinary full-canvas masks wrote a valid container the
/// reader then refused ([`MAX_TOTAL_TILES_PER_DOCUMENT`]), and an
/// oversized layer extent — `aurora_doc` bounds a layer's origin but
/// not its extent — aborted from inside the tile loop, after the
/// mimetype, manifest and history entries had already been written.
///
/// Called from [`read`] and from `write_with_policy` — the shared body
/// behind [`write()`] and [`write_best_effort`] — so one call site each
/// covers every path in and out of the format.
///
/// `tile_grid` still runs again per surface inside those loops, since
/// that is what produces the grid dimensions they walk. That is a
/// repeat of two comparisons per surface, not a second policy: this
/// function is the one that decides, and it decides first.
fn validate_persisted_rects(layers: &LayerTree) -> Result<(), IoError> {
    let mut total_tiles: u64 = 0;
    for (_surface, bounds) in persisted_surfaces(layers) {
        let (tiles_x, tiles_y) = tile_grid(bounds)?;
        // Charged before the next surface is even looked at, so an
        // over-budget document costs one addition per surface rather
        // than a scan -- see `MAX_TOTAL_TILES_PER_DOCUMENT`.
        total_tiles = total_tiles.saturating_add(u64::from(tiles_x) * u64::from(tiles_y));
        if total_tiles > MAX_TOTAL_TILES_PER_DOCUMENT {
            return Err(IoError::TooManyTiles {
                total: total_tiles,
                max: MAX_TOTAL_TILES_PER_DOCUMENT,
            });
        }
    }
    Ok(())
}

/// The ZIP entry name one tile's own encoded bytes live under —
/// `tiles/<surface>/<x>_<y>.tile`, a real, inspectable path (ADR 0009's
/// own "open format... a user can inspect a `.aur` file's contents with
/// a file manager" goal), not an opaque or flat-namespaced one.
///
/// `surface` is whatever [`persisted_surfaces`] yielded, so this names
/// **mask coverage tiles as well as layer content tiles**. The two are
/// told apart by the surface id itself and nothing else: a mask
/// surface has `aurora_doc::MASK_SURFACE_BIT` set, which puts it in the
/// top half of the id space and makes its directory name a very large
/// decimal number. That is enough for the format (the reader derives
/// which surface it expects rather than parsing these names), though it
/// does mean the "inspect it with a file manager" goal is served less
/// well for masks than for layers — a real, accepted cost of deriving
/// mask surface ids rather than storing them.
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
        // `.tile` only, never a bare entry list: a scratch directory
        // also holds `aurora_tile::LOCK_FILE_NAME` (0.67.0), which is
        // not an evicted tile and must not be counted as one.
        let files: Vec<std::path::PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "tile"))
            .collect();
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
        let super::AurDocument {
            layers: restored_layers,
            history: restored_history,
            canvas_size,
            profile,
            ..
        } = match read(bytes, &mut fresh_store) {
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
        let restored_profile = match read(bytes, &mut fresh_store) {
            Ok(result) => result.profile,
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

    /// Which shape of layer a crafted manifest should carry, and where
    /// its own rectangle sits.
    ///
    /// A group is not a redundant variant. A group carries no `bounds`
    /// of its own, so it never reaches `tile_grid` — which is exactly
    /// why a *group's* mask was the half of this check that stayed open
    /// after 0.57.12: `tile_grid` is only ever called on a
    /// `LayerKind::Pixel` arm, so a check placed there cannot see one.
    #[derive(Clone, Copy)]
    enum CraftedKind {
        /// A 16x16 pixel layer whose own origin is `(x, y)`.
        Pixel(i64, i64),
        /// A childless group, which has no origin of its own at all.
        Group,
    }

    /// The mirror structs behind [`crafted_manifest`], hoisted out of it
    /// so [`crafted_tree_bytes`] can reuse them for the write-side test.
    /// They match `LayerTree`'s own derived `Serialize` field-for-field.
    mod mirror {
        #[derive(serde::Serialize)]
        pub(super) enum Kind {
            Pixel { x: i64, y: i64, w: u32, h: u32 },
            Group { children: Vec<u64> },
        }
        #[derive(serde::Serialize)]
        pub(super) struct Lock {
            pub(super) transparency: bool,
            pub(super) pixels: bool,
            pub(super) position: bool,
        }
        /// Mirrors `aurora_doc::LayerMask`: an `aurora_core::Rect`
        /// (four positional fields) then the two toggles.
        #[derive(serde::Serialize)]
        pub(super) struct Mask {
            pub(super) x: i64,
            pub(super) y: i64,
            pub(super) w: u32,
            pub(super) h: u32,
            pub(super) enabled: bool,
            pub(super) inverted: bool,
        }
        #[derive(serde::Serialize)]
        pub(super) struct Entry {
            pub(super) name: String,
            pub(super) parent: Option<u64>,
            pub(super) kind: Kind,
            pub(super) opacity: f32,
            pub(super) fill_opacity: f32,
            pub(super) blend_mode: u32,
            pub(super) visible: bool,
            pub(super) lock: Lock,
            pub(super) mask: Option<Mask>,
        }
        #[derive(serde::Serialize)]
        pub(super) struct Tree {
            pub(super) ids: u64,
            pub(super) layers: std::collections::HashMap<u64, Entry>,
            pub(super) roots: Vec<u64>,
        }
        #[derive(serde::Serialize)]
        pub(super) struct Manifest {
            pub(super) version: u32,
            pub(super) canvas_width: u32,
            pub(super) canvas_height: u32,
            pub(super) color_space: u32,
            pub(super) layers: Tree,
        }
    }

    /// A mask a crafted fixture should attach, rectangle and all.
    ///
    /// The extent is a field rather than the fixed 16x16 it used to be
    /// because a mask's *extent* is checked as of 0.71.0 — it now
    /// derives a real tile grid, so an unbounded one is an unfinishable
    /// loop, and a fixture has to be able to declare one.
    #[derive(Clone, Copy)]
    struct CraftedMask {
        x: i64,
        y: i64,
        w: u32,
        h: u32,
    }

    impl CraftedMask {
        /// A 16x16 mask at `(x, y)` — the origin-only cases.
        fn at(x: i64, y: i64) -> Self {
            Self { x, y, w: 16, h: 16 }
        }
    }

    /// The one-layer tree every crafted fixture below is built from.
    fn crafted_tree(kind: CraftedKind, mask: Option<CraftedMask>) -> mirror::Tree {
        let mut layers = std::collections::HashMap::new();
        layers.insert(
            0u64,
            mirror::Entry {
                name: "far".to_owned(),
                parent: None,
                kind: match kind {
                    CraftedKind::Pixel(x, y) => mirror::Kind::Pixel { x, y, w: 16, h: 16 },
                    CraftedKind::Group => mirror::Kind::Group { children: vec![] },
                },
                opacity: 1.0,
                fill_opacity: 1.0,
                blend_mode: 0,
                visible: true,
                lock: mirror::Lock {
                    transparency: false,
                    pixels: false,
                    position: false,
                },
                mask: mask.map(|mask| mirror::Mask {
                    x: mask.x,
                    y: mask.y,
                    w: mask.w,
                    h: mask.h,
                    enabled: true,
                    inverted: false,
                }),
            },
        );
        mirror::Tree {
            ids: 1,
            layers,
            roots: vec![0],
        }
    }

    /// A manifest declaring exactly one layer — hand-crafted rather
    /// than round-tripped through `LayerTree`'s own API, and that is
    /// the whole point.
    ///
    /// Its neighbour `read_rejects_a_manifest_declaring_a_layer_past_the_document_ceiling`
    /// *can* still build its fixture through the real API, because
    /// `aurora-doc` deliberately does not bound a layer's extent. It
    /// does bound the *origin* now
    /// (`aurora_doc::DocError::LayerOriginOutOfRange`), so there is no
    /// longer any path through `LayerTree`'s public API that produces
    /// an out-of-range one — for a layer's own bounds *or* for a
    /// mask's, since `add_mask` is checked too. The fixture has to be
    /// assembled on the wire instead. That the doc API refuses to build
    /// it is itself the evidence that the live-edit half of this fix
    /// works; what remains to test here is the *reader's* own
    /// independent check, which is what stops a file that never went
    /// through that API.
    ///
    /// `mask` attaches a mask with that rectangle, or leaves the layer
    /// maskless when `None`.
    ///
    /// The mirror structs match `LayerTree`'s own derived `Serialize`
    /// field-for-field, the same trick
    /// `read_rejects_a_manifest_whose_layer_tree_is_cyclic_rather_than_aborting`
    /// uses — `postcard`'s wire format is positional, so an identical
    /// shape encodes to identical bytes. `x`/`y` are `i64` here,
    /// matching `Rect`'s own types: `postcard` zigzag-varint-encodes
    /// signed integers by width, so an `i32` mirror could not even
    /// represent `i64::MAX`.
    fn crafted_manifest(kind: CraftedKind, mask: Option<CraftedMask>) -> Vec<u8> {
        let manifest = mirror::Manifest {
            version: super::MANIFEST_VERSION,
            canvas_width: 1,
            canvas_height: 1,
            color_space: 0,
            layers: crafted_tree(kind, mask),
        };
        match postcard::to_allocvec(&manifest) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    /// The same one-layer tree on its own, without a manifest around
    /// it — what the *write*-side test needs, since it has to hand
    /// `write` a real `LayerTree` and `LayerTree`'s own API refuses to
    /// build one carrying an out-of-range origin. Its derived
    /// `Deserialize` does not: origin is a value-range property, and
    /// this crate is where that range is enforced for a file.
    fn crafted_tree_bytes(kind: CraftedKind, mask: Option<CraftedMask>) -> Vec<u8> {
        match postcard::to_allocvec(&crafted_tree(kind, mask)) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    /// The maskless pixel-layer case, which most of these tests want.
    fn crafted_origin_manifest(x: i64, y: i64) -> Vec<u8> {
        crafted_manifest(CraftedKind::Pixel(x, y), None)
    }

    #[test]
    fn read_rejects_a_manifest_declaring_a_layer_origin_past_the_document_range() {
        // Pre-fix this returned `Ok` -- measured against a real crafted
        // container, not assumed. The origin then flowed on into
        // `aurora-app`'s own `read_layer_window`, which subtracts it
        // from a document origin in `i64` with no check of its own.
        let manifest_bytes = crafted_origin_manifest(i64::MAX, 0);
        let (_dir, mut store) = real_tile_store();
        match read(container_with(&manifest_bytes, &[]), &mut store) {
            Err(super::IoError::LayerOriginOutOfRange { x, y, max }) => {
                assert_eq!(x, i64::MAX);
                assert_eq!(y, 0);
                assert_eq!(max, aurora_core::MAX_DOCUMENT_ORIGIN);
            }
            other => unreachable!("expected LayerOriginOutOfRange, got {other:?}"),
        }
    }

    #[test]
    fn read_accepts_a_layer_origin_exactly_at_the_document_range() {
        // The other side of the same check, and the one that keeps the
        // bound honest: a layer a whole document extent off the top
        // edge is legal scope, not a defect. The negative `y` is the
        // case that would break if this had been written as `x >= 0`.
        let manifest_bytes = crafted_origin_manifest(
            aurora_core::MAX_DOCUMENT_ORIGIN,
            -aurora_core::MAX_DOCUMENT_ORIGIN,
        );
        let (_dir, mut store) = real_tile_store();
        if let Err(err) = read(container_with(&manifest_bytes, &[]), &mut store) {
            unreachable!("an origin exactly at the document range must still read: {err:?}");
        }
    }

    #[test]
    fn read_rejects_a_manifest_whose_layer_mask_origin_is_past_the_document_range() {
        // The gap 0.57.12 left open, and the reason `validate_persisted_rects`
        // exists as a walk of its own rather than another line inside
        // `tile_grid`: a mask carries a `Rect` that is *not* the layer's
        // own `bounds`, and `tile_grid` never sees it. Measured against
        // the 0.57.12 tree before this fix: this manifest read back as
        // `Ok`, and the origin survived a write-then-read round trip
        // unchanged, on its way to `apply_mask` (then named
        // `apply_mask_clip`) -> `Rect::contains_point` in `aurora-app`
        // -- which saturates
        // rather than panicking now, and so renders the wrong picture
        // (the masked layer fully hidden, or fully shown when
        // `inverted`) instead of failing loudly.
        for bad in [
            i64::MAX,
            i64::MIN,
            aurora_core::MAX_DOCUMENT_ORIGIN + 1,
            -aurora_core::MAX_DOCUMENT_ORIGIN - 1,
        ] {
            let manifest_bytes =
                crafted_manifest(CraftedKind::Pixel(0, 0), Some(CraftedMask::at(bad, 0)));
            let (_dir, mut store) = real_tile_store();
            match read(container_with(&manifest_bytes, &[]), &mut store) {
                Err(super::IoError::LayerOriginOutOfRange { x, y, max }) => {
                    assert_eq!(x, bad);
                    assert_eq!(y, 0);
                    assert_eq!(max, aurora_core::MAX_DOCUMENT_ORIGIN);
                }
                other => unreachable!("expected LayerOriginOutOfRange, got {other:?}"),
            }
        }
    }

    #[test]
    fn read_rejects_a_manifest_whose_group_layer_mask_origin_is_past_the_document_range() {
        // The half a `tile_grid`-placed check could not reach in
        // 0.57.12. A group has no `LayerKind::Pixel` arm, so the walk
        // of the day skipped it and `tile_grid` was never called for it
        // at all -- yet a group can carry a mask, and that mask's own
        // rectangle reaches the same compositing arithmetic a pixel
        // layer's does. (Since 0.71.0 `persisted_surfaces` does yield a
        // group's mask surface, so `tile_grid` would eventually see this
        // rectangle anyway -- but only after the reader had started
        // walking grids, which is exactly why `validate_persisted_rects`
        // still runs first.)
        let manifest_bytes =
            crafted_manifest(CraftedKind::Group, Some(CraftedMask::at(0, i64::MIN)));
        let (_dir, mut store) = real_tile_store();
        match read(container_with(&manifest_bytes, &[]), &mut store) {
            Err(super::IoError::LayerOriginOutOfRange { x, y, max }) => {
                assert_eq!(x, 0);
                assert_eq!(y, i64::MIN);
                assert_eq!(max, aurora_core::MAX_DOCUMENT_ORIGIN);
            }
            other => unreachable!("expected LayerOriginOutOfRange, got {other:?}"),
        }
    }

    #[test]
    fn read_accepts_a_layer_mask_origin_exactly_at_the_document_range() {
        // The other side of both checks above, for a pixel layer and a
        // group alike: the limits are legal scope, and a mask a whole
        // document extent off the top edge is a mask a user can still
        // drag back.
        for kind in [CraftedKind::Pixel(0, 0), CraftedKind::Group] {
            let manifest_bytes = crafted_manifest(
                kind,
                Some(CraftedMask::at(
                    aurora_core::MAX_DOCUMENT_ORIGIN,
                    -aurora_core::MAX_DOCUMENT_ORIGIN,
                )),
            );
            let (_dir, mut store) = real_tile_store();
            if let Err(err) = read(container_with(&manifest_bytes, &[]), &mut store) {
                unreachable!(
                    "a mask origin exactly at the document range must still read: {err:?}"
                );
            }
        }
    }

    #[test]
    fn write_and_best_effort_write_both_refuse_a_layer_mask_origin_past_the_document_range() {
        // `validate_persisted_rects` is called from `write_with_policy`,
        // the shared body behind both public writers, so this covers
        // the way *out* of the format as well as the way in — the same
        // property `tile_grid` already gives the layer-bounds check.
        //
        // The tree is deserialized rather than built: `add_mask` now
        // refuses this origin, so the derived `Deserialize` is the only
        // way to get one into a `LayerTree` at all.
        let tree_bytes =
            crafted_tree_bytes(CraftedKind::Pixel(0, 0), Some(CraftedMask::at(0, i64::MAX)));
        let layers: LayerTree = match postcard::from_bytes(&tree_bytes) {
            Ok(tree) => tree,
            Err(err) => unreachable!("the crafted tree must still deserialize: {err:?}"),
        };
        let history = History::new();

        let (_dir, mut store) = real_tile_store();
        let mut out = Cursor::new(Vec::new());
        match write(&mut out, &layers, &history, (1, 1), None, &mut store) {
            Err(super::IoError::LayerOriginOutOfRange { x, y, max }) => {
                assert_eq!(x, 0);
                assert_eq!(y, i64::MAX);
                assert_eq!(max, aurora_core::MAX_DOCUMENT_ORIGIN);
            }
            other => unreachable!("expected LayerOriginOutOfRange, got {other:?}"),
        }

        // The crash-recovery writer refuses it too rather than skipping
        // the layer under its `UnreadableTile::Skip` policy. That is
        // deliberate and disclosed in PLAN.md: `Skip` is scoped to an
        // unreadable *tile*, and a bad rectangle says the tree itself is
        // wrong, not that one piece of input is unreadable.
        let mut out = Cursor::new(Vec::new());
        match write_best_effort(&mut out, &layers, &history, (1, 1), None, &mut store) {
            Err(super::IoError::LayerOriginOutOfRange { .. }) => {}
            other => unreachable!("expected LayerOriginOutOfRange, got {other:?}"),
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
        let canvas_size = match read(container_with(&manifest_bytes, &[]), &mut store) {
            Ok(result) => result.canvas_size,
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
        let super::AurDocument {
            layers: restored_layers,
            canvas_size,
            ..
        } = match read(salvaged, &mut fresh_store) {
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

    // ---------------------------------------------------------------
    // Mask coverage persistence (0.71.0).
    //
    // Exercised by these tests and nothing else: no tool in the app
    // paints mask coverage yet (`aurora_doc::mask`'s own follow-on 1),
    // so none of this has ever run end to end through the editor. Read
    // it as evidence about the format, not about the product.
    // ---------------------------------------------------------------

    /// A mask rectangle deliberately unlike [`bounds`]: a different
    /// origin, and a different *extent* -- 300x200, which is two tiles
    /// wide where the 10x10 layer is one.
    ///
    /// **The extent is what makes a round trip here discriminating, and
    /// an earlier version of this comment credited the origin instead**
    /// (corrected 0.71.4; both reviews of 0.71.0 caught it
    /// independently). `tile_grid` derives a grid from `width`/`height`
    /// alone and the loops walk `0..tiles_x`/`0..tiles_y`, so a tile
    /// *index* is identical whatever the rectangle's origin is -- a
    /// reader that wrongly addressed mask tiles relative to the layer
    /// would still produce `0_0` for both frames and pass. What it
    /// cannot do is invent the second tile column the mask's own 300px
    /// width has and the layer's 10px width does not: mutating the
    /// enumerator to yield the layer's rectangle for a mask surface
    /// makes this test fail there.
    ///
    /// The differing origin is still worth keeping. It is what the
    /// *cross-crate* frame check needs -- `aurora-app`'s `apply_mask`
    /// windows coverage by `mask.bounds.x/y`, where an origin mix-up
    /// does show up as a shifted picture (see
    /// `aurora-app`'s own `.aur`-round-trip mask test) -- but within
    /// this module's own tile addressing it proves nothing on its own.
    fn mask_bounds() -> aurora_core::Rect {
        aurora_core::Rect {
            x: 500,
            y: 300,
            width: 300,
            height: 200,
        }
    }

    /// Exact float equality expressed as bit equality -- these round
    /// trips really are exact, and this workspace denies
    /// `clippy::float_cmp`. Same helper `aurora_doc::mask`'s own tests
    /// use, for the same reason.
    fn exactly(actual: f32, expected: f32) -> bool {
        actual.to_bits() == expected.to_bits()
    }

    /// One texel's mask coverage, read through the same public reader
    /// (`aurora_doc::read_mask_coverage`) the compositor uses -- not by
    /// poking at raw channels here, which would let this test agree
    /// with a broken convention.
    fn coverage_at(
        store: &mut TileStore,
        surface: aurora_tile::SurfaceId,
        tile: TileId,
        x: usize,
        y: usize,
    ) -> f32 {
        let entry = match store.get(surface, tile) {
            Ok(entry) => entry,
            Err(err) => unreachable!("a real store must serve this tile: {err:?}"),
        };
        let base = (y * aurora_tile::TILE as usize + x) * aurora_tile::CHANNELS;
        let Some(texel) = entry.texels().get(base..base + aurora_tile::CHANNELS) else {
            unreachable!("(x, y) is constructed in range for a whole tile");
        };
        aurora_doc::read_mask_coverage(texel)
    }

    #[test]
    fn round_trips_real_mask_coverage_painted_at_the_masks_own_origin() {
        // The headline case. Coverage `0.0` is what makes it a real
        // test: a mask entry that was never written, or written and
        // lost, reads back as `1.0` (the never-painted default), so an
        // explicit `0.0` is the only value that tells "really
        // persisted" apart from "silently defaulted".
        let (_dir, mut store) = real_tile_store();
        let mut layers = LayerTree::new();
        let mut history = History::new();
        let id = match history.add_pixel_layer(&mut layers, "Masked", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.add_mask(id, mask_bounds()) {
            unreachable!("{err:?}");
        }
        let Some(mask_surface) = layers.mask_surface_id(id) else {
            unreachable!("a layer that exists has a mask surface");
        };

        // Two different tiles of the mask's own grid (300px wide is two
        // 256px tiles), addressed relative to the mask's own origin.
        let painted = [
            (TileId { x: 0, y: 0 }, 1_usize, 2_usize, 0.0_f32),
            (TileId { x: 1, y: 0 }, 3, 4, 0.5),
        ];
        for &(tile, x, y, coverage) in &painted {
            if let Err(err) =
                aurora_doc::write_mask_coverage(&mut store, mask_surface, tile, x, y, coverage)
            {
                unreachable!("{err:?}");
            }
        }

        let mut bytes = Cursor::new(Vec::new());
        if let Err(err) = write(
            &mut bytes,
            &layers,
            &history,
            (1000, 1000),
            None,
            &mut store,
        ) {
            unreachable!("{err:?}");
        }

        // The entries really are under the *mask* surface, not the
        // layer's own -- the two are different halves of the id space.
        let archive = match zip::ZipArchive::new(Cursor::new(bytes.get_ref().clone())) {
            Ok(archive) => archive,
            Err(err) => unreachable!("{err:?}"),
        };
        let prefix = format!("tiles/{}/", mask_surface.to_raw());
        let mask_entries = archive
            .file_names()
            .filter(|name| name.starts_with(&prefix))
            .count();
        assert_eq!(
            mask_entries,
            2,
            "both painted mask tiles must be written under the mask surface: {:?}",
            archive.file_names().collect::<Vec<_>>()
        );

        let (_dir2, mut fresh_store) = real_tile_store();
        bytes.set_position(0);
        let restored_layers = match read(bytes, &mut fresh_store) {
            Ok(result) => result.layers,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(
            restored_layers.mask(id).map(|mask| mask.bounds),
            Some(mask_bounds()),
            "the mask's own rectangle must survive as well as its pixels"
        );

        for &(tile, x, y, coverage) in &painted {
            let restored = coverage_at(&mut fresh_store, mask_surface, tile, x, y);
            // `f16` cannot hold every `f32`, so the expectation is what
            // the written value quantizes to -- still exact, not close.
            assert!(
                exactly(restored, half::f16::from_f32(coverage).to_f32()),
                "coverage {coverage} at {tile:?} ({x}, {y}) must survive the round trip, got {restored}"
            );
        }
        assert!(
            exactly(
                coverage_at(&mut fresh_store, mask_surface, TileId { x: 0, y: 0 }, 2, 2),
                1.0
            ),
            "an unpainted neighbour must still read as fully visible"
        );
    }

    #[test]
    fn round_trips_mask_coverage_on_a_group_which_has_no_pixel_surface() {
        // A group has no content surface at all, so its mask is the
        // only thing it can contribute to the archive -- the case a
        // walk that only enumerated pixel layers would miss entirely.
        let (_dir, mut store) = real_tile_store();
        let mut layers = LayerTree::new();
        let mut history = History::new();
        let group = match history.add_group(&mut layers, "Group", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.add_mask(group, mask_bounds()) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            layers.surface_id(group),
            None,
            "a group must have no content surface -- that is the point of this test"
        );
        let Some(mask_surface) = layers.mask_surface_id(group) else {
            unreachable!("a group still gets a mask surface");
        };
        let tile = TileId { x: 1, y: 0 };
        if let Err(err) = aurora_doc::write_mask_coverage(&mut store, mask_surface, tile, 7, 8, 0.0)
        {
            unreachable!("{err:?}");
        }

        let mut bytes = Cursor::new(Vec::new());
        if let Err(err) = write(
            &mut bytes,
            &layers,
            &history,
            (1000, 1000),
            None,
            &mut store,
        ) {
            unreachable!("{err:?}");
        }
        let (_dir2, mut fresh_store) = real_tile_store();
        bytes.set_position(0);
        if let Err(err) = read(bytes, &mut fresh_store) {
            unreachable!("{err:?}");
        }
        assert!(
            exactly(coverage_at(&mut fresh_store, mask_surface, tile, 7, 8), 0.0),
            "a group's own mask coverage must round trip"
        );
    }

    #[test]
    fn round_trips_mask_coverage_of_a_disabled_mask() {
        // `enabled` is a UI toggle (shift-click a thumbnail), not a
        // statement about what is stored. Gating persistence on it
        // would mean switching a mask off and saving silently destroys
        // the painted pixels -- unrecoverable, and exactly the failure
        // CLAUDE.md calls the worst this project can have. Locked in
        // here so a later "optimization" cannot quietly reintroduce it.
        let (_dir, mut store) = real_tile_store();
        let mut layers = LayerTree::new();
        let mut history = History::new();
        let id = match history.add_pixel_layer(&mut layers, "Masked", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.add_mask(id, mask_bounds()) {
            unreachable!("{err:?}");
        }
        if let Err(err) = layers.set_mask_enabled(id, false) {
            unreachable!("{err:?}");
        }
        let Some(mask_surface) = layers.mask_surface_id(id) else {
            unreachable!("a layer that exists has a mask surface");
        };
        let tile = TileId { x: 0, y: 0 };
        if let Err(err) = aurora_doc::write_mask_coverage(&mut store, mask_surface, tile, 9, 9, 0.0)
        {
            unreachable!("{err:?}");
        }

        let mut bytes = Cursor::new(Vec::new());
        if let Err(err) = write(
            &mut bytes,
            &layers,
            &history,
            (1000, 1000),
            None,
            &mut store,
        ) {
            unreachable!("{err:?}");
        }
        let (_dir2, mut fresh_store) = real_tile_store();
        bytes.set_position(0);
        let restored_layers = match read(bytes, &mut fresh_store) {
            Ok(result) => result.layers,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(
            restored_layers.mask(id).map(|mask| mask.enabled),
            Some(false),
            "the disabled flag itself must round trip too"
        );
        assert!(
            exactly(coverage_at(&mut fresh_store, mask_surface, tile, 9, 9), 0.0),
            "a disabled mask's painted coverage must still be persisted"
        );
    }

    #[test]
    fn reads_a_container_written_before_mask_persistence_as_fully_visible_coverage() {
        // What every `.aur` file and autosave written by 0.70.4 or
        // earlier looks like: a manifest that declares a mask, and not
        // one mask tile entry anywhere. It must still open, and the
        // mask must read back exactly as it composited then -- fully
        // visible, per `aurora_doc::mask`'s alpha-as-presence-flag
        // convention. This is why the round of work that added mask
        // entries did *not* bump `MANIFEST_VERSION`: nothing about the
        // manifest's shape changed, and a bump would have hard-rejected
        // every file already on disk.
        let mut layers = LayerTree::new();
        let id = match layers.add_pixel_layer("Masked", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.add_mask(id, mask_bounds()) {
            unreachable!("{err:?}");
        }
        let Some(mask_surface) = layers.mask_surface_id(id) else {
            unreachable!("a layer that exists has a mask surface");
        };
        let manifest = ManifestReadForTest {
            version: super::MANIFEST_VERSION,
            canvas_width: 1000,
            canvas_height: 1000,
            color_space: super::ColorSpaceTag::Srgb,
            layers,
        };
        let manifest_bytes = match postcard::to_allocvec(&manifest) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };

        let (_dir, mut store) = real_tile_store();
        let restored_layers = match read(container_with(&manifest_bytes, &[]), &mut store) {
            Ok(result) => result.layers,
            Err(err) => unreachable!("a pre-0.71.0 container must still open: {err:?}"),
        };
        assert!(
            restored_layers.mask(id).is_some(),
            "the mask itself must survive"
        );
        for tile in [TileId { x: 0, y: 0 }, TileId { x: 1, y: 0 }] {
            assert!(
                exactly(coverage_at(&mut store, mask_surface, tile, 0, 0), 1.0),
                "a mask with no persisted entries must read back fully visible at {tile:?}"
            );
        }
    }

    #[test]
    fn write_skips_a_never_painted_mask_surface_entirely() {
        // Every document written today has masks nobody has painted
        // (nothing in the app can paint one yet), so an unpainted mask
        // surface must cost zero bytes -- not a grid's worth of
        // all-zero entries. The layer's *own* painted tile is expected
        // in the same archive, so this is a statement about masks
        // specifically and not about an empty writer.
        let (_dir, mut store) = real_tile_store();
        let mut layers = LayerTree::new();
        let history = History::new();
        let id = match layers.add_pixel_layer("Masked", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.add_mask(id, mask_bounds()) {
            unreachable!("{err:?}");
        }
        let (Some(surface), Some(mask_surface)) =
            (layers.surface_id(id), layers.mask_surface_id(id))
        else {
            unreachable!("a pixel layer has both surfaces");
        };
        {
            let tile = match store.get_mut(surface, TileId { x: 0, y: 0 }) {
                Ok(tile) => tile,
                Err(err) => unreachable!("{err:?}"),
            };
            if let Some(sample) = tile.texels_mut().first_mut() {
                *sample = half::f16::from_f32(0.5);
            }
        }

        let mut bytes = Cursor::new(Vec::new());
        if let Err(err) = write(
            &mut bytes,
            &layers,
            &history,
            (1000, 1000),
            None,
            &mut store,
        ) {
            unreachable!("{err:?}");
        }
        let archive = match zip::ZipArchive::new(Cursor::new(bytes.get_ref().clone())) {
            Ok(archive) => archive,
            Err(err) => unreachable!("{err:?}"),
        };
        let mask_prefix = format!("tiles/{}/", mask_surface.to_raw());
        let names: Vec<&str> = archive.file_names().collect();
        assert!(
            !names.iter().any(|name| name.starts_with(&mask_prefix)),
            "an unpainted mask must write no entries at all: {names:?}"
        );
        assert!(
            names
                .iter()
                .any(|name| name.starts_with(&format!("tiles/{}/", surface.to_raw()))),
            "the layer's own painted tile must still be there: {names:?}"
        );
    }

    #[test]
    fn read_rejects_a_manifest_whose_mask_extent_is_past_the_document_ceiling() {
        // The gap this round had to close. Before mask surfaces were
        // persisted, a mask's extent drove no loop here and was
        // deliberately unchecked; now it derives a real tile grid, and
        // `aurora_doc::LayerTree::add_mask` bounds a mask's origin but
        // never its extent -- so an unchecked `u32::MAX` mask is an
        // unfinishable loop (~2.8e14 iterations) reached from
        // `aurora-app`'s own pre-window startup path. As with its
        // layer-bounds twin, the elapsed-time assertion is the point:
        // the answer must come back immediately, not eventually.
        for kind in [CraftedKind::Pixel(0, 0), CraftedKind::Group] {
            let manifest_bytes = crafted_manifest(
                kind,
                Some(CraftedMask {
                    x: 0,
                    y: 0,
                    w: u32::MAX,
                    h: u32::MAX,
                }),
            );
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
                "an oversized mask extent must be rejected up front, not looped over"
            );
        }
    }

    #[test]
    fn write_and_best_effort_write_both_refuse_a_mask_extent_past_the_document_ceiling() {
        // The way *out* of the format, for the same reason: `add_mask`
        // does not bound a mask's extent, so a `LayerTree` carrying an
        // oversized one is reachable in a live session (not only from a
        // crafted file), and a writer that walked its grid would hang
        // the app rather than refuse the save.
        let tree_bytes = crafted_tree_bytes(
            CraftedKind::Pixel(0, 0),
            Some(CraftedMask {
                x: 0,
                y: 0,
                w: u32::MAX,
                h: u32::MAX,
            }),
        );
        let layers: LayerTree = match postcard::from_bytes(&tree_bytes) {
            Ok(tree) => tree,
            Err(err) => unreachable!("the crafted tree must still deserialize: {err:?}"),
        };
        let history = History::new();
        let (_dir, mut store) = real_tile_store();

        let started = std::time::Instant::now();
        let mut out = Cursor::new(Vec::new());
        match write(&mut out, &layers, &history, (1, 1), None, &mut store) {
            Err(super::IoError::LayerBoundsTooLarge { width, max, .. }) => {
                assert_eq!(width, u32::MAX);
                assert_eq!(max, aurora_core::MAX_DOCUMENT_EXTENT);
            }
            other => unreachable!("expected LayerBoundsTooLarge, got {other:?}"),
        }
        // And nothing was written: `validate_persisted_rects` runs before the
        // first byte, so a refused save leaves no half-built container.
        assert!(
            out.get_ref().is_empty(),
            "a refused write must not leave a partial container behind"
        );

        let mut out = Cursor::new(Vec::new());
        match write_best_effort(&mut out, &layers, &history, (1, 1), None, &mut store) {
            Err(super::IoError::LayerBoundsTooLarge { .. }) => {}
            other => unreachable!("expected LayerBoundsTooLarge, got {other:?}"),
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "an oversized mask extent must be refused up front, not looped over"
        );
    }

    /// One ceiling-sized layer carrying a full-canvas mask — the
    /// largest single document PRD §7.3.1 says can exist, and the case
    /// `MAX_TOTAL_TILES_PER_DOCUMENT`'s `2 *` factor exists for.
    fn ceiling_layer_with_a_full_canvas_mask() -> LayerTree {
        let mut layers = LayerTree::new();
        let ceiling = aurora_core::Rect {
            x: 0,
            y: 0,
            width: aurora_core::MAX_DOCUMENT_EXTENT,
            height: aurora_core::MAX_DOCUMENT_EXTENT,
        };
        let id = match layers.add_pixel_layer("ceiling", ceiling, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.add_mask(id, ceiling) {
            unreachable!("{err:?}");
        }
        layers
    }

    #[test]
    fn the_largest_legal_document_still_writes_and_reads_with_its_mask() {
        // The behavioural replacement (0.71.1) for a test that used to
        // assert `2 * side * side <= MAX_TOTAL_TILES_PER_DOCUMENT` one
        // line after asserting `MAX == 2 * side * side` -- an arithmetic
        // tautology that could not fail whatever the constant actually
        // permitted, and so covered none of the property its name
        // claimed. This calls the real `write` and the real `read` on
        // the real document the widening exists for: one ceiling-sized
        // layer plus its own full-canvas mask, both grids charged
        // against the one budget.
        //
        // Both directions must say `Ok`. Before 0.71.0's widening the
        // *reader* refused this (one grid's worth of budget, two grids
        // declared); before 0.71.1 the *writer* had no budget check at
        // all, which is the opposite defect -- it would have written
        // this file whatever the budget said.
        //
        // No tiles are painted, so the cost here is the grid walk
        // itself (~2.75M positions, twice) and nothing else. That is
        // deliberately the expensive half: it is what proves the
        // document is *scanned*, not short-circuited.
        let layers = ceiling_layer_with_a_full_canvas_mask();
        let history = History::new();
        let (_dir, mut store) = real_tile_store();

        let started = std::time::Instant::now();
        let mut bytes = Cursor::new(Vec::new());
        if let Err(err) = write(
            &mut bytes,
            &layers,
            &history,
            (
                aurora_core::MAX_DOCUMENT_EXTENT,
                aurora_core::MAX_DOCUMENT_EXTENT,
            ),
            None,
            &mut store,
        ) {
            unreachable!("the largest legal document must still save: {err:?}");
        }

        bytes.set_position(0);
        let (_dir2, mut fresh_store) = real_tile_store();
        let super::AurDocument {
            layers: restored,
            canvas_size: canvas,
            ..
        } = match read(bytes, &mut fresh_store) {
            Ok(result) => result,
            Err(err) => unreachable!("the largest legal document must still reopen: {err:?}"),
        };
        assert_eq!(
            canvas,
            (
                aurora_core::MAX_DOCUMENT_EXTENT,
                aurora_core::MAX_DOCUMENT_EXTENT
            )
        );
        assert_eq!(
            restored.len(),
            1,
            "the ceiling-sized layer must survive the round trip"
        );
        // Loose on purpose -- this is a CI-safety bound on an
        // unoptimized build, not a performance claim. Its job is to
        // fail if this document ever becomes unfinishable, not to
        // measure anything.
        assert!(
            started.elapsed() < std::time::Duration::from_secs(100),
            "the largest legal document must finish both directions, not spin"
        );
    }

    #[test]
    fn write_refuses_two_ordinary_full_canvas_masked_layers_before_writing_a_byte() {
        // The gap 0.71.1 closed, and it needed no crafted input at all:
        // two ordinary layers, each given an ordinary full-canvas mask
        // through the public `add_mask`, used to *write* a perfectly
        // valid container that this module's own `read` then refused
        // with `TooManyTiles` -- a file an ordinary user could save and
        // never reopen, which is exactly the "silently degrading a
        // professional's file" failure CLAUDE.md names as the worst
        // this project can have.
        //
        // The layers here are individually legal (each is exactly the
        // documented ceiling, which
        // `the_largest_legal_document_still_writes_and_reads_with_its_mask`
        // proves must keep working); it is their sum that is not.
        let mut layers = LayerTree::new();
        let ceiling = aurora_core::Rect {
            x: 0,
            y: 0,
            width: aurora_core::MAX_DOCUMENT_EXTENT,
            height: aurora_core::MAX_DOCUMENT_EXTENT,
        };
        for index in 0..2 {
            let id = match layers.add_pixel_layer(format!("ceiling {index}"), ceiling, None) {
                Ok(id) => id,
                Err(err) => unreachable!("{err:?}"),
            };
            if let Err(err) = layers.add_mask(id, ceiling) {
                unreachable!("{err:?}");
            }
        }
        let history = History::new();
        let (_dir, mut store) = real_tile_store();

        let started = std::time::Instant::now();
        let mut out = Cursor::new(Vec::new());
        match write(&mut out, &layers, &history, (1, 1), None, &mut store) {
            Err(super::IoError::TooManyTiles { total, max }) => {
                assert!(total > max, "{total} must exceed the {max}-tile budget");
                assert_eq!(max, super::MAX_TOTAL_TILES_PER_DOCUMENT);
            }
            other => unreachable!("expected TooManyTiles, got {other:?}"),
        }
        assert!(
            out.get_ref().is_empty(),
            "a refused write must not leave a partial container behind"
        );

        // The autosave path refuses it too. `write_best_effort` tolerates
        // an unreadable *tile*, never a tree it cannot finish scanning.
        let mut out = Cursor::new(Vec::new());
        match write_best_effort(&mut out, &layers, &history, (1, 1), None, &mut store) {
            Err(super::IoError::TooManyTiles { .. }) => {}
            other => unreachable!("expected TooManyTiles, got {other:?}"),
        }
        assert!(
            out.get_ref().is_empty(),
            "a refused best-effort write must not leave a partial container behind"
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "an over-budget document must be refused on arithmetic, not by scanning it"
        );
    }

    #[test]
    fn write_refuses_an_oversized_layer_extent_before_writing_a_byte() {
        // The mask half of this was hoisted in 0.71.0 and the layer half
        // was not, so an oversized *layer* extent still failed from
        // inside the per-surface loop -- after the mimetype, manifest
        // and history entries had already been written, leaving a
        // well-formed 3-entry partial container at the destination. Its
        // real-world blast radius was bounded (`write_autosave` stages
        // to a temp path and renames on success), but a caller writing
        // straight to its destination would have been left with one,
        // and this module cannot see which kind of caller it has.
        //
        // Ordinary API use, again: `add_pixel_layer` bounds a layer's
        // origin but not its extent, so no crafted manifest is needed.
        let mut layers = LayerTree::new();
        let oversized = aurora_core::Rect {
            x: 0,
            y: 0,
            width: u32::MAX,
            height: u32::MAX,
        };
        if let Err(err) = layers.add_pixel_layer("oversized", oversized, None) {
            unreachable!("{err:?}");
        }
        let history = History::new();
        let (_dir, mut store) = real_tile_store();

        let started = std::time::Instant::now();
        for policy in 0..2 {
            let mut out = Cursor::new(Vec::new());
            let result = if policy == 0 {
                write(&mut out, &layers, &history, (1, 1), None, &mut store).map(|()| Vec::new())
            } else {
                write_best_effort(&mut out, &layers, &history, (1, 1), None, &mut store)
            };
            match result {
                Err(super::IoError::LayerBoundsTooLarge { width, max, .. }) => {
                    assert_eq!(width, u32::MAX);
                    assert_eq!(max, aurora_core::MAX_DOCUMENT_EXTENT);
                }
                other => unreachable!("expected LayerBoundsTooLarge, got {other:?}"),
            }
            assert!(
                out.get_ref().is_empty(),
                "a refused write must not leave a partial container behind"
            );
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "an oversized layer extent must be refused up front, not looped over"
        );
    }

    #[test]
    fn read_rejects_a_manifest_whose_layers_and_masks_together_exceed_the_tile_budget() {
        // Mask grids are charged against the same whole-document budget
        // layer grids are, from inside the same loop. Without that, a
        // manifest could stay under the budget on its layers alone and
        // then blow past it entirely in mask rectangles -- a small file
        // that spins on `aurora-app`'s pre-window startup path, which
        // is the exact defect the budget exists to prevent. The layers
        // here are deliberately tiny; the masks are what overruns it.
        let mut layers = LayerTree::new();
        let ceiling = aurora_core::Rect {
            x: 0,
            y: 0,
            width: aurora_core::MAX_DOCUMENT_EXTENT,
            height: aurora_core::MAX_DOCUMENT_EXTENT,
        };
        for index in 0..2 {
            let id = match layers.add_pixel_layer(format!("small {index}"), bounds(), None) {
                Ok(id) => id,
                Err(err) => unreachable!("{err:?}"),
            };
            if let Err(err) = layers.add_mask(id, ceiling) {
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
        match read(container, &mut store) {
            Err(super::IoError::TooManyTiles { total, max }) => {
                assert!(total > max, "{total} must exceed the {max}-tile budget");
                assert_eq!(max, super::MAX_TOTAL_TILES_PER_DOCUMENT);
            }
            other => unreachable!("expected TooManyTiles, got {other:?}"),
        }
    }

    #[test]
    fn best_effort_write_skips_an_unreadable_mask_tile_while_write_still_refuses() {
        // The refuse-vs-degrade split, applied to the surfaces this
        // round added: an explicit save must refuse rather than quietly
        // drop painted mask coverage (it is a user's work either way),
        // while an autosave writes what it can and names what it could
        // not. Same technique as its pixel-tile twin above: a one-tile
        // store budget forces the mask tile out to the scratch disk,
        // `flush` makes that write real, and truncating the file it
        // landed in leaves an ATIL file `codec::decode` rejects.
        let (dir, mut store) = one_tile_store();
        let mut layers = LayerTree::new();
        let mut history = History::new();
        let masked = match history.add_pixel_layer(&mut layers, "masked", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.add_mask(masked, bounds()) {
            unreachable!("{err:?}");
        }
        let Some(mask_surface) = layers.mask_surface_id(masked) else {
            unreachable!("a layer that exists has a mask surface");
        };
        if let Err(err) = aurora_doc::write_mask_coverage(
            &mut store,
            mask_surface,
            TileId { x: 0, y: 0 },
            2,
            3,
            0.0,
        ) {
            unreachable!("{err:?}");
        }
        // A second layer, whose own tile evicts the mask tile above.
        let other = match history.add_pixel_layer(&mut layers, "other", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(other_surface) = layers.surface_id(other) else {
            unreachable!("just created as a pixel layer");
        };
        {
            let tile = match store.get_mut(other_surface, TileId { x: 0, y: 0 }) {
                Ok(tile) => tile,
                Err(err) => unreachable!("{err:?}"),
            };
            if let Some(sample) = tile.texels_mut().first_mut() {
                *sample = half::f16::from_f32(0.5);
            }
        }
        if let Err(err) = store.flush() {
            unreachable!("test-local scratch disk must accept the write: {err:?}");
        }
        break_the_only_scratch_file(&dir);

        let mut refused = Cursor::new(Vec::new());
        match write(&mut refused, &layers, &history, (10, 10), None, &mut store) {
            Err(super::IoError::Tile(_)) => {}
            Ok(()) => {
                unreachable!("an explicit save must not silently drop unreadable mask coverage")
            }
            Err(other) => unreachable!("expected IoError::Tile, got {other:?}"),
        }

        let mut salvaged = Cursor::new(Vec::new());
        let skipped =
            match write_best_effort(&mut salvaged, &layers, &history, (10, 10), None, &mut store) {
                Ok(skipped) => skipped,
                Err(err) => unreachable!("a best-effort write must still produce a file: {err:?}"),
            };
        let [only] = skipped.as_slice() else {
            unreachable!("exactly one tile is unreadable, got {skipped:?}");
        };
        assert_eq!(only.surface, mask_surface);
        assert_eq!(only.tile, TileId { x: 0, y: 0 });
        assert!(
            only.reason.contains("corrupt tile file"),
            "the skip must carry the real underlying tile error: {}",
            only.reason
        );

        // And the salvaged file is a real document: the other layer's
        // pixels intact, the lost mask reading back as fully visible
        // rather than as a hidden layer.
        let (_fresh_dir, mut fresh_store) = real_tile_store();
        salvaged.set_position(0);
        if let Err(err) = read(salvaged, &mut fresh_store) {
            unreachable!("the salvaged autosave must reopen: {err:?}");
        }
        assert!(
            exactly(
                coverage_at(&mut fresh_store, mask_surface, TileId { x: 0, y: 0 }, 2, 3),
                1.0
            ),
            "a dropped mask tile must fail open (fully visible), never hide a layer"
        );
    }

    /// Field-for-field identical to `super::ManifestRead`, so its own
    /// `postcard` bytes decode identically -- used only to hand-craft a
    /// manifest with an unsupported `version` for
    /// `read_rejects_an_unsupported_manifest_version`, since the real
    /// `ManifestWrite`/`ManifestRead` always write the current,
    /// supported version.
    /// One tile's worth of real, valid `codec::encode` output, every
    /// texel set to `value` -- what a container's tile entry actually
    /// holds, produced through the same encoder the writer uses rather
    /// than hand-rolled bytes.
    fn encoded_tile(value: f32) -> Vec<u8> {
        let (_dir, mut store) = real_tile_store();
        let surface = aurora_tile::SurfaceId::from_raw(9_999);
        let tile = match store.get_mut(surface, TileId { x: 0, y: 0 }) {
            Ok(tile) => tile,
            Err(err) => unreachable!("{err:?}"),
        };
        for sample in tile.texels_mut() {
            *sample = f16::from_f32(value);
        }
        aurora_tile::codec::encode(tile.texels())
    }

    #[test]
    fn a_read_that_fails_on_a_mask_tile_leaves_no_content_tile_behind_either() {
        // The gap 0.71.2 closed, and 0.71.0 is what widened it: `read`
        // decodes tiles straight into the caller's live store as it
        // goes, and a masked layer now has *two* surfaces, walked one
        // after the other. So a container whose mask entry is corrupt
        // has already committed the layer's own content tile in full by
        // the time it fails -- and until 0.71.2 that tile stayed
        // resident, under exactly the `SurfaceId` the caller's next
        // document was about to claim (a fresh `LayerTree` restarts
        // layer ids, and so surface ids, from the bottom of the space).
        //
        // A real corrupted archive, not a mock: a real manifest, a real
        // `codec::encode`d content tile, and a mask entry holding bytes
        // `codec::decode` genuinely rejects.
        let mut layers = LayerTree::new();
        let id = match layers.add_pixel_layer("masked", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.add_mask(id, mask_bounds()) {
            unreachable!("{err:?}");
        }
        let (Some(content_surface), Some(mask_surface)) =
            (layers.surface_id(id), layers.mask_surface_id(id))
        else {
            unreachable!("a pixel layer with a mask has both surfaces");
        };
        // The two really are different surfaces -- otherwise this test
        // would prove nothing about ordering.
        assert_ne!(content_surface.to_raw(), mask_surface.to_raw());

        let manifest = ManifestReadForTest {
            version: super::MANIFEST_VERSION,
            canvas_width: 1000,
            canvas_height: 1000,
            color_space: super::ColorSpaceTag::Srgb,
            layers,
        };
        let manifest_bytes = match postcard::to_allocvec(&manifest) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        let content_entry = super::tile_entry_name(content_surface, TileId { x: 0, y: 0 });
        let mask_entry = super::tile_entry_name(mask_surface, TileId { x: 0, y: 0 });
        let container = container_with(
            &manifest_bytes,
            &[
                (content_entry, encoded_tile(0.75)),
                // Not an ATIL frame at all. `codec::decode` refuses it,
                // which is the mid-read failure this test needs -- and
                // it comes *after* the content surface, because
                // `persisted_surfaces` yields a layer's content before
                // its mask.
                (mask_entry, b"this is not an encoded tile".to_vec()),
            ],
        );

        let (_dir, mut store) = real_tile_store();
        match read(container, &mut store) {
            Err(super::IoError::Tile(_)) => {}
            other => unreachable!("expected a tile decode failure, got {other:?}"),
        }

        // The whole point: nothing the failed read committed survives,
        // on either surface it touched. `contains_tile` is the right
        // question rather than reading pixels back -- `get` would
        // materialize a blank tile and answer "clean" whether the
        // rollback happened or not.
        assert!(
            !store.contains_tile(content_surface, TileId { x: 0, y: 0 }),
            "the content tile committed before the failure must not stay resident"
        );
        assert!(
            !store.contains_tile(mask_surface, TileId { x: 0, y: 0 }),
            "no mask tile can survive a read that failed on one"
        );
        assert_eq!(
            store.resident_len(),
            0,
            "a failed read must leave the store as empty as it found it"
        );
    }

    #[test]
    fn a_successful_read_still_commits_its_tiles() {
        // The other side of the rollback: it must fire on failure only.
        // Same container as the test above, with a *valid* mask entry
        // in place of the corrupt one.
        let mut layers = LayerTree::new();
        let id = match layers.add_pixel_layer("masked", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.add_mask(id, mask_bounds()) {
            unreachable!("{err:?}");
        }
        let (Some(content_surface), Some(mask_surface)) =
            (layers.surface_id(id), layers.mask_surface_id(id))
        else {
            unreachable!("a pixel layer with a mask has both surfaces");
        };
        let manifest = ManifestReadForTest {
            version: super::MANIFEST_VERSION,
            canvas_width: 1000,
            canvas_height: 1000,
            color_space: super::ColorSpaceTag::Srgb,
            layers,
        };
        let manifest_bytes = match postcard::to_allocvec(&manifest) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        let container = container_with(
            &manifest_bytes,
            &[
                (
                    super::tile_entry_name(content_surface, TileId { x: 0, y: 0 }),
                    encoded_tile(0.75),
                ),
                (
                    super::tile_entry_name(mask_surface, TileId { x: 0, y: 0 }),
                    encoded_tile(0.25),
                ),
            ],
        );

        let (_dir, mut store) = real_tile_store();
        if let Err(err) = read(container, &mut store) {
            unreachable!("a valid container must still read: {err:?}");
        }
        assert!(
            store.contains_tile(content_surface, TileId { x: 0, y: 0 }),
            "a successful read must leave its content tile in the store"
        );
        assert!(
            store.contains_tile(mask_surface, TileId { x: 0, y: 0 }),
            "a successful read must leave its mask tile in the store"
        );
    }

    /// The headline test for 0.74.0: a tile a best-effort write had to
    /// drop must still be *known to be missing* after the process that
    /// dropped it is gone.
    ///
    /// Before this, the only two signals were session-local — a
    /// `tracing::warn!` line and `aurora-app`'s own `.partial` autosave
    /// filename — and the container itself was silent, because a
    /// dropped tile and a never-painted one are both "no entry for this
    /// tile". So this reads back into a *completely fresh* store, which
    /// is what a new process has, and asserts the file itself carries
    /// the loss.
    #[test]
    fn a_skipped_tile_survives_as_skipped_across_a_fresh_read() {
        let (dir, mut store) = one_tile_store();
        let mut layers = LayerTree::new();
        let mut history = History::new();
        let mut surfaces = Vec::new();
        for name in ["broken", "intact"] {
            let id = match history.add_pixel_layer(&mut layers, name, bounds(), None) {
                Ok(id) => id,
                Err(err) => unreachable!("{err:?}"),
            };
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            let tile = match store.get_mut(surface, TileId { x: 0, y: 0 }) {
                Ok(tile) => tile,
                Err(err) => unreachable!("{err:?}"),
            };
            if let Some(sample) = tile.texels_mut().first_mut() {
                *sample = f16::from_f32(0.5);
            }
            surfaces.push(surface);
        }
        if let Err(err) = store.flush() {
            unreachable!("test-local scratch disk must accept the write: {err:?}");
        }
        break_the_only_scratch_file(&dir);

        let mut salvaged = Cursor::new(Vec::new());
        let skipped =
            match write_best_effort(&mut salvaged, &layers, &history, (10, 10), None, &mut store) {
                Ok(skipped) => skipped,
                Err(err) => unreachable!("a best-effort write must still produce a file: {err:?}"),
            };
        let [in_memory] = skipped.as_slice() else {
            unreachable!("exactly one tile is unreadable, got {skipped:?}");
        };

        // A fresh store, standing in for a fresh process: nothing here
        // knows what the writer knew.
        let (_fresh_dir, mut fresh_store) = real_tile_store();
        salvaged.set_position(0);
        let document = match read(salvaged, &mut fresh_store) {
            Ok(document) => document,
            Err(err) => unreachable!("the salvaged autosave must reopen: {err:?}"),
        };
        let [persisted] = document.skipped_tiles.as_slice() else {
            unreachable!(
                "the reopened document must name the dropped tile, got {:?}",
                document.skipped_tiles
            );
        };
        // The persisted record and the in-memory one describe the same
        // loss -- surface, tile, and reason all round-trip.
        assert_eq!(persisted.surface, in_memory.surface.to_raw());
        assert_eq!(persisted.tile, in_memory.tile);
        assert_eq!(persisted.reason, in_memory.reason);
        assert!(
            persisted.reason.contains("corrupt tile file"),
            "the persisted skip must carry the real underlying tile error: {}",
            persisted.reason
        );
        let Some(&broken_surface) = surfaces.first() else {
            unreachable!("two layers were just created");
        };
        assert_eq!(persisted.surface, broken_surface.to_raw());
    }

    /// The backward-compatibility proof, and the reason the skip list is
    /// a separate ZIP entry rather than a manifest field (see this
    /// module's own doc comment): `container_with` writes only
    /// `manifest` and `history`, which is exactly the shape of every
    /// `.aur` file and every crash-recovery autosave written before
    /// 0.74.0. Absence must read as "nothing was dropped", not as an
    /// error and not as an unknown.
    #[test]
    fn a_container_without_the_skipped_tiles_entry_reads_as_none_skipped() {
        let mut layers = LayerTree::new();
        if let Err(err) = layers.add_pixel_layer("old", bounds(), None) {
            unreachable!("{err:?}");
        }
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
        let (_dir, mut store) = real_tile_store();
        let document = match read(container_with(&manifest_bytes, &[]), &mut store) {
            Ok(document) => document,
            Err(err) => unreachable!("a pre-0.74.0 container must still open: {err:?}"),
        };
        assert!(
            document.skipped_tiles.is_empty(),
            "a container with no skipped-tiles entry lost nothing, got {:?}",
            document.skipped_tiles
        );
    }

    /// The other half of the same contract, pinned from the *write*
    /// side: an ordinary `write()` -- the `Refuse` policy, and so every
    /// user-facing save -- must produce exactly the bytes it produced
    /// before this entry existed. The `is_empty()` guard in
    /// `write_with_policy` is what makes that true, and this is the test
    /// that would fail if someone removed it as a micro-optimization.
    #[test]
    fn an_ordinary_write_carries_no_skipped_tiles_entry() {
        let (_dir, mut store) = real_tile_store();
        let mut layers = LayerTree::new();
        let mut history = History::new();
        let id = match history.add_pixel_layer(&mut layers, "Background", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(surface) = layers.surface_id(id) else {
            unreachable!("just created as a pixel layer");
        };
        {
            let tile = match store.get_mut(surface, TileId { x: 0, y: 0 }) {
                Ok(tile) => tile,
                Err(err) => unreachable!("{err:?}"),
            };
            if let Some(sample) = tile.texels_mut().first_mut() {
                *sample = f16::from_f32(0.5);
            }
        }
        let mut bytes = Cursor::new(Vec::new());
        if let Err(err) = write(&mut bytes, &layers, &history, (10, 10), None, &mut store) {
            unreachable!("a healthy document must save: {err:?}");
        }
        bytes.set_position(0);
        let mut archive = match zip::ZipArchive::new(bytes) {
            Ok(archive) => archive,
            Err(err) => unreachable!("{err:?}"),
        };
        match archive.by_name(super::SKIPPED_TILES_ENTRY) {
            Err(zip::result::ZipError::FileNotFound) => {}
            Ok(_) => unreachable!("an ordinary save must not add a skipped-tiles entry"),
            Err(other) => unreachable!("expected FileNotFound, got {other:?}"),
        }
    }

    /// `skipped-tiles` is read out of a file that may be crafted or
    /// corrupt, so both of its failure shapes get a real answer rather
    /// than a panic or an unbounded allocation: too many records is
    /// truncated to `MAX_SKIPPED_TILE_RECORDS`, and bytes that are not a
    /// record list at all become `IoError::ManifestDeserialization`.
    #[test]
    fn a_hostile_skipped_tiles_entry_is_bounded() {
        let mut layers = LayerTree::new();
        if let Err(err) = layers.add_pixel_layer("small", bounds(), None) {
            unreachable!("{err:?}");
        }
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

        let overlong: Vec<super::SkippedTileRecord> = (0..super::MAX_SKIPPED_TILE_RECORDS + 500)
            .map(|i| super::SkippedTileRecord {
                surface: i as u64,
                tile: TileId { x: 0, y: 0 },
                reason: "crafted".to_owned(),
            })
            .collect();
        let overlong_bytes = match postcard::to_allocvec(&overlong) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        let (_dir, mut store) = real_tile_store();
        let document = match read(
            container_with(
                &manifest_bytes,
                &[(super::SKIPPED_TILES_ENTRY.to_owned(), overlong_bytes)],
            ),
            &mut store,
        ) {
            Ok(document) => document,
            Err(err) => {
                unreachable!("an over-long skip list must be bounded, not refused: {err:?}")
            }
        };
        assert_eq!(
            document.skipped_tiles.len(),
            super::MAX_SKIPPED_TILE_RECORDS,
            "the skip list must be truncated to its own bound"
        );

        let (_garbage_dir, mut garbage_store) = real_tile_store();
        match read(
            container_with(
                &manifest_bytes,
                &[(super::SKIPPED_TILES_ENTRY.to_owned(), vec![0xff_u8; 64])],
            ),
            &mut garbage_store,
        ) {
            Err(super::IoError::ManifestDeserialization(_)) => {}
            other => unreachable!("expected ManifestDeserialization, got {other:?}"),
        }
    }

    #[derive(serde::Serialize)]
    struct ManifestReadForTest {
        version: u32,
        canvas_width: u32,
        canvas_height: u32,
        color_space: super::ColorSpaceTag,
        layers: LayerTree,
    }
}
