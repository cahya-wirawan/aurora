//! CI-gated regression check for the GPU-dependent half of the stroke
//! pipeline: uploading a dirtied tile into `aurora_gpu::TileResidency`'s
//! atlas and compositing it via [`crate::TileCompositor`]. See
//! `aurora-tile`'s own `store.rs` test module
//! (`paint_and_dirty_round_trip_stays_within_a_tight_cpu_budget`) for
//! this check's CPU-only counterpart, which carries a far tighter,
//! genuinely hardware-independent budget; this module's threshold is
//! deliberately looser, for reasons specific to what this half measures.
//!
//! `spike/FINDINGS.md` measured "stroke latency (input -> frame
//! submitted)" end to end on two real dev machines: ~9.1ms p99 on a
//! 2019 AMD mobile GPU, ~2.0ms p99 on a discrete RTX 3090 — both against
//! a 10ms budget with, per that document's own words, "little
//! headroom." This module cannot reproduce that number: there is no
//! real frame/present loop yet (that needs a window, `aurora-app`,
//! still M1.8), so what it measures instead is the closest real
//! substitute available today — the CPU-side cost of encoding and
//! submitting (not waiting for completion of) one tile's upload
//! (`TileResidency::sync`) and composite (`TileCompositor::composite_over`)
//! against a real GPU device.
//!
//! The threshold here is deliberately generous, not the PRD's 10ms
//! brush budget itself: CI runners' GPU availability is completely
//! uncontrolled, from a real discrete GPU down to a weak or software
//! adapter, down to no adapter at all (in which case this whole check
//! is skipped, matching every other real-GPU test in this workspace —
//! see `real_context`'s own doc comment). A tight assertion here would
//! either be consistently blown on weak CI hardware regardless of code
//! correctness, or so loose it's never enforced by real hardware.
//! Measuring submission time rather than completion time sidesteps most
//! of that variance already (driver/CPU submission overhead dominates,
//! not GPU execution speed) — the trip-wire this check is actually
//! aimed at is a regression class this project has already hit once
//! (`spike/FINDINGS.md` finding #4: a naive implementation re-uploaded
//! the whole visible grid every frame instead of just the changed
//! tile), or an accidental blocking wait creeping into a path that
//! should stay non-blocking (§7.3.4) — either of which would show up as
//! a latency spike far larger than this budget, not a borderline miss.

#![cfg(test)]

use std::time::{Duration, Instant};

use aurora_gpu::TileResidency;
use aurora_tile::{TILE, TileId, TileStore};
use half::f16;

use crate::TileCompositor;
use crate::test_support::real_context;

fn tile_store() -> (tempfile::TempDir, TileStore) {
    let dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(err) => unreachable!("tempdir creation must succeed in a test environment: {err}"),
    };
    let Some(budget) = std::num::NonZeroUsize::new(16) else {
        unreachable!("16 is non-zero");
    };
    let store = match TileStore::new(dir.path().to_path_buf(), budget) {
        Ok(store) => store,
        Err(err) => unreachable!("scratch dir just created by tempfile must be usable: {err}"),
    };
    (dir, store)
}

fn percentile(samples: &mut [Duration], pct: usize) -> Duration {
    samples.sort_unstable();
    let index = (samples.len() * pct / 100).min(samples.len().saturating_sub(1));
    match samples.get(index) {
        Some(&value) => value,
        None => unreachable!("samples is non-empty by construction"),
    }
}

#[test]
fn upload_and_composite_one_dirtied_tile_stays_within_a_generous_ci_budget() {
    const ITERATIONS: usize = 30;

    let Some(context) = real_context() else {
        return;
    };
    let device = context.device();
    let queue = context.queue();
    let (_dir, mut store) = tile_store();
    let id = TileId { x: 0, y: 0 };

    // A one-tile viewport -> a 2x2 slot grid (`TileResidency::new`'s own
    // `viewport/TILE + 1` sizing), so only the one tile this loop keeps
    // touching stays dirty across iterations after the first (cold
    // start) sync -- the steady-state cost a real, continuous stroke on
    // an already-visible tile would actually pay.
    let mut residency = TileResidency::new(device, queue, (TILE, TILE));
    let dst = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("latency-dst"),
        size: wgpu::Extent3d {
            width: TILE,
            height: TILE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());
    let mut compositor = TileCompositor::new(device);

    let mut samples = Vec::with_capacity(ITERATIONS);
    for i in 0..ITERATIONS {
        {
            let Ok(tile) = store.get_mut(id) else {
                unreachable!("test-local scratch store must accept this");
            };
            let value = f16::from_f32(f32::from(u8::from(i % 2 == 0)));
            for sample in tile.texels_mut() {
                *sample = value;
            }
            tile.mark_dirty(aurora_core::Rect {
                x: 0,
                y: 0,
                width: TILE,
                height: TILE,
            });
        }

        let start = Instant::now();
        let _stats = residency.sync(queue, &mut store, false, usize::MAX);
        let src_view = residency.view();
        compositor.composite_over(&context, &dst_view, src_view);
        samples.push(start.elapsed());
    }
    // Flush before the test ends, tidy rather than load-bearing --
    // nothing here reads the result back.
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });

    let p95 = percentile(&mut samples.clone(), 95);
    let p99 = percentile(&mut samples, 99);
    eprintln!(
        "upload+composite submission latency over {ITERATIONS} iterations: p95={p95:?} p99={p99:?}"
    );

    assert!(
        p95 < Duration::from_millis(200),
        "upload+composite p95 submission latency regressed: {p95:?} \
         (generous CI budget: 200ms -- see this module's own doc comment \
         for why this threshold is loose rather than tight)"
    );
}
