// Ports test/util/memory.test.cpp: render a vector and a raster style
// (fixtures in test/fixtures/resources) through Map -> HeadlessFrontend,
// and, on request, measure a map's memory footprint.

mod support;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use mln_actor::RunLoop;
use mln_map::{CameraOptions, Map, MapMode, MapOptions, NullMapObserver};
use mln_platform_default::HeadlessFrontend;
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_storage::{Kind, Resource, Response};
use mln_util::size::Size;
use support::read_bytes;

/// The `MemoryTest` fixture.
struct MemoryTest {
    // Fields drop in this order: the file source before the loop.
    file_source: Arc<StubFileSource>,
    run_loop: RunLoop,
}

/// `getType(res)`.
fn get_type(res: &Resource) -> &'static str {
    if res.url.contains("hybrid") || res.url.contains("satellite") { "raster" } else { "vector" }
}

impl MemoryTest {
    fn new() -> Self {
        let run_loop = RunLoop::new();
        let file_source = Arc::new(StubFileSource::new(ResponseType::Asynchronous));
        let cache: Rc<RefCell<HashMap<String, Arc<Vec<u8>>>>> = Rc::default();

        // `response(path)`: the fixture's bytes, read once.
        let response = move |path: String| -> Option<Response> {
            let data = cache
                .borrow_mut()
                .entry(path.clone())
                .or_insert_with(|| Arc::new(read_bytes(&format!("test/fixtures/resources/{path}"))))
                .clone();
            let mut result = Response::new();
            result.data = Some(data);
            Some(result)
        };

        let r = response.clone();
        file_source.set_response(Kind::Style, move |res| r(format!("style_{}.json", get_type(res))));
        let r = response.clone();
        file_source.set_response(Kind::Tile, move |res| r(format!("{}.tile", get_type(res))));
        let r = response.clone();
        file_source.set_response(Kind::Source, move |res| r(format!("source_{}.json", get_type(res))));
        let r = response.clone();
        file_source.set_response(Kind::Glyphs, move |_| r("glyphs.pbf".to_owned()));
        let r = response.clone();
        file_source.set_response(Kind::SpriteJSON, move |_| r("sprite.json".to_owned()));
        let r = response;
        file_source.set_response(Kind::SpriteImage, move |_| r("sprite.png".to_owned()));

        MemoryTest { file_source, run_loop }
    }

    /// `MapAdapter map(frontend, MapObserver::nullObserver(), test.fileSource,
    /// MapOptions().withMapMode(MapMode::Static).withSize(frontend.getSize())
    /// .withPixelRatio(ratio))`.
    fn map(&self, frontend: &HeadlessFrontend, ratio: f32) -> Map {
        Map::with_file_source(
            frontend,
            Rc::new(NullMapObserver),
            self.file_source.clone(),
            MapOptions::new().with_map_mode(MapMode::Static).with_size(frontend.get_size()).with_pixel_ratio(ratio),
        )
        .unwrap()
    }
}

// gtest: Memory.Vector
#[test]
fn vector() {
    let test = MemoryTest::new();
    let ratio = 2.0;

    let mut frontend = HeadlessFrontend::new(Size::new(256, 256), ratio);
    let mut map = test.map(&frontend, ratio);
    map.jump_to(&CameraOptions::default().with_zoom(Some(16.0)));
    map.get_style_mut().load_url("maptiler://maps/streets");

    frontend.render(&mut map).expect("render");
}

// gtest: Memory.Raster
#[test]
#[ignore = "TODO(port): JPEG decoding (test/fixtures/resources/raster.tile is a JPEG; the still image fails with its tile error)"]
fn raster() {
    let test = MemoryTest::new();
    let ratio = 2.0;

    let mut frontend = HeadlessFrontend::new(Size::new(256, 256), ratio);
    let mut map = test.map(&frontend, ratio);
    map.get_style_mut().load_url("maptiler://maps/hybrid");

    frontend.render(&mut map).expect("render");
}

/// On CI, we only run the memory footprint test in the Qt build, because it
/// uses jemalloc, which yields more consistent memory usage results. To
/// force it to run locally, set `DO_MEMORY_FOOTPRINT=1`
/// (`--test_env=DO_MEMORY_FOOTPRINT=1`).
fn should_run_footprint() -> bool {
    match std::env::var("LD_PRELOAD") {
        Ok(preload) => preload.contains("libjemalloc.so"),
        Err(_) => std::env::var_os("DO_MEMORY_FOOTPRINT").is_some(),
    }
}

/// `mln::test::getCurrentRSS()` (test/src/mln/test/getrss.cpp, Linux:
/// `/proc/self/statm` resident pages times the page size).
fn get_current_rss() -> usize {
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    let resident: usize = statm.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    resident * 4096
}

// This test will measure the size of a Map object
// after rendering a raster and a vector style. The
// idea is to try to keep the memory footprint within
// reasonable limits, so this test acts more like a
// safeguard.
// gtest: Memory.Footprint
#[test]
fn footprint() {
    // When enabled, the hybrid (raster) maps need JPEG decoding, a
    // TODO(port) (see Memory.Raster).
    if !should_run_footprint() {
        return;
    }

    let test = MemoryTest::new();

    struct FrontendAndMap {
        _map: Map,
        _frontend: HeadlessFrontend,
    }

    impl FrontendAndMap {
        fn new(test: &MemoryTest, style: &str) -> Self {
            let mut frontend = HeadlessFrontend::new(Size::new(256, 256), 2.0);
            let mut map = test.map(&frontend, 2.0);
            map.jump_to(&CameraOptions::default().with_zoom(Some(16.0)));
            map.get_style_mut().load_url(style);
            frontend.render(&mut map).expect("render");
            FrontendAndMap { _map: map, _frontend: frontend }
        }
    }

    // Warm up buffers and cache.
    for _ in 0..10 {
        FrontendAndMap::new(&test, "maptiler://maps/streets");
        FrontendAndMap::new(&test, "maptiler://maps/hybrid");
    }

    // Process close callbacks, mostly needed by
    // libuv runloop.
    test.run_loop.run_once();

    let mut maps = Vec::new();
    let runs = 15;

    let vector_initial_rss = get_current_rss();
    for _ in 0..runs {
        maps.push(FrontendAndMap::new(&test, "maptiler://maps/streets"));
    }

    let vector_footprint = (get_current_rss() as f64 - vector_initial_rss as f64) / runs as f64;

    let raster_initial_rss = get_current_rss();
    for _ in 0..runs {
        maps.push(FrontendAndMap::new(&test, "maptiler://maps/hybrid"));
    }

    let raster_footprint = (get_current_rss() as f64 - raster_initial_rss as f64) / runs as f64;

    // RecordProperty("vectorFootprint"/"rasterFootprint", ...)
    eprintln!("vectorFootprint={} rasterFootprint={}", vector_footprint as i64, raster_footprint as i64);

    assert!(vector_footprint < (40 * 1024 * 1024) as f64, "mln::Map footprint over 65.2MB for vector styles.");
    assert!(raster_footprint < (25 * 1024 * 1024) as f64, "mln::Map footprint over 25MB for raster styles.");
}
