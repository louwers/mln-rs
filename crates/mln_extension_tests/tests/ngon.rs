//! The `org.maplibre.ngon` layer type through the facade, as a caller adds
//! it: a runtime whose registry has it draws a style with an n-gon layer,
//! and what edits of the layer and of its source make.

mod support;

use std::sync::Arc;

use mln_ngon::NgonLayer;
use mln_rs::mln_render::{Registry, RegistryBuilder};
use support::{Fixture, SIZE, wait};

const WHITE: [u8; 4] = [255, 255, 255, 255];
const RED: [u8; 4] = [255, 0, 0, 255];

/// A red square of radius 8 at the center of a white map, its sides level.
const LAYER: &str = r##"{"id":"ngons","type":"org.maplibre.ngon","source":"points",
    "paint":{"ngon-color":"#ff0000","ngon-radius":8,"ngon-corners":4,"ngon-rotate":45}}"##;

fn style() -> String {
    format!(
        r##"{{"version":8,"transition":{{"duration":0}},
        "sources":{{"points":{{"type":"geojson","data":{{"type":"Feature","properties":{{}},
            "geometry":{{"type":"Point","coordinates":[0,0]}}}}}}}},
        "layers":[{{"id":"background","type":"background","paint":{{"background-color":"#ffffff"}}}},{LAYER}]}}"##
    )
}

fn registry() -> Arc<Registry> {
    let mut builder = RegistryBuilder::new();
    mln_rs::register_builtins(&mut builder).unwrap();
    mln_ngon::register(&mut builder).unwrap();
    builder.build()
}

/// The premultiplied RGBA of the pixel `x` pixels right of the center of
/// the last frame.
fn pixel(fixture: &Fixture, x: u32) -> [u8; 4] {
    let image = wait(fixture.session.read_texture());
    let offset = ((SIZE.height / 2 * SIZE.width + SIZE.width / 2 + x) * 4) as usize;
    image.data[offset..offset + 4].try_into().unwrap()
}

#[test]
fn a_removed_layer_goes_and_a_new_source_draws_its_own_data() {
    let fixture = Fixture::new(registry(), &style());
    assert_eq!(RED, pixel(&fixture, 0));
    assert_eq!(RED, pixel(&fixture, 4));
    assert_eq!(WHITE, pixel(&fixture, 8));

    wait(fixture.map.remove_style_layer("ngons"));
    fixture.render();
    assert_eq!(WHITE, pixel(&fixture, 0));
    let error =
        fixture.session.query_rendered_layer::<NgonLayer, _>("ngons", |_, _| Ok(())).unwrap().wait().unwrap_err();
    assert_eq!(mln_rs::mln_util::ErrorKind::NotFound, error.kind);

    wait(fixture.map.add_style_layer_json(LAYER, None));
    fixture.render_loaded();
    assert_eq!(RED, pixel(&fixture, 0));

    // The layer's source changes to one whose only point is 14 pixels east
    // of the center, at zoom 0.
    let east = r#"{"type":"geojson","data":{"type":"Feature","properties":{},
        "geometry":{"type":"Point","coordinates":[9.84375,0]}}}"#;
    wait(fixture.map.add_style_source_json("east", east));
    wait(fixture.map.set_style_layer_property("ngons", "source", "\"east\""));
    fixture.render_loaded();
    assert_eq!(WHITE, pixel(&fixture, 0));
    assert_eq!(RED, pixel(&fixture, 14));
}

/// A white map at `zoom` with the n-gon layer `layer` over the points at the
/// longitudes `longitudes`, on the equator, whose properties fade over
/// `fade` milliseconds.
fn points_style(zoom: f64, fade: u32, longitudes: &[f64], layer: &str) -> String {
    let features: Vec<String> = longitudes
        .iter()
        .map(|longitude| {
            format!(
                r#"{{"type":"Feature","properties":{{"r":6}},"geometry":{{"type":"Point","coordinates":[{longitude},0]}}}}"#
            )
        })
        .collect();
    format!(
        r##"{{"version":8,"zoom":{zoom},"transition":{{"duration":{fade}}},
        "sources":{{"points":{{"type":"geojson","data":{{"type":"FeatureCollection","features":[{}]}}}}}},
        "layers":[{{"id":"background","type":"background","paint":{{"background-color":"#ffffff"}}}},{layer}]}}"##,
        features.join(",")
    )
}

/// Red squares of radius 6, their sides level, that collide with each
/// other, sized by a feature property.
const COLLIDING: &str = r##"{"id":"ngons","type":"org.maplibre.ngon","source":"points",
    "layout":{"ngon-allow-overlap":false,"ngon-ignore-placement":false},
    "paint":{"ngon-color":"#ff0000","ngon-radius":["get","r"],"ngon-corners":4,"ngon-rotate":45}}"##;

