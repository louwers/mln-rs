// The index gives the symbols of the buckets a layer places their ids in
// the frame's update, which has the buckets mutably (`SymbolBuckets`).

use std::collections::BTreeSet;
use std::num::NonZeroU32;

use geometry::Point;
use glam::Vec2;
use mln_map_types::{TileProjector, TransformState};
use mln_tile_id::OverscaledTileID;
use mln_util::constants::EXTENT;
use mln_util::hash::HashSet;
use mln_vec_map::{VecMap, VecSet};

use crate::buckets::symbol_bucket::SymbolBucket;
use crate::layout::symbol_instance::{SymbolInstance, SymbolInstancePools};
use crate::render_layer::{PlacedTile, RenderLayer, SymbolBuckets};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct IndexedSymbolInstance {
    pub(crate) cross_tile_id: NonZeroU32,
    pub(crate) coord: Point<i64>,
}

impl IndexedSymbolInstance {
    #[must_use]
    pub(crate) fn new(cross_tile_id: NonZeroU32, coord: Point<i64>) -> Self {
        IndexedSymbolInstance { cross_tile_id, coord }
    }
}

/// An indexed symbol and where its key is in its index's `keys`.
#[derive(Clone, Copy, Debug)]
struct IndexedSymbol {
    key_start: u32,
    key_len: u32,
    instance: IndexedSymbolInstance,
}

#[derive(Clone, Debug)]
pub(crate) struct TileLayerIndex {
    pub(crate) coord: OverscaledTileID,
    pub(crate) bucket_instance_id: u32,
    pub(crate) bucket_leader_id: String,
    /// The keys of `symbols`, end to end. The render thread builds an index
    /// each time it adds a bucket, so the keys share one allocation rather
    /// than taking one each.
    keys: Vec<u16>,
    /// The bucket's symbols that have a cross-tile id, sorted by key, and in
    /// the bucket's order among equal keys.
    symbols: Vec<IndexedSymbol>,
}

impl TileLayerIndex {
    #[must_use]
    pub(crate) fn new(
        coord: OverscaledTileID,
        symbol_instances: &[SymbolInstance],
        pools: &SymbolInstancePools,
        bucket_instance_id: u32,
        bucket_leader_id: String,
    ) -> Self {
        let mut index =
            TileLayerIndex { coord, bucket_instance_id, bucket_leader_id, keys: Vec::new(), symbols: Vec::new() };
        let indexed = || symbol_instances.iter().filter(|symbol_instance| symbol_instance.cross_tile_id().is_some());
        index.keys.reserve_exact(indexed().map(|symbol_instance| pools.key(symbol_instance).len()).sum());
        index.symbols.reserve_exact(indexed().count());
        for symbol_instance in indexed() {
            let Some(cross_tile_id) = symbol_instance.cross_tile_id() else {
                continue;
            };
            let key = pools.key(symbol_instance);
            index.symbols.push(IndexedSymbol {
                key_start: index.keys.len() as u32,
                key_len: key.len() as u32,
                instance: IndexedSymbolInstance::new(cross_tile_id, index.scaled_coordinates(symbol_instance, &coord)),
            });
            index.keys.extend_from_slice(key);
        }
        // A stable sort, so symbols with one key stay in the bucket's order.
        let keys = &index.keys;
        index.symbols.sort_by(|a, b| key_of(keys, a).cmp(key_of(keys, b)));
        index
    }

    /// The indexed symbols with `key`, in their bucket's order.
    fn symbols_with_key(&self, key: &[u16]) -> &[IndexedSymbol] {
        let start = self.symbols.partition_point(|symbol| key_of(&self.keys, symbol) < key);
        let len = self.symbols[start..].partition_point(|symbol| key_of(&self.keys, symbol) == key);
        &self.symbols[start..][..len]
    }

