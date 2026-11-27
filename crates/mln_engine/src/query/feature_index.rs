//! The index of a tile's features by their bounding boxes, which
//! `queryRenderedFeatures` searches. A tile worker fills a
//! [`FeatureIndexBuilder`] as it lays the tile out and finishes it into a
//! [`FeatureIndex`], which is only read after. Most tiles are never queried,
//! so the index builds its R-tree at its first query.

use std::borrow::Cow;
use std::cmp::Reverse;
use std::sync::{Arc, OnceLock};

use geo_index::rtree::sort::HilbertSort;
use geo_index::rtree::{RTree, RTreeBuilder, RTreeIndex};
use mln_style::collections::VecMap;
use mln_style::constants;
use mln_style::error::Error;
use mln_style::expression::{EvaluationContext, GlobalStateMap};
use mln_style::feature::{
    FeatureState, GeometryCollection, GeometryCoordinate, GeometryCoordinates, GeometryTileFeature,
};
use mln_style::hash::{HashMap, HashSet};
use mln_style::types::TranslateAnchorType;

use crate::camera::{TileProjector, VerticalPerspectiveProjection, View};
use crate::geo::ScreenCoordinate;
use crate::layer::render_layer::RenderLayer;
use crate::placement::Overlap;
use crate::query::RenderedQueryOptions;
use crate::source::source_state::SourceFeatureState;
use crate::tile::feature::{Feature, feature_id_to_string};
use crate::tile::{OverscaledTileID, TileData};

/// The render layers a query looks at, by id.
pub(crate) type QueryLayers<'a> = HashMap<String, &'a dyn RenderLayer>;

/// What a query reads of a tile's features: those of its layers that its
/// filter accepts, with their state.
#[derive(Clone, Copy)]
pub(crate) struct FeatureSelection<'a> {
    pub(crate) layers: &'a QueryLayers<'a>,
    pub(crate) options: &'a RenderedQueryOptions,
    pub(crate) global_state: Option<&'a GlobalStateMap>,
    pub(crate) tile_id: &'a OverscaledTileID,
    pub(crate) feature_state: Option<&'a SourceFeatureState>,
}

/// The ground a frame drew of the terrain, which a query finds the features
/// of some layers on.
#[derive(Clone, Copy)]
pub(crate) struct QueryGround<'a> {
    pub(crate) elevation: &'a crate::render::terrain::TerrainElevation,
    /// Whether the layers' points stand on the ground, rather than their
    /// features lying on it. A point that stands shows raised, where the
    /// query's lines of sight can meet the ground far beyond it, or not at
    /// all, so every feature of a rendered tile is a candidate, and the
    /// layer's hit test decides.
    pub(crate) standing: bool,
}

/// Where a query looks in a tile: its geometry in tile units, seen through
/// the camera.
#[derive(Clone, Copy)]
pub struct QueryRegion<'a> {
    pub geometry: &'a GeometryCoordinates,
    /// The screen points of `geometry`, from the view's top left: where the
    /// query's lines of sight start.
    pub screen: &'a [ScreenCoordinate],
    /// Over terrain, the ground the frame drew under the tile, where the
    /// lines of sight meet it at `geometry`, for the layers that stand on it.
    pub ground: Option<crate::render::terrain::TileGround<'a>>,
    /// Whether every feature of the tile is a candidate, rather than those
    /// around `geometry` ([`QueryGround::standing`]).
    pub(crate) whole_tile: bool,
    pub view: &'a View,
    pub projector: &'a TileProjector<'a>,
    pub pixels_to_tile_units: f32,
    /// How far around the geometry the layers' features may reach, in
    /// pixels.
    pub(crate) padding: f32,
}

