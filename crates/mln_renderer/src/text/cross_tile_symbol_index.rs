// Ports src/mln/text/cross_tile_symbol_index.hpp and
// src/mln/text/cross_tile_symbol_index.cpp, plus
// `SymbolBucket::registerAtCrossTileIndex` (symbol_bucket.cpp).
//
// C++ shares `maxCrossTileID` between the layer indexes by reference
// (`uint32_t&`); here it is an `Rc<Cell<u32>>`. The bucket's symbol
// instances live behind its state lock (see symbol_bucket.rs).
//
// Not ported: the MLN_SYMBOL_GUARDS `check` calls (always true without
// guards) and the render-thread id.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::rc::Rc;

use geometry::Point;
use mln_map_types::{TileProjector, TransformState};
use mln_tile_id::OverscaledTileID;
use mln_util::constants::EXTENT;
use mln_vec_map::{VecMap, VecSet};

use crate::bucket::Bucket;
use crate::buckets::symbol_bucket::SymbolBucket;
use crate::layout::symbol_instance::SymbolInstance;
use crate::render_layer::RenderLayer;
use crate::render_tile::RenderTile;

/// `IndexedSymbolInstance`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IndexedSymbolInstance {
    pub cross_tile_id: u32,
    pub coord: Point<i64>,
}

impl IndexedSymbolInstance {
    #[must_use]
    pub fn new(cross_tile_id: u32, coord: Point<i64>) -> Self {
        IndexedSymbolInstance { cross_tile_id, coord }
    }
}

/// `TileLayerIndex`.
#[derive(Clone, Debug)]
pub struct TileLayerIndex {
    pub coord: OverscaledTileID,
    pub bucket_instance_id: u32,
    pub bucket_leader_id: String,
    pub indexed_symbol_instances: BTreeMap<Vec<u16>, Vec<IndexedSymbolInstance>>,
}

impl TileLayerIndex {
    /// `TileLayerIndex(coord, symbolInstances, bucketInstanceId, bucketLeaderId)`.
    #[must_use]
    pub fn new(
        coord: OverscaledTileID,
        symbol_instances: &[SymbolInstance],
        bucket_instance_id: u32,
        bucket_leader_id: String,
    ) -> Self {
        let mut index =
            TileLayerIndex { coord, bucket_instance_id, bucket_leader_id, indexed_symbol_instances: BTreeMap::new() };
        for symbol_instance in symbol_instances {
            if symbol_instance.get_cross_tile_id() == SymbolInstance::INVALID_CROSS_TILE_ID {
                continue;
            }
            let scaled = index.get_scaled_coordinates(symbol_instance, &coord);
            index
                .indexed_symbol_instances
                .entry(symbol_instance.get_key().to_vec())
                .or_default()
                .push(IndexedSymbolInstance::new(symbol_instance.get_cross_tile_id(), scaled));
        }
        index
    }

    /// `getScaledCoordinates(symbolInstance, childTileCoord)`.
    #[must_use]
    pub fn get_scaled_coordinates(
        &self,
        symbol_instance: &SymbolInstance,
        child_tile_coord: &OverscaledTileID,
    ) -> Point<i64> {
        // Round anchor positions to roughly 4 pixel grid
        let rounding_factor: f64 = 512.0 / f64::from(EXTENT) / 2.0;
        let scale =
            rounding_factor / 2f64.powi(i32::from(child_tile_coord.canonical.z) - i32::from(self.coord.canonical.z));
        // `childTileCoord.canonical.x * util::EXTENT` is uint32_t arithmetic;
        // adding the float anchor makes it a float, the scale a double.
        let anchor = symbol_instance.get_anchor().point;
        let x = child_tile_coord.canonical.x.wrapping_mul(EXTENT as u32) as f32 + anchor.x;
        let y = child_tile_coord.canonical.y.wrapping_mul(EXTENT as u32) as f32 + anchor.y;
        Point::new((f64::from(x) * scale).floor() as i64, (f64::from(y) * scale).floor() as i64)
    }

