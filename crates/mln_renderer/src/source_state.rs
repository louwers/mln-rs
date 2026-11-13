// `coalesceChanges(std::vector<RenderTile>&)` calls `tile.setFeatureState`
// on every render tile; a Rust RenderTile has no reference to its Tile, so
// `coalesce_changes` takes the tiles of the render source's pyramid (the same
// tiles).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use mln_style::expression::expression::FeatureState;
use mln_vec_map::VecMap;

use crate::bucket::{FeatureStates, LayerFeatureStates};
use crate::tile::tile::Tile;

/// The feature state of a map's sources, as the map's updates carry it: by
/// source ID, the state of each source layer's features, where the empty
/// source layer stands for none. A source without state has no entry, and no
/// entry holds an empty state. Each source's state is shared, so an update
/// shares the state of the sources it leaves alone with the update before.
pub type FeatureStatesBySource = BTreeMap<String, Arc<LayerFeatureStates>>;

#[derive(Debug, Default)]
pub(crate) struct SourceFeatureState {
    current_states: LayerFeatureStates,
    state_changes: LayerFeatureStates,
    deleted_states: LayerFeatureStates,
    /// Goes up in each `coalesce_changes` call that has changes.
    version: u64,
    /// The state that [`set_states`](Self::set_states) last gave the source.
    given: Option<Arc<LayerFeatureStates>>,
}

impl SourceFeatureState {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn update_state(&mut self, source_layer_id: Option<&str>, feature_id: &str, new_state: &FeatureState) {
        let source_layer = source_layer_id.unwrap_or("").to_owned();
        for (key, value) in new_state {
            let layer_states = self.state_changes.entry(source_layer.clone()).or_default();
            let feature_states = layer_states.entry(feature_id.to_owned()).or_default();
            feature_states.insert(key.clone(), value.clone());
        }
        // `coalesce_changes` applies removals after changes, so a removal
        // queued before this change would undo it.
        self.cancel_queued_removals(&source_layer, feature_id, new_state);
    }

    /// Narrows the queued removals of `feature_id` so they don't cover the
    /// keys of `new_state`. A removal of the whole layer or feature becomes a
    /// removal of the other current state.
    fn cancel_queued_removals(&mut self, source_layer: &str, feature_id: &str, new_state: &FeatureState) {
        let Some(feature_deletions) = self.deleted_states.get_mut(source_layer) else {
            return;
        };
        let current_layer = self.current_states.get(source_layer);
        let removal_of_keys_not_in_new_state = || -> FeatureState {
            current_layer
                .and_then(|layer| layer.get(feature_id))
                .into_iter()
                .flat_map(|current| current.keys())
                .filter(|key| !new_state.contains_key(*key))
                .map(|key| (key.clone(), feature::Value::default()))
                .collect()
        };
        if feature_deletions.is_empty() {
            for other in current_layer.into_iter().flat_map(|layer| layer.keys()) {
                if other != feature_id {
                    feature_deletions.insert(other.clone(), FeatureState::new());
                }
            }
            let removal = removal_of_keys_not_in_new_state();
            if !removal.is_empty() {
                feature_deletions.insert(feature_id.to_owned(), removal);
            }
        } else if let Some(deleted_keys) = feature_deletions.get_mut(feature_id) {
            if deleted_keys.is_empty() {
                *deleted_keys = removal_of_keys_not_in_new_state();
            } else {
                for key in new_state.keys() {
                    deleted_keys.remove(key);
                }
            }
            if deleted_keys.is_empty() {
                feature_deletions.remove(feature_id);
            }
        }
        if feature_deletions.is_empty() {
            self.deleted_states.remove(source_layer);
        }
    }

