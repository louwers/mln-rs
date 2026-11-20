//! The registrations that a runtime, its renderers and its tile workers
//! share: the expression functions, the layer types and their programs'
//! shader sources, and the source types.
//!
//! An app fills a [`RegistryBuilder`] by explicit calls and builds the
//! [`Registry`] once. Tile workers in other wasm instances make the same
//! calls, since they run the app's own module.

use std::sync::Arc;

use mln_gfx::shaders::source::ShaderSources;
use mln_style::StyleRegistry;
use mln_style::expression::Functions;
use mln_style::expression::functions::RegisterError;
use mln_style::layer_type::ErasedStyleLayerType;
use mln_style::source_type::ErasedStyleSourceType;
use mln_vec_map::VecMap;

use crate::erased_layer::{ErasedLayerType, entry};
use crate::layer::LayerType;
use crate::render_tile::FindView;
use crate::source_type::{self, ErasedSourceType, SourceType};

/// What a [`Registry`] is built from.
#[derive(Default)]
pub struct RegistryBuilder {
    functions: Functions,
    layer_types: VecMap<&'static str, &'static dyn ErasedLayerType>,
    shader_sources: ShaderSources,
    source_types: VecMap<&'static str, &'static dyn ErasedSourceType>,
}

impl std::fmt::Debug for RegistryBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistryBuilder")
            .field("functions", &self.functions)
            .field("layer_types", &self.layer_types.keys().collect::<Vec<_>>())
            .field("shader_sources", &self.shader_sources)
            .field("source_types", &self.source_types.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl RegistryBuilder {
    #[must_use]
    pub fn new() -> Self {
        RegistryBuilder::default()
    }

    pub fn functions_mut(&mut self) -> &mut Functions {
        &mut self.functions
    }

    /// Adds the layer type `T`, with the sources of its programs
    /// ([`LayerType::add_shader_sources`]). Fails for a name that an earlier
    /// type took. A type whose [`View`](LayerType::View) is of another kind
    /// of source than its style half's doesn't compile.
    pub fn add_layer_type<T: LayerType>(&mut self) -> Result<(), RegisterError> {
        const {
            assert!(
                T::INFO.source as u8 == <T::View<'static> as FindView<'static>>::SOURCE as u8,
                "a layer type's tile view is of another kind of source than its style half's",
            );
        }
        let name = T::INFO.name;
        if self.layer_types.contains_key(name) {
            return Err(RegisterError::Duplicate(name.to_owned()));
        }
        self.layer_types.insert(name, entry::<T>());
        T::add_shader_sources(&mut self.shader_sources);
        Ok(())
    }

    /// Adds the source type `T`. Fails for a name that an earlier type took.
    pub(crate) fn add_source_type<T: SourceType>(&mut self) -> Result<(), RegisterError> {
        let name = T::INFO.name;
        if self.source_types.contains_key(name) {
            return Err(RegisterError::Duplicate(name.to_owned()));
        }
        self.source_types.insert(name, source_type::entry::<T>());
        Ok(())
    }

    #[must_use]
    pub fn build(self) -> Arc<Registry> {
        Registry::new(Arc::new(self.functions), self.layer_types, self.shader_sources, self.source_types)
    }
}

fn style_half(layer_type: &'static dyn ErasedLayerType) -> &'static dyn ErasedStyleLayerType {
    layer_type
}

fn style_source_half(source_type: &'static dyn ErasedSourceType) -> &'static dyn ErasedStyleSourceType {
    source_type
}

/// The expression functions, layer types and source types that a style
/// parses against, its renderer draws and its tile workers lay out, built by
/// a [`RegistryBuilder`]. It doesn't change once built.
///
/// A map and the renderer and tile workers that draw it must be given
/// registries filled by the same calls: they identify functions, layer
/// types and source types by name.
pub struct Registry {
    style: Arc<StyleRegistry>,
    layer_types: VecMap<&'static str, &'static dyn ErasedLayerType>,
    shader_sources: ShaderSources,
    source_types: VecMap<&'static str, &'static dyn ErasedSourceType>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry").field("style", &self.style).finish_non_exhaustive()
    }
}

impl Registry {
    /// The built-in expression functions, the engine's own layer and source
    /// types, without those of `mln_layers` and `mln_sources`, and the tests'
    /// `dot` layer type, for this crate's tests, which can't reach the
    /// facade's registry.
    #[cfg(test)]
    pub(crate) fn with_builtins() -> Arc<Registry> {
        static BUILTINS: std::sync::LazyLock<Arc<Registry>> = std::sync::LazyLock::new(|| {
            let mut builder = RegistryBuilder::new();
            mln_style::expression::functions::register_builtin_functions(builder.functions_mut()).unwrap();
            crate::builtins::register_layer_types(&mut builder).unwrap();
            builder.add_layer_type::<crate::tests::fixture_layer::Dot>().unwrap();
            builder.add_layer_type::<crate::tests::fixture_layer::Tint>().unwrap();
            builder.add_layer_type::<crate::tests::fixture_layer::Shade>().unwrap();
            crate::builtins::register_source_types(&mut builder).unwrap();
            builder.build()
        });
        Arc::clone(&BUILTINS)
    }

    fn new(
        functions: Arc<Functions>,
        layer_types: VecMap<&'static str, &'static dyn ErasedLayerType>,
        shader_sources: ShaderSources,
        source_types: VecMap<&'static str, &'static dyn ErasedSourceType>,
    ) -> Arc<Registry> {
        let style_halves = layer_types.values().map(|&layer_type| style_half(layer_type));
        let style_source_halves = source_types.values().map(|&source_type| style_source_half(source_type));
        let style = StyleRegistry::new(functions, style_halves, style_source_halves);
        Arc::new(Registry { style: Arc::new(style), layer_types, shader_sources, source_types })
    }

    #[must_use]
    pub fn style(&self) -> &Arc<StyleRegistry> {
        &self.style
    }

    #[must_use]
    pub fn functions(&self) -> &Arc<Functions> {
        self.style.functions()
    }

    /// The sources of the programs of the layer types, which a renderer's
    /// backend compiles them from.
    #[must_use]
    pub fn shader_sources(&self) -> &ShaderSources {
        &self.shader_sources
    }

    pub(crate) fn layer_type(&self, name: &str) -> Option<&'static dyn ErasedLayerType> {
        self.layer_types.get(name).copied()
    }

    pub(crate) fn source_type(&self, name: &str) -> Option<&'static dyn ErasedSourceType> {
        self.source_types.get(name).copied()
    }
}

/// A context in which the values that the worker codec reads resolve the
/// registrations they name against this registry: layers' evaluated values
/// their layer type, and as for a [`StyleRegistry`], layers' style data and
/// compound expressions.
impl mln_wire::Context for &Registry {
    fn provide<'a>(&'a mut self, request: &mut mln_wire::Request<'a>) {
        let registry: &'a Registry = self;
        request.give(registry).give::<StyleRegistry>(registry.style()).give::<Functions>(registry.functions());
    }
}

#[cfg(test)]
mod tests {
    use mln_style::sources::GeoJsonSourceType;

    use super::*;
    use crate::tests::fixture_layer::Tint;

    #[test]
    fn a_source_type_registers_once() {
        let mut builder = RegistryBuilder::new();
        builder.add_source_type::<GeoJsonSourceType>().unwrap();
        assert_eq!(builder.add_source_type::<GeoJsonSourceType>(), Err(RegisterError::Duplicate("geojson".to_owned())));
    }

    #[test]
    fn a_layer_type_registers_once() {
        let mut builder = RegistryBuilder::new();
        builder.add_layer_type::<Tint>().unwrap();
        assert_eq!(builder.add_layer_type::<Tint>(), Err(RegisterError::Duplicate("tint".to_owned())));
    }
}
