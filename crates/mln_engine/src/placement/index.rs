//! The collision index of a placement: the shapes placed so far, which each
//! shape placed after them may not collide with, and the features they stand
//! for, which rendered-feature queries of the placement find.

use std::sync::Arc;

use geometry::Point;
use glam::{DVec2, Vec2};
use mln_style::hash::{HashMap, HashSet};
use smallvec::SmallVec;

use super::grid::{Aabb, Grid, Shape};
use crate::camera::View;
use crate::geo::ScreenLineString;
use crate::map::MapMode;
use crate::query::feature_index::{FeatureIndex, FeatureSortOrder, IndexedSubfeature};
use crate::query::shapes::meets_polygon;
use crate::tile::{OverscaledTileID, UnwrappedTileID};
use crate::worker::job::SourceKey;

/// How far the grid reaches past each edge of the viewport, in pixels.
/// Shapes that cross into the viewport change what shows near its edge, so
/// the padding moves most of those changes offscreen; more padding steadies
/// what shows, and costs more.
const VIEWPORT_PADDING_DEFAULT: f32 = 100.0;
/// The padding of a still image of a tile, which must be much larger so
/// that what crosses the tile's borders isn't cut off.
const VIEWPORT_PADDING_FOR_STATIC_TILES: f32 = 1024.0;
/// The width of a grid cell, in pixels.
const CELL_SIZE: f32 = 25.0;

fn find_viewport_padding(view: &View, map_mode: MapMode) -> f32 {
    if map_mode == MapMode::Tile {
        return VIEWPORT_PADDING_FOR_STATIC_TILES;
    }
    if view.pitch() == 0.0 { VIEWPORT_PADDING_DEFAULT } else { VIEWPORT_PADDING_DEFAULT * 2.0 }
}

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

/// The shapes a shape collides with: with the map's cross-source
/// collisions, those of every layer; without, those of its own layer's
/// source.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CollisionGroup(Option<u16>);

impl CollisionGroup {
    /// The group of every shape, as with the map's cross-source collisions.
    pub const ALL: CollisionGroup = CollisionGroup(None);

    /// The id of the group's shapes in the grid: 0 for every group.
    fn id(self) -> u16 {
        self.0.unwrap_or(0)
    }

    /// Whether a shape of this group collides with `feature`.
    fn has(self, feature: &IndexedSubfeature) -> bool {
        self.0.is_none_or(|group| feature.collision_group_id == group)
    }
}

/// The collision groups of the sources of a placement's layers.
pub(crate) struct CollisionGroups<'a> {
    ids: SmallVec<[&'a str; 4]>,
    cross_source_collisions: bool,
}

impl<'a> CollisionGroups<'a> {
    pub(crate) fn new(cross_source_collisions: bool) -> Self {
        CollisionGroups { ids: SmallVec::new(), cross_source_collisions }
    }

    /// The group of the shapes of the layers on `source`.
    pub(crate) fn get(&mut self, source: &'a str) -> CollisionGroup {
        if self.cross_source_collisions {
            return CollisionGroup(None);
        }
        let index = self.ids.iter().position(|id| *id == source).unwrap_or_else(|| {
            self.ids.push(source);
            self.ids.len() - 1
        });
        CollisionGroup(Some(index as u16 + 1))
    }
}

/// Where a placed mark stands: a point of a tile, in its units, and the
/// height above sea level, in meters, at which the mark's draws hide it
/// behind the ground in front of it, over terrain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stand {
    pub tile: UnwrappedTileID,
    pub point: Point<f64>,
    pub height: f64,
}

/// A shape in a grid: the feature it stands for, and where the mark it is
/// of stands, in the index's `stands`.
struct Placed {
    feature: IndexedSubfeature,
    stands: Option<u32>,
}

/// What a shape inserted in a [`CollisionIndex`] stands for.
#[derive(Clone, Copy)]
pub struct Item<'a> {
    /// The feature, as queries return it.
    pub feature: &'a IndexedSubfeature,
    /// The record of the feature's bucket ([`CollisionIndex::retain`]),
    /// which queries look the feature up in.
    pub record: u32,
    /// How the shape was placed, which a later shape placed with
    /// [`Overlap::Cooperative`] reads.
    pub overlap: Overlap,
    /// Whether the shape stays out of the way of the shapes placed after
    /// it: they don't collide with it, and queries still find it.
    pub ignore_placement: bool,
    /// Where the mark the shape is of stands, for a mark whose draws hide
    /// it behind the ground in front of it: queries find it only where the
    /// ground doesn't hide that point.
    pub stands: Option<Stand>,
}

