//! The tiles a map loads at reduced detail far from the camera, as its tile
//! options' `lod_*` settings pick them. A vector source of zooms 0 to 18
//! counts them by its tile requests, as C++'s `TileLOD` tests count a custom
//! geometry source's fetches; C++ also compares images of the tile borders,
//! a debug overlay that mln-rs doesn't draw.

use mln_engine::camera::CameraOptions;
use mln_engine::storage::{Kind, Response};
use mln_runtime::TileOptions;
use mln_style::math::rad2deg;

use crate::support::still::{StillMap, read_file, serve};
use crate::support::wait;

/// One still of a [`Case`]: the tile options and pitch it sets first, and
/// how many tiles it requests.
struct Still {
    options: TileOptions,
    pitch: Option<f64>,
    tiles: usize,
}

/// A map at `zoom` that draws `stills` one after another.
struct Case {
    name: &'static str,
    zoom: f64,
    stills: Vec<Still>,
}

fn cases() -> Vec<Case> {
    let lod = |min_radius, scale, pitch_threshold, zoom_shift| TileOptions {
        lod_min_radius: Some(min_radius),
        lod_scale: Some(scale),
        lod_pitch_threshold: Some(pitch_threshold),
        lod_zoom_shift: Some(zoom_shift),
        ..TileOptions::default()
    };
    let shift = |zoom_shift| TileOptions { lod_zoom_shift: Some(zoom_shift), ..TileOptions::default() };
    vec![
        // gtest: TileLOD.disabled
        Case {
            name: "disabled",
            zoom: 10.0,
            stills: vec![Still {
                options: TileOptions { lod_pitch_threshold: Some(180.0), ..TileOptions::default() },
                pitch: None,
                tiles: 4,
            }],
        },
        // gtest: TileLOD.zoomShift
        Case {
            name: "zoom shift",
            zoom: 12.0,
            stills: vec![
                Still { options: shift(4.0), pitch: None, tiles: 64 },
                Still { options: shift(-4.0), pitch: None, tiles: 4 },
            ],
        },
        // gtest: TileLOD.pitchThreshold
        Case {
            name: "pitch threshold",
            zoom: 10.0,
            stills: vec![
                Still { options: lod(1.0, 1.0, 45.0, 4.0), pitch: Some(44.0), tiles: 140 },
                Still { options: TileOptions::default(), pitch: Some(46.0), tiles: 22 },
            ],
        },
        // gtest: TileLOD.scale
        Case {
            name: "scale",
            zoom: 0.0,
            stills: vec![Still { options: lod(1.0, 0.4, rad2deg(-1.0), 5.0), pitch: None, tiles: 112 }],
        },
        // gtest: TileLOD.radius
        Case {
            name: "radius",
            zoom: 0.0,
            stills: vec![Still { options: lod(5.0, 1.0, rad2deg(-1.0), 5.0), pitch: None, tiles: 172 }],
        },
    ]
}

/// Each still of each case requests as many tiles as C++'s does. A still
/// after the first counts only the tiles that the stills before it didn't
/// load.
#[test]
fn the_level_of_detail_settings_pick_the_tiles_to_load() {
    for case in cases() {
        let map = StillMap::still();
        let requests = serve(&map.runtime, |_| Response { no_content: true, ..Response::new() });
        map.load_json(&read_file("test/fixtures/api/empty.json"));
        let source = r#"{"type": "vector", "tiles": ["custom/{z}/{x}/{y}"], "maxzoom": 18}"#;
        wait(map.map.add_style_source_json("custom", source));
        let layer =
            r#"{"id": "circles", "type": "circle", "source": "custom", "paint": {"circle-color": "rgba(0,0,255,0)"}}"#;
        wait(map.map.add_style_layer_json(layer, None));
        map.jump(CameraOptions { zoom: Some(case.zoom), ..CameraOptions::default() });

        for (index, still) in case.stills.into_iter().enumerate() {
            wait(map.map.set_tile_options(still.options));
            if let Some(pitch) = still.pitch {
                map.jump(CameraOptions { pitch: Some(pitch), ..CameraOptions::default() });
            }
            requests.clear();
            map.render();
            assert_eq!(requests.count(Kind::Tile), still.tiles, "{}, still {index}", case.name);
        }
    }
}
