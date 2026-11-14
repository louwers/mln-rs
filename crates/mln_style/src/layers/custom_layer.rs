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

use feature::Value;
use mln_conversion::{Convertible, Error};
use mln_map_types::{FrameProjection, MapView};

use crate::expression::Dependency;
use crate::layer::{self, ImplBase, Layer, LayerTypeInfo, layer_type_info, serialize_base};
use crate::layer_properties::{LayerProperties, LayerPropertiesBase};
use crate::style_property::StyleProperty;

static TYPE_INFO: LayerTypeInfo = LayerTypeInfo {
    type_: "custom",
    source: layer_type_info::Source::NotRequired,
    pass3d: layer_type_info::Pass3D::NotRequired,
    layout: layer_type_info::Layout::NotRequired,
    fading_tiles: layer_type_info::FadingTiles::NotRequired,
    cross_tile_index: layer_type_info::CrossTileIndex::NotRequired,
    tile_kind: layer_type_info::TileKind::NotRequired,
};

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

#[derive(Clone)]
pub struct Impl {
    pub base: ImplBase,
    /// The host, while the style has the layer or a renderer draws it.
    host: Weak<dyn CustomLayerHost>,
}

impl std::fmt::Debug for Impl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomLayer::Impl").field("base", &self.base).finish_non_exhaustive()
    }
}

impl Impl {
    #[must_use]
    pub fn new(layer_id: String, host: &Arc<dyn CustomLayerHost>) -> Self {
        Impl { base: ImplBase::new(layer_id, String::new()), host: Arc::downgrade(host) }
    }

    /// The layer's host, unless the style no longer has the layer and no
    /// renderer draws it.
    #[must_use]
    pub fn host(&self) -> Option<Arc<dyn CustomLayerHost>> {
        self.host.upgrade()
    }

    /// Whether `host` is the layer's.
    #[must_use]
    pub fn has_host(&self, host: &Arc<dyn CustomLayerHost>) -> bool {
        std::ptr::addr_eq(self.host.as_ptr(), Arc::as_ptr(host))
    }

    #[must_use]
    pub fn static_type_info() -> &'static LayerTypeInfo {
        &TYPE_INFO
    }
}

impl layer::Impl for Impl {
    fn base(&self) -> &ImplBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut ImplBase {
        &mut self.base
    }

    fn has_layout_difference(&self, _other: &dyn layer::Impl) -> bool {
        false
    }

    fn get_type_info(&self) -> &'static LayerTypeInfo {
        &TYPE_INFO
    }
}

/// A layer that the host draws.
pub struct CustomLayer {
    base_impl: Arc<Impl>,
    host: Arc<dyn CustomLayerHost>,
}

impl std::fmt::Debug for CustomLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomLayer").field("base_impl", &self.base_impl).finish_non_exhaustive()
    }
}

impl CustomLayer {
    #[must_use]
    pub fn new(layer_id: &str, host: Arc<dyn CustomLayerHost>) -> Self {
        CustomLayer { base_impl: Arc::new(Impl::new(layer_id.to_owned(), &host)), host }
    }

    #[must_use]
    pub fn impl_(&self) -> &Impl {
        &self.base_impl
    }
}

impl Layer for CustomLayer {
    fn base_impl(&self) -> Arc<dyn layer::Impl> {
        Arc::<Impl>::clone(&self.base_impl)
    }

    fn base_impl_ref(&self) -> &dyn layer::Impl {
        &*self.base_impl
    }

    fn mutate_base_impl(&mut self, f: &mut dyn FnMut(&mut ImplBase)) {
        let mut impl_ = (*self.base_impl).clone();
        f(&mut impl_.base);
        self.base_impl = Arc::new(impl_);
    }

    fn get_property(&self, _name: &str) -> StyleProperty {
        StyleProperty::default()
    }

    fn serialize(&self) -> Value {
        serialize_base(self)
    }

    /// The host draws one layer, so a copy shares it: both draw through it.
    fn clone_ref(&self, id: &str) -> Box<dyn Layer> {
        let mut impl_ = (*self.base_impl).clone();
        impl_.base.id = id.to_owned();
        Box::new(CustomLayer { base_impl: Arc::new(impl_), host: Arc::clone(&self.host) })
    }

    fn set_property_internal(&mut self, name: &str, _value: &Convertible<'_>) -> Option<Error> {
        Some(Error::new(format!("layer '{}' doesn't support property '{name}'", self.get_id())))
    }
}

#[derive(Debug)]
pub struct CustomLayerProperties {
    pub base: LayerPropertiesBase,
}

impl CustomLayerProperties {
    #[must_use]
    pub fn new(impl_: Arc<Impl>) -> Self {
        CustomLayerProperties { base: LayerPropertiesBase::new(impl_) }
    }
}

impl LayerProperties for CustomLayerProperties {
    fn base(&self) -> &LayerPropertiesBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut LayerPropertiesBase {
        &mut self.base
    }

    fn get_dependencies(&self) -> Dependency {
        Dependency::NONE
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{CustomLayer, CustomLayerHost, Impl};
    use crate::layer::Layer;

    struct Host;

    impl CustomLayerHost for Host {}

    /// A renderer that is leaked with the style's impls, because its graphics
    /// objects are gone, must not keep the host, whose user data the C API
    /// releases once the host goes.
    #[test]
    fn the_impls_of_a_removed_layer_dont_keep_its_host() {
        let host: Arc<dyn CustomLayerHost> = Arc::new(Host);
        let layer = CustomLayer::new("custom", Arc::clone(&host));
        let impl_ = layer.base_impl();
        let impl_ = impl_.downcast_ref::<Impl>().unwrap();
        assert!(impl_.has_host(&host));
        assert!(impl_.host().is_some());
        drop(layer);
        assert!(impl_.host().is_some(), "the caller still holds the host");
        drop(host);
        assert!(impl_.host().is_none());
    }
}
