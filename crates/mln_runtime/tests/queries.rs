//! The features that queries find: rendered features at a point, and a
//! source's features, through filters, global state and zoom.

use std::sync::Arc;

use mln_engine::camera::CameraOptions;
use mln_engine::geo::ScreenCoordinate;
use mln_engine::storage::Response;
use mln_engine::tile::feature::Feature;
use mln_runtime::{RenderedQueryOptions, SourceQueryOptions};
use mln_style::bitmap::PremultipliedImage;
use mln_style::geo::LatLng;

use crate::support::still::{StillMap, decode_image, read_file, serve};
use crate::support::wait;

fn ll(lat: f64, lon: f64) -> LatLng {
    LatLng::new(lat, lon).expect("LatLng")
}

/// Rendered queries of the layers `ids`.
fn in_layers(ids: &[&str], filter: Option<mln_style::filter::Filter>) -> RenderedQueryOptions {
    RenderedQueryOptions::new(Some(ids.iter().map(ToString::to_string).collect()), filter)
}

/// Where `map` draws `lat_lng` as of its last command.
fn pixel(map: &StillMap, lat_lng: LatLng) -> ScreenCoordinate {
    map.map.projection(None).unwrap().pixel_for_lat_lng(&lat_lng)
}

/// The alpha of `image` at `point`.
fn alpha_at(image: &PremultipliedImage, point: ScreenCoordinate) -> u8 {
    image.data[4 * (point.y as u32 * image.size.width + point.x as u32) as usize + 3]
}

/// A still map of `test/fixtures/api/query_style.json`, with its
/// "test-icon" image, drawn once: four icons at 0, 0 from the GeoJSON
/// sources "source1" to "source4", whose layers are "layer1" to "layer4".
fn query_map() -> StillMap {
    let map = StillMap::still();
    map.load_json(&read_file("test/fixtures/api/query_style.json"));
    let icon = decode_image("test/fixtures/sprites/default_marker.png");
    let icon = mln_style::image::Impl::new("test-icon".to_owned(), icon, 1.0, false, vec![], vec![], None, None, None);
    wait(map.map.set_style_image(icon.unwrap()));
    map.render();
    map
}

fn set_global_state(map: &StillMap, name: &str, value: &str) {
    wait(map.map.set_global_state_property(name, serde_json::to_string(value).unwrap()));
}

/// A still image with nothing to place finds no symbols: the icons of the
/// image before it are gone from its queries.
#[test]
fn a_still_image_with_nothing_to_place_finds_nothing_placed() {
    let map = query_map();
    let center = pixel(&map, ll(0.0, 0.0));
    assert_eq!(map.query_point(center.x, center.y, RenderedQueryOptions::default()).len(), 4);

    // Far from the sources' one point, so no tile in view has a symbol.
    map.jump(CameraOptions { center: Some(ll(40.0, 90.0)), zoom: Some(6.0), ..Default::default() });
    map.render();
    assert_eq!(map.query_point(center.x, center.y, RenderedQueryOptions::default()).len(), 0);
}

/// A change of the global state that a layer's filter reads lays the layer
/// out again.
#[test]
fn global_state_change_relayouts_source_filter() {
    let map = query_map();
    let filter = r#"["==", ["get", "key1"], ["global-state", "filterValue"]]"#;
    wait(map.map.set_style_layer_property("layer4", "filter", filter));
    let point = pixel(&map, ll(0.0, 0.0));
    let options = || in_layers(&["layer4"], None);

    set_global_state(&map, "filterValue", "value1");
    map.render();
    assert_eq!(1, map.query_point(point.x, point.y, options()).len());

    set_global_state(&map, "filterValue", "other");
    map.render();
    assert_eq!(map.query_point(point.x, point.y, options()), Vec::<Feature>::new());

    set_global_state(&map, "filterValue", "value1");
    map.render();
    assert_eq!(1, map.query_point(point.x, point.y, options()).len());
}

