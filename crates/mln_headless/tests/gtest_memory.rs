// Ports test/util/memory.test.cpp: render a vector and a raster style
// (fixtures in test/fixtures/resources) through Map -> HeadlessFrontend,
// and, on request, measure a map's memory footprint.

mod support;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use mln_actor::RunLoop;
use mln_headless::HeadlessFrontend;
use mln_map::{CameraOptions, Map, MapMode, MapOptions};
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

    /// `MapAdapter map(frontend, test.fileSource,
    /// MapOptions().withMapMode(MapMode::Static).withSize(frontend.getSize())
    /// .withPixelRatio(ratio))`.
    fn map(&self, frontend: &HeadlessFrontend, ratio: f32) -> Map {
        let map = Map::with_file_source(
            self.file_source.clone(),
            MapOptions {
                map_mode: MapMode::Static,
                size: frontend.get_size(),
                pixel_ratio: ratio,
                ..Default::default()
            },
        )
        .unwrap();
        frontend.attach(&map);
        map
    }
}

// gtest: Memory.Vector
#[test]
fn vector() {
    let test = MemoryTest::new();
    let ratio = 2.0;

    let mut frontend = HeadlessFrontend::new(Size::new(256, 256), ratio).expect("a headless frontend");
    let mut map = test.map(&frontend, ratio);
    map.jump_to(&CameraOptions { zoom: Some(16.0), ..Default::default() });
    map.style_mut().load_url("https://example.com/maps/streets");

    frontend.render(&mut map).expect("render");
}

// gtest: Memory.Raster
#[test]
fn raster() {
    let test = MemoryTest::new();
    let ratio = 2.0;

    let mut frontend = HeadlessFrontend::new(Size::new(256, 256), ratio).expect("a headless frontend");
    let mut map = test.map(&frontend, ratio);
    map.style_mut().load_url("https://example.com/maps/hybrid");

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
    struct FrontendAndMap {
        _map: Map,
        _frontend: HeadlessFrontend,
    }

    impl FrontendAndMap {
        fn new(test: &MemoryTest, style: &str) -> Self {
            let mut frontend = HeadlessFrontend::new(Size::new(256, 256), 2.0).expect("a headless frontend");
            let mut map = test.map(&frontend, 2.0);
            map.jump_to(&CameraOptions { zoom: Some(16.0), ..Default::default() });
            map.style_mut().load_url(style);
            frontend.render(&mut map).expect("render");
            FrontendAndMap { _map: map, _frontend: frontend }
        }
    }

    if !should_run_footprint() {
        return;
    }

    let test = MemoryTest::new();

    // Warm up buffers and cache.
    for _ in 0..10 {
        FrontendAndMap::new(&test, "https://example.com/maps/streets");
        FrontendAndMap::new(&test, "https://example.com/maps/hybrid");
    }

    // Process close callbacks, mostly needed by
    // libuv runloop.
    test.run_loop.run_once();

    let mut maps = Vec::new();
    let runs = 15;

    let vector_initial_rss = get_current_rss();
    for _ in 0..runs {
        maps.push(FrontendAndMap::new(&test, "https://example.com/maps/streets"));
    }

    let vector_footprint = (get_current_rss() as f64 - vector_initial_rss as f64) / f64::from(runs);

    let raster_initial_rss = get_current_rss();
    for _ in 0..runs {
        maps.push(FrontendAndMap::new(&test, "https://example.com/maps/hybrid"));
    }

    let raster_footprint = (get_current_rss() as f64 - raster_initial_rss as f64) / f64::from(runs);

    // RecordProperty("vectorFootprint"/"rasterFootprint", ...)
    eprintln!("vectorFootprint={} rasterFootprint={}", vector_footprint as i64, raster_footprint as i64);

    assert!(vector_footprint < f64::from(40 * 1024 * 1024), "mln::Map footprint over 65.2MB for vector styles.");
    assert!(raster_footprint < f64::from(25 * 1024 * 1024), "mln::Map footprint over 25MB for raster styles.");
}

