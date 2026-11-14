//! A map and its headless frontend over the benchmarks' data (README.md):
//! the shared render suite's real-world styles, read from its cache
//! database, and maplibre-native-ffi's basemap, read from the files
//! `fixtures.py` prepared. Nothing uses the network.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use feature::{Value, ValueObject};
use mln_actor::RunLoop;
use mln_actor::timer::Timer;
use mln_headless::HeadlessFrontend;
use mln_map::{CameraOptions, Map, MapEvent, MapMode, MapOptions, RenderFrameStatus, RenderMode};
use mln_renderer::query::SourceQueryOptions;
use mln_runtime::Device;
use mln_storage::{FileSource, FileSourceManager, FileSourceType, ResourceOptions};
use mln_style::sources::geojson_source::GeoJSONSource;
use mln_style_types::VisibilityType;
use mln_text::GlyphCache;
use mln_util::feature::feature_id_to_string;
use mln_util::geo::LatLng;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

/// A display's refresh period at 60 Hz, the clock continuous cases draw on.
pub(crate) const REFRESH: Duration = Duration::from_nanos(16_666_667);

/// How long a scene may take to show a complete frame.
const WAIT_LIMIT: Duration = Duration::from_secs(60);

/// A `render-tests/real-world/*` style whose tiles the cache database
/// holds, at its test's camera and size.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RealWorld {
    pub(crate) name: &'static str,
    pub(crate) latitude: f64,
    pub(crate) longitude: f64,
    pub(crate) zoom: f64,
}

pub(crate) const REAL_WORLD: &[RealWorld] = &[
    RealWorld { name: "chicago", latitude: 41.870_396_5, longitude: -87.694_244_5, zoom: 13.0 },
    RealWorld { name: "sanfrancisco", latitude: 37.766_999_5, longitude: -122.448_635, zoom: 15.0 },
    RealWorld { name: "norway", latitude: 64.863_174, longitude: 10.8181, zoom: 12.0 },
];

/// The render tests' view, whose 1x sprites the cache database holds.
pub(crate) const REAL_WORLD_SIZE: Size = Size { width: 512, height: 512 };

impl RealWorld {
    #[must_use]
    pub(crate) fn style_url(&self) -> String {
        format!("local://styles/{}.json", self.name)
    }

    #[must_use]
    pub(crate) fn center(&self) -> LatLng {
        LatLng::new(self.latitude, self.longitude).expect("a scene's center")
    }
}

/// The render suite's `text-font/chinese` style, without its unused
/// sprite: `data/chinese.geojson`'s 500 labels in Noto Sans CJK, about 2,500
/// characters over 80 glyph ranges, all in the cache database.
pub(crate) const CJK_STYLE: &str = r#"{
  "version": 8,
  "sources": { "geojson": { "type": "geojson", "data": "local://data/chinese.geojson" } },
  "glyphs": "local://glyphs/{fontstack}/{range}.pbf",
  "layers": [
    { "id": "background", "type": "background", "paint": { "background-color": "white" } },
    {
      "id": "labels",
      "type": "symbol",
      "source": "geojson",
      "layout": { "text-field": "{name}", "text-font": ["NotoCJK"], "symbol-placement": "point" }
    }
  ]
}"#;

/// The `text-font/chinese` test's camera, at its size.
#[must_use]
pub(crate) fn cjk_center() -> LatLng {
    LatLng::new(43.390_048_986_773_8, -70.752_954_126).expect("the CJK view's center")
}
pub(crate) const CJK_ZOOM: f64 = 11.0;
pub(crate) const CJK_SIZE: Size = Size { width: 512, height: 512 };

/// maplibre-native-ffi's view of the basemap.
pub(crate) const BASEMAP_SIZE: Size = Size { width: 1024, height: 768 };
/// The zoom of the basemap's tiles, and of its views.
pub(crate) const BASEMAP_ZOOM: f64 = 14.0;

/// A point of the basemap's 5 x 5 block of zoom-14 tiles, in tile
/// coordinates: x from 2618 to 2623, y from 6330 to 6335.
#[must_use]
pub(crate) fn basemap_point(x: f64, y: f64) -> LatLng {
    let tiles = 16384.0;
    let longitude = x / tiles * 360.0 - 180.0;
    let latitude = (std::f64::consts::PI * (1.0 - 2.0 * y / tiles)).sinh().atan().to_degrees();
    LatLng::new(latitude, longitude).expect("a point of the block")
}

