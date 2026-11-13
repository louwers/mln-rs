// Differences from C++:
// - The host's prerender runs in a step of its own after the layer groups
//   upload (`RenderLayer::prerender`), not in `update`, so it gets the frame's
//   final projection, whose depth range depends on how many layer groups the
//   frame has.
// - The drawable is a type of its own whose `draw` lends the backend's live
//   pass to the host (`gfx::RenderPass::lend_to_custom_layer`), which resets
//   the pass's cached state after it; C++ adds a tweaker to an empty drawable.
// - A host that drew is told the renderer stopped once the layer leaves it,
//   after the GPU finished the host's work (`gfx::Context::finish`), or that
//   its graphics objects are lost (`Renderer::lose_custom_layers`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::drawable::{self, DrawableBase, UniqueDrawSegment};
use mln_gfx::index_vector::IndexVectorBasePtr;
use mln_gfx::paint_parameters::PaintParameters as GfxPaintParameters;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::uniform_buffer::UniformBufferArray;
use mln_gfx::vertex_attribute::VertexAttributeArrayPtr;
use mln_gfx::{AttributeDataType, Context, DrawMode, ShaderRegistry};
use mln_map_types::TransformState;
use mln_style::layer::downcast_arc;
use mln_style::layer_properties::LayerProperties;
use mln_style::layers::custom_layer::Impl as CustomLayerImpl;
use mln_style::layers::{
    CustomLayerFrame, CustomLayerHost, CustomLayerPrerenderParameters, CustomLayerProperties,
    CustomLayerRenderParameters,
};
use mln_style::{PropertyEvaluationParameters, TransitionParameters, layer};

use crate::change_request::UniqueChangeRequestVec;
use crate::paint_parameters::PaintParameters;
use crate::render_layer::{LayerPrepareParameters, RenderLayer, RenderLayerBase};
use crate::render_pass::RenderPass;
use crate::render_tree::RenderTree;
use crate::update_parameters::UpdateParameters;

/// What the render layer and its drawable share: the host, while the
/// renderer draws through it, and the frame it prerendered for.
struct Shared {
    host: RefCell<Option<Arc<dyn CustomLayerHost>>>,
    frame: Cell<Option<CustomLayerFrame>>,
    /// Whether a callback ran since the renderer took the host, which then
    /// has to hear that the renderer stopped.
    used: Cell<bool>,
    /// The context of the frame the host last prerendered for, to wait for
    /// its work.
    context: RefCell<Option<Rc<dyn Context>>>,
}

impl Shared {
    fn host(&self) -> Option<Arc<dyn CustomLayerHost>> {
        self.host.borrow().clone()
    }

    /// Lets go of the host: tells it the renderer stopped, once the GPU
    /// finished its work, or that its graphics objects are lost.
    fn end(&self, lost: bool) {
        let Some(host) = self.host.borrow_mut().take() else { return };
        let context = self.context.borrow_mut().take();
        if !self.used.replace(false) {
            return;
        }
        if lost {
            host.context_lost();
        } else {
            if let Some(context) = context {
                context.finish();
            }
            host.deinitialize();
        }
    }
}

pub(crate) struct RenderCustomLayer {
    base: RenderLayerBase,
    shared: Rc<Shared>,
}

impl RenderCustomLayer {
    pub(crate) fn new(impl_: Arc<CustomLayerImpl>) -> Self {
        let host = impl_.host();
        let properties: Arc<dyn LayerProperties> = Arc::new(CustomLayerProperties::new(impl_));
        let mut base = RenderLayerBase::new(properties);
        base.passes = RenderPass::Translucent;
        RenderCustomLayer {
            base,
            shared: Rc::new(Shared {
                host: RefCell::new(host),
                frame: Cell::new(None),
                used: Cell::new(false),
                context: RefCell::new(None),
            }),
        }
    }

    /// Tells the host that its graphics objects are lost, and lets go of it.
    pub(crate) fn lose_context(&mut self) {
        self.shared.end(true);
    }
}

impl Drop for RenderCustomLayer {
    fn drop(&mut self) {
        self.shared.end(false);
    }
}

impl RenderLayer for RenderCustomLayer {
    fn base(&self) -> &RenderLayerBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut RenderLayerBase {
        &mut self.base
    }

    fn transition(&mut self, _parameters: &TransitionParameters) {}

    /// The layer has no properties: `baseImpl` never changes for it.
    fn evaluate(&mut self, _parameters: &PropertyEvaluationParameters) {
        self.base.passes = RenderPass::Translucent;
    }

    fn has_transition(&self) -> bool {
        false
    }

