//! The runner's maps: `mln_runtime` maps, each drawn by an offscreen render
//! session, which the operations drive through the runtime's public API.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use mln_engine::Registry;
use mln_engine::camera::{BoundOptions, CameraUpdate, CameraUpdateMode, ProjectionMode};
use mln_engine::exec::pool::Pool;
use mln_engine::storage::{FileSource, FileSourceExt, Request, Resource, Response};
use mln_runtime::{
    AttachOptions, BackendType, Completion, Device, DeviceOptions, Driver, DriverWork, Error, ErrorKind,
    FeatureStateSelector, GlApi, LogicalExtent, Map, MapOptions, RenderSession, RenderSessionState, RenderTarget,
    RenderedQueryGeometry, ResourceProvider, ResourceRequest, Runtime, RuntimeOptions, TileOptions, ViewportOptions,
};
use mln_style::bitmap::PremultipliedImage;
use mln_style::conversion::{Convertible, convert};
use mln_style::feature::Identifier;
use mln_style::geo::LatLng;

use crate::file_source::{Assets, proxy_loader};
use crate::metadata::{MapMode, TerrainSkirtLength, TestMetadata};
use crate::runner::{
    CameraOptions, ImplFactory, ImplOptions, MapError, QueryGeometry, ResourceOptions, TestRunnerImpl,
};
use crate::watchdog;

/// The device that the runner, the benchmarks and the tests of the crates
/// that run the render tests render on: the backend that
/// `MLN_RENDER_BACKEND` names, or WebGPU. `gl` names OpenGL ES, and
/// `gl-desktop` desktop OpenGL.
///
/// # Errors
///
/// For a name that `MLN_RENDER_BACKEND` doesn't know, and as
/// [`Device::with_backend`] fails.
pub fn harness_device() -> Result<Device, Error> {
    let mut options = DeviceOptions::default();
    let backend = match std::env::var("MLN_RENDER_BACKEND").map(|backend| backend.to_ascii_lowercase()).as_deref() {
        Ok("metal") => BackendType::Metal,
        Ok("vulkan") => BackendType::Vulkan,
        Ok("d3d12") => BackendType::D3D12,
        Ok("gl") => BackendType::OpenGL,
        Ok("gl-desktop") => {
            options.gl_api = GlApi::Gl;
            BackendType::OpenGL
        }
        Ok("webgpu") | Err(_) => BackendType::WebGPU,
        Ok(other) => return Err(Error::invalid_argument(format!("MLN_RENDER_BACKEND names no backend: {other}"))),
    };
    Device::with_backend(backend, options)
}

/// How long the drop of a map waits for its session, map and runtime to
/// let go of their graphics objects and threads.
const RELEASE_TIMEOUT: Duration = Duration::from_secs(10);

/// Waits for `completion`, no longer than the test's deadline
/// (watchdog.rs).
fn wait<T: Send + 'static>(completion: Result<Completion<T>, Error>) -> Result<T, Error> {
    let (sender, receiver) = mpsc::channel();
    completion?.on_complete(move |result| {
        let _ = sender.send(result);
    });
    watchdog::recv(&receiver).unwrap_or_else(|| Err(Error::cancelled("the completion was dropped")))
}

/// Waits for `completion` for up to [`RELEASE_TIMEOUT`], outside the test's
/// deadline.
fn wait_to_release<T: Send + 'static>(completion: Result<Completion<T>, Error>) {
    let Ok(completion) = completion else { return };
    let (sender, receiver) = mpsc::channel();
    completion.on_complete(move |result| {
        let _ = sender.send(result.map(drop));
    });
    let _ = receiver.recv_timeout(RELEASE_TIMEOUT);
}

/// A render session's graphics work, which runs on the runner's thread
/// ([`Driver::CallerThread`]) while it waits, as the maps of C++'s runner
/// render on its thread: no two maps' frames ever render at once.
struct Driven {
    work: DriverWork,
    /// Whether the session has work for this thread.
    woken: Arc<AtomicBool>,
    /// Wakes the thread when the session has work or a completion it waits
    /// for comes.
    signal: Arc<(parking_lot::Mutex<bool>, parking_lot::Condvar)>,
}

