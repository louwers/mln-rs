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
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use mln_engine::exec::pool::Pool;
use mln_engine::exec::queue::Queue;
use mln_engine::map::{Map, MapEvent, MapMode, MapOptions, RenderFrameStatus};
use mln_engine::storage::stub_file_source::{ResponseType, StubFileSource};
use mln_engine::storage::{FileSource, Kind, Resource, Responder, Response};
use mln_engine::tile::OverscaledTileID;
use mln_engine::{FontStack, GlyphRange, TileOperation, UpdateReader};
use mln_headless::{HeadlessFrontend, RenderResult};
use mln_style::bitmap::PremultipliedImage;
use mln_style::error::Error;
use mln_style::sprite::Sprite;
use parking_lot::{Mutex, MutexGuard};

// MARK: - Fixtures

/// testdata/, the copy of the C++ repository's files these tests read.
pub(crate) fn cpp_root() -> PathBuf {
    std::path::Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("testdata")
}

/// Reads the file at `path`, relative to [`cpp_root`].
pub(crate) fn read_bytes(path: &str) -> Vec<u8> {
    let full = cpp_root().join(path);
    std::fs::read(&full).unwrap_or_else(|e| panic!("{}: {e}", full.display()))
}

pub(crate) fn read_file(path: &str) -> String {
    String::from_utf8(read_bytes(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// A GeoJSON source `id` with `options` and the data of `geojson`.
pub(crate) fn geojson_source_with_data(
    id: &str,
    options: Arc<mln_style::sources::GeoJSONOptions>,
    geojson: geojson::GeoJson,
) -> mln_style::Source {
    use mln_style::source::SourceData;
    use mln_style::sources::geojson::{GeoJsonSourceOptions, GeoJsonSourceType};
    let data = mln_builtins::sources::geojson::create_geojson_data(geojson, &options).expect("GeoJSON data");
    let options = GeoJsonSourceOptions::new(options);
    mln_style::Source::new(id, SourceData::<GeoJsonSourceType>::with_description(options, Arc::new(data)))
}

/// Replaces the data of `map`'s GeoJSON source `id` with `geojson`.
pub(crate) fn set_geojson(map: &mut Map, id: &str, geojson: geojson::GeoJson) {
    use mln_style::source::SourceData;
    use mln_style::sources::geojson::GeoJsonSourceType;
    let source = map.source(id).expect("the source");
    let options = &SourceData::<GeoJsonSourceType>::recover(source).expect("a GeoJSON source").options.options;
    let data = mln_builtins::sources::geojson::create_geojson_data(geojson, options).expect("GeoJSON data");
    let edit = mln_builtins::sources::geojson::set_data(id, Some(data));
    map.edit_style(|style| style.apply(edit)).expect("a GeoJSON source");
}

pub(crate) fn decode_image(path: &str) -> PremultipliedImage {
    mln_engine::image::decode_image(&read_bytes(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// A `Response` with `data` = the fixture's bytes.
pub(crate) fn file_response(path: &str) -> Response {
    let mut response = Response::new();
    response.data = Some(Arc::new(read_bytes(path)));
    response
}

// MARK: - checkImage (test/src/mln/test/util.cpp)

fn write_output(base: &str, name: &str, image: &PremultipliedImage) {
    let dir = mln_headless::test_output_dir(env!("CARGO_TARGET_TMPDIR")).join(base);
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::write(dir.join(name), mln_engine::image::encode_png(image));
    }
}

pub(crate) fn get_image_diff(base: &str, actual: &PremultipliedImage, pixel_threshold: f64) -> f64 {
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

    /// Dispatches `map`'s events in the turns of `run_loop` whenever the map
    /// has some, while the returned value lives. With a `frontend`, which
    /// renders `map`, it also draws each update that the map publishes
    /// first, as a window's frontend draws when the display asks for a
    /// frame, and repaints the map when it changes next on its own
    /// ([`Map::repaint_at`]); while someone else holds the frontend, they
    /// draw.
    pub(crate) fn observe(
        self: &Rc<Self>,
        run_loop: &TestLoop,
        map: &Rc<RefCell<Map>>,
        frontend: Option<&Rc<RefCell<HeadlessFrontend>>>,
    ) -> Observing {
        let turn: Rc<dyn Fn()> = {
            let observer = Rc::downgrade(self);
            let map = Rc::downgrade(map);
            let frontend = frontend.map(Rc::downgrade);
            let queue = run_loop.handle();
            // The repaint the loop wakes for.
            let woken_at = Cell::new(None);
            Rc::new(move || {
                if let Some(frontend) = frontend.as_ref().and_then(Weak::upgrade)
                    && let Ok(mut frontend) = frontend.try_borrow_mut()
                {
                    frontend.render_update();
                    if let Some(map) = map.upgrade()
                        && let Ok(mut map) = map.try_borrow_mut()
                        && let Some(at) = map.repaint_at()
                    {
                        if mln_style::time::Instant::now() >= at {
                            map.trigger_repaint();
                        } else if woken_at.replace(Some(at)) != Some(at) {
                            queue.post_at(at, |()| {});
                        }
                    }
                }
                if let (Some(observer), Some(map)) = (observer.upgrade(), map.upgrade()) {
                    observer.dispatch_events(&map);
                }
            })
        };
        run_loop.each_turn(&turn);
        map.borrow_mut().set_waker(Some(run_loop.waker()));
        let updates = frontend.map(|_| {
            let updates = map.borrow().update_reader();
            updates.set_waker(Some(run_loop.waker()));
            // The update that the map published already.
            run_loop.queue().wake();
            updates
        });
        Observing { _turn: turn, _updates: updates }
    }
}

/// What [`StubMapObserver::observe`] runs in the loop's turns, while it
/// lives.
pub(crate) struct Observing {
    _turn: Rc<dyn Fn()>,
    /// The map's updates, whose waker wakes the loop.
    _updates: Option<UpdateReader>,
}

// MARK: - FakeFileSource (test/include/mln/test/fake_file_source.hpp)

struct FakeRequestEntry {
    resource: Resource,
    responder: Responder,
}

/// Records the requests made; the test responds to them.
pub(crate) struct FakeFileSource {
    requests: Mutex<Vec<FakeRequestEntry>>,
    /// Every request made, open or cancelled since.
    loads: AtomicUsize,
}

impl FakeFileSource {
    pub(crate) fn new() -> Self {
        FakeFileSource { requests: Mutex::new(Vec::new()), loads: AtomicUsize::new(0) }
    }

    /// The number of requests made, including those cancelled since.
    pub(crate) fn loads(&self) -> usize {
        self.loads.load(Ordering::SeqCst)
    }

    /// The requests that weren't cancelled.
    fn list(&self) -> MutexGuard<'_, Vec<FakeRequestEntry>> {
        let mut list = self.requests.lock();
        list.retain(|entry| !entry.responder.is_cancelled());
        list
    }

    pub(crate) fn requests_len(&self) -> usize {
        self.list().len()
    }

    /// `respond(kind, response)`: answers the first pending request of that
    /// kind; returns whether there was one.
    pub(crate) fn respond(&self, kind: Kind, response: &Response) -> bool {
        let responder =
            self.list().iter().find(|entry| entry.resource.kind == kind).map(|entry| entry.responder.clone());
        match responder {
            Some(responder) => {
                responder.send(response.clone());
                true
            }
            None => false,
        }
    }

    /// Answers every pending request of `kind`; returns how many there were.
    pub(crate) fn respond_all(&self, kind: Kind, response: &Response) -> usize {
        let responders: Vec<Responder> = self
            .list()
            .iter()
            .filter(|entry| entry.resource.kind == kind)
            .map(|entry| entry.responder.clone())
            .collect();
        for responder in &responders {
            responder.send(response.clone());
        }
        responders.len()
    }
}

impl Default for FakeFileSource {
    fn default() -> Self {
        Self::new()
    }
}

impl FileSource for FakeFileSource {
    fn load(&self, resource: &Resource, responder: Responder) {
        self.loads.fetch_add(1, Ordering::SeqCst);
        self.list().push(FakeRequestEntry { resource: resource.clone(), responder });
    }

    fn can_request(&self, _resource: &Resource) -> bool {
        true
    }
}

// MARK: - FixtureLog (test/include/mln/test/fixture_log_observer.hpp)

/// A caught record: its level, target and message.
type LogRecord = (log::Level, String, String);
type LogRecords = Arc<Mutex<Vec<(LogRecord, bool)>>>;

thread_local! {
    /// The records of the thread's live `FixtureLog`, if any.
    static CAUGHT: RefCell<Option<LogRecords>> = const { RefCell::new(None) };
}

/// The crates whose records describe the machine's GPU, which no test checks.
const GRAPHICS_CRATES: [&str; 6] = ["mln_gfx", "mln_gl", "mln_webgpu", "mln_vulkan", "mln_mtl", "mln_d3d12"];

/// The test binary's `log` logger. It hands each of the library's records,
/// but the graphics backends', to the `FixtureLog` of the thread that logged
/// it, so tests that run on threads of one process don't see each other's.
struct Catcher;

impl log::Log for Catcher {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        let krate = metadata.target().split("::").next().unwrap_or_default();
        krate.starts_with("mln_") && !GRAPHICS_CRATES.contains(&krate)
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        CAUGHT.with_borrow(|caught| {
            if let Some(records) = caught {
                let message = (record.level(), record.target().to_owned(), record.args().to_string());
                records.lock().push((message, false));
            }
        });
    }

    fn flush(&self) {}
}

/// Captures the log records of the calling thread while it lives (C++
/// installs a process-wide observer).
pub(crate) struct FixtureLog {
    messages: LogRecords,
}

impl FixtureLog {
    pub(crate) fn new() -> Self {
        static INSTALL: std::sync::Once = std::sync::Once::new();
        INSTALL.call_once(|| {
            log::set_logger(&Catcher).expect("the test binary has no other logger");
            log::set_max_level(log::LevelFilter::Info);
        });
        let messages = LogRecords::default();
        CAUGHT.set(Some(Arc::clone(&messages)));
        FixtureLog { messages }
    }

    /// Counts the matching records, and marks them checked.
    pub(crate) fn count(&self, level: log::Level, target: &str, msg: &str) -> usize {
        let mut count = 0;
        for ((l, t, m), checked) in self.messages.lock().iter_mut() {
            if *l == level && t == target && m == msg {
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
        CAUGHT.set(None);
    }
}

// MARK: - The test's loop

/// Work that runs after each turn of a [`TestLoop`] while it lives.
type Turn = Weak<dyn Fn()>;

/// The test thread's loop: a queue of `Send` tasks, which the frontends on
/// the thread run too, and the work of what observes the thread's maps,
/// which runs after each turn. Clones share the loop.
#[derive(Clone, Default)]
pub(crate) struct TestLoop {
    queue: Queue<()>,
    turns: Rc<RefCell<Vec<Turn>>>,
}

impl TestLoop {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The queue, which the thread's frontends run while they wait.
    pub(crate) fn queue(&self) -> &Queue<()> {
        &self.queue
    }

    /// A handle that other threads post to and stop the loop with.
    pub(crate) fn handle(&self) -> Queue<()> {
        self.queue.clone()
    }

    /// Wakes the loop, from any thread.
    pub(crate) fn waker(&self) -> Arc<dyn Fn() + Send + Sync> {
        let queue = self.queue.downgrade();
        Arc::new(move || {
            if let Some(queue) = queue.upgrade() {
                queue.wake();
            }
        })
    }

    /// Runs `turn` after each turn of the loop while it lives.
    pub(crate) fn each_turn(&self, turn: &Rc<dyn Fn()>) {
        self.turns.borrow_mut().push(Rc::downgrade(turn));
    }

    fn run_turns(&self) {
        let turns: Vec<Rc<dyn Fn()>> = {
            let mut turns = self.turns.borrow_mut();
            turns.retain(|turn| turn.strong_count() > 0);
            turns.iter().filter_map(Weak::upgrade).collect()
        };
        for turn in turns {
            turn();
        }
    }

    /// Runs what waits now, then the turns' work. Returns whether a task ran.
    pub(crate) fn run_once(&self) -> bool {
        let ran = self.queue.run_once(&mut ());
        self.run_turns();
        ran
    }

    /// Runs the loop until [`stop`](Self::stop).
    pub(crate) fn run(&self) {
        self.queue.run_with(&mut (), |()| self.run_turns());
    }

    pub(crate) fn stop(&self) {
        self.queue.stop();
    }
}

/// Runs `run_loop` until it stops or `limit` passes, and returns whether it
/// stopped in time.
pub(crate) fn run_within(run_loop: &TestLoop, limit: std::time::Duration) -> bool {
    let timeout = LoopTimer::new(run_loop);
    let timed_out = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let (timed_out, stop) = (Arc::clone(&timed_out), run_loop.handle());
        timeout.start(limit, move || {
            timed_out.store(true, Ordering::SeqCst);
            stop.stop();
        });
    }
    run_loop.run();
    !timed_out.load(Ordering::SeqCst)
}

/// Runs `run_loop`, stopping it and failing after `seconds` if nothing
/// stopped it: a hang fails the test instead of hanging the test run.
#[track_caller]
pub(crate) fn run_with_timeout(run_loop: &TestLoop, seconds: u64) {
    assert!(
        run_within(run_loop, std::time::Duration::from_secs(seconds)),
        "the run loop was not stopped within {seconds} s"
    );
}

/// A wait on the test's loop that runs its callback in the first turn after
/// it ends. Starting it again replaces the wait in progress, and dropping
/// it withdraws it.
pub(crate) struct LoopTimer {
    queue: Queue<()>,
    /// Counts the starts and stops: a wait runs only while it's the last.
    generation: Arc<std::sync::atomic::AtomicU64>,
}

impl LoopTimer {
    pub(crate) fn new(run_loop: &TestLoop) -> Self {
        LoopTimer { queue: run_loop.handle(), generation: Arc::default() }
    }

    pub(crate) fn start(&self, delay: std::time::Duration, callback: impl FnOnce() + Send + 'static) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let current = Arc::clone(&self.generation);
        self.queue.post_after(delay, move |()| {
            if current.load(Ordering::SeqCst) == generation {
                callback();
            }
        });
    }

    pub(crate) fn stop(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }
}

impl Drop for LoopTimer {
    fn drop(&mut self) {
        self.stop();
    }
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
/// observers get the calls at once. Otherwise they come in the loop's turns.
pub(crate) struct MapTest<FS: FileSource + 'static = StubFileSource> {
    events: Observing,
    pub map: Rc<RefCell<Map>>,
    pub frontend: Rc<RefCell<HeadlessFrontend>>,
    pub observer: Rc<StubMapObserver>,
    pub file_source: Arc<FS>,
    pub run_loop: TestLoop,
    pub pool: Pool,
}

impl MapTest<StubFileSource> {
    /// A test at pixel ratio 1, in `MapMode::Static`.
    pub(crate) fn new() -> Self {
        Self::with_mode(1.0, MapMode::Static)
    }

    pub(crate) fn with_mode(pixel_ratio: f32, mode: MapMode) -> Self {
        let run_loop = TestLoop::new();
        let file_source = StubFileSource::on_queue(run_loop.queue(), ResponseType::Asynchronous);
        Self::create(run_loop, file_source, pixel_ratio, mode)
    }
}

impl MapTest<FakeFileSource> {
    pub(crate) fn fake() -> Self {
        let run_loop = TestLoop::new();
        let file_source = Arc::new(FakeFileSource::new());
        Self::create(run_loop, file_source, 1.0, MapMode::Static)
    }
}

impl<FS: FileSource + 'static> MapTest<FS> {
    /// The shared constructor: `fileSource`, `frontend(pixelRatio)`,
    /// `map(frontend, observer, fileSource, MapOptions().withMapMode(mode)
    /// .withSize(frontend.getSize()).withPixelRatio(pixelRatio))`.
    pub(crate) fn create(run_loop: TestLoop, file_source: Arc<FS>, pixel_ratio: f32, mode: MapMode) -> Self {
        let observer = Rc::new(StubMapObserver::default());
        let pool = Pool::new(4);
        let mut frontend =
            HeadlessFrontend::with_pixel_ratio(pixel_ratio, run_loop.queue(), &pool).expect("a headless frontend");
        let options = MapOptions { map_mode: mode, size: frontend.size(), pixel_ratio, ..Default::default() };
        let mut map =
            Map::with_file_source(Arc::<FS>::clone(&file_source), options, mln_builtins::registry(), &pool).unwrap();
        frontend.attach(&mut map);
        let map = Rc::new(RefCell::new(map));
        let frontend = Rc::new(RefCell::new(frontend));
        let events = observer.observe(&run_loop, &map, Some(&frontend));
        MapTest { events, map, frontend, observer, file_source, run_loop, pool }
    }

    /// `test.frontend`, which draws each update of the map in the loop's
    /// turns while nobody holds it.
    pub(crate) fn frontend(&self) -> RefMut<'_, HeadlessFrontend> {
        self.frontend.borrow_mut()
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
        let mut map = self.map();
        let result = self.frontend().render(&mut map);
        // The observer's callbacks, which run as the map goes, may use the
        // frontend.
        drop(map);
        result
    }

    pub(crate) fn load_json(&self, json: &str) {
        self.map().load_style_json(json);
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
