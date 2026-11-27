//! The map handle's style commands: sources and their data, values held
//! for sources, images, layers, the light, the global state and
//! transitions.

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use geojson::{GeoJson, GeometryValue};
use mln_builtins::sources::geojson::{GeoJSONData, create_geojson_data, set_data};
use mln_builtins::sources::image::set_image;
use mln_style::bitmap::PremultipliedImage;
use mln_style::error::Error;
use mln_style::expression::Functions;
use mln_style::geo::{LatLng, LatLngBounds};
use mln_style::hash::HashMap;
use mln_style::image::Impl as StyleImage;
use mln_style::range::Range;
use mln_style::source::SourceData;
use mln_style::sources::image::ImageSourceOptions;
use mln_style::sources::{GeoJSONOptions, GeoJsonSourceType, ImageSourceType, geojson_source};
use mln_style::tileset::{RasterEncoding, Scheme, VectorEncoding};
use mln_style::{Edit, Source, SourceInstance, TransitionOptions};

use crate::completion::{Completion, Resolver};
use crate::map::{Applied, Map, MapEntry, MapId, apply_command, flush, missing_map, style_not_loaded};
use crate::runtime::{Background, Outbox, RuntimeCore, post};

/// The tiles that a source lists in the style itself, rather than at a URL.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceTiles {
    pub urls: Vec<String>,
    pub zoom_range: Range<u8>,
    pub scheme: Scheme,
    pub bounds: Option<LatLngBounds>,
    pub vector_encoding: Option<VectorEncoding>,
    pub raster_encoding: Option<RasterEncoding>,
}

