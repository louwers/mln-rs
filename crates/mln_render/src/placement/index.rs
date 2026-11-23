//! The collision index of a placement: the shapes placed so far, which
//! each shape placed after them may not collide with, and the features they
//! stand for, which rendered-feature queries of the placement find.

use std::cmp::Reverse;
use std::sync::Arc;

use glam::{DVec2, Vec2};
use mln_map_types::{MapMode, View};
use mln_style::expression::GlobalStateMap;
use mln_tile_id::OverscaledTileID;
use mln_tile_id::geometry_tile_data::GeometryCoordinates;
use mln_util::error::Error;
use mln_util::geo::ScreenCoordinate;
use mln_util::hash::{HashMap, HashSet};
use mln_util::intersection_tests::polygon_intersects_polygon;

use super::erased::LaidBuckets;
use super::grid::{Aabb, Grid, Shape};
use super::{BucketKey, PlacingTile};
use crate::erased_layer::ErasedEvaluatedLayer;
use crate::geometry::feature_index::{FeatureIndex, FeatureSelection, QueryLayers, QueryResults};
use crate::query::RenderedQueryOptions;

/// How far the grid reaches past each edge of the viewport, in pixels.
/// Shapes that cross into the viewport change what shows near its edge,
/// so the padding moves most of those changes offscreen; more padding
/// steadies what shows, and costs more.
const VIEWPORT_PADDING: f32 = 100.0;
/// A still image of a tile places as far again as a tile reaches across
/// its borders, so the tile's neighbors agree on what crosses them.
const VIEWPORT_PADDING_TILE: f32 = 1024.0;
/// The width of a grid cell, in pixels.
const CELL_SIZE: f32 = 25.0;

/// The record of a tile whose layout lost its layer, whose shapes queries
/// don't find.
const NO_RECORD: u32 = u32::MAX;

/// How a shape may overlap the shapes placed before it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Overlap {
    /// Collides with every shape.
    #[default]
    Never,
    /// Collides with none.
    Always,
    /// Collides only with the shapes placed with `Never`.
    Cooperative,
}

/// What a shape inserted in a [`CollisionIndex`] stands for.
#[derive(Clone, Copy, Debug)]
pub struct Item {
    /// The index, among its layer's tiles, of the tile whose feature the
    /// shape is.
    pub tile: u32,
    /// The feature's index in its tile's source layer, which queries look
    /// up.
    pub feature: u32,
    /// Where queries return the feature among those of its tile: the
    /// greater first, unless the bucket set an order of its own
    /// ([`PlacementCx::set_query_order`](super::PlacementCx::set_query_order)).
    pub sort: u32,
    /// How the shape was placed, which a later shape placed with
    /// [`Overlap::Cooperative`] reads.
    pub overlap: Overlap,
    /// Whether the shape stays out of the way of the shapes placed after
    /// it: they don't collide with it, and queries still find it.
    pub ignore_placement: bool,
}

/// A shape's [`Item`] as the grids hold it.
#[derive(Clone, Copy, Debug)]
struct Entry {
    /// The index of the shape's tile among the index's records, or
    /// [`NO_RECORD`].
    record: u32,
    feature: u32,
    sort: u32,
    group: u16,
    overlap: Overlap,
}

/// A tile of a layer that places, whose features the shapes of its
/// [`Entry`]s are: what a query looks them up in.
struct Record {
    tile: OverscaledTileID,
    features: Arc<FeatureIndex>,
    /// The layer that leads the tile's bucket, as its layout laid it out,
    /// which names the features' source layer and the layers that draw
    /// them.
    leader: Arc<dyn ErasedEvaluatedLayer>,
    /// The order of the bucket's features that queries return them in the
    /// reverse of, if the bucket set one.
    order: Option<Arc<[u32]>>,
}

/// A placing layer of a placement, as the index places its shapes: their
/// collision group, and where its tiles' records are.
#[derive(Clone, Copy, Debug, Default)]
struct PlacingLayerSlot {
    group: Option<u16>,
    first_tile: usize,
}

