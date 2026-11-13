// C++ `LayerProperties` is an abstract class with data members; as with
// `Layer::Impl`, the data is `LayerPropertiesBase` and the virtual interface
// is the `LayerProperties` trait. `dyn LayerProperties` derefs to its base.

use std::any::Any;
use std::sync::Arc;

use mln_vec_map::VecSet;
use serde::{Deserialize, Serialize};

use crate::expression::Dependency;
use crate::layer::Impl;

/// The data members of `LayerProperties`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LayerPropertiesBase {
    #[serde(with = "crate::serialization::shared_layer")]
    pub base_impl: Arc<dyn Impl>,
    /// Contains render passes used by the renderer, see `mln::RenderPass`.
    pub render_passes: u8,
}

impl LayerPropertiesBase {
    pub fn new(impl_: Arc<dyn Impl>) -> Self {
        LayerPropertiesBase { base_impl: impl_, render_passes: 0 }
    }
}

/// An interface, wrapping evaluated layer properties.
pub trait LayerProperties: Any + Send + Sync {
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
    fn collect_evaluated_global_state_refs(&self, _refs: &mut VecSet<String>) {}
}

impl dyn LayerProperties {
    /// The concrete properties, if they are a `D`.
    pub fn downcast_ref<D: LayerProperties>(&self) -> Option<&D> {
        let any: &dyn Any = self;
        any.downcast_ref()
    }
}

impl std::ops::Deref for dyn LayerProperties {
    type Target = LayerPropertiesBase;
    fn deref(&self) -> &LayerPropertiesBase {
        self.base()
    }
}

/// `properties` as a `D`; panics on a type mismatch.
fn downcast_properties<'a, D: LayerProperties>(properties: &'a Arc<dyn LayerProperties>, what: &str) -> &'a D {
    match properties.downcast_ref::<D>() {
        Some(derived) => derived,
        None => panic!("{what}: layer properties type mismatch"),
    }
}

/// The evaluated properties of `properties` as a `D`. Panics on a type
/// mismatch.
pub fn get_evaluated<D: HasEvaluated>(properties: &Arc<dyn LayerProperties>) -> &D::Evaluated {
    downcast_properties::<D>(properties, "get_evaluated").evaluated()
}

pub fn get_crossfade<D: HasCrossfade>(
    properties: &Arc<dyn LayerProperties>,
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
