// Differences from C++:
// - A group hands its uniform buffers to the context, which binds the ones
//   each drawable's shader reads (`Context::bind_uniform_blocks`).
// - A framebuffer without a stencil buffer needs no `stencilClippingAvailable`:
//   every stencil test passes there, and the context draws no clip masks
//   into it.
// - A 3D drawable outside a tile layer group is drawn without a stencil
//   test, as on the other backends (C++ leaves the last drawable's).

use mln_gfx::paint_parameters::PaintParameters;
use mln_gfx::renderer::layer_group::{self, LayerGroupBaseData};
use mln_gfx::stencil_mode::StencilMode;
use mln_gfx::uniform_buffer::UniformBufferArray;

use crate::context::Context;
use crate::drawable::Drawable;
use crate::upload_pass::UploadPass;

/// The GL context `parameters` draws with.
fn gl_context(parameters: &dyn PaintParameters) -> Option<&Context> {
    parameters.context().downcast_ref::<Context>()
}

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
        let Some(upload_pass) = upload_pass.downcast_ref::<UploadPass>() else {
            return;
        };
        self.inner.visit_drawables(|drawable| {
            if drawable.get_enabled()
                && let Some(drawable) = drawable.downcast_mut::<Drawable>()
            {
                drawable.upload(upload_pass);
            }
        });
    }

    fn render(&mut self, parameters: &mut dyn PaintParameters) {
        if !self.inner.base.enabled || self.inner.get_drawable_count() == 0 || parameters.render_pass().is_none() {
            return;
        }

        // `stencilModeFor3D` uses a different stencil mask value each time
        // it's called, so if the drawables in this layer use 3D stencil mode
        // with stencil tiles, we need to set it up here so that all the
        // drawables end up using the same mode value; without stencil tiles
        // each drawable gets its own. 2D and 3D features in the same layer
        // group are not supported.
        let mut features_3d = false;
        let mut stencil_3d = false;
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

        // If we're doing 3D stenciling and have any features to draw, set
        // up the single-value stencil mask. If we're doing 2D stenciling and
        // have any drawables with tile IDs, render each tile into the
        // stencil buffer with a different value.
        let mut stencil_mode_3d = StencilMode::disabled();
        if features_3d {
            if stencil_3d && has_stencil_tiles {
                stencil_mode_3d = parameters.stencil_mode_for_3d();
            }
        } else if let Some(tiles) = &stencil_tiles {
            parameters.render_tile_clipping_masks(tiles);
        }

        if let Some(context) = gl_context(parameters) {
            context.set_layer_uniform_buffers(&self.uniform_buffers);
        }

        let mut visited = 0;
        self.inner.visit_drawables(|drawable| {
            let own_slot = own_stencil_slots.get(visited).copied().flatten();
            visited += 1;
            if !drawable.get_enabled() || !drawable.has_render_pass(pass) {
                return;
            }
            let tweakers = drawable.get_tweakers().clone();
            for tweaker in tweakers {
                tweaker.execute(drawable, parameters);
            }

            // For layer groups with 3D features, enable either the
            // single-value stencil mode for features with stencil enabled or
            // disable stenciling. 2D drawables set their own stencil mode
            // within `draw`.
            if features_3d {
                if let Some(slot) = own_slot
                    && !own_stencil_modes_taken
                {
                    own_stencil_modes[slot] = parameters.stencil_mode_for_3d();
                }
                let stencil_mode = if !drawable.get_enable_stencil() {
                    StencilMode::disabled()
                } else if let Some(slot) = own_slot {
                    own_stencil_modes[slot]
                } else {
                    stencil_mode_3d
                };
                if let Some(context) = gl_context(parameters) {
                    context.set_stencil_mode(&stencil_mode);
                }
            }

            drawable.draw(parameters);
        });
    }

    fn mutable_uniform_buffers(&mut self) -> &mut UniformBufferArray {
        &mut self.uniform_buffers
    }
}

pub struct LayerGroup {
    inner: layer_group::LayerGroup,
    uniform_buffers: UniformBufferArray,
}

impl LayerGroup {
    #[must_use]
    pub fn new(layer_index: i32, initial_capacity: usize, name: String) -> Self {
        LayerGroup {
            inner: layer_group::LayerGroup::new(layer_index, initial_capacity, name),
            uniform_buffers: UniformBufferArray::new(),
        }
    }
}

impl layer_group::LayerGroupBase for LayerGroup {
    fn base(&self) -> &LayerGroupBaseData {
        &self.inner.base
    }
    fn base_mut(&mut self) -> &mut LayerGroupBaseData {
        &mut self.inner.base
    }
    fn as_layer_group_mut(&mut self) -> Option<&mut layer_group::LayerGroup> {
        Some(&mut self.inner)
    }

    fn get_drawable_count(&self) -> usize {
        self.inner.get_drawable_count()
    }

    fn clear_drawables(&mut self) -> usize {
        self.inner.clear_drawables()
    }

    fn upload(&mut self, upload_pass: &dyn mln_gfx::upload_pass::UploadPass) {
        if !self.inner.base.enabled {
            return;
        }
        let Some(upload_pass) = upload_pass.downcast_ref::<UploadPass>() else {
            return;
        };
        self.inner.visit_drawables(|drawable| {
            if drawable.get_enabled()
                && let Some(drawable) = drawable.downcast_mut::<Drawable>()
            {
                drawable.upload(upload_pass);
            }
        });
    }

    fn render(&mut self, parameters: &mut dyn PaintParameters) {
        if !self.inner.base.enabled || self.inner.get_drawable_count() == 0 || parameters.render_pass().is_none() {
            return;
        }
        if let Some(context) = gl_context(parameters) {
            context.set_layer_uniform_buffers(&self.uniform_buffers);
        }
        let pass = parameters.pass();
        self.inner.visit_drawables(|drawable| {
            if !drawable.get_enabled() || !drawable.has_render_pass(pass) {
                return;
            }
            let tweakers = drawable.get_tweakers().clone();
            for tweaker in tweakers {
                tweaker.execute(drawable, parameters);
            }
            if drawable.get_is_3d()
                && let Some(context) = gl_context(parameters)
            {
                context.set_stencil_mode(&StencilMode::disabled());
            }
            drawable.draw(parameters);
        });
    }

    fn mutable_uniform_buffers(&mut self) -> &mut UniformBufferArray {
        &mut self.uniform_buffers
    }
}
