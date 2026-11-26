// The C++ GlyphManager notifies requestors; the port's GlyphStore answers
// waits that ask it again (`resolve`). The fixture here keeps the waits of
// its requestors, takes the responses from its inbox and asks the store
// again whenever it wakes, as the tile store's update does for waiting
// tiles.

use std::cell::{Cell, RefCell};
use std::num::NonZeroU16;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicUsize, Ordering};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use mln_storage::response::{Reason, ResourceError};
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_storage::{FileSource, Kind, Resource, Response};
use mln_style::bitmap::AlphaImage;
use mln_style::error::Error;
use mln_style::i18n;
use mln_style::size::Size;
use mln_text::glyph::{
    FontData, FontFace, GLYPH_RANGES_PER_FONT_STACK, GLYPHS_PER_GLYPH_RANGE, Glyph, GlyphDependencies, GlyphID,
    GlyphIDType, GlyphIDs, GlyphMap, GlyphMetrics, GlyphRange, NON_IDEOGRAPH_GLYPH_RANGES_PER_FONT_STACK,
    ShapeRequests, get_glyph_range,
};
use mln_text::glyph_cache::GlyphCache;
use mln_text::local_fonts::{FontChooser, LocalGlyphRasterizer, LocalGlyphs};
use mln_text::tagged_string::font_stack_hash;
use mln_text::{GlyphAdjust, ShapedText};
use mln_util::font_stack::FontStack;
use mln_util::pool::{Pool, TaskGroup};
use mln_util::queue::Queue;
use mln_vec_map::{VecMap, VecSet};
use parking_lot::Mutex;

use super::glyph_store_bitmaps::{SDF_BITMAP, STUB_BITMAP};
use super::read_file;
use crate::render::events::RendererEvent;
use crate::text::font_face_engine::{FontFaceEngine, FontFaceShaper};
use crate::text::glyph_store::{GlyphRanges, GlyphStore, GlyphWait, ShapeResults};
use crate::tile_store::inbox::{Inbox, TileEvent};

fn stub_glyph(glyph_id: GlyphID) -> Glyph {
    Glyph {
        id: glyph_id,
        metrics: GlyphMetrics { width: 24, height: 24, left: 0.0, top: -8.0, advance: 24, ..Default::default() },
        bitmap: AlphaImage::from_slice(Size::new(30, 30), &STUB_BITMAP).unwrap(),
    }
}

/// Local fonts with the glyphs of CJK characters
/// (`i18n::allows_fixed_width_glyph_generation`).
struct StubLocalGlyphRasterizer;

impl LocalGlyphRasterizer for StubLocalGlyphRasterizer {
    fn rasterize_glyph(&self, _font_stack: &FontStack, glyph_id: GlyphID) -> Option<Glyph> {
        glyph_id.char().is_some_and(i18n::allows_fixed_width_glyph_generation).then(|| stub_glyph(glyph_id))
    }
}

/// Local fonts with one glyph, katakana テ.
struct KatakanaOnlyLocalGlyphRasterizer;

impl LocalGlyphRasterizer for KatakanaOnlyLocalGlyphRasterizer {
    fn rasterize_glyph(&self, _font_stack: &FontStack, glyph_id: GlyphID) -> Option<Glyph> {
        (glyph_id.code == 'テ' as u16).then(|| stub_glyph(glyph_id))
    }
}

/// Local fonts with every glyph.
struct EveryLocalGlyphRasterizer;

impl LocalGlyphRasterizer for EveryLocalGlyphRasterizer {
    fn rasterize_glyph(&self, _font_stack: &FontStack, glyph_id: GlyphID) -> Option<Glyph> {
        Some(stub_glyph(glyph_id))
    }
}

/// Local glyphs from `rasterizer`, CJK first, without device fonts, as in
/// the browser.
fn local(rasterizer: impl LocalGlyphRasterizer + 'static) -> LocalGlyphs {
    LocalGlyphs { ideographs: true, chooser: None, rasterizer: Some(Arc::new(rasterizer)) }
}

/// The device's fonts as faces whose files hold their names: `fonts` names
/// the font of each character, if one draws it, and a cluster takes its first
/// character's unless `clusters` names one for the whole cluster. Records the
/// first character of each cluster it is asked about, and the thread that
/// asked.
struct StubChooser {
    fonts: fn(char) -> Option<&'static str>,
    clusters: fn(&[char]) -> Option<&'static str>,
    weight: u16,
    asked: Mutex<Vec<(char, ThreadId)>>,
}

impl StubChooser {
    fn new(fonts: fn(char) -> Option<&'static str>) -> Arc<StubChooser> {
        Arc::new(StubChooser { fonts, clusters: |_| None, weight: 400, asked: Mutex::default() })
    }

    /// The characters asked about, in order.
    fn asked(&self) -> Vec<char> {
        self.asked.lock().iter().map(|(c, _)| *c).collect()
    }
}

impl FontChooser for StubChooser {
    fn choose(&self, _font_stack: &FontStack, cluster: &[char]) -> Option<FontFace> {
        self.asked.lock().push((cluster[0], std::thread::current().id()));
        let font = (self.clusters)(cluster).or_else(|| (self.fonts)(cluster[0]))?;
        Some(FontFace::device(font, self.weight, FontData::from(font.as_bytes().to_vec())))
    }
}

/// Local glyphs from `chooser`'s fonts, CJK first.
fn chooser(chooser: &Arc<StubChooser>) -> LocalGlyphs {
    LocalGlyphs { ideographs: true, chooser: Some(Arc::<StubChooser>::clone(chooser)), rasterizer: None }
}

type LoadedFn = Box<dyn FnMut(&FontStack, &GlyphRange)>;
type ErrorFn = Box<dyn FnMut(&FontStack, &GlyphRange, Error)>;
type RequestedFn = Box<dyn FnMut(&FontStack, &GlyphRange)>;

#[derive(Default)]
struct StubGlyphStoreObserver {
    glyphs_loaded: RefCell<Option<LoadedFn>>,
    glyphs_error: RefCell<Option<ErrorFn>>,
    glyphs_requested: RefCell<Option<RequestedFn>>,
}

impl StubGlyphStoreObserver {
    /// Calls the callback of a glyph event of the store.
    fn report(&self, event: RendererEvent) {
        match event {
            RendererEvent::GlyphsLoaded { font_stack, range } => {
                if let Some(f) = self.glyphs_loaded.borrow_mut().as_mut() {
                    f(&font_stack, &range);
                }
            }
            RendererEvent::GlyphsError { font_stack, range, error } => {
                if let Some(f) = self.glyphs_error.borrow_mut().as_mut() {
                    f(&font_stack, &range, error);
                }
            }
            RendererEvent::GlyphsRequested { font_stack, range } => {
                if let Some(f) = self.glyphs_requested.borrow_mut().as_mut() {
                    f(&font_stack, &range);
                }
            }
            _ => {}
        }
    }
}

type OnGlyphsAvailable = Rc<dyn Fn(GlyphMap)>;

/// A glyph requestor stub. The callback is an `Rc<dyn Fn>` because
/// `local_cjk_glyph_prefers_local_rasterization_over_range` requests glyphs
/// from inside it.
#[derive(Default)]
struct StubGlyphRequestor {
    glyphs_available: RefCell<Option<OnGlyphsAvailable>>,
    /// The shaping of the last answer.
    shaping: RefCell<Option<ShapeResults>>,
    /// The failed ranges the last answer lacked glyphs of.
    failed: RefCell<GlyphRanges>,
}

/// A request's wait for its answer.
struct Wait {
    requestor: Rc<StubGlyphRequestor>,
    dependencies: GlyphDependencies,
    wait: GlyphWait,
}

/// What the test's queue runs its tasks with, on the test's thread.
#[derive(Clone)]
struct Local {
    store: Weak<Store>,
    requestor: Weak<StubGlyphRequestor>,
}

/// A store and the waits of its requestors, which it answers when it wakes,
/// and the observer it reports its events to after each call.
struct Store {
    store: RefCell<GlyphStore>,
    observer: Rc<StubGlyphStoreObserver>,
    /// Where the responses arrive.
    inbox: Inbox,
    /// Where the store parses glyph PBFs.
    pool: TaskGroup,
    /// Where it requests from: `None` once the test takes it away.
    file_source: RefCell<Option<Arc<dyn FileSource>>>,
    waits: RefCell<Vec<Wait>>,
}