    /// `findMatches(bucket, newCoord, zoomCrossTileIDs)`.
    pub fn find_matches(
        &self,
        symbol_instances: &mut [SymbolInstance],
        bucket_leader_id: &str,
        new_coord: &OverscaledTileID,
        zoom_cross_tile_ids: &mut BTreeSet<u32>,
    ) {
        let tolerance: f32 = if self.coord.canonical.z < new_coord.canonical.z {
            1.0
        } else {
            2f64.powi(i32::from(self.coord.canonical.z) - i32::from(new_coord.canonical.z)) as f32
        };

        if bucket_leader_id != self.bucket_leader_id {
            return;
        }

        for symbol_instance in symbol_instances.iter_mut() {
            if symbol_instance.get_cross_tile_id() != 0 {
                // already has a match, skip
                continue;
            }

            let Some(indexed) = self.indexed_symbol_instances.get(symbol_instance.get_key()) else {
                // No symbol with this key in this bucket
                continue;
            };

            let scaled_symbol_coord = self.get_scaled_coordinates(symbol_instance, new_coord);

            for this_tile_symbol in indexed {
                // Return any symbol with the same keys whose coordinates are within
                // 1 grid unit. (with a 4px grid, this covers a 12px by 12px area)
                if ((this_tile_symbol.coord.x - scaled_symbol_coord.x).abs() as f32) <= tolerance
                    && ((this_tile_symbol.coord.y - scaled_symbol_coord.y).abs() as f32) <= tolerance
                    && !zoom_cross_tile_ids.contains(&this_tile_symbol.cross_tile_id)
                {
                    // Once we've marked ourselves duplicate against this parent
                    // symbol, don't let any other symbols at the same zoom level
                    // duplicate against the same parent (see issue #10844)
                    zoom_cross_tile_ids.insert(this_tile_symbol.cross_tile_id);
                    symbol_instance.set_cross_tile_id(this_tile_symbol.cross_tile_id);
                    break;
                }
            }
        }
    }
}

/// `isInVewport(posMatrix, point)`.
fn is_in_viewport(projector: &TileProjector<'_>, point: Point<f32>) -> bool {
    // buffer covers area of the next zoom level (current zoom - 1 covered area).
    const BUFFER: f64 = 1.0;
    const EDGE: f64 = 1.0 + BUFFER;

    let projected = projector.project_as_drawn(&Point::new(f64::from(point.x), f64::from(point.y)), 0.0);
    let p = &projected.point;
    !projected.occluded && (p.x > -EDGE && p.y > -EDGE && p.x < EDGE && p.y < EDGE)
}

/// `removeBucketCrossTileIDs(zoom, removedBucket)`. A free function over the
/// used-ID sets so callers can pass an index that is still borrowed from
/// `CrossTileSymbolLayerIndex::indexes`.
fn remove_bucket_cross_tile_ids(
    used_cross_tile_ids: &mut VecMap<u8, BTreeSet<u32>>,
    zoom: u8,
    removed_bucket: &TileLayerIndex,
) {
    for indexed in removed_bucket.indexed_symbol_instances.values() {
        for indexed_symbol_instance in indexed {
            used_cross_tile_ids.entry(zoom).or_default().remove(&indexed_symbol_instance.cross_tile_id);
        }
    }
}

/// `CrossTileSymbolLayerIndex`.
#[derive(Debug)]
pub struct CrossTileSymbolLayerIndex {
    indexes: VecMap<u8, VecMap<OverscaledTileID, TileLayerIndex>>,
    used_cross_tile_ids: VecMap<u8, BTreeSet<u32>>,
    lng: f32,
    max_cross_tile_id: Rc<Cell<u32>>,
}

impl CrossTileSymbolLayerIndex {
    /// `CrossTileSymbolLayerIndex(uint32_t& maxCrossTileID)`.
    pub fn new(max_cross_tile_id: Rc<Cell<u32>>) -> Self {
        CrossTileSymbolLayerIndex {
            indexes: VecMap::new(),
            used_cross_tile_ids: VecMap::new(),
            lng: 0.0,
            max_cross_tile_id,
        }
    }

