// Ports src/mln/style/sources/geojson_source_impl.hpp and
// src/mln/style/sources/geojson_source_impl.cpp (GeoJSONSource::Impl,
// GeoJSONVTData, SuperclusterData and GeoJSONData::create).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use feature::{Feature, PropertyMap, Value};
use geojson::GeoJSON;
use geometry::Geometry;
use mln_actor::scheduler::{self, SchedulerPtr};
use mln_expression::expression::evaluate_accumulated;
use mln_style_types::SourceType;
use mln_tile_id::CanonicalTileID;
use mln_util::constants;
use mln_util::exception::RuntimeError;
use mln_util::identity::SimpleIdentity;
use mln_util::immutable::Immutable;
use mln_util::logging::{Event, Log};
use mln_util::range::Range;
use mln_util::sync::MutexExt;

use super::geojson_source::{Features, GeoJSONData, GeoJSONOptions, TileFeatures};
use crate::expression::{Expression, from_expression_value};
use crate::source_impl::{self, ImplBase};

/// `geoJSONVT_impl->getTile(z, x, y).features`. C++ `getTile` throws
/// `std::runtime_error` for a zoom above maxZoom or a missing parent tile;
/// the port logs it and hands out an empty tile.
fn geojsonvt_tile_features(vt: &Mutex<geojson_vt::GeoJSONVT>, id: &CanonicalTileID) -> TileFeatures {
    match vt.lock_unpoisoned().get_tile(id.z, id.x, id.y) {
        Ok(tile) => tile.features.clone(),
        Err(e) => {
            // TODO(port): C++ lets the exception propagate.
            Log::error(Event::General, e);
            TileFeatures::new()
        }
    }
}

/// `GeoJSONVTData`.
struct GeoJSONVTData {
    /// Accessed on worker thread.
    impl_: Arc<Mutex<geojson_vt::GeoJSONVT>>,
    sequenced_scheduler: SchedulerPtr,
}

impl std::fmt::Debug for GeoJSONVTData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GeoJSONVTData")
    }
}

impl GeoJSONData for GeoJSONVTData {
    fn get_tile(&self, id: &CanonicalTileID, callback: Box<dyn FnOnce(TileFeatures)>, run_synchronously: bool) {
        if run_synchronously {
            callback(geojsonvt_tile_features(&self.impl_, id));
        } else {
            let id = *id;
            let geojsonvt_impl = self.impl_.clone();
            scheduler::schedule_and_reply_value(
                &*self.sequenced_scheduler,
                SimpleIdentity::EMPTY,
                move || -> TileFeatures { geojsonvt_tile_features(&geojsonvt_impl, &id) },
                callback,
            );
        }
    }

    fn get_children(&self, _cluster_id: u32) -> Result<Features, RuntimeError> {
        Ok(Features::new())
    }

    fn get_leaves(&self, _cluster_id: u32, _limit: u32, _offset: u32) -> Result<Features, RuntimeError> {
        Ok(Features::new())
    }

    fn get_cluster_expansion_zoom(&self, _cluster_id: u32) -> Result<u8, RuntimeError> {
        Ok(0)
    }
}

/// `SuperclusterData`.
struct SuperclusterData {
    impl_: supercluster::Supercluster,
}

impl std::fmt::Debug for SuperclusterData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SuperclusterData")
    }
}

impl GeoJSONData for SuperclusterData {
    fn get_tile(&self, id: &CanonicalTileID, callback: Box<dyn FnOnce(TileFeatures)>, _run_synchronously: bool) {
        callback(self.impl_.get_tile(id.z, id.x, id.y));
    }

    fn get_children(&self, cluster_id: u32) -> Result<Features, RuntimeError> {
        self.impl_.get_children(cluster_id).map_err(RuntimeError)
    }

    fn get_leaves(&self, cluster_id: u32, limit: u32, offset: u32) -> Result<Features, RuntimeError> {
        self.impl_.get_leaves(cluster_id, limit, offset).map_err(RuntimeError)
    }

    fn get_cluster_expansion_zoom(&self, cluster_id: u32) -> Result<u8, RuntimeError> {
        self.impl_.get_cluster_expansion_zoom(cluster_id).map_err(RuntimeError)
    }
}

/// `evaluateFeature<Value>(f, expression, accumulated)`.
fn evaluate_feature(f: &Feature<f64>, expression: &Arc<dyn Expression>, accumulated: Option<&Value>) -> Value {
    if let Ok(result) = evaluate_accumulated(expression.as_ref(), accumulated, f) {
        if let Some(typed) = from_expression_value::<Value>(&result) {
            return typed;
        }
    }
    Value::default()
}

