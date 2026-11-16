//! The registrations that a runtime, its renderers and its tile workers
//! share: the expression functions and the layer types.
//!
//! An app fills a [`RegistryBuilder`] by explicit calls, [`register_builtins`]
//! first, and builds the [`Registry`] once. Tile workers in other wasm
//! instances make the same calls, since they run the app's own module.

use std::sync::{Arc, LazyLock};

use mln_style::StyleRegistry;
use mln_style::expression::Functions;
use mln_style::expression::functions::{RegisterError, register_builtin_functions};
use mln_style::layer_type::ErasedStyleLayerType;
use mln_style::layers::{
    Background, Circle, ColorRelief, Custom, Fill, FillExtrusion, Heatmap, Hillshade, Line, Raster, Symbol,
};
use mln_vec_map::VecMap;

use crate::layer_type::{ErasedLayerType, LayerType, entry};

/// What a [`Registry`] is built from.
#[derive(Default)]
pub struct RegistryBuilder {
    functions: Functions,
    layer_types: VecMap<&'static str, &'static dyn ErasedLayerType>,
}

impl std::fmt::Debug for RegistryBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistryBuilder")
            .field("functions", &self.functions)
            .field("layer_types", &self.layer_types.keys().collect::<Vec<_>>())
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
    pub(crate) fn add_layer_type<T: LayerType>(&mut self) -> Result<(), RegisterError> {
        let name = T::INFO.name;
        if self.layer_types.contains_key(name) {
            return Err(RegisterError::Duplicate(name.to_owned()));
        }
        self.layer_types.insert(name, entry::<T>());
        Ok(())
    }

    #[must_use]
    pub fn build(self) -> Arc<Registry> {
        Registry::new(Arc::new(self.functions), self.layer_types)
    }
}

fn style_half(layer_type: &'static dyn ErasedLayerType) -> &'static dyn ErasedStyleLayerType {
    layer_type
}

/// Adds the style specification's expression functions and layer types.
/// Fails for a name that an earlier call took, or a role that an earlier
/// function has.
pub fn register_builtins(builder: &mut RegistryBuilder) -> Result<(), RegisterError> {
    register_builtin_functions(builder.functions_mut())?;
    register_builtin_layer_types(builder)
}

fn register_builtin_layer_types(builder: &mut RegistryBuilder) -> Result<(), RegisterError> {
    builder.add_layer_type::<Fill>()?;
    builder.add_layer_type::<Line>()?;
    builder.add_layer_type::<Circle>()?;
    builder.add_layer_type::<Symbol>()?;
    builder.add_layer_type::<Raster>()?;
    builder.add_layer_type::<Background>()?;
    builder.add_layer_type::<Hillshade>()?;
    builder.add_layer_type::<ColorRelief>()?;
    builder.add_layer_type::<FillExtrusion>()?;
    builder.add_layer_type::<Heatmap>()?;
    builder.add_layer_type::<Custom>()
}

/// The expression functions and layer types that a style parses against,
/// its renderer draws and its tile workers lay out, built by a
/// [`RegistryBuilder`]. It doesn't change once built.
///
/// A map and the renderer and tile workers that draw it must be given
/// registries filled by the same calls: they identify functions and layer
/// types by name.
pub struct Registry {
    style: Arc<StyleRegistry>,
    layer_types: VecMap<&'static str, &'static dyn ErasedLayerType>,
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
            // Into no layer types, the built-ins register.
            let registered = register_builtin_layer_types(&mut builder);
            debug_assert_eq!(registered, Ok(()));
            Registry::new(Functions::with_builtins(), builder.layer_types)
        });
        Arc::clone(&BUILTINS)
    }

    fn new(
        functions: Arc<Functions>,
        layer_types: VecMap<&'static str, &'static dyn ErasedLayerType>,
    ) -> Arc<Registry> {
        let style_halves = layer_types.values().map(|&layer_type| style_half(layer_type));
        Arc::new(Registry { style: Arc::new(StyleRegistry::new(functions, style_halves)), layer_types })
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
    fn a_layer_type_registers_once() {
        let mut builder = RegistryBuilder::new();
        builder.add_layer_type::<Circle>().unwrap();
        assert_eq!(builder.add_layer_type::<Circle>(), Err(RegisterError::Duplicate("circle".to_owned())));
    }
}
