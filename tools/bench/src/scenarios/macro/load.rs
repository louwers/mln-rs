//! A map shown from nothing: the map, its renderer and its workers start,
//! the style loads, and the tiles, glyphs and sprite it needs load, until
//! the first frame with nothing missing.

use mln_map::{CameraOptions, MapMode};
use mln_util::geo::LatLng;

use super::VIEW;
use crate::scenario::{Bench, Kind, Scenario};
use crate::support::fixtures::{Fixtures, Tiles};
use crate::support::map::{Frontend, HeadlessMap};

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
    let style = super::globe_style(&fixtures);
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
    let camera = CameraOptions {
        center: Some(super::area_center(&fixtures, Tiles::Cjk)),
        zoom: Some(14.0),
        ..Default::default()
    };
    load(bench, &fixtures, &style, &camera);
}

/// Measures a map's start, over the fixtures, at `camera`, with `style`,
/// until its first complete frame. The GPU device and its target are made
/// before, and left out.
fn load(bench: &mut Bench, fixtures: &Fixtures, style: &str, camera: &CameraOptions) {
    let frontend = Frontend::new(VIEW);
    bench.gpu = Some(frontend.gpu());
    let (map, frames) = bench.measure(|| {
        let file_source = fixtures.file_source(frontend.workers());
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