/// The live-resource counters of the frontend's context, by name.
fn live_resources(frontend: &HeadlessFrontend) -> Vec<(&'static str, i32)> {
    let _scope = mln_gfx::BackendScope::new(frontend.get_backend(), mln_gfx::ScopeType::Explicit);
    let stats = frontend.get_backend().get_context().rendering_stats().clone();
    vec![
        ("num_active_textures", stats.num_active_textures),
        ("num_buffers", stats.num_buffers),
        ("num_frame_buffers", stats.num_frame_buffers),
        ("num_index_buffers", stats.num_index_buffers),
        ("num_vertex_buffers", stats.num_vertex_buffers),
        ("num_uniform_buffers", stats.num_uniform_buffers),
        ("mem_textures", stats.mem_textures),
        ("mem_buffers", stats.mem_buffers),
        ("mem_index_buffers", stats.mem_index_buffers),
        ("mem_vertex_buffers", stats.mem_vertex_buffers),
        ("mem_uniform_buffers", stats.mem_uniform_buffers),
    ]
}

fn live(counters: &[(&'static str, i32)], name: &str) -> i32 {
    counters.iter().find(|(n, _)| *n == name).map(|(_, v)| *v).expect("a counter")
}

// Port-only: the live buffer and texture counters of the rendering stats
// rise when a map loads its tiles and fall back when the tiles go and when
// the map is dropped.
#[test]
fn rendering_stats_count_live_resources() {
    const EMPTY: &str = r#"{"version": 8, "sources": {}, "layers": []}"#;
    let test = MemoryTest::new();
    let mut frontend = HeadlessFrontend::new(Size::new(256, 256), 1.0).expect("a headless frontend");
    let mut map = test.map(&frontend, 1.0);
    map.jump_to(&CameraOptions { zoom: Some(16.0), ..Default::default() });

    map.style_mut().load_url("https://example.com/maps/streets");
    frontend.render(&mut map).expect("render");
    let loaded = live_resources(&frontend);
    for (name, value) in &loaded {
        assert!(*value > 0, "{name} is {value} with the map loaded");
    }
    let kinds = ["index", "vertex", "uniform"];
    let sum = |prefix: &str| kinds.iter().map(|k| live(&loaded, &format!("{prefix}_{k}_buffers"))).sum::<i32>();
    assert!(live(&loaded, "num_buffers") >= sum("num"));
    assert!(live(&loaded, "mem_buffers") >= sum("mem"));

    // The tiles go with their style.
    map.style_mut().load_json(EMPTY);
    frontend.render(&mut map).expect("render");
    let evicted = live_resources(&frontend);
    assert_eq!(live(&evicted, "num_index_buffers"), 0);
    assert_eq!(live(&evicted, "mem_index_buffers"), 0);
    for name in ["num_buffers", "num_vertex_buffers", "num_uniform_buffers", "mem_buffers", "mem_vertex_buffers"] {
        assert!(live(&evicted, name) < live(&loaded, name), "{name} stays at {} without tiles", live(&loaded, name));
    }

    // Loading the tiles again and dropping them leaves the same resources.
    map.style_mut().load_url("https://example.com/maps/streets");
    frontend.render(&mut map).expect("render");
    assert!(live(&live_resources(&frontend), "num_index_buffers") > 0);
    map.style_mut().load_json(EMPTY);
    frontend.render(&mut map).expect("render");
    assert_eq!(live_resources(&frontend), evicted);

    // The frontend drops its renderer on the run loop once the map goes.
    drop(map);
    test.run_loop.run_once();
    let torn_down = live_resources(&frontend);
    assert!(live(&torn_down, "num_active_textures") < live(&loaded, "num_active_textures"));
    assert!(live(&torn_down, "mem_textures") < live(&loaded, "mem_textures"));
}

// Port-only: on the globe, each frame's clip masks take the place of the
// last frame's in the rendering stats, as the flat clip masks do, rather
// than adding to them.
#[test]
fn rendering_stats_count_one_frame_of_globe_clip_masks() {
    let test = MemoryTest::new();
    let mut frontend = HeadlessFrontend::new(Size::new(256, 256), 1.0).expect("a headless frontend");
    let mut map = test.map(&frontend, 1.0);
    map.style_mut().load_json(
        r#"{"version": 8, "projection": {"type": "vertical-perspective"},
            "sources": {"shape": {"type": "geojson", "data": {"type": "Polygon",
                "coordinates": [[[-60, -40], [60, -40], [60, 40], [-60, 40], [-60, -40]]]}}},
            "layers": [{"id": "shape", "type": "fill", "source": "shape"}]}"#,
    );
    frontend.render(&mut map).expect("render");
    let first = live_resources(&frontend);
    frontend.render(&mut map).expect("render");
    let second = live_resources(&frontend);
    for name in ["num_buffers", "mem_buffers"] {
        assert_eq!(live(&second, name), live(&first, name), "{name} after the same view again");
    }
}
