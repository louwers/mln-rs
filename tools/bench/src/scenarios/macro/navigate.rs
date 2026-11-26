//! Camera flights across zooms, as when a user flies to a place and looks
//! around it, with the tiles they need arriving over a network while they
//! fly (`support::network`).

use std::sync::Arc;

use mln_engine::camera::CameraOptions;
use mln_style::geo::LatLng;

use super::VIEW;
use crate::scenario::{Bench, Kind, Scenario};
use crate::support::fixtures::{Fixtures, Tiles};
use crate::support::map::{Drapes, Frontend};
use crate::support::network::{CDN, FLAKY, LOCAL, MOBILE_4G, Network, Profile, SLOW, Serve, Server};
use crate::support::online::OnlineMap;

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
        name: "macro/navigate/fly-4g",
        kind: Kind::Macro,
        question: "How much of the view does a flight from one city to another leave blank over a mobile link, and how many bytes go to tiles it passes over?",
        run: fly_4g,
    },
    Scenario {
        name: "macro/navigate/street-flaky",
        kind: Kind::Macro,
        question: "How soon does the view of a city's streets come back complete after the network drops out during a flight?",
        run: street_flaky,
    },
    Scenario {
        name: "macro/navigate/street-pmtiles-flaky",
        kind: Kind::Macro,
        question: "How soon does the view of a city's streets, read from a PMTiles archive by byte ranges, come back complete after the network drops out during a flight?",
        run: street_pmtiles_flaky,
    },
    Scenario {
        name: "macro/navigate/zoom-slow",
        kind: Kind::Macro,
        question: "Does zooming out to the world and back in over a slow link fetch again the street tiles the map just let go of?",
        run: zoom_slow,
    },
    Scenario {
        name: "macro/navigate/terrain",
        kind: Kind::Macro,
        question: "What does a flight across a city's hills in 3D cost, low and tilted, with elevation and street tiles arriving while it flies?",
        run: terrain,
    },
];

/// How many frames a flight that waits for the view to complete may draw
/// after the camera stops: 30 seconds.
const REST_LIMIT: u64 = 1_800;

/// Whether a flight measures the frames after the camera stops, until the
/// view is complete.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Rest {
    Skip,
    UntilComplete,
}

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
/// into its streets at zoom 15.5 and back out to zoom 12 cost
/// ([`street_keys`]), with tiles coming from a nearby CDN?
fn street(bench: &mut Bench) {
    let fixtures = Fixtures::open(bench);
    let server = Server::new(&fixtures, &fixtures.style(Tiles::Street), Serve::Tiles);
    flight(bench, server, &street_keys(&fixtures), CDN, Rest::Skip);
}

/// The question: how soon does the view come back complete after the
/// network drops out? The flight of `macro/navigate/street` into the
/// streets, stopping there, tilted at zoom 15.5 after 160 frames, over a
/// 4G link that drops: every transfer fails for 3 seconds from a second
/// into the flight, as the street tiles start to load, and 2% of the others
/// do. Its counts run until the view is complete after the camera stops, so
/// `frames_to_complete` counts the frames from the end of the flight, which
/// the outage reaches 1.3 seconds past, until the tiles that failed in it
/// have come back.
fn street_flaky(bench: &mut Bench) {
    let fixtures = Fixtures::open(bench);
    let keys = street_keys(&fixtures);
    let server = Server::new(&fixtures, &fixtures.style(Tiles::Street), Serve::Tiles);
    flight(bench, server, &keys[..3], FLAKY, Rest::UntilComplete);
}

/// The question of `macro/navigate/street-flaky`, the tiles read from a
/// PMTiles archive on the server: each tile's range, and the directories
/// before it, fail in the outage and come back through the archive's
/// requests.
fn street_pmtiles_flaky(bench: &mut Bench) {
    let fixtures = Fixtures::open(bench);
    let keys = street_keys(&fixtures);
    let server = Server::new(&fixtures, &fixtures.style(Tiles::Street), Serve::Archive);
    flight(bench, server, &keys[..3], FLAKY, Rest::UntilComplete);
}