/// The screen points, from the top left of `view`, that a query of
/// `geometry` tests. A geometry's edges are followed in steps where its
/// corners alone would cut off part of what it covers: past the globe's
/// limb, where a point stands for the nearest horizon point, and over
/// terrain (`over_terrain`), where the ground an edge passes over doesn't
/// lie between the ground under its ends.
pub(crate) fn query_screen_geometry<'a>(
    view: &View,
    geometry: &'a [ScreenCoordinate],
    over_terrain: bool,
) -> Cow<'a, [ScreenCoordinate]> {
    const STEPS: i32 = 32;
    let height = f64::from(view.size().height);
    let on_planet = |p: &ScreenCoordinate| {
        VerticalPerspectiveProjection::screen_coordinate_hits_globe(
            view,
            &ScreenCoordinate::new(p.x, height - p.y),
            0.0,
        )
    };
    let past_limb = || view.is_globe() && !geometry.iter().all(on_planet);
    if geometry.len() < 2 || !(over_terrain || past_limb()) {
        return Cow::Borrowed(geometry);
    }
    let mut traced = Vec::with_capacity((geometry.len() - 1) * STEPS as usize + 1);
    for edge in geometry.windows(2) {
        let (from, to) = (edge[0], edge[1]);
        for k in 0..STEPS {
            let t = f64::from(k) / f64::from(STEPS);
            traced.push(ScreenCoordinate::new(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t));
        }
    }
    traced.extend(geometry.last().copied());
    Cow::Owned(traced)
}

/// The features a query found, by layer id.
pub(crate) type QueryResults = HashMap<String, Vec<Feature>>;

/// A feature of a feature index or a collision index, which a query looks
/// up in its tile's data.
#[derive(Clone, Debug, PartialEq, mln_wire::Wire)]
pub struct IndexedSubfeature {
    /// The feature's index in its source layer.
    pub index: usize,
    /// The feature's source layer.
    pub source_layer_name: Arc<str>,
    /// The id of the layer that leads the bucket the feature is in.
    pub bucket_leader_id: Arc<str>,
    /// Where the feature is among those the bucket indexed, which results
    /// sort by.
    pub sort_index: usize,
    /// The record of the bucket of a feature in a collision index
    /// ([`Item::record`](crate::placement::Item::record)); 0 elsewhere.
    pub bucket_instance_id: u32,
    /// The id of the collision group a collision index placed the feature
    /// in; 0 elsewhere.
    pub collision_group_id: u16,
    /// How a collision index placed the feature's shapes. Only a collision
    /// index sets it, so the worker codec doesn't carry it.
    #[wire(skip)]
    pub overlap_mode: Overlap,
}

/// A shared order of feature indices that queries sort a bucket's features
/// by, for a bucket whose features sort by their position; `None` for the
/// order they were indexed in.
pub type FeatureSortOrder = Option<Arc<Vec<usize>>>;

/// A box of a feature index: one ring of a feature.
#[derive(Clone, Debug, PartialEq)]
struct Entry {
    /// The feature's index in its source layer.
    index: usize,
    /// The order the feature was inserted in; a query returns later features
    /// first.
    sort_index: usize,
    source_layer_name: Arc<str>,
    bucket_leader_id: Arc<str>,
}

/// A box's min x, min y, max x and max y, in tile units.
type Bounds = [i16; 4];

/// A feature index as a tile's layout fills it.
pub(crate) struct FeatureIndexBuilder {
    /// The rings' entries and boxes, in insertion order.
    entries: Vec<Entry>,
    boxes: Vec<Bounds>,
    sort_index: usize,
    source_layer_names: HashSet<Arc<str>>,
    bucket_layer_ids: HashMap<Arc<str>, Vec<String>>,
    tile_data: Option<TileData>,
}

impl FeatureIndexBuilder {
    /// An empty index of `tile_data`'s features, sharing the tile data.
    #[must_use]
    pub(crate) fn new(tile_data: Option<TileData>) -> Self {
        FeatureIndexBuilder {
            entries: Vec::new(),
            boxes: Vec::new(),
            sort_index: 0,
            source_layer_names: HashSet::default(),
            bucket_layer_ids: HashMap::default(),
            tile_data,
        }
    }

