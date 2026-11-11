# Bristle brush feasibility spike (task 168)

Status: measurement only. Branch `168-bristle_spike`, no version bump, nothing shipped, no UI, no tool, no
saving. Code: `crates/aurora-brush/src/bristle.rs` (engine) and `bristle_tests.rs` (10 unit tests plus the
ignored measurement `bristle_measure` and picture dump `bristle_dump`). The module is private to
`aurora-brush` and marked experimental.

Every number below is **measured** unless the text says **estimated**. Nothing here was seen on a real
tablet, a Mac or Windows, and no human has judged how the output looks.

## 1. What was built

A brush of N bristles (16..1024) on the unit disc (golden-angle spiral, seeded jitter; splitmix64, no wall
clock). Each bristle has a stiffness (0.3..1), a colour (brush colour, +-8 % seeded shade) and a paint load.
`PenSample { x, y, pressure, tilt_x, tilt_y, rotation }` drives it:

- pressure scales layout spread (0.35..1 of the radius), bristle dab radius and flow; pressure 0 deposits nothing;
- tilt shifts the footprint centre (up to 0.6 radius) and stretches the layout into an ellipse (x2.5 long, x0.7 short at full tilt);
- rotation (barrel twist) rotates the layout;
- each bristle springs toward its target (`k = 1 - lag * (1 - stiffness/2)`, default lag 0.6), so bristles trail and splay on a curve and converge when the pen stops;
- between samples each bristle deposits soft dabs along its own path (step = max(0.5 x its radius, 1 px), at most 64 per bristle per sample), each through the **existing** `stamp_dab_amount` with a stroke snapshot (so it gets stroke compositing, selection-free, undo capture), flow = brush flow x pressure x load;
- load depletes with distance travelled.

Bristle dab radius is `1.6 r / sqrt(N)` floored at 0.75 px (N = 1024, r = 12 gives 0.75 px; N = 32, r = 150 gives 42 px).
Non-finite pen fields refuse the sample (state unchanged); finite values are clamped (coordinates to
+-1e6 px, pressure 0..1, tilt -1..1, rotation wrapped).

**Limits of the model.** It is a cost model, not a good brush. No pick-up or mixing of canvas paint (the
part of a real Painter/wet brush that would cost most), no stiffness response to pressure, no bristle
collision or clumping, no paper grain, bristles composite through the soft Brush's shared stroke coverage
and not a wet layer. The deposit goes through a general dab routine, not a purpose-built kernel, so the
CPU numbers are an upper-ish bound for a CPU path (see 3). Unit tests (all pass): determinism to the bit,
lag then convergence, splay on a curve, load depletion, pressure 0 deposits nothing and creates no tile,
pressure/tilt/rotation geometry, valid premultiplied f16 output (alpha 0..1, colour <= alpha), hostile
samples (NaN, inf, 1e30) refused or clamped without panic.

One stroke was dumped to a PPM and viewed (not committed, scratch only): a 256-bristle, radius-60 brush on
a circular arc. I looked at it: it reads as a red band with fine streaks following the arc, ragged ends
where bristles start and stop, and width that varies with pressure/tilt. I make no claim that it looks
good; the bristles are all one hue with small shade jitter.

## 2. Measurement method

- Machine: i3-10100 (4 cores / 8 threads) + RTX 3090, Linux (as given in the task; the GPU is not used by
  this CPU measurement). Single thread. A foreground release run took about 7 minutes.
- Load before the runs: 1-minute load average 1.6-1.9 (the box was not idle: other login sessions were
  running; load went above 2 only while I was compiling, and I waited for it to fall below 2 before each
  timing run, never building during one). The release matrix was run twice; the two runs agree to about
  5 % on means (run 2 is quoted). One of the two release-run logs shows a ~10x worst-case outlier on a few
  configurations (max column), so read `max` as noise-prone and use mean/median/p99.
- Target: real `aurora_tile::TileStore` layer, 512-tile budget, 2600 x 2600 canvas, the same
  `stamp_dab_amount` + `StrokeSnapshot` pattern the soft Brush tests use. Baseline: the existing soft Brush
  at `DEFAULT_SPACING` (dabs along the pen path via `advance_segment`) over the same pen input.
- 8 strokes x 300 samples = 2,400 timed samples per configuration (`n=2400`), a fresh brush and snapshot per
  stroke, paint kept wet (`load_length_radii` 400) so late samples are not free. Timed per pen sample with
  `Instant`; "adv" is the bristle update (no tile access), "dep" the tile writes. The first stroke touches
  cold tiles; later ones overpaint.
