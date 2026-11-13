// Test support for the ported test/map and test/api gtests (gtests-core).
//
// Ports, for the tests in this directory:
// - test/src/mln/test/util.cpp: `checkImage` / `getImageDiff` (pixelmatch
//   against `<base>/expected.png`; actual and diff images go to
//   target/tmp/<base>/, never next to the fixture);
// - test/include/mln/test/stub_map_observer.hpp (`StubMapObserver`, which
//   gets the map's events as C++ observers get the calls);
// - test/include/mln/test/fake_file_source.hpp (`FakeFileSource`);
// - test/include/mln/test/fixture_log_observer.hpp (`FixtureLog`, per
//   thread: Rust runs tests on parallel threads of one process);
// - the `MapTest` fixture of test/map/map.test.cpp (C++ `MapAdapter` is
//   `Map::with_file_source`).
//
// Fixtures are read from the copy of the C++ repository's files in
// testdata/; paths are written as in C++, relative to the maplibre-native
// root ("test/fixtures/api/empty.json").

#![expect(dead_code, reason = "each test binary uses some of the shared helpers")]

use std::cell::{Cell, RefCell, RefMut};
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::{Arc, OnceLock};

use mln_actor::{AsyncTask, LocalObject, RunLoop, Timer};
use mln_headless::{HeadlessFrontend, RenderResult};
use mln_map::{Map, MapEvent, MapMode, MapOptions, RenderFrameStatus};
use mln_renderer::{FontStack, GlyphRange, OverscaledTileID, TileOperation};
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_storage::{AsyncRequest, Callback, FileSource, Kind, Resource, ResourceOptions, Response};
use mln_style::custom_tile_loader::TileFunction;
use mln_style::sources::custom_geometry_source::CustomGeometrySourceHandle;
use mln_style::sprite::Sprite;
use mln_tile_id::CanonicalTileID;
use mln_util::Error;
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, EventSeverity, Log};
use parking_lot::Mutex;

// MARK: - Fixtures

/// testdata/, the copy of the C++ repository's files these tests read.
pub(crate) fn cpp_root() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata")
}

/// Reads the file at `path`, relative to [`cpp_root`].
pub(crate) fn read_bytes(path: &str) -> Vec<u8> {
    let full = cpp_root().join(path);
    std::fs::read(&full).unwrap_or_else(|e| panic!("{}: {e}", full.display()))
}

