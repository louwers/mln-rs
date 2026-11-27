//! The `custom` layer type, whose layers a host draws with its own GPU
//! commands: its style half ([`Custom`]) and its render half.

// Differences from C++:
// - The host's callbacks get the backend's live commands as an `Any` of the
//   backend crate's type, where C++ has a parameters subclass per backend.
//   Each frame's view and projection come with them, so the host never reads
//   a camera newer than the frame.
// - There is no `initialize`: the first prerender or render after the host
//   starts, or after a `deinitialize` or `context_lost`, is for a new session.
// - The style's layer holds the host, and its style data only refers to it,
//   so a renderer that is leaked with its style data, because its graphics
//   objects are gone, doesn't keep the host.
//
// How a custom layer draws differently from the others:
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

use std::any::Any;
use std::sync::{Arc, Weak};

use mln_gfx::gpu::Gpu;
use mln_style::StyleRegistry;
use mln_style::layer::{Layer, LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use mln_style::property::table::{PropertyTable, PropertyVisitor};

use crate::camera::{FrameProjection, MapView};
use crate::draw::context::Context;
use crate::layer::layer_drawing::{DrawGroup, DrawOrder, LayerDrawing, TileMap};
use crate::layer::layer_type::LayerType;
use crate::layer::render_layer::{Hosted, LayerCx, LayerValues, RenderLayer, TypedLayer};
use crate::layer::tile_input::NoTiles;
use crate::log_target;
use crate::render::paint_parameters::PaintParameters;
use crate::render::pass::RenderPass;

/// The frame that a custom layer draws into.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CustomLayerFrame {
    /// The camera, extent and scale factor that the frame shows.
    pub view: MapView,
    /// Where the frame draws the map, and the depth it writes there.
    pub projection: FrameProjection,
    /// The size of the frame's attachments, in physical pixels.
    pub width: u32,
    pub height: u32,
}

/// What [`CustomLayerHost::prerender`] gets.
pub struct CustomLayerPrerenderParameters<'a> {
    /// The frame's commands outside the map's render pass, as the backend's
    /// type: `mln_vulkan::CustomLayerEncoder`, `mln_mtl::CustomLayerEncoder`,
    /// `mln_gl::CustomLayerEncoder` or `mln_webgpu::CustomLayerEncoder`.
    pub encoder: &'a mut dyn Any,
    pub frame: &'a CustomLayerFrame,
}

/// What [`CustomLayerHost::render`] gets.
pub struct CustomLayerRenderParameters<'a> {
    /// The map's render pass, as the backend's type:
    /// `mln_vulkan::CustomLayerPass`, `mln_mtl::CustomLayerPass`,
    /// `mln_gl::CustomLayerPass` or `mln_webgpu::CustomLayerPass`, whose
    /// docs say what the host may change.
    pub pass: &'a mut dyn Any,
    pub frame: &'a CustomLayerFrame,
    /// The depth that the map's 2D layers write at the layer's place in the
    /// style. The map draws its opaque 2D layers before `render`, those above
    /// the layer too, and those wrote less depth.
    pub layer_depth: f64,
}

