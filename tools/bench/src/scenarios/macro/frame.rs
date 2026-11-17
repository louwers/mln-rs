//! Frames of a loaded map as its camera pans, turns and tilts, as during a
//! gesture or an animation over tiles already there.

use std::time::Instant;

use mln_bench_workload::paths::{FRAMES, Path};
use mln_map::{CameraOptions, MapMode};

use super::VIEW;
use crate::scenario::{Bench, Kind, Scenario};
use crate::support::fixtures::Fixtures;
use crate::support::map::HeadlessMap;

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

/// The question: what does a frame of western San Francisco at zoom 13.5
/// cost while the camera turns, pans and tilts over loaded tiles
/// ([`Path::Street`])? One measured unit is one frame: its update, symbol
/// placement, draw calls and the GPU finishing it.
fn street(bench: &mut Bench) {
    path(bench, Path::Street);
}

/// The question: what does a frame of the whole globe cost while it turns
/// and tilts over loaded tiles ([`Path::Globe`])? One measured unit is one
/// frame.
fn globe(bench: &mut Bench) {
    path(bench, Path::Globe);
}

fn path(bench: &mut Bench, path: Path) {
    let fixtures = Fixtures::open(bench);
    let camera = path.camera(&fixtures);
    let style = path.style(&fixtures);
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
    let tasks = map.background_work();
    let mut times = Vec::with_capacity(FRAMES);
    for frame in 0..FRAMES {
        map.jump(&camera(frame));
        let start = Instant::now();
        bench.measure(|| map.draw_frame());
        times.push(start.elapsed());
    }
    assert_eq!(map.background_work(), tasks, "the path needs tiles it didn't start with");
    assert_eq!(map.frames() - drawn, FRAMES as u64, "one frame per camera");
    bench.per("frame", FRAMES as u64);
    bench.bytes("gpu_buffers", map.gpu_buffer_bytes());
    bench.count("gpu_objects", map.gpu_objects_created() - objects);
    bench.percentiles("frame", &times);
    if let Some(path) = &bench.image {
        map.save_image(path);
    }
}
