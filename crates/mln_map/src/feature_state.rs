//! The feature state that a map keeps for its sources' features, and sends to
//! its renderer with each update.

use std::collections::BTreeMap;
use std::sync::Arc;

use mln_renderer::bucket::LayerFeatureStates;
use mln_renderer::source_state::{FeatureStatesBySource, SourceFeatureStates};
use mln_style::expression::expression::FeatureState;

/// A map's feature state, by source, source layer and feature ID, where the
/// empty source layer stands for none. It keeps no empty entry, so a removal
/// leaves no trace.
///
/// An update carries each source's state as a [`SourceFeatureStates`]: a
/// whole state and the changes since it, so a change costs the renderer only
/// that change. Once a source's changes outnumber the features with state,
/// its next change starts again from the whole state, which bounds both the
/// changes kept and the work a renderer that missed them redoes.
#[derive(Clone, Debug, Default)]
pub(crate) struct FeatureStateStore {
    sources: BTreeMap<String, Source>,
    /// The state as updates carry it, shared until the store changes.
    shared: Arc<FeatureStatesBySource>,
}

#[derive(Clone, Debug, Default)]
struct Source {
    states: LayerFeatureStates,
    /// The features with state.
    features: usize,
    /// The changes since the whole state in `shared`.
    changes: usize,
}

impl FeatureStateStore {
    /// The state as an update carries it.
    pub(crate) fn shared(&self) -> Arc<FeatureStatesBySource> {
        Arc::clone(&self.shared)
    }

    fn feature(&self, source_id: &str, source_layer_id: &str, feature_id: &str) -> Option<&FeatureState> {
        self.sources.get(source_id)?.states.get(source_layer_id)?.get(feature_id)
    }

    /// Publishes the feature's state in `source`, as it now is, in `shared`.
    fn publish(
        shared: &mut Arc<FeatureStatesBySource>,
        source_id: &str,
        source: &mut Source,
        source_layer_id: &str,
        feature_id: &str,
    ) {
        let state = source.states.get(source_layer_id).and_then(|layer| layer.get(feature_id)).cloned();
        let shared = Arc::make_mut(shared);
        let published = match shared.get(source_id) {
            Some(published) if source.changes < source.features.max(16) => {
                source.changes += 1;
                published.with_change(source_layer_id, feature_id, state.unwrap_or_default())
            }
            _ => {
                source.changes = 0;
                SourceFeatureStates::new(source.states.clone())
            }
        };
        shared.insert(source_id.to_owned(), published);
    }

    /// Drops the source's entry when it holds no state.
    fn prune(&mut self, source_id: &str) {
        if self.sources.get(source_id).is_some_and(|source| source.features == 0) {
            self.sources.remove(source_id);
            Arc::make_mut(&mut self.shared).remove(source_id);
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
        let current = self.feature(source_id, source_layer_id, feature_id);
        let changed: FeatureState = state
            .into_iter()
            .filter(|(key, value)| current.and_then(|current| current.get(key)) != Some(value))
            .collect();
        if changed.is_empty() {
            return false;
        }
        let source = self.sources.entry(source_id.to_owned()).or_default();
        let layer = source.states.entry(source_layer_id.to_owned()).or_default();
        if !layer.contains_key(feature_id) {
            source.features += 1;
        }
        let feature = layer.entry(feature_id.to_owned()).or_default();
        for (key, value) in changed {
            feature.insert(key, value);
        }
        Self::publish(&mut self.shared, source_id, source, source_layer_id, feature_id);
        true
    }

    /// The feature's state, empty when it has none.
    pub(crate) fn get(&self, source_id: &str, source_layer_id: &str, feature_id: &str) -> FeatureState {
        self.feature(source_id, source_layer_id, feature_id).cloned().unwrap_or_default()
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
        let Some(source) = self.sources.get_mut(source_id) else {
            return false;
        };
        let Some(features) = source.states.get_mut(source_layer_id) else {
            return false;
        };
        let removed: Vec<String> = match (feature_id, state_key) {
            (None, _) => {
                let removed: Vec<String> = features.keys().cloned().collect();
                source.states.remove(source_layer_id);
                removed
            }
            (Some(feature_id), None) => {
                if features.remove(feature_id).is_none() {
                    return false;
                }
                vec![feature_id.to_owned()]
            }
            (Some(feature_id), Some(key)) => {
                let Some(state) = features.get_mut(feature_id) else {
                    return false;
                };
                if state.remove(key).is_none() {
                    return false;
                }
                if state.is_empty() {
                    features.remove(feature_id);
                    vec![feature_id.to_owned()]
                } else {
                    // The feature keeps state: publish its change alone.
                    Self::publish(&mut self.shared, source_id, source, source_layer_id, feature_id);
                    return true;
                }
            }
        };
        if source.states.get(source_layer_id).is_some_and(std::collections::HashMap::is_empty) {
            source.states.remove(source_layer_id);
        }
        source.features -= removed.len();
        if source.features == 0 {
            self.prune(source_id);
        } else {
            for feature_id in &removed {
                Self::publish(&mut self.shared, source_id, source, source_layer_id, feature_id);
            }
        }
        true
    }

    /// Removes the state of every feature of the source `source_id`.
    pub(crate) fn remove_source(&mut self, source_id: &str) -> bool {
        if self.sources.remove(source_id).is_none() {
            return false;
        }
        Arc::make_mut(&mut self.shared).remove(source_id);
        true
    }

    /// Removes every feature's state.
    pub(crate) fn clear(&mut self) -> bool {
        if self.sources.is_empty() {
            return false;
        }
        *self = FeatureStateStore::default();
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
        assert!(before["b"].ptr_eq(&after["b"]));
        assert!(!before["a"].ptr_eq(&after["a"]));
        assert_eq!(Some(&Value::Uint(1)), before["a"].get("", "1").get("n"));
        assert_eq!(Some(&Value::Uint(2)), after["a"].get("", "1").get("n"));
    }

    // A source's published state reads as the store does through every
    // change, including those past the point where it starts again from
    // the whole state.
    #[test]
    fn published_state_follows_the_store() {
        let mut store = FeatureStateStore::default();
        let check = |store: &FeatureStateStore| {
            let shared = store.shared();
            for layer in ["", "roads"] {
                for id in 0..40 {
                    let id = id.to_string();
                    let published = shared.get("s").map(|s| s.get(layer, &id)).unwrap_or_default();
                    assert_eq!(store.get("s", layer, &id), published, "{layer}/{id}");
                }
            }
        };
        for round in 0..200_u64 {
            let id = (round * 7 % 40).to_string();
            let layer = if round % 3 == 0 { "roads" } else { "" };
            match round % 5 {
                0..=2 => {
                    store.set("s", layer, &id, state(&[("n", Value::Uint(round)), ("k", Value::Uint(round % 2))]));
                }
                3 => {
                    store.remove("s", layer, Some(&id), Some("k"));
                }
                _ => {
                    store.remove("s", layer, Some(&id), None);
                }
            }
            check(&store);
        }
        store.remove("s", "roads", None, None);
        check(&store);
        store.remove("s", "", None, None);
        check(&store);
        assert!(store.shared().is_empty());
    }
}
