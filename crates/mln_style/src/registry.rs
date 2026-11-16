//! The registrations that a style parses against: its expression functions
//! and its layer types.

use std::cell::Cell;
use std::sync::{Arc, LazyLock};

use mln_conversion::{Convertible, Error};
use mln_vec_map::VecMap;

use crate::expression::{Functions, functions};
use crate::layer::Layer;
use crate::layer_type::{BUILTIN_STYLE_TYPES, ErasedStyleLayerType, SourceKind};

/// The expression functions and layer types that a style's JSON, its layers'
/// property values and its filters parse against.
pub struct StyleRegistry {
    functions: Arc<Functions>,
    layer_types: VecMap<&'static str, &'static dyn ErasedStyleLayerType>,
}

impl StyleRegistry {
    /// The registry of `functions` and `layer_types`. A layer type is known
    /// by its name: of two types of one name, the first counts.
    #[must_use]
    pub fn new(
        functions: Arc<Functions>,
        layer_types: impl IntoIterator<Item = &'static dyn ErasedStyleLayerType>,
    ) -> Self {
        let mut by_name = VecMap::new();
        for layer_type in layer_types {
            by_name.entry(layer_type.info().name).or_insert(layer_type);
        }
        StyleRegistry { functions, layer_types: by_name }
    }

    /// The built-in layer types and expression functions. Every call
    /// returns the same value, built on the first: it never changes, so
    /// sharing it is a cache.
    #[must_use]
    pub fn with_builtins() -> Arc<Self> {
        static BUILTINS: LazyLock<Arc<StyleRegistry>> =
            LazyLock::new(|| Arc::new(StyleRegistry::new(Functions::with_builtins(), BUILTIN_STYLE_TYPES)));
        Arc::clone(&BUILTINS)
    }

    /// The expression functions.
    #[must_use]
    pub fn functions(&self) -> &Arc<Functions> {
        &self.functions
    }

    /// The layer type named `name`.
    #[must_use]
    pub fn layer_type(&self, name: &str) -> Option<&'static dyn ErasedStyleLayerType> {
        self.layer_types.get(name).copied()
    }

    /// The layer types, by name.
    pub fn layer_types(&self) -> impl Iterator<Item = &'static dyn ErasedStyleLayerType> + '_ {
        self.layer_types.values().copied()
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
            .finish_non_exhaustive()
    }
}

thread_local! {
    /// The registry that layers read on this thread resolve their types
    /// against, while a `decode_with` runs.
    static DECODING: Cell<Option<Arc<StyleRegistry>>> = const { Cell::new(None) };
}

/// Runs `f`, in which layers read with [`mln_wire`] resolve their type's
/// name against `registry`, and compound expressions their function's
/// against its functions (as in [`functions::decode_with`]). Writing needs
/// no registry. Scopes nest: the innermost wins.
pub fn decode_with<R>(registry: &Arc<StyleRegistry>, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Arc<StyleRegistry>>);

    impl Drop for Restore {
        fn drop(&mut self) {
            DECODING.set(self.0.take());
        }
    }

    let _restore = Restore(DECODING.replace(Some(Arc::clone(registry))));
    functions::decode_with(registry.functions(), f)
}

/// The layer type named `name` in the registry of the enclosing
/// [`decode_with`].
pub(crate) fn decoding_layer_type(name: &str) -> Result<&'static dyn ErasedStyleLayerType, &'static str> {
    let registry = DECODING.take();
    let layer_type = match &registry {
        None => Err("a layer outside of a decoding session"),
        Some(registry) => registry.layer_type(name).ok_or("unknown layer type"),
    };
    DECODING.set(registry);
    layer_type
}
