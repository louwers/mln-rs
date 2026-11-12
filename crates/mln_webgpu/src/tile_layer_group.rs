// Ports include/mln/webgpu/tile_layer_group.hpp and src/mln/webgpu/tile_layer_group.cpp

use std::any::Any;

use mln_gfx::depth_mode::DepthMode;
use mln_gfx::drawable::Drawable as _;
use mln_gfx::paint_parameters::PaintParameters;
use mln_gfx::renderer::layer_group::{self, LayerGroupBaseData};
use mln_gfx::stencil_mode::StencilMode;
use mln_gfx::types::DepthMaskType;
use mln_gfx::uniform_buffer::UniformBufferArray;

use crate::drawable::Drawable;
use crate::upload_pass::UploadPass;

pub struct TileLayerGroup {
    inner: layer_group::TileLayerGroup,
    uniform_buffers: UniformBufferArray,
}

impl TileLayerGroup {
    #[must_use]
    pub fn new(layer_index: i32, initial_capacity: usize, name: String) -> Self {
        TileLayerGroup {
            inner: layer_group::TileLayerGroup::new(layer_index, initial_capacity, name),
            uniform_buffers: UniformBufferArray::new(),
        }
    }
}

impl layer_group::LayerGroupBase for TileLayerGroup {
    fn base(&self) -> &LayerGroupBaseData {
        &self.inner.base
    }
    fn base_mut(&mut self) -> &mut LayerGroupBaseData {
        &mut self.inner.base
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn as_tile_layer_group(&self) -> Option<&layer_group::TileLayerGroup> {
        Some(&self.inner)
    }
    fn as_tile_layer_group_mut(&mut self) -> Option<&mut layer_group::TileLayerGroup> {
        Some(&mut self.inner)
    }

    fn get_drawable_count(&self) -> usize {
        self.inner.get_drawable_count()
    }

    fn clear_drawables(&mut self) -> usize {
        self.inner.clear_drawables()
    }

    fn upload(&mut self, upload_pass: &dyn mln_gfx::upload_pass::UploadPass) {
        if !self.inner.base.enabled || self.inner.get_drawable_count() == 0 {
            return;
        }
        let Some(upload_pass) = upload_pass.as_any().downcast_ref::<UploadPass>() else {
            return;
        };
        self.inner.visit_drawables(|drawable| {
            if drawable.get_enabled()
                && let Some(drawable) = drawable.as_any_mut().downcast_mut::<Drawable>()
            {
                drawable.upload(upload_pass);
            }
        });
    }

    fn render(&mut self, parameters: &mut dyn PaintParameters) {
        if !self.inner.base.enabled || self.inner.get_drawable_count() == 0 || parameters.render_pass().is_none() {
            return;
        }

        // `stencilModeFor3D` increments a shared mask value each call, so we
        // cache the results when a layer contains 3D drawables to ensure
        // consistent state across drawables.
        let mut features_3d = false;
        let mut stencil_3d = false;
        let mut depth_mode_3d: Option<DepthMode> = None;
        let mut stencil_mode_3d: Option<StencilMode> = None;

        // 3D features take the group-wide depth state, and a stencil value
        // shared across the group when it has stencil tiles or their own when
        // it does not
        let stencil_tiles = self.inner.stencil_tiles.clone().filter(|t| !t.is_empty());
        let has_stencil_tiles = stencil_tiles.is_some();
        // `ownStencilModes`: C++ keys them by drawable; here each drawable
        // that takes one has a slot, by its place in the visiting order.
        let mut own_stencil_slots: Vec<Option<usize>> = Vec::new();
        let mut own_stencil_modes: Vec<StencilMode> = Vec::new();
        let pass = parameters.pass();
        self.inner.visit_drawables_ref(|drawable| {
            let mut slot = None;
            if drawable.get_enabled() && drawable.get_is_3d() && drawable.has_render_pass(pass) {
                features_3d = true;
                if drawable.get_enable_stencil() {
                    stencil_3d = true;
                    if !has_stencil_tiles {
                        slot = Some(own_stencil_modes.len());
                        own_stencil_modes.push(StencilMode::default());
                    }
                }
            }
            own_stencil_slots.push(slot);
        });
        let own_stencil_modes_taken = parameters.stencil_modes_for_3d(&mut own_stencil_modes);

        if features_3d {
            depth_mode_3d = Some(parameters.depth_mode_for_3d(DepthMaskType::ReadWrite));
            if stencil_3d && has_stencil_tiles {
                stencil_mode_3d = Some(parameters.stencil_mode_for_3d());
            }
        } else if let Some(tiles) = &stencil_tiles {
            // Handle stencil tiles if present (2D stenciling)
            parameters.render_tile_clipping_masks(tiles);
        }

        // Rely on drawables to provide their shaders; no layer-level override needed.
        let mut visit_count = 0;
        let mut draw_count = 0;
        let initial_draw_calls = parameters.context().rendering_stats().num_draw_calls;
        let name = self.inner.base.name.clone();
        let uniform_buffers = &self.uniform_buffers;

        self.inner.visit_drawables(|drawable| {
            let own_slot = own_stencil_slots.get(visit_count).copied().flatten();
            visit_count += 1;
            if !drawable.get_enabled() || !drawable.has_render_pass(pass) {
                return;
            }
            draw_count += 1;

            if drawable.get_shader().is_none() {
                eprintln!("Drawable {} in {name} missing shader; skipping", drawable.get_name());
                return;
            }

            // Bind uniform buffers once for all drawables (a no-op in WebGPU).

            // Copy any layer-provided uniform buffers so drawables can build
            // bind groups from them. This includes consolidated per-drawable
            // data which lives in the layer UBO array and is selected via the
            // UBO index each drawable writes later.
            let drawable_uniforms = drawable.mutable_uniform_buffers();
            for i in 0..uniform_buffers.allocated_size() {
                if let Some(uniform_buffer) = uniform_buffers.get(i) {
                    drawable_uniforms.set(i, Some(uniform_buffer.clone()));
                }
            }

            // Execute tweakers - they create drawable-specific uniform buffers
            let tweakers = drawable.get_tweakers().clone();
            for tweaker in tweakers {
                tweaker.execute(drawable, parameters);
            }

            if features_3d && let Some(webgpu_drawable) = drawable.as_any_mut().downcast_mut::<Drawable>() {
                let depth = match (webgpu_drawable.get_enable_depth(), depth_mode_3d) {
                    (true, Some(d)) => d,
                    _ => DepthMode::disabled(),
                };
                if let Some(slot) = own_slot
                    && !own_stencil_modes_taken
                {
                    // A clear between two draws draws over the zoom 0 tile
                    // with buffers of its own. (C++ then binds the group's
                    // uniform buffers again; here each drawable carries its
                    // copies, so there is nothing to bind.)
                    own_stencil_modes[slot] = parameters.stencil_mode_for_3d();
                }
                let stencil = if !webgpu_drawable.get_enable_stencil() {
                    StencilMode::disabled()
                } else if let Some(slot) = own_slot {
                    own_stencil_modes[slot]
                } else if let Some(s) = stencil_mode_3d {
                    s
                } else {
                    StencilMode::disabled()
                };
                webgpu_drawable.set_depth_mode_for_3d(&depth);
                webgpu_drawable.set_stencil_mode_for_3d(&stencil);
            }

            drawable.draw(parameters);
        });

        let final_draw_calls = parameters.context().rendering_stats().num_draw_calls;
        if draw_count > 0 && final_draw_calls == initial_draw_calls {
            eprintln!("{name} visited {visit_count} drawables but produced no draw calls!");
        }
    }

    fn get_uniform_buffers(&self) -> &UniformBufferArray {
        &self.uniform_buffers
    }

    fn mutable_uniform_buffers(&mut self) -> &mut UniformBufferArray {
        &mut self.uniform_buffers
    }
}
