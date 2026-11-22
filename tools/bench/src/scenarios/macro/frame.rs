//! Frames of a loaded map as its camera pans, turns and tilts, as during a
//! gesture or an animation over tiles already there.

use mln_bench_workload::paths::{FRAMES, Path};
use mln_map::{BoundOptions, CameraOptions, MapMode};

use super::VIEW;
use crate::scenario::{Bench, Kind, Scenario};
use crate::support::fixtures::Fixtures;
use crate::support::map::{DrawMeter, HeadlessMap, gesture_phase};

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
        question: "What does a frame over 3D terrain cost, with draped layers and layers that stand on it, while the camera circles, turns and tilts over loaded tiles?",
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

/// The question: what does a frame over San Francisco's hills in 3D cost,
/// Liberty draped over the terrain tiles' ground in four stacks with its
/// extruded buildings and labels standing between and above them, while the
/// camera circles Twin Peaks tilted to 65 degrees ([`Path::Terrain3d`])? One
/// measured unit is one frame: its update, the drapes it draws again, the
/// surface and the layers that stand on it. The camera moves as a gesture
/// moves it, which keeps the drapes it drew at another zoom, and draws
/// those of the hillshade, lit from the viewport, again in every frame that
/// turns the map; the last frame ends the gesture, which draws the others
/// again where they differ from what the map at rest shows. It also reports
/// the frames' render passes and draw calls, the drapes they drew, and the
/// most the terrain held ([`DrawMeter`]).
fn terrain_3d(bench: &mut Bench) {
    let fixtures = Fixtures::open(bench);
    let path = Path::Terrain3d;
    let camera = path.camera(&fixtures);
    let style = path.style(&fixtures);
    let mut map = HeadlessMap::open(bench, &fixtures, VIEW, MapMode::Continuous, &style, &camera(0));
    // The path tilts past the map's default bound.
    let max_pitch = (0..FRAMES).filter_map(|frame| camera(frame).pitch).fold(0.0, f64::max);
    let bounds = BoundOptions { max_pitch: Some(max_pitch), ..Default::default() };
    map.map().set_bounds(&bounds).expect("a pitch the map can take");
    frames(bench, map, camera, Some(DrawMeter::default()));
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
    frames(bench, map, camera, None);
}

/// Loads the path, drawing it once with its tiles loaded before each
/// frame, then measures one frame at each of its cameras: the frame, and
/// the background work it starts, such as the layout of a tile that came
/// back into view, which the frame doesn't wait for, but a frame soon after
/// shows. With a `meter`, the camera moves as one gesture's, and it also
/// reports what the measured frames drew.
fn frames(
    bench: &mut Bench,
    mut map: HeadlessMap,
    camera: impl Fn(usize) -> CameraOptions,
    mut meter: Option<DrawMeter>,
) {
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
    for frame in 0..FRAMES {
        match meter {
            Some(_) => map.gesture(&camera(frame), gesture_phase(frame, FRAMES)),
            None => map.jump(&camera(frame)),
        }
        bench.measure(|| {
            map.draw_frame();
            map.wait_for_workers();
        });
        if let Some(meter) = &mut meter {
            meter.frame(&map);
        }
    }
    if let Some(meter) = &meter {
        meter.report(bench);
    }
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