    /// Makes the source's state `states`, `None` for no state, as of the next
    /// `coalesce_changes`, in place of whatever it held or had queued. A
    /// source given the same `states` again changes nothing, so a map's
    /// update can give every source its state each time.
    pub(crate) fn set_states(&mut self, states: Option<&Arc<LayerFeatureStates>>) {
        let unchanged = match (&self.given, states) {
            (None, None) => true,
            (Some(given), Some(states)) => Arc::ptr_eq(given, states),
            _ => false,
        };
        if unchanged {
            return;
        }
        self.given = states.cloned();
        self.state_changes.clear();
        self.deleted_states.clear();
        let empty = LayerFeatureStates::new();
        let target = states.map_or(&empty, |states| &**states);
        let target_state = |source_layer: &str, feature_id: &str| {
            target.get(source_layer).and_then(|layer| layer.get(feature_id)).filter(|state| !state.is_empty())
        };

        // The keys that change or go, as changes and removals of keys that
        // `coalesce_changes` applies to the current state.
        for (source_layer, features) in target {
            let current_layer = self.current_states.get(source_layer);
            for (feature_id, state) in features {
                let current = current_layer.and_then(|layer| layer.get(feature_id));
                let changed: FeatureState = state
                    .iter()
                    .filter(|(key, value)| current.and_then(|current| current.get(*key)) != Some(*value))
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect();
                if !changed.is_empty() {
                    self.state_changes.entry(source_layer.clone()).or_default().insert(feature_id.clone(), changed);
                }
                let removed: FeatureState = current
                    .into_iter()
                    .flat_map(|current| current.keys())
                    .filter(|key| !state.contains_key(*key))
                    .map(|key| (key.clone(), feature::Value::default()))
                    .collect();
                // An empty state removes every key below.
                if !removed.is_empty() && !state.is_empty() {
                    self.deleted_states.entry(source_layer.clone()).or_default().insert(feature_id.clone(), removed);
                }
            }
        }
        // The features whose state goes whole. An empty removal map would
        // remove the whole source layer, so each entry names its feature.
        for (source_layer, features) in &self.current_states {
            for (feature_id, current) in features {
                if !current.is_empty() && target_state(source_layer, feature_id).is_none() {
                    self.deleted_states
                        .entry(source_layer.clone())
                        .or_default()
                        .insert(feature_id.clone(), FeatureState::new());
                }
            }
        }
    }

    #[must_use]
    pub(crate) fn get_state(&self, source_layer_id: Option<&str>, feature_id: &str) -> FeatureState {
        let source_layer = source_layer_id.unwrap_or("");
        let current = self
            .current_states
            .get(source_layer)
            .and_then(|layer_states| layer_states.get(feature_id))
            .cloned()
            .unwrap_or_default();
        let changes = self
            .state_changes
            .get(source_layer)
            .and_then(|layer_states| layer_states.get(feature_id))
            .cloned()
            .unwrap_or_default();
        // A queued removal never covers a key with a queued change, so the
        // result is the current state less the removals, plus the changes.
        let mut result = changes;
        let layer_deletions = self.deleted_states.get(source_layer);
        let deleted_keys = layer_deletions.and_then(|layer_deletions| layer_deletions.get(feature_id));
        let all_removed = layer_deletions.is_some_and(HashMap::is_empty) || deleted_keys.is_some_and(VecMap::is_empty);
        if !all_removed {
            for (key, value) in current {
                if !deleted_keys.is_some_and(|deleted_keys| deleted_keys.contains_key(&key)) {
                    result.entry(key).or_insert(value);
                }
            }
        }
        result
    }

