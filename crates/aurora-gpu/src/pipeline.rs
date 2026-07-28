//! The WGSL pipeline cache: memoizes `wgpu::RenderPipeline`s by a small,
//! explicitly-hashable key, so the same logical pipeline shape is never
//! rebuilt twice.

use std::collections::HashMap;

/// What varies about a render pipeline built from [`crate::ShaderLibrary`]
/// content — enough fields to distinguish every pipeline shape this
/// crate's consumers are known to need so far (nothing speculative: no
/// vertex-buffer-layout or multi-bind-group-layout fields, since nothing
/// downstream needs those yet).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PipelineKey {
    pub shader: &'static str,
    pub vertex_entry: &'static str,
    pub fragment_entry: &'static str,
    pub target_format: wgpu::TextureFormat,
    pub blend: Blend,
}

/// Our own enum, not `wgpu::BlendState` directly — keeps [`PipelineKey`]'s
/// `Hash`/`Eq` bound to a type this crate controls rather than depending
/// on `wgpu::BlendState` deriving them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Blend {
    None,
    AlphaBlending,
}

impl Blend {
    #[must_use]
    pub const fn to_wgpu(self) -> Option<wgpu::BlendState> {
        match self {
            Self::None => None,
            Self::AlphaBlending => Some(wgpu::BlendState::ALPHA_BLENDING),
        }
    }
}

/// Caches render pipelines by [`PipelineKey`].
///
/// Deliberately doesn't know about shaders, bind group layouts, or
/// devices at all — the caller builds the pipeline however it needs to;
/// this only memoizes by key. Simpler than a descriptor-taking API, and
/// doesn't invent structure (compute pipelines, multiple bind group
/// layouts, vertex buffers) nothing downstream needs yet.
#[derive(Default)]
pub struct PipelineCache {
    pipelines: HashMap<PipelineKey, wgpu::RenderPipeline>,
}

impl PipelineCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the cached pipeline for `key`; on a miss, calls `build`
    /// (only then) and caches the result.
    pub fn get_or_create_with(
        &mut self,
        key: PipelineKey,
        build: impl FnOnce() -> wgpu::RenderPipeline,
    ) -> &wgpu::RenderPipeline {
        self.pipelines.entry(key).or_insert_with(build)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.pipelines.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pipelines.is_empty()
    }
}

impl std::fmt::Debug for PipelineCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PipelineCache")
            .field("len", &self.pipelines.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::{Blend, PipelineCache, PipelineKey};
    use crate::ShaderLibrary;
    use crate::test_support::{build_canvas_pipeline, canvas_bind_group_layout, real_context};
    use std::cell::Cell;

    fn key() -> PipelineKey {
        PipelineKey {
            shader: "canvas",
            vertex_entry: "vs_canvas",
            fragment_entry: "fs_canvas",
            target_format: wgpu::TextureFormat::Rgba8Unorm,
            blend: Blend::None,
        }
    }

    #[test]
    fn caches_by_key_and_rebuilds_on_a_different_key() {
        let Some(context) = real_context() else {
            return;
        };
        let library = ShaderLibrary::new(context.device());
        let Some(shader) = library.get("canvas") else {
            unreachable!("ShaderLibrary::new always compiles the canvas shader");
        };
        let layout = canvas_bind_group_layout(context.device());

        let mut cache = PipelineCache::new();
        let builds = Cell::new(0u32);

        let k = key();
        cache.get_or_create_with(k.clone(), || {
            builds.set(builds.get() + 1);
            build_canvas_pipeline(context.device(), &layout, shader, &k)
        });
        assert_eq!(builds.get(), 1);
        assert_eq!(cache.len(), 1);

        // Same key again: must be a cache hit, `build` must not run.
        let k2 = key();
        cache.get_or_create_with(k2.clone(), || {
            builds.set(builds.get() + 1);
            build_canvas_pipeline(context.device(), &layout, shader, &k2)
        });
        assert_eq!(
            builds.get(),
            1,
            "identical key must hit the cache, not rebuild"
        );
        assert_eq!(cache.len(), 1);

        // Different key (different target format): must be a real miss.
        let k3 = PipelineKey {
            target_format: wgpu::TextureFormat::Bgra8Unorm,
            ..key()
        };
        cache.get_or_create_with(k3.clone(), || {
            builds.set(builds.get() + 1);
            build_canvas_pipeline(context.device(), &layout, shader, &k3)
        });
        assert_eq!(builds.get(), 2, "a different key must rebuild");
        assert_eq!(cache.len(), 2);
    }
}
