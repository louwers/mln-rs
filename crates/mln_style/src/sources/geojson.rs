//! GeoJSON sources: features from GeoJSON data that the style or the API
//! gives, or that the source's URL points to, cut into tiles or clustered.

use std::sync::{Arc, OnceLock};

use feature::{Feature, PropertyMap, Value};
use geojson::GeoJson;
use mln_expression::expression::evaluate_accumulated;
use mln_storage::{Data, Resource};
use mln_style_types::SourceType;
use mln_tile_id::CanonicalTileID;
use mln_util::constants;
use mln_util::error::Error;
use mln_util::logging::{Event, Log};
use mln_util::pool::{Pool, Respond, TaskGroup};
use mln_util::range::Range;
use mln_vec_map::VecMap;
use parking_lot::Mutex;

use crate::conversion::geojson::convert_json_geojson;
use crate::conversion::{Convertible, Error as ConversionError};
use crate::expression::{Expr, Functions, from_expression_value};
use crate::layer_type::SourceKind;
use crate::source_type::{
    Described, Load, OfflineResources, Source, SourceData, SourceSummary, SourceTypeInfo, StyleSourceType,
};

/// The features of tile `id`. For a zoom above the maximum zoom or a missing
/// parent tile, `get_tile` fails (C++ throws); this logs the error and hands
/// out an empty tile.
fn geojsonvt_tile_features(vt: &Mutex<geojson_vt::GeoJSONVT>, id: &CanonicalTileID) -> TileFeatures {
    match vt.lock().get_tile(id.z, id.x, id.y) {
        Ok(tile) => tile.features.clone(),
        Err(e) => {
            // C++ lets the exception propagate; here it is logged.
            Log::error(Event::General, e);
            TileFeatures::new()
        }
    }
}

/// A `GeoJSONVT`'s tiles, which it cuts on the calling thread, or, given a
/// pool, one at a time and in the order asked, so that one source never
/// holds more than one of the pool's threads.
struct GeoJSONVTData {
    vt: Arc<Mutex<geojson_vt::GeoJSONVT>>,
    /// Where the tiles asked for with a pool are cut: a serial group on the
    /// first pool given, kept while the data lives. The data is made when
    /// the style is parsed, before any renderer gives it a pool.
    slices: OnceLock<TaskGroup>,
}

impl std::fmt::Debug for GeoJSONVTData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GeoJSONVTData")
    }
}

impl GeoJSONData for GeoJSONVTData {
    fn get_tile(&self, id: &CanonicalTileID, callback: Box<dyn FnOnce(TileFeatures) + Send>, pool: Option<&Pool>) {
        match pool {
            Some(pool) => {
                let (vt, id) = (Arc::clone(&self.vt), *id);
                self.slices.get_or_init(|| TaskGroup::serial(pool)).spawn(move || {
                    callback(geojsonvt_tile_features(&vt, &id));
                });
            }
            None => callback(geojsonvt_tile_features(&self.vt, id)),
        }
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

struct SuperclusterData {
    impl_: supercluster::Supercluster,
}

impl std::fmt::Debug for SuperclusterData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SuperclusterData")
    }
}

impl GeoJSONData for SuperclusterData {
    fn get_tile(&self, id: &CanonicalTileID, callback: Box<dyn FnOnce(TileFeatures) + Send>, _pool: Option<&Pool>) {
        callback(self.impl_.get_tile(id.z, id.x, id.y));
    }

    fn get_children(&self, cluster_id: u32) -> Result<Features, Error> {
        self.impl_.get_children(cluster_id).map_err(Error::not_found)
    }

    fn get_leaves(&self, cluster_id: u32, limit: u32, offset: u32) -> Result<Features, Error> {
        self.impl_.get_leaves(cluster_id, limit, offset).map_err(Error::not_found)
    }