/// `GeoJSONData::create(geoJSON, sequencedScheduler, options)`.
pub(super) fn create(
    geojson: &GeoJSON,
    sequenced_scheduler: SchedulerPtr,
    options: &Immutable<GeoJSONOptions>,
) -> Result<Arc<dyn GeoJSONData>, String> {
    let scale: f64 = f64::from(constants::EXTENT) / constants::TILE_SIZE_D;
    if options.cluster {
        if let GeoJSON::FeatureCollection(features) = geojson {
            if !features.is_empty() {
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
                let map_options = options.clone();
                cluster_options.map = Arc::new(move |properties: &PropertyMap| -> PropertyMap {
                    let mut ret = PropertyMap::new();
                    if properties.is_empty() {
                        return ret;
                    }
                    for (key, (map_expression, _)) in &map_options.cluster_properties {
                        let mut feature = Feature::new(Geometry::Empty);
                        feature.properties = properties.clone();
                        ret.insert(key.clone(), evaluate_feature(&feature, map_expression, None));
                    }
                    ret
                });
                let reduce_options = options.clone();
                cluster_options.reduce = Some(Arc::new(move |to_return: &mut PropertyMap, to_fill: &PropertyMap| {
                    for (key, (_, reduce_expression)) in &reduce_options.cluster_properties {
                        if !to_fill.contains_key(key) {
                            continue;
                        }
                        let mut feature = Feature::new(Geometry::Empty);
                        feature.properties = to_fill.clone();
                        // `toReturn[p.first]` default-constructs a missing entry.
                        let accumulated = to_return.entry(key.clone()).or_default().clone();
                        let value = evaluate_feature(&feature, reduce_expression, Some(&accumulated));
                        to_return.insert(key.clone(), value);
                    }
                }));
                let impl_ = supercluster::Supercluster::new(features, cluster_options)?;
                return Ok(Arc::new(SuperclusterData { impl_ }));
            }
        }
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
    Ok(Arc::new(GeoJSONVTData {
        impl_: Arc::new(Mutex::new(geojson_vt::GeoJSONVT::from_geojson(geojson, vt_options))),
        sequenced_scheduler,
    }))
}

/// `GeoJSONSource::Impl`.
#[derive(Debug)]
pub struct Impl {
    base: ImplBase,
    options: Immutable<GeoJSONOptions>,
    data: Option<Arc<dyn GeoJSONData>>,
    /// `mutable bool overrideSynchronousUpdate`.
    override_synchronous_update: AtomicBool,
}

impl Clone for Impl {
    /// The C++ copy constructor (`Impl(const Impl&) = default` via
    /// `createMutable`).
    fn clone(&self) -> Self {
        Impl {
            base: self.base.clone(),
            options: self.options.clone(),
            data: self.data.clone(),
            override_synchronous_update: AtomicBool::new(self.override_synchronous_update.load(Ordering::Relaxed)),
        }
    }
}

impl Impl {
    /// `Impl(std::string id, Immutable<GeoJSONOptions>)`.
    pub fn new(id: String, options: Immutable<GeoJSONOptions>) -> Self {
        Impl {
            base: ImplBase::new(SourceType::GeoJSON, id),
            options,
            data: None,
            override_synchronous_update: AtomicBool::new(false),
        }
    }

    /// `Impl(const GeoJSONSource::Impl&, std::shared_ptr<GeoJSONData>)`.
    pub fn with_data(other: &Impl, data: Option<Arc<dyn GeoJSONData>>) -> Self {
        Impl {
            base: other.base.clone(),
            options: other.options.clone(),
            data,
            override_synchronous_update: AtomicBool::new(other.override_synchronous_update.load(Ordering::Relaxed)),
        }
    }

    pub fn get_zoom_range(&self) -> Range<u8> {
        Range::new(self.options.minzoom, self.options.maxzoom)
    }

    /// `std::weak_ptr<GeoJSONData> getData()` (`None` = an empty weak_ptr).
    pub fn get_data(&self) -> Option<Weak<dyn GeoJSONData>> {
        self.data.as_ref().map(Arc::downgrade)
    }

    pub fn get_options(&self) -> &Immutable<GeoJSONOptions> {
        &self.options
    }

    pub fn set_override_synchronous_update(&self, new_override: bool) {
        self.override_synchronous_update.store(new_override, Ordering::Relaxed);
    }
}

impl source_impl::Impl for Impl {
    fn base(&self) -> &ImplBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut ImplBase {
        &mut self.base
    }

    fn get_attribution(&self) -> Option<String> {
        None
    }

    fn is_update_synchronous(&self) -> bool {
        self.options.synchronous_update || self.override_synchronous_update.load(Ordering::Relaxed)
    }
}
