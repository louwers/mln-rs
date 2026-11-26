// An offline download test that renders a map after the download, in a
// separate target for the `mln_map` and `mln_headless` dependencies. The
// database is a file in the temporary directory.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use mln_engine::Registry;
use mln_headless::HeadlessFrontend;
use mln_map::{CameraOptions, Map, MapEvent, MapMode, MapOptions, RenderFrameStatus, RenderMode};
use mln_storage::offline::{
    OfflineRegionDownloadState, OfflineRegionMetadata, OfflineRegionObserver, OfflineRegionStatus,
    OfflineTilePyramidRegionDefinition,
};
use mln_storage::offline_database::OfflineDatabase;
use mln_storage::resource::{Priority, Usage};
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_storage::{FileSource, Kind, Resource, Response};
use mln_storage_offline::{Downloads, OfflineDownload, Post};
use mln_style::geo::{LatLng, LatLngBounds};
use mln_util::compression::compress;
use mln_util::pool::Pool;
use mln_util::queue::Queue;

fn cpp_path(relative: &str) -> PathBuf {
    std::path::Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("testdata").join(relative)
}

/// `test.response(path)`.
fn fixture_response(size: &AtomicU64, path: &str) -> Response {
    let mut result = Response::new();
    let data = std::fs::read(cpp_path(&format!("test/fixtures/offline_download/{path}"))).unwrap();
    let uncompressed = data.len() as u64;
    let compressed = compress(&data).unwrap().len() as u64;
    size.fetch_add(uncompressed.min(compressed), Ordering::SeqCst);
    result.data = Some(Arc::new(data));
    result
}

struct StatusObserver<F: Fn(OfflineRegionStatus)>(F);

impl<F: Fn(OfflineRegionStatus)> OfflineRegionObserver for StatusObserver<F> {
    fn status_changed(&self, status: OfflineRegionStatus) {
        (self.0)(status);
    }
}

