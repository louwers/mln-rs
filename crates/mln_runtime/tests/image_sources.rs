//! Image sources: where they draw, and those without an image yet.

use mln_engine::camera::CameraOptions;
use mln_engine::storage::Kind;
use mln_runtime::RuntimeEventKind;
use mln_style::bitmap::PremultipliedImage;
use mln_style::size::Size;

use crate::support::still::{StillMap, assert_near, center_pixel, decode_image, file_response, ll, read_file, serve};
use crate::support::{EMPTY_STYLE, eventually, wait};

fn add_raster_layer(map: &StillMap, id: &str, source: &str) {
    let layer = format!(r#"{{"id": "{id}", "type": "raster", "source": "{source}"}}"#);
    wait(map.map.add_style_layer_json(layer, None));
}

/// A raster layer over an image source whose corners are all one point
/// draws (mapbox/mapbox-gl-native#15216).
#[test]
fn issue15216() {
    let map = StillMap::continuous();
    map.load_json(EMPTY_STYLE);
    let source = r#"{"type": "image", "coordinates": [[0, 0], [0, 0], [0, 0], [0, 0]]}"#;
    wait(map.map.add_style_source_json("ImageSource", source));
    add_raster_layer(&map, "RasterLayer", "ImageSource");
    map.frame();
}

/// A still image of the globe below zoom 0 with an image source finishes: the
/// source's tile cover at zoom -1.5 was cast to an unsigned tile zoom, and the
/// render never returned.
#[test]
fn globe_image_source_below_zoom_zero() {
    let map = StillMap::still();
    map.load_json(&read_file("test/fixtures/api/empty.json"));
    wait(map.map.set_style_root_property("projection", r#"{"type": "vertical-perspective"}"#));
    let coordinates = [ll(60.0, -10.0), ll(60.0, 10.0), ll(50.0, 10.0), ll(50.0, -10.0)];
    wait(map.map.add_image_source("image", coordinates, decode_image("test/fixtures/image/no_profile.png")));
    add_raster_layer(&map, "raster", "image");
    // The camera takes the style's projection at the map's next update: on
    // the globe, a zoom below 0 stays.
    map.render();
    assert!(map.map.snapshot().unwrap().globe_rendering);
    map.jump(CameraOptions { center: Some(ll(80.0, 0.0)), zoom: Some(-1.5), ..Default::default() });
    let camera = CameraOptions::from(wait(map.map.camera()).camera);
    assert_near(-1.5, camera.zoom.unwrap(), 1e-9);
    assert_eq!(256, map.render().size.width);
}

/// A frame with an image source wholly beyond the latitudes Web Mercator can
/// show, whose tile cover is empty, draws (maplibre/maplibre-native#4753).
#[test]
fn image_source_beyond_mercator_latitudes() {
    let map = StillMap::continuous();
    map.load_json(&read_file("test/fixtures/api/empty.json"));
    let coordinates = [ll(88.0, -10.0), ll(88.0, 10.0), ll(86.0, 10.0), ll(86.0, -10.0)];
    wait(map.map.add_image_source("image", coordinates, PremultipliedImage::new(Size::new(4, 4))));
    add_raster_layer(&map, "image", "image");
    map.frame();
}

/// An image source without a url starts empty: it requests nothing and draws
/// nothing until it gets an image or a url.
#[test]
fn an_image_source_without_a_url_draws_once_it_has_an_image() {
    let map = StillMap::still();
    let requests = serve(&map.runtime, |_| file_response("test/fixtures/image/no_profile.png"));
    map.load_json(
        r#"{
      "version": 8,
      "sources": {
        "image": {"type": "image", "coordinates": [[-180, 80], [180, 80], [180, -80], [-180, -80]]}
      },
      "layers": [{"id": "image", "type": "raster", "source": "image"}]
    }"#,
    );
    assert!(wait(map.map.style_source("image")).is_some());

    assert_eq!(center_pixel(&map.render()), [0, 0, 0, 0]);
    assert_eq!(requests.len(), 0);

    let red = PremultipliedImage::from_vec(Size::new(1, 1), vec![255, 0, 0, 255]);
    wait(map.map.set_image_source_image("image", red));
    assert_eq!(center_pixel(&map.render()), [255, 0, 0, 255]);

    wait(map.map.set_image_source_url("image", "fixture://image.png"));
    assert!(eventually(|| requests.len() == 1), "{:?}", requests.all());
    assert_eq!(requests.urls(Kind::Image), ["fixture://image.png"]);
    let errors: Vec<_> = map
        .events
        .events()
        .into_iter()
        .filter(|event| matches!(&event.kind, RuntimeEventKind::Map(event) if format!("{event:?}").contains("Error")))
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
}
