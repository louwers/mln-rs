//! A map shown from nothing: the map, its renderer and its workers start,
//! the style loads, and the tiles, glyphs and sprite it needs load, until
//! the first frame with nothing missing.

use mln_map::{CameraOptions, CameraUpdate, MapMode};
use mln_runtime::{AttachOptions, MapOptions, RenderTarget, Runtime, RuntimeOptions};
use mln_util::geo::LatLng;
use mln_util::size::Size;

use super::VIEW;
use crate::scenario::{Bench, Kind, Scenario};
use crate::support::fixtures::{Fixtures, Tiles};
use crate::support::map::{Frontend, HeadlessMap, save_png};

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