// Test verifies that resource requests for invalidated region don't
// have Resource::Usage::Offline tag set.
// gtest: OfflineDownload.ResourceOfflineUsageUnset
#[test]
fn resource_offline_usage_unset() {
    // `OfflineTest test{filename_test_fs}`.
    let run_loop = Queue::new();
    let pool = Pool::new(4);
    let file_source = StubFileSource::on_queue(&run_loop, ResponseType::Asynchronous);
    // A database of this run's own, which goes when the test ends.
    let dir = tempfile::tempdir().unwrap();
    let filename = dir.path().join("offline_download.db").to_string_lossy().into_owned();
    // The database and its downloads, which this thread owns as a
    // database's worker does; the downloads' tasks run on this thread's
    // queue, with the host.
    let host: Rc<RefCell<Option<(OfflineDatabase, Downloads)>>> = Rc::default();
    let handle = run_loop.downgrade();
    let post: Post = Arc::new(move |task| {
        if let Some(queue) = handle.upgrade() {
            queue.post(move |host: &mut Rc<RefCell<Option<(OfflineDatabase, Downloads)>>>| {
                if let Some((db, downloads)) = &mut *host.borrow_mut() {
                    task(db, downloads);
                }
            });
        }
    });
    let network: Arc<dyn FileSource> = Arc::<StubFileSource>::clone(&file_source);
    *host.borrow_mut() = Some((
        OfflineDatabase::new(filename.as_str()),
        Downloads::new(Some(network), Arc::clone(&post), Arc::clone(Registry::with_builtins().style())),
    ));
    let with = |f: &mut dyn FnMut(&mut OfflineDatabase, &mut Downloads)| {
        let mut host = host.borrow_mut();
        let (db, downloads) = host.as_mut().expect("the host");
        f(db, downloads);
    };
    let size = Arc::new(AtomicU64::new(0));

    let mut region = None;
    with(&mut |db, _| {
        region = Some(
            db.create_region(
                &OfflineTilePyramidRegionDefinition::new(
                    "",
                    LatLngBounds::hull(LatLng::new(1.0, 2.0).unwrap(), LatLng::new(3.0, 4.0).unwrap()),
                    5.0,
                    6.0,
                    1.0,
                    true,
                )
                .unwrap()
                .into(),
                &OfflineRegionMetadata::new(),
            ),
        );
    });
    let region = region.expect("a region").expect("region");

    let download = OfflineDownload::new(
        region.id,
        OfflineTilePyramidRegionDefinition::new(
            "http://127.0.0.1:3000/inline_source.style.json",
            LatLngBounds::world(),
            0.0,
            0.0,
            1.0,
            false,
        )
        .unwrap()
        .into(),
        Arc::<StubFileSource>::clone(&file_source),
        Arc::clone(&post),
        Arc::clone(Registry::with_builtins().style()),
    );

    let s = Arc::clone(&size);
    file_source.set_response(Kind::Style, move |resource: &Resource| {
        assert_eq!(resource.priority, Priority::Low);
        assert_eq!(resource.usage, Usage::Offline);
        Some(fixture_response(&s, "inline_source.style.json"))
    });

    let s = Arc::clone(&size);
    file_source.set_response(Kind::Tile, move |resource: &Resource| {
        assert_eq!(resource.priority, Priority::Low);
        assert_eq!(resource.usage, Usage::Offline);
        Some(fixture_response(&s, "0-0-0.vector.pbf"))
    });

    let stop = run_loop.clone();
    let mut download = download;
    download.set_observer(Some(Box::new(StatusObserver(move |status: OfflineRegionStatus| {
        if status.complete() {
            stop.stop();
        }
    }))));
    download.set_state(OfflineRegionDownloadState::Active);
    let region_id = region.id;
    let mut download = Some(download);
    with(&mut |_, downloads| downloads.insert(download.take().expect("the download")));
    run_loop.run(&mut Rc::clone(&host));
    // Once download completes, invalidate region and try rendering map.
    // Resource requests must not have Offline usage tag.
    with(&mut |db, _| assert!(db.invalidate_region(region_id).is_ok()));

    // The map's thread's queue.
    let queue = Queue::new();
    let stubfile_source = StubFileSource::on_queue(&queue, ResponseType::Asynchronous);
    let s = Arc::clone(&size);
    stubfile_source.set_response(Kind::Style, move |resource: &Resource| {
        assert_ne!(resource.usage, Usage::Offline);
        Some(fixture_response(&s, "inline_source.style.json"))
    });

    let s = Arc::clone(&size);
    stubfile_source.set_response(Kind::Tile, move |resource: &Resource| {
        assert_ne!(resource.usage, Usage::Offline);
        Some(fixture_response(&s, "0-0-0.vector.pbf"))
    });

    let mut frontend = HeadlessFrontend::with_pixel_ratio(1.0, &queue, &pool).expect("a headless frontend");
    let mut map = Map::with_file_source(
        stubfile_source,
        MapOptions { map_mode: MapMode::Continuous, size: frontend.size(), ..Default::default() },
        &pool,
    )
    .unwrap();
    frontend.attach(&mut map);

    map.load_style_url("http://127.0.0.1:3000/inline_source.style.json");
    map.jump_to(&CameraOptions { center: Some(LatLng::new(0.0, 0.0).unwrap()), zoom: Some(0.0), ..Default::default() });
    // `didFinishRenderingFrameCallback` stops the loop at the first full
    // frame.
    loop {
        let full_frame = map.drain_events().into_iter().any(|event| {
            matches!(event, MapEvent::RenderFrameFinished(RenderFrameStatus { mode: RenderMode::Full, .. }))
        });
        if full_frame {
            break;
        }
        // A frame reports to the map, which handles that before the queue
        // parks.
        let drew = frontend.render_update();
        if !queue.run_once(&mut ()) && !drew {
            queue.park();
        }
    }
}
