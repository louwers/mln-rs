//! A map that loads one style after another draws each with what it added
//! to that style.

use serde_json::json;

use crate::support::still::{StillMap, check_image, decode_image, read_file};
use crate::support::wait;

/// The style image `id` of the image file at `path`.
fn style_image(id: &str, path: &str) -> mln_style::image::Impl {
    mln_style::image::Impl::new(id.to_owned(), decode_image(path), 1.0, false, Vec::new(), Vec::new(), None, None, None)
        .expect("a style image")
}

// gtest: API.RecycleMapUpdateImages
#[test]
fn recycle_map_update_images() {
    let map = StillMap::still();

    // Loads an empty style, and adds to it a point that shows the marker
    // `name` of the image file at `path`.
    let load_style = |name: &str, path: &str| {
        map.load_json(&read_file("test/fixtures/api/empty.json"));
        let source = json!({"type": "geojson", "data": {"type": "Point", "coordinates": [0, 0]}});
        wait(map.map.add_style_source_json("geometry", source.to_string()));
        let layer = json!({"id": "geometry", "type": "symbol", "source": "geometry", "layout": {"icon-image": name}});
        wait(map.map.add_style_layer_json(layer.to_string(), None));
        wait(map.map.set_style_image(style_image(name, path)));
    };

    load_style("default_marker", "test/fixtures/sprites/default_marker.png");
    check_image("test/fixtures/recycle_map/default_marker", &map.render(), 0.0006, 0.1);

    load_style("flipped_marker", "test/fixtures/sprites/flipped_marker.png");
    check_image("test/fixtures/recycle_map/flipped_marker", &map.render(), 0.0006, 0.1);
}
