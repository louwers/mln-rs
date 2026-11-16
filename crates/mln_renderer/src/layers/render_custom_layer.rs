// How a custom layer differs from the others:
// - The host's prerender runs in a step of its own after the layer groups
//   upload (`RenderLayer::prerender`), not in `update`, so it gets the frame's
//   final projection, whose depth range depends on how many layer groups the
//   frame has.
// - The layer group is of a kind of its own, whose `render` lends the
//   frame's pass to the host (`PaintParameters::lend_pass`, through
//   `Gpu::lend`).
// - A host that drew is told the renderer stopped once the layer leaves it,
//   after the GPU finished the host's work (`Gpu::flush`), or that
//   its graphics objects are lost (`Renderer::lose_custom_layers`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::gpu::Gpu;
use mln_gfx::{RendererBackend, ShaderRegistry};
use mln_map_types::TransformState;
use mln_style::layers::custom_layer::Impl as CustomLayerImpl;
use mln_style::layers::{
    Custom, CustomLayerFrame, CustomLayerHost, CustomLayerPrerenderParameters, CustomLayerRenderParameters,
};
use mln_style::{PropertyEvaluationParameters, TransitionParameters, layer};
use mln_util::logging::{Event, Log};

use crate::bucket::NoBucket;
use crate::change_request::UniqueChangeRequestVec;
use crate::gfx::context::Context;
use crate::layer_group::{LayerGroup, LayerGroupKind};
use crate::layer_type::{ErasedEvaluatedLayer, EvaluatedLayer, LayerType};
use crate::layout::layout::NoLayout;
use crate::paint_parameters::PaintParameters;
use crate::render_layer::{LayerPrepareParameters, RenderLayer, RenderLayerBase, RenderLayerPtr, set_style};
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
    /// The backend of the frame the host last prerendered for, to wait for
    /// its work.
    backend: RefCell<Option<Rc<dyn RendererBackend>>>,
}

impl Shared {
    fn host(&self) -> Option<Arc<dyn CustomLayerHost>> {
        self.host.borrow().clone()
    }

    /// Lets go of the host: tells it the renderer stopped, once the GPU
    /// finished its work, or that its graphics objects are lost. The
    /// renderer calls it inside the backend's hold, so the host
    /// deinitializes with what it drew with ready, such as OpenGL's context
    /// current.
    fn end(&self, lost: bool) {
        let Some(host) = self.host.borrow_mut().take() else { return };
        let backend = self.backend.borrow_mut().take();
        if !self.used.replace(false) {
            return;
        }
        if lost {
            host.context_lost();
        } else {
            // The host frees what it drew with once the GPU is done with it.
            if let Some(backend) = backend
                && let Err(error) = backend.gpu().flush(true)
            {
                Log::error(Event::Render, format!("Waiting for a custom layer's frames failed: {error}"));
            }
            host.deinitialize();
        }
    }
}

impl LayerType for Custom {
    type Layout = NoLayout;
    type Bucket = NoBucket;

    fn render_layer(style: Arc<CustomLayerImpl>) -> RenderLayerPtr {
        Rc::new(RefCell::new(RenderCustomLayer::new(style)))
    }
}

pub(crate) struct RenderCustomLayer {
    base: RenderLayerBase,
    style: Arc<CustomLayerImpl>,
    shared: Rc<Shared>,
}

impl RenderCustomLayer {
    pub(crate) fn new(impl_: Arc<CustomLayerImpl>) -> Self {
        let host = impl_.host();
        let properties: Arc<dyn ErasedEvaluatedLayer> = Arc::new(EvaluatedLayer::<Custom>::new(Arc::clone(&impl_)));
        let mut base = RenderLayerBase::new(properties);
        base.passes = RenderPass::Translucent;
        RenderCustomLayer {
            base,
            style: impl_,
            shared: Rc::new(Shared {
                host: RefCell::new(host),
                frame: Cell::new(None),
                used: Cell::new(false),
                backend: RefCell::new(None),
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

    /// A new host for the same layer ID is a new layer to the host: the old
    /// one hears that the renderer stopped.
    fn restyle(&mut self, parameters: &TransitionParameters, style: &Arc<dyn layer::Impl>) {
        set_style(&mut self.base, &mut self.style, style);
        let same = self.shared.host.borrow().as_ref().is_some_and(|host| self.style.has_host(host));
        if !same {
            self.shared.end(false);
            *self.shared.host.borrow_mut() = self.style.host();
        }
        self.transition(parameters);
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

    fn update(
        &mut self,
        _shaders: &ShaderRegistry,
        _context: &Context,
        _state: &TransformState,
        _update_parameters: &Arc<UpdateParameters>,
        _paint_parameters: &PaintParameters<'_>,
        _render_tree: &RenderTree,
        changes: &mut UniqueChangeRequestVec,
    ) {
        if self.base.layer_group.is_none() {
            let kind = LayerGroupKind::Custom(CustomLayerDraw { shared: Rc::clone(&self.shared) });
            self.set_layer_group(Some(LayerGroup::new(self.base.layer_index, kind)), changes);
        }
    }

    fn prerender(&mut self, parameters: &PaintParameters<'_>) {
        let Some(frame) = parameters.custom_layer_frame else {
            return;
        };
        self.shared.frame.set(Some(frame));
        self.shared.backend.replace(Some(Rc::clone(parameters.context.backend())));
        let Some(host) = self.shared.host() else { return };
        let lent = parameters.context.gpu().lend(&mut |encoder| {
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
    pub(crate) fn render(&self, gpu: &mut dyn Gpu, parameters: &PaintParameters<'_>) {
        if parameters.pass != RenderPass::Translucent {
            return;
        }
        let (Some(host), Some(frame)) = (self.shared.host(), self.shared.frame.get()) else {
            return;
        };
        let layer_depth = f64::from(parameters.layer_depth());
        let lent = parameters.lend_pass(gpu, &mut |pass| {
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
