//! A map shown from nothing: the map, its renderer and its workers start,
//! the style loads, and the tiles, glyphs and sprite it needs load, until
//! the first frame with nothing missing: from the fixtures, or from a
//! server over a simulated network (`support::network`).

use std::sync::Arc;

use mln_map::{CameraOptions, CameraUpdate, MapMode};
use mln_runtime::{AttachOptions, MapOptions, RenderTarget, Runtime, RuntimeOptions};
use mln_style::geo::LatLng;
use mln_style::size::Size;

use super::VIEW;
use crate::meter;
use crate::scenario::{Bench, Kind, Scenario};
use crate::support::cache::DiskCache;
use crate::support::fixtures::{Fixtures, Tiles};
use crate::support::map::{Frontend, HeadlessMap, save_png};
use crate::support::network::{Counts, LOCAL, MOBILE_4G, Network, Profile, SLOW, Serve, Server};
use crate::support::online::OnlineMap;

pub(crate) const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "macro/load/street",
        kind: Kind::Macro,
        question: "What does showing a city's streets from nothing take, until the first complete frame?",
        run: street,
    },
    Scenario {
        name: "macro/load/globe",
        kind: Kind::Macro,
        question: "What does showing the whole world on the globe from nothing take, until the first complete frame?",
        run: globe,
    },
    Scenario {
        name: "macro/load/cjk",
        kind: Kind::Macro,
        question: "What does showing a city labelled in Japanese script from nothing take, until the first complete frame?",
        run: cjk,
    },
    Scenario {
        name: "macro/load/street-4g",
        kind: Kind::Macro,
        question: "How long does a city's streets take to show complete from nothing over a mobile link, and how much of the view is blank meanwhile?",
        run: street_4g,
    },
    Scenario {
        name: "macro/load/street-z16-4g",
        kind: Kind::Macro,
        question: "How long does a city's streets take to show complete from nothing over a mobile link, closer in than its tileset's last zoom?",
        run: street_z16_4g,
    },
    Scenario {
        name: "macro/load/street-pmtiles-4g",
        kind: Kind::Macro,
        question: "How long does a city's streets take to show complete from nothing over a mobile link, read from a PMTiles archive by byte ranges?",
        run: street_pmtiles_4g,
    },
    Scenario {
        name: "macro/load/street-slow",
        kind: Kind::Macro,
        question: "How long does a city's streets take to show complete from nothing over a slow link, and how much of the view is blank meanwhile?",
        run: street_slow,
    },
    Scenario {
        name: "macro/load/street-warm",
        kind: Kind::Macro,
        question: "What does a disk cache that holds a city's streets save a map that shows them again over a mobile link?",
        run: street_warm,
    },
    Scenario {
        name: "macro/load/street-expired",
        kind: Kind::Macro,
        question: "What does revalidating a disk cache of a city's streets that has expired cost a map that shows them again over a mobile link?",
        run: street_expired,
    },
    Scenario {
        name: "macro/load/runtime",
        kind: Kind::Macro,
        question: "What does showing a city's streets take through the runtime, from its start until a still image?",
        run: runtime,
    },
];

/// The question: what does showing central San Francisco at street level
/// (zoom 14) from nothing take? One measured unit is one load: from making
/// the map until the first complete frame.
fn street(bench: &mut Bench) {
    let fixtures = Fixtures::open(bench);
    let style = fixtures.style(Tiles::Street);
    load(bench, &fixtures, &style, &super::street_center(&fixtures, 14.0));
}

/// The question: what does showing the whole world on the globe (zoom 1.5,
/// tiles of zooms 0 and 1) from nothing take? One measured unit is one load.
fn globe(bench: &mut Bench) {
    let fixtures = Fixtures::open(bench);
    let style = fixtures.globe_style();
    let camera = CameraOptions {
        center: Some(LatLng::new(30.0, 10.0).expect("a center")),
        zoom: Some(1.5),
        ..Default::default()
    };
    load(bench, &fixtures, &style, &camera);
}

