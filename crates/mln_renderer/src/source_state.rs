// `coalesceChanges(std::vector<RenderTile>&)` calls `tile.setFeatureState`
// on every render tile; a Rust RenderTile has no reference to its Tile, so
// `coalesce_changes` takes the tiles of the render source's pyramid (the same
// tiles).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock};

use mln_style::expression::expression::FeatureState;
use mln_vec_map::VecMap;

use crate::bucket::{FeatureStates, LayerFeatureStates};
use crate::tile::tile::Tile;

/// The feature state of a map's sources, as the map's updates carry it: by
/// source ID, the state of each source layer's features, where the empty
/// source layer stands for none. A source without state has no entry. An
/// update shares the state of the sources it leaves alone with the update
/// before.
pub type FeatureStatesBySource = BTreeMap<String, SourceFeatureStates>;

/// A source's feature state, as of its newest change: a whole state, and the
/// changes made since it, newest first. A renderer that holds an older
/// value of the same source applies only the changes in between.
///
/// Cloning is cheap: the state and its changes are shared.
#[derive(Clone, Debug)]
pub struct SourceFeatureStates(Arc<StatesNode>);

#[derive(Debug)]
enum StatesNode {
    Whole(LayerFeatureStates),
    /// One feature's new state, empty for none.
    Change {
        source_layer: String,
        feature_id: String,
        state: FeatureState,
        previous: SourceFeatureStates,
    },
}

/// The empty whole state, which a dropping chain of changes leaves in each
/// node it takes apart.
static EMPTY: LazyLock<SourceFeatureStates> =
    LazyLock::new(|| SourceFeatureStates(Arc::new(StatesNode::Whole(LayerFeatureStates::new()))));

/// One feature's new state in a chain of changes.
struct Change<'a> {
    source_layer: &'a str,
    feature_id: &'a str,
    state: &'a FeatureState,
}

impl SourceFeatureStates {
    /// The state `states`, whole.
    #[must_use]
    pub fn new(states: LayerFeatureStates) -> Self {
        SourceFeatureStates(Arc::new(StatesNode::Whole(states)))
    }

    /// This state with the feature `feature_id` of `source_layer` taking
    /// `state`, the whole of its new state, empty for none.
    #[must_use]
    pub fn with_change(&self, source_layer: &str, feature_id: &str, state: FeatureState) -> Self {
        SourceFeatureStates(Arc::new(StatesNode::Change {
            source_layer: source_layer.to_owned(),
            feature_id: feature_id.to_owned(),
            state,
            previous: self.clone(),
        }))
    }

    /// Whether `self` and `other` are the same value, not just equal ones.
    #[must_use]
    pub fn ptr_eq(&self, other: &SourceFeatureStates) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// The feature's state, empty when it has none. Walks the changes since
    /// the whole state, so it costs as many steps as there are changes.
    #[must_use]
    pub fn get(&self, source_layer: &str, feature_id: &str) -> FeatureState {
        let mut node = self;
        loop {
            match &*node.0 {
                StatesNode::Whole(states) => {
                    return states
                        .get(source_layer)
                        .and_then(|layer| layer.get(feature_id))
                        .cloned()
                        .unwrap_or_default();
                }
                StatesNode::Change { source_layer: layer, feature_id: id, state, previous } => {
                    if layer == source_layer && id == feature_id {
                        return state.clone();
                    }
                    node = previous;
                }
            }
        }
    }

    /// The whole state this one builds on, and the changes since it, oldest
    /// first.
    fn base(&self) -> (&LayerFeatureStates, Vec<Change<'_>>) {
        let mut changes = Vec::new();
        let mut node = self;
        loop {
            match &*node.0 {
                StatesNode::Whole(states) => {
                    changes.reverse();
                    return (states, changes);
                }
                StatesNode::Change { source_layer, feature_id, state, previous } => {
                    changes.push(Change { source_layer, feature_id, state });
                    node = previous;
                }
            }
        }
    }

    /// The changes from `older` to this state, oldest first, or `None` when
    /// this state doesn't build on `older` through changes alone.
    fn changes_since(&self, older: &SourceFeatureStates) -> Option<Vec<Change<'_>>> {
        let mut changes = Vec::new();
        let mut node = self;
        while !Arc::ptr_eq(&node.0, &older.0) {
            let StatesNode::Change { source_layer, feature_id, state, previous } = &*node.0 else {
                return None;
            };
            changes.push(Change { source_layer, feature_id, state });
            node = previous;
        }
        changes.reverse();
        Some(changes)
    }
}

