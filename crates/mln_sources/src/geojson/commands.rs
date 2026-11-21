//! A runtime map's commands for its GeoJSON sources, and a render session's
//! queries of their clusters.

use std::sync::Arc;

use geojson::{GeoJson, GeometryValue};
use mln_runtime::{Applied, Completion, Map, NewSourceData, PrepareSourceData, RenderSession};
use mln_style::expression::Functions;
use mln_style::source_type::{Source, SourceData};
use mln_util::error::Error;

use super::{
    ClusterProperties, Features, GeoJSONData, GeoJSONOptions, GeoJsonSourceType, convert_geojson_options,
    geojson_source, parse_geojson,
};
use crate::not_a;

/// The options of a GeoJSON source as the API gives and reports them, which
/// stay fixed for as long as the style holds the source. The defaults are
/// the style specification's.
#[derive(Clone, Debug, PartialEq)]
pub struct GeoJsonSourceSettings {
    /// The lowest zoom level that the source has tiles for.
    pub min_zoom: u8,
    /// The highest zoom level that the source has tiles for, at most 24.
    pub max_zoom: u8,
    /// How far geometry simplifies, in tile pixels: finite and not negative.
    pub tolerance: f64,
    /// How far each tile reaches past its edges, in tile pixels, at most 512.
    pub buffer: u16,
    /// Whether line features carry their length, for `line-progress`.
    pub line_metrics: bool,
    /// Whether points gather into clusters.
    pub cluster: bool,
    /// How far apart points cluster, in pixels, at most 4095.
    pub cluster_radius: u16,
    /// The highest zoom level that clusters points, at most 30.
    pub cluster_max_zoom: u8,
    /// How many points make a cluster.
    pub cluster_min_points: usize,
    /// The
    /// [`clusterProperties`](https://maplibre.org/maplibre-style-spec/sources/#clusterproperties)
    /// object as JSON text, which the runtime's thread parses.
    pub cluster_properties: Option<String>,
    pub attribution: Option<String>,
}

impl Default for GeoJsonSourceSettings {
    fn default() -> Self {
        GeoJsonSourceSettings::from_core(&GeoJSONOptions::default())
    }
}

impl GeoJsonSourceSettings {
    /// The settings of `source`, if it's a GeoJSON source.
    #[must_use]
    pub fn of(source: &Source) -> Option<Self> {
        SourceData::<GeoJsonSourceType>::recover(source).map(|data| Self::from_core(&data.options.options))
    }

    fn from_core(options: &GeoJSONOptions) -> Self {
        GeoJsonSourceSettings {
            min_zoom: options.minzoom,
            max_zoom: options.maxzoom,
            tolerance: options.tolerance,
            buffer: options.buffer,
            line_metrics: options.line_metrics,
            cluster: options.cluster,
            cluster_radius: options.cluster_radius,
            cluster_max_zoom: options.cluster_max_zoom,
            cluster_min_points: options.cluster_min_points,
            cluster_properties: cluster_properties_json(options),
            attribution: options.attribution.clone(),
        }
    }

    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for an
    /// option out of range.
    fn check(&self) -> Result<(), Error> {
        // Tiling stops at zoom 24, and a cluster ID holds its zoom level plus
        // one in five bits.
        if self.max_zoom > 24 {
            return Err(Error::invalid_argument("max_zoom must be an integer within [0, 24]"));
        }
        if self.cluster_max_zoom > 30 {
            return Err(Error::invalid_argument("cluster_max_zoom must be an integer within [0, 30]"));
        }
        if self.min_zoom > self.max_zoom {
            return Err(Error::invalid_argument("min_zoom must be less than or equal to max_zoom"));
        }
        if !self.tolerance.is_finite() || self.tolerance < 0.0 {
            return Err(Error::invalid_argument("tolerance must be finite and non-negative"));
        }
        if self.buffer > 512 {
            return Err(Error::invalid_argument("buffer must be at most 512"));
        }
        if self.cluster_radius > 4095 {
            return Err(Error::invalid_argument("cluster_radius must be at most 4095"));
        }
        Ok(())
    }

