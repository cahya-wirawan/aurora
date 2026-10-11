# 0012. Background CPU compositing through a mirrored tile store

**Status:** Accepted
**Date:** 2026-10-11
**Deciders:** engineering (round R3 of the large-template performance plan)
**Related:** PRD §7.3.4 (the UI thread never blocks on rendering), §7.3.5 (brush bypass), §7.3.8 (one GPU device, one frame), ADR 0005, ADR 0010, ADR 0011

## Context

Up to 0.185.0 the CPU compositing path ran inside the redraw handler: `recomposite_visible_tiles` folded every stale visible tile synchronously. On the Nyepi template (release build, 16,384-tile budget) a warm full composite took about 3.3 s, almost all of it folds, so every structural edit (visibility, opacity, blend mode, undo of one) froze the window for seconds. That is a direct breach of invariant §7.3.4.

Moving the folds to another thread meets three obstacles:

- `TileStore::get` takes `&mut self` (page-in and LRU bookkeeping), and the store is the UI thread's: the brush writes it on every dab.
- The drop-shadow coverage cache and the Curves LUT cache are `thread_local` `Rc`s.
- The composite surface has a single writer (the materializer guard in `install_composited_tile`).

## Decision

**One worker thread with its own *mirror* store, fed by content-version deltas.** (`aurora-app`'s `background_composite` module.)

- **Mirror.** The worker owns a second `aurora_tile::TileStore`, in the session scratch directory, with the source store's budget. It composites with the very same `composite_roots_into_tile` as the synchronous path, so its output is bit-identical to a synchronous composite of the same state. Its shadow and LUT caches are its own thread's thread-locals, keyed by the mirror's own store serial.
- **Deltas, not snapshots.** Each job carries a clone of the `LayerTree` and only those source tiles whose `content_version` changed since the last job sent to that mirror (`TileStore::held_tile_versions`, diffed against what was sent), plus the keys that disappeared. Only surfaces the tree can composite are shipped: its layers' pixels and its masks (`tree_surfaces`). A structural edit changes no pixels, so its job carries no tiles at all. The first job for a store ships every held tile.
- **Shared, not copied.** `Tile` texels are now `Arc<Vec<f16>>`, copy-on-write: `TileStore::share_tile` hands the mirror the resident tile's own buffer (a reference count), and `Tile::texels_mut` copies it once only if the mirror still holds that version. An evicted tile goes as its encoded bytes (`pending`, or read from scratch, the one case that does I/O on the UI thread).
- **Installing.** Results come back over a channel. A coalesced wake (`AppEvent::CompositeReady`) ends the loop's wait, and `about_to_wait` installs them. A result installs only if (1) its generation is the active job's, (2) the cache id and store serial are the current document's, (3) the grid's reference origin is unchanged, and (4) `CompositeCache::unchanged_since(tile, job.stamp)` holds. The cache keeps an invalidation stamp: each invalidation advances it, and tiles in flight record theirs, so a dab, an undo or any edit that lands mid-job makes that tile's result stale.
- **Provisional tiles.** A finished tile from a superseded job of the same document (the last four are kept) is uploaded but never marked current (`install_provisional_tile`), and only if it is newer than what that tile already shows (a per-tile *displayed* stamp). A continuous structural drag (an opacity slider, say) keeps the canvas moving, and the newest job still recomposites every provisional tile. A cancel drops them.
- **Memory.** A pooled document's mirror joins the same `ResidencyPool` (`TileStore::pool`). The two budgets are shares of one total (`max(floor, total / members)`), with one floor (256 tiles) of slack, not two whole budgets. The UI store shrinks lazily: one `EVICTION_BURST` when the mirror is created, then its own insertions. So the total + floor bound holds once the UI store has shrunk to its share; until its next insertions it can briefly be exceeded. Measured on Nyepi: after an open the UI store holds only 40 resident tiles, because an open leaves them encoded, while the mirror decodes the 2,559-tile working set. RSS rose from 1.6 GiB to 3.2 GiB at a 16,384-tile pool and to 2.4 GiB at 2,048. Before this round the UI store decoded the same working set itself.
- **App seams.** `app_event_requests_redraw` (from `user_event`) and `drain_background_composite_into` (from `about_to_wait`) are free functions, tested headlessly from dispatch through the wake, the drain, the install and the dirty mark the atlas upload reads. The redraw handler's own dispatch call (`recomposite_or_dispatch`) and the GPU upload itself are not covered by those tests.
- **Scheduling.** At most `SYNC_TILE_LIMIT` (3) stale tiles that no healthy job covers are still composited synchronously: the brush's own case, unchanged from 0.185.0. More go to the worker. A job in flight is superseded only when it belongs to another document or a tile of it was invalidated; a pan waits for it to finish. Until a tile installs, the canvas shows its previous composite.
- **Lifecycle.** Switching away from or closing a document cancels its job and releases the mirror. Quit cancels the worker first in `run_shutdown_cleanup` and joins it: at most the one tile in progress finishes, and it is never installed.

## Alternatives considered

- **Snapshot every needed source tile per job.** This needs each target tile's exact source windows and shadow aprons, a fragile duplicate of the compositor's own reach logic, and it copies megabytes per job even when nothing changed. The delta mirror needs neither. A missed apron tile cannot happen, because the mirror holds every tile the source holds.
- **Move the store behind a lock.** Every dab would contend with a multi-second composite for the lock, which breaks §7.3.5's 10 ms budget.
- **Make the compositor generic over a read-only tile source.** This touches every function on the fold path. It remains possible later; the mirror needed no change to the compositor at all.

## Consequences

- **Invariant §7.3.4** holds for the CPU path's whole-view recomposite. It still does not hold for: up to 3 synchronous tiles (a dab, or a small pan); the GPU fast path, which is unchanged; export, histogram and PSD-merged composites, which are explicit actions; and an open's first job, which shares every held tile and reads evicted ones from scratch.
- **Memory.** Bounded as above. Counting shared tiles in both shares is conservative: it halves each store's cache, so a document whose working set needs more than half the pool pages from scratch. That is correct, just slower: 0.3 s on Nyepi at 2,048 tiles. The worker's drop-shadow cache (256 MiB cap) is extra.
- **Brush.** A dab on a tile the mirror still shares pays one 512 KiB copy on its first write after a job.
- **One worker.** Its folds are sequential; rayon across tiles is R4 (done in 0.188.0, see the addendum).
- **Deferred.** The first dispatch still reads evicted (non-pending) tiles from scratch on the UI thread, and the mirror is released on every document switch, so switching back pays a full first job again. `TileStore::held_tile_versions` scans the whole store per job (about 1 ms on Nyepi). `IdGenerator` gained `Clone` only so the worker can hold a read-only tree copy; it is never used to mint ids there.
- **Reopen** if the mirror's worst-case memory proves too high on small machines (bound it with its own budget, or composite through a read-only view), or if more than one worker thread is wanted (the mirror would then need sharing or sharding).

## Addendum (0.188.0): folding a job's tiles in parallel

**Decision.** The worker folds a job's tiles on its own rayon pool, one tile per task, through a lock around the one mirror; the fold path reads every store through a four-method trait.

- **Pool.** `background_composite::composite_threads()` threads: `available_parallelism − 1`, at least 1, so the UI thread and the brush keep a core. It is the worker's own `rayon::ThreadPool`, never the global one (`aurora-gpu`'s residency serializer has its own too). If rayon cannot build it the worker folds sequentially on its own thread, as before.
- **Read-only access through a lock.** The fold path (`composite_roots_into_tile` and the 20 functions under it) now takes `&mut dyn TileSource`: `contains_tile`, `content_version`, `serial` and `texels`, the only four store operations it ever used. `TileStore` implements it exactly as before (`texels` is `get`), so the synchronous path is unchanged. During a job the mirror sits in a `Mutex`, and each task reads it through a `SharedMirror`: a resident tile is a reference count on its copy-on-write buffer, taken under the lock (`TileStore::peek_shared`, new, `&self`, no LRU bump, no page-in) and copied by the fold outside it. Only a tile that is not resident pages in under the lock. Every result is a pure function of the mirror's content and the job's tree, so it is bit-identical to the sequential fold.
- **Recency.** Peeks do not bump the LRU. Each task promotes the keys it read in one go, before any page-in it does and when it finishes, so a page-in's eviction spares what the job is using. That affects only which tiles stay resident, never a pixel.
- **Caches.** The drop-shadow coverage cache is one `Mutex<ShadowCache>` for the worker and its whole pool (`SharedShadowCache`, set per thread by `use_shared_shadow_cache`), still keyed by the store serial and capped at 256 MiB for the pool together, not per thread. Per-thread caches would have cost 256 MiB × threads and would mostly miss, because a tile goes to a different thread from job to job. Coverage is `Arc` now, not `Rc`. Every other thread keeps its own thread-local cache, so the UI thread's cache is separate, as in 0.186.0. Curves LUTs (8 per thread) and fold scratch buffers (at most 4 × 512 KiB per thread) stay per thread; with 7 pool threads that is about 2 MiB of LUTs plus 2 MiB of scratch per thread (about 28 MiB across 7 threads).
- **Cancellation and shutdown.** Each task checks the job's generation before it starts. A newer job, a cancel or a shutdown stops the pool starting tiles; only the tiles in progress finish (at most one per thread, side by side). The install and staleness rules of 0.186.0 (generation, document, origin, `unchanged_since`, provisional tiles) are untouched: they act on results, whatever thread made them.
- **Alternatives.** *A per-thread store view* (each thread a `TileStore` of shared `Arc`s): every evicted tile would be decoded once per thread, and the budget accounting is per store. *Prefetching each tile's source set*: the fragile duplicate of the compositor's reach logic this ADR already rejected. *An `RwLock`*: `TileStore` is not `Sync` (its writer's channel), and critical sections are a hash lookup, so a `Mutex` costs nothing measurable.

**Measured** (Nyepi, release, this 8-core Linux box, 7 pool threads; the view is only 40 composite tiles):

| | 0.187.0 | 0.188.0 |
|---|---|---|
| Warm structural edit to final pixels, 16,384-tile pool | 2.97 s | 0.62 s |
| First job after open, 16,384-tile pool (decodes the working set) | 7.02 s / 8.23 s | 1.51 s / 2.22 s |
| Warm opacity edit, 2,048-tile pool (active share) | 5.03–5.07 s | 2.37–2.71 s |
| First job, 2,048-tile pool | 8.49 s | 3.10 s |
| RSS after first job / after 3 edits, 16,384 pool | 3,217 / 3,217 MiB | 3,360 / 3,368 MiB |

Each pair of "first job" figures comes from two different tests of the same build. The same warm edit on a one-thread pool, in the 0.188.0 build, takes 3.70 s against 0.187.0's 2.97 s. That ~25 % per-thread overhead (lock round trips, `Arc` texel reads, shared-cache locking) was not investigated. With 40 tiles of very unequal cost, 7 threads give 6×, not 7×. At 2,048 tiles page-ins are serialized under the lock, which is why that row gains only 2×.

**Consequences.** On a machine with 2 cores the pool has 1 thread, and the one-thread path was measured slower than 0.187.0's sequential fold (3.70 s against 2.97 s on Nyepi, about 25%, from the mirror lock and `Arc` reads; not investigated), so such machines should expect a slower background composite, not a faster one. Tiles evicted while a task still holds their `Arc` stay alive outside the residency budget, bounded per thread by its working set (cover candidates plus up to 25 apron cells). Not measured: Windows or macOS, machines with fewer cores, contention with a live brush stroke, and the UI thread's frame time while the pool runs (the pool leaves one core, which was not observed under load). A cold mirror still decodes on one thread at a time. Moving the decode outside the lock needs a two-step page-in in `aurora-tile`, a possible follow-on.
