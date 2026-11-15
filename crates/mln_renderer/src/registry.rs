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

/// What a [`Registry`] is built from.
#[derive(Debug, Default)]
pub struct RegistryBuilder {
    functions: Functions,
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

    /// The registry of the functions added, with the built-in layer types.
    #[must_use]
    pub fn build(self) -> Arc<Registry> {
        let style = StyleRegistry::new(Arc::new(self.functions));
        let mut hasher = Fnv::default();
        style.digest(&mut hasher);
        Arc::new(Registry { style: Arc::new(style), digest: hasher.finish() })
    }
}

/// Adds the style specification's expression functions. Fails for a name
/// that an earlier call took, or a role that an earlier function has.
pub fn register_builtins(builder: &mut RegistryBuilder) -> Result<(), RegisterError> {
    register_builtin_functions(builder.functions_mut())
}

/// The expression functions and layer types that a style parses against,
/// its renderer draws and its tile workers lay out, built by a
/// [`RegistryBuilder`]. It doesn't change once built.
///
/// A map and the renderer and tile workers that draw it must be given
/// registries filled by the same calls: they identify functions and layer
/// types by name.
#[derive(Debug)]
pub struct Registry {
    style: Arc<StyleRegistry>,
    /// [`Registry::digest`].
    digest: u64,
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
    fn builtins_register_the_builtin_layer_types() {
        let registry = Registry::with_builtins();
        let registered: Vec<_> =
            registry.style().layer_types().map(mln_style::layer_type::ErasedStyleLayerType::info).collect();
        let mut builtins: Vec<_> =
            mln_style::layer_type::BUILTIN_STYLE_TYPES.iter().map(|layer_type| layer_type.info()).collect();
        builtins.sort_by_key(|info| info.name);
        assert_eq!(registered, builtins);
    }
}