    /// Folds the queued changes and deletions into the current states and
    /// brings each tile's buckets up to date. A tile that holds the previous
    /// version gets this frame's changes; a tile with new buckets, or one that
    /// was not rendered in a frame with changes, gets the whole state; a tile
    /// that is up to date gets nothing.
    pub(crate) fn coalesce_changes<'a>(&mut self, tiles: impl IntoIterator<Item = &'a dyn Tile>) {
        let mut changes: LayerFeatureStates = LayerFeatureStates::new();
        let state_changes = std::mem::take(&mut self.state_changes);
        for (source_layer, layer_state_changes) in &state_changes {
            let layer_states = changes.entry(source_layer.clone()).or_default();
            for (feature_id, feature_state_changes) in layer_state_changes {
                let current =
                    self.current_states.entry(source_layer.clone()).or_default().entry(feature_id.clone()).or_default();
                for (state_key, state_val) in feature_state_changes {
                    current.insert(state_key.clone(), state_val.clone());
                }
                layer_states.insert(feature_id.clone(), current.clone());
            }
        }

        let deleted_states = std::mem::take(&mut self.deleted_states);
        for (source_layer, layer_deleted_states) in &deleted_states {
            let layer_states = changes.entry(source_layer.clone()).or_default();
            let current_layer = self.current_states.entry(source_layer.clone()).or_default();
            if layer_deleted_states.is_empty() {
                for (feature_id, current) in current_layer.iter_mut() {
                    layer_states.insert(feature_id.clone(), FeatureState::new());
                    *current = FeatureState::new();
                }
            } else {
                for (feature_id, deleted_keys) in layer_deleted_states {
                    let delete_whole_feature_state = deleted_keys.is_empty();
                    let current = current_layer.entry(feature_id.clone()).or_default();
                    if delete_whole_feature_state {
                        *current = FeatureState::new();
                    } else {
                        for state_key in deleted_keys.keys() {
                            current.remove(state_key);
                        }
                    }
                    layer_states.insert(feature_id.clone(), current.clone());
                }
            }
        }

        if !changes.is_empty() {
            self.version += 1;
        }

        for tile in tiles {
            let tile_version = &tile.base().feature_state_version;
            if tile_version.get() == self.version {
                continue;
            }
            let previous = !changes.is_empty() && tile_version.get() != 0 && tile_version.get() + 1 == self.version;
            tile.set_feature_state(if previous { &changes } else { &self.current_states });
            tile_version.set(self.version);
        }
    }

    pub(crate) fn remove_state(
        &mut self,
        source_layer_id: Option<&str>,
        feature_id: Option<&str>,
        state_key: Option<&str>,
    ) {
        let source_layer = source_layer_id.unwrap_or("");

        let layer_deletions = self.deleted_states.get(source_layer);
        if layer_deletions.is_some_and(HashMap::is_empty) {
            return;
        }

        match (feature_id, state_key) {
            (Some(feature_id), Some(state_key)) => {
                let feature_deleted = layer_deletions
                    .and_then(|layer_deletions| layer_deletions.get(feature_id))
                    .is_some_and(VecMap::is_empty);
                if feature_deleted {
                    return;
                }
                self.deleted_states
                    .entry(source_layer.to_owned())
                    .or_default()
                    .entry(feature_id.to_owned())
                    .or_default()
                    .insert(state_key.to_owned(), feature::Value::default());
            }
            (Some(feature_id), None) => {
                self.deleted_states
                    .entry(source_layer.to_owned())
                    .or_default()
                    .insert(feature_id.to_owned(), FeatureState::new());
            }
            (None, _) => {
                self.deleted_states.insert(source_layer.to_owned(), FeatureStates::new());
            }
        }
        // A later change turns this removal into removals of the current
        // state, which don't cover queued changes.
        self.cancel_queued_changes(source_layer, feature_id, state_key);
    }

    /// Drops the queued changes that a removal covers.
    fn cancel_queued_changes(&mut self, source_layer: &str, feature_id: Option<&str>, state_key: Option<&str>) {
        let Some(feature_id) = feature_id else {
            self.state_changes.remove(source_layer);
            return;
        };
        let Some(layer_changes) = self.state_changes.get_mut(source_layer) else {
            return;
        };
        match state_key {
            Some(state_key) => {
                if let Some(feature_changes) = layer_changes.get_mut(feature_id) {
                    feature_changes.remove(state_key);
                    if feature_changes.is_empty() {
                        layer_changes.remove(feature_id);
                    }
                }
            }
            None => {
                layer_changes.remove(feature_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use feature::Value;
    use mln_style::layer_properties::LayerProperties;
    use mln_tile_id::OverscaledTileID;

    use super::*;
    use crate::tile::tile::{Kind, TileBase};
    use crate::tile_render_data::TileRenderData;

    struct StubTile {
        base: TileBase,
        applied: RefCell<Vec<LayerFeatureStates>>,
    }

    impl StubTile {
        fn new(x: u32) -> Self {
            let base = TileBase::new(Kind::Geometry, OverscaledTileID::from_zxy(1, x, 0), "source".to_owned(), None);
            StubTile { base, applied: RefCell::default() }
        }

        fn applied(&self, i: usize) -> LayerFeatureStates {
            self.applied.borrow()[i].clone()
        }

        fn applied_count(&self) -> usize {
            self.applied.borrow().len()
        }

        /// The states that the tile was given, each feature's last.
        fn drawn(&self) -> FeatureStates {
            let mut drawn = FeatureStates::new();
            for applied in self.applied.borrow().iter() {
                drawn.extend(applied.get("layer").cloned().unwrap_or_default());
            }
            drawn.retain(|_, state| !state.is_empty());
            drawn
        }
    }

    /// The options for one feature's state that `states_of` combines.
    fn feature_states() -> Vec<FeatureState> {
        let entries: [&[(&str, &str)]; 5] =
            [&[], &[("a", "x")], &[("a", "y")], &[("a", "x"), ("b", "x")], &[("b", "y")]];
        entries
            .iter()
            .map(|entries| {
                entries.iter().map(|(key, text)| ((*key).to_owned(), Value::String((*text).to_owned()))).collect()
            })
            .collect()
    }

    /// The source layer `layer` whose features take `states`, as a map's
    /// update carries it, and its features.
    fn states_of(states: &[&FeatureState]) -> (Arc<LayerFeatureStates>, FeatureStates) {
        let features: FeatureStates = FEATURES
            .iter()
            .zip(states)
            .filter(|(_, state)| !state.is_empty())
            .map(|(feature, state)| ((*feature).to_owned(), (*state).clone()))
            .collect();
        let layers = if features.is_empty() {
            LayerFeatureStates::new()
        } else {
            [("layer".to_owned(), features.clone())].into_iter().collect()
        };
        (Arc::new(layers), features)
    }

    impl Tile for StubTile {
        fn base(&self) -> &TileBase {
            &self.base
        }
        fn create_render_data(&self) -> Box<dyn TileRenderData> {
            unreachable!()
        }
        fn cancel(&self) {}
        fn layer_properties_updated(&self, _layer_properties: &Arc<dyn LayerProperties>) -> bool {
            false
        }
        fn set_feature_state(&self, states: &LayerFeatureStates) {
            self.applied.borrow_mut().push(states.clone());
        }
    }

    fn hover() -> FeatureState {
        let mut state = FeatureState::new();
        state.insert("hover".to_owned(), Value::Bool(true));
        state
    }

    fn render_frame(state: &mut SourceFeatureState, tiles: &[&dyn Tile]) {
        state.coalesce_changes(tiles.iter().copied());
    }

    // gtest: SourceFeatureState.CoalesceChanges
    #[test]
    fn coalesce_changes() {
        let mut state = SourceFeatureState::new();
        let a = StubTile::new(0);
        let b = StubTile::new(1);
        let layer = Some("layer");

        state.update_state(layer, "1", &hover());
        render_frame(&mut state, &[&a]);
        assert_eq!(a.applied_count(), 1);
        assert_eq!(a.applied(0)["layer"]["1"].get("hover"), Some(&Value::Bool(true)));

        // Nothing changed: nothing is applied again.
        render_frame(&mut state, &[&a]);
        assert_eq!(a.applied_count(), 1);

        // A tile rendered for the first time gets the stored state.
        render_frame(&mut state, &[&a, &b]);
        assert_eq!(b.applied_count(), 1);
        assert_eq!(b.applied(0)["layer"]["1"].get("hover"), Some(&Value::Bool(true)));
        assert_eq!(a.applied_count(), 1);

        // A tile that is up to date gets only the change.
        state.update_state(layer, "2", &hover());
        render_frame(&mut state, &[&a]);
        assert_eq!(a.applied_count(), 2);
        assert_eq!(a.applied(1)["layer"].len(), 1);
        assert_eq!(a.applied(1)["layer"]["2"].get("hover"), Some(&Value::Bool(true)));

        // A tile that was not rendered when the state changed gets the whole
        // state.
        render_frame(&mut state, &[&a, &b]);
        assert_eq!(a.applied_count(), 2);
        assert_eq!(b.applied_count(), 2);
        assert_eq!(b.applied(1)["layer"].len(), 2);

        // A removal and a change in the same frame both reach the tile.
        state.remove_state(layer, Some("1"), None);
        state.update_state(layer, "3", &hover());
        render_frame(&mut state, &[&a, &b]);
        assert_eq!(a.applied_count(), 3);
        assert!(a.applied(2)["layer"]["1"].is_empty());
        assert_eq!(a.applied(2)["layer"]["3"].get("hover"), Some(&Value::Bool(true)));
    }

    // A re-laid-out tile has new buckets and gets the whole state again.
    #[test]
    fn coalesce_changes_after_relayout() {
        let mut state = SourceFeatureState::new();
        let a = StubTile::new(0);
        let layer = Some("layer");

        state.update_state(layer, "1", &hover());
        render_frame(&mut state, &[&a]);
        state.update_state(layer, "2", &hover());
        a.base.feature_state_version.set(0);
        render_frame(&mut state, &[&a]);
        assert_eq!(a.applied_count(), 2);
        assert_eq!(a.applied(1)["layer"].len(), 2);
    }

    #[derive(Clone, Copy, Debug)]
    enum Op {
        Set(&'static str, &'static str),
        RemoveKey(&'static str, &'static str),
        RemoveFeature(&'static str),
        RemoveLayer,
    }

    const FEATURES: [&str; 2] = ["1", "2"];
    const KEYS: [&str; 2] = ["a", "b"];

    fn ops() -> Vec<Op> {
        let mut ops = vec![Op::RemoveLayer];
        for feature in FEATURES {
            ops.push(Op::RemoveFeature(feature));
            for key in KEYS {
                ops.push(Op::Set(feature, key));
                ops.push(Op::RemoveKey(feature, key));
            }
        }
        ops
    }

    fn apply(state: &mut SourceFeatureState, expected: &mut FeatureStates, op: Op) {
        let layer = Some("layer");
        match op {
            Op::Set(feature, key) => {
                let mut new_state = FeatureState::new();
                new_state.insert(key.to_owned(), Value::String(format!("{feature}{key}")));
                state.update_state(layer, feature, &new_state);
                expected.entry(feature.to_owned()).or_default().extend(new_state);
            }
            Op::RemoveKey(feature, key) => {
                state.remove_state(layer, Some(feature), Some(key));
                if let Some(feature_state) = expected.get_mut(feature) {
                    feature_state.remove(key);
                }
            }
            Op::RemoveFeature(feature) => {
                state.remove_state(layer, Some(feature), None);
                expected.remove(feature);
            }
            Op::RemoveLayer => {
                state.remove_state(layer, None, None);
                expected.clear();
            }
        }
    }

    fn assert_state(state: &SourceFeatureState, expected: &FeatureStates, context: &str) {
        for feature in FEATURES {
            let expected = expected.get(feature).cloned().unwrap_or_default();
            assert_eq!(state.get_state(Some("layer"), feature), expected, "feature {feature}, {context}");
        }
    }

    // Every sequence of three calls before one render ends in the state the
    // calls would leave if each applied at once, both read back before the
    // render and drawn by a tile that holds the previous frame.
    #[test]
    fn changes_and_removals_apply_in_call_order() {
        let ops = ops();
        for &first in &ops {
            for &second in &ops {
                for &third in &ops {
                    let sequence = [first, second, third];
                    let mut state = SourceFeatureState::new();
                    let mut expected = FeatureStates::new();
                    let tile = StubTile::new(0);
                    apply(&mut state, &mut expected, Op::Set("1", "a"));
                    apply(&mut state, &mut expected, Op::Set("1", "b"));
                    apply(&mut state, &mut expected, Op::Set("2", "a"));
                    render_frame(&mut state, &[&tile]);

                    for op in sequence {
                        apply(&mut state, &mut expected, op);
                    }
                    assert_state(&state, &expected, &format!("before rendering {sequence:?}"));

                    render_frame(&mut state, &[&tile]);
                    assert_state(&state, &expected, &format!("after rendering {sequence:?}"));
                    let mut drawn = tile.applied(0).remove("layer").unwrap_or_default();
                    if tile.applied_count() > 1 {
                        drawn.extend(tile.applied(1).remove("layer").unwrap_or_default());
                    }
                    for feature in FEATURES {
                        let expected = expected.get(feature).cloned().unwrap_or_default();
                        let drawn = drawn.get(feature).cloned().unwrap_or_default();
                        assert_eq!(drawn, expected, "tile's feature {feature} after {sequence:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn given_states_replace_what_the_source_held() {
        let mut state = SourceFeatureState::new();
        let tile = StubTile::new(0);
        let (first, _) = states_of(&[&hover(), &hover()]);
        state.set_states(Some(&first));
        render_frame(&mut state, &[&tile]);
        assert_eq!(1, tile.applied_count());
        assert_eq!(2, tile.drawn().len());

        // The same state again changes nothing.
        state.set_states(Some(&first));
        render_frame(&mut state, &[&tile]);
        assert_eq!(1, tile.applied_count());

        // Feature 2 loses its state, and the tile gets only that change.
        let (second, _) = states_of(&[&hover(), &FeatureState::new()]);
        state.set_states(Some(&second));
        assert!(state.get_state(Some("layer"), "2").is_empty());
        render_frame(&mut state, &[&tile]);
        assert_eq!(2, tile.applied_count());
        assert_eq!(vec!["2"], tile.applied(1)["layer"].keys().map(String::as_str).collect::<Vec<_>>());
        assert_eq!(1, tile.drawn().len());

        // No state takes the rest, and a tile seen for the first time draws
        // none.
        state.set_states(None);
        let fresh = StubTile::new(1);
        render_frame(&mut state, &[&tile, &fresh]);
        assert!(tile.drawn().is_empty());
        assert!(fresh.drawn().is_empty());
        assert!(state.get_state(Some("layer"), "1").is_empty());
    }

    // From every state to every other, with a queued call of the renderer's
    // own API in between, the given state is the one read back before the
    // render and drawn after it.
    #[test]
    fn given_states_win_over_what_was_queued() {
        let options = feature_states();
        let targets: Vec<_> =
            options.iter().flat_map(|one| options.iter().map(move |two| states_of(&[one, two]))).collect();
        for (first, _) in &targets {
            for op in ops() {
                for (second, expected) in &targets {
                    let mut state = SourceFeatureState::new();
                    let tile = StubTile::new(0);
                    state.set_states(Some(first));
                    render_frame(&mut state, &[&tile]);
                    apply(&mut state, &mut FeatureStates::new(), op);
                    // A copy, as the same state again changes nothing.
                    state.set_states(Some(&Arc::new((**second).clone())));
                    let context = format!("from {first:?} by {op:?} to {second:?}");
                    assert_state(&state, expected, &format!("before rendering, {context}"));
                    render_frame(&mut state, &[&tile]);
                    assert_state(&state, expected, &format!("after rendering, {context}"));
                    assert_eq!(&tile.drawn(), expected, "drawn, {context}");
                }
            }
        }
    }
}