/// The shapes a placement places, in viewport pixels plus
/// [`padding`](Self::padding) on each side: one grid that later shapes
/// collide with, and one of the shapes placed with
/// [`Item::ignore_placement`], which they don't. Each layer's shapes are in
/// its source's collision group: without cross-source collisions, a shape
/// collides only with those of its group.
pub struct CollisionIndex {
    /// The viewport's size, in pixels.
    size: DVec2,
    padding: f32,
    /// The viewport's far corner in the grid.
    screen_end: Vec2,
    /// The grid's size: the viewport and its padding on each side.
    grid_end: Vec2,
    collision: Grid<Entry>,
    ignored: Grid<Entry>,
    /// The layer whose shapes follow.
    layer: PlacingLayerSlot,
    /// The placement's layers, bottom to top.
    layers: Vec<PlacingLayerSlot>,
    /// The record of each tile of each layer, by `first_tile` and the
    /// tile's index; [`NO_RECORD`] for a tile whose layout lost the layer.
    tile_records: Vec<u32>,
    records: Vec<Record>,
    /// Where each record is, by the address of its tile's feature index
    /// and its bucket's index, which name the bucket of one layout.
    by_bucket: HashMap<(usize, usize), u32>,
    /// The groups of the sources, in the order the layers have them.
    groups: Vec<crate::tile::job::SourceKey>,
}

/// An empty index, whose grids the first placement sizes to its view.
impl Default for CollisionIndex {
    fn default() -> Self {
        CollisionIndex {
            size: DVec2::ZERO,
            padding: 0.0,
            screen_end: Vec2::ZERO,
            grid_end: Vec2::ONE,
            collision: Grid::new(Vec2::ONE, CELL_SIZE),
            ignored: Grid::new(Vec2::ONE, CELL_SIZE),
            layer: PlacingLayerSlot::default(),
            layers: Vec::new(),
            tile_records: Vec::new(),
            records: Vec::new(),
            by_bucket: HashMap::default(),
            groups: Vec::new(),
        }
    }
}

impl CollisionIndex {
    /// How far the grid reaches past each edge of the viewport, in pixels:
    /// where a shape at the viewport's top-left corner is.
    #[inline]
    #[must_use]
    pub fn padding(&self) -> f32 {
        self.padding
    }

    /// Where `ndc`, a point in normalized device coordinates (y up), is in
    /// the grid (y down).
    #[inline]
    #[must_use]
    pub fn to_grid(&self, ndc: DVec2) -> Vec2 {
        let padding = f64::from(self.padding);
        Vec2::new(
            (f64::midpoint(ndc.x, 1.0) * self.size.x + padding) as f32,
            (f64::midpoint(-ndc.y, 1.0) * self.size.y + padding) as f32,
        )
    }

    /// Whether `bounds` reach into the grid. A shape outside it can't
    /// place, though [`collides`](Self::collides) doesn't say so.
    #[inline]
    #[must_use]
    pub fn in_grid(&self, bounds: &Aabb) -> bool {
        bounds.max.x >= 0.0 && bounds.min.x < self.grid_end.x && bounds.max.y >= 0.0 && bounds.min.y < self.grid_end.y
    }

    /// Whether `bounds` are wholly outside the viewport, in its padding or
    /// past it.
    #[inline]
    #[must_use]
    pub fn offscreen(&self, bounds: &Aabb) -> bool {
        bounds.max.x < self.padding
            || bounds.min.x >= self.screen_end.x
            || bounds.max.y < self.padding
            || bounds.min.y >= self.screen_end.y
    }

    /// Whether `shape`, placed with `overlap`, collides with a shape placed
    /// before it.
    #[inline]
    #[must_use]
    pub fn collides(&self, shape: &Shape, overlap: Overlap) -> bool {
        let group = self.layer.group;
        let in_group = |entry: &Entry| group.is_none_or(|group| entry.group == group);
        match overlap {
            Overlap::Never => self.collision.any(shape, in_group),
            Overlap::Always => false,
            Overlap::Cooperative => {
                self.collision.any(shape, |entry| entry.overlap == Overlap::Never && in_group(entry))
            }
        }
    }

    /// Places `shape`, which stands for `item`.
    #[inline]
    pub fn insert(&mut self, shape: Shape, item: Item) {
        let record = self.tile_records.get(self.layer.first_tile + item.tile as usize).copied();
        debug_assert!(record.is_some(), "an item of a tile that its layer doesn't have");
        let record = record.unwrap_or(NO_RECORD);
        let entry = Entry {
            record,
            feature: item.feature,
            sort: item.sort,
            group: self.layer.group.unwrap_or(0),
            overlap: item.overlap,
        };
        let grid = if item.ignore_placement { &mut self.ignored } else { &mut self.collision };
        grid.insert(entry, shape);
    }