    #[must_use]
    pub(crate) fn scaled_coordinates(
        &self,
        symbol_instance: &SymbolInstance,
        child_tile_coord: &OverscaledTileID,
    ) -> Point<i64> {
        // Round anchor positions to roughly 4 pixel grid
        let rounding_factor: f64 = 512.0 / f64::from(EXTENT) / 2.0;
        let scale =
            rounding_factor / 2f64.powi(i32::from(child_tile_coord.canonical.z) - i32::from(self.coord.canonical.z));
        // The tile's position wraps in `u32`; adding the anchor makes it an
        // `f32`, and the scale an `f64`.
        let anchor = symbol_instance.anchor().point;
        let x = child_tile_coord.canonical.x.wrapping_mul(EXTENT as u32) as f32 + anchor.x;
        let y = child_tile_coord.canonical.y.wrapping_mul(EXTENT as u32) as f32 + anchor.y;
        Point::new((f64::from(x) * scale).floor() as i64, (f64::from(y) * scale).floor() as i64)
    }

    pub(crate) fn find_matches(
        &self,
        symbol_instances: &mut [SymbolInstance],
        pools: &SymbolInstancePools,
        pending: &mut [bool],
        bucket_leader_id: &str,
        new_coord: &OverscaledTileID,
        zoom_cross_tile_ids: &mut BTreeSet<NonZeroU32>,
    ) {
        let tolerance: f32 = if self.coord.canonical.z < new_coord.canonical.z {
            1.0
        } else {
            2f64.powi(i32::from(self.coord.canonical.z) - i32::from(new_coord.canonical.z)) as f32
        };

        if bucket_leader_id != self.bucket_leader_id {
            return;
        }

        for (symbol_instance, pending) in symbol_instances.iter_mut().zip(pending.iter_mut()) {
            if !*pending {
                // already has a match, or is skipped
                continue;
            }

            let indexed = self.symbols_with_key(pools.key(symbol_instance));
            if indexed.is_empty() {
                // No symbol with this key in this bucket
                continue;
            }

            let scaled_symbol_coord = self.scaled_coordinates(symbol_instance, new_coord);

            for this_tile_symbol in indexed.iter().map(|symbol| &symbol.instance) {
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
                    symbol_instance.set_cross_tile_id(Some(this_tile_symbol.cross_tile_id));
                    *pending = false;
                    break;
                }
            }
        }
    }
}

/// The key of `symbol`, in its index's `keys`.
fn key_of<'a>(keys: &'a [u16], symbol: &IndexedSymbol) -> &'a [u16] {
    &keys[symbol.key_start as usize..][..symbol.key_len as usize]
}

/// Whether `point`, `elevation` meters above the ground, is in or near the
/// viewport.
fn is_in_viewport(projector: &TileProjector<'_>, point: Vec2, elevation: f32) -> bool {
    // buffer covers area of the next zoom level (current zoom - 1 covered area).
    const BUFFER: f64 = 1.0;
    const EDGE: f64 = 1.0 + BUFFER;

    let projected =
        projector.project_as_drawn(&Point::new(f64::from(point.x), f64::from(point.y)), f64::from(elevation));
    let p = &projected.point;
    !projected.occluded && (p.x > -EDGE && p.y > -EDGE && p.x < EDGE && p.y < EDGE)
}

/// Frees the cross-tile ids of a removed bucket at `zoom`. A free function
/// over the used-ID sets so callers can pass an index that is still borrowed
/// from `CrossTileSymbolLayerIndex::indexes`.
fn remove_bucket_cross_tile_ids(
    used_cross_tile_ids: &mut VecMap<u8, BTreeSet<NonZeroU32>>,
    zoom: u8,
    removed_bucket: &TileLayerIndex,
) {
    for symbol in &removed_bucket.symbols {
        used_cross_tile_ids.entry(zoom).or_default().remove(&symbol.instance.cross_tile_id);
    }
}

#[derive(Debug)]
pub(crate) struct CrossTileSymbolLayerIndex {
    indexes: VecMap<u8, VecMap<OverscaledTileID, TileLayerIndex>>,
    used_cross_tile_ids: VecMap<u8, BTreeSet<NonZeroU32>>,
    lng: f32,
}

