//! A camera flight across zooms, as when a user flies to a place and looks
//! around it, with the tiles it needs arriving while it flies.

use std::sync::Arc;

use mln_map::{CameraOptions, MapMode};
use mln_util::geo::LatLng;

use super::VIEW;
use crate::scenario::{Bench, Kind, Scenario};
use crate::support::fixtures::{Fixtures, Tiles};
use crate::support::latency::Latency;
use crate::support::map::{Frontend, HeadlessMap};

pub(crate) const SCENARIOS: &[Scenario] = &[Scenario {
    name: "macro/navigate/street",
    kind: Kind::Macro,
    question: "What does a flight into a city's streets and back out cost, with tiles arriving while it flies?",
    run: street,
}];

/// The frames of the flight: four seconds at 60 Hz.
const FRAMES: usize = 240;

/// The frames between a request and its answer: 100 ms at 60 Hz, about
/// what a tile takes to come from a nearby CDN.
const LATENCY: u64 = 6;

/// A point of the flight: at frame `frame`, the camera is `east` and
/// `north` degrees from the center of the street tiles, at `zoom`,
/// `bearing` and `pitch`.
struct Key {
    frame: usize,
    east: f64,
    north: f64,
    zoom: f64,
    bearing: f64,
    pitch: f64,
}

/// The flight: from an overview of the city into its streets, along them,
/// and back out. Each leg eases in and out. Every center stays inside the
/// street tiles' area, so the camera only sees tiles of the set.
const FLIGHT: [Key; 4] = [
    Key { frame: 0, east: -0.03, north: -0.01, zoom: 11.0, bearing: 0.0, pitch: 0.0 },
    Key { frame: 100, east: 0.015, north: 0.012, zoom: 15.0, bearing: 20.0, pitch: 30.0 },
    Key { frame: 160, east: 0.035, north: 0.018, zoom: 15.5, bearing: 40.0, pitch: 30.0 },
    Key { frame: FRAMES, east: 0.0, north: 0.0, zoom: 12.0, bearing: 0.0, pitch: 0.0 },
];

/// The question: what does flying from a view of San Francisco at zoom 11
/// into its streets at zoom 15.5 and back out to zoom 12 cost, as each
/// tile, glyph range and image it asks for arrives 6 frames after it asked?
/// One measured unit is one frame: sending on the answers due at it,
/// laying out what they brought, its update, symbol placement, draw calls
/// and the GPU finishing it.
///
/// The map starts settled at the start of the flight. Its frames then show
/// what has arrived, and parent or child tiles where a tile is missing, as
/// a map does over a network; tiles arrive on a schedule that only frames
/// advance, so every run draws the same frames with the same tiles.
fn street(bench: &mut Bench) {
    let fixtures = Fixtures::open(bench);
    let center = fixtures.area_center(Tiles::Street);
    let camera = |frame: usize| {
        let leg = FLIGHT.windows(2).find(|leg| frame <= leg[1].frame).expect("a frame of the flight");
        let (from, to) = (&leg[0], &leg[1]);
        let t = (frame - from.frame) as f64 / (to.frame - from.frame) as f64;
        // Smoothstep: the leg starts and ends at rest.
        let t = t * t * (3.0 - 2.0 * t);
        let at = |from: f64, to: f64| from + (to - from) * t;
        CameraOptions {
            center: Some(
                LatLng::new(center.latitude() + at(from.north, to.north), center.longitude() + at(from.east, to.east))
                    .expect("a center"),
            ),
            zoom: Some(at(from.zoom, to.zoom)),
            bearing: Some(at(from.bearing, to.bearing)),
            pitch: Some(at(from.pitch, to.pitch)),
            ..Default::default()
        }
    };

    let frontend = Frontend::new(VIEW);
    bench.gpu = Some(frontend.gpu());
    let latency = Latency::new(fixtures.file_source(frontend.pool()));
    let mut map = HeadlessMap::new(frontend, MapMode::Continuous, Arc::<Latency>::clone(&latency));
    map.jump(&camera(0));
    map.map().load_style_json(&fixtures.style(Tiles::Street));
    map.settle();

    latency.set_delay(LATENCY);
    let incomplete = map.frames_incomplete();
    let requests = latency.tile_requests();
    for frame in 1..=FRAMES {
        map.jump(&camera(frame));
        bench.measure(|| {
            latency.tick();
            map.wait_for_workers();
            map.draw_frame()
        });
    }
    bench.per_part("frame");
    bench.count("frames_incomplete", map.frames_incomplete() - incomplete);
    bench.count("tile_requests", latency.tile_requests() - requests);
    bench.bytes("gpu_buffers", map.gpu_buffer_bytes());
    // The image shows where the flight ends, once everything has arrived.
    if let Some(path) = &bench.image {
        latency.set_delay(0);
        map.settle();
        map.save_image(path);
    }
}
