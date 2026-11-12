// Ports src/mln/style/layers/raster_layer_impl.hpp +
// src/mln/style/layers/raster_layer_impl.cpp. `staticTypeInfo()` and
// `stringifyLayout` are generated in C++ (layer.cpp.ejs); the first is in
// raster_layer.rs.

use serde::{Deserialize, Serialize};

use crate::layer::{self, ImplBase, LayerTypeInfo};
use crate::layers::raster_layer_properties::RasterPaintProperties;

/// `RasterLayer::Impl`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Impl {
    pub base: ImplBase,
    pub paint: RasterPaintProperties::Transitionable,
}

impl Impl {
    /// `using Layer::Impl::Impl;` (`Impl(std::string layerID, std::string sourceID)`).
    #[must_use]
    pub fn new(layer_id: String, source_id: String) -> Self {
        Impl { base: ImplBase::new(layer_id, source_id), paint: RasterPaintProperties::Transitionable::default() }
    }
}

/// `RasterLayer::Impl : public Layer::Impl`: the base members read as
/// `impl_.id`, `impl_.visibility`, ... as in C++.
impl std::ops::Deref for Impl {
    type Target = ImplBase;
    fn deref(&self) -> &ImplBase {
        &self.base
    }
}

impl std::ops::DerefMut for Impl {
    fn deref_mut(&mut self) -> &mut ImplBase {
        &mut self.base
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

    fn stringify_layout(&self, _writer: &mut String) {}

    fn get_type_info(&self) -> &'static LayerTypeInfo {
        Impl::static_type_info()
    }
}