    /// Sometimes when a user pans across the antimeridian the longitude value
    /// gets wrapped. To prevent labels from flashing out and in we adjust the
    /// tileID values in the indexes so that they match the new wrapped
    /// version of the map.
    pub fn handle_wrap_jump(&mut self, new_lng: f32) {
        let wrap_delta = ((new_lng - self.lng) / 360.0).round() as i32;
        if wrap_delta != 0 {
            let mut new_indexes: VecMap<u8, VecMap<OverscaledTileID, TileLayerIndex>> = VecMap::new();
            for (zoom, zoom_index) in std::mem::take(&mut self.indexes) {
                let mut new_zoom_index: VecMap<OverscaledTileID, TileLayerIndex> = VecMap::new();
                for (_, mut index) in zoom_index {
                    // change the tileID's wrap and move its index
                    index.coord = index.coord.unwrap_to((i32::from(index.coord.wrap) + wrap_delta) as i16);
                    new_zoom_index.entry(index.coord).or_insert(index);
                }
                new_indexes.entry(zoom).or_insert(new_zoom_index);
            }

            self.indexes = new_indexes;
        }

        self.lng = new_lng;
    }

    /// `addBucket(tileID, tileMatrix, bucket)`.
    pub fn add_bucket(
        &mut self,
        tile_id: &OverscaledTileID,
        projector: &TileProjector<'_>,
        bucket: &SymbolBucket,
    ) -> bool {
        let mut state = bucket.state();
        let state = &mut *state;

        let this_zoom_indexes = self.indexes.entry(tile_id.overscaled_z).or_default();
        if !this_zoom_indexes.contains_key(tile_id) {
            // The globe moves a loaded tile to another wrap instead of loading
            // it again: same bucket, same symbols.
            let moved = this_zoom_indexes
                .iter()
                .find(|(key, index)| {
                    index.bucket_instance_id == bucket.bucket_instance_id && key.canonical == tile_id.canonical
                })
                .map(|(key, _)| *key);
            if let Some(mut index) = moved.and_then(|moved| this_zoom_indexes.remove(&moved)) {
                index.coord = *tile_id;
                this_zoom_indexes.insert(*tile_id, index);
            }
        }

        if let Some(previous_index) = this_zoom_indexes.get(tile_id) {
            if previous_index.bucket_instance_id == bucket.bucket_instance_id && !state.has_uninitialized_symbols {
                return false;
            }
            // We're replacing this bucket with an updated version
            // Remove the old bucket's "used crossTileIDs" now so that the new
            // bucket can claim them. We have to keep the old index entries
            // themselves until the end of 'addBucket' so that we can copy them
            // with 'findMatches'.
            remove_bucket_cross_tile_ids(&mut self.used_cross_tile_ids, tile_id.overscaled_z, previous_index);
        }

        state.has_uninitialized_symbols = false;

        if tile_id.overscale_factor() > 1 {
            // For overscaled tiles the viewport might be showing only a small part of the tile,
            // so we filter out the off-screen symbols to improve the performance.
            for symbol_instance in &mut state.symbol_instances {
                if is_in_viewport(projector, symbol_instance.get_anchor().point) {
                    symbol_instance.set_cross_tile_id(0);
                } else {
                    symbol_instance.set_cross_tile_id(SymbolInstance::INVALID_CROSS_TILE_ID);
                    state.has_uninitialized_symbols = true;
                }
            }
        } else {
            for symbol_instance in &mut state.symbol_instances {
                symbol_instance.set_cross_tile_id(0);
            }
        }

        let this_zoom_used_cross_tile_ids = self.used_cross_tile_ids.entry(tile_id.overscaled_z).or_default();

        for (&zoom, zoom_indexes) in &self.indexes {
            if zoom > tile_id.overscaled_z {
                for child_index in zoom_indexes.values() {
                    if child_index.coord.is_child_of(tile_id) {
                        child_index.find_matches(
                            &mut state.symbol_instances,
                            &bucket.bucket_leader_id,
                            tile_id,
                            this_zoom_used_cross_tile_ids,
                        );
                    }
                }
            } else {
                let parent_tile_id = tile_id.scaled_to(zoom);
                if let Some(parent_index) = zoom_indexes.get(&parent_tile_id) {
                    parent_index.find_matches(
                        &mut state.symbol_instances,
                        &bucket.bucket_leader_id,
                        tile_id,
                        this_zoom_used_cross_tile_ids,
                    );
                }
            }
        }

        for symbol_instance in &mut state.symbol_instances {
            if symbol_instance.get_cross_tile_id() == 0 {
                // symbol did not match any known symbol, assign a new id
                self.max_cross_tile_id.set(self.max_cross_tile_id.get().wrapping_add(1));
                symbol_instance.set_cross_tile_id(self.max_cross_tile_id.get());
                this_zoom_used_cross_tile_ids.insert(symbol_instance.get_cross_tile_id());
            }
        }

        let this_zoom_indexes = self.indexes.entry(tile_id.overscaled_z).or_default();
        this_zoom_indexes.remove(tile_id);
        this_zoom_indexes.insert(
            *tile_id,
            TileLayerIndex::new(
                *tile_id,
                &state.symbol_instances,
                bucket.bucket_instance_id,
                bucket.bucket_leader_id.clone(),
            ),
        );
        true
    }