    /// Makes `layer`, among the placement's layers bottom to top, the one
    /// whose shapes follow: they take its source's collision group, and
    /// their [`Item::tile`] indexes its tiles.
    pub(crate) fn select_layer(&mut self, layer: usize) {
        self.layer = self.layers.get(layer).copied().unwrap_or_default();
    }

    /// Empties the index for a placement in `view`, in `mode`, keeping the
    /// grids' cells while the view keeps its size.
    pub(crate) fn begin(&mut self, view: &View, mode: MapMode) {
        let padding = match mode {
            MapMode::Tile => VIEWPORT_PADDING_TILE,
            _ if view.pitch() == 0.0 => VIEWPORT_PADDING,
            _ => VIEWPORT_PADDING * 2.0,
        };
        let size = view.size();
        let (width, height) = (size.width as f32, size.height as f32);
        self.size = DVec2::new(f64::from(size.width), f64::from(size.height));
        self.padding = padding;
        self.screen_end = Vec2::new(width + padding, height + padding);
        self.grid_end = Vec2::new(width + 2.0 * padding, height + 2.0 * padding);
        self.collision.reset(self.grid_end, CELL_SIZE);
        self.ignored.reset(self.grid_end, CELL_SIZE);
        self.clear_records();
    }

    /// Adds the placement's layers, bottom to top, each by its id with the
    /// tiles whose bucket it leads, whose layouts are in `store`. The layers
    /// place topmost first,
    /// and each source's layers share a collision group unless
    /// `cross_source_collisions`, where every layer's shapes collide with
    /// every other's.
    pub(crate) fn add_layers<'a>(
        &mut self,
        layers: impl DoubleEndedIterator<Item = (&'a str, &'a [PlacingTile])> + ExactSizeIterator,
        store: &dyn LaidBuckets,
        cross_source_collisions: bool,
    ) {
        self.layers.clear();
        self.layers.resize(layers.len(), PlacingLayerSlot::default());
        for (position, (id, tiles)) in layers.enumerate().rev() {
            let group = tiles.first().filter(|_| !cross_source_collisions).map(|tile| {
                let source = tile.bucket.source;
                let group = self.groups.iter().position(|&other| other == source).unwrap_or_else(|| {
                    self.groups.push(source);
                    self.groups.len() - 1
                });
                group as u16 + 1
            });
            self.layers[position] = PlacingLayerSlot { group, first_tile: self.tile_records.len() };
            for tile in tiles {
                let record = self.add_record(id, tile, store).unwrap_or(NO_RECORD);
                self.tile_records.push(record);
            }
        }
    }

    /// Adds the record of `tile` of the layer `id`, which leads the tile's
    /// bucket.
    fn add_record(&mut self, id: &str, tile: &PlacingTile, store: &dyn LaidBuckets) -> Option<u32> {
        let layout = store.layout(&tile.bucket)?;
        let leader = layout.layer_render_data.layer(id)?;
        let record = self.records.len() as u32;
        self.by_bucket.insert((Arc::as_ptr(&layout.feature_index) as usize, tile.bucket.bucket), record);
        self.records.push(Record {
            tile: tile.overscaled_id,
            features: Arc::clone(&layout.feature_index),
            leader: Arc::clone(leader),
            order: None,
        });
        Some(record)
    }

    /// Has queries return the features of the bucket at `bucket`, of the
    /// layout in `store`, in the reverse of `order`, if a record has it.
    pub(crate) fn set_query_order(&mut self, store: &dyn LaidBuckets, bucket: &BucketKey, order: Option<Arc<[u32]>>) {
        let Some(layout) = store.layout(bucket) else { return };
        let key = (Arc::as_ptr(&layout.feature_index) as usize, bucket.bucket);
        if let Some(&record) = self.by_bucket.get(&key) {
            self.records[record as usize].order = order;
        }
    }

    /// Empties the index, keeping its view and its grids' cells.
    pub(crate) fn clear(&mut self) {
        self.collision.reset(self.grid_end, CELL_SIZE);
        self.ignored.reset(self.grid_end, CELL_SIZE);
        self.clear_records();
    }

    /// Drops the records, which hold their tiles' feature indexes, and the
    /// layers.
    pub(crate) fn clear_records(&mut self) {
        self.layer = PlacingLayerSlot::default();
        self.layers.clear();
        self.tile_records.clear();
        self.records.clear();
        self.by_bucket.clear();
        self.groups.clear();
    }

    /// Adds to `results` the features of `layers` whose shapes the query
    /// `geometry`, in the view's pixels, touches: tile by tile, from the
    /// deepest zoom and from the top of the view, and within a tile in the
    /// reverse of its bucket's order, or of their [`Item::sort`]. A shape is
    /// hit where the query touches its bounding box.
    pub(crate) fn query_rendered_features(
        &self,
        geometry: &[ScreenCoordinate],
        layers: &QueryLayers<'_>,
        options: &RenderedQueryOptions,
        global_state: Option<&GlobalStateMap>,
        results: &mut QueryResults,
    ) -> Result<(), Error> {
        if geometry.is_empty() || (self.collision.is_empty() && self.ignored.is_empty()) {
            return Ok(());
        }
        let padding = self.padding;
        let polygon: GeometryCoordinates = geometry
            .iter()
            .map(|point| {
                // The integer polygon test limits the query's accuracy to a
                // pixel.
                let (x, y) = (point.x as f32 + padding, point.y as f32 + padding);
                geometry::Point::new(x as i16, y as i16)
            })
            .collect();
        let mut envelope = Aabb::new(Vec2::INFINITY, Vec2::NEG_INFINITY);
        for point in geometry {
            let point = Vec2::new(point.x as f32 + padding, point.y as f32 + padding);
            envelope.min = envelope.min.min(point);
            envelope.max = envelope.max.max(point);
        }
        let envelope = Shape::Box(envelope);

        // Each feature of a tile once, at the first of its shapes that the
        // query touches.
        let mut seen: HashSet<(u32, u32)> = HashSet::default();
        let mut hits: Vec<Entry> = Vec::new();
        for (entry, shape) in self.collision.query(&envelope).chain(self.ignored.query(&envelope)) {
            if entry.record == NO_RECORD || seen.contains(&(entry.record, entry.feature)) {
                continue;
            }
            if !polygon_intersects_box(&polygon, &shape.bounds()) {
                continue;
            }
            seen.insert((entry.record, entry.feature));
            hits.push(*entry);
        }

        // A stable sort: the records of a tile keep the order they were
        // added in, and a record's hits the order the grids found them in.
        let tile_order = |record: u32| {
            let tile = &self.records[record as usize].tile;
            (tile.canonical.z, tile.canonical.y, tile.wrap, tile.canonical.x)
        };
        hits.sort_by(|a, b| tile_order(a.record).cmp(&tile_order(b.record)).then(a.record.cmp(&b.record)));
        let mut ranks: Vec<(u32, usize)> = Vec::new();
        for hits in hits.chunk_by_mut(|a, b| a.record == b.record) {
            let record = &self.records[hits[0].record as usize];
            match &record.order {
                Some(order) => {
                    // Each hit feature's first place in the order, found in
                    // one pass over it; a feature it lacks goes last.
                    ranks.clear();
                    ranks.extend(hits.iter().map(|entry| (entry.feature, order.len())));
                    ranks.sort_unstable();
                    ranks.dedup_by_key(|(feature, _)| *feature);
                    for (position, feature) in order.iter().enumerate() {
                        if let Ok(at) = ranks.binary_search_by_key(feature, |(feature, _)| *feature) {
                            ranks[at].1 = ranks[at].1.min(position);
                        }
                    }
                    let rank = |entry: &Entry| {
                        ranks.binary_search_by_key(&entry.feature, |(feature, _)| *feature).map_or(order.len(), |at| ranks[at].1)
                    };
                    hits.sort_by_key(|entry| Reverse(rank(entry)));
                }
                None => hits.sort_by_key(|entry| Reverse(entry.sort)),
            }
            let style = record.leader.style().base();
            let selection =
                FeatureSelection { layers, options, global_state, tile_id: &record.tile, feature_state: None };
            let features = hits.iter().map(|entry| entry.feature as usize);
            for (layer_id, features) in
                record.features.lookup_features(features, &style.source_layer, &style.id, &selection)?
            {
                results.entry(layer_id).or_default().extend(features);
            }
        }
        Ok(())
    }
}

