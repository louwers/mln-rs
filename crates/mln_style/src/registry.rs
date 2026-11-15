//! The registrations that a style parses against: its expression functions
//! and its layer types.

use std::cell::Cell;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use mln_conversion::{Convertible, Error};
use mln_vec_map::VecMap;

use crate::expression::{Functions, functions};
use crate::layer::{Layer, SourceKind};
use crate::layer_type::{BUILTIN_STYLE_TYPES, ErasedStyleLayerType};

/// The expression functions and layer types that a style's JSON, its layers'
/// property values and its filters parse against.
pub struct StyleRegistry {
    functions: Arc<Functions>,
    layer_types: VecMap<&'static str, &'static dyn ErasedStyleLayerType>,
}

impl StyleRegistry {
    /// The built-in layer types, with `functions`.
    #[must_use]
    pub fn new(functions: Arc<Functions>) -> Self {
        let layer_types = BUILTIN_STYLE_TYPES.iter().map(|&layer_type| (layer_type.info().name, layer_type)).collect();
        StyleRegistry { functions, layer_types }
    }

    /// The built-in layer types and expression functions.
    #[must_use]
    pub fn with_builtins() -> Arc<Self> {
        Arc::new(StyleRegistry::new(Functions::with_builtins()))
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

    /// Feeds `hasher` what parsing and decoding resolve against: the
    /// functions' [`digest`](Functions::digest), then the layer types'
    /// infos, by name.
    pub fn digest(&self, hasher: &mut impl Hasher) {
        self.functions.digest(hasher);
        self.layer_types.len().hash(hasher);
        for layer_type in self.layer_types.values() {
            layer_type.info().hash(hasher);
        }
    }

    /// A layer of the type `type_` from its style JSON `value`, without its
    /// properties.
    pub(crate) fn create_layer(
        &self,
        type_: &str,
        id: &str,
        value: &Convertible<'_>,
        error: &mut Error,
    ) -> Option<Box<dyn Layer>> {
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
        let layer = layer_type.create_layer(id, &source, &self.functions);
        if layer.is_none() {
            error.message = format!("Error parsing layer {id} of type: {type_}");
        }
        layer
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