/// Draws a custom layer with the host's own GPU commands, on the thread that
/// renders the map.
///
/// Each frame that shows the layer calls `prerender` for every custom layer,
/// bottom to top, before the map's render pass begins, then `render` at the
/// layer's place in the pass. Once a renderer stops drawing the layer, it
/// calls `deinitialize`, or `context_lost` when the renderer's graphics
/// objects may be gone. The next `prerender` or `render` is for a new
/// renderer, which can be on another device or backend.
pub trait CustomLayerHost: Send + Sync {
    fn prerender(&self, _parameters: &mut CustomLayerPrerenderParameters<'_>) {}

    fn render(&self, _parameters: &mut CustomLayerRenderParameters<'_>) {}

    /// Destroy the GPU objects made for the renderer that stopped: the GPU
    /// no longer uses them.
    fn deinitialize(&self) {}

    /// Forget the GPU objects made for the renderer that stopped, without
    /// destroying them.
    fn context_lost(&self) {}
}

/// The `custom` layer type, whose layers a [`CustomLayerHost`] draws.
pub struct Custom;

static PROPERTIES: PropertyTable = PropertyTable::new(&[], &[]);

impl StyleLayerType for Custom {
    const INFO: LayerTypeInfo = LayerTypeInfo {
        name: "custom",
        source: SourceKind::None,
        fading_tiles: false,
        cross_tile_index: false,
        from_json: false,
    };
    const PROPERTIES: &'static PropertyTable = &PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = NoProperties;
    type Extra = CustomHost;

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(_slot: usize, _visitor: V) -> Option<V::Output> {
        None
    }
}

impl Custom {
    /// A layer named `id` that `host` draws, whose filter and visibility
    /// parse against `registry`'s functions. The layer holds `host`, and
    /// its style data only refers to it.
    #[must_use]
    pub fn layer(registry: &StyleRegistry, id: &str, host: Arc<dyn CustomLayerHost>) -> Layer {
        let mut style = LayerStyle::<Custom>::new(id.to_owned(), String::new());
        style.extra = CustomHost(Arc::downgrade(&host));
        Layer::from_style_holding(Arc::new(style), Arc::clone(registry.functions()), Arc::new(host))
    }
}

/// The host of a custom layer, as its style data refers to it: there while
/// the style has the layer or a renderer draws it.
#[derive(Clone)]
pub struct CustomHost(Weak<dyn CustomLayerHost>);

impl Default for CustomHost {
    /// No host.
    fn default() -> Self {
        struct NoHost;
        impl CustomLayerHost for NoHost {}
        CustomHost(Weak::<NoHost>::new())
    }
}

impl CustomHost {
    /// The host, unless the style no longer has the layer and no renderer
    /// draws it.
    #[must_use]
    pub fn get(&self) -> Option<Arc<dyn CustomLayerHost>> {
        self.0.upgrade()
    }

    /// Whether `host` is this one.
    #[must_use]
    pub fn is(&self, host: &Arc<dyn CustomLayerHost>) -> bool {
        std::ptr::addr_eq(self.0.as_ptr(), Arc::as_ptr(host))
    }
}

impl LayerType for Custom {
    type Input = NoTiles;

    fn render_layer(style: Arc<LayerStyle<Custom>>) -> Box<dyn RenderLayer> {
        Hosted::<RenderCustomLayer>::boxed(style)
    }
}

pub(crate) struct RenderCustomLayer {
    /// The host of the layer's style when the layer was made, until a hook
    /// gives it to the drawing ([`drawing`](Self::drawing)).
    host: Option<Arc<dyn CustomLayerHost>>,
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
    /// `drawing`, the layer's, once it holds the host the layer was made
    /// with, unless an earlier hook gave it that host.
    fn drawing<'a>(&mut self, drawing: &'a mut CustomDrawing) -> &'a mut CustomDrawing {
        if let Some(host) = self.host.take() {
            drawing.host = Some(host);
        }
        drawing
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
        log::error!(target: log_target::RENDER, "Waiting for a custom layer's frames failed: {error}");
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

impl LayerDrawing<()> for CustomDrawing {
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

    fn upload(&mut self, _tiles: &mut TileMap<()>, _gpu: &mut dyn Gpu, _context: &mut Context) {}

    fn render(
        &mut self,
        _tiles: &mut TileMap<()>,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        order: DrawOrder,
    ) {
        if self.updated {
            self.lend_pass(gpu, parameters);
            order.step(parameters);
        }
    }
}

impl TypedLayer for RenderCustomLayer {
    type Type = Custom;
    type Tile = ();
    type Drawing = CustomDrawing;

    fn new(values: &LayerValues<Custom>) -> Self {
        RenderCustomLayer { host: values.style.extra.get() }
    }

    /// A new host for the same layer ID is a new layer to the host: the old
    /// one goes to `ended`, to hear that the renderer stopped.
    fn style_changed(&mut self, cx: LayerCx<'_, Self>, ended: &mut EndedHosts) {
        let drawing = self.drawing(cx.drawing);
        let same = drawing.host.as_ref().is_some_and(|host| cx.values.style.extra.is(host));
        if !same {
            ended.extend(drawing.take_host());
            drawing.host = cx.values.style.extra.get();
        }
    }

    fn prerender(&mut self, cx: LayerCx<'_, Self>, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        let drawing = self.drawing(cx.drawing);
        let Some(frame) = parameters.custom_layer_frame else {
            return;
        };
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

    fn leave(&mut self, cx: LayerCx<'_, Self>, ended: &mut EndedHosts) {
        ended.extend(self.drawing(cx.drawing).take_host());
    }

    fn lose_context(&mut self, cx: LayerCx<'_, Self>) {
        self.drawing(cx.drawing).lose();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mln_style::layer::LayerStyle;

    use super::{Custom, CustomLayerHost};

    struct Host;

    impl CustomLayerHost for Host {}

    /// A renderer that is leaked with the style data, because its graphics
    /// objects are gone, must not keep the host, whose user data the C API
    /// releases once the host goes.
    #[test]
    fn the_style_data_of_a_removed_layer_doesnt_keep_its_host() {
        let host: Arc<dyn CustomLayerHost> = Arc::new(Host);
        let layer = Custom::layer(crate::tests::registry().style(), "custom", Arc::clone(&host));
        let style = LayerStyle::<Custom>::recover(layer.style()).unwrap();
        assert!(style.extra.is(&host));
        assert!(style.extra.get().is_some());
        drop(layer);
        assert!(style.extra.get().is_some(), "the caller still holds the host");
        drop(host);
        assert!(style.extra.get().is_none());
    }
}
