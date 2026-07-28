//! Shared "get a real device or skip" pattern for every test in this
//! crate that needs actual GPU hardware — real verification, not a mock,
//! matching this project's general practice.

use crate::pipeline::PipelineKey;
use crate::{GpuContext, GpuError};
use std::num::NonZeroUsize;
use std::sync::{Mutex, MutexGuard};

/// Serializes every real-GPU test in this crate.
///
/// Confirmed necessary, not a theoretical precaution: running this
/// crate's GPU tests under `cargo test`'s default multi-threaded runner
/// (several tests each creating their own `wgpu::Instance`/`Device` and
/// submitting real work concurrently) reproducibly deadlocked on this
/// machine — one test would report done and the rest would simply never
/// return, with the GPU sitting idle (confirmed via `nvidia-smi`).
/// Single-threaded (`--test-threads=1`) never deadlocked. `cargo-nextest`
/// (the runner CLAUDE.md names for CI, which isolates each test in its
/// own process rather than a thread) may well not have this problem at
/// all — but this crate has to be correct under plain `cargo test` too,
/// which is what's actually installed here, so every real-GPU test holds
/// this lock for its duration rather than assuming a particular runner.
static GPU_TEST_LOCK: Mutex<()> = Mutex::new(());

/// Bundles the lock guard with the context so holding one holds the
/// other — `Deref` means every existing call site (`context.device()`,
/// `context.queue()`, ...) keeps working unchanged.
pub(crate) struct GpuTestContext {
    _guard: MutexGuard<'static, ()>,
    context: GpuContext,
}

impl std::ops::Deref for GpuTestContext {
    type Target = GpuContext;
    fn deref(&self) -> &GpuContext {
        &self.context
    }
}

/// A real, tempdir-backed `TileStore` for tests — the tempdir must
/// outlive the store (it's the scratch directory), so both are returned
/// together; the caller keeps `_dir` alive by binding it, even unused.
pub(crate) fn real_tile_store(budget: usize) -> (tempfile::TempDir, aurora_tile::TileStore) {
    let dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(err) => unreachable!("tempdir creation must succeed in a test environment: {err}"),
    };
    let Some(budget) = NonZeroUsize::new(budget) else {
        unreachable!("test budgets are always non-zero literals");
    };
    let store = match aurora_tile::TileStore::new(dir.path().to_path_buf(), budget) {
        Ok(store) => store,
        Err(err) => unreachable!("scratch dir just created by tempfile must be usable: {err}"),
    };
    (dir, store)
}

/// `NoSuitableAdapter` is an inconclusive skip: this pass can only
/// confirm real device creation on this dev machine's actual GPU (see
/// `spike/FINDINGS.md`'s Linux/Vulkan/RTX 3090 run) — whether every CI
/// runner image has a usable GPU or software adapter is genuinely
/// unverified, not assumed. Any other error means an adapter *was*
/// found but device/queue creation itself failed, which is a real bug.
pub(crate) fn real_context() -> Option<GpuTestContext> {
    let guard = GPU_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match GpuContext::new() {
        Ok(context) => Some(GpuTestContext {
            _guard: guard,
            context,
        }),
        Err(GpuError::NoSuitableAdapter) => {
            eprintln!("SKIPPED: no GPU adapter available on this machine/CI runner");
            None
        }
        Err(err) => {
            // The one deliberate, visible `panic!` in this crate's test
            // support: a real, reachable failure (not `unreachable!()`'s
            // territory), and the test-failure mechanism this codebase
            // uses in exactly this spot already (see the original in
            // context.rs's own test).
            #[allow(clippy::panic)]
            {
                panic!("device request failed with a real adapter present: {err}");
            }
        }
    }
}

/// The canvas shader's bind-group layout (texture + sampler + uniform),
/// matching `spike/vertical-slice`'s `canvas_layout` exactly — shared by
/// every test that needs to build a real canvas pipeline.
pub(crate) fn canvas_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("canvas"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    })
}

/// Builds a real canvas render pipeline from `key` — shared by the
/// pipeline-cache test (which only cares that this gets called the right
/// *number* of times) and the end-to-end render test (which cares that
/// the pipeline it builds actually draws correct pixels).
pub(crate) fn build_canvas_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
    key: &PipelineKey,
) -> wgpu::RenderPipeline {
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("canvas"),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("canvas"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(key.vertex_entry),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(key.fragment_entry),
            targets: &[Some(wgpu::ColorTargetState {
                format: key.target_format,
                blend: key.blend.to_wgpu(),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        multiview_mask: None,
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        cache: None,
    })
}