pub(crate) fn read_file(path: &str) -> String {
    String::from_utf8(read_bytes(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

pub(crate) fn decode_image(path: &str) -> PremultipliedImage {
    mln_image::decode_image(&read_bytes(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// A `Response` with `data` = the fixture's bytes.
pub(crate) fn file_response(path: &str) -> Response {
    let mut response = Response::new();
    response.data = Some(Arc::new(read_bytes(path)));
    response
}

// MARK: - checkImage (test/src/mln/test/util.cpp)

fn write_output(base: &str, name: &str, image: &PremultipliedImage) {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(base);
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::write(dir.join(name), mln_image::encode_png(image));
    }
}

pub(crate) fn get_image_diff(base: &str, actual: &PremultipliedImage, pixel_threshold: f64) -> f64 {
    let expected = decode_image(&format!("{base}/expected.png"));
    let mut diff = PremultipliedImage::new(expected.size);

    write_output(base, "actual.png", actual);

    if expected.size != actual.size {
        return 1.0;
    }

    let pixels = pixelmatch::pixelmatch(
        &actual.data,
        &expected.data,
        expected.size.width as usize,
        expected.size.height as usize,
        Some(&mut diff.data),
        pixel_threshold,
        false,
    );

    write_output(base, "diff.png", &diff);

    pixels as f64 / (f64::from(expected.size.width) * f64::from(expected.size.height))
}

/// Asserts that `actual` matches the expected image of one of `bases`.
#[track_caller]
pub(crate) fn check_images(bases: &[&str], actual: &PremultipliedImage, image_threshold: f64, pixel_threshold: f64) {
    let diffs: Vec<f64> = bases.iter().map(|base| get_image_diff(base, actual, pixel_threshold)).collect();
    assert!(
        diffs.iter().any(|diff| *diff <= image_threshold),
        "{bases:?}: image differences {diffs:?}, none <= {image_threshold} (pixel threshold {pixel_threshold})"
    );
}

/// Asserts that `actual` differs from `<base>/expected.png` by at most
/// `image_threshold`. The C++ defaults for both thresholds are 0.
#[track_caller]
pub(crate) fn check_image(base: &str, actual: &PremultipliedImage, image_threshold: f64, pixel_threshold: f64) {
    let diff = get_image_diff(base, actual, pixel_threshold);
    assert!(
        diff <= image_threshold,
        "{base}: image difference {diff} > {image_threshold} (pixel threshold {pixel_threshold})"
    );
}

/// The channel values of the centre pixel (C++ `image.data.get() +
/// (height / 2) * stride + (width / 2) * channels`).
pub(crate) fn center_pixel(image: &PremultipliedImage) -> [u8; 4] {
    let offset = (image.size.height as usize / 2) * image.stride() + (image.size.width as usize / 2) * 4;
    [image.data[offset], image.data[offset + 1], image.data[offset + 2], image.data[offset + 3]]
}

// MARK: - Custom geometry sources

/// A custom geometry source's fetch function that answers with `features`
/// through the handle in `handle`, once it's set, as the C++ tests answer
/// through `getSource("custom")`.
pub(crate) fn answer_with(
    handle: &Arc<OnceLock<CustomGeometrySourceHandle>>,
    features: impl Fn(&CanonicalTileID) -> Vec<geojson::Feature> + Send + Sync + 'static,
) -> TileFunction {
    let handle = Arc::clone(handle);
    Arc::new(move |tile_id: &CanonicalTileID| {
        if let Some(handle) = handle.get() {
            handle.set_tile_data(*tile_id, geojson::GeoJSON::FeatureCollection(features(tile_id)));
        }
    })
}

// MARK: - ASSERT_DOUBLE_EQ

/// gtest's `DoubleNear`-free `ASSERT_DOUBLE_EQ`: within 4 ULPs.
#[track_caller]
pub(crate) fn assert_double_eq(actual: f64, expected: f64) {
    fn biased(x: f64) -> u64 {
        const SIGN: u64 = 1 << 63;
        let bits = x.to_bits();
        if bits & SIGN != 0 { (!bits).wrapping_add(1) } else { bits | SIGN }
    }
    let ok = if actual.is_nan() || expected.is_nan() {
        false
    } else {
        let (a, b) = (biased(actual), biased(expected));
        a.abs_diff(b) <= 4
    };
    assert!(ok, "expected {expected} (4 ULPs), got {actual}");
}

/// `EXPECT_NEAR(a, b, tolerance)`.
#[track_caller]
pub(crate) fn assert_near(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "expected {expected} +- {tolerance}, got {actual} (difference {})",
        (actual - expected).abs()
    );
}

// MARK: - StubMapObserver (test/include/mln/test/stub_map_observer.hpp)

/// A callback in a `CallbackSlot`.
type SlotCallback<A> = Box<dyn FnMut(A)>;

/// A `std::function` member of `StubMapObserver`. A callback may replace or
/// clear any slot, including its own, while it runs.
pub(crate) struct CallbackSlot<A> {
    callback: RefCell<Option<SlotCallback<A>>>,
    generation: Cell<u64>,
}

impl<A> Default for CallbackSlot<A> {
    fn default() -> Self {
        CallbackSlot { callback: RefCell::new(None), generation: Cell::new(0) }
    }
}

impl<A> CallbackSlot<A> {
    pub(crate) fn set(&self, callback: impl FnMut(A) + 'static) {
        *self.callback.borrow_mut() = Some(Box::new(callback));
        self.generation.set(self.generation.get() + 1);
    }

    pub(crate) fn clear(&self) {
        *self.callback.borrow_mut() = None;
        self.generation.set(self.generation.get() + 1);
    }

    pub(crate) fn call(&self, arg: A) {
        let generation = self.generation.get();
        let callback = self.callback.borrow_mut().take();
        if let Some(mut callback) = callback {
            callback(arg);
            if self.generation.get() == generation {
                *self.callback.borrow_mut() = Some(callback);
            }
        }
    }
}

/// A map observer that forwards each event to a settable callback,
/// including the source and sprite events.
#[derive(Default)]
pub(crate) struct StubMapObserver {
    pub will_start_loading_map_callback: CallbackSlot<()>,
    pub did_finish_loading_map_callback: CallbackSlot<()>,
    pub did_fail_loading_map_callback: CallbackSlot<()>,
    pub did_finish_loading_style_callback: CallbackSlot<()>,
    pub did_finish_rendering_frame_callback: CallbackSlot<RenderFrameStatus>,
    pub did_become_idle_callback: CallbackSlot<()>,
    pub on_style_image_missing_callback: CallbackSlot<String>,
    pub on_glyphs_loaded_callback: CallbackSlot<(FontStack, GlyphRange)>,
    pub on_glyphs_error_callback: CallbackSlot<(FontStack, GlyphRange, Error)>,
    pub on_glyphs_requested_callback: CallbackSlot<(FontStack, GlyphRange)>,
    pub on_tile_action_callback: CallbackSlot<(TileOperation, OverscaledTileID, String)>,
    pub on_source_changed_callback: CallbackSlot<String>,
    pub on_sprite_loaded_callback: CallbackSlot<Option<Sprite>>,
    pub on_sprite_error_callback: CallbackSlot<(Option<Sprite>, Error)>,
    pub on_sprite_requested_callback: CallbackSlot<Option<Sprite>>,
}

impl StubMapObserver {
    /// Calls the callback of `event`.
    pub(crate) fn dispatch(&self, event: MapEvent) {
        match event {
            MapEvent::LoadingStarted => self.will_start_loading_map_callback.call(()),
            MapEvent::LoadingFinished => self.did_finish_loading_map_callback.call(()),
            MapEvent::LoadingFailed(_) => self.did_fail_loading_map_callback.call(()),
            MapEvent::StyleLoaded => self.did_finish_loading_style_callback.call(()),
            MapEvent::RenderFrameFinished(status) => self.did_finish_rendering_frame_callback.call(status),
            MapEvent::Idle => self.did_become_idle_callback.call(()),
            MapEvent::StyleImageMissing { id } => self.on_style_image_missing_callback.call(id),
            MapEvent::GlyphsLoaded { font_stack, range } => self.on_glyphs_loaded_callback.call((font_stack, range)),
            MapEvent::GlyphsError { font_stack, range, error } => {
                self.on_glyphs_error_callback.call((font_stack, range, error));
            }
            MapEvent::GlyphsRequested { font_stack, range } => {
                self.on_glyphs_requested_callback.call((font_stack, range));
            }
            MapEvent::TileAction { op, id, source_id } => self.on_tile_action_callback.call((op, id, source_id)),
            MapEvent::SourceChanged { source_id } => self.on_source_changed_callback.call(source_id),
            MapEvent::SpriteLoaded(sprite) => self.on_sprite_loaded_callback.call(sprite),
            MapEvent::SpriteError(sprite, error) => self.on_sprite_error_callback.call((sprite, error)),
            MapEvent::SpriteRequested(sprite) => self.on_sprite_requested_callback.call(sprite),
            _ => {}
        }
    }

    /// Takes the map's events and calls their callbacks, which may use the
    /// map. Does nothing while someone holds the map: they dispatch the
    /// events when they let go of it.
    pub(crate) fn dispatch_events(&self, map: &RefCell<Map>) {
        let Ok(mut map) = map.try_borrow_mut() else {
            return;
        };
        let events = map.drain_events();
        drop(map);
        for event in events {
            self.dispatch(event);
        }
    }

    /// Dispatches `map`'s events from the run loop whenever the map has
    /// some, while the returned task lives.
    pub(crate) fn observe(self: &Rc<Self>, map: &Rc<RefCell<Map>>) -> AsyncTask {
        let task = {
            let observer = Rc::downgrade(self);
            let map = Rc::downgrade(map);
            AsyncTask::new(move || {
                if let (Some(observer), Some(map)) = (observer.upgrade(), map.upgrade()) {
                    observer.dispatch_events(&map);
                }
            })
        };
        let sender = task.sender();
        map.borrow_mut().set_waker(Some(Arc::new(move || sender.send())));
        task
    }
}

// MARK: - FakeFileSource (test/include/mln/test/fake_file_source.hpp)

struct FakeRequestEntry {
    id: u64,
    resource: Resource,
    callback: Rc<RefCell<Callback>>,
}

type FakeRequests = RefCell<Vec<FakeRequestEntry>>;

/// A request of a [`FakeFileSource`]: removes itself from the list when
/// dropped.
struct FakeFileRequest {
    id: u64,
    list: Weak<FakeRequests>,
}

impl AsyncRequest for FakeFileRequest {}

impl Drop for FakeFileRequest {
    fn drop(&mut self) {
        if let Some(list) = self.list.upgrade() {
            list.borrow_mut().retain(|entry| entry.id != self.id);
        }
    }
}

/// Records the requests made; the test responds to them.
pub(crate) struct FakeFileSource {
    requests: LocalObject<FakeRequests>,
    next_id: Mutex<u64>,
    resource_options: Mutex<ResourceOptions>,
}

impl FakeFileSource {
    /// A file source with the default resource options.
    pub(crate) fn new() -> Self {
        FakeFileSource {
            requests: LocalObject::new(RefCell::new(Vec::new())),
            next_id: Mutex::new(0),
            resource_options: Mutex::new(ResourceOptions::default()),
        }
    }

    fn list(&self) -> Rc<FakeRequests> {
        self.requests.get().expect("FakeFileSource used from another thread")
    }

    pub(crate) fn requests_len(&self) -> usize {
        self.list().borrow().len()
    }

    /// `respond(kind, response)`: answers the first pending request of that
    /// kind; returns whether there was one.
    pub(crate) fn respond(&self, kind: Kind, response: &Response) -> bool {
        let callback = {
            let list = self.list();
            let list = list.borrow();
            list.iter().find(|entry| entry.resource.kind == kind).map(|entry| Rc::clone(&entry.callback))
        };
        match callback {
            // Copy the callback, in case calling it deallocates the AsyncRequest.
            Some(callback) => {
                (callback.borrow_mut())(response.clone());
                true
            }
            None => false,
        }
    }
}

impl Default for FakeFileSource {
    fn default() -> Self {
        Self::new()
    }
}

impl FileSource for FakeFileSource {
    fn request(&self, resource: &Resource, callback: Callback) -> Box<dyn AsyncRequest> {
        let id = {
            let mut next = self.next_id.lock();
            *next += 1;
            *next
        };
        let list = self.list();
        list.borrow_mut().push(FakeRequestEntry {
            id,
            resource: resource.clone(),
            callback: Rc::new(RefCell::new(callback)),
        });
        Box::new(FakeFileRequest { id, list: Rc::downgrade(&list) })
    }

    fn can_request(&self, _resource: &Resource) -> bool {
        true
    }

    fn set_resource_options(&self, options: ResourceOptions) {
        *self.resource_options.lock() = options;
    }

    fn get_resource_options(&self) -> ResourceOptions {
        self.resource_options.lock().clone()
    }
}

// MARK: - FixtureLog (test/include/mln/test/fixture_log_observer.hpp)

type LogRecord = (EventSeverity, Event, i64, String);
type LogRecords = Arc<Mutex<Vec<(LogRecord, bool)>>>;

struct Recorder(LogRecords);

impl mln_util::logging::Observer for Recorder {
    fn on_record(&self, severity: EventSeverity, event: Event, code: i64, msg: &str) -> bool {
        self.0.lock().push(((severity, event, code, msg.to_owned()), false));
        true
    }
}

/// Captures the log records of the calling thread while it lives (C++
/// installs a process-wide observer).
pub(crate) struct FixtureLog {
    messages: LogRecords,
}

impl FixtureLog {
    pub(crate) fn new() -> Self {
        let messages: LogRecords = Arc::new(Mutex::new(Vec::new()));
        let _ = Log::set_thread_observer(Some(Box::new(Recorder(Arc::clone(&messages)))));
        FixtureLog { messages }
    }

    /// Counts the matching records, and marks them checked.
    pub(crate) fn count(&self, severity: EventSeverity, event: Event, code: i64, msg: &str) -> usize {
        let mut count = 0;
        for ((s, e, c, m), checked) in self.messages.lock().iter_mut() {
            if *s == severity && *e == event && *c == code && m == msg {
                *checked = true;
                count += 1;
            }
        }
        count
    }

    /// The records nobody counted.
    pub(crate) fn unchecked(&self) -> Vec<LogRecord> {
        self.messages.lock().iter().filter(|(_, checked)| !checked).map(|(record, _)| record.clone()).collect()
    }
}

impl Default for FixtureLog {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for FixtureLog {
    fn drop(&mut self) {
        let _ = Log::set_thread_observer(None);
    }
}

// MARK: - RunLoop with an emergency stop

/// Runs `run_loop`, stopping it and failing after `seconds` if nothing
/// stopped it: a hang fails the test instead of hanging the test run.
#[track_caller]
pub(crate) fn run_with_timeout(run_loop: &RunLoop, seconds: u64) {
    let timed_out = Rc::new(Cell::new(false));
    let timer = Timer::new();
    {
        let timed_out = Rc::clone(&timed_out);
        let run_loop = run_loop.clone();
        timer.start(std::time::Duration::from_secs(seconds), std::time::Duration::ZERO, move || {
            timed_out.set(true);
            run_loop.stop();
        });
    }
    run_loop.run();
    timer.stop();
    assert!(!timed_out.get(), "the run loop was not stopped within {seconds} s");
}

// MARK: - MapTest (test/map/map.test.cpp)

/// A map with its frontend, observer and file source. Fields drop in
/// declaration order, which is the C++ destruction order (reverse of
/// declaration there). The map is shared (`Rc<RefCell<..>>`) so observer
/// callbacks can reach it through a `Weak` as C++ lambdas capture `test`.
///
/// [`map`](Self::map) hands out the map once it has handled what its style
/// and renderer reported, and the observer gets the map's events when the
/// call through it, or through [`render`](Self::render), returns, as C++
/// observers get the calls at once. Otherwise they come from the run loop.
pub(crate) struct MapTest<FS: FileSource + 'static = StubFileSource> {
    events: AsyncTask,
    pub map: Rc<RefCell<Map>>,
    pub frontend: HeadlessFrontend,
    pub observer: Rc<StubMapObserver>,
    pub file_source: Arc<FS>,
    pub run_loop: RunLoop,
}

impl MapTest<StubFileSource> {
    /// A test at pixel ratio 1, in `MapMode::Static`.
    pub(crate) fn new() -> Self {
        Self::with_mode(1.0, MapMode::Static)
    }

    pub(crate) fn with_mode(pixel_ratio: f32, mode: MapMode) -> Self {
        let run_loop = RunLoop::new();
        let file_source = Arc::new(StubFileSource::new(ResponseType::Asynchronous));
        Self::create(run_loop, file_source, pixel_ratio, mode)
    }
}

impl MapTest<FakeFileSource> {
    pub(crate) fn fake() -> Self {
        let run_loop = RunLoop::new();
        let file_source = Arc::new(FakeFileSource::new());
        Self::create(run_loop, file_source, 1.0, MapMode::Static)
    }
}

impl<FS: FileSource + 'static> MapTest<FS> {
    /// The shared constructor: `fileSource`, `frontend(pixelRatio)`,
    /// `map(frontend, observer, fileSource, MapOptions().withMapMode(mode)
    /// .withSize(frontend.getSize()).withPixelRatio(pixelRatio))`.
    pub(crate) fn create(run_loop: RunLoop, file_source: Arc<FS>, pixel_ratio: f32, mode: MapMode) -> Self {
        let observer = Rc::new(StubMapObserver::default());
        let frontend = HeadlessFrontend::with_pixel_ratio(pixel_ratio).expect("a headless frontend");
        let options = MapOptions { map_mode: mode, size: frontend.get_size(), pixel_ratio, ..Default::default() };
        let map = Map::with_file_source(Arc::<FS>::clone(&file_source), options).unwrap();
        frontend.attach(&map);
        let map = Rc::new(RefCell::new(map));
        let events = observer.observe(&map);
        MapTest { events, map, frontend, observer, file_source, run_loop }
    }

    /// `test.map`: the map, whose events reach the observer when the
    /// guard drops.
    pub(crate) fn map(&self) -> MapGuard<'_> {
        let mut map = self.map.borrow_mut();
        map.process_events();
        MapGuard { map: Some(map), cell: &self.map, observer: &self.observer }
    }

    /// A handle for callbacks (no reference cycle through the observer).
    pub(crate) fn weak_map(&self) -> Weak<RefCell<Map>> {
        Rc::downgrade(&self.map)
    }

    /// `test.frontend.render(test.map)`; C++ rethrows a still image error.
    pub(crate) fn render(&self) -> RenderResult {
        self.try_render().unwrap_or_else(|e| panic!("render: {e}"))
    }

    pub(crate) fn try_render(&self) -> Result<RenderResult, Error> {
        let mut frontend = self.frontend.clone();
        frontend.render(&mut self.map())
    }

    pub(crate) fn load_json(&self, json: &str) {
        self.map().style_mut().load_json(json);
    }
}

/// The map of a [`MapTest`], borrowed. When it drops, the observer gets the
/// map's events.
pub(crate) struct MapGuard<'a> {
    map: Option<RefMut<'a, Map>>,
    cell: &'a RefCell<Map>,
    observer: &'a StubMapObserver,
}

impl Deref for MapGuard<'_> {
    type Target = Map;
    fn deref(&self) -> &Map {
        self.map.as_deref().expect("the map")
    }
}

impl DerefMut for MapGuard<'_> {
    fn deref_mut(&mut self) -> &mut Map {
        self.map.as_deref_mut().expect("the map")
    }
}

impl Drop for MapGuard<'_> {
    fn drop(&mut self) {
        drop(self.map.take());
        self.observer.dispatch_events(self.cell);
    }
}