impl Store {
    /// A store whose wakes run in a task of `run_loop`; the wakes before it
    /// runs share it.
    fn new(
        local: LocalGlyphs,
        glyph_cache: Arc<GlyphCache>,
        engine: Arc<dyn FontFaceEngine>,
        file_source: Arc<dyn FileSource>,
        observer: Rc<StubGlyphStoreObserver>,
        run_loop: &Queue<Local>,
    ) -> Rc<Store> {
        let (woken, queue) = (Arc::new(AtomicBool::new(false)), run_loop.downgrade());
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            if woken.swap(true, Ordering::SeqCst) {
                return;
            }
            let woken = Arc::clone(&woken);
            if let Some(queue) = queue.upgrade() {
                queue.post(move |local: &mut Local| {
                    woken.store(false, Ordering::SeqCst);
                    if let Some(this) = local.store.upgrade() {
                        this.take_responses();
                        this.store.borrow_mut().update();
                        this.report();
                        this.answer();
                    }
                });
            }
        });
        let pool = TaskGroup::new(&Pool::new(4));
        let store = GlyphStore::new(local, glyph_cache, pool.clone(), engine, Arc::clone(&wake));
        let inbox = Inbox::new(wake);
        let file_source = RefCell::new(Some(file_source));
        Rc::new(Store { store: RefCell::new(store), observer, inbox, pool, file_source, waits: RefCell::default() })
    }

    /// Reports what the store reported since the last call to the observer.
    fn report(&self) {
        let events = self.store.borrow_mut().take_events();
        for event in events {
            self.observer.report(event);
        }
    }

    /// Asks for `dependencies` for `requestor`, which gets them once the
    /// store has them.
    fn request_glyphs(&self, requestor: &Rc<StubGlyphRequestor>, dependencies: GlyphDependencies) {
        self.waits.borrow_mut().push(Wait {
            requestor: Rc::clone(requestor),
            dependencies,
            wait: GlyphWait::default(),
        });
        self.answer();
    }

    /// Gives the store the responses and device fonts that arrived.
    fn take_responses(&self) {
        while let Some(event) = self.inbox.next() {
            match event {
                TileEvent::Glyphs { font_stack, range, id, result } => {
                    self.store.borrow_mut().take_response(&font_stack, range, id, result);
                }
                TileEvent::DeviceFonts { font_stack, faces } => {
                    self.store.borrow_mut().take_device_fonts(&font_stack, faces);
                }
                _ => {}
            }
        }
        self.report();
    }

    /// Answers the waits the store can answer now.
    fn answer(&self) {
        let waits = std::mem::take(&mut *self.waits.borrow_mut());
        let mut answered = Vec::new();
        let file_source = self.file_source.borrow().clone();
        for mut wait in waits {
            let resolved = self.store.borrow_mut().resolve(
                &wait.dependencies,
                &mut wait.wait,
                file_source.as_ref(),
                self.inbox.sender(),
            );
            match resolved {
                Some(resolved) => answered.push((wait.requestor, resolved)),
                None => self.waits.borrow_mut().push(wait),
            }
        }
        self.report();
        for (requestor, resolved) in answered {
            *requestor.shaping.borrow_mut() = Some(resolved.shaping);
            *requestor.failed.borrow_mut() = resolved.failed;
            let f = requestor.glyphs_available.borrow().clone();
            if let Some(f) = f {
                f(resolved.glyphs);
            }
        }
    }
}

struct GlyphStoreTest {
    run_loop: Queue<Local>,
    file_source: Arc<StubFileSource>,
    observer: Rc<StubGlyphStoreObserver>,
    requestor: Rc<StubGlyphRequestor>,
    store: Rc<Store>,
}

impl GlyphStoreTest {
    fn new() -> Self {
        Self::with(local(StubLocalGlyphRasterizer), ResponseType::Asynchronous)
    }

    fn with(local: LocalGlyphs, response_type: ResponseType) -> Self {
        Self::with_all(local, response_type, Arc::default(), crate::text::font_face_engine::default_engine())
    }

    fn with_glyph_cache(local: LocalGlyphs, glyph_cache: Arc<GlyphCache>) -> Self {
        Self::with_all(local, ResponseType::Asynchronous, glyph_cache, crate::text::font_face_engine::default_engine())
    }

    fn with_all(
        local: LocalGlyphs,
        response_type: ResponseType,
        glyph_cache: Arc<GlyphCache>,
        engine: Arc<dyn FontFaceEngine>,
    ) -> Self {
        let run_loop = Queue::new();
        let file_source = StubFileSource::on_queue(&run_loop, response_type);
        let observer = Rc::new(StubGlyphStoreObserver::default());
        let source: Arc<dyn FileSource> = Arc::<StubFileSource>::clone(&file_source);
        let store =
            Store::new(local, glyph_cache, engine, source, Rc::<StubGlyphStoreObserver>::clone(&observer), &run_loop);
        GlyphStoreTest { file_source, observer, requestor: Rc::new(StubGlyphRequestor::default()), store, run_loop }
    }

    /// What the test's queue runs its tasks with.
    fn local(&self) -> Local {
        Local { store: Rc::downgrade(&self.store), requestor: Rc::downgrade(&self.requestor) }
    }

    fn glyph_store(&self) -> std::cell::RefMut<'_, GlyphStore> {
        self.store.store.borrow_mut()
    }

    fn request_glyphs(&self, dependencies: GlyphDependencies) {
        self.store.request_glyphs(&self.requestor, dependencies);
    }

    fn run(&mut self, url: &str, dependencies: GlyphDependencies) {
        self.glyph_store().set_url(url);
        self.request_glyphs(dependencies);

        self.run_loop.run(&mut self.local());
    }

    fn end(&self) {
        self.run_loop.stop();
    }

    /// Runs the test's queue until `done`, as answers arrive from the pool.
    fn run_until(&self, done: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < deadline, "no answer in 10 s");
            if !self.run_loop.run_once(&mut self.local()) {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    /// Requests `dependencies` and runs the test's queue until the requestor
    /// gets them: what it got.
    fn run_until_answered(&self, dependencies: GlyphDependencies) -> GlyphMap {
        let answer: Rc<RefCell<Option<GlyphMap>>> = Rc::default();
        let a = Rc::clone(&answer);
        *self.requestor.glyphs_available.borrow_mut() = Some(Rc::new(move |glyphs: GlyphMap| {
            *a.borrow_mut() = Some(glyphs);
        }));
        self.request_glyphs(dependencies);
        self.run_until(|| answer.borrow().is_some());
        answer.take().expect("an answer")
    }

    /// Loads `chars` of `font_stack` from the glyphs.pbf fixture.
    fn load(&mut self, font_stack: FontStack, chars: &[char]) {
        self.file_source.set_response(Kind::Glyphs, glyphs_response("test/fixtures/resources/glyphs.pbf"));
        let run_loop = self.run_loop.clone();
        *self.requestor.glyphs_available.borrow_mut() = Some(Rc::new(move |_: GlyphMap| run_loop.stop()));
        let ids: GlyphIDs = chars.iter().map(|c| gid(*c)).collect();
        let dependencies =
            GlyphDependencies { glyphs: VecMap::from([(font_stack, ids)]), shapes: ShapeRequests::default() };
        self.run("test/fixtures/resources/glyphs.pbf", dependencies);
    }
}

fn test_stack() -> FontStack {
    vec!["Test Stack".to_owned()]
}

fn dependencies(chars: &[char]) -> GlyphDependencies {
    let ids: GlyphIDs = chars.iter().map(|c| GlyphID::new(*c as u32)).collect();
    GlyphDependencies { glyphs: VecMap::from([(test_stack(), ids)]), shapes: ShapeRequests::default() }
}

fn gid(c: char) -> GlyphID {
    GlyphID::new(c as u32)
}

fn glyphs_response(path: &'static str) -> impl Fn(&Resource) -> Option<Response> {
    move |_resource: &Resource| {
        let mut response = Response::new();
        response.data = Some(Arc::new(read_file(path)));
        Some(response)
    }
}

// gtest: GlyphManager.LoadingSuccess
#[test]
fn loading_success() {
    let mut test = GlyphStoreTest::new();

    test.file_source.set_response(Kind::Glyphs, |resource: &Resource| {
        assert_eq!(Kind::Glyphs, resource.kind);
        let mut response = Response::new();
        response.data = Some(Arc::new(read_file("test/fixtures/resources/glyphs.pbf")));
        Some(response)
    });

    *test.observer.glyphs_error.borrow_mut() = Some(Box::new(|_, _, _| panic!("glyphsError")));

    *test.observer.glyphs_loaded.borrow_mut() = Some(Box::new(|font_stack, range| {
        assert_eq!(*font_stack, test_stack());
        assert_eq!(*range, GlyphRange::new(0, 255, GlyphIDType::FontPBF));
    }));

    let available = Rc::new(Cell::new(false));
    let (a, run_loop) = (Rc::clone(&available), test.run_loop.clone());
    *test.requestor.glyphs_available.borrow_mut() = Some(Rc::new(move |glyphs: GlyphMap| {
        let test_positions = &glyphs[&font_stack_hash(&test_stack())];

        assert_eq!(test_positions.len(), 3);
        assert!(test_positions.contains_key(&gid('a')));
        assert!(test_positions.contains_key(&gid('å')));
        assert!(test_positions.contains_key(&gid(' ')));
        assert!(test_positions[&gid(' ')].is_some());

        a.set(true);
        run_loop.stop();
    }));

    test.run("test/fixtures/resources/glyphs.pbf", dependencies(&['a', 'å', ' ']));
    assert!(available.get());
}

/// The observer's `glyphsError` and the requestor's `glyphsAvailable` for
/// `LoadingFail` / `LoadingCorrupted`: the error is reported as the store
/// takes the response in, and the requestor then gets the range's glyphs as
/// missing.
fn expect_error(test: &GlyphStoreTest, message: &'static str) -> Rc<Cell<bool>> {
    let reported = Rc::new(Cell::new(false));
    let r = Rc::clone(&reported);
    *test.observer.glyphs_error.borrow_mut() = Some(Box::new(move |font_stack, glyph_range, error| {
        assert_eq!(*font_stack, test_stack());
        assert_eq!(*glyph_range, GlyphRange::new(0, 255, GlyphIDType::FontPBF));
        assert_eq!(error.to_string(), message);
        r.set(true);
    }));

    let answered = Rc::new(Cell::new(false));
    let (a, run_loop) = (Rc::clone(&answered), test.run_loop.clone());
    *test.requestor.glyphs_available.borrow_mut() = Some(Rc::new(move |glyphs: GlyphMap| {
        let test_positions = &glyphs[&font_stack_hash(&test_stack())];
        assert_eq!(test_positions.len(), 2);
        assert!(test_positions[&gid('a')].is_none());
        assert!(test_positions[&gid('å')].is_none());
        assert!(reported.get());
        a.set(true);
        run_loop.stop();
    }));
    answered
}

// gtest: GlyphManager.LoadingFail
#[test]
fn loading_fail() {
    let mut test = GlyphStoreTest::new();

    test.file_source.set_response(Kind::Glyphs, |_: &Resource| {
        let mut response = Response::new();
        response.error = Some(Box::new(ResourceError::new(Reason::Other, "Failed by the test case")));
        Some(response)
    });

    let called = expect_error(&test, "Failed by the test case");

    test.run("test/fixtures/resources/glyphs.pbf", dependencies(&['a', 'å']));
    assert!(called.get());
}

