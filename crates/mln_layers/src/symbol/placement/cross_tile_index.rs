//! The cross-tile symbol index gives a label one id in every tile that has
//! it, so placement can fade it continuously as tiles of other zooms, newer
//! buckets or other wraps replace the ones that showed it.
//!
//! Each symbol layer has a [`LayerIndex`] of the tiles it places. A symbol
//! a bucket adds takes the id of a symbol already indexed when they:
//!
//! - are in the same tile (an earlier bucket), a parent tile or a child
//!   tile, from buckets with the same leader layer,
//! - have the same key (their text, or icon),
//! - and lie within one cell of each other on a grid of 256 cells per tile
//!   side at the indexed tile's zoom (2 to 4 px as the tile shows), or
//!   within one cell of the child tile's grid when the indexed tile is a
//!   child.
//!
//! The tiles are matched from the lowest zoom up, and each tile's symbols in
//! its bucket's order. Each id goes to at most one symbol per zoom, so two
//! symbols of a child never take one parent symbol's id. A symbol without a
//! match takes a new id. In an overscaled tile, symbols outside the viewport
//! take no id until the bucket is added again with them in view.
//!
//! The index runs in the frame's update, which has the buckets mutably
//! ([`PlacementBuckets`]).

use std::num::NonZeroU32;
use std::sync::Arc;

use geometry::Point;
use glam::{I64Vec2, Vec2};
use mln_map_types::{TileProjector, View};
use mln_render::placement::{PlacementBuckets, PlacingTile};
use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_util::constants::EXTENT;
use mln_util::hash::HashSet;
use mln_vec_map::VecMap;

use crate::symbol::bucket::SymbolBucket;
use crate::symbol::instance::{SymbolInstance, SymbolInstancePools};

/// What adding a layer to the index changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct AddLayerResult {
    pub(crate) buckets_added: bool,
    pub(crate) buckets_removed: bool,
}

/// The index of every symbol layer the renderer places.
#[derive(Debug, Default)]
pub(crate) struct CrossTileSymbolIndex {
    layers: VecMap<String, LayerIndex>,
    /// Ids are unique across layers, and outlive [`reset`](Self::reset).
    ids: IdAllocator,
    /// The last instance id given to a symbol bucket. It outlives `reset`,
    /// so that a bucket indexed again keeps an id no other bucket has.
    last_bucket_instance_id: u32,
    /// The instance ids of the buckets a layer places, as `index_layer`
    /// collects them.
    current_bucket_ids: HashSet<u32>,
}

impl CrossTileSymbolIndex {
    /// Gives the symbols of the buckets in `buckets` on `tiles`, which the
    /// layer `layer_id` places, their cross-tile ids, and forgets the
    /// buckets of the layer it no longer places. `lng` is the view's
    /// unwrapped longitude.
    pub(crate) fn index_layer<'a>(
        &mut self,
        layer_id: &str,
        tiles: impl IntoIterator<Item = &'a PlacingTile>,
        lng: f32,
        view: &View,
        buckets: &mut PlacementBuckets<'_>,
    ) -> AddLayerResult {
        let index = self.layers.get_or_insert_with(layer_id, LayerIndex::default);
        index.follow_wrap_jump(lng);

        self.current_bucket_ids.clear();
        let mut buckets_added = false;
        for tile in tiles {
            let Some(bucket) = buckets.get_mut::<SymbolBucket>(&tile.bucket) else {
                continue;
            };
            if bucket.bucket_instance_id == 0 {
                self.last_bucket_instance_id += 1;
                bucket.bucket_instance_id = self.last_bucket_instance_id;
            }
            let projector = TileProjector::with_data(view, &tile.id, tile.projection);
            buckets_added |= index.add_bucket(tile.overscaled_id, &projector, bucket, &mut self.ids);
            self.current_bucket_ids.insert(bucket.bucket_instance_id);
        }
        let buckets_removed = index.remove_stale_buckets(&self.current_bucket_ids);
        AddLayerResult { buckets_added, buckets_removed }
    }

    /// Forgets the layers that `used` doesn't keep, by their ids.
    pub(crate) fn prune_unused_layers(&mut self, used: impl Fn(&str) -> bool) {
        self.layers.retain(|id, _| used(id));
    }

    /// Forgets every layer.
    pub(crate) fn reset(&mut self) {
        self.layers.clear();
    }
}