/// Whether `polygon` meets `bounds`, with both in whole pixels.
fn polygon_intersects_box(polygon: &[geometry::Point<i16>], bounds: &Aabb) -> bool {
    let (min_x, min_y) = (bounds.min.x as i16, bounds.min.y as i16);
    let (max_x, max_y) = (bounds.max.x as i16, bounds.max.y as i16);
    let corners = [
        geometry::Point::new(min_x, min_y),
        geometry::Point::new(max_x, min_y),
        geometry::Point::new(max_x, max_y),
        geometry::Point::new(min_x, max_y),
    ];
    polygon_intersects_polygon(polygon, &corners)
}

#[cfg(test)]
mod tests {
    use mln_map_types::TransformState;
    use mln_util::size::Size;

    use super::*;

    /// An index over a 512 px view, with one layer selected, whose one tile
    /// has no record.
    fn index(mode: MapMode) -> CollisionIndex {
        let mut state = TransformState::default();
        state.set_size(Size::new(512, 512));
        let mut index = CollisionIndex::default();
        index.begin(state.view(), mode);
        index.tile_records.push(NO_RECORD);
        index
    }

    fn square(min: f32, max: f32) -> Shape {
        Shape::Box(Aabb::new(Vec2::splat(min), Vec2::splat(max)))
    }