/// Zoom 0 draws 1.40625 degrees of longitude in 2 pixels.
const TWO_PIXELS: f64 = 1.406_25;

#[test]
fn colliding_ngons_fade_until_the_map_goes_idle() {
    // Squares 4 pixels apart: the first shows, and the second, which it
    // hides, would reach 8 pixels right of the center.
    let fixture = Fixture::new(registry(), &points_style(0.0, 300, &[0.0, 2.0 * TWO_PIXELS], COLLIDING));
    assert_eq!(RED, pixel(&fixture, 0));
    assert_eq!(WHITE, pixel(&fixture, 6));
    assert!(!fixture.render().needs_repaint, "the map placed again with nothing to change");
}

#[test]
fn placement_follows_the_zoom() {
    let fixture = Fixture::new(registry(), &points_style(0.0, 0, &[0.0, 2.0 * TWO_PIXELS], COLLIDING));
    assert_eq!(WHITE, pixel(&fixture, 6));

    // At zoom 2 the squares are 16 pixels apart, and both show.
    let zoom = |zoom| {
        let camera = mln_rs::mln_map_types::CameraOptions { zoom: Some(zoom), ..Default::default() };
        wait(fixture.map.update_camera(mln_rs::mln_map_types::CameraUpdate { camera, ..Default::default() }));
        fixture.render_loaded();
    };
    zoom(2.0);
    assert_eq!(RED, pixel(&fixture, 0));
    assert_eq!(RED, pixel(&fixture, 16));

    zoom(0.0);
    assert_eq!(RED, pixel(&fixture, 0));
    assert_eq!(WHITE, pixel(&fixture, 6));
}

#[test]
fn a_layer_that_doesnt_collide_draws_in_a_new_world_copy_at_once() {
    // A point at 170 degrees west, which the map shows only once it looks
    // across the antimeridian.
    let fixture = Fixture::new(registry(), &points_style(0.0, 0, &[-170.0], LAYER));
    assert_eq!(WHITE, pixel(&fixture, 16));

    // Centered at 179 degrees east, the map shows the point in the world
    // copy east of the antimeridian, 11 degrees or 16 pixels right of the
    // center, from the tile that it laid out west of it.
    let center = mln_rs::mln_util::geo::LatLng::new(0.0, 179.0).ok();
    let camera = mln_rs::mln_map_types::CameraOptions { center, ..Default::default() };
    wait(fixture.map.update_camera(mln_rs::mln_map_types::CameraUpdate { camera, ..Default::default() }));
    fixture.render();
    assert_eq!(RED, pixel(&fixture, 16));
}

/// Two n-gon layers whose filters are equal but parsed apart share a bucket,
/// and each draws and is found with its own paint: a blue square of radius 8
/// under a red one of radius 4.
#[test]
fn layers_with_equal_filters_draw_and_query_from_one_bucket() {
    const BLUE: [u8; 4] = [0, 0, 255, 255];
    let layer = |id: &str, color: &str, radius: u32| {
        format!(
            r#"{{"id":"{id}","type":"org.maplibre.ngon","source":"points",
            "filter":["==",["get","kind"],"stop"],
            "paint":{{"ngon-color":"{color}","ngon-radius":{radius},"ngon-corners":4,"ngon-rotate":45}}}}"#
        )
    };
    let (below, above) = (layer("below", "#0000ff", 8), layer("above", "#ff0000", 4));
    let style = format!(
        r##"{{"version":8,"transition":{{"duration":0}},
        "sources":{{"points":{{"type":"geojson","data":{{"type":"Feature","properties":{{"kind":"stop"}},
            "geometry":{{"type":"Point","coordinates":[0,0]}}}}}}}},
        "layers":[{{"id":"background","type":"background","paint":{{"background-color":"#ffffff"}}}},{below},{above}]}}"##
    );
    let fixture = Fixture::new(registry(), &style);
    assert_eq!(RED, pixel(&fixture, 0), "the layer above");
    assert_eq!(BLUE, pixel(&fixture, 4), "the layer below, past the one above");
    assert_eq!(WHITE, pixel(&fixture, 8));

    let found = |x: f64| {
        let center = mln_rs::mln_util::geo::ScreenCoordinate::new(f64::from(SIZE.width / 2) + x, 32.0);
        let geometry = mln_rs::RenderedQueryGeometry::Point(center);
        let features = wait(fixture.session.query_rendered_features(geometry, mln_rs::RenderedQueryOptions::default()));
        let mut layers: Vec<String> = features.into_iter().map(|feature| feature.layer).collect();
        layers.sort();
        layers
    };
    assert_eq!(found(0.0), ["above", "below"]);
    assert_eq!(found(4.0), ["below"]);
    assert_eq!(found(8.0), [] as [std::string::String; 0]);
}
