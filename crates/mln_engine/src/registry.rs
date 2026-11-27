//! The registrations that a runtime, its renderers and its tile workers
//! share: the expression functions, the layer types and the source types.
//!
//! An app fills a [`RegistryBuilder`] by explicit calls, [`register_builtins`]
//! first, and builds the [`Registry`] once. Tile workers in other wasm
//! instances make the same calls, since they run the app's own module.

use std::sync::{Arc, LazyLock};

use mln_style::StyleRegistry;
use mln_style::expression::Functions;
use mln_style::expression::functions::{RegisterError, register_builtin_functions};
use mln_style::layer::{ErasedStyleLayerType, StyleLayerType};
use mln_style::layers::{Background, Circle, ColorRelief, Fill, FillExtrusion, Heatmap, Hillshade, Line, Raster};
use mln_style::source::ErasedStyleSourceType;
use mln_style::sources::{GeoJsonSourceType, ImageSourceType, RasterDemSourceType, RasterSourceType, VectorSourceType};
use mln_vec_map::VecMap;

use crate::layer::layer_type::{ErasedLayerType, LayerType, entry};
use crate::layer::tile_input::input::Find;
use crate::layers::custom::Custom;
use crate::placement::{ErasedPlacement, Placement, participant};
use crate::source::erased::{self, ErasedSourceType};
use crate::source::{SourceType, kind};
use crate::symbol::placement::SymbolPlacement;

/// Makes a renderer's participant of a placing layer type.
type MakePlacement = fn() -> Box<dyn ErasedPlacement>;

/// What a [`Registry`] is built from.
#[derive(Default)]
pub struct RegistryBuilder {
    functions: Functions,
    layer_types: VecMap<&'static str, &'static dyn ErasedLayerType>,
    placing_types: VecMap<&'static str, MakePlacement>,
    source_types: VecMap<&'static str, &'static dyn ErasedSourceType>,
}

impl std::fmt::Debug for RegistryBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistryBuilder")
            .field("functions", &self.functions)
            .field("layer_types", &self.layer_types.keys().collect::<Vec<_>>())
            .field("placing_types", &self.placing_types.keys().collect::<Vec<_>>())
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

    /// Adds the layer type `T`. Fails for a name that an earlier type took.
    /// A type whose input is of another kind of source than its style half
    /// names doesn't compile.
    pub(crate) fn add_layer_type<T: LayerType>(&mut self) -> Result<(), RegisterError> {
        const {
            assert!(
                T::INFO.source as u8 == <T::Input as Find<T>>::KIND as u8,
                "a layer type draws from the kind of source its style half names"
            );
        }
        let name = T::INFO.name;
        if self.layer_types.contains_key(name) {
            return Err(RegisterError::Duplicate(name.to_owned()));
        }
        self.layer_types.insert(name, entry::<T>());
        Ok(())
    }

    /// Adds the layer type of `P`, a placement participant, whose layers
    /// place with `P` ([`Placement`]). Fails for a name that an earlier type
    /// took.
    pub(crate) fn add_placing_layer_type<P: Placement>(&mut self) -> Result<(), RegisterError> {
        self.add_layer_type::<P::Layer>()?;
        self.placing_types.insert(P::Layer::INFO.name, participant::<P>);
        Ok(())
    }

    /// Adds the source type `T`, both its halves. Fails for a name that an
    /// earlier type took. A type whose halves name different kinds of data
    /// doesn't compile.
    pub fn add_source_type<T: SourceType>(&mut self) -> Result<(), RegisterError> {
        const {
            assert!(
                T::INFO.layer_kind as u8 == <T::Data as kind::Deliver>::KIND as u8,
                "a source type's halves name the same kind of data"
            );
        }
        let name = T::INFO.name;
        if self.source_types.contains_key(name) {
            return Err(RegisterError::Duplicate(name.to_owned()));
        }
        self.source_types.insert(name, erased::entry::<T>());
        Ok(())
    }

    #[must_use]
    pub fn build(self) -> Arc<Registry> {
        Registry::new(Arc::new(self.functions), self.layer_types, self.placing_types, self.source_types)
    }
}

