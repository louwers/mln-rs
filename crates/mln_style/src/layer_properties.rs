// Ports include/mln/style/layer_properties.hpp.
//
// C++ `LayerProperties` is an abstract class with data members; as with
// `Layer::Impl`, the data is `LayerPropertiesBase` and the virtual interface
// is the `LayerProperties` trait. `dyn LayerProperties` derefs to its base.

use std::collections::BTreeSet;

use mln_util::immutable::Immutable;
use serde::{Deserialize, Serialize};

use crate::expression::Dependency;
use crate::layer::Impl;
use crate::util::AsAny;

/// The data members of `LayerProperties`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LayerPropertiesBase {
    #[serde(with = "crate::serialization::shared_layer")]
    pub base_impl: Immutable<dyn Impl>,
    /// Contains render passes used by the renderer, see `mln::RenderPass`.
    pub render_passes: u8,
}

impl LayerPropertiesBase {
    pub fn new(impl_: Immutable<dyn Impl>) -> Self {
        LayerPropertiesBase { base_impl: impl_, render_passes: 0 }
    }
}

/// An interface, wrapping evaluated layer properties.
pub trait LayerProperties: AsAny {
    fn base(&self) -> &LayerPropertiesBase;
    fn base_mut(&mut self) -> &mut LayerPropertiesBase;

    /// Returns constants mask for the data-driven properties.
    fn constants_mask(&self) -> u64 {
        0
    }

    fn get_dependencies(&self) -> Dependency;

    /// Returns the combined dependencies of the expressions retained in the
    /// evaluated properties (i.e. those that still require per-feature
    /// evaluation, such as data-driven paint properties).
    fn get_evaluated_dependencies(&self) -> Dependency {
        Dependency::NONE
    }

    /// Collect the global-state properties referenced by the expressions
    /// retained in the evaluated properties.
    fn collect_evaluated_global_state_refs(&self, _refs: &mut BTreeSet<String>) {}
}

impl std::ops::Deref for dyn LayerProperties {
    type Target = LayerPropertiesBase;
    fn deref(&self) -> &LayerPropertiesBase {
        self.base()
    }
}

/// `static_cast<const Derived&>(*properties)`, checked. Downcasts the
/// pointee: `properties.as_any()` would pick the blanket `AsAny` impl of the
/// `Arc` itself (an `Arc<dyn LayerProperties>` is `Any` too) and never match.
fn downcast_properties<'a, D: LayerProperties>(properties: &'a Immutable<dyn LayerProperties>, what: &str) -> &'a D {
    let pointee: &dyn LayerProperties = &**properties;
    match pointee.as_any().downcast_ref::<D>() {
        Some(derived) => derived,
        None => panic!("{what}: layer properties type mismatch"),
    }
}

/// `getEvaluated<Derived>(properties)`. C++ static_casts; this panics on a
/// type mismatch.
pub fn get_evaluated<D: HasEvaluated>(properties: &Immutable<dyn LayerProperties>) -> &D::Evaluated {
    downcast_properties::<D>(properties, "get_evaluated").evaluated()
}

/// `getCrossfade<Derived>(properties)`.
pub fn get_crossfade<D: HasCrossfade>(
    properties: &Immutable<dyn LayerProperties>,
) -> &crate::renderer::property_evaluation_parameters::CrossfadeParameters {
    downcast_properties::<D>(properties, "get_crossfade").crossfade()
}

/// The `evaluated` member `getEvaluated` reads (implemented by the generated
/// `FooLayerProperties`).
pub trait HasEvaluated: LayerProperties {
    type Evaluated;
    fn evaluated(&self) -> &Self::Evaluated;
}

/// The `crossfade` member `getCrossfade` reads: only the layer types whose
/// properties hold one (background, fill, line, fill-extrusion).
pub trait HasCrossfade: LayerProperties {
    fn crossfade(&self) -> &crate::renderer::property_evaluation_parameters::CrossfadeParameters;
}

// Debug is not a supertrait, for the reason at `impl Debug for dyn Layer`
// (layer.rs).
impl std::fmt::Debug for dyn LayerProperties {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let base_impl = &self.base().base_impl;
        f.debug_struct("LayerProperties")
            .field("id", &base_impl.base().id)
            .field("type", &base_impl.get_type_info().type_)
            .finish_non_exhaustive()
    }
}