    /// Parses the cluster properties on this thread against `functions`, for
    /// a caller that must reject them before it sends the source, which
    /// [`GeoJsonSourceCommands::add_geojson_source`] otherwise parses on the
    /// runtime's thread. Fails with
    /// [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for text that
    /// isn't a `clusterProperties` object.
    pub fn check_cluster_properties(&self, functions: &Arc<Functions>) -> Result<(), Error> {
        self.cluster_properties.as_deref().map_or(Ok(()), |json| parse_cluster_properties(json, functions).map(drop))
    }

    /// The core's options, with the cluster properties parsed against
    /// `functions`.
    fn into_core(self, functions: &Arc<Functions>) -> Result<GeoJSONOptions, Error> {
        let cluster_properties = match &self.cluster_properties {
            Some(json) => parse_cluster_properties(json, functions)?,
            None => ClusterProperties::default(),
        };
        Ok(GeoJSONOptions {
            minzoom: self.min_zoom,
            maxzoom: self.max_zoom,
            tolerance: self.tolerance,
            buffer: self.buffer,
            line_metrics: self.line_metrics,
            cluster: self.cluster,
            cluster_radius: self.cluster_radius,
            cluster_max_zoom: self.cluster_max_zoom,
            cluster_min_points: self.cluster_min_points,
            cluster_properties,
            attribution: self.attribution,
            ..GeoJSONOptions::default()
        })
    }
}

/// Parses a `clusterProperties` object through the style's conversion of a
/// GeoJSON source, as its one member. Fails with
/// [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for text that isn't
/// such an object.
fn parse_cluster_properties(json: &str, functions: &Arc<Functions>) -> Result<ClusterProperties, Error> {
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|error| Error::invalid_argument(format!("cluster_properties is not valid JSON: {error}")))?;
    if !value.is_object() {
        return Err(Error::invalid_argument("cluster_properties must contain a JSON object"));
    }
    let wrapper = serde_json::json!({ "clusterProperties": value });
    let mut error = mln_conversion::Error::default();
    let options = convert_geojson_options(&mln_conversion::Convertible::from(&wrapper), functions, &mut error)
        .ok_or_else(|| Error::invalid_argument(format!("GeoJSON source options: {}", error.message)))?;
    Ok(options.cluster_properties)
}

/// The `clusterProperties` of `options` as a JSON object.
fn cluster_properties_json(options: &GeoJSONOptions) -> Option<String> {
    if options.cluster_properties.is_empty() {
        return None;
    }
    let members = options
        .cluster_properties
        .iter()
        .map(|(key, (map, reduce))| {
            let pair = feature::Value::Array(vec![reduce.serialize(), map.serialize()]);
            (key.clone(), mln_conversion::value_to_json(&pair))
        })
        .collect::<serde_json::Map<_, _>>();
    Some(serde_json::Value::Object(members).to_string())
}

/// Clustering reads a feature collection of points, so a clustered source
/// refuses other data up front, naming what it found. An empty collection
/// holds nothing to cluster.
fn check_clusterable(geojson: &GeoJson) -> Result<(), Error> {
    let features = match geojson {
        GeoJson::FeatureCollection(collection) => &collection.features,
        other => {
            let found = if matches!(other, GeoJson::Feature(_)) { "a single feature" } else { "a bare geometry" };
            return Err(Error::invalid_argument(format!(
                "clustered GeoJSON data requires a feature collection; the data is {found}"
            )));
        }
    };
    let point = |feature: &geojson::Feature| matches!(&feature.geometry, Some(g) if matches!(g.value, GeometryValue::Point { .. }));
    match features.iter().position(|feature| !point(feature)) {
        None => Ok(()),
        Some(index) => Err(Error::invalid_argument(format!(
            "clustered GeoJSON data requires point geometry on every feature; feature {index} has {} geometry",
            features[index].geometry.as_ref().map_or("no", |g| g.value.type_name())
        ))),
    }
}

fn geojson_data(source: &Source) -> Result<&SourceData<GeoJsonSourceType>, Error> {
    SourceData::recover(source).ok_or_else(|| not_a(source.id(), "a GeoJSON source"))
}

/// Parsed GeoJSON for a source, which builds its data with the source's
/// options.
struct NewGeoJson(GeoJson);