/// The block's center, and the corners of the views that share no tile.
#[must_use]
pub(crate) fn basemap_center() -> LatLng {
    basemap_point(2620.5, 6332.5)
}
#[must_use]
pub(crate) fn basemap_north_west() -> LatLng {
    basemap_point(2619.0, 6331.0)
}
#[must_use]
pub(crate) fn basemap_south_east() -> LatLng {
    basemap_point(2622.0, 6334.0)
}

/// The basemap's style, its URLs pointing into `fixtures`.
#[must_use]
pub(crate) fn basemap_style(fixtures: &Path) -> String {
    let style = include_str!("../style.json");
    style.replace("mln-bench-fixtures:/", &format!("file://{}/", fixtures.display()))
}

/// Where `prepare_basemap_mlt` writes the basemap's tiles as MLT, under
/// the fixtures.
const BASEMAP_MLT_DIR: &str = "basemap/mlt-tessellated";

/// The basemap's style over its tiles as MLT that carry their polygons'
/// triangles, which `prepare_basemap_mlt` wrote.
#[must_use]
pub(crate) fn basemap_mlt_style(fixtures: &Path) -> String {
    let tiles = r#""tiles": ["mln-bench-fixtures:/basemap/tiles/{z}/{x}/{y}.pbf"],"#;
    let style = include_str!("../style.json");
    assert!(style.contains(tiles), "the basemap style's tile URL");
    let style = style.replace(
        tiles,
        &format!(r#""tiles": ["mln-bench-fixtures:/{BASEMAP_MLT_DIR}/{{z}}/{{x}}/{{y}}.mlt"], "encoding": "mlt","#),
    );
    style.replace("mln-bench-fixtures:/", &format!("file://{}/", fixtures.display()))
}

/// Writes the basemap's tiles as MLT, every polygon with its triangles, as
/// mlt-core encodes them for MapLibre Native; once per process.
pub(crate) fn prepare_basemap_mlt(fixtures: &Path) {
    static PREPARED: OnceLock<()> = OnceLock::new();
    PREPARED.get_or_init(|| {
        use mlt_core::encoder::{EncoderConfig, Network, Profile, Tessellation};

        // The tiles' decoder reads no FastPFOR streams.
        let config = EncoderConfig::from_profile(Profile::Native(Network::default()))
            .with_tessellation(Tessellation::Always)
            .with_fastpfor(false);
        let tiles = fixtures.join("basemap/tiles");
        let walk = |dir: &Path| -> Vec<PathBuf> {
            let mut entries: Vec<PathBuf> =
                std::fs::read_dir(dir).expect("a basemap tile directory").map(|e| e.expect("entry").path()).collect();
            entries.sort();
            entries
        };
        for z in walk(&tiles) {
            for x in walk(&z) {
                for mvt in walk(&x) {
                    let relative = mvt.strip_prefix(&tiles).expect("under the tiles").with_extension("mlt");
                    let out = fixtures.join(BASEMAP_MLT_DIR).join(relative);
                    let layers = mlt_core::mvt::mvt_to_tile_layers(std::fs::read(&mvt).expect("a basemap tile"))
                        .expect("an MVT tile");
                    let mut mlt = Vec::new();
                    for layer in layers {
                        mlt.extend(layer.encode(config).expect("an MLT layer"));
                    }
                    std::fs::create_dir_all(out.parent().expect("a directory")).expect("the MLT tile directory");
                    std::fs::write(&out, mlt).expect("an MLT tile");
                }
            }
        }
    });
}

/// When the map draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// A static map: `Scene::render_still` draws a complete frame.
    Still,
    /// A continuous map, drawn only when the case calls
    /// `Scene::render_frame`, as a host on a display clock does.
    OnDemand,
    /// A continuous map, drawn whenever the map updates, as the run loop
    /// delivers the update.
    Driven,
}

/// The frames the map reports, and why it failed, if it did.
#[derive(Default)]
struct Observer {
    frames: u64,
    last_frame: Option<RenderFrameStatus>,
    failure: Option<String>,
}

impl Observer {
    fn observe(&mut self, events: Vec<MapEvent>) {
        for event in events {
            match event {
                MapEvent::RenderFrameFinished(status) => {
                    self.frames += 1;
                    self.last_frame = Some(status);
                }
                MapEvent::LoadingFailed(error) => self.failure = Some(error.to_string()),
                _ => {}
            }
        }
    }
}

