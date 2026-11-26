// Deviation: C++ groups into an `unordered_map` keyed by `LayoutGroupKey`
// and the worker iterates it in hash order; the port keeps the groups in
// the order their first layer appears. Groups are independent (one bucket
// each); only the feature index's sort indices, used by queries, follow
// the group order. Filters key the groups by their expressions, so layers
// with equal filters share a bucket whether or not they share the filter.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use mln_style::expression::Expr;
use mln_style::hash::{HashMap, RandomState};
use mln_style::layer::{ErasedLayerStyle, LayerTypeInfo};
use mln_style::types::VisibilityType;

use crate::layer::layer_type::ErasedEvaluatedLayer;

pub(crate) struct LayoutGroupKey {
    // Direct comparable fields (fast path)
    pub(crate) type_info: LayerTypeInfo,
    pub(crate) source: String,
    pub(crate) source_layer: String,
    pub(crate) min_zoom: f32,
    pub(crate) max_zoom: f32,
    pub(crate) visibility: VisibilityType,

    // Filter - compared by pointer, then by expression
    pub(crate) filter_expression: Option<Arc<Expr>>,

    // The style, for the layout comparison
    pub(crate) style: Arc<dyn ErasedLayerStyle>,

    /// A hash of the fields above but the style: keys that are equal hash
    /// alike.
    hash: u64,
}

impl PartialEq for LayoutGroupKey {
    fn eq(&self, other: &Self) -> bool {
        // Fast path: compare types and scalars first
        if self.type_info.name != other.type_info.name {
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

        !self.style.layout_difference(&*other.style)
    }
}

fn create_group_key(style: &Arc<dyn ErasedLayerStyle>) -> LayoutGroupKey {
    // 0.0 == -0.0.
    let zoom_bits = |zoom: f32| if zoom == 0.0 { 0 } else { zoom.to_bits() };
    let mut state = std::hash::DefaultHasher::new();
    style.info().name.hash(&mut state);
    style.base().source.hash(&mut state);
    style.base().source_layer.hash(&mut state);
    zoom_bits(style.base().min_zoom).hash(&mut state);
    zoom_bits(style.base().max_zoom).hash(&mut state);
    style.base().visibility.hash(&mut state);
    style.base().filter.expression_hash().hash(&mut state);
    LayoutGroupKey {
        type_info: style.info(),
        source: style.base().source.clone(),
        source_layer: style.base().source_layer.clone(),
        min_zoom: style.base().min_zoom,
        max_zoom: style.base().max_zoom,
        visibility: style.base().visibility,
        filter_expression: style.base().filter.expression.clone(),
        style: Arc::clone(style),
        hash: state.finish(),
    }
}

/// The groups with their keys, in first-appearance order.
pub(crate) type GroupMap = Vec<(LayoutGroupKey, Vec<Arc<dyn ErasedEvaluatedLayer>>)>;

/// Groups `layers` by their layout keys. A layer joins the first group, in
/// group order, whose key equals its own; only groups whose keys hash alike
/// are compared.
pub(crate) fn group_layers(layers: &[Arc<dyn ErasedEvaluatedLayer>]) -> GroupMap {
    let mut group_map: GroupMap = Vec::with_capacity(layers.len());
    // The first group with each key hash, and for each group the next
    // group with its key's hash.
    let mut first: HashMap<u64, usize> = HashMap::with_capacity_and_hasher(layers.len(), RandomState::default());
    let mut next: Vec<Option<usize>> = Vec::with_capacity(layers.len());

    for layer in layers {
        let key = create_group_key(&layer.shared_style());
        let mut candidate = first.get(&key.hash).copied();
        let mut last = None;
        // libstdc++ compares the new key against the stored one.
        while let Some(index) = candidate
            && key != group_map[index].0
        {
            last = Some(index);
            candidate = next[index];
        }
        if let Some(index) = candidate {
            group_map[index].1.push(Arc::clone(layer));
        } else {
            let index = group_map.len();
            match last {
                Some(last) => next[last] = Some(index),
                None => {
                    first.insert(key.hash, index);
                }
            }
            next.push(None);
            group_map.push((key, vec![Arc::clone(layer)]));
        }
    }
    group_map
}

#[cfg(test)]
mod tests {
    use mln_style::conversion::{Convertible, Error};
    use mln_style::expression::Functions;
    use mln_style::filter::{Filter, convert_filter};
    use mln_style::layer::{Layer, LayerStyle};
    use mln_style::layers::{Circle, Fill, Raster};

    use super::*;
    use crate::Registry;
    use crate::layer::layer_type::{EvaluatedLayer, LayerType};

    /// A filter of its own, which shares no expression with another.
    fn filter(json: &str) -> Filter {
        let json: serde_json::Value = serde_json::from_str(json).unwrap();
        convert_filter(&Convertible::from(&json), &Functions::with_builtins(), &mut Error::default()).expect("filter")
    }

