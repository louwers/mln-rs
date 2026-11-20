// Differences from C++:
// - The host's callbacks get the backend's live commands as an `Any` of the
//   backend crate's type, where C++ has a parameters subclass per backend.
//   Each frame's view and projection come with them, so the host never reads
//   a camera newer than the frame.
// - There is no `initialize`: the first prerender or render after the host
//   starts, or after a `deinitialize` or `context_lost`, is for a new session.
// - The style's layer holds the host, and its impl only refers to it, so a
//   renderer that is leaked with its impls, because its graphics objects are
//   gone, doesn't keep the host.

use std::any::Any;
use std::sync::{Arc, Weak};

use mln_map_types::{FrameProjection, MapView};

use crate::layer_type::{LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use crate::property_table::{PropertyTable, PropertyVisitor};

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
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "custom", source: SourceKind::None, from_json: false };
    const PROPERTIES: &'static PropertyTable = &PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = NoProperties;
    type Extra = CustomHost;

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(_slot: usize, _visitor: V) -> Option<V::Output> {
        None
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

impl LayerStyle<Custom> {
    /// A layer named `layer_id` that `host` draws.
    #[must_use]
    pub fn with_host(layer_id: String, host: &Arc<dyn CustomLayerHost>) -> Self {
        let mut style = LayerStyle::new(layer_id, String::new());
        style.extra = CustomHost(Arc::downgrade(host));
        style
    }

    /// The layer's host, unless the style no longer has the layer and no
    /// renderer draws it.
    #[must_use]
    pub fn host(&self) -> Option<Arc<dyn CustomLayerHost>> {
        self.extra.0.upgrade()
    }

    /// Whether `host` is the layer's.
    #[must_use]
    pub fn has_host(&self, host: &Arc<dyn CustomLayerHost>) -> bool {
        std::ptr::addr_eq(self.extra.0.as_ptr(), Arc::as_ptr(host))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{Custom, CustomLayerHost};
    use crate::StyleRegistry;
    use crate::layer::Layer;
    use crate::layer_type::LayerStyle;

    struct Host;

    impl CustomLayerHost for Host {}

    /// A renderer that is leaked with the style's impls, because its graphics
    /// objects are gone, must not keep the host, whose user data the C API
    /// releases once the host goes.
    #[test]
    fn the_impls_of_a_removed_layer_dont_keep_its_host() {
        let host: Arc<dyn CustomLayerHost> = Arc::new(Host);
        let layer = Layer::custom(&StyleRegistry::with_builtins(), "custom", Arc::clone(&host));
        let impl_ = LayerStyle::<Custom>::recover(layer.style()).unwrap();
        assert!(impl_.has_host(&host));
        assert!(impl_.host().is_some());
        drop(layer);
        assert!(impl_.host().is_some(), "the caller still holds the host");
        drop(host);
        assert!(impl_.host().is_none());
    }
}