    fn intern_layer_name(&mut self, name: &str) -> Arc<str> {
        if let Some(existing) = self.source_layer_names.get(name) {
            return Arc::clone(existing);
        }
        let name: Arc<str> = Arc::from(name);
        self.source_layer_names.insert(Arc::clone(&name));
        name
    }

    fn intern_leader_id(&mut self, id: &str) -> Arc<str> {
        if let Some((existing, _)) = self.bucket_layer_ids.get_key_value(id) {
            return Arc::clone(existing);
        }
        let id: Arc<str> = Arc::from(id);
        self.bucket_layer_ids.insert(Arc::clone(&id), Vec::new());
        id
    }

    /// Records the layers of the bucket whose leader is `bucket_leader_id`,
    /// which a query of the bucket's features looks at.
    pub(crate) fn set_bucket_layer_ids(&mut self, bucket_leader_id: &str, layer_ids: &[String]) {
        let key = self.intern_leader_id(bucket_leader_id);
        self.bucket_layer_ids.insert(key, layer_ids.to_vec());
    }

    /// Adds the feature at `index` of `source_layer_name`, with the box of
    /// each of its rings that reaches into the tile.
    pub(crate) fn insert(
        &mut self,
        geometries: &GeometryCollection,
        index: usize,
        source_layer_name: &str,
        bucket_leader_id: &str,
    ) {
        let source_layer_name = self.intern_layer_name(source_layer_name);
        let bucket_leader_id = self.intern_leader_id(bucket_leader_id);
        let sort_index = self.sort_index;
        self.sort_index += 1;
        for ring in geometries.iter() {
            let Some((min, max)) = envelope(ring.iter().copied()) else {
                continue;
            };
            let extent = constants::EXTENT;
            if i32::from(min.x) < extent && i32::from(min.y) < extent && max.x >= 0 && max.y >= 0 {
                self.entries.push(Entry {
                    index,
                    sort_index,
                    source_layer_name: Arc::clone(&source_layer_name),
                    bucket_leader_id: Arc::clone(&bucket_leader_id),
                });
                self.boxes.push([min.x, min.y, max.x, max.y]);
            }
        }
    }

    /// The index of the features inserted.
    #[must_use]
    pub(crate) fn finish(self) -> FeatureIndex {
        FeatureIndex {
            tree: OnceLock::new(),
            entries: self.entries,
            boxes: self.boxes,
            bucket_layer_ids: self.bucket_layer_ids,
            tile_data: self.tile_data,
        }
    }
}

/// An R-tree of `boxes`, whose items are their indices; `None` if there are
/// none. It holds the coordinates as `i32`, as sorting sums them.
fn build_tree(boxes: &[Bounds]) -> Option<RTree<i32>> {
    if boxes.is_empty() {
        return None;
    }
    let mut builder = RTreeBuilder::new(boxes.len() as u32);
    for &bounds in boxes {
        let [min_x, min_y, max_x, max_y] = bounds.map(i32::from);
        builder.add(min_x, min_y, max_x, max_y);
    }
    Some(builder.finish::<HilbertSort>())
}

/// A tile's features by their bounding boxes, and the tile data they are in.
pub struct FeatureIndex {
    /// Indexes `boxes`, once a query has built it; `None` when there are
    /// none.
    tree: OnceLock<Option<RTree<i32>>>,
    /// The rings' entries and boxes, in insertion order.
    entries: Vec<Entry>,
    boxes: Vec<Bounds>,
    bucket_layer_ids: HashMap<Arc<str>, Vec<String>>,
    tile_data: Option<TileData>,
}

impl std::fmt::Debug for FeatureIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeatureIndex")
            .field("entries", &self.entries.len())
            .field("bucket_layer_ids", &self.bucket_layer_ids)
            .finish_non_exhaustive()
    }
}

