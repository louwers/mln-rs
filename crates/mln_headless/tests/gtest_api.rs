use std::sync::Arc;

use geojson::{Feature, GeoJson, Geometry};
use mln_engine::Registry;
use mln_engine::camera::CameraOptions;
use mln_engine::exec::pool::Pool;
use mln_engine::map::{Map, MapMode, MapOptions};
use mln_engine::storage::ResourceOptions;
use mln_engine::storage::stub_file_source::{ResponseType, StubFileSource};
use mln_headless::HeadlessFrontend;
use mln_style::Edit;
use mln_style::conversion::Convertible;
use mln_style::geo::LatLng;
use mln_style::layer::Layer;
use mln_style::sources::geojson::GeoJSONOptions;
use serde_json::json;

use crate::support::*;

#[must_use]
pub(crate) fn style_image(
    id: &str,
    image: mln_style::bitmap::PremultipliedImage,
    pixel_ratio: f32,
) -> Arc<mln_style::image::Impl> {
    Arc::new(
        mln_style::image::Impl::new(id.to_owned(), image, pixel_ratio, false, Vec::new(), Vec::new(), None, None, None)
            .expect("style::Image"),
    )
}

/// Sets `layer`'s property `name` from style JSON.
#[track_caller]
pub(crate) fn set(layer: &mut Layer, name: &str, value: &serde_json::Value) {
    let error = layer.set_property(name, &Convertible::from(value), None);
    assert!(error.is_none(), "{name}: {error:?}");
}

// gtest: CustomGeometrySource.Grid
#[test]
fn grid() {
    let run_loop = TestLoop::new();
    let pool = Pool::new(4);

    let mut frontend = HeadlessFrontend::with_pixel_ratio(1.0, run_loop.queue(), &pool).expect("a headless frontend");
    let mut map = Map::with_resource_options(
        MapOptions { map_mode: MapMode::Static, size: frontend.size(), ..Default::default() },
        &ResourceOptions {
            asset_path: cpp_root().join("test/fixtures/api/assets").to_str().unwrap().to_owned(),
            ..ResourceOptions::default()
        },
        &pool,
    )
    .unwrap();
    frontend.attach(&mut map);
    map.load_style_json(&read_file("test/fixtures/api/water.json"));
    map.jump_to(&CameraOptions {
        center: Some(LatLng::new(37.8, -122.5).unwrap()),
        zoom: Some(10.0),
        ..Default::default()
    });

    // C++ answers a custom geometry source with each tile's grid lines; a
    // GeoJSON source of the lines around the view draws them alike.
    let grid_spacing = 0.1;
    let (west, east, south, north) = (-123.5, -121.5, 37.0, 38.6);
    let mut features = Vec::new();
    let mut y = south;
    while y <= north {
        features.push(Feature::from(Geometry::new_line_string([[west, y], [east, y]])));
        y += grid_spacing;
    }
    let mut x = west;
    while x <= east {
        features.push(Feature::from(Geometry::new_line_string([[x, south], [x, north]])));
        x += grid_spacing;
    }
    let data = serde_json::to_value(GeoJson::from(features)).unwrap();
    map.add_source_json("custom", &json!({ "type": "geojson", "data": data }).to_string()).unwrap();

    let mut fill_layer = Layer::new(Registry::with_builtins().style(), "fill", "landcover", "mapbox").unwrap();
    fill_layer.set_source_layer("landcover");
    set(&mut fill_layer, "fill-color", &json!("rgba(255,255,0,1)"));
    map.edit_style(|style| style.apply(Edit::AddLayer { layer: fill_layer, before: None })).unwrap();

    let mut layer = Layer::new(Registry::with_builtins().style(), "line", "grid", "custom").unwrap();
    set(&mut layer, "line-color", &json!("rgba(255,255,255,1)"));
    map.edit_style(|style| style.apply(Edit::AddLayer { layer, before: None })).unwrap();

    check_image(
        "test/fixtures/custom_geometry_source/grid",
        &frontend.render(&mut map).expect("render").image,
        0.0006,
        0.1,
    );
}

// gtest: API.RecycleMapUpdateImages
#[test]
fn recycle_map_update_images() {
    let run_loop = TestLoop::new();
    let pool = Pool::new(4);

    let mut frontend = HeadlessFrontend::with_pixel_ratio(1.0, run_loop.queue(), &pool).expect("a headless frontend");
    let mut map = Map::with_file_source(
        StubFileSource::on_queue(run_loop.queue(), ResponseType::Asynchronous),
        MapOptions { map_mode: MapMode::Static, size: frontend.size(), ..Default::default() },
        &pool,
    )
    .unwrap();
    frontend.attach(&mut map);

    let load_style = |map: &mut Map, marker_name: &str, marker_path: &str| {
        let source = geojson_source_with_data(
            "geometry",
            GeoJSONOptions::default_options(),
            GeoJson::from(Geometry::new_point([0.0, 0.0])),
        );

        let mut layer = Layer::new(Registry::with_builtins().style(), "symbol", "geometry", "geometry").unwrap();
        set(&mut layer, "icon-image", &json!(marker_name));

        map.load_style_json(&read_file("test/fixtures/api/empty.json"));
        map.add_source(source).unwrap();
        map.edit_style(|style| style.apply(Edit::AddLayer { layer, before: None })).unwrap();
        map.edit_style(|style| style.apply(Edit::AddImage(style_image(marker_name, decode_image(marker_path), 1.0))))
            .unwrap();
    };

    // default marker

    load_style(&mut map, "default_marker", "test/fixtures/sprites/default_marker.png");
    check_image(
        "test/fixtures/recycle_map/default_marker",
        &frontend.render(&mut map).expect("render").image,
        0.0006,
        0.1,
    );

    // flipped marker

    load_style(&mut map, "flipped_marker", "test/fixtures/sprites/flipped_marker.png");
    check_image(
        "test/fixtures/recycle_map/flipped_marker",
        &frontend.render(&mut map).expect("render").image,
        0.0006,
        0.1,
    );
}
