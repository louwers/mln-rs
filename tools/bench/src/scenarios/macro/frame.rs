//! Frames of a loaded map as its camera pans, turns and tilts, as during a
//! gesture or an animation over tiles already there.

use mln_bench_workload::paths::{FRAMES, Path};
use mln_engine::camera::CameraOptions;
use mln_engine::map::MapMode;

use super::VIEW;
use crate::scenario::{Bench, Kind, Scenario};
use crate::support::fixtures::Fixtures;
use crate::support::map::{Drapes, HeadlessMap};

pub(crate) const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "macro/frame/street",
        kind: Kind::Macro,
        question: "What does a frame of a city's streets cost while the camera pans, turns and tilts over loaded tiles?",
        run: street,
    },
    Scenario {
        name: "macro/frame/overzoom",
        kind: Kind::Macro,
        question: "What does a frame of a city's streets cost far past its tiles' maximum zoom, tilted, while the camera turns over loaded tiles?",
        run: overzoom,
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
    Scenario {
        name: "macro/frame/earthquakes",
        kind: Kind::Macro,
        question: "What does a frame of a map of earthquakes cost, a heatmap that gives way to circles, while the camera circles and zooms in and out?",
        run: earthquakes,
    },
    Scenario {
        name: "macro/frame/terrain",
        kind: Kind::Macro,
        question: "What does a frame of a city's hills cost, tinted by elevation and hillshaded, while the camera circles, turns and tilts over loaded tiles?",
        run: terrain,
    },
    Scenario {
        name: "macro/frame/terrain-3d",
        kind: Kind::Macro,
        question: "What does a frame of a city's hills in 3D cost, tinted by elevation and hillshaded, while the camera circles, turns and tilts over loaded tiles?",
        run: terrain_3d,
    },
    Scenario {
        name: "macro/frame/labels",
        kind: Kind::Macro,
        question: "What does a frame of a city's dense labels cost, POI labels choosing among anchors, while the tilted camera turns over loaded tiles?",
        run: labels,
    },
];

/// The question: what does a frame of western San Francisco at zoom 13.5
/// cost while the camera turns, pans and tilts over loaded tiles
/// ([`Path::Street`])? One measured unit is one frame: its update, symbol
/// placement, draw calls and the GPU finishing it.
fn street(bench: &mut Bench) {
    path(bench, Path::Street);
}

/// The question: what does a frame of downtown San Francisco cost at zoom
/// 22, eight zooms past its tiles' 14 and tilted to 60 degrees, while the
/// camera turns over loaded tiles ([`Path::Overzoom`])? The tiles in view
/// are drawn many times their size, so what a tile's symbols cost per frame
/// shows here as it does at no other zoom. One measured unit is one frame.
fn overzoom(bench: &mut Bench) {
    path(bench, Path::Overzoom);
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

/// The question: what does a frame of a month's earthquakes over the world
/// cost, a heatmap and a circle for each over Liberty, while the camera
/// circles central Japan zooming in and out ([`Path::Earthquakes`])? One
/// measured unit is one frame. Tiles keep leaving the view and coming back
/// along this path, and the map lays a tile out again when it comes back, so
/// most frames start background work, which counts in them.
fn earthquakes(bench: &mut Bench) {
    path(bench, Path::Earthquakes);
}

/// The question: what does a frame of San Francisco's hills cost, Liberty
/// with color relief and hillshading from the terrain tiles, while the
/// camera circles Twin Peaks ([`Path::Terrain`])? One measured unit is one
/// frame.
fn terrain(bench: &mut Bench) {
    path(bench, Path::Terrain);
}

/// The question: what does the frame of `macro/frame/terrain` cost with 3D
/// terrain on ([`Path::Terrain3d`]): the terrain tiles raise the ground,
/// Liberty's fills and lines and the color relief and hillshade drape over
/// it, and its labels stand on it? The two differ only in the terrain, so
/// their difference is what it adds. One measured unit is one frame. It also
/// reports what the frames drew of their drapes, and the most they held
/// ([`Drapes`]).
fn terrain_3d(bench: &mut Bench) {
    path(bench, Path::Terrain3d);
}

/// The question: what does a frame of downtown San Francisco's labels cost,
/// Liberty with POI labels that choose among anchors, while the camera turns
/// a full circle tilted to 60 degrees ([`Path::Labels`])? One measured unit
/// is one frame.
fn labels(bench: &mut Bench) {
    path(bench, Path::Labels);
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
/// shows. Over terrain, it also reports what the frames drew of their
/// drapes ([`Drapes`]).
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
    let pipeline = map.pipeline_statistics();
    let draw_calls = map.draw_calls();
    let mut drapes = Drapes::default();
    for frame in 0..FRAMES {
        map.jump(&camera(frame));
        bench.measure(|| {
            map.draw_frame();
            map.wait_for_workers();
        });
        drapes.frame(&map);
    }
    bench.count("draw_calls", map.draw_calls() - draw_calls);
    drapes.declare(bench, &map);
    bench.count("background_work", map.background_work() - work);
    map.count_pipeline(bench, pipeline);
    assert_eq!(map.frames() - drawn, FRAMES as u64, "one frame per camera");
    bench.per_part("frame");
    bench.bytes("gpu_buffers", map.gpu_buffer_bytes());
    bench.count("gpu_objects", map.gpu_objects_created() - objects);
    if let Some(path) = &bench.image {
        map.save_image(path);
    }
}