/// The options of a GeoJSON source, which stay fixed for as long as the
/// style holds it. The defaults are the style specification's.
#[derive(Clone, Debug, PartialEq)]
pub struct GeoJsonSourceOptions {
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

impl Default for GeoJsonSourceOptions {
    fn default() -> Self {
        GeoJsonSourceOptions::from_core(&GeoJSONOptions::default())
    }
}

impl GeoJsonSourceOptions {
    fn from_core(options: &GeoJSONOptions) -> Self {
        GeoJsonSourceOptions {
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

    /// Fails with [`ErrorKind::InvalidArgument`](mln_style::error::ErrorKind) for an
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
    /// [`Map::add_geojson_source`] otherwise parses on the runtime's thread.
    /// Fails with [`ErrorKind::InvalidArgument`](mln_style::error::ErrorKind) for
    /// text that isn't a `clusterProperties` object.
    pub fn check_cluster_properties(&self, functions: &Arc<Functions>) -> Result<(), Error> {
        self.cluster_properties.as_deref().map_or(Ok(()), |json| parse_cluster_properties(json, functions).map(drop))
    }

    /// The core's options, with the cluster properties parsed against
    /// `functions`.
    fn into_core(self, functions: &Arc<Functions>) -> Result<GeoJSONOptions, Error> {
        let cluster_properties = match &self.cluster_properties {
            Some(json) => parse_cluster_properties(json, functions)?,
            None => mln_style::sources::geojson::ClusterProperties::default(),
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
/// [`ErrorKind::InvalidArgument`](mln_style::error::ErrorKind) for text that isn't
/// such an object.
fn parse_cluster_properties(
    json: &str,
    functions: &Arc<Functions>,
) -> Result<mln_style::sources::geojson::ClusterProperties, Error> {
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|error| Error::invalid_argument(format!("cluster_properties is not valid JSON: {error}")))?;
    if !value.is_object() {
        return Err(Error::invalid_argument("cluster_properties must contain a JSON object"));
    }
    let wrapper = serde_json::json!({ "clusterProperties": value });
    let mut error = mln_style::conversion::Error::default();
    let options = mln_style::conversion::convert_geojson_options(
        &mln_style::conversion::Convertible::from(&wrapper),
        functions,
        &mut error,
    )
    .ok_or_else(|| Error::invalid_argument(format!("GeoJSON source options: {}", error.message)))?;
    Ok(options.cluster_properties)
}

/// A source of the map's style.
#[derive(Clone, Debug)]
pub struct SourceInfo {
    pub id: String,
    /// The source's type, such as `geojson`: a type of the style
    /// specification's, or one that the runtime's registry adds.
    pub source_type: &'static str,
    pub is_volatile: bool,
    /// The source's attribution. A tile source with a URL has none until its
    /// TileJSON loads.
    pub attribution: Option<String>,
    pub url: Option<String>,
    /// The tiles a tile source lists in the style itself.
    pub tiles: Option<SourceTiles>,
    /// A tile source's tile size.
    pub tile_size: Option<u16>,
    /// A vector source's tile format.
    pub vector_encoding: Option<VectorEncoding>,
    pub geojson: Option<GeoJsonSourceOptions>,
}

/// A layer of the map's style.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerInfo {
    pub id: String,
    /// The layer's [type](https://maplibre.org/maplibre-style-spec/layers/#type),
    /// such as `fill`.
    pub layer_type: &'static str,
    /// The source's ID, for a layer that takes one.
    pub source_id: Option<String>,
    /// The layer of a vector source that the layer draws.
    pub source_layer: Option<String>,
    pub min_zoom: Option<f64>,
    pub max_zoom: Option<f64>,
    pub visible: bool,
}

/// A value that the map holds for a source that
/// [`Map::add_source_holding`] added, while that source is in the style.
struct HeldSource {
    id: String,
    instance: SourceInstance,
    held: Box<dyn Any + Send>,
}

/// The values that a map holds for its sources, which it drops once their
/// source leaves the style.
#[derive(Default)]
pub(crate) struct HeldSources(Vec<HeldSource>);

/// Why [`Map::with_held_source`] found no held value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeldSourceError {
    /// The style has no source with the ID.
    NoSource,
    /// The source holds no value of the type asked for.
    NotHeld,
}

impl HeldSourceError {
    /// The error for the source `id`: `NotFound` for
    /// [`NoSource`](Self::NoSource) and `InvalidArgument` for
    /// [`NotHeld`](Self::NotHeld).
    #[must_use]
    pub fn into_error(self, id: &str) -> Error {
        match self {
            HeldSourceError::NoSource => source_not_found(id),
            HeldSourceError::NotHeld => Error::invalid_argument(format!("source {} holds no such value", quote(id))),
        }
    }
}

/// GeoJSON data for one source: the data being built, and the newest data
/// that waits to follow it.
struct GeoJsonUpdate {
    /// The source the data is for; a source that replaced it under its id
    /// doesn't take it.
    instance: SourceInstance,
    /// The build in progress, by its number, and its completion, which
    /// resolves when the data is installed or when something replaced the
    /// source's data first.
    building: Option<(u64, Resolver<Applied>)>,
    waiting: Option<(GeoJson, Resolver<Applied>)>,
}

/// The GeoJSON sources whose new data is still being built, by source ID.
#[derive(Default)]
pub(crate) struct GeoJsonUpdates(HashMap<String, GeoJsonUpdate>);

/// Lets go of the values held for the sources that left the style, and
/// cancels the GeoJSON data that waits for sources that left. The values
/// drop from the outbox, so before the completions that follow, and with
/// the runtime's state unborrowed.
pub(crate) fn reconcile(entry: &mut MapEntry, outbox: &mut Outbox) {
    let map = &entry.core;
    let present = |id: &str, instance: SourceInstance| map.source(id).is_some_and(|found| found.instance() == instance);
    let (kept, gone): (Vec<_>, Vec<_>) =
        std::mem::take(&mut entry.held_sources.0).into_iter().partition(|held| present(&held.id, held.instance));
    entry.held_sources.0 = kept;
    if !gone.is_empty() {
        outbox.push(move || drop(gone));
    }
    entry.geojson_updates.0.retain(|id, update| {
        if present(id, update.instance) {
            return true;
        }
        let cancelled = || Err(Error::cancelled("the source left the style before its data was ready"));
        if let Some((_, resolver)) = update.building.take() {
            outbox.resolve(resolver, cancelled());
        }
        if let Some((_, resolver)) = update.waiting.take() {
            outbox.resolve(resolver, cancelled());
        }
        false
    });
}

/// Lets go of everything a released map's sources hold, before its release
/// completes.
pub(crate) fn release(entry: &mut MapEntry, outbox: &mut Outbox) {
    let held = std::mem::take(&mut entry.held_sources.0);
    if !held.is_empty() {
        outbox.push(move || drop(held));
    }
    for (_, mut update) in entry.geojson_updates.0.drain() {
        let cancelled = || Err(Error::cancelled("the map was released before the data was ready"));
        if let Some((_, resolver)) = update.building.take() {
            outbox.resolve(resolver, cancelled());
        }
        if let Some((_, resolver)) = update.waiting.take() {
            outbox.resolve(resolver, cancelled());
        }
    }
}

fn quote(id: &str) -> String {
    format!("\"{id}\"")
}

fn require_loaded(map: &mln_engine::map::Map) -> Result<(), Error> {
    if map.style_document_loaded() { Ok(()) } else { Err(style_not_loaded()) }
}

fn source_not_found(id: &str) -> Error {
    Error::not_found(format!("source {} does not exist", quote(id)))
}

fn find_source<'a>(map: &'a mln_engine::map::Map, id: &str) -> Result<&'a Source, Error> {
    map.source(id).ok_or_else(|| source_not_found(id))
}

/// The error for the source `id`, which isn't `kind`, as in `source "water"
/// is not a GeoJSON source`.
fn not_a(id: &str, kind: &str) -> Error {
    Error::invalid_argument(format!("source {} is not {kind}", quote(id)))
}

/// The data of the source `id`, a GeoJSON source.
fn geojson_data<'a>(map: &'a mln_engine::map::Map, id: &str) -> Result<&'a SourceData<GeoJsonSourceType>, Error> {
    SourceData::recover(find_source(map, id)?).ok_or_else(|| not_a(id, "a GeoJSON source"))
}