/// The question: what does showing central Tokyo (zoom 14), whose labels
/// are in Japanese script, from nothing take: its many glyph ranges to load
/// and its shaping included? One measured unit is one load.
fn cjk(bench: &mut Bench) {
    let fixtures = Fixtures::open(bench);
    let style = fixtures.style(Tiles::Cjk);
    let camera =
        CameraOptions { center: Some(fixtures.area_center(Tiles::Cjk)), zoom: Some(14.0), ..Default::default() };
    load(bench, &fixtures, &style, &camera);
}

/// Measures a map's start, over the fixtures, at `camera`, with `style`,
/// until its first complete frame. The GPU device and its target are made
/// before, and left out.
fn load(bench: &mut Bench, fixtures: &Fixtures, style: &str, camera: &CameraOptions) {
    let frontend = Frontend::new(VIEW);
    bench.gpu = Some(frontend.gpu());
    let (map, frames) = bench.measure(|| {
        let file_source = fixtures.file_source(frontend.pool());
        let mut map = HeadlessMap::new(frontend, MapMode::Continuous, file_source);
        map.jump(camera);
        map.map().load_style_json(style);
        let frames = map.load();
        (map, frames)
    });
    bench.count("frames", frames);
    bench.bytes("gpu_buffers", map.gpu_buffer_bytes());
    bench.count("gpu_objects", map.gpu_objects_created());
    // The first complete frame shows its symbols faded out: the image shows
    // them once they have faded in.
    if let Some(path) = &bench.image {
        let mut map = map;
        map.settle();
        map.save_image(path);
    }
}

/// What a map's disk cache holds when it starts.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cached {
    /// Nothing: the map has no cache.
    Nothing,
    /// What an earlier visit stored, still fresh: nothing goes to the
    /// network.
    Fresh,
    /// What an earlier visit stored, expired since: the map shows it at
    /// once and revalidates it, which the server answers with 304s.
    Expired,
}

/// The question: how long does central San Francisco at street level
/// (zoom 14) take to show complete from nothing over a 4G link, its style,
/// sprite, glyphs and tiles coming from a server?
fn street_4g(bench: &mut Bench) {
    load_online(bench, MOBILE_4G, Cached::Nothing);
}

/// The question of `macro/load/street-4g` at zoom 16, past the tileset's
/// last zoom, 14: each tile the view shows is cut from one there, which
/// the tiles cut from it all need at once.
fn street_z16_4g(bench: &mut Bench) {
    load_online_from(bench, 16.0, MOBILE_4G, Cached::Nothing, Serve::Tiles);
}

/// The question of `macro/load/street-4g`, the tiles read from a PMTiles
/// archive on the server: the map reads its header and directories before
/// each tile's range, through the disk cache's place in the loader.
fn street_pmtiles_4g(bench: &mut Bench) {
    load_online_from(bench, 14.0, MOBILE_4G, Cached::Nothing, Serve::Archive);
}

/// The question of `macro/load/street-4g` over a slow link.
fn street_slow(bench: &mut Bench) {
    load_online(bench, SLOW, Cached::Nothing);
}

/// The question: what does a disk cache save? `macro/load/street-4g` with
/// a cache that an earlier visit to the view filled, still fresh.
fn street_warm(bench: &mut Bench) {
    load_online(bench, MOBILE_4G, Cached::Fresh);
}

/// The question: what does revalidating a cache that has expired cost?
/// `macro/load/street-warm` with the cache's entries expired: the map
/// shows them while it revalidates each over the 4G link.
fn street_expired(bench: &mut Bench) {
    load_online(bench, MOBILE_4G, Cached::Expired);
}

/// How many frames a load over the network may take: 60 seconds.
const ONLINE_FRAME_LIMIT: u64 = 3_600;

/// Measures a map's start over the simulated network on `profile`, at San
/// Francisco's streets (zoom 14), with what `cached` says in its disk
/// cache, each frame a part, until the first complete frame and then until
/// no transfer is in flight, such as a revalidation of what the cache
/// showed. The GPU device and its target, and filling the cache, are left
/// out.
fn load_online(bench: &mut Bench, profile: Profile, cached: Cached) {
    load_online_from(bench, 14.0, profile, cached, Serve::Tiles);
}

