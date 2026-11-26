//! GeoJSON sources: features from GeoJSON data that the style or the API
//! gives, or that the source's URL points to, cut into tiles or clustered.

use std::sync::{Arc, OnceLock};

use feature::{Feature, PropertyMap, Value};
use geojson::GeoJson;
use mln_geojson::{cluster, tile};
use mln_util::constants;
use mln_util::error::Error;
use mln_util::range::Range;
use mln_vec_map::VecMap;

use crate::conversion::geojson::convert_json_geojson;
use crate::conversion::{Convertible, Error as ConversionError};
use crate::expression::{Expr, Functions, evaluate_accumulated, from_expression_value};
use crate::layer::SourceKind;
use crate::source::{
    Data, Described, Fetch, Load, OfflineResources, Respond, Source, SourceData, SourceSummary, SourceTypeInfo,
    StyleSourceType,
};
use crate::tile_id::CanonicalTileID;
use crate::{Edit, log_target};

/// The features of tile `id`. A tile past the maximum zoom or the last row
/// is logged, and empty.
fn tile_features(index: &tile::TileIndex, id: &CanonicalTileID) -> Arc<TileFeatures> {
    index.tile(id.z, id.x, id.y).unwrap_or_else(|e| {
        log::error!(target: log_target::GENERAL, "{e}");
        Arc::default()
    })
}

/// A `TileIndex`'s tiles, which it cuts as they are asked for.
struct TileData {
    index: tile::TileIndex,
}

impl std::fmt::Debug for TileData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TileData")
    }
}

impl GeoJSONData for TileData {
    fn get_tile(&self, id: &CanonicalTileID) -> Arc<TileFeatures> {
        tile_features(&self.index, id)
    }

    fn cuts_on_demand(&self) -> bool {
        true
    }

    fn get_children(&self, _cluster_id: u32) -> Result<Features, Error> {
        Ok(Features::new())
    }

    fn get_leaves(&self, _cluster_id: u32, _limit: u32, _offset: u32) -> Result<Features, Error> {
        Ok(Features::new())
    }

    fn get_cluster_expansion_zoom(&self, _cluster_id: u32) -> Result<u8, Error> {
        Ok(0)
    }
}

struct ClusterData {
    clusters: cluster::Clusters,
}

impl std::fmt::Debug for ClusterData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ClusterData")
    }
}

impl GeoJSONData for ClusterData {
    fn get_tile(&self, id: &CanonicalTileID) -> Arc<TileFeatures> {
        Arc::new(self.clusters.tile(id.z, id.x, id.y))
    }

    fn get_children(&self, cluster_id: u32) -> Result<Features, Error> {
        self.clusters.children(cluster_id).map_err(|e| Error::not_found(e.to_string()))
    }

    fn get_leaves(&self, cluster_id: u32, limit: u32, offset: u32) -> Result<Features, Error> {
        self.clusters.leaves(cluster_id, limit as usize, offset as usize).map_err(|e| Error::not_found(e.to_string()))
    }

    fn get_cluster_expansion_zoom(&self, cluster_id: u32) -> Result<u8, Error> {
        self.clusters.expansion_zoom(cluster_id).map_err(|e| Error::not_found(e.to_string()))
    }
}

fn evaluate_feature(f: &Feature<f64>, expression: &Arc<Expr>, accumulated: Option<&Value>) -> Value {
    if let Ok(result) = evaluate_accumulated(expression.as_ref(), accumulated, f)
        && let Some(typed) = from_expression_value::<Value>(&result)
    {
        return typed;
    }
    Value::default()
}

/// A source's `clusterProperties`: each property's map expression evaluated
/// on a point, folded with its reduce expression.
struct ClusterExpressions<'a>(&'a ClusterProperties);

impl cluster::Aggregate for ClusterExpressions<'_> {
    fn map(&self, properties: &PropertyMap) -> PropertyMap {
        let mut mapped = PropertyMap::new();
        let feature = Feature { properties: properties.clone(), ..Feature::default() };
        for (key, (map_expression, _)) in self.0 {
            mapped.insert(key.clone(), evaluate_feature(&feature, map_expression, None));
        }
        mapped
    }

    fn reduce(&self, accumulated: &mut PropertyMap, other: &PropertyMap) {
        let feature = Feature { properties: other.clone(), ..Feature::default() };
        for (key, (_, reduce_expression)) in self.0 {
            if !other.contains_key(key) {
                continue;
            }
            let value =
                evaluate_feature(&feature, reduce_expression, Some(accumulated.get(key).unwrap_or(&Value::Null)));
            accumulated.insert(key.clone(), value);
        }
    }
}

