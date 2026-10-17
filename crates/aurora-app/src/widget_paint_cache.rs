//! The widget layer's paint cache (60 FPS round 3): `App::redraw` used to
//! tessellate every widget and create two GPU buffers per mesh on every
//! frame (1.8 ms idle, up to 3.8 ms loaded, measured in
//! `frame_cost_tests`), although on most frames nothing in the UI changed.
//!
//! **What is cached.** Per widget, the *mesh* half of its paint
//! ([`aurora_widgets::paint_widget_mesh_ops`]: gradients then solids, the
//! solids' colours converted for the target as [`crate::target_paint_ops`]
//! converts them) and, once uploaded, the GPU meshes made from exactly
//! those ops. Text runs and the focus ring
//! ([`aurora_widgets::paint_widget_overlay_ops`]) are resolved fresh every
//! frame, as before: a glyph mesh refers to atlas slots a later frame may
//! reuse.
//!
//! **Why reuse is sound.** The mesh half is a function of the widget's
//! [`aurora_widgets::WidgetMeshKey`] (its payload, full bounds, measured
//! flag and paint clip: everything those painters read from the tree)
//! plus the theme, the scales, the scale factor and the target format. An
//! entry is reused only while its key still matches the tree
//! ([`aurora_widgets::WidgetMeshKey::matches`]) and the four globals are
//! equal to the ones it was painted with (any change clears the cache).
//! A widget whose paint fails is skipped for the frame and never cached,
//! and an entry for a widget that left the paint order is dropped.

use std::collections::HashMap;

use aurora_theme::{Scales, Theme};
use aurora_widgets::widgets::WidgetKind;
use aurora_widgets::{
    FocusPaint, FrameOp, GpuColorMesh, GpuMesh, GpuPaintOp, PaintOp, WidgetId, WidgetMeshKey,
    WidgetTree,
};

/// The inputs every cached mesh depends on besides its own key.
#[derive(Debug, Clone, PartialEq)]
struct Globals {
    theme: Theme,
    scales: Scales,
    scale_factor: u64,
    format: wgpu::TextureFormat,
}

struct Entry {
    key: WidgetMeshKey,
    /// Target-converted mesh ops, exactly what `target_paint_ops` gives.
    ops: Vec<PaintOp>,
    /// The GPU meshes of `ops`, once a GPU frame has uploaded them.
    gpu: Option<Vec<GpuPaintOp>>,
    /// The frame this widget was last painted in.
    frame: u64,
}

/// Hit and miss counts of the last frame, for tests and measurement.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PaintCacheStats {
    pub(crate) hits: usize,
    pub(crate) misses: usize,
}

/// See the module documentation.
#[derive(Default)]
pub(crate) struct WidgetPaintCache {
    globals: Option<Globals>,
    entries: HashMap<WidgetId, Entry>,
    frame: u64,
    stats: PaintCacheStats,
}

/// One widget's ops for the frame: its (cached) mesh entry and its fresh
/// overlay ops.
struct WidgetOps {
    id: WidgetId,
    overlay: Vec<PaintOp>,
}

impl WidgetPaintCache {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn stats(&self) -> PaintCacheStats {
        self.stats
    }

    /// Starts a frame: clears everything if a global input changed.
    fn begin(
        &mut self,
        theme: &Theme,
        scales: &Scales,
        format: wgpu::TextureFormat,
        scale_factor: f64,
    ) {
        let globals = Globals {
            theme: *theme,
            scales: scales.clone(),
            scale_factor: scale_factor.to_bits(),
            format,
        };
        if self.globals.as_ref() != Some(&globals) {
            self.entries.clear();
            self.globals = Some(globals);
        }
        self.frame = self.frame.wrapping_add(1);
        self.stats = PaintCacheStats::default();
    }