- Pen: circle/line at 800 px/s (3.3 px per sample at 240 Hz, 6.7 at 120 Hz), zig-zag about 3,000 px/s
  (12.5 / 25 px per sample), pressure, tilt and rotation varying smoothly. Speed sweep 200..6,400 px/s.
- Quoted: mean, median, p99 of the 2,400 samples. Maxima are in the raw logs, not used as ratios.
- This is the cost of one pen sample until pixels are in the tile store. It does **not** include
  recomposite, tile upload, present, or invalidation (`invalidate_painted_tiles`), which the real stroke
  path adds per frame.

## 3. Results (release, circle at 240 Hz, 800 px/s = 3.3 px/sample), microseconds per pen sample

| brush | r | mean | median | p99 | dabs/sample | adv share |
|---|---|---|---|---|---|---|
| soft Brush | 12 | 40 | 34 | 187 | 1.1 | - |
| bristle N=32 | 12 | 179 | 179 | 259 | 77 | 1 % |
| bristle N=128 | 12 | 265 | 258 | 382 | 424 | 1 % |
| bristle N=512 | 12 | 539 | 535 | 708 | 1,694 | 3 % |
| bristle N=1024 | 12 | 999 | 990 | 1,340 | 3,388 | 3 % |
| soft Brush | 48 | 113 | 0 (72 % of samples place no dab) | 502 | 0.3 | - |
| bristle N=32 | 48 | 468 | 442 | 850 | 19 | <1 % |
| bristle N=128 | 48 | 901 | 903 | 1,137 | 154 | <1 % |
| bristle N=512 | 48 | 2,134 | 2,092 | 2,937 | 1,227 | 1 % |
| bristle N=1024 | 48 | 3,132 | 3,092 | 4,382 | 3,219 | 1 % |
| soft Brush | 150 | 344 | 0 (91 % no dab) | 4,021 | 0.1 | - |
| bristle N=32 | 150 | 1,302 | 1,218 | 3,040 | 6 | <1 % |
| bristle N=128 | 150 | 2,594 | 2,535 | 4,203 | 49 | <1 % |
| bristle N=512 | 150 | 5,317 | 5,256 | 8,094 | 394 | <1 % |
| bristle N=1024 | 150 | 7,576 | 7,670 | 10,314 | 1,113 | <1 % |

Cost is dominated (97-99 %) by tile writes inside `stamp_dab_amount`; the bristle update itself
is 1-30 us (127 us at the extreme 6,400 px/s, N=1024). Rough fit from the table: **about 37-48 ns per dab
texel plus 0.2-0.5 us fixed per dab** (r=12 N=512: 0.31 us per 2.3-texel dab; r=48 N=512: 1.73 us per
36-texel dab; r=150 N=1024: 6.8 us per 177-texel dab).

### Pen speed sweep (r = 48, 240 Hz circle), mean / p99 microseconds per sample

| speed | px/sample | soft | N=128 | N=512 | N=1024 |
|---|---|---|---|---|---|
| 200 px/s | 0.8 | 29 / 436 | 242 / 418 | 566 / 753 | 881 / 1,426 |
| 800 px/s | 3.3 | 125 / 606 | 901 / 1,153 | 2,142 / 3,057 | 3,133 / 4,426 |
| 1,600 px/s | 6.7 | 233 / 581 | 1,873 / 2,672 | 4,351 / 6,031 | 6,422 / 8,732 |
| 3,200 px/s | 13.3 | 447 / 954 | 3,726 / 4,820 | 8,574 / 11,622 | 12,697 / 17,535 |
| 6,400 px/s | 26.7 | 912 / 1,491 | 7,384 / 9,826 | 16,299 / 23,266 | 23,931 / 35,331 |

Cost scales with pen travel, not sample count: about 280 us/px (N=128), 640 us/px (N=512), 950 us/px
(N=1024), against the soft Brush's 34 us/px. At 120 Hz the same strokes cost about twice per sample
(circle 6.7 px/sample: N=128 1,839, N=512 4,289), so the per-pixel figure is the portable one.

### Other shapes (r = 48), mean / p99 us

| case | soft | N=128 | N=512 |
|---|---|---|---|
| line @ 240 Hz (3.3 px) | 112 / 485 | 920 / 1,216 | 2,089 / 2,922 |
| line @ 120 Hz (6.7 px) | 219 / 513 | 1,836 / 2,509 | 4,034 / 5,707 |
| zig-zag @ 240 Hz (12.5 px) | 433 / 927 | 3,444 / 4,955 | 7,940 / 10,718 |
| zig-zag @ 120 Hz (25 px) | 816 / 1,249 | 6,843 / 9,514 | 15,780 / 21,565 |

