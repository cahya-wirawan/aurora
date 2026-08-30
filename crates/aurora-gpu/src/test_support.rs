//! Shared "get a real device or skip" pattern for every test in this
//! crate that needs actual GPU hardware — real verification, not a mock,
//! matching this project's general practice.
//!
//! Most of this module is `#[cfg(test)]`-private to `aurora-gpu`. The
//! exception is the skip-vs-fail *decision* ([`real_context_or_skip`]
//! and the pure predicates it is built from — [`gpu_error_action`],
//! [`require_gpu_from_value`],
//! [`adapter_is_software_when_gpu_required`]), which is also
//! reachable behind this crate's `test-support` Cargo feature so that
//! `aurora-render`, `aurora-widgets`, and `aurora-app` — each of which
//! compiles to its own test binary and so keeps its own lock and its own
//! context wrapper — share one copy of that decision instead of five.

#[cfg(test)]
use crate::pipeline::PipelineKey;
use crate::{GpuContext, GpuError};
#[cfg(test)]
use std::num::NonZeroUsize;
#[cfg(test)]
use std::sync::{Mutex, MutexGuard};

/// Environment variable that turns the "no GPU adapter" self-skip into a
/// hard test failure, *and* rejects a software rasterizer standing in for
/// real hardware.
///
/// Unset is off. Present, it is on unless its value is one of the
/// explicitly falsy spellings [`require_gpu_from_value`] documents. See
/// [`real_context_or_skip`].
pub const REQUIRE_GPU_ENV_VAR: &str = "AURORA_REQUIRE_GPU";

/// The falsy spellings of [`REQUIRE_GPU_ENV_VAR`], matched after trimming
/// surrounding whitespace and lowercasing.
const FALSY_VALUES: [&str; 5] = ["", "0", "false", "off", "no"];

/// What a real-GPU test helper should do when [`GpuContext::new`] fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuTestAction {
    /// No adapter, and no one asked for one: print `SKIPPED` and return
    /// `None`, the long-standing dev-box behaviour.
    Skip,
    /// No adapter, but `AURORA_REQUIRE_GPU` is set: fail the test.
    FailNoAdapter,
    /// An adapter *was* found and device/queue creation still failed —
    /// a real bug, and a hard failure regardless of the environment.
    FailDeviceCreation,
}

/// The skip-vs-fail decision as a pure function: the environment value
/// is passed in, never read here.
///
/// Deliberately not reading the environment itself. This workspace
/// denies `unsafe_code`, and `std::env::set_var` is `unsafe` as of
/// edition 2024, so a test can never flip the variable to exercise the
/// other branch — keeping the decision a plain predicate is what makes
/// both branches testable at all (see this module's own tests).
#[must_use]
pub fn gpu_error_action(err: &GpuError, require_gpu: bool) -> GpuTestAction {
    match err {
        GpuError::NoSuitableAdapter if !require_gpu => GpuTestAction::Skip,
        GpuError::NoSuitableAdapter => GpuTestAction::FailNoAdapter,
        _ => GpuTestAction::FailDeviceCreation,
    }
}

/// Whether a given [`REQUIRE_GPU_ENV_VAR`] value means "require a real
/// GPU" — `None` being "the variable is not set at all".
///
/// Presence alone deliberately does *not* mean on. Two concrete reasons,
/// both real rather than hypothetical:
///
/// * `wgpu-types` — a direct dependency of this very workspace —
///   documents the opposite convention for its own `WGPU_*` variables
///   ("if the value is `0`, the flag is unset"), so `AURORA_REQUIRE_GPU=0`
///   meaning *on* would contradict the one precedent a reader here is
///   most likely to have in mind.
/// * GitHub Actions sets a variable to the **empty string**, not to
///   nothing, when a `${{ }}` expression evaluates to empty or null. A
///   natural `AURORA_REQUIRE_GPU: ${{ matrix.gpu }}` would otherwise
///   silently turn this on for every matrix leg, including the ones with
///   no GPU — failing the whole matrix for a reason nothing in the
///   workflow file names.
///
/// So: unset is off; the empty string, `0`, `false`, `off` and `no` —
/// compared after trimming whitespace and ASCII-lowercasing — are off;
/// anything else present is on. A value that is not valid UTF-8 is on,
/// since it is certainly not one of those spellings.
#[must_use]
pub fn require_gpu_from_value(value: Option<&str>) -> bool {
    match value {
        None => false,
        Some(value) => !FALSY_VALUES.contains(&value.trim().to_ascii_lowercase().as_str()),
    }
}

