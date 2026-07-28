//! The shader library: named, eagerly-compiled WGSL modules.

use std::collections::HashMap;

const CANVAS_SHADER: &str = include_str!("shaders/canvas.wgsl");

/// A named collection of compiled `wgpu::ShaderModule`s.
///
/// Compilation is eager and infallible: every source here is fixed,
/// compile-time-known WGSL (`include_str!`), not user- or
/// runtime-supplied. A broken embedded shader would be caught by this
/// crate's own tests, not something to design runtime error handling
/// around. (wgpu's `create_shader_module` doesn't return a `Result`
/// either way — validation errors surface through its own error-scope
/// mechanism, which is real, separate follow-on work for if/when
/// `aurora-gpu` needs to compile *dynamic* shader source, e.g. a future
/// user-facing custom filter.)
pub struct ShaderLibrary {
    modules: HashMap<&'static str, wgpu::ShaderModule>,
}

impl ShaderLibrary {
    #[must_use]
    pub fn new(device: &wgpu::Device) -> Self {
        let mut modules = HashMap::new();
        modules.insert(
            "canvas",
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("canvas"),
                source: wgpu::ShaderSource::Wgsl(CANVAS_SHADER.into()),
            }),
        );
        Self { modules }
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&wgpu::ShaderModule> {
        self.modules.get(name)
    }
}

impl std::fmt::Debug for ShaderLibrary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShaderLibrary")
            .field("modules", &self.modules.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::ShaderLibrary;
    use crate::test_support::real_context;

    #[test]
    fn compiles_the_canvas_shader() {
        let Some(context) = real_context() else {
            return;
        };
        let library = ShaderLibrary::new(context.device());
        assert!(library.get("canvas").is_some());
        assert!(library.get("nonexistent").is_none());
    }
}