/// The question: what does a flight across San Francisco's hills in 3D
/// cost, as a user who tilts the map to look across them does? Four
/// seconds at 60 Hz over
/// [`Snapshot::terrain_3d_style`](mln_bench_workload::Snapshot::terrain_3d_style),
/// with tiles from a nearby CDN: from Ocean Beach at zoom 12.5, tilted to
/// 45 degrees, east up the Sunset to Twin Peaks at zoom 14.5 and 60
/// degrees, over them toward downtown, and back out to zoom 13. Each leg
/// needs elevation and street tiles that the one before didn't, the camera
/// stands on the ground the elevation tiles give it, and its zoom or
/// bearing changes in every frame, which draws drapes again. It also
/// reports what the
/// frames drew of their drapes, and the most they held ([`Drapes`]).
fn terrain(bench: &mut Bench) {
    let fixtures = Fixtures::open(bench);
    let keys = [
        Key { frame: 0, center: (37.757, -122.502), zoom: 12.5, bearing: 70.0, pitch: 45.0 },
        Key { frame: 100, center: (37.753, -122.458), zoom: 14.5, bearing: 80.0, pitch: 60.0 },
        Key { frame: 160, center: (37.762, -122.432), zoom: 14.5, bearing: 50.0, pitch: 60.0 },
        Key { frame: 240, center: (37.765, -122.445), zoom: 13.0, bearing: 20.0, pitch: 40.0 },
    ];
    let server = Server::new(&fixtures, &fixtures.terrain_3d_style(), Serve::Tiles);
    flight(bench, server, &keys, CDN, Rest::Skip);
}

/// The flight of `macro/navigate/street`: from a view of San Francisco at
/// zoom 11 into its streets at zoom 15.5 and back out to zoom 12, four
/// seconds at 60 Hz, tilted along the streets. Every center stays inside
/// the street tiles' area, so the camera only sees tiles of the set.
fn street_keys(fixtures: &Fixtures) -> [Key; 4] {
    let center = fixtures.area_center(Tiles::Street);
    let (lat, lon) = (center.latitude(), center.longitude());
    [
        Key::at(0, (lat - 0.01, lon - 0.03), 11.0),
        Key { frame: 100, center: (lat + 0.012, lon + 0.015), zoom: 15.0, bearing: 20.0, pitch: 30.0 },
        Key { frame: 160, center: (lat + 0.018, lon + 0.035), zoom: 15.5, bearing: 40.0, pitch: 30.0 },
        Key::at(240, (lat, lon), 12.0),
    ]
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
    let server = Server::new(&fixtures, &fixtures.style(Tiles::Flight), Serve::Tiles);
    flight(bench, server, &fly_keys(&fixtures), CDN, Rest::Skip);
}

/// The question: how much of the view does the flight of
/// `macro/navigate/fly` leave blank over a 4G link, and what does it spend
/// on tiles it passes over? `bytes_wasted` counts the bytes of transfers
/// that the map cancelled once the camera had passed their tiles. Its
/// counts end with the flight: Tokyo's labels need about 30 MiB of glyphs,
/// half a minute of the link, which is a question of its own.
fn fly_4g(bench: &mut Bench) {
    let fixtures = Fixtures::open(bench);
    let server = Server::new(&fixtures, &fixtures.style(Tiles::Flight), Serve::Tiles);
    flight(bench, server, &fly_keys(&fixtures), MOBILE_4G, Rest::Skip);
}

/// The flight of `macro/navigate/fly`: four seconds at 60 Hz, zooming out
/// over San Francisco from 14 to 3, west across the Pacific and the
/// antimeridian at zoom 3, and into Tokyo to 14.
fn fly_keys(fixtures: &Fixtures) -> [Key; 4] {
    let from = fixtures.area_center(Tiles::Street);
    let to = fixtures.area_center(Tiles::Cjk);
    let (from, to) = ((from.latitude(), from.longitude()), (to.latitude(), to.longitude() - 360.0));
    [Key::at(0, from, 14.0), Key::at(80, from, 3.0), Key::at(160, to, 3.0), Key::at(240, to, 14.0)]
}

