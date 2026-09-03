//! Real GPU proof that uploads land in the *correct* slot region — a
//! call-count-only test (see `residency.rs`'s own tests) can't catch a
//! wrong-axis or off-by-one slot-offset bug; this reads the atlas
//! texture back and checks actual pixel content, matching this crate's
//! established practice (`render_test.rs`).

#![cfg(test)]

use crate::TileResidency;
use crate::test_support::{real_context, real_tile_store};
use aurora_tile::{SurfaceId, TILE, TileId};
use half::f16;

#[test]
// One linear setup-render-readback flow for a single real GPU test --
// splitting it into helper functions would just relocate the same lines
// without reducing the actual complexity (texture, buffer, bind group,
// pass, readback) -- same call already made for render_test.rs.
#[allow(clippy::too_many_lines)]
fn upload_lands_in_the_correct_slot() {
    let Some(context) = real_context() else {
        return;
    };
    let device = context.device();
    let queue = context.queue();
    let (_dir, mut store) = real_tile_store(64);

    // 256x256 viewport -> grid (2, 2): tile (1, 0) maps to slot (1, 0),
    // i.e. the texture's second column, first row.
    let viewport = (256, 256);
    let mut residency = TileResidency::new(device, queue, viewport);

    let surface = SurfaceId::from_raw(0);
    let target_tile = TileId { x: 1, y: 0 };
    {
        let tile = match store.get_mut(surface, target_tile) {
            Ok(tile) => tile,
            Err(err) => unreachable!("test-local scratch store must accept this: {err}"),
        };
        for (i, sample) in tile.texels_mut().iter_mut().enumerate() {
            // R, G, B, A = 0, 1, 0, 1 (opaque green) -- distinct from the
            // zero-initialized blank default so a wrong-slot read is
            // visibly wrong, not accidentally matching.
            let channel = match i % 4 {
                1 | 3 => 1.0,
                _ => 0.0,
            };
            *sample = f16::from_f32(channel);
        }
        tile.mark_dirty(aurora_core::Rect {
            x: 0,
            y: 0,
            width: TILE,
            height: TILE,
        });
    }

    let stats = residency.sync(queue, &mut store, surface, false, usize::MAX);
    assert_eq!(
        stats.uploaded, 4,
        "first sync uploads the whole visible grid"
    );

    // Read back exactly the slot-(1,0) region: x in [256, 512), y in [0, 256).
    let bytes_per_row = TILE * 8; // Rgba16Float = 8 bytes/px; 256*8 = 2048, already
    // a multiple of wgpu's 256-byte copy alignment -- no row padding needed.
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("residency-readback"),
        size: u64::from(bytes_per_row) * u64::from(TILE),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("residency-readback"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: residency.texture(),
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: TILE,
                y: 0,
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(TILE),
            },
        },
        wgpu::Extent3d {
            width: TILE,
            height: TILE,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));

    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
    let Ok(Ok(())) = rx.recv() else {
        unreachable!("map_async must complete once the device has been polled to idle");
    };

    let Ok(data) = slice.get_mapped_range() else {
        unreachable!("the buffer was just confirmed mapped successfully above");
    };
    let Some(first_texel) = data.get(0..8) else {
        unreachable!("a 256x256 Rgba16Float readback buffer is at least 8 bytes");
    };
    match first_texel {
        [r0, r1, g0, g1, b0, b1, a0, a1] => {
            let r = f16::from_le_bytes([*r0, *r1]).to_f32();
            let g = f16::from_le_bytes([*g0, *g1]).to_f32();
            let b = f16::from_le_bytes([*b0, *b1]).to_f32();
            let a = f16::from_le_bytes([*a0, *a1]).to_f32();
            assert_eq!(
                (r, g, b, a),
                (0.0, 1.0, 0.0, 1.0),
                "slot (1,0) must hold the painted tile's own colour, not a neighbour's"
            );
        }
        _ => unreachable!("sliced exactly 8 bytes"),
    }
    drop(data);
    readback.unmap();
}

