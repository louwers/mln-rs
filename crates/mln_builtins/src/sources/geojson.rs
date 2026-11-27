//! GeoJSON sources: features from GeoJSON data that the style or the API
//! gives, or that the source's URL points to, cut into tiles or clustered.
//! A source's description is its data, indexed for tiling or clustered as it
//! loads; its tiles take their features from it, cut off the tiles' thread
//! unless the source updates synchronously.

use std::sync::Arc;

use geojson::GeoJson;
use mln_engine::exec::pool::TaskGroup;
use mln_engine::map::MapMode;
use mln_engine::query::FeatureExtensionValue;
use mln_engine::source::{
    Cover, DataOrigin, Described, Fetch, GeometryTiles, Load, LoadContext, OfflineResources, OriginUpdate, Replier,
    Respond, SourceSummary, SourceType, Supply, WorldCopy,
};
use mln_engine::storage::Data;
use mln_engine::tile::TileData;
use mln_geojson::{cluster, tile};
use mln_style::error::Error;
use mln_style::expression::{Expr, evaluate_accumulated, from_expression_value};
use mln_style::feature::{Feature, PropertyMap, Value};
use mln_style::source::SourceData;
use mln_style::sources::GeoJsonSourceType;
use mln_style::sources::geojson::{ClusterProperties, GeoJSONOptions, GeoJsonSourceOptions};
use mln_style::tile_id::CanonicalTileID;
use mln_style::{Edit, Source, constants};
use parking_lot::Mutex;

/// The `log` targets of this module's records, which name the event that
/// `mln_c` reports them under (`mln_c::logging`).
const PARSE_STYLE: &str = "mln_builtins::parse_style";
const GENERAL: &str = "mln_builtins::general";

/// The engine half of the GeoJSON source type, whose style half is
/// `mln_style`'s [`GeoJsonSourceType`].
pub struct GeoJsonSource;

impl SourceType for GeoJsonSource {
    type Style = GeoJsonSourceType;
    type Data = GeometryTiles;
    type Description = Arc<dyn GeoJSONData>;

    const DESCRIBES_IN_BACKGROUND: bool = true;

    /// The GeoJSON at the source's URL, or the GeoJSON the style gives,
    /// which the source indexes as it describes it.
    fn load(source: &SourceData<GeoJsonSourceType>) -> Load<Arc<dyn GeoJSONData>> {
        match (&source.options.url, &source.options.inline) {
            (Some(url), _) => Load::Fetch(Fetch::Source(url.clone())),
            (None, Some(_)) => Load::Describe,
            (None, None) => Load::Done,
        }
    }

    /// Builds the fetched GeoJSON, or the GeoJSON the style gives. Data that
    /// doesn't build is logged and describes an empty source, so that its
    /// tiles don't wait for data.
    fn describe(options: &GeoJsonSourceOptions, data: Option<Data>, respond: Respond<Described<Arc<dyn GeoJSONData>>>) {
        respond(match (data, &options.inline) {
            (Some(data), _) => Ok(build_geojson(&data, options)),
            (None, Some(inline)) => Ok(build_inline(inline, options)),
            (None, None) => Err(Error::native("unexpectedly empty GeoJSON")),
        });
    }

    fn attribution(source: &SourceData<GeoJsonSourceType>) -> Option<String> {
        source.options.options.attribution.clone()
    }

    fn summary(source: &SourceData<GeoJsonSourceType>) -> SourceSummary {
        SourceSummary {
            url: source.options.url.clone(),
            geojson: Some(Arc::clone(&source.options.options)),
            ..SourceSummary::default()
        }
    }

    fn offline(source: &SourceData<GeoJsonSourceType>) -> OfflineResources {
        match &source.options.url {
            Some(url) => OfflineResources::Source { url: url.clone() },
            None => OfflineResources::None,
        }
    }

    fn supply(_source: &Source) -> Supply<GeometryTiles> {
        Supply::tiles(GeoJsonOrigin { data: None, synchronous: false })
    }

    /// The "supercluster" extension: a cluster feature's "children",
    /// "leaves" (`args` "limit", default 10, and "offset", default 0) or
    /// "expansion-zoom". Anything else is a null value.
    fn query_feature_extension(
        source: &Source,
        feature: &mln_engine::tile::feature::Feature,
        extension: &str,
        field: &str,
        args: Option<&PropertyMap>,
    ) -> Result<FeatureExtensionValue, Error> {
        query_feature_extension(source, feature, extension, field, args)
    }
}

/// The features of tile `id`. A tile past the maximum zoom or the last row
/// is logged, and empty.
fn tile_features(index: &tile::TileIndex, id: &CanonicalTileID) -> Arc<TileFeatures> {
    index.tile(id.z, id.x, id.y).unwrap_or_else(|e| {
        log::error!(target: GENERAL, "{e}");
        Arc::default()
    })
}

/// A `TileIndex`'s tiles, which it cuts as they are asked for.
struct IndexData {
    index: tile::TileIndex,
}

