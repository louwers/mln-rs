//! The `custom` layer type, whose layers a host outside the renderer draws
//! with its own GPU commands, and the host's API ([`CustomLayerHost`]).
//!
//! - The host's callbacks get the backend's live commands as an `Any` of the
//!   backend crate's type. Each frame's view and projection come with them,
//!   so the host never reads a camera newer than the frame.
//! - There is no `initialize`: the first prerender or render after the host
//!   starts, or after a `deinitialize` or `context_lost`, is for a new
//!   session.
//! - The style's layer keeps the host, and its style data only refers to
//!   it, so a renderer that is leaked with copies of the style data, because
//!   its graphics objects are gone, doesn't keep the host.

pub(crate) mod render_layer;

use std::any::Any;
use std::sync::{Arc, Weak};

use mln_map_types::{FrameProjection, MapView};
use mln_style::layer::Layer;
use mln_style::layer_type::{LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use mln_style::property_table::{PropertyTable, PropertyVisitor};
use mln_style::registry::StyleRegistry;

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

/// The `custom` layer type, whose layers a [`CustomLayerHost`] draws. Its
/// layers aren't made from style JSON: [`Custom::layer`] makes one.
pub struct Custom;

static PROPERTIES: PropertyTable = PropertyTable::new(&[], &[]);

impl StyleLayerType for Custom {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "custom", source: SourceKind::None, from_json: false };
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
    /// parse against `registry`'s functions. The layer, and each copy of it,
    /// keeps the host.
    #[must_use]
    pub fn layer(registry: &StyleRegistry, id: &str, host: Arc<dyn CustomLayerHost>) -> Layer {
        let mut style = LayerStyle::<Custom>::new(id.to_owned(), String::new());
        style.extra = CustomHost(Arc::downgrade(&host));
        Layer::from_style_keeping(Arc::new(style), Arc::clone(registry.functions()), Arc::new(host))
    }
}

/// The host of a custom layer, while the style has the layer or a renderer
/// draws it.
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
