// Ports test/api/{api_misuse.test.cpp, custom_geometry_source.test.cpp,
// custom_layer.test.cpp, recycle_map.cpp}. test/api/annotations.test.cpp,
// test/api/location_indicator.test.cpp and test/api/query.test.cpp are the
// modules below; the fixture is in support/mod.rs.

mod gtest_annotations;
mod gtest_location_indicator;
mod gtest_query;
mod support;

use std::rc::Rc;
use std::sync::Arc;

use geojson::{GeoJSON, Geometry, Point};
use mln_actor::RunLoop;
use mln_map::{Map, MapMode, MapOptions, NullMapObserver};
use mln_platform_default::HeadlessFrontend;
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_style::PropertyValue;
use mln_style::layers::symbol_layer::SymbolLayer;
use mln_style::sources::geojson_source::GeoJSONSource;
use support::*;

/// `std::make_unique<style::Image>(id, image, pixelRatio)`.
pub fn style_image(
    id: &str,
    image: mln_util::image::PremultipliedImage,
    pixel_ratio: f32,
) -> Arc<mln_style::image::Impl> {
    Arc::new(
        mln_style::image::Impl::new(id.to_owned(), image, pixel_ratio, false, Vec::new(), Vec::new(), None, None, None)
            .expect("style::Image"),
    )
}

// gtest: API.RenderWithoutCallback
#[test]
#[ignore = "not applicable as written: the Rust StillImageCallback is a Box<dyn FnOnce>, so there is no null callback to pass (C++ renderStill(nullptr) logs \"StillImageCallback not set\"); see api_misuse.test.cpp:16"]
fn render_without_callback() {
    // C++: map->renderStill(nullptr); map.reset(); expects one Error/General
    // log "StillImageCallback not set". The Rust type system rules the
    // misuse out; nothing to run.
}

// gtest: CustomGeometrySource.Grid
#[test]
#[ignore = "TODO(port): CustomGeometrySource, and AssetFileSource for test/fixtures/api/water.json's asset:// URLs; see custom_geometry_source.test.cpp:19"]
fn grid() {
    // test/api/custom_geometry_source.test.cpp:19: a CustomGeometrySource
    // whose fetchTileFunction sets 0.1-degree grid lines per tile, drawn by a
    // white line layer over the water style near San Francisco at z10
    // (test/fixtures/custom_geometry_source/grid, 0.0006, 0.1).
}

// gtest: CustomLayer.Basic
#[test]
#[ignore = "OpenGL-only in C++ (returns early on other backends; the CustomLayerHost draws with GL calls); also TODO(port): CustomLayer and AssetFileSource; see custom_layer.test.cpp:109"]
fn basic() {
    // test/api/custom_layer.test.cpp:109: a GL CustomLayer drawing a triangle
    // under a fill layer (test/fixtures/custom_layer/basic, 0.0006, 0.1).
}

// gtest: API.RecycleMapUpdateImages
#[test]
fn recycle_map_update_images() {
    let _loop = RunLoop::new();

    let mut frontend = HeadlessFrontend::with_pixel_ratio(1.0);
    let mut map = Map::with_file_source(
        &frontend,
        Rc::new(NullMapObserver),
        Arc::new(StubFileSource::new(ResponseType::Asynchronous)),
        MapOptions::new().with_map_mode(MapMode::Static).with_size(frontend.get_size()),
    );

    let load_style = |map: &mut Map, marker_name: &str, marker_path: &str| {
        let source = GeoJSONSource::with_default_options("geometry");
        source.set_geojson(&GeoJSON::Geometry(Geometry::Point(Point::new(0.0, 0.0)))).unwrap();

        let mut layer = SymbolLayer::new("geometry", "geometry");
        layer.set_icon_image(&PropertyValue::Constant(mln_style::expression::Image::new(marker_name)));

        map.get_style_mut().load_json(&read_file("test/fixtures/api/empty.json"));
        map.get_style_mut().add_source(Box::new(source)).unwrap();
        map.get_style_mut().add_layer(Box::new(layer), None).unwrap();
        map.get_style_mut().add_image(style_image(marker_name, decode_image(marker_path), 1.0));
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