/// The bounding box (min, max) of int16 points; `None` if there are none.
fn envelope(points: impl IntoIterator<Item = GeometryCoordinate>) -> Option<(GeometryCoordinate, GeometryCoordinate)> {
    let mut iter = points.into_iter();
    let first = iter.next()?;
    let (mut min, mut max) = (first, first);
    for p in iter {
        min.x = min.x.min(p.x);
        min.y = min.y.min(p.y);
        max.x = max.x.max(p.x);
        max.y = max.y.max(p.y);
    }
    Some((min, max))
}

impl FeatureIndex {
    #[must_use]
    pub(crate) fn data(&self) -> Option<&TileData> {
        self.tile_data.as_ref()
    }

    /// Sets the tile data the index's features are in.
    pub(crate) fn set_data(&mut self, tile_data: Option<TileData>) {
        self.tile_data = tile_data;
    }

    /// Adds the features of `selection` that `region` touches to `result`,
    /// later features first.
    pub(crate) fn query(
        &self,
        result: &mut QueryResults,
        selection: &FeatureSelection<'_>,
        region: &QueryRegion<'_>,
    ) -> Result<(), Error> {
        if self.tile_data.is_none() {
            return Ok(());
        }
        let Some(tree) = self.tree.get_or_init(|| build_tree(&self.boxes)) else {
            return Ok(());
        };

        let additional_padding = (region.padding * region.pixels_to_tile_units).min(constants::EXTENT as f32) as i16;
        let whole_tile = || {
            let extent = constants::EXTENT as i16;
            (GeometryCoordinate::new(0, 0), GeometryCoordinate::new(extent, extent))
        };
        let candidates = if region.whole_tile { Some(whole_tile()) } else { envelope(region.geometry.iter().copied()) };
        let Some((min, max)) = candidates else {
            return Ok(());
        };
        // The padded box's corners are tile coordinates, which wrap in i16.
        let (min_x, min_y) = (min.x.wrapping_sub(additional_padding), min.y.wrapping_sub(additional_padding));
        let (max_x, max_y) = (max.x.wrapping_add(additional_padding), max.y.wrapping_add(additional_padding));
        // Only a box that reaches into the tile finds features, and one that
        // wrapped around is empty.
        let extent = constants::EXTENT;
        if min_x > max_x
            || min_y > max_y
            || max_x < 0
            || max_y < 0
            || i32::from(min_x) >= extent
            || i32::from(min_y) >= extent
        {
            return Ok(());
        }
        let mut features: Vec<&Entry> = tree
            .search(i32::from(min_x), i32::from(min_y), i32::from(max_x), i32::from(max_y))
            .into_iter()
            .filter_map(|i| self.entries.get(i as usize))
            .collect();
        // Later features first, each once.
        features.sort_unstable_by_key(|feature| Reverse(feature.sort_index));
        features.dedup_by_key(|feature| feature.sort_index);
        for feature in features {
            let entry = (feature.index, &*feature.source_layer_name, &*feature.bucket_leader_id);
            self.add_feature(result, entry, selection, Some(region))?;
        }
        Ok(())
    }

    /// The features of `symbol_features`, which the collision index found,
    /// that `selection` selects, in the order their bucket last drew them.
    pub(crate) fn lookup_symbol_features(
        &self,
        symbol_features: &[IndexedSubfeature],
        selection: &FeatureSelection<'_>,
        feature_sort_order: &FeatureSortOrder,
    ) -> Result<QueryResults, Error> {
        let mut result = QueryResults::default();
        if self.tile_data.is_none() {
            return Ok(result);
        }
        let mut sorted_features: Vec<&IndexedSubfeature> = symbol_features.iter().collect();

        sorted_features.sort_by(|a, b| {
            // Same idea as the non-symbol sort order, but symbol features may
            // have changed their sort order since their indexed subfeature
            // was added to the collision index. The feature sort order is
            // relatively inefficient for querying but cheap to build on every
            // bucket sort.
            if let Some(order) = feature_sort_order {
                // A query of rendered symbols returns features in
                // top-to-bottom rendering order, last to first.
                // Actually there can be multiple symbol instances per feature, so
                // we sort each feature based on the first matching symbol instance.
                let sorted_a = order.iter().position(|&i| i == a.index).unwrap_or(order.len());
                let sorted_b = order.iter().position(|&i| i == b.index).unwrap_or(order.len());
                debug_assert_ne!(sorted_a, order.len());
                debug_assert_ne!(sorted_b, order.len());
                sorted_b.cmp(&sorted_a)
            } else {
                // Bucket hasn't been re-sorted based on angle, so use same
                // "reverse of appearance in source data" logic as non-symboles
                b.sort_index.cmp(&a.sort_index)
            }
        });

        for feature in sorted_features {
            let entry = (feature.index, &*feature.source_layer_name, &*feature.bucket_leader_id);
            self.add_feature(&mut result, entry, selection, None)?;
        }
        Ok(result)
    }

