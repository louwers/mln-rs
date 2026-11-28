//! Maps that tests render and query through the runtime's public API: a
//! runtime, a map and its offscreen render session, the fixtures in
//! `testdata/`, resource providers that serve them, and comparisons of the
//! images the session renders with expected ones.

use std::path::PathBuf;
use std::sync::{Arc, mpsc};

use mln_engine::camera::{CameraOptions, CameraUpdate, CameraUpdateMode};
use mln_engine::map::MapMode;
use mln_engine::storage::{Kind, Resource, Response};
use mln_engine::tile::feature::Feature;
use mln_runtime::{
    AttachOptions, BackendType, DemandMode, Device, DeviceOptions, Error, FrameDemand, FrameResult, GlApi, Map,
    MapOptions, RenderSession, RenderTarget, RenderedQueryGeometry, RenderedQueryOptions, ResourceRequest, Runtime,
    RuntimeOptions, SourceQueryOptions,
};
use mln_style::bitmap::PremultipliedImage;
use mln_style::size::Size;
use parking_lot::Mutex;

use super::{COMPLETION_TIMEOUT, EventLog, TestRuntime, result_of, wait};

// MARK: - Fixtures

/// `testdata/`, the files these tests read: a copy of the C++ repository's
/// test fixtures at their paths there ("test/fixtures/api/empty.json").
pub(crate) fn fixtures_root() -> PathBuf {
    PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR")).join("testdata")
}

/// The bytes of the fixture at `path`, relative to [`fixtures_root`].
pub(crate) fn read_bytes(path: &str) -> Vec<u8> {
    let full = fixtures_root().join(path);
    std::fs::read(&full).unwrap_or_else(|e| panic!("{}: {e}", full.display()))
}

