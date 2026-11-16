//! The registrations that a runtime, its renderers and its tile workers
//! share: the expression functions and the layer types.
//!
//! An app fills a [`RegistryBuilder`] by explicit calls, [`register_builtins`]
//! first, and builds the [`Registry`] once. Tile workers in other wasm
//! instances make the same calls, since they run the app's own module: the
//! worker codec checks that both sides' registries have the same
//! [`digest`](Registry::digest) before it decodes a job.

use std::hash::Hasher;
use std::sync::Arc;

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
    /// The layer types added, by name.
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
    /// No registrations.
    #[must_use]
    pub fn new() -> Self {
        RegistryBuilder::default()
    }

    /// The expression functions, to add to.
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

    /// The registry of what was added.
    #[must_use]
    pub fn build(self) -> Arc<Registry> {
        let style_halves = self.layer_types.values().map(|&layer_type| style_half(layer_type));
        let style = StyleRegistry::new(Arc::new(self.functions), style_halves);
        let mut hasher = Fnv::default();
        style.digest(&mut hasher);
        Arc::new(Registry { style: Arc::new(style), layer_types: self.layer_types, digest: hasher.finish() })
    }
}

/// The style half of `layer_type`.
fn style_half(layer_type: &'static dyn ErasedLayerType) -> &'static dyn ErasedStyleLayerType {
    layer_type
}

/// Adds the style specification's expression functions and layer types.
/// Fails for a name that an earlier call took, or a role that an earlier
/// function has.
pub fn register_builtins(builder: &mut RegistryBuilder) -> Result<(), RegisterError> {
    register_builtin_functions(builder.functions_mut())?;
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
    /// The render halves of the style's layer types, by name.
    layer_types: VecMap<&'static str, &'static dyn ErasedLayerType>,
    /// [`Registry::digest`].
    digest: u64,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry").field("style", &self.style).field("digest", &self.digest).finish_non_exhaustive()
    }
}

impl Registry {
    /// The built-in registrations: what [`register_builtins`] adds.
    #[must_use]
    pub fn with_builtins() -> Arc<Registry> {
        let mut builder = RegistryBuilder::new();
        // Into no registrations, the built-ins register.
        let registered = register_builtins(&mut builder);
        debug_assert_eq!(registered, Ok(()));
        builder.build()
    }

    /// What a style's JSON, its layers' property values and its filters
    /// parse against.
    #[must_use]
    pub fn style(&self) -> &Arc<StyleRegistry> {
        &self.style
    }

    /// The expression functions.
    #[must_use]
    pub fn functions(&self) -> &Arc<Functions> {
        self.style.functions()
    }

    /// The layer type named `name`.
    pub(crate) fn layer_type(&self, name: &str) -> Option<&'static dyn ErasedLayerType> {
        self.layer_types.get(name).copied()
    }

    /// A hash of the registrations' names and signatures, in the order they
    /// were added, which is the same in every instance of the module:
    /// registries filled by the same calls have the same digest.
    #[must_use]
    pub fn digest(&self) -> u64 {
        self.digest
    }
}

/// The FNV-1a hash, which unlike the maps' hasher has no per-process seed.
struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
}

impl Hasher for Fnv {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

#[cfg(test)]
mod tests {
    use mln_style::expression::Type;
    use mln_style::expression::functions::{Eval, OverloadSpec, Params, Role};

    use super::*;

    fn with(extra: &str) -> Arc<Registry> {
        let mut builder = RegistryBuilder::new();
        register_builtins(&mut builder).unwrap();
        let overload = OverloadSpec {
            result: Type::Number,
            params: Params::Fixed(Vec::new()),
            dependencies: mln_style::expression::Dependency::NONE,
            role: Role::Plain,
            eval: Eval::Number(|_, _| Ok(1.0)),
        };
        builder.functions_mut().add(extra, vec![overload]).unwrap();
        builder.build()
    }

    #[test]
    fn registries_filled_alike_have_one_digest() {
        assert_eq!(Registry::with_builtins().digest(), Registry::with_builtins().digest());
        assert_eq!(with("one").digest(), with("one").digest());
        assert_ne!(with("one").digest(), Registry::with_builtins().digest());
        assert_ne!(with("one").digest(), with("two").digest());
    }

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

    #[test]
    fn builtins_register_the_builtin_layer_types() {
        let registry = Registry::with_builtins();
        let registered: Vec<_> = registry.style().layer_types().map(ErasedStyleLayerType::info).collect();
        let mut builtins: Vec<_> =
            mln_style::layer_type::BUILTIN_STYLE_TYPES.iter().map(|layer_type| layer_type.info()).collect();
        builtins.sort_by_key(|info| info.name);
        assert_eq!(registered, builtins);
        let rendered: Vec<_> = registry.layer_types.values().map(|layer_type| layer_type.info()).collect();
        assert_eq!(rendered, builtins);
    }
}
