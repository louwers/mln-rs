// Ports src/mln/style/layers/circle_layer_impl.hpp +
// src/mln/style/layers/circle_layer_impl.cpp. `staticTypeInfo()` and
// `stringifyLayout` are generated in C++ (layer.cpp.ejs); the first is in
// circle_layer.rs.

use mln_vec_map::VecSet;
use serde::{Deserialize, Serialize};

use crate::expression::Dependency;
use crate::layer::{self, ImplBase, LayerTypeInfo};
use crate::layers::circle_layer_properties::{CircleLayoutProperties, CirclePaintProperties};

/// `CircleLayer::Impl`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Impl {
    pub base: ImplBase,
    pub layout: CircleLayoutProperties::Unevaluated,
    pub paint: CirclePaintProperties::Transitionable,
}

impl Impl {
    /// `using Layer::Impl::Impl;` (`Impl(std::string layerID, std::string sourceID)`).
    #[must_use]
    pub fn new(layer_id: String, source_id: String) -> Self {
        Impl {
            base: ImplBase::new(layer_id, source_id),
            layout: CircleLayoutProperties::Unevaluated::default(),
            paint: CirclePaintProperties::Transitionable::default(),
        }
    }
}

/// `CircleLayer::Impl : public Layer::Impl`: the base members read as
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

    fn has_layout_difference(&self, other: &dyn layer::Impl) -> bool {
        debug_assert!(std::ptr::eq(other.get_type_info(), layer::Impl::get_type_info(self)));
        let impl_ = other.downcast_ref::<Impl>().expect("CircleLayer::Impl::hasLayoutDifference: not a circle layer");
        self.filter != impl_.filter
            || self.visibility != impl_.visibility
            || self.layout != impl_.layout
            || self.paint.has_data_driven_property_difference(&impl_.paint)
    }

    fn stringify_layout(&self, _writer: &mut String) {
        // Generated in C++ (layer.cpp.ejs): `layout.stringify(writer)`.
        // TODO(port): conversion/stringify.hpp; nothing in the C++ core
        // calls stringifyLayout.
    }

    fn get_type_info(&self) -> &'static LayerTypeInfo {
        Impl::static_type_info()
    }

    fn get_layout_dependencies(&self) -> Dependency {
        self.layout.get_dependencies() | self.base.get_filter_dependencies()
    }

    fn collect_layout_global_state_refs(&self, refs: &mut VecSet<String>) {
        self.layout.collect_global_state_refs(refs);
        self.base.collect_filter_global_state_refs(refs);
    }
}