    /// `removeStaleBuckets(currentIDs)`.
    pub fn remove_stale_buckets(&mut self, current_ids: &HashSet<u32>) -> bool {
        let mut tiles_changed = false;
        let zooms: Vec<u8> = self.indexes.keys().copied().collect();
        for zoom in zooms {
            let stale: Vec<OverscaledTileID> = self.indexes[&zoom]
                .iter()
                .filter(|(_, index)| !current_ids.contains(&index.bucket_instance_id))
                .map(|(id, _)| *id)
                .collect();
            for id in stale {
                let removed = self.indexes.get_mut(&zoom).expect("zoom").remove(&id).expect("stale index");
                remove_bucket_cross_tile_ids(&mut self.used_cross_tile_ids, zoom, &removed);
                tiles_changed = true;
            }
        }
        tiles_changed
    }
}

/// `CrossTileSymbolIndex::AddLayerResult` (bit flags).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AddLayerResult(pub u8);

impl AddLayerResult {
    pub const NO_CHANGES: AddLayerResult = AddLayerResult(0);
    pub const BUCKETS_ADDED: AddLayerResult = AddLayerResult(1 << 0);
    pub const BUCKETS_REMOVED: AddLayerResult = AddLayerResult(1 << 1);

    /// C++ `bool operator&(AddLayerResult, AddLayerResult)`.
    #[must_use]
    pub fn contains(self, rhs: AddLayerResult) -> bool {
        (self.0 & rhs.0) != 0
    }
}

impl std::ops::BitOrAssign for AddLayerResult {
    fn bitor_assign(&mut self, rhs: AddLayerResult) {
        self.0 |= rhs.0;
    }
}

/// `CrossTileSymbolIndex`.
#[derive(Debug, Default)]
pub struct CrossTileSymbolIndex {
    layer_indexes: VecMap<String, CrossTileSymbolLayerIndex>,
    max_cross_tile_id: Rc<Cell<u32>>,
}

impl CrossTileSymbolIndex {
    #[must_use]
    pub fn new() -> Self {
        CrossTileSymbolIndex::default()
    }

    /// `addLayer(layer, lng)`.
    pub fn add_layer(&mut self, layer: &dyn RenderLayer, lng: f32, state: &TransformState) -> AddLayerResult {
        let max_cross_tile_id = self.max_cross_tile_id.clone();
        let layer_index = self
            .layer_indexes
            .entry(layer.get_id().to_owned())
            .or_insert_with(|| CrossTileSymbolLayerIndex::new(max_cross_tile_id));

        let mut result = AddLayerResult::NO_CHANGES;
        let mut current_bucket_ids: HashSet<u32> = HashSet::new();

        layer_index.handle_wrap_jump(lng);

        for item in layer.get_placement_data() {
            let render_tile = item.tile();
            let pair = register_at_cross_tile_index(&*item.bucket, layer_index, render_tile, state);
            debug_assert!(pair.0 != 0);
            if pair.1 {
                result |= AddLayerResult::BUCKETS_ADDED;
            }
            current_bucket_ids.insert(pair.0);
        }

        if layer_index.remove_stale_buckets(&current_bucket_ids) {
            result |= AddLayerResult::BUCKETS_REMOVED;
        }

        result
    }

    /// `pruneUnusedLayers(usedLayers)`.
    pub fn prune_unused_layers(&mut self, used_layers: &VecSet<String>) {
        self.layer_indexes.retain(|id, _| used_layers.contains(id));
    }

    /// `reset()`.
    pub fn reset(&mut self) {
        self.layer_indexes.clear();
    }
}

