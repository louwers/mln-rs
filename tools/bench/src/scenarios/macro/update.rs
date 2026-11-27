//! Edits an application makes to a live map: each scenario edits a
//! settled map of a city's streets, and runs until the map has drawn a
//! complete frame showing the edit, with nothing left to load or animate.
//!
//! The data an application would supply (points, layers, colors, images
//! and the features it hovers) is generated from a fixed seed.

use std::sync::Arc;

use mln_engine::camera::CameraOptions;
use mln_engine::map::MapMode;
use mln_style::Edit;
use mln_style::bitmap::PremultipliedImage;
use mln_style::feature::{FeatureState, Value};
use mln_style::geo::LatLngBounds;
use mln_style::image::Impl as StyleImage;
use mln_style::size::Size;
use serde_json::json;

use super::VIEW;
use crate::scenario::{Bench, Kind, Scenario};
use crate::support::fixtures::{Fixtures, Tiles};
use crate::support::map::HeadlessMap;
use crate::support::random::Random;

pub(crate) const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "macro/update/geojson",
        kind: Kind::Macro,
        question: "What does replacing a GeoJSON source's data cost, until the map shows the new data?",
        run: geojson,
    },
    Scenario {
        name: "macro/update/layers",
        kind: Kind::Macro,
        question: "What does adding a batch of layers to a city's map, then removing them, cost until the map shows each?",
        run: layers,
    },
    Scenario {
        name: "macro/update/paint",
        kind: Kind::Macro,
        question: "What does setting a layer's paint property cost, until the map shows the new value?",
        run: paint,
    },
    Scenario {
        name: "macro/update/feature-state",
        kind: Kind::Macro,
        question: "What does a frame cost that moves a hover state from one feature to another?",
        run: feature_state,
    },
    Scenario {
        name: "macro/update/images",
        kind: Kind::Macro,
        question: "What does adding the images that visible symbols use cost, until the map shows them?",
        run: images,
    },
];

/// The zoom of every update's map: a street view of central San
/// Francisco, with buildings in 3D and points of interest.
const ZOOM: f64 = 15.0;

/// The question: what does replacing the data of a GeoJSON source of 2,000
/// points cost, until the map shows the points where the new data puts
/// them? 2,000 points are the vehicles of a city's transit or delivery
/// fleet that a tracking application shows live. One measured unit is one
/// update: parsing the GeoJSON text, indexing it for tiling, cutting and
/// laying out the tiles in view, and the frames until the map shows them.
/// Ten updates run, each moving every point a step along its heading.
fn geojson(bench: &mut Bench) {
    const POINTS: usize = 2000;
    const UPDATES: usize = 10;

    let fixtures = Fixtures::open(bench);
    let (mut map, view) = open(bench, &fixtures);
    let mut random = Random::new(1);
    let mut vehicles: Vec<(f64, f64, f64)> = (0..POINTS)
        .map(|_| {
            (random.range(view.west(), view.east()), random.range(view.south(), view.north()), random.range(0.0, 360.0))
        })
        .collect();
    let collection = |vehicles: &[(f64, f64, f64)]| {
        let features: Vec<serde_json::Value> = vehicles
            .iter()
            .enumerate()
            .map(|(id, &(longitude, latitude, heading))| {
                let kind = ["bus", "tram", "van", "bike"][id % 4];
                json!({
                    "type": "Feature",
                    "id": id,
                    "geometry": { "type": "Point", "coordinates": [longitude, latitude] },
                    "properties": { "kind": kind, "heading": heading.round() },
                })
            })
            .collect();
        json!({ "type": "FeatureCollection", "features": features }).to_string()
    };
    map.add_geojson_source("vehicles", &collection(&vehicles));
    add_layer(
        &mut map,
        &json!({
            "id": "vehicles",
            "type": "circle",
            "source": "vehicles",
            "paint": {
                "circle-radius": 4,
                "circle-color": ["match", ["get", "kind"], "bus", "#d7301f", "tram", "#2b8cbe", "van", "#31a354", "#756bb1"],
                "circle-stroke-width": 1,
                "circle-stroke-color": "#ffffff",
            },
        }),
    );
    map.settle();

    let mut frames = 0;
    for _ in 0..UPDATES {
        for (longitude, latitude, heading) in &mut vehicles {
            *heading += random.range(-20.0, 20.0);
            // About 30 m, a few seconds of driving.
            *longitude += 0.0003 * heading.to_radians().sin();
            *latitude += 0.0003 * heading.to_radians().cos();
        }
        let json = collection(&vehicles);
        frames += bench.measure(|| {
            map.set_geojson("vehicles", &json);
            map.settle()
        });
    }
    bench.per_part("update");
    finish(bench, &mut map, frames);
}

