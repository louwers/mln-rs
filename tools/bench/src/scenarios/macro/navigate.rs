//! Camera flights across zooms, as when a user flies to a place and looks
//! around it, with the tiles they need arriving while they fly.

use std::sync::Arc;

use mln_map::{BoundOptions, CameraOptions, MapMode};
use mln_util::geo::LatLng;

use super::VIEW;
use crate::scenario::{Bench, Kind, Scenario};
use crate::support::fixtures::{Fixtures, Tiles};
use crate::support::latency::Latency;
use crate::support::map::{DrawMeter, Frontend, HeadlessMap, gesture_phase};

pub(crate) const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "macro/navigate/street",
        kind: Kind::Macro,
        question: "What does a flight into a city's streets and back out cost, with tiles arriving while it flies?",
        run: street,
    },
    Scenario {
        name: "macro/navigate/fly",
        kind: Kind::Macro,
        question: "What does a flight from one city's streets, out over an ocean and into another city's streets, cost, with tiles arriving while it flies?",
        run: fly,
    },
    Scenario {
        name: "macro/navigate/zoom",
        kind: Kind::Macro,
        question: "What does zooming from a city's streets out to the world and back in cost, with tiles arriving while it zooms?",
        run: zoom,
    },
    Scenario {
        name: "macro/navigate/terrain-3d",
        kind: Kind::Macro,
        question: "What does a flight over 3D terrain into ground it hasn't loaded cost: pan, zoom and rotate at pitch 60 to 75?",
        run: terrain_3d,
    },
];

/// The steepest pitch a map takes by default, in degrees.
const DEFAULT_MAX_PITCH: f64 = 60.0;

/// The frames between a request and its answer: 100 ms at 60 Hz, about
/// what a tile takes to come from a nearby CDN.
const LATENCY: u64 = 6;

/// A point of a flight: at frame `frame`, the camera is over `center`
/// (latitude, longitude; a longitude past 180 degrees goes on around the
/// world), at `zoom`, `bearing` and `pitch`.
struct Key {
    frame: usize,
    center: (f64, f64),
    zoom: f64,
    bearing: f64,
    pitch: f64,
}

impl Key {
    const fn at(frame: usize, center: (f64, f64), zoom: f64) -> Key {
        Key { frame, center, zoom, bearing: 0.0, pitch: 0.0 }
    }
}

/// The question: what does flying from a view of San Francisco at zoom 11
/// into its streets at zoom 15.5 and back out to zoom 12 cost? Four seconds
/// at 60 Hz: from an overview of the city into its streets, along them,
/// tilted, and back out. Every center stays inside the street tiles' area,
/// so the camera only sees tiles of the set.
fn street(bench: &mut Bench) {
    let fixtures = Fixtures::open(bench);
    let center = fixtures.area_center(Tiles::Street);
    let (lat, lon) = (center.latitude(), center.longitude());
    let keys = [
        Key::at(0, (lat - 0.01, lon - 0.03), 11.0),
        Key { frame: 100, center: (lat + 0.012, lon + 0.015), zoom: 15.0, bearing: 20.0, pitch: 30.0 },
        Key { frame: 160, center: (lat + 0.018, lon + 0.035), zoom: 15.5, bearing: 40.0, pitch: 30.0 },
        Key::at(240, (lat, lon), 12.0),
    ];
    flight(bench, &fixtures, &fixtures.style(Tiles::Street), &keys, None);
}

/// The question: what does flying from San Francisco's streets to Tokyo's
/// cost, as a user who looks up another city does? Four seconds at 60 Hz:
/// the camera zooms out over San Francisco from 14 to 3, pans west across
/// the Pacific and the antimeridian at zoom 3, and zooms into Tokyo to 14.
/// The tiles of every zoom of the way come and go, and Tokyo's labels need
/// their Japanese glyphs. Over the flight tiles, which hold each city's
/// tiles to zoom 14 and the world's to 4, so that every view of the way
/// shows only tiles of the set.
fn fly(bench: &mut Bench) {
    let fixtures = Fixtures::open(bench);
    let from = fixtures.area_center(Tiles::Street);
    let to = fixtures.area_center(Tiles::Cjk);
    let (from, to) = ((from.latitude(), from.longitude()), (to.latitude(), to.longitude() - 360.0));
    let keys = [Key::at(0, from, 14.0), Key::at(80, from, 3.0), Key::at(160, to, 3.0), Key::at(240, to, 14.0)];
    flight(bench, &fixtures, &fixtures.style(Tiles::Flight), &keys, None);
}

/// The question: what does zooming out from San Francisco's streets at
/// zoom 15 to the world at zoom 2, and back in to the same view, cost? Two
/// seconds at 60 Hz, a second each way: the map lets go of the street
/// tiles on the way out and needs them again on the way back.
fn zoom(bench: &mut Bench) {
    let fixtures = Fixtures::open(bench);
    let center = fixtures.area_center(Tiles::Street);
    let center = (center.latitude(), center.longitude());
    let keys = [Key::at(0, center, 15.0), Key::at(60, center, 2.0), Key::at(120, center, 15.0)];
    flight(bench, &fixtures, &fixtures.style(Tiles::Street), &keys, None);
}