impl Drop for StatesNode {
    // A chain of changes drops one node at a time, not by recursion, which
    // a long chain would overflow the stack with.
    fn drop(&mut self) {
        let StatesNode::Change { previous, .. } = self else {
            return;
        };
        let mut next = std::mem::replace(previous, EMPTY.clone());
        // Each node this drop owns alone gives up its previous one before
        // it drops, so it drops without recursing.
        while let Ok(mut node) = Arc::try_unwrap(next.0) {
            let StatesNode::Change { previous, .. } = &mut node else {
                return;
            };
            next = std::mem::replace(previous, EMPTY.clone());
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct SourceFeatureState {
    current_states: LayerFeatureStates,
    state_changes: LayerFeatureStates,
    deleted_states: LayerFeatureStates,
    /// Goes up in each `coalesce_changes` call that has changes.
    version: u64,
    /// The state that [`set_states`](Self::set_states) last gave the source.
    given: Option<SourceFeatureStates>,
    /// Whether the renderer's own calls changed the state since the source
    /// was last given a state whole, so it no longer holds `given` alone.
    changed_since_given: bool,
}

impl SourceFeatureState {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn update_state(&mut self, source_layer_id: Option<&str>, feature_id: &str, new_state: &FeatureState) {
        self.changed_since_given = true;
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
    /// update can give every source its state each time. A state that builds
    /// on the one given before, by changes alone, costs only those changes.
    pub(crate) fn set_states(&mut self, states: Option<&SourceFeatureStates>) {
        let changes = match (&self.given, states) {
            (None, None) => return,
            (Some(given), Some(states)) if Arc::ptr_eq(&given.0, &states.0) => return,
            (Some(given), Some(states)) if !self.changed_since_given => states.changes_since(given),
            _ => None,
        };
        match changes {
            Some(changes) => {
                for change in changes {
                    self.retarget_feature(change.source_layer, change.feature_id, change.state);
                }
            }
            None => self.set_whole_states(states),
        }
        self.given = states.cloned();
    }

    /// Queues the changes and removals that take the current state to
    /// `states`, in place of whatever was queued.
    fn set_whole_states(&mut self, states: Option<&SourceFeatureStates>) {
        self.changed_since_given = false;
        self.state_changes.clear();
        self.deleted_states.clear();
        let empty = LayerFeatureStates::new();
        let (base, changes) = states.map_or((&empty, Vec::new()), SourceFeatureStates::base);
        // The features the changes leave alone take their base state, and
        // each changed feature its newest.
        let changed: HashMap<(&str, &str), &FeatureState> =
            changes.iter().map(|change| ((change.source_layer, change.feature_id), change.state)).collect();
        let target_state = |source_layer: &str, feature_id: &str| -> Option<&FeatureState> {
            match changed.get(&(source_layer, feature_id)) {
                Some(state) => Some(*state),
                None => base.get(source_layer).and_then(|layer| layer.get(feature_id)),
            }
            .filter(|state| !state.is_empty())
        };

        for (source_layer, features) in base {
            for (feature_id, state) in features {
                if !changed.contains_key(&(source_layer.as_str(), feature_id.as_str())) {
                    self.queue_feature_diff(source_layer, feature_id, state);
                }
            }
        }
        for (&(source_layer, feature_id), state) in &changed {
            self.queue_feature_diff(source_layer, feature_id, state);
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

    /// Queues what takes the feature's current state to `state`, which an
    /// empty state leaves to the removals of features whose state goes
    /// whole.
    fn queue_feature_diff(&mut self, source_layer: &str, feature_id: &str, state: &FeatureState) {
        let current = self.current_states.get(source_layer).and_then(|layer| layer.get(feature_id));
        let changed: FeatureState = state
            .iter()
            .filter(|(key, value)| current.and_then(|current| current.get(*key)) != Some(*value))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let removed: FeatureState = current
            .into_iter()
            .flat_map(|current| current.keys())
            .filter(|key| !state.contains_key(*key))
            .map(|key| (key.clone(), feature::Value::default()))
            .collect();
        if !changed.is_empty() {
            self.state_changes.entry(source_layer.to_owned()).or_default().insert(feature_id.to_owned(), changed);
        }
        // An empty state removes every key below.
        if !removed.is_empty() && !state.is_empty() {
            self.deleted_states.entry(source_layer.to_owned()).or_default().insert(feature_id.to_owned(), removed);
        }
    }

    /// Makes the feature's state `state`, empty for none, in place of what
    /// was queued for it. Only the map's states are queued here, so no
    /// removal covers a whole source layer.
    fn retarget_feature(&mut self, source_layer: &str, feature_id: &str, state: &FeatureState) {
        if let Some(layer) = self.state_changes.get_mut(source_layer) {
            layer.remove(feature_id);
            if layer.is_empty() {
                self.state_changes.remove(source_layer);
            }
        }
        if let Some(layer) = self.deleted_states.get_mut(source_layer) {
            layer.remove(feature_id);
            if layer.is_empty() {
                self.deleted_states.remove(source_layer);
            }
        }
        self.queue_feature_diff(source_layer, feature_id, state);
        let current = self.current_states.get(source_layer).and_then(|layer| layer.get(feature_id));
        if state.is_empty() && current.is_some_and(|current| !current.is_empty()) {
            self.deleted_states
                .entry(source_layer.to_owned())
                .or_default()
                .insert(feature_id.to_owned(), FeatureState::new());
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
        self.changed_since_given = true;
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
    fn states_of(states: &[&FeatureState]) -> (SourceFeatureStates, FeatureStates) {
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
        (SourceFeatureStates::new(layers), features)
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
                    let (copy, _) = states_of(&[&second.get("layer", "1"), &second.get("layer", "2")]);
                    state.set_states(Some(&copy));
                    let context = format!("from {first:?} by {op:?} to {second:?}");
                    assert_state(&state, expected, &format!("before rendering, {context}"));
                    render_frame(&mut state, &[&tile]);
                    assert_state(&state, expected, &format!("after rendering, {context}"));
                    assert_eq!(&tile.drawn(), expected, "drawn, {context}");
                }
            }
        }
    }

    /// `base` with each `(feature, state)` of `changes` applied as a change.
    fn changed(base: &SourceFeatureStates, changes: &[(&str, &FeatureState)]) -> SourceFeatureStates {
        changes
            .iter()
            .fold(base.clone(), |states, (feature, state)| states.with_change("layer", feature, (*state).clone()))
    }

    // From every state to every other through a chain of two changes, given
    // one at a time with or without a render between, or together, the
    // source holds and draws the newest state, as a whole state would leave
    // it.
    #[test]
    fn changes_since_the_given_state_apply_as_the_whole_state_would() {
        let options = feature_states();
        let starts: Vec<_> =
            options.iter().flat_map(|one| options.iter().map(move |two| states_of(&[one, two]))).collect();
        for (start, start_expected) in &starts {
            for first in FEATURES {
                for first_state in &options {
                    for second in FEATURES {
                        for second_state in &options {
                            for between in ["render", "no render", "together"] {
                                let mut state = SourceFeatureState::new();
                                let tile = StubTile::new(0);
                                state.set_states(Some(start));
                                render_frame(&mut state, &[&tile]);
                                let one = changed(start, &[(first, first_state)]);
                                if between != "together" {
                                    state.set_states(Some(&one));
                                }
                                if between == "render" {
                                    render_frame(&mut state, &[&tile]);
                                }
                                let two = changed(&one, &[(second, second_state)]);
                                state.set_states(Some(&two));
                                let mut expected = start_expected.clone();
                                for (feature, feature_state) in [(first, first_state), (second, second_state)] {
                                    if feature_state.is_empty() {
                                        expected.remove(feature);
                                    } else {
                                        expected.insert(feature.to_owned(), feature_state.clone());
                                    }
                                }
                                let context = format!(
                                    "from {start_expected:?}: {first} to {first_state:?}, then {second} to \
                                     {second_state:?}, {between}"
                                );
                                assert_state(&state, &expected, &format!("before rendering, {context}"));
                                render_frame(&mut state, &[&tile]);
                                assert_state(&state, &expected, &format!("after rendering, {context}"));
                                assert_eq!(tile.drawn(), expected, "drawn, {context}");
                            }
                        }
                    }
                }
            }
        }
    }

    // A tile that holds the previous frame gets only the changed feature.
    #[test]
    fn a_change_reaches_the_tiles_alone() {
        let mut state = SourceFeatureState::new();
        let tile = StubTile::new(0);
        let (start, _) = states_of(&[&hover(), &hover()]);
        state.set_states(Some(&start));
        render_frame(&mut state, &[&tile]);
        state.set_states(Some(&changed(&start, &[("2", &FeatureState::new())])));
        render_frame(&mut state, &[&tile]);
        assert_eq!(2, tile.applied_count());
        assert_eq!(vec!["2"], tile.applied(1)["layer"].keys().map(String::as_str).collect::<Vec<_>>());
    }

    // The map's state wins over the renderer's own calls, also when the
    // map's next state is only a change: the source takes it whole.
    #[test]
    fn a_change_after_the_renderers_own_call_takes_the_whole_state() {
        let mut state = SourceFeatureState::new();
        let tile = StubTile::new(0);
        let (start, _) = states_of(&[&hover(), &FeatureState::new()]);
        state.set_states(Some(&start));
        render_frame(&mut state, &[&tile]);
        state.update_state(Some("layer"), "2", &hover());
        render_frame(&mut state, &[&tile]);
        state.set_states(Some(&changed(&start, &[("1", &FeatureState::new())])));
        render_frame(&mut state, &[&tile]);
        assert!(tile.drawn().is_empty());
        assert!(state.get_state(Some("layer"), "2").is_empty());
    }

    // Dropping a long chain of changes doesn't overflow the stack.
    #[test]
    fn a_long_chain_of_changes_drops() {
        let mut states = SourceFeatureStates::new(LayerFeatureStates::new());
        for index in 0..1_000_000 {
            states = states.with_change("layer", "1", if index % 2 == 0 { hover() } else { FeatureState::new() });
        }
        drop(states);
    }
}