/// The file sources every scene shares: the render suite's proxy over its
/// cache database, with the tile latency of [`crate::latency`].
fn file_sources() -> FileSourceManager {
    static FILE_SOURCES: OnceLock<FileSourceManager> = OnceLock::new();
    FILE_SOURCES
        .get_or_init(|| {
            let file_sources = render_test::file_source::proxy_file_sources();
            crate::latency::register(&file_sources);
            file_sources
        })
        .clone()
}

pub struct Scene {
    frontend: HeadlessFrontend,
    map: Map,
    observer: Observer,
    _file_source: Option<Arc<dyn FileSource>>,
}

impl Scene {
    /// A map without a style; its GPU device is ready. Its renderer has a
    /// glyph cache of its own.
    #[must_use]
    pub fn new(size: Size, mode: Mode) -> Scene {
        Scene::with_glyph_cache(size, mode, Arc::default())
    }

    /// A map whose renderer shares `glyph_cache`.
    pub(crate) fn with_glyph_cache(size: Size, mode: Mode, glyph_cache: Arc<GlyphCache>) -> Scene {
        let file_sources = file_sources();
        // MLN_BENCH_CACHE_DB names the render suite's cache database where
        // the binary runs without its checkout, as on an Android device.
        let cache = std::env::var_os("MLN_BENCH_CACHE_DB").map_or_else(
            || Path::new(env!("CARGO_MANIFEST_DIR")).join("../render-test/testdata/metrics/cache-style.db"),
            PathBuf::from,
        );
        let resource_options =
            ResourceOptions { cache_path: cache.display().to_string(), ..ResourceOptions::default() };
        let frontend = HeadlessFrontend::with_glyph_cache(size, 1.0, mode != Mode::OnDemand, glyph_cache)
            .expect("a headless frontend");
        let file_source = file_sources.get_file_source(FileSourceType::ResourceLoader, &resource_options);
        let map_mode = if mode == Mode::Still { MapMode::Static } else { MapMode::Continuous };
        let map = Map::with_resource_options(
            MapOptions { map_mode, size, pixel_ratio: 1.0, ..Default::default() },
            &file_sources,
            &resource_options,
        )
        .expect("the map");
        frontend.attach(&map);
        Scene { frontend, map, observer: Observer::default(), _file_source: file_source }
    }

    /// Starts loading a style; it loads while the scene renders.
    pub(crate) fn load_url(&mut self, url: &str) {
        self.map.style_mut().load_url(url);
    }

    pub(crate) fn load_json(&mut self, json: &str) {
        self.map.style_mut().load_json(json);
    }

    /// Replaces the data of the GeoJSON source `id` with `json`.
    pub(crate) fn set_geojson(&mut self, id: &str, json: &str) {
        let geojson: geojson::GeoJson = json.parse().expect("valid GeoJSON");
        let source = self.map.style().get_source(id).expect("the source");
        let source = source.downcast_ref::<GeoJSONSource>().expect("a GeoJSON source");
        source.set_geojson(geojson).expect("the source's data");
    }

    /// Adds a layer from its JSON below `before`, or above every layer.
    pub(crate) fn add_layer_json(&mut self, json: &str, before: Option<&str>) {
        let value: serde_json::Value = serde_json::from_str(json).expect("a layer is JSON");
        let mut error = mln_style::conversion::Error::default();
        let layer = mln_style::conversion::convert_layer(&mln_style::conversion::Convertible::from(&value), &mut error)
            .expect("a valid layer");
        self.map.style_mut().add_layer(layer, before).expect("a new layer");
    }

    pub(crate) fn remove_layer(&mut self, id: &str) {
        self.map.style_mut().remove_layer(id).expect("the layer");
    }

    /// Replaces the data of the GeoJSON source `id` with `json`.
    pub(crate) fn set_geojson(&mut self, id: &str, json: &str) {
        let document: serde_json::Value = serde_json::from_str(json).expect("GeoJSON is JSON");
        let mut error = mln_conversion::Error::default();
        let geojson =
            mln_conversion::to_geojson(&mln_conversion::Convertible::from(&document), &mut error).expect("GeoJSON");
        let source: &dyn std::any::Any = self.map.style().get_source(id).expect("the source");
        let source =
            source.downcast_ref::<mln_style::sources::geojson_source::GeoJSONSource>().expect("a GeoJSON source");
        source.set_geojson(geojson).expect("the data");
    }