#[test]
// Proves `resize` actually leaves a usable atlas behind, not just a
// struct that no longer panics: after growing the atlas, a real upload
// through `sync` must land in the *new* grid's slot region, matching
// `upload_lands_in_the_correct_slot`'s own readback rigor rather than
// just asserting the resize call didn't panic.
#[allow(clippy::too_many_lines)]
fn resize_then_upload_lands_in_the_correct_slot() {
    let Some(context) = real_context() else {
        return;
    };
    let device = context.device();
    let queue = context.queue();
    let (_dir, mut store) = real_tile_store(64);

    // Start at a 256x256 viewport (grid (2, 2)), then grow to 512x512
    // (grid (3, 3)): tile (2, 0) only exists in the *new* grid, mapping
    // to slot (2, 0), the atlas's third column, first row.
    let mut residency = TileResidency::new(device, queue, (256, 256));
    residency.resize(device, queue, (512, 512));

    let surface = SurfaceId::from_raw(0);
    let target_tile = TileId { x: 2, y: 0 };
    {
        let tile = match store.get_mut(surface, target_tile) {
            Ok(tile) => tile,
            Err(err) => unreachable!("test-local scratch store must accept this: {err}"),
        };
        for (i, sample) in tile.texels_mut().iter_mut().enumerate() {
            // R, G, B, A = 1, 0, 1, 1 (opaque magenta) -- distinct from
            // this file's other test colour (green) and from a blank
            // tile, so a wrong-slot read after resize is visibly wrong.
            let channel = match i % 4 {
                0 | 2 | 3 => 1.0,
                _ => 0.0,
            };
            *sample = f16::from_f32(channel);
        }
        tile.mark_dirty(aurora_core::Rect {
            x: 0,
            y: 0,
            width: TILE,
            height: TILE,
        });
    }

    let stats = residency.sync(queue, &mut store, surface, false, usize::MAX);
    assert_eq!(
        stats.uploaded, 9,
        "post-resize sync uploads the whole (3,3) visible grid"
    );

    // Read back exactly the slot-(2,0) region: x in [512, 768), y in [0, 256).
    let bytes_per_row = TILE * 8; // Rgba16Float = 8 bytes/px; 256*8 = 2048, already
    // a multiple of wgpu's 256-byte copy alignment -- no row padding needed.
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("residency-resize-readback"),
        size: u64::from(bytes_per_row) * u64::from(TILE),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("residency-resize-readback"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: residency.texture(),
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: TILE * 2,
                y: 0,
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(TILE),
            },
        },
        wgpu::Extent3d {
            width: TILE,
            height: TILE,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));

    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
    let Ok(Ok(())) = rx.recv() else {
        unreachable!("map_async must complete once the device has been polled to idle");
    };

    let Ok(data) = slice.get_mapped_range() else {
        unreachable!("the buffer was just confirmed mapped successfully above");
    };
    let Some(first_texel) = data.get(0..8) else {
        unreachable!("a 256x256 Rgba16Float readback buffer is at least 8 bytes");
    };
    match first_texel {
        [r0, r1, g0, g1, b0, b1, a0, a1] => {
            let r = f16::from_le_bytes([*r0, *r1]).to_f32();
            let g = f16::from_le_bytes([*g0, *g1]).to_f32();
            let b = f16::from_le_bytes([*b0, *b1]).to_f32();
            let a = f16::from_le_bytes([*a0, *a1]).to_f32();
            assert_eq!(
                (r, g, b, a),
                (1.0, 0.0, 1.0, 1.0),
                "post-resize slot (2,0) must hold the painted tile's own colour, in the new, bigger atlas"
            );
        }
        _ => unreachable!("sliced exactly 8 bytes"),
    }
    drop(data);
    readback.unmap();
}