    fn get_cluster_expansion_zoom(&self, cluster_id: u32) -> Result<u8, Error> {
        self.impl_.get_cluster_expansion_zoom(cluster_id).map_err(Error::not_found)
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

/// The data of `features`. Only the features of a feature collection
/// (`collection`) cluster.
fn create(features: Features, collection: bool, options: &Arc<GeoJSONOptions>) -> Result<Arc<dyn GeoJSONData>, Error> {
    let scale: f64 = f64::from(constants::EXTENT) / constants::TILE_SIZE_D;
    if options.cluster && collection && !features.is_empty() {
        let mut cluster_options = supercluster::Options {
            max_zoom: options.cluster_max_zoom,
            extent: constants::EXTENT as u16,
            radius: (scale * f64::from(options.cluster_radius)).round() as u16,
            min_points: options.cluster_min_points,
            ..Default::default()
        };

        // C++ shares one feature (default geometry) between the two
        // closures and assigns its properties before each
        // evaluation; a fresh feature per evaluation is the same.
        let map_options = Arc::clone(options);
        cluster_options.map = Arc::new(move |properties: &PropertyMap| -> PropertyMap {
            let mut ret = PropertyMap::new();
            if properties.is_empty() {
                return ret;
            }
            for (key, (map_expression, _)) in &map_options.cluster_properties {
                let feature = Feature { properties: properties.clone(), ..Feature::default() };
                ret.insert(key.clone(), evaluate_feature(&feature, map_expression, None));
            }
            ret
        });
        let reduce_options = Arc::clone(options);
        cluster_options.reduce = Some(Arc::new(move |to_return: &mut PropertyMap, to_fill: &PropertyMap| {
            for (key, (_, reduce_expression)) in &reduce_options.cluster_properties {
                if !to_fill.contains_key(key) {
                    continue;
                }
                let feature = Feature { properties: to_fill.clone(), ..Feature::default() };
                // `toReturn[p.first]` default-constructs a missing entry.
                let accumulated = to_return.entry(key.clone()).or_default().clone();
                let value = evaluate_feature(&feature, reduce_expression, Some(&accumulated));
                to_return.insert(key.clone(), value);
            }
        }));
        let impl_ = supercluster::Supercluster::new(features, cluster_options).map_err(Error::invalid_argument)?;
        return Ok(Arc::new(SuperclusterData { impl_ }));
    }

    let vt_options = geojson_vt::Options {
        max_zoom: options.maxzoom,
        tile: geojson_vt::TileOptions {
            extent: constants::EXTENT as u16,
            buffer: (scale * f64::from(options.buffer)).round() as u16,
            tolerance: scale * options.tolerance,
            line_metrics: options.line_metrics,
        },
        ..Default::default()
    };
    let vt = Arc::new(Mutex::new(geojson_vt::GeoJSONVT::new(&features, vt_options)));
    Ok(Arc::new(GeoJSONVTData { vt, slices: OnceLock::new() }))
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

    // Supercluster options
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

pub type TileFeatures = geojson_vt::TileFeatures;
pub type Features = Vec<Feature<f64>>;

pub trait GeoJSONData: Send + Sync {
    /// Gets the features of tile `id`. `callback` runs either right away,
    /// on the calling thread, or later on a pool, for data that cuts its
    /// tiles there: the first `pool` it was given, which it keeps. Without a
    /// pool, always right away.
    fn get_tile(&self, id: &CanonicalTileID, callback: Box<dyn FnOnce(TileFeatures) + Send>, pool: Option<&Pool>);

    // SuperclusterData: data that isn't clustered has no clusters, and
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
        Log::error(Event::ParseStyle, format!("Failed to parse GeoJSON data: {}", error.message));
        return None;
    };
    match options.data_builder().build(geojson) {
        Ok(data) => Some(data),
        Err(e) => {
            Log::error(Event::ParseStyle, format!("Failed to parse GeoJSON data: {e}"));
            None
        }
    }
}

impl StyleSourceType for GeoJsonSourceType {
    const INFO: SourceTypeInfo = SourceTypeInfo {
        name: "geojson",
        layer_kind: SourceKind::Geometry,
        from_json: true,
        type_: SourceType::GeoJSON,
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
            Some(url) => Load::Fetch(Resource::source(url)),
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

/// The data of `source`, if it's a GeoJSON source with data.
#[must_use]
pub fn geojson_data(source: &Source) -> Option<&Arc<dyn GeoJSONData>> {
    SourceData::<GeoJsonSourceType>::recover(source)?.description.as_ref()
}

// Debug is not a supertrait, for the reason at
// `impl Debug for dyn ErasedLayerStyle` (layer_type.rs).
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