    /// Adds an image to the style, as a sprite's would be.
    pub(crate) fn add_image(&mut self, id: &str, image: PremultipliedImage) {
        let image =
            mln_style::image::Impl::new(id.to_owned(), image, 1.0, false, Vec::new(), Vec::new(), None, None, None)
                .expect("a style image");
        self.map.style_mut().add_image(Arc::new(image));
    }

    pub(crate) fn jump(&mut self, center: LatLng, zoom: f64) {
        self.jump_rotated(center, zoom, 0.0);
    }

    /// Jumps to a camera with `bearing` in degrees.
    pub(crate) fn jump_rotated(&mut self, center: LatLng, zoom: f64, bearing: f64) {
        self.map.jump_to(&CameraOptions {
            center: Some(center),
            zoom: Some(zoom),
            bearing: Some(bearing),
            ..Default::default()
        });
    }

    /// Jumps to a camera with `bearing` and `pitch` in degrees.
    pub(crate) fn jump_pitched(&mut self, center: LatLng, zoom: f64, bearing: f64, pitch: f64) {
        self.map.jump_to(&CameraOptions {
            center: Some(center),
            zoom: Some(zoom),
            bearing: Some(bearing),
            pitch: Some(pitch),
            ..Default::default()
        });
    }

    /// A complete still frame, read back. Waits on the run loop, which
    /// sleeps until there is work, rather than polling it, so the wait
    /// costs no CPU time.
    pub(crate) fn render_still(&mut self) -> PremultipliedImage {
        let outcome: Rc<RefCell<Option<Result<(), String>>>> = Rc::default();
        let done = Rc::clone(&outcome);
        self.map.render_still(Box::new(move |result| {
            *done.borrow_mut() = Some(result.map_err(|error| error.to_string()));
        }));
        self.run_until(|| outcome.borrow().is_some());
        if let Some(Err(error)) = outcome.take() {
            panic!("still: {error}");
        }
        self.frontend.read_still_image()
    }

    /// A complete still frame through `HeadlessFrontend::render`, as the
    /// render tests draw one.
    pub(crate) fn render(&mut self) -> PremultipliedImage {
        let result = self.frontend.render(&mut self.map);
        self.observe();
        match result {
            Ok(result) => result.image,
            Err(error) => panic!("still: {error}"),
        }
    }

    /// Delivers what the map's workers have finished, without waiting.
    pub(crate) fn pump(&mut self) {
        RunLoop::get().run_once();
        self.observe();
    }

    /// Runs the run loop, and the map's events, until `done`. The loop
    /// sleeps until there is work, rather than polling it, so the wait
    /// costs no CPU time.
    fn run_until(&mut self, done: impl Fn() -> bool) {
        let run_loop = RunLoop::get();
        loop {
            self.observe();
            if done() {
                return;
            }
            if !run_loop.run_once() {
                run_loop.wait_for_work();
            }
        }
    }

    /// Hands the map's events to the observer.
    fn observe(&mut self) {
        self.observer.observe(self.map.drain_events());
    }

    /// Draws a frame of what has loaded and waits for the GPU to finish
    /// it, as a host does before presenting it.
    pub(crate) fn render_frame(&mut self) -> RenderFrameStatus {
        self.frontend.render_frame();
        self.wait_for_gpu();
        self.observe();
        self.observer.last_frame.clone().expect("the renderer reports each frame")
    }

    /// The Metal and Vulkan backends finish a frame before `render_frame`
    /// returns (their swap waits for the frame's commands); WebGPU's is left
    /// to the device.
    fn wait_for_gpu(&self) {
        if let Device::WebGpu(device) = self.frontend.device() {
            let device = device.device();
            // PowerVR's Vulkan driver on Android ends a wait shortly before
            // wgpu sees the submission finished, which wgpu reports as a
            // timeout; waiting again returns at once then.
            let start = Instant::now();
            loop {
                match device.poll(wgpu::PollType::wait_indefinitely()) {
                    Err(wgpu::PollError::Timeout) if start.elapsed() < WAIT_LIMIT => {}
                    result => {
                        result.expect("the GPU finishes the frame");
                        break;
                    }
                }
            }
        }
    }

    /// The last frame [`Scene::render_frame`] drew, read back.
    pub(crate) fn read_frame(&self) -> PremultipliedImage {
        self.frontend.read_still_image()
    }

    /// Draws frames on the display's clock until one shows the view
    /// complete, and returns how many it drew.
    pub(crate) fn present_until_complete(&mut self) -> usize {
        self.present_until(|status| status.mode == RenderMode::Full)
    }

