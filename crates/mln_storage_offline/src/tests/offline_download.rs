// The test that renders a map after a download is in
// tests/offline_download_map.rs. Fixtures are read in place from
// test/fixtures/offline_download.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};

use mln_renderer::Registry;
use mln_storage::offline::{
    OfflineRegion, OfflineRegionDefinition, OfflineRegionDownloadState, OfflineRegionMetadata, OfflineRegionObserver,
    OfflineRegionStatus, OfflineTilePyramidRegionDefinition,
};
use mln_storage::offline_database::{DatabaseError, OfflineDatabase};
use mln_storage::resource::{Priority, Usage};
use mln_storage::response::{Reason, ResourceError};
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_storage::{FileSource, Kind, LoadingMethod, Resource, Responder, Response, TileFormat};
use mln_style::geo::{LatLng, LatLngBounds};
use mln_style::tileset::Scheme;
use mln_text::glyph::NON_IDEOGRAPH_GLYPH_RANGES_PER_FONT_STACK;
use mln_util::chrono::{Timestamp, now};
use mln_util::compression::compress;
use mln_util::queue::Queue;
use parking_lot::{Mutex, MutexGuard};

use super::cpp_path;
use crate::offline_download::{Downloads, OfflineDownload, Post};

/// The database and its downloads, which the test's thread owns as a
/// database's worker does.
struct Host {
    db: OfflineDatabase,
    downloads: Downloads,
}

/// Forwards to an observer that a test keeps.
struct Shared(Rc<dyn OfflineRegionObserver>);

impl OfflineRegionObserver for Shared {
    fn status_changed(&self, status: OfflineRegionStatus) {
        self.0.status_changed(status);
    }

    fn response_error(&self, error: ResourceError) {
        self.0.response_error(error);
    }
}

/// A download in the test's host.
#[derive(Clone)]
struct TestDownload {
    host: Rc<RefCell<Host>>,
    post: Post,
    id: i64,
}

impl TestDownload {
    fn with<R>(&self, f: impl FnOnce(&mut OfflineDownload, &mut OfflineDatabase) -> R) -> R {
        let mut host = self.host.borrow_mut();
        let Host { db, downloads } = &mut *host;
        f(downloads.get_mut(self.id).expect("the download"), db)
    }

    fn set_observer(&self, observer: Option<Rc<dyn OfflineRegionObserver>>) {
        self.with(|download, _| {
            download.set_observer(
                observer.map(|observer| -> Box<dyn OfflineRegionObserver> { Box::new(Shared(observer)) }),
            );
        });
    }

    fn set_state(&self, state: OfflineRegionDownloadState) {
        self.with(|download, _| download.set_state(state));
    }

    fn get_status(&self) -> OfflineRegionStatus {
        self.with(|download, db| download.get_status(db))
    }

    fn set_maximum_concurrent_requests(&self, maximum: u32) {
        self.with(|download, _| download.set_maximum_concurrent_requests(maximum));
    }

    /// A handle for the download's observer, which reaches the download in a
    /// task of the test's loop, after the download's work in progress.
    fn downgrade(&self) -> LaterDownload {
        LaterDownload { post: Arc::clone(&self.post), id: self.id }
    }
}

/// What an observer does to its download, in a task.
#[derive(Clone)]
struct LaterDownload {
    post: Post,
    id: i64,
}

impl LaterDownload {
    fn set_state(&self, state: OfflineRegionDownloadState) {
        let id = self.id;
        (self.post)(Box::new(move |_, downloads| {
            if let Some(download) = downloads.get_mut(id) {
                download.set_state(state);
            }
        }));
    }
}

#[derive(Default)]
struct MockObserver {
    status_changed_fn: Option<Box<dyn Fn(OfflineRegionStatus)>>,
    response_error_fn: Option<Box<dyn Fn(ResourceError)>>,
}

impl OfflineRegionObserver for MockObserver {
    fn status_changed(&self, status: OfflineRegionStatus) {
        if let Some(f) = &self.status_changed_fn {
            f(status);
        }
    }

    fn response_error(&self, error: ResourceError) {
        if let Some(f) = &self.response_error_fn {
            f(error);
        }
    }
}

/// `test.response(path)`: the fixture, adding its stored size to `size`.
fn fixture_response(size: &AtomicU64, path: &str) -> Response {
    let mut result = Response::new();
    let data = std::fs::read(cpp_path(&format!("test/fixtures/offline_download/{path}")))
        .unwrap_or_else(|e| panic!("Cannot read file {path}: {e}"));
    let uncompressed = data.len() as u64;
    let compressed = compress(&data).unwrap().len() as u64;
    size.fetch_add(uncompressed.min(compressed), Ordering::SeqCst);
    result.data = Some(Arc::new(data));
    result
}

/// The test thread's loop: a queue whose tasks run with the test's host,
/// and the closures that [`schedule`](Self::schedule) defers. Clones share
/// the loop.
#[derive(Clone)]
struct TestLoop {
    queue: Queue<Local>,
    local: Local,
}

/// A closure that [`TestLoop::schedule`] defers.
type Later = Box<dyn FnOnce()>;

/// What the loop's tasks run with, on the test's thread.
#[derive(Clone)]
struct Local {
    host: Rc<RefCell<Host>>,
    later: Rc<RefCell<VecDeque<Later>>>,
}

impl TestLoop {
    fn run(&self) {
        self.queue.run(&mut self.local.clone());
    }

    fn run_once(&self) -> bool {
        self.queue.run_once(&mut self.local.clone())
    }

    fn stop(&self) {
        self.queue.stop();
    }

    /// Runs `f` in a later turn of the loop.
    fn schedule(&self, f: impl FnOnce() + 'static) {
        self.local.later.borrow_mut().push_back(Box::new(f));
        self.queue.post(|local| {
            let f = local.later.borrow_mut().pop_front();
            if let Some(f) = f {
                f();
            }
        });
    }
}

struct OfflineTest {
    run_loop: TestLoop,
    file_source: Arc<StubFileSource>,
    host: Rc<RefCell<Host>>,
    post: Post,
    size: Arc<AtomicU64>,
}

impl OfflineTest {
    fn new() -> Self {
        Self::with_path(":memory:")
    }

    fn with_path(path: &str) -> Self {
        let queue = Queue::new();
        let file_source = StubFileSource::on_queue(&queue, ResponseType::Asynchronous);
        // The downloads' tasks run on the test's loop, with the host.
        let handle = queue.downgrade();
        let post: Post = Arc::new(move |task| {
            if let Some(queue) = handle.upgrade() {
                queue.post(move |local: &mut Local| {
                    let mut host = local.host.borrow_mut();
                    let Host { db, downloads } = &mut *host;
                    task(db, downloads);
                });
            }
        });
        let network: Arc<dyn FileSource> = Arc::<StubFileSource>::clone(&file_source);
        let host = Rc::new(RefCell::new(Host {
            db: OfflineDatabase::new(path),
            downloads: Downloads::new(Some(network), Arc::clone(&post), Arc::clone(Registry::with_builtins().style())),
        }));
        let run_loop = TestLoop { queue, local: Local { host: Rc::clone(&host), later: Rc::default() } };
        OfflineTest { run_loop, file_source, host, post, size: Arc::default() }
    }