/// A file source response that fails on the first request and serves the
/// glyphs.pbf fixture after that.
fn fails_first_then_glyphs() -> impl Fn(&Resource) -> Option<Response> + Send + Sync {
    let loads = AtomicU32::new(0);
    move |_: &Resource| {
        let mut response = Response::new();
        if loads.fetch_add(1, Ordering::SeqCst) == 0 {
            response.error = Some(Box::new(ResourceError::new(Reason::NotFound, "Failed by the test case")));
        } else {
            response.data = Some(Arc::new(read_file("test/fixtures/resources/glyphs.pbf")));
        }
        Some(response)
    }
}

/// A file source response that fails on the first request and serves
/// `data` after that.
fn fails_first_then(data: &'static [u8]) -> impl Fn(&Resource) -> Option<Response> + Send + Sync {
    let loads = AtomicU32::new(0);
    move |_: &Resource| {
        let mut response = Response::new();
        if loads.fetch_add(1, Ordering::SeqCst) == 0 {
            response.error = Some(Box::new(ResourceError::new(Reason::NotFound, "Failed by the test case")));
        } else {
            response.data = Some(Arc::new(data.to_vec()));
        }
        Some(response)
    }
}

fn count_requests(test: &GlyphStoreTest) -> Arc<AtomicU32> {
    let requests = Arc::new(AtomicU32::new(0));
    let r = Arc::clone(&requests);
    *test.observer.glyphs_requested.borrow_mut() = Some(Box::new(move |_, _| {
        r.fetch_add(1, Ordering::SeqCst);
    }));
    requests
}

// gtest: GlyphManager.LoadingFailRequestsTheRangeAgain
#[test]
fn loading_fail_requests_the_range_again() {
    let mut test = GlyphStoreTest::new();
    let requests = count_requests(&test);
    test.file_source.set_response(Kind::Glyphs, fails_first_then_glyphs());

    let answers = Rc::new(Cell::new(0));
    let (n, run_loop) = (Rc::clone(&answers), test.run_loop.clone());
    *test.requestor.glyphs_available.borrow_mut() = Some(Rc::new(move |glyphs: GlyphMap| {
        let test_positions = &glyphs[&font_stack_hash(&test_stack())];
        n.set(n.get() + 1);
        if n.get() == 1 {
            assert!(test_positions[&gid('a')].is_none());
            // C++ asks again through `loop.invoke`, after this response is
            // processed.
            run_loop.post(|local| {
                local
                    .store
                    .upgrade()
                    .unwrap()
                    .request_glyphs(&local.requestor.upgrade().unwrap(), dependencies(&['a']));
            });
        } else {
            assert!(test_positions[&gid('a')].is_some());
            run_loop.stop();
        }
    }));

    let run_loop = test.run_loop.clone();
    test.run_loop.post_after(Duration::from_millis(1000), move |_| run_loop.stop());

    test.run("test/fixtures/resources/glyphs.pbf", dependencies(&['a']));

    assert_eq!(requests.load(Ordering::SeqCst), 2);
    assert_eq!(answers.get(), 2);
}

// A wait answered without the glyphs of a range that failed while it waited
// says so, so that a still image lays its tile out again; that layout's wait
// requests the range again.
#[test]
fn an_answer_says_a_range_it_needed_failed() {
    let mut test = GlyphStoreTest::new();
    let requests = count_requests(&test);
    test.file_source.set_response(Kind::Glyphs, fails_first_then_glyphs());
    let run_loop = test.run_loop.clone();
    *test.requestor.glyphs_available.borrow_mut() = Some(Rc::new(move |_: GlyphMap| run_loop.stop()));

    test.run("test/fixtures/resources/glyphs.pbf", dependencies(&['a']));
    assert!(!test.requestor.failed.borrow().is_empty());
    assert_eq!(requests.load(Ordering::SeqCst), 1);

    // A wait that began after the failure requests the range again, which
    // loads.
    test.run("test/fixtures/resources/glyphs.pbf", dependencies(&['a']));
    assert_eq!(requests.load(Ordering::SeqCst), 2);
    assert!(test.requestor.failed.borrow().is_empty());
}

/// A range that a later wait loads after it failed is reported once, so that
/// the tiles laid out without it lay out again.
#[test]
fn a_range_that_loads_after_failing_is_reported_once() {
    let mut test = GlyphStoreTest::new();
    test.file_source.set_response(Kind::Glyphs, fails_first_then_glyphs());
    let run_loop = test.run_loop.clone();
    *test.requestor.glyphs_available.borrow_mut() = Some(Rc::new(move |_: GlyphMap| run_loop.stop()));
    let range = (test_stack(), GlyphRange::new(0, 255, GlyphIDType::FontPBF));

    test.run("test/fixtures/resources/glyphs.pbf", dependencies(&['a']));
    assert_eq!(*test.requestor.failed.borrow(), VecSet::from([range.clone()]), "the answer names the range");
    assert!(test.glyph_store().take_recovered().is_empty(), "nothing loaded yet");

    test.run("test/fixtures/resources/glyphs.pbf", dependencies(&['a']));
    assert_eq!(test.glyph_store().take_recovered(), VecSet::from([range]));
    assert!(test.glyph_store().take_recovered().is_empty(), "reported once");
}

/// A wait for a range that fails while it waits gets the range's glyphs as
/// missing however often it asks again: a failure can't leave it waiting.
#[test]
fn a_wait_resolves_a_range_that_failed_during_it_as_missing() {
    let test = GlyphStoreTest::new();
    let requests = count_requests(&test);
    test.file_source.set_response(Kind::Glyphs, |_: &Resource| {
        let mut response = Response::new();
        response.error = Some(Box::new(ResourceError::new(Reason::Other, "Failed by the test case")));
        Some(response)
    });
    test.glyph_store().set_url("test/fixtures/resources/glyphs.pbf");

    let file_source = test.store.file_source.borrow().clone().unwrap();
    let mut wait = GlyphWait::default();
    let inbox = test.store.inbox.sender().clone();
    assert!(test.glyph_store().resolve(&dependencies(&['a']), &mut wait, Some(&file_source), &inbox).is_none());
    test.run_loop.run_once(&mut test.local());
    test.store.take_responses();
    test.glyph_store().update();
    for _ in 0..2 {
        let resolved =
            test.glyph_store().resolve(&dependencies(&['a']), &mut wait, Some(&file_source), &inbox).expect("answered");
        assert!(!resolved.failed.is_empty());
        assert!(resolved.glyphs[&font_stack_hash(&test_stack())][&gid('a')].is_none());
    }
    assert_eq!(requests.load(Ordering::SeqCst), 1);
}

// gtest: GlyphManager.LoadingFailStillAnswersWithTheOtherRanges
#[test]
fn loading_fail_still_answers_with_the_other_ranges() {
    let mut test = GlyphStoreTest::new();

    test.file_source.set_response(Kind::Glyphs, |resource: &Resource| {
        let mut response = Response::new();
        if resource.url == "test/fixtures/resources/glyphs-0-255.pbf" {
            response.data = Some(Arc::new(read_file("test/fixtures/resources/glyphs.pbf")));
        } else {
            response.error = Some(Box::new(ResourceError::new(Reason::NotFound, "Failed by the test case")));
        }
        Some(response)
    });

    let answered = Rc::new(Cell::new(false));
    let (a, run_loop) = (Rc::clone(&answered), test.run_loop.clone());
    *test.requestor.glyphs_available.borrow_mut() = Some(Rc::new(move |glyphs: GlyphMap| {
        let test_positions = &glyphs[&font_stack_hash(&test_stack())];
        assert_eq!(test_positions.len(), 2);
        assert!(test_positions[&gid('a')].is_some());
        assert!(test_positions[&gid('ở')].is_none());
        a.set(true);
        run_loop.stop();
    }));

    let run_loop = test.run_loop.clone();
    test.run_loop.post_after(Duration::from_millis(1000), move |_| run_loop.stop());

    test.run("test/fixtures/resources/glyphs-{range}.pbf", dependencies(&['a', 'ở']));

    assert!(answered.get());
}

// gtest: GlyphManager.LoadingCorrupted
#[test]
fn loading_corrupted() {
    let mut test = GlyphStoreTest::new();

    test.file_source.set_response(Kind::Glyphs, |_: &Resource| {
        let mut response = Response::new();
        response.data = Some(Arc::new(b"CORRUPTED".to_vec()));
        Some(response)
    });

    let called = expect_error(&test, "invalid wire type: 7");

    test.run("test/fixtures/resources/glyphs.pbf", dependencies(&['a', 'å']));
    assert!(called.get());
}

// gtest: GlyphManager.SharesParsedGlyphsBetweenManagersAndFontStacks
#[test]
fn shares_parsed_glyphs_between_managers_and_font_stacks() {
    let cache = Arc::new(GlyphCache::new());
    let load = |font_stack: &str| {
        let mut test = GlyphStoreTest::with_glyph_cache(local(StubLocalGlyphRasterizer), Arc::clone(&cache));
        let font_stack = vec![font_stack.to_owned()];
        test.load(font_stack.clone(), &['a']);
        test.glyph_store().glyph(&font_stack, gid('a'))
    };

    let first = load("Test Stack");

    assert!(Arc::ptr_eq(&first, &load("Other Stack")));
}

// gtest: GlyphManager.KeepsItsOwnGlyphsWithoutACache
#[test]
fn keeps_its_own_glyphs_without_a_cache() {
    let load = || {
        let mut test = GlyphStoreTest::new();
        test.load(test_stack(), &['a']);
        test.glyph_store().glyph(&test_stack(), gid('a'))
    };

    let first = load();

    assert!(!Arc::ptr_eq(&first, &load()));
}

