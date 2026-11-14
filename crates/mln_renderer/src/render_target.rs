// The tweakers of the contained layer groups are kept by the orchestrator
// (see change_request.rs), so `render` runs them through it.

use std::cell::RefCell;
use std::rc::Rc;

use mln_gfx::Context;
use mln_gfx::attachment::Attachment;
use mln_gfx::gpu::{ColorAttachment, DepthStencilAttachment, GpuError, LoadOp, Pass, TargetFormats};
use mln_gfx::scissor_rect::ScissorRect;
use mln_gfx::texture2d::{SamplerState, Texture2DPtr, texture_format};
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TexturePixelType, TextureWrapType};
use mln_util::Color;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;
use mln_vec_map::VecMap;

use crate::layer_group::LayerGroupPtr;
use crate::paint_parameters::{PaintParameters, RecordedPass};
use crate::render_orchestrator::RenderOrchestrator;
use crate::render_pass::RenderPass;

pub(crate) type RenderTargetPtr = Rc<RefCell<RenderTarget>>;

/// A texture that layer groups draw into ahead of the frame, which other
/// drawables sample, with the depth texture the backend draws it with
/// (`Context::render_target_depth`).
pub struct RenderTarget {
    texture: Texture2DPtr,
    /// Made with the first pass, of the texture's size.
    depth: Option<Attachment>,
    /// The channel type the texture was asked for.
    type_: TextureChannelDataType,
    layer_groups_by_layer_index: VecMap<i32, LayerGroupPtr>,
}

impl RenderTarget {
    pub(crate) fn new(context: &dyn Context, size: Size, type_: TextureChannelDataType) -> Self {
        RenderTarget {
            texture: texture(context, size, type_),
            depth: None,
            type_,
            layer_groups_by_layer_index: VecMap::new(),
        }
    }

    /// Creates a shared render target of `size` with channel type `type_`.
    pub(crate) fn create(context: &dyn Context, size: Size, type_: TextureChannelDataType) -> RenderTargetPtr {
        Rc::new(RefCell::new(RenderTarget::new(context, size, type_)))
    }

    /// Get the render target texture
    #[must_use]
    pub(crate) fn get_texture(&self) -> Texture2DPtr {
        Rc::clone(&self.texture)
    }

    /// Draws into a texture of `size` from now on: a texture of another size
    /// replaces the current one, and [`get_texture`](Self::get_texture)
    /// returns the new one.
    pub(crate) fn set_size(&mut self, context: &dyn Context, size: Size) {
        if self.texture.borrow().size() != size {
            self.texture = texture(context, size, self.type_);
            self.depth = None;
        }
    }

    /// Add a layer group to the render target; `replace`: replace if
    /// exists. Returns whether added.
    pub(crate) fn add_layer_group(&mut self, layer_group: LayerGroupPtr, replace: bool) -> bool {
        let index = layer_group.borrow().layer_index;
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

    /// Get a specific layer group by index, if existent.
    #[must_use]
    pub(crate) fn get_layer_group(&self, layer_index: i32) -> Option<&LayerGroupPtr> {
        self.layer_groups_by_layer_index.get(&layer_index)
    }

    /// The contained layer groups, in layer index order.
    #[must_use]
    pub(crate) fn layer_groups(&self) -> Vec<LayerGroupPtr> {
        self.layer_groups_by_layer_index.values().cloned().collect()
    }

    /// Upload the layer groups
    pub(crate) fn upload(&self, context: &dyn Context) {
        for layer_group in self.layer_groups_by_layer_index.values() {
            layer_group.borrow_mut().upload(context);
        }
    }

    /// Render the layer groups. `renderTree` is only passed on to
    /// `runTweakers` in C++; the orchestrator runs them here.
    pub(crate) fn render(&mut self, orchestrator: &mut RenderOrchestrator, parameters: &mut PaintParameters<'_>) {
        let pass = match self.begin_pass(parameters.context) {
            Ok(pass) => pass,
            Err(error) => {
                Log::error(Event::Render, format!("Beginning a render target's pass failed: {error}"));
                return;
            }
        };
        let size = pass.size;
        *parameters.recorded.borrow_mut() = Some(pass);

        let prev_scissor_rect = parameters.scissor_rect;
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

        // Submitted now, so that what samples the texture later sees it.
        if let Err(error) = parameters.submit() {
            Log::error(Event::Render, format!("Submitting a render target's pass failed: {error}"));
        }

        parameters.scissor_rect = prev_scissor_rect;
    }

    /// The pass that clears the texture to opaque black and its depth to 1.
    fn begin_pass(&mut self, context: &dyn Context) -> Result<RecordedPass, GpuError> {
        let mut texture = self.texture.borrow_mut();
        let (color, size, format) = (texture.create(context)?, texture.size(), texture.format());
        drop(texture);
        if self.depth.is_none()
            && let Some(depth_format) = context.render_target_depth()
        {
            self.depth = Some(Attachment::new(context, "render target depth", size, depth_format, false)?);
        }
        let pass = Pass {
            label: "render target".to_owned(),
            color: Some(ColorAttachment { texture: color, load: LoadOp::Clear(Color::new(0.0, 0.0, 0.0, 1.0)) }),
            depth_stencil: self.depth.as_ref().map(|depth| DepthStencilAttachment {
                texture: depth.id(),
                depth: LoadOp::Clear(1.0),
                stencil: LoadOp::Load,
            }),
            commands: Vec::new(),
        };
        let targets = TargetFormats { color: Some(format), depth_stencil: self.depth.as_ref().map(Attachment::format) };
        Ok(RecordedPass { pass, size, targets })
    }
}

/// The color texture of a render target of `size`, which drawables draw into
/// and then sample, linear and clamped.
fn texture(context: &dyn Context, size: Size, type_: TextureChannelDataType) -> Texture2DPtr {
    let texture = context.create_texture_2d();
    {
        let mut texture = texture.borrow_mut();
        texture.set_size(size);
        texture.set_format(texture_format(TexturePixelType::RGBA, context.render_target_type(type_)));
        texture.set_render_target();
        texture.set_sampler(SamplerState {
            filter: TextureFilterType::Linear,
            wrap_u: TextureWrapType::Clamp,
            wrap_v: TextureWrapType::Clamp,
            ..SamplerState::default()
        });
    }
    texture
}
