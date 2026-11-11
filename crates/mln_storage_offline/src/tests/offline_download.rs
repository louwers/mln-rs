// Ports test/storage/offline_download.test.cpp (but ResourceOfflineUsageUnset,
// which renders a map: tests/offline_download_map.rs). Fixtures are read in
// place from test/fixtures/offline_download.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use feature::Value;
use mln_actor::{AsyncTask, LocalObject, RunLoop};
use mln_storage::file_source::MAX_CONCURRENT_REQUESTS_KEY;
use mln_storage::offline::{
    OfflineRegion, OfflineRegionDownloadState, OfflineRegionMetadata, OfflineRegionObserver, OfflineRegionStatus,
    OfflineTilePyramidRegionDefinition,
};
use mln_storage::offline_database::{DatabaseError, OfflineDatabase};
use mln_storage::online_file_source::OnlineFileSource;
use mln_storage::resource::{Priority, Usage};
use mln_storage::response::{Error, Reason};
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_storage::{
    AsyncRequest, Callback, ClientOptions, FileSource, Kind, LoadingMethod, Resource, ResourceOptions, Response,
};
use mln_text::glyph::NON_IDEOGRAPH_GLYPH_RANGES_PER_FONT_STACK;
use mln_util::compression::compress;
use mln_util::geo::{LatLng, LatLngBounds};
use mln_util::tile_server_options::TileServerOptions;
use mln_util::tileset::Scheme;

use super::cpp_path;
use crate::offline_download::{OfflineDownload, WeakOfflineDownload};

/// `MockObserver`.
#[derive(Default)]
struct MockObserver {
    status_changed_fn: Option<Box<dyn Fn(OfflineRegionStatus)>>,
    response_error_fn: Option<Box<dyn Fn(Error)>>,
    mapbox_tile_count_limit_exceeded_fn: Option<Box<dyn Fn(u64)>>,
}

impl OfflineRegionObserver for MockObserver {
    fn status_changed(&self, status: OfflineRegionStatus) {
        if let Some(f) = &self.status_changed_fn {
            f(status);
        }
    }

    fn response_error(&self, error: Error) {
        if let Some(f) = &self.response_error_fn {
            f(error);
        }
    }

    fn mapbox_tile_count_limit_exceeded(&self, limit: u64) {
        if let Some(f) = &self.mapbox_tile_count_limit_exceeded_fn {
            f(limit);
        }
    }
}

/// `test.response(path)`: the fixture, adding its stored size to `size`.
fn fixture_response(size: &Cell<u64>, path: &str) -> Response {
    let mut result = Response::new();
    let data = std::fs::read(cpp_path(&format!("test/fixtures/offline_download/{path}")))
        .unwrap_or_else(|e| panic!("Cannot read file {path}: {e}"));
    let uncompressed = data.len() as u64;
    let compressed = compress(&data).unwrap().len() as u64;
    size.set(size.get() + uncompressed.min(compressed));
    result.data = Some(Arc::new(data));
    result
}

/// `OfflineTest`.
struct OfflineTest {
    run_loop: RunLoop,
    file_source: Arc<StubFileSource>,
    db: Rc<RefCell<OfflineDatabase>>,
    size: Rc<Cell<u64>>,
}

impl OfflineTest {
    fn new() -> Self {
        Self::with_path(":memory:")
    }

    fn with_path(path: &str) -> Self {
        let run_loop = RunLoop::new();
        OfflineTest {
            run_loop,
            file_source: Arc::new(StubFileSource::new(ResponseType::Asynchronous)),
            db: Rc::new(RefCell::new(OfflineDatabase::new(path, &TileServerOptions::map_tiler_configuration()))),
            size: Rc::new(Cell::new(0)),
        }
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
        self.db.borrow_mut().create_region(&definition.into(), &metadata)
    }

    fn response(&self, path: &str) -> Response {
        fixture_response(&self.size, path)
    }

    /// `fileSource.<kind>Response = [&](const Resource&) { ... }`.
    fn respond(&self, kind: Kind, f: impl Fn(&Resource) -> Response + 'static) {
        self.file_source.set_response(kind, move |resource| Some(f(resource)));
    }

