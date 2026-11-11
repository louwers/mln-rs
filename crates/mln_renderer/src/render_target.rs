// Ports include/mln/renderer/render_target.hpp and
// src/mln/renderer/render_target.cpp.
//
// C++ `gfx::Context::createRenderTarget` returns `std::make_shared<RenderTarget>`;
// RenderTarget is a renderer type and mln_gfx can't name it, so callers use
// `RenderTarget::new(context, size, type)` directly. The tweakers of the
// contained layer groups are kept by the orchestrator (see
// change_request.rs), so `render` runs them through it.

use std::cell::RefCell;
use std::rc::Rc;

use mln_gfx::offscreen_texture::OffscreenTexture;
use mln_gfx::render_pass::RenderPassDescriptor;
use mln_gfx::renderable::Renderable;
use mln_gfx::scissor_rect::ScissorRect;
use mln_gfx::texture2d::Texture2DPtr;
use mln_gfx::types::TextureChannelDataType;
use mln_gfx::{Context, LayerGroupBasePtr, UploadPass};
use mln_util::Color;
use mln_util::size::Size;
use mln_vec_map::VecMap;

use crate::paint_parameters::PaintParameters;
use crate::render_orchestrator::RenderOrchestrator;
use crate::render_pass::RenderPass;

/// `RenderTargetPtr = std::shared_ptr<RenderTarget>`.
pub type RenderTargetPtr = Rc<RefCell<RenderTarget>>;

/// Render target class
pub struct RenderTarget {
    offscreen_texture: Box<dyn OffscreenTexture>,
    size: Size,
    layer_groups_by_layer_index: VecMap<i32, LayerGroupBasePtr>,
}

impl RenderTarget {
    /// `RenderTarget(context, size, type)`.
    pub fn new(context: &dyn Context, size: Size, type_: TextureChannelDataType) -> Self {
        RenderTarget {
            offscreen_texture: context.create_offscreen_texture(size, type_),
            size,
            layer_groups_by_layer_index: VecMap::new(),
        }
    }

    /// `context.createRenderTarget(size, type)` as a shared pointer.
    pub fn create(context: &dyn Context, size: Size, type_: TextureChannelDataType) -> RenderTargetPtr {
        Rc::new(RefCell::new(RenderTarget::new(context, size, type_)))
    }

    /// Get the render target texture
    pub fn get_texture(&self) -> Texture2DPtr {
        self.offscreen_texture.get_texture()
    }

    /// The size the render target was created with (the texture's size).
    pub fn get_size(&self) -> Size {
        self.size
    }

    /// Add a layer group to the render target; `replace`: replace if
    /// exists. Returns whether added.
    pub fn add_layer_group(&mut self, layer_group: LayerGroupBasePtr, replace: bool) -> bool {
        let index = layer_group.borrow().get_layer_index();
        match self.layer_groups_by_layer_index.entry(index) {
            mln_vec_map::Entry::Vacant(entry) => {
                // added
                entry.insert(layer_group);
                true
            }
            mln_vec_map::Entry::Occupied(mut entry) => {
                // not added
                if replace {
                    entry.insert(layer_group);
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Remove a layer group by layer index; returns whether removed.
    pub fn remove_layer_group(&mut self, layer_index: i32) -> bool {
        self.layer_groups_by_layer_index.remove(&layer_index).is_some()
    }

    /// Get the layer group count
    pub fn num_layer_groups(&self) -> usize {
        self.layer_groups_by_layer_index.len()
    }

    /// Get a specific layer group by index, if existent.
    pub fn get_layer_group(&self, layer_index: i32) -> Option<&LayerGroupBasePtr> {
        self.layer_groups_by_layer_index.get(&layer_index)
    }

    /// The contained layer groups, in layer index order.
    pub fn layer_groups(&self) -> Vec<LayerGroupBasePtr> {
        self.layer_groups_by_layer_index.values().cloned().collect()
    }

    /// Upload the layer groups
    pub fn upload(&self, upload_pass: &dyn UploadPass) {
        for layer_group in self.layer_groups_by_layer_index.values() {
            layer_group.borrow_mut().upload(upload_pass);
        }
    }

    /// Render the layer groups. `renderTree` is only passed on to
    /// `runTweakers` in C++; the orchestrator runs them here.
    pub fn render(&self, orchestrator: &mut RenderOrchestrator, parameters: &mut PaintParameters<'_>) {
        let renderable: &dyn Renderable = &*self.offscreen_texture;
        {
            let encoder = parameters.encoder.as_ref().expect("encoder");
            parameters.render_pass = Some(encoder.create_render_pass(
                "render target",
                &RenderPassDescriptor {
                    renderable,
                    clear_color: Some(Color::new(0.0, 0.0, 0.0, 1.0)),
                    clear_depth: Some(1.0),
                    clear_stencil: None,
                },
            ));
        }

        let prev_scissor_rect = parameters.scissor_rect;
        let size = self.get_texture().borrow().get_size();
        parameters.scissor_rect = ScissorRect { x: 0, y: 0, width: size.width, height: size.height };

        let layer_groups = self.layer_groups();
        let num_layer_groups = layer_groups.len();
        let num_sublayers = PaintParameters::NUM_SUBLAYERS as usize;
        let depth_epsilon = PaintParameters::DEPTH_EPSILON;

        // Run layer tweakers to update any dynamic elements
        parameters.current_layer = 0;
        for layer_group in &layer_groups {
            orchestrator.run_tweakers(layer_group, parameters);
            parameters.current_layer += 1;
        }

        // draw layer groups, opaque pass
        parameters.pass = RenderPass::Opaque;
        parameters.depth_range_size = 1.0 - ((num_layer_groups + 2) * num_sublayers) as f32 * depth_epsilon;

        parameters.current_layer = 0;
        for layer_group in layer_groups.iter().rev() {
            layer_group.borrow_mut().render(parameters);
            parameters.current_layer += 1;
        }

        // draw layer groups, translucent pass
        parameters.pass = RenderPass::Translucent;
        parameters.depth_range_size = 1.0 - ((num_layer_groups + 2) * num_sublayers) as f32 * depth_epsilon;

        parameters.current_layer = (num_layer_groups as u32).wrapping_sub(1);
        for layer_group in &layer_groups {
            layer_group.borrow_mut().render(parameters);
            if parameters.current_layer > 0 {
                parameters.current_layer -= 1;
            }
        }

        parameters.render_pass = None;
        parameters.encoder.as_ref().expect("encoder").present(renderable);

        parameters.scissor_rect = prev_scissor_rect;
    }
}
