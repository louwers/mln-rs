use std::sync::Arc;

use mln_render::draw::Draws;
use mln_render::layer::{AfterGpu, EvaluateCx, Evaluation, GpuEnd, Layer, LendAt, Lent, NewCx, NoTiles, PrepareCx};

use super::{Custom, CustomLayerFrame, CustomLayerHost, CustomLayerPrerenderParameters, CustomLayerRenderParameters};

/// A custom layer: what its host draws with the backend's commands, which
/// the frame lends it before its passes and at the layer's place.
pub struct CustomLayer {
    /// The host, while the renderer draws through it.
    host: Option<Arc<dyn CustomLayerHost>>,
    /// Whether the host drew since the renderer took it, so that it hears
    /// when the renderer stops.
    used: bool,
    after_gpu: AfterGpu,
}

impl CustomLayer {
    /// Lets go of the host, which hears that the renderer stopped once the
    /// GPU finished what it drew, if it drew.
    fn end(&mut self) {
        let Some(host) = self.host.take().filter(|_| std::mem::take(&mut self.used)) else { return };
        self.after_gpu.push(move |end| match end {
            GpuEnd::Finished => host.deinitialize(),
            GpuEnd::Lost => host.context_lost(),
        });
    }
}

impl Layer for CustomLayer {
    type Style = Custom;
    type Input = NoTiles;
    type Tile = ();

    fn new(cx: &mut NewCx<'_, Self>) -> Self {
        CustomLayer { host: cx.layer.style.data.get(), used: false, after_gpu: cx.after_gpu.clone() }
    }

    /// A new host for the layer is a new layer to the host: the old one
    /// ends. The layer always draws.
    fn evaluate(&mut self, cx: EvaluateCx<'_, Self>) -> Evaluation {
        let data = &cx.layer.style.data;
        if !self.host.as_ref().is_some_and(|host| data.is(host)) {
            self.end();
            self.host = data.get();
        }
        Evaluation::TRANSLUCENT
    }

    fn prepare(&self, _cx: &PrepareCx<'_, Self>, out: &mut Draws) {
        out.lend(LendAt::BeforePasses).lend(LendAt::Pass);
    }

    fn lend(&mut self, at: LendAt, lent: Lent<'_>) {
        let Some(host) = &self.host else { return };
        let frame = lent.frame;
        let frame = CustomLayerFrame {
            view: frame.camera.map_view(f64::from(frame.pixel_ratio)),
            projection: *lent.projection,
            width: frame.target_size.width,
            height: frame.target_size.height,
        };
        match (at, lent.layer_depth) {
            (LendAt::BeforePasses, _) => {
                host.prerender(&mut CustomLayerPrerenderParameters { encoder: lent.raw, frame: &frame });
            }
            (LendAt::Pass, Some(layer_depth)) => {
                host.render(&mut CustomLayerRenderParameters { pass: lent.raw, frame: &frame, layer_depth });
            }
            (LendAt::Pass, None) => return,
        }
        self.used = true;
    }
}

impl Drop for CustomLayer {
    fn drop(&mut self) {
        self.end();
    }
}