#[test]
// Same linear setup-upload-readback shape as `upload_lands_in_the_correct_slot`
// above, targeting `upload_mip`'s non-zero mip level instead of `sync`'s
// mip level 0 -- proves the progressive-rendering GPU wiring lands
// downsampled bytes in the *correct* slot and level, not just that the
// call succeeds.
#[allow(clippy::too_many_lines)]
fn upload_mip_lands_in_the_correct_slot_and_level() {
    let Some(context) = real_context() else {
        return;
    };
    let device = context.device();
    let queue = context.queue();

    // 256x256 viewport -> grid (2, 2): tile (1, 0) maps to slot (1, 0).
    let viewport = (256, 256);
    let residency = TileResidency::new(device, queue, viewport);

    // Mip level 1 ("Half" in aurora-render's MipLevel): TILE/2 = 128.
    let level = 1u32;
    let size = TILE / 2;
    let mut texels = Vec::with_capacity((size * size * 4) as usize);
    for _ in 0..(size * size) {
        // Half-transparent magenta -- distinct from this file's other
        // test colour (green) and from a blank tile, so a wrong
        // slot/level reads as visibly wrong, not coincidentally right.
        //
        // **Translucent, deliberately (0.68.4).** This fixture was opaque
        // until then, and premultiplying an opaque texel is the identity
        // -- so the assertion below held whether or not `upload_mip`
        // premultiplied at all, and deleting that call left the suite
        // green. Alpha 0.5 makes the expected readback differ from the
        // uploaded straight-alpha texel, so the premultiply is now
        // actually load-bearing for this test.
        for channel in [1.0, 0.0, 1.0, 0.5] {
            texels.push(f16::from_f32(channel));
        }
    }

    let target_tile = TileId { x: 1, y: 0 };
    if let Err(err) = residency.upload_mip(queue, target_tile, level, &texels) {
        unreachable!("a correctly-sized texel buffer at a valid level must upload: {err}");
    }

    // Read back mip level 1's slot-(1,0) region: x in [128, 256), y in [0, 128).
    let bytes_per_row = size * 8; // Rgba16Float = 8 bytes/px; 128*8 = 1024,
    // already a multiple of wgpu's 256-byte copy alignment.
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("residency-mip-readback"),
        size: u64::from(bytes_per_row) * u64::from(size),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("residency-mip-readback"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: residency.texture(),
            mip_level: level,
            origin: wgpu::Origin3d {
                x: size,
                y: 0,
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(size),
            },
        },
        wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));

    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
    let Ok(Ok(())) = rx.recv() else {
        unreachable!("map_async must complete once the device has been polled to idle");
    };

    let Ok(data) = slice.get_mapped_range() else {
        unreachable!("the buffer was just confirmed mapped successfully above");
    };
    let Some(first_texel) = data.get(0..8) else {
        unreachable!("a 128x128 Rgba16Float readback buffer is at least 8 bytes");
    };
    match first_texel {
        [r0, r1, g0, g1, b0, b1, a0, a1] => {
            let r = f16::from_le_bytes([*r0, *r1]).to_f32();
            let g = f16::from_le_bytes([*g0, *g1]).to_f32();
            let b = f16::from_le_bytes([*b0, *b1]).to_f32();
            let a = f16::from_le_bytes([*a0, *a1]).to_f32();
            assert_eq!(
                (r, g, b, a),
                (0.5, 0.0, 0.5, 0.5),
                "slot (1,0) at mip level 1 must hold the uploaded preview's own colour, \
                 premultiplied: straight (1, 0, 1, 0.5) lands as (0.5, 0, 0.5, 0.5)"
            );
        }
        _ => unreachable!("sliced exactly 8 bytes"),
    }
    drop(data);
    readback.unmap();
}

/// Paints every texel of `id` one solid straight-alpha colour and marks
/// the whole tile dirty, exactly as a real edit would.
fn paint(store: &mut aurora_tile::TileStore, surface: SurfaceId, id: TileId, rgba: [f32; 4]) {
    let tile = match store.get_mut(surface, id) {
        Ok(tile) => tile,
        Err(err) => unreachable!("test-local scratch store must accept this: {err}"),
    };
    for (i, sample) in tile.texels_mut().iter_mut().enumerate() {
        let Some(&channel) = rgba.get(i % 4) else {
            unreachable!("i % 4 is always in range 0..4");
        };
        *sample = f16::from_f32(channel);
    }
    tile.mark_dirty(aurora_core::Rect {
        x: 0,
        y: 0,
        width: TILE,
        height: TILE,
    });
}

