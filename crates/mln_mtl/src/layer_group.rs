// Ports include/mln/mtl/layer_group.hpp and src/mln/mtl/layer_group.cpp

use std::any::Any;

use mln_gfx::paint_parameters::PaintParameters;
use mln_gfx::renderer::layer_group::{self, LayerGroupBaseData};
use mln_gfx::uniform_buffer::UniformBufferArray;

use crate::drawable::Drawable;
use crate::render_pass::RenderPass;
use crate::uniform_buffer::bind_uniform_buffer_array;
use crate::upload_pass::UploadPass;

pub struct LayerGroup {
    inner: layer_group::LayerGroup,
    uniform_buffers: UniformBufferArray,
}

impl LayerGroup {
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
