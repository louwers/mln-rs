//! The registrations that a style parses against: its expression functions,
//! its layer types and its source types.

use std::sync::Arc;

use mln_vec_map::VecMap;

use crate::conversion::{Convertible, Error};
use crate::expression::Functions;
use crate::layer::Layer;
use crate::layer_type::{ErasedStyleLayerType, SourceKind};
use crate::source_type::ErasedStyleSourceType;

/// The expression functions, layer types and source types that a style's
/// JSON, its layers' property values and its filters parse against.
pub struct StyleRegistry {
    functions: Arc<Functions>,
    layer_types: VecMap<&'static str, &'static dyn ErasedStyleLayerType>,
    source_types: VecMap<&'static str, &'static dyn ErasedStyleSourceType>,
}

impl StyleRegistry {
    /// The registry of `functions`, `layer_types` and `source_types`. A type
    /// is known by its name: of two types of one name, the first counts.
    #[must_use]
    pub fn new(
        functions: Arc<Functions>,
        layer_types: impl IntoIterator<Item = &'static dyn ErasedStyleLayerType>,
        source_types: impl IntoIterator<Item = &'static dyn ErasedStyleSourceType>,
    ) -> Self {
        let mut layers_by_name = VecMap::new();
        for layer_type in layer_types {
            layers_by_name.entry(layer_type.info().name).or_insert(layer_type);
        }
        let mut sources_by_name = VecMap::new();
        for source_type in source_types {
            sources_by_name.entry(source_type.info().name).or_insert(source_type);
        }
        StyleRegistry { functions, layer_types: layers_by_name, source_types: sources_by_name }
    }

    /// The built-in expression functions, and the tests' fixture layer and
    /// source types, for this crate's tests, which can't reach the facade's
    /// registry.
    #[cfg(test)]
    pub(crate) fn with_builtins() -> Arc<Self> {
        use crate::layer_type::style_entry;
        use crate::source_type::style_source_entry;
        use crate::tests::fixture_source::{Heights, Points, Shapes};
        let layer_types = [
            style_entry::<crate::tests::fixture_layer::Dot>(),
            style_entry::<crate::tests::fixture_layer::Plain>(),
            style_entry::<crate::tests::fixture_layer::Shade>(),
        ];
        let source_types =
            [style_source_entry::<Shapes>(), style_source_entry::<Points>(), style_source_entry::<Heights>()];
        Arc::new(StyleRegistry::new(Functions::with_builtins(), layer_types, source_types))
    }

    #[must_use]
    pub fn functions(&self) -> &Arc<Functions> {
        &self.functions
    }

    #[must_use]
    pub fn layer_type(&self, name: &str) -> Option<&'static dyn ErasedStyleLayerType> {
        self.layer_types.get(name).copied()
    }

    /// The layer types, in name order.
    pub fn layer_types(&self) -> impl Iterator<Item = &'static dyn ErasedStyleLayerType> + '_ {
        self.layer_types.values().copied()
    }

    #[must_use]
    pub fn source_type(&self, name: &str) -> Option<&'static dyn ErasedStyleSourceType> {
        self.source_types.get(name).copied()
    }

    /// The source types, in name order.
    pub fn source_types(&self) -> impl Iterator<Item = &'static dyn ErasedStyleSourceType> + '_ {
        self.source_types.values().copied()
    }

    /// A layer of the type `type_` from its style JSON `value`, without its
    /// properties.
    pub(crate) fn create_layer(
        &self,
        type_: &str,
        id: &str,
        value: &Convertible<'_>,
        error: &mut Error,
    ) -> Option<Layer> {
        let Some(layer_type) = self.layer_type(type_) else {
            error.message = format!("Unsupported layer type! Null factory for type: {type_}");
            return None;
        };
        let info = layer_type.info();
        if !info.from_json {
            error.message = format!("layer type '{type_}' can't be created from style JSON");
            return None;
        }
        let source = if info.source == SourceKind::None {
            String::new()
        } else {
            let Some(source) = value.object_member("source").and_then(|source| source.to_string()) else {
                error.message = format!("Error parsing layer {id} of type: {type_}");
                return None;
            };
            source
        };
        Some(Layer::from_style(layer_type.create(id, &source), Arc::clone(&self.functions)))
    }
}

impl std::fmt::Debug for StyleRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StyleRegistry")
            .field("layer_types", &self.layer_types.keys().collect::<Vec<_>>())
            .field("source_types", &self.source_types.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

/// A context in which layers read with [`mln_wire`] resolve their type's
/// name against this registry, and compound expressions their function's
/// against its functions.
impl mln_wire::Context for &StyleRegistry {
    fn provide<'a>(&'a mut self, request: &mut mln_wire::Request<'a>) {
        let registry: &'a StyleRegistry = self;
        request.give(registry).give::<Functions>(registry.functions());
    }
}