/// The first texel of the atlas slot `id` maps to, read back off the real
/// texture as `[r, g, b, a]` — **premultiplied**, which is the convention
/// the atlas holds (see `premultiply_rgba`).
fn atlas_first_texel(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    residency: &TileResidency,
    id: TileId,
) -> [f32; 4] {
    let texture = residency.texture();
    let grid = (texture.width() / TILE, texture.height() / TILE);
    assert!(
        grid.0 > 0 && grid.1 > 0,
        "the atlas is at least one tile on each axis"
    );
    let slot = (id.x % grid.0, id.y % grid.1);
    // Rgba16Float = 8 bytes/texel; at TILE = 256 that is 2048, already a
    // multiple of wgpu's 256-byte row alignment, so no padding arithmetic.
    let bytes_per_row = TILE * 8;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("residency-slot-readback"),
        size: u64::from(bytes_per_row) * u64::from(TILE),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("residency-slot-readback"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: slot.0 * TILE,
                y: slot.1 * TILE,
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(TILE),
            },
        },
        wgpu::Extent3d {
            width: TILE,
            height: TILE,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));

    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
    let Ok(Ok(())) = rx.recv() else {
        unreachable!("map_async must complete once the device has been polled to idle");
    };
    let Ok(data) = slice.get_mapped_range() else {
        unreachable!("the buffer was just confirmed mapped successfully above");
    };
    let mut out = [0.0_f32; 4];
    for (channel, dest) in out.iter_mut().enumerate() {
        let Some(&[lo, hi]) = data.get(channel * 2..channel * 2 + 2) else {
            unreachable!("one whole tile was copied, so its first 8 bytes exist");
        };
        *dest = f16::from_le_bytes([lo, hi]).to_f32();
    }
    drop(data);
    readback.unmap();
    out
}

/// Asserts an atlas texel matches `expected` within `f16` tolerance,
/// channel by channel — which is also what keeps `clippy::float_cmp`
/// satisfied where comparing the arrays outright would not.
fn assert_texel(actual: [f32; 4], expected: [f32; 4], context: &str) {
    for (channel, (got, want)) in actual.iter().zip(expected.iter()).enumerate() {
        assert!(
            (got - want).abs() < 0.01,
            "channel {channel}: got {got}, expected {want} -- {context}. Whole texel {actual:?} \
             against {expected:?}"
        );
    }
}

/// The one scratch file `(surface, id)` has on disk. Found by name rather
/// than through `TileStore`'s private `tile_path`: the leading per-store
/// `instance` component is deliberately unpredictable, the trailing
/// `_{surface}_{x}_{y}.tile` is not.
fn scratch_file(dir: &std::path::Path, surface: SurfaceId, id: TileId) -> std::path::PathBuf {
    let suffix = format!("_{}_{}_{}.tile", surface.to_raw(), id.x, id.y);
    let Ok(entries) = std::fs::read_dir(dir) else {
        unreachable!("the scratch directory lives as long as the store");
    };
    let mut found: Vec<std::path::PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.to_string_lossy().ends_with(&suffix) {
            found.push(path);
        }
    }
    match found.as_slice() {
        [only] => only.clone(),
        other => unreachable!("exactly one scratch file must end in {suffix}: {other:?}"),
    }
}