    fn has_crossfade(&self) -> bool {
        false
    }

    fn prepare(&mut self, _parameters: &mut LayerPrepareParameters<'_>) {}

    /// A new host for the same layer ID is a new layer to the host: the old
    /// one hears that the renderer stopped.
    fn layer_changed(
        &mut self,
        _parameters: &TransitionParameters,
        new_layer: &Arc<dyn layer::Impl>,
        _changes: &mut UniqueChangeRequestVec,
    ) {
        let new_layer = downcast_arc::<CustomLayerImpl>(new_layer);
        let same = self.shared.host.borrow().as_ref().is_some_and(|host| new_layer.has_host(host));
        if !same {
            self.shared.end(false);
            *self.shared.host.borrow_mut() = new_layer.host();
        }
    }

    fn update(
        &mut self,
        _shaders: &ShaderRegistry,
        context: &dyn Context,
        _state: &TransformState,
        _update_parameters: &Arc<UpdateParameters>,
        _paint_parameters: &PaintParameters<'_>,
        _render_tree: &RenderTree,
        changes: &mut UniqueChangeRequestVec,
    ) {
        if self.base.layer_group.is_none() {
            match context.create_layer_group(
                self.base.layer_index,
                /*initial_capacity=*/ 1,
                self.get_id().to_owned(),
            ) {
                Some(layer_group) => self.set_layer_group(Some(layer_group), changes),
                None => return,
            }
        }
        let Some(layer_group) = self.base.layer_group.clone() else { return };
        let mut group = layer_group.borrow_mut();
        if group.get_drawable_count() == 0
            && let Some(group) = group.as_layer_group_mut()
        {
            group.add_drawable(Box::new(CustomDrawable::new(self.get_id().to_owned(), Rc::clone(&self.shared))));
        }
    }

    fn prerender(&mut self, parameters: &PaintParameters<'_>, target: &dyn mln_gfx::DrawTarget) {
        let Some(frame) = parameters.custom_layer_frame else {
            return;
        };
        self.shared.frame.set(Some(frame));
        self.shared.context.replace(Some(Rc::clone(parameters.backend.get_context_ptr())));
        let Some(host) = self.shared.host() else { return };
        let Some(encoder) = parameters.encoder.as_deref() else { return };
        let lent = encoder.lend_to_custom_layer(target, &mut |encoder| {
            host.prerender(&mut CustomLayerPrerenderParameters { encoder, frame: &frame });
        });
        if lent {
            self.shared.used.set(true);
        }
    }

    fn lose_context(&mut self) {
        RenderCustomLayer::lose_context(self);
    }
}

/// The drawable of a custom layer: drawing it lends the live pass to the
/// host.
struct CustomDrawable {
    base: DrawableBase,
    shared: Rc<Shared>,
    uniform_buffers: UniformBufferArray,
}

impl CustomDrawable {
    fn new(name: String, shared: Rc<Shared>) -> Self {
        let mut base = DrawableBase::new(name);
        base.render_pass = RenderPass::Translucent;
        CustomDrawable { base, shared, uniform_buffers: UniformBufferArray::new() }
    }
}

impl drawable::Drawable for CustomDrawable {
    fn base(&self) -> &DrawableBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut DrawableBase {
        &mut self.base
    }

    fn draw(&mut self, parameters: &mut dyn GfxPaintParameters) {
        let parameters: &dyn GfxPaintParameters = parameters;
        let (Some(host), Some(frame), Some(render_pass)) =
            (self.shared.host(), self.shared.frame.get(), parameters.render_pass())
        else {
            return;
        };
        let layer_depth = f64::from(parameters.layer_depth());
        let lent = render_pass.lend_to_custom_layer(&mut |pass| {
            host.render(&mut CustomLayerRenderParameters { pass, frame: &frame, layer_depth });
        });
        if lent {
            self.shared.used.set(true);
        }
    }

    fn update_vertex_attributes(
        &mut self,
        _vertices: Option<VertexAttributeArrayPtr>,
        _vertex_count: usize,
        _mode: DrawMode,
        _indexes: Option<IndexVectorBasePtr>,
        _segments: &[SegmentBase],
    ) {
    }

    fn set_vertices(&mut self, _data: Vec<u8>, _count: usize, _type: AttributeDataType) {}

    fn set_index_data(&mut self, _indexes: Option<IndexVectorBasePtr>, _segments: Vec<UniqueDrawSegment>) {}

    fn mutable_uniform_buffers(&mut self) -> &mut UniformBufferArray {
        &mut self.uniform_buffers
    }
}