/// What queries of a placement read of a bucket whose shapes it placed.
#[derive(Debug)]
pub struct RetainedQueryData {
    pub bucket_instance_id: u32,
    pub feature_index: Option<Arc<FeatureIndex>>,
    pub tile_id: OverscaledTileID,
    /// The tile's source, whose feature state queries return.
    pub source: SourceKey,
    /// The order of the bucket's features as the bucket last sorted them,
    /// for a bucket whose features sort by their position.
    pub feature_sort_order: FeatureSortOrder,
}

/// The shapes a placement places, in viewport pixels plus
/// [`viewport_padding`](Self::viewport_padding) on each side: one grid that
/// later shapes collide with, and one of the shapes placed with
/// [`Item::ignore_placement`], which they don't. It keeps the query data of
/// the buckets whose shapes it holds.
pub struct CollisionIndex {
    view: View,
    /// The viewport's size, in pixels.
    size: DVec2,

    viewport_padding: f32,
    collision_grid: Grid<Placed>,
    ignored_grid: Grid<Placed>,
    /// Where the marks of the shapes stand ([`Item::stands`]).
    stands: Vec<Stand>,

    screen_right_boundary: f32,
    screen_bottom_boundary: f32,
    grid_right_boundary: f32,
    grid_bottom_boundary: f32,

    retained_query_data: HashMap<u32, RetainedQueryData>,
}

impl CollisionIndex {
    /// An empty collision index of `view` in `map_mode`.
    #[must_use]
    pub fn new(view: &View, map_mode: MapMode) -> Self {
        CollisionIndex::reusing(view, map_mode, None)
    }

    /// An empty collision index, as [`new`](Self::new) makes, that reuses
    /// the grids of `previous`, an index placement no longer reads.
    /// Placement makes one for every placement, and the grids' cells keep
    /// their allocations while the viewport keeps its size.
    pub(crate) fn reusing(view: &View, map_mode: MapMode, previous: Option<CollisionIndex>) -> Self {
        let viewport_padding = find_viewport_padding(view, map_mode);
        let size = view.size();
        // The grid covers the viewport and its padding on each side.
        let width = size.width as f32;
        let height = size.height as f32;
        let grid_width = width + 2.0 * viewport_padding;
        let grid_height = height + 2.0 * viewport_padding;
        let grid_size = Vec2::new(grid_width, grid_height);
        let reset = |mut grid: Grid<Placed>| {
            grid.reset(grid_size, CELL_SIZE);
            grid
        };
        let (collision_grid, ignored_grid, mut stands) = match previous {
            Some(previous) => (reset(previous.collision_grid), reset(previous.ignored_grid), previous.stands),
            None => (Grid::new(grid_size, CELL_SIZE), Grid::new(grid_size, CELL_SIZE), Vec::new()),
        };
        stands.clear();
        CollisionIndex {
            view: view.clone(),
            size: DVec2::new(f64::from(size.width), f64::from(size.height)),
            viewport_padding,
            collision_grid,
            ignored_grid,
            stands,
            screen_right_boundary: width + viewport_padding,
            screen_bottom_boundary: height + viewport_padding,
            grid_right_boundary: grid_width,
            grid_bottom_boundary: grid_height,
            retained_query_data: HashMap::default(),
        }
    }

    /// The view the index places in.
    #[must_use]
    pub fn view(&self) -> &View {
        &self.view
    }

    /// How far the grid reaches past each edge of the viewport, in pixels:
    /// where a shape at the viewport's top-left corner is.
    #[must_use]
    pub fn viewport_padding(&self) -> f32 {
        self.viewport_padding
    }

    /// Where `x` and `y`, a point in normalized device coordinates (y up),
    /// are in the grid (y down).
    #[inline]
    #[must_use]
    pub fn to_grid(&self, x: f64, y: f64) -> Vec2 {
        let viewport_padding = f64::from(self.viewport_padding);
        Vec2::new(
            (f64::midpoint(x, 1.0) * self.size.x + viewport_padding) as f32,
            (f64::midpoint(-y, 1.0) * self.size.y + viewport_padding) as f32,
        )
    }

