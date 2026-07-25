# Vertical slice — findings

PRD §13 Step 4. Measured 2026-07-26 on an **AMD Radeon Pro 5300M (Metal, macOS)** — a
2019-era mobile discrete GPU, so treat these as a slow-ish modern baseline rather
than a best case.

Reproduce: `cd spike/vertical-slice && cargo run --release -- --headless`

## What the slice covers

Window → `wgpu` → 100,000 × 100,000 px half-float tiled document (80 GB at
8 bytes/px) → per-tile composite → brush stroke → save/reload, with a docked
panel drawn in the same frame as the canvas. Memory budget deliberately set to
64 MB (128 resident tiles of 512 KB) so paging is exercised rather than avoided.

**Not covered here:** screen reader and CJK IME (ADR 0001's escape-hatch
triggers). Those are now a separate spike — see
[`a11y-ime/FINDINGS.md`](a11y-ime/FINDINGS.md) — which is **partially** done: the
tree builds and the adapter initializes, but the decisive human tests (does a
screen reader speak it, does CJK compose) have not been run. Also absent: tile compression, SIMD, threading — all
of which the real implementation has and this does not, so several numbers below
are pessimistic by design.

## Results

| Measurement | Budget | p50 | p95 | p99 | Verdict |
|---|---|---|---|---|---|
| Stroke latency (input → frame submitted) | 10 ms | 4.1 | 7.6 | 9.1 | **within, but tight** |
| Idle frame (UI + present, no canvas change) | 16.7 ms | 0.6 | 0.8 | 0.8 | comfortable |
| Pan with page-in from disk | 16.7 ms | 7.0 | 9.5 | 16.7 | within |
| Pan while painting | 16.7 ms | 29.2 | 33.6 | 34.6 | **over** |

Frame breakdown while painting and panning:

| Stage | p50 | Share |
|---|---|---|
| Stroke + merge (CPU) | 19.6 ms | ~65 % |
| Composite + upload | 6.0 ms | ~20 % |
| Draw + present | 4.7 ms | ~15 % |

Paging, over the full run: 665 tiles created, 930 evicted, 393 page faults,
488 MB written and 206 MB read from scratch.

Save/reload: 568 tiles, 298 MB at 260 MB/s write and 585 MB/s read, uncompressed.
Round-trip verified **bit-exact** — f16 in, f16 out, no conversion anywhere.

## What this validates

- **Invariant §7.3.1 holds.** An 80 GB document is editable in 64 MB of tile
  budget. Eviction and page-in work, and page-in panning stays within frame
  budget. The tiled architecture is sound.
- **Invariant §7.3.8 holds.** Canvas and UI share one device and one render pass
  with no interop layer and no measurable cost — the idle frame is 0.6 ms.
- **ADR 0003 (half-float) is affordable.** Nothing here is bottlenecked on the
  2× memory of f16 versus 8-bit. The bottleneck is elsewhere entirely.
- **Round-trip precision is exact**, as invariant §7.3.6b requires.

## What it invalidates or corrects

### 1. Naive tile compositing is the bottleneck, not I/O

Going in, the assumed risk was disk paging. It is not: page-in panning runs at
7 ms. The real cost is CPU compositing — 65 % of a painting frame — because
`end_stroke` merges **whole tiles** regardless of how little the stroke touched,
converting f16 → f32 → f16 scalar over 262,144 texels per tile.

Consequences for the real implementation, none of which are surprising in
hindsight but all of which were unbudgeted:

- Tiles need **per-tile dirty rectangles**, so a merge touches only the affected
  region. This is the single biggest win available.
- Compositing belongs on the **GPU**, not the CPU. The slice does it CPU-side to
  keep the spike small; production should not.
- Where CPU compositing is unavoidable, it needs **SIMD** — scalar f16
  conversion is the specific hot path.

### 2. The 10 ms brush budget is achievable but has little headroom

p99 of 9.1 ms against a 10 ms budget, on a naive implementation with no
compression, no SIMD, no threading, and CPU compositing — but also with a small
brush (24 px) on a fast local disk. A larger brush touches more tiles and scales
this up roughly linearly.

The budget is not in danger, but it is **not comfortable either**. It should be
treated as a number to defend continuously in CI from Phase 1, not as settled.

### 3. Upload bandwidth sets a real pan-speed ceiling

At 256 px tiles and 8 bytes/px, a screenful is ~18 MB. A fast fling exposes a
full screenful per frame, and p99 degrades to 53 ms. Mitigations to design in
rather than retrofit: tile compression, and rendering a lower-resolution mip
while panning fast, refining when motion stops (progressive rendering is already
a §6 requirement — this is what it is for).

### 4. Toroidal slot addressing matters

First implementation cleared the whole slot map when the viewport moved,
re-uploading all 35 visible tiles every frame. Addressing slots as
`tile_index mod grid_size` means panning invalidates one row or column instead.
Worth building in from the start; it is awkward to retrofit because it changes
how UVs are computed.

## Recommended follow-ups before Phase 1

1. **Per-tile dirty rectangles** in `aurora-tile` — the largest single win.
2. **GPU-side compositing** in `aurora-render`; treat the CPU path as a fallback.
3. **Tile compression**, which the memory budget already assumes (ADR 0003) and
   this slice omits.
4. **The accessibility and IME spike** — still the biggest unmeasured risk in the
   project, and the one that could still overturn ADR 0001.
5. **A latency regression test in CI** from the first Phase 1 commit, since the
   brush budget has under 1 ms of margin today.

## Honest limitations

- One machine, one GPU, one OS. Windows and Linux are unmeasured, and `wgpu`'s
  DX12 and Vulkan backends may differ materially.
- Single-threaded throughout; no attempt at overlapping I/O with rendering.
- The brush is a circle with a falloff — no texture, no dual brush, no
  stabilization. A real brush engine does considerably more work per dab.
- 100,000 px, not the 300,000 px ceiling of ADR 0002. Tile-count scaling beyond
  this is untested, though the sparse store should be indifferent to it.
- No text rendering anywhere, so nothing here speaks to the cost of the widget
  toolkit's text stack.