/// The data of the source `id`, an image source.
fn image_data<'a>(map: &'a mln_engine::map::Map, id: &str) -> Result<&'a SourceData<ImageSourceType>, Error> {
    SourceData::recover(find_source(map, id)?).ok_or_else(|| not_a(id, "an image source"))
}

fn require_new_source(map: &mln_engine::map::Map, id: &str) -> Result<(), Error> {
    require_loaded(map)?;
    if map.source(id).is_some() {
        return Err(Error::already_exists(format!("source {} already exists", quote(id))));
    }
    Ok(())
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
            let pair = mln_style::feature::Value::Array(vec![reduce.serialize(), map.serialize()]);
            (key.clone(), mln_style::conversion::value_to_json(&pair))
        })
        .collect::<serde_json::Map<_, _>>();
    Some(serde_json::Value::Object(members).to_string())
}

/// What the API reports about `source`.
fn source_info(registry: &mln_engine::Registry, source: &Source) -> SourceInfo {
    let summary = registry.source_summary(source);
    SourceInfo {
        id: source.id().to_owned(),
        source_type: source.info().name,
        is_volatile: source.common().volatile,
        attribution: registry.source_attribution(source).filter(|attribution| !attribution.is_empty()),
        url: summary.url,
        tiles: summary.inline_tileset.map(|tileset| SourceTiles {
            urls: tileset.tiles,
            zoom_range: tileset.zoom_range,
            scheme: tileset.scheme,
            bounds: tileset.bounds,
            vector_encoding: tileset.vector_encoding,
            raster_encoding: tileset.raster_encoding,
        }),
        tile_size: summary.tile_size,
        vector_encoding: summary.vector_encoding,
        geojson: summary.geojson.map(|options| GeoJsonSourceOptions::from_core(&options)),
    }
}