// gtest: GlyphManager.ReleasesTheGlyphsOfEvictedFontStacks
#[test]
fn releases_the_glyphs_of_evicted_font_stacks() {
    let cache = Arc::new(GlyphCache::new());
    let mut test = GlyphStoreTest::with_glyph_cache(local(StubLocalGlyphRasterizer), Arc::clone(&cache));
    test.load(test_stack(), &['a']);
    assert_ne!(0, cache.size());

    test.glyph_store().evict(&VecSet::new());

    assert_eq!(0, cache.size());
}

// A wait whose font stacks are evicted, the one still loading and the one it
// already has, requests them again and is answered with both.
#[test]
fn a_wait_requests_the_font_stacks_evicted_while_it_waits_again() {
    let mut test = GlyphStoreTest::new();
    let loaded_stack = vec!["Loaded Stack".to_owned()];
    test.load(loaded_stack.clone(), &['a']);

    let answer = Rc::new(RefCell::new(None));
    let (a, run_loop) = (Rc::clone(&answer), test.run_loop.clone());
    *test.requestor.glyphs_available.borrow_mut() = Some(Rc::new(move |glyphs: GlyphMap| {
        *a.borrow_mut() = Some(glyphs);
        run_loop.stop();
    }));
    let ids: GlyphIDs = [gid('a')].into_iter().collect();
    let dependencies = GlyphDependencies {
        glyphs: VecMap::from([(loaded_stack.clone(), ids.clone()), (test_stack(), ids)]),
        shapes: ShapeRequests::default(),
    };
    test.request_glyphs(dependencies);

    test.glyph_store().evict(&VecSet::new());
    // The renderer's update asks the waiting tiles again after an eviction.
    test.store.answer();

    let run_loop = test.run_loop.clone();
    test.run_loop.post_after(Duration::from_millis(1000), move |_| run_loop.stop());
    test.run_loop.run(&mut test.local());

    let answer = answer.borrow();
    let glyphs = answer.as_ref().expect("an answer");
    for font_stack in [loaded_stack, test_stack()] {
        assert!(glyphs[&font_stack_hash(&font_stack)][&gid('a')].is_some(), "{font_stack:?}");
    }
}

// A response that arrived for a range evicted since is stale: the range
// isn't loaded by it, and a later wait requests it again, from the glyph
// URL the store has then.
#[test]
fn a_response_to_an_evicted_range_is_dropped() {
    let test = GlyphStoreTest::with(local(StubLocalGlyphRasterizer), ResponseType::Synchronous);
    let urls = Arc::new(Mutex::new(Vec::new()));
    let u = Arc::clone(&urls);
    test.file_source.set_response(Kind::Glyphs, move |resource: &Resource| {
        u.lock().push(resource.url.clone());
        glyphs_response("test/fixtures/resources/glyphs.pbf")(resource)
    });

    test.glyph_store().set_url("old/{fontstack}/{range}.pbf");
    test.request_glyphs(dependencies(&['a']));
    test.glyph_store().evict(&VecSet::new());
    test.store.take_responses();

    test.glyph_store().set_url("new/{fontstack}/{range}.pbf");
    test.request_glyphs(dependencies(&['a']));
    let urls = urls.lock();
    assert_eq!(urls.len(), 2, "requested again: {urls:?}");
    assert!(urls[1].starts_with("new/"));
}

// gtest: GlyphManager.LoadingCancel
#[test]
fn loading_cancel() {
    let mut test = GlyphStoreTest::new();

    let run_loop = test.run_loop.clone();
    test.file_source.set_response(Kind::Glyphs, move |_: &Resource| {
        run_loop.stop();
        None
    });

    *test.observer.glyphs_loaded.borrow_mut() = Some(Box::new(|_, _| panic!("Should never be called")));

    test.run("test/fixtures/resources/glyphs.pbf", dependencies(&['a', 'å']));
}

// gtest: GlyphManager.LoadLocalCJKGlyph
#[test]
fn load_local_cjk_glyph() {
    let mut test = GlyphStoreTest::new();
    let glyph_responses = Arc::new(AtomicI32::new(0));

    let g = Arc::clone(&glyph_responses);
    test.file_source.set_response(Kind::Glyphs, move |_: &Resource| {
        g.fetch_add(1, Ordering::SeqCst);
        None
    });

    let g = Arc::clone(&glyph_responses);
    *test.observer.glyphs_loaded.borrow_mut() = Some(Box::new(move |_, _| {
        g.fetch_add(1, Ordering::SeqCst);
    }));

    let available = Rc::new(Cell::new(false));
    let (a, g, run_loop) = (Rc::clone(&available), Arc::clone(&glyph_responses), test.run_loop.clone());
    *test.requestor.glyphs_available.borrow_mut() = Some(Rc::new(move |glyphs: GlyphMap| {
        assert_eq!(g.load(Ordering::SeqCst), 0); // Local generation should prevent requesting any glyphs

        let test_positions = &glyphs[&font_stack_hash(&test_stack())];

        assert_eq!(test_positions.len(), 1);
        assert!(test_positions.contains_key(&gid('中')));

        let glyph = test_positions[&gid('中')].clone().expect("glyph");
        assert_eq!(glyph.id.code, '中' as u16);
        assert_eq!(glyph.metrics.width, 24);
        assert_eq!(glyph.metrics.height, 24);
        assert_eq!(glyph.metrics.left, 0.0);
        assert_eq!(glyph.metrics.top, -8.0);
        assert_eq!(glyph.metrics.advance, 24);
        assert_eq!(glyph.bitmap.size, Size::new(30, 30));

        let pixel_count = (glyph.bitmap.size.width * glyph.bitmap.size.height) as usize;
        for (i, &expected) in SDF_BITMAP.iter().enumerate().take(pixel_count) {
            assert_eq!(glyph.bitmap.data[i], expected, "pixel {i}");
        }

        a.set(true);
        run_loop.stop();
    }));

    test.run("test/fixtures/resources/glyphs.pbf", dependencies(&['中']));
    assert!(available.get());
}

// gtest: GlyphManager.LoadLocalCJKSymbols
#[test]
fn load_local_cjk_symbols() {
    let mut test = GlyphStoreTest::new();
    let glyph_responses = Arc::new(AtomicI32::new(0));

    let g = Arc::clone(&glyph_responses);
    test.file_source.set_response(Kind::Glyphs, move |_: &Resource| {
        g.fetch_add(1, Ordering::SeqCst);
        None
    });

    let g = Arc::clone(&glyph_responses);
    *test.observer.glyphs_loaded.borrow_mut() = Some(Box::new(move |_, _| {
        g.fetch_add(1, Ordering::SeqCst);
    }));

    let available = Rc::new(Cell::new(false));
    let (a, g, run_loop) = (Rc::clone(&available), Arc::clone(&glyph_responses), test.run_loop.clone());
    *test.requestor.glyphs_available.borrow_mut() = Some(Rc::new(move |glyphs: GlyphMap| {
        assert_eq!(g.load(Ordering::SeqCst), 0); // Local generation should prevent requesting any glyphs.

        let test_positions = &glyphs[&font_stack_hash(&test_stack())];

        assert_eq!(test_positions.len(), 2);
        assert!(test_positions.contains_key(&gid('〒')));
        assert!(test_positions.contains_key(&gid('！')));

        a.set(true);
        run_loop.stop();
    }));

    test.run("test/fixtures/resources/glyphs.pbf", dependencies(&['〒', '！']));
    assert!(available.get());
}

// gtest: GlyphManager.LocalCJKGlyphPrefersLocalRasterizationOverRange
#[test]
fn local_cjk_glyph_prefers_local_rasterization_over_range() {
    // Verifies that locally-rasterized glyphs (like テ) are produced by the
    // local rasterizer instead of using a glyph from an already-loaded PBF
    // range.
    let mut test = GlyphStoreTest::with(local(KatakanaOnlyLocalGlyphRasterizer), ResponseType::Asynchronous);
    let first_glyph_response = Arc::new(AtomicBool::new(false));

    let f = Arc::clone(&first_glyph_response);
    test.file_source.set_response(Kind::Glyphs, move |_: &Resource| {
        f.store(true, Ordering::SeqCst);
        let mut response = Response::new();
        response.data = Some(Arc::new(read_file("test/fixtures/resources/glyphs-12244-12543.pbf")));
        Some(response)
    });

    let done = Rc::new(Cell::new(false));
    let (f, d, run_loop) = (Arc::clone(&first_glyph_response), Rc::clone(&done), test.run_loop.clone());
    let store = Rc::downgrade(&test.store);
    let requestor = Rc::downgrade(&test.requestor);
    *test.requestor.glyphs_available.borrow_mut() = Some(Rc::new(move |glyphs: GlyphMap| {
        let test_positions = &glyphs[&font_stack_hash(&test_stack())];

        if f.swap(false, Ordering::SeqCst) {
            assert_eq!(test_positions.len(), 1);
            assert!(test_positions.contains_key(&gid('々')));

            // Katakana letter te, should be locally rasterized
            //  instead of using the glyph received from the range
            //  for the ideagraphic mark
            store.upgrade().unwrap().request_glyphs(
                &requestor.upgrade().unwrap(),
                dependencies(&['テ']), // 0x30c6
            );
        } else {
            assert_eq!(test_positions.len(), 1);
            assert!(test_positions.contains_key(&gid('テ')));

            let glyph = test_positions[&gid('テ')].clone().expect("glyph");
            assert_eq!(glyph.id.code, 'テ' as u16);
            assert_eq!(glyph.metrics.width, 24);
            assert_eq!(glyph.metrics.height, 24);
            assert_eq!(glyph.metrics.left, 0.0);
            assert_eq!(glyph.metrics.top, -8.0);
            assert_eq!(glyph.metrics.advance, 24);
            assert_eq!(glyph.bitmap.size, Size::new(30, 30));

            d.set(true);
            run_loop.stop();
        }
    }));

    test.run(
        "test/fixtures/resources/glyphs-12244-12543.pbf",
        dependencies(&['々']), // 0x3005
    );
    assert!(done.get());
}