/// [`load_online`] at `zoom`, the server serving the tiles as `serve` says.
fn load_online_from(bench: &mut Bench, zoom: f64, profile: Profile, cached: Cached, serve: Serve) {
    let fixtures = Fixtures::open(bench);
    let camera = super::street_center(&fixtures, zoom);
    let server = Server::new(&fixtures, &fixtures.style(Tiles::Street), serve);
    let cache = (cached != Cached::Nothing).then(|| {
        // An earlier visit's, over a link that takes no time, on a map of
        // its own.
        let cache = DiskCache::new();
        server.set_expired(cached == Cached::Expired);
        let network = Network::new(Arc::clone(&server), LOCAL);
        OnlineMap::new(Frontend::new(VIEW), network, Some(Arc::clone(&cache)), &camera).settle();
        server.set_expired(false);
        cache
    });

    let frontend = Frontend::new(VIEW);
    bench.gpu = Some(frontend.gpu());
    let network = Network::new(server, profile);
    let mut online = OnlineMap::new(frontend, network, cache, &camera);
    loop {
        online.frame(bench);
        if online.sight().is_complete() && online.network().is_idle() {
            break;
        }
        assert!(online.map().frames() < ONLINE_FRAME_LIMIT, "the load isn't done after {ONLINE_FRAME_LIMIT} frames");
    }
    bench.per_part("frame");
    online.sight().declare(bench, true, true);
    online.network().counts().declare(bench, Counts::default());
    if let Some(path) = &bench.image {
        online.settle();
        online.map().save_image(path);
    }
}

/// The question: what does showing central San Francisco at street level
/// (zoom 14) take through the runtime, as an application or the C API
/// drives it? One measured unit is one load: the runtime's thread and
/// workers start, a static map is made on it, its style and camera are
/// set, an offscreen render session attaches to it on a thread of its own,
/// and the map renders a still image once it has loaded everything. The GPU
/// device is made before, and left out.
///
/// The other load scenarios drive the map on the scenario's thread, a step
/// at a time, so they do the same work every run. Here the runtime's
/// threads, workers and render session take their own turns, so how its
/// work interleaves, and with it the counts, varies a little from run to
/// run. A static map renders one frame, once nothing is missing, so the
/// frames drawn don't vary.
fn runtime(bench: &mut Bench) {
    let fixtures = Fixtures::open(bench);
    let style = fixtures.style(Tiles::Street);
    let camera = super::street_center(&fixtures, 14.0);
    // The image is done when the runtime's thread has drawn it, before the
    // GPU has, and nothing waits for the GPU inside the part.
    meter::leave_gpu_untimed();
    let device = {
        let frontend = Frontend::new(Size::new(1, 1));
        bench.gpu = Some(frontend.gpu());
        frontend.device()
    };
    let options =
        RuntimeOptions { asset_path: Some(fixtures.dir().to_owned()), cache_path: None, network_status: None };
    let map_options = MapOptions {
        map: mln_map::MapOptions { size: VIEW, pixel_ratio: 1.0, map_mode: MapMode::Static, ..Default::default() },
        // The fonts of the machine would make the work differ between
        // machines: the snapshot's glyphs have every label's.
        system_fonts: false,
        ..Default::default()
    };
    let target = RenderTarget::Offscreen { size: VIEW, device: Some(device) };
    let (runtime, map, session) = bench.measure(move || {
        let runtime = Runtime::new(&options).expect("a runtime");
        let (map, created) = runtime.create_map(map_options).expect("a map");
        created.wait().expect("the map");
        // Sent without waiting, as an application sends them: the still
        // image runs after them and fails if they do.
        drop(map.update_camera(CameraUpdate { camera, ..CameraUpdate::default() }).expect("a camera"));
        drop(map.set_style_json(style).expect("the style"));
        let (session, attached) = map.attach(target, AttachOptions::default()).expect("a render session");
        attached.wait().expect("the session attaches");
        map.request_still_image().expect("a still image").wait().expect("the still image");
        (runtime, map, session)
    });
    if let Some(path) = &bench.image {
        save_png(&session.read_texture().expect("the image").wait().expect("the image"), path);
    }
    // The runtime's threads end before the process does.
    session.detach().expect("a detach").wait().expect("the session detaches");
    drop((session, map));
    runtime.release().expect("a release").wait().expect("the runtime ends");
}