impl Driven {
    /// Runs the session's work, if it has any.
    fn service(&self) {
        if self.woken.swap(false, Ordering::SeqCst) {
            // A session that failed reports it through its operations.
            let _ = self.work.service();
        }
    }

    /// Waits on the signal for up to `limit`.
    fn park(&self, limit: Duration) {
        let (woken, changed) = &*self.signal;
        let mut woken = woken.lock();
        if !*woken {
            changed.wait_for(&mut woken, limit);
        }
        *woken = false;
    }

    /// The result of `completion`, with the session's work running
    /// meanwhile, no later than the test's deadline (watchdog.rs).
    fn wait<T: Send + 'static>(&self, completion: Result<Completion<T>, Error>) -> Result<T, Error> {
        let slot = self.on_complete(completion?);
        loop {
            if let Some(result) = slot.lock().take() {
                return result;
            }
            self.service();
            if slot.lock().is_none() {
                self.park(watchdog::remaining().unwrap_or(Duration::MAX).min(Duration::from_millis(50)));
                watchdog::check_deadline();
            }
        }
    }

    /// Waits for `completion` for up to [`RELEASE_TIMEOUT`], outside the
    /// test's deadline, with the session's work running meanwhile.
    fn wait_to_release<T: Send + 'static>(&self, completion: Result<Completion<T>, Error>) {
        let Ok(completion) = completion else { return };
        let slot = self.on_complete(completion);
        let deadline = Instant::now() + RELEASE_TIMEOUT;
        while slot.lock().is_none() && !self.work.is_finished() && Instant::now() < deadline {
            self.service();
            self.park(Duration::from_millis(10));
        }
    }

    /// A slot that `completion` fills, waking the thread.
    fn on_complete<T: Send + 'static>(
        &self,
        completion: Completion<T>,
    ) -> Arc<parking_lot::Mutex<Option<Result<T, Error>>>> {
        let slot = Arc::new(parking_lot::Mutex::new(None));
        let (filled, signal) = (Arc::clone(&slot), Arc::clone(&self.signal));
        completion.on_complete(move |result| {
            *filled.lock() = Some(result);
            *signal.0.lock() = true;
            signal.1.notify_all();
        });
        slot
    }
}

/// Whether `error` says that the runtime or the session stopped after a
/// panic in the library, which leaves the map unusable.
fn stopped(error: &Error) -> bool {
    error.kind == ErrorKind::Native && error.message.contains("stopped after a panic")
}

/// An error of a command that C++ would throw and the runner doesn't catch.
fn exception(error: &Error) -> MapError {
    if stopped(error) {
        // The runner drops a map whose test panicked.
        panic!("{}", error.message);
    }
    MapError::Exception(error.message.clone())
}

/// A style conversion's error, or one the runner doesn't catch.
fn conversion_or_exception(error: &Error) -> MapError {
    match error.kind {
        ErrorKind::InvalidArgument => MapError::Conversion(error.message.clone()),
        _ => exception(error),
    }
}

/// Serves the runtime's requests from the runner's file source: the cache
/// database and the assets, through [`ProxyFileSource`](crate::file_source::ProxyFileSource).
struct Provider {
    file_source: Arc<dyn FileSource>,
}

impl ResourceProvider for Provider {
    fn request(&self, request: ResourceRequest) {
        let resource = request.resource().clone();
        // The file source's first answer answers the runtime's request.
        let answering = Arc::new(parking_lot::Mutex::new(None::<ResourceRequest>));
        let answer = Arc::clone(&answering);
        let (handle, responder) = Request::once(move |response| {
            if let Some(request) = answer.lock().take() {
                let _ = request.respond(response);
            }
        });
        // The runtime's cancellation cancels the file source's request.
        if request.on_cancel(move || drop(handle)).is_err() {
            return;
        }
        *answering.lock() = Some(request);
        self.file_source.load(&resource, responder);
    }
}

/// A runtime, and a map on it that an offscreen session draws, for the
/// tests of one map mode, pixel ratio, cross-source collision setting,
/// terrain skirt and asset setting.
pub(crate) struct Impl {
    session: RenderSession,
    driven: Driven,
    map: Map,
    runtime: Runtime,
    /// The runner's file source, which the provider serves the runtime's
    /// requests from, and `add_image` loads images from.
    file_source: Arc<dyn FileSource>,
    pixel_ratio: f32,
    /// Why the last style failed to load, which a render reports: the map
    /// keeps the style before.
    style_error: Option<String>,
}