/// The question: what does zooming out from San Francisco's streets at
/// zoom 15 to the world at zoom 2, and back in to the same view, cost? Two
/// seconds at 60 Hz, a second each way: the map lets go of the street
/// tiles on the way out and needs them again on the way back.
fn zoom(bench: &mut Bench) {
    let fixtures = Fixtures::open(bench);
    let server = Server::new(&fixtures, &fixtures.style(Tiles::Street), Serve::Tiles);
    flight(bench, server, &zoom_keys(&fixtures), CDN, Rest::Skip);
}

/// The question: does the map fetch again what it just let go of? The zoom
/// of `macro/navigate/zoom` over a slow link, whose counts run until the
/// street view is complete again after it: `refetches` counts the
/// transfers of tiles that an earlier transfer had brought.
fn zoom_slow(bench: &mut Bench) {
    let fixtures = Fixtures::open(bench);
    let server = Server::new(&fixtures, &fixtures.style(Tiles::Street), Serve::Tiles);
    flight(bench, server, &zoom_keys(&fixtures), SLOW, Rest::UntilComplete);
}

/// The zoom of `macro/navigate/zoom`: from San Francisco's streets at zoom
/// 15 to the world at zoom 2 and back, a second each way.
fn zoom_keys(fixtures: &Fixtures) -> [Key; 3] {
    let center = fixtures.area_center(Tiles::Street);
    let center = (center.latitude(), center.longitude());
    [Key::at(0, center, 15.0), Key::at(60, center, 2.0), Key::at(120, center, 15.0)]
}

/// Flies through `keys` over `server`'s style, every resource coming from
/// the server over `profile`, and measures each frame: the answers that came since the frame before, laying out what they brought,
/// its update, symbol placement, draw calls, the GPU finishing it and the
/// transfers it starts. Each leg between keys eases in and out. With
/// [`Rest::UntilComplete`], the frames after the camera stops count too,
/// until one shows the view complete, or [`REST_LIMIT`] of them.
///
/// The map starts settled at the first key, over a link that takes no
/// time and has no faults, which then changes to `profile`. Its frames then
/// show what has arrived, and parent or child tiles where a tile is
/// missing, as a map does over a network; transfers take a time that only
/// frames advance, so every run draws the same frames with the same tiles.
fn flight(bench: &mut Bench, server: Arc<Server>, keys: &[Key], profile: Profile, rest: Rest) {
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
    let network = Network::new(server, LOCAL);
    let mut online = OnlineMap::new(frontend, network, None, &camera(0));
    online.settle();

    online.network().set_profile(profile);
    online.network().begin_faults();
    let incomplete = online.map().frames_incomplete();
    let counts = online.network().counts();
    let draw_calls = online.map().draw_calls();
    let mut drapes = Drapes::default();
    for frame in 1..=frames {
        online.map().jump(&camera(frame));
        online.frame(bench);
        drapes.frame(online.map());
    }
    if rest == Rest::UntilComplete {
        online.sight().rest();
        for _ in 0..REST_LIMIT {
            if online.sight().is_complete() {
                break;
            }
            online.frame(bench);
            drapes.frame(online.map());
        }
    }
    bench.per_part("frame");
    bench.count("draw_calls", online.map().draw_calls() - draw_calls);
    drapes.declare(bench, online.map());
    bench.count("frames_incomplete", online.map().frames_incomplete() - incomplete);
    online.sight().declare(bench, false, rest == Rest::UntilComplete);
    online.network().counts().declare(bench, counts);
    bench.bytes("gpu_buffers", online.map().gpu_buffer_bytes());
    // The image shows where the flight ends, once everything has arrived.
    if let Some(path) = &bench.image {
        online.network().set_profile(LOCAL);
        online.settle();
        online.map().save_image(path);
    }
}