    /// Whether `bounds` are wholly outside the viewport, in its padding or
    /// past it.
    #[inline]
    #[must_use]
    pub fn is_offscreen(&self, bounds: &Aabb) -> bool {
        bounds.max.x < self.viewport_padding
            || bounds.min.x >= self.screen_right_boundary
            || bounds.max.y < self.viewport_padding
            || bounds.min.y >= self.screen_bottom_boundary
    }

    /// Whether `bounds` reach into the grid. A shape outside it can't place,
    /// though [`collides`](Self::collides) doesn't say so.
    #[inline]
    #[must_use]
    pub fn is_inside_grid(&self, bounds: &Aabb) -> bool {
        bounds.max.x >= 0.0
            && bounds.min.x < self.grid_right_boundary
            && bounds.max.y >= 0.0
            && bounds.min.y < self.grid_bottom_boundary
    }

    /// Whether `shape`, placed with `overlap`, collides with a shape of
    /// `group` placed before it.
    #[inline]
    #[must_use]
    pub fn collides(&self, shape: &Shape, overlap: Overlap, group: CollisionGroup) -> bool {
        match overlap {
            Overlap::Never => self.collision_grid.any(shape, |placed| group.has(&placed.feature)),
            Overlap::Always => false,
            Overlap::Cooperative => self
                .collision_grid
                .any(shape, |placed| placed.feature.overlap_mode == Overlap::Never && group.has(&placed.feature)),
        }
    }

    /// Inserts `shapes`, which stand for `item`, in `group`.
    #[inline]
    pub fn insert(&mut self, shapes: &[Option<Shape>], item: &Item<'_>, group: CollisionGroup) {
        let stands = item.stands.map(|stand| {
            self.stands.push(stand);
            (self.stands.len() - 1) as u32
        });
        let grid = if item.ignore_placement { &mut self.ignored_grid } else { &mut self.collision_grid };
        for shape in shapes.iter().flatten() {
            let feature = IndexedSubfeature {
                bucket_instance_id: item.record,
                collision_group_id: group.id(),
                overlap_mode: item.overlap,
                ..item.feature.clone()
            };
            grid.insert(Placed { feature, stands }, *shape);
        }
    }

    /// Keeps `data` for queries of the features of the bucket of `record`,
    /// unless the index keeps the bucket's already.
    pub fn retain(&mut self, record: u32, data: impl FnOnce() -> RetainedQueryData) {
        self.retained_query_data.entry(record).or_insert_with(data);
    }

    /// Has queries return the features of the bucket of `record` in
    /// `order`, if the index keeps its query data.
    pub fn set_query_order(&mut self, record: u32, order: &FeatureSortOrder) {
        if let Some(retained) = self.retained_query_data.get_mut(&record) {
            retained.feature_sort_order.clone_from(order);
        }
    }

    /// The query data of a bucket instance; `Err` if there is none.
    pub(crate) fn query_data(&self, record: u32) -> Result<&RetainedQueryData, String> {
        self.retained_query_data.get(&record).ok_or_else(|| format!("no query data of the bucket instance {record}"))
    }