impl Impl {
    /// A runtime with `registry`, which loads through a
    /// [`proxy_loader`], and a map on it for a test with `metadata`, which
    /// renders on `device`.
    pub(crate) fn new(
        registry: &Arc<Registry>,
        metadata: &TestMetadata,
        resource_options: &ResourceOptions,
        impl_options: ImplOptions,
        pool: &Pool,
        device: &Device,
    ) -> Result<Impl, String> {
        let assets = (!resource_options.asset_path.is_empty())
            .then_some(Assets { path: resource_options.asset_path.as_str(), only: resource_options.assets_only });
        let loader_options = mln_engine::storage::ResourceOptions {
            cache_path: resource_options.cache_path.clone(),
            ..mln_engine::storage::ResourceOptions::default()
        };
        let file_source = proxy_loader(&loader_options, assets, pool);
        let runtime = Runtime::with_registry(&RuntimeOptions::default(), Arc::clone(registry))
            .map_err(|e| format!("a runtime: {}", e.message))?;
        mln_runtime::testing::set_tile_worker_codec(&runtime, impl_options.tile_worker_codec);
        wait(runtime.set_resource_provider(Provider { file_source: Arc::clone(&file_source) }))
            .map_err(|e| e.message)?;
        let options = MapOptions {
            map: mln_engine::map::MapOptions {
                map_mode: map_mode(metadata.map_mode),
                size: metadata.size,
                pixel_ratio: metadata.pixel_ratio,
                cross_source_collisions: metadata.cross_source_collisions,
                terrain: mln_engine::map::TerrainOptions {
                    skirt_length: terrain_skirt_length(metadata.terrain_skirt_length),
                    ..Default::default()
                },
                ..Default::default()
            },
            // The expected images draw every glyph from the styles' glyph
            // PBFs.
            system_fonts: false,
            local_ideographs: false,
            ..MapOptions::default()
        };
        let (map, created) = runtime.create_map(options).map_err(|e| e.message)?;
        wait(Ok(created)).map_err(|e| format!("a map: {}", e.message))?;
        let target = RenderTarget::Offscreen { size: metadata.size, device: Some(device.clone()) };
        let woken = Arc::new(AtomicBool::new(false));
        let signal: Arc<(parking_lot::Mutex<bool>, parking_lot::Condvar)> = Arc::default();
        let (wake, wake_signal) = (Arc::clone(&woken), Arc::clone(&signal));
        let attach = AttachOptions {
            driver: Driver::CallerThread,
            driver_work_wake: Some(Arc::new(move || {
                wake.store(true, Ordering::SeqCst);
                *wake_signal.0.lock() = true;
                wake_signal.1.notify_all();
            })),
            ..AttachOptions::default()
        };
        let (session, attached) = map.attach(target, attach).map_err(|e| e.message)?;
        let work = session.take_driver_work().map_err(|e| e.message)?;
        let driven = Driven { work, woken, signal };
        driven.wait(Ok(attached)).map_err(|e| format!("a render session: {}", e.message))?;
        if let Some(max_pitch) = metadata.max_pitch {
            driven
                .wait(map.set_bounds(BoundOptions { max_pitch: Some(f64::from(max_pitch)), ..Default::default() }))
                .map_err(|e| e.message)?;
        }
        Ok(Impl { session, driven, map, runtime, file_source, pixel_ratio: metadata.pixel_ratio, style_error: None })
    }

    /// Records whether the style load of `loaded` failed.
    fn style_loaded<T>(&mut self, loaded: Result<T, Error>) {
        self.style_error = match loaded {
            Ok(_) => None,
            Err(error) if stopped(&error) => panic!("{}", error.message),
            Err(error) => Some(error.message),
        };
    }