    fn db(&self) -> std::cell::RefMut<'_, OfflineDatabase> {
        std::cell::RefMut::map(self.host.borrow_mut(), |host| &mut host.db)
    }

    /// Adds the download of `region_id` over `file_source` to the host.
    fn add_download(
        &self,
        region_id: i64,
        definition: OfflineRegionDefinition,
        file_source: Arc<dyn FileSource>,
    ) -> TestDownload {
        let download = OfflineDownload::new(
            region_id,
            definition,
            file_source,
            Arc::clone(&self.post),
            Arc::clone(Registry::with_builtins().style()),
        );
        self.host.borrow_mut().downloads.insert(download);
        TestDownload { host: Rc::clone(&self.host), post: Arc::clone(&self.post), id: region_id }
    }

    fn create_region(&self) -> Result<OfflineRegion, DatabaseError> {
        let definition = OfflineTilePyramidRegionDefinition::new(
            "",
            LatLngBounds::hull(LatLng::new(1.0, 2.0).unwrap(), LatLng::new(3.0, 4.0).unwrap()),
            5.0,
            6.0,
            1.0,
            true,
        )
        .unwrap();
        let metadata = OfflineRegionMetadata::new();
        self.db().create_region(&definition.into(), &metadata)
    }

    fn response(&self, path: &str) -> Response {
        fixture_response(&self.size, path)
    }

    fn respond(&self, kind: Kind, f: impl Fn(&Resource) -> Response + Send + Sync + 'static) {
        self.file_source.set_response(kind, move |resource| Some(f(resource)));
    }

    /// A responder that returns the fixture at `path`.
    fn respond_with(&self, kind: Kind, path: &'static str, check: impl Fn(&Resource) + Send + Sync + 'static) {
        let size = Arc::clone(&self.size);
        self.respond(kind, move |resource| {
            check(resource);
            fixture_response(&size, path)
        });
    }

    fn download(&self, region_id: i64, style_url: &str, include_ideographs: bool) -> TestDownload {
        self.download_with_zoom(
            region_id,
            style_url,
            0.0,
            0.0,
            include_ideographs,
            Arc::<StubFileSource>::clone(&self.file_source),
        )
    }

    fn download_with_zoom(
        &self,
        region_id: i64,
        style_url: &str,
        min_zoom: f64,
        max_zoom: f64,
        include_ideographs: bool,
        file_source: Arc<dyn FileSource>,
    ) -> TestDownload {
        self.add_download(
            region_id,
            OfflineTilePyramidRegionDefinition::new(
                style_url,
                LatLngBounds::world(),
                min_zoom,
                max_zoom,
                1.0,
                include_ideographs,
            )
            .unwrap()
            .into(),
            file_source,
        )
    }
}

const STYLE_URL: &str = "http://127.0.0.1:3000/style.json";

fn expect_style_url(resource: &Resource) {
    assert_eq!(STYLE_URL, resource.url);
}

fn expect_tile_0_0_0(template: &'static str) -> impl Fn(&Resource) {
    move |resource: &Resource| {
        let tile = resource.tile_data.as_ref().expect("tile data");
        assert_eq!(template, tile.url_template);
        assert_eq!(1, tile.pixel_ratio);
        assert_eq!(0, tile.x);
        assert_eq!(0, tile.y);
        assert_eq!(0, tile.z);
    }
}

/// The responders of the tests that download style.json and all its
/// resources (Activate, `ExcludeIdeographs`, `InterruptAndResume`,
/// `NoFreezingOnNotFoundError` but its sprite JSON).
fn respond_to_full_style(test: &OfflineTest, sprite_json: bool) {
    test.respond_with(Kind::Style, "style.json", expect_style_url);
    test.respond_with(Kind::SpriteImage, "sprite.png", |resource| {
        assert!(
            resource.url == "http://127.0.0.1:3000/sprite.png" || resource.url == "http://127.0.0.1:3000/sprite@2x.png"
        );
    });
    test.respond_with(Kind::Image, "radar.gif", |resource| {
        assert_eq!("http://127.0.0.1:3000/radar.gif", resource.url);
    });
    if sprite_json {
        test.respond_with(Kind::SpriteJSON, "sprite.json", |resource| {
            assert!(
                resource.url == "http://127.0.0.1:3000/sprite.json"
                    || resource.url == "http://127.0.0.1:3000/sprite@2x.json"
            );
        });
    }
    test.respond_with(Kind::Glyphs, "glyph.pbf", |_| {});
    test.respond_with(Kind::Source, "streets.json", |resource| {
        assert_eq!("http://127.0.0.1:3000/streets.json", resource.url);
    });
    test.respond_with(
        Kind::Tile,
        "0-0-0.vector.pbf",
        expect_tile_0_0_0("http://127.0.0.1:3000/{z}-{x}-{y}.vector.pbf"),
    );
}

// gtest: OfflineDownload.NoSubresources
#[test]
fn no_subresources() {
    let test = OfflineTest::new();
    let region = test.create_region().expect("region");
    let download = test.download(region.id, STYLE_URL, false);

    test.respond_with(Kind::Style, "empty.style.json", expect_style_url);

    let expects_inactive_status = Cell::new(false);
    let (size, stop) = (Arc::clone(&test.size), test.run_loop.clone());
    let observer = MockObserver {
        status_changed_fn: Some(Box::new(move |status: OfflineRegionStatus| {
            if status.complete() {
                if expects_inactive_status.get() {
                    assert_eq!(OfflineRegionDownloadState::Inactive, status.download_state);
                    assert_eq!(1, status.completed_resource_count);
                    assert_eq!(size.load(Ordering::SeqCst), status.completed_resource_size);
                    assert!(status.required_resource_count_is_precise);
                    stop.stop();
                } else {
                    expects_inactive_status.set(true);
                    assert_eq!(OfflineRegionDownloadState::Active, status.download_state);
                    assert_eq!(1, status.completed_resource_count);
                    assert_eq!(size.load(Ordering::SeqCst), status.completed_resource_size);
                    assert!(status.required_resource_count_is_precise);
                }
            }
        })),
        ..Default::default()
    };

    download.set_observer(Some(Rc::new(observer)));
    download.set_state(OfflineRegionDownloadState::Active);

    test.run_loop.run();
}

// gtest: OfflineDownload.InlineSource
#[test]
fn inline_source() {
    let test = OfflineTest::new();
    let region = test.create_region().expect("region");
    let download = test.download(region.id, STYLE_URL, true);

    test.respond_with(Kind::Style, "inline_source.style.json", expect_style_url);
    test.respond_with(
        Kind::Tile,
        "0-0-0.vector.pbf",
        expect_tile_0_0_0("http://127.0.0.1:3000/{z}-{x}-{y}.vector.pbf"),
    );

    let (size, stop) = (Arc::clone(&test.size), test.run_loop.clone());
    let observer = MockObserver {
        status_changed_fn: Some(Box::new(move |status: OfflineRegionStatus| {
            if status.complete() {
                assert_eq!(1, status.completed_tile_count);
                assert_eq!(2, status.completed_resource_count);
                assert_eq!(size.load(Ordering::SeqCst), status.completed_resource_size);
                assert!(status.required_resource_count_is_precise);
                stop.stop();
            }
        })),
        ..Default::default()
    };

    download.set_observer(Some(Rc::new(observer)));
    download.set_state(OfflineRegionDownloadState::Active);

    test.run_loop.run();
}

// gtest: OfflineDownload.GeoJSONSource
#[test]
fn geojson_source() {
    let test = OfflineTest::new();
    let region = test.create_region().expect("region");
    let download = test.download(region.id, STYLE_URL, false);

    test.respond_with(Kind::Style, "geojson_source.style.json", expect_style_url);
    test.respond_with(Kind::Source, "geojson.json", |resource| {
        assert_eq!("http://127.0.0.1:3000/geojson.json", resource.url);
    });

    let (size, stop) = (Arc::clone(&test.size), test.run_loop.clone());
    let observer = MockObserver {
        status_changed_fn: Some(Box::new(move |status: OfflineRegionStatus| {
            if status.complete() {
                assert_eq!(2, status.completed_resource_count);
                assert_eq!(size.load(Ordering::SeqCst), status.completed_resource_size);
                assert!(status.required_resource_count_is_precise);
                stop.stop();
            }
        })),
        ..Default::default()
    };

    download.set_observer(Some(Rc::new(observer)));
    download.set_state(OfflineRegionDownloadState::Active);

    test.run_loop.run();
}

