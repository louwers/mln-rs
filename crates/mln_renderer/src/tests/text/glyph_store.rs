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
use std::time::Duration;

use mln_actor::{AsyncTask, RunLoop, Timer};
use mln_storage::response::{Reason, ResourceError};
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_storage::{FileSource, Kind, Resource, Response};
use mln_text::HBShapeAdjust;
use mln_text::glyph::{
    FontData, FontFace, GLYPH_RANGES_PER_FONT_STACK, GLYPHS_PER_GLYPH_RANGE, Glyph, GlyphDependencies, GlyphID,
    GlyphIDType, GlyphIDs, GlyphMap, GlyphMetrics, GlyphRange, HBShapeRequests,
    NON_IDEOGRAPH_GLYPH_RANGES_PER_FONT_STACK,
};
use mln_text::glyph_cache::GlyphCache;
use mln_text::local_glyph_rasterizer::LocalGlyphRasterizer;
use mln_text::tagged_string::font_stack_hash;
use mln_util::error::Error;
use mln_util::font_stack::FontStack;
use mln_util::i18n;
use mln_util::image::AlphaImage;
use mln_util::pool::{Pool, TaskGroup};
use mln_util::size::Size;
use mln_vec_map::{VecMap, VecSet};
use parking_lot::Mutex;

use super::glyph_store_bitmaps::{SDF_BITMAP, STUB_BITMAP};
use super::{SquelchLog, read_file};
use crate::text::font_face_engine::{FontFaceEngine, FontFaceShaper};
use crate::text::glyph_store::{GlyphStore, GlyphStoreObserver, GlyphWait, HBShapeResults};
use crate::tile::inbox::{Inbox, TileEvent};

fn stub_glyph(glyph_id: GlyphID) -> Glyph {
    Glyph {
        id: glyph_id,
        metrics: GlyphMetrics { width: 24, height: 24, left: 0.0, top: -8.0, advance: 24, ..Default::default() },
        bitmap: AlphaImage::from_slice(Size::new(30, 30), &STUB_BITMAP).unwrap(),
    }
}

struct StubLocalGlyphRasterizer;

impl LocalGlyphRasterizer for StubLocalGlyphRasterizer {
    fn can_rasterize_glyph(&self, _font_stack: &FontStack, glyph_id: GlyphID) -> bool {
        i18n::allows_fixed_width_glyph_generation(glyph_id.code)
    }

    fn rasterize_glyph(&self, _font_stack: &FontStack, glyph_id: GlyphID) -> Glyph {
        stub_glyph(glyph_id)
    }
}

struct KatakanaOnlyLocalGlyphRasterizer;

impl LocalGlyphRasterizer for KatakanaOnlyLocalGlyphRasterizer {
    fn can_rasterize_glyph(&self, _font_stack: &FontStack, glyph_id: GlyphID) -> bool {
        glyph_id.code == 'テ' as u16
    }

    fn rasterize_glyph(&self, _font_stack: &FontStack, glyph_id: GlyphID) -> Glyph {
        stub_glyph(glyph_id)
    }
}

struct LatinALocalGlyphRasterizer;

impl LocalGlyphRasterizer for LatinALocalGlyphRasterizer {
    fn can_rasterize_glyph(&self, _font_stack: &FontStack, glyph_id: GlyphID) -> bool {
        glyph_id == gid('a')
    }

    fn rasterize_glyph(&self, _font_stack: &FontStack, glyph_id: GlyphID) -> Glyph {
        stub_glyph(glyph_id)
    }
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

impl GlyphStoreObserver for StubGlyphStoreObserver {
    fn on_glyphs_loaded(&self, font_stack: &FontStack, range: GlyphRange) {
        if let Some(f) = self.glyphs_loaded.borrow_mut().as_mut() {
            f(font_stack, &range);
        }
    }

    fn on_glyphs_error(&self, font_stack: &FontStack, range: GlyphRange, error: Error) {
        if let Some(f) = self.glyphs_error.borrow_mut().as_mut() {
            f(font_stack, &range, error);
        }
    }

