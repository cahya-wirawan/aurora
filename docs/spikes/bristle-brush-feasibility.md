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
