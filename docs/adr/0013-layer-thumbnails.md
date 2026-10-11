# 0013. Layer thumbnails: lazy, sampled, off the UI thread

**Status:** Accepted
**Date:** 2026-10-11
**Deciders:** engineering (round L4 of the Photoshop-style Layers panel); the design owner (Cahya) asked for thumbnails like the reference.
**Related:** PRD §7.3.1 (nothing assumes a document fits in memory), §7.3.4 (the UI thread never blocks), ADR 0010, ADR 0011, ADR 0012

## Context

The Nyepi template has 223 layers and about 4,000 source tiles (189 Mpx). A thumbnail for every layer, built by reading every tile, would page in the whole document and ruin memory and open time. The Layers panel only ever shows a dozen rows.

## Decision

`aurora-app`'s `layer_thumbnails` module: one worker thread, one capped cache, and a share-based read path. No second tile store.

- **Lazy.** `visible_layer_rows` gives the pixel rows within two rows of the panel viewport; only those are requested, at most 4 jobs per event-loop turn.
- **Bounded reads, among tiles that exist.** A thumbnail reads at most 16 tiles. `plan` probes the layer's tile grid with `contains_tile` (at most 4,096 cells; a layer with more grid cells, i.e. over about 16,000 px a side, is probed on a regular lattice of step `ceil(sqrt(cells / 4096))`, and tiles between lattice points are unseen) and chooses among the tiles that *exist*, so a layer painted only in a corner is not blank.
  - With 16 or fewer existing tiles all are read in one pass and the picture is the **exact area average** of the layer's bounds (a partition of the layer's pixels into texels; missing tiles are transparent), whatever the layer's size.
  - With more, one existing tile per coarse bucket of a regular grid is read (at most 16) and the picture is **sampled**: a texel whose own tile does not exist is transparent, one that does averages an 8 x 8 window of the nearest sampled tile. That window is about 1/100 of a texel's area on a 4096 px layer, so photographic detail aliases; the picture is a hint, not a downsample.
  - A new tile, a forgotten tile or a size change changes the stamp (the existing-tile count and the sampled list are in it), so it refreshes. **Remaining blindness:** an edit inside an existing but unsampled tile of a layer with more than 16 existing tiles changes no stamp and no picture until something else does; and pixels outside a layer's `bounds` are ignored.
  - The plan and stamp are memoised per layer on `TileStore::content_epoch` (moves on every write, insert and forget) and re-derived at most every 100 ms for a layer that already has a picture, so a stroke elsewhere does not rescan every visible layer each turn.
- **Read path.** The UI thread takes shares of the sampled tiles with `TileStore::share_tile_without_io`: an `Arc` clone of a resident tile's texels, the encoded bytes of a pending one, never a page-in or decode. A tile that exists only as a scratch file is read on the UI thread, **at most 8 per event-loop turn across all layers**; a layer with more waits for the next turn (`Gather`, the loop is woken at once). The queue's capacity is checked before any share is taken. The worker decodes (one 512 KiB tile at a time) and downsamples; it owns **no store and joins no residency pool**, so ADR 0011's budget is untouched. Extra memory: the queued jobs' shares (at most 8 jobs, shared not copied, so zero unless a dab copy-on-writes one) plus the cache. Non-finite half-float samples count as 0.
- **Cache.** Keyed per layer on a stamp over the sampled tiles' `content_version`s, presence and the layer's size: a dab or a pixel undo changes only the touched layer's stamp, an opacity, visibility or blend edit changes none. LRU, capped at 256 pictures and 8 MiB (a picture is up to 36 KiB: 48 x 48 premultiplied `f32` RGBA).
- **Throttle.** A layer whose stamp changed is recomputed at most every 250 ms while it keeps changing, and once more after the last change (the loop is woken at the due time).
- **Generations.** A document switch or close moves the generation (clearing cache and queue); the worker skips stale jobs and `drain` discards stale results. A deleted layer's entry is dropped on the next turn.
- **Paint.** A new `WidgetKind::Thumbnail`, drawn as a vertex-coloured `ColorMesh` of flat rectangles (the colour picker's gradient path; run-merged per row), rebuilt on every repaint: measured worst case (twelve 48 x 48 pictures of per-texel noise at 2x, release) **110,592 vertices (2.5 MiB) and 165,888 indices (0.6 MiB), 0.59 ms of CPU per repaint**, plus that much buffer upload; real pictures merge more. A cache would save the 0.6 ms and cost a clone of the same size, so none was added. Over the canvas shader's checkerboard (`aurora_gpu::CHECKERBOARD_SHADES`, the shader's two literals now a pinned constant; they are not design tokens). No texture, no new pipeline. Flat cells, not bilinear: every cell lands on whole physical pixels at 1x and 2x, so the picture is crisp and the checkerboard stays hard-edged. Exactly cut to a clipping ancestor. AT-hidden.
- **Rows grow.** A Layers row is 32 px (`spacing.lg` thumbnail + `spacing.xxs` twice) instead of 21 (`TreeItemState::row_height`). The Layers panel's content cap stays "`size.content_panel_max_rows` rows", now of its own rows (320 px).

## Alternatives considered

- *Share the ADR 0012 mirror.* It exists only on the CPU composite path and is released on every document switch; thumbnails would depend on a worker the GPU path never starts.
- *A second mirror-like store.* Another pool member and another copy of the working set, for pictures of 12 layers.
- *Whole-layer averages.* Unbounded cost per layer (a 300,000 px layer).
- *A fixed sub-grid chosen from the layer's size alone* (the first draft): painting in an unsampled tile left the picture blank and never refreshed it. Replaced by sampling among existing tiles.
- *A GPU texture per thumbnail.* A new pipeline and an atlas for a 24 px picture; the mesh path costs a few hundred quads.

## Consequences

- Thumbnails are derived data: never saved, never in History, never dirty.
- A layer with more than 16 existing tiles shows a sampled picture (see the remaining blindness above). Hidden layers are not dimmed.
- **Not seen on real hardware** (macOS, a 2x display, a screen reader). On Nyepi, 8 pixel rows cost the UI thread 0.5 ms and 32 tile shares in total.
- **Reopen** if sampled pictures prove misleading on real templates (raise the 16-tile bound or sample by content), or if the capped scratch read on the UI thread shows in a profile.