fn layer_info(layer: &mln_style::Layer) -> LayerInfo {
    LayerInfo {
        id: layer.get_id().to_owned(),
        layer_type: layer.info().name,
        source_id: Some(layer.get_source_id()).filter(|id| !id.is_empty()).map(str::to_owned),
        source_layer: Some(layer.get_source_layer()).filter(|id| !id.is_empty()).map(str::to_owned),
        min_zoom: Some(f64::from(layer.get_min_zoom())).filter(|zoom| zoom.is_finite()),
        max_zoom: Some(f64::from(layer.get_max_zoom())).filter(|zoom| zoom.is_finite()),
        visible: layer.get_visibility() != mln_style::types::VisibilityType::None,
    }
}

/// Parses GeoJSON text, on the runtime's thread.
pub(crate) fn parse_geojson(json: &str) -> Result<GeoJson, Error> {
    serde_json::from_str(json).map_err(|error| {
        let what = if error.is_data() { "invalid" } else { "not valid JSON" };
        Error::invalid_argument(format!("GeoJSON data is {what}: {error}"))
    })
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

impl Map {
    // MARK: - Global state

    /// Sets the value `name` that the style's `global-state` expressions
    /// read, from JSON. See `Style::set_global_state_property_json`.
    pub fn set_global_state_property(
        &self,
        name: impl Into<String>,
        json: impl Into<String>,
    ) -> Result<Completion<Applied>, Error> {
        let (name, json) = (name.into(), json.into());
        self.command(move |map| map.edit_loaded_style(|style| style.set_global_state_property_json(&name, &json)))
    }

    /// The style's global state as a JSON object.
    pub fn global_state(&self) -> Result<Completion<String>, Error> {
        self.query(|map| Ok(map.style().global_state_json()))
    }

    // MARK: - Sources

    /// A source of the style, or `None` when the style has no such source.
    pub fn style_source(&self, source_id: impl Into<String>) -> Result<Completion<Option<SourceInfo>>, Error> {
        let source_id = source_id.into();
        self.query(move |map| Ok(map.source(&source_id).map(|source| source_info(map.registry(), source))))
    }

    /// The style's sources, in style order.
    pub fn style_sources(&self) -> Result<Completion<Vec<SourceInfo>>, Error> {
        self.query(|map| Ok(map.sources().map(|source| source_info(map.registry(), source)).collect()))
    }

    /// Sets whether a source keeps its tiles out of the persistent cache.
    pub fn set_style_source_volatile(
        &self,
        source_id: impl Into<String>,
        is_volatile: bool,
    ) -> Result<Completion<Applied>, Error> {
        let source_id = source_id.into();
        self.command(move |map| {
            require_loaded(map)?;
            let mut common = *find_source(map, &source_id)?.common();
            common.volatile = is_volatile;
            map.edit_style(|style| style.apply(Edit::SetSourceCommon { id: source_id, common }))
        })
    }

    // MARK: - GeoJSON sources

    /// Adds a GeoJSON source with no data and `options`, which stay fixed
    /// for as long as the style holds it.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_style::error::ErrorKind) for an
    /// option out of range, and completes with it for cluster properties
    /// that don't parse.
    pub fn add_geojson_source(
        &self,
        source_id: impl Into<String>,
        options: GeoJsonSourceOptions,
    ) -> Result<Completion<Applied>, Error> {
        options.check()?;
        let source_id = source_id.into();
        self.command(move |map| {
            let functions = Arc::clone(map.style().registry().functions());
            let options = options.into_core(&functions)?;
            require_new_source(map, &source_id)?;
            map.add_source(geojson_source(&source_id, Arc::new(options)))
        })
    }

    /// Makes a GeoJSON source load its data from `url`. Data that a
    /// [`set_geojson_source_data`](Self::set_geojson_source_data) sent before
    /// and that isn't ready yet is cancelled.
    pub fn set_geojson_source_url(
        &self,
        source_id: impl Into<String>,
        url: impl Into<String>,
    ) -> Result<Completion<Applied>, Error> {
        let (source_id, url) = (source_id.into(), url.into());
        self.send(
            move |entry, outbox| {
                require_loaded(&entry.core)?;
                let edit = GeoJsonSourceType::set_url(source_id.clone(), url);
                entry.core.edit_style(|style| style.apply(edit))?;
                if let Some((_, resolver)) =
                    entry.geojson_updates.0.get_mut(&source_id).and_then(|update| update.waiting.take())
                {
                    outbox.resolve(resolver, Err(Error::cancelled("the source's URL replaced its data")));
                }
                Ok(())
            },
            |(), entry| entry.applied(),
        )
    }

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
    /// Completes with [`ErrorKind::NotFound`](mln_style::error::ErrorKind) when the
    /// style has no such source, with `InvalidArgument` when it isn't a
    /// GeoJSON source, when `json` isn't GeoJSON, or when the source clusters
    /// and the data isn't a feature collection of points, and with
    /// `Cancelled` when newer data replaces it while it waits, or when the
    /// source's URL, its removal, a style load or the map's release replaces
    /// the source's data before it is ready.
    pub fn set_geojson_source_data(
        &self,
        source_id: impl Into<String>,
        json: impl Into<String>,
    ) -> Result<Completion<Applied>, Error> {
        let (source_id, json) = (source_id.into(), json.into());
        self.post(move |core, outbox, id, sequence, resolver| {
            if !core.maps.contains_key(&id) {
                return outbox.resolve(resolver, Err(missing_map()));
            }
            let background = core.background();
            apply_command(
                core,
                id,
                sequence,
                outbox,
                move |entry, outbox| match queue_geojson(entry, id, &source_id, &json, resolver, background, outbox) {
                    Ok(installed) => installed,
                    Err((error, resolver)) => {
                        outbox.resolve(resolver, Err(error));
                        None
                    }
                },
                |_, installed, applied, outbox| {
                    if let Some(resolver) = installed {
                        outbox.resolve(resolver, Ok(applied));
                    }
                },
            );
        })
    }

    /// Sets whether a GeoJSON source builds its tiles while the map renders,
    /// in place of the `synchronousUpdate` of the source's JSON.
    pub fn set_geojson_source_synchronous_tiling(
        &self,
        source_id: impl Into<String>,
        enabled: bool,
    ) -> Result<Completion<Applied>, Error> {
        let source_id = source_id.into();
        self.command(move |map| {
            require_loaded(map)?;
            map.edit_style(|style| style.apply(GeoJsonSourceType::set_synchronous(source_id, enabled)))
        })
    }

    // MARK: - Held values

    /// Adds `source` to the style, with `held`, a value that the map holds
    /// for that source while it is in the style, which
    /// [`with_held_source`](Self::with_held_source) reaches. The map drops
    /// it once the source leaves the style, by its removal, a style load or
    /// the map's release, before the command that removed it completes. A
    /// command that fails drops it before it completes.
    ///
    /// Completes with [`ErrorKind::AlreadyExists`](mln_style::error::ErrorKind)
    /// when the style has a source with the ID, and as
    /// [`add_style_source_json`](Self::add_style_source_json) otherwise.
    pub fn add_source_holding(&self, source: Source, held: Box<dyn Any + Send>) -> Result<Completion<Applied>, Error> {
        self.send(
            move |entry, outbox| {
                let map = &mut entry.core;
                let (id, instance) = (source.id().to_owned(), source.instance());
                match require_new_source(map, &id).and_then(|()| map.add_source(source)) {
                    Ok(()) => {
                        entry.held_sources.0.push(HeldSource { id, instance, held });
                        Ok(())
                    }
                    Err(error) => {
                        outbox.push(move || drop(held));
                        Err(error)
                    }
                }
            },
            |(), entry| entry.applied(),
        )
    }

    /// A command that runs `apply` on the runtime's thread with the value of
    /// type `T` that the map holds for the source `source_id`, or why it
    /// holds none, and completes with what `apply` returns.
    pub fn with_held_source<T: Any + Send>(
        &self,
        source_id: impl Into<String>,
        apply: impl FnOnce(Result<&mut T, HeldSourceError>) -> Result<(), Error> + Send + 'static,
    ) -> Result<Completion<Applied>, Error> {
        let source_id = source_id.into();
        self.send(
            move |entry, _| {
                let held = match entry.core.source(&source_id) {
                    None => Err(HeldSourceError::NoSource),
                    Some(source) => {
                        let instance = source.instance();
                        entry
                            .held_sources
                            .0
                            .iter_mut()
                            .find(|held| held.id == source_id && held.instance == instance)
                            .and_then(|held| held.held.downcast_mut::<T>())
                            .ok_or(HeldSourceError::NotHeld)
                    }
                };
                apply(held)
            },
            |(), entry| entry.applied(),
        )
    }

    // MARK: - Images

    /// Adds `image` to the style, or replaces the image with its ID.
    pub fn set_style_image(&self, image: StyleImage) -> Result<Completion<Applied>, Error> {
        self.command(move |map| map.edit_loaded_style(|style| style.apply(Edit::AddImage(Arc::new(image)))))
    }

    /// Removes an image from the style. Completes with
    /// [`ErrorKind::NotFound`](mln_style::error::ErrorKind) when the style has no
    /// such image.
    pub fn remove_style_image(&self, image_id: impl Into<String>) -> Result<Completion<Applied>, Error> {
        let image_id = image_id.into();
        self.command(move |map| {
            map.edit_loaded_style(|style| {
                if style.get_image(&image_id).is_none() {
                    return Err(Error::not_found(format!("style image {} does not exist", quote(&image_id))));
                }
                style.apply(Edit::RemoveImage(image_id))
            })
        })
    }

    /// An image of the style, or `None` when the style has no such image.
    pub fn style_image(&self, image_id: impl Into<String>) -> Result<Completion<Option<Arc<StyleImage>>>, Error> {
        let image_id = image_id.into();
        self.query(move |map| Ok(map.style().get_image(&image_id).cloned()))
    }

    // MARK: - Image sources

    /// Adds an image source that shows `image` at `coordinates`, its four
    /// corners clockwise from the top left.
    pub fn add_image_source(
        &self,
        source_id: impl Into<String>,
        coordinates: [LatLng; 4],
        image: PremultipliedImage,
    ) -> Result<Completion<Applied>, Error> {
        let source_id = source_id.into();
        self.command(move |map| {
            require_new_source(map, &source_id)?;
            let options = ImageSourceOptions { url: None, coordinates };
            let data = SourceData::<ImageSourceType>::with_description(options, Arc::new(image));
            map.add_source(Source::new(&source_id, data))
        })
    }

    /// Makes an image source load its image from `url`.
    pub fn set_image_source_url(
        &self,
        source_id: impl Into<String>,
        url: impl Into<String>,
    ) -> Result<Completion<Applied>, Error> {
        let (source_id, url) = (source_id.into(), url.into());
        self.command(move |map| {
            require_loaded(map)?;
            map.edit_style(|style| style.apply(ImageSourceType::set_url(source_id, url)))
        })
    }

    /// Replaces the image of an image source.
    pub fn set_image_source_image(
        &self,
        source_id: impl Into<String>,
        image: PremultipliedImage,
    ) -> Result<Completion<Applied>, Error> {
        let source_id = source_id.into();
        self.command(move |map| {
            require_loaded(map)?;
            map.edit_style(|style| style.apply(set_image(source_id, image)))
        })
    }

    /// Moves the image of an image source to `coordinates`, its four corners
    /// clockwise from the top left.
    pub fn set_image_source_coordinates(
        &self,
        source_id: impl Into<String>,
        coordinates: [LatLng; 4],
    ) -> Result<Completion<Applied>, Error> {
        let source_id = source_id.into();
        self.command(move |map| {
            require_loaded(map)?;
            map.edit_style(|style| style.apply(ImageSourceType::set_coordinates(source_id, coordinates)))
        })
    }

    /// The corners of an image source's image, clockwise from the top left.
    pub fn image_source_coordinates(&self, source_id: impl Into<String>) -> Result<Completion<[LatLng; 4]>, Error> {
        let source_id = source_id.into();
        self.query(move |map| Ok(image_data(map, &source_id)?.options.coordinates))
    }

    // MARK: - Layers

    /// A layer of the style, or `None` when the style has no such layer.
    pub fn style_layer(&self, layer_id: impl Into<String>) -> Result<Completion<Option<LayerInfo>>, Error> {
        let layer_id = layer_id.into();
        self.query(move |map| Ok(map.style().get_layer(&layer_id).map(layer_info)))
    }

    /// The style's layers, from bottom to top.
    pub fn style_layers(&self) -> Result<Completion<Vec<LayerInfo>>, Error> {
        self.query(|map| Ok(map.style().get_layers().map(layer_info).collect()))
    }

    /// Moves a layer below `before_layer_id`, or above every layer. See
    /// `Style::move_layer` for the errors.
    pub fn move_style_layer(
        &self,
        layer_id: impl Into<String>,
        before_layer_id: Option<String>,
    ) -> Result<Completion<Applied>, Error> {
        let layer_id = layer_id.into();
        self.command(move |map| map.edit_loaded_style(|style| style.move_layer(&layer_id, before_layer_id.as_deref())))
    }

    // MARK: - Transitions

    /// Sets the default transition of the style's paint properties.
    pub fn set_style_transition_options(&self, options: TransitionOptions) -> Result<Completion<Applied>, Error> {
        self.command(move |map| map.edit_loaded_style(|style| style.apply(Edit::SetTransition(options))))
    }

    /// The default transition of the style's paint properties.
    pub fn style_transition_options(&self) -> Result<Completion<TransitionOptions>, Error> {
        self.query(|map| Ok(map.style().get_transition_options()))
    }
}

/// Takes new data for the GeoJSON source `source_id`: builds it now with
/// synchronous tiling on, and returns its resolver for the command to
/// resolve once it ran; otherwise starts building it, or leaves it waiting
/// behind the data being built. An error returns the resolver with it.
fn queue_geojson(
    entry: &mut MapEntry,
    map_id: MapId,
    source_id: &str,
    json: &str,
    resolver: Resolver<Applied>,
    background: Background,
    outbox: &mut Outbox,
) -> Result<Option<Resolver<Applied>>, (Error, Resolver<Applied>)> {
    let found = (|| {
        require_loaded(&entry.core)?;
        let data = geojson_data(&entry.core, source_id)?;
        let geojson = parse_geojson(json)?;
        if data.options.options.cluster {
            check_clusterable(&geojson)?;
        }
        let instance = find_source(&entry.core, source_id)?.instance();
        Ok((instance, data.load, Arc::clone(&data.options.options), data.options.synchronous_update, geojson))
    })();
    let (instance, load, options, synchronous, geojson) = match found {
        Ok(found) => found,
        Err(error) => return Err((error, resolver)),
    };
    if synchronous {
        return match create_geojson_data(geojson, &options) {
            Ok(data) => {
                let edit = set_data(source_id, Some(data));
                match entry.core.edit_style(|style| style.apply(edit)) {
                    Ok(()) => Ok(Some(resolver)),
                    Err(error) => Err((error, resolver)),
                }
            }
            Err(error) => Err((error, resolver)),
        };
    }
    let update = entry.geojson_updates.0.entry(source_id.to_owned()).or_insert_with(|| GeoJsonUpdate {
        instance,
        building: None,
        waiting: None,
    });
    if update.building.is_none() {
        update.instance = instance;
        start_geojson(update, map_id, source_id.to_owned(), (options, load), geojson, resolver, background);
    } else if let Some((_, replaced)) = update.waiting.replace((geojson, resolver)) {
        outbox.resolve(replaced, Err(Error::cancelled("newer data for the source replaced it")));
    }
    Ok(None)
}

/// Builds `geojson` with `options` for `update`'s source, whose data is at
/// `load`, on a background thread, then installs it from the runtime's
/// thread.
fn start_geojson(
    update: &mut GeoJsonUpdate,
    map_id: MapId,
    source_id: String,
    (options, load): (Arc<GeoJSONOptions>, u64),
    geojson: GeoJson,
    resolver: Resolver<Applied>,
    background: Background,
) {
    // Numbers each build, so a build that something replaced finds no
    // completion of its own.
    static BUILDS: AtomicU64 = AtomicU64::new(0);
    let build = BUILDS.fetch_add(1, Ordering::Relaxed);
    update.building = Some((build, resolver));
    let next = background.clone();
    background.pool.spawn(move || {
        let data = create_geojson_data(geojson, &options);
        post(&background.queue, move |core, outbox| {
            install_geojson(core, map_id, &source_id, (build, load), data, next, outbox);
        });
    });
}

/// The options and the data's load of the source `id`, if it's the GeoJSON
/// source `instance`.
fn current_geojson(
    map: &mln_engine::map::Map,
    id: &str,
    instance: SourceInstance,
) -> Option<(Arc<GeoJSONOptions>, u64)> {
    let source = map.source(id).filter(|source| source.instance() == instance)?;
    let data = SourceData::<GeoJsonSourceType>::recover(source)?;
    Some((Arc::clone(&data.options.options), data.load))
}

/// Installs data that a background thread built, unless something replaced
/// the source's data while it was built, then starts the data that waits.
fn install_geojson(
    core: &mut RuntimeCore,
    map_id: MapId,
    source_id: &str,
    (build, load): (u64, u64),
    data: Result<Arc<dyn GeoJSONData>, Error>,
    background: Background,
    outbox: &mut Outbox,
) {
    flush(core, map_id, outbox);
    let Some(entry) = core.maps.get_mut(&map_id) else { return };
    let Some(update) = entry.geojson_updates.0.get_mut(source_id) else { return };
    let Some((_, resolver)) = update.building.take_if(|(building, _)| *building == build) else { return };
    let current = current_geojson(&entry.core, source_id, update.instance);
    let installed = match data {
        Err(error) => Err(error),
        Ok(_) if current.as_ref().is_none_or(|(_, current_load)| *current_load != load) => {
            Err(Error::cancelled("the source's data was replaced before this data was ready"))
        }
        Ok(data) => {
            let edit = set_data(source_id, Some(data));
            entry.core.edit_style(|style| style.apply(edit))
        }
    };
    match (update.waiting.take(), current_geojson(&entry.core, source_id, update.instance)) {
        (Some((geojson, next)), Some(current)) => {
            start_geojson(update, map_id, source_id.to_owned(), current, geojson, next, background);
        }
        (Some((_, next)), None) => outbox.resolve(next, Err(Error::cancelled("the source left the style"))),
        (None, _) => {
            entry.geojson_updates.0.remove(source_id);
        }
    }
    flush(core, map_id, outbox);
    let Some(entry) = core.maps.get(&map_id) else { return };
    outbox.resolve(resolver, installed.map(|()| entry.applied()));
}
