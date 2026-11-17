//! Frames of a loaded map as its camera moves.

use std::time::Instant;

use mln_map::{CameraOptions, RenderMode};
use mln_util::geo::LatLng;
use mln_util::size::Size;
use serde_json::{Value, json};

use crate::scenario::{Bench, Kind, Scenario};
use crate::support::map::HeadlessMap;
use crate::support::random::Random;

pub(crate) const SCENARIOS: &[Scenario] = &[Scenario {
    name: "macro/frame/inline",
    kind: Kind::Macro,
    question: "What does drawing a frame cost while the camera turns and tilts over a style whose data is all inline?",
    run: inline,
}];

const SIZE: Size = Size { width: 512, height: 512 };
const FRAMES: usize = 120;

fn inline(bench: &mut Bench) {
    let mut map = HeadlessMap::new(SIZE);
    bench.gpu = Some(map.gpu());
    map.map().load_style_json(&style().to_string());
    // Every camera of the path once, so that its tiles are loaded and laid
    // out before the measured pass.
    for frame in 0..FRAMES {
        map.jump(&camera(frame));
        map.settle();
    }
    let drawn_before = map.frames();
    let mut times = Vec::with_capacity(FRAMES);
    let mut incomplete = 0;
    for frame in 0..FRAMES {
        map.jump(&camera(frame));
        let start = Instant::now();
        let status = bench.measure(|| map.draw_frame());
        times.push(start.elapsed());
        if status.mode != RenderMode::Full {
            incomplete += 1;
        }
    }
    bench.count("frames", map.frames() - drawn_before);
    bench.count("frames_incomplete", incomplete);
    bench.bytes("gpu_buffers", map.gpu_buffer_bytes());
    bench.percentiles("frame", &times);
}

/// The camera at `frame`: a full turn over the path, tilting up and down.
fn camera(frame: usize) -> CameraOptions {
    let t = frame as f64 / FRAMES as f64;
    CameraOptions {
        center: Some(LatLng::new(10.0, 15.0).expect("a center")),
        zoom: Some(2.5),
        bearing: Some(360.0 * t),
        pitch: Some(30.0 * (std::f64::consts::TAU * t).sin().abs()),
        ..Default::default()
    }
}

/// A background, and fills, lines and circles over GeoJSON data in the
/// style itself. Transitions are off, so a frame shows its final state.
fn style() -> Value {
    let mut random = Random::new(2);
    let mut features = Vec::new();
    for _ in 0..300 {
        let (x, y) = (random.range(-60.0, 90.0), random.range(-40.0, 60.0));
        let (w, h) = (random.range(1.0, 6.0), random.range(1.0, 6.0));
        let ring = [[x, y], [x + w, y], [x + w, y + h], [x, y + h], [x, y]];
        features.push(json!({
            "type": "Feature",
            "properties": { "kind": "area", "rank": random.below(5) },
            "geometry": { "type": "Polygon", "coordinates": [ring] },
        }));
    }
    for _ in 0..200 {
        let mut point = [random.range(-60.0, 90.0), random.range(-40.0, 60.0)];
        let mut line = vec![point];
        for _ in 0..20 {
            point = [point[0] + random.range(-2.0, 2.0), point[1] + random.range(-2.0, 2.0)];
            line.push(point);
        }
        features.push(json!({
            "type": "Feature",
            "properties": { "kind": "road", "rank": random.below(5) },
            "geometry": { "type": "LineString", "coordinates": line },
        }));
    }
    for _ in 0..1000 {
        features.push(json!({
            "type": "Feature",
            "properties": { "kind": "place", "rank": random.below(5) },
            "geometry": { "type": "Point", "coordinates": [random.range(-60.0, 90.0), random.range(-40.0, 60.0)] },
        }));
    }
    json!({
        "version": 8,
        "transition": { "duration": 0, "delay": 0 },
        "sources": {
            "data": { "type": "geojson", "data": { "type": "FeatureCollection", "features": features } },
        },
        "layers": [
            { "id": "background", "type": "background", "paint": { "background-color": "#f2efe9" } },
            {
                "id": "areas",
                "type": "fill",
                "source": "data",
                "filter": ["==", ["get", "kind"], "area"],
                "paint": {
                    "fill-color": ["match", ["get", "rank"], 0, "#c8e6c9", 1, "#a5d6a7", 2, "#81c784", "#66bb6a"],
                    "fill-outline-color": "#2e7d32",
                },
            },
            {
                "id": "roads",
                "type": "line",
                "source": "data",
                "filter": ["==", ["get", "kind"], "road"],
                "layout": { "line-join": "round", "line-cap": "round" },
                "paint": {
                    "line-color": "#e57373",
                    "line-width": ["interpolate", ["linear"], ["zoom"], 0, 1, 6, ["+", 2, ["get", "rank"]]],
                },
            },
            {
                "id": "places",
                "type": "circle",
                "source": "data",
                "filter": ["==", ["get", "kind"], "place"],
                "paint": {
                    "circle-radius": ["+", 2, ["get", "rank"]],
                    "circle-color": "#1565c0",
                    "circle-stroke-width": 1,
                    "circle-stroke-color": "#ffffff",
                },
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_style_is_the_same_every_time() {
        assert_eq!(style(), style());
        assert_eq!(camera(0).bearing, Some(0.0));
    }
}
