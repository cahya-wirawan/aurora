# The `.aur` document file format

**Status:** descriptive specification of the format as implemented by Aurora **0.244.0** (`main` at commit `80a8f00`, 2026-10-15). It is derived from the source code, which is authoritative; where this document, [ADR 0009](adr/0009-aur-document-format.md) and the code disagree, the code wins and the disagreement is listed in [section 12](#12-known-gaps-and-ambiguities).

**Scope:** the on-disk bytes of a `.aur` file and the rules a reader or writer must follow. It does not describe the in-memory model, rendering, or the user interface, except where they decide what the bytes mean.

**Provenance rule.** Every factual statement below was taken from the code or tests named beside it (file path and function or constant; line numbers are for commit `80a8f00` and are given only where they are likely to stay close). Things that could not be found in the code are not stated; they are listed under Known gaps. Byte sequences in [section 11](#11-worked-example) were **derived by hand from the encoding rules** and cross-checked with a throwaway Python encoder; they were **not** produced by running Aurora.

Paths below are relative to the repository root. `aur.rs` means `crates/aurora-io/src/aur.rs`.

## Table of contents

1. [Purpose, versions, conformance language](#1-purpose-versions-conformance-language)
2. [Primitive encodings (`postcard`)](#2-primitive-encodings-postcard)
3. [The container](#3-the-container)
4. [The manifest](#4-the-manifest)
5. [The history journal](#5-the-history-journal)
6. [Pixel data: surfaces, tiles and the tile codec](#6-pixel-data-surfaces-tiles-and-the-tile-codec)
7. [The layer model on disk](#7-the-layer-model-on-disk)
8. [Optional entries in detail](#8-optional-entries-in-detail)
9. [Safety and robustness](#9-safety-and-robustness)
10. [Compatibility rules and introduction table](#10-compatibility-rules-and-introduction-table)
11. [Worked example](#11-worked-example)
12. [Known gaps and ambiguities](#12-known-gaps-and-ambiguities)
13. [Appendices](#13-appendices)

---

## 1. Purpose, versions, conformance language

### 1.1 Purpose

`.aur` is Aurora's native, lossless document format. One file holds: the canvas size, the colour space, the whole layer tree (pixel layers, groups, adjustment layers, their properties, masks, effects, locks, clipping, preserved Photoshop smart objects), the document's edit journal, every non-blank pixel tile (layer content and mask coverage), and optionally selections, saved selections, Bezier paths and some provenance notes. The same container, written by a background thread, is also Aurora's crash-recovery autosave (section 9.6).

Design intent (ADR 0009): an open format that can be read by any implementation that has a ZIP library and a decoder for the `postcard` wire format, with an off-the-shelf LZ4 block decompressor for tile payloads.

### 1.2 Format and schema versions

There is **no single global file-format version number**. Versioning is layered:

| Level | Where | Current value | Source |
|---|---|---|---|
| Container | ZIP archive; no version field | n/a | `aur.rs` module doc |
| Manifest schema | first field of the `manifest` entry | `1` (`MANIFEST_VERSION`) | `aur.rs:881` |
| Per-entry schema versions | first field of each optional versioned entry | all `1` (see table below) | per-entry constants |
| Tile codec | byte 4 of every tile payload | `1` (`VERSION` in `codec.rs`) | `crates/aurora-tile/src/codec.rs` |
| Enum ordinals | `postcard` variant indices inside the manifest and journal | see Appendix B | `layer.rs`, `history.rs` |

Per-entry schema versions: `mask-density` 1 (`MASK_DENSITY_VERSION`), `layer-effects` 1 (`LAYER_EFFECTS_VERSION`), `layer-clipping` 1 (`LAYER_CLIPPING_VERSION`), `smart-objects` 1 (`SMART_OBJECTS_VERSION`), `import-losses` 1 (`IMPORT_LOSSES_VERSION`), `skipped-tiles` 1 (`SKIPPED_TILES_VERSION`), `selection` 1 (`SELECTION_VERSION`, `aur_selection.rs`), `paths` 1 (`PATHS_VERSION`, `aur_paths.rs`).

The format has **evolved without bumping the manifest version**: every change since the first release was made by (a) adding an optional top-level ZIP entry, or (b) appending an enum variant (section 10). `MANIFEST_VERSION` has never changed from 1, and a reader that meets any other value refuses the file (`aur.rs` `read`, "unsupported manifest version").

### 1.3 Conformance language

The key words MUST, MUST NOT, SHOULD, SHOULD NOT and MAY are used as in RFC 2119. Because there is no independent standards body, "MUST" here means "the reference implementation (`aurora-io`) requires it, or a file that violates it is refused or misread by the reference reader". "SHOULD" marks behaviour of the reference writer that other writers are advised to copy so that the reference reader and older Aurora versions behave sensibly.

Two conformance roles are used: a **reader** (opens an existing file) and a **writer** (produces a file the reference reader accepts).

### 1.4 Endianness and units

* `postcard` integers are variable-length (LEB128), so endianness is not relevant to them. `f32` values inside `postcard` are IEEE-754 binary32, **little-endian** (section 2).
* Tile payloads store `f16` (IEEE-754 binary16) samples **little-endian**, and the LZ4 size prefix is a **little-endian** `u32` (`codec.rs`, `raw_bytes`, `decode`).
* The `selection` entry's mask blobs are single bytes, no endianness.
* Document coordinates are signed 64-bit integers (`Rect.x`, `Rect.y`), extents are unsigned 32-bit.
* The framed smart-object file entries use **big-endian** `u64`/lengths because they are verbatim PSD structures (section 8.5).

---

## 2. Primitive encodings (`postcard`)

All structured metadata (manifest, journal and every versioned optional entry except the stored smart-object file blobs and tile payloads) is encoded with the `postcard` crate, version 1.1.3 as locked in `Cargo.lock`, features `use-std` only (`Cargo.toml:57`), via `serde` derive. Nothing in the repository documents the wire format beyond tests; the rules below are the standard `postcard` 1.x wire format as used by the code, and each is pinned by a field layout that the repo's own tests re-create (for example the `mirror` structs in `aur.rs` tests, line ~3560, which "match `LayerTree`'s own derived `Serialize` field-for-field").

### 2.1 Primitive mapping

| Rust / serde type | Wire encoding |
|---|---|
| `bool` | one byte, `0x00` or `0x01` |
| `u8`, `i8` | one raw byte (not varint) |
| `u16`, `u32`, `u64`, `usize` | unsigned LEB128 varint: 7 data bits per byte, least-significant group first, high bit set on every byte except the last. `usize` is encoded as `u64`. Max length 3 / 5 / 10 / 10 bytes |
| `i16`, `i32`, `i64` | zig-zag (`(n << 1) ^ (n >> (bits-1))`) then unsigned varint of the same width |
| `f32` | 4 bytes, IEEE-754 binary32, little-endian |
| `f64` | 8 bytes, little-endian (not used by `.aur` structures) |
| `String` / `&str` | varint byte length, then that many UTF-8 bytes |
| `Vec<T>` / slice | varint element count, then the elements back to back |
| `[T; N]` (fixed array) | the `N` elements back to back, **no length prefix** (serde treats arrays as tuples) |
| `Option<T>` | `0x00` for `None`; `0x01` then the value for `Some` |
| tuple, tuple struct | the fields back to back, no framing |
| struct | the fields in declaration order, no names, no framing, no padding |
| unit (`()`), unit struct | zero bytes |
| newtype struct | transparent: the inner value |
| enum | the variant index as a varint `u32`, then the variant's payload (nothing for a unit variant; the fields in order otherwise) |
| `HashMap<K, V>` | varint entry count, then `(key, value)` pairs, **in the writer's iteration order** (unspecified, see 12) |
| `Box<T>` | transparent |

Types with custom serde: `aurora_core::Id<T>` (`LayerId`, `SurfaceId`) is a `u64` varint; `IdGenerator<T>` is a `u64` varint (the next id to hand out) (`crates/aurora-core/src/id.rs`); `ToneCurve` is a sequence of `(f32, f32)` pairs (a `Vec`, so a varint count first) (`tone_curve.rs` `impl Serialize`); several adjustment parameter types are serialized through a tuple (`#[serde(try_from, into)]`) so they appear as plain tuples on the wire (section 7.8).

### 2.2 `postcard` is positional

There are no field names, tags, lengths for structs, or defaults on the wire. A struct is decoded strictly by position from a bare byte stream. Consequences, all load-bearing for the compatibility rules:

* **A struct cannot grow.** Appending a field to a serialized struct makes the decoder run off the end of old data (`DeserializeUnexpectedEnd`). `#[serde(default)]` does not help, because there are no names to omit. This is pinned by the test `postcard_really_is_positional_so_a_trailing_field_breaks_old_bytes` (`aur.rs:6937`).
* **Enum variants can only be appended.** An enum is its ordinal; inserting a variant renumbers the later ones. An older reader meeting an unknown ordinal fails to decode the whole value.
* **Fields that must be added later travel elsewhere.** The code marks such fields `#[serde(skip)]` in the manifest/journal structs (`LayerMask::density`, `LayerEntry::effects`, `::smart_object`, `::clipped`, `LayerTree::linked_files`) and carries them in separate optional ZIP entries and in appended journal ops (sections 5 and 8).
* **Trailing bytes.** `postcard::from_bytes` is used to decode every versioned entry; it returns the value and discards any remaining bytes. Whether a reader must reject trailing bytes is not specified by the code (section 12, gap G-9).

### 2.3 Floating-point rules

Writers MUST NOT write `NaN` or infinities into any field that is range-validated (all adjustment parameters, opacities, effect parameters, mask density, path coordinates), because the reference reader rejects them (`is_finite` and `contains` checks, which are false for `NaN`).

---

## 3. The container

### 3.1 Format

A `.aur` file is a ZIP archive (the `zip` crate 2.4.2, `default-features = false, features = ["deflate"]`, `Cargo.toml:201`). Only two compression methods are used: **Stored** (method 0) and **Deflated** (method 8). No encryption, bzip2, lzma, zstd or ppmd. A reader MUST support Stored and Deflated; it need not support anything else (the reference reader cannot).

Files are recognised by extension (`.aur`, case-insensitive; `is_aur_path`, `crates/aurora-app/src/lib.rs:969`). The reference reader does **not** check the `mimetype` entry on open (gap G-3).

### 3.2 Entry table

All entry names are exact, case-sensitive ASCII strings, with `/` as separator and no leading slash. "Req" is required for a file to open.

| Entry name | Req? | Compression | Content | Written when | Reader cap (uncompressed) | Source |
|---|---|---|---|---|---|---|
| `mimetype` | no (not read) | Stored | ASCII `application/vnd.aurora.document`, no trailing newline | always, first | n/a (never read) | `MIME_ENTRY`, `MIME_TYPE`, `aur.rs:256-257` |
| `manifest` | **yes** | Deflated | `postcard` `Manifest` (section 4) | always | 64 MiB | `MANIFEST_ENTRY` |
| `history` | **yes** | Deflated | `postcard` `Vec<LayerOp>` (section 5) | always | 64 MiB | `HISTORY_ENTRY` |
| `mask-density` | no | Deflated | `postcard` `{version: u32, densities: Vec<(u64, f32)>}` (8.2) | some mask has density < 1.0 | 64 MiB | `MASK_DENSITY_ENTRY` |
| `layer-effects` | no | Deflated | `postcard` `{version: u32, effects: Vec<(u64, LayerEffects)>}` (7.6) | some layer has effects | 16 MiB | `LAYER_EFFECTS_ENTRY` |
| `layer-clipping` | no | Deflated | `postcard` `{version: u32, clipped: Vec<u64>}` (7.5) | some layer is clipped to a base | 16 MiB | `LAYER_CLIPPING_ENTRY` |
| `smart-objects` | no | Deflated | `postcard` `{version: u32, objects, files}` (8.5) | document has smart-object data or embedded files | 80 MiB | `SMART_OBJECTS_ENTRY` |
| `smart-object-file-<n>` | only if named by `smart-objects` | **Stored** | one PSD linked-layer entry, framed (8.5) | one per record in `smart-objects.files`, `n` = 0-based index in that list | the record's `len` | `SMART_OBJECT_FILE_PREFIX` |
| `import-losses` | no | Deflated | `postcard` `{version: u32, lines: Vec<String>}` (8.6) | the document was opened from another format with losses | 1,048,640 bytes | `IMPORT_LOSSES_ENTRY` |
| `selection` | no | Deflated | `postcard` header + raw mask blobs (8.7) | active or saved selection exists | 201,392,128 bytes | `aur_selection.rs` `SELECTION_ENTRY`, `MAX_SELECTION_ENTRY_BYTES` |
| `paths` | no | Deflated | `postcard` paths header (8.8) | document has a saved path or a work path | 5,262,144 bytes | `aur_paths.rs` `PATHS_ENTRY`, `MAX_PATHS_ENTRY_BYTES` |
| `tiles/<surface>/<x>_<y>.tile` | no | **Stored** | one tile codec payload (section 6.4) | per non-blank tile | 589,824 bytes | `tile_entry_name`, `MAX_TILE_ENTRY_BYTES` |
| `skipped-tiles` | no | Deflated | `postcard` `{version: u32, total: u64, records: Vec<SkippedTileRecord>}` (8.1) | the writer left out unreadable tiles, or the document already knew it had lost some | 2,199,616 bytes | `SKIPPED_TILES_ENTRY`, `MAX_SKIPPED_TILES_ENTRY_BYTES` |

`<surface>` is the decimal `u64` surface id; `<x>` and `<y>` are decimal tile column/row, no zero padding, joined by `_` (`tile_entry_name`: `format!("tiles/{}/{}_{}.tile", surface.to_raw(), id.x, id.y)`).

Unknown entry names (including directory entries) are ignored by the reference reader: each entry is looked up by exact name, and nothing enumerates the archive (the test-suite confirms an extra unknown entry is tolerated: `an_older_reader_and_an_unknown_extra_entry_both_open` in `aur_paths_tests.rs`, `an_old_file_and_an_unknown_extra_entry_both_open` in `aur_selection_tests.rs`).

### 3.3 Ordering requirements

* The writer emits entries in this order: `mimetype`, `manifest`, `history`, `mask-density`, `layer-effects`, `layer-clipping`, `smart-objects` followed by its `smart-object-file-<n>` entries in index order, `import-losses`, `selection`, `paths`, then all tile entries, then `skipped-tiles` (`DocumentEntries::write_to`, then the tile loop in `write_with_policy` and `write_snapshot`, then `skipped_tiles_entry`, all in `aur.rs`). The snapshot writer produces the same order.
* `mimetype` SHOULD be first and Stored so that a magic-byte sniff works without parsing the central directory (ADR 0009; the app test at `crates/aurora-app/src/lib.rs:67594` asserts the entry exists and is Stored). No reader code depends on the position.
* **A reader MUST NOT rely on entry order**; it looks every entry up by name through the central directory.
* Tile entries are written in `persisted_surfaces` order (section 6.3): layers in depth-first pre-order of the tree (siblings in stored order), a pixel layer's content surface before its mask surface, within a surface row-major (rows outer, columns inner). This order is not significant to a reader.

### 3.4 Reader algorithm and failure policy

`aurora_io::aur::read` (`aur.rs:2300`) performs the following steps, in this order. Any step that says "refuse" aborts the open with an `IoError` and, for the tile step, rolls back (9.4).

1. Open the ZIP central directory (`ZipArchive::new`); a container failure is `IoError::Zip`/`Io`.
2. Read `manifest` (required; `MissingEntry("manifest")` if absent), capped at 64 MiB. Decode `Manifest` (4.1). Undecodable: refuse (`ManifestDeserialization`). `version != 1`: refuse.
3. Colour space: `Srgb` -> none; `Icc(bytes)` -> parse with `IccProfile::from_bytes`; parse failure: refuse (`IoError::Color`).
4. Canvas size: either dimension above `MAX_DOCUMENT_EXTENT` (300,000): refuse (`CanvasTooLarge`). Zero is not checked (G-6).
5. `validate_persisted_rects`: every pixel-layer `bounds` and every mask `bounds`: origin within +/-300,000 (`LayerOriginOutOfRange`), extent <= 300,000 (`LayerBoundsTooLarge`), sum of tile-grid areas <= 2,747,168 (`TooManyTiles`). Refuse on violation.
6. Apply optional side entries to the decoded tree, in this order: `mask-density`, `layer-effects`, `layer-clipping`, `smart-objects` (plus its files). For each, **absence is fine**; any other failure refuses (table below).
7. Read `history` (required) and decode with `History::load_journal`. Undecodable: refuse (`IoError::Doc`). The journal is **not replayed and not compared with the manifest** on open (G-1).
8. `skipped-tiles`, `import-losses`, `selection`, `paths` (policies below).
9. Scan the tile grid of every persisted surface (6.3), decoding each present tile entry into the caller's tile store. Missing entries are blank tiles (6.5).

Per-entry policy for an entry that is absent, malformed, oversized, or of an unexpected version:

| Entry | Absent | Over cap | Container/IO error reading it | Does not decode / fails validation | Version != understood |
|---|---|---|---|---|---|
| `manifest` | refuse (`MissingEntry`) | refuse (`EntryTooLarge`) | refuse | refuse | any value != 1: refuse |
| `history` | refuse (`MissingEntry`) | refuse | refuse | refuse (no version field) | no version field |
| `mask-density` | ok (all masks density 1.0) | refuse | refuse | refuse (also: a pair naming a layer with no mask, or density outside 0..=1) | refuse (any value != 1, smaller or larger) |
| `layer-effects` | ok | refuse | refuse | refuse (also: unknown layer, adjustment layer, `LayerEffects::validate` failure) | refuse (message distinguishes newer from damaged) |
| `layer-clipping` | ok | refuse | refuse | refuse (also: unknown layer, a group, a layer with nothing beneath it) | refuse |
| `smart-objects` | ok | refuse | refuse | refuse (also: object on missing or non-pixel layer, validation failure, missing/damaged/oversized embedded file, pool bounds) | refuse |
| `skipped-tiles` | ok (nothing skipped) | refuse | refuse | refuse | **keep the `total` count, drop the record detail, do not fail** |
| `import-losses` | `None` | one generic line, never fails | one generic line | one generic line | one generic line |
| `selection` | no selection | **drop selection, keep file** | drop, keep file (a failure to *locate* it that is not "not found" refuses) | drop, keep file | `> 1`: **refuse**; `0` or other: drop, keep file |
| `paths` | no paths | drop, keep file | drop, keep file (same locate caveat) | drop, keep file | `> 1`: **refuse**; `0`: drop, keep file |
| tile entry | blank tile | refuse (`EntryTooLarge`) | refuse | refuse (`IoError::Tile`) | tile codec version != 1: refuse |

(`import-losses`: `read_import_losses`, `aur.rs:721`. `selection`: `decode_selection`/`read_selection`. `paths`: `decode_paths`/`read_paths`. A `ZipError` other than "file not found" from the name lookup of `selection`/`paths` is returned as an error, i.e. refuses the open.)

A **reader MUST NOT** treat an optional entry's absence as an error. A **reader SHOULD** ignore unknown ZIP entry names. A reader that does not implement an optional entry MUST understand that opening and re-saving then loses that entry's content (and, for several entries, that it will also be unable to decode the journal; see 10.3).

### 3.5 Writer behaviour

`write`, `write_best_effort` and their `_with_import_losses` variants share `write_with_policy` (`aur.rs:1643`):

1. `validate_persisted_rects` runs **before the first byte is written** (a refusal leaves nothing at the destination).
2. `DocumentEntries::build` serialises the manifest, the journal and the optional side entries; selection and paths are encoded when written.
3. Entries are written in the order of 3.3. `mimetype` and tile entries and embedded smart-object files use Stored; all others use Deflated.
4. For every persisted surface, for every grid position: positions the store has never held are skipped without being materialised; an all-zero tile (bit pattern `0x0000` or `0x8000` in every sample, `is_blank`) is skipped; otherwise `aurora_tile::codec::encode` is called and its bytes written verbatim as a Stored entry.
5. An unreadable tile fails the whole write under the `write` policy (`UnreadableTile::Refuse`); under `write_best_effort` (autosave only) it is left out and returned in the skip list.
6. `skipped-tiles` is written last, only when the union of what the document already knew it had lost and what this write lost is non-empty. An ordinary save of an undamaged document writes **no** `skipped-tiles` entry.

---

## 4. The manifest

### 4.1 Layout

`Manifest` (`ManifestWrite`/`ManifestRead`, `aur.rs:1084-1105`) is a `postcard` struct with these fields in this exact order:

| # | Field | Type | Notes |
|---|---|---|---|
| 1 | `version` | `u32` | MUST be `1` |
| 2 | `canvas_width` | `u32` | pixels; must be <= 300,000 (`MAX_DOCUMENT_EXTENT`) |
| 3 | `canvas_height` | `u32` | same |
| 4 | `color_space` | `ColorSpaceTag` | enum, 4.2 |
| 5 | `layers` | `LayerTree` | 4.3 |

### 4.2 Colour space

`ColorSpaceTag` (`aur.rs:1111`): variant `0` = `Srgb` (no payload), variant `1` = `Icc(Vec<u8>)` (varint length, then the raw bytes of an ICC profile as produced by `IccProfile::to_bytes`). `Srgb` MUST remain ordinal 0. A writer with no profile writes `Srgb`. In the shipped application every caller passes no profile, so `Icc` is currently exercised by tests only (`round_trips_a_real_non_srgb_icc_profile`, `aur.rs:3118`; doc comment in `aur.rs`).

Colour values in the pixel tiles are expressed in this colour space. The code never converts them on read or write. The adjustment filters treat tile RGB as "straight encoded sRGB" (e.g. `HueSaturationOp` per the ADR addendum); no field on disk records a linear/encoded distinction.

### 4.3 `LayerTree`

`LayerTree` (`crates/aurora-doc/src/tree.rs:179`) is deserialised through `LayerTreeRepr` and validated before use. Wire fields in order:

| # | Field | Type | Meaning |
|---|---|---|---|
| 1 | `ids` | `u64` | the id generator's *next* id (not yet used by any layer) |
| 2 | `layers` | map `LayerId (u64)` -> `LayerEntry` | every layer, groups included, in unspecified order |
| 3 | `roots` | `Vec<u64>` | root-level layer ids, **top-to-bottom** (index 0 is the topmost layer, painted last) |

`LayerTree::linked_files` is `#[serde(skip)]` (section 8.5).

`LayerId`s are tree-local 64-bit integers. Ids MUST be `< 2^63` (`MASK_SURFACE_BIT`); an id with bit 63 set, or a counter value >= `2^63`, is refused (`validate_layer_id_range`, `tree.rs:514`). `ids` MUST be strictly greater than every id present (`validate_id_allocator`, `tree.rs:465`).

### 4.4 `LayerEntry`

`LayerEntry` (`crates/aurora-doc/src/layer.rs:568`), fields in order:

| # | Field | Type | Validation by reader |
|---|---|---|---|
| 1 | `name` | `String` | none on the manifest path (G-7) |
| 2 | `parent` | `Option<u64>` | must equal where the tree walk found the entry (`None` for roots) |
| 3 | `kind` | `LayerKind` | enum below |
| 4 | `opacity` | `f32` | `0.0..=1.0`, finite |
| 5 | `fill_opacity` | `f32` | `0.0..=1.0`, finite |
| 6 | `blend_mode` | `BlendMode` | enum (7.1); `PassThrough` only on groups |
| 7 | `visible` | `bool` | |
| 8 | `lock` | `LayerLock` | three `bool`s: `transparency`, `pixels`, `position` (in that order) |
| 9 | `mask` | `Option<LayerMask>` | 7.4 |

Skipped on the wire (carried by other entries): `effects`, `smart_object`, `clipped`.

`LayerKind` (`layer.rs:28`), variant ordinals: `0` `Pixel { bounds: Rect }`; `1` `Group { children: Vec<u64> }` (children top-to-bottom); `2` `Adjustment(Adjustment)`. `Adjustment` is the enum of 7.8 (ordinals 0..=12).

`Rect` (`crates/aurora-core/src/geometry.rs:129`): `x: i64`, `y: i64` (zig-zag varints), `width: u32`, `height: u32` (varints).

### 4.5 Validation applied while decoding the manifest

`LayerTree`'s `Deserialize` is `try_from = "LayerTreeRepr"` and runs, in order (`tree.rs` `TryFrom<LayerTreeRepr>`):

1. `validate_shape`: every id named by `roots` or a group's `children` has an entry (else `DanglingLayerReference`); each entry is reached exactly once (`MalformedLayerTree` on a cycle or a second parent); each entry's recorded `parent` equals where it was found (`InconsistentLayerParent`); every entry is reachable (`OrphanedLayer`); nesting depth <= `MAX_LAYER_TREE_DEPTH` = 256 (`LayerTreeTooDeep`). Iterative, not recursive.
2. `validate_id_allocator`, 3. `validate_layer_id_range` (above), 4. `validate_opacities`, 5. `validate_blend_modes` (`PassThrough` on a non-group is `GroupOnlyBlendMode`).

Then `aurora-io` adds the rectangle checks of 3.4 step 5 (mask and layer bounds origin and extent, total tile budget).

Not validated on this path: layer name content or length (G-7); that an adjustment layer carries no mask or children; duplicate keys in the `layers` map (serde inserts; the later wins, G-8); that `canvas_*` is non-zero (G-6).

---

## 5. The history journal

### 5.1 What it is

`history` is `postcard` of `Vec<LayerOp>`: `History::save_journal` (`history.rs:884`) serialises the **journal**, an ever-growing chronological log of every operation applied to the tree (adds, edits, undos and redos alike), not the undo/redo stacks. `History::load_journal` (`history.rs:943`) restores it with empty undo/redo stacks. Replaying the journal from an empty `LayerTree` (`History::replay`, `history.rs:840`) reconstructs the *current* state.

Reader semantics: `read` only decodes the journal. **It does not replay it and does not check it against the manifest** (the live document is the manifest's tree, G-1). The journal is used by the application for descriptions and crash recovery logic, and by tests as a consistency check (`round_trips_*` tests call `History::replay` and compare with the manifest tree).

### 5.2 `LayerOp` ordinals

`LayerOp` (`history.rs:59`) is an enum; the ordinal is the declaration position. **New variants are only ever appended.**

| Ord | Variant | Payload (wire order) | Introduced |
|---|---|---|---|
| 0 | `RemoveById` | `LayerId (u64)` | original format (pre-0.1) |
| 1 | `Restore` | `RemovedSubtree { root: u64, parent: Option<u64>, index: usize, entries: Vec<(u64, LayerEntry)> }` | original |
| 2 | `Reparent` | `id: u64, parent: Option<u64>, index: usize` | original |
| 3 | `Rename` | `id: u64, name: String` | original |
| 4 | `SetOpacity` | `id: u64, value: f32` | original |
| 5 | `SetFillOpacity` | `id: u64, value: f32` | original |
| 6 | `SetBlendMode` | `id: u64, value: BlendMode` | original |
| 7 | `SetVisible` | `id: u64, value: bool` | original |
| 8 | `SetLock` | `id: u64, value: LayerLock` | original |
| 9 | `RemoveMask` | `u64` | original |
| 10 | `RestoreMask` | `(u64, LayerMask)` | original |
| 11 | `SetMaskEnabled` | `id: u64, value: bool` | original |
| 12 | `SetMaskInverted` | `id: u64, value: bool` | original |
| 13 | `SetBounds` | `id: u64, value: Rect` | after the original set (version not determined from the sources read) |
| 14 | `SetMaskDensity` | `id: u64, value: f32` | 0.149.0 |
| 15 | `SetAdjustment` | `id: u64, value: Adjustment` | 0.155.0 |
| 16 | `SetEffects` | `id: u64, value: Option<LayerEffects>` | 0.180.0 |
| 17 | `SetSmartObject` | `id: u64, value: Option<SmartObject>` | 0.200.0 |
| 18 | `SetClipped` | `id: u64, value: bool` | 0.201.0 |
| 19 | `Translate` | `dx: i64, dy: i64` | 0.226.0 |

Ordinals 17 and 18 are stated by ADR 0009 and match the declaration order. "Original" means present in the first implementation (`ade24bc`, 2026-08-06) or at least before the earliest dated addendum; the exact version for ordinals 0 to 13 was not determined, and the ADR does not date `SetBounds` (G-10).

Semantics of ops that the wire alone does not convey:

* Adding a layer is journaled as `Restore` with a one-entry `RemovedSubtree` (`History::add_pixel_layer`, `add_group`), not as a dedicated op. `index` is the sibling position the layer landed at (0 = top).
* `Translate { dx, dy }` shifts every pixel layer's `bounds` and every layer's and group's mask `bounds` by `(dx, dy)`; tile contents are layer- and mask-local and are not touched. It is the Crop operation: a crop to `(x, y, w, h)` records `Translate { -x, -y }` and sets the canvas size (which lives in the manifest, not the journal). The inverse is the negated op (`tree.translate_all`, `history.rs:449`). A shift that would leave a layer origin outside +/-300,000 is refused.
* `SetEffects`, `SetSmartObject`, `SetClipped` and `SetMaskDensity` also act as **carriers**: `LayerEntry::effects`, `::smart_object`, `::clipped` and `LayerMask::density` are not on the wire inside `Restore`/`RestoreMask`, so `save_journal` writes an extra op immediately after each journaled `Restore`/`RestoreMask` whose entries hold such a value (`reduced_mask_densities`, `restored_effects`, `restored_clipped`, `restored_smart_objects`, `history.rs:193-254`). The in-memory journal is unchanged; the extra ops exist only on the wire. A reader replaying a journal MUST apply them in sequence.

### 5.3 Compatibility consequence

Because a document whose effects, smart objects, clipping or reduced-density masks came from an edit or an open has the corresponding carrier op in its journal, **an Aurora older than the carrier's introduction cannot decode the `history` entry and refuses the whole file**, even though it would otherwise ignore the optional entry. This is intentional ("loud, never a wrong picture"). The same holds for `Translate` (a cropped document), `SetAdjustment` and for adjustment ordinals 4..=12 inside `SetAdjustment`/`Restore` payloads.

### 5.4 Journal validation

`load_journal` validates nothing beyond decodability; the `Adjustment`, `Rect`, `LayerEntry` payloads inside ops go through their own `Deserialize` checks (adjustment parameter validation, `LayerEntry` has none beyond types). Replay (`History::replay`) validates the resulting tree with `LayerTree::validate`, and `apply` of each op may fail (`DocError`), e.g. a `Restore` whose recorded parent disagrees, a duplicate id, an out-of-range origin (tests `replaying_a_journal_*` in `history.rs`).

---

## 6. Pixel data: surfaces, tiles and the tile codec

### 6.1 Surfaces

All pixel storage is addressed by a **surface id** (`aurora_tile::SurfaceId`, a `u64`) and a **tile id** (`TileId { x: u32, y: u32 }`, 256x256 pixel tiles, column `x`, row `y`; ADR 0010, ADR 0005).

* A pixel layer's content surface id is the layer's own `LayerId` raw value (`LayerTree::surface_id`, `tree.rs:1872`).
* A layer's **mask coverage** surface id is `layer_id | 2^63` (`MASK_SURFACE_BIT = 1 << 63`, `mask.rs:329`; `LayerTree::mask_surface_id`). Any layer kind (including a group) can have one. `u64::MAX` is reserved for the application's composite surface and never persisted.
* A group and an adjustment layer have no content surface.

### 6.2 Tile geometry and conventions

* `TILE = 256` (`crates/aurora-tile/src/tile.rs:29`), `CHANNELS = 4`, `TEXELS = 65,536`, `SAMPLES = 262,144` `f16` values = 524,288 bytes per decoded tile.
* **Sample format:** IEEE binary16 half-float, little-endian, interleaved `R, G, B, A`, **row-major** within the tile (`import.rs fill_placed_tile`: destination offset `((row * 256) + col) * 4`). Values are not clamped to 0..1 by the format.
* **Alpha convention:** tiles in the store hold **straight (non-premultiplied)** RGBA. Evidence: `aurora-gpu/src/residency.rs:90` ("Converts a tile's straight-alpha texels to premultiplied ... in place" and "the store's own tile stays straight alpha", line ~1645); `aurora-render/src/mip.rs:~103` ("straight alpha ... the workspace's universal convention (`aurora-tile`'s store ...)"). Some brush-engine documentation speaks of "premultiplied" values for its internal math (G-11); the stored-bytes convention above is the one the GPU upload and compositor use.
* **Coordinates:** a pixel layer's tile `(tx, ty)` covers layer-local pixels `[tx*256, tx*256+256) x [ty*256, ty*256+256)` measured from the origin of that layer's `bounds` (`x`, `y`). A mask's tiles are addressed relative to the **mask's own `bounds` origin**, not the layer's or the document's (`mask.rs` "addressing convention"). Tile contents are never shifted by a Crop; only `bounds` change.
* **Mask texel convention** (`mask.rs` module doc): coverage `v` in `0..=1` is stored as `(v, v, v, 1.0)`; coverage is read from the **red** channel; **alpha is a presence flag**: `a == 0.0` means "never painted", and reads as coverage `1.0` (fully visible). Hence a missing or blank mask tile is "fully visible".
* **8-bit data never appears on disk**; the format is `f16` throughout (invariant 7.3.1b).

### 6.3 Which tiles a file may contain

For each persisted surface the reader and writer derive a grid from a rectangle (`persisted_surfaces`, `aur.rs:2753`):

* pixel layer content: `Rect = LayerKind::Pixel { bounds }`;
* mask coverage of any layer: `Rect = mask.bounds`.

`tile_grid(rect)` = `(ceil(width/256), ceil(height/256))`, after checking origin and extent (`aur.rs:1056`). Only grid positions in `[0, tiles_x) x [0, tiles_y)` are written or read. **Tile entries outside those grids, or for surface ids that no persisted surface owns, are ignored by the reader; pixels a layer holds outside the grid of its own `bounds` are not written** (G-4).

Mask coverage is persisted regardless of `LayerMask::enabled` and `inverted` (these are interpretation flags; `aur.rs` module doc).

A masked pixel layer therefore has two surfaces; both grids are charged against the same tile budget `MAX_TOTAL_TILES_PER_DOCUMENT` (section 9.2).

### 6.4 Tile entry payload: the tile codec

Each `tiles/<surface>/<x>_<y>.tile` entry holds the output of `aurora_tile::codec::encode` verbatim, Stored (not deflated again) (`crates/aurora-tile/src/codec.rs`):

```text
offset size  field
0      4     magic        = "ATIL"  (0x41 0x54 0x49 0x4C)
4      1     version      = 1
5      1     compressed   = 0 raw, 1 LZ4 block with prepended size
6      2     reserved     = 0x00 0x00 (written as zero; not checked on read)
8      ...   payload
```

* `compressed = 0`: the payload is exactly 524,288 bytes: `SAMPLES` `f16` values, little-endian.
* `compressed = 1`: the payload is `lz4_flex::compress_prepend_size` output: a **little-endian `u32`** giving the uncompressed length (MUST be exactly 524,288), then one LZ4 **block** (not frame) compressing the raw little-endian bytes of the `f16` samples. A reader uses any LZ4 block decompressor with the size prefix.
* The writer picks `compressed = 1` only when the compressed payload (including its 4-byte prefix) is strictly smaller than the raw bytes; otherwise `0` (`encode`, `encode_with`: `n + 4 < raw.len()`).
* Decoding MUST verify: length >= 8; magic; `version == 1`; `compressed` in `{0, 1}`; for `1`, the declared size equals 524,288 **before** decompression (a decompression-allocation defence, `EXPECTED_DECODED_BYTES`); the decoded length equals 524,288 (also for the raw branch, before copying).
* Maximum encoded length: 8 + 524,288 = 524,296 bytes (`MAX_ENCODED_LEN`). The container's per-tile cap is 589,824 bytes (524,288 + 65,536 slack, `MAX_TILE_ENTRY_BYTES`).
* The codec is bit-exact: `f16` values, including `NaN` payloads and negative zero, round-trip unchanged (tests `round_trips_bit_exact` etc. in `codec.rs`).

### 6.5 Blank-tile elision

A tile whose every sample has bit pattern `0x0000` or `0x8000` (zero or negative zero) is **not written** (`is_blank`, `aur.rs:1760`). A grid position with no entry means "blank": the reader makes that position read as a blank tile, even if the destination store already held pixels there (`read_persisted_tiles` records the position in `elided` and `forget_tile`s it only after the whole scan succeeded). For a mask surface a blank tile is "never painted" and reads as fully visible. Consequently a layer with no entries at all is an empty (transparent) layer.

There is no explicit "this tile is blank" marker and no per-tile checksum beyond ZIP's CRC-32 (G-12).

### 6.6 Thumbnails and previews

None. The format contains no thumbnail, preview image or composite. (`aurora-app`'s layer thumbnails are computed at runtime, ADR 0013; a PSD's merged image is generated by the PSD writer.)

---

## 7. The layer model on disk

### 7.1 Blend modes

`BlendMode` (`layer.rs:261`) serialised as a varint ordinal. `EffectBlendMode` (used inside effects) has the same ordinals 0..=26 (test `every_effect_blend_mode_maps_to_the_layer_mode_of_the_same_position`, `layer.rs`). The PSD key column is the inverse mapping used by the PSD writer (`psd_blend_key`, `crates/aurora-io/src/psd/write.rs:597`).

| Ord | Mode | PSD key | Ord | Mode | PSD key |
|---|---|---|---|---|---|
| 0 | Normal | `norm` | 14 | HardLight | `hLit` |
| 1 | Dissolve | `diss` | 15 | VividLight | `vLit` |
| 2 | Darken | `dark` | 16 | LinearLight | `lLit` |
| 3 | Multiply | `mul ` | 17 | PinLight | `pLit` |
| 4 | ColorBurn | `idiv` | 18 | HardMix | `hMix` |
| 5 | LinearBurn | `lbrn` | 19 | Difference | `diff` |
| 6 | DarkerColor | `dkCl` | 20 | Exclusion | `smud` |
| 7 | Lighten | `lite` | 21 | Subtract | `fsub` |
| 8 | Screen | `scrn` | 22 | Divide | `fdiv` |
| 9 | ColorDodge | `div ` | 23 | Hue | `hue ` |
| 10 | LinearDodge | `lddg` | 24 | Saturation | `sat ` |
| 11 | LighterColor | `lgCl` | 25 | Color | `colr` |
| 12 | Overlay | `over` | 26 | Luminosity | `lum ` |
| 13 | SoftLight | `sLit` | **27** | **PassThrough** | `pass` |

`PassThrough` (ordinal 27, introduced 0.179.0) is valid on a **group only**; the reader refuses a tree that puts it on a pixel or adjustment layer (`validate_blend_modes`). `BlendMode::ALL` has 28 entries. Compositing semantics of the modes are not defined by the file format; they are defined by Aurora's compositor (`aurora-render`).

### 7.2 Opacity, fill, visibility

`opacity` and `fill_opacity` are `f32` in `0.0..=1.0` (the only constraint the reader checks). By the compositor's documentation (not by anything in the file), opacity fades content and effects while Fill fades content only; Aurora does not apply Fill to a Pass Through group (CLAUDE.md 0.179.0, 0.181.0). `visible` is a plain flag.

### 7.3 Locks

`LayerLock { transparency, pixels, position }`: three booleans in that order, mirroring PSD's `lspf` bits. "Lock All" is all three set. A lock is enforced by the application's tools; the file format only stores it (journal op `SetLock`, ordinal 8, in the manifest as part of `LayerEntry`). PSD mapping: record flags bit 0 plus the `lspf` block (ADR/CLAUDE.md 0.198.0; not re-verified here).

### 7.4 Masks

`LayerMask` (`layer.rs:510`) on the wire is `{ bounds: Rect, enabled: bool, inverted: bool }`.

* `bounds`: the mask's rectangle in document space and the origin of its tile grid. Origin must be within +/-300,000 and extent <= 300,000 (reader-checked).
* `enabled`: UI toggle ("disabled" masks are kept and persisted).
* `inverted`: the coverage used at composite time is `1 - c` when set.
* `density` (not on the wire inside the manifest): `0.0..=1.0`, default `1.0`; effective coverage is `density * c' + (1 - density)` where `c'` is the coverage after inversion. Carried by the `mask-density` entry (8.2) and by `SetMaskDensity` journal ops.
* A mask is allowed on any layer kind including groups; `add_mask` refuses only an unknown layer or a layer that already has one (`tree.rs:2205`), and the reader does not refuse a mask on an adjustment layer.

### 7.5 Clipping

`LayerEntry::clipped` is not in the manifest. The `layer-clipping` entry (`aur.rs:417`) is `postcard` `{ version: u32 = 1, clipped: Vec<u64> }`: the raw ids of the layers clipped to the layer beneath them. It is written only for layers that **have** an unclipped base (`clip_base(id).is_some()`), so a flagged layer with nothing beneath it is not written. A reader applies each id with `LayerTree::set_clipped(id, true)`, which refuses (and thereby refuses the whole open): an unknown id, a group, a layer with no sibling beneath it. The *base* of a clipped layer is the nearest layer beneath it in its own sibling list that is not itself clipped (`LayerTree::clip_base`); the file stores only the flag. Introduced 0.201.0. The corresponding journal carrier is `SetClipped` (ordinal 18).

### 7.6 Layer effects

The `layer-effects` entry (`aur.rs:338`) is `postcard` `{ version: u32 = 1, effects: Vec<(u64 layer id, LayerEffects)> }`, written in layer-tree pre-order for layers that have effects. Effects may sit on a pixel layer or a group; on an adjustment layer they are refused (`DocError::EffectsOnAdjustment`). Each value is validated with `LayerEffects::validate` (`crates/aurora-core/src/effects.rs:600`). Positional layout of `LayerEffects`:

```text
LayerEffects {
  enabled: bool, scale: f32,
  drop_shadows: Vec<DropShadow>, color_overlays: Vec<ColorOverlay>,
  strokes: Vec<Stroke>, outer_glows: Vec<OuterGlow>,
  other: OtherEffects { inner_shadows, inner_glows, bevels, satins,
                        gradient_overlays, pattern_overlays: u32 each }   // counts only, not stored
}
EffectGate   { present: bool, enabled: bool }
EffectColor  { r, g, b: f32 }                       // each 0..=1
DropShadow   { gate, blend_mode: EffectBlendMode, color, opacity: f32, use_global_light: bool,
               angle: f32, local_angle: f32, distance: f32, spread: f32, size: f32, noise: f32,
               anti_aliased: bool, linear_contour: bool, knocks_out: bool }
ColorOverlay { gate, blend_mode, color, opacity: f32 }
Stroke       { gate, position: StrokePosition, fill: StrokeFill, blend_mode, opacity: f32,
               size: f32, color }
OuterGlow    { gate, blend_mode, color, opacity: f32, precise: bool, spread: f32, size: f32, noise: f32 }
StrokePosition: 0 Outside, 1 Inside, 2 Center        StrokeFill: 0 Color, 1 Gradient, 2 Pattern
```

Hand-derived wire sizes (blend-mode ordinals are < 128 so one byte): `DropShadow` 47 bytes, `ColorOverlay` 19, `Stroke` 25, `OuterGlow` 32.

Validation (`LayerEffects::validate`): each list at most 10 items (`MAX_EFFECT_INSTANCES`); `scale` in 0..=1000; colours and `opacity`, `spread`, `noise` in 0..=1; `angle`, `local_angle` in -360..=360; `distance` in 0..=30,000; `size` in 0..=250; all finite. `other` counts are not range-checked. Gradient and pattern fills of strokes and the `OtherEffects` kinds are *recorded but not drawn* by Aurora (counts or the fill kind only); gradient glows are not stored at all (ADR/CLAUDE.md 0.191.0). Introduced 0.180.0 (data) with rendering added in later rounds; a version this build does not know refuses the open.

### 7.7 Smart objects

See 8.5 for the entry layout. A `SmartObject` (`crates/aurora-doc/src/smart_object.rs`) is `{ unique_id: String, blocks: Vec<SmartObjectBlock { key: [u8; 4], data: Vec<u8> }>, bounds: Rect, pixel_fingerprint: u64 }`. Only pixel layers can hold one (`DocError::NotAPixelLayer`). `unique_id` is non-empty and at most 256 bytes; 1..=8 blocks; each `key` one of `SoLd`, `PlLd`, `SoLE`, `fxrp`, no repeats; each block's `data` at most 4 MiB. `pixel_fingerprint` is a content hash of the layer's tiles at open time, used by the PSD writer to decide whether the layer is unchanged (`texel_fingerprint`, `tile_fingerprint_term`, `surface_fingerprint` in `smart_object.rs`); the on-disk format stores the value but defines no consumer other than that PSD policy. The raw layer pixels are in the normal tiles; the smart object payload is preserved Photoshop data, **not interpreted**.

### 7.8 Adjustment layers

`LayerKind::Adjustment(Adjustment)`; `Adjustment` is an enum whose ordinal is its variant position. Parameter structs are decoded with validation (`try_from`); **a value that fails validation is a decode error, never clamped** (so a manifest or journal containing it is refused). Sizes are the wire sizes after the ordinal.

| Ord | Variant | Parameters in wire order | Wire size | Valid ranges (reader-enforced) | PSD key | Introduced |
|---|---|---|---|---|---|---|
| 0 | `Curves(CurvesParams)` | `composite: ToneCurve`, `red`, `green`, `blue: Option<ToneCurve>`; a `ToneCurve` = `Vec<(f32 x, f32 y)>` | variable | 2..=19 points; all finite; `y` in 0..=1; first `x` >= 0, last `x` <= 1; consecutive `x` at least 1/256 apart; decoder stops at 20th element (`tone_curve.rs validate`, `MAX_POINTS = 19`) | `curv` | 0.155.0 (point limit raised 16 -> 19 and interior endpoints in 0.157.0/0.158.0; wire shape unchanged) |
| 1 | `Levels(LevelsParams)` | four `LevelsChannel` records in order master, red, green, blue; each a tuple `(input_black, input_white, gamma, output_black, output_white)` of `f32` | 80 | finite; the four levels in 0..=1; `input_white - input_black >= (1/255) * 0.999`; gamma in 0.10..=9.99 | `levl` | 0.202.0 |
| 2 | `HueSaturation(HueSaturationParams)` | tuple `(hue, saturation, lightness: f32, colorize: bool, colorize_hue, colorize_saturation, colorize_lightness: f32)` | 25 | hue -180..=180; saturation, lightness -100..=100; colorize hue 0..=360; colorize saturation 0..=100; colorize lightness -100..=100 | `hue2` | 0.203.0 |
| 3 | `HueSaturationRanges(HueSaturationRangesParams)` | tuple `(HueSaturationParams, [HueRange; 6])`; each `HueRange` a tuple `(edge1, edge2, edge3, edge4, hue, saturation, lightness)` of `f32`; range order Reds, Yellows, Greens, Cyans, Blues, Magentas | 25 + 6*28 = 193 | each edge in 0..=360 and the three forward gaps `(e2-e1) mod 360 + (e3-e2) mod 360 + (e4-e3) mod 360 <= 360`; hue -180..=180; saturation, lightness -100..=100 | `hue2` | 0.204.0 |
| 4 | `Vibrance(VibranceParams)` | tuple `(vibrance, saturation)` `f32` | 8 | each -100..=100 | `vibA` | 0.239.0 |
| 5 | `Exposure(ExposureParams)` | tuple `(exposure, offset, gamma)` `f32` | 12 | exposure -20..=20; offset -0.5..=0.5; gamma 0.10..=9.99 | `expA` | 0.239.0 |
| 6 | `PhotoFilter(PhotoFilterParams)` | tuple `(red, green, blue, density: f32, preserve_luminosity: bool)` | 17 | colours 0..=1; density 1..=100 | `phfl` | 0.239.0 |
| 7 | `BlackWhite(BlackWhiteParams)` | tuple `([f32; 6] weights, tint: bool, tint_hue: f32, tint_saturation: f32)`; weights order reds, yellows, greens, cyans, blues, magentas | 33 | weights -200..=300; tint hue 0..=360; tint saturation 0..=100 | `blwh` | 0.240.0 |
| 8 | `ColorBalance(ColorBalanceParams)` | tuple `([f32; 3] shadows, [f32; 3] midtones, [f32; 3] highlights, preserve_luminosity: bool)`; each triple is (cyan-red, magenta-green, yellow-blue) | 37 | each slider -100..=100 | `blnc` | 0.240.0 |
| 9 | `Invert` | none | 0 | n/a | `nvrt` | 0.244.0 |
| 10 | `Threshold(ThresholdParams)` | `u8` level (one raw byte) | 1 | 1..=255 | `thrs` | 0.244.0 |
| 11 | `Posterize(PosterizeParams)` | `u8` level count (one raw byte) | 1 | 2..=255 | `post` | 0.244.0 |
| 12 | `BrightnessContrast(BrightnessContrastParams)` | tuple `(brightness: i16, contrast: i16, use_legacy: bool)` (zig-zag varints, then a byte) | 3..=7 | non-legacy: brightness -150..=150, contrast -50..=100; legacy: both -100..=100 | `brit` (+ `CgEd` for non-legacy per `write_adjust.rs`) | 0.244.0 |
| 13 | `ChannelMixer(ChannelMixerParams)` | tuple of three rows `[red, green, blue, constant]` (percent) plus a `monochrome: bool`, 49 bytes | - | sources -200..=200, constant -100..=100 (each validated at decode via `try_from`) | `mixr` (a Monochrome mixer is refused on PSD save: the Gray row is unverified) | 0.245.0 |
| 14 | `SelectiveColor(SelectiveColorParams)` | nine rows `[cyan, magenta, yellow, black]` (ranges in the order Reds, Yellows, Greens, Cyans, Blues, Magentas, Whites, Neutrals, Blacks) plus an `absolute: bool`, 145 bytes | - | sliders -100..=100 | `selc` (a non-neutral layer is refused on PSD save: the file's plate order is unverified) | 0.245.0 |

Sources: `crates/aurora-doc/src/layer.rs:66-122`; parameter files in `crates/aurora-core/src/` (`curves.rs`, `tone_curve.rs`, `levels.rs`, `hue_saturation.rs`, `hue_ranges.rs`, `vibrance.rs`, `exposure.rs`, `photo_filter.rs`, `black_white.rs`, `color_balance.rs`, `threshold.rs`, `posterize.rs`, `brightness_contrast.rs`); PSD key mapping from `Adjustment` in `crates/aurora-io/src/psd/write_adjust.rs:161-182` (`encode_adjustment`).

Standard Hue/Saturation range edges (`DEFAULT_HUE_RANGE_EDGES`, `hue_ranges.rs`): Reds `[315, 345, 15, 45]`, Yellows `[15, 45, 75, 105]`, Greens `[75, 105, 135, 165]`, Cyans `[135, 165, 195, 225]`, Blues `[195, 225, 255, 285]`, Magentas `[255, 285, 315, 345]`.

Normalisation rule for writers: a Hue/Saturation layer whose six ranges equal the standard edges with all adjustments zero SHOULD be written as ordinal 2 (`Adjustment::from_hue_saturation`), so that 0.203.0-era bytes are unchanged; ordinal 3 is also valid in that case and is decoded.

Ordinals 13 (Channel Mixer) and 14 (Selective Color) were appended in 0.245.0, with no manifest version change; an older reader refuses such a file (see G-13).

Adjustment layers have no content surface and no `bounds`; they apply to the composite below them *within their own group* (groups are isolated for adjustments, CLAUDE.md).

---

## 8. Optional entries in detail

Common rules (`aur.rs` entry docs): every optional entry is **written only when it has content**, so an ordinary save stays byte-identical to the previous version's; absence means "nothing", which is also what every older file means. Entries carrying a leading `version: u32` are read through a *frozen prefix* (`take_from_bytes::<{version}>`) before the full payload is decoded, so a reader can recognise a newer version without understanding its shape.

### 8.1 `skipped-tiles`

`{ version: u32 = 1, total: u64, records: Vec<SkippedTileRecord { surface: u64, tile_x: u32, tile_y: u32, reason: String }> }` (`SkippedTilesWire`, `aur.rs`). `total` is the true number of dropped tiles; `records` holds at most 4,096 of them (`MAX_SKIPPED_TILE_RECORDS`), each reason at most 128 characters (`MAX_SKIPPED_REASON_CHARS`, cut on a character boundary). Meaning: tiles a best-effort (autosave) writer could not read and therefore left out. Absent means nothing was dropped. A reader MUST surface a non-empty record to the user (the reference app warns: `open_aur_file`). Cap: 4096 x 537 + 64 = 2,199,616 bytes (`MAX_SKIPPED_RECORD_BYTES` = 10 + 5 + 5 + 5 + 128 x 4). Unknown version: only the frozen prefix `(version, total)` is trusted; the count is kept and the detail dropped, with a log warning.

An explicit save of a document opened from such a file carries the record forward (`known_missing`), otherwise a reopen would claim nothing was lost.

### 8.2 `mask-density`

`{ version: u32 = 1, densities: Vec<(u64 layer id, f32 density)> }`; one pair per mask whose density is `< 1.0`, in layer-tree pre-order. Read cap 64 MiB. Every pair must name a layer that has a mask and a density in `0.0..=1.0` (else the open is refused). Any version other than 1 refuses the open. Introduced 0.149.0.

### 8.3 `layer-effects`

See 7.6. Cap 16 MiB (`MAX_LAYER_EFFECTS_BYTES`).

### 8.4 `layer-clipping`

See 7.5. Cap 16 MiB (`MAX_LAYER_CLIPPING_BYTES`).

### 8.5 `smart-objects` and `smart-object-file-<n>`

`smart-objects` payload: `{ version: u32 = 1, objects: Vec<(u64 layer id, SmartObject)>, files: Vec<SmartFileWire { list: [u8; 4], unique_id: String, len: u64 }> }`. `list` is one of `lnk2`, `lnkD`, `lnk3`, `lnkE` (PSD linked-layer list keys). `len` is the byte length of the matching file entry.

For each record `i` in `files` the container MUST hold a Stored entry `smart-object-file-<i>` whose length equals `len` and whose bytes are a **framed** PSD linked-layer entry: a big-endian `u64` body length `L` (with `L >= 9`), then `L` bytes, then zero padding up to a multiple of four bytes; total size exactly `8 + L + (4 - L mod 4) mod 4` (`framed_entry_is_valid`, `aur.rs:576`). Entries are not interpreted beyond that.

Reader bounds (`read_smart_objects`, constants in `smart_object.rs`): `smart-objects` entry at most 80 MiB; at most 4,096 files (`MAX_LINKED_FILES`); at most 16 MiB of block data over all objects (`MAX_SMART_OBJECT_TOTAL_BLOCK_BYTES`); embedded files at most 512 MiB in total (`MAX_LINKED_FILE_POOL_BYTES`); each file entry read through a cap equal to its recorded `len`. A file in the pool is keyed by `unique_id`; adding one with an existing id replaces it (so duplicate ids in `files` silently collapse; G-8). The writer enumerates the pool in `unique_id` order (a `BTreeMap`) and numbers the files by that order. The reader does not check that each object's `unique_id` names a pooled file.

The embedded files travel in every save and autosave of such a document and count towards the autosave snapshot budget. Introduced 0.200.0.

### 8.6 `import-losses`

`{ version: u32 = 1, lines: Vec<String> }`: the text of a PSD open's "Opened With Changes" report, so a later Photoshop save can still list what the open dropped. Written only if non-empty. Read as **untrusted display text** and never fails the open: entry over 1,048,640 bytes (`256 x 1024 x 4 + 64`), unreadable, undecodable, version != 1, or no non-blank line after sanitising -> the single fallback line `"This document was opened with changes from another file."` (`IMPORT_LOSSES_FALLBACK`). Each line is passed through `aurora_doc::sanitize_display_name` (control and bidirectional-control characters removed; effectively at most 128 characters plus an ellipsis, G-5), blank lines dropped; at most 256 lines are kept (a 257th replaces the last with "…and more changes than can be listed."). Introduced 0.177.0.

### 8.7 `selection`

Source: `crates/aurora-io/src/aur_selection.rs`. The entry is `postcard` header bytes followed immediately by raw mask blobs; the container deflates the whole entry.

```text
SelectionHeader {
  version: u32 (= 1),
  active:  Option<SelectionMeta>,
  saved:   Vec<(String name, SelectionMeta)>      // sorted by name (bytewise String order)
}
SelectionMeta { x: i64, y: i64, width: u32, height: u32, inverted: bool, kind: u8 }
kind: 0 = Rect, 1 = Ellipse, 2 = Mask
```

After the header comes, in order, the coverage bytes of every `Mask` selection: the active selection first (if it is a mask), then each saved selection in header order. A mask blob is `width * height` bytes, row-major, one byte per pixel (`0` = not selected, `255` = fully selected), no padding, no per-blob header. For `Rect` and `Ellipse` selections `x, y, width, height` are the selection's bounds (the ellipse is inscribed in them) and there is no blob. For a `Mask`, the bounds are the mask's own bounds. `inverted` applies to the whole shape. Written only if the set holds an active or a saved selection (`SelectionSet::is_empty` ignores paths; the entry is independent of the `paths` entry).

Reader rules (everything below *drops the selection and keeps the file* unless noted; the reason is shown to the user):

* entry larger than 64 Mpx + 128 MiB + 64 KiB = 201,392,128 bytes (`MAX_SELECTION_ENTRY_BYTES`): dropped without reading;
* header does not decode / prefix does not decode: dropped;
* `version > 1`: **refuse the open** (`ManifestDeserialization`, "saved by a newer Aurora"); `version == 0`: dropped;
* more than 64 saved selections (`MAX_SAVED_SELECTIONS`): dropped;
* any `kind` other than 0, 1, 2: dropped; any `|x|` or `|y|` greater than `2^32` (`MAX_ORIGIN`): dropped (but see G-2 for `i64::MIN`);
* a mask with zero pixels or more than 67,108,864 (64 Mpx, `MAX_SELECTION_MASK_PIXELS`) pixels: dropped;
* total saved-mask bytes over 128 MiB (`MAX_SAVED_SELECTION_BYTES`): dropped;
* remaining bytes after the header not equal to the sum of all mask sizes: dropped ("its size does not add up");
* a saved name that is not already trimmed, is empty, contains a control/bidi character, exceeds 64 characters (`SELECTION_NAME_MAX_CHARS`), or repeats: dropped; a saved selection that is an empty plain (non-inverted) rectangle: dropped.

All sizes are computed from the header and checked **before any coverage byte is copied**. An active selection that is an empty non-inverted rectangle is stored as "no selection" by `SelectionSet::from_parts`. The selection coordinates are document coordinates (not layer-local) and are shifted by `Translate`/Crop in memory; they are *not* part of the journal. Introduced 0.218.0.

### 8.8 `paths`

Source: `crates/aurora-io/src/aur_paths.rs`, model in `crates/aurora-doc/src/path.rs`.

```text
PathsHeader {
  version: u32 (= 1),
  paths:   Vec<PathDto>,           // saved paths, in panel order
  work:    Option<PathDto>         // the unsaved Work Path, always named "Work Path"
}
PathDto { name: String, subpaths: Vec<(closed: bool, anchors: Vec<[f32; 6]>)> }
anchor = [x, y, handle_in_dx, handle_in_dy, handle_out_dx, handle_out_dy]    // document pixels
```

The handles are **offsets from the anchor point**, not absolute positions. A closed subpath includes the segment from its last anchor back to its first. Segments are cubic Beziers `[a.point, a.point + a.handle_out, b.point + b.handle_in, b.point]`. Written only if the document has a saved path or a work path.

Reader rules (drop paths, keep file, unless noted):

* entry larger than 5,262,144 bytes (`MAX_PATH_ANCHORS_TOTAL x 25 + 256 KiB`): dropped; the entry is read through that cap before decoding;
* `version > 1`: **refuse the open**; `0`: dropped; undecodable: dropped;
* more than 64 paths including the work path (`MAX_PATHS`); more than 256 subpaths in a path (`MAX_SUBPATHS`); more than 10,000 anchors in a subpath (`MAX_ANCHORS_PER_SUBPATH`); more than 200,000 anchors in the document (`MAX_PATH_ANCHORS_TOTAL`);
* any coordinate, handle, or handle-resolved position that is non-finite or has magnitude above 1e7 (`MAX_PATH_COORDINATE`);
* a saved-path name that is empty, not already trimmed, contains a control/bidi character, exceeds 64 characters, repeats, or equals "Work Path" in any letter case (reserved); a work path not named exactly "Work Path".

If both `selection` and `paths` are dropped, both reasons are reported ("a; b"). Paths ride inside the in-memory `SelectionSet`, so the writer only emits the `paths` entry when a selection set exists (the app always passes its set; `DocumentEntries::write_to`). Introduced 0.233.0. Path edits are undo steps and make a document unsaved (they are authored work), unlike selection changes.

---

## 9. Safety and robustness

### 9.1 Trust model

Every byte of a `.aur` file is untrusted (it can be a file someone sent, or a crash-recovery file in a world-writable temp directory). The reader must never panic, hang, or allocate without bound. The reference reader is also used on the **pre-window startup path** (crash recovery), where a stack overflow or giant allocation would abort the process.

### 9.2 Constants

All limits (full list in Appendix A):

* Manifest/journal/mask-density read cap: 64 MiB (`MAX_METADATA_ENTRY_BYTES`, `aur.rs:894`).
* Tile entry read cap: 589,824 bytes (`MAX_TILE_ENTRY_BYTES`, `aur.rs:903`).
* Per-document tile budget: `2 * ceil(300000/256)^2 = 2 * 1172^2 = 2,747,168` grid positions across all persisted surfaces (`MAX_TOTAL_TILES_PER_DOCUMENT`, `aur.rs:967`). It is charged for **grid positions** (layer grids plus mask grids), not for tiles actually present.
* Document ceiling: extent 300,000 px; origin magnitude 300,000 (`MAX_DOCUMENT_EXTENT`, `MAX_DOCUMENT_ORIGIN`, `crates/aurora-core/src/geometry.rs`).
* Tree depth 256 (`MAX_LAYER_TREE_DEPTH`).

### 9.3 Allocation-before-validation rules

* Every entry is read through `read_capped(file, name, cap)` (`aur.rs:978`): refuse if the *declared* size exceeds the cap, **and** read at most `cap + 1` bytes (`Read::take`), refusing if the read produces more than `cap`. Both halves are required because the declared size in a ZIP header is a claim a crafted archive can falsify.
* The tile codec's LZ4 size prefix is checked for equality with 524,288 **before** `lz4_flex::decompress_size_prepended` is invoked, because that function allocates the prefix value up front (measured in `codec.rs`: a 26-byte input claiming 3,000,000,000 bytes moved the process's `VmSize` by 2.9 GB).
* `selection` and `paths` sizes are derived from the header and compared with the caps before data is copied; `paths` is read through its byte cap so the decoder never sees more than the cap and "every element costs at least one byte".
* `skipped-tiles` has its own tight cap instead of the 64 MiB one, because a minimal record is about four bytes and 64 MiB decoded to ~790 MiB of RSS before the cap was introduced (0.74.1).
* The layer-tree walk is iterative (explicit stack), never recursive, and bounded by the number of layers (`layer_ids`); a cyclic or over-deep tree is rejected during decode (`validate_shape`).

### 9.4 Zip-bomb, traversal, duplicates

* **Zip bombs:** Deflate has no ratio limit; defence is the per-entry caps above plus the total-grid budget. The reader writes each decoded tile into the tile store (memory, then the scratch disk), so a hostile archive can force I/O proportional to `number of present tile entries x 512 KiB`; the code documents this "archive-to-scratch-disk amplification" and deliberately does not add a cap on materialised tiles (comment on `MAX_TOTAL_TILES_PER_DOCUMENT`).
* **Path traversal:** entry names are never turned into file-system paths. The reader looks up a fixed set of names plus generated names `tiles/<u64>/<u32>_<u32>.tile` and `smart-object-file-<usize>`, and never extracts the archive. A writer-supplied name such as `../x` is therefore inert.
* **Duplicate entries:** the behaviour with two ZIP entries of the same name is that of `zip::ZipArchive::by_name`, which the repository does not specify or test (G-14).
* **Unknown entries:** ignored.
* **Integrity:** the only integrity data is ZIP's per-entry CRC-32 (the `zip` crate is expected to verify it when an entry is read to its end, which `read_capped` does for entries within the cap; this is library behaviour not asserted by a repo test, G-12); there is no whole-file checksum or signature.

### 9.5 Failed reads commit nothing

`read` writes tiles into the caller's tile store as it decodes them. If a later entry fails, every tile it committed is dropped again (`roll_back_committed_tiles`, 0.71.2) and positions the scan would have cleared are left untouched (`elided` deferral, 0.82.2). The application additionally pre-checks a file by reading it into a throwaway store first (`read_aur_for_open`, `precheck_aur`) because surface ids restart at zero for every document, so a damaged file read straight into a live store could overwrite then erase the open document's tiles (0.143.1).

### 9.6 Atomic writing, verification, autosave and recovery

**Explicit Save As `.aur`** (`App::save_aur_file`, `crates/aurora-app/src/lib.rs:32230`): create `<file name>.tmp` beside the destination (`File::create`, a fixed name), write the container, **verify** by reopening it with `read_aur` into a throwaway 16-tile-budget store (`verify_aur`, line 1402; "verify" means "the whole container parses and every tile decodes", no comparison with the source document), then `std::fs::rename` over the destination. This path does not `fsync` the file or the directory (G-15), unlike the PSD path.

**Generic safe replace** (`aurora_io::safe_save::replace_verified` / `write_verified_temp`, `safe_save.rs`): unique temp file `.<name>.<random>.tmp` opened `O_EXCL` in the target's directory, write, `sync_all`, run the verifier on the path, rename over the target, `fsync` the directory (Unix). Symlinks are followed to their real target. Used by the PSD save; the `.aur` save does not use it.

**Autosave** (crash recovery), `crates/aurora-app`:

* On the UI thread `aurora_io::snapshot_aur_best_effort` copies the document into an `AurSnapshot` (manifest, journal, optional entries, a copy of every resident tile's texels or the encoded bytes of an evicted tile; never pages in or decodes). If the I/O-free size bound exceeds `AUTOSAVE_SNAPSHOT_BUDGET_BYTES` (2 GiB, `lib.rs:2120`) it returns `None` and the caller falls back to the streaming `write_best_effort`. The bound includes the embedded smart-object files.
* A worker thread (`background_autosave.rs`) runs `write_aur_snapshot`: same entries in the same order as `write`, tile entries from the snapshot (an evicted tile's bytes are decoded only to detect blankness and validity, and written verbatim).
* The temp file is `<name>.<pid>.<n>.<random:016x>.tmp` created `create_new`, mode `0600` on Unix (`autosave_temp_path`, `create_autosave_temp`), written, `sync_all`ed, then `rename`d over the destination under the worker's lock (`land_autosave_temp`).
* **Complete vs partial:** a write that left tiles out is renamed to the sibling `<name>.partial.aur` (`partial_autosave_path`: `path.with_extension("partial.aur")`) instead, so an existing complete autosave is never replaced by a degraded one; a complete landing deletes any stale partial.
* **File naming** (`autosave_files.rs`): in the OS temp directory, `aurora-autosave-<key>-<docid>.aur` (`<key>` is the process id, or `<pid>.<generation>` if a crashed run with the same pid still has files; `<docid>` the document session id), `aurora-autosave-<key>.index` (text: first line `aurora-autosave-index 1`, then `active <id>|-`, then `session <id> <file name>` lines, then `end` and a newline; unique temp + sync + rename), `aurora-autosave-<key>.lock` (held with an exclusive OS file lock for the run's life), `aurora-session.marker` (text: `aurora-session 1` then the run keys; presence means "last run did not shut down cleanly"). The pre-0.171.0 single file `aurora-autosave.aur` is still recovered from. Autosave files are plain `.aur` containers; recovery opens them with the same reader. The index/marker/lock files are not part of the `.aur` format.
* A clean quit deletes its own namespace; a crashed run's files are adopted by rename after recovery.

---

## 10. Compatibility rules and introduction table

### 10.1 Policy (ADR 0009)

* **Backward, unconditional:** every file any Aurora ever wrote MUST keep opening in every later Aurora. A shape change to a serialized struct would require a new manifest version with every past version still readable; this has never been needed, and the current reader accepts only `version == 1`.
* **Forward, best effort:** an unknown *ZIP entry* is skipped. An unknown *enum ordinal* or a *newer entry version* of a refusing entry is a clean, loud failure ("Couldn't Open File"), not silent loss.
* There is no "save for an older Aurora" export; every write uses the current format.

### 10.2 What requires what

| Change | Mechanism | Version bump? | Effect on an older Aurora |
|---|---|---|---|
| New optional data | new top-level ZIP entry | no | entry ignored; **but** if the producing operation also journaled a carrier op (appended `LayerOp`), the older reader refuses the file anyway |
| New layer kind / blend mode / adjustment | append an enum variant | no | older reader fails to decode manifest or journal: **refuses the open** |
| New journal operation | append a `LayerOp` variant | no | older reader refuses the journal, hence the file |
| Change to a positional struct's shape (e.g. `LayerEffects`, `SmartObject`) | new `version` in that entry's header | per-entry version | older reader: `layer-effects`, `layer-clipping`, `smart-objects`, `mask-density`, `selection`, `paths` refuse (`selection`/`paths`: only when the version is *greater* than the reader's) |
| Change to the manifest struct | new `MANIFEST_VERSION`; reader must keep old versions | would be the first | older reader refuses |
| Change to tile payload | tile codec `version` byte | n/a | older reader refuses the tile |

Fields that were wanted on already-serialised structs were added as `#[serde(skip)]` plus a separate carrier (the pattern in 7.4-7.7), because bumping the manifest version would make every existing file unopenable by that build ("a certain, universal loss traded against a conditional one", `aur.rs` module doc).

### 10.3 Silent-downgrade caveats the code documents

* **Mask coverage (0.71.0):** a build older than 0.71.0 opens such a file without error but never looks for mask tile entries; re-saving from it silently drops every painted mask pixel. No signal in the file prevents this.
* **`skipped-tiles` unknown future version:** a v1 reader keeps the count and drops the detail; re-saving writes `total` with an empty record list.
* **`import-losses` (0.177.0):** an Aurora older than 0.177.0 that opens and re-saves drops the entry.
* **Optional `selection`, `paths`:** older builds ignore them and a re-save drops them. No carrier op exists, so these older builds do not refuse the file.

### 10.4 "Introduced in" table

Aurora version numbers (SemVer, from 0.0.1; the first implementation predates the numbering: its commits carry Cargo version `0.0.0`, 2026-08-06). Versions come from the ADR 0009 addenda, the module docs in `aur.rs`/`layer.rs`/`history.rs`, and `git log` subjects; where I could not tie an item to a version it is marked.

| Item | Kind | Introduced | Evidence |
|---|---|---|---|
| ZIP container, `mimetype`, `manifest` v1, `history`, tile entries | entry | first implementation, 2026-08-06 (Cargo `0.0.0`) | commits `94e9a12` (ADR), `ade24bc`, `857fd00` |
| `ColorSpaceTag::Icc` | enum variant | 2026-08-06 (Cargo `0.0.0`) | commit `a517f43` |
| Mask coverage tiles (mask surfaces) | tile entries | 0.71.0 | commit `0a3defd` |
| `skipped-tiles` (bare list, then versioned wrapper) | entry | 0.74.0; wrapper form 0.74.1 | commits `60c8389`, `2d3d2d1` |
| `mask-density` entry, `SetMaskDensity` op (14) | entry, op | 0.149.0 | ADR addenda; commit `495210f` |
| `LayerKind::Adjustment`, `Adjustment::Curves` (0), `SetAdjustment` op (15) | variants | 0.155.0 | `layer.rs` docs; CHANGELOG |
| Per-session autosave naming | autosave files | 0.171.0 (hardening 0.171.1) | commits `d51063e`, `1070c3b` |
| Background autosave, `AurSnapshot` | API | 0.152.0 | commit `d99b5f8` |
| `import-losses` | entry | 0.177.0 | ADR addendum |
| `BlendMode::PassThrough` (27) | variant | 0.179.0 | ADR; CHANGELOG |
| `layer-effects`, `SetEffects` op (16) | entry, op | 0.180.0 | ADR; commit `ecd4555` |
| `smart-objects`, `smart-object-file-<n>`, `SetSmartObject` op (17) | entries, op | 0.200.0 | ADR; commit `05deaae` |
| `layer-clipping`, `SetClipped` op (18) | entry, op | 0.201.0 | ADR; commit `e5a6505` |
| `Adjustment::Levels` (1) | variant | 0.202.0 | ADR |
| `Adjustment::HueSaturation` (2) | variant | 0.203.0 | ADR |
| `Adjustment::HueSaturationRanges` (3) | variant | 0.204.0 | ADR |
| `selection` | entry | 0.218.0 | ADR; commit `dd00fc4` |
| `Translate` op (19) | op | 0.226.0 | ADR; commit `0afd450` |
| `paths` | entry | 0.233.0 | ADR; commit `98f71b0` |
| `Adjustment::{Vibrance, Exposure, PhotoFilter}` (4, 5, 6) | variants | 0.239.0 | ADR |
| `Adjustment::{BlackWhite, ColorBalance}` (7, 8) | variants | 0.240.0 | ADR |
| `Adjustment::{Invert, Threshold, Posterize, BrightnessContrast}` (9-12) | variants | 0.244.0 | ADR |
| `.partial.aur` autosave sibling | autosave naming | 0.52.2 | `lib.rs` docs |
| Ops 0-13 | ops | original or early; exact versions not determined | G-10 |

---

## 11. Worked example

**Derivation notice:** the bytes below were derived by hand from the encoding rules of section 2 and the layouts of sections 4 and 5, then double-checked with a short Python encoder. They were **not produced by running Aurora**. The ZIP envelope (local headers, central directory, CRC-32, timestamps) is described, not dumped, because those bytes are produced by the `zip` crate and were not inspected.

### 11.1 The document

A 4 x 2 px canvas, sRGB, one visible pixel layer "Background" whose bounds are `(0, 0, 4, 2)`, id 0, opacity 1, fill 1, Normal, no locks, no mask. It holds some non-blank pixel content, so the layer has exactly one tile entry, `(0, 0)`.

### 11.2 Entry list

```text
mimetype                      Stored    31 bytes: "application/vnd.aurora.document"
manifest                      Deflated  40 bytes uncompressed (below)
history                       Deflated  38 bytes uncompressed (below)
tiles/0/0_0.tile              Stored    8-byte header + payload (ATIL, version 1)
```

No optional entries are present: no masks, effects, clipping, smart objects, selection, paths, losses or skipped tiles.

### 11.3 `manifest` (40 bytes, hex)

```text
01 04 02 00 01 01 00 0a 42 61 63 6b 67 72 6f 75 6e 64 00 00 00 00 04 02
00 00 80 3f 00 00 80 3f 00 01 00 00 00 00 01 00
```

Annotated:

```text
01                       version              = 1                       (u32 varint)
04                       canvas_width         = 4
02                       canvas_height        = 2
00                       color_space          = Srgb (variant 0)
-- layers: LayerTree --
01                       ids                  = 1 (next id to allocate)
01                       layers map length    = 1
00                         key LayerId        = 0
0a 42 61 63 6b 67 72 6f 75 6e 64
                           name               = "Background"  (len 10)
00                         parent             = None
00                         kind               = Pixel (variant 0)
00 00 00 04 02             bounds x=0 (zig-zag 0), y=0, width=4, height=2
00 00 80 3f                opacity            = 1.0f32 (3f800000 LE)
00 00 80 3f                fill_opacity       = 1.0f32
00                         blend_mode         = Normal (0)
01                         visible            = true
00 00 00                   lock               = {transparency, pixels, position} all false
00                         mask               = None
01 00                    roots                = [0]   (length 1, id 0)
```

### 11.4 `history` (38 bytes, hex)

The journal `History::add_pixel_layer` produces for adding that one layer (a single `Restore` of a one-entry subtree):

```text
01 01 00 00 00 01 00 0a 42 61 63 6b 67 72 6f 75 6e 64 00 00 00 00 04 02
00 00 80 3f 00 00 80 3f 00 01 00 00 00 00
```

```text
01                       journal length       = 1 op
01                       LayerOp ordinal      = Restore
00                         root               = 0
00                         parent             = None
00                         index              = 0
01                         entries length     = 1
00                           id               = 0
0a "Background" 00 00 00 00 04 02 00 00 80 3f 00 00 80 3f 00 01 00 00 00 00
                             LayerEntry (same 31 bytes as in the manifest)
```

A real application document would normally journal more (renames, a New Document origin, etc.); this is the minimal journal that replays to the manifest's tree.

### 11.5 Tile entry

`tiles/0/0_0.tile` (surface id = layer id 0; tile column 0, row 0; the layer is 4 x 2 so the grid is 1 x 1). Header bytes: `41 54 49 4c 01 <flag> 00 00`, where `<flag>` is `00` (raw, then exactly 524,288 payload bytes) or `01` (LZ4: a `u32` LE `00 00 08 00`, i.e. 524,288, then the LZ4 block). The compressed bytes depend on pixel content and the LZ4 implementation and were not derived. All 262,144 samples of a tile are present, including the 65,528 outside the 4 x 2 layer, which are whatever the store held (normally zero).

### 11.6 What a minimal reader does with it

1. Open the ZIP, read `manifest`: decode as in 11.3, check `version == 1`, canvas 4 x 2, colour space sRGB.
2. Check: canvas <= 300,000; the Pixel bounds `(0,0,4,2)` have origin and extent in range; tile grid area 1 <= 2,747,168.
3. Look up `mask-density`, `layer-effects`, `layer-clipping`, `smart-objects`, `skipped-tiles`, `import-losses`, `selection`, `paths`: all absent, fine.
4. Read `history`, decode `Vec<LayerOp>`.
5. For the only persisted surface (id 0, grid 1 x 1) look up `tiles/0/0_0.tile`, read at most 589,824 bytes, decode the codec payload into 262,144 `f16` samples, install as tile `(0, 0)` of surface 0.

---

## 12. Known gaps and ambiguities

Numbering is for cross-reference; "G-n" is used above. Citations are file and function (line numbers at `80a8f00`).

* **G-1. The journal and the manifest are two descriptions of the same tree, and nothing on open checks that they agree.** `read` (`aur.rs:2300`) decodes `history` with `History::load_journal` but never calls `History::replay` or compares it with `manifest.layers`; the application uses the manifest tree. Only tests replay and compare (`aur.rs` tests such as `round_trips_*`, lines 4548, 4665, ...). A file whose journal disagrees with its manifest is accepted. Which one is authoritative for a third-party reader is therefore "the manifest" by implementation but not by any stated rule.
* **G-2. A hostile `selection` header can bypass its origin check.** `checked_mask_bytes` (`aur_selection.rs`) tests `meta.x.abs() > MAX_ORIGIN`. For `x == i64::MIN`, `abs()` overflows: in a build with overflow checks (dev and test profiles) this panics; in release (`panic = "abort"`, no overflow checks) it wraps to a negative value that passes the check. I did not run it; this is read from the source. The tests use values like `1 << 40` rather than `i64::MIN` (`aur_selection_tests.rs`, `bad_names_unknown_kinds_and_absurd_origins_drop_the_selection`).
* **G-3. `mimetype` is never validated and its position is never checked.** The reader does not open it; extension is the only recognition (`is_aur_path`, `lib.rs:969`). ODF-style sniffing is therefore a convention of the writer only. The tests assert the entry exists and is Stored (`lib.rs:67594`), not that it is first or that the bytes at a fixed offset spell the MIME type.
* **G-4. Pixels outside a layer's `bounds` grid are silently not persisted.** `persisted_surfaces`/`tile_grid` only visit `ceil(w/256) x ceil(h/256)` positions; `aur.rs` module docs say nothing in the paint pipeline clips to layer bounds ("pixels painted past them were never on documented, reliable ground"). The format does not say what a reader should do with extra tile entries (the reference ignores them) or what a writer should do with such tiles (the reference drops them).
* **G-5. `import-losses` line length is documented as 1,024 characters but is effectively 128.** `MAX_IMPORT_LOSS_CHARS = 1024` (`aur.rs:680`) is applied *after* `sanitize_display_name`, which caps at `MAX_NAME_CHARS = 128` (plus an ellipsis). ADR 0009 repeats "1,024".
* **G-6. Zero canvas dimensions and zero-sized layers are not rejected.** Only `> 300,000` is checked for the canvas (`read`, step 4); a `0 x 0` canvas passes the format layer.
* **G-7. Layer names are not validated or length-limited on the manifest/journal path.** The only cap is the 64 MiB entry size. `sanitize_display_name` is applied by callers when *displaying* or *creating* names, not by `LayerTree`'s `Deserialize`.
* **G-8. Duplicate keys.** A `layers` map (manifest) with a repeated id and a `smart-objects.files` list with a repeated `unique_id` are decoded without an explicit duplicate check (serde `HashMap` insertion / `LayerTree::add_linked_file` replacement); later wins. Shape validation may or may not then reject the tree.
* **G-9. Trailing bytes after a decoded `postcard` value** are not checked; `postcard::from_bytes` discards them (library behaviour, not tested in this repo). The format does not say whether a reader should reject them.
* **G-10. Introduction versions of the original `LayerOp` variants and of `SetBounds` are not documented** in the sources read. The ADR addenda start at 0.149.0.
* **G-11. Documentation conflict on pixel alpha.** `aurora-gpu`'s residency and `aurora-render`'s mip filter describe the tile store as **straight alpha**; `aurora-brush` docs speak of "premultiplied" texels for their own arithmetic (`clone.rs`, `amount.rs`, `tone_brush.rs`, `filter_brush.rs`), and CLAUDE.md describes the brush's math as on "premultiplied `f16` channels". I took the GPU/mip statements (which describe what the store holds when uploading) as the stored convention. A third-party reader should verify with a real file before depending on it. No field in the file records the alpha mode.
* **G-12. Integrity beyond ZIP CRC-32 is absent**, and the code does not itself assert that CRCs are checked on read; that is `zip` crate behaviour not exercised by a repo test.
* **G-13. Adjustment ordinals 13 and 14** (Channel Mixer, Selective Color) landed in 0.245.0 and are appended without a manifest version change; any older reader refuses such files. Their parameter layouts above come from `aurora-core`'s `channel_mixer.rs` and `selective_color.rs`; this spec was written before they merged, so re-check the byte counts against the code.
* **G-14. Duplicate ZIP entry names, zero-length entries, ZIP64, and archive comments** are not specified. Tile entries are small, but the archive as a whole can exceed 4 GiB (a 300,000 px document can hold millions of tiles); the `zip` crate's Zip64 handling is assumed, not asserted by any test in this repo.
* **G-15. Save As `.aur` is less careful than the PSD save and than the autosave.** `save_aur_file` (`lib.rs:32230`) writes `<name>.tmp` with a fixed name via `File::create` (it truncates an existing file of that name and is not exclusive), does not call `sync_all` on the file or the directory, then renames. The generic `safe_save` module (O_EXCL random temp name, `sync_all`, verify by path, rename, directory fsync) and the autosave writer (`create_new`, mode 0600, `sync_all`) are stronger. CLAUDE.md's "never overwrite a user's file in place; write to temp, verify, swap" is met; its durability implication is not.
* **G-16. Stale ADR text.** ADR 0009 lines 57 and 59 (Vibrance/Exposure/PhotoFilter, ColorBalance addenda) say "PSD save of these layers is refused by name"; since 0.241.0 the PSD writer writes `vibA`, `expA`, `phfl`, `blwh`, `blnc` (CHANGELOG 0.241.0; `psd/write_adjust.rs:161-170`). The ADR's opening line ("a first entry `mimetype`" and "`history`: `History::replay`'s log") describes intent, not enforced rules (G-1, G-3). `aur.rs`'s comment in `read_layer_clipping` ("Bottom-most first") is also stale: the wire order is top-first (`layer_ids` pre-order) and `set_clipped` does not depend on order.
* **G-17. The `layers` map and `smart-object-file` numbering make output bytes non-canonical.** The manifest's `HashMap` is written in the process's hash iteration order, so two saves of the same document can differ byte-for-byte. By contrast saved selections are sorted by name and the smart-object pool is a `BTreeMap` (sorted by `unique_id`). The `selection` entry has a test showing hash-order independence (`the_bytes_do_not_depend_on_hash_order`, `aur_selection_tests.rs`); no such test exists for the manifest.
* **G-18. Interaction of `layer-effects` with adjustment layers on the *journal* path is unchecked on open**, because the journal is not replayed (G-1); `SetEffects` for an adjustment layer in a hostile journal is only caught if something replays it.
* **G-19. No semantic validation of cross-entry references.** The reader does not verify that a `SmartObject.unique_id` names a pooled file, that `SmartObject.bounds` equals the layer's bounds, that a smart-object layer's `pixel_fingerprint` matches its tiles, or that the `kind` of a recorded `SelectionMeta` agrees with the presence of a blob beyond the size sum.
* **G-20. Colour management is a tag, not a transform.** `color_space` records the document's profile but no code converts tile values on open or save; the application passes no profile when saving, so every `.aur` Aurora currently writes says `Srgb` regardless of the source image's profile (`save_aur_file`, `write_autosave`: `profile: None`).

---

## 13. Appendices

### Appendix A. Constants

| Constant | Value | Where | Meaning |
|---|---|---|---|
| `MIME_TYPE` | `"application/vnd.aurora.document"` | `aur.rs:256` | `mimetype` contents |
| `MIME_ENTRY`, `MANIFEST_ENTRY`, `HISTORY_ENTRY` | `"mimetype"`, `"manifest"`, `"history"` | `aur.rs:257-259` | entry names |
| `SKIPPED_TILES_ENTRY` | `"skipped-tiles"` | `aur.rs:264` | |
| `MASK_DENSITY_ENTRY` / `_VERSION` | `"mask-density"` / 1 | `aur.rs:292/295` | |
| `LAYER_EFFECTS_ENTRY` / `_VERSION` / `MAX_LAYER_EFFECTS_BYTES` | `"layer-effects"` / 1 / 16 MiB | `aur.rs:338/341/345` | |
| `LAYER_CLIPPING_ENTRY` / `_VERSION` / `MAX_LAYER_CLIPPING_BYTES` | `"layer-clipping"` / 1 / 16 MiB | `aur.rs:417/420/423` | |
| `SMART_OBJECTS_ENTRY` / `SMART_OBJECT_FILE_PREFIX` / `_VERSION` / `MAX_SMART_OBJECTS_BYTES` | `"smart-objects"` / `"smart-object-file-"` / 1 / 80 MiB | `aur.rs:505-515` | |
| `IMPORT_LOSSES_ENTRY` / `_VERSION` | `"import-losses"` / 1 | `aur.rs:672/675` | |
| `MAX_IMPORT_LOSS_LINES`, `MAX_IMPORT_LOSS_CHARS` | 256, 1024 | `aur.rs:679-680` | line count / char cap (see G-5) |
| `IMPORT_LOSSES_FALLBACK` | `"This document was opened with changes from another file."` | `aur.rs:684` | |
| `MAX_SKIPPED_TILE_RECORDS` | 4,096 | `aur.rs:810` | |
| `MAX_SKIPPED_REASON_CHARS` | 128 | `aur.rs:834` | |
| `MAX_SKIPPED_RECORD_BYTES` | 537 | `aur.rs:845` | 10+5+5+5+128x4 |
| `MAX_SKIPPED_TILES_ENTRY_BYTES` | 2,199,616 | `aur.rs:867` | 4096x537+64 |
| `SKIPPED_TILES_VERSION` | 1 | `aur.rs:874` | |
| `MANIFEST_VERSION` | 1 | `aur.rs:881` | |
| `MAX_METADATA_ENTRY_BYTES` | 64 MiB (67,108,864) | `aur.rs:894` | manifest, history, mask-density |
| `MAX_TILE_ENTRY_BYTES` | 589,824 | `aur.rs:903` | `SAMPLES*2 + 64 KiB` |
| `MAX_TOTAL_TILES_PER_DOCUMENT` | 2,747,168 | `aur.rs:967` | `2 * ceil(300000/256)^2` |
| `SELECTION_ENTRY`, `SELECTION_VERSION` | `"selection"`, 1 | `aur_selection.rs` | |
| `HEADER_ALLOWANCE` | 65,536 | `aur_selection.rs` | |
| `MAX_SELECTION_ENTRY_BYTES` | 201,392,128 | `aur_selection.rs` | 64Mi + 128Mi + 64Ki |
| `MAX_ORIGIN` (selection) | `2^32` | `aur_selection.rs` | |
| `MAX_SELECTION_MASK_PIXELS` | 67,108,864 | `crates/aurora-doc/src/selection_mask.rs:27` | 64 Mpx |
| `MAX_SAVED_SELECTIONS` | 64 | `selection.rs:13` | |
| `MAX_SAVED_SELECTION_BYTES` | 134,217,728 | `selection.rs:18` | 128 MiB |
| `SELECTION_NAME_MAX_CHARS` | 64 | `selection.rs:22` | |
| `PATHS_ENTRY`, `PATHS_VERSION` | `"paths"`, 1 | `aur_paths.rs` | |
| `MAX_PATHS_ENTRY_BYTES` | 5,262,144 | `aur_paths.rs` | 200,000x25 + 256 KiB |
| `MAX_PATH_COORDINATE` | 1.0e7 | `path.rs:28` | |
| `MAX_ANCHORS_PER_SUBPATH`, `MAX_SUBPATHS`, `MAX_PATHS`, `MAX_PATH_ANCHORS_TOTAL` | 10,000; 256; 64; 200,000 | `path.rs:30-37` | |
| `PATH_NAME_MAX_CHARS`, `WORK_PATH_NAME` | 64, `"Work Path"` | `path.rs:39,45` | |
| `MAX_DOCUMENT_EXTENT`, `MAX_DOCUMENT_ORIGIN` | 300,000; 300,000 | `crates/aurora-core/src/geometry.rs:10,54` | |
| `MAX_LAYER_TREE_DEPTH` | 256 | `tree.rs:69` | |
| `MASK_SURFACE_BIT` | `1 << 63` | `mask.rs:329` | |
| `FULL_MASK_DENSITY` | 1.0 | `layer.rs:519` | |
| `MAX_EFFECT_INSTANCES`, `MAX_EFFECT_SIZE`, `MAX_EFFECT_DISTANCE`, `MAX_EFFECT_SCALE` | 10; 250; 30,000; 1000 | `crates/aurora-core/src/effects.rs` | |
| `MAX_SMART_OBJECT_BLOCKS`, `_BLOCK_BYTES`, `_TOTAL_BLOCK_BYTES`, `_ID_BYTES` | 8; 4 MiB; 16 MiB; 256 | `smart_object.rs:48-66` | |
| `MAX_LINKED_FILE_POOL_BYTES`, `MAX_LINKED_FILES` | 512 MiB; 4,096 | `smart_object.rs:72,75` | |
| `SMART_OBJECT_BLOCK_KEYS`, `LINKED_LIST_KEYS` | `SoLd PlLd SoLE fxrp`; `lnk2 lnkD lnk3 lnkE` | `smart_object.rs:79,82` | |
| `TILE`, `CHANNELS`, `TEXELS`, `SAMPLES` | 256; 4; 65,536; 262,144 | `aurora-tile/src/tile.rs:29-47` | |
| tile codec magic / version | `"ATIL"` / 1 | `codec.rs` | |
| `MAX_ENCODED_LEN` | 524,296 | `codec.rs` | |
| `EXPECTED_DECODED_BYTES` | 524,288 | `codec.rs` | |
| ToneCurve `MIN_POINTS`, `MAX_POINTS`, `MIN_POINT_SEPARATION` | 2; 19; 1/256 | `tone_curve.rs` | |
| `GAMMA_MIN`, `GAMMA_MAX` | 0.10; 9.99 | `levels.rs` | |
| `AUTOSAVE_SNAPSHOT_BUDGET_BYTES` | 2 GiB | `aurora-app/src/lib.rs:2120` | |
| autosave index magic/version, marker magic | `"aurora-autosave-index"` 1; `"aurora-session"` 1 | `autosave_files.rs` | |

### Appendix B. Ordinal tables

**`LayerKind`:** 0 Pixel, 1 Group, 2 Adjustment.

**`ColorSpaceTag`:** 0 Srgb, 1 Icc.

**`Adjustment`:** 0 Curves, 1 Levels, 2 HueSaturation, 3 HueSaturationRanges, 4 Vibrance, 5 Exposure, 6 PhotoFilter, 7 BlackWhite, 8 ColorBalance, 9 Invert, 10 Threshold, 11 Posterize, 12 BrightnessContrast, 13 ChannelMixer, 14 SelectiveColor. (Detail: 7.8.)

**`BlendMode`:** 0..=27 per 7.1 (27 = PassThrough, group-only).

**`LayerOp`:** 0 RemoveById, 1 Restore, 2 Reparent, 3 Rename, 4 SetOpacity, 5 SetFillOpacity, 6 SetBlendMode, 7 SetVisible, 8 SetLock, 9 RemoveMask, 10 RestoreMask, 11 SetMaskEnabled, 12 SetMaskInverted, 13 SetBounds, 14 SetMaskDensity, 15 SetAdjustment, 16 SetEffects, 17 SetSmartObject, 18 SetClipped, 19 Translate.

**`StrokePosition`:** 0 Outside, 1 Inside, 2 Center. **`StrokeFill`:** 0 Color, 1 Gradient, 2 Pattern. **`EffectBlendMode`:** same ordinals as `BlendMode` 0..=26.

**Selection `kind` byte:** 0 Rect, 1 Ellipse, 2 Mask.

**Tile codec `compressed` byte:** 0 raw, 1 LZ4 block with prepended LE `u32` size.

### Appendix C. Implementer checklist

Reader:

1. Open as ZIP; support Stored and Deflated; look entries up by exact name; ignore unknown entries.
2. Cap every entry read (declared size and bytes actually read) with the caps of 3.2 before decoding; never trust a ZIP header size.
3. Decode `manifest` with a `postcard` decoder; require `version == 1`; handle `ColorSpaceTag` ordinals 0 and 1.
4. Enforce: canvas <= 300,000; every layer and mask `bounds` origin within +/-300,000 and extent <= 300,000; total tile-grid area <= 2,747,168; tree is a tree (no dangling, no cycles, no orphans, parent fields consistent, depth <= 256); ids < 2^63 and `ids` > every id; opacity and fill in 0..=1; `PassThrough` only on groups.
5. Implement every enum ordinal in Appendix B; **refuse** (do not skip) a manifest or journal that uses an ordinal you do not know.
6. Apply side entries in the order of 3.4 with the per-entry failure policy of the table there (note the three different policies: refuse, drop-with-notice, ignore-with-fallback).
7. Decode `history` as `Vec<LayerOp>`; decide what you do with it (the reference only stores it).
8. Probe tile entries by name for every grid position of every persisted surface (layer content: `bounds`; mask: `mask.bounds`); treat a missing entry as blank (mask: fully visible); validate each tile with the codec rules of 6.4; do not allocate from the LZ4 size prefix before checking it equals 524,288.
9. Treat `selection` and `paths` as optional metadata: size-check from the header, drop on any defect except a newer version, which refuses.
10. Treat `import-losses` text and layer names as untrusted when displaying.
11. A failed read must leave the caller's state untouched.

Writer:

1. First entry `mimetype`, Stored, exact MIME text; `manifest`, `history` Deflated; tiles and smart-object files Stored.
2. Run the rectangle and budget pre-flight before writing anything.
3. Always write `version = 1`; write `Srgb` unless a profile is embedded.
4. Skip blank tiles (all bit patterns `0x0000`/`0x8000`); never write a blank mask tile expecting "hidden" (blank means fully visible).
5. Write each optional entry only when it has content; write `skipped-tiles` last and carry forward any known loss.
6. Order within lists is free for readers, but write maps and lists in a deterministic order if you want byte-stable output (G-17).
7. Write to a temp file in the same directory, `fsync`, verify by reading it back, rename, fsync the directory.
8. Never reuse or reorder enum ordinals; add new variants at the end; add new data as a new optional entry, not a new struct field.

### Appendix D. Tests that act as conformance tests

`crates/aurora-io/src/aur.rs` (module `tests`, line 2929 onwards): round trips - `round_trips_a_real_document_with_a_painted_tile`, `round_trips_a_real_non_srgb_icc_profile`, `round_trips_real_mask_coverage_painted_at_the_masks_own_origin`, `round_trips_mask_coverage_on_a_group_which_has_no_pixel_surface`, `round_trips_mask_coverage_of_a_disabled_mask`, `round_trips_a_group_created_pass_through_by_add_group_at`, `round_trips_mask_density_and_its_history_replays_to_it`, `round_trips_a_curves_adjustment_layer_and_its_history`, `round_trips_a_levels_adjustment_layer_and_its_history`, `round_trips_a_hue_saturation_adjustment_layer_and_its_history`, `round_trips_a_ranged_hue_saturation_adjustment_layer_and_its_history`, `round_trips_vibrance_exposure_and_photo_filter_layers_and_their_history`, `round_trips_black_white_and_color_balance_layers_and_their_history`, `round_trips_invert_threshold_posterize_brightness_contrast_layers_and_their_history`, `round_trips_a_curves_layer_with_moved_endpoints`, `round_trips_layer_and_group_locks_and_their_history`, `round_trips_clipping_and_refuses_a_newer_entry_version`; blank handling - `write_skips_a_fully_blank_layer_and_read_leaves_it_blank`, `a_blank_tile_elided_from_the_file_reads_back_blank_over_a_stale_store`, `write_skips_a_never_painted_mask_surface_entirely`, `reads_a_container_written_before_mask_persistence_as_fully_visible_coverage`; refusals - `read_rejects_a_file_missing_its_manifest`, `read_rejects_an_unsupported_manifest_version`, `read_rejects_a_manifest_declaring_a_layer_past_the_document_ceiling`, `read_accepts_bounds_exactly_at_the_document_ceiling`, `read_rejects_a_manifest_declaring_a_layer_origin_past_the_document_range`, `read_accepts_a_layer_origin_exactly_at_the_document_range`, `read_rejects_a_manifest_whose_layer_mask_origin_is_past_the_document_range`, `read_rejects_a_manifest_whose_group_layer_mask_origin_is_past_the_document_range`, `read_rejects_a_manifest_whose_mask_extent_is_past_the_document_ceiling`, `read_rejects_a_tile_entry_far_larger_than_one_tile_can_hold`, `read_rejects_a_manifest_whose_layers_add_up_past_the_whole_document_tile_budget`, `read_rejects_a_manifest_whose_layers_and_masks_together_exceed_the_tile_budget`, `read_rejects_a_manifest_declaring_a_canvas_past_the_document_ceiling`, `read_accepts_a_canvas_exactly_at_the_document_ceiling`, `read_rejects_a_manifest_whose_layer_tree_is_cyclic_rather_than_aborting`, `the_largest_legal_document_still_writes_and_reads_with_its_mask`, `write_refuses_two_ordinary_full_canvas_masked_layers_before_writing_a_byte`, `write_refuses_an_oversized_layer_extent_before_writing_a_byte`, `write_and_best_effort_write_both_refuse_*`; failure atomicity - `a_read_that_fails_on_a_mask_tile_leaves_no_content_tile_behind_either`, `a_successful_read_still_commits_its_tiles`, `a_failed_read_does_not_clear_a_position_it_only_elided`; side entries - `a_container_without_a_mask_density_entry_opens_every_mask_at_full_density`, `a_full_density_save_writes_no_mask_density_entry`, `a_hostile_mask_density_entry_is_refused_not_applied`, `effects_survive_aur_round_trip`, `an_old_aur_without_effects_entry_opens_unchanged`, `a_newer_effects_entry_version_refuses_loudly`; skipped tiles - `best_effort_write_skips_an_unreadable_tile_while_write_still_refuses`, `a_skipped_tile_survives_as_skipped_across_a_fresh_read`, `an_ordinary_save_of_an_opened_lossy_document_keeps_its_skip_record`, `a_container_without_the_skipped_tiles_entry_reads_as_none_skipped`, `an_ordinary_write_carries_no_skipped_tiles_entry`, `a_hostile_skipped_tiles_entry_is_bounded`, `a_skipped_tiles_entry_past_its_own_byte_cap_is_refused_before_it_is_decoded`, `an_unrecognised_skipped_tiles_version_still_reports_the_count`, `an_overlong_multibyte_reason_is_cut_on_a_char_boundary_on_read`; positional postcard - `postcard_really_is_positional_so_a_trailing_field_breaks_old_bytes`; `import_losses_tests`: `import_losses_round_trip_and_an_ordinary_save_writes_no_entry`, `a_damaged_or_hostile_import_losses_entry_errs_safe`.

`crates/aurora-io/src/aur_selection_tests.rs`: `every_shape_round_trips_bit_exactly_through_a_save`, `a_document_without_selections_writes_no_entry_and_the_same_bytes_as_before`, `an_old_file_and_an_unknown_extra_entry_both_open`, `the_bytes_do_not_depend_on_hash_order`, `a_newer_version_refuses_the_open_but_other_wrong_versions_only_drop_the_selection`, `a_damaged_entry_drops_the_selection_and_never_the_file`, `caps_are_checked_from_the_header_before_any_coverage_is_read`, `bad_names_unknown_kinds_and_absurd_origins_drop_the_selection`, `an_entry_past_the_read_cap_is_dropped_not_read`, `the_autosave_snapshot_carries_the_selection_and_counts_its_bytes`, `a_large_sparse_mask_is_small_in_the_file`.

`crates/aurora-io/src/aur_paths_tests.rs`: `paths_and_the_work_path_round_trip_bit_exactly_in_order`, `a_document_without_paths_writes_no_entry_and_the_same_bytes`, `an_older_reader_and_an_unknown_extra_entry_both_open`, `a_newer_version_refuses_the_open_and_an_older_one_is_dropped`, `damage_drops_the_paths_and_never_the_file`, `caps_and_names_are_checked_when_reading`, `an_entry_past_the_read_cap_is_dropped_not_read`, `the_autosave_snapshot_carries_the_paths`, `paths_and_a_damaged_selection_report_both_reasons`.

`crates/aurora-doc/src/layer.rs` (ordinal and wire-compat pins): `curves_bytes_are_unchanged_and_an_old_reader_refuses_levels`, `curves_and_levels_bytes_are_unchanged_and_an_old_reader_refuses_hue_saturation`, `ranged_hue_saturation_is_ordinal_three_and_plain_stays_ordinal_two`, `old_blend_mode_bytes_decode_unchanged_and_an_old_reader_refuses_pass_through`, `earlier_adjustments_keep_their_bytes`, `the_new_adjustments_are_ordinals_four_five_six_and_an_old_reader_refuses_them`, `ordinals_zero_to_six_keep_their_bytes`, `the_new_adjustments_are_ordinals_seven_and_eight_and_an_old_reader_refuses_them`, `ordinals_zero_to_eight_keep_their_bytes`, `the_new_adjustments_are_ordinals_nine_to_twelve_and_an_old_reader_refuses_them`, `layer_mask_density_is_off_the_wire_so_old_bytes_decode_at_full_density`, `every_effect_blend_mode_maps_to_the_layer_mode_of_the_same_position`.

`crates/aurora-doc/src/history.rs` (journal): `replay_reconstructs_*`, `replay_reconstructs_translated_layers_and_masks`, `replaying_a_journal_*errors` (the crafted-journal refusal set, lines ~4096-4405).

`crates/aurora-tile/src/codec.rs`: `round_trips_bit_exact`, `round_trips_incompressible_data_through_the_raw_branch`, `rejects_bad_magic`, `rejects_truncated_header`, `rejects_a_compressed_payload_claiming_more_than_one_tile_of_output`, `accepts_a_compressed_payload_claiming_exactly_one_whole_tile`, `rejects_a_compressed_payload_that_decodes_to_less_than_one_whole_tile`, `rejects_a_raw_payload_that_is_not_exactly_one_whole_tile`, `rejects_a_raw_payload_of_odd_length`, `encode_with_is_byte_identical_to_encode_for_every_kind_of_tile`.

`crates/aurora-app/src/lib.rs` (autosave and open): the autosave container test near line 67570 (entries exist, `mimetype` Stored with the exact text), `verify_aur_accepts_a_real_written_file_and_rejects_garbage`, `read_aur_for_open_refuses_a_damaged_file_before_touching_the_live_store`, `open_aur_file_warns_when_the_file_was_written_with_tiles_missing`; `crates/aurora-app/src/selection_persist_tests.rs`.

### Appendix E. Source map

| Topic | Files |
|---|---|
| Container read/write, manifest, caps, optional entries | `crates/aurora-io/src/aur.rs` |
| Selection entry | `crates/aurora-io/src/aur_selection.rs`, `crates/aurora-doc/src/selection.rs`, `selection_mask.rs` |
| Paths entry | `crates/aurora-io/src/aur_paths.rs`, `crates/aurora-doc/src/path.rs` |
| Layer tree, entries, validation | `crates/aurora-doc/src/tree.rs`, `layer.rs`, `mask.rs` |
| Journal | `crates/aurora-doc/src/history.rs` |
| Smart objects | `crates/aurora-doc/src/smart_object.rs` |
| Effects | `crates/aurora-core/src/effects.rs` |
| Adjustment parameters | `crates/aurora-core/src/{curves,tone_curve,levels,hue_saturation,hue_ranges,vibrance,exposure,photo_filter,black_white,color_balance,threshold,posterize,brightness_contrast}.rs` |
| Ids, rect | `crates/aurora-core/src/id.rs`, `geometry.rs` |
| Tile codec and constants | `crates/aurora-tile/src/codec.rs`, `tile.rs` |
| Safe replace | `crates/aurora-io/src/safe_save.rs` |
| Autosave, recovery, Save As | `crates/aurora-app/src/background_autosave.rs`, `autosave_files.rs`, `lib.rs` (`save_aur_file`, `write_autosave_temp`, `land_autosave_temp`, `verify_aur`, `read_aur_for_open`) |
| PSD key mapping | `crates/aurora-io/src/psd/write_adjust.rs`, `psd/write.rs` |
| Decision record | `docs/adr/0009-aur-document-format.md` (addenda are append-only and partly stale, G-16) |
