//! Drawing one layer type: a style of that one layer, over data already
//! loaded, at a fixed view, frame after frame. A frame's work is then mostly
//! the layer's draw calls and its shader programs, which run on the GPU, or
//! as code of the process on a CPU rasterizer, whose instructions count
//! them. The cases set a layer's paint as constants, which its programs take
//! as uniforms, or from each feature's data, which they read per vertex.

use mln_bench_workload::paths::{VIEW, tile_center};
use mln_engine::camera::CameraOptions;
use mln_engine::map::MapMode;
use mln_style::geo::LatLng;
use serde_json::{Value, json};

use crate::scenario::{Bench, Kind, Scenario};
use crate::support::fixtures::{Fixtures, Tiles};
use crate::support::map::HeadlessMap;
use crate::support::random::Random;

/// The frames each scenario measures.
const FRAMES: usize = 30;

macro_rules! scenarios {
    ($($name:literal, $question:literal, $run:ident => $case:expr;)*) => {
        pub(crate) const SCENARIOS: &[Scenario] = &[
            $(Scenario { name: $name, kind: Kind::Micro, question: $question, run: $run },)*
        ];
        $(fn $run(bench: &mut Bench) { draw(bench, &$case) })*
    };
}

scenarios! {
    "micro/draw/fill/constant", "What does drawing a city's buildings as fills of one colour cost?",
        fill_constant => Case::street(|_| fill(&json!("#d6c8bc")));
    "micro/draw/fill/data-driven", "What does drawing a city's buildings as fills coloured by their height cost?",
        fill_data_driven => Case::street(|_| fill(&by_height(["#e8ded4", "#a08670"])));
    "micro/draw/fill/pattern", "What does drawing a city's buildings as fills of a sprite's pattern cost?",
        fill_pattern => Case::street(|_| json!({
            "id": "layer", "type": "fill", "source": "openmaptiles", "source-layer": "building",
            "paint": { "fill-pattern": "wetland_bg_11" },
        }));
    "micro/draw/line/constant", "What does drawing a city's roads as lines of one width and colour cost?",
        line_constant => Case::street(|_| line(&json!({ "line-color": "#f2934a", "line-width": 3 })));
    "micro/draw/line/data-driven", "What does drawing a city's roads as lines whose width and colour follow their class cost?",
        line_data_driven => Case::street(|_| line(&json!({
            "line-color": ["match", ["get", "class"], ["motorway", "trunk"], "#e892a2", "primary", "#fcd6a4", "#ffffff"],
            "line-width": ["match", ["get", "class"], ["motorway", "trunk"], 6, "primary", 4, 2],
        })));
    "micro/draw/line/dash", "What does drawing a city's roads as dashed lines cost?",
        line_dash => Case::street(|_| line(&json!({ "line-color": "#9e9cab", "line-width": 2, "line-dasharray": [2, 2] })));
    "micro/draw/line/pattern", "What does drawing a city's roads as lines of a sprite's pattern cost?",
        line_pattern => Case::street(|_| line(&json!({ "line-pattern": "pedestrian_polygon", "line-width": 6 })));
    "micro/draw/line/gradient", "What does drawing a long route as a line coloured along its length cost?",
        line_gradient => Case::route();
    "micro/draw/fill-extrusion/constant", "What does drawing a city's buildings as extrusions of one height and colour cost, seen tilted?",
        fill_extrusion_constant => Case::tilted(|_| extrusion(&json!(20), &json!("#d6c8bc")));
    "micro/draw/fill-extrusion/data-driven", "What does drawing a city's buildings as extrusions of their own height, coloured by it, cost, seen tilted?",
        fill_extrusion_data_driven => Case::tilted(|_| extrusion(&json!(["get", "render_height"]), &by_height(["#e8ded4", "#a08670"])));
    "micro/draw/circle/constant", "What does drawing a month's earthquakes over the world as circles of one size and colour cost?",
        circle_constant => Case::earthquakes(|_| circle(&json!(4), &json!("#d73027")));
    "micro/draw/circle/data-driven", "What does drawing a month's earthquakes over the world as circles sized by magnitude and coloured by depth cost?",
        circle_data_driven => Case::earthquakes(|_| circle(
            &json!(["interpolate", ["linear"], ["get", "mag"], 0, 2, 7, 12]),
            &json!(["interpolate", ["linear"], ["coalesce", ["get", "depth"], 0], 0, "#d73027", 300, "#4575b4"]),
        ));
    "micro/draw/heatmap/constant", "What does drawing a month's earthquakes over the world as a heatmap of equal weights cost?",
        heatmap_constant => Case::earthquakes(|_| heatmap(&json!(1)));
    "micro/draw/heatmap/data-driven", "What does drawing a month's earthquakes over the world as a heatmap weighted by magnitude cost?",
        heatmap_data_driven => Case::earthquakes(|_| heatmap(&json!(["interpolate", ["linear"], ["get", "mag"], 0, 0, 6, 1])));
    "micro/draw/symbol/icon", "What does drawing a city's points of interest as icons cost, those that collide hidden?",
        symbol_icon => Case::street(|_| json!({
            "id": "layer", "type": "symbol", "source": "openmaptiles", "source-layer": "poi",
            "layout": { "icon-image": ["get", "class"] },
        }));
    "micro/draw/symbol/text", "What does drawing a city's points of interest as haloed labels cost, those that collide hidden?",
        symbol_text => Case::street(|_| json!({
            "id": "layer", "type": "symbol", "source": "openmaptiles", "source-layer": "poi",
            "layout": { "text-field": ["get", "name"], "text-font": ["Noto Sans Regular"], "text-size": 12 },
            "paint": { "text-color": "#666666", "text-halo-color": "#ffffff", "text-halo-width": 1, "text-halo-blur": 0.5 },
        }));
    "micro/draw/symbol/text-and-icon", "What does drawing a city's points of interest as haloed labels led by their icon cost, those that collide hidden?",
        symbol_text_and_icon => Case::street(|_| json!({
            "id": "layer", "type": "symbol", "source": "openmaptiles", "source-layer": "poi",
            "layout": {
                "text-field": ["format", ["image", ["get", "class"]], {}, " ", {}, ["get", "name"], {}],
                "text-font": ["Noto Sans Regular"], "text-size": 12,
            },
            "paint": { "text-color": "#666666", "text-halo-color": "#ffffff", "text-halo-width": 1, "text-halo-blur": 0.5 },
        }));
    "micro/draw/raster/terrain", "What does drawing a city's terrain tiles as raster images cost?",
        raster_terrain => Case::terrain(|fixtures| {
            let mut source = fixtures.terrain_source();
            source["type"] = json!("raster");
            (source, json!({ "id": "layer", "type": "raster", "source": "data" }))
        });
    "micro/draw/hillshade/terrain", "What does hillshading a city's hills cost?",
        hillshade_terrain => Case::terrain(|fixtures| (fixtures.terrain_source(), json!({
            "id": "layer", "type": "hillshade", "source": "data",
            "paint": { "hillshade-shadow-color": "#473b24" },
        })));
    "micro/draw/color-relief/terrain", "What does tinting a city's hills by elevation cost?",
        color_relief_terrain => Case::terrain(|fixtures| (fixtures.terrain_source(), json!({
            "id": "layer", "type": "color-relief", "source": "data",
            "paint": { "color-relief-color": ["interpolate", ["linear"], ["elevation"],
                0, "#709959", 100, "#f2eea2", 250, "#deb887", 500, "#a67b5b"] },
        })));
}