/// The completion check of Activate and `ExcludeIdeographs`: deactivate, then
/// compare the computed status with the reported one.
fn check_completed_and_computed(
    download: &TestDownload,
    size: u64,
    expected_resources: u64,
    status: OfflineRegionStatus,
) {
    assert_eq!(status.completed_tile_count, status.required_tile_count);
    assert_eq!(expected_resources, status.completed_resource_count);
    assert_eq!(size, status.completed_resource_size);

    download.set_state(OfflineRegionDownloadState::Inactive);
    let computed_status = download.get_status();
    assert_eq!(OfflineRegionDownloadState::Inactive, computed_status.download_state);
    assert_eq!(status.required_resource_count, computed_status.required_resource_count);
    assert_eq!(status.required_tile_count, computed_status.required_tile_count);
    assert_eq!(status.completed_resource_count, computed_status.completed_resource_count);
    assert_eq!(status.completed_resource_size, computed_status.completed_resource_size);
    assert_eq!(status.completed_tile_count, computed_status.completed_tile_count);
    assert_eq!(status.completed_tile_size, computed_status.completed_tile_size);
    assert!(status.required_resource_count_is_precise);
}

// gtest: OfflineDownload.Activate
#[test]
fn activate() {
    let test = OfflineTest::new();
    let region = test.create_region().expect("region");
    let download = test.download(region.id, STYLE_URL, true);

    respond_to_full_style(&test, true);

    let completed = Rc::new(Cell::new(None));
    let (last, stop) = (Rc::clone(&completed), test.run_loop.clone());
    let observer = MockObserver {
        status_changed_fn: Some(Box::new(move |status: OfflineRegionStatus| {
            if status.complete() && last.get().is_none() {
                last.set(Some(status));
                stop.stop();
            }
        })),
        ..Default::default()
    };

    download.set_observer(Some(Rc::new(observer)));
    download.set_state(OfflineRegionDownloadState::Active);

    test.run_loop.run();
    // 256 glyphs, 2 sprite images, 2 sprite jsons, 1 tile, 1 style, source,
    // image
    check_completed_and_computed(&download, test.size.load(Ordering::SeqCst), 264, completed.get().expect("complete"));
}

// gtest: OfflineDownload.ExcludeIdeographs
#[test]
fn exclude_ideographs() {
    let test = OfflineTest::new();
    let region = test.create_region().expect("region");
    let download = test.download(region.id, STYLE_URL, false);

    respond_to_full_style(&test, true);

    let completed = Rc::new(Cell::new(None));
    let (last, stop) = (Rc::clone(&completed), test.run_loop.clone());
    let observer = MockObserver {
        status_changed_fn: Some(Box::new(move |status: OfflineRegionStatus| {
            if status.complete() && last.get().is_none() {
                last.set(Some(status));
                stop.stop();
            }
        })),
        ..Default::default()
    };

    download.set_observer(Some(Rc::new(observer)));
    download.set_state(OfflineRegionDownloadState::Active);

    test.run_loop.run();
    // glyph ranges, 2 sprite images, 2 sprite jsons, 1 tile, 1 style,
    // source, image
    let expected = u64::from(NON_IDEOGRAPH_GLYPH_RANGES_PER_FONT_STACK) + 8;
    check_completed_and_computed(
        &download,
        test.size.load(Ordering::SeqCst),
        expected,
        completed.get().expect("complete"),
    );
}

struct FakeRequestEntry {
    resource: Resource,
    responder: Responder,
}

/// Records the requests; `respond` answers the first of a kind.
struct FakeOnlineFileSource {
    requests: Mutex<Vec<FakeRequestEntry>>,
}

impl FakeOnlineFileSource {
    fn new() -> Self {
        FakeOnlineFileSource { requests: Mutex::new(Vec::new()) }
    }

    /// The requests that weren't cancelled.
    fn requests(&self) -> MutexGuard<'_, Vec<FakeRequestEntry>> {
        let mut requests = self.requests.lock();
        requests.retain(|entry| !entry.responder.is_cancelled());
        requests
    }

    fn respond(&self, kind: Kind, response: &Response) -> bool {
        let responder =
            self.requests().iter().find(|entry| entry.resource.kind == kind).map(|entry| entry.responder.clone());
        let request_found = responder.is_some();
        if let Some(responder) = responder {
            responder.send(response.clone());
        }
        request_found
    }
}

impl FileSource for FakeOnlineFileSource {
    fn load(&self, resource: &Resource, responder: Responder) {
        self.requests().push(FakeRequestEntry { resource: resource.clone(), responder });
    }

    fn can_request(&self, _resource: &Resource) -> bool {
        true
    }
}

// gtest: OfflineDownload.DoesNotFloodTheFileSourceWithRequests
#[test]
fn does_not_flood_the_file_source_with_requests() {
    let test = OfflineTest::new();
    let file_source = Arc::new(FakeOnlineFileSource::new());
    let region = test.create_region().expect("region");
    let download =
        test.download_with_zoom(region.id, STYLE_URL, 0.0, 0.0, true, Arc::<FakeOnlineFileSource>::clone(&file_source));

    download.set_observer(Some(Rc::new(MockObserver::default())));
    download.set_state(OfflineRegionDownloadState::Active);
    test.run_loop.run_once();

    assert_eq!(1, file_source.requests().len());

    file_source.respond(Kind::Style, &test.response("style.json"));
    test.run_loop.run_once();

    assert_eq!(mln_util::constants::DEFAULT_MAXIMUM_CONCURRENT_REQUESTS as usize, file_source.requests().len());
}

// gtest: OfflineDownload.GetStatusNoResources
#[test]
fn get_status_no_resources() {
    let test = OfflineTest::new();
    let region = test.create_region().expect("region");
    let download = test.download(region.id, STYLE_URL, false);
    let status = download.get_status();

    assert_eq!(OfflineRegionDownloadState::Inactive, status.download_state);
    assert_eq!(0, status.completed_resource_count);
    assert_eq!(0, status.completed_resource_size);
    assert_eq!(1, status.required_resource_count);
    assert_eq!(0, status.completed_tile_count);
    assert_eq!(0, status.required_tile_count);
    assert!(!status.required_resource_count_is_precise);
    assert!(!status.complete());
}

// gtest: OfflineDownload.GetStatusStyleComplete
#[test]
fn get_status_style_complete() {
    let test = OfflineTest::new();
    let region = test.create_region().expect("region");
    let download = test.download(region.id, STYLE_URL, true);

    let response = test.response("style.json");
    test.db().put_region_resource(1, &Resource::style(STYLE_URL), &response);

    let status = download.get_status();

    assert_eq!(OfflineRegionDownloadState::Inactive, status.download_state);
    assert_eq!(1, status.completed_resource_count);
    assert_eq!(test.size.load(Ordering::SeqCst), status.completed_resource_size);
    assert_eq!(263, status.required_resource_count);
    assert_eq!(0, status.completed_tile_count);
    assert_eq!(0, status.required_tile_count);
    assert!(!status.required_resource_count_is_precise);
    assert!(!status.complete());
}

