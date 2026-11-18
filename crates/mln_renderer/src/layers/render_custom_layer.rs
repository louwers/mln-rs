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
use crate::draw_cache::{ChangedBuckets, DrawGroup, DrawOrder};
use crate::gfx::context::Context;
use crate::layer_type::LayerType;
use crate::layout::layout::NoLayout;
use crate::paint_parameters::PaintParameters;
use crate::render_layer::{
    LayerParts, LayerPrepareParameters, LayerSource, LayerTiles, LayerValues, RenderLayer, RenderLayerBase,
};
use crate::render_pass::RenderPass;
use crate::render_source::TileOrder;
use crate::update_parameters::UpdateParameters;

impl LayerType for Custom {
    type Layout = NoLayout;
    type Bucket = NoBucket;

    fn render_layer(style: Arc<LayerStyle<Custom>>) -> Box<dyn RenderLayer> {
        Box::new(RenderCustomLayer::new(style))
    }
}

pub(crate) struct RenderCustomLayer {
    base: RenderLayerBase,
    style: Arc<LayerStyle<Custom>>,
    /// The host, while the renderer draws through it.
    host: Option<Arc<dyn CustomLayerHost>>,
    /// The frame the host last prerendered for.
    frame: Option<CustomLayerFrame>,
    /// Whether a callback ran since the renderer took the host, which then
    /// has to hear that the renderer stopped.
    used: bool,
    /// The backend of the frame the host last prerendered for, to wait for
    /// its work.
    backend: Option<Rc<dyn RendererBackend>>,
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
            style: values.style,
            host,
            frame: None,
            used: false,
            backend: None,
            updated: false,
        }
    }

    /// Lets go of the host: tells it the renderer stopped, once the GPU
    /// finished its work, or that its graphics objects are lost. The
    /// renderer calls it inside the backend's hold, so the host
    /// deinitializes with what it drew with ready, such as OpenGL's context
    /// current.
    fn end(&mut self, lost: bool) {
        let Some(host) = self.host.take() else { return };
        let backend = self.backend.take();
        if !std::mem::take(&mut self.used) {
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

    /// Whether the frame draws the layer's draw group.
    fn draws(&self) -> bool {
        self.updated && self.base.is_renderable
    }

    /// In the translucent pass, lends the live pass to the host.
    fn lend_pass(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        if parameters.pass_target().is_none() || parameters.pass != RenderPass::Translucent {
            return;
        }
        let (Some(host), Some(frame)) = (&self.host, self.frame) else {
            return;
        };
        let layer_depth = f64::from(parameters.layer_depth());
        let lent = parameters.lend_pass(gpu, &mut |pass| {
            host.render(&mut CustomLayerRenderParameters { pass, frame: &frame, layer_depth });
        });
        match lent {
            Ok(lent) => {
                self.used |= lent;
            }
            Err(error) => Log::error(Event::Render, format!("Lending a custom layer the pass failed: {error}")),
        }
    }
}

impl Drop for RenderCustomLayer {
    fn drop(&mut self) {
        self.end(false);
    }
}

/// The layer has no paint properties to transition, and no draws of its
/// own: its one draw group is what the host draws.
impl LayerParts for RenderCustomLayer {
    fn base(&self) -> &RenderLayerBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut RenderLayerBase {
        &mut self.base
    }

    fn set_style(&mut self, style: &Arc<dyn ErasedLayerStyle>) {
        if let Some(recovered) = LayerStyle::recover(style) {
            self.style = recovered;
            self.base.style = Arc::clone(style);
        }
    }

    fn transition(&mut self, _parameters: &TransitionParameters) {}

    fn has_transition(&self) -> bool {
        false
    }

    fn has_crossfade(&self) -> bool {
        false
    }

    fn set_tiles(&mut self, _tiles: Option<LayerTiles<'_>>) {}

    fn mark_changed(&mut self, _changed: &ChangedBuckets) {}

    fn handle_wrap_jump(&mut self, _wrap_delta: i16) {}

    fn remove_all_draws(&mut self) {}

    fn visit_draw_groups<'a>(&'a self, visit: &mut dyn FnMut(DrawGroup<'a>)) {
        if self.draws() {
            visit(DrawGroup::Custom);
        }
    }

    fn upload(&mut self, _context: &Context) {}

    fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, order: DrawOrder) {
        if self.draws() {
            self.lend_pass(gpu, parameters);
            order.step(parameters);
        }
    }
}

impl RenderLayer for RenderCustomLayer {
    /// A new host for the same layer ID is a new layer to the host: the old
    /// one hears that the renderer stopped.
    fn restyle(&mut self, parameters: &TransitionParameters, style: &Arc<dyn ErasedLayerStyle>) {
        self.set_style(style);
        let same = self.host.as_ref().is_some_and(|host| self.style.has_host(host));
        if !same {
            self.end(false);
            self.host = self.style.host();
        }
        self.transition(parameters);
    }

    /// The layer has no properties: it only keeps drawing in the
    /// translucent pass.
    fn evaluate(&mut self, _parameters: &PropertyEvaluationParameters) {
        self.base.passes = RenderPass::Translucent;
    }

    fn prepare(&mut self, _parameters: &LayerPrepareParameters<'_>) -> Option<TileOrder> {
        None
    }

    fn update(
        &mut self,
        _parameters: &PaintParameters<'_>,
        _update_parameters: &UpdateParameters,
        _source: Option<LayerSource<'_>>,
    ) {
        self.updated = true;
    }

    fn prerender(&mut self, parameters: &PaintParameters<'_>) {
        let Some(frame) = parameters.custom_layer_frame else {
            return;
        };
        self.frame = Some(frame);
        self.backend = Some(Rc::clone(parameters.context.backend()));
        let Some(host) = &self.host else { return };
        let lent = parameters.context.gpu().lend(&mut |encoder| {
            host.prerender(&mut CustomLayerPrerenderParameters { encoder, frame: &frame });
        });
        match lent {
            Ok(()) => self.used = true,
            Err(error) => Log::error(Event::Render, format!("Lending a custom layer the commands failed: {error}")),
        }
    }

    fn lose_context(&mut self) {
        self.end(true);
    }
}