    /// The style's default camera, as its loaded document gives it.
    fn style_camera(&mut self) -> Result<CameraOptions, MapError> {
        let root = |name: &str| -> Result<Option<serde_json::Value>, MapError> {
            let json = self.driven.wait(self.map.style_root_property(name)).map_err(|e| exception(&e))?;
            Ok(json.and_then(|json| serde_json::from_str(&json).ok()))
        };
        // A missing or invalid member is its default, as the style parser
        // reads it.
        let center = root("center")?
            .and_then(|value| {
                let mut error = mln_style::conversion::Error::default();
                convert::<LatLng>(&Convertible::from(&value), &mut error)
            })
            .unwrap_or_default();
        let number = |name: &str| -> Result<f64, MapError> {
            Ok(root(name)?.as_ref().and_then(serde_json::Value::as_f64).unwrap_or_default())
        };
        Ok(CameraOptions {
            center: Some(center),
            center_altitude: Some(number("centerAltitude")?),
            zoom: Some(number("zoom")?),
            bearing: Some(number("bearing")?),
            pitch: Some(number("pitch")?),
            roll: Some(number("roll")?),
        })
    }

    /// Changes the map's tile settings.
    fn set_tile(&mut self, options: TileOptions) -> Result<(), MapError> {
        self.driven.wait(self.map.set_tile_options(options)).map(drop).map_err(|e| exception(&e))
    }

    /// Renders a still image first, as C++ does before it reaches the
    /// renderer for feature state: `false` when the image fails.
    fn rendered(&mut self) -> bool {
        self.render().is_ok()
    }
}

impl Drop for Impl {
    /// Lets go of the session, the map and the runtime, and waits for them,
    /// so that nothing of them is left running when the runner exits.
    fn drop(&mut self) {
        self.driven.wait_to_release(self.session.release());
        while !self.driven.work.is_finished() && self.driven.work.service().is_ok() {}
        wait_to_release(self.map.release());
        wait_to_release(self.runtime.release());
    }
}

fn engine_camera(camera: &CameraOptions) -> mln_engine::camera::CameraOptions {
    mln_engine::camera::CameraOptions {
        center: camera.center,
        center_altitude: camera.center_altitude,
        zoom: camera.zoom,
        bearing: camera.bearing,
        pitch: camera.pitch,
        roll: camera.roll,
        ..Default::default()
    }
}

/// The selector of a feature state operation: its feature, unless the id is
/// empty.
fn selector(source_id: &str, source_layer: Option<&str>, feature_id: Option<&str>) -> FeatureStateSelector {
    FeatureStateSelector {
        source_id: source_id.to_owned(),
        source_layer_id: source_layer.map(str::to_owned),
        feature_id: feature_id.filter(|id| !id.is_empty()).map(|id| Identifier::String(id.to_owned())),
        state_key: None,
    }
}

impl TestRunnerImpl for Impl {
    fn reset_context(&mut self, metadata: &TestMetadata) -> Result<(), MapError> {
        self.driven.wait(self.session.clear_data()).map_err(|e| exception(&e))?;
        let extent = LogicalExtent { size: metadata.size, scale_factor: f64::from(self.pixel_ratio) };
        self.driven.wait(self.session.resize(extent)).map_err(|e| exception(&e))?;
        let mode = ProjectionMode::default()
            .with_axonometric(metadata.axonometric)
            .with_x_skew(metadata.x_skew)
            .with_y_skew(metadata.y_skew);
        self.driven.wait(self.map.set_projection_mode(mode)).map_err(|e| exception(&e))?;
        // A test that unclamps the center leaves the map to the next one.
        let viewport = ViewportOptions { center_clamped_to_ground: Some(true), ..Default::default() };
        self.driven.wait(self.map.set_viewport_options(viewport)).map_err(|e| exception(&e))?;
        // The operations after the load see the parsed style, as in C++.
        let loaded = self.driven.wait(self.map.set_style_json(metadata.document_json.clone()));
        self.style_loaded(loaded);
        Ok(())
    }

    fn jump_to_default_camera(&mut self) -> Result<(), MapError> {
        let camera = self.style_camera()?;
        self.jump_to(&camera)
    }

    fn render(&mut self) -> Result<PremultipliedImage, String> {
        if let Some(error) = &self.style_error {
            return Err(error.clone());
        }
        let image = self
            .driven
            .wait(self.map.request_still_image())
            .and_then(|()| self.driven.wait(self.session.read_texture()));
        image.map_err(|error| {
            let failed =
                matches!(self.session.snapshot(), Ok(snapshot) if snapshot.state == RenderSessionState::Failed);
            if stopped(&error) || failed {
                // The runner drops a map whose test panicked.
                panic!("{}", error.message);
            }
            error.message
        })
    }