// gtest: OfflineDownload.GetStatusStyleAndSourceComplete
#[test]
fn get_status_style_and_source_complete() {
    let test = OfflineTest::new();
    let region = test.create_region().expect("region");
    let download = test.download(region.id, STYLE_URL, true);

    let response = test.response("style.json");
    test.db().put_region_resource(1, &Resource::style(STYLE_URL), &response);

    let response = test.response("streets.json");
    test.db().put_region_resource(1, &Resource::source("http://127.0.0.1:3000/streets.json"), &response);

    let status = download.get_status();

    assert_eq!(OfflineRegionDownloadState::Inactive, status.download_state);
    assert_eq!(2, status.completed_resource_count);
    assert_eq!(test.size.load(Ordering::SeqCst), status.completed_resource_size);
    assert_eq!(264, status.required_resource_count);
    assert_eq!(0, status.completed_tile_count);
    assert_eq!(1, status.required_tile_count);
    assert!(status.required_resource_count_is_precise);
    assert!(!status.complete());
}

// gtest: OfflineDownload.GetStatusInfiniteMaxZoom
#[test]
fn get_status_infinite_max_zoom() {
    let test = OfflineTest::new();
    let region = test.create_region().expect("region");
    let download = test.add_download(
        region.id,
        OfflineTilePyramidRegionDefinition::new(
            STYLE_URL,
            LatLngBounds::hull(LatLng::new(0.5, 0.5).unwrap(), LatLng::new(0.5001, 0.5001).unwrap()),
            14.0,
            f64::INFINITY,
            1.0,
            true,
        )
        .unwrap()
        .into(),
        Arc::<StubFileSource>::clone(&test.file_source),
    );

    let response = test.response("style.json");
    test.db().put_region_resource(1, &Resource::style(STYLE_URL), &response);
    let response = test.response("streets.json");
    test.db().put_region_resource(1, &Resource::source("http://127.0.0.1:3000/streets.json"), &response);

    let status = download.get_status();

    // One tile at z14 and one at the source's maxzoom, z15.
    assert_eq!(2, status.required_tile_count);
}

fn connection_error() -> Response {
    let mut response = Response::new();
    response.error = Some(Box::new(ResourceError::new(Reason::Connection, "connection error")));
    response
}

// gtest: OfflineDownload.RequestError
#[test]
fn request_error() {
    let test = OfflineTest::new();
    let region = test.create_region().expect("region");
    let download = test.download(region.id, STYLE_URL, false);

    test.respond(Kind::Style, |_| connection_error());

    let stop = test.run_loop.clone();
    let observer = MockObserver {
        response_error_fn: Some(Box::new(move |error: ResourceError| {
            assert_eq!(Reason::Connection, error.reason);
            assert_eq!("connection error", error.message);
            stop.stop();
        })),
        ..Default::default()
    };

    download.set_observer(Some(Rc::new(observer)));
    download.set_state(OfflineRegionDownloadState::Active);

    test.run_loop.run();
}

// gtest: OfflineDownload.RequestErrorsAreRetried
#[test]
fn request_errors_are_retried() {
    let test = OfflineTest::new();
    let region = test.create_region().expect("region");
    let download = test.download(region.id, STYLE_URL, true);

    let (file_source, size) = (Arc::downgrade(&test.file_source), Arc::clone(&test.size));
    test.respond(Kind::Style, move |_| {
        let size = Arc::clone(&size);
        if let Some(file_source) = file_source.upgrade() {
            file_source.set_response(Kind::Style, move |_| Some(fixture_response(&size, "empty.style.json")));
        }

        connection_error()
    });

    let stop = test.run_loop.clone();
    let observer = MockObserver {
        status_changed_fn: Some(Box::new(move |status: OfflineRegionStatus| {
            if status.complete() {
                assert_eq!(1, status.completed_resource_count);
                stop.stop();
            }
        })),
        ..Default::default()
    };

    download.set_observer(Some(Rc::new(observer)));
    download.set_state(OfflineRegionDownloadState::Active);

    test.run_loop.run();
}

fn put_tile(test: &OfflineTest, x: i32, y: i32, z: i8) {
    let response = test.response("0-0-0.vector.pbf");
    test.db().put(
        &Resource::tile(
            "http://127.0.0.1:3000/{z}-{x}-{y}.vector.pbf",
            1.0,
            x,
            y,
            z,
            Scheme::XYZ,
            TileFormat::Mvt,
            LoadingMethod::All,
        ),
        &response,
    );
}

// gtest: OfflineDownload.WithPreviouslyExistingTile
#[test]
fn with_previously_existing_tile() {
    let test = OfflineTest::new();
    let region = test.create_region().expect("region");
    let download = test.download(region.id, STYLE_URL, false);

    test.respond_with(Kind::Style, "inline_source.style.json", expect_style_url);

    put_tile(&test, 0, 0, 0);

    let (size, stop) = (Arc::clone(&test.size), test.run_loop.clone());
    let observer = MockObserver {
        status_changed_fn: Some(Box::new(move |status: OfflineRegionStatus| {
            if status.complete() {
                assert_eq!(2, status.completed_resource_count);
                assert_eq!(size.load(Ordering::SeqCst), status.completed_resource_size);
                assert!(status.required_resource_count_is_precise);
                stop.stop();
            }
        })),
        ..Default::default()
    };

    download.set_observer(Some(Rc::new(observer)));
    download.set_state(OfflineRegionDownloadState::Active);

    test.run_loop.run();
}

// gtest: OfflineDownload.ReactivatePreviouslyCompletedDownload
#[test]
fn reactivate_previously_completed_download() {
    let test = OfflineTest::new();
    let region = test.create_region().expect("region");
    let download = test.download(region.id, STYLE_URL, true);

    test.respond_with(Kind::Style, "inline_source.style.json", expect_style_url);

    put_tile(&test, 0, 0, 0);

    let stop = test.run_loop.clone();
    let observer = MockObserver {
        status_changed_fn: Some(Box::new(move |status: OfflineRegionStatus| {
            if status.complete() {
                stop.stop();
            }
        })),
        ..Default::default()
    };

    download.set_observer(Some(Rc::new(observer)));
    download.set_state(OfflineRegionDownloadState::Active);

    test.run_loop.run();

    let redownload = test.download(region.id, STYLE_URL, false);

    let statuses_after_reactivate: Rc<RefCell<Vec<OfflineRegionStatus>>> = Rc::new(RefCell::new(Vec::new()));

    let (statuses, stop) = (Rc::clone(&statuses_after_reactivate), test.run_loop.clone());
    let observer = MockObserver {
        status_changed_fn: Some(Box::new(move |status: OfflineRegionStatus| {
            statuses.borrow_mut().push(status);
            if status.complete() {
                stop.stop();
            }
        })),
        ..Default::default()
    };

    redownload.set_observer(Some(Rc::new(observer)));
    redownload.set_state(OfflineRegionDownloadState::Active);

    test.run_loop.run();

    let statuses = statuses_after_reactivate.borrow();
    assert_eq!(4, statuses.len());

    assert_eq!(OfflineRegionDownloadState::Active, statuses[0].download_state);
    assert!(!statuses[0].required_resource_count_is_precise);
    assert_eq!(1, statuses[0].required_resource_count);
    assert_eq!(0, statuses[0].completed_resource_count);

    assert_eq!(OfflineRegionDownloadState::Active, statuses[1].download_state);
    assert!(statuses[1].required_resource_count_is_precise);
    assert_eq!(2, statuses[1].required_resource_count);
    assert_eq!(1, statuses[1].completed_resource_count);

    assert_eq!(OfflineRegionDownloadState::Active, statuses[2].download_state);
    assert!(statuses[2].required_resource_count_is_precise);
    assert_eq!(2, statuses[2].required_resource_count);
    assert_eq!(2, statuses[2].completed_resource_count);
}