    /// Walks the paint order, refreshing each widget's mesh entry where its
    /// key changed, and returns the widgets painted this frame with their
    /// overlay ops (colour-converted). Drops entries of widgets not painted.
    #[allow(clippy::too_many_arguments)]
    fn walk(
        &mut self,
        tree: &WidgetTree<WidgetKind>,
        focus: Option<FocusPaint>,
        focused: Option<WidgetId>,
        theme: &Theme,
        scales: &Scales,
        format: wgpu::TextureFormat,
        scale_factor: f64,
    ) -> Vec<WidgetOps> {
        self.begin(theme, scales, format, scale_factor);
        #[allow(clippy::cast_possible_truncation)]
        let sf = scale_factor as f32;
        let mut out = Vec::new();
        for id in tree.paint_order() {
            let overlay = match aurora_widgets::paint_widget_overlay_ops(
                tree, id, focus, focused, theme, scales, sf,
            ) {
                Ok(ops) => ops,
                Err(err) => {
                    tracing::warn!(
                        ?err,
                        ?id,
                        "failed to paint a widget; skipping it this frame"
                    );
                    self.entries.remove(&id);
                    continue;
                }
            };
            let current = self
                .entries
                .get(&id)
                .is_some_and(|entry| entry.key.matches(tree, id));
            if current {
                self.stats.hits += 1;
            } else {
                self.stats.misses += 1;
                let fresh = aurora_widgets::widget_mesh_key(tree, id).and_then(|key| {
                    match aurora_widgets::paint_widget_mesh_ops(tree, id, theme, scales, sf) {
                        Ok(ops) => Some((key, ops)),
                        Err(err) => {
                            tracing::warn!(
                                ?err,
                                ?id,
                                "failed to paint a widget; skipping it this frame"
                            );
                            None
                        }
                    }
                });
                let Some((key, ops)) = fresh else {
                    self.entries.remove(&id);
                    continue;
                };
                let ops = ops
                    .into_iter()
                    .map(|op| crate::target_paint_op(op, format))
                    .collect();
                self.entries.insert(
                    id,
                    Entry {
                        key,
                        ops,
                        gpu: None,
                        frame: self.frame,
                    },
                );
            }
            if let Some(entry) = self.entries.get_mut(&id) {
                entry.frame = self.frame;
            }
            out.push(WidgetOps {
                id,
                overlay: overlay
                    .into_iter()
                    .map(|op| crate::target_paint_op(op, format))
                    .collect(),
            });
        }
        let frame = self.frame;
        self.entries.retain(|_, entry| entry.frame == frame);
        out
    }

    /// The frame's CPU ops through the cache: equal, op for op, to
    /// [`crate::target_paint_ops`] (the property `widget_paint_cache_tests`
    /// pins), with every unchanged widget's meshes reused rather than
    /// re-tessellated.
    #[allow(clippy::too_many_arguments)]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn frame_paint_ops(
        &mut self,
        tree: &WidgetTree<WidgetKind>,
        focus: Option<FocusPaint>,
        focused: Option<WidgetId>,
        theme: &Theme,
        scales: &Scales,
        format: wgpu::TextureFormat,
        scale_factor: f64,
    ) -> Vec<PaintOp> {
        let widgets = self.walk(tree, focus, focused, theme, scales, format, scale_factor);
        let mut ops = Vec::new();
        for widget in widgets {
            if let Some(entry) = self.entries.get(&widget.id) {
                ops.extend(entry.ops.iter().cloned());
            }
            ops.extend(widget.overlay);
        }
        ops
    }

    /// [`crate::collect_widget_paints`] through the cache: every unchanged
    /// widget's meshes are the GPU buffers uploaded on an earlier frame.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn frame_gpu_ops(
        &mut self,
        tree: &WidgetTree<WidgetKind>,
        focus: Option<FocusPaint>,
        focused: Option<WidgetId>,
        theme: &Theme,
        scales: &Scales,
        gpu: &aurora_gpu::GpuContext,
        format: wgpu::TextureFormat,
        scale_factor: f64,
        text: Option<(
            &mut aurora_text::TextEngine,
            &mut aurora_widgets::GlyphAtlas,
        )>,
    ) -> Vec<GpuPaintOp> {
        let widgets = self.walk(tree, focus, focused, theme, scales, format, scale_factor);
        let mut frame_ops = Vec::new();
        for widget in widgets {
            if let Some(entry) = self.entries.get_mut(&widget.id) {
                let meshes = entry
                    .gpu
                    .get_or_insert_with(|| upload_meshes(gpu, &entry.ops));
                frame_ops.extend(meshes.iter().cloned().map(FrameOp::Ready));
            }
            frame_ops.extend(widget.overlay.into_iter().map(FrameOp::Fresh));
        }
        if let Some((engine, atlas)) = text {
            #[allow(clippy::cast_possible_truncation)]
            let scale = scale_factor as f32;
            // Colours were already converted for `format`.
            return aurora_widgets::upload_frame_ops(
                gpu.device(),
                gpu.queue(),
                engine,
                atlas,
                frame_ops,
                scale,
                |c| c,
            );
        }
        frame_ops
            .into_iter()
            .filter_map(|op| match op {
                FrameOp::Ready(op) => Some(op),
                FrameOp::Fresh(op) => upload_mesh_op(gpu, &op),
            })
            .collect()
    }
}

/// The GPU meshes of an entry's mesh ops (solids and gradients only).
fn upload_meshes(gpu: &aurora_gpu::GpuContext, ops: &[PaintOp]) -> Vec<GpuPaintOp> {
    ops.iter()
        .filter_map(|op| upload_mesh_op(gpu, op))
        .collect()
}

fn upload_mesh_op(gpu: &aurora_gpu::GpuContext, op: &PaintOp) -> Option<GpuPaintOp> {
    match op {
        PaintOp::Solid((mesh, color)) => Some(GpuPaintOp::Solid(
            GpuMesh::upload(gpu.device(), gpu.queue(), mesh),
            *color,
        )),
        PaintOp::Gradient(mesh) => Some(GpuPaintOp::Gradient(GpuColorMesh::upload(
            gpu.device(),
            gpu.queue(),
            mesh,
        ))),
        PaintOp::Text(_) => None,
    }
}