// gtest: GlyphManager.LoadingInvalid
#[test]
fn loading_invalid() {
    let mut test = GlyphStoreTest::new();

    test.file_source.set_response(Kind::Glyphs, |resource: &Resource| {
        assert_eq!(Kind::Glyphs, resource.kind);
        let mut response = Response::new();
        response.data = Some(Arc::new(read_file("test/fixtures/resources/fake_glyphs-0-255.pbf")));
        Some(response)
    });

    *test.observer.glyphs_error.borrow_mut() = Some(Box::new(|_, _, _| panic!("glyphsError")));

    *test.observer.glyphs_loaded.borrow_mut() = Some(Box::new(|font_stack, range| {
        assert_eq!(*font_stack, test_stack());
        assert_eq!(*range, GlyphRange::new(0, 255, GlyphIDType::FontPBF));
    }));

    let available = Rc::new(Cell::new(false));
    let (a, run_loop) = (Rc::clone(&available), test.run_loop.clone());
    *test.requestor.glyphs_available.borrow_mut() = Some(Rc::new(move |glyphs: GlyphMap| {
        let test_positions = &glyphs[&font_stack_hash(&test_stack())];

        assert_eq!(test_positions.len(), 2);
        assert!(test_positions[&gid('A')].is_none());
        assert!(test_positions[&gid('E')].is_some());

        a.set(true);
        run_loop.stop();
    }));

    test.run("test/fixtures/resources/glyphs.pbf", dependencies(&['A', 'E']));
    assert!(available.get());
}

// gtest: GlyphManager.ImmediateFileSource
#[test]
fn immediate_file_source() {
    // `GlyphManagerTestSynchronous`: the default store (no local
    // rasterizer) and a synchronous StubFileSource.
    let mut test = GlyphStoreTest::with(LocalGlyphs::default(), ResponseType::Synchronous);

    test.file_source.set_response(Kind::Glyphs, glyphs_response("test/fixtures/resources/glyphs.pbf"));

    *test.observer.glyphs_error.borrow_mut() = Some(Box::new(|_, _, _| panic!("glyphsError")));

    let available = Rc::new(Cell::new(false));
    let (a, run_loop) = (Rc::clone(&available), test.run_loop.clone());
    *test.requestor.glyphs_available.borrow_mut() = Some(Rc::new(move |_| {
        a.set(true);
        run_loop.stop();
    }));

    test.run("test/fixtures/resources/glyphs.pbf", dependencies(&['a', 'å', ' ']));
    assert!(available.get());
    test.end();
}

// gtest: GlyphManager.ImmediateFileSourceFailureRequestsTheRangeAgain
#[test]
fn immediate_file_source_failure_requests_the_range_again() {
    let test = GlyphStoreTest::with(local(StubLocalGlyphRasterizer), ResponseType::Synchronous);
    let requests = count_requests(&test);
    test.file_source.set_response(Kind::Glyphs, fails_first_then_glyphs());

    let answers = Rc::new(RefCell::new(Vec::new()));
    let a = Rc::clone(&answers);
    *test.requestor.glyphs_available.borrow_mut() = Some(Rc::new(move |glyphs: GlyphMap| {
        a.borrow_mut().push(glyphs[&font_stack_hash(&test_stack())][&gid('a')].is_some());
    }));

    test.glyph_store().set_url("test/fixtures/resources/glyphs.pbf");

    // The failure is taken in when the store wakes the renderer, and the
    // glyphs once the pool has parsed them.
    test.request_glyphs(dependencies(&['a']));
    test.run_loop.run_once(&mut test.local());
    test.request_glyphs(dependencies(&['a']));
    test.store.pool.wait();
    test.run_loop.run_once(&mut test.local());

    assert_eq!(requests.load(Ordering::SeqCst), 2);
    assert_eq!(*answers.borrow(), vec![false, true]);
}

// Guards against silent drift between allowsFixedWidthGlyphGeneration's range
// coverage and the precomputed NON_IDEOGRAPH_GLYPH_RANGES_PER_FONT_STACK used
// for offline download progress estimation.
// gtest: i18n.GlyphRangeLocalGenerationCountMatchesConstant
#[test]
fn glyph_range_local_generation_count_matches_constant() {
    let mut locally_generated_ranges: u32 = 0;
    for i in 0..GLYPH_RANGES_PER_FONT_STACK {
        if i18n::glyph_range_is_entirely_locally_generated((i * GLYPHS_PER_GLYPH_RANGE) as u16) {
            locally_generated_ranges += 1;
        }
    }
    assert_eq!(GLYPH_RANGES_PER_FONT_STACK - locally_generated_ranges, NON_IDEOGRAPH_GLYPH_RANGES_PER_FONT_STACK);
}

// Port-only: a font-face engine that gets ready later (the browser build's,
// which loads its shaper as a second wasm module).

type ReadyCallback = Box<dyn FnOnce(Result<(), String>) + Send>;

/// An engine that is ready once the test calls back the `ready`s it holds.
#[derive(Default)]
struct DeferredEngine {
    ready: Mutex<Vec<ReadyCallback>>,
    prepares: AtomicUsize,
    loads: AtomicUsize,
}

impl DeferredEngine {
    fn report(&self, result: &Result<(), String>) {
        let ready = std::mem::take(&mut *self.ready.lock());
        for ready in ready {
            ready(result.clone());
        }
    }
}

impl FontFaceEngine for DeferredEngine {
    fn prepare(&self, ready: Box<dyn FnOnce(Result<(), String>) + Send>) {
        self.prepares.fetch_add(1, Ordering::SeqCst);
        self.ready.lock().push(ready);
    }

    fn load(&self, _type: GlyphIDType, data: FontData, _weight: u16) -> Option<Box<dyn FontFaceShaper>> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        (data.as_bytes() == b"font").then(|| -> Box<dyn FontFaceShaper> { Box::new(FakeShaper) })
    }
}

/// Shapes each character as the glyph of that index, 10 px wide.
struct FakeShaper;

impl FontFaceShaper for FakeShaper {
    fn shape(&self, text: &[char], _rtl: bool, shaped: &mut ShapedText) {
        for &c in text {
            shaped.glyphs.push(GlyphID::with_type(u32::from(c) as u16, FONT_FACE));
            shaped.adjusts.push(GlyphAdjust::new(0.0, 0.0, 10.0));
        }
    }

    fn glyph_of(&self, c: char) -> Option<GlyphID> {
        Some(GlyphID::with_type(u32::from(c) as u16, FONT_FACE))
    }

    fn rasterize(&self, glyph: GlyphID) -> Glyph {
        stub_glyph(glyph)
    }
}

const FONT_FACE: GlyphIDType = GlyphIDType::face(NonZeroU16::new(7).unwrap());

/// A store on `engine` with one font face, and a synchronous file source
/// that answers its file with "font".
fn font_face_test(engine: &Arc<DeferredEngine>) -> GlyphStoreTest {
    let test = GlyphStoreTest::with_all(
        local(StubLocalGlyphRasterizer),
        ResponseType::Synchronous,
        Arc::default(),
        Arc::<DeferredEngine>::clone(engine),
    );
    test.file_source.set_response(Kind::Glyphs, |resource: &Resource| {
        assert_eq!(resource.url, "test://face.ttf");
        let mut response = Response::new();
        response.data = Some(Arc::new(b"font".to_vec()));
        Some(response)
    });
    let face = FontFace { type_: FONT_FACE, ..FontFace::new("Test Stack", "test://face.ttf", vec![]) };
    test.glyph_store().set_font_faces(Some(Arc::new(vec![face])));
    test
}

/// Requests the font face's file and the shaping of "\u{5}\u{6}" with it;
/// returns what the requestor got.
fn request_font_face(test: &GlyphStoreTest) -> Rc<RefCell<Option<GlyphMap>>> {
    let available: Rc<RefCell<Option<GlyphMap>>> = Rc::default();
    let a = Rc::clone(&available);
    *test.requestor.glyphs_available.borrow_mut() = Some(Rc::new(move |glyphs: GlyphMap| {
        *a.borrow_mut() = Some(glyphs);
    }));
    let ids: GlyphIDs = [GlyphID::with_type(0, FONT_FACE)].into_iter().collect();
    let shapes =
        VecMap::from([(test_stack(), VecMap::from([(FONT_FACE, [vec!['\u{5}', '\u{6}']].into_iter().collect())]))]);
    test.request_glyphs(GlyphDependencies { glyphs: VecMap::from([(test_stack(), ids)]), shapes });
    available
}

/// The shaping of "\u{5}\u{6}" in the requestor's last answer.
fn shaped(test: &GlyphStoreTest) -> Vec<u16> {
    let shaping = test.requestor.shaping.borrow();
    let shaping = shaping.as_ref().expect("an answer");
    shaping[&test_stack()][&FONT_FACE][&vec!['\u{5}', '\u{6}']].glyphs.iter().map(|glyph| glyph.code).collect()
}

#[test]
fn font_face_waits_for_the_engine() {
    let engine = Arc::new(DeferredEngine::default());
    let test = font_face_test(&engine);
    // Asked to get ready by the font faces, before any file arrives.
    assert_eq!(engine.prepares.load(Ordering::SeqCst), 1);

    // The file arrived (synchronously), but the engine isn't ready: the
    // requestor waits, and the file isn't parsed.
    let available = request_font_face(&test);
    test.run_loop.run_once(&mut test.local());
    assert!(available.borrow().is_none());
    assert_eq!(engine.loads.load(Ordering::SeqCst), 0);

    // Once it is, the store wakes the renderer, which parses the file and
    // answers the wait.
    engine.report(&Ok(()));
    assert!(available.borrow().is_none());
    test.run_loop.run_once(&mut test.local());
    test.run_loop.run_once(&mut test.local());
    assert_eq!(engine.loads.load(Ordering::SeqCst), 1);
    assert_eq!(engine.prepares.load(Ordering::SeqCst), 1);
    let glyphs = available.borrow_mut().take().expect("notified");
    let glyphs = &glyphs[&font_stack_hash(&test_stack())];
    // The file's glyph 0 only marks it as a dependency; the shaped glyphs
    // are drawn.
    assert!(glyphs[&GlyphID::with_type(0, FONT_FACE)].is_none());
    assert_eq!(shaped(&test), [5, 6]);
    let glyph = glyphs[&GlyphID::with_type(5, FONT_FACE)].clone().expect("shaped glyph");
    assert_eq!(glyph.id, GlyphID::with_type(5, FONT_FACE));
    assert_eq!(glyph.metrics.width, 24);

    // A later request is answered at once.
    let available = request_font_face(&test);
    assert!(available.borrow().is_some());
}