    /// A `T` layer `id` of `source_layer` in "source".
    fn layer<T: LayerType>(id: &str, source_layer: &str, filter: Option<&Filter>) -> Arc<dyn ErasedEvaluatedLayer> {
        let mut layer = Layer::new(Registry::with_builtins().style(), T::INFO.name, id, "source").unwrap();
        layer.set_source_layer(source_layer);
        if let Some(filter) = filter {
            layer.set_filter(filter);
        }
        Arc::new(EvaluatedLayer::<T>::new(LayerStyle::recover(layer.style()).unwrap()))
    }

    fn circle(id: &str, source_layer: &str, filter: Option<&Filter>) -> Arc<dyn ErasedEvaluatedLayer> {
        layer::<Circle>(id, source_layer, filter)
    }

    fn ids(groups: &GroupMap) -> Vec<Vec<String>> {
        groups.iter().map(|(_, group)| group.iter().map(|l| l.style().base().id.clone()).collect()).collect()
    }

    /// The grouping by comparing each key with every group's, in order.
    fn grouped_by_comparison(layers: &[Arc<dyn ErasedEvaluatedLayer>]) -> Vec<Vec<String>> {
        let mut groups: GroupMap = Vec::new();
        for layer in layers {
            let key = create_group_key(&layer.shared_style());
            match groups.iter_mut().find(|(k, _)| key == *k) {
                Some((_, group)) => group.push(Arc::clone(layer)),
                None => groups.push((key, vec![Arc::clone(layer)])),
            }
        }
        ids(&groups)
    }

    /// Layers group by their keys in the order of their groups' first
    /// layers, as comparing each key with every group's does.
    #[test]
    fn groups_as_comparison_does_in_first_appearance_order() {
        let motorway = filter(r#"["==", ["get", "class"], "motorway"]"#);
        let primary = filter(r#"["==", ["get", "class"], "primary"]"#);
        let layers = [
            circle("a", "roads", Some(&motorway)),
            circle("b", "roads", Some(&primary)),
            circle("c", "roads", Some(&motorway)),
            circle("d", "roads", None),
            circle("e", "water", Some(&motorway)),
            layer::<Fill>("f", "roads", Some(&motorway)),
            circle("g", "roads", Some(&primary)),
            circle("h", "roads", None),
            // An equal filter that isn't shared.
            circle("i", "roads", Some(&filter(r#"["==", ["get", "class"], "motorway"]"#))),
        ];
        let groups = ids(&group_layers(&layers));
        assert_eq!(groups, [vec!["a", "c", "i"], vec!["b", "g"], vec!["d", "h"], vec!["e"], vec!["f"]]);
        assert_eq!(groups, grouped_by_comparison(&layers));
    }

    /// Equal filters group whether or not they share their expressions, both
    /// where the layout ignores the filter and where it doesn't.
    #[test]
    fn groups_equal_filters_that_are_not_shared() {
        fn check(new_layer: impl Fn(&str, &str) -> Arc<dyn ErasedEvaluatedLayer>) {
            let layers = [
                new_layer("a", r#"["==", ["get", "rank"], 0]"#),
                new_layer("b", r#"["==", ["get", "rank"], 1]"#),
                new_layer("c", r#"["==", ["get", "rank"], -0.0]"#),
                // A `let` compares its bindings too.
                new_layer("d", r#"["let", "x", 1, ["==", ["get", "rank"], 2]]"#),
                new_layer("e", r#"["let", "x", 2, ["==", ["get", "rank"], 2]]"#),
                new_layer("f", r#"["==", ["get", "rank"], 2]"#),
                new_layer("g", r#"["==", ["get", "rank"], 1]"#),
            ];
            let groups = ids(&group_layers(&layers));
            assert_eq!(groups, [vec!["a", "c"], vec!["b", "g"], vec!["d"], vec!["e"], vec!["f"]]);
            assert_eq!(groups, grouped_by_comparison(&layers));
        }
        check(|id, json| layer::<Raster>(id, "", Some(&filter(json))));
        check(|id, json| circle(id, "roads", Some(&filter(json))));
    }

    /// Filters that differ only in a final literal, as a style that splits
    /// a source's features among many layers has.
    #[test]
    fn groups_many_filters_as_comparison_does() {
        let filters: Vec<_> =
            (0..50).map(|i| filter(&format!(r#"["==", ["%", ["number", ["id"]], 50], {i}]"#))).collect();
        let layers: Vec<_> =
            (0..200).map(|i| circle(&format!("layer-{i}"), "points", Some(&filters[i % 50]))).collect();
        let groups = ids(&group_layers(&layers));
        assert_eq!(groups.len(), 50);
        assert_eq!(groups, grouped_by_comparison(&layers));
        // Each group's key hashes apart from the others.
        let hashes: mln_style::hash::HashSet<u64> =
            layers.iter().map(|l| create_group_key(&l.shared_style()).hash).collect();
        assert_eq!(hashes.len(), 50);
    }
}