/// A scenario's one layer, and the source it reads besides the street
/// tiles, if any.
type Layer = Box<dyn Fn(&Fixtures) -> (Option<Value>, Value)>;

/// What a scenario draws: its layer, and the camera.
struct Case {
    layer: Layer,
    camera: Box<dyn Fn(&Fixtures) -> CameraOptions>,
}

impl Case {
    /// A layer of the street tiles, over the street tile with the most
    /// features (South of Market) at zoom 14.
    fn street(layer: fn(&Fixtures) -> Value) -> Case {
        Case { layer: Box::new(move |fixtures| (None, layer(fixtures))), camera: Box::new(|_| soma(0.0)) }
    }

    /// [`Case::street`], tilted by 50 degrees and turned, so that walls show.
    fn tilted(layer: fn(&Fixtures) -> Value) -> Case {
        Case { layer: Box::new(move |fixtures| (None, layer(fixtures))), camera: Box::new(|_| soma(50.0)) }
    }

    /// A layer of the earthquakes, over the Pacific at zoom 2, where most of
    /// them are.
    fn earthquakes(layer: fn(&Fixtures) -> Value) -> Case {
        Case {
            layer: Box::new(move |fixtures| (Some(fixtures.earthquakes_source()), layer(fixtures))),
            camera: Box::new(|_| CameraOptions {
                center: Some(LatLng::new(15.0, 160.0).expect("a center")),
                zoom: Some(2.0),
                ..Default::default()
            }),
        }
    }