/// The data of `features`. Only the features of a feature collection
/// (`collection`) cluster.
fn create(features: Features, collection: bool, options: &Arc<GeoJSONOptions>) -> Result<Arc<dyn GeoJSONData>, Error> {
    let scale: f64 = f64::from(constants::EXTENT) / constants::TILE_SIZE_D;
    if options.cluster && collection && !features.is_empty() {
        let cluster_options = cluster::Options {
            max_zoom: options.cluster_max_zoom,
            extent: constants::EXTENT as u16,
            radius: scale * f64::from(options.cluster_radius),
            min_points: options.cluster_min_points,
        };
        let expressions = ClusterExpressions(&options.cluster_properties);
        let aggregate: Option<&dyn cluster::Aggregate> =
            (!options.cluster_properties.is_empty()).then_some(&expressions);
        let clusters = cluster::Clusters::new(features, cluster_options, aggregate)
            .map_err(|e| Error::invalid_argument(e.to_string()))?;
        return Ok(Arc::new(ClusterData { clusters }));
    }

    let tile_options = tile::Options {
        max_zoom: options.maxzoom,
        tile: tile::TileOptions {
            extent: constants::EXTENT as u16,
            buffer: (scale * f64::from(options.buffer)).round() as u16,
            tolerance: scale * options.tolerance,
            line_metrics: options.line_metrics,
        },
        ..Default::default()
    };
    Ok(Arc::new(TileData { index: tile::TileIndex::new(&features, tile_options) }))
}

pub(crate) type ClusterExpression = (Arc<Expr>, Arc<Expr>);
pub type ClusterProperties = VecMap<String, ClusterExpression>;

#[derive(Clone)]
pub struct GeoJSONOptions {
    // GeoJSON-VT options
    pub minzoom: u8,
    pub maxzoom: u8,
    pub tile_size: u16,
    pub buffer: u16,
    pub tolerance: f64,
    pub line_metrics: bool,

    // Clustering options
    pub cluster: bool,
    pub cluster_radius: u16,
    pub cluster_max_zoom: u8,
    pub cluster_min_points: usize,
    pub cluster_properties: ClusterProperties,

    // Update options
    pub synchronous_update: bool,

    /// The style's attribution for the source, reported by
    /// `Source::get_attribution`.
    pub attribution: Option<String>,
}

impl Default for GeoJSONOptions {
    fn default() -> Self {
        GeoJSONOptions {
            minzoom: 0,
            maxzoom: 18,
            tile_size: constants::TILE_SIZE_I,
            buffer: 128,
            tolerance: 0.375,
            line_metrics: false,
            cluster: false,
            cluster_radius: 50,
            cluster_max_zoom: 17,
            cluster_min_points: 2,
            cluster_properties: ClusterProperties::new(),
            synchronous_update: false,
            attribution: None,
        }
    }
}

impl std::fmt::Debug for GeoJSONOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeoJSONOptions")
            .field("minzoom", &self.minzoom)
            .field("maxzoom", &self.maxzoom)
            .field("tile_size", &self.tile_size)
            .field("buffer", &self.buffer)
            .field("tolerance", &self.tolerance)
            .field("line_metrics", &self.line_metrics)
            .field("cluster", &self.cluster)
            .field("cluster_radius", &self.cluster_radius)
            .field("cluster_max_zoom", &self.cluster_max_zoom)
            .field("cluster_min_points", &self.cluster_min_points)
            .field("cluster_properties", &self.cluster_properties.keys().collect::<Vec<_>>())
            .field("synchronous_update", &self.synchronous_update)
            .field("attribution", &self.attribution)
            .finish()
    }
}

impl GeoJSONOptions {
    /// The default options, as one shared instance.
    pub fn default_options() -> Arc<GeoJSONOptions> {
        static OPTIONS: OnceLock<Arc<GeoJSONOptions>> = OnceLock::new();
        Arc::clone(OPTIONS.get_or_init(|| Arc::new(GeoJSONOptions::default())))
    }
}