    fn item(overlap: Overlap, ignore_placement: bool) -> Item {
        Item { tile: 0, feature: 0, sort: 0, overlap, ignore_placement }
    }

    /// Whether a box over the one `index` holds collides, placed with
    /// `overlap`.
    fn collides(index: &CollisionIndex, overlap: Overlap) -> bool {
        index.collides(&square(150.0, 250.0), overlap)
    }

    #[test]
    fn every_mode_but_always_collides_with_a_never_shape() {
        let mut index = index(MapMode::Static);
        index.insert(square(100.0, 200.0), item(Overlap::Never, false));
        assert!(collides(&index, Overlap::Never));
        assert!(!collides(&index, Overlap::Always));
        assert!(collides(&index, Overlap::Cooperative));
    }

    #[test]
    fn only_never_collides_with_an_overlapping_shape() {
        for placed in [Overlap::Always, Overlap::Cooperative] {
            let mut index = index(MapMode::Static);
            index.insert(square(100.0, 200.0), item(placed, false));
            assert!(collides(&index, Overlap::Never), "never over {placed:?}");
            assert!(!collides(&index, Overlap::Always), "always over {placed:?}");
            assert!(!collides(&index, Overlap::Cooperative), "cooperative over {placed:?}");
        }
    }

    #[test]
    fn a_shape_that_ignores_placement_blocks_nothing() {
        let mut index = index(MapMode::Static);
        index.insert(square(100.0, 200.0), item(Overlap::Never, true));
        assert!(!collides(&index, Overlap::Never));
    }

    #[test]
    fn shapes_collide_within_their_layers_group() {
        let mut index = index(MapMode::Static);
        index.layers = vec![
            PlacingLayerSlot { group: Some(1), first_tile: 0 },
            PlacingLayerSlot { group: Some(2), first_tile: 0 },
        ];
        index.select_layer(0);
        index.insert(square(100.0, 200.0), item(Overlap::Never, false));
        assert!(collides(&index, Overlap::Never));
        assert!(collides(&index, Overlap::Cooperative));
        index.select_layer(1);
        assert!(!collides(&index, Overlap::Never));
        assert!(!collides(&index, Overlap::Cooperative));
    }

    #[test]
    fn the_grid_reaches_past_the_viewport_by_its_padding() {
        let index = index(MapMode::Static);
        assert_eq!(index.padding(), 100.0);
        assert_eq!(index.to_grid(DVec2::new(-1.0, 1.0)), Vec2::splat(100.0));
        assert_eq!(index.to_grid(DVec2::new(1.0, -1.0)), Vec2::splat(612.0));
        let bounds = |min: f32, max: f32| Aabb::new(Vec2::splat(min), Vec2::splat(max));
        assert!(index.in_grid(&bounds(-10.0, 0.0)));
        assert!(!index.in_grid(&bounds(712.0, 720.0)));
        assert!(index.offscreen(&bounds(0.0, 99.0)));
        assert!(!index.offscreen(&bounds(0.0, 100.0)));
        assert!(index.offscreen(&bounds(612.0, 620.0)));
        assert_eq!(self::index(MapMode::Tile).padding(), 1024.0);
    }
}