/// `Bucket::registerAtCrossTileIndex(index, renderTile, state)`: only
/// symbol buckets override the default `{0, false}`.
pub fn register_at_cross_tile_index(
    bucket: &dyn Bucket,
    index: &mut CrossTileSymbolLayerIndex,
    render_tile: &RenderTile,
    state: &TransformState,
) -> (u32, bool) {
    match bucket.downcast_ref::<SymbolBucket>() {
        Some(symbol_bucket) => symbol_bucket.register_at_cross_tile_index(index, render_tile, state),
        None => (0, false),
    }
}

impl SymbolBucket {
    /// `registerAtCrossTileIndex(index, renderTile, state)`.
    pub fn register_at_cross_tile_index(
        &self,
        index: &mut CrossTileSymbolLayerIndex,
        render_tile: &RenderTile,
        state: &TransformState,
    ) -> (u32, bool) {
        let first_time_added = index.add_bucket(
            render_tile.get_overscaled_tile_id(),
            &TileProjector::with_data(state, &render_tile.id, render_tile.projection),
            self,
        );
        (self.bucket_instance_id, first_time_added)
    }
}

#[cfg(test)]
mod tests {
    // Ports test/text/cross_tile_symbol_index.test.cpp.
    use std::sync::Arc;

    use mln_style::layers::symbol_layer_properties::SymbolLayoutProperties;
    use mln_style::property_value::PropertyValue;
    use mln_style_types::image::ImageMap;
    use mln_style_types::{
        AnchorOffsetPair, ProjectionDefinition, SymbolAnchorType, SymbolPlacementType, VariableAnchorOffsetCollection,
    };
    use mln_tile_id::geometry_tile_data::GeometryCoordinates;
    use mln_util::Padding;
    use mln_util::geo::LatLng;
    use mln_util::size::Size;

    use super::*;
    use crate::buckets::symbol_bucket::SortKeyRange;
    use crate::geometry::anchor::Anchor;
    use crate::geometry::feature_index::IndexedSubfeature;
    use crate::layout::symbol_instance::{ShapedTextOrientations, SymbolContent, SymbolInstanceSharedData};

    fn make_symbol_instance(x: f32, y: f32, key: &str) -> SymbolInstance {
        let line = GeometryCoordinates::new();
        let image_map = ImageMap::new();
        let shaping = ShapedTextOrientations::default();
        let layout = SymbolLayoutProperties::Evaluated::default();
        let subfeature = IndexedSubfeature {
            index: 0,
            source_layer_name: String::new(),
            bucket_leader_id: String::new(),
            sort_index: 0,
            bucket_instance_id: 0,
            collision_group_id: 0,
        };
        let anchor = Anchor::new(x, y, 0.0, Some(0));
        let text_offset = [0.0f32, 0.0];
        let icon_offset = [0.0f32, 0.0];
        let variable_text_offset = [0.0f32, 0.0];
        let anchor_offsets = vec![AnchorOffsetPair::new(SymbolAnchorType::Left, variable_text_offset)];
        let variable_anchor_offset_collection = VariableAnchorOffsetCollection::new(anchor_offsets);
        let placement_type = SymbolPlacementType::Point;

        let shared_data = Arc::new(SymbolInstanceSharedData::new(
            line,
            &shaping,
            &None,
            &None,
            &layout,
            placement_type,
            &text_offset,
            &image_map,
            0.0,
            SymbolContent::IconSDF,
            false,
            false,
        ));
        SymbolInstance::new(
            anchor,
            shared_data,
            &shaping,
            &None,
            &None,
            0.0,
            0.0,
            placement_type,
            text_offset,
            0.0,
            Padding::uniform(0.0),
            icon_offset,
            &subfeature,
            0,
            0,
            key.encode_utf16().collect(),
            0.0,
            0.0,
            0.0,
            &Some(variable_anchor_offset_collection),
            false,
            SymbolContent::IconSDF,
        )
    }

    fn make_bucket(instances: Vec<SymbolInstance>, bucket_instance_id: u32) -> SymbolBucket {
        let layout = Arc::new(SymbolLayoutProperties::PossiblyEvaluated::default());
        let ranges: Vec<SortKeyRange> = Vec::new();
        let mut bucket = SymbolBucket::new(
            layout,
            &VecMap::new(),
            &PropertyValue::Constant(16.0f32),
            &PropertyValue::Constant(1.0f32),
            0.0,
            false,
            false,
            "test".to_owned(),
            instances,
            ranges,
            1.0,
            false,
            Vec::new(),
            false, /*iconsInText*/
        );
        bucket.bucket_instance_id = bucket_instance_id;
        bucket
    }

