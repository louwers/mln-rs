// Deviation: C++ groups into an `unordered_map` keyed by `LayoutGroupKey`
// and the worker iterates it in hash order; the port keeps the groups in
// the order their first layer appears. Groups are independent (one bucket
// each); only the feature index's sort indices, used by queries, follow
// the group order.

use std::sync::Arc;

use mln_style::expression::Expression;
use mln_style::layer;
use mln_style::layer::LayerTypeInfo;
use mln_style::layer_properties::LayerProperties;
use mln_style_types::VisibilityType;

pub(crate) struct LayoutGroupKey {
    // Direct comparable fields (fast path)
    pub(crate) type_info: &'static LayerTypeInfo,
    pub(crate) source: String,
    pub(crate) source_layer: String,
    pub(crate) min_zoom: f32,
    pub(crate) max_zoom: f32,
    pub(crate) visibility: VisibilityType,

    // Filter - use the shared pointer for identity comparison
    pub(crate) filter_expression: Option<Arc<dyn Expression>>,

    // Reference to impl for layout comparison
    pub(crate) impl_: Arc<dyn layer::Impl>,
}

impl PartialEq for LayoutGroupKey {
    fn eq(&self, other: &Self) -> bool {
        // Fast path: compare pointers and scalars first
        if !std::ptr::eq(self.type_info, other.type_info) {
            return false;
        }
        if self.min_zoom != other.min_zoom {
            return false;
        }
        if self.max_zoom != other.max_zoom {
            return false;
        }
        if self.visibility != other.visibility {
            return false;
        }

        // Compare source strings
        if self.source != other.source {
            return false;
        }
        if self.source_layer != other.source_layer {
            return false;
        }

        // Filter comparison - prefer pointer equality
        let same_filter_pointer = match (&self.filter_expression, &other.filter_expression) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        };
        if !same_filter_pointer {
            match (&self.filter_expression, &other.filter_expression) {
                (Some(a), Some(b)) => {
                    // Deep comparison if pointers differ
                    if **a != **b {
                        return false;
                    }
                }
                _ => return false,
            }
        }

        !self.impl_.has_layout_difference(&*other.impl_)
    }
}

fn create_group_key(impl_: &Arc<dyn layer::Impl>) -> LayoutGroupKey {
    LayoutGroupKey {
        type_info: impl_.get_type_info(),
        source: impl_.source.clone(),
        source_layer: impl_.source_layer.clone(),
        min_zoom: impl_.min_zoom,
        max_zoom: impl_.max_zoom,
        visibility: impl_.visibility,
        filter_expression: impl_.filter.expression.clone(),
        impl_: impl_.clone(),
    }
}

/// The groups with their keys, in first-appearance order.
pub(crate) type GroupMap = Vec<(LayoutGroupKey, Vec<Arc<dyn LayerProperties>>)>;

pub(crate) fn group_layers(layers: &[Arc<dyn LayerProperties>]) -> GroupMap {
    let mut group_map: GroupMap = Vec::with_capacity(layers.len());

    for layer in layers {
        let key = create_group_key(&layer.base_impl);
        // libstdc++ compares the new key against the stored one.
        match group_map.iter_mut().find(|(k, _)| key == *k) {
            Some((_, group)) => group.push(layer.clone()),
            None => group_map.push((key, vec![layer.clone()])),
        }
    }
    group_map
}