impl NewSourceData for NewGeoJson {
    fn prepare(self: Box<Self>, source: &Source) -> PrepareSourceData {
        let id = source.id().to_owned();
        let builder = geojson_data(source).map(|data| data.options.data_builder());
        Box::new(move || {
            let data = builder?.build(self.0)?;
            Ok(GeoJsonSourceType::set_data(id, Some(data)))
        })
    }
}

/// A runtime map's commands for its GeoJSON sources.
pub trait GeoJsonSourceCommands {
    /// Adds a GeoJSON source with no data and `settings`, which stay fixed
    /// for as long as the style holds it.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for an
    /// option out of range, and completes with it for cluster properties
    /// that don't parse, and as
    /// [`Map::add_style_source`](mln_runtime::Map::add_style_source) does.
    fn add_geojson_source(
        &self,
        source_id: impl Into<String>,
        settings: GeoJsonSourceSettings,
    ) -> Result<Completion<Applied>, Error>;

    /// Makes a GeoJSON source load its data from `url`. Data that a
    /// [`set_geojson_source_data`](Self::set_geojson_source_data) sent before
    /// and that isn't ready yet is cancelled.
    fn set_geojson_source_url(
        &self,
        source_id: impl Into<String>,
        url: impl Into<String>,
    ) -> Result<Completion<Applied>, Error>;

    /// Replaces the data of a GeoJSON source with the GeoJSON text `json`, a
    /// feature collection, feature or geometry.
    ///
    /// The runtime's thread parses the data, and a background thread
    /// prepares it with the source's options; the command completes once
    /// the source holds it. Later commands don't wait for it. While the
    /// source is still preparing earlier data, only the newest data waits
    /// to follow it. With synchronous tiling on, the command prepares the
    /// data before later commands run.
    ///
    /// Completes with [`ErrorKind::NotFound`](mln_util::ErrorKind) when the
    /// style has no such source, with `InvalidArgument` when it isn't a
    /// GeoJSON source, when `json` isn't GeoJSON, or when the source clusters
    /// and the data isn't a feature collection of points, and with
    /// `Cancelled` when newer data replaces it while it waits, or when the
    /// source's URL, its removal, a style load or the map's release replaces
    /// the source's data before it is ready.
    fn set_geojson_source_data(
        &self,
        source_id: impl Into<String>,
        json: impl Into<String>,
    ) -> Result<Completion<Applied>, Error>;

    /// Sets whether a GeoJSON source builds its tiles while the map renders,
    /// in place of the `synchronousUpdate` of the source's JSON.
    fn set_geojson_source_synchronous_tiling(
        &self,
        source_id: impl Into<String>,
        enabled: bool,
    ) -> Result<Completion<Applied>, Error>;

    /// The settings of a GeoJSON source. Completes with
    /// [`ErrorKind::NotFound`](mln_util::ErrorKind) when the style has no
    /// such source, and with `InvalidArgument` when it isn't a GeoJSON
    /// source.
    fn geojson_source_settings(&self, source_id: impl Into<String>)
    -> Result<Completion<GeoJsonSourceSettings>, Error>;
}

impl GeoJsonSourceCommands for Map {
    fn add_geojson_source(
        &self,
        source_id: impl Into<String>,
        settings: GeoJsonSourceSettings,
    ) -> Result<Completion<Applied>, Error> {
        settings.check()?;
        let source_id = source_id.into();
        self.add_style_source(move |map| {
            let options = settings.into_core(map.style().registry().functions())?;
            Ok(geojson_source(&source_id, Arc::new(options)))
        })
    }

    fn set_geojson_source_url(
        &self,
        source_id: impl Into<String>,
        url: impl Into<String>,
    ) -> Result<Completion<Applied>, Error> {
        self.apply_style_edit(GeoJsonSourceType::set_url(source_id, url))
    }

    fn set_geojson_source_data(
        &self,
        source_id: impl Into<String>,
        json: impl Into<String>,
    ) -> Result<Completion<Applied>, Error> {
        let json = json.into();
        self.set_style_source_data(source_id, move |source| {
            let data = geojson_data(source)?;
            let geojson = parse_geojson(&json)?;
            if data.options.options.cluster {
                check_clusterable(&geojson)?;
            }
            Ok(NewGeoJson(geojson))
        })
    }