### Dev profile (opt-level 1), r = 48, mean / p99 us

| case | soft | N=128 | N=512 |
|---|---|---|---|
| circle 240 Hz, 800 px/s | 142 / 668 | 1,163 / 1,698 | 2,765 / 4,059 |
| circle 240 Hz, 3,200 px/s | 548 / 1,060 | 4,675 / 6,359 | 11,069 / 15,137 |
| zig-zag 240 Hz | 527 / 1,032 | 4,309 / 5,763 | 10,238 / 14,211 |

Dev is about 1.2-1.3x release here (the dab code is already compiled at opt-level 1 plus optimised dependencies).

### Undo snapshot cost (same runs)

A 300-sample stroke captures 7-9 tiles at 800 px/s (22-43 at scribble speed), 0.1-2.9 MiB after the
lossless pack, `PixelHistory::push` 0.6-5 ms on the UI thread. The bristle brush is within noise of the soft
Brush here because the touched tiles, not the dab count, set the undo cost. Per-stroke coverage adds up to
256 KiB of f32 per touched tile while the stroke is live (`stroke_cover.rs`).

### Not tried

Thread scaling: **skipped**. The bristle update is 1-3 % of the cost, so rayon over bristles would buy
nothing; the cost is tile writes through `&mut TileStore`, and parallelising those needs a tile-partitioned
deposit (bin dabs by tile, one task per tile), which is a redesign of `stamp.rs`'s entry point, not a
spike-sized experiment. A purpose-built deposit kernel (no per-dab tile lookups, f32 working tile) was not
written; **estimated** (not measured) that it could cut the 37-48 ns per texel several-fold, which would be
the first thing to try before any GPU work.

## 4. Verdict against the 10 ms brush budget

Two separate constraints matter and the budget text only names one:
(a) latency: one pen sample to pixels in under 10 ms; (b) throughput: at 240 Hz a sample arrives every
4.17 ms (8.3 ms at 120 Hz), so a sustained mean above that falls behind however good the p99 is. All numbers
are CPU tile-write time only, before the composite/upload/present work the app does per frame.

Single-threaded CPU through the existing dab path, release, r = 48:
- **N <= 128**: fits comfortably up to about 3,000 px/s (mean 3.7 ms, p99 4.8 ms, at 3,200 px/s on a 240 Hz circle); at the zig-zag 3,000 px/s: 3.4 / 5.0 ms. Margin against 10 ms: 2-3x at scribble speed, 8x at 800 px/s. At 6,400 px/s p99 is 9.8 ms: no margin.
- **N = 512**: fits at ordinary speed (800 px/s: 2.1 ms mean, 3.1 ms p99, 3-4.5x margin; 1,600 px/s: 4.4 / 6.0 ms) but misses the 4.17 ms throughput bound above about 1,500 px/s at 240 Hz and the 10 ms latency bound at 3,200 px/s (mean 8.6, p99 11.6).
- **N = 1024**: 3.1 ms at 800 px/s, 6.4 ms at 1,600; misses at 3,200 px/s (12.7 ms).
- **Radius**: r = 12 is cheap for every N (N = 1024: 1.0 ms at 800 px/s); r = 150 is the problem: N = 512 costs 5.3 ms (p99 8.1) and N = 1024 7.6 ms (p99 10.3) at only 800 px/s, which already exceeds the 240 Hz throughput bound and has no latency margin. Large radius with many bristles does not fit on this path at any real speed.
- The existing soft Brush for comparison: 34 us/px; its worst case here is r = 150 with p99 4.0 ms.

Summary: a bristle brush of up to about 128 bristles at radius up to about 50 px fits the budget on CPU with
a real margin even for fast strokes; 512 bristles fits only for ordinary strokes and moderate radii;
1024 bristles or radius 150 with more than 128 bristles does not fit with the existing dab path. This is a
verdict about a model with no paint pick-up; adding canvas sampling per bristle dab (reading and mixing the
existing texels) would add reads to the same loop and is not covered by these numbers. Every figure is
single thread, on one machine, release/dev profiles, quiet-ish but not idle.

## 5. GPU feasibility (read from the code; nothing prototyped)