impl CrossTileSymbolLayerIndex {
    pub(crate) fn new() -> Self {
        CrossTileSymbolLayerIndex { indexes: VecMap::new(), used_cross_tile_ids: VecMap::new(), lng: 0.0 }
    }

    /// Sometimes when a user pans across the antimeridian the longitude value
    /// gets wrapped. To prevent labels from flashing out and in we adjust the
    /// tile ids in the indexes so that they match the new wrapped version of
    /// the map.
    pub(crate) fn handle_wrap_jump(&mut self, new_lng: f32) {
        let wrap_delta = ((new_lng - self.lng) / 360.0).round() as i32;
        if wrap_delta != 0 {
            let mut new_indexes: VecMap<u8, VecMap<OverscaledTileID, TileLayerIndex>> = VecMap::new();
            for (zoom, zoom_index) in std::mem::take(&mut self.indexes) {
                let mut new_zoom_index: VecMap<OverscaledTileID, TileLayerIndex> = VecMap::new();
                for (_, mut index) in zoom_index {
                    // Change the tile id's wrap and move its index.
                    index.coord = index.coord.unwrap_to((i32::from(index.coord.wrap) + wrap_delta) as i16);
                    new_zoom_index.entry(index.coord).or_insert(index);
                }
                new_indexes.entry(zoom).or_insert(new_zoom_index);
            }

            self.indexes = new_indexes;
        }

        self.lng = new_lng;
    }

    /// Matches `bucket`'s symbols with the ones already indexed, and gives
    /// each new symbol the id after `max_cross_tile_id`, which it advances.
    pub(crate) fn add_bucket(
        &mut self,
        tile_id: &OverscaledTileID,
        projector: &TileProjector<'_>,
        bucket: &mut SymbolBucket,
        max_cross_tile_id: &mut u32,
    ) -> bool {
        let state = &mut bucket.state;

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
            // An updated version replaces this bucket: the old bucket's used
            // cross-tile ids go now, so that the new bucket can claim them.
            // The old index entries stay until the end of `add_bucket`,
            // which copies them as it finds matches.
            remove_bucket_cross_tile_ids(&mut self.used_cross_tile_ids, tile_id.overscaled_z, previous_index);
        }

        state.has_uninitialized_symbols = false;

        // Symbols that still need an id; skipped ones keep `None`.
        let mut pending: Vec<bool> = Vec::with_capacity(state.symbol_instances.len());
        if tile_id.overscale_factor() > 1 {
            // For overscaled tiles the viewport might be showing only a small part of the tile,
            // so we filter out the off-screen symbols to improve the performance.
            for symbol_instance in &mut state.symbol_instances {
                symbol_instance.set_cross_tile_id(None);
                let in_viewport =
                    is_in_viewport(projector, symbol_instance.anchor().point, symbol_instance.height_offset());
                pending.push(in_viewport);
                state.has_uninitialized_symbols |= !in_viewport;
            }
        } else {
            for symbol_instance in &mut state.symbol_instances {
                symbol_instance.set_cross_tile_id(None);
            }
            pending.resize(state.symbol_instances.len(), true);
        }

        let this_zoom_used_cross_tile_ids = self.used_cross_tile_ids.entry(tile_id.overscaled_z).or_default();