    fn dump_debug_logs(&mut self) {
        self.driven.wait_to_release(self.map.dump_debug_logs());
        self.driven.wait_to_release(self.session.dump_debug_logs());
    }

    fn default_camera(&mut self) -> Result<CameraOptions, MapError> {
        self.style_camera()
    }

    fn tile_cover_for_camera(
        &mut self,
        camera: &CameraOptions,
        zoom: u8,
    ) -> Result<Vec<crate::runner::TileId>, MapError> {
        let bounds =
            self.driven.wait(self.map.lat_lng_bounds_for_camera(engine_camera(camera))).map_err(|e| exception(&e))?;
        Ok(mln_engine::tile_store::tile_cover::tile_cover_bounds(&bounds, zoom)
            .into_iter()
            .map(|id| crate::runner::TileId { z: id.canonical.z, x: id.canonical.x, y: id.canonical.y, wrap: id.wrap })
            .collect())
    }

    fn jump_to(&mut self, camera: &CameraOptions) -> Result<(), MapError> {
        let update =
            CameraUpdate { mode: CameraUpdateMode::Jump, camera: engine_camera(camera), ..CameraUpdate::default() };
        self.driven.wait(self.map.update_camera(update)).map(drop).map_err(|e| exception(&e))
    }

    fn load_style_json(&mut self, json: &str) -> Result<(), MapError> {
        let loaded = self.driven.wait(self.map.set_style_json(json));
        self.style_loaded(loaded);
        Ok(())
    }

    fn load_style_url(&mut self, url: &str) -> Result<(), MapError> {
        let loaded = self.driven.wait(self.map.set_style_url(url));
        self.style_loaded(loaded);
        Ok(())
    }

    fn set_layer_property(&mut self, layer: &str, name: &str, value: &serde_json::Value) -> Result<(), MapError> {
        match self.driven.wait(self.map.set_style_layer_property(layer, name, value.to_string())) {
            Ok(_) => Ok(()),
            Err(error) if error.kind == ErrorKind::NotFound => Err(MapError::LayerNotFound),
            // The runner ignores a value that doesn't fit the property.
            Err(error) => Err(conversion_or_exception(&error)),
        }
    }

    fn set_layer_zoom_range(&mut self, layer: &str, min: f32, max: f32) -> Result<(), MapError> {
        for (name, zoom) in [("minzoom", min), ("maxzoom", max)] {
            match self.driven.wait(self.map.set_style_layer_property(layer, name, zoom.to_string())) {
                Ok(_) => {}
                Err(error) if error.kind == ErrorKind::NotFound => return Err(MapError::LayerNotFound),
                Err(error) => return Err(conversion_or_exception(&error)),
            }
        }
        Ok(())
    }

    fn add_layer(&mut self, json: &str) -> Result<(), MapError> {
        self.driven.wait(self.map.add_style_layer_json(json, None)).map(drop).map_err(|e| conversion_or_exception(&e))
    }

    fn remove_layer(&mut self, id: &str) -> Result<bool, MapError> {
        match self.driven.wait(self.map.remove_style_layer(id)) {
            Ok(_) => Ok(true),
            Err(error) if error.kind == ErrorKind::NotFound => Ok(false),
            Err(error) => Err(exception(&error)),
        }
    }

    fn set_layer_filter(&mut self, layer: &str, filter: &serde_json::Value) -> Result<(), MapError> {
        match self.driven.wait(self.map.set_style_layer_property(layer, "filter", filter.to_string())) {
            Ok(_) => Ok(()),
            Err(error) if error.kind == ErrorKind::NotFound => Err(MapError::LayerNotFound),
            Err(error) => Err(conversion_or_exception(&error)),
        }
    }

    fn set_global_state_property(&mut self, name: &str, value: &serde_json::Value) -> Result<(), MapError> {
        self.driven
            .wait(self.map.set_global_state_property(name, value.to_string()))
            .map(drop)
            .map_err(|e| exception(&e))
    }