// gtest: OfflineDownload.Deactivate
#[test]
fn deactivate() {
    let test = OfflineTest::new();
    let region = test.create_region().expect("region");
    let download = test.download(region.id, STYLE_URL, false);

    test.respond_with(Kind::Style, "mapbox_source.style.json", expect_style_url);

    let (stop, weak) = (test.run_loop.clone(), download.downgrade());
    let observer = MockObserver {
        status_changed_fn: Some(Box::new(move |status: OfflineRegionStatus| {
            if status.download_state == OfflineRegionDownloadState::Inactive {
                stop.stop();
            } else {
                weak.set_state(OfflineRegionDownloadState::Inactive);
            }
        })),
        ..Default::default()
    };

    download.set_observer(Some(Rc::new(observer)));
    download.set_state(OfflineRegionDownloadState::Active);

    test.run_loop.run();
}

// gtest: OfflineDownload.AllOfflineRequestsHaveLowPriorityAndOfflineUsage
#[test]
fn all_offline_requests_have_low_priority_and_offline_usage() {
    let test = OfflineTest::new();
    let region = test.create_region().expect("region");
    let download = test.download(region.id, STYLE_URL, true);

    let low_offline = |resource: &Resource| {
        assert_eq!(resource.priority, Priority::Low);
        assert_eq!(resource.usage, Usage::Offline);
    };
    test.respond_with(Kind::Style, "style.json", low_offline);
    test.respond_with(Kind::SpriteImage, "sprite.png", low_offline);
    test.respond_with(Kind::Image, "radar.gif", low_offline);
    test.respond_with(Kind::SpriteJSON, "sprite.json", low_offline);
    test.respond_with(Kind::Glyphs, "glyph.pbf", low_offline);
    test.respond_with(Kind::Source, "streets.json", low_offline);
    test.respond_with(Kind::Tile, "0-0-0.vector.pbf", low_offline);

    let stop = test.run_loop.clone();
    let observer = MockObserver {
        status_changed_fn: Some(Box::new(move |status: OfflineRegionStatus| {
            if status.complete() {
                stop.stop();
            }
        })),
        ..Default::default()
    };

    download.set_observer(Some(Rc::new(observer)));
    download.set_state(OfflineRegionDownloadState::Active);

    test.run_loop.run();
}

// Test verifies that resource batch buffer is cleared at loading interrupt.
// gtest: OfflineDownload.InterruptAndResume
#[test]
fn interrupt_and_resume() {
    let test = OfflineTest::new();
    let region = test.create_region().expect("region");
    let download = test.download(region.id, STYLE_URL, true);

    respond_to_full_style(&test, true);

    let interrupted = Cell::new(false);
    let (stop, weak) = (test.run_loop.clone(), download.downgrade());
    // `test.loop.schedule([&] { ... })`: a task on the test's loop.
    let observer = MockObserver {
        status_changed_fn: Some(Box::new(move |status: OfflineRegionStatus| {
            if !interrupted.get() && status.completed_resource_count > 0 {
                interrupted.set(true);
                assert!(!status.complete());
                let (run_loop, weak) = (stop.clone(), weak.clone());
                stop.schedule(move || {
                    weak.set_state(OfflineRegionDownloadState::Inactive);
                    run_loop.stop();
                });
            }
        })),
        ..Default::default()
    };

    download.set_observer(Some(Rc::new(observer)));
    download.set_state(OfflineRegionDownloadState::Active);
    test.run_loop.run();

    let stop = test.run_loop.clone();
    let new_observer = MockObserver {
        status_changed_fn: Some(Box::new(move |status: OfflineRegionStatus| {
            if status.complete() {
                assert_eq!(status.completed_tile_count, status.required_tile_count);
                // 256 glyphs, 2 sprite images, 2 sprite jsons, 1 tile, 1
                // style, source, image
                assert_eq!(264, status.completed_resource_count);
                stop.stop();
            }
        })),
        ..Default::default()
    };

    download.set_observer(Some(Rc::new(new_observer)));
    download.set_state(OfflineRegionDownloadState::Active);
    test.run_loop.run();
}

// gtest: OfflineDownload.NoFreezingOnCachedTilesAndNewStyle
#[test]
fn no_freezing_on_cached_tiles_and_new_style() {
    let test = OfflineTest::new();
    let region = test.create_region().expect("region");
    let download =
        test.download_with_zoom(region.id, STYLE_URL, 1.0, 1.0, true, Arc::<StubFileSource>::clone(&test.file_source));

    download.set_maximum_concurrent_requests(2);
    test.respond_with(Kind::Style, "inline_source.style.json", |_| {});
    // Number of resources must exceed the maximum concurrent requests
    put_tile(&test, 0, 0, 1);
    put_tile(&test, 0, 1, 1);
    put_tile(&test, 1, 0, 1);
    put_tile(&test, 1, 1, 1);

    let stop = test.run_loop.clone();
    let observer = MockObserver {
        status_changed_fn: Some(Box::new(move |status: OfflineRegionStatus| {
            if status.complete() {
                stop.stop();
            }
        })),
        ..Default::default()
    };

    download.set_observer(Some(Rc::new(observer)));
    download.set_state(OfflineRegionDownloadState::Active);

    test.run_loop.run();
    // Passes if does not freeze.
}

// gtest: OfflineDownload.NoFreezingOnNotFoundError
#[test]
fn no_freezing_on_not_found_error() {
    let test = OfflineTest::new();
    let region = test.create_region().expect("region");
    let download = test.download(region.id, STYLE_URL, true);

    respond_to_full_style(&test, false);
    test.respond(Kind::SpriteJSON, |_| {
        let mut response = Response::new();
        response.error = Some(Box::new(ResourceError::new(Reason::NotFound, "")));
        response
    });

    let error_reported = Rc::new(Cell::new(false));
    let (reported, reported2, stop) = (Rc::clone(&error_reported), Rc::clone(&error_reported), test.run_loop.clone());
    let observer = MockObserver {
        response_error_fn: Some(Box::new(move |error: ResourceError| {
            assert_eq!(Reason::NotFound, error.reason);
            reported.set(true);
        })),
        status_changed_fn: Some(Box::new(move |status: OfflineRegionStatus| {
            if status.complete() {
                assert!(reported2.get());
                stop.stop();
            }
        })),
    };

    download.set_observer(Some(Rc::new(observer)));
    download.set_state(OfflineRegionDownloadState::Active);

    test.run_loop.run();
    // Passes if does not freeze.
}

/// The size the database stores `data` in.
fn stored_size(data: &[u8]) -> u64 {
    (data.len() as u64).min(compress(data).unwrap().len() as u64)
}

fn read_fixture(path: &str) -> Vec<u8> {
    std::fs::read(cpp_path(&format!("test/fixtures/offline_download/{path}"))).unwrap()
}

fn with_data(data: impl Into<Vec<u8>>) -> Response {
    let mut response = Response::new();
    response.data = Some(Arc::new(data.into()));
    response
}

fn no_content() -> Response {
    let mut response = Response::new();
    response.no_content = true;
    response
}

fn not_found() -> Response {
    let mut response = Response::new();
    response.error = Some(Box::new(ResourceError::new(Reason::NotFound, "")));
    response
}

fn hours_from_now(hours: i64) -> Timestamp {
    Timestamp::from_secs(now().secs_since_epoch() + hours * 3600)
}

fn xyz_tile(url_template: &str, x: i32, y: i32, z: i8) -> Resource {
    Resource::tile(url_template, 1.0, x, y, z, Scheme::XYZ, TileFormat::Mvt, LoadingMethod::All)
}

fn world_at_zoom(max_zoom: f64) -> OfflineRegionDefinition {
    OfflineTilePyramidRegionDefinition::new(STYLE_URL, LatLngBounds::world(), 0.0, max_zoom, 1.0, false).unwrap().into()
}

