use serde::{Deserialize, Serialize};

use crate::layer::{self, ImplBase, LayerTypeInfo};
use crate::layers::raster_layer_properties::RasterPaintProperties;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Impl {
    pub base: ImplBase,
    pub paint: RasterPaintProperties::Transitionable,
}

impl Impl {
    #[must_use]
    pub fn new(layer_id: String, source_id: String) -> Self {
        Impl { base: ImplBase::new(layer_id, source_id), paint: RasterPaintProperties::Transitionable::default() }
    }
}

/// The base members read as `impl_.id`, `impl_.visibility`, and so on.
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

    fn get_type_info(&self) -> &'static LayerTypeInfo {
        Impl::static_type_info()
    }
}
