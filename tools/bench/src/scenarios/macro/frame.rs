//! Frames of a loaded map as its camera pans, turns and tilts, as during a
//! gesture or an animation over tiles already there.

use std::time::Instant;

use mln_map::{CameraOptions, MapMode};
use mln_util::geo::LatLng;

use super::VIEW;
use crate::scenario::{Bench, Kind, Scenario};
use crate::support::fixtures::{Fixtures, Tiles};
use crate::support::map::{HeadlessMap, tile_at, tile_center};

pub(crate) const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "macro/frame/street",
        kind: Kind::Macro,
        question: "What does a frame of a city's streets cost while the camera pans, turns and tilts over loaded tiles?",
        run: street,
    },
    Scenario {
        name: "macro/frame/globe",
        kind: Kind::Macro,
        question: "What does a frame of the globe cost while it turns and tilts over loaded tiles?",
        run: globe,
    },
];

/// Frames of each path: two seconds at 60 Hz, which places symbols about
/// seven times and fades them in and out between placements.
const FRAMES: usize = 120;

/// The question: what does a frame of western San Francisco (the Sunset
/// District) at zoom 13.5 cost while the camera turns a full circle, pans
/// around a small circle and tilts to 10 degrees and back? One measured
/// unit is one frame: its update, symbol placement, draw calls and the GPU
/// finishing it.
///
/// The camera circles the corner where four tiles of zoom 13 meet, and its
/// view stays inside them; at a steeper tilt it would see more.
fn street(bench: &mut Bench) {
    let fixtures = Fixtures::open(bench);
    let (x, y) = tile_at(12, super::area_center(&fixtures, Tiles::Street));
    let corner = tile_center(12, x, y);
    let camera = move |frame: usize| {
        let t = frame as f64 / FRAMES as f64;
        let turn = std::f64::consts::TAU * t;
        CameraOptions {
            center: Some(
                LatLng::new(corner.latitude() + 0.002 * turn.sin(), corner.longitude() + 0.003 * turn.cos())
                    .expect("a center"),
            ),
            zoom: Some(13.5),
            bearing: Some(360.0 * t),
            pitch: Some(10.0 * (std::f64::consts::PI * t).sin()),
            ..Default::default()
        }
    };
    let style = fixtures.style(Tiles::Street);
    let map = HeadlessMap::open(bench, &fixtures, VIEW, MapMode::Continuous, &style, &camera(0));
    frames(bench, map, camera);
}

/// The question: what does a frame of the whole globe (zoom 1.5) cost
/// while it turns 120 degrees of longitude under the camera, which turns
/// and tilts to 30 degrees and back? One measured unit is one frame.
fn globe(bench: &mut Bench) {
    let fixtures = Fixtures::open(bench);
    let camera = |frame: usize| {
        let t = frame as f64 / FRAMES as f64;
        let wave = (std::f64::consts::PI * t).sin();
        CameraOptions {
            center: Some(LatLng::new(20.0 + 15.0 * wave, -60.0 + 120.0 * t).expect("a center")),
            zoom: Some(1.5),
            bearing: Some(30.0 * t),
            pitch: Some(30.0 * wave),
            ..Default::default()
        }
    };
    let style = super::globe_style(&fixtures);
    let map = HeadlessMap::open(bench, &fixtures, VIEW, MapMode::Continuous, &style, &camera(0));
    frames(bench, map, camera);
}

/// Loads the start of the path, then measures one frame at each of its
/// cameras.
///
/// The renderer requests a tile again, and lays it out again, whenever it
/// comes back into view, so a path over loaded tiles keeps the same tiles in
/// view throughout. A frame that starts background work fails the scenario.
fn frames(bench: &mut Bench, mut map: HeadlessMap, camera: impl Fn(usize) -> CameraOptions) {
    map.settle();

    let drawn = map.frames();
    let objects = map.gpu_objects_created();
    let tasks = map.background_tasks();
    let mut times = Vec::with_capacity(FRAMES);
    for frame in 0..FRAMES {
        map.jump(&camera(frame));
        let start = Instant::now();
        bench.measure(|| map.draw_frame());
        times.push(start.elapsed());
    }
    assert_eq!(map.background_tasks(), tasks, "the path needs tiles it didn't start with");
    assert_eq!(map.frames() - drawn, FRAMES as u64, "one frame per camera");
    bench.per("frame", FRAMES as u64);
    bench.bytes("gpu_buffers", map.gpu_buffer_bytes());
    bench.count("gpu_objects", map.gpu_objects_created() - objects);
    bench.percentiles("frame", &times);
    if let Some(path) = &bench.image {
        map.save_image(path);
    }
}
