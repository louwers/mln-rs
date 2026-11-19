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
//   its graphics objects are lost (`Renderer::lose_custom_layers`). The
//   layer leaves without a `Gpu` at hand, so it queues its host
//   (`EndedHosts`), which the renderer tells in the frame or the tear-down
//   that follows, with the `Gpu` passed in (`Context::end_custom_layers`).

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::gpu::Gpu;
use mln_style::layer_type::LayerStyle;
use mln_style::layers::{
    Custom, CustomLayerFrame, CustomLayerHost, CustomLayerPrerenderParameters, CustomLayerRenderParameters,
};
use mln_util::logging::{Event, Log};

use crate::bucket::NoBucket;
use crate::gfx::context::Context;
use crate::layer_drawing::{DrawGroup, DrawOrder, LayerDrawing, TileMap};
use crate::layer_type::{EvaluatedLayer, LayerType};
use crate::layout::layout::NoLayout;
use crate::paint_parameters::PaintParameters;
use crate::render_layer::{LayerValues, Parts, PartsMut, RenderLayer, RenderLayerBase, TypedLayer};
use crate::render_pass::RenderPass;
use crate::render_tile::NoTileBucket;

impl LayerType for Custom {
    type Layout = NoLayout;
    type Bucket = NoBucket;
    type TileBucket<'a> = NoTileBucket;

    fn render_layer(style: Arc<LayerStyle<Custom>>) -> Box<dyn RenderLayer> {
        Box::new(RenderCustomLayer::new(style))
    }
}

pub(crate) struct RenderCustomLayer {
    base: RenderLayerBase,
    values: LayerValues<Custom>,
    drawing: CustomDrawing,
}

/// The layer's drawing, which keeps no tiles and no draws of its own: its
/// one draw group is what the host draws.
pub(crate) struct CustomDrawing {
    /// The host, while the renderer draws through it.
    host: Option<Arc<dyn CustomLayerHost>>,
    /// The frame the host last prerendered for.
    frame: Option<CustomLayerFrame>,
    /// Whether a callback ran since the renderer took the host, which then
    /// has to hear that the renderer stopped.
    used: bool,
    /// Where the host goes when the layer leaves it, once it drew: the
    /// renderer's queue, from the first prerender on.
    ended: Option<Rc<EndedHosts>>,
    /// Whether the layer draws, which it does from the first frame that
    /// draws it.
    updated: bool,
}

impl RenderCustomLayer {
    pub(crate) fn new(impl_: Arc<LayerStyle<Custom>>) -> Self {
        let host = impl_.host();
        let (mut base, values) = LayerValues::new(impl_);
        base.passes = RenderPass::Translucent;
        let drawing = CustomDrawing { host, frame: None, used: false, ended: None, updated: false };
        RenderCustomLayer { base, values, drawing }
    }
}

/// The hosts of custom layers that left the renderer after they drew, which
/// the renderer tells that it stopped once the GPU finished their work
/// (`Context::end_custom_layers`). The hosts left in it when it drops, as
/// when a renderer goes without its tear-down, hear that their graphics
/// objects are lost instead.
#[derive(Default)]
pub(crate) struct EndedHosts(Cell<Vec<Arc<dyn CustomLayerHost>>>);

impl EndedHosts {
    fn push(&self, host: Arc<dyn CustomLayerHost>) {
        let mut hosts = self.0.take();
        hosts.push(host);
        self.0.set(hosts);
    }

    /// The hosts queued since the last call.
    pub(crate) fn take(&self) -> Vec<Arc<dyn CustomLayerHost>> {
        self.0.take()
    }
}

impl Drop for EndedHosts {
    fn drop(&mut self) {
        for host in self.0.take() {
            host.context_lost();
        }
    }
}

impl CustomDrawing {
    /// Lets go of the host: queues it to hear that the renderer stopped, or
    /// tells it that its graphics objects are lost.
    fn end(&mut self, lost: bool) {
        let Some(host) = self.host.take() else { return };
        let ended = self.ended.take();
        if !std::mem::take(&mut self.used) {
            return;
        }
        match ended {
            Some(ended) if !lost => ended.push(host),
            _ => host.context_lost(),
        }
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

impl Drop for CustomDrawing {
    fn drop(&mut self) {
        self.end(false);
    }
}

impl LayerDrawing for CustomDrawing {
    type Tile = ();

    fn tiles(&mut self) -> Option<&mut TileMap<()>> {
        None
    }

    fn visit_groups<'a>(&'a self, visit: &mut dyn FnMut(DrawGroup<'a>)) {
        if self.updated {
            visit(DrawGroup::Custom);
        }
    }

    fn clear(&mut self) {}

    /// The layer draws in every frame it is renderable in.
    fn begin_frame(&mut self) {
        self.updated = true;
    }

    fn clear_frame(&mut self) {}

    fn upload(&mut self, _gpu: &mut dyn Gpu, _context: &mut Context) {}

    fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, order: DrawOrder) {
        if self.updated {
            self.lend_pass(gpu, parameters);
            order.step(parameters);
        }
    }
}

impl TypedLayer for RenderCustomLayer {
    type Type = Custom;
    type Drawing = CustomDrawing;

    fn parts(&self) -> Parts<'_, Self> {
        Parts { base: &self.base, values: &self.values, drawing: &self.drawing }
    }

    fn parts_mut(&mut self) -> PartsMut<'_, Self> {
        PartsMut { base: &mut self.base, values: &mut self.values, drawing: &mut self.drawing }
    }

    fn passes(&self, _evaluated: &EvaluatedLayer<Custom>) -> RenderPass {
        RenderPass::Translucent
    }

    /// A new host for the same layer ID is a new layer to the host: the old
    /// one hears that the renderer stopped.
    fn style_changed(&mut self) {
        let drawing = &mut self.drawing;
        let same = drawing.host.as_ref().is_some_and(|host| self.values.style.has_host(host));
        if !same {
            drawing.end(false);
            drawing.host = self.values.style.host();
        }
    }
}

impl RenderLayer for RenderCustomLayer {
    /// The layer has no properties: it only keeps drawing in the
    /// translucent pass.
    fn prerender(&mut self, gpu: &mut dyn Gpu, parameters: &PaintParameters<'_>) {
        let Some(frame) = parameters.custom_layer_frame else {
            return;
        };
        let drawing = &mut self.drawing;
        drawing.frame = Some(frame);
        drawing.ended.get_or_insert_with(|| Rc::clone(parameters.context.ended_hosts()));
        let Some(host) = &drawing.host else { return };
        let lent = gpu.lend(&mut |encoder| {
            host.prerender(&mut CustomLayerPrerenderParameters { encoder, frame: &frame });
        });
        match lent {
            Ok(()) => drawing.used = true,
            Err(error) => Log::error(Event::Render, format!("Lending a custom layer the commands failed: {error}")),
        }
    }

    fn lose_context(&mut self) {
        self.drawing.end(true);
    }
}
