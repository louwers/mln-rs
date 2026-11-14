// How a custom layer differs from the others:
// - The host's prerender runs in a step of its own after the layer groups
//   upload (`RenderLayer::prerender`), not in `update`, so it gets the frame's
//   final projection, whose depth range depends on how many layer groups the
//   frame has.
// - The layer group is of a kind of its own, whose `render` lends the
//   frame's pass to the host (`PaintParameters::lend_pass`, through
//   `Gpu::lend_to_custom_layer`).
// - A host that drew is told the renderer stopped once the layer leaves it,
//   after the GPU finished the host's work (`Gpu::finish`), or that
//   its graphics objects are lost (`Renderer::lose_custom_layers`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::{Context, ShaderRegistry};
use mln_map_types::TransformState;
use mln_style::layer::downcast_arc;
use mln_style::layer_properties::LayerProperties;
use mln_style::layers::custom_layer::Impl as CustomLayerImpl;
use mln_style::layers::{
    CustomLayerFrame, CustomLayerHost, CustomLayerPrerenderParameters, CustomLayerProperties,
    CustomLayerRenderParameters,
};
use mln_style::{PropertyEvaluationParameters, TransitionParameters, layer};
use mln_util::logging::{Event, Log};

use crate::change_request::UniqueChangeRequestVec;
use crate::layer_group::{LayerGroup, LayerGroupKind};
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
            // The host frees what it drew with once the GPU is done with it.
            if let Some(context) = context
                && let Err(error) = context.gpu().finish()
            {
                Log::error(Event::Render, format!("Waiting for a custom layer's frames failed: {error}"));
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
        _context: &dyn Context,
        _state: &TransformState,
        _update_parameters: &Arc<UpdateParameters>,
        _paint_parameters: &PaintParameters<'_>,
        _render_tree: &RenderTree,
        changes: &mut UniqueChangeRequestVec,
    ) {
        if self.base.layer_group.is_none() {
            let kind = LayerGroupKind::Custom(CustomLayerDraw { shared: Rc::clone(&self.shared) });
            self.set_layer_group(Some(LayerGroup::new(self.base.layer_index, self.get_id(), kind)), changes);
        }
    }

    fn prerender(&mut self, parameters: &PaintParameters<'_>) {
        let Some(frame) = parameters.custom_layer_frame else {
            return;
        };
        self.shared.frame.set(Some(frame));
        self.shared.context.replace(Some(Rc::clone(parameters.backend.get_context_ptr())));
        let Some(host) = self.shared.host() else { return };
        let lent = parameters.context.gpu().lend_to_custom_layer(None, &mut |encoder| {
            host.prerender(&mut CustomLayerPrerenderParameters { encoder, frame: &frame });
        });
        match lent {
            Ok(()) => self.shared.used.set(true),
            Err(error) => Log::error(Event::Render, format!("Lending a custom layer the commands failed: {error}")),
        }
    }

    fn lose_context(&mut self) {
        RenderCustomLayer::lose_context(self);
    }
}

/// What a custom layer's layer group draws: in the translucent pass, it
/// lends the live pass to the host.
pub(crate) struct CustomLayerDraw {
    shared: Rc<Shared>,
}

impl CustomLayerDraw {
    pub(crate) fn render(&self, parameters: &PaintParameters<'_>) {
        if parameters.pass != RenderPass::Translucent {
            return;
        }
        let (Some(host), Some(frame)) = (self.shared.host(), self.shared.frame.get()) else {
            return;
        };
        let layer_depth = f64::from(parameters.layer_depth());
        let lent = parameters.lend_pass(&mut |pass| {
            host.render(&mut CustomLayerRenderParameters { pass, frame: &frame, layer_depth });
        });
        match lent {
            Ok(lent) => {
                if lent {
                    self.shared.used.set(true);
                }
            }
            Err(error) => Log::error(Event::Render, format!("Lending a custom layer the pass failed: {error}")),
        }
    }
}