        for (&zoom, zoom_indexes) in &self.indexes {
            if zoom > tile_id.overscaled_z {
                for child_index in zoom_indexes.values() {
                    if child_index.coord.is_child_of(tile_id) {
                        child_index.find_matches(
                            &mut state.symbol_instances,
                            &state.symbol_instance_pools,
                            &mut pending,
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
                        &state.symbol_instance_pools,
                        &mut pending,
                        &bucket.bucket_leader_id,
                        tile_id,
                        this_zoom_used_cross_tile_ids,
                    );
                }
            }
        }

        for (symbol_instance, pending) in state.symbol_instances.iter_mut().zip(&pending) {
            if *pending {
                // symbol did not match any known symbol, assign a new id
                let id = loop {
                    *max_cross_tile_id = max_cross_tile_id.wrapping_add(1);
                    if let Some(id) = NonZeroU32::new(*max_cross_tile_id) {
                        break id;
                    }
                };
                symbol_instance.set_cross_tile_id(Some(id));
                this_zoom_used_cross_tile_ids.insert(id);
            }
        }

        let this_zoom_indexes = self.indexes.entry(tile_id.overscaled_z).or_default();
        this_zoom_indexes.remove(tile_id);
        this_zoom_indexes.insert(
            *tile_id,
            TileLayerIndex::new(
                *tile_id,
                &state.symbol_instances,
                &state.symbol_instance_pools,
                bucket.bucket_instance_id,
                bucket.bucket_leader_id.clone(),
            ),
        );
        true
    }

    pub(crate) fn remove_stale_buckets(&mut self, current_ids: &HashSet<u32>) -> bool {
        let mut tiles_changed = false;
        let used_cross_tile_ids = &mut self.used_cross_tile_ids;
        for (&zoom, indexes) in &mut self.indexes {
            indexes.retain(|_, index| {
                let current = current_ids.contains(&index.bucket_instance_id);
                if !current {
                    remove_bucket_cross_tile_ids(used_cross_tile_ids, zoom, index);
                    tiles_changed = true;
                }
                current
            });
        }
        tiles_changed
    }
}

/// What adding a layer to the index changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct AddLayerResult {
    pub(crate) buckets_added: bool,
    pub(crate) buckets_removed: bool,
}

#[derive(Debug, Default)]
pub(crate) struct CrossTileSymbolIndex {
    layer_indexes: VecMap<String, CrossTileSymbolLayerIndex>,
    /// The last cross-tile id given to a symbol, in any layer.
    max_cross_tile_id: u32,
    /// The last instance id given to a symbol bucket. It outlives `reset`,
    /// so that a bucket indexed again keeps an id no other bucket has.
    max_bucket_instance_id: u32,
    /// The instance ids of the buckets a layer places, as `index_layer`
    /// collects them.
    current_bucket_ids: HashSet<u32>,
}

impl CrossTileSymbolIndex {
    /// Gives the symbols of the buckets `layer` places, in `buckets`, their
    /// cross-tile ids, and forgets the buckets of the layer it no longer
    /// places.
    pub(crate) fn index_layer(
        &mut self,
        layer: &dyn RenderLayer,
        lng: f32,
        state: &TransformState,
        buckets: &mut (impl SymbolBuckets + ?Sized),
    ) -> AddLayerResult {
        let layer_index = self.layer_indexes.get_or_insert_with(layer.id(), CrossTileSymbolLayerIndex::new);

        let mut result = AddLayerResult::default();
        let current_bucket_ids = &mut self.current_bucket_ids;
        current_bucket_ids.clear();

        layer_index.handle_wrap_jump(lng);

        for item in layer.placement_data() {
            let Some(bucket) = buckets.get_mut(&item.bucket) else {
                continue;
            };
            if bucket.bucket_instance_id == 0 {
                self.max_bucket_instance_id += 1;
                bucket.bucket_instance_id = self.max_bucket_instance_id;
            }
            let render_tile = &item.tile;
            let pair =
                bucket.register_at_cross_tile_index(layer_index, render_tile, state, &mut self.max_cross_tile_id);
            debug_assert!(pair.0 != 0);
            result.buckets_added |= pair.1;
            current_bucket_ids.insert(pair.0);
        }

        result.buckets_removed = layer_index.remove_stale_buckets(current_bucket_ids);

        result
    }

    /// [`index_layer`](Self::index_layer) of the buckets that
    /// `HeldBuckets` holds, for the benchmarks.
    #[cfg(feature = "bench")]
    pub(crate) fn add_layer(&mut self, layer: &dyn RenderLayer, lng: f32, state: &TransformState) -> AddLayerResult {
        let mut buckets = crate::render_layer::HeldBuckets::take();
        let result = self.index_layer(layer, lng, state, &mut buckets);
        buckets.restore();
        result
    }