/// The question: what does adding 20 layers over the map's vector source,
/// then removing them, cost until the map shows each change? 20 layers
/// are an overlay an application turns on and off as a group, such as a
/// theme of roads, areas and places to highlight: five each of lines over
/// roads, fills over land use, circles and labels over points of interest,
/// each picking one class of feature. One measured unit is one batch: the
/// edits, laying out the tiles in view again, and the frames until the map
/// shows them. Two batches run, the additions and the removals.
fn layers(bench: &mut Bench) {
    const LAYERS: usize = 20;

    let fixtures = Fixtures::open(bench);
    let (mut map, _) = open(bench, &fixtures);
    map.settle();

    let mut random = Random::new(2);
    let layers: Vec<String> = (0..LAYERS).map(|index| overlay_layer(index, &mut random).to_string()).collect();
    let ids: Vec<String> = (0..LAYERS).map(|index| format!("overlay-{index}")).collect();
    let mut frames = bench.measure(|| {
        for layer in &layers {
            map.map().edit_loaded_style(|style| style.add_layer_json(layer, None)).expect("a layer");
        }
        map.settle()
    });
    // The image shows the layers added (the map without them is
    // macro/load/street's), written before the removals so that it holds no
    // memory while they run.
    if let Some(path) = &bench.image {
        map.save_image(path);
    }
    frames += bench.measure(|| {
        for id in &ids {
            map.map().edit_loaded_style(|style| style.apply(Edit::RemoveLayer(id.clone()))).expect("a layer");
        }
        map.settle()
    });
    bench.per("batch", 2);
    bench.count("frames", frames);
    bench.bytes("gpu_buffers", map.gpu_buffer_bytes());
}

/// The `index`th layer of the overlay that [`layers`] adds: a line, a
/// fill, a circle or a label layer in turn, over one class of feature, its
/// colors and sizes from `random`.
fn overlay_layer(index: usize, random: &mut Random) -> serde_json::Value {
    const ROADS: [&str; 6] = ["primary", "secondary", "tertiary", "minor", "service", "path"];
    const AREAS: [(&str, &str); 5] = [
        ("landuse", "residential"),
        ("landuse", "commercial"),
        ("landcover", "grass"),
        ("landcover", "wood"),
        ("park", "park"),
    ];
    const PLACES: [&str; 8] = ["restaurant", "cafe", "shop", "bus", "school", "lodging", "bar", "grocery"];
    let id = format!("overlay-{index}");
    let color = color(random);
    let class = |classes: &[&str], random: &mut Random| classes[random.below(classes.len())].to_owned();
    match index % 4 {
        0 => json!({
            "id": id, "type": "line", "source": "openmaptiles", "source-layer": "transportation",
            "filter": ["==", ["get", "class"], class(&ROADS, random)],
            "layout": { "line-cap": "round", "line-join": "round" },
            "paint": { "line-color": color, "line-width": random.range(1.0, 4.0), "line-opacity": 0.7 },
        }),
        1 => {
            let (layer, class) = AREAS[random.below(AREAS.len())];
            json!({
                "id": id, "type": "fill", "source": "openmaptiles", "source-layer": layer,
                "filter": ["==", ["get", "class"], class],
                "paint": { "fill-color": color, "fill-opacity": 0.3 },
            })
        }
        2 => json!({
            "id": id, "type": "circle", "source": "openmaptiles", "source-layer": "poi",
            "filter": ["==", ["get", "class"], class(&PLACES, random)],
            "paint": {
                "circle-radius": random.range(3.0, 6.0), "circle-color": color,
                "circle-stroke-width": 1, "circle-stroke-color": "#ffffff",
            },
        }),
        _ => json!({
            "id": id, "type": "symbol", "source": "openmaptiles", "source-layer": "poi",
            "filter": ["==", ["get", "class"], class(&PLACES, random)],
            "layout": {
                "text-field": ["get", "name"], "text-font": ["Noto Sans Regular"],
                "text-size": random.range(10.0, 14.0).round(), "text-offset": [0, 1.2], "text-anchor": "top",
            },
            "paint": { "text-color": color, "text-halo-color": "#ffffff", "text-halo-width": 1 },
        }),
    }
}

/// The question: what does setting the color of the city's 3D buildings
/// to a new value that depends on each building's height cost, until the
/// map shows it? Such a value is how an application colors features by
/// their data; one that depends on a feature's properties is evaluated for
/// each feature, and transitions from the old one over the style's
/// default 300 ms. One measured unit is one edit: the edit, and the frames
/// until the transition has ended. Six edits run, each with new colors.
fn paint(bench: &mut Bench) {
    const EDITS: usize = 6;

    let fixtures = Fixtures::open(bench);
    let (mut map, _) = open(bench, &fixtures);
    let mut random = Random::new(3);
    let by_height = |random: &mut Random| {
        json!(["interpolate", ["linear"], ["get", "render_height"], 0, color(random), 60, color(random)]).to_string()
    };
    let first = by_height(&mut random);
    set_paint(&mut map, "building-3d", "fill-extrusion-color", &first);
    map.settle();

    let mut frames = 0;
    for _ in 0..EDITS {
        let value = by_height(&mut random);
        frames += bench.measure(|| {
            set_paint(&mut map, "building-3d", "fill-extrusion-color", &value);
            map.settle()
        });
    }
    bench.per_part("edit");
    finish(bench, &mut map, frames);
}

