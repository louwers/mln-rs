// Ports src/mln/geometry/feature_index.hpp and
// src/mln/geometry/feature_index.cpp.
//
// `RefIndexedSubfeature` refers to the index's own copies of the source
// layer and bucket leader names; here those are shared `Arc<str>`s interned
// in the index, so the grid entries stay cheap to copy and the index can be
// built on a worker thread and shared (`Arc<FeatureIndex>`).
//
// The `layers` argument (C++ `std::unordered_map<std::string, const
// RenderLayer*>`) is `QueryLayers`. `DynamicFeatureIndex` (filled only by
// the location indicator layer) is TODO(port).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use geometry::Point;
use mln_map_types::{ProjectionData, TileProjector, TransformState};
use mln_style::expression::expression::FeatureState;
use mln_style::expression::{EvaluationContext, GlobalStateMap};
use mln_style::layer::layer_type_info;
use mln_style_types::TranslateAnchorType;
use mln_tile_id::OverscaledTileID;
use mln_tile_id::geometry_tile_data::{GeometryCollection, GeometryCoordinate, GeometryCoordinates, GeometryTileData};
use mln_util::constants;
use mln_util::error::Error;
use mln_util::feature::{Feature, feature_id_to_string};
use mln_util::grid_index::{BBox, GridIndex};
use mln_vec_map::VecMap;
use serde::{Deserialize, Serialize};

use crate::query::RenderedQueryOptions;
use crate::render_layer::RenderLayer;
use crate::source_state::SourceFeatureState;

/// The render layers a query looks at, by id.
pub type QueryLayers<'a> = HashMap<String, &'a dyn RenderLayer>;

/// The features a query found, by layer id.
pub type QueryResults = HashMap<String, Vec<Feature>>;

/// An indexed element within a feature index, referring to the index's copy
/// of its strings.
#[derive(Clone, Debug, PartialEq)]
pub struct RefIndexedSubfeature {
    pub index: usize,
    pub sort_index: usize,
    pub source_layer_name: Arc<str>,
    pub bucket_leader_id: Arc<str>,
    // Only used for symbol features
    pub bucket_instance_id: u32,
    pub collision_group_id: u16,
}

/// This version of an indexed feature has copies of the necessary strings,
/// so that it can be used outside the index.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IndexedSubfeature {
    pub index: usize,
    pub source_layer_name: String,
    pub bucket_leader_id: String,
    pub sort_index: usize,
    // Only used for symbol features
    pub bucket_instance_id: u32,
    pub collision_group_id: u16,
}

impl IndexedSubfeature {
    /// `IndexedSubfeature(const RefIndexedSubfeature&)`.
    #[must_use]
    pub fn from_ref(other: &RefIndexedSubfeature) -> Self {
        IndexedSubfeature {
            index: other.index,
            source_layer_name: other.source_layer_name.to_string(),
            bucket_leader_id: other.bucket_leader_id.to_string(),
            sort_index: other.sort_index,
            bucket_instance_id: other.bucket_instance_id,
            collision_group_id: other.collision_group_id,
        }
    }

    /// `IndexedSubfeature(const RefIndexedSubfeature&, bucketInstanceId, collisionGroupId)`.
    #[must_use]
    pub fn from_ref_with(other: &RefIndexedSubfeature, bucket_instance_id: u32, collision_group_id: u16) -> Self {
        IndexedSubfeature { bucket_instance_id, collision_group_id, ..IndexedSubfeature::from_ref(other) }
    }
}

/// `FeatureSortOrder`: `std::shared_ptr<const std::vector<size_t>>`.
pub type FeatureSortOrder = Option<Arc<Vec<usize>>>;

const EXPECTED_UNIQUE_LAYER_IDS: usize = 10;
const EXPECTED_UNIQUE_LEADER_IDS: usize = 30;

/// `FeatureIndex`.
pub struct FeatureIndex {
    grid: GridIndex<RefIndexedSubfeature>,
    sort_index: usize,

    bucket_layer_ids: HashMap<Arc<str>, Vec<String>>,
    unique_layer_ids: HashSet<Arc<str>>,
    tile_data: Option<Arc<dyn GeometryTileData + Send + Sync>>,
}

impl std::fmt::Debug for FeatureIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeatureIndex")
            .field("sort_index", &self.sort_index)
            .field("bucket_layer_ids", &self.bucket_layer_ids)
            .finish_non_exhaustive()
    }
}

