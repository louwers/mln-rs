// Ports src/mln/renderer/source_state.hpp and src/mln/renderer/source_state.cpp
//
// `coalesceChanges(std::vector<RenderTile>&)` calls `tile.setFeatureState`
// on every render tile; a Rust RenderTile has no reference to its Tile, so
// `coalesce_changes` takes the tiles of the render source's pyramid (the same
// tiles).

use mln_style::expression::expression::FeatureState;

use crate::bucket::{FeatureStates, LayerFeatureStates};
use crate::tile::tile::Tile;

/// `SourceFeatureState`.
#[derive(Debug, Default)]
pub struct SourceFeatureState {
    current_states: LayerFeatureStates,
    state_changes: LayerFeatureStates,
    deleted_states: LayerFeatureStates,
    /// Goes up in each `coalesce_changes` call that has changes.
    version: u64,
}

impl SourceFeatureState {
    pub fn new() -> Self {
        Self::default()
    }

    /// `updateState(sourceLayerID, featureID, newState)`.
    pub fn update_state(&mut self, source_layer_id: Option<&str>, feature_id: &str, new_state: &FeatureState) {
        let source_layer = source_layer_id.unwrap_or("").to_owned();
        for (key, value) in new_state {
            let layer_states = self.state_changes.entry(source_layer.clone()).or_default();
            let feature_states = layer_states.entry(feature_id.to_owned()).or_default();
            feature_states.insert(key.clone(), value.clone());
        }
    }

    /// `getState(result, sourceLayerID, featureID)`.
    pub fn get_state(&self, source_layer_id: Option<&str>, feature_id: &str) -> FeatureState {
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
        // result = changes; result.insert(current.begin(), current.end()):
        // insert keeps the existing (changed) values.
        let mut result = changes;
        for (key, value) in current {
            result.entry(key).or_insert(value);
        }
        result
    }

    /// `coalesceChanges(tiles)`: folds the queued changes and deletions into
    /// the current states and brings each tile's buckets up to date. A tile
    /// that holds the previous version gets this frame's changes; a tile with
    /// new buckets, or one that was not rendered in a frame with changes,
    /// gets the whole state; a tile that is up to date gets nothing.
    pub fn coalesce_changes<'a>(&mut self, tiles: impl IntoIterator<Item = &'a dyn Tile>) {
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

    /// `removeState(sourceLayerID, featureID, stateKey)`.
    pub fn remove_state(&mut self, source_layer_id: Option<&str>, feature_id: Option<&str>, state_key: Option<&str>) {
        let source_layer = source_layer_id.unwrap_or("").to_owned();

        let source_layer_deleted =
            self.deleted_states.get(&source_layer).is_some_and(|layer_states| layer_states.is_empty());
        if source_layer_deleted {
            return;
        }

        match (state_key, feature_id) {
            (Some(state_key), Some(feature_id)) => {
                // `!deletedStates.contains(sourceLayer) &&
                // !deletedStates[sourceLayer].contains(featureID)`: the
                // second operand's operator[] inserts the layer; when the
                // layer is already there, nothing is recorded (C++ quirk).
                if !self.deleted_states.contains_key(&source_layer) {
                    let layer_states = self.deleted_states.entry(source_layer).or_default();
                    if !layer_states.contains_key(feature_id) {
                        layer_states
                            .entry(feature_id.to_owned())
                            .or_default()
                            .insert(state_key.to_owned(), Default::default());
                    }
                }
            }
            (None, Some(feature_id)) => {
                let update_in_queue = self
                    .state_changes
                    .get(&source_layer)
                    .is_some_and(|layer_states| layer_states.contains_key(feature_id));
                if update_in_queue {
                    let keys: Vec<String> = self.state_changes[&source_layer][feature_id].keys().cloned().collect();
                    let deleted =
                        self.deleted_states.entry(source_layer).or_default().entry(feature_id.to_owned()).or_default();
                    for key in keys {
                        deleted.insert(key, Default::default());
                    }
                } else {
                    self.deleted_states
                        .entry(source_layer)
                        .or_default()
                        .insert(feature_id.to_owned(), FeatureState::new());
                }
            }
            _ => {
                self.deleted_states.insert(source_layer, FeatureStates::new());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::any::Any;
    use std::cell::RefCell;

    use feature::Value;
    use mln_style::layer_properties::LayerProperties;
    use mln_tile_id::OverscaledTileID;
    use mln_util::immutable::Immutable;

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
    }

    impl Tile for StubTile {
        fn base(&self) -> &TileBase {
            &self.base
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn create_render_data(&self) -> Box<dyn TileRenderData> {
            unreachable!()
        }
        fn cancel(&self) {}
        fn layer_properties_updated(&self, _layer_properties: &Immutable<dyn LayerProperties>) -> bool {
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

    fn render_frame(state: &mut SourceFeatureState, tiles: &[&StubTile]) {
        state.coalesce_changes(tiles.iter().map(|tile| *tile as &dyn Tile));
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
}
