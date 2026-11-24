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
//   its graphics objects are lost (`Renderer::lose_custom_layers`). A layer
//   leaves (`RenderLayer::leave`) without a `Gpu` at hand, so its host waits
//   in the orchestrator's `EndedHosts` until the renderer ends it with one
//   (`end_hosts`).

use std::sync::Arc;

use mln_gfx::gpu::Gpu;
use mln_style::layer::LayerStyle;
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
#[derive(Default)]
pub(crate) struct CustomDrawing {
    /// The host, while the renderer draws through it.
    host: Option<Arc<dyn CustomLayerHost>>,
    /// The frame the host last prerendered for.
    frame: Option<CustomLayerFrame>,
    /// Whether a callback ran since the renderer took the host, which then
    /// has to hear that the renderer stopped.
    used: bool,
    /// Whether the layer draws, which it does from the first frame that
    /// draws it.
    updated: bool,
}

impl RenderCustomLayer {
    pub(crate) fn new(impl_: Arc<LayerStyle<Custom>>) -> Self {
        let host = impl_.host();
        let (mut base, values) = LayerValues::new(impl_);
        base.passes = RenderPass::Translucent;
        let drawing = CustomDrawing { host, frame: None, used: false, updated: false };
        RenderCustomLayer { base, values, drawing }
    }
}

/// The hosts of custom layers that left the renderer after they drew, which
/// [`end_hosts`] tells that it stopped.
pub(crate) type EndedHosts = Vec<Arc<dyn CustomLayerHost>>;

/// Tells `hosts`, which left the renderer after drawing with `gpu`, that it
/// stopped, once `gpu` finished their work, so that they free what they drew
/// with.
pub(crate) fn end_hosts(gpu: &mut dyn Gpu, hosts: EndedHosts) {
    if hosts.is_empty() {
        return;
    }
    if let Err(error) = gpu.flush(true) {
        Log::error(Event::Render, format!("Waiting for a custom layer's frames failed: {error}"));
    }
    for host in hosts {
        host.deinitialize();
    }
}

impl CustomDrawing {
    /// Lets go of the host, and returns it if it drew.
    fn take_host(&mut self) -> Option<Arc<dyn CustomLayerHost>> {
        self.host.take().filter(|_| std::mem::take(&mut self.used))
    }

    /// Lets go of the host, telling it that its graphics objects are lost if
    /// it drew.
    fn lose(&mut self) {
        if let Some(host) = self.take_host() {
            host.context_lost();
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
            Err(error) => parameters.context.fail(format!("Lending a custom layer the pass failed: {error}")),
        }
    }
}

impl Drop for CustomDrawing {
    /// A layer dropped without leaving, as with a renderer dropped without
    /// its tear-down, loses its host's graphics objects.
    fn drop(&mut self) {
        self.lose();
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
    /// one goes to `ended`, to hear that the renderer stopped.
    fn style_changed(&mut self, ended: &mut EndedHosts) {
        let drawing = &mut self.drawing;
        let same = drawing.host.as_ref().is_some_and(|host| self.values.style.has_host(host));
        if !same {
            ended.extend(drawing.take_host());
            drawing.host = self.values.style.host();
        }
    }
}

impl RenderLayer for RenderCustomLayer {
    /// The layer has no properties: it only keeps drawing in the
    /// translucent pass.
    fn prerender(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        let Some(frame) = parameters.custom_layer_frame else {
            return;
        };
        let drawing = &mut self.drawing;
        drawing.frame = Some(frame);
        let Some(host) = &drawing.host else { return };
        let lent = gpu.lend(&mut |encoder| {
            host.prerender(&mut CustomLayerPrerenderParameters { encoder, frame: &frame });
        });
        match lent {
            Ok(()) => drawing.used = true,
            Err(error) => parameters.context.fail(format!("Lending a custom layer the commands failed: {error}")),
        }
    }

    fn leave(&mut self, ended: &mut EndedHosts) {
        ended.extend(self.drawing.take_host());
    }

    fn lose_context(&mut self) {
        self.drawing.lose();
    }
}