#[test]
fn font_face_engine_failure_leaves_the_face_out() {
    let engine = Arc::new(DeferredEngine::default());
    let test = font_face_test(&engine);
    let available = request_font_face(&test);
    engine.report(&Err("no shaper".to_owned()));
    test.run_loop.run_once(&mut test.local());
    test.run_loop.run_once(&mut test.local());
    // Answered, as for a file that isn't a font: without shaping.
    assert_eq!(engine.loads.load(Ordering::SeqCst), 0);
    assert!(available.borrow_mut().take().is_some());
    assert_eq!(shaped(&test), Vec::<u16>::new());
}

// Port-only: a face that holds its font file, as an app's fallback faces
// can, loads it without a request.
#[test]
fn font_face_with_data_needs_no_request() {
    let engine = Arc::new(DeferredEngine::default());
    let test = font_face_test(&engine);
    test.file_source.set_response(Kind::Glyphs, |resource: &Resource| -> Option<Response> {
        panic!("requested {}", resource.url);
    });
    let face = FontFace { type_: FONT_FACE, ..FontFace::new("", "app://face.ttf", vec![(0, 0xFFFF)]) }
        .with_data(FontData::new(Arc::new(b"font".to_vec())));
    test.glyph_store().set_font_faces(Some(Arc::new(vec![face])));
    engine.report(&Ok(()));
    test.run_loop.run_once(&mut test.local());

    let available = request_font_face(&test);
    assert_eq!(engine.loads.load(Ordering::SeqCst), 1);
    assert!(available.borrow_mut().take().is_some(), "answered at once");
    assert_eq!(shaped(&test), [5, 6]);
}

/// The glyphs of `chars` of the test stack that a store answers with at once,
/// with no file source response: for the glyphs that need no request.
fn answer_at_once(test: &GlyphStoreTest, chars: &[char]) -> Option<GlyphMap> {
    let answer: Rc<RefCell<Option<GlyphMap>>> = Rc::default();
    let a = Rc::clone(&answer);
    *test.requestor.glyphs_available.borrow_mut() = Some(Rc::new(move |glyphs: GlyphMap| {
        *a.borrow_mut() = Some(glyphs);
    }));
    test.request_glyphs(dependencies(chars));
    answer.take()
}

// A style without a glyph URL draws its text from local fonts, and requests
// no glyph PBF.
#[test]
fn without_a_glyph_url_glyphs_come_from_local_fonts() {
    for (local, drawn) in [(local(EveryLocalGlyphRasterizer), true), (LocalGlyphs::default(), false)] {
        let test = GlyphStoreTest::with(local, ResponseType::Asynchronous);
        let requests = count_requests(&test);
        test.glyph_store().set_url("");

        let glyphs = answer_at_once(&test, &['a', '中']).expect("answered at once");

        let glyphs = &glyphs[&font_stack_hash(&test_stack())];
        assert_eq!(glyphs[&gid('a')].is_some(), drawn);
        assert_eq!(glyphs[&gid('中')].is_some(), drawn);
        assert_eq!(requests.load(Ordering::SeqCst), 0);
    }
}

// A glyph that the stack's PBF lacks comes from local fonts, and the PBF's
// glyphs stay the PBF's.
#[test]
fn a_glyph_the_pbf_lacks_comes_from_local_fonts() {
    let mut test = GlyphStoreTest::with(local(EveryLocalGlyphRasterizer), ResponseType::Asynchronous);
    // The fixture has 'E', not 'A'.
    test.file_source.set_response(Kind::Glyphs, glyphs_response("test/fixtures/resources/fake_glyphs-0-255.pbf"));

    let available = Rc::new(Cell::new(false));
    let (a, run_loop) = (Rc::clone(&available), test.run_loop.clone());
    *test.requestor.glyphs_available.borrow_mut() = Some(Rc::new(move |glyphs: GlyphMap| {
        let glyphs = &glyphs[&font_stack_hash(&test_stack())];
        let local = glyphs[&gid('A')].as_ref().expect("a local 'A'");
        assert_eq!(local.metrics, stub_glyph(gid('A')).metrics);
        let pbf = glyphs[&gid('E')].as_ref().expect("the PBF's 'E'");
        assert_ne!(pbf.metrics, stub_glyph(gid('E')).metrics);
        a.set(true);
        run_loop.stop();
    }));

    test.run("test/fixtures/resources/glyphs.pbf", dependencies(&['A', 'E']));
    assert!(available.get());
}

// Without local ideographs, a CJK glyph comes from the PBF first.
#[test]
fn ideographs_come_from_pbfs_first_unless_local_ideographs_are_set() {
    let local = LocalGlyphs { ideographs: false, ..local(StubLocalGlyphRasterizer) };
    let mut test = GlyphStoreTest::with(local, ResponseType::Asynchronous);
    let requests = count_requests(&test);
    test.file_source.set_response(Kind::Glyphs, glyphs_response("test/fixtures/resources/glyphs-12244-12543.pbf"));

    let available = Rc::new(Cell::new(false));
    let (a, run_loop) = (Rc::clone(&available), test.run_loop.clone());
    *test.requestor.glyphs_available.borrow_mut() = Some(Rc::new(move |glyphs: GlyphMap| {
        let glyphs = &glyphs[&font_stack_hash(&test_stack())];
        let glyph = glyphs[&gid('々')].as_ref().expect("the PBF's glyph");
        assert_ne!(glyph.metrics, stub_glyph(gid('々')).metrics);
        a.set(true);
        run_loop.stop();
    }));

    test.run("test/fixtures/resources/glyphs-12244-12543.pbf", dependencies(&['々']));
    assert!(available.get());
    assert_eq!(requests.load(Ordering::SeqCst), 1);
}

// A local glyph comes from the fallback font faces that cover it before the
// device's fonts, one glyph at a time, and the device's are asked only about
// what the faces lack.
#[test]
fn a_fallback_face_draws_local_glyphs_before_the_device() {
    let engine = Arc::new(DeferredEngine::default());
    let katakana = StubChooser::new(|c| (c == 'テ').then_some("font"));
    let test = GlyphStoreTest::with_all(
        chooser(&katakana),
        ResponseType::Synchronous,
        Arc::default(),
        Arc::<DeferredEngine>::clone(&engine),
    );
    test.file_source.set_response(Kind::Glyphs, |resource: &Resource| -> Option<Response> {
        panic!("requested {}", resource.url);
    });
    let face = FontFace { type_: FONT_FACE, ..FontFace::new("", "app://face.ttf", vec![(0x4E2D, 0x4E2D)]) }
        .with_data(FontData::new(Arc::new(b"font".to_vec())));
    test.glyph_store().set_font_faces(Some(Arc::new(vec![face])));
    engine.report(&Ok(()));
    test.run_loop.run_once(&mut test.local());
    test.glyph_store().set_url("test/fixtures/resources/glyphs.pbf");

    // The device's fonts have only テ.
    let glyphs = test.run_until_answered(dependencies(&['中', 'テ']));
    let glyphs = &glyphs[&font_stack_hash(&test_stack())];
    assert_eq!(glyphs[&gid('中')].as_ref().expect("the face's glyph").id, gid('中'));
    assert_eq!(glyphs[&gid('テ')].as_ref().expect("the device's glyph").id, gid('テ'));
    assert_eq!(katakana.asked(), ['テ']);
    // The fallback face and the device's font.
    assert_eq!(engine.loads.load(Ordering::SeqCst), 2);
}

/// The device's fonts as three: one for Hebrew, one for Arabic and one for
/// everything else but marks, which none has on its own.
fn script_fonts(c: char) -> Option<&'static str> {
    match c {
        '\u{0300}'..='\u{036F}' => None,
        '\u{0590}'..='\u{05FF}' => Some("hebrew"),
        '\u{0600}'..='\u{06FF}' => Some("arabic"),
        _ => Some("latin"),
    }
}

/// Loads each font as a shaper that maps each character to the glyph of
/// that index, in its font's type, and shapes it as that glyph, reversed for
/// right-to-left text.
#[derive(Default)]
struct LetterEngine {
    /// The type and weight of each font loaded.
    loads: Mutex<Vec<(GlyphIDType, u16)>>,
    /// Whether its fonts fail to load, as files that aren't fonts do.
    invalid: bool,
    /// Whether shaping splits each character into its glyph and glyph 1, as
    /// `ccmp` splits a dotted Arabic letter into its dotless form and dots.
    splits: bool,
}

impl FontFaceEngine for LetterEngine {
    fn prepare(&self, ready: Box<dyn FnOnce(Result<(), String>) + Send>) {
        ready(Ok(()));
    }

    fn load(&self, type_: GlyphIDType, _data: FontData, weight: u16) -> Option<Box<dyn FontFaceShaper>> {
        self.loads.lock().push((type_, weight));
        (!self.invalid).then(|| -> Box<dyn FontFaceShaper> { Box::new(LetterShaper { type_, splits: self.splits }) })
    }
}

struct LetterShaper {
    type_: GlyphIDType,
    splits: bool,
}