    pub(crate) fn prune_unused_layers(&mut self, used_layers: &VecSet<String>) {
        self.layer_indexes.retain(|id, _| used_layers.contains(id));
    }

    pub(crate) fn reset(&mut self) {
        self.layer_indexes.clear();
    }
}

impl SymbolBucket {
    pub(crate) fn register_at_cross_tile_index(
        &mut self,
        index: &mut CrossTileSymbolLayerIndex,
        render_tile: &PlacedTile,
        state: &TransformState,
        max_cross_tile_id: &mut u32,
    ) -> (u32, bool) {
        let first_time_added = index.add_bucket(
            &render_tile.overscaled_id,
            &TileProjector::with_data(state, &render_tile.id, render_tile.projection),
            self,
            max_cross_tile_id,
        );
        (self.bucket_instance_id, first_time_added)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mln_style::layers::symbol_layer_properties::SymbolLayoutProperties;
    use mln_style::property_value::PropertyValue;
    use mln_style_types::image::ImagePositions;
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
    use crate::text::collision_index::OverlapMode;

    fn make_symbol_instance(x: f32, y: f32, key: &str, pools: &mut SymbolInstancePools) -> SymbolInstance {
        let line = GeometryCoordinates::new();
        let image_map = ImagePositions::default();
        let shaping = ShapedTextOrientations::default();
        let layout = SymbolLayoutProperties::Values::default();
        let subfeature = IndexedSubfeature {
            index: 0,
            source_layer_name: Arc::from(""),
            bucket_leader_id: Arc::from(""),
            sort_index: 0,
            bucket_instance_id: 0,
            collision_group_id: 0,
            overlap_mode: OverlapMode::Never,
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
            None.as_ref(),
            None.as_ref(),
            &layout,
            placement_type,
            text_offset,
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
            None.as_ref(),
            None.as_ref(),
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
            &key.encode_utf16().collect::<Vec<u16>>(),
            0.0,
            0.0,
            0.0,
            Some(variable_anchor_offset_collection).as_ref(),
            false,
            SymbolContent::IconSDF,
            0.0,
            pools,
        )
    }

    /// A bucket of symbols at `(x, y)` with the text `key`.
    fn make_bucket(symbols: Vec<(f32, f32, &str)>, bucket_instance_id: u32) -> SymbolBucket {
        let mut pools = SymbolInstancePools::default();
        let instances = symbols.into_iter().map(|(x, y, key)| make_symbol_instance(x, y, key, &mut pools)).collect();
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
            pools,
            ranges,
            1.0,
            false,
            Vec::new(),
            false, /*iconsInText*/
        );
        bucket.bucket_instance_id = bucket_instance_id;
        bucket
    }