/// Whether [`REQUIRE_GPU_ENV_VAR`] is set to a truthy value in this
/// process's environment — [`require_gpu_from_value`] holds the actual
/// rule and is where it is tested.
#[must_use]
pub fn require_gpu_from_env() -> bool {
    match std::env::var_os(REQUIRE_GPU_ENV_VAR) {
        None => false,
        Some(raw) => raw
            .to_str()
            .is_none_or(|value| require_gpu_from_value(Some(value))),
    }
}

/// Whether an adapter that *was* successfully created should nonetheless
/// be treated as a failure — the second half of what
/// [`REQUIRE_GPU_ENV_VAR`] promises.
///
/// Finding *an* adapter is not the same as finding real hardware. The gap
/// CLAUDE.md names ("a typical Linux dev box here has only Mesa llvmpipe
/// software rendering... a green test run is not evidence that canvas or
/// UI work is correct") survives in its most likely CI form: a runner
/// that is *supposed* to have a GPU, whose driver silently fell back to a
/// software rasterizer — llvmpipe on Linux, WARP / "Microsoft Basic
/// Render Driver" on Windows. `wgpu` reports both as
/// [`wgpu::DeviceType::Cpu`], so requiring a GPU means rejecting that.
///
/// Only `Cpu` is rejected, and only when `require_gpu` is true:
///
/// * With the variable unset this is a no-op. A CPU adapter is a
///   perfectly good dev box, and that path is unchanged.
/// * Every other `DeviceType` (including `Other`, which some backends
///   report for genuine hardware) is accepted, so a future `wgpu` variant
///   fails open rather than failing a runner for a name this code has
///   never heard of. That is the same "unknown means don't invent a
///   verdict" direction [`gpu_error_action`]'s wildcard arm takes.
#[must_use]
pub const fn adapter_is_software_when_gpu_required(
    device_type: wgpu::DeviceType,
    require_gpu: bool,
) -> bool {
    require_gpu && matches!(device_type, wgpu::DeviceType::Cpu)
}

/// A real [`GpuContext`], or `None` when this machine has no usable GPU
/// adapter and nobody asked for one.
///
/// `NoSuitableAdapter` is an inconclusive skip on a dev box: whether a
/// given machine or CI runner image has a usable GPU (or software
/// adapter) is genuinely unverified, not assumed. But a runner that is
/// *supposed* to have a real adapter must not go green while every
/// GPU-gated test silently prints `SKIPPED` — so setting
/// [`REQUIRE_GPU_ENV_VAR`] turns that same skip into a hard failure, and
/// additionally rejects an adapter that turned out to be a software
/// rasterizer (see [`adapter_is_software_when_gpu_required`]). Unset is
/// the default, and no workflow in `.github/workflows/` sets it yet —
/// wiring it to a specific runner is a separate, deliberate decision.
/// Unset behaves byte-for-byte as before, `SKIPPED` line included; the
/// device-type check is a no-op in that case.
///
/// Any *other* error means an adapter was found but device/queue
/// creation itself failed, which is a real bug either way and fails the
/// test regardless of the environment.
///
/// The selected adapter's name, backend and device type are printed on
/// every successful creation, so a CI log always carries proof of what
/// was actually tested rather than only that *something* was.
///
/// Callers keep their own cross-test lock and their own context wrapper
/// — this shares only the decision, not the locking.
///
/// # Panics
///
/// Panics if [`REQUIRE_GPU_ENV_VAR`] is truthy and either no adapter was
/// found or the one found is a CPU/software adapter, or if device
/// creation failed for any other reason (regardless of that variable).
#[must_use]
pub fn real_context_or_skip() -> Option<GpuContext> {
    // The two `panic!`s below are the *first* deliberate `panic!`s in
    // this workspace's `crates/` tree, and the only
    // `#[allow(clippy::panic)]` in it — a new precedent, flagged as one
    // rather than dressed up as an existing convention. The established
    // idiom elsewhere is `unreachable!()` (~2,400 uses), and it does not
    // fit here: `unreachable!()` asserts a condition cannot occur, while
    // both cases below genuinely can — a real machine with no adapter,
    // and a real device request that failed.
    match GpuContext::new() {
        Ok(context) => {
            let info = context.adapter_info();
            eprintln!(
                "GPU adapter: {} ({:?}, {:?})",
                info.name, info.backend, info.device_type
            );
            // `assert!` rather than a third `panic!`: it says the same
            // thing, and clippy's `manual_assert` rejects the `if`/`panic!`
            // spelling outright.
            assert!(
                !adapter_is_software_when_gpu_required(info.device_type, require_gpu_from_env()),
                "{REQUIRE_GPU_ENV_VAR} is set, but the only adapter available is a software rasterizer ({}, {:?}), not real GPU hardware",
                info.name,
                info.device_type
            );
            Some(context)
        }
        Err(err) => match gpu_error_action(&err, require_gpu_from_env()) {
            GpuTestAction::Skip => {
                eprintln!("SKIPPED: no GPU adapter available on this machine/CI runner");
                None
            }
            #[allow(clippy::panic)]
            GpuTestAction::FailNoAdapter => {
                panic!("{REQUIRE_GPU_ENV_VAR} is set, but no GPU adapter was found: {err}")
            }
            #[allow(clippy::panic)]
            GpuTestAction::FailDeviceCreation => {
                panic!("device request failed with a real adapter present: {err}")
            }
        },
    }
}

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
#[cfg(test)]
static GPU_TEST_LOCK: Mutex<()> = Mutex::new(());