/// `mapbox::geometry::envelope` of int16 points (`box<int16_t>`).
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
    /// `FeatureIndex(std::unique_ptr<const GeometryTileData>)`. C++ gets a
    /// `clone()` of the tile data, which shares the data; this shares the
    /// `Arc`.
    #[must_use]
    pub fn new(tile_data: Option<Arc<dyn GeometryTileData + Send + Sync>>) -> Self {
        FeatureIndex {
            // 16x16 grid -> 32px cell
            grid: GridIndex::new(constants::EXTENT as f32, constants::EXTENT as f32, (constants::EXTENT / 16) as u32),
            sort_index: 0,
            bucket_layer_ids: HashMap::new(),
            unique_layer_ids: HashSet::new(),
            tile_data,
        }
    }

    /// `getData()`.
    #[must_use]
    pub fn get_data(&self) -> Option<&(dyn GeometryTileData + Send + Sync)> {
        self.tile_data.as_deref()
    }

    /// Set the expected number of elements per cell to avoid small
    /// re-allocations for populated cells.
    pub fn reserve(&mut self, value: usize) {
        self.grid.reserve(value);
    }

    fn intern_layer_name(&mut self, name: &str) -> Arc<str> {
        if self.unique_layer_ids.is_empty() {
            self.unique_layer_ids.reserve(EXPECTED_UNIQUE_LAYER_IDS);
        }
        if let Some(existing) = self.unique_layer_ids.get(name) {
            return existing.clone();
        }
        let name: Arc<str> = Arc::from(name);
        self.unique_layer_ids.insert(name.clone());
        name
    }

    fn intern_leader_id(&mut self, id: &str) -> Arc<str> {
        if self.bucket_layer_ids.is_empty() {
            self.bucket_layer_ids.reserve(EXPECTED_UNIQUE_LEADER_IDS);
        }
        if let Some((existing, _)) = self.bucket_layer_ids.get_key_value(id) {
            return existing.clone();
        }
        let id: Arc<str> = Arc::from(id);
        self.bucket_layer_ids.insert(id.clone(), Vec::new());
        id
    }

    /// `insert(geometries, index, sourceLayerName, bucketLeaderID)`.
    pub fn insert(
        &mut self,
        geometries: &GeometryCollection,
        index: usize,
        source_layer_name: &str,
        bucket_leader_id: &str,
    ) {
        let emplaced_layer_name = self.intern_layer_name(source_layer_name);
        let emplaced_leader_id = self.intern_leader_id(bucket_leader_id);

        let feature_sort_index = self.sort_index;
        self.sort_index += 1;
        for ring in geometries.iter() {
            let Some((min, max)) = envelope(ring.iter().copied()) else {
                // C++ envelope of an empty ring is {max, lowest}: never
                // inside the tile.
                continue;
            };
            let extent = constants::EXTENT;
            if i32::from(min.x) < extent && i32::from(min.y) < extent && max.x >= 0 && max.y >= 0 {
                self.grid.insert(
                    RefIndexedSubfeature {
                        index,
                        sort_index: feature_sort_index,
                        source_layer_name: emplaced_layer_name.clone(),
                        bucket_leader_id: emplaced_leader_id.clone(),
                        bucket_instance_id: 0,
                        collision_group_id: 0,
                    },
                    &BBox::new(
                        Point::new(f32::from(min.x), f32::from(min.y)),
                        Point::new(f32::from(max.x), f32::from(max.y)),
                    ),
                );
            }
        }
    }

    /// `query(result, queryGeometry, transformState, projector, tileSize,
    /// scale, options, globalState, tileID, layers, additionalQueryPadding,
    /// sourceFeatureState)`.
    #[allow(clippy::too_many_arguments)]
    pub fn query(
        &self,
        result: &mut QueryResults,
        query_geometry: &GeometryCoordinates,
        transform_state: &TransformState,
        projector: &TileProjector<'_>,
        tile_size: f64,
        scale: f64,
        query_options: &RenderedQueryOptions,
        global_state: Option<&GlobalStateMap>,
        tile_id: &OverscaledTileID,
        layers: &QueryLayers<'_>,
        additional_query_padding: f32,
        source_feature_state: &SourceFeatureState,
    ) -> Result<(), Error> {
        if self.tile_data.is_none() {
            return Ok(());
        }

        // Determine query radius
        let pixels_to_tile_units = (f64::from(constants::EXTENT) / tile_size / scale) as f32;
        let additional_padding = {
            let p = additional_query_padding * pixels_to_tile_units;
            let extent = constants::EXTENT as f32;
            (if p < extent { p } else { extent }) as i16
        };

        // Query the grid index
        let Some((min, max)) = envelope(query_geometry.iter().copied()) else {
            return Ok(());
        };
        // `box.min - additionalPadding` is Point<int16_t> arithmetic.
        let query_box = BBox::new(
            Point::new(
                f32::from(min.x.wrapping_sub(additional_padding)),
                f32::from(min.y.wrapping_sub(additional_padding)),
            ),
            Point::new(
                f32::from(max.x.wrapping_add(additional_padding)),
                f32::from(max.y.wrapping_add(additional_padding)),
            ),
        );
        let mut features = self.grid.query(&query_box);

        features.sort_by_key(|feature| std::cmp::Reverse(feature.sort_index));
        let mut previous_sort_index = usize::MAX;
        for indexed_feature in &features {
            // If this feature is the same as the previous feature, skip it.
            if indexed_feature.sort_index == previous_sort_index {
                continue;
            }
            previous_sort_index = indexed_feature.sort_index;

            self.add_feature(
                result,
                indexed_feature.index,
                &indexed_feature.source_layer_name,
                &indexed_feature.bucket_leader_id,
                query_options,
                global_state,
                tile_id,
                layers,
                query_geometry,
                transform_state,
                pixels_to_tile_units,
                projector,
                Some(source_feature_state),
            )?;
        }
        Ok(())
    }

    /// `lookupSymbolFeatures(symbolFeatures, options, globalState, layers,
    /// tileID, featureSortOrder)`.
    pub fn lookup_symbol_features(
        &self,
        symbol_features: &[IndexedSubfeature],
        query_options: &RenderedQueryOptions,
        global_state: Option<&GlobalStateMap>,
        layers: &QueryLayers<'_>,
        tile_id: &OverscaledTileID,
        feature_sort_order: &FeatureSortOrder,
    ) -> Result<QueryResults, Error> {
        let mut result = QueryResults::new();
        if self.tile_data.is_none() {
            return Ok(result);
        }
        let mut sorted_features: Vec<&IndexedSubfeature> = symbol_features.iter().collect();

        sorted_features.sort_by(|a, b| {
            // Same idea as the non-symbol sort order, but symbol features may
            // have changed their sort order since their corresponding
            // IndexedSubfeature was added to the CollisionIndex The
            // 'featureSortOrder' is relatively inefficient for querying but
            // cheap to build on every bucket sort
            if let Some(order) = feature_sort_order {
                // queryRenderedSymbols documentation says we'll return features in
                // "top-to-bottom" rendering order (aka last-to-first).
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

        let transform_state = TransformState::default();
        let unused_projector =
            TileProjector::with_data(&transform_state, &tile_id.to_unwrapped(), ProjectionData::default());
        for symbol_feature in sorted_features {
            self.add_feature(
                &mut result,
                symbol_feature.index,
                &symbol_feature.source_layer_name,
                &symbol_feature.bucket_leader_id,
                query_options,
                global_state,
                tile_id,
                layers,
                &GeometryCoordinates::new(),
                &transform_state,
                0.0,
                &unused_projector,
                None,
            )?;
        }
        Ok(result)
    }

    /// `addFeature(result, indexedFeature, options, globalState, tileID,
    /// layers, queryGeometry, transformState, pixelsToTileUnits, projector,
    /// sourceFeatureState)`.
    #[allow(clippy::too_many_arguments)]
    fn add_feature(
        &self,
        result: &mut QueryResults,
        index: usize,
        source_layer_name: &str,
        bucket_leader_id: &str,
        options: &RenderedQueryOptions,
        global_state: Option<&GlobalStateMap>,
        tile_id: &OverscaledTileID,
        layers: &QueryLayers<'_>,
        query_geometry: &GeometryCoordinates,
        transform_state: &TransformState,
        pixels_to_tile_units: f32,
        projector: &TileProjector<'_>,
        source_feature_state: Option<&SourceFeatureState>,
    ) -> Result<(), Error> {
        let Some(tile_data) = &self.tile_data else {
            return Ok(());
        };
        // C++ `bucketLayerIDs.at(..)` throws for an unknown leader.
        let Some(layer_ids) = self.bucket_layer_ids.get(bucket_leader_id) else {
            debug_assert!(false, "unknown bucket leader {bucket_leader_id}");
            return Ok(());
        };

        // C++ fetches the source layer, the feature and its state lazily, at
        // the first of the bucket's layers that is queried; they are the
        // same for every layer.
        if !layer_ids.iter().any(|layer_id| layers.contains_key(layer_id)) {
            return Ok(());
        }
        let Some(source_layer) = tile_data.get_layer(source_layer_name)? else {
            debug_assert!(false, "source layer {source_layer_name} missing");
            return Ok(());
        };
        let feature = source_layer.get_feature(index)?;
        let source_layer_id = source_layer.get_name();
        let mut state = FeatureState::default();
        if let Some(source_feature_state) = source_feature_state
            && let Some(id_str) = feature_id_to_string(&feature.get_id())
        {
            state = source_feature_state.get_state(Some(&source_layer_id), &id_str);
        }

        for layer_id in layer_ids {
            let Some(render_layer) = layers.get(layer_id) else {
                continue;
            };

            let needs_cross_tile_index = render_layer.base().base_impl.get_type_info().cross_tile_index
                == layer_type_info::CrossTileIndex::Required;
            // we use the float camera zoom here, not the integer tile ID zoom,
            // because for the query we take into account the actual rendered
            // size of the geometry (#2781)
            let intersects = needs_cross_tile_index
                || render_layer.query_intersects_feature(
                    query_geometry,
                    &*feature,
                    transform_state.get_zoom() as f32,
                    transform_state,
                    pixels_to_tile_units,
                    projector,
                    &state,
                );
            feature.check()?;
            if !intersects {
                continue;
            }

            // Match layer filters and source queries, including overzoomed tiles.
            if let Some(filter) = &options.filter {
                let context = EvaluationContext::with_zoom_feature(f32::from(tile_id.overscaled_z), Some(&*feature))
                    .with_global_state(global_state);
                let passes = filter.evaluate(&context);
                feature.check()?;
                if !passes {
                    continue;
                }
            }

            // C++ throws out of the query for a malformed geometry (from
            // `convertFeature`) or value.
            let converted = mln_tile::geometry_tile_data::convert_feature(&*feature, &tile_id.canonical)
                .map_err(|e| Error::native(e.to_string()))?;
            feature.check()?;
            let mut converted = Feature::from(converted);
            converted.source = render_layer.base().base_impl.source.clone();
            converted.source_layer = source_layer_id.clone();
            converted.state = state.clone();
            result.entry(layer_id.clone()).or_default().push(converted);
        }
        Ok(())
    }

    /// `translateQueryGeometry(queryGeometry, translate, anchorType, bearing,
    /// pixelsToTileUnits)`.
    #[must_use]
    pub fn translate_query_geometry(
        query_geometry: &GeometryCoordinates,
        translate: &[f32; 2],
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
            translate_vec = mln_util::math::rotate_i16(translate_vec, -bearing);
        }

        Some(
            query_geometry
                .iter()
                .map(|p| GeometryCoordinate::new(p.x.wrapping_sub(translate_vec.x), p.y.wrapping_sub(translate_vec.y)))
                .collect(),
        )
    }

    /// `setBucketLayerIDs(bucketLeaderID, layerIDs)`.
    pub fn set_bucket_layer_ids(&mut self, bucket_leader_id: &str, layer_ids: &[String]) {
        let key = self.intern_leader_id(bucket_leader_id);
        self.bucket_layer_ids.insert(key, layer_ids.to_vec());
    }

    /// The bucket leaders and their layer ids (sorted, for inspection).
    #[must_use]
    pub fn bucket_layer_ids(&self) -> VecMap<String, Vec<String>> {
        self.bucket_layer_ids.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }
}

/// Port-only: a `FeatureIndex` as serde writes it, for a tile worker in
/// another wasm instance (`tile/worker_codec)`: the grid's boxes in insertion
/// order, which `insert` puts in an empty grid again, with their strings once
/// each, and the tile data by key (the side that gets the index has the data
/// it sent). A derive would write the grid's cells too, and every element's
/// strings. Only symbol layouts insert circles, which a worker never does.
#[derive(Serialize, Deserialize)]
struct FeatureIndexParts {
    grid: Grid,
    #[serde(with = "crate::tile::worker_codec::fields::tile_data")]
    tile_data: Option<Arc<dyn GeometryTileData + Send + Sync>>,
}

#[derive(Serialize, Deserialize)]
struct Grid {
    sort_index: usize,
    strings: Vec<String>,
    bucket_layer_ids: VecMap<String, Vec<String>>,
    elements: Vec<Element>,
}

/// A box of the grid: its `RefIndexedSubfeature`, with indices into
/// `strings`, and min x, min y, max x, max y.
#[derive(Serialize, Deserialize)]
struct Element {
    index: usize,
    sort_index: usize,
    source_layer_name: u32,
    bucket_leader_id: u32,
    bucket_instance_id: u32,
    collision_group_id: u16,
    bbox: [f32; 4],
}

impl FeatureIndex {
    fn serde_grid(&self) -> Result<Grid, &'static str> {
        if self.grid.has_circles() {
            return Err("a feature index with circles");
        }
        let mut strings: Vec<String> = Vec::new();
        let mut string_ids: HashMap<*const u8, u32> = HashMap::new();
        let mut intern = |s: &Arc<str>| -> u32 {
            *string_ids.entry(s.as_ptr()).or_insert_with(|| {
                strings.push(s.to_string());
                (strings.len() - 1) as u32
            })
        };
        let elements = self
            .grid
            .box_elements()
            .iter()
            .map(|(element, bbox)| Element {
                index: element.index,
                sort_index: element.sort_index,
                source_layer_name: intern(&element.source_layer_name),
                bucket_leader_id: intern(&element.bucket_leader_id),
                bucket_instance_id: element.bucket_instance_id,
                collision_group_id: element.collision_group_id,
                bbox: [bbox.min.x, bbox.min.y, bbox.max.x, bbox.max.y],
            })
            .collect();
        Ok(Grid { sort_index: self.sort_index, strings, bucket_layer_ids: self.bucket_layer_ids(), elements })
    }

    /// The index as serde writes it, without its tile data.
    #[cfg(test)]
    pub(crate) fn encode_grid(&self) -> Vec<u8> {
        postcard::to_allocvec(&self.serde_grid().unwrap()).unwrap()
    }
}