    fn cross_tile_id(bucket: &SymbolBucket, i: usize) -> Option<u32> {
        bucket.state.symbol_instances[i].cross_tile_id().map(NonZeroU32::get)
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
        let mut index = CrossTileSymbolLayerIndex::new();
        let mut max_cross_tile_id = 0;

        let main_id = OverscaledTileID::with_zxy(6, 0, 6, 8, 8);
        max_bucket_instance_id += 1;
        let mut main_bucket =
            make_bucket(vec![(1000.0, 1000.0, "Detroit"), (2000.0, 2000.0, "Toronto")], max_bucket_instance_id);
        index.add_bucket(
            &main_id,
            &TileProjector::new(&any_camera, &main_id.to_unwrapped()),
            &mut main_bucket,
            &mut max_cross_tile_id,
        );

        // Assigned new IDs
        assert_eq!(cross_tile_id(&main_bucket, 0), Some(1));
        assert_eq!(cross_tile_id(&main_bucket, 1), Some(2));

        let child_id = OverscaledTileID::with_zxy(7, 0, 7, 16, 16);
        max_bucket_instance_id += 1;
        let mut child_bucket = make_bucket(
            vec![
                (2000.0, 2000.0, "Detroit"),
                (2000.0, 2000.0, "Windsor"),
                (3000.0, 3000.0, "Toronto"),
                (4001.0, 4001.0, "Toronto"),
            ],
            max_bucket_instance_id,
        );
        index.add_bucket(
            &child_id,
            &TileProjector::new(&any_camera, &child_id.to_unwrapped()),
            &mut child_bucket,
            &mut max_cross_tile_id,
        );

        // matched parent tile
        assert_eq!(cross_tile_id(&child_bucket, 0), Some(1));
        // does not match because of different key
        assert_eq!(cross_tile_id(&child_bucket, 1), Some(3));
        // does not match because of different location
        assert_eq!(cross_tile_id(&child_bucket, 2), Some(4));
        // matches with a slightly different location
        assert_eq!(cross_tile_id(&child_bucket, 3), Some(2));

        let parent_id = OverscaledTileID::with_zxy(5, 0, 5, 4, 4);
        max_bucket_instance_id += 1;
        let mut parent_bucket = make_bucket(vec![(500.0, 500.0, "Detroit")], max_bucket_instance_id);
        index.add_bucket(
            &parent_id,
            &TileProjector::new(&any_camera, &parent_id.to_unwrapped()),
            &mut parent_bucket,
            &mut max_cross_tile_id,
        );

        // matched child tile
        assert_eq!(cross_tile_id(&parent_bucket, 0), Some(1));

        let mut current_ids: HashSet<u32> = HashSet::default();
        current_ids.insert(main_bucket.bucket_instance_id);
        index.remove_stale_buckets(&current_ids);

        // grandchild
        let grandchild_id = OverscaledTileID::with_zxy(8, 0, 8, 32, 32);
        max_bucket_instance_id += 1;
        let mut grandchild_bucket =
            make_bucket(vec![(4000.0, 4000.0, "Detroit"), (4000.0, 4000.0, "Windsor")], max_bucket_instance_id);
        index.add_bucket(
            &grandchild_id,
            &TileProjector::new(&any_camera, &grandchild_id.to_unwrapped()),
            &mut grandchild_bucket,
            &mut max_cross_tile_id,
        );

        // Matches the symbol in the main bucket.
        assert_eq!(cross_tile_id(&grandchild_bucket, 0), Some(1));
        // Does not match the previous value for Windsor because that tile was removed
        assert_eq!(cross_tile_id(&grandchild_bucket, 1), Some(5));
    }

    // gtest: CrossTileSymbolLayerIndex.tileMovedToAnotherWrapKeepsItsSymbols
    #[test]
    fn tile_moved_to_another_wrap_keeps_its_symbols() {
        let any_camera = any_camera();
        // The globe moves a loaded tile to another wrap instead of loading it
        // again: its bucket keeps the IDs.
        let mut index = CrossTileSymbolLayerIndex::new();
        let mut max_cross_tile_id = 0;

        let mut bucket = make_bucket(vec![(1000.0, 1000.0, "Detroit"), (2000.0, 2000.0, "Toronto")], 1);

        let id = OverscaledTileID::with_zxy(0, 0, 0, 0, 0);
        assert!(index.add_bucket(
            &id,
            &TileProjector::new(&any_camera, &id.to_unwrapped()),
            &mut bucket,
            &mut max_cross_tile_id
        ));
        assert_eq!(cross_tile_id(&bucket, 0), Some(1));
        assert_eq!(cross_tile_id(&bucket, 1), Some(2));

        let moved = id.unwrap_to(1);
        assert!(!index.add_bucket(
            &moved,
            &TileProjector::new(&any_camera, &moved.to_unwrapped()),
            &mut bucket,
            &mut max_cross_tile_id
        ));
        assert_eq!(cross_tile_id(&bucket, 0), Some(1));
        assert_eq!(cross_tile_id(&bucket, 1), Some(2));
        let current_ids: HashSet<u32> = [1].into_iter().collect();
        assert!(!index.remove_stale_buckets(&current_ids));
    }