/// The question: what does a flight over San Francisco's hills in 3D cost,
/// into ground whose tiles haven't loaded, as a user who tilts the map to
/// look across the hills does? Four seconds at 60 Hz over
/// [`Snapshot::terrain_3d_style`](mln_bench_workload::Snapshot::terrain_3d_style),
/// tilted from 60 to 75 degrees all the way: from Ocean Beach at zoom 12 in
/// to Twin Peaks at 14.5 as the camera turns, across to downtown, and back
/// out to zoom 13 over the city, turning half a turn in all. Each leg needs
/// terrain, DEM and street tiles that the one before didn't, and the drapes
/// of the tiles that arrive draw as they do. The camera moves as one
/// gesture moves it, which keeps the drapes it drew at another zoom or
/// bearing, and the last frame ends the gesture, which draws them again at
/// the zoom and bearing it ends at. It also reports the frames'
/// render passes and draw calls, the drapes they drew, and the most the
/// terrain held ([`DrawMeter`]).
fn terrain_3d(bench: &mut Bench) {
    let fixtures = Fixtures::open(bench);
    let keys = [
        Key { frame: 0, center: (37.755, -122.495), zoom: 12.0, bearing: 0.0, pitch: 60.0 },
        Key { frame: 80, center: (37.752, -122.447), zoom: 14.5, bearing: 60.0, pitch: 70.0 },
        Key { frame: 160, center: (37.788, -122.405), zoom: 14.5, bearing: 120.0, pitch: 75.0 },
        Key { frame: 240, center: (37.77, -122.43), zoom: 13.0, bearing: 180.0, pitch: 65.0 },
    ];
    flight(bench, &fixtures, &fixtures.terrain_3d_style(), &keys, Some(DrawMeter::default()));
}

/// Flies through `keys` over `style`, such as Liberty, as each tile, glyph range
/// and image the map asks for arrives [`LATENCY`] frames after it asked,
/// and measures each frame: sending on the answers due at it, laying out
/// what they brought, its update, symbol placement, draw calls and the GPU
/// finishing it. Each leg between keys eases in and out.
///
/// The map starts settled at the first key. Its frames then show what has
/// arrived, and parent or child tiles where a tile is missing, as a map does
/// over a network; tiles arrive on a schedule that only frames advance, so
/// every run draws the same frames with the same tiles. The map's pitch
/// may reach the steepest of the keys'. With a `meter`, the camera moves as
/// one gesture's, and it also reports what the frames drew.
fn flight(bench: &mut Bench, fixtures: &Fixtures, style: &str, keys: &[Key], mut meter: Option<DrawMeter>) {
    let camera = |frame: usize| {
        let leg = keys.windows(2).find(|leg| frame <= leg[1].frame).expect("a frame of the flight");
        let (from, to) = (&leg[0], &leg[1]);
        let t = (frame - from.frame) as f64 / (to.frame - from.frame) as f64;
        // Smoothstep: the leg starts and ends at rest.
        let t = t * t * (3.0 - 2.0 * t);
        let at = |from: f64, to: f64| from + (to - from) * t;
        let longitude = (at(from.center.1, to.center.1) + 540.0).rem_euclid(360.0) - 180.0;
        CameraOptions {
            center: Some(LatLng::new(at(from.center.0, to.center.0), longitude).expect("a center")),
            zoom: Some(at(from.zoom, to.zoom)),
            bearing: Some(at(from.bearing, to.bearing)),
            pitch: Some(at(from.pitch, to.pitch)),
            ..Default::default()
        }
    };
    let frames = keys.last().expect("a flight's keys").frame;
    let frontend = Frontend::new(VIEW);
    bench.gpu = Some(frontend.gpu());
    let latency = Latency::new(fixtures.file_source(frontend.pool()));
    let mut map = HeadlessMap::new(frontend, MapMode::Continuous, Arc::<Latency>::clone(&latency));
    let max_pitch = keys.iter().map(|key| key.pitch).fold(DEFAULT_MAX_PITCH, f64::max);
    if max_pitch > DEFAULT_MAX_PITCH {
        let bounds = BoundOptions { max_pitch: Some(max_pitch), ..Default::default() };
        map.map().set_bounds(&bounds).expect("a pitch the map can take");
    }
    map.jump(&camera(0));
    map.map().load_style_json(style);
    map.settle();

    latency.set_delay(LATENCY);
    let incomplete = map.frames_incomplete();
    let requests = latency.tile_requests();
    for frame in 1..=frames {
        match meter {
            Some(_) => map.gesture(&camera(frame), gesture_phase(frame - 1, frames)),
            None => map.jump(&camera(frame)),
        }
        bench.measure(|| {
            latency.tick();
            map.wait_for_workers();
            map.draw_frame()
        });
        if let Some(meter) = &mut meter {
            meter.frame(&map);
        }
    }
    if let Some(meter) = &meter {
        meter.report(bench);
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
