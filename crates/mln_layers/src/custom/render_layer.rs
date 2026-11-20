// How a custom layer differs from the others:
// - The host's prerender runs in the layer's offscreen pass, after the
//   layers' draws upload, not in `make_draws`, so it gets the frame's final
//   projection, whose depth range depends on how many draw groups the frame
//   has.
// - What it draws is a draw group of its own (`DrawGroup::Own`), whose
//   `render` lends the frame's pass to the host
//   (`PaintParameters::lend_pass`, through `Gpu::lend`).
// - A host that drew is told the renderer stopped once the layer leaves it,
//   after the GPU finished the host's work, or that its graphics objects
//   are lost (`Renderer::lose_layer_hosts`, or a renderer dropped without
//   its tear-down). A layer leaves (`LayerHost::leave`) without a `Gpu` at
//   hand, so the engine keeps the `EndedHost` it returns until it can end
//   it with one.

use std::sync::Arc;

use mln_gfx::gpu::Gpu;
use mln_renderer::layer::{
    DrawContext, EndedHost, EvaluatedLayer, LayerHost, LayerType, LayerValues, NoBucket, NoLayout, NoTileView,
    OffscreenPass, RenderPass,
};
use mln_renderer::unstable::{Context, DrawGroup, DrawOrder, LayerDrawing, PaintParameters, TileMap};
use mln_util::logging::{Event, Log};

use super::{Custom, CustomLayerFrame, CustomLayerHost, CustomLayerPrerenderParameters, CustomLayerRenderParameters};

/// The layer's drawing, which keeps no tiles and no draws of its own: its
/// one draw group is what the host draws.
pub struct CustomDrawing {
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

impl CustomDrawing {
    /// Lets go of the host, and returns it if it drew.
    fn take_host(&mut self) -> Option<Arc<dyn CustomLayerHost>> {
        self.host.take().filter(|_| std::mem::take(&mut self.used))
    }

    /// Lets go of the host, and returns what tells it that the renderer
    /// stopped if it drew.
    fn end_host(&mut self) -> Option<Box<dyn EndedHost>> {
        Some(Box::new(EndedCustomHost(self.take_host()?)))
    }

    /// In the translucent pass, lends the live pass to the host.
    fn lend_pass(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        if parameters.pass() != RenderPass::TRANSLUCENT {
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

impl LayerDrawing for CustomDrawing {
    type Tile = ();

    fn tiles(&mut self) -> Option<&mut TileMap<()>> {
        None
    }

    fn visit_groups<'a>(&'a self, visit: &mut dyn FnMut(DrawGroup<'a>)) {
        if self.updated {
            visit(DrawGroup::Own);
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

impl LayerType for Custom {
    type Layout = NoLayout;
    type Bucket = NoBucket;
    type View<'a> = NoTileView;

    type Drawing = CustomDrawing;
    type State = ();

    fn new_layer(layer: &LayerValues<Custom>) -> ((), CustomDrawing) {
        let drawing = CustomDrawing { host: layer.style.extra.get(), frame: None, used: false, updated: false };
        ((), drawing)
    }

    /// The layer has no properties: it only keeps drawing in the
    /// translucent pass.
    fn passes(_layer: &LayerValues<Custom>, _evaluated: &EvaluatedLayer<Custom>) -> RenderPass {
        RenderPass::TRANSLUCENT
    }

    /// The layer's draws are what its host draws, in its draw group.
    fn make_draws(
        (): &mut (),
        _drawing: &mut CustomDrawing,
        _gpu: &mut dyn Gpu,
        _cx: &mut DrawContext<'_, '_, '_, Custom>,
    ) {
    }

    fn offscreen_pass<'a>(_state: &'a mut (), drawing: &'a mut CustomDrawing) -> Option<&'a mut dyn OffscreenPass> {
        Some(drawing)
    }

    fn host(drawing: &mut CustomDrawing) -> Option<&mut dyn LayerHost<Custom>> {
        Some(drawing)
    }
}

/// The host's prerender, with the frame's projection.
impl OffscreenPass for CustomDrawing {
    fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        let Some(projection) = parameters.frame_projection else {
            return;
        };
        let frame = CustomLayerFrame {
            view: parameters.view.map_view(f64::from(parameters.pixel_ratio)),
            projection,
            width: parameters.renderable_size.width,
            height: parameters.renderable_size.height,
        };
        self.frame = Some(frame);
        let Some(host) = &self.host else { return };
        let lent = gpu.lend(&mut |encoder| {
            host.prerender(&mut CustomLayerPrerenderParameters { encoder, frame: &frame });
        });
        match lent {
            Ok(()) => self.used = true,
            Err(error) => Log::error(Event::Render, format!("Lending a custom layer the commands failed: {error}")),
        }
    }
}

/// A host that drew and that its layer let go of.
struct EndedCustomHost(Arc<dyn CustomLayerHost>);

impl EndedHost for EndedCustomHost {
    fn end(self: Box<Self>) {
        self.0.deinitialize();
    }

    fn lose(self: Box<Self>) {
        self.0.context_lost();
    }
}

impl LayerHost<Custom> for CustomDrawing {
    /// A new host for the same layer ID is a new layer to the host: the old
    /// one ends.
    fn restyled(&mut self, layer: &LayerValues<Custom>) -> Option<Box<dyn EndedHost>> {
        let same = self.host.as_ref().is_some_and(|host| layer.style.extra.is(host));
        if same {
            return None;
        }
        let ended = self.end_host();
        self.host = layer.style.extra.get();
        ended
    }

    fn leave(&mut self) -> Option<Box<dyn EndedHost>> {
        self.end_host()
    }

    fn lose_context(&mut self) {
        if let Some(host) = self.take_host() {
            host.context_lost();
        }
    }
}