    // gtest: CrossTileSymbolLayerIndex.resetIDs
    #[test]
    fn reset_ids() {
        let any_camera = any_camera();
        let mut index = CrossTileSymbolLayerIndex::new();
        let mut max_cross_tile_id = 0;

        let main_id = OverscaledTileID::with_zxy(6, 0, 6, 8, 8);
        let mut main_bucket = make_bucket(vec![(1000.0, 1000.0, "Detroit")], 1);

        let child_id = OverscaledTileID::with_zxy(7, 0, 7, 16, 16);
        let mut child_bucket = make_bucket(vec![(2000.0, 2000.0, "Detroit")], 2);

        // assigns a new id
        index.add_bucket(
            &main_id,
            &TileProjector::new(&any_camera, &main_id.to_unwrapped()),
            &mut main_bucket,
            &mut max_cross_tile_id,
        );
        assert_eq!(cross_tile_id(&main_bucket, 0), Some(1));

        // removes the tile
        let current_ids: HashSet<u32> = HashSet::default();
        index.remove_stale_buckets(&current_ids);

        // assigns a new id
        index.add_bucket(
            &child_id,
            &TileProjector::new(&any_camera, &child_id.to_unwrapped()),
            &mut child_bucket,
            &mut max_cross_tile_id,
        );
        assert_eq!(cross_tile_id(&child_bucket, 0), Some(2));

        // overwrites the old id to match the already-added tile
        index.add_bucket(
            &main_id,
            &TileProjector::new(&any_camera, &main_id.to_unwrapped()),
            &mut main_bucket,
            &mut max_cross_tile_id,
        );
        assert_eq!(cross_tile_id(&main_bucket, 0), Some(2));
    }

    // gtest: CrossTileSymbolLayerIndex.noDuplicatesWithinZoomLevel
    #[test]
    fn no_duplicates_within_zoom_level() {
        let any_camera = any_camera();
        let mut index = CrossTileSymbolLayerIndex::new();
        let mut max_cross_tile_id = 0;

        let main_id = OverscaledTileID::with_zxy(6, 0, 6, 8, 8);
        let mut main_bucket = make_bucket(
            vec![
                (1000.0, 1000.0, ""), // A
                (1000.0, 1000.0, ""), // B
            ],
            1,
        );

        let child_id = OverscaledTileID::with_zxy(7, 0, 7, 16, 16);
        let mut child_bucket = make_bucket(
            vec![
                (2000.0, 2000.0, ""), // A'
                (2000.0, 2000.0, ""), // B'
                (2000.0, 2000.0, ""), // C'
            ],
            2,
        );

        // assigns new ids
        index.add_bucket(
            &main_id,
            &TileProjector::new(&any_camera, &main_id.to_unwrapped()),
            &mut main_bucket,
            &mut max_cross_tile_id,
        );
        assert_eq!(cross_tile_id(&main_bucket, 0), Some(1));
        assert_eq!(cross_tile_id(&main_bucket, 1), Some(2));

        // copies parent ids without duplicate ids in this tile
        index.add_bucket(
            &child_id,
            &TileProjector::new(&any_camera, &child_id.to_unwrapped()),
            &mut child_bucket,
            &mut max_cross_tile_id,
        );
        assert_eq!(cross_tile_id(&child_bucket, 0), Some(1)); // A' copies from A
        assert_eq!(cross_tile_id(&child_bucket, 1), Some(2)); // B' copies from B
        assert_eq!(cross_tile_id(&child_bucket, 2), Some(3)); // C' gets new ID
    }