fn set_paint(map: &mut HeadlessMap, layer: &str, name: &str, value: &str) {
    map.map().edit_loaded_style(|style| style.set_layer_property_json(layer, name, value)).expect("a paint property");
}

/// The question: what does a frame cost that moves the hover state from
/// one feature of a GeoJSON source to the next, as a pointer moving over a
/// map of regions does? The source holds 600 regions, about the census
/// block groups of a city such as San Francisco, which an application
/// shades and highlights under the pointer. One measured unit is one
/// frame: clearing the state of the feature hovered before, setting the
/// state of the next, the frame that shows it, and any background work the
/// frame starts. The pointer moves to a neighbouring region at each of 120
/// frames.
fn feature_state(bench: &mut Bench) {
    const COLUMNS: usize = 30;
    const ROWS: usize = 20;
    const FRAMES: usize = 120;

    let fixtures = Fixtures::open(bench);
    let (mut map, view) = open(bench, &fixtures);
    let mut random = Random::new(4);
    // A lattice of corners across the view, each moved by up to a third of
    // a region, which the regions share.
    let (width, height) = ((view.east() - view.west()) / COLUMNS as f64, (view.north() - view.south()) / ROWS as f64);
    let corners: Vec<Vec<[f64; 2]>> = (0..=ROWS)
        .map(|row| {
            (0..=COLUMNS)
                .map(|column| {
                    let inner = |i: usize, n: usize| i > 0 && i < n;
                    let jitter = |random: &mut Random, size: f64, moves: bool| {
                        if moves { random.range(-size / 3.0, size / 3.0) } else { 0.0 }
                    };
                    let moves = inner(row, ROWS) && inner(column, COLUMNS);
                    [
                        view.west() + column as f64 * width + jitter(&mut random, width, moves),
                        view.south() + row as f64 * height + jitter(&mut random, height, moves),
                    ]
                })
                .collect()
        })
        .collect();
    let features: Vec<serde_json::Value> = (0..ROWS)
        .flat_map(|row| (0..COLUMNS).map(move |column| (row, column)))
        .map(|(row, column)| {
            let ring = [
                corners[row][column],
                corners[row][column + 1],
                corners[row + 1][column + 1],
                corners[row + 1][column],
                corners[row][column],
            ];
            json!({
                "type": "Feature",
                "id": row * COLUMNS + column,
                "geometry": { "type": "Polygon", "coordinates": [ring] },
                "properties": { "share": random.range(0.0, 1.0) },
            })
        })
        .collect();
    map.add_geojson_source("regions", &json!({ "type": "FeatureCollection", "features": features }).to_string());
    add_layer(
        &mut map,
        &json!({
            "id": "regions",
            "type": "fill",
            "source": "regions",
            "paint": {
                "fill-color": [
                    "case", ["boolean", ["feature-state", "hover"], false], "#f03b20",
                    ["interpolate", ["linear"], ["get", "share"], 0, "#ffffcc", 1, "#253494"],
                ],
                "fill-opacity": 0.45,
                "fill-outline-color": "#ffffff",
            },
        }),
    );
    map.settle();

    let hover =
        |hovered: bool| -> FeatureState { std::iter::once(("hover".to_owned(), Value::Bool(hovered))).collect() };
    let (mut row, mut column) = (ROWS / 2, COLUMNS / 2);
    let mut hovered: Option<String> = None;
    let work = map.background_work();
    for _ in 0..FRAMES {
        match random.below(4) {
            0 => row = (row + 1).min(ROWS - 1),
            1 => row = row.saturating_sub(1),
            2 => column = (column + 1).min(COLUMNS - 1),
            _ => column = column.saturating_sub(1),
        }
        let next = (row * COLUMNS + column).to_string();
        bench.measure(|| {
            if let Some(previous) = &hovered {
                map.map().set_feature_state("regions", None, previous, hover(false));
            }
            map.map().set_feature_state("regions", None, &next, hover(true));
            map.draw_frame();
            map.wait_for_workers();
        });
        hovered = Some(next);
    }
    bench.count("background_work", map.background_work() - work);
    bench.per_part("frame");
    bench.bytes("gpu_buffers", map.gpu_buffer_bytes());
    if let Some(path) = &bench.image {
        map.save_image(path);
    }
}