    /// Adds the feature `index` of the source layer `source_layer_name`,
    /// of the bucket of `bucket_leader_id`, to `result` under each layer of
    /// `selection` it is in, where it touches `region`. A symbol layer's
    /// features need no region: the collision index found them.
    fn add_feature(
        &self,
        result: &mut QueryResults,
        (index, source_layer_name, bucket_leader_id): (usize, &str, &str),
        selection: &FeatureSelection<'_>,
        region: Option<&QueryRegion<'_>>,
    ) -> Result<(), Error> {
        let FeatureSelection { layers, options, global_state, tile_id, feature_state } = *selection;
        let Some(tile_data) = &self.tile_data else {
            return Ok(());
        };
        // Every leader the index holds has its bucket's layers.
        let Some(layer_ids) = self.bucket_layer_ids.get(bucket_leader_id) else {
            debug_assert!(false, "unknown bucket leader {bucket_leader_id}");
            return Ok(());
        };

        // The source layer, the feature and its state are the same for every
        // layer of the bucket: they are read once, where one of its layers
        // is queried.
        if !layer_ids.iter().any(|layer_id| layers.contains_key(layer_id)) {
            return Ok(());
        }
        let Some(source_layer) = tile_data.layer(source_layer_name)? else {
            debug_assert!(false, "source layer {source_layer_name} missing");
            return Ok(());
        };
        let feature = source_layer.feature(index)?;
        let geometry = feature.geometry()?;
        let source_layer_id = source_layer.name().to_owned();
        let mut state = FeatureState::default();
        if let Some(source_feature_state) = feature_state
            && let Some(id_str) = feature_id_to_string(&feature.get_id())
        {
            state = source_feature_state.state(Some(&source_layer_id), &id_str);
        }

        for layer_id in layer_ids {
            let Some(render_layer) = layers.get(layer_id) else {
                continue;
            };

            let needs_cross_tile_index = render_layer.base().evaluated_properties.entry().places();
            // we use the float camera zoom here, not the integer tile ID zoom,
            // because for the query we take into account the actual rendered
            // size of the geometry (#2781)
            let intersects = needs_cross_tile_index
                || region.is_some_and(|region| render_layer.hit(region, &feature, &geometry, &state));
            if !intersects {
                continue;
            }

            // Match layer filters and source queries, including overzoomed tiles.
            if let Some(filter) = &options.filter {
                let context = EvaluationContext::with_zoom_feature(f32::from(tile_id.overscaled_z), Some(&feature))
                    .with_global_state(global_state);
                if !filter.evaluate(&context) {
                    continue;
                }
            }

            let converted = crate::tile::convert_feature(&feature, &geometry, &tile_id.canonical)?;
            let mut converted = Feature::from(converted);
            converted.source = render_layer.base().style.base().source.clone();
            converted.source_layer = source_layer_id.clone();
            converted.state = state.clone();
            result.entry(layer_id.clone()).or_default().push(converted);
        }
        Ok(())
    }

    /// The bucket leaders and their layer ids (sorted, for inspection).
    #[must_use]
    pub(crate) fn bucket_layer_ids(&self) -> VecMap<String, Vec<String>> {
        self.bucket_layer_ids.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }
}