#[test]
/// **RT-01, 0.91.1.** The regression guard for the gap `aurora-tile`
/// 0.91.0 opened in this very function: `sync` consumes a tile's dirty
/// record with `take_dirty` *before* the `store.get` that can fail, and
/// before 0.91.1 a failing `get` returned without touching `self.slots`.
/// The slot therefore still mapped to that tile id, so the *next* call's
/// resident check — which reads the slot map, not the store — said `true`,
/// the consumed record said "clean", and the tile was skipped for the life
/// of the mapping. Silently: `SyncStats` reported `errors: 0,
/// remaining: 0`, i.e. complete success, while the atlas held pre-edit
/// pixels indefinitely.
///
/// Newly reachable *because* of 0.91.0. Before it, `is_dirty` could only
/// be `true` for a resident tile, whose `get` cannot fail the way a
/// page-in can; 0.91.0 made it `true` for evicted tiles too, which is
/// exactly the case where the read hits the scratch disk.
///
/// Read back off the real atlas texture, not from `SyncStats` alone: the
/// failure mode is a tile store holding exactly the right pixels while the
/// GPU shows something else, so the stats *and* the content both have to
/// be checked — and on the broken code the stats look perfect.
///
/// The failure injection is the file simply not being there, which is one
/// real spelling of what `TileStore` itself can do to a tile: a transient
/// scratch-disk read error, `ENOSPC`, or its own
/// `discard_stale_scratch_file`/`cap_failed_writes` dropping a file after
/// a failed write.
#[allow(clippy::too_many_lines)]
fn a_failed_page_in_does_not_lose_an_evicted_tile_s_owed_upload() {
    let Some(context) = real_context() else {
        return;
    };
    let device = context.device();
    let queue = context.queue();
    // Budget 4 == the visible grid exactly, so one extra touch evicts.
    let (dir, mut store) = real_tile_store(4);
    let surface = SurfaceId::from_raw(0);
    let mut residency = TileResidency::new(device, queue, (256, 256));
    let visible = [
        TileId { x: 0, y: 0 },
        TileId { x: 1, y: 0 },
        TileId { x: 0, y: 1 },
        TileId { x: 1, y: 1 },
    ];
    let target = TileId { x: 0, y: 0 };
    let red = [1.0, 0.0, 0.0, 1.0];
    let blue = [0.0, 0.0, 1.0, 1.0];

    // Frame 1: red everywhere, and really in the atlas.
    for id in visible {
        paint(&mut store, surface, id, red);
    }
    let first = residency.sync(queue, &mut store, surface, false, usize::MAX);
    assert_eq!(first.uploaded, 4, "the whole visible grid uploads");
    assert_eq!((first.errors, first.remaining), (0, 0));
    assert_texel(
        atlas_first_texel(device, queue, &residency, target),
        red,
        "the red frame must really be in the atlas, or this test proves nothing",
    );

    // The edit that must not be lost.
    paint(&mut store, surface, target, blue);

    // Force `target` out of the store *while dirty*, by making it the
    // least-recently-used tile and then touching a fifth one. This is the
    // scenario `aurora-tile` 0.91.0 exists for.
    for id in visible {
        if id != target && store.get(surface, id).is_err() {
            unreachable!("a resident tile must serve");
        }
    }
    if store.get(surface, TileId { x: 7, y: 7 }).is_err() {
        unreachable!("a blank tile is always materializable");
    }
    // Confirm the write, so `target`'s only copy is the file on disk and
    // the page-in below must actually read it (the `pending` reinstate
    // path cannot fail, so it would defeat the injection).
    if let Err(err) = store.flush() {
        unreachable!("the scratch write must land: {err}");
    }
    assert!(
        !store.is_resident(surface, target),
        "the budget must really have evicted it, or this test exercises nothing"
    );
    assert!(
        store.is_dirty(surface, target),
        "evicted while dirty, so an upload is still owed (aurora-tile 0.91.0)"
    );

    // Frame 2, with the page-in guaranteed to fail.
    let path = scratch_file(dir.path(), surface, target);
    let hidden = path.with_extension("hidden");
    if let Err(err) = std::fs::rename(&path, &hidden) {
        unreachable!("renaming a file this test just watched appear: {err}");
    }
    let failed = residency.sync(queue, &mut store, surface, false, usize::MAX);
    assert_eq!(failed.errors, 1, "the injected read failure must be seen");
    assert_eq!(failed.uploaded, 0, "and nothing can have been uploaded");
    assert_eq!(
        failed.remaining, 1,
        "reported as still owed, which is the honest answer"
    );
    // The store no longer remembers anything: the record was consumed
    // before the read that failed. Only the atlas dropping its own slot
    // mapping can bring this tile back -- which is the fix under test.
    assert!(
        !store.is_dirty(surface, target),
        "consumed by `take_dirty` before the failing `get`"
    );

    // Frame 3: the transient failure is over.
    if let Err(err) = std::fs::rename(&hidden, &path) {
        unreachable!("restoring the file: {err}");
    }
    let recovered = residency.sync(queue, &mut store, surface, false, usize::MAX);
    assert_eq!(
        recovered.errors, 0,
        "the read works again, so nothing should error"
    );
    assert_eq!(
        recovered.uploaded, 1,
        "the tile whose page-in failed must be retried. Without the `slots.remove` in `sync`'s \
         error arm this is 0, with `errors: 0, remaining: 0` alongside it -- a reported success \
         that never uploads anything, for the life of the slot mapping."
    );
    assert_texel(
        atlas_first_texel(device, queue, &residency, target),
        blue,
        "and the atlas must show the edit; without the fix it still holds the previous frame's \
         red, permanently",
    );
}
