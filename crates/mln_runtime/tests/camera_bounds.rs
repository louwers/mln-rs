//! The limits on the camera, and the area that a camera shows.

use mln_engine::camera::{BoundOptions, CameraOptions};
use mln_style::geo::LatLng;
use mln_style::size::Size;

use crate::support::still::{StillMap, StillOptions, assert_near};
use crate::support::wait;

const GLOBE_STYLE: &str = r#"{"version":8,"projection":{"type":"vertical-perspective"},"sources":{},"layers":[]}"#;

fn ll(lat: f64, lon: f64) -> LatLng {
    LatLng::new(lat, lon).expect("LatLng")
}

fn zoom(map: &StillMap) -> f64 {
    wait(map.map.camera()).camera.zoom
}

/// A minimum zoom keeps a lower zoom that the globe allows.
#[test]
fn the_globes_bounds_keep_a_zoom_that_the_globe_allows() {
    let map = StillMap::still();
    map.load_json(GLOBE_STYLE);
    map.jump(CameraOptions { center: Some(ll(70.0, 0.0)), zoom: Some(-1.5), ..Default::default() });
    assert_near(-1.5, zoom(&map), 1e-9);
    wait(map.map.set_bounds(BoundOptions { min_zoom: Some(0.0), ..Default::default() }));
    assert_near(-1.5, zoom(&map), 1e-9);
}

/// The area that a globe's camera shows, near the poles too, and looking
/// away from a pole behind the camera with the top of the view in the sky.
#[test]
fn the_globe_gives_the_bounds_of_a_camera() {
    // The center's longitude and latitude, the zoom, bearing and pitch, and
    // the west, south, east and north of the bounds.
    let cases: [[f64; 9]; 9] = [
        [
            0.0,
            0.0,
            1.0,
            0.0,
            0.0,
            -79.917_960_421_069_67,
            -79.917_960_421_069_67,
            79.917_960_421_069_67,
            79.917_960_421_069_67,
        ],
        [
            30.0,
            20.0,
            1.5,
            30.0,
            30.0,
            -56.805_975_633_856_576,
            -56.895_025_199_618_885,
            94.307_727_447_969_46,
            73.433_065_214_319_64,
        ],
        [0.0, 75.0, 1.0, 0.0, 0.0, -180.0, 32.709_224_814_213_71, 180.0, 90.0],
        [
            0.0,
            75.0,
            2.5,
            0.0,
            0.0,
            -54.890_622_567_870_65,
            64.728_839_252_589_52,
            54.890_622_567_870_65,
            83.474_993_581_706_49,
        ],
        [
            10.0,
            10.0,
            3.0,
            0.0,
            0.0,
            -25.902_645_413_682_99,
            -19.587_774_363_137_328,
            45.902_645_413_698_34,
            36.437_839_913_113_18,
        ],
        [
            170.0,
            -10.0,
            2.0,
            45.0,
            40.0,
            90.751_538_705_752_07,
            -71.205_725_634_777_2,
            267.178_008_819_614_6,
            42.321_226_000_316_87,
        ],
        [
            0.0,
            80.0,
            3.0,
            180.0,
            75.0,
            -27.136_824_618_012_838,
            69.572_707_087_864_5,
            27.136_824_618_012_724,
            86.227_340_286_179_19,
        ],
        [
            0.0,
            84.5,
            5.0,
            180.0,
            84.0,
            -9.769_090_999_914_397,
            81.424_540_635_886_96,
            9.769_090_999_914_397,
            85.708_481_434_804_87,
        ],
        [
            0.0,
            85.0,
            6.0,
            180.0,
            85.0,
            -7.124_132_111_811_15,
            82.925_902_579_453_24,
            7.124_132_111_811_15,
            85.578_001_140_518_55,
        ],
    ];

    let map = StillMap::new(StillOptions { size: Size::new(512, 512), ..StillOptions::default() });
    wait(map.map.set_bounds(BoundOptions { max_pitch: Some(85.0), ..Default::default() }));
    map.load_json(GLOBE_STYLE);
    for [lng, lat, zoom, bearing, pitch, west, south, east, north] in cases {
        let camera = CameraOptions {
            center: Some(ll(lat, lng)),
            zoom: Some(zoom),
            bearing: Some(bearing),
            pitch: Some(pitch),
            ..Default::default()
        };
        let bounds = wait(map.map.lat_lng_bounds_for_camera(camera));
        assert_near(west, bounds.west(), 1e-5);
        assert_near(south, bounds.south(), 1e-5);
        assert_near(east, bounds.east(), 1e-5);
        assert_near(north, bounds.north(), 1e-5);
    }
}

/// A minimum zoom above the zoom lifts it, though the zoom was at the
/// maximum before.
#[test]
fn bounds_lift_the_zoom_to_a_minimum_above_the_old_maximum() {
    let map = StillMap::still();
    wait(map.map.set_bounds(BoundOptions { max_zoom: Some(8.0), ..Default::default() }));
    map.jump(CameraOptions { zoom: Some(6.0), ..Default::default() });
    wait(map.map.set_bounds(BoundOptions { min_zoom: Some(10.0), max_zoom: Some(16.0), ..Default::default() }));
    assert_near(10.0, zoom(&map), 1e-9);
}
