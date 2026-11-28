//! The images that a style's layers ask for: which stay in the style, and
//! which names count as no image.

use std::sync::Arc;

use mln_engine::map::MapEvent;
use mln_engine::storage::{Kind, Response};
use mln_runtime::RuntimeEventKind;
use mln_style::PropertyValue;
use mln_style::bitmap::PremultipliedImage;
use mln_style::expression::{Formatted, FormattedSection, Functions, Image};
use mln_style::layer::{Layer, LayerStyle};
use mln_style::layers::symbol_layer::Symbol;
use mln_style::size::Size;

use crate::support::still::{StillMap, not_found, serve};
use crate::support::wait;

fn red_image(width: u32, height: u32) -> PremultipliedImage {
    PremultipliedImage::from_vec(Size::new(width, height), [255, 0, 0, 255].repeat((width * height) as usize))
}

fn style_image(id: &str, image: PremultipliedImage) -> mln_style::image::Impl {
    mln_style::image::Impl::new(id.to_owned(), image, 1.0, false, Vec::new(), Vec::new(), None, None, None)
        .expect("a style image")
}

/// Once the images the map was given for missing-image events take more than
/// the cache limit, rendering removes those no layer uses anymore and keeps
/// those still in use.
#[test]
fn rendering_trims_unused_requested_images_past_the_cache_limit() {
    let map = StillMap::still();
    map.load_json(
        r#"{
      "version": 8,
      "sources": {
        "points": {"type": "geojson", "data": {"type": "Feature", "properties": {},
          "geometry": {"type": "Point", "coordinates": [0, 0]}}}
      },
      "layers": [
        {"id": "used", "type": "symbol", "source": "points", "layout": {"icon-image": "used-icon"}},
        {"id": "unused", "type": "symbol", "source": "points", "layout": {"icon-image": "unused-icon"}}
      ]
    }"#,
    );
    map.render();
    // Together more than the limit of 819200 bytes.
    wait(map.map.set_style_image(style_image("used-icon", red_image(330, 330))));
    wait(map.map.set_style_image(style_image("unused-icon", red_image(330, 330))));
    map.render();
    let images = || ["used-icon", "unused-icon"].map(|id| wait(map.map.style_image(id)).is_some());
    assert_eq!(images(), [true, true]);

    wait(map.map.set_style_layer_property("unused", "icon-image", r#""used-icon""#));
    map.render();
    assert_eq!(images(), [true, false]);
}

/// An image section with an empty name is no image: the map reports no
/// missing image for it, nor for an empty `fill-pattern` or `icon-image`,
/// only for the image of a name that the style lacks.
#[test]
fn empty_image_name_is_no_image() {
    let map = StillMap::still();
    serve(&map.runtime, |resource| match resource.kind {
        Kind::Glyphs => Response { no_content: true, ..Response::default() },
        _ => not_found(),
    });
    map.load_json(
        r#"{
      "version": 8,
      "sources": {
        "polygon": {"type": "geojson", "data": {"type": "Feature", "properties": {"image": ""},
          "geometry": {"type": "Polygon", "coordinates": [[[-10, -10], [10, -10], [10, 10], [-10, 10], [-10, -10]]]}}}
      },
      "layers": [
        {"id": "fill", "type": "fill", "source": "polygon", "paint": {"fill-pattern": ["get", "image"]}},
        {"id": "symbol", "type": "symbol", "source": "polygon", "layout": {"icon-image": ["get", "image"]}},
        {"id": "absent", "type": "symbol", "source": "polygon", "layout": {"icon-image": "absent"}}
      ]
    }"#,
    );
    // A `format` expression drops an empty image section, so only a layer
    // made in code can have one.
    let mut text = LayerStyle::<Symbol>::new("text".to_owned(), "polygon".to_owned());
    text.layout.text_field = PropertyValue::Constant(Formatted::new(vec![
        FormattedSection::new("a".to_owned(), None, None, None),
        FormattedSection::from_image(Image::new("")),
    ]));
    wait(map.map.add_style_layer(Layer::from_style(Arc::new(text), Functions::with_builtins()), None));
    map.render();

    let missing: Vec<String> = map
        .events
        .events()
        .into_iter()
        .filter_map(|event| match event.kind {
            RuntimeEventKind::Map(MapEvent::StyleImageMissing { id }) => Some(id),
            _ => None,
        })
        .collect();
    assert_eq!(missing, ["absent"]);
}