    /// Draws frames on the display's clock until the view is complete and
    /// no transition or fade needs another frame.
    pub(crate) fn settle(&mut self) -> usize {
        self.present_until(|status| status.mode == RenderMode::Full && !status.needs_repaint)
    }

    /// As [`settle`](Self::settle), until `shown` also holds, adding to
    /// `times` how long each frame took, from asking for it until the GPU
    /// finished it.
    pub(crate) fn settle_timed_until(&mut self, shown: impl Fn(&Scene) -> bool, times: &mut Vec<Duration>) -> usize {
        self.present_until_timed(
            |scene, status| status.mode == RenderMode::Full && !status.needs_repaint && shown(scene),
            Some(times),
        )
    }

    fn present_until(&mut self, done: impl Fn(&RenderFrameStatus) -> bool) -> usize {
        self.present_until_timed(|_, status| done(status), None)
    }

    fn present_until_timed(
        &mut self,
        done: impl Fn(&Scene, &RenderFrameStatus) -> bool,
        mut times: Option<&mut Vec<Duration>>,
    ) -> usize {
        let start = Instant::now();
        let mut frames = 0;
        loop {
            let deadline = start + REFRESH * frames as u32;
            std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
            self.pump();
            frames += 1;
            let demanded = Instant::now();
            let status = self.render_frame();
            if let Some(times) = times.as_deref_mut() {
                times.push(demanded.elapsed());
            }
            if done(self, &status) {
                return frames;
            }
            self.check(start);
        }
    }

    /// Panics when the map failed, or took longer than its limit.
    fn check(&self, start: Instant) {
        if let Some(failure) = self.observer.failure.as_ref() {
            panic!("the map failed to load: {failure}");
        }
        assert!(start.elapsed() < WAIT_LIMIT, "the view isn't complete after {WAIT_LIMIT:?}");
    }

    /// Lets the run loop work for `duration`, drawing as the map asks
    /// (`Mode::Driven`).
    pub(crate) fn run_for(&mut self, duration: Duration) {
        let timer = Timer::new();
        let elapsed = Rc::new(Cell::new(false));
        let done = Rc::clone(&elapsed);
        timer.start(duration, Duration::ZERO, move || done.set(true));
        self.run_until(|| elapsed.get());
    }

    /// The frames the renderer has drawn.
    pub(crate) fn frames(&self) -> u64 {
        self.observer.frames
    }

    /// Whether the last frame showed the view complete with nothing left to
    /// animate.
    pub(crate) fn is_settled(&self) -> bool {
        self.observer.last_frame.as_ref().is_some_and(|status| status.mode == RenderMode::Full && !status.needs_repaint)
    }

    /// Shows or hides every layer that draws `source`.
    pub(crate) fn set_source_visible(&mut self, source: &str, visible: bool) {
        let ids: Vec<String> = self
            .map
            .style()
            .get_layers()
            .iter()
            .filter(|layer| layer.get_source_id() == source)
            .map(|layer| layer.get_id())
            .collect();
        assert!(!ids.is_empty(), "no layer draws {source}");
        let visibility = if visible { VisibilityType::Visible } else { VisibilityType::None };
        for id in ids {
            self.map.style_mut().get_layer_mut(&id).expect("the layer").set_visibility(visibility);
        }
    }

    /// The vertex and index buffers the GPU held after the last frame, in
    /// MiB.
    pub(crate) fn gpu_buffers_mib(&self) -> f64 {
        let stats = &self.observer.last_frame.as_ref().expect("a frame").rendering_stats;
        f64::from(stats.mem_vertex_buffers + stats.mem_index_buffers) / (1024.0 * 1024.0)
    }

    /// Drops every loaded tile and the renderer's other data, as after a
    /// memory warning: the next frames load and lay out again.
    pub(crate) fn clear_data(&self) {
        self.frontend.with_renderer(mln_renderer::Renderer::clear_data);
    }

    /// Sets `selected` on every other feature of `source_layer` in the
    /// loaded tiles, and `false` on the rest.
    pub(crate) fn select_every_feature(&self, source: &str, source_layer: &str) {
        self.frontend.with_renderer(|renderer| {
            let options = SourceQueryOptions::new(Some(vec![source_layer.to_string()]), None);
            let features = renderer.query_source_features(source, &options).expect("the source's features");
            let ids: BTreeSet<String> =
                features.iter().filter_map(|feature| feature_id_to_string(&feature.id)).collect();
            assert!(!ids.is_empty(), "{source_layer} has no features with ids");
            for (index, id) in ids.iter().enumerate() {
                let mut state = ValueObject::new();
                state.insert("selected".to_string(), Value::Bool(index % 2 == 0));
                renderer.set_feature_state(source, Some(source_layer), id, &state);
            }
        });
    }