How a stroke works today:
- A dab is stamped straight into the CPU `TileStore` (`aurora-app/src/lib.rs`, `App::paint_dab`, ~line 35623, via `selection_paint::dab_through_selection` into `aurora_brush::stamp_dab_amount`, `crates/aurora-brush/src/stamp.rs` ~583), then `invalidate_painted_tiles` marks composite tiles stale, and the composite (GPU or CPU) plus `TileResidency::sync` upload them. PRD invariant 5 and ADR 0010 describe a separate scratch store for the active stroke; ADR 0010's own closing note says it was **not built**: "painting writes straight into the shared document store" (`docs/adr/0010-layer-pixel-storage.md`, last paragraph).
- Tile format: 256 x 256 x 4 `f16`, `Arc<Vec<f16>>` copy-on-write (`aurora-tile/src/tile.rs`, `TILE = 256`, `SAMPLES`), 512 KiB. GPU side is `Rgba16Float` everywhere (`aurora-gpu/src/residency.rs` ~1111 atlas, `aurora-gpu/src/display.rs` ~1028/1059, `aurora-render/src/composite.rs` render targets).
- Upload: `queue.write_texture` per dirty tile, from `TileResidency::sync_ids` (`residency.rs` ~1950, `write_texture` ~2176) and `display.rs` ~726. Data flows CPU to GPU only.
- No compute pipeline exists: a search of `aurora-gpu`, `aurora-render` and `aurora-app` for `ComputePipeline`, `@compute` and storage textures found none. All GPU work is render passes (`TileCompositor`, `aurora-render/src/composite.rs`). The latency test (`aurora-render/src/latency.rs`) measures submission, not completion.
- Undo: `StrokeSnapshot` (`aurora-brush/src/undo.rs`) copies each tile's raw f16 content (512 KiB) before first write; packed losslessly at push.