/// Bundles the lock guard with the context so holding one holds the
/// other — `Deref` means every existing call site (`context.device()`,
/// `context.queue()`, ...) keeps working unchanged.
#[cfg(test)]
pub(crate) struct GpuTestContext {
    _guard: MutexGuard<'static, ()>,
    context: GpuContext,
}

#[cfg(test)]
impl std::ops::Deref for GpuTestContext {
    type Target = GpuContext;
    fn deref(&self) -> &GpuContext {
        &self.context
    }
}

/// A real, tempdir-backed `TileStore` for tests — the tempdir must
/// outlive the store (it's the scratch directory), so both are returned
/// together; the caller keeps `_dir` alive by binding it, even unused.
#[cfg(test)]
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

/// This crate's own real-GPU entry point: take the cross-test lock,
/// then defer the skip-vs-fail decision to [`real_context_or_skip`] —
/// the one shared copy every crate in this workspace now uses.
#[cfg(test)]
pub(crate) fn real_context() -> Option<GpuTestContext> {
    let guard = GPU_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    real_context_or_skip().map(|context| GpuTestContext {
        _guard: guard,
        context,
    })
}

/// The canvas shader's bind-group layout (texture + sampler + uniform),
/// matching `spike/vertical-slice`'s `canvas_layout` exactly — shared by
/// every test that needs to build a real canvas pipeline.
#[cfg(test)]
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
#[cfg(test)]
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

/// Covers the env-var branching and the adapter-acceptance rule
/// themselves, with no GPU adapter involved — deliberately, since the
/// point of the mechanism is what happens on a machine that has no
/// adapter, or only a software one.
///
/// Every decision function here takes its inputs as parameters for
/// exactly this reason: `std::env::set_var` is `unsafe` in edition 2024
/// and this workspace denies `unsafe_code`, so a test can only reach the
/// "required" branch by passing `true`, never by setting the variable.
///
/// The end-to-end behaviour these predicates drive *is* separately
/// observable here, just not from a test: PLAN.md's entry for this work
/// records the two shell recipes (llvmpipe for the software-adapter
/// rejection, `VK_ICD_FILENAMES=/nonexistent/none.json` for the
/// no-adapter case) and what each produces.
#[cfg(test)]
mod tests {
    use super::{
        GpuTestAction, REQUIRE_GPU_ENV_VAR, adapter_is_software_when_gpu_required,
        gpu_error_action, require_gpu_from_value,
    };
    use crate::GpuError;