    /// Sets `selected` on a feature through the map, as an app does: the
    /// map sends its state to the renderer with its next update.
    pub(crate) fn set_map_feature_state(&mut self, source: &str, id: &str, selected: bool) {
        let mut state = ValueObject::new();
        state.insert("selected".to_string(), Value::Bool(selected));
        self.map.set_feature_state(source, None, id, state);
    }

    /// Runs `change` with the map's updates held, so its changes reach the
    /// renderer in one update.
    pub(crate) fn held(&mut self, change: impl FnOnce(&mut Scene)) {
        self.map.hold_updates();
        change(self);
        self.map.release_updates();
    }

    /// Sets the `selected` state of one feature.
    pub(crate) fn select_feature(&self, source: &str, source_layer: &str, id: &str, selected: bool) {
        self.frontend.with_renderer(|renderer| {
            let mut state = ValueObject::new();
            state.insert("selected".to_string(), Value::Bool(selected));
            renderer.set_feature_state(source, Some(source_layer), id, &state);
        });
    }

    /// The ids of the loaded features of a source layer.
    pub(crate) fn feature_ids(&self, source: &str, source_layer: &str) -> Vec<String> {
        self.frontend.with_renderer(|renderer| {
            let options = SourceQueryOptions::new(Some(vec![source_layer.to_string()]), None);
            let features = renderer.query_source_features(source, &options).expect("the source's features");
            let ids: BTreeSet<String> =
                features.iter().filter_map(|feature| feature_id_to_string(&feature.id)).collect();
            ids.into_iter().collect()
        })
    }

    /// Whether the source `source` has features loaded, each with the
    /// number `value` as its property `key`.
    pub(crate) fn source_shows(&self, source: &str, key: &str, value: u64) -> bool {
        self.frontend.with_renderer(|renderer| {
            let options = SourceQueryOptions::new(None, None);
            let features = renderer.query_source_features(source, &options).expect("the source's features");
            !features.is_empty()
                && features.iter().all(|feature| match feature.properties.get(key) {
                    Some(Value::Uint(number)) => *number == value,
                    Some(Value::Int(number)) => u64::try_from(*number) == Ok(value),
                    _ => false,
                })
        })
    }

    /// The GPU's description, and whether it is a software rasterizer.
    pub fn gpu(&self) -> (String, bool) {
        let backend = self.frontend.get_backend();
        #[cfg(target_vendor = "apple")]
        if let Device::Metal(metal) = self.frontend.device() {
            return (format!("{} (Metal)", metal.name()), false);
        }
        #[cfg(not(any(target_vendor = "apple", target_family = "wasm")))]
        if let Some(vulkan) = backend.downcast_ref::<mln_vulkan::RendererBackend>() {
            return (format!("{} (Vulkan)", vulkan.get_device_name()), vulkan.is_software_device());
        }
        if let Some(gl) = backend.downcast_ref::<mln_gl::RendererBackend>() {
            let _scope = mln_gfx::BackendScope::new(gl, mln_gfx::ScopeType::Explicit);
            let name = gl.get_renderer_name();
            let software = ["llvmpipe", "softpipe", "swiftshader"].iter().any(|s| name.to_lowercase().contains(s));
            return (format!("{name} (OpenGL ES)"), software);
        }
        let Device::WebGpu(device) = self.frontend.device() else {
            return ("no adapter".to_string(), true);
        };
        let info = device.adapter_info();
        let name = info.name.to_lowercase();
        let software = info.device_type == wgpu::DeviceType::Cpu
            || ["llvmpipe", "lavapipe", "swiftshader", "software", "warp"].iter().any(|s| name.contains(s));
        (format!("{} ({:?}, {} {})", info.name, info.backend, info.driver, info.driver_info), software)
    }
}

/// Writes `image` as `<dir>/<name>.png`, the name's slashes as dashes.
pub(crate) fn write_frame(dir: &Path, name: &str, image: &PremultipliedImage) {
    std::fs::create_dir_all(dir).expect("the frames directory");
    let path = dir.join(format!("{}.png", name.replace('/', "-")));
    std::fs::write(&path, mln_image::png_writer::encode_png(image))
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
}
