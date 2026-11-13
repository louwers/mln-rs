//! The feature state that a map keeps for its sources' features, and sends to
//! its renderer with each update.

use std::sync::Arc;

use mln_renderer::bucket::LayerFeatureStates;
use mln_renderer::source_state::FeatureStatesBySource;
use mln_style::expression::expression::FeatureState;

/// A map's feature state, by source, source layer and feature ID, where the
/// empty source layer stands for none. It keeps no empty entry, so a removal
/// leaves no trace.
///
/// The state is copied on write: an update shares it until the store
/// changes, and then shares the sources that the change leaves alone.
#[derive(Clone, Debug, Default)]
pub(crate) struct FeatureStateStore {
    sources: Arc<FeatureStatesBySource>,
}

impl FeatureStateStore {
    /// The state as an update carries it.
    pub(crate) fn shared(&self) -> Arc<FeatureStatesBySource> {
        Arc::clone(&self.sources)
    }

    fn layers(&self, source_id: &str) -> Option<&LayerFeatureStates> {
        self.sources.get(source_id).map(|layers| &**layers)
    }

    /// The source's state, to change.
    fn layers_mut(&mut self, source_id: &str) -> &mut LayerFeatureStates {
        Arc::make_mut(Arc::make_mut(&mut self.sources).entry(source_id.to_owned()).or_default())
    }

    /// Drops the source's entry when it holds no state.
    fn prune(&mut self, source_id: &str) {
        if self.layers(source_id).is_some_and(LayerFeatureStates::is_empty) {
            Arc::make_mut(&mut self.sources).remove(source_id);
        }
    }

    /// Merges `state` into the feature's state. Returns whether anything
    /// changed.
    pub(crate) fn set(
        &mut self,
        source_id: &str,
        source_layer_id: &str,
        feature_id: &str,
        state: FeatureState,
    ) -> bool {
        let current = self.layers(source_id).and_then(|layers| layers.get(source_layer_id)?.get(feature_id));
        let changed: FeatureState = state
            .into_iter()
            .filter(|(key, value)| current.and_then(|current| current.get(key)) != Some(value))
            .collect();
        if changed.is_empty() {
            return false;
        }
        let feature = self
            .layers_mut(source_id)
            .entry(source_layer_id.to_owned())
            .or_default()
            .entry(feature_id.to_owned())
            .or_default();
        for (key, value) in changed {
            feature.insert(key, value);
        }
        true
    }

    /// The feature's state, empty when it has none.
    pub(crate) fn get(&self, source_id: &str, source_layer_id: &str, feature_id: &str) -> FeatureState {
        self.layers(source_id)
            .and_then(|layers| layers.get(source_layer_id)?.get(feature_id))
            .cloned()
            .unwrap_or_default()
    }

    /// Removes the state of the feature `feature_id`, only its `state_key`
    /// when there is one, or of every feature in the source layer without
    /// `feature_id`. Returns whether anything changed.
    pub(crate) fn remove(
        &mut self,
        source_id: &str,
        source_layer_id: &str,
        feature_id: Option<&str>,
        state_key: Option<&str>,
    ) -> bool {
        let Some(features) = self.layers(source_id).and_then(|layers| layers.get(source_layer_id)) else {
            return false;
        };
        let present = match (feature_id, state_key) {
            (None, _) => true,
            (Some(feature_id), None) => features.contains_key(feature_id),
            (Some(feature_id), Some(key)) => features.get(feature_id).is_some_and(|state| state.contains_key(key)),
        };
        if !present {
            return false;
        }
        let layers = self.layers_mut(source_id);
        if let (Some(feature_id), Some(features)) = (feature_id, layers.get_mut(source_layer_id)) {
            match state_key {
                None => {
                    features.remove(feature_id);
                }
                Some(key) => {
                    if let Some(state) = features.get_mut(feature_id) {
                        state.remove(key);
                        if state.is_empty() {
                            features.remove(feature_id);
                        }
                    }
                }
            }
            if features.is_empty() {
                layers.remove(source_layer_id);
            }
        } else {
            layers.remove(source_layer_id);
        }
        self.prune(source_id);
        true
    }

    /// Removes the state of every feature of the source `source_id`.
    pub(crate) fn remove_source(&mut self, source_id: &str) -> bool {
        self.sources.contains_key(source_id) && Arc::make_mut(&mut self.sources).remove(source_id).is_some()
    }

    /// Removes every feature's state.
    pub(crate) fn clear(&mut self) -> bool {
        if self.sources.is_empty() {
            return false;
        }
        self.sources = Arc::default();
        true
    }
}

#[cfg(test)]
mod tests {
    use feature::Value;

    use super::*;

    fn state(entries: &[(&str, Value)]) -> FeatureState {
        entries.iter().map(|(key, value)| ((*key).to_owned(), value.clone())).collect()
    }

    #[test]
    fn state_merges_on_set_and_prunes_on_remove() {
        let mut store = FeatureStateStore::default();
        assert!(store.set("points", "", "a", state(&[("hover", Value::Bool(true))])));
        assert!(store.set("points", "", "a", state(&[("count", Value::Uint(3))])));
        assert!(!store.set("points", "", "a", state(&[("count", Value::Uint(3))])));
        assert!(!store.set("points", "", "a", FeatureState::new()));
        assert_eq!(state(&[("hover", Value::Bool(true)), ("count", Value::Uint(3))]), store.get("points", "", "a"));

        assert!(store.remove("points", "", Some("a"), Some("hover")));
        assert!(!store.remove("points", "", Some("a"), Some("hover")));
        assert!(store.remove("points", "", Some("a"), Some("count")));
        assert!(store.shared().is_empty());
        assert!(!store.remove("points", "", Some("a"), None));
        assert!(!store.remove("points", "", None, None));
    }

    #[test]
    fn a_removal_without_a_feature_takes_its_source_layer() {
        let mut store = FeatureStateStore::default();
        for layer in ["roads", "water", ""] {
            store.set("tiles", layer, "1", state(&[("n", Value::Uint(1))]));
        }
        assert!(store.remove("tiles", "roads", None, None));
        assert!(store.get("tiles", "roads", "1").is_empty());
        assert!(!store.get("tiles", "water", "1").is_empty());
        assert!(store.remove_source("tiles"));
        assert!(!store.remove_source("tiles"));
        assert!(!store.clear());
    }

    #[test]
    fn a_change_shares_the_sources_it_leaves_alone() {
        let mut store = FeatureStateStore::default();
        store.set("a", "", "1", state(&[("n", Value::Uint(1))]));
        store.set("b", "", "1", state(&[("n", Value::Uint(1))]));
        let before = store.shared();
        // Nothing changes, so the update's state stays the same.
        store.set("a", "", "1", state(&[("n", Value::Uint(1))]));
        store.remove("a", "", Some("2"), None);
        assert!(Arc::ptr_eq(&before, &store.shared()));

        store.set("a", "", "1", state(&[("n", Value::Uint(2))]));
        let after = store.shared();
        assert!(Arc::ptr_eq(&before["b"], &after["b"]));
        assert!(!Arc::ptr_eq(&before["a"], &after["a"]));
        assert_eq!(Some(&Value::Uint(1)), before["a"][""]["1"].get("n"));
    }
}