    #[test]
    fn no_adapter_skips_when_gpu_is_not_required() {
        assert_eq!(
            gpu_error_action(&GpuError::NoSuitableAdapter, false),
            GpuTestAction::Skip
        );
    }

    #[test]
    fn no_adapter_fails_when_gpu_is_required() {
        assert_eq!(
            gpu_error_action(&GpuError::NoSuitableAdapter, true),
            GpuTestAction::FailNoAdapter
        );
    }

    /// `SurfaceUnsupported` and `InvalidMipLevel` are **proxies**, not
    /// the reachable case. `GpuContext::new` can only ever return
    /// `NoSuitableAdapter` or `DeviceRequestFailed`
    /// (`context.rs:44,64`), and the latter wraps a
    /// `wgpu::RequestDeviceError`, which `wgpu` does not expose any
    /// constructor for — a test cannot build one without a real adapter
    /// refusing a real device request. What is actually under test here
    /// is [`gpu_error_action`]'s wildcard arm, which is variant-blind:
    /// every `GpuError` that is not `NoSuitableAdapter` takes it, so any
    /// non-adapter variant exercises the same code path
    /// `DeviceRequestFailed` would.
    #[test]
    fn a_non_adapter_error_fails_whether_or_not_gpu_is_required() {
        for require_gpu in [false, true] {
            assert_eq!(
                gpu_error_action(&GpuError::SurfaceUnsupported, require_gpu),
                GpuTestAction::FailDeviceCreation,
                "an adapter was found and setup still failed: a real bug either way"
            );
            assert_eq!(
                gpu_error_action(&GpuError::InvalidMipLevel(7), require_gpu),
                GpuTestAction::FailDeviceCreation
            );
        }
    }

    /// The variable's name is part of the contract with CI, so pin it
    /// rather than leaving a rename silently unobserved.
    #[test]
    fn the_env_var_name_is_the_one_ci_would_set() {
        assert_eq!(REQUIRE_GPU_ENV_VAR, "AURORA_REQUIRE_GPU");
    }

    /// Pinned deliberately: presence-based parsing (the obvious
    /// one-liner, and what a refactor would drift back to) makes
    /// `AURORA_REQUIRE_GPU=0` mean *on*, and makes GitHub Actions'
    /// empty-string-for-an-empty-expression turn it on across a whole
    /// matrix. Both directions are asserted so neither can be lost
    /// silently.
    #[test]
    fn falsy_values_mean_off_even_though_the_variable_is_present() {
        for value in ["", "0", "false", "off", "no", "FALSE", "Off", "  0  "] {
            assert!(
                !require_gpu_from_value(Some(value)),
                "{value:?} must read as off"
            );
        }
    }

    #[test]
    fn any_other_present_value_means_on_and_unset_means_off() {
        for value in ["1", "true", "yes", "TRUE", "on", "please", "00"] {
            assert!(
                require_gpu_from_value(Some(value)),
                "{value:?} must read as on"
            );
        }
        assert!(
            !require_gpu_from_value(None),
            "unset is the default and must stay off"
        );
    }

    /// A CPU adapter is only a failure when someone asked for real
    /// hardware — the llvmpipe dev box this project is normally
    /// developed on must keep working exactly as before.
    #[test]
    fn a_software_adapter_is_rejected_only_when_a_gpu_is_required() {
        assert!(adapter_is_software_when_gpu_required(
            wgpu::DeviceType::Cpu,
            true
        ));
        assert!(!adapter_is_software_when_gpu_required(
            wgpu::DeviceType::Cpu,
            false
        ));
    }

    /// Every non-`Cpu` device type is accepted in both modes, `Other`
    /// included: some backends report genuine hardware that way, so
    /// rejecting it would fail a real GPU runner.
    #[test]
    fn a_real_adapter_is_accepted_whether_or_not_a_gpu_is_required() {
        for device_type in [
            wgpu::DeviceType::DiscreteGpu,
            wgpu::DeviceType::IntegratedGpu,
            wgpu::DeviceType::VirtualGpu,
            wgpu::DeviceType::Other,
        ] {
            for require_gpu in [false, true] {
                assert!(
                    !adapter_is_software_when_gpu_required(device_type, require_gpu),
                    "{device_type:?} must be accepted (require_gpu = {require_gpu})"
                );
            }
        }
    }
}