/// Hands out cross-tile ids, each new until the `u32`s wrap around.
#[derive(Debug, Default)]
pub(crate) struct IdAllocator {
    last: u32,
}

impl IdAllocator {
    fn allocate(&mut self) -> NonZeroU32 {
        self.last = self.last.wrapping_add(1);
        NonZeroU32::new(self.last).unwrap_or_else(|| {
            self.last = 1;
            NonZeroU32::MIN
        })
    }
}

/// One layer's indexed tiles.
#[derive(Debug, Default)]
pub(crate) struct LayerIndex {
    /// Sorted by tile, so by zoom first.
    tiles: Vec<TileIndex>,
    /// The ids the symbols of each zoom have, as `(zoom, id)`.
    used: HashSet<(u8, NonZeroU32)>,
    /// The unwrapped longitude of the view the tiles' wraps are for.
    lng: f32,
}

impl LayerIndex {
    /// Moves the indexed tiles to the wrap the view has jumped to, if `lng`
    /// is in another one, as when a pan crosses the antimeridian, so that
    /// labels don't flash out and in.
    fn follow_wrap_jump(&mut self, lng: f32) {
        let wrap_delta = ((lng - self.lng) / 360.0).round() as i16;
        if wrap_delta != 0 {
            // Moving every tile by one delta keeps them sorted.
            for index in &mut self.tiles {
                index.tile = index.tile.unwrap_to(index.tile.wrap.wrapping_add(wrap_delta));
            }
        }
        self.lng = lng;
    }

    fn position(&self, tile: &OverscaledTileID) -> Result<usize, usize> {
        self.tiles.binary_search_by(|index| index.tile.cmp(tile))
    }

    /// Gives `bucket`'s symbols, in `tile`, their ids, and indexes them in
    /// place of the bucket indexed for `tile` before. Returns whether it
    /// indexed the bucket: not when it is indexed already with every symbol
    /// in view.
    fn add_bucket(
        &mut self,
        tile: OverscaledTileID,
        projector: &TileProjector<'_>,
        bucket: &mut SymbolBucket,
        ids: &mut IdAllocator,
    ) -> bool {
        let previous = match self.position(&tile) {
            Ok(at) => Some(at),
            // The globe moves a loaded tile to another wrap instead of
            // loading it again: same bucket, same symbols.
            Err(_) => self.rewrap(tile, bucket.bucket_instance_id),
        };
        if let Some(at) = previous {
            let previous = &self.tiles[at];
            if previous.bucket_instance_id == bucket.bucket_instance_id && !bucket.state.has_uninitialized_symbols {
                return false;
            }
            // The new bucket replaces this one, and may take its ids. Its
            // symbols stay indexed until the new ones have matched them.
            for id in previous.ids() {
                self.used.remove(&(tile.overscaled_z, id));
            }
        }

        let state = &mut bucket.state;
        let symbols = &mut state.symbol_instances;
        let mut changed = false;

        // The symbols still without an id, in the bucket's order.
        let mut unmatched: Vec<usize> = Vec::with_capacity(symbols.len());
        state.has_uninitialized_symbols = false;
        let check_viewport = tile.overscale_factor() > 1;
        for (i, symbol) in symbols.iter_mut().enumerate() {
            // An overscaled tile might be only partly in view: the symbols
            // out of view wait for a later add.
            if check_viewport && !is_in_viewport(projector, symbol.anchor().point, symbol.height_offset()) {
                changed |= set_id(symbol, None);
                state.has_uninitialized_symbols = true;
            } else {
                unmatched.push(i);
            }
        }

        let Self { tiles, used, .. } = self;
        for index in tiles.iter() {
            if unmatched.is_empty() {
                break;
            }
            let z = index.tile.overscaled_z;
            let related =
                if z > tile.overscaled_z { index.tile.is_child_of(&tile) } else { index.tile == tile.scaled_to(z) };
            if related && index.bucket_leader_id == bucket.bucket_leader_id {
                unmatched.retain(|&i| {
                    let symbol = &mut symbols[i];
                    let key = state.symbol_instance_pools.key(symbol);
                    let id = index.find_match(key, symbol.anchor().point, tile.canonical, |id| {
                        used.insert((tile.overscaled_z, id))
                    });
                    changed |= id.is_some() && set_id(symbol, id);
                    id.is_none()
                });
            }
        }

        for i in unmatched {
            let id = ids.allocate();
            self.used.insert((tile.overscaled_z, id));
            changed |= set_id(&mut symbols[i], Some(id));
        }
        if changed {
            state.cross_tile_ids_revision = state.cross_tile_ids_revision.wrapping_add(1);
        }

        let index = TileIndex::new(tile, bucket);
        match self.position(&tile) {
            Ok(at) => self.tiles[at] = index,
            Err(at) => self.tiles.insert(at, index),
        }
        true
    }