impl FontFaceShaper for LetterShaper {
    fn shape(&self, text: &[char], rtl: bool, shaped: &mut ShapedText) {
        let mut glyphs: Vec<GlyphID> = Vec::new();
        for &c in text {
            glyphs.push(GlyphID::with_type(c as u16, self.type_));
            if self.splits {
                glyphs.push(GlyphID::with_type(1, self.type_));
            }
        }
        if rtl {
            glyphs.reverse();
        }
        shaped.adjusts.extend(glyphs.iter().map(|_| GlyphAdjust::new(0.0, 0.0, 10.0)));
        shaped.glyphs.extend(glyphs);
    }

    fn glyph_of(&self, c: char) -> Option<GlyphID> {
        Some(GlyphID::with_type(c as u16, self.type_))
    }

    fn rasterize(&self, glyph: GlyphID) -> Glyph {
        stub_glyph(glyph)
    }
}

/// A store on a `LetterEngine` that draws from `fonts`' fonts, with a
/// synchronous file source.
fn letter_test(fonts: &Arc<StubChooser>) -> (GlyphStoreTest, Arc<LetterEngine>) {
    letter_test_on(fonts, LetterEngine::default())
}

/// [`letter_test`], on `engine`.
fn letter_test_on(fonts: &Arc<StubChooser>, engine: LetterEngine) -> (GlyphStoreTest, Arc<LetterEngine>) {
    let engine = Arc::new(engine);
    let test = GlyphStoreTest::with_all(
        chooser(fonts),
        ResponseType::Synchronous,
        Arc::default(),
        Arc::<LetterEngine>::clone(&engine),
    );
    (test, engine)
}

/// The dependencies of `text`, shaped with the device's fonts.
fn system_text(text: &str) -> GlyphDependencies {
    let text: Vec<char> = text.chars().collect();
    let shapes = VecMap::from([(test_stack(), VecMap::from([(GlyphIDType::SYSTEM, [text].into())]))]);
    GlyphDependencies { glyphs: VecMap::new(), shapes }
}

/// The glyph type of the stub chooser's `font` at regular weight.
fn device_type(font: &str) -> GlyphIDType {
    FontFace::device(font, 400, FontData::default()).type_
}

/// The glyphs that the device's `fonts` shape `text` as, by character and
/// font.
fn shape_with_system_fonts(fonts: &Arc<StubChooser>, text: &str) -> Vec<(char, GlyphIDType)> {
    let (test, _) = letter_test(fonts);
    test.run_until_answered(system_text(text));
    let text: Vec<char> = text.chars().collect();
    let shaping = test.requestor.shaping.borrow();
    let shaped = &shaping.as_ref().expect("an answer")[&test_stack()][&GlyphIDType::SYSTEM][&text];
    shaped.glyphs.iter().map(|glyph| (char::from_u32(u32::from(glyph.code)).unwrap(), glyph.type_)).collect()
}

#[test]
fn the_devices_fonts_shape_each_piece_with_its_font() {
    let glyphs = shape_with_system_fonts(&StubChooser::new(script_fonts), "ab \u{05D0}\u{05D1}\u{062C} c\u{0301}");
    let chars: String = glyphs.iter().map(|(c, _)| *c).collect();
    // The right-to-left run's pieces in visual order: the Arabic after the
    // Hebrew comes first; the mark stays with the letter before it.
    assert_eq!(chars, "ab \u{062C}\u{05D1}\u{05D0} c\u{0301}");
    let type_of = |c: char| glyphs.iter().find(|(glyph, _)| *glyph == c).unwrap().1;
    assert_eq!(type_of('a'), type_of('c'));
    assert_eq!(type_of('\u{0301}'), type_of('c'));
    assert_ne!(type_of('\u{05D0}'), type_of('a'));
    assert_ne!(type_of('\u{062C}'), type_of('\u{05D0}'));
    assert!(glyphs.iter().all(|(_, type_)| ![GlyphIDType::FontPBF, GlyphIDType::SYSTEM].contains(type_)));
}

// A mark that its character's font lacks takes the cluster to a font that
// draws both, as a browser does, rather than splitting it across two.
#[test]
fn a_cluster_takes_a_font_that_draws_all_of_it() {
    let fonts = Arc::new(StubChooser {
        fonts: |_| Some("latin"),
        clusters: |cluster| (cluster == ['c', '\u{20E3}']).then_some("symbols"),
        weight: 400,
        asked: Mutex::default(),
    });
    let glyphs = shape_with_system_fonts(&fonts, "bc\u{20E3}");
    assert_eq!(glyphs.iter().map(|(c, _)| *c).collect::<String>(), "bc\u{20E3}");
    assert_eq!(glyphs[0].1, device_type("latin"));
    assert_eq!(glyphs[1].1, device_type("symbols"));
    assert_eq!(glyphs[2].1, device_type("symbols"));
}

#[test]
fn a_character_that_no_device_font_draws_is_left_out() {
    let glyphs = shape_with_system_fonts(&StubChooser::new(|c| (c != 'b').then_some("latin")), "abc");
    assert_eq!(glyphs.iter().map(|(c, _)| *c).collect::<String>(), "ac");
}

#[test]
fn the_devices_fonts_wait_for_the_engine() {
    let engine = Arc::new(DeferredEngine::default());
    let one_font = StubChooser::new(|_| Some("font"));
    let test = GlyphStoreTest::with_all(
        chooser(&one_font),
        ResponseType::Synchronous,
        Arc::default(),
        Arc::<DeferredEngine>::clone(&engine),
    );
    let requested = Rc::new(Cell::new(false));
    let r = Rc::clone(&requested);
    *test.observer.glyphs_requested.borrow_mut() = Some(Box::new(move |_, _| r.set(true)));
    let available: Rc<RefCell<Option<GlyphMap>>> = Rc::default();
    let a = Rc::clone(&available);
    *test.requestor.glyphs_available.borrow_mut() = Some(Rc::new(move |glyphs: GlyphMap| {
        *a.borrow_mut() = Some(glyphs);
    }));
    test.request_glyphs(system_text("\u{5}\u{6}"));
    // Chosen, and the chosen face's file parked for the engine.
    test.run_until(|| requested.get());
    assert!(available.borrow().is_none());
    assert_eq!(engine.loads.load(Ordering::SeqCst), 0);

    engine.report(&Ok(()));
    test.run_until(|| available.borrow().is_some());
    assert_eq!(engine.loads.load(Ordering::SeqCst), 1);
    let shaping = test.requestor.shaping.borrow();
    let shaped = &shaping.as_ref().expect("an answer")[&test_stack()][&GlyphIDType::SYSTEM][&vec!['\u{5}', '\u{6}']];
    assert_eq!(shaped.glyphs.iter().map(|glyph| glyph.code).collect::<Vec<_>>(), [5, 6]);
}

#[test]
fn a_chooser_never_runs_on_the_render_thread() {
    let fonts = StubChooser::new(|_| Some("latin"));
    let (test, _) = letter_test(&fonts);
    test.glyph_store().set_url("");
    let mut dependencies = system_text("ab");
    dependencies.glyphs = VecMap::from([(test_stack(), [gid('中')].into_iter().collect())]);
    test.run_until_answered(dependencies);
    let asked = fonts.asked.lock();
    assert!(!asked.is_empty());
    assert!(asked.iter().all(|(_, thread)| *thread != std::thread::current().id()));
}

#[test]
fn a_glyph_waits_for_its_device_font_then_draws_from_it() {
    let (test, _) = letter_test(&StubChooser::new(|_| Some("font")));
    test.glyph_store().set_url("");
    let events = Rc::new(RefCell::new(Vec::new()));
    let (requested, loaded) = (Rc::clone(&events), Rc::clone(&events));
    *test.observer.glyphs_requested.borrow_mut() =
        Some(Box::new(move |_, range| requested.borrow_mut().push(("requested", *range))));
    *test.observer.glyphs_loaded.borrow_mut() =
        Some(Box::new(move |_, range| loaded.borrow_mut().push(("loaded", *range))));

    let answer: Rc<RefCell<Option<GlyphMap>>> = Rc::default();
    let a = Rc::clone(&answer);
    *test.requestor.glyphs_available.borrow_mut() = Some(Rc::new(move |glyphs: GlyphMap| {
        *a.borrow_mut() = Some(glyphs);
    }));
    test.request_glyphs(dependencies(&['a']));
    assert!(answer.borrow().is_none(), "waits for the choice");
    assert!(events.borrow().is_empty());

    test.run_until(|| answer.borrow().is_some());
    let glyphs = answer.take().unwrap();
    let glyph = glyphs[&font_stack_hash(&test_stack())][&gid('a')].clone().expect("the device's glyph");
    assert_eq!(glyph.id, gid('a'));
    assert_eq!(glyph.metrics, stub_glyph(gid('a')).metrics);
    let range = get_glyph_range(GlyphID::with_type(0, device_type("font")));
    assert_eq!(*events.borrow(), [("requested", range), ("loaded", range)]);
}

#[test]
fn a_device_face_loads_at_its_weight() {
    let bold =
        Arc::new(StubChooser { fonts: |_| Some("bold"), clusters: |_| None, weight: 700, asked: Mutex::default() });
    let (test, engine) = letter_test(&bold);
    let face = FontFace { type_: FONT_FACE, ..FontFace::new("Test Stack", "app://face.ttf", vec![(0, 0xFFFF)]) }
        .with_data(FontData::from(b"font".to_vec()));
    test.glyph_store().set_font_faces(Some(Arc::new(vec![face])));
    let mut dependencies = system_text("a");
    dependencies.glyphs = VecMap::from([(test_stack(), [GlyphID::with_type(0, FONT_FACE)].into_iter().collect())]);
    test.run_until_answered(dependencies);
    let device = FontFace::device("bold", 700, FontData::default()).type_;
    let mut loads = engine.loads.lock().clone();
    loads.sort();
    let mut expected = vec![(FONT_FACE, 400), (device, 700)];
    expected.sort();
    assert_eq!(loads, expected);
}