    fn set_feature_state(
        &mut self,
        source_id: &str,
        source_layer: Option<&str>,
        feature_id: &str,
        state: &mln_style::feature::PropertyMap,
    ) -> Result<bool, MapError> {
        if !self.rendered() {
            return Ok(false);
        }
        let selector = selector(source_id, source_layer, Some(feature_id));
        let state = state.iter().map(|(key, value)| (key.clone(), value.clone())).collect();
        match self.driven.wait(self.map.set_feature_state(&selector, state)) {
            Ok(_) => Ok(true),
            Err(error) if error.kind == ErrorKind::InvalidArgument => Ok(false),
            Err(error) => Err(exception(&error)),
        }
    }

    fn get_feature_state(
        &mut self,
        source_id: &str,
        source_layer: Option<&str>,
        feature_id: &str,
    ) -> Result<bool, MapError> {
        if !self.rendered() {
            return Ok(false);
        }
        let selector = selector(source_id, source_layer, Some(feature_id));
        match self.driven.wait(self.map.feature_state(&selector)) {
            Ok(_) => Ok(true),
            Err(error) if error.kind == ErrorKind::InvalidArgument => Ok(false),
            Err(error) => Err(exception(&error)),
        }
    }

    fn remove_feature_state(
        &mut self,
        source_id: &str,
        source_layer: Option<&str>,
        feature_id: Option<&str>,
        state_key: Option<&str>,
    ) -> Result<bool, MapError> {
        if !self.rendered() {
            return Ok(false);
        }
        let selector = FeatureStateSelector {
            state_key: state_key.map(str::to_owned),
            ..selector(source_id, source_layer, feature_id)
        };
        match self.driven.wait(self.map.remove_feature_state(&selector)) {
            Ok(_) => Ok(true),
            Err(error) if error.kind == ErrorKind::InvalidArgument => Ok(false),
            Err(error) => Err(exception(&error)),
        }
    }

    fn query_rendered_features(
        &mut self,
        geometry: &QueryGeometry,
        options: &crate::metadata::RenderedQueryOptions,
    ) -> Result<Vec<mln_engine::tile::feature::Feature>, MapError> {
        let filter = match &options.filter {
            Some(json) => {
                let mut error = mln_style::conversion::Error::default();
                let functions = self.map.registry().functions();
                match mln_style::filter::convert_filter(&Convertible::from(json), functions, &mut error) {
                    Some(filter) => Some(filter),
                    None => return Err(MapError::Conversion(error.message)),
                }
            }
            None => None,
        };
        let options = mln_runtime::RenderedQueryOptions::new(options.layer_ids.clone(), filter);
        let geometry = match geometry {
            QueryGeometry::Point(point) => RenderedQueryGeometry::Point(*point),
            QueryGeometry::Box(screen_box) => RenderedQueryGeometry::Box { min: screen_box.min, max: screen_box.max },
        };
        self.driven.wait(self.session.query_rendered_features(geometry, options)).map_err(|e| exception(&e))
    }

    fn set_tile_lod_min_radius(&mut self, value: f64) -> Result<(), MapError> {
        self.set_tile(TileOptions { lod_min_radius: Some(value), ..Default::default() })
    }

    fn set_tile_lod_scale(&mut self, value: f64) -> Result<(), MapError> {
        self.set_tile(TileOptions { lod_scale: Some(value), ..Default::default() })
    }

    /// The runner gives the threshold in radians, as C++ takes it.
    fn set_tile_lod_pitch_threshold(&mut self, value: f64) -> Result<(), MapError> {
        self.set_tile(TileOptions { lod_pitch_threshold: Some(mln_style::math::rad2deg(value)), ..Default::default() })
    }

    fn set_tile_lod_zoom_shift(&mut self, value: f64) -> Result<(), MapError> {
        self.set_tile(TileOptions { lod_zoom_shift: Some(value), ..Default::default() })
    }

    fn set_tile_lod_mode(&mut self, mode: crate::parser::TileLodMode) -> Result<(), MapError> {
        let lod_mode = match mode {
            crate::parser::TileLodMode::Default => mln_engine::tile_store::TileLodMode::Default,
            crate::parser::TileLodMode::Distance => mln_engine::tile_store::TileLodMode::Distance,
        };
        self.set_tile(TileOptions { lod_mode: Some(lod_mode), ..Default::default() })
    }

