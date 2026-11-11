// Ports OfflineDownload.ResourceOfflineUsageUnset of
// test/storage/offline_download.test.cpp, which renders a map after the
// download (a separate target for the mln_map / mln_headless deps).
// C++ opens the database through test::SQLite3TestFS without limiting it
// (a pass-through); the port opens a file in the temporary directory
// directly.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use mln_actor::RunLoop;
use mln_headless::HeadlessFrontend;
use mln_map::{CameraOptions, Map, MapEvent, MapMode, MapOptions, RenderFrameStatus, RenderMode};
use mln_storage::offline::{
    OfflineRegionDownloadState, OfflineRegionMetadata, OfflineRegionObserver, OfflineRegionStatus,
    OfflineTilePyramidRegionDefinition,
};
use mln_storage::offline_database::OfflineDatabase;
use mln_storage::resource::{Priority, Usage};
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_storage::{Kind, Resource, Response};
use mln_storage_offline::OfflineDownload;
use mln_util::compression::compress;
use mln_util::geo::{LatLng, LatLngBounds};
use mln_util::tile_server_options::TileServerOptions;

fn cpp_path(relative: &str) -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join(relative)
}

/// `test.response(path)`.
fn fixture_response(size: &Cell<u64>, path: &str) -> Response {
    let mut result = Response::new();
    let data = std::fs::read(cpp_path(&format!("test/fixtures/offline_download/{path}"))).unwrap();
    let uncompressed = data.len() as u64;
    let compressed = compress(&data).unwrap().len() as u64;
    size.set(size.get() + uncompressed.min(compressed));
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
    let run_loop = RunLoop::new();
    let file_source = Arc::new(StubFileSource::new(ResponseType::Asynchronous));
    let dir = std::env::temp_dir().to_string_lossy().into_owned();
    let filename = format!("{dir}/offline_download.db");
    let _ = std::fs::remove_file(&filename);
    let db =
        Rc::new(RefCell::new(OfflineDatabase::new(filename.as_str(), &TileServerOptions::map_tiler_configuration())));
    let size = Rc::new(Cell::new(0u64));

    let region = db
        .borrow_mut()
        .create_region(
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
        )
        .expect("region");

    let download = OfflineDownload::new(
        region.get_id(),
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
        db.clone(),
        file_source.clone(),
    );

    let s = size.clone();
    file_source.set_response(Kind::Style, move |resource: &Resource| {
        assert!(resource.priority == Priority::Low);
        assert!(resource.usage == Usage::Offline);
        Some(fixture_response(&s, "inline_source.style.json"))
    });

    let s = size.clone();
    file_source.set_response(Kind::Tile, move |resource: &Resource| {
        assert!(resource.priority == Priority::Low);
        assert!(resource.usage == Usage::Offline);
        Some(fixture_response(&s, "0-0-0.vector.pbf"))
    });

    let (stop, db2, region_id) = (run_loop.clone(), db.clone(), region.get_id());
    download.set_observer(Some(Rc::new(StatusObserver(move |status: OfflineRegionStatus| {
        if status.complete() {
            // Once download completes, invalidate region and try rendering
            // map. Resource requests must not have Offline usage tag.
            assert!(db2.borrow_mut().invalidate_region(region_id).is_none());
            stop.stop();
        }
    }))));
    download.set_state(OfflineRegionDownloadState::Active);
    run_loop.run();

    let stubfile_source = Arc::new(StubFileSource::new(ResponseType::Asynchronous));
    let s = size.clone();
    stubfile_source.set_response(Kind::Style, move |resource: &Resource| {
        assert!(resource.usage != Usage::Offline);
        Some(fixture_response(&s, "inline_source.style.json"))
    });

    let s = size.clone();
    stubfile_source.set_response(Kind::Tile, move |resource: &Resource| {
        assert!(resource.usage != Usage::Offline);
        Some(fixture_response(&s, "0-0-0.vector.pbf"))
    });

    let frontend = HeadlessFrontend::with_pixel_ratio(1.0);
    let mut map = Map::with_file_source(
        stubfile_source,
        MapOptions::new().with_map_mode(MapMode::Continuous).with_size(frontend.get_size()),
    )
    .unwrap();
    frontend.attach(&map);

    map.style_mut().load_url("http://127.0.0.1:3000/inline_source.style.json");
    map.jump_to(&CameraOptions::default().with_center(Some(LatLng::new(0.0, 0.0).unwrap())).with_zoom(Some(0.0)));
    // `didFinishRenderingFrameCallback` stops the loop at the first full
    // frame.
    loop {
        let full_frame = map.drain_events().into_iter().any(|event| {
            matches!(event, MapEvent::RenderFrameFinished(RenderFrameStatus { mode: RenderMode::Full, .. }))
        });
        if full_frame {
            break;
        }
        if !run_loop.run_once() {
            run_loop.wait_for_work();
        }
    }
}