    /// Moves the index of the bucket `bucket_instance_id` at `tile`'s zoom
    /// and canonical tile, in another wrap, to `tile`, and returns where it
    /// is now.
    fn rewrap(&mut self, tile: OverscaledTileID, bucket_instance_id: u32) -> Option<usize> {
        let from = self.tiles.iter().position(|index| {
            index.bucket_instance_id == bucket_instance_id
                && index.tile.overscaled_z == tile.overscaled_z
                && index.tile.canonical == tile.canonical
        })?;
        let mut index = self.tiles.remove(from);
        index.tile = tile;
        let to = self.position(&tile).unwrap_or_else(|at| at);
        self.tiles.insert(to, index);
        Some(to)
    }

    /// Forgets the buckets not in `current`. Returns whether it forgot any.
    fn remove_stale_buckets(&mut self, current: &HashSet<u32>) -> bool {
        let before = self.tiles.len();
        let used = &mut self.used;
        self.tiles.retain(|index| {
            let current = current.contains(&index.bucket_instance_id);
            if !current {
                for id in index.ids() {
                    used.remove(&(index.tile.overscaled_z, id));
                }
            }
            current
        });
        self.tiles.len() != before
    }
}

/// Sets `symbol`'s id, and returns whether that changed it.
fn set_id(symbol: &mut SymbolInstance, id: Option<NonZeroU32>) -> bool {
    let changed = symbol.cross_tile_id() != id;
    symbol.set_cross_tile_id(id);
    changed
}

/// Whether `point`, `elevation` meters above the ground, is in the viewport
/// or within a viewport's width or height of it, which covers the area of
/// the zoom below.
fn is_in_viewport(projector: &TileProjector<'_>, point: Vec2, elevation: f32) -> bool {
    const EDGE: f64 = 2.0;
    let projected =
        projector.project_as_drawn(&Point::new(f64::from(point.x), f64::from(point.y)), f64::from(elevation));
    let p = &projected.point;
    !projected.occluded && p.x.abs() < EDGE && p.y.abs() < EDGE
}

/// The cell, on the grid of tiles at zoom `grid_z`, of `anchor` in `tile`.
/// The grid has 256 cells per tile side.
fn grid_cell(anchor: Vec2, tile: CanonicalTileID, grid_z: u8) -> I64Vec2 {
    let scale = 256.0 / f64::from(EXTENT) * 2f64.powi(i32::from(grid_z) - i32::from(tile.z));
    let extent = f64::from(EXTENT);
    let x = f64::from(tile.x) * extent + f64::from(anchor.x);
    let y = f64::from(tile.y) * extent + f64::from(anchor.y);
    I64Vec2::new((x * scale).floor() as i64, (y * scale).floor() as i64)
}

/// A bucket's symbols that have an id, in one tile.
#[derive(Clone, Debug)]
struct TileIndex {
    tile: OverscaledTileID,
    bucket_instance_id: u32,
    bucket_leader_id: Arc<str>,
    /// The keys of `symbols`, end to end, in one allocation rather than one
    /// each.
    keys: Vec<u8>,
    /// Sorted by key, and in the bucket's order among equal keys.
    symbols: Vec<IndexedSymbol>,
}