/// A query's filter reads the style's global state.
#[test]
fn query_filters_use_global_state() {
    let map = query_map();
    let filter = map.filter(r#"["==", ["get", "key1"], ["global-state", "queryValue"]]"#);
    let point = pixel(&map, ll(0.0, 0.0));
    let rendered = || in_layers(&["layer4"], Some(filter.clone()));
    let source = || SourceQueryOptions::new(None, Some(filter.clone()));

    set_global_state(&map, "queryValue", "value1");
    map.render();
    assert_eq!(1, map.query_point(point.x, point.y, rendered()).len());
    assert_eq!(1, map.query_source("source4", source()).len());

    set_global_state(&map, "queryValue", "other");
    map.render();
    assert_eq!(map.query_point(point.x, point.y, rendered()), Vec::<Feature>::new());
    assert_eq!(map.query_source("source4", source()), Vec::<Feature>::new());
}

/// A rendered query finds an offset line where it draws, to its end, and
/// not where it would be without the offset.
#[test]
fn query_rendered_features_line_offset_endpoint() {
    let map = StillMap::still();
    for offset in [22.0f64, -22.0] {
        map.load_json(&format!(
            r#"{{"version":8,"sources":{{"source":{{"type":"geojson","data":{{
                "type":"LineString","coordinates":[[1.0025,1],[1.0035,1]]
            }}}}}},"layers":[{{"id":"layer","type":"line","source":"source","paint":{{
                "line-width":6,"line-offset":{offset:.6}}}}}]}}"#
        ));
        map.jump(CameraOptions { center: Some(ll(1.0, 1.003)), zoom: Some(16.5), ..Default::default() });
        let image = map.render();
        let endpoint = pixel(&map, ll(1.0, 1.0035));
        // Inside the butt cap, away from antialiased edges.
        let inside = ScreenCoordinate::new(endpoint.x - 5.0, endpoint.y + offset);
        let outside = ScreenCoordinate::new(endpoint.x - 5.0, endpoint.y);
        assert!(alpha_at(&image, inside) > 0, "offset {offset}");
        assert_eq!(alpha_at(&image, outside), 0, "offset {offset}");
        assert_eq!(map.query_point(inside.x, inside.y, in_layers(&["layer"], None)).len(), 1, "offset {offset}");
        assert!(map.query_point(outside.x, outside.y, in_layers(&["layer"], None)).is_empty(), "offset {offset}");
    }
}

/// A rendered query evaluates a paint property that interpolates a
/// feature's values by zoom at the camera's zoom, as the map draws it, not
/// at the tile's.
#[test]
fn query_rendered_features_composite_paint_zoom() {
    // (property, inside, outside)
    let cases: [(&str, f64, f64); 4] = [
        ("line-width", 8.0, 14.0),
        ("line-gap-width", 14.0, 20.0),
        ("circle-radius", 15.0, 26.0),
        ("circle-stroke-width", 24.0, 32.0),
    ];

    let map = StillMap::still();
    // Fractional zooms below and above the default GeoJSON maxzoom, 18.
    for zoom in [16.5f64, 20.5] {
        for (property, inside_offset, outside_offset) in cases {
            let trace = format!("{property} at zoom {zoom}");
            let circle = property.starts_with("circle-");
            let geometry = if circle {
                r#"{"type":"Point","coordinates":[1.003,1]}"#
            } else {
                r#"{"type":"LineString","coordinates":[[0.993,1],[1.013,1]]}"#
            };
            let paint = if property == "circle-stroke-width" {
                r#""circle-radius":6,"#
            } else if circle || property == "line-width" {
                ""
            } else {
                r#""line-width":6,"#
            };
            let layer_type = if circle { "circle" } else { "line" };
            let lower_zoom = zoom as i32;
            let upper_zoom = zoom as i32 + 1;
            map.load_json(&format!(
                r#"{{"version":8,"sources":{{"source":{{"type":"geojson","data":{{
                    "type":"Feature","properties":{{"small":4,"large":40}},"geometry":{geometry}
                }}}}}},"layers":[{{"id":"layer","type":"{layer_type}","source":"source","paint":{{{paint}
                    "{property}":["interpolate",["linear"],["zoom"],{lower_zoom},["get","small"],
                    {upper_zoom},["get","large"]]}}}}]}}"#
            ));
            map.jump(CameraOptions { center: Some(ll(1.0, 1.003)), zoom: Some(zoom), ..Default::default() });
            let image = map.render();
            let center = pixel(&map, ll(1.0, 1.003));
            let inside = ScreenCoordinate::new(center.x, center.y + inside_offset);
            let outside = ScreenCoordinate::new(center.x, center.y + outside_offset);

            // At the camera's zoom the value is 22; at the tile's it is 4.
            // The painted pixels agree with the queries, away from
            // antialiased edges.
            assert!(alpha_at(&image, inside) > 0, "{trace}");
            assert_eq!(alpha_at(&image, outside), 0, "{trace}");
            assert_eq!(map.query_point(inside.x, inside.y, in_layers(&["layer"], None)).len(), 1, "{trace}");
            assert!(map.query_point(outside.x, outside.y, in_layers(&["layer"], None)).is_empty(), "{trace}");
        }
    }
}