pub type TileFeatures = mln_geojson::TileFeatures;
pub type Features = Vec<Feature<f64>>;

pub trait GeoJSONData: Send + Sync {
    /// The features of tile `id`, made on the calling thread. The data may
    /// keep them, and hand them out again.
    fn get_tile(&self, id: &CanonicalTileID) -> Arc<TileFeatures>;

    /// Whether [`get_tile`](Self::get_tile) cuts the tile from the features
    /// as it is asked, work worth taking off the caller's thread, rather
    /// than reading it from an index made with the data, as clusters do.
    fn cuts_on_demand(&self) -> bool {
        false
    }

    // Data that isn't clustered has no clusters, and
    // gives empty results. A cluster ID the data doesn't have fails with
    // `ErrorKind::NotFound`.
    fn get_children(&self, cluster_id: u32) -> Result<Features, Error>;
    fn get_leaves(&self, cluster_id: u32, limit: u32, offset: u32) -> Result<Features, Error>;
    fn get_cluster_expansion_zoom(&self, cluster_id: u32) -> Result<u8, Error>;
}

/// Prepares GeoJSON for tiling. GeoJSON whose geometry `geo_types` can't
/// hold (a position of fewer than two numbers), or that can't be clustered,
/// such as a non-point feature, fails with
/// [`ErrorKind::InvalidArgument`](mln_util::ErrorKind).
pub fn create_geojson_data(geojson: GeoJson, options: &Arc<GeoJSONOptions>) -> Result<Arc<dyn GeoJSONData>, Error> {
    let collection = matches!(geojson, GeoJson::FeatureCollection(_));
    let features = feature::geojson::features(geojson).map_err(|e| Error::invalid_argument(e.to_string()))?;
    create(features, collection, options)
}

/// Builds the data of a GeoJSON source with the source's options. It is
/// `Send`, so the work can leave the source's thread.
#[derive(Clone)]
pub struct GeoJSONDataBuilder {
    options: Arc<GeoJSONOptions>,
}

impl GeoJSONDataBuilder {
    /// A builder of data with `options`.
    #[must_use]
    pub fn new(options: Arc<GeoJSONOptions>) -> Self {
        GeoJSONDataBuilder { options }
    }

    /// Fails as [`create_geojson_data`] does.
    pub fn build(&self, geojson: GeoJson) -> Result<Arc<dyn GeoJSONData>, Error> {
        create_geojson_data(geojson, &self.options)
    }
}

/// The GeoJSON source type.
pub struct GeoJsonSourceType;

/// A GeoJSON source's options.
#[derive(Clone, Debug)]
pub struct GeoJsonSourceOptions {
    pub options: Arc<GeoJSONOptions>,
    /// The URL of the source's GeoJSON, if it loads it from one.
    pub url: Option<String>,
    /// `options.synchronous_update` until the source's override replaces it.
    pub synchronous_update: bool,
}

impl GeoJsonSourceOptions {
    #[must_use]
    pub fn new(options: Arc<GeoJSONOptions>) -> Self {
        let synchronous_update = options.synchronous_update;
        GeoJsonSourceOptions { options, url: None, synchronous_update }
    }

    #[must_use]
    pub fn get_zoom_range(&self) -> Range<u8> {
        Range::new(self.options.minzoom, self.options.maxzoom)
    }

    /// What builds data for the source with its options, on any thread.
    #[must_use]
    pub fn data_builder(&self) -> GeoJSONDataBuilder {
        GeoJSONDataBuilder::new(Arc::clone(&self.options))
    }
}

/// The data of GeoJSON text `data`, built with `options`. Text that isn't
/// GeoJSON, or GeoJSON that can't be built, is logged and builds nothing.
fn build_geojson(data: &[u8], options: &GeoJsonSourceOptions) -> Option<Arc<dyn GeoJSONData>> {
    let mut error = ConversionError::default();
    let Some(geojson) = convert_json_geojson(data, &mut error) else {
        log::error!(target: log_target::PARSE_STYLE, "Failed to parse GeoJSON data: {}", error.message);
        return None;
    };
    match options.data_builder().build(geojson) {
        Ok(data) => Some(data),
        Err(e) => {
            log::error!(target: log_target::PARSE_STYLE, "Failed to parse GeoJSON data: {e}");
            None
        }
    }
}