/// Runs `download` until it goes inactive, and returns its last active
/// status. Collects the errors the download reports in `errors`, and fails
/// on any if it's `None`.
fn run_until_inactive(
    test: &OfflineTest,
    download: &TestDownload,
    errors: Option<Rc<RefCell<Vec<ResourceError>>>>,
) -> OfflineRegionStatus {
    let completed = Rc::new(Cell::new(OfflineRegionStatus::default()));
    let (last, stop) = (Rc::clone(&completed), test.run_loop.clone());
    let observer = MockObserver {
        response_error_fn: Some(Box::new(move |error: ResourceError| match &errors {
            Some(errors) => errors.borrow_mut().push(error),
            None => panic!("unexpected error: {}", error.message),
        })),
        status_changed_fn: Some(Box::new(move |status: OfflineRegionStatus| {
            if status.download_state == OfflineRegionDownloadState::Active {
                last.set(status);
            } else {
                stop.stop();
            }
        })),
    };
    download.set_observer(Some(Rc::new(observer)));
    download.set_state(OfflineRegionDownloadState::Active);
    test.run_loop.run();
    completed.get()
}

/// Runs a download of `region_id` until it goes inactive, and returns its
/// last active status. The completed counts and sizes that the database
/// reports afterwards must match it.
fn download_until_inactive(
    test: &OfflineTest,
    region_id: i64,
    definition: &OfflineRegionDefinition,
    errors: Option<Rc<RefCell<Vec<ResourceError>>>>,
) -> OfflineRegionStatus {
    let download = test.add_download(region_id, definition.clone(), Arc::<StubFileSource>::clone(&test.file_source));
    let completed = run_until_inactive(test, &download, errors);
    assert!(completed.complete());
    let stored = download.get_status();
    assert_eq!(completed.completed_resource_count, stored.completed_resource_count);
    assert_eq!(completed.completed_resource_size, stored.completed_resource_size);
    assert_eq!(completed.completed_tile_count, stored.completed_tile_count);
    assert_eq!(completed.completed_tile_size, stored.completed_tile_size);
    completed
}

fn invalidate_region(test: &OfflineTest, region_id: i64) {
    assert!(test.db().invalidate_region(region_id).is_ok());
}

/// Downloads a region of a style and one tile, invalidates it, and downloads
/// it again. The server answers 304 Not Modified for the resources marked,
/// and sends new data for the others.
fn test_refresh_invalidated_region(style_not_modified: bool, tile_not_modified: bool) {
    let test = OfflineTest::new();
    let region = test.create_region().expect("region");
    let definition = world_at_zoom(0.0);
    let tile_url = "http://127.0.0.1:3000/{z}-{x}-{y}.vector.pbf";
    let modified = hours_from_now(-1);
    let expires = hours_from_now(1);

    let old_style = read_fixture("inline_source.style.json");
    let new_style = [old_style.as_slice(), b"\n"].concat();
    let old_tile = read_fixture("0-0-0.vector.pbf");
    let new_tile = b"new tile".to_vec();

    let response = move |data: &[u8], etag: &str| {
        let mut response = with_data(data);
        response.etag = Some(etag.to_owned());
        response.modified = Some(modified);
        response
    };

    let (style, tile) = (old_style.clone(), old_tile.clone());
    test.respond(Kind::Style, move |_| response(&style, "style-1"));
    test.respond(Kind::Tile, move |_| response(&tile, "tile-1"));
    download_until_inactive(&test, region.id, &definition, None);

    invalidate_region(&test, region.id);

    let revalidate = move |resource: &Resource, prior_etag: &str, not_modified: bool, data: &[u8], etag: &str| {
        assert_eq!(Some(prior_etag), resource.prior_etag.as_deref());
        assert_eq!(Some(modified), resource.prior_modified);
        let mut result = if not_modified {
            let mut result = Response::new();
            result.not_modified = true;
            result
        } else {
            response(data, etag)
        };
        result.caching.expires = Some(expires);
        result
    };

    let style_requests = Arc::new(AtomicI32::new(0));
    let tile_requests = Arc::new(AtomicI32::new(0));
    let (requests, style) = (Arc::clone(&style_requests), new_style.clone());
    test.respond(Kind::Style, move |resource| {
        requests.fetch_add(1, Ordering::SeqCst);
        revalidate(resource, "style-1", style_not_modified, &style, "style-2")
    });
    let (requests, tile) = (Arc::clone(&tile_requests), new_tile.clone());
    test.respond(Kind::Tile, move |resource| {
        requests.fetch_add(1, Ordering::SeqCst);
        revalidate(resource, "tile-1", tile_not_modified, &tile, "tile-2")
    });
    let status = download_until_inactive(&test, region.id, &definition, None);

    assert_eq!(1, style_requests.load(Ordering::SeqCst));
    assert_eq!(1, tile_requests.load(Ordering::SeqCst));

    let style = if style_not_modified { &old_style } else { &new_style };
    let tile = if tile_not_modified { &old_tile } else { &new_tile };
    assert_eq!(2, status.completed_resource_count);
    assert_eq!(stored_size(style) + stored_size(tile), status.completed_resource_size);
    assert_eq!(1, status.completed_tile_count);
    assert_eq!(stored_size(tile), status.completed_tile_size);

    let check_stored = |resource: &Resource, data: &[u8], etag: &str| {
        let stored = test.db().get(resource).expect("stored");
        assert!(!stored.caching.must_revalidate);
        assert_eq!(Some(expires), stored.caching.expires);
        assert!(stored.caching.is_fresh(now()));
        assert_eq!(data, stored.data.expect("data").as_slice());
        assert_eq!(Some(etag), stored.etag.as_deref());
    };
    check_stored(&Resource::style(STYLE_URL), style, if style_not_modified { "style-1" } else { "style-2" });
    check_stored(&xyz_tile(tile_url, 0, 0, 0), tile, if tile_not_modified { "tile-1" } else { "tile-2" });
}

// gtest: OfflineDownload.RefreshInvalidatedRegionStyleNotModified
#[test]
fn refresh_invalidated_region_style_not_modified() {
    test_refresh_invalidated_region(true, false);
}

// gtest: OfflineDownload.RefreshInvalidatedRegionTileNotModified
#[test]
fn refresh_invalidated_region_tile_not_modified() {
    test_refresh_invalidated_region(false, true);
}

/// A download keeps the fresh resources that the region holds, and
/// revalidates the stale ones.
#[test]
fn revalidates_only_stale_stored_resources() {
    let test = OfflineTest::new();
    let region = test.create_region().expect("region");
    let definition = world_at_zoom(1.0);
    let tile_url = "http://127.0.0.1:3000/{z}-{x}-{y}.vector.pbf";
    let past = hours_from_now(-1);
    let future = hours_from_now(1);

    // The style and the z0 tile have expired; the z1 tiles are fresh.
    let mut style = test.response("inline_source.style.json");
    style.etag = Some("style-1".to_owned());
    style.caching.must_revalidate = true;
    style.caching.expires = Some(past);
    test.db().put(&Resource::style(STYLE_URL), &style);
    let mut expired_tile = test.response("0-0-0.vector.pbf");
    expired_tile.etag = Some("tile-1".to_owned());
    expired_tile.caching.expires = Some(past);
    test.db().put(&xyz_tile(tile_url, 0, 0, 0), &expired_tile);
    for x in 0..2 {
        for y in 0..2 {
            let mut fresh_tile = test.response("0-0-0.vector.pbf");
            fresh_tile.caching.must_revalidate = true;
            fresh_tile.caching.expires = Some(future);
            test.db().put(&xyz_tile(tile_url, x, y, 1), &fresh_tile);
        }
    }

    let requests = Arc::new(Mutex::new(Vec::new()));
    for kind in [Kind::Style, Kind::Tile] {
        let requests = Arc::clone(&requests);
        test.respond(kind, move |resource| {
            requests.lock().push((resource.url.clone(), resource.prior_etag.clone()));
            let mut response = Response::new();
            response.not_modified = true;
            response.caching.expires = Some(future);
            response
        });
    }

    let status = download_until_inactive(&test, region.id, &definition, None);

    let etag = |etag: &str| Some(etag.to_owned());
    let expected = vec![
        (STYLE_URL.to_owned(), etag("style-1")),
        ("http://127.0.0.1:3000/0-0-0.vector.pbf".to_owned(), etag("tile-1")),
    ];
    assert_eq!(expected, *requests.lock());
    assert_eq!(6, status.completed_resource_count);
    assert_eq!(test.size.load(Ordering::SeqCst), status.completed_resource_size);
    assert_eq!(5, status.completed_tile_count);
    let refreshed = test.db().get(&xyz_tile(tile_url, 0, 0, 0)).expect("tile");
    assert_eq!((Some(future), Some("tile-1")), (refreshed.caching.expires, refreshed.etag.as_deref()));
}