fn style_half(layer_type: &'static dyn ErasedLayerType) -> &'static dyn ErasedStyleLayerType {
    layer_type
}

fn style_source_half(source_type: &'static dyn ErasedSourceType) -> &'static dyn ErasedStyleSourceType {
    source_type
}

/// Adds the style specification's expression functions, layer types and
/// source types and the custom layer type. Fails for a name that an earlier
/// call took, or a role that an earlier function has.
pub fn register_builtins(builder: &mut RegistryBuilder) -> Result<(), RegisterError> {
    register_builtin_functions(builder.functions_mut())?;
    register_builtin_types(builder)
}

fn register_builtin_types(builder: &mut RegistryBuilder) -> Result<(), RegisterError> {
    register_builtin_layer_types(builder)?;
    register_builtin_source_types(builder)
}

fn register_builtin_layer_types(builder: &mut RegistryBuilder) -> Result<(), RegisterError> {
    builder.add_layer_type::<Fill>()?;
    builder.add_layer_type::<Line>()?;
    builder.add_layer_type::<Circle>()?;
    builder.add_placing_layer_type::<SymbolPlacement>()?;
    builder.add_layer_type::<Raster>()?;
    builder.add_layer_type::<Background>()?;
    builder.add_layer_type::<Hillshade>()?;
    builder.add_layer_type::<ColorRelief>()?;
    builder.add_layer_type::<FillExtrusion>()?;
    builder.add_layer_type::<Heatmap>()?;
    builder.add_layer_type::<Custom>()
}

fn register_builtin_source_types(builder: &mut RegistryBuilder) -> Result<(), RegisterError> {
    builder.add_source_type::<VectorSourceType>()?;
    builder.add_source_type::<RasterSourceType>()?;
    builder.add_source_type::<RasterDemSourceType>()?;
    builder.add_source_type::<GeoJsonSourceType>()?;
    builder.add_source_type::<ImageSourceType>()
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
    placing_types: VecMap<&'static str, MakePlacement>,
    source_types: VecMap<&'static str, &'static dyn ErasedSourceType>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry").field("style", &self.style).finish_non_exhaustive()
    }
}

impl Registry {
    /// The built-in registrations: what [`register_builtins`] adds. Every
    /// call returns the same value, built on the first: a registry never
    /// changes, so sharing it is a cache, and an app with registrations of
    /// its own still builds its registry with a [`RegistryBuilder`].
    #[must_use]
    pub fn with_builtins() -> Arc<Registry> {
        static BUILTINS: LazyLock<Arc<Registry>> = LazyLock::new(|| {
            let mut builder = RegistryBuilder::new();
            // Into no types, the built-ins register.
            let registered = register_builtin_types(&mut builder);
            debug_assert_eq!(registered, Ok(()));
            Registry::new(Functions::with_builtins(), builder.layer_types, builder.placing_types, builder.source_types)
        });
        Arc::clone(&BUILTINS)
    }

    fn new(
        functions: Arc<Functions>,
        layer_types: VecMap<&'static str, &'static dyn ErasedLayerType>,
        placing_types: VecMap<&'static str, MakePlacement>,
        source_types: VecMap<&'static str, &'static dyn ErasedSourceType>,
    ) -> Arc<Registry> {
        let style_halves = layer_types.values().map(|&layer_type| style_half(layer_type));
        let style_source_halves = source_types.values().map(|&source_type| style_source_half(source_type));
        let style = StyleRegistry::new(functions, style_halves, style_source_halves);
        Arc::new(Registry { style: Arc::new(style), layer_types, placing_types, source_types })
    }

    #[must_use]
    pub fn style(&self) -> &Arc<StyleRegistry> {
        &self.style
    }

    #[must_use]
    pub fn functions(&self) -> &Arc<Functions> {
        self.style.functions()
    }

    pub(crate) fn layer_type(&self, name: &str) -> Option<&'static dyn ErasedLayerType> {
        self.layer_types.get(name).copied()
    }

    /// The makers of the participants of the placing layer types, in the
    /// order they registered.
    pub(crate) fn placing_types(&self) -> impl Iterator<Item = MakePlacement> + '_ {
        self.placing_types.values().copied()
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
    use super::*;

    #[test]
    fn builtins_register_once() {
        let mut builder = RegistryBuilder::new();
        register_builtins(&mut builder).unwrap();
        assert!(matches!(register_builtins(&mut builder), Err(RegisterError::Duplicate(_))));
    }

    #[test]
    fn builtins_register_seven_source_types() {
        let mut builder = RegistryBuilder::new();
        register_builtins(&mut builder).unwrap();
        let registry = builder.build();
        let names: Vec<_> = registry.style().source_types().map(|source_type| source_type.info().name).collect();
        assert_eq!(names, ["geojson", "image", "raster", "raster-dem", "vector"]);
    }

    #[test]
    fn a_source_type_registers_once() {
        let mut builder = RegistryBuilder::new();
        builder.add_source_type::<GeoJsonSourceType>().unwrap();
        assert_eq!(builder.add_source_type::<GeoJsonSourceType>(), Err(RegisterError::Duplicate("geojson".to_owned())));
    }

    #[test]
    fn a_layer_type_registers_once() {
        let mut builder = RegistryBuilder::new();
        builder.add_layer_type::<Circle>().unwrap();
        assert_eq!(builder.add_layer_type::<Circle>(), Err(RegisterError::Duplicate("circle".to_owned())));
    }
}