impl StyleSourceType for GeoJsonSourceType {
    const INFO: SourceTypeInfo = SourceTypeInfo {
        name: "geojson",
        layer_kind: SourceKind::Geometry,
        from_json: true,
        round_zoom: false,
        overscale: true,
        prefetch: false,
    };

    const DESCRIBES_IN_BACKGROUND: bool = true;

    type Options = GeoJsonSourceOptions;
    type Description = Arc<dyn GeoJSONData>;

    fn from_json(
        value: &Convertible<'_>,
        functions: &Arc<Functions>,
        error: &mut ConversionError,
    ) -> Option<SourceData<Self>> {
        crate::conversion::source::convert_geojson_source(value, functions, error)
    }

    /// The GeoJSON at the source's URL, if it has one.
    fn load(source: &SourceData<Self>) -> Load<Arc<dyn GeoJSONData>> {
        match &source.options.url {
            Some(url) => Load::Fetch(Fetch::Source(url.clone())),
            None => Load::Done,
        }
    }

    /// Builds the fetched GeoJSON. Data that doesn't build is logged and
    /// describes an empty source, so that its tiles don't wait for data.
    fn describe(options: &GeoJsonSourceOptions, data: Option<Data>, respond: Respond<Described<Arc<dyn GeoJSONData>>>) {
        respond(match data {
            None => Err(Error::native("unexpectedly empty GeoJSON")),
            Some(data) => Ok(build_geojson(&data, options)),
        });
    }

    fn attribution(source: &SourceData<Self>) -> Option<String> {
        source.options.options.attribution.clone()
    }

    fn summary(source: &SourceData<Self>) -> SourceSummary {
        SourceSummary {
            url: source.options.url.clone(),
            geojson: Some(Arc::clone(&source.options.options)),
            ..SourceSummary::default()
        }
    }

    fn is_update_synchronous(source: &SourceData<Self>) -> bool {
        source.options.synchronous_update
    }

    fn offline(source: &SourceData<Self>) -> OfflineResources {
        match &source.options.url {
            Some(url) => OfflineResources::Source { url: url.clone() },
            None => OfflineResources::None,
        }
    }
}

impl GeoJsonSourceType {
    /// The edit that makes the GeoJSON source `id` load its data from
    /// `url`. Its data stays until the URL's arrives.
    pub fn set_url(id: impl Into<String>, url: impl Into<String>) -> Edit {
        let url = url.into();
        Edit::update_source::<Self>(id, "a GeoJSON source", |data| {
            data.options.url = Some(url);
            data.load += 1;
            true
        })
    }

    /// The edit that replaces the data of the GeoJSON source `id`, which no
    /// longer loads it from a URL.
    pub fn set_data(id: impl Into<String>, geojson: Option<Arc<dyn GeoJSONData>>) -> Edit {
        Edit::update_source::<Self>(id, "a GeoJSON source", |data| {
            data.options.url = None;
            data.description = geojson;
            data.load += 1;
            true
        })
    }

    /// The edit that sets whether the tiles of the GeoJSON source `id` lay
    /// out in the call that asks for them, in place of its
    /// `synchronous_update` option.
    pub fn set_synchronous(id: impl Into<String>, synchronous: bool) -> Edit {
        Edit::update_source::<Self>(id, "a GeoJSON source", move |data| {
            let changed = data.options.synchronous_update != synchronous;
            data.options.synchronous_update = synchronous;
            changed
        })
    }
}

/// The data of `source`, if it's a GeoJSON source with data.
#[must_use]
pub fn geojson_data(source: &Source) -> Option<&Arc<dyn GeoJSONData>> {
    SourceData::<GeoJsonSourceType>::recover(source)?.description.as_ref()
}

// Debug is not a supertrait, for the reason at
// `impl Debug for dyn ErasedLayerStyle` (layer/type_.rs).
impl std::fmt::Debug for dyn GeoJSONData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeoJSONData").finish_non_exhaustive()
    }
}

/// A GeoJSON source with `options` and no data until it gets some or a URL.
#[must_use]
pub fn geojson_source(id: &str, options: Arc<GeoJSONOptions>) -> Source {
    Source::new(id, SourceData::<GeoJsonSourceType>::new(GeoJsonSourceOptions::new(options)))
}