#[test]
fn a_device_face_loads_without_a_file_source() {
    let katakana = StubChooser::new(|c| (c == 'テ').then_some("font"));
    let (test, engine) = letter_test(&katakana);
    *test.store.file_source.borrow_mut() = None;
    let face = FontFace { type_: FONT_FACE, ..FontFace::new("", "app://face.ttf", vec![(0x4E2D, 0x4E2D)]) }
        .with_data(FontData::from(b"font".to_vec()));
    test.glyph_store().set_font_faces(Some(Arc::new(vec![face])));
    test.glyph_store().set_url("");

    let glyphs = test.run_until_answered(dependencies(&['中', 'テ']));
    let glyphs = &glyphs[&font_stack_hash(&test_stack())];
    assert!(glyphs[&gid('中')].is_some(), "the fallback face's glyph");
    assert!(glyphs[&gid('テ')].is_some(), "the device's glyph");
    assert_eq!(engine.loads.lock().len(), 2);
}

/// A later wait requests a fallback face's file that failed to load again
/// and draws the face's glyphs once it loads.
#[test]
fn a_fallback_face_that_failed_to_load_is_requested_again() {
    let (test, _) = letter_test(&StubChooser::new(|_| None));
    let requests = Arc::new(AtomicU32::new(0));
    let r = Arc::clone(&requests);
    test.file_source.set_response(Kind::Glyphs, move |resource: &Resource| {
        assert_eq!(resource.url, "app://face.ttf");
        let mut response = Response::new();
        if r.fetch_add(1, Ordering::SeqCst) == 0 {
            response.error = Some(Box::new(ResourceError::new(Reason::Other, "Failed by the test case")));
        } else {
            response.data = Some(Arc::new(b"font".to_vec()));
        }
        Some(response)
    });
    let face = FontFace { type_: FONT_FACE, ..FontFace::new("", "app://face.ttf", vec![(0x4E2D, 0x4E2D)]) };
    test.glyph_store().set_font_faces(Some(Arc::new(vec![face])));
    test.glyph_store().set_url("");
    let range = (test_stack(), get_glyph_range(GlyphID::with_type(0, FONT_FACE)));

    let glyphs = test.run_until_answered(dependencies(&['中']));
    assert!(glyphs[&font_stack_hash(&test_stack())][&gid('中')].is_none());
    assert_eq!(*test.requestor.failed.borrow(), VecSet::from([range.clone()]), "the answer names the face");

    let glyphs = test.run_until_answered(dependencies(&['中']));
    assert!(glyphs[&font_stack_hash(&test_stack())][&gid('中')].is_some(), "the face's glyph");
    assert_eq!(requests.load(Ordering::SeqCst), 2);
    assert_eq!(test.glyph_store().take_recovered(), VecSet::from([range]));
}

/// Local fonts with every glyph, drawn wider than any other source's, so
/// that a test tells them apart.
struct WideLocalGlyphRasterizer;

impl LocalGlyphRasterizer for WideLocalGlyphRasterizer {
    fn rasterize_glyph(&self, _font_stack: &FontStack, glyph_id: GlyphID) -> Option<Glyph> {
        let glyph = stub_glyph(glyph_id);
        Some(Glyph { metrics: GlyphMetrics { advance: 99, ..glyph.metrics }, ..glyph })
    }
}

/// The advance of `c`'s glyph in `glyphs`, which has it.
fn advance(glyphs: &GlyphMap, c: char) -> u32 {
    glyphs[&font_stack_hash(&test_stack())][&gid(c)].as_ref().expect("the glyph").metrics.advance
}

/// The rasterizer stands in for a fallback face whose file failed to load,
/// until a later wait loads the file and draws the face's glyph.
#[test]
fn a_rasterized_glyph_stands_in_for_a_face_that_failed_to_load() {
    let local = LocalGlyphs { ideographs: true, chooser: None, rasterizer: Some(Arc::new(WideLocalGlyphRasterizer)) };
    let test =
        GlyphStoreTest::with_all(local, ResponseType::Synchronous, Arc::default(), Arc::new(LetterEngine::default()));
    test.file_source.set_response(Kind::Glyphs, fails_first_then(b"font"));
    let face = FontFace { type_: FONT_FACE, ..FontFace::new("", "app://face.ttf", vec![(0x4E2D, 0x4E2D)]) };
    test.glyph_store().set_font_faces(Some(Arc::new(vec![face])));
    test.glyph_store().set_url("");
    let range = (test_stack(), get_glyph_range(GlyphID::with_type(0, FONT_FACE)));

    let glyphs = test.run_until_answered(dependencies(&['中']));
    assert_eq!(advance(&glyphs, '中'), 99, "the rasterizer's glyph");
    assert_eq!(*test.requestor.failed.borrow(), VecSet::from([range.clone()]), "the answer names the face");

    let glyphs = test.run_until_answered(dependencies(&['中']));
    assert_ne!(advance(&glyphs, '中'), 99, "the face's glyph");
    assert_eq!(test.glyph_store().take_recovered(), VecSet::from([range]));
}

/// A local glyph stands in for a glyph PBF that failed to load, until a
/// later wait loads the PBF and draws its glyph.
#[test]
fn a_local_glyph_stands_in_for_a_range_that_failed_to_load() {
    let test = GlyphStoreTest::with(local(WideLocalGlyphRasterizer), ResponseType::Synchronous);
    test.file_source.set_response(Kind::Glyphs, fails_first_then_glyphs());
    test.glyph_store().set_url("test/fixtures/resources/glyphs.pbf");
    let range = (test_stack(), GlyphRange::new(0, 255, GlyphIDType::FontPBF));

    let glyphs = test.run_until_answered(dependencies(&['a']));
    assert_eq!(advance(&glyphs, 'a'), 99, "the rasterizer's glyph");
    assert_eq!(*test.requestor.failed.borrow(), VecSet::from([range.clone()]), "the answer names the range");

    let glyphs = test.run_until_answered(dependencies(&['a']));
    assert_ne!(advance(&glyphs, 'a'), 99, "the PBF's glyph");
    assert_eq!(test.glyph_store().take_recovered(), VecSet::from([range]));
}

// A device glyph is the glyph its font maps the character to, as a PBF's
// is, even where shaping the character alone splits it.
#[test]
fn a_device_glyph_is_its_fonts_glyph_of_the_character() {
    let engine = LetterEngine { splits: true, ..LetterEngine::default() };
    let (test, _) = letter_test_on(&StubChooser::new(|_| Some("arabic")), engine);
    test.glyph_store().set_url("");
    let glyphs = test.run_until_answered(dependencies(&['\u{0628}']));
    let glyph = glyphs[&font_stack_hash(&test_stack())][&gid('\u{0628}')].clone().expect("the device's glyph");
    assert_eq!(glyph.metrics, stub_glyph(gid('\u{0628}')).metrics);
}

#[test]
fn a_cjk_glyph_without_a_device_font_comes_from_the_pbf() {
    let (test, _) = letter_test(&StubChooser::new(|_| None));
    let requested = Rc::new(RefCell::new(Vec::new()));
    let r = Rc::clone(&requested);
    *test.observer.glyphs_requested.borrow_mut() = Some(Box::new(move |_, range| r.borrow_mut().push(*range)));
    test.file_source.set_response(Kind::Glyphs, glyphs_response("test/fixtures/resources/glyphs.pbf"));
    test.glyph_store().set_url("test/fixtures/resources/glyphs.pbf");
    test.run_until_answered(dependencies(&['中']));
    assert_eq!(*requested.borrow(), [get_glyph_range(gid('中'))]);
}

#[test]
fn a_device_face_that_fails_to_load_draws_nothing() {
    let engine = LetterEngine { invalid: true, ..LetterEngine::default() };
    let (test, _) = letter_test_on(&StubChooser::new(|_| Some("font")), engine);
    test.glyph_store().set_url("");
    let loaded = Rc::new(RefCell::new(Vec::new()));
    let l = Rc::clone(&loaded);
    *test.observer.glyphs_loaded.borrow_mut() = Some(Box::new(move |_, range| l.borrow_mut().push(*range)));
    let mut dependencies = system_text("b");
    dependencies.glyphs = VecMap::from([(test_stack(), [gid('a')].into_iter().collect())]);

    let glyphs = test.run_until_answered(dependencies);
    assert!(glyphs[&font_stack_hash(&test_stack())][&gid('a')].is_none());
    let shaping = test.requestor.shaping.borrow();
    let shaped = &shaping.as_ref().expect("an answer")[&test_stack()][&GlyphIDType::SYSTEM][&vec!['b']];
    assert_eq!(shaped.glyphs, []);
    assert_eq!(*loaded.borrow(), [get_glyph_range(GlyphID::with_type(0, device_type("font")))]);
}

#[test]
fn an_answer_for_an_evicted_stack_is_dropped() {
    let fonts = StubChooser::new(|_| Some("latin"));
    let (test, _) = letter_test(&fonts);
    let answered = Rc::new(Cell::new(false));
    let a = Rc::clone(&answered);
    *test.requestor.glyphs_available.borrow_mut() = Some(Rc::new(move |_: GlyphMap| a.set(true)));
    test.request_glyphs(system_text("a"));
    test.glyph_store().evict(&VecSet::new());

    let revision = test.glyph_store().revision();
    let face = FontFace::device("latin", 400, FontData::from(b"latin".to_vec()));
    test.glyph_store().take_device_fonts(&test_stack(), vec![(vec!['a'], Some(face))]);
    assert_eq!(test.glyph_store().revision(), revision);

    // The wait resolves again, asks again, and is answered.
    test.store.answer();
    test.run_until(|| answered.get());
    assert_eq!(fonts.asked().last(), Some(&'a'));
}

#[test]
fn characters_are_asked_once_per_stack() {
    let fonts = StubChooser::new(|_| Some("latin"));
    let (test, _) = letter_test(&fonts);
    test.request_glyphs(system_text("ab"));
    test.run_until_answered(system_text("ab"));
    let mut asked = fonts.asked();
    asked.sort_unstable();
    assert_eq!(asked, ['a', 'b']);
}