    /// A responder that returns the fixture at `path`.
    fn respond_with(&self, kind: Kind, path: &'static str, check: impl Fn(&Resource) + 'static) {
        let size = self.size.clone();
        self.respond(kind, move |resource| {
            check(resource);
            fixture_response(&size, path)
        });
    }

    fn download(&self, region_id: i64, style_url: &str, include_ideographs: bool) -> OfflineDownload {
        self.download_with_zoom(region_id, style_url, 0.0, 0.0, include_ideographs, self.file_source.clone())
    }

    fn download_with_zoom(
        &self,
        region_id: i64,
        style_url: &str,
        min_zoom: f64,
        max_zoom: f64,
        include_ideographs: bool,
        file_source: Arc<dyn FileSource>,
    ) -> OfflineDownload {
        OfflineDownload::new(
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
            self.db.clone(),
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
/// resources (Activate, ExcludeIdeographs, InterruptAndResume,
/// NoFreezingOnNotFoundError but its sprite JSON).
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
    let download = test.download(region.get_id(), STYLE_URL, false);

    test.respond_with(Kind::Style, "empty.style.json", expect_style_url);

    let expects_inactive_status = Cell::new(false);
    let (size, stop) = (test.size.clone(), test.run_loop.clone());
    let observer = MockObserver {
        status_changed_fn: Some(Box::new(move |status: OfflineRegionStatus| {
            if status.complete() {
                if !expects_inactive_status.get() {
                    expects_inactive_status.set(true);
                    assert_eq!(OfflineRegionDownloadState::Active, status.download_state);
                    assert_eq!(1, status.completed_resource_count);
                    assert_eq!(size.get(), status.completed_resource_size);
                    assert!(status.required_resource_count_is_precise);
                } else {
                    assert_eq!(OfflineRegionDownloadState::Inactive, status.download_state);
                    assert_eq!(1, status.completed_resource_count);
                    assert_eq!(size.get(), status.completed_resource_size);
                    assert!(status.required_resource_count_is_precise);
                    stop.stop();
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
    let download = test.download(region.get_id(), STYLE_URL, true);

    test.respond_with(Kind::Style, "inline_source.style.json", expect_style_url);
    test.respond_with(
        Kind::Tile,
        "0-0-0.vector.pbf",
        expect_tile_0_0_0("http://127.0.0.1:3000/{z}-{x}-{y}.vector.pbf"),
    );

    let (size, stop) = (test.size.clone(), test.run_loop.clone());
    let observer = MockObserver {
        status_changed_fn: Some(Box::new(move |status: OfflineRegionStatus| {
            if status.complete() {
                assert_eq!(1, status.completed_tile_count);
                assert_eq!(2, status.completed_resource_count);
                assert_eq!(size.get(), status.completed_resource_size);
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
    let download = test.download(region.get_id(), STYLE_URL, false);

    test.respond_with(Kind::Style, "geojson_source.style.json", expect_style_url);
    test.respond_with(Kind::Source, "geojson.json", |resource| {
        assert_eq!("http://127.0.0.1:3000/geojson.json", resource.url);
    });

    let (size, stop) = (test.size.clone(), test.run_loop.clone());
    let observer = MockObserver {
        status_changed_fn: Some(Box::new(move |status: OfflineRegionStatus| {
            if status.complete() {
                assert_eq!(2, status.completed_resource_count);
                assert_eq!(size.get(), status.completed_resource_size);
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

/// The completion check of Activate and ExcludeIdeographs: deactivate, then
/// compare the computed status with the reported one.
fn check_completed_and_computed(
    download: &WeakOfflineDownload,
    size: u64,
    expected_resources: u64,
    status: OfflineRegionStatus,
) {
    assert_eq!(status.completed_tile_count, status.required_tile_count);
    assert_eq!(expected_resources, status.completed_resource_count);
    assert_eq!(size, status.completed_resource_size);

    download.set_state(OfflineRegionDownloadState::Inactive);
    let computed_status = download.get_status().expect("download");
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
    let download = test.download(region.get_id(), STYLE_URL, true);

    respond_to_full_style(&test, true);

    let (size, stop, weak) = (test.size.clone(), test.run_loop.clone(), download.downgrade());
    let observer = MockObserver {
        status_changed_fn: Some(Box::new(move |status: OfflineRegionStatus| {
            if status.complete() {
                // 256 glyphs, 2 sprite images, 2 sprite jsons, 1 tile, 1
                // style, source, image
                check_completed_and_computed(&weak, size.get(), 264, status);
                stop.stop();
            }
        })),
        ..Default::default()
    };

    download.set_observer(Some(Rc::new(observer)));
    download.set_state(OfflineRegionDownloadState::Active);

    test.run_loop.run();
}

// gtest: OfflineDownload.ExcludeIdeographs
#[test]
fn exclude_ideographs() {
    let test = OfflineTest::new();
    let region = test.create_region().expect("region");
    let download = test.download(region.get_id(), STYLE_URL, false);

    respond_to_full_style(&test, true);

    let (size, stop, weak) = (test.size.clone(), test.run_loop.clone(), download.downgrade());
    let observer = MockObserver {
        status_changed_fn: Some(Box::new(move |status: OfflineRegionStatus| {
            if status.complete() {
                // glyph ranges, 2 sprite images, 2 sprite jsons, 1 tile, 1
                // style, source, image
                check_completed_and_computed(
                    &weak,
                    size.get(),
                    u64::from(NON_IDEOGRAPH_GLYPH_RANGES_PER_FONT_STACK) + 8,
                    status,
                );
                stop.stop();
            }
        })),
        ..Default::default()
    };

    download.set_observer(Some(Rc::new(observer)));
    download.set_state(OfflineRegionDownloadState::Active);

    test.run_loop.run();
}

/// `FakeFileSource::FakeFileRequest`.
struct FakeFileRequest {
    id: u64,
    requests: Rc<RefCell<Vec<FakeRequestEntry>>>,
}

struct FakeRequestEntry {
    id: u64,
    resource: Resource,
    callback: Rc<RefCell<Callback>>,
}

impl AsyncRequest for FakeFileRequest {}

impl Drop for FakeFileRequest {
    fn drop(&mut self) {
        self.requests.borrow_mut().retain(|entry| entry.id != self.id);
    }
}

/// `FakeOnlineFileSource` (test/src/mln/test/fake_file_source.hpp): records
/// the requests; `respond` answers the first of a kind. Requests come from
/// the thread that created it.
struct FakeOnlineFileSource {
    requests: LocalObject<RefCell<Vec<FakeRequestEntry>>>,
    next_id: std::sync::atomic::AtomicU64,
    online_fs: OnlineFileSource,
    resource_options: Mutex<ResourceOptions>,
    client_options: Mutex<ClientOptions>,
}

impl FakeOnlineFileSource {
    fn new() -> Self {
        FakeOnlineFileSource {
            requests: LocalObject::new(RefCell::new(Vec::new())),
            next_id: std::sync::atomic::AtomicU64::new(0),
            online_fs: OnlineFileSource::new(&ResourceOptions::default_options(), &ClientOptions::default()),
            resource_options: Mutex::new(ResourceOptions::default_options()),
            client_options: Mutex::new(ClientOptions::default()),
        }
    }

    fn requests(&self) -> Rc<RefCell<Vec<FakeRequestEntry>>> {
        // LocalObject<RefCell<..>> holds an Rc<RefCell<..>>.
        self.requests.get().expect("FakeOnlineFileSource used from another thread")
    }

    fn respond(&self, kind: Kind, response: &Response) -> bool {
        let callback = self
            .requests()
            .borrow()
            .iter()
            .find(|entry| entry.resource.kind == kind)
            .map(|entry| entry.callback.clone());

        let request_found = callback.is_some();

        if let Some(callback) = callback {
            // Copy the callback, in case calling it deallocates the
            // AsyncRequest.
            (callback.borrow_mut())(response.clone());
        }

        request_found
    }
}

impl FileSource for FakeOnlineFileSource {
    fn request(&self, resource: &Resource, callback: Callback) -> Box<dyn AsyncRequest> {
        let id = self.next_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let requests = self.requests();
        requests.borrow_mut().push(FakeRequestEntry {
            id,
            resource: resource.clone(),
            callback: Rc::new(RefCell::new(callback)),
        });
        Box::new(FakeFileRequest { id, requests })
    }

    fn can_request(&self, _resource: &Resource) -> bool {
        true
    }

    fn get_property(&self, property: &str) -> Value {
        self.online_fs.get_property(property)
    }

    fn set_resource_options(&self, options: ResourceOptions) {
        *self.resource_options.lock().unwrap() = options;
    }

    fn get_resource_options(&self) -> ResourceOptions {
        self.resource_options.lock().unwrap().clone()
    }

    fn set_client_options(&self, options: ClientOptions) {
        *self.client_options.lock().unwrap() = options;
    }

    fn get_client_options(&self) -> ClientOptions {
        self.client_options.lock().unwrap().clone()
    }
}

// gtest: OfflineDownload.DoesNotFloodTheFileSourceWithRequests
#[test]
fn does_not_flood_the_file_source_with_requests() {
    let test = OfflineTest::new();
    let file_source = Arc::new(FakeOnlineFileSource::new());
    let region = test.create_region().expect("region");
    let download = test.download_with_zoom(region.get_id(), STYLE_URL, 0.0, 0.0, true, file_source.clone());

    download.set_observer(Some(Rc::new(MockObserver::default())));
    download.set_state(OfflineRegionDownloadState::Active);
    test.run_loop.run_once();

    assert_eq!(1, file_source.requests().borrow().len());

    file_source.respond(Kind::Style, &test.response("style.json"));
    test.run_loop.run_once();

    let Value::Uint(max) = file_source.get_property(MAX_CONCURRENT_REQUESTS_KEY) else {
        panic!("max-concurrent-requests is not a uint");
    };
    assert_eq!(max as usize, file_source.requests().borrow().len());
}

// gtest: OfflineDownload.GetStatusNoResources
#[test]
fn get_status_no_resources() {
    let test = OfflineTest::new();
    let region = test.create_region().expect("region");
    let download = test.download(region.get_id(), STYLE_URL, false);
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
    let download = test.download(region.get_id(), STYLE_URL, true);

    let response = test.response("style.json");
    test.db.borrow_mut().put_region_resource(1, &Resource::style(STYLE_URL), &response).unwrap();

    let status = download.get_status();

    assert_eq!(OfflineRegionDownloadState::Inactive, status.download_state);
    assert_eq!(1, status.completed_resource_count);
    assert_eq!(test.size.get(), status.completed_resource_size);
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
    let download = test.download(region.get_id(), STYLE_URL, true);

    let response = test.response("style.json");
    test.db.borrow_mut().put_region_resource(1, &Resource::style(STYLE_URL), &response).unwrap();

    let response = test.response("streets.json");
    test.db
        .borrow_mut()
        .put_region_resource(1, &Resource::source("http://127.0.0.1:3000/streets.json"), &response)
        .unwrap();

    let status = download.get_status();

    assert_eq!(OfflineRegionDownloadState::Inactive, status.download_state);
    assert_eq!(2, status.completed_resource_count);
    assert_eq!(test.size.get(), status.completed_resource_size);
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
    let download = OfflineDownload::new(
        region.get_id(),
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
        test.db.clone(),
        test.file_source.clone(),
    );

    let response = test.response("style.json");
    test.db.borrow_mut().put_region_resource(1, &Resource::style(STYLE_URL), &response).unwrap();
    let response = test.response("streets.json");
    test.db
        .borrow_mut()
        .put_region_resource(1, &Resource::source("http://127.0.0.1:3000/streets.json"), &response)
        .unwrap();

    let status = download.get_status();

    // One tile at z14 and one at the source's maxzoom, z15.
    assert_eq!(2, status.required_tile_count);
}

fn connection_error() -> Response {
    let mut response = Response::new();
    response.error = Some(Box::new(Error::new(Reason::Connection, "connection error")));
    response
}

// gtest: OfflineDownload.RequestError
#[test]
fn request_error() {
    let test = OfflineTest::new();
    let region = test.create_region().expect("region");
    let download = test.download(region.get_id(), STYLE_URL, false);

    test.respond(Kind::Style, |_| connection_error());

    let stop = test.run_loop.clone();
    let observer = MockObserver {
        response_error_fn: Some(Box::new(move |error: Error| {
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
    let download = test.download(region.get_id(), STYLE_URL, true);

    let (file_source, size) = (Arc::downgrade(&test.file_source), test.size.clone());
    test.respond(Kind::Style, move |_| {
        let size = size.clone();
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

// gtest: OfflineDownload.TileCountLimitExceededNoTileResponse
#[test]
fn tile_count_limit_exceeded_no_tile_response() {
    let test = OfflineTest::new();
    let region = test.create_region().expect("region");
    let download = test.download(region.get_id(), STYLE_URL, false);

    let tile_limit: u64 = 0;

    test.db.borrow_mut().set_offline_mapbox_tile_count_limit(tile_limit);

    test.respond_with(Kind::Style, "mapbox_source.style.json", expect_style_url);

    let mapbox_tile_count_limit_exceeded_called = Rc::new(Cell::new(false));

    let (called, called2, stop) = (
        mapbox_tile_count_limit_exceeded_called.clone(),
        mapbox_tile_count_limit_exceeded_called.clone(),
        test.run_loop.clone(),
    );
    let observer = MockObserver {
        mapbox_tile_count_limit_exceeded_fn: Some(Box::new(move |limit: u64| {
            assert!(!called.get());
            assert_eq!(tile_limit, limit);
            called.set(true);
        })),
        status_changed_fn: Some(Box::new(move |status: OfflineRegionStatus| {
            if !called2.get() {
                assert!(!status.complete());
                assert_eq!(OfflineRegionDownloadState::Active, status.download_state);
            } else {
                assert_eq!(OfflineRegionDownloadState::Inactive, status.download_state);
                stop.stop();
            }
        })),
        ..Default::default()
    };

    download.set_observer(Some(Rc::new(observer)));
    download.set_state(OfflineRegionDownloadState::Active);

    test.run_loop.run();
}

// gtest: OfflineDownload.TileCountLimitExceededWithTileResponse
#[test]
fn tile_count_limit_exceeded_with_tile_response() {
    let test = OfflineTest::new();
    let region = test.create_region().expect("region");
    let download = test.download(region.get_id(), STYLE_URL, true);

    let tile_limit: u64 = 1;

    test.db.borrow_mut().set_offline_mapbox_tile_count_limit(tile_limit);

    test.respond_with(Kind::Style, "mapbox_source.style.json", expect_style_url);
    test.respond_with(Kind::Tile, "0-0-0.vector.pbf", expect_tile_0_0_0("maptiler://{z}-{x}-{y}.vector.pbf"));

    let mapbox_tile_count_limit_exceeded_called = Rc::new(Cell::new(false));

    let (called, called2, stop) = (
        mapbox_tile_count_limit_exceeded_called.clone(),
        mapbox_tile_count_limit_exceeded_called.clone(),
        test.run_loop.clone(),
    );
    let observer = MockObserver {
        mapbox_tile_count_limit_exceeded_fn: Some(Box::new(move |limit: u64| {
            assert!(!called.get());
            assert_eq!(tile_limit, limit);
            called.set(true);
        })),
        status_changed_fn: Some(Box::new(move |status: OfflineRegionStatus| {
            if !called2.get() {
                assert_eq!(OfflineRegionDownloadState::Active, status.download_state);
            } else {
                assert_eq!(OfflineRegionDownloadState::Inactive, status.download_state);
                stop.stop();
            }
            if status.completed_resource_count > tile_limit {
                stop.stop();
            }
        })),
        ..Default::default()
    };

    download.set_observer(Some(Rc::new(observer)));
    download.set_state(OfflineRegionDownloadState::Active);

    test.run_loop.run();
}

/// `test.db.put(Resource::tile(...), test.response(...))`.
fn put_tile(test: &OfflineTest, x: i32, y: i32, z: i8) {
    let response = test.response("0-0-0.vector.pbf");
    test.db.borrow_mut().put(
        &Resource::tile("http://127.0.0.1:3000/{z}-{x}-{y}.vector.pbf", 1.0, x, y, z, Scheme::XYZ, LoadingMethod::All),
        &response,
    );
}

// gtest: OfflineDownload.WithPreviouslyExistingTile
#[test]
fn with_previously_existing_tile() {
    let test = OfflineTest::new();
    let region = test.create_region().expect("region");
    let download = test.download(region.get_id(), STYLE_URL, false);

    test.respond_with(Kind::Style, "inline_source.style.json", expect_style_url);

    put_tile(&test, 0, 0, 0);

    let (size, stop) = (test.size.clone(), test.run_loop.clone());
    let observer = MockObserver {
        status_changed_fn: Some(Box::new(move |status: OfflineRegionStatus| {
            if status.complete() {
                assert_eq!(2, status.completed_resource_count);
                assert_eq!(size.get(), status.completed_resource_size);
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
    let download = test.download(region.get_id(), STYLE_URL, true);

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

    let redownload = test.download(region.get_id(), STYLE_URL, false);

    let statuses_after_reactivate: Rc<RefCell<Vec<OfflineRegionStatus>>> = Rc::new(RefCell::new(Vec::new()));

    let (statuses, stop) = (statuses_after_reactivate.clone(), test.run_loop.clone());
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
    let download = test.download(region.get_id(), STYLE_URL, false);

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
    let download = test.download(region.get_id(), STYLE_URL, true);

    let low_offline = |resource: &Resource| {
        assert!(resource.priority == Priority::Low);
        assert!(resource.usage == Usage::Offline);
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

// gtest: OfflineDownload.DiskFull
#[test]
#[ignore = "TODO(port): test::SQLite3TestFS (test/src/mln/test/sqlite3_test_fs.cpp), a custom SQLite VFS that needs unsafe FFI"]
fn disk_full() {}

// Test verifies that resource batch buffer is cleared at loading interrupt.
// gtest: OfflineDownload.InterruptAndResume
#[test]
fn interrupt_and_resume() {
    let test = OfflineTest::new();
    let region = test.create_region().expect("region");
    let download = test.download(region.get_id(), STYLE_URL, true);

    respond_to_full_style(&test, true);

    let interrupted = Cell::new(false);
    let (stop, weak) = (test.run_loop.clone(), download.downgrade());
    // `test.loop.schedule([&] { ... })`: a task on the test's RunLoop.
    let interrupt = Rc::new(RefCell::new(None::<AsyncTask>));
    let interrupt2 = interrupt.clone();
    let observer = MockObserver {
        status_changed_fn: Some(Box::new(move |status: OfflineRegionStatus| {
            if !interrupted.get() && status.completed_resource_count > 0 {
                interrupted.set(true);
                assert!(!status.complete());
                let (stop, weak) = (stop.clone(), weak.clone());
                let task = AsyncTask::new(move || {
                    weak.set_state(OfflineRegionDownloadState::Inactive);
                    stop.stop();
                });
                task.send();
                *interrupt2.borrow_mut() = Some(task);
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
    drop(interrupt);
}

// gtest: OfflineDownload.NoFreezingOnCachedTilesAndNewStyle
#[test]
fn no_freezing_on_cached_tiles_and_new_style() {
    let test = OfflineTest::new();
    let region = test.create_region().expect("region");
    let download = test.download_with_zoom(region.get_id(), STYLE_URL, 1.0, 1.0, true, test.file_source.clone());

    test.file_source.set_property(MAX_CONCURRENT_REQUESTS_KEY, &Value::Uint(2));
    test.respond_with(Kind::Style, "inline_source.style.json", |_| {});
    // Number of resources must exceed MAX_CONCURRENT_REQUESTS_KEY
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
    let download = test.download(region.get_id(), STYLE_URL, true);

    respond_to_full_style(&test, false);
    test.respond(Kind::SpriteJSON, |_| {
        let mut response = Response::new();
        response.error = Some(Box::new(Error::new(Reason::NotFound, "")));
        response
    });

    let error_reported = Rc::new(Cell::new(false));
    let (reported, reported2, stop) = (error_reported.clone(), error_reported.clone(), test.run_loop.clone());
    let observer = MockObserver {
        response_error_fn: Some(Box::new(move |error: Error| {
            assert_eq!(Reason::NotFound, error.reason);
            reported.set(true);
        })),
        status_changed_fn: Some(Box::new(move |status: OfflineRegionStatus| {
            if status.complete() {
                assert!(reported2.get());
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
