//! Frames of a loaded map as its camera pans, turns and tilts, as during a
//! gesture or an animation over tiles already there.

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
    Scenario {
        name: "macro/frame/antimeridian",
        kind: Kind::Macro,
        question: "What does a frame of the world on a flat map cost while the view crosses the antimeridian over loaded tiles?",
        run: antimeridian,
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

/// The question: what does a frame of the world on a flat map cost while
/// its center swings across the antimeridian and back
/// ([`Path::Antimeridian`]), each crossing moving the tiles in view to other
/// world copies? One measured unit is one frame.
fn antimeridian(bench: &mut Bench) {
    path(bench, Path::Antimeridian);
}

fn path(bench: &mut Bench, path: Path) {
    let fixtures = Fixtures::open(bench);
    let camera = path.camera(&fixtures);
    let style = path.style(&fixtures);
    let map = HeadlessMap::open(bench, &fixtures, VIEW, MapMode::Continuous, &style, &camera(0));
    frames(bench, map, camera);
}

/// Loads the path, drawing it once with its tiles loaded before each
/// frame, then measures one frame at each of its cameras: the frame, and
/// the background work it starts, such as the layout of a tile that came
/// back into view, which the frame doesn't wait for, but a frame soon after
/// shows.
fn frames(bench: &mut Bench, mut map: HeadlessMap, camera: impl Fn(usize) -> CameraOptions) {
    map.settle();
    for frame in 0..FRAMES {
        map.jump(&camera(frame));
        map.wait_for_workers();
        map.draw_frame();
    }
    map.jump(&camera(0));
    map.settle();

    let drawn = map.frames();
    let objects = map.gpu_objects_created();
    let work = map.background_work();
    for frame in 0..FRAMES {
        map.jump(&camera(frame));
        bench.measure(|| {
            map.draw_frame();
            map.wait_for_workers();
        });
    }
    bench.count("background_work", map.background_work() - work);
    assert_eq!(map.frames() - drawn, FRAMES as u64, "one frame per camera");
    bench.per_part("frame");
    bench.bytes("gpu_buffers", map.gpu_buffer_bytes());
    bench.count("gpu_objects", map.gpu_objects_created() - objects);
    if let Some(path) = &bench.image {
        map.save_image(path);
    }
}