#[derive(Clone, Copy, Debug)]
struct IndexedSymbol {
    /// Where its key is in `keys`.
    key_start: u32,
    key_len: u32,
    id: NonZeroU32,
    /// Its cell on the tile's grid.
    cell: I64Vec2,
}

impl TileIndex {
    fn new(tile: OverscaledTileID, bucket: &SymbolBucket) -> Self {
        let pools: &SymbolInstancePools = &bucket.state.symbol_instance_pools;
        let with_ids =
            || bucket.state.symbol_instances.iter().filter_map(|symbol| Some((symbol, symbol.cross_tile_id()?)));
        let mut keys = Vec::with_capacity(with_ids().map(|(symbol, _)| pools.key(symbol).len()).sum());
        let mut symbols = Vec::with_capacity(with_ids().count());
        for (symbol, id) in with_ids() {
            let key = pools.key(symbol);
            symbols.push(IndexedSymbol {
                key_start: keys.len() as u32,
                key_len: key.len() as u32,
                id,
                cell: grid_cell(symbol.anchor().point, tile.canonical, tile.canonical.z),
            });
            keys.extend_from_slice(key);
        }
        // A stable sort, so symbols with one key stay in the bucket's order.
        symbols.sort_by(|a, b| key_of(&keys, a).cmp(key_of(&keys, b)));
        TileIndex {
            tile,
            bucket_instance_id: bucket.bucket_instance_id,
            bucket_leader_id: Arc::clone(&bucket.bucket_leader_id),
            keys,
            symbols,
        }
    }

    fn ids(&self) -> impl Iterator<Item = NonZeroU32> + '_ {
        self.symbols.iter().map(|symbol| symbol.id)
    }

    /// The id of the first symbol here that matches a symbol with `key` at
    /// `anchor` in `tile` and that `claim` takes, which it does if no symbol
    /// at `tile`'s zoom has the id yet.
    fn find_match(
        &self,
        key: &[u8],
        anchor: Vec2,
        tile: CanonicalTileID,
        mut claim: impl FnMut(NonZeroU32) -> bool,
    ) -> Option<NonZeroU32> {
        let start = self.symbols.partition_point(|symbol| key_of(&self.keys, symbol) < key);
        let len = self.symbols[start..].partition_point(|symbol| key_of(&self.keys, symbol) == key);
        let candidates = &self.symbols[start..][..len];
        if candidates.is_empty() {
            return None;
        }
        // One cell of this tile's grid, or of the child's if `tile` is one.
        let tolerance = 1i64 << self.tile.canonical.z.saturating_sub(tile.z);
        let cell = grid_cell(anchor, tile, self.tile.canonical.z);
        candidates
            .iter()
            .find(|symbol| (symbol.cell - cell).abs().max_element() <= tolerance && claim(symbol.id))
            .map(|symbol| symbol.id)
    }
}

