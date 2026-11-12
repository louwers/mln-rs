// Ports include/mln/mtl/layer_group.hpp and src/mln/mtl/layer_group.cpp

use std::any::Any;

use mln_gfx::depth_mode::DepthMode;
use mln_gfx::paint_parameters::PaintParameters;
use mln_gfx::renderer::layer_group::{self, LayerGroupBaseData};
use mln_gfx::stencil_mode::StencilMode;
use mln_gfx::types::DepthMaskType;
use mln_gfx::uniform_buffer::UniformBufferArray;

use crate::context::Context;
use crate::drawable::Drawable;
use crate::mtl_fwd::MTLDepthStencilStatePtr;
use crate::render_pass::RenderPass;
use crate::uniform_buffer::bind_uniform_buffer_array;
use crate::upload_pass::UploadPass;

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
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn as_layer_group(&self) -> Option<&layer_group::LayerGroup> {
        Some(&self.inner)
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
        let uniform_buffers = &self.uniform_buffers;
        let pass = parameters.pass();
        let mut bind_ubos = false;
        // The depth-stencil states of 3D drawables, built when first used.
        let mut states_3d: [Option<Option<MTLDepthStencilStatePtr>>; 3] = Default::default();
        self.inner.visit_drawables(|drawable| {
            if !drawable.get_enabled() || !drawable.has_render_pass(pass) {
                return;
            }
            if !bind_ubos {
                if let Some(render_pass) =
                    parameters.render_pass().and_then(|p| p.as_any().downcast_ref::<RenderPass>())
                {
                    bind_uniform_buffer_array(uniform_buffers, render_pass);
                }
                bind_ubos = true;
            }
            let tweakers = drawable.get_tweakers().clone();
            for tweaker in tweakers {
                tweaker.execute(drawable, parameters);
            }
            if drawable.get_is_3d()
                && let Some(render_pass) =
                    parameters.render_pass().and_then(|p| p.as_any().downcast_ref::<RenderPass>())
                && let Some(context) = parameters.context().as_any().downcast_ref::<Context>()
            {
                // By depth: none, read-only or read-write.
                let slot = if !drawable.get_enable_depth() {
                    0
                } else if drawable.get_depth_type() == DepthMaskType::ReadOnly {
                    1
                } else {
                    2
                };
                let state = states_3d[slot].get_or_insert_with(|| {
                    let depth_mode = match slot {
                        0 => DepthMode::disabled(),
                        1 => parameters.depth_mode_for_3d(DepthMaskType::ReadOnly),
                        _ => parameters.depth_mode_for_3d(DepthMaskType::ReadWrite),
                    };
                    context.make_depth_stencil_state(&depth_mode, &StencilMode::disabled(), render_pass)
                });
                render_pass.set_depth_stencil_state(state.as_ref());
            }
            drawable.draw(parameters);
        });
    }

    fn get_uniform_buffers(&self) -> &UniformBufferArray {
        &self.uniform_buffers
    }

    fn mutable_uniform_buffers(&mut self) -> &mut UniformBufferArray {
        &mut self.uniform_buffers
    }
}
