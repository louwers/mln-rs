// C++ `Layer::Impl` is an abstract base class with data members. In Rust the
// data members are `ImplBase` and the virtual interface is the `Impl` trait
// (re-exported as `layer::Impl`); a concrete impl such as
// `background_layer::Impl` holds an `ImplBase` and implements the trait.
// `dyn Impl` derefs to its `ImplBase`, so `impl_.id` reads as in C++.

use std::any::Any;
use std::sync::Arc;

use mln_style_types::VisibilityType;
use mln_vec_map::VecSet;
use serde::{Deserialize, Serialize};

use crate::expression::{Dependency, Expression};
use crate::filter::Filter;
use crate::layer::LayerTypeInfo;
use crate::serialization::shared_expression;

pub(crate) type FontStack = Vec<String>;

/// The data members of `Layer::Impl`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImplBase {
    // Backs an expression-valued "visibility" layout value; the evaluated
    // result is kept in `visibility` and re-evaluated by the style whenever
    // the global state changes.
    #[serde(with = "shared_expression::option")]
    pub(crate) visibility_expression: Option<Arc<dyn Expression>>,

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

    /// The base `Layer::Impl::collectLayoutGlobalStateRefs` (the filter's
    /// global-state refs), which overrides call as
    /// `Layer::Impl::collectLayoutGlobalStateRefs(refs)`.
    pub(crate) fn collect_filter_global_state_refs(&self, refs: &mut VecSet<String>) {
        if let Some(expression) = &self.filter.expression {
            crate::expression::collect_global_state_refs(expression.as_ref(), refs);
        }
    }
}

/// The virtual interface of `Layer::Impl`.
pub trait Impl: Any + Send + Sync {
    fn base(&self) -> &ImplBase;
    fn base_mut(&mut self) -> &mut ImplBase;

    /// Returns true buckets if properties affecting layout have changed: i.e.
    /// filter, visibility, layout properties, or data-driven paint properties.
    fn has_layout_difference(&self, other: &dyn Impl) -> bool;

    /// Returns the statically allocated layer type info structure.
    fn get_type_info(&self) -> &'static LayerTypeInfo;

    /// Populates the given `font_stack` with fonts being used by the layer.
    fn populate_font_stack(&self, _font_stack: &mut VecSet<FontStack>) {}

    /// Collect the style dependencies for this layer
    fn get_dependencies(&self) -> Dependency {
        Dependency::NONE
    }

    /// Collect the dependencies of properties that affect tile layout:
    /// the filter and the layout properties.
    fn get_layout_dependencies(&self) -> Dependency {
        self.base().get_filter_dependencies()
    }

    /// Collect the global-state properties referenced by expressions that
    /// affect tile layout, mirroring `get_layout_dependencies()`.
    fn collect_layout_global_state_refs(&self, refs: &mut VecSet<String>) {
        self.base().collect_filter_global_state_refs(refs);
    }
}

impl std::ops::Deref for dyn Impl {
    type Target = ImplBase;
    fn deref(&self) -> &ImplBase {
        self.base()
    }
}

impl dyn Impl {
    /// The impl as a `T`, if it is one.
    pub fn downcast_ref<T: Impl>(&self) -> Option<&T> {
        (self as &dyn Any).downcast_ref()
    }
}

/// Returns a new handle to `impl_` as its concrete layer impl type.
///
/// # Panics
///
/// If `impl_` is not a `T`.
pub fn downcast_arc<T: Impl>(impl_: &Arc<dyn Impl>) -> Arc<T> {
    let any: Arc<dyn Any + Send + Sync> = impl_.clone();
    let Ok(typed) = any.downcast::<T>() else {
        panic!("downcast_arc: layer impl type mismatch");
    };
    typed
}

// Debug is not a supertrait, for the reason at `impl Debug for dyn Layer`
// (layer.rs).
impl std::fmt::Debug for dyn Impl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Layer::Impl")
            .field("id", &self.base().id)
            .field("type", &self.get_type_info().type_)
            .finish_non_exhaustive()
    }
}