    fn set_light(&mut self, json: &str) -> Result<(), MapError> {
        self.driven
            .wait(self.map.set_style_root_property("light", json))
            .map(drop)
            .map_err(|e| conversion_or_exception(&e))
    }

    fn set_terrain(&mut self, json: &str) -> Result<(), MapError> {
        self.driven.wait(self.map.set_style_root_property("terrain", json)).map(drop).map_err(|e| match e.kind {
            ErrorKind::Unsupported => MapError::Unsupported(e.message),
            _ => conversion_or_exception(&e),
        })
    }

    fn set_center_clamped_to_ground(&mut self, clamped: bool) -> Result<(), MapError> {
        let viewport = ViewportOptions { center_clamped_to_ground: Some(clamped), ..Default::default() };
        self.driven.wait(self.map.set_viewport_options(viewport)).map(drop).map_err(|e| exception(&e))
    }

    fn add_source(&mut self, id: &str, json: &str) -> Result<(), MapError> {
        self.driven.wait(self.map.add_style_source_json(id, json)).map(drop).map_err(|e| conversion_or_exception(&e))
    }

    fn remove_source(&mut self, id: &str) -> Result<bool, MapError> {
        match self.driven.wait(self.map.remove_style_source(id)) {
            Ok(_) => Ok(true),
            Err(error) if matches!(error.kind, ErrorKind::NotFound | ErrorKind::InvalidState) => Ok(false),
            Err(error) => Err(exception(&error)),
        }
    }

    fn add_image(&mut self, name: &str, image_path: &str, pixel_ratio: f32, sdf: bool) -> Result<(), MapError> {
        // C++ requests "mapbox://render-tests/" + imagePath from the
        // runner's file source.
        let (sender, responses) = mpsc::channel();
        let _request = self.file_source.request(
            &Resource::image(&format!("mapbox://render-tests/{image_path}")),
            move |response: Response| {
                let _ = sender.send(response.data);
            },
        );
        let Some(bytes) = watchdog::recv(&responses).flatten() else {
            return Err(MapError::LoadFailed);
        };
        // C++ `decodeImage` and the `style::Image` constructor throw (not
        // caught by the runner).
        let image = mln_engine::image::decode_image(&bytes).map_err(|e| MapError::Exception(e.to_string()))?;
        let image = mln_style::image::Impl::new(
            name.to_string(),
            image,
            pixel_ratio,
            sdf,
            Vec::new(),
            Vec::new(),
            None,
            None,
            None,
        )
        .map_err(|e| MapError::Exception(e.to_string()))?;
        self.driven.wait(self.map.set_style_image(image)).map(drop).map_err(|e| exception(&e))
    }

    fn remove_image(&mut self, name: &str) -> Result<(), MapError> {
        match self.driven.wait(self.map.remove_style_image(name)) {
            Ok(_) => Ok(()),
            Err(error) if error.kind == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(exception(&error)),
        }
    }

    fn sleep(&mut self, duration: Duration) -> Result<(), MapError> {
        watchdog::sleep(duration);
        Ok(())
    }
}

fn terrain_skirt_length(length: TerrainSkirtLength) -> mln_engine::map::TerrainSkirtLength {
    match length {
        TerrainSkirtLength::Auto => mln_engine::map::TerrainSkirtLength::Auto,
        TerrainSkirtLength::None => mln_engine::map::TerrainSkirtLength::None,
    }
}

fn map_mode(mode: MapMode) -> mln_engine::map::MapMode {
    match mode {
        MapMode::Continuous => mln_engine::map::MapMode::Continuous,
        MapMode::Static => mln_engine::map::MapMode::Static,
        MapMode::Tile => mln_engine::map::MapMode::Tile,
    }
}

/// The factory `TestRunner::run` uses for `maps[key]`, whose maps draw with
/// `registry`'s layer and source types: the built-in ones, and those of the
/// crates whose tests run.
pub fn factory(registry: Arc<Registry>) -> ImplFactory {
    // The runner's file source reads the cache database on these threads.
    let pool = Pool::new(4);
    Box::new(move |metadata: &TestMetadata, resource_options: &ResourceOptions, impl_options: &ImplOptions| {
        let device = harness_device().map_err(|e| e.message)?;
        Ok(Box::new(Impl::new(&registry, metadata, resource_options, *impl_options, &pool, &device)?))
    })
}