fn inline_source_style(tile_url: &str) -> String {
    format!(r#"{{"version":8,"sources":{{"inline":{{"type":"vector","tiles":["{tile_url}"]}}}},"layers":[]}}"#)
}

const TILE_JSON_SOURCE_STYLE: &str =
    r#"{"version":8,"sources":{"remote":{"type":"vector","url":"http://127.0.0.1:3000/tiles.json"}},"layers":[]}"#;

fn tile_json(tile_url: &str) -> String {
    format!(r#"{{"tilejson":"2.1.0","tiles":["{tile_url}"]}}"#)
}

/// The status of the region `region_id` as the database reports it while no
/// download runs.
fn stored_status(test: &OfflineTest, region_id: i64) -> OfflineRegionStatus {
    test.add_download(region_id, world_at_zoom(0.0), Arc::<StubFileSource>::clone(&test.file_source)).get_status()
}

fn collect_errors() -> Rc<RefCell<Vec<ResourceError>>> {
    Rc::new(RefCell::new(Vec::new()))
}

// gtest: OfflineDownload.RefreshKeepsTilesThatOnlyAnEarlierStyleListed
#[test]
fn refresh_keeps_tiles_that_only_an_earlier_style_listed() {
    let test = OfflineTest::new();
    let region = test.create_region().expect("region");
    let v1 = "http://127.0.0.1:3000/v1/{z}-{x}-{y}.pbf";
    let v2 = "http://127.0.0.1:3000/v2/{z}-{x}-{y}.pbf";
    let tile_url = Arc::new(Mutex::new(v1));
    let current_url = Arc::clone(&tile_url);
    test.respond(Kind::Style, move |_| with_data(inline_source_style(&current_url.lock())));
    test.respond(Kind::Tile, |resource| {
        with_data(format!("tile {}", resource.tile_data.as_ref().unwrap().url_template))
    });
    download_until_inactive(&test, region.id, &world_at_zoom(0.0), None);

    // The refreshed style moves the tile to another URL.
    *tile_url.lock() = v2;
    invalidate_region(&test, region.id);
    let download = test.add_download(region.id, world_at_zoom(0.0), Arc::<StubFileSource>::clone(&test.file_source));
    let active = run_until_inactive(&test, &download, None);

    // While it runs, the refresh counts what the current style lists.
    assert!(active.complete());
    assert_eq!(2, active.completed_resource_count);
    assert_eq!(1, active.completed_tile_count);
    assert_eq!(stored_size(format!("tile {v2}").as_bytes()), active.completed_tile_size);

    // Once it stops, the status counts the earlier tile as well.
    let stored = download.get_status();
    assert!(stored.complete());
    assert_eq!(2, stored.required_resource_count);
    assert_eq!(1, stored.required_tile_count);
    assert_eq!(3, stored.completed_resource_count);
    assert_eq!(2, stored.completed_tile_count);
    assert_eq!(
        stored_size(format!("tile {v1}").as_bytes()) + stored_size(format!("tile {v2}").as_bytes()),
        stored.completed_tile_size
    );

    let earlier = test.db().get(&xyz_tile(v1, 0, 0, 0)).expect("earlier tile");
    assert!(!earlier.caching.is_fresh(now()) && earlier.caching.is_usable(now()));
    let current = test.db().get(&xyz_tile(v2, 0, 0, 0)).expect("current tile");
    assert_eq!(format!("tile {v2}").as_bytes(), current.data.expect("data").as_slice());
}

// gtest: OfflineDownload.InlineSourceWithoutTileURLsHasNoTiles
#[test]
fn inline_source_without_tile_urls_has_no_tiles() {
    let test = OfflineTest::new();
    let region = test.create_region().expect("region");
    test.respond(Kind::Style, |_| {
        with_data(r#"{"version":8,"sources":{"inline":{"type":"vector","tiles":[]}},"layers":[]}"#)
    });
    let status = download_until_inactive(&test, region.id, &world_at_zoom(0.0), None);
    assert_eq!(1, status.completed_resource_count);
    assert_eq!(0, status.required_tile_count);

    let stored = stored_status(&test, region.id);
    assert_eq!(1, stored.required_resource_count);
    assert_eq!(0, stored.required_tile_count);
    assert!(stored.required_resource_count_is_precise);
    assert!(stored.complete());
}

// gtest: OfflineDownload.ReportsATileJSONWithoutTileURLs
#[test]
fn reports_a_tile_json_without_tile_urls() {
    let test = OfflineTest::new();
    let region = test.create_region().expect("region");
    test.respond(Kind::Style, |_| with_data(TILE_JSON_SOURCE_STYLE));
    test.respond(Kind::Source, |_| with_data(r#"{"tilejson":"2.1.0","tiles":[]}"#));
    let errors = collect_errors();
    let status = download_until_inactive(&test, region.id, &world_at_zoom(0.0), Some(Rc::clone(&errors)));
    assert_eq!(1, errors.borrow().len());
    assert_eq!(Reason::Other, errors.borrow()[0].reason);
    assert_eq!(2, status.completed_resource_count);
    assert_eq!(0, status.required_tile_count);
    assert!(!status.required_resource_count_is_precise);

    let stored = stored_status(&test, region.id);
    assert_eq!(2, stored.required_resource_count);
    assert_eq!(0, stored.required_tile_count);
    assert!(!stored.required_resource_count_is_precise);
    assert!(stored.complete());
}

// gtest: OfflineDownload.ReportsAStyleWithNoContent
#[test]
fn reports_a_style_with_no_content() {
    let test = OfflineTest::new();
    let region = test.create_region().expect("region");
    test.respond(Kind::Style, |_| no_content());
    let errors = collect_errors();
    let status = download_until_inactive(&test, region.id, &world_at_zoom(0.0), Some(Rc::clone(&errors)));
    assert_eq!(1, errors.borrow().len());
    assert_eq!(Reason::Other, errors.borrow()[0].reason);
    assert_eq!(1, status.completed_resource_count);
    assert!(!status.required_resource_count_is_precise);

    let stored = stored_status(&test, region.id);
    assert_eq!(1, stored.required_resource_count);
    assert!(!stored.required_resource_count_is_precise);
}

/// Downloads a region of a style, a TileJSON, and one tile, then invalidates
/// it. The style and the TileJSON have the bodies the case sets.
struct RefreshedRegion {
    test: OfflineTest,
    region: OfflineRegion,
    style: Arc<Mutex<Response>>,
    source: Arc<Mutex<Response>>,
    tile_requests: Arc<AtomicI32>,
    downloaded: OfflineRegionStatus,
}

const REFRESHED_TILE_URL: &str = "http://127.0.0.1:3000/{z}-{x}-{y}.pbf";

impl RefreshedRegion {
    fn new() -> Self {
        let test = OfflineTest::new();
        let region = test.create_region().expect("region");
        let style = Arc::new(Mutex::new(with_data(TILE_JSON_SOURCE_STYLE)));
        let source = Arc::new(Mutex::new(with_data(tile_json(REFRESHED_TILE_URL))));
        let tile_requests = Arc::new(AtomicI32::new(0));
        let body = Arc::clone(&style);
        test.respond(Kind::Style, move |_| body.lock().clone());
        let body = Arc::clone(&source);
        test.respond(Kind::Source, move |_| body.lock().clone());
        let requests = Arc::clone(&tile_requests);
        test.respond(Kind::Tile, move |_| {
            requests.fetch_add(1, Ordering::SeqCst);
            with_data("tile")
        });
        let downloaded = download_until_inactive(&test, region.id, &world_at_zoom(0.0), None);
        invalidate_region(&test, region.id);
        RefreshedRegion { test, region, style, source, tile_requests, downloaded }
    }

    /// Refreshes the region, expecting one error of `reason`, and checks that
    /// the region keeps every resource, and that the stale tile was
    /// refreshed.
    fn refresh_keeping_resources(&self, reason: Reason) {
        let errors = collect_errors();
        let status = download_until_inactive(&self.test, self.region.id, &world_at_zoom(0.0), Some(Rc::clone(&errors)));
        assert_eq!(1, errors.borrow().len());
        assert_eq!(reason, errors.borrow()[0].reason);
        assert!(status.required_resource_count_is_precise);
        assert_eq!(self.downloaded.completed_resource_count, status.completed_resource_count);
        assert_eq!(self.downloaded.completed_resource_size, status.completed_resource_size);
        assert_eq!(self.downloaded.completed_tile_count, status.completed_tile_count);
        assert_eq!(2, self.tile_requests.load(Ordering::SeqCst));
        let tile = self.test.db().get(&xyz_tile(REFRESHED_TILE_URL, 0, 0, 0)).expect("tile");
        assert!(!tile.caching.must_revalidate);
    }
}

// gtest: OfflineDownload.RefreshContinuesFromAStoredStyleThatIsNotFound
#[test]
fn refresh_continues_from_a_stored_style_that_is_not_found() {
    let refreshed = RefreshedRegion::new();
    *refreshed.style.lock() = not_found();
    refreshed.refresh_keeping_resources(Reason::NotFound);
}

// gtest: OfflineDownload.RefreshContinuesFromAStoredTileJSONThatIsNotFound
#[test]
fn refresh_continues_from_a_stored_tile_json_that_is_not_found() {
    let refreshed = RefreshedRegion::new();
    *refreshed.source.lock() = not_found();
    refreshed.refresh_keeping_resources(Reason::NotFound);
}

// gtest: OfflineDownload.RefreshContinuesFromAStoredStyleWhenTheNewOneIsInvalid
#[test]
fn refresh_continues_from_a_stored_style_when_the_new_one_is_invalid() {
    let refreshed = RefreshedRegion::new();
    *refreshed.style.lock() = with_data("not JSON");
    refreshed.refresh_keeping_resources(Reason::Other);
    let style = refreshed.test.db().get(&Resource::style(STYLE_URL)).expect("style");
    assert_eq!(TILE_JSON_SOURCE_STYLE.as_bytes(), style.data.expect("data").as_slice());
}

// gtest: OfflineDownload.RefreshContinuesFromAStoredTileJSONWhenTheNewOneIsInvalid
#[test]
fn refresh_continues_from_a_stored_tile_json_when_the_new_one_is_invalid() {
    let refreshed = RefreshedRegion::new();
    *refreshed.source.lock() = with_data("not JSON");
    refreshed.refresh_keeping_resources(Reason::Other);
}

// gtest: OfflineDownload.RefreshContinuesFromAStoredStyleWhenTheNewOneHasNoContent
#[test]
fn refresh_continues_from_a_stored_style_when_the_new_one_has_no_content() {
    let refreshed = RefreshedRegion::new();
    *refreshed.style.lock() = no_content();
    refreshed.refresh_keeping_resources(Reason::Other);
}

// gtest: OfflineDownload.RefreshContinuesFromAStoredTileJSONWhenTheNewOneHasNoContent
#[test]
fn refresh_continues_from_a_stored_tile_json_when_the_new_one_has_no_content() {
    let refreshed = RefreshedRegion::new();
    *refreshed.source.lock() = no_content();
    refreshed.refresh_keeping_resources(Reason::Other);
}

// gtest: OfflineDownload.RefreshContinuesFromAStoredTileJSONWhenTheNewOneHasNoTileURLs
#[test]
fn refresh_continues_from_a_stored_tile_json_when_the_new_one_has_no_tile_urls() {
    let refreshed = RefreshedRegion::new();
    *refreshed.source.lock() = with_data(r#"{"tilejson":"2.1.0","tiles":[]}"#);
    refreshed.refresh_keeping_resources(Reason::Other);
}

// gtest: OfflineDownload.PausingFromAnErrorStopsTheRefresh
#[test]
fn pausing_from_an_error_stops_the_refresh() {
    let refreshed = RefreshedRegion::new();
    let test = &refreshed.test;
    // One request at a time, so that work the pause failed to stop would
    // still be queued when the download resumes.
    *refreshed.style.lock() = with_data("not JSON");

    // The observer pauses the download when it reports the invalid style.
    let download =
        test.add_download(refreshed.region.id, world_at_zoom(0.0), Arc::<StubFileSource>::clone(&test.file_source));
    download.set_maximum_concurrent_requests(1);
    let (weak, stop) = (download.downgrade(), test.run_loop.clone());
    let observer = MockObserver {
        response_error_fn: Some(Box::new(move |_| {
            weak.set_state(OfflineRegionDownloadState::Inactive);
            stop.stop();
        })),
        ..Default::default()
    };
    download.set_observer(Some(Rc::new(observer)));
    download.set_state(OfflineRegionDownloadState::Active);
    test.run_loop.run();
    assert_eq!(1, refreshed.tile_requests.load(Ordering::SeqCst));

    // Resumed with a valid style, the refresh counts each resource once.
    *refreshed.style.lock() = with_data(TILE_JSON_SOURCE_STYLE);
    let completed = run_until_inactive(test, &download, None);
    let stored = download.get_status();
    assert_eq!(3, completed.completed_resource_count);
    assert_eq!(1, completed.completed_tile_count);
    assert_eq!(completed.completed_resource_count, stored.completed_resource_count);
    assert_eq!(completed.completed_resource_size, stored.completed_resource_size);
    assert_eq!(2, refreshed.tile_requests.load(Ordering::SeqCst));
}

// gtest: OfflineDownload.MissingTileJSONLeavesAPreciseResourceCount
#[test]
fn missing_tile_json_leaves_a_precise_resource_count() {
    let test = OfflineTest::new();
    let region = test.create_region().expect("region");
    test.respond(Kind::Style, |_| with_data(TILE_JSON_SOURCE_STYLE));
    test.respond(Kind::Source, |_| not_found());
    let errors = collect_errors();
    let status = download_until_inactive(&test, region.id, &world_at_zoom(0.0), Some(Rc::clone(&errors)));
    assert_eq!(1, errors.borrow().len());
    assert_eq!(1, status.required_resource_count);
    assert!(status.required_resource_count_is_precise);
}