/// The question: what does adding the 24 marker images that a layer of
/// visible symbols names cost, until the map shows the markers? 24 images
/// are an application's set of category icons, and its 300 markers the
/// results of a search or the stores of a locator in view. One measured
/// unit is the 24 additions, laying out the tiles that use them again, and
/// the frames until the markers have faded in.
fn images(bench: &mut Bench) {
    const IMAGES: usize = 24;
    const MARKERS: usize = 300;

    let fixtures = Fixtures::open(bench);
    let (mut map, view) = open(bench, &fixtures);
    let mut random = Random::new(5);
    let features: Vec<serde_json::Value> = (0..MARKERS)
        .map(|id| {
            let point = [random.range(view.west(), view.east()), random.range(view.south(), view.north())];
            json!({
                "type": "Feature",
                "id": id,
                "geometry": { "type": "Point", "coordinates": point },
                "properties": { "icon": format!("marker-{}", random.below(IMAGES)) },
            })
        })
        .collect();
    map.add_geojson_source("markers", &json!({ "type": "FeatureCollection", "features": features }).to_string());
    add_layer(
        &mut map,
        &json!({
            "id": "markers",
            "type": "symbol",
            "source": "markers",
            "layout": { "icon-image": ["get", "icon"], "icon-allow-overlap": true },
        }),
    );
    map.settle();

    let images: Vec<Arc<StyleImage>> = (0..IMAGES).map(|index| Arc::new(marker(index, &mut random))).collect();
    let frames = bench.measure(|| {
        for image in images {
            map.map().edit_loaded_style(|style| style.apply(Edit::AddImage(image))).expect("an image");
        }
        map.settle()
    });
    finish(bench, &mut map, frames);
}

/// A round marker of 24 pixels, its color from `random`, with a white
/// ring.
fn marker(index: usize, random: &mut Random) -> StyleImage {
    const SIZE: u32 = 24;
    let color = [random.below(256) as u8, random.below(256) as u8, random.below(256) as u8, 255];
    let mut pixels = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let center = f64::from(SIZE) / 2.0;
            let distance = (f64::from(x) + 0.5 - center).hypot(f64::from(y) + 0.5 - center);
            pixels.extend_from_slice(&match distance {
                d if d <= 9.0 => color,
                d if d <= 11.5 => [255; 4],
                _ => [0; 4],
            });
        }
    }
    let image = PremultipliedImage::from_vec(Size { width: SIZE, height: SIZE }, pixels);
    StyleImage::new(format!("marker-{index}"), image, 1.0, false, Vec::new(), Vec::new(), None, None, None)
        .expect("an image")
}

/// A map of the street tiles at [`ZOOM`] over their center, and the bounds
/// of its view. The scenario settles it once it has added what it needs.
fn open(bench: &mut Bench, fixtures: &Fixtures) -> (HeadlessMap, LatLngBounds) {
    let camera: CameraOptions = super::street_center(fixtures, ZOOM);
    let style = fixtures.style(Tiles::Street);
    let mut map = HeadlessMap::open(bench, fixtures, VIEW, MapMode::Continuous, &style, &camera);
    let view = map.map().lat_lng_bounds_for_camera(&camera).expect("the view's bounds");
    (map, view)
}

fn add_layer(map: &mut HeadlessMap, layer: &serde_json::Value) {
    let layer = layer.to_string();
    map.map().edit_loaded_style(|style| style.add_layer_json(&layer, None)).expect("a layer");
}

/// A color of a hue, saturation and lightness from `random`.
fn color(random: &mut Random) -> String {
    format!("hsl({}, {}%, {}%)", random.below(360), 40 + random.below(50), 35 + random.below(30))
}

/// Declares the frames the measured edits drew and the GPU buffers they
/// left, and writes the last frame as the image.
fn finish(bench: &mut Bench, map: &mut HeadlessMap, frames: u64) {
    bench.count("frames", frames);
    bench.bytes("gpu_buffers", map.gpu_buffer_bytes());
    if let Some(path) = &bench.image {
        map.save_image(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_are_round_and_ringed() {
        let image = marker(0, &mut Random::new(5));
        let pixel = |x: usize, y: usize| &image.image.data[(y * 24 + x) * 4..][..4];
        assert_eq!(pixel(0, 0), [0; 4]);
        assert_eq!(pixel(12, 2), [255; 4]);
        assert_eq!(pixel(12, 12)[3], 255);
    }

    #[test]
    fn the_overlay_has_each_kind_of_layer() {
        let mut random = Random::new(2);
        let kinds: Vec<String> =
            (0..4).map(|index| overlay_layer(index, &mut random)["type"].as_str().unwrap().to_owned()).collect();
        assert_eq!(kinds, ["line", "fill", "circle", "symbol"]);
    }
}
