//! The tiles of lower zooms that a map loads alongside those of its zoom,
//! as its tile options' `prefetch_zoom_delta` picks them.

use std::sync::Arc;

use mln_engine::camera::CameraOptions;
use mln_engine::storage::{Kind, Response};
use mln_runtime::TileOptions;
use mln_style::geo::LatLng;
use mln_style::size::Size;

use crate::support::still::{StillMap, StillOptions, read_bytes, read_file, serve};
use crate::support::wait;

// gtest: Map.PrefetchTiles
#[test]
fn prefetch_tiles() {
    let map = StillMap::new(StillOptions {
        size: Size::new(512, 512),
        mode: mln_engine::map::MapMode::Continuous,
        ..StillOptions::default()
    });
    let tile = Arc::new(read_bytes("test/fixtures/map/prefetch/tile.png"));
    let requests = serve(&map.runtime, move |_| {
        let mut response = Response { data: Some(Arc::clone(&tile)), ..Response::new() };
        // Expired, so that reloading the style requests the tiles again.
        response.caching.expires = Some(mln_engine::storage::chrono::Timestamp::from_secs(0));
        response
    });

    // Loads the style again at `zoom` over Manhattan, and checks the zooms
    // of the tiles it requests, in any order: the style's tile URLs are
    // their zooms.
    let check_tiles_for_zoom = |zoom: u8, expected: &[u8]| {
        requests.clear();
        map.load_json(&read_file("test/fixtures/map/prefetch/empty.json"));
        map.load_json(&read_file("test/fixtures/map/prefetch/style.json"));
        map.jump(CameraOptions {
            center: Some(LatLng::new(40.726_989, -73.992_857).unwrap()),
            zoom: Some(f64::from(zoom)),
            ..CameraOptions::default()
        });
        map.settle();

        let mut actual: Vec<u8> = requests
            .urls(Kind::Tile)
            .iter()
            .map(|url| url.parse().unwrap_or_else(|_| panic!("a tile URL of a zoom: {url}")))
            .collect();
        let mut expected = expected.to_vec();
        actual.sort_unstable();
        expected.sort_unstable();
        assert_eq!(actual, expected, "zoom {zoom}");
    };
    let set_prefetch_zoom_delta = |delta: u8| {
        wait(map.map.set_tile_options(TileOptions { prefetch_zoom_delta: Some(delta), ..TileOptions::default() }));
    };

    // Check defaults, should be 4.
    assert_eq!(map.map.snapshot().unwrap().tile.prefetch_zoom_delta, 4);
    check_tiles_for_zoom(12, &[13, 13, 13, 13, 13, 13, 13, 13, 13, 9]);

    // Setting it to 0 disables prefetching.
    set_prefetch_zoom_delta(0);

    // No prefetching, raster tiles will use ideal tiles instead of the
    // actual zoom level, that is why the zoom levels for non-prefetched
    // tiles are not the same.
    check_tiles_for_zoom(10, &[11, 11, 11, 11, 11, 11, 11, 11, 11]);

    set_prefetch_zoom_delta(5);
    check_tiles_for_zoom(12, &[13, 13, 13, 13, 13, 13, 13, 13, 13, 8]);

    // Should clamp at `minzoom`.
    set_prefetch_zoom_delta(20);
    check_tiles_for_zoom(10, &[11, 11, 11, 11, 11, 11, 11, 11, 11, 0]);

    // Disabled again.
    set_prefetch_zoom_delta(0);
    check_tiles_for_zoom(13, &[14, 14, 14, 14, 14, 14, 14, 14, 14]);
}