/// Layer filters and query filters see the zoom of the tile that a feature
/// is laid out in, the integer zoom of an overscaled tile, for symbols
/// (through the collision index) and circles (through the geometry index).
#[test]
fn query_rendered_features_filter_uses_overscaled_zoom() {
    for camera_zoom in [16.5f64, 20.5] {
        let map = query_map();
        // Away from tile boundaries, so source queries return one copy.
        wait(map.map.set_geojson_source_data("source1", r#"{"type": "Point", "coordinates": [1.003, 1.003]}"#));
        wait(map.map.add_style_layer_json(r#"{"id": "circle_layer", "type": "circle", "source": "source1"}"#, None));
        let tile_zoom = camera_zoom as i32;
        let equal_zoom = format!(r#"["==", ["zoom"], {tile_zoom}]"#);
        let lower_zoom = map.filter(&format!(r#"["<", ["zoom"], {tile_zoom}]"#));
        let higher_zoom = map.filter(&format!(r#"[">", ["zoom"], {tile_zoom}]"#));

        for layer_id in ["layer1", "circle_layer"] {
            wait(map.map.set_style_layer_property(layer_id, "filter", &equal_zoom));
        }
        let equal_zoom = map.filter(&equal_zoom);
        map.jump(CameraOptions { center: Some(ll(1.003, 1.003)), zoom: Some(camera_zoom), ..Default::default() });
        map.render();
        let point = pixel(&map, ll(1.003, 1.003));
        for layer_id in ["layer1", "circle_layer"] {
            let trace = format!("{layer_id} at zoom {camera_zoom}");
            let query = |filter: Option<&mln_style::filter::Filter>| {
                map.query_point(point.x, point.y, in_layers(&[layer_id], filter.cloned())).len()
            };
            assert_eq!(query(None), 1, "{trace}");
            assert_eq!(query(Some(&equal_zoom)), 1, "{trace}");
            // Neither the canonical zoom of an overscaled tile nor the
            // camera's fractional zoom.
            assert_eq!(query(Some(&lower_zoom)), 0, "{trace}");
            assert_eq!(query(Some(&higher_zoom)), 0, "{trace}");
        }
        let options = SourceQueryOptions::new(None, Some(equal_zoom.clone()));
        assert_eq!(map.query_source("source1", options).len(), 1, "zoom {camera_zoom}");
    }
}

/// One MVT tile with two layers, "points" and "labels", each holding one
/// point feature at the tile's center: ID 1 in "points" and ID 2 in
/// "labels".
const TWO_LAYER_TILE: &[u8] = &[
    0x1a, 0x1a, 0x78, 0x02, 0x0a, 0x06, b'p', b'o', b'i', b'n', b't', b's', 0x12, 0x0b, 0x08, 0x01, 0x18, 0x01, 0x22,
    0x05, 0x09, 0x80, 0x20, 0x80, 0x20, 0x28, 0x80, 0x20, 0x1a, 0x1a, 0x78, 0x02, 0x0a, 0x06, b'l', b'a', b'b', b'e',
    b'l', b's', 0x12, 0x0b, 0x08, 0x02, 0x18, 0x01, 0x22, 0x05, 0x09, 0x80, 0x20, 0x80, 0x20, 0x28, 0x80, 0x20,
];

/// Source queries filter a source's features, need the source layers of a
/// vector source, read those they name, whether a layer draws them or not,
/// and find nothing in a raster source.
#[test]
fn query_source_features_source_layer() {
    let map = query_map();
    let query = |source: &str, filter: Option<&str>| {
        map.query_source(source, SourceQueryOptions::new(None, filter.map(|filter| map.filter(filter)))).len()
    };
    // A GeoJSON source needs no source layers.
    assert_eq!(query("source3", None), 1);
    // A vector source does.
    assert_eq!(query("source5", None), 0);
    // A raster source has no features.
    assert_eq!(query("source6", None), 0);
    assert_eq!(query("source4", Some(r#"["==", ["get", "key1"], "value1"]"#)), 1);
    assert_eq!(query("source4", Some(r#"["!=", ["id"], "feature1"]"#)), 0);
    assert_eq!(query("source4", Some(r#"[">", ["number", ["get", "key2"]], 1]"#)), 1);

    let map = StillMap::still();
    serve(&map.runtime, |_| Response { data: Some(Arc::new(TWO_LAYER_TILE.to_vec())), ..Response::default() });
    map.load_json(
        r#"{
          "version": 8,
          "sources": { "tiles": { "type": "vector", "tiles": ["http://example.com/{z}/{x}/{y}.pbf"], "maxzoom": 0 } },
          "layers": [{ "id": "dots", "type": "circle", "source": "tiles", "source-layer": "points" }]
        }"#,
    );
    map.render();

    let query = |source_layers: &[&str], filter: Option<&str>| -> Vec<(u64, String)> {
        let source_layers = Some(source_layers.iter().map(ToString::to_string).collect());
        let options = SourceQueryOptions::new(source_layers, filter.map(|filter| map.filter(filter)));
        let mut hits: Vec<(u64, String)> = map
            .query_source("tiles", options)
            .into_iter()
            .map(|feature| {
                assert_eq!(feature.layer, "");
                let mln_style::feature::Identifier::Uint(id) = feature.id else {
                    panic!("feature ID: {:?}", feature.id)
                };
                (id, feature.source_layer)
            })
            .collect();
        hits.sort_unstable();
        hits
    };
    assert_eq!(query(&["labels"], None), [(2, "labels".to_owned())]);
    assert_eq!(query(&["points", "labels"], None), [(1, "points".to_owned()), (2, "labels".to_owned())]);
    assert_eq!(query(&["points", "labels"], Some(r#"["==", ["id"], 1]"#)), [(1, "points".to_owned())]);
}