    fn set_geojson_source_synchronous_tiling(
        &self,
        source_id: impl Into<String>,
        enabled: bool,
    ) -> Result<Completion<Applied>, Error> {
        self.apply_style_edit(GeoJsonSourceType::set_synchronous(source_id, enabled))
    }

    fn geojson_source_settings(
        &self,
        source_id: impl Into<String>,
    ) -> Result<Completion<GeoJsonSourceSettings>, Error> {
        self.query_style_source(source_id, |source| {
            Ok(GeoJsonSourceSettings::from_core(&geojson_data(source)?.options.options))
        })
    }
}

/// A render session's queries of the clusters of a clustered GeoJSON source.
///
/// A query reads the source as the session's most recent render attempt had
/// it. A cluster ID belongs to one version of the source's data, so query
/// the features again after the data changes.
pub trait ClusterQueries {
    /// The clusters and points that a cluster splits into at the next zoom
    /// level. `cluster_id` is a cluster feature's `cluster_id` property.
    ///
    /// Completes with [`ErrorKind::NotFound`](mln_util::ErrorKind) when that
    /// render had no source `source_id`, or the source has no cluster
    /// `cluster_id`, and with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind)
    /// when the source isn't a clustered GeoJSON source.
    fn cluster_children(&self, source_id: impl Into<String>, cluster_id: u32) -> Result<Completion<Features>, Error>;

    /// The points in a cluster: it skips the first `offset`, then holds up
    /// to `limit`. It fails as [`cluster_children`](Self::cluster_children)
    /// does.
    fn cluster_leaves(
        &self,
        source_id: impl Into<String>,
        cluster_id: u32,
        limit: u32,
        offset: u32,
    ) -> Result<Completion<Features>, Error>;

    /// The zoom level at which a cluster splits into two or more clusters or
    /// points. It fails as [`cluster_children`](Self::cluster_children)
    /// does.
    fn cluster_expansion_zoom(&self, source_id: impl Into<String>, cluster_id: u32) -> Result<Completion<u8>, Error>;
}

impl ClusterQueries for RenderSession {
    fn cluster_children(&self, source_id: impl Into<String>, cluster_id: u32) -> Result<Completion<Features>, Error> {
        self.query_rendered_source(source_id, move |source| cluster(source, cluster_id, |_, children| Ok(children)))
    }

    fn cluster_leaves(
        &self,
        source_id: impl Into<String>,
        cluster_id: u32,
        limit: u32,
        offset: u32,
    ) -> Result<Completion<Features>, Error> {
        self.query_rendered_source(source_id, move |source| {
            cluster(source, cluster_id, |index, _| index.get_leaves(cluster_id, limit, offset))
        })
    }

    fn cluster_expansion_zoom(&self, source_id: impl Into<String>, cluster_id: u32) -> Result<Completion<u8>, Error> {
        self.query_rendered_source(source_id, move |source| {
            cluster(source, cluster_id, |index, _| index.get_cluster_expansion_zoom(cluster_id))
        })
    }
}

/// Runs `read` on the data of `source`, a clustered GeoJSON source, with the
/// children of its cluster `cluster_id`.
fn cluster<T>(
    source: &Source,
    cluster_id: u32,
    read: impl FnOnce(&dyn GeoJSONData, Features) -> Result<T, Error>,
) -> Result<T, Error> {
    // A cluster's ID holds its zoom plus one in its low five bits, so an ID
    // without them names no cluster, though it would find the points at
    // zoom 0 as children. An index of no points has no clusters.
    const ZOOM_BITS: u32 = 0x1F;

    let source_id = source.id();
    let geojson = SourceData::<GeoJsonSourceType>::recover(source)
        .ok_or_else(|| Error::invalid_argument(format!("source {source_id} is not a GeoJSON source")))?;
    if !geojson.options.options.cluster {
        return Err(Error::invalid_argument(format!("GeoJSON source {source_id} does not cluster")));
    }
    let data = geojson.description.as_ref();
    let children = data
        .filter(|_| cluster_id & ZOOM_BITS != 0)
        .and_then(|data| data.get_children(cluster_id).ok())
        .filter(|children| !children.is_empty());
    match (data, children) {
        (Some(data), Some(children)) => read(&**data, children),
        _ => Err(Error::not_found(format!("the source has no cluster with ID {cluster_id}"))),
    }
}