    fn cross_tile_id(bucket: &SymbolBucket, i: usize) -> u32 {
        bucket.state().symbol_instances[i].get_cross_tile_id()
    }

    fn camera_at(lat: f64, lon: f64, zoom: f64, globe: bool) -> TransformState {
        let mut state = TransformState::default();
        state.set_size(Size { width: 512, height: 512 });
        if globe {
            state.set_projection_definition(&ProjectionDefinition::from_name("vertical-perspective"));
        }
        state.set_lat_lng_zoom(&LatLng::new(lat, lon).expect("valid LatLng"), zoom);
        state
    }

    // Only overscaled tiles check the viewport, so any camera does for the
    // others.
    fn any_camera() -> TransformState {
        camera_at(0.0, 0.0, 0.0, false)
    }

    // gtest: CrossTileSymbolLayerIndex.addBucket
    #[test]
    fn add_bucket() {
        let any_camera = any_camera();
        let mut max_bucket_instance_id = 0u32;
        let mut index = CrossTileSymbolLayerIndex::new(Rc::new(Cell::new(0)));

        let main_id = OverscaledTileID::with_zxy(6, 0, 6, 8, 8);
        max_bucket_instance_id += 1;
        let main_bucket = make_bucket(
            vec![make_symbol_instance(1000.0, 1000.0, "Detroit"), make_symbol_instance(2000.0, 2000.0, "Toronto")],
            max_bucket_instance_id,
        );
        index.add_bucket(&main_id, &TileProjector::new(&any_camera, &main_id.to_unwrapped()), &main_bucket);

        // Assigned new IDs
        assert_eq!(cross_tile_id(&main_bucket, 0), 1);
        assert_eq!(cross_tile_id(&main_bucket, 1), 2);

        let child_id = OverscaledTileID::with_zxy(7, 0, 7, 16, 16);
        max_bucket_instance_id += 1;
        let child_bucket = make_bucket(
            vec![
                make_symbol_instance(2000.0, 2000.0, "Detroit"),
                make_symbol_instance(2000.0, 2000.0, "Windsor"),
                make_symbol_instance(3000.0, 3000.0, "Toronto"),
                make_symbol_instance(4001.0, 4001.0, "Toronto"),
            ],
            max_bucket_instance_id,
        );
        index.add_bucket(&child_id, &TileProjector::new(&any_camera, &child_id.to_unwrapped()), &child_bucket);

        // matched parent tile
        assert_eq!(cross_tile_id(&child_bucket, 0), 1);
        // does not match because of different key
        assert_eq!(cross_tile_id(&child_bucket, 1), 3);
        // does not match because of different location
        assert_eq!(cross_tile_id(&child_bucket, 2), 4);
        // matches with a slightly different location
        assert_eq!(cross_tile_id(&child_bucket, 3), 2);

        let parent_id = OverscaledTileID::with_zxy(5, 0, 5, 4, 4);
        max_bucket_instance_id += 1;
        let parent_bucket = make_bucket(vec![make_symbol_instance(500.0, 500.0, "Detroit")], max_bucket_instance_id);
        index.add_bucket(&parent_id, &TileProjector::new(&any_camera, &parent_id.to_unwrapped()), &parent_bucket);

        // matched child tile
        assert_eq!(cross_tile_id(&parent_bucket, 0), 1);

        let mut current_ids: HashSet<u32> = HashSet::new();
        current_ids.insert(main_bucket.bucket_instance_id);
        index.remove_stale_buckets(&current_ids);

        // grandchild
        let grandchild_id = OverscaledTileID::with_zxy(8, 0, 8, 32, 32);
        max_bucket_instance_id += 1;
        let grandchild_bucket = make_bucket(
            vec![make_symbol_instance(4000.0, 4000.0, "Detroit"), make_symbol_instance(4000.0, 4000.0, "Windsor")],
            max_bucket_instance_id,
        );
        index.add_bucket(
            &grandchild_id,
            &TileProjector::new(&any_camera, &grandchild_id.to_unwrapped()),
            &grandchild_bucket,
        );

        // Matches the symbol in `mainBucket`
        assert_eq!(cross_tile_id(&grandchild_bucket, 0), 1);
        // Does not match the previous value for Windsor because that tile was removed
        assert_eq!(cross_tile_id(&grandchild_bucket, 1), 5);
    }