impl Serialize for FeatureIndex {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let grid = self.serde_grid().map_err(serde::ser::Error::custom)?;
        FeatureIndexParts { grid, tile_data: self.tile_data.clone() }.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for FeatureIndex {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let FeatureIndexParts { grid: parts, tile_data } = FeatureIndexParts::deserialize(deserializer)?;
        let mut index = FeatureIndex::new(tile_data);
        let strings: Vec<Arc<str>> = parts.strings.iter().map(|string| index.intern_layer_name(string)).collect();
        let string = |id: u32| {
            strings
                .get(id as usize)
                .cloned()
                .ok_or_else(|| serde::de::Error::custom("feature index string out of range"))
        };
        for (leader, layer_ids) in &parts.bucket_layer_ids {
            index.set_bucket_layer_ids(leader, layer_ids);
        }
        for element in parts.elements {
            // The leaders share the strings of `bucket_layer_ids`, as `insert`
            // interns them.
            let bucket_leader_id = index.intern_leader_id(&string(element.bucket_leader_id)?);
            let [min_x, min_y, max_x, max_y] = element.bbox;
            index.grid.insert(
                RefIndexedSubfeature {
                    index: element.index,
                    sort_index: element.sort_index,
                    source_layer_name: string(element.source_layer_name)?,
                    bucket_leader_id,
                    bucket_instance_id: element.bucket_instance_id,
                    collision_group_id: element.collision_group_id,
                },
                &BBox::new(Point::new(min_x, min_y), Point::new(max_x, max_y)),
            );
        }
        index.sort_index = parts.sort_index;
        Ok(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translate_query_geometry() {
        let query: GeometryCoordinates = vec![Point::new(10, 10), Point::new(20, 20)];
        assert_eq!(
            FeatureIndex::translate_query_geometry(&query, &[0.0, 0.0], TranslateAnchorType::Map, 0.0, 8.0),
            None
        );
        assert_eq!(
            FeatureIndex::translate_query_geometry(&query, &[1.0, -2.0], TranslateAnchorType::Map, 1.0, 8.0),
            Some(vec![Point::new(2, 26), Point::new(12, 36)])
        );
        // Viewport anchor: the translation is rotated by -bearing.
        assert_eq!(
            FeatureIndex::translate_query_geometry(
                &query,
                &[1.0, 0.0],
                TranslateAnchorType::Viewport,
                std::f32::consts::FRAC_PI_2,
                8.0
            ),
            Some(vec![Point::new(10, 18), Point::new(20, 28)])
        );
    }

    #[test]
    fn insert_records_leaders_and_skips_rings_outside_the_tile() {
        let mut index = FeatureIndex::new(None);
        index.set_bucket_layer_ids("a", &["a".to_owned(), "b".to_owned()]);
        let inside = GeometryCollection::from(vec![vec![Point::new(10, 10)]]);
        let outside = GeometryCollection::from(vec![vec![Point::new(9000, 10)]]);
        index.insert(&inside, 0, "layer", "a");
        index.insert(&outside, 1, "layer", "a");
        assert_eq!(index.bucket_layer_ids()["a"], vec!["a".to_owned(), "b".to_owned()]);
        let found = index.grid.query(&BBox::new(Point::new(0.0, 0.0), Point::new(8192.0, 8192.0)));
        assert_eq!(found.iter().map(|f| f.index).collect::<Vec<_>>(), vec![0]);
    }
}