impl std::fmt::Debug for IndexData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("IndexData")
    }
}

impl GeoJSONData for IndexData {
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
    Ok(Arc::new(IndexData { index: tile::TileIndex::new(&features, tile_options) }))
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
/// [`ErrorKind::InvalidArgument`](mln_style::error::ErrorKind).
pub fn create_geojson_data(geojson: GeoJson, options: &Arc<GeoJSONOptions>) -> Result<Arc<dyn GeoJSONData>, Error> {
    let collection = matches!(geojson, GeoJson::FeatureCollection(_));
    let features =
        mln_style::feature::geojson::features(geojson).map_err(|e| Error::invalid_argument(e.to_string()))?;
    create(features, collection, options)
}

/// The data of GeoJSON text `data`, built with `options`. Text that isn't
/// GeoJSON, or GeoJSON that can't be built, is logged and builds nothing.
fn build_geojson(data: &[u8], options: &GeoJsonSourceOptions) -> Option<Arc<dyn GeoJSONData>> {
    match serde_json::from_slice(data) {
        Ok(geojson) => build_logged(geojson, options),
        Err(e) => {
            log::error!(target: PARSE_STYLE, "Failed to parse GeoJSON data: {e}");
            None
        }
    }
}

/// The data of `inline`, the GeoJSON a style gives, built with `options`,
/// as [`build_geojson`] builds fetched GeoJSON.
fn build_inline(inline: &Result<GeoJson, String>, options: &GeoJsonSourceOptions) -> Option<Arc<dyn GeoJSONData>> {
    match inline {
        Ok(geojson) => build_logged(geojson.clone(), options),
        Err(e) => {
            log::error!(target: PARSE_STYLE, "Failed to parse GeoJSON data: {e}");
            None
        }
    }
}

/// The data of `geojson` built with `options`, or `None`, logged, for
/// GeoJSON that can't be built.
fn build_logged(geojson: GeoJson, options: &GeoJsonSourceOptions) -> Option<Arc<dyn GeoJSONData>> {
    match create_geojson_data(geojson, &options.options) {
        Ok(data) => Some(data),
        Err(e) => {
            log::error!(target: PARSE_STYLE, "Failed to parse GeoJSON data: {e}");
            None
        }
    }
}

/// A GeoJSON source's tiles: the data they were made from, and whether they
/// were made with synchronous updates.
struct GeoJsonOrigin {
    /// The data, and where tiles of data that cuts them on demand are cut,
    /// off the tiles' thread: a lane of its own, which takes turns with the
    /// workers' other work.
    data: Option<(Arc<dyn GeoJSONData>, TaskGroup)>,
    synchronous: bool,
}

impl GeoJsonOrigin {
    /// Loads the tile from the source's data: cut on the pool unless the
    /// source updates synchronously. With `relayout`, the tile first drops
    /// its data and style.
    fn load(&self, cx: &mut LoadContext<'_, '_, Self>, relayout: bool) {
        let Some((data, cuts)) = &self.data else {
            return;
        };
        if relayout {
            cx.reset();
        }
        let replier = cx.begin_load();
        let cuts = (!self.synchronous).then_some(cuts);
        if let Some(features) = request_tile(data, cx.id().canonical, replier, cuts) {
            cx.deliver(Some(features));
        }
    }
}

impl DataOrigin for GeoJsonOrigin {
    type Data = GeometryTiles;
    type Tile = ();
    type Reply = TileData;

    fn update(&mut self, update: &mut OriginUpdate<'_, Self>) -> Option<Cover> {
        let source = SourceData::<GeoJsonSourceType>::recover(update.source())?;
        // A tile lays out in the mode it was created with, so a mode change
        // recreates the tiles.
        let synchronous = source.options.synchronous_update;
        if synchronous != self.synchronous {
            self.synchronous = synchronous;
            update.clear_tiles();
        }
        let data = source.description::<Arc<dyn GeoJSONData>>();
        let same = match (self.data.as_ref().map(|(data, _)| data), data) {
            (None, None) => true,
            (Some(a), Some(b)) => std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(b)),
            _ => false,
        };
        if !same {
            self.data = data.map(|data| (Arc::clone(data), TaskGroup::new(update.pool())));
            if update.map().mode != MapMode::Continuous {
                // Clearing the tiles in order to avoid render tests being
                // flaky.
                update.clear_tiles();
            } else if data.is_some() {
                update.clear_cache();
                let max_z = source.options.get_zoom_range().max;
                let relayout = update.needs_relayout();
                update.update_tiles(|id| id.canonical.z <= max_z, |(), cx| self.load(cx, relayout));
            }
        }
        data?;
        Some(Cover {
            tile_size: constants::TILE_SIZE_I,
            zoom_range: source.options.get_zoom_range(),
            bounds: None,
            round_zoom: false,
            overscale: true,
            prefetch: false,
            synchronous,
        })
    }

    fn start(&mut self, cx: &mut LoadContext<'_, '_, Self>, _copy: Option<WorldCopy<'_, ()>>) {
        self.load(cx, false);
    }

