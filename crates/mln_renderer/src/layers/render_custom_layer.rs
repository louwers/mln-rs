// How a custom layer differs from the others:
// - The host's prerender runs in a step of its own after the layers' draws
//   upload (`RenderLayer::prerender`), not in `update`, so it gets the frame's
//   final projection, whose depth range depends on how many draw groups the
//   frame has.
// - What it draws is a draw group of its own (`DrawGroup::Custom`), whose
//   `render` lends the frame's pass to the host
//   (`PaintParameters::lend_pass`, through `Gpu::lend`).
// - A host that drew is told the renderer stopped once the layer leaves it,
//   after the GPU finished the host's work (`Gpu::flush`), or that
//   its graphics objects are lost (`Renderer::lose_custom_layers`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::RendererBackend;
use mln_gfx::gpu::Gpu;
use mln_style::layer_type::{ErasedLayerStyle, LayerStyle};
use mln_style::layers::{
    Custom, CustomLayerFrame, CustomLayerHost, CustomLayerPrerenderParameters, CustomLayerRenderParameters,
};
use mln_style::{PropertyEvaluationParameters, TransitionParameters};
use mln_util::logging::{Event, Log};

use crate::bucket::NoBucket;
use crate::draw_cache::{DrawCaches, DrawGroup};
use crate::layer_type::LayerType;
use crate::layout::layout::NoLayout;
use crate::paint_parameters::PaintParameters;
use crate::render_layer::{
    DrawGroups, LayerParts, LayerPrepareParameters, LayerTiles, LayerValues, RenderLayer, RenderLayerBase, TypedLayer,
};
use crate::render_pass::RenderPass;
use crate::update_parameters::UpdateParameters;

/// What the render layer and what it draws share: the host, while the
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

    fn render_layer(style: Arc<LayerStyle<Custom>>) -> Box<dyn RenderLayer> {
        Box::new(RenderCustomLayer::new(style))
    }
}

pub(crate) struct RenderCustomLayer {
    base: RenderLayerBase,
    values: LayerValues<Custom>,
    /// Empty: the host draws with objects of its own.
    caches: DrawCaches<()>,
    shared: Rc<Shared>,
    /// Whether the layer draws, which it does from its first update.
    updated: bool,
}

impl RenderCustomLayer {
    pub(crate) fn new(impl_: Arc<LayerStyle<Custom>>) -> Self {
        let host = impl_.host();
        let (mut base, values) = LayerValues::new(impl_);
        base.passes = RenderPass::Translucent;
        RenderCustomLayer {
            base,
            values,
            caches: DrawCaches::default(),
            shared: Rc::new(Shared {
                host: RefCell::new(host),
                frame: Cell::new(None),
                used: Cell::new(false),
                backend: RefCell::new(None),
            }),
            updated: false,
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

impl TypedLayer for RenderCustomLayer {
    type Type = Custom;
    type Tile = ();
    type Shared = ();

    fn parts(&self) -> (&RenderLayerBase, &LayerValues<Custom>) {
        (&self.base, &self.values)
    }

    fn parts_mut(&mut self) -> (&mut RenderLayerBase, &mut LayerValues<Custom>) {
        (&mut self.base, &mut self.values)
    }

    fn caches_mut(&mut self) -> (&mut RenderLayerBase, &mut DrawCaches<()>) {
        (&mut self.base, &mut self.caches)
    }
}

impl RenderLayer for RenderCustomLayer {
    /// A new host for the same layer ID is a new layer to the host: the old
    /// one hears that the renderer stopped.
    fn restyle(&mut self, parameters: &TransitionParameters, style: &Arc<dyn ErasedLayerStyle>) {
        self.set_style(style);
        let same = self.shared.host.borrow().as_ref().is_some_and(|host| self.values.style.has_host(host));
        if !same {
            self.shared.end(false);
            *self.shared.host.borrow_mut() = self.values.style.host();
        }
        self.transition(parameters);
    }

    /// The layer has no properties: `baseImpl` never changes for it.
    fn evaluate(&mut self, _parameters: &PropertyEvaluationParameters) {
        self.base.passes = RenderPass::Translucent;
    }

    fn prepare(&mut self, _parameters: &LayerPrepareParameters<'_>) -> Option<Vec<usize>> {
        None
    }

    fn update(
        &mut self,
        _parameters: &PaintParameters<'_>,
        _update_parameters: &UpdateParameters,
        _tiles: Option<LayerTiles<'_>>,
    ) {
        self.updated = true;
    }

    fn draw_groups(&self) -> DrawGroups<'_> {
        (self.updated && self.base.is_renderable).then_some(DrawGroup::Custom).into_iter().collect()
    }

    /// In the translucent pass, lends the live pass to the host.
    fn render(&mut self, _group: usize, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        if parameters.pass_target().is_none() || parameters.pass != RenderPass::Translucent {
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