    /// The features whose shapes `query_geometry`, in viewport pixels,
    /// touches, by their record, of marks that `shows` says show where they
    /// stand, if they say ([`Item::stands`]).
    pub(crate) fn query_rendered(
        &self,
        query_geometry: &ScreenLineString,
        shows: impl Fn(&Stand) -> bool,
    ) -> HashMap<u32, Vec<IndexedSubfeature>> {
        let mut result: HashMap<u32, Vec<IndexedSubfeature>> = HashMap::default();
        if query_geometry.is_empty() || (self.collision_grid.is_empty() && self.ignored_grid.is_empty()) {
            return result;
        }

        let grid_query: Vec<Vec2> = query_geometry
            .iter()
            .map(|point| Vec2::new(point.x as f32, point.y as f32) + self.viewport_padding)
            .collect();

        // The bounding box of the query in the grid.
        let mut envelope = Aabb::new(Vec2::INFINITY, Vec2::NEG_INFINITY);
        for &point in &grid_query {
            envelope.min = envelope.min.min(point);
            envelope.max = envelope.max.max(point);
        }
        let envelope = Shape::Box(envelope);

        let features = self.collision_grid.query(&envelope).chain(self.ignored_grid.query(&envelope));
        let mut seen_buckets: HashMap<u32, HashSet<usize>> = HashMap::default();
        for (Placed { feature, stands }, shape) in features {
            // Skip already seen features.
            let seen_features = seen_buckets.entry(feature.bucket_instance_id).or_default();
            if seen_features.contains(&feature.index) {
                continue;
            }

            let Aabb { min, max } = shape.bounds();
            let bounds = [min, Vec2::new(max.x, min.y), max, Vec2::new(min.x, max.y)];
            let hidden = || stands.and_then(|at| self.stands.get(at as usize)).is_some_and(|at| !shows(at));
            if !meets_polygon(&grid_query, &[bounds]) || hidden() {
                continue;
            }

            seen_features.insert(feature.index);
            result.entry(feature.bucket_instance_id).or_default().push(feature.clone());
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use mln_style::size::Size;

    use super::*;

    fn feature() -> IndexedSubfeature {
        IndexedSubfeature {
            index: 0,
            source_layer_name: Arc::from(""),
            bucket_leader_id: Arc::from(""),
            sort_index: 0,
            bucket_instance_id: 0,
            collision_group_id: 0,
            overlap_mode: Overlap::Never,
        }
    }

    fn collision_index() -> CollisionIndex {
        let mut state = crate::camera::TransformState::default();
        state.set_size(Size::new(512, 512));
        CollisionIndex::new(state.view(), MapMode::Static)
    }

    fn insert(index: &mut CollisionIndex, overlap: Overlap, group: CollisionGroup) {
        let projected = [Some(Shape::Box(Aabb::new(Vec2::splat(100.0), Vec2::splat(200.0))))];
        let feature = feature();
        let item = Item { feature: &feature, record: 1, overlap, ignore_placement: false, stands: None };
        index.insert(&projected, &item, group);
    }

    fn collides(index: &CollisionIndex, overlap: Overlap, group: CollisionGroup) -> bool {
        let projected = Shape::Box(Aabb::new(Vec2::splat(150.0), Vec2::splat(250.0)));
        index.collides(&projected, overlap, group)
    }

    const ALL: CollisionGroup = CollisionGroup::ALL;

    #[test]
    fn modes_collide_with_a_never_box() {
        let mut index = collision_index();
        insert(&mut index, Overlap::Never, ALL);
        assert!(collides(&index, Overlap::Never, ALL));
        assert!(!collides(&index, Overlap::Always, ALL));
        assert!(collides(&index, Overlap::Cooperative, ALL));
    }

    #[test]
    fn only_never_collides_with_an_overlapping_box() {
        for placed in [Overlap::Always, Overlap::Cooperative] {
            let mut index = collision_index();
            insert(&mut index, placed, ALL);
            assert!(collides(&index, Overlap::Never, ALL), "never over {placed:?}");
            assert!(!collides(&index, Overlap::Always, ALL), "always over {placed:?}");
            assert!(!collides(&index, Overlap::Cooperative, ALL), "cooperative over {placed:?}");
        }
    }

    #[test]
    fn cooperative_keeps_the_collision_group() {
        let mut index = collision_index();
        insert(&mut index, Overlap::Never, CollisionGroup(Some(2)));
        assert!(collides(&index, Overlap::Cooperative, CollisionGroup(Some(2))));
        assert!(!collides(&index, Overlap::Cooperative, CollisionGroup(Some(3))));
        assert!(collides(&index, Overlap::Never, CollisionGroup(Some(2))));
        assert!(!collides(&index, Overlap::Never, CollisionGroup(Some(3))));
    }

    #[test]
    fn sources_take_groups_unless_they_collide_across() {
        let mut groups = CollisionGroups::new(false);
        let (a, b) = (groups.get("a"), groups.get("b"));
        assert_ne!(a, b);
        assert_eq!(groups.get("a"), a);
        assert_eq!(CollisionGroups::new(true).get("a"), ALL);
    }
}