    /// A layer of the terrain tiles, over Twin Peaks at zoom 12.
    fn terrain(layer: fn(&Fixtures) -> (Value, Value)) -> Case {
        Case {
            layer: Box::new(move |fixtures| {
                let (source, layer) = layer(fixtures);
                (Some(source), layer)
            }),
            camera: Box::new(|_| CameraOptions {
                center: Some(LatLng::new(37.752, -122.447).expect("a center")),
                zoom: Some(12.0),
                ..Default::default()
            }),
        }
    }

    /// A route of 2,000 points wandering across the view, coloured from
    /// start to end, as a navigation app draws the route ahead.
    fn route() -> Case {
        Case {
            layer: Box::new(|_| {
                let center = soma_center();
                let mut random = Random::new(0x707e);
                let (mut lon, mut lat, mut heading) = (center.longitude() - 0.012, center.latitude(), 0.0f64);
                let mut points = Vec::new();
                for _ in 0..2000 {
                    heading += random.range(-0.3, 0.3);
                    lon += 0.000_012 * heading.cos() + 0.000_012;
                    lat += 0.000_012 * heading.sin();
                    points.push([lon, lat]);
                }
                let data = json!({ "type": "Feature", "properties": {}, "geometry": { "type": "LineString", "coordinates": points } });
                let source = json!({ "type": "geojson", "data": data, "lineMetrics": true });
                let layer = json!({
                    "id": "layer", "type": "line", "source": "data",
                    "layout": { "line-cap": "round", "line-join": "round" },
                    "paint": { "line-width": 8, "line-gradient":
                        ["interpolate", ["linear"], ["line-progress"], 0, "#2c7bb6", 0.5, "#ffffbf", 1, "#d7191c"] },
                });
                (Some(source), layer)
            }),
            camera: Box::new(|_| soma(0.0)),
        }
    }
}

/// The center of the densest street tile, 14/2621/6332.
fn soma_center() -> LatLng {
    tile_center(14, 2621, 6332)
}

/// A camera over the densest street tile at zoom 14, tilted by `pitch`.
fn soma(pitch: f64) -> CameraOptions {
    CameraOptions {
        center: Some(soma_center()),
        zoom: Some(14.0),
        pitch: Some(pitch),
        bearing: Some(if pitch > 0.0 { 30.0 } else { 0.0 }),
        ..Default::default()
    }
}

/// A colour from `low` to `high` by a building's height, up to 60 m.
fn by_height([low, high]: [&str; 2]) -> Value {
    json!(["interpolate", ["linear"], ["coalesce", ["get", "render_height"], 0], 0, low, 60, high])
}

fn fill(color: &Value) -> Value {
    json!({ "id": "layer", "type": "fill", "source": "openmaptiles", "source-layer": "building", "paint": { "fill-color": color } })
}

fn line(paint: &Value) -> Value {
    json!({ "id": "layer", "type": "line", "source": "openmaptiles", "source-layer": "transportation", "paint": paint })
}

fn extrusion(height: &Value, color: &Value) -> Value {
    json!({
        "id": "layer", "type": "fill-extrusion", "source": "openmaptiles", "source-layer": "building",
        "paint": { "fill-extrusion-height": height, "fill-extrusion-color": color },
    })
}

fn circle(radius: &Value, color: &Value) -> Value {
    json!({
        "id": "layer", "type": "circle", "source": "data",
        "paint": { "circle-radius": radius, "circle-color": color, "circle-stroke-color": "#ffffff", "circle-stroke-width": 1 },
    })
}

fn heatmap(weight: &Value) -> Value {
    json!({ "id": "layer", "type": "heatmap", "source": "data", "paint": { "heatmap-weight": weight, "heatmap-radius": 12 } })
}

/// Draws `case`'s layer alone, once its data has loaded and its symbols have
/// faded in, then measures [`FRAMES`] frames of it at the same view.
fn draw(bench: &mut Bench, case: &Case) {
    let fixtures = Fixtures::open(bench);
    let mut style = fixtures.style_value(Tiles::Street);
    let (source, layer) = (case.layer)(&fixtures);
    if let Some(source) = source {
        style["sources"]["data"] = source;
    }
    style["layers"] = json!([layer]);
    let mut map =
        HeadlessMap::open(bench, &fixtures, VIEW, MapMode::Continuous, &style.to_string(), &(case.camera)(&fixtures));
    map.settle();
    let work = map.background_work();
    let pipeline = map.pipeline_statistics();
    for _ in 0..FRAMES {
        bench.measure(|| {
            map.draw_frame();
            map.wait_for_workers();
        });
    }
    bench.per_part("frame");
    bench.count("background_work", map.background_work() - work);
    map.count_pipeline(bench, pipeline);
    bench.bytes("gpu_buffers", map.gpu_buffer_bytes());
    if let Some(path) = &bench.image {
        map.save_image(path);
    }
}