    // gtest: CrossTileSymbolLayerIndex.bucketReplacement
    #[test]
    fn bucket_replacement() {
        let any_camera = any_camera();
        let mut index = CrossTileSymbolLayerIndex::new();
        let mut max_cross_tile_id = 0;

        let tile_id = OverscaledTileID::with_zxy(6, 0, 6, 8, 8);
        let mut first_bucket = make_bucket(
            vec![
                (1000.0, 1000.0, ""), // A
                (1000.0, 1000.0, ""), // B
            ],
            1,
        );

        let mut second_bucket = make_bucket(
            vec![
                (1000.0, 1000.0, ""), // A'
                (1000.0, 1000.0, ""), // B'
                (1000.0, 1000.0, ""), // C'
            ],
            2,
        );

        // assigns new ids
        index.add_bucket(
            &tile_id,
            &TileProjector::new(&any_camera, &tile_id.to_unwrapped()),
            &mut first_bucket,
            &mut max_cross_tile_id,
        );
        assert_eq!(cross_tile_id(&first_bucket, 0), Some(1));
        assert_eq!(cross_tile_id(&first_bucket, 1), Some(2));

        // copies parent ids without duplicate ids in this tile
        index.add_bucket(
            &tile_id,
            &TileProjector::new(&any_camera, &tile_id.to_unwrapped()),
            &mut second_bucket,
            &mut max_cross_tile_id,
        );
        assert_eq!(cross_tile_id(&second_bucket, 0), Some(1)); // A' copies from A
        assert_eq!(cross_tile_id(&second_bucket, 1), Some(2)); // B' copies from B
        assert_eq!(cross_tile_id(&second_bucket, 2), Some(3)); // C' gets new ID
    }

    fn expect_offscreen_symbols_skipped(globe: bool) {
        let mut index = CrossTileSymbolLayerIndex::new();
        let mut max_cross_tile_id = 0;

        let tile_id = OverscaledTileID::with_zxy(7, 0, 6, 18, 24);
        let mut symbol_bucket = make_bucket(vec![(1000.0, 1000.0, "Washington"), (2000.0, 2000.0, "Richmond")], 1);
        let away = camera_at(60.0, 25.0, 7.0, globe);
        index.add_bucket(
            &tile_id,
            &TileProjector::new(&away, &tile_id.to_unwrapped()),
            &mut symbol_bucket,
            &mut max_cross_tile_id,
        );

        assert_eq!(cross_tile_id(&symbol_bucket, 0), None);
        assert_eq!(cross_tile_id(&symbol_bucket, 1), None);

        let over = camera_at(39.0, -76.0, 7.0, globe);
        index.add_bucket(
            &tile_id,
            &TileProjector::new(&over, &tile_id.to_unwrapped()),
            &mut symbol_bucket,
            &mut max_cross_tile_id,
        );

        assert_eq!(cross_tile_id(&symbol_bucket, 0), Some(1));
        assert_eq!(cross_tile_id(&symbol_bucket, 1), Some(2));
    }

    // An overscaled tile with symbols off the screen registers again each
    // frame; under the same camera that changes no symbol's id, which
    // placement relies on to compare a bucket by its instance id.
    #[test]
    fn registering_again_under_the_same_camera_keeps_the_ids() {
        let mut index = CrossTileSymbolLayerIndex::new();
        let mut max_cross_tile_id = 0;
        let tile_id = OverscaledTileID::with_zxy(7, 0, 6, 18, 24);
        let mut symbol_bucket = make_bucket(vec![(1000.0, 1000.0, "Pittsburgh"), (8000.0, 8000.0, "Norfolk")], 1);
        let camera = camera_at(40.5, -78.0, 7.0, false);
        let projector = TileProjector::new(&camera, &tile_id.to_unwrapped());

        assert!(index.add_bucket(&tile_id, &projector, &mut symbol_bucket, &mut max_cross_tile_id));
        let ids = (cross_tile_id(&symbol_bucket, 0), cross_tile_id(&symbol_bucket, 1));
        assert_eq!(ids, (Some(1), None));

        assert!(index.add_bucket(&tile_id, &projector, &mut symbol_bucket, &mut max_cross_tile_id));
        assert_eq!((cross_tile_id(&symbol_bucket, 0), cross_tile_id(&symbol_bucket, 1)), ids);
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