    // gtest: CrossTileSymbolLayerIndex.tileMovedToAnotherWrapKeepsItsSymbols
    #[test]
    fn tile_moved_to_another_wrap_keeps_its_symbols() {
        let any_camera = any_camera();
        // The globe moves a loaded tile to another wrap instead of loading it
        // again: its bucket keeps the IDs.
        let mut index = CrossTileSymbolLayerIndex::new(Rc::new(Cell::new(0)));

        let bucket = make_bucket(
            vec![make_symbol_instance(1000.0, 1000.0, "Detroit"), make_symbol_instance(2000.0, 2000.0, "Toronto")],
            1,
        );

        let id = OverscaledTileID::with_zxy(0, 0, 0, 0, 0);
        assert!(index.add_bucket(&id, &TileProjector::new(&any_camera, &id.to_unwrapped()), &bucket));
        assert_eq!(cross_tile_id(&bucket, 0), 1);
        assert_eq!(cross_tile_id(&bucket, 1), 2);

        let moved = id.unwrap_to(1);
        assert!(!index.add_bucket(&moved, &TileProjector::new(&any_camera, &moved.to_unwrapped()), &bucket));
        assert_eq!(cross_tile_id(&bucket, 0), 1);
        assert_eq!(cross_tile_id(&bucket, 1), 2);
        let current_ids: HashSet<u32> = [1].into_iter().collect();
        assert!(!index.remove_stale_buckets(&current_ids));
    }

    // gtest: CrossTileSymbolLayerIndex.resetIDs
    #[test]
    fn reset_ids() {
        let any_camera = any_camera();
        let mut index = CrossTileSymbolLayerIndex::new(Rc::new(Cell::new(0)));

        let main_id = OverscaledTileID::with_zxy(6, 0, 6, 8, 8);
        let main_bucket = make_bucket(vec![make_symbol_instance(1000.0, 1000.0, "Detroit")], 1);

        let child_id = OverscaledTileID::with_zxy(7, 0, 7, 16, 16);
        let child_bucket = make_bucket(vec![make_symbol_instance(2000.0, 2000.0, "Detroit")], 2);

        // assigns a new id
        index.add_bucket(&main_id, &TileProjector::new(&any_camera, &main_id.to_unwrapped()), &main_bucket);
        assert_eq!(cross_tile_id(&main_bucket, 0), 1);

        // removes the tile
        let current_ids: HashSet<u32> = HashSet::new();
        index.remove_stale_buckets(&current_ids);

        // assigns a new id
        index.add_bucket(&child_id, &TileProjector::new(&any_camera, &child_id.to_unwrapped()), &child_bucket);
        assert_eq!(cross_tile_id(&child_bucket, 0), 2);

        // overwrites the old id to match the already-added tile
        index.add_bucket(&main_id, &TileProjector::new(&any_camera, &main_id.to_unwrapped()), &main_bucket);
        assert_eq!(cross_tile_id(&main_bucket, 0), 2);
    }

    // gtest: CrossTileSymbolLayerIndex.noDuplicatesWithinZoomLevel
    #[test]
    fn no_duplicates_within_zoom_level() {
        let any_camera = any_camera();
        let mut index = CrossTileSymbolLayerIndex::new(Rc::new(Cell::new(0)));

        let main_id = OverscaledTileID::with_zxy(6, 0, 6, 8, 8);
        let main_bucket = make_bucket(
            vec![
                make_symbol_instance(1000.0, 1000.0, ""), // A
                make_symbol_instance(1000.0, 1000.0, ""), // B
            ],
            1,
        );

        let child_id = OverscaledTileID::with_zxy(7, 0, 7, 16, 16);
        let child_bucket = make_bucket(
            vec![
                make_symbol_instance(2000.0, 2000.0, ""), // A'
                make_symbol_instance(2000.0, 2000.0, ""), // B'
                make_symbol_instance(2000.0, 2000.0, ""), // C'
            ],
            2,
        );

        // assigns new ids
        index.add_bucket(&main_id, &TileProjector::new(&any_camera, &main_id.to_unwrapped()), &main_bucket);
        assert_eq!(cross_tile_id(&main_bucket, 0), 1);
        assert_eq!(cross_tile_id(&main_bucket, 1), 2);

        // copies parent ids without duplicate ids in this tile
        index.add_bucket(&child_id, &TileProjector::new(&any_camera, &child_id.to_unwrapped()), &child_bucket);
        assert_eq!(cross_tile_id(&child_bucket, 0), 1); // A' copies from A
        assert_eq!(cross_tile_id(&child_bucket, 1), 2); // B' copies from B
        assert_eq!(cross_tile_id(&child_bucket, 2), 3); // C' gets new ID
    }