    fn on_glyphs_requested(&self, font_stack: &FontStack, range: GlyphRange) {
        if let Some(f) = self.glyphs_requested.borrow_mut().as_mut() {
            f(font_stack, &range);
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
    shaping: RefCell<Option<HBShapeResults>>,
    /// Whether the last answer lacked glyphs of a failed range.
    failed: Cell<bool>,
}

/// A request's wait for its answer.
struct Wait {
    requestor: Rc<StubGlyphRequestor>,
    dependencies: GlyphDependencies,
    wait: GlyphWait,
}

/// A store and the waits of its requestors, which it answers when it wakes.
struct Store {
    store: RefCell<GlyphStore>,
    /// Where the responses arrive.
    inbox: Inbox,
    /// Where the store parses glyph PBFs.
    pool: TaskGroup,
    file_source: Arc<dyn FileSource>,
    waits: RefCell<Vec<Wait>>,
    _wake: AsyncTask,
}

impl Store {
    fn new(
        local_glyph_rasterizer: Box<dyn LocalGlyphRasterizer>,
        glyph_cache: Arc<GlyphCache>,
        engine: Arc<dyn FontFaceEngine>,
        file_source: Arc<dyn FileSource>,
        run_loop: &RunLoop,
    ) -> Rc<Store> {
        Rc::new_cyclic(|this: &Weak<Store>| {
            let this = Weak::clone(this);
            let wake = AsyncTask::new(run_loop, move || {
                if let Some(this) = this.upgrade() {
                    this.take_responses();
                    this.store.borrow_mut().update();
                    this.answer();
                }
            });
            let sender = wake.sender();
            let pool = TaskGroup::new(&Pool::new(4).expect("a pool"));
            let store = GlyphStore::new(
                Some(local_glyph_rasterizer),
                glyph_cache,
                pool.clone(),
                engine,
                run_loop.clone(),
                Rc::new(move || sender.send()),
            );
            let sender = wake.sender();
            let inbox = Inbox::new(Arc::new(move || sender.send()));
            Store { store: RefCell::new(store), inbox, pool, file_source, waits: RefCell::default(), _wake: wake }
        })
    }

    /// Asks for `dependencies` for `requestor`, which gets them once the
    /// store has them.
    fn get_glyphs(&self, requestor: &Rc<StubGlyphRequestor>, dependencies: GlyphDependencies) {
        self.waits.borrow_mut().push(Wait {
            requestor: Rc::clone(requestor),
            dependencies,
            wait: GlyphWait::default(),
        });
        self.answer();
    }

    /// Gives the store the responses that arrived.
    fn take_responses(&self) {
        while let Some(event) = self.inbox.next() {
            if let TileEvent::Glyphs { font_stack, range, id, result } = event {
                self.store.borrow_mut().take_response(&font_stack, range, id, result);
            }
        }
    }

    /// Answers the waits the store can answer now.
    fn answer(&self) {
        let waits = std::mem::take(&mut *self.waits.borrow_mut());
        let mut answered = Vec::new();
        for mut wait in waits {
            let resolved = self.store.borrow_mut().resolve(
                &wait.dependencies,
                &mut wait.wait,
                Some(&self.file_source),
                self.inbox.sender(),
            );
            match resolved {
                Some(resolved) => answered.push((wait.requestor, resolved)),
                None => self.waits.borrow_mut().push(wait),
            }
        }
        for (requestor, resolved) in answered {
            *requestor.shaping.borrow_mut() = Some(resolved.shaping);
            requestor.failed.set(resolved.failed);
            let f = requestor.glyphs_available.borrow().clone();
            if let Some(f) = f {
                f(resolved.glyphs);
            }
        }
    }
}

struct GlyphStoreTest {
    run_loop: RunLoop,
    file_source: Arc<StubFileSource>,
    observer: Rc<StubGlyphStoreObserver>,
    requestor: Rc<StubGlyphRequestor>,
    store: Rc<Store>,
    squelch_log: Option<SquelchLog>,
}

impl GlyphStoreTest {
    fn new() -> Self {
        Self::with(Box::new(StubLocalGlyphRasterizer), ResponseType::Asynchronous)
    }

    fn with(local_glyph_rasterizer: Box<dyn LocalGlyphRasterizer>, response_type: ResponseType) -> Self {
        Self::with_all(
            local_glyph_rasterizer,
            response_type,
            Arc::default(),
            crate::text::font_face_engine::default_engine(),
        )
    }

    fn with_glyph_cache(local_glyph_rasterizer: Box<dyn LocalGlyphRasterizer>, glyph_cache: Arc<GlyphCache>) -> Self {
        Self::with_all(
            local_glyph_rasterizer,
            ResponseType::Asynchronous,
            glyph_cache,
            crate::text::font_face_engine::default_engine(),
        )
    }

    fn with_all(
        local_glyph_rasterizer: Box<dyn LocalGlyphRasterizer>,
        response_type: ResponseType,
        glyph_cache: Arc<GlyphCache>,
        engine: Arc<dyn FontFaceEngine>,
    ) -> Self {
        let run_loop = RunLoop::new();
        let file_source = StubFileSource::on_loop(&run_loop, response_type);
        let observer = Rc::new(StubGlyphStoreObserver::default());
        let source: Arc<dyn FileSource> = Arc::<StubFileSource>::clone(&file_source);
        let store = Store::new(local_glyph_rasterizer, glyph_cache, engine, source, &run_loop);
        store.store.borrow_mut().set_observer(Some(Rc::<StubGlyphStoreObserver>::clone(&observer)));
        GlyphStoreTest {
            file_source,
            observer,
            requestor: Rc::new(StubGlyphRequestor::default()),
            store,
            run_loop,
            squelch_log: None,
        }
    }

    fn glyph_store(&self) -> std::cell::RefMut<'_, GlyphStore> {
        self.store.store.borrow_mut()
    }

    fn get_glyphs(&self, dependencies: GlyphDependencies) {
        self.store.get_glyphs(&self.requestor, dependencies);
    }

    fn run(&mut self, url: &str, dependencies: GlyphDependencies) {
        // Squelch logging.
        self.squelch_log = Some(SquelchLog::new());

        self.glyph_store().set_url(url);
        self.get_glyphs(dependencies);

        self.run_loop.run();
    }

    fn end(&self) {
        self.run_loop.stop();
    }

    /// Loads `chars` of `font_stack` from the glyphs.pbf fixture.
    fn load(&mut self, font_stack: FontStack, chars: &[char]) {
        self.file_source.set_response(Kind::Glyphs, glyphs_response("test/fixtures/resources/glyphs.pbf"));
        let run_loop = self.run_loop.clone();
        *self.requestor.glyphs_available.borrow_mut() = Some(Rc::new(move |_: GlyphMap| run_loop.stop()));
        let ids: GlyphIDs = chars.iter().map(|c| gid(*c)).collect();
        let dependencies =
            GlyphDependencies { glyphs: VecMap::from([(font_stack, ids)]), shapes: HBShapeRequests::default() };
        self.run("test/fixtures/resources/glyphs.pbf", dependencies);
    }
}

fn test_stack() -> FontStack {
    vec!["Test Stack".to_owned()]
}

fn dependencies(chars: &[char]) -> GlyphDependencies {
    let ids: GlyphIDs = chars.iter().map(|c| GlyphID::new(*c as u32)).collect();
    GlyphDependencies { glyphs: VecMap::from([(test_stack(), ids)]), shapes: HBShapeRequests::default() }
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

/// A file source response that fails on the first request (`requests` is 1)
/// and serves the glyphs.pbf fixture after that.
fn fails_first_then_glyphs(requests: Arc<AtomicU32>) -> impl Fn(&Resource) -> Option<Response> + Send + Sync {
    move |_: &Resource| {
        let mut response = Response::new();
        if requests.load(Ordering::SeqCst) == 1 {
            response.error = Some(Box::new(ResourceError::new(Reason::NotFound, "Failed by the test case")));
        } else {
            response.data = Some(Arc::new(read_file("test/fixtures/resources/glyphs.pbf")));
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
    test.file_source.set_response(Kind::Glyphs, fails_first_then_glyphs(Arc::clone(&requests)));

    let answers = Rc::new(Cell::new(0));
    let retry = Rc::new(Timer::new(&test.run_loop));
    let (n, run_loop) = (Rc::clone(&answers), test.run_loop.clone());
    let store = Rc::downgrade(&test.store);
    let requestor = Rc::downgrade(&test.requestor);
    let retry_timer = Rc::clone(&retry);
    *test.requestor.glyphs_available.borrow_mut() = Some(Rc::new(move |glyphs: GlyphMap| {
        let test_positions = &glyphs[&font_stack_hash(&test_stack())];
        n.set(n.get() + 1);
        if n.get() == 1 {
            assert!(test_positions[&gid('a')].is_none());
            // C++ asks again through `loop.invoke`, after this response is
            // processed.
            let (store, requestor) = (Weak::clone(&store), Weak::clone(&requestor));
            retry_timer.start(Duration::ZERO, Duration::ZERO, move || {
                store.upgrade().unwrap().get_glyphs(&requestor.upgrade().unwrap(), dependencies(&['a']));
            });
        } else {
            assert!(test_positions[&gid('a')].is_some());
            run_loop.stop();
        }
    }));

    let timeout = Timer::new(&test.run_loop);
    let run_loop = test.run_loop.clone();
    timeout.start(Duration::from_millis(1000), Duration::ZERO, move || run_loop.stop());

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
    test.file_source.set_response(Kind::Glyphs, fails_first_then_glyphs(Arc::clone(&requests)));
    let run_loop = test.run_loop.clone();
    *test.requestor.glyphs_available.borrow_mut() = Some(Rc::new(move |_: GlyphMap| run_loop.stop()));

    test.run("test/fixtures/resources/glyphs.pbf", dependencies(&['a']));
    assert!(test.requestor.failed.get());
    assert_eq!(requests.load(Ordering::SeqCst), 1);

    // A wait that began after the failure requests the range again, which
    // loads.
    test.run("test/fixtures/resources/glyphs.pbf", dependencies(&['a']));
    assert_eq!(requests.load(Ordering::SeqCst), 2);
    assert!(!test.requestor.failed.get());
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
    let _log = SquelchLog::new();
    test.glyph_store().set_url("test/fixtures/resources/glyphs.pbf");

    let file_source = Arc::clone(&test.store.file_source);
    let mut wait = GlyphWait::default();
    let inbox = test.store.inbox.sender().clone();
    assert!(test.glyph_store().resolve(&dependencies(&['a']), &mut wait, Some(&file_source), &inbox).is_none());
    test.run_loop.run_once();
    test.store.take_responses();
    test.glyph_store().update();
    for _ in 0..2 {
        let resolved =
            test.glyph_store().resolve(&dependencies(&['a']), &mut wait, Some(&file_source), &inbox).expect("answered");
        assert!(resolved.failed);
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

    let timeout = Timer::new(&test.run_loop);
    let run_loop = test.run_loop.clone();
    timeout.start(Duration::from_millis(1000), Duration::ZERO, move || run_loop.stop());

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
        let mut test = GlyphStoreTest::with_glyph_cache(Box::new(StubLocalGlyphRasterizer), Arc::clone(&cache));
        let font_stack = vec![font_stack.to_owned()];
        test.load(font_stack.clone(), &['a']);
        test.glyph_store().get_glyph(&font_stack, gid('a'))
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
        test.glyph_store().get_glyph(&test_stack(), gid('a'))
    };

    let first = load();

    assert!(!Arc::ptr_eq(&first, &load()));
}

// gtest: GlyphManager.ReleasesTheGlyphsOfEvictedFontStacks
#[test]
fn releases_the_glyphs_of_evicted_font_stacks() {
    let cache = Arc::new(GlyphCache::new());
    let mut test = GlyphStoreTest::with_glyph_cache(Box::new(StubLocalGlyphRasterizer), Arc::clone(&cache));
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
        shapes: HBShapeRequests::default(),
    };
    test.get_glyphs(dependencies);

    test.glyph_store().evict(&VecSet::new());
    // The renderer's update asks the waiting tiles again after an eviction.
    test.store.answer();

    let timeout = Timer::new(&test.run_loop);
    let run_loop = test.run_loop.clone();
    timeout.start(Duration::from_millis(1000), Duration::ZERO, move || run_loop.stop());
    test.run_loop.run();

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
    let test = GlyphStoreTest::with(Box::new(StubLocalGlyphRasterizer), ResponseType::Synchronous);
    let urls = Arc::new(Mutex::new(Vec::new()));
    let u = Arc::clone(&urls);
    test.file_source.set_response(Kind::Glyphs, move |resource: &Resource| {
        u.lock().push(resource.url.clone());
        glyphs_response("test/fixtures/resources/glyphs.pbf")(resource)
    });

    test.glyph_store().set_url("old/{fontstack}/{range}.pbf");
    test.get_glyphs(dependencies(&['a']));
    test.glyph_store().evict(&VecSet::new());
    test.store.take_responses();

    test.glyph_store().set_url("new/{fontstack}/{range}.pbf");
    test.get_glyphs(dependencies(&['a']));
    let urls = urls.lock();
    assert_eq!(urls.len(), 2, "requested again: {urls:?}");
    assert!(urls[1].starts_with("new/"));
}

// gtest: GlyphManager.DoesNotKeepTheRangeGlyphsItDrawsLocally
#[test]
fn does_not_keep_the_range_glyphs_it_draws_locally() {
    let loaded_glyphs = |rasterizer: Box<dyn LocalGlyphRasterizer>| {
        let cache = Arc::new(GlyphCache::new());
        let mut test = GlyphStoreTest::with_glyph_cache(rasterizer, Arc::clone(&cache));
        test.load(test_stack(), &['a', 'å']);
        cache.size()
    };

    let all = loaded_glyphs(Box::new(StubLocalGlyphRasterizer));

    // The fixture has 'a', the only glyph LatinALocalGlyphRasterizer draws.
    assert_eq!(all - 1, loaded_glyphs(Box::new(LatinALocalGlyphRasterizer)));
}

// gtest: GlyphManager.LoadingCancel
#[test]
fn loading_cancel() {
    let mut test = GlyphStoreTest::new();

    let run_loop = test.run_loop.handle();
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
    let mut test = GlyphStoreTest::with(Box::new(KatakanaOnlyLocalGlyphRasterizer), ResponseType::Asynchronous);
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
            store.upgrade().unwrap().get_glyphs(
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
    let mut test = GlyphStoreTest::with(
        Box::new(mln_text::local_glyph_rasterizer::DefaultLocalGlyphRasterizer::new(None)),
        ResponseType::Synchronous,
    );

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
    let test = GlyphStoreTest::with(Box::new(StubLocalGlyphRasterizer), ResponseType::Synchronous);
    let _log = SquelchLog::new();
    let requests = count_requests(&test);
    test.file_source.set_response(Kind::Glyphs, fails_first_then_glyphs(Arc::clone(&requests)));

    let answers = Rc::new(RefCell::new(Vec::new()));
    let a = Rc::clone(&answers);
    *test.requestor.glyphs_available.borrow_mut() = Some(Rc::new(move |glyphs: GlyphMap| {
        a.borrow_mut().push(glyphs[&font_stack_hash(&test_stack())][&gid('a')].is_some());
    }));

    test.glyph_store().set_url("test/fixtures/resources/glyphs.pbf");

    // The failure is taken in when the store wakes the renderer, and the
    // glyphs once the pool has parsed them.
    test.get_glyphs(dependencies(&['a']));
    test.run_loop.run_once();
    test.get_glyphs(dependencies(&['a']));
    test.store.pool.wait();
    test.run_loop.run_once();

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

    fn load(&self, _type: GlyphIDType, data: FontData) -> Option<Box<dyn FontFaceShaper>> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        (data.as_bytes() == b"font").then(|| -> Box<dyn FontFaceShaper> { Box::new(FakeShaper) })
    }
}

/// Shapes each code unit as the glyph of that index, 10 px wide.
struct FakeShaper;

impl FontFaceShaper for FakeShaper {
    fn shape(&self, text: &[u16], _rtl: bool, glyph_ids: &mut Vec<GlyphID>, adjusts: &mut Vec<HBShapeAdjust>) {
        for &unit in text {
            glyph_ids.push(GlyphID::with_type(unit, FONT_FACE));
            adjusts.push(HBShapeAdjust::new(0.0, 0.0, 10.0));
        }
    }

    fn rasterize(&self, glyph: &GlyphID) -> Glyph {
        stub_glyph(*glyph)
    }
}

const FONT_FACE: GlyphIDType = GlyphIDType::face(NonZeroU16::new(7).unwrap());

/// A store on `engine` with one font face, and a synchronous file source
/// that answers its file with "font".
fn font_face_test(engine: &Arc<DeferredEngine>) -> GlyphStoreTest {
    let test = GlyphStoreTest::with_all(
        Box::new(StubLocalGlyphRasterizer),
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
    let face = FontFace {
        type_: FONT_FACE,
        name: "Test Stack".to_owned(),
        url: "test://face.ttf".to_owned(),
        ranges: vec![],
        data: None,
    };
    test.glyph_store().set_font_faces(Some(Arc::new(vec![face])));
    GlyphStoreTest { squelch_log: Some(SquelchLog::new()), ..test }
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
    let shapes = VecMap::from([(test_stack(), VecMap::from([(FONT_FACE, [vec![5, 6]].into_iter().collect())]))]);
    test.get_glyphs(GlyphDependencies { glyphs: VecMap::from([(test_stack(), ids)]), shapes });
    available
}

/// The shaping of "\u{5}\u{6}" in the requestor's last answer.
fn shaped(test: &GlyphStoreTest) -> Vec<u16> {
    let shaping = test.requestor.shaping.borrow();
    let shaping = shaping.as_ref().expect("an answer");
    shaping[&test_stack()][&FONT_FACE][&vec![5, 6]].str_.clone()
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
    test.run_loop.run_once();
    assert!(available.borrow().is_none());
    assert_eq!(engine.loads.load(Ordering::SeqCst), 0);

    // Once it is, the store wakes the renderer, which parses the file and
    // answers the wait.
    engine.report(&Ok(()));
    assert!(available.borrow().is_none());
    test.run_loop.run_once();
    test.run_loop.run_once();
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
    test.run_loop.run_once();
    test.run_loop.run_once();
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
    test.run_loop.run_once();

    let available = request_font_face(&test);
    assert_eq!(engine.loads.load(Ordering::SeqCst), 1);
    assert!(available.borrow_mut().take().is_some(), "answered at once");
    assert_eq!(shaped(&test), [5, 6]);
}
