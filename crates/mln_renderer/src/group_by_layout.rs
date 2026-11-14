// Deviation: C++ groups into an `unordered_map` keyed by `LayoutGroupKey`
// and the worker iterates it in hash order; the port keeps the groups in
// the order their first layer appears. Groups are independent (one bucket
// each); only the feature index's sort indices, used by queries, follow
// the group order. C++ hashes the filter by its pointer, so filters that
// are equal without being shared seldom meet in a bucket; the port hashes
// the filter's structure and groups them.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
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

    // Filter - compared by pointer, then by expression
    pub(crate) filter_expression: Option<Arc<dyn Expression>>,

    // Reference to impl for layout comparison
    pub(crate) impl_: Arc<dyn layer::Impl>,

    /// A hash of the fields above but the impl: keys that are equal hash
    /// alike.
    hash: u64,
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
    // 0.0 == -0.0.
    let zoom_bits = |zoom: f32| if zoom == 0.0 { 0 } else { zoom.to_bits() };
    let mut state = std::hash::DefaultHasher::new();
    std::ptr::from_ref(impl_.get_type_info()).hash(&mut state);
    impl_.source.hash(&mut state);
    impl_.source_layer.hash(&mut state);
    zoom_bits(impl_.min_zoom).hash(&mut state);
    zoom_bits(impl_.max_zoom).hash(&mut state);
    impl_.visibility.hash(&mut state);
    impl_.filter.expression_hash().hash(&mut state);
    LayoutGroupKey {
        type_info: impl_.get_type_info(),
        source: impl_.source.clone(),
        source_layer: impl_.source_layer.clone(),
        min_zoom: impl_.min_zoom,
        max_zoom: impl_.max_zoom,
        visibility: impl_.visibility,
        filter_expression: impl_.filter.expression.clone(),
        impl_: Arc::clone(impl_),
        hash: state.finish(),
    }
}

/// The groups with their keys, in first-appearance order.
pub(crate) type GroupMap = Vec<(LayoutGroupKey, Vec<Arc<dyn LayerProperties>>)>;

/// Groups `layers` by their layout keys. A layer joins the first group, in
/// group order, whose key equals its own; only groups whose keys hash alike
/// are compared.
pub(crate) fn group_layers(layers: &[Arc<dyn LayerProperties>]) -> GroupMap {
    let mut group_map: GroupMap = Vec::with_capacity(layers.len());
    // The first group with each key hash, and for each group the next
    // group with its key's hash.
    let mut first: HashMap<u64, usize> = HashMap::with_capacity(layers.len());
    let mut next: Vec<Option<usize>> = Vec::with_capacity(layers.len());

    for layer in layers {
        let key = create_group_key(&layer.base_impl);
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
    use mln_style::conversion::{Convertible, Error, convert};
    use mln_style::expression::Dependency;
    use mln_style::filter::Filter;
    use mln_style::layer::Layer;
    use mln_style::layer_properties::LayerPropertiesBase;
    use mln_style::layers::circle_layer::CircleLayer;
    use mln_style::layers::fill_layer::FillLayer;
    use mln_style::layers::raster_layer::RasterLayer;

    use super::*;

    struct Properties(LayerPropertiesBase);

    impl LayerProperties for Properties {
        fn base(&self) -> &LayerPropertiesBase {
            &self.0
        }

        fn base_mut(&mut self) -> &mut LayerPropertiesBase {
            &mut self.0
        }

        fn get_dependencies(&self) -> Dependency {
            Dependency::NONE
        }
    }

    /// A filter of its own, which shares no expression with another.
    fn filter(json: &str) -> Filter {
        let json: serde_json::Value = serde_json::from_str(json).unwrap();
        convert::<Filter>(&Convertible::from(&json), &mut Error::default()).expect("filter")
    }

    fn layer(mut layer: impl Layer, source_layer: &str, filter: Option<&Filter>) -> Arc<dyn LayerProperties> {
        layer.set_source_layer(source_layer);
        if let Some(filter) = filter {
            layer.set_filter(filter);
        }
        Arc::new(Properties(LayerPropertiesBase::new(layer.base_impl())))
    }

    fn circle(id: &str, source_layer: &str, filter: Option<&Filter>) -> Arc<dyn LayerProperties> {
        layer(CircleLayer::new(id, "source"), source_layer, filter)
    }

    fn ids(groups: &GroupMap) -> Vec<Vec<String>> {
        groups.iter().map(|(_, group)| group.iter().map(|l| l.base_impl.id.clone()).collect()).collect()
    }

    /// The grouping by comparing each key with every group's, in order.
    fn grouped_by_comparison(layers: &[Arc<dyn LayerProperties>]) -> Vec<Vec<String>> {
        let mut groups: GroupMap = Vec::new();
        for layer in layers {
            let key = create_group_key(&layer.base_impl);
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
            layer(FillLayer::new("f", "source"), "roads", Some(&motorway)),
            circle("g", "roads", Some(&primary)),
            circle("h", "roads", None),
            // An equal filter that isn't shared: a filterable layer's
            // layout differs from another's unless they share the filter.
            circle("i", "roads", Some(&filter(r#"["==", ["get", "class"], "motorway"]"#))),
        ];
        let groups = ids(&group_layers(&layers));
        assert_eq!(groups, [vec!["a", "c"], vec!["b", "g"], vec!["d", "h"], vec!["e"], vec!["f"], vec!["i"]]);
        assert_eq!(groups, grouped_by_comparison(&layers));
    }

    /// Where the layout ignores the filter, the keys compare filters by
    /// their expressions, so equal filters group whether or not they share
    /// them.
    #[test]
    fn groups_equal_filters_that_are_not_shared() {
        let raster = |id: &str, json: &str| layer(RasterLayer::new(id, "source"), "", Some(&filter(json)));
        let layers = [
            raster("a", r#"["==", ["get", "rank"], 0]"#),
            raster("b", r#"["==", ["get", "rank"], 1]"#),
            raster("c", r#"["==", ["get", "rank"], -0.0]"#),
            // `let` compares its result alone.
            raster("d", r#"["let", "x", 1, ["==", ["get", "rank"], 2]]"#),
            raster("e", r#"["let", "x", 2, ["==", ["get", "rank"], 2]]"#),
            raster("f", r#"["==", ["get", "rank"], 2]"#),
            raster("g", r#"["==", ["get", "rank"], 1]"#),
        ];
        let groups = ids(&group_layers(&layers));
        assert_eq!(groups, [vec!["a", "c"], vec!["b", "g"], vec!["d", "e"], vec!["f"]]);
        assert_eq!(groups, grouped_by_comparison(&layers));
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
        let hashes: std::collections::HashSet<u64> =
            layers.iter().map(|l| create_group_key(&l.base_impl).hash).collect();
        assert_eq!(hashes.len(), 50);
    }
}
