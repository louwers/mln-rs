use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use mln_actor::RunLoop;
use mln_headless::HeadlessFrontend;
use mln_map::{CameraOptions, Map, MapMode, MapOptions, TileSettings};
use mln_storage::Resource;
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_util::geo::LatLng;
use mln_util::pool::Pool;
use mln_util::size::Size;

use crate::support::*;

/// Parses the leading, optionally signed, decimal integer of `s`, as
/// `std::stoi` does.
fn set_prefetch_zoom_delta(map: &RefCell<Map>, prefetch_zoom_delta: u8) {
    let tile = TileSettings { prefetch_zoom_delta, ..map.borrow().snapshot().tile };
    map.borrow_mut().set_tile_settings(&tile);
}

fn stoi(s: &str) -> i32 {
    let trimmed = s.trim_start();
    let end = trimmed
        .char_indices()
        .find(|&(i, c)| !(c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+'))))
        .map_or(trimmed.len(), |(i, _)| i);
    trimmed[..end].parse().unwrap_or_else(|_| panic!("stoi: invalid argument {s:?}"))
}

// gtest: Map.PrefetchTiles
#[test]
fn prefetch_tiles() {
    let run_loop = RunLoop::new();
    let pool = Pool::new(4).unwrap();
    let file_source = Arc::new(StubFileSource::new(&run_loop, ResponseType::Asynchronous));

    // C++ `emergencyShutoff`: a 10 s timer that fails the test; here every
    // `runLoop.run()` below fails after 10 s (run_with_timeout).

    let observer = Rc::new(StubMapObserver::default());
    {
        let run_loop = run_loop.clone();
        observer.did_finish_loading_map_callback.set(move |()| run_loop.stop());
    }

    let mut frontend = HeadlessFrontend::new(Size::new(512, 512), 1.0, &run_loop, &pool).expect("a headless frontend");
    let map = Map::with_file_source(
        Arc::<StubFileSource>::clone(&file_source),
        MapOptions { map_mode: MapMode::Continuous, size: frontend.size(), ..Default::default() },
        &run_loop,
        &pool,
    )
    .unwrap();
    frontend.attach(&map);
    let map = Rc::new(RefCell::new(map));
    let frontend = Rc::new(RefCell::new(frontend));
    let _events = observer.observe(&map, Some(&frontend));

    let tiles: Rc<RefCell<Vec<i32>>> = Rc::new(RefCell::new(Vec::new()));

    {
        let tiles = Rc::clone(&tiles);
        let tile = Rc::new(read_bytes("test/fixtures/map/prefetch/tile.png"));
        file_source.set_global_response(move |res: &Resource| {
            let zoom = stoi(&res.url);
            tiles.borrow_mut().push(zoom);

            let mut response = mln_storage::Response::new();
            response.data = Some(Arc::new((*tile).clone()));
            Some(response)
        });
    }

    let check_tiles_for_zoom = |map: &RefCell<Map>, zoom: i32, expected: &[i32]| {
        tiles.borrow_mut().clear();

        {
            let mut map = map.borrow_mut();
            // Force tile reloading.
            map.load_style_json(&read_file("test/fixtures/map/prefetch/empty.json"));
            map.load_style_json(&read_file("test/fixtures/map/prefetch/style.json"));
            map.jump_to(&CameraOptions {
                center: Some(LatLng::new(40.726_989, -73.992_857).unwrap()),
                zoom: Some(f64::from(zoom)),
                ..Default::default()
            }); // Manhattan
        }
        run_with_timeout(&run_loop, 10);

        let mut actual = tiles.borrow().clone();
        assert_eq!(actual.len(), expected.len(), "zoom {zoom}: {actual:?} vs {expected:?}");
        // std::is_permutation
        let mut expected = expected.to_vec();
        actual.sort_unstable();
        expected.sort_unstable();
        assert_eq!(actual, expected, "zoom {zoom}");
    };

    // Check defaults, should be 4.
    assert_eq!(map.borrow().snapshot().tile.prefetch_zoom_delta, 4);
    check_tiles_for_zoom(&map, 12, &[13, 13, 13, 13, 13, 13, 13, 13, 13, 9]);

    // Setting it to 0 disables prefetching.
    set_prefetch_zoom_delta(&map, 0);

    // No prefetching, raster tiles will use ideal
    // tiles instead of the actual zoom level, that is
    // why the zoom levels for non-prefetched tiles are
    // not the same.
    check_tiles_for_zoom(&map, 10, &[11, 11, 11, 11, 11, 11, 11, 11, 11]);

    set_prefetch_zoom_delta(&map, 5);
    check_tiles_for_zoom(&map, 12, &[13, 13, 13, 13, 13, 13, 13, 13, 13, 8]);

    // Should clamp at `minzoom`.
    set_prefetch_zoom_delta(&map, 20);
    check_tiles_for_zoom(&map, 10, &[11, 11, 11, 11, 11, 11, 11, 11, 11, 0]);

    // Disabled again.
    set_prefetch_zoom_delta(&map, 0);
    check_tiles_for_zoom(&map, 13, &[14, 14, 14, 14, 14, 14, 14, 14, 14]);
}