/// `query_geometry`, a query's geometry in tile units, moved back by a
/// layer's `translate` (in pixels, at `pixels_to_tile_units`), turned by
/// `-bearing` for a translation anchored to the viewport: what a layer that
/// translates its features tests them against. `None` without a
/// translation.
#[must_use]
pub fn translate_query_geometry(
    query_geometry: &GeometryCoordinates,
    translate: [f32; 2],
    anchor_type: TranslateAnchorType,
    bearing: f32,
    pixels_to_tile_units: f32,
) -> Option<GeometryCoordinates> {
    if translate[0] == 0.0 && translate[1] == 0.0 {
        return None;
    }

    let mut translate_vec = GeometryCoordinate::new(
        (translate[0] * pixels_to_tile_units) as i16,
        (translate[1] * pixels_to_tile_units) as i16,
    );
    if anchor_type == TranslateAnchorType::Viewport {
        translate_vec = crate::geo::math::rotate_i16(translate_vec, -bearing);
    }

    Some(
        query_geometry
            .iter()
            .map(|p| GeometryCoordinate::new(p.x.wrapping_sub(translate_vec.x), p.y.wrapping_sub(translate_vec.y)))
            .collect(),
    )
}

/// A `FeatureIndex` as it crosses, for a tile worker in another wasm instance
/// (`tile/worker_codec`): its entries and boxes, each string once, and not
/// its tile data, which the side that gets the index has (it sent it).
#[derive(mln_wire::Wire)]
struct WireIndex {
    strings: Vec<String>,
    bucket_layer_ids: VecMap<String, Vec<String>>,
    elements: Vec<WireEntry>,
}

/// An [`Entry`], with indices into `strings`, and its box.
#[derive(mln_wire::Wire)]
struct WireEntry {
    index: usize,
    sort_index: usize,
    source_layer_name: u32,
    bucket_leader_id: u32,
    bounds: Bounds,
}

impl FeatureIndex {
    fn to_wire(&self) -> WireIndex {
        let mut strings: Vec<String> = Vec::new();
        let mut string_ids: HashMap<*const u8, u32> = HashMap::default();
        let mut intern = |s: &Arc<str>| -> u32 {
            *string_ids.entry(s.as_ptr()).or_insert_with(|| {
                strings.push(s.to_string());
                (strings.len() - 1) as u32
            })
        };
        let elements = self
            .entries
            .iter()
            .zip(&self.boxes)
            .map(|(entry, bounds)| WireEntry {
                index: entry.index,
                sort_index: entry.sort_index,
                source_layer_name: intern(&entry.source_layer_name),
                bucket_leader_id: intern(&entry.bucket_leader_id),
                bounds: *bounds,
            })
            .collect();
        WireIndex { strings, bucket_layer_ids: self.bucket_layer_ids(), elements }
    }

    /// The bucket leader of each box, in the order they were inserted.
    #[cfg(test)]
    pub(crate) fn leaders(&self) -> Vec<&str> {
        self.entries.iter().map(|entry| &*entry.bucket_leader_id).collect()
    }

    /// The index as it crosses, without its tile data.
    #[cfg(test)]
    pub(crate) fn encode_grid(&self) -> Vec<u8> {
        mln_wire::to_vec(&self.to_wire()).unwrap()
    }
}

/// An index crosses without its tile data, which the side that reads it
/// sets (`set_data`).
impl mln_wire::Wire for FeatureIndex {
    fn write(&self, w: &mut mln_wire::Writer) {
        self.to_wire().write(w);
    }

