// A layer's style data is a `LayerStyle<T>` of its type `T`, shared as a
// `dyn Impl` (`ErasedLayerStyle`). `ImplBase` holds the members every layer
// has, and `dyn Impl` derefs to it, so `impl_.id` reads them.

use std::sync::Arc;

use mln_style_types::VisibilityType;
use mln_vec_map::VecSet;

use crate::expression::{Dependency, Expr};
use crate::filter::Filter;
pub use crate::layer_type::ErasedLayerStyle as Impl;
use crate::serialization::shared_expression;

pub(crate) type FontStack = Vec<String>;

/// The font stacks `layers` use, found by statically evaluating their
/// `text-font` values.
pub fn font_stacks<L: AsRef<dyn Impl>>(layers: impl IntoIterator<Item = L>) -> VecSet<FontStack> {
    let mut result = VecSet::new();
    for layer in layers {
        layer.as_ref().populate_font_stack(&mut result);
    }
    result
}

/// The members every layer has.
#[derive(Clone, Debug, mln_wire::Wire)]
pub struct ImplBase {
    // Backs an expression-valued "visibility" layout value; the evaluated
    // result is kept in `visibility` and re-evaluated by the style whenever
    // the global state changes.
    #[wire(with = "shared_expression::option")]
    pub(crate) visibility_expression: Option<Arc<Expr>>,

    pub id: String,
    pub source: String,
    pub source_layer: String,
    pub filter: Filter,
    pub min_zoom: f32,
    pub max_zoom: f32,
    pub visibility: VisibilityType,
}

impl ImplBase {
    #[must_use]
    pub fn new(layer_id: String, source_id: String) -> Self {
        ImplBase {
            visibility_expression: None,
            id: layer_id,
            source: source_id,
            source_layer: String::new(),
            filter: Filter::default(),
            min_zoom: f32::NEG_INFINITY,
            max_zoom: f32::INFINITY,
            visibility: VisibilityType::Visible,
        }
    }

    pub(crate) fn get_filter_dependencies(&self) -> Dependency {
        match &self.filter.expression {
            Some(expression) => expression.dependencies(),
            None => Dependency::NONE,
        }
    }

    /// Adds the global-state keys that the filter reads to `refs`.
    pub(crate) fn collect_filter_global_state_refs(&self, refs: &mut VecSet<String>) {
        if let Some(expression) = &self.filter.expression {
            crate::expression::collect_global_state_refs(expression.as_ref(), refs);
        }
    }
}

impl std::ops::Deref for dyn Impl {
    type Target = ImplBase;
    fn deref(&self) -> &ImplBase {
        self.base()
    }
}

// Debug is not a supertrait of this trait, nor of the other layer and source
// traits that have a `dyn` Debug impl like this one: through the vtable it
// would keep every implementation's derived Debug (and those of everything it
// holds) in the binary. Each prints what tells its values apart instead.
impl std::fmt::Debug for dyn Impl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Layer::Impl")
            .field("id", &self.base().id)
            .field("type", &self.get_type_info().name)
            .finish_non_exhaustive()
    }
}
