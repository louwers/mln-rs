// Ports include/mln/webgpu/layer_group.hpp and src/mln/webgpu/layer_group.cpp

use std::any::Any;

use mln_gfx::depth_mode::DepthMode;
use mln_gfx::paint_parameters::PaintParameters;
use mln_gfx::renderer::layer_group::{self, LayerGroupBaseData};
use mln_gfx::stencil_mode::StencilMode;
use mln_gfx::uniform_buffer::UniformBufferArray;

use crate::drawable::Drawable;
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
        self.inner.visit_drawables(|drawable| {
            if !drawable.get_enabled() || !drawable.has_render_pass(pass) {
                return;
            }
            // In WebGPU, binding happens per-drawable through bind groups
            // (bindWebgpu is a no-op).

            // Copy uniform buffers from layer group to drawable for WebGPU's bind groups
            let drawable_uniforms = drawable.mutable_uniform_buffers();
            for i in 0..uniform_buffers.allocated_size() {
                if let Some(uniform_buffer) = uniform_buffers.get(i) {
                    drawable_uniforms.set(i, Some(uniform_buffer.clone()));
                }
            }

            let tweakers = drawable.get_tweakers().clone();
            for tweaker in tweakers {
                tweaker.execute(drawable, parameters);
            }

            if drawable.get_is_3d() {
                let depth = if drawable.get_enable_depth() {
                    parameters.depth_mode_for_3d(drawable.get_depth_type())
                } else {
                    DepthMode::disabled()
                };
                if let Some(webgpu_drawable) = drawable.as_any_mut().downcast_mut::<Drawable>() {
                    webgpu_drawable.set_depth_mode_for_3d(&depth);
                    webgpu_drawable.set_stencil_mode_for_3d(&StencilMode::disabled());
                }
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