pub(crate) fn read_file(path: &str) -> String {
    String::from_utf8(read_bytes(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

pub(crate) fn decode_image(path: &str) -> PremultipliedImage {
    mln_engine::image::decode_image(&read_bytes(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// A response holding the fixture's bytes.
pub(crate) fn file_response(path: &str) -> Response {
    Response { data: Some(Arc::new(read_bytes(path))), ..Response::default() }
}

/// A response that the resource doesn't exist.
pub(crate) fn not_found() -> Response {
    let mut response = Response::new();
    response.error = Some(Box::new(mln_engine::storage::ResourceError::new(
        mln_engine::storage::response::Reason::NotFound,
        "not found",
    )));
    response
}

// MARK: - Images

/// The directory that a test writes its images to: the one that
/// `MLN_TEST_OUTPUT` names, a temporary directory on devices whose tests run
/// from a copy of their package (Android and `OpenHarmony`), or else the
/// test's `CARGO_TARGET_TMPDIR`.
pub(crate) fn test_output_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("MLN_TEST_OUTPUT") {
        return dir.into();
    }
    if cfg!(any(target_os = "android", target_env = "ohos")) {
        let base = std::env::var_os("TMPDIR")
            .filter(|dir| !dir.is_empty())
            .map_or_else(|| PathBuf::from("/data/local/tmp"), PathBuf::from);
        return base.join("mln-test-output");
    }
    env!("CARGO_TARGET_TMPDIR").into()
}

fn write_output(base: &str, name: &str, image: &PremultipliedImage) {
    let dir = test_output_dir().join(base);
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::write(dir.join(name), mln_engine::image::encode_png(image));
    }
}

/// The share of `actual`'s pixels that differ from `<base>/expected.png` by
/// more than `pixel_threshold`, 1 for an image of another size. The actual
/// and diff images go to [`test_output_dir`]`/<base>`.
pub(crate) fn image_diff(base: &str, actual: &PremultipliedImage, pixel_threshold: f64) -> f64 {
    let expected = decode_image(&format!("{base}/expected.png"));
    let mut diff = PremultipliedImage::new(expected.size);
    write_output(base, "actual.png", actual);
    if expected.size != actual.size {
        return 1.0;
    }
    let pixels = image_diff::differing_pixels(
        &actual.data,
        &expected.data,
        expected.size.width,
        expected.size.height,
        pixel_threshold,
        Some(&mut diff.data),
    );
    write_output(base, "diff.png", &diff);
    pixels as f64 / (f64::from(expected.size.width) * f64::from(expected.size.height))
}

/// Asserts that `actual` differs from `<base>/expected.png` in at most
/// `image_threshold` of its pixels.
#[track_caller]
pub(crate) fn check_image(base: &str, actual: &PremultipliedImage, image_threshold: f64, pixel_threshold: f64) {
    let diff = image_diff(base, actual, pixel_threshold);
    assert!(diff <= image_threshold, "{base}: image difference {diff} > {image_threshold}");
}

/// Asserts that `actual` matches the expected image of one of `bases`.
#[track_caller]
pub(crate) fn check_images(bases: &[&str], actual: &PremultipliedImage, image_threshold: f64, pixel_threshold: f64) {
    let diffs: Vec<f64> = bases.iter().map(|base| image_diff(base, actual, pixel_threshold)).collect();
    assert!(diffs.iter().any(|diff| *diff <= image_threshold), "{bases:?}: image differences {diffs:?}");
}

/// How many pixels of `a` differ from those of `b`, an image of the same
/// size, by more than a small threshold, which antialiasing stays within.
pub(crate) fn differing(a: &PremultipliedImage, b: &PremultipliedImage) -> u64 {
    image_diff::differing_pixels(&a.data, &b.data, a.size.width, a.size.height, 0.1285, None)
}

/// The pixel at `x`, `y`.
pub(crate) fn pixel(image: &PremultipliedImage, x: u32, y: u32) -> [u8; 4] {
    let offset = y as usize * image.stride() + x as usize * 4;
    image.data[offset..offset + 4].try_into().expect("four channels")
}

/// The pixel in the image's center.
pub(crate) fn center_pixel(image: &PremultipliedImage) -> [u8; 4] {
    pixel(image, image.size.width / 2, image.size.height / 2)
}

/// Asserts that `actual` is within 4 ULPs of `expected`.
#[track_caller]
pub(crate) fn assert_double_eq(actual: f64, expected: f64) {
    fn biased(x: f64) -> u64 {
        const SIGN: u64 = 1 << 63;
        let bits = x.to_bits();
        if bits & SIGN != 0 { (!bits).wrapping_add(1) } else { bits | SIGN }
    }
    let ok = !actual.is_nan() && !expected.is_nan() && biased(actual).abs_diff(biased(expected)) <= 4;
    assert!(ok, "expected {expected} (4 ULPs), got {actual}");
}

/// Asserts that `actual` is within `tolerance` of `expected`.
#[track_caller]
pub(crate) fn assert_near(actual: f64, expected: f64, tolerance: f64) {
    assert!((actual - expected).abs() <= tolerance, "expected {expected} +- {tolerance}, got {actual}");
}

/// A latitude and longitude that are valid.
pub(crate) fn ll(lat: f64, lon: f64) -> mln_style::geo::LatLng {
    mln_style::geo::LatLng::new(lat, lon).expect("a valid latitude and longitude")
}

// MARK: - Devices

/// The device the tests render on: the backend that `MLN_RENDER_BACKEND`
/// names (`webgpu`, `metal`, `vulkan`, `d3d12`, `gl` or `gl-desktop`), or
/// without it the platform's default.
pub(crate) fn test_device() -> Option<Device> {
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
        Ok("webgpu") => BackendType::WebGPU,
        Ok(other) => panic!("MLN_RENDER_BACKEND names no backend: {other}"),
        Err(_) => return None,
    };
    Some(Device::with_backend(backend, options).expect("a device of MLN_RENDER_BACKEND's backend"))
}

// MARK: - Resource providers

/// The requests that a provider served: each resource's kind and URL.
#[derive(Clone, Default)]
pub(crate) struct Requests(Arc<Mutex<Vec<(Kind, String)>>>);

impl Requests {
    pub(crate) fn push(&self, resource: &Resource) {
        self.0.lock().push((resource.kind, resource.url.clone()));
    }

    pub(crate) fn all(&self) -> Vec<(Kind, String)> {
        self.0.lock().clone()
    }

    /// The URLs of the requests of `kind`.
    pub(crate) fn urls(&self, kind: Kind) -> Vec<String> {
        self.0.lock().iter().filter(|(k, _)| *k == kind).map(|(_, url)| url.clone()).collect()
    }

    pub(crate) fn count(&self, kind: Kind) -> usize {
        self.0.lock().iter().filter(|(k, _)| *k == kind).count()
    }

    pub(crate) fn len(&self) -> usize {
        self.0.lock().len()
    }

    pub(crate) fn clear(&self) {
        self.0.lock().clear();
    }
}

/// Serves every request of `runtime` with what `answer` makes of its
/// resource, and records it.
pub(crate) fn serve(runtime: &Runtime, answer: impl Fn(&Resource) -> Response + Send + Sync + 'static) -> Requests {
    let requests = Requests::default();
    let log = requests.clone();
    wait(runtime.set_resource_provider(move |request: ResourceRequest| {
        log.push(request.resource());
        let response = answer(request.resource());
        let _ = request.respond(response);
    }));
    requests
}

/// Serves each request of `runtime` from the fixture whose path `path`
/// makes of its URL, and a not-found error where it makes none.
pub(crate) fn serve_fixtures(
    runtime: &Runtime,
    path: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
) -> Requests {
    serve(runtime, move |resource| match path(&resource.url) {
        Some(path) if fixtures_root().join(&path).is_file() => file_response(&path),
        _ => not_found(),
    })
}

// MARK: - Maps

/// How [`StillMap::new`] makes its map.
pub(crate) struct StillOptions {
    pub(crate) size: Size,
    pub(crate) pixel_ratio: f32,
    pub(crate) mode: MapMode,
    pub(crate) registry: Option<Arc<mln_engine::Registry>>,
    pub(crate) runtime_options: RuntimeOptions,
    /// Edits the map's options before it is made.
    pub(crate) map: MapOptions,
}

impl Default for StillOptions {
    fn default() -> Self {
        StillOptions {
            size: Size::new(256, 256),
            pixel_ratio: 1.0,
            mode: MapMode::Static,
            registry: None,
            runtime_options: RuntimeOptions::default(),
            // The fonts of the machine would make the images differ between
            // machines: the fixtures' glyphs have every label's.
            map: MapOptions { system_fonts: false, local_ideographs: false, ..MapOptions::default() },
        }
    }
}

/// A runtime, a map on it, and an offscreen render session that draws the
/// map, with the runtime's events recorded.
pub(crate) struct StillMap {
    // The session before the map, and the map before the runtime.
    pub(crate) session: RenderSession,
    pub(crate) map: Map,
    pub(crate) events: EventLog,
    pub(crate) runtime: TestRuntime,
    next_token: std::cell::Cell<u64>,
    frames: Arc<(Mutex<Vec<FrameResult>>, parking_lot::Condvar)>,
}

impl StillMap {
    /// A static 256 by 256 map at pixel ratio 1.
    pub(crate) fn still() -> StillMap {
        StillMap::new(StillOptions::default())
    }

    /// A continuous 256 by 256 map at pixel ratio 1.
    pub(crate) fn continuous() -> StillMap {
        StillMap::new(StillOptions { mode: MapMode::Continuous, ..StillOptions::default() })
    }

    pub(crate) fn new(options: StillOptions) -> StillMap {
        let runtime = TestRuntime(
            match &options.registry {
                Some(registry) => Runtime::with_registry(&options.runtime_options, Arc::clone(registry)),
                None => Runtime::new(&options.runtime_options),
            }
            .expect("a runtime"),
        );
        StillMap::on(runtime, options)
    }

    /// A map as [`new`](Self::new) makes, on `runtime`, which may serve its
    /// resources already.
    pub(crate) fn on(runtime: TestRuntime, options: StillOptions) -> StillMap {
        let events = EventLog::install(&runtime);
        let mut map_options = options.map;
        map_options.map.size = options.size;
        map_options.map.pixel_ratio = options.pixel_ratio;
        map_options.map.map_mode = options.mode;
        let (map, created) = runtime.create_map(map_options).expect("a map");
        result_of(created).expect("the map");
        let frames: Arc<(Mutex<Vec<FrameResult>>, parking_lot::Condvar)> = Arc::default();
        let attach = AttachOptions {
            frame_wake: Some(Arc::new({
                let frames = Arc::clone(&frames);
                move || {
                    frames.1.notify_all();
                }
            })),
            ..AttachOptions::default()
        };
        let target = RenderTarget::Offscreen { size: options.size, device: test_device() };
        let (session, attached) = map.attach(target, attach).expect("a render session");
        result_of(attached).expect("the session attaches");
        StillMap { session, map, events, runtime, next_token: std::cell::Cell::new(1), frames }
    }

    /// Loads the style from JSON text, and waits until it has parsed.
    #[track_caller]
    pub(crate) fn load_json(&self, json: &str) {
        wait(self.map.set_style_json(json));
    }

    /// Loads the style from `url`, and waits until it has parsed.
    #[track_caller]
    pub(crate) fn load_url(&self, url: &str) {
        wait(self.map.set_style_url(url));
    }

    /// Jumps the camera to `camera`.
    #[track_caller]
    pub(crate) fn jump(&self, camera: CameraOptions) {
        let update = CameraUpdate { mode: CameraUpdateMode::Jump, camera, ..CameraUpdate::default() };
        wait(self.map.update_camera(update));
    }

    /// A still image of the static or tile map, once it has loaded what it
    /// needs, or why it failed.
    pub(crate) fn try_render(&self) -> Result<PremultipliedImage, Error> {
        result_of(self.map.request_still_image()?)?;
        result_of(self.session.read_texture()?)
    }

    /// A still image of the static or tile map.
    #[track_caller]
    pub(crate) fn render(&self) -> PremultipliedImage {
        self.try_render().unwrap_or_else(|error| panic!("a still image: {error}"))
    }

    /// The pixels of the frame the session rendered last.
    #[track_caller]
    pub(crate) fn image(&self) -> PremultipliedImage {
        wait(self.session.read_texture())
    }

    /// Demands a frame of a continuous map in `mode`, and waits for its
    /// result.
    #[track_caller]
    pub(crate) fn frame_with(&self, mode: DemandMode) -> FrameResult {
        let token = self.next_token.get();
        self.next_token.set(token + 1);
        self.session.demand_frame(FrameDemand::new(mode, token)).expect("a frame demand");
        let deadline = std::time::Instant::now() + COMPLETION_TIMEOUT;
        let (results, woken) = &*self.frames;
        let mut results = results.lock();
        loop {
            results.extend(self.session.drain_frame_results().expect("the frame results"));
            if let Some(index) = results.iter().position(|result| result.token == Some(token)) {
                return results.remove(index);
            }
            let now = std::time::Instant::now();
            assert!(now < deadline, "the frame didn't finish within {COMPLETION_TIMEOUT:?}");
            woken.wait_for(&mut results, (deadline - now).min(std::time::Duration::from_millis(50)));
        }
    }

    /// A frame of a continuous map, drawn even if nothing changed.
    #[track_caller]
    pub(crate) fn frame(&self) -> FrameResult {
        self.frame_with(DemandMode::Force)
    }

    /// Draws frames of a continuous map until one shows everything loaded
    /// with nothing left to animate. Returns that frame's result.
    #[track_caller]
    pub(crate) fn settle(&self) -> FrameResult {
        for _ in 0..10_000 {
            let result = self.frame_with(DemandMode::WaitForUpdate);
            if result.mode == mln_engine::RenderMode::Full && !result.needs_repaint {
                return result;
            }
        }
        panic!("the map didn't settle");
    }

    /// The features drawn in `geometry` in the last frame.
    #[track_caller]
    pub(crate) fn query(&self, geometry: RenderedQueryGeometry, options: RenderedQueryOptions) -> Vec<Feature> {
        wait(self.session.query_rendered_features(geometry, options))
    }

    /// The features drawn at `x`, `y` in the last frame.
    #[track_caller]
    pub(crate) fn query_point(&self, x: f64, y: f64, options: RenderedQueryOptions) -> Vec<Feature> {
        self.query(RenderedQueryGeometry::Point(mln_engine::geo::ScreenCoordinate::new(x, y)), options)
    }

    /// The features of the source `source_id` in its loaded tiles.
    #[track_caller]
    pub(crate) fn query_source(&self, source_id: &str, options: SourceQueryOptions) -> Vec<Feature> {
        wait(self.session.query_source_features(source_id, options))
    }

    /// A filter parsed from JSON against the map's registry.
    pub(crate) fn filter(&self, json: &str) -> mln_style::filter::Filter {
        let value: serde_json::Value = serde_json::from_str(json).expect("filter JSON");
        let mut error = mln_style::conversion::Error::default();
        mln_style::filter::convert_filter(
            &mln_style::conversion::Convertible::from(&value),
            self.map.registry().functions(),
            &mut error,
        )
        .unwrap_or_else(|| panic!("filter: {}", error.message))
    }
}

/// Waits for the next value from `receiver`, or fails the test after
/// [`COMPLETION_TIMEOUT`].
#[track_caller]
pub(crate) fn recv<T>(receiver: &mpsc::Receiver<T>) -> T {
    receiver.recv_timeout(COMPLETION_TIMEOUT).expect("a value within the timeout")
}