    // gtest: CrossTileSymbolLayerIndex.bucketReplacement
    #[test]
    fn bucket_replacement() {
        let any_camera = any_camera();
        let mut index = CrossTileSymbolLayerIndex::new(Rc::new(Cell::new(0)));

        let tile_id = OverscaledTileID::with_zxy(6, 0, 6, 8, 8);
        let first_bucket = make_bucket(
            vec![
                make_symbol_instance(1000.0, 1000.0, ""), // A
                make_symbol_instance(1000.0, 1000.0, ""), // B
            ],
            1,
        );

        let second_bucket = make_bucket(
            vec![
                make_symbol_instance(1000.0, 1000.0, ""), // A'
                make_symbol_instance(1000.0, 1000.0, ""), // B'
                make_symbol_instance(1000.0, 1000.0, ""), // C'
            ],
            2,
        );

        // assigns new ids
        index.add_bucket(&tile_id, &TileProjector::new(&any_camera, &tile_id.to_unwrapped()), &first_bucket);
        assert_eq!(cross_tile_id(&first_bucket, 0), 1);
        assert_eq!(cross_tile_id(&first_bucket, 1), 2);

        // copies parent ids without duplicate ids in this tile
        index.add_bucket(&tile_id, &TileProjector::new(&any_camera, &tile_id.to_unwrapped()), &second_bucket);
        assert_eq!(cross_tile_id(&second_bucket, 0), 1); // A' copies from A
        assert_eq!(cross_tile_id(&second_bucket, 1), 2); // B' copies from B
        assert_eq!(cross_tile_id(&second_bucket, 2), 3); // C' gets new ID
    }

    fn expect_offscreen_symbols_skipped(globe: bool) {
        let mut index = CrossTileSymbolLayerIndex::new(Rc::new(Cell::new(0)));

        let tile_id = OverscaledTileID::with_zxy(7, 0, 6, 18, 24);
        let symbol_bucket = make_bucket(
            vec![make_symbol_instance(1000.0, 1000.0, "Washington"), make_symbol_instance(2000.0, 2000.0, "Richmond")],
            1,
        );
        let away = camera_at(60.0, 25.0, 7.0, globe);
        index.add_bucket(&tile_id, &TileProjector::new(&away, &tile_id.to_unwrapped()), &symbol_bucket);

        assert_eq!(cross_tile_id(&symbol_bucket, 0), SymbolInstance::INVALID_CROSS_TILE_ID);
        assert_eq!(cross_tile_id(&symbol_bucket, 1), SymbolInstance::INVALID_CROSS_TILE_ID);

        let over = camera_at(39.0, -76.0, 7.0, globe);
        index.add_bucket(&tile_id, &TileProjector::new(&over, &tile_id.to_unwrapped()), &symbol_bucket);

        assert_eq!(cross_tile_id(&symbol_bucket, 0), 1);
        assert_eq!(cross_tile_id(&symbol_bucket, 1), 2);
    }

    // gtest: CrossTileSymbolLayerIndex.offscreenSymbols
    #[test]
    fn offscreen_symbols() {
        expect_offscreen_symbols_skipped(false);
    }

    // gtest: CrossTileSymbolLayerIndex.offscreenSymbolsOnTheGlobe
    // On the globe the anchors go through the sphere, not the tile matrix.
    #[test]
    fn offscreen_symbols_on_the_globe() {
        expect_offscreen_symbols_skipped(true);
    }
}