    fn receive(&mut self, (): &mut (), features: TileData, cx: &mut LoadContext<'_, '_, Self>) {
        cx.deliver(Some(features));
    }
}

/// Asks GeoJSON `data` for tile `id`'s features, cut in `cuts` if there is
/// one and the data cuts its tiles on demand: returns them if they answer
/// before this returns, else `replier` sends them.
fn request_tile(
    data: &Arc<dyn GeoJSONData>,
    id: CanonicalTileID,
    replier: Replier<TileData>,
    cuts: Option<&TaskGroup>,
) -> Option<TileData> {
    let Some(cuts) = cuts.filter(|_| data.cuts_on_demand()) else {
        return Some(TileData::Features(data.get_tile(&id)));
    };
    let slot: Arc<Mutex<Answered>> = Arc::default();
    let task_slot = Arc::clone(&slot);
    let data_for_task = Arc::clone(data);
    cuts.spawn(move || {
        let data = TileData::Features(data_for_task.get_tile(&id));
        let mut slot = task_slot.lock();
        if matches!(*slot, Answered::Later) {
            drop(slot);
            replier.send(data);
        } else {
            *slot = Answered::Now(data);
        }
    });
    match std::mem::replace(&mut *slot.lock(), Answered::Later) {
        Answered::Now(data) => Some(data),
        Answered::Pending | Answered::Later => None,
    }
}

/// When a tile cut off the tiles' thread answered: before its request
/// returned, or after.
#[derive(Default)]
enum Answered {
    #[default]
    Pending,
    Now(TileData),
    Later,
}

/// [`GeoJsonSource::query_feature_extension`]: a null value for a source of
/// another type.
fn query_feature_extension(
    source: &Source,
    feature: &mln_engine::tile::feature::Feature,
    extension: &str,
    extension_field: &str,
    args: Option<&PropertyMap>,
) -> Result<FeatureExtensionValue, Error> {
    if extension != "supercluster" {
        return Ok(FeatureExtensionValue::default());
    }
    let Some(cluster_id) = get_uint_property(&feature.properties, "cluster_id") else {
        return Ok(FeatureExtensionValue::default());
    };
    let Some(json_data) = geojson_data(source) else {
        return Ok(FeatureExtensionValue::default());
    };
    supercluster_extension(&**json_data, extension_field, cluster_id as u32, args)
        .unwrap_or_else(|| Ok(FeatureExtensionValue::default()))
}

/// The property `name`, if it is an unsigned integer.
fn get_uint_property(properties: &PropertyMap, name: &str) -> Option<u64> {
    match properties.get(name) {
        Some(Value::Uint(value)) => Some(*value),
        _ => None,
    }
}

/// The supercluster `extensionGetters`: the `extension_field` of cluster
/// `cluster_id`, or `None` for a field it doesn't have.
fn supercluster_extension(
    cluster_data: &dyn GeoJSONData,
    extension_field: &str,
    cluster_id: u32,
    args: Option<&PropertyMap>,
) -> Option<Result<FeatureExtensionValue, Error>> {
    Some(match extension_field {
        "children" => cluster_data.get_children(cluster_id).map(FeatureExtensionValue::FeatureCollection),
        "leaves" => {
            // Offset cannot be set without limit.
            let (limit, offset) = match args.and_then(|args| Some((get_uint_property(args, "limit")?, args))) {
                Some((limit, args)) => {
                    (limit as u32, get_uint_property(args, "offset").map_or(0, |offset| offset as u32))
                }
                None => (10, 0),
            };
            cluster_data.get_leaves(cluster_id, limit, offset).map(FeatureExtensionValue::FeatureCollection)
        }
        "expansion-zoom" => cluster_data
            .get_cluster_expansion_zoom(cluster_id)
            .map(|zoom| FeatureExtensionValue::Value(Value::Uint(u64::from(zoom)))),
        _ => return None,
    })
}

/// The data of `source`, if it's a GeoJSON source with data.
#[must_use]
pub fn geojson_data(source: &Source) -> Option<&Arc<dyn GeoJSONData>> {
    SourceData::<GeoJsonSourceType>::recover(source)?.description::<Arc<dyn GeoJSONData>>()
}

/// The edit that replaces the data of the GeoJSON source `id`, which no
/// longer loads it from a URL or the style.
pub fn set_data(id: impl Into<String>, geojson: Option<Arc<dyn GeoJSONData>>) -> Edit {
    Edit::update_source::<GeoJsonSourceType>(id, "a GeoJSON source", |data| {
        data.options.url = None;
        data.options.inline = None;
        data.description = geojson.map(|geojson| -> mln_style::source::ErasedDescription { Arc::new(geojson) });
        data.load += 1;
        true
    })
}

// Debug is not a supertrait, for the reason at mln_style's
// `impl Debug for dyn ErasedLayerStyle`.
impl std::fmt::Debug for dyn GeoJSONData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeoJSONData").finish_non_exhaustive()
    }
}