    fn read(r: &mut mln_wire::Reader<'_>) -> Option<Self> {
        let parts = WireIndex::read(r)?;
        let mut builder = FeatureIndexBuilder::new(None);
        let strings: Vec<Arc<str>> = parts.strings.iter().map(|string| builder.intern_layer_name(string)).collect();
        for (leader, layer_ids) in &parts.bucket_layer_ids {
            builder.set_bucket_layer_ids(leader, layer_ids);
        }
        for element in parts.elements {
            let (Some(source_layer_name), Some(bucket_leader_id)) =
                (strings.get(element.source_layer_name as usize), strings.get(element.bucket_leader_id as usize))
            else {
                return r.fail("feature index string out of range");
            };
            // The leaders share the strings of `bucket_layer_ids`, as `insert`
            // interns them.
            let bucket_leader_id = builder.intern_leader_id(bucket_leader_id);
            builder.entries.push(Entry {
                index: element.index,
                sort_index: element.sort_index,
                source_layer_name: Arc::clone(source_layer_name),
                bucket_leader_id,
            });
            builder.boxes.push(element.bounds);
        }
        Some(builder.finish())
    }
}

#[cfg(test)]
mod tests {
    use geometry::Point;

    use super::*;

    #[test]
    fn translate_query_geometry() {
        let query: GeometryCoordinates = vec![Point::new(10, 10), Point::new(20, 20)];
        assert_eq!(super::translate_query_geometry(&query, [0.0, 0.0], TranslateAnchorType::Map, 0.0, 8.0), None);
        assert_eq!(
            super::translate_query_geometry(&query, [1.0, -2.0], TranslateAnchorType::Map, 1.0, 8.0),
            Some(vec![Point::new(2, 26), Point::new(12, 36)])
        );
        // Viewport anchor: the translation is rotated by -bearing.
        assert_eq!(
            super::translate_query_geometry(
                &query,
                [1.0, 0.0],
                TranslateAnchorType::Viewport,
                std::f32::consts::FRAC_PI_2,
                8.0
            ),
            Some(vec![Point::new(10, 18), Point::new(20, 28)])
        );
    }

    #[test]
    fn insert_records_leaders_and_skips_rings_outside_the_tile() {
        let mut builder = FeatureIndexBuilder::new(None);
        builder.set_bucket_layer_ids("a", &["a".to_owned(), "b".to_owned()]);
        let inside = GeometryCollection::from(vec![vec![Point::new(10, 10)]]);
        let outside = GeometryCollection::from(vec![vec![Point::new(9000, 10)]]);
        builder.insert(&inside, 0, "layer", "a");
        builder.insert(&outside, 1, "layer", "a");
        let index = builder.finish();
        assert_eq!(index.bucket_layer_ids()["a"], vec!["a".to_owned(), "b".to_owned()]);
        assert_eq!(index.leaders(), ["a"]);
    }

    #[test]
    fn crosses_with_its_boxes() {
        let mut builder = FeatureIndexBuilder::new(None);
        builder.set_bucket_layer_ids("leader", &["leader".to_owned()]);
        for i in 0..100 {
            let x = (i * 80) as i16;
            let ring = vec![Point::new(x, 10), Point::new(x + 5, 4000)];
            builder.insert(&GeometryCollection::from(vec![ring.clone(), ring]), i, "layer", "leader");
        }
        let index = builder.finish();
        let encoded = index.encode_grid();
        let decoded: FeatureIndex = mln_wire::from_slice(&mln_wire::to_vec(&index).unwrap()).unwrap();
        assert_eq!(decoded.entries, index.entries);
        assert_eq!(decoded.boxes, index.boxes);
        assert_eq!(decoded.encode_grid(), encoded);
        assert_eq!(index.boxes[14], [560, 10, 565, 4000]);
    }

    #[test]
    fn indexes_boxes_whose_extent_or_center_overflows_i16() {
        // Wider than i16 holds, and with an x sum past it.
        for (min_x, max_x) in [(-2000, 31000), (2000, 31000)] {
            let boxes: Vec<Bounds> = (0..20).map(|i| [min_x, i * 10, max_x, i * 10 + 5]).collect();
            let tree = build_tree(&boxes).expect("boxes");
            assert_eq!(tree.search(3000, 0, 3010, 10).len(), 2);
        }
    }
}