/// The key of `symbol`, in its index's `keys`.
fn key_of<'a>(keys: &'a [u8], symbol: &IndexedSymbol) -> &'a [u8] {
    &keys[symbol.key_start as usize..][..symbol.key_len as usize]
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mln_render::layer::LayerBinders;
    use mln_style::property_value::PropertyValue;
    use mln_style_types::ProjectionDefinition;
    use mln_util::geo::LatLng;
    use mln_util::size::Size;

    use super::*;
    use crate::symbol::SymbolLayoutProperties;
    use crate::symbol::anchor::Anchor;
    use crate::symbol::bucket::{SymbolBucketState, SymbolLayoutFlags, SymbolSizeBinder};
    use crate::symbol::pool::PoolRange;

    fn make_symbol_instance(x: f32, y: f32, key: &str, pools: &mut SymbolInstancePools) -> SymbolInstance {
        SymbolInstance {
            anchor: Anchor::new(x, y, 0.0, Some(0)),
            key: PoolRange::extend(&mut pools.keys, key.bytes()),
            data_feature_index: 0,
            sort_index: 0,
            height_offset: 0.0,
            cross_tile_id: None,
            text: None,
            icon: None,
        }
    }

    /// A bucket of symbols at `(x, y)` with the text `key`.
    fn make_bucket(symbols: Vec<(f32, f32, &str)>, bucket_instance_id: u32) -> SymbolBucket {
        let mut pools = SymbolInstancePools::default();
        let instances = symbols.into_iter().map(|(x, y, key)| make_symbol_instance(x, y, key, &mut pools)).collect();
        let layout = Arc::new(SymbolLayoutProperties::PossiblyEvaluated::default());
        SymbolBucket {
            layout,
            bucket_leader_id: Arc::from("test"),
            flags: SymbolLayoutFlags::default(),
            placement_modes: Vec::new(),
            sort_key_ranges: Vec::new(),
            icon_binders: LayerBinders::default(),
            text_binders: LayerBinders::default(),
            text_size_binder: SymbolSizeBinder::create(0.0, &PropertyValue::Constant(16.0), 16.0),
            icon_size_binder: SymbolSizeBinder::create(0.0, &PropertyValue::Constant(1.0), 1.0),
            tile_pixel_ratio: 1.0,
            max_height_offset: 0.0,
            bucket_instance_id,
            state: SymbolBucketState {
                sorted_angle: f32::MAX,
                symbol_instances: instances,
                symbol_instance_pools: pools,
                ..SymbolBucketState::default()
            },
        }
    }

    fn cross_tile_id(bucket: &SymbolBucket, i: usize) -> Option<u32> {
        bucket.state.symbol_instances[i].cross_tile_id().map(NonZeroU32::get)
    }

    fn camera_at(lat: f64, lon: f64, zoom: f64, globe: bool) -> View {
        let mut state = mln_map_types::TransformState::default();
        state.set_size(Size { width: 512, height: 512 });
        if globe {
            state.set_projection_definition(&ProjectionDefinition::from_name("vertical-perspective"));
        }
        state.set_lat_lng_zoom(&LatLng::new(lat, lon).expect("valid LatLng"), zoom);
        state.view().clone()
    }

    // Only overscaled tiles check the viewport, so any camera does for the
    // others.
    fn any_camera() -> View {
        camera_at(0.0, 0.0, 0.0, false)
    }

    /// A symbol takes the id of a symbol of a parent or child tile with its
    /// key near its position, and a new id without one; a removed tile's
    /// symbols match no more.
    #[test]
    fn a_symbol_takes_the_id_of_its_match_in_a_parent_or_child_tile() {
        let any_camera = any_camera();
        let mut max_bucket_instance_id = 0u32;
        let mut index = LayerIndex::default();
        let mut ids = IdAllocator::default();

        let main_id = OverscaledTileID::with_zxy(6, 0, 6, 8, 8);
        max_bucket_instance_id += 1;
        let mut main_bucket =
            make_bucket(vec![(1000.0, 1000.0, "Detroit"), (2000.0, 2000.0, "Toronto")], max_bucket_instance_id);
        index.add_bucket(
            main_id,
            &TileProjector::new(&any_camera, &main_id.to_unwrapped()),
            &mut main_bucket,
            &mut ids,
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
            child_id,
            &TileProjector::new(&any_camera, &child_id.to_unwrapped()),
            &mut child_bucket,
            &mut ids,
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
            parent_id,
            &TileProjector::new(&any_camera, &parent_id.to_unwrapped()),
            &mut parent_bucket,
            &mut ids,
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
            grandchild_id,
            &TileProjector::new(&any_camera, &grandchild_id.to_unwrapped()),
            &mut grandchild_bucket,
            &mut ids,
        );

        // Matches the symbol in the main bucket.
        assert_eq!(cross_tile_id(&grandchild_bucket, 0), Some(1));
        // Does not match the previous value for Windsor because that tile was removed
        assert_eq!(cross_tile_id(&grandchild_bucket, 1), Some(5));
    }

    #[test]
    fn tile_moved_to_another_wrap_keeps_its_symbols() {
        let any_camera = any_camera();
        // The globe moves a loaded tile to another wrap instead of loading it
        // again: its bucket keeps the IDs.
        let mut index = LayerIndex::default();
        let mut ids = IdAllocator::default();

        let mut bucket = make_bucket(vec![(1000.0, 1000.0, "Detroit"), (2000.0, 2000.0, "Toronto")], 1);

        let id = OverscaledTileID::with_zxy(0, 0, 0, 0, 0);
        assert!(index.add_bucket(id, &TileProjector::new(&any_camera, &id.to_unwrapped()), &mut bucket, &mut ids));
        assert_eq!(cross_tile_id(&bucket, 0), Some(1));
        assert_eq!(cross_tile_id(&bucket, 1), Some(2));

        let moved = id.unwrap_to(1);
        assert!(!index.add_bucket(
            moved,
            &TileProjector::new(&any_camera, &moved.to_unwrapped()),
            &mut bucket,
            &mut ids
        ));
        assert_eq!(cross_tile_id(&bucket, 0), Some(1));
        assert_eq!(cross_tile_id(&bucket, 1), Some(2));
        let current_ids: HashSet<u32> = [1].into_iter().collect();
        assert!(!index.remove_stale_buckets(&current_ids));
    }

    /// A tile added again after its removal takes the ids of the symbols
    /// that it matches now, not the ones it had.
    #[test]
    fn a_tile_added_again_takes_the_ids_it_matches_now() {
        let any_camera = any_camera();
        let mut index = LayerIndex::default();
        let mut ids = IdAllocator::default();

        let main_id = OverscaledTileID::with_zxy(6, 0, 6, 8, 8);
        let mut main_bucket = make_bucket(vec![(1000.0, 1000.0, "Detroit")], 1);

        let child_id = OverscaledTileID::with_zxy(7, 0, 7, 16, 16);
        let mut child_bucket = make_bucket(vec![(2000.0, 2000.0, "Detroit")], 2);

        // assigns a new id
        index.add_bucket(
            main_id,
            &TileProjector::new(&any_camera, &main_id.to_unwrapped()),
            &mut main_bucket,
            &mut ids,
        );
        assert_eq!(cross_tile_id(&main_bucket, 0), Some(1));

        // removes the tile
        let current_ids: HashSet<u32> = HashSet::default();
        index.remove_stale_buckets(&current_ids);

        // assigns a new id
        index.add_bucket(
            child_id,
            &TileProjector::new(&any_camera, &child_id.to_unwrapped()),
            &mut child_bucket,
            &mut ids,
        );
        assert_eq!(cross_tile_id(&child_bucket, 0), Some(2));

        // overwrites the old id to match the already-added tile
        index.add_bucket(
            main_id,
            &TileProjector::new(&any_camera, &main_id.to_unwrapped()),
            &mut main_bucket,
            &mut ids,
        );
        assert_eq!(cross_tile_id(&main_bucket, 0), Some(2));
    }

    /// Each id of a parent's symbols goes to one symbol of a child tile:
    /// a third symbol that matches the same place gets a new one.
    #[test]
    fn a_parents_id_goes_to_one_symbol_of_a_child_tile() {
        let any_camera = any_camera();
        let mut index = LayerIndex::default();
        let mut ids = IdAllocator::default();

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
            main_id,
            &TileProjector::new(&any_camera, &main_id.to_unwrapped()),
            &mut main_bucket,
            &mut ids,
        );
        assert_eq!(cross_tile_id(&main_bucket, 0), Some(1));
        assert_eq!(cross_tile_id(&main_bucket, 1), Some(2));

        // copies parent ids without duplicate ids in this tile
        index.add_bucket(
            child_id,
            &TileProjector::new(&any_camera, &child_id.to_unwrapped()),
            &mut child_bucket,
            &mut ids,
        );
        assert_eq!(cross_tile_id(&child_bucket, 0), Some(1)); // A' copies from A
        assert_eq!(cross_tile_id(&child_bucket, 1), Some(2)); // B' copies from B
        assert_eq!(cross_tile_id(&child_bucket, 2), Some(3)); // C' gets new ID
    }

    /// A tile's new bucket takes the ids of its old bucket's symbols, each
    /// once.
    #[test]
    fn a_tiles_new_bucket_takes_the_ids_of_its_old_one() {
        let any_camera = any_camera();
        let mut index = LayerIndex::default();
        let mut ids = IdAllocator::default();

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
            tile_id,
            &TileProjector::new(&any_camera, &tile_id.to_unwrapped()),
            &mut first_bucket,
            &mut ids,
        );
        assert_eq!(cross_tile_id(&first_bucket, 0), Some(1));
        assert_eq!(cross_tile_id(&first_bucket, 1), Some(2));

        // copies parent ids without duplicate ids in this tile
        index.add_bucket(
            tile_id,
            &TileProjector::new(&any_camera, &tile_id.to_unwrapped()),
            &mut second_bucket,
            &mut ids,
        );
        assert_eq!(cross_tile_id(&second_bucket, 0), Some(1)); // A' copies from A
        assert_eq!(cross_tile_id(&second_bucket, 1), Some(2)); // B' copies from B
        assert_eq!(cross_tile_id(&second_bucket, 2), Some(3)); // C' gets new ID
    }

    fn expect_offscreen_symbols_skipped(globe: bool) {
        let mut index = LayerIndex::default();
        let mut ids = IdAllocator::default();

        let tile_id = OverscaledTileID::with_zxy(7, 0, 6, 18, 24);
        let mut symbol_bucket = make_bucket(vec![(1000.0, 1000.0, "Washington"), (2000.0, 2000.0, "Richmond")], 1);
        let away = camera_at(60.0, 25.0, 7.0, globe);
        index.add_bucket(tile_id, &TileProjector::new(&away, &tile_id.to_unwrapped()), &mut symbol_bucket, &mut ids);

        assert_eq!(cross_tile_id(&symbol_bucket, 0), None);
        assert_eq!(cross_tile_id(&symbol_bucket, 1), None);

        let over = camera_at(39.0, -76.0, 7.0, globe);
        index.add_bucket(tile_id, &TileProjector::new(&over, &tile_id.to_unwrapped()), &mut symbol_bucket, &mut ids);

        assert_eq!(cross_tile_id(&symbol_bucket, 0), Some(1));
        assert_eq!(cross_tile_id(&symbol_bucket, 1), Some(2));
    }

    // An overscaled tile with symbols off the screen registers again each
    // frame. Under the same camera that changes no symbol's id; a camera
    // that leaves a symbol and comes back gives it a new one. Placement
    // keys symbols by their ids, so the bucket's revision counts the
    // changes.
    #[test]
    fn registering_again_revises_the_ids_only_when_they_change() {
        let mut index = LayerIndex::default();
        let mut ids = IdAllocator::default();
        let tile_id = OverscaledTileID::with_zxy(7, 0, 6, 18, 24);
        let mut symbol_bucket = make_bucket(vec![(1000.0, 1000.0, "Pittsburgh"), (8000.0, 8000.0, "Norfolk")], 1);
        let over = camera_at(40.5, -78.0, 7.0, false);
        let away = camera_at(60.0, 25.0, 7.0, false);
        let mut register = |camera: &View| {
            let projector = TileProjector::new(camera, &tile_id.to_unwrapped());
            assert!(index.add_bucket(tile_id, &projector, &mut symbol_bucket, &mut ids));
            let ids = (cross_tile_id(&symbol_bucket, 0), cross_tile_id(&symbol_bucket, 1));
            (ids, symbol_bucket.state.cross_tile_ids_revision)
        };

        let (ids, revision) = register(&over);
        assert_eq!(ids.1, None, "a symbol off the screen");
        assert_eq!(register(&over), (ids, revision), "the same camera");

        assert_eq!(register(&away).0, (None, None));
        let (back, back_revision) = register(&over);
        assert_ne!(back, ids, "a new id for the symbol the camera left");
        assert_ne!(back_revision, revision);
    }

    /// An overscaled tile's symbols off the screen get no id until the
    /// camera shows them.
    #[test]
    fn an_overscaled_tiles_symbols_off_the_screen_get_no_id() {
        expect_offscreen_symbols_skipped(false);
    }

    /// As `an_overscaled_tiles_symbols_off_the_screen_get_no_id`, on the
    /// globe, where the anchors go through the sphere, not the tile matrix.
    #[test]
    fn an_overscaled_tiles_symbols_off_the_screen_get_no_id_on_the_globe() {
        expect_offscreen_symbols_skipped(true);
    }
}