What a GPU bristle or wet-paint brush needs from this:
1. **A stroke scratch texture** per active layer region in `Rgba16Float` (matches the existing 16-bit float pipeline, invariant 1b), plus per-stroke accumulated-coverage storage equivalent to `stroke_cover.rs` (f32 per texel, 256 KiB per tile) as an `R32Float` texture. Writing it needs either a render pass with blending/instanced dab quads (works with the existing render-pass-only plumbing; one instance per dab: 3,000-26,000 dabs per sample in the measurements above is trivial) or a new compute pipeline with storage-texture writes (new infrastructure; I believe write-only `rgba16float` storage is baseline in WebGPU but have not checked wgpu 30's feature requirements).
2. **Per-stroke state buffers** for the bristles (position, target, load, carry: 32-48 bytes each, 1024 bristles is about 48 KiB), updated by a tiny pass or still on the CPU (the bristle update measured 1-30 us, so keeping it on the CPU and uploading a dab list each sample is viable).
3. **Readback or a CPU mirror.** `TileStore` is the source of truth (invariant 1, saves, export, history, PSD write); the CPU path must get the pixels back. Today no stroke path reads GPU data back at all. Options: read the scratch tiles back at stroke end (a stall of order one 512 KiB readback per touched tile, 7-43 tiles above), or keep running the CPU dab path in parallel as the authority and use the GPU only for the live preview. Wet-paint pick-up needs the **current** canvas under the brush readable by the shader, which means the layer's tiles must be GPU-resident and kept in sync with CPU edits (today `TileResidency` is read-only on the GPU side).
4. **Undo.** A GPU stroke still needs the pre-stroke tile content: either keep the CPU snapshot (cheap as measured: 0.1-2.9 MiB packed, but it needs the tile on the CPU, i.e. a readback of the pre-stroke tile for tiles only resident on the GPU) or a `copy_texture_to_texture` of the start state that is read back lazily at push time.
5. **Hooks into the frame:** `invalidate_painted_tiles` and the composite cache currently key on CPU-written tiles; a GPU scratch layer composited on top (what invariant 5 describes) would be a new layer-over-layer pass in `TileCompositor`.

This is a design sketch tied to files, not a measured result. I did not build or time any GPU path.

## 6. Pen input audit (winit 0.30.13 from Cargo.lock; source read in `~/.cargo/registry`)

Aurora's app handles only `WindowEvent::CursorMoved`, `MouseInput`, `MouseWheel`, `CursorLeft` for pointing
(`aurora-app/src/lib.rs` ~100240-100254). Nothing in `aurora-app`, `aurora-gpu` or `aurora-ui` references
`Touch`, `Force`, `TouchPhase`, `AxisMotion`, `TouchpadPressure` or any pen/tablet API (grep). So **today no
pressure, tilt or rotation reaches the brush on any platform**; a pen acts as a mouse where the OS emulates one.

What winit 0.30.13 can deliver (reading its source):

| | pressure | tilt | rotation (twist) |
|---|---|---|---|
| Linux X11 | only as raw `WindowEvent::AxisMotion {axis, value}` for non-scroll XI2 valuators on motion (`x11/event_processor.rs` ~1150-1170); no label, range or meaning, you must know the device's valuator layout; `Touch.force` is `None` ("TODO", ~1399) | same raw valuators (Wacom drivers expose tilt as valuators) | not exposed |
| Linux Wayland | none: no `tablet` (zwp_tablet_v2) code anywhere in `platform_impl/linux/wayland`; touch `force: None` | none | none |
| macOS | none from pens: tablet pressure/tilt/rotation live on mouse `NSEvent`s and winit's `view.rs` reads none of them; the only pressure event is `TouchpadPressure` (force-touch trackpad, `pressureChangeWithEvent:`) and `Touch` is unsupported | none | none |
| Windows | `WM_POINTER*` pen/touch is converted to `WindowEvent::Touch` with `force: Normalized(pressure/1024)` for `PT_PEN` (`windows/event_loop.rs` ~1985-2125, `normalize_pointer_pressure`) | discarded (winit reads `pen_info.pressure` only, not `tiltX/tiltY/rotation`) | discarded |

Missing and the options:
- All platforms lack tilt and twist through winit 0.30; macOS and Wayland lack even pressure.
- Windows: the pen's pressure arrives as `Touch` events, which Aurora ignores. I could **not verify** whether the pen also produces `CursorMoved`/`MouseInput` there: winit's `WM_POINTER` handler returns 0 without calling `DefWindowProc` (so Windows may not synthesise mouse messages for the pen), in which case a pen would currently deliver no strokes at all. This needs a Windows tablet run before anyone relies on either answer.
- Options (none tried): (1) call platform APIs next to winit using the raw window handle: Windows `GetPointerPenInfo` (pressure, tilt, rotation) inside a window subclass/hook, macOS `NSEvent` tablet fields from a view method, X11 `XIQueryDevice` valuator labels/ranges, Wayland `zwp_tablet_v2` via `wayland-client`; (2) a tablet crate: from memory (not verified) `octotablet` covers Wayland tablet-v2, X11 XI2 and Windows Ink but I believe not macOS; (3) winit upgrade/fork: from memory (not verified, and not in the registry here) winit's 0.31 line reworks pointer events with a tablet-tool kind carrying force, tilt and twist, but it is a breaking upgrade touching `accesskit_winit` and the whole event loop; (4) WinTab on Windows as a legacy fallback.
- Not verified at all: any real tablet on any platform, Wayland compositors' behaviour of emulating a pointer from a tablet tool, X11 axis numbering on a real Wacom device.

## 7. Recommended next step

(Step 2 was done in task 170; see section 9.)

1. Decide the pen path first; it gates everything. A one-day spike per platform that logs raw pressure/tilt/twist from the native API (start with Windows `GetPointerPenInfo` and macOS `NSEvent`) next to winit. Without tilt/twist the bristle model degrades to pressure-only; without pressure on macOS and Wayland it is a mouse brush there.
2. Before touching the GPU, write a purpose-built CPU bristle deposit (tile-binned, f32 working tile, no per-dab hash lookups, optionally rayon across tiles) and re-measure; the measured 37-48 ns/texel leaves room and the CPU path keeps `TileStore` authoritative with no readback problem. Target: N = 512 at r = 150 under 4 ms at 800 px/s.
3. Only then, if pick-up/mixing (wet paint) is wanted, scope a GPU scratch layer (section 5), which is a new-infrastructure project (no compute plumbing, no readback today).
4. Keep the bristle count/radius defaults inside the measured envelope (N <= 128, r <= 50 first), and treat 1024 bristles as a quality setting that needs the faster kernel.

## 8. What was not verified

No real tablet or pen device; no Mac, Windows or Wayland run; the pen-input audit is source reading, with the
unverified claims marked above; GPU section is analysis only; timings are one machine (single thread, not
idle: other sessions ran), release and dev only; the stroke picture was seen only once and is not a quality
claim; the model has no canvas pick-up; no live-app or composite/upload/present cost was measured; the
speed-sweep and 120 Hz figures come from synthetic strokes, not human pen data.

## 9. Task 170: a purpose-built batch deposit (measured)

Branch `170-bristle_deposit`. Code: `crates/aurora-brush/src/deposit.rs` (`deposit_soft_dabs`,
`deposit_soft_dabs_with`, `Mode`), tests in `deposit_tests.rs`, the measurement `bristle_deposit_measure` in
`bristle_tests.rs`. `BristleBrush::deposit` now uses the batch deposit; the spike's per-dab deposit is kept as
`BristleBrush::deposit_per_dab` as the reference. Still experimental: no tool, no UI, no tokens, no saving.
`stamp_dab_amount` and every app brush are unchanged (no line of `stamp.rs` changed).

**What it does.** One pen sample's dabs are binned by the tiles their bounding boxes reach (a sort of
`(tile, dab index)` pairs, so each tile's dabs stay in input order). Per tile: one `TileStore::get_mut`, one
take of the stroke coverage tile, one snapshot capture at most; per dab only a `raw_content` lookup. Each dab
walks only the rows and columns its disc (grown by 2 texels) can reach instead of its whole box, and a texel's
four channels go through one `f16 -> f32` and one `f32 -> f16` conversion call (`half`'s slice conversions:
F16C on this machine) instead of eight scalar calls. A batch whose dab boxes hold at least 32 K texels
(`PARALLEL_MIN_TEXELS`, an untuned engineering default) runs on a lazily built 4-thread rayon pool
(`aurora-deposit-*`), split into bands of 64 rows (4 per tile); each band copies its rows of the tile out
of the store's shared buffer (`peek_shared`), applies its tile's dabs in order, and the changed bands are
copied back on the calling thread, which also captures the snapshot from the untouched original.

**Not done, and why.** An `f32` working tile (the findings' suggestion) is deliberately not used: the
per-dab path decides capture, the painted list, the dirty rectangle and (without a snapshot) the next dab's
start by comparing the rounded `f16` against the stored bits after every dab, so keeping `f32` between dabs
would change those decisions. `hypot` and `powf(1.5)` per texel (about 3.4 + 7.2 ns here, measured in a
micro-benchmark) are kept because a cheaper falloff would change bits; they are now most of the per-texel
cost.

**Exactness.** The batch must leave the same tile bits as `stamp_dab_amount(.., None, false, false, amount)`
per dab in order. The argument (in the module doc): every value a texel's update reads is per texel (stored
texel, stroke coverage) or per tile (stroke-start content), each texel sees its dabs in the original order,
the start content is the snapshot's raw copy or the untouched tile (equal by construction), the arithmetic is
a copy of `stamp_dab_impl`'s soft unlocked uncovered branch, and the skipped texels have falloff 0. Proved by
tests, all passing: `batch_deposit_equals_per_dab_stamping_bit_for_bit_with_a_stroke_snapshot` (16 seeds x 3
modes, 2 strokes x 4 random batches: clustered, scattered, tile edges and corners, negative and >2^22
centres, texel-aligned centres, degenerate and NaN radii, dabs reaching no texel, out-of-range colours,
random opacity/flow, seeded tiles with NaN/inf/negative texels; compares painted tiles, which tiles exist,
every texel bit, every dirty rectangle, the captured tiles and their raw copies, every stroke-coverage value,
then undo and redo of both strokes), `..._when_tiles_page_to_disk` (budget 3: tiles evict and page back in;
dirty rectangles compared for presence only because an evicted dirty tile reports the whole tile and the LRU
order differs), `..._with_no_snapshot`, `heavily_overlapping_dabs_keep_their_order` (400 dabs on the same
texels; the reversed order is shown to give different bits, so the test can see order),
`the_four_channel_conversions_match_one_at_a_time` (every `f16` bit pattern, every rounding midpoint and one
ulp either side, specials, 200,000 random `f32`), `an_empty_batch_and_dabs_that_reach_nothing_change_nothing`,
`a_snapshot_for_another_surface_is_ignored_like_the_per_dab_path`, and a whole bristle stroke through both
deposits (`the_batch_deposit_paints_the_same_stroke_as_the_per_dab_deposit`). Six hand mutations were run
and each failed the differential tests (reversed dab order in a tile, a too-tight row limit, a dirty rectangle
from the last dab only, the start content ignoring the raw copy in the serial and in the parallel path,
capturing the painted copy instead of the original); a zero margin on the row limit survived, as expected
(the margin is a safety allowance, not needed on this machine's arithmetic).

**Refused, not differing.** `SoftDab` has no field for selection coverage, the transparency lock or the hard
Pencil dab, so the batch only takes the plain soft case. Known differences that are not pixels: the tile
content-version *numbers* (the per-dab path bumps a tile once per dab, the batch once, or twice when the
parallel path writes it back), LRU recency (so which tiles a small budget evicts), and a failed page-in skips
the tile for every dab of the batch where the per-dab path would retry it for each dab.

### Results (release, same method and machine as section 2), microseconds per pen sample

Machine: i3-10100 (4 cores / 8 threads, 1 MiB L2, 6 MiB L3), Linux. 1-minute load average 1.68 before the
run and 2.63 after (other login sessions, plus the deposit's own 4 threads while it ran); nothing was built
during the run. The three paths of each row ran back to back on fresh stores, per-dab first, n = 2,400 samples
each. PerDab = the task 168 path (`stamp_dab_amount` per dab), Serial = the batch deposit on the calling
thread, Batch = the batch deposit as the brush runs it (`Mode::Auto`: on the pool above 32 K texels). The
PerDab column re-measures the task 168 numbers of section 3: within about 4 % for N >= 128, while the
N = 32 rows at r = 12 and r = 48 are 23 % and 11 % lower than then (not investigated). Mean / median / p99:

Circle at 240 Hz, 800 px/s (3.3 px/sample):

| brush | r | PerDab | Serial | Batch | dabs/sample |
|---|---|---|---|---|---|
| N=32 | 12 | 137 / 132 / 215 | 116 / 109 / 197 | 124 / 121 / 202 | 77 |
| N=128 | 12 | 286 / 281 / 443 | 225 / 223 / 320 | 227 / 223 / 354 | 424 |
| N=512 | 12 | 535 / 534 / 668 | 455 / 451 / 581 | 482 / 472 / 688 | 1,694 |
| N=1024 | 12 | 1,025 / 1,020 / 1,433 | 842 / 842 / 1,057 | 931 / 922 / 1,370 | 3,388 |
| N=32 | 48 | 418 / 406 / 695 | 296 / 286 / 539 | 294 / 284 / 508 | 19 |
| N=128 | 48 | 918 / 911 / 1,271 | 688 / 673 / 1,029 | 666 / 664 / 959 | 154 |
| N=512 | 48 | 2,091 / 2,068 / 2,752 | 1,727 / 1,701 / 2,316 | 1,436 / 1,467 / 2,067 | 1,227 |
| N=1024 | 48 | 3,115 / 3,095 / 5,084 | 2,680 / 2,703 / 3,764 | 2,186 / 2,197 / 3,271 | 3,219 |
| N=32 | 150 | 1,256 / 1,184 / 2,801 | 872 / 808 / 2,279 | 696 / 688 / 1,482 | 6 |
| N=128 | 150 | 2,548 / 2,489 / 4,161 | 1,773 / 1,723 / 2,956 | 1,030 / 969 / 2,228 | 49 |
| **N=512** | **150** | **5,241 / 5,248 / 7,345** | 3,824 / 3,760 / 5,657 | **1,775 / 1,696 / 3,088** | 394 |
| N=1024 | 150 | 7,476 / 7,530 / 10,556 | 5,642 / 5,681 / 7,869 | 2,578 / 2,530 / 3,769 | 1,113 |

Pen speed sweep, circle at 240 Hz, mean / p99:

| speed (px/sample) | case | PerDab | Serial | Batch |
|---|---|---|---|---|
| 200 px/s (0.8) | N=512 r=48 | 584 / 909 | 473 / 724 | 442 / 603 |
| | N=1024 r=48 | 821 / 1,058 | 731 / 1,077 | 685 / 1,037 |
| | N=512 r=150 | 1,884 / 3,039 | 1,318 / 2,238 | 829 / 1,494 |
| | N=1024 r=150 | 2,672 / 4,858 | 2,019 / 3,800 | 1,062 / 1,588 |
| 800 px/s (3.3) | N=512 r=48 | 2,092 / 2,784 | 1,691 / 2,202 | 1,431 / 2,167 |
| | N=1024 r=48 | 3,117 / 4,529 | 2,700 / 3,775 | 2,166 / 3,075 |
| | N=512 r=150 | 5,240 / 7,795 | 3,790 / 5,474 | 1,814 / 2,854 |
| | N=1024 r=150 | 7,468 / 10,635 | 5,713 / 7,581 | 2,559 / 3,645 |
| 1,600 px/s (6.7) | N=512 r=48 | 4,257 / 5,810 | 3,509 / 4,895 | 2,662 / 3,979 |
| | N=1024 r=48 | 6,328 / 9,253 | 5,485 / 7,369 | 4,127 / 6,201 |
| | N=512 r=150 | 11,134 / 17,101 | 8,077 / 11,089 | 3,488 / 6,125 |
| | N=1024 r=150 | 15,637 / 23,037 | 12,226 / 17,236 | 4,977 / 7,230 |
| 3,200 px/s (13.3) | N=512 r=48 | 8,381 / 10,844 | 6,920 / 8,989 | 5,055 / 7,701 |
| | N=1024 r=48 | 12,477 / 17,891 | 10,838 / 14,195 | 7,884 / 12,094 |
| | N=512 r=150 | 21,216 / 28,720 | 15,809 / 21,489 | 6,333 / 9,389 |
| | N=1024 r=150 | 30,200 / 40,609 | 23,594 / 30,730 | 9,342 / 13,078 |
| 6,400 px/s (26.7) | N=512 r=48 | 16,015 / 22,226 | 13,497 / 18,311 | 9,596 / 14,822 |
| | N=1024 r=48 | 23,485 / 34,871 | 20,914 / 29,713 | 15,015 / 23,606 |
| | N=512 r=150 | 39,761 / 57,145 | 30,440 / 42,926 | 11,764 / 17,699 |
| | N=1024 r=150 | 54,770 / 78,581 | 44,163 / 62,519 | 17,079 / 26,990 |

Undo capture is unchanged by construction (the same tiles, e.g. 12.9 per 300-sample stroke for N=512 r=150
in all three paths).

**Target: met.** N = 512 at r = 150, 800 px/s: mean 1.78 ms (median 1.70, p99 3.09) against 4 ms, from
5.24 ms (p99 7.34): 2.95x on the mean. The serial kernel alone gives 1.37x (3.82 ms mean, p99 5.66: misses
4 ms at p99); the rest is the 4-thread pool, so the target is met only with the threads. In the quick runs
before the full one the same case measured 1.85 ms mean / 3.44 p99 (Batch) and 3.77-3.82 / 5.62-5.68
(Serial).

Reading the numbers:
- The gain grows with the dab radius: small dabs (r = 12, bristle radius about 0.75-2.8 px) gain 1.1-1.2x
  serially and nothing more from the pool; the big-dab cases (r = 150) gain 2.5-3.3x. At r = 12 `Batch` is
  2-10 % slower than `Serial` although most of its batches stay under the threshold and run the identical
  serial code; I did not establish why (idle pool threads spinning after a parallel batch, or run-order
  noise, are guesses). The threshold is untuned.
- Cost still scales with pen travel. With the batch, N = 512 r = 150 stays under the 4.17 ms 240 Hz
  throughput bound up to about 1,600 px/s (3.5 ms mean, p99 6.1) and under the 10 ms latency bound up to
  about 3,200 px/s (6.3 / 9.4); N = 1024 r = 150 up to about 1,600 px/s (5.0 / 7.2) for latency. r = 48
  with tiny bristle dabs gains least: N = 1024 at 3,200 px/s is 7.9 / 12.1 ms.
- Per dab texel the serial kernel is now roughly 40 ns on the r = 150 cases, from about 53 (**estimated**
  from dabs per sample and the disc area of the mean bristle radius, not counted); `hypot` plus `powf` are
  about 11 ns of it (micro-benchmark).

**Known weaknesses and unverified points.**
- One machine, Linux, release only, not idle (load 1.7-2.6); the pool's gain depends on free cores, and in
  the app the deposit would share the CPU with the frame-composite pool (4 threads), the background
  compositor and the UI thread: not measured. A second process-wide pool (4 threads, idle when unused) is new
  permanent state if this is ever wired in.
- Each parallel batch allocates a 512 KiB work tile per reached tile (zeroed by the allocator) and a 256 KiB
  coverage tile per newly reached tile; allocation cost is inside the numbers above but not studied.
- Exactness is proved on x86_64 with F16C only. The four-channel conversions and the per-texel libm calls are
  the same functions on both paths on any one platform, and the tests compare against the per-dab path on
  whatever machine runs them, but no Mac (aarch64 `fp16` conversions) or Windows run has happened.
- The dirty rectangle is compared exactly only while nothing is evicted; with eviction only its presence.
- Still only the tile-write cost of a pen sample: recomposite, upload, present and invalidation are not
  included, no live window, no real tablet, no human looked at a stroke made by the new path (its pixels are
  identical to the old path's by the tests).

