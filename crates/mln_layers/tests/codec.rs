//! The layer types' layouts behind the worker codec, as the browser build's
//! tile workers run them: a tile's job crosses to a worker instance, its
//! buckets come back, and they are the buckets of a job run directly.

use std::sync::Arc;

use feature::{Feature, Identifier, PropertyMap, Value};
use geo_types::{Geometry, LineString, Point, Polygon};
use mln_render::bench::{Codec, TileLayout};
use mln_style::layer_type::SourceKind;
use mln_style::{Edit, Style};
use mln_style_types::image::Impl as ImageImpl;
use mln_tile::TileData;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

mod support;
use support::builtin_registry;

/// A layer of each geometry layer type of this crate, with data-driven,
/// zoom-dependent and pattern values, over source `s`.
const LAYERS: &str = r#"[
    {"id": "water", "type": "fill", "source": "s",
     "paint": {"fill-color": ["case", ["has", "class"], "red", "rgba(0, 0, 255, 0.678)"], "fill-opacity": 0.5}},
    {"id": "water-legacy", "type": "fill", "source": "s",
     "paint": {"fill-color": {"property": "class", "type": "categorical", "stops": [["ocean", "red"]]}}},
    {"id": "coast", "type": "line", "source": "s", "layout": {"line-join": "round"},
     "paint": {"line-width": ["interpolate", ["linear"], ["zoom"], 0, ["get", "w"], 4, 2], "line-color": "red"}},
    {"id": "circles", "type": "circle", "source": "s",
     "paint": {"circle-radius": ["interpolate", ["linear"], ["zoom"], 0, ["get", "w"], 10, 4],
        "circle-color": ["case", ["has", "class"], "blue", "red"], "circle-stroke-width": 1}},
    {"id": "heat", "type": "heatmap", "source": "s", "paint": {"heatmap-weight": ["get", "w"], "heatmap-radius": 10}},
    {"id": "extrusion", "type": "fill-extrusion", "source": "s",
     "paint": {"fill-extrusion-height": ["case", ["has", "class"], 10, 20], "fill-extrusion-color": "red"}},
    {"id": "fill-pattern", "type": "fill", "source": "s", "paint": {"fill-pattern": "grass"}},
    {"id": "line-pattern", "type": "line", "source": "s",
     "paint": {"line-width": 4, "line-pattern": {"stops": [[0, "grass"]], "type": "interval"}}},
    {"id": "extrusion-pattern", "type": "fill-extrusion", "source": "s",
     "paint": {"fill-extrusion-height": 10, "fill-extrusion-pattern": ["coalesce", ["get", "pattern"], "grass"]}}
]"#;

/// The style of [`LAYERS`], with the image `grass` that the patterns draw.
/// One image: the atlas packs a layout's images in the order of a
/// `HashMap`, which differs between two workers, so with two images their
/// positions (and the binders' vertices) could differ between the worker
/// behind the codec and the one run directly.
fn style() -> Style {
    let mut style = style_without_images();
    let mut pixels = PremultipliedImage::new(Size::new(4, 4));
    pixels.data.fill(255);
    let grass = ImageImpl::new("grass".to_owned(), pixels, 1.0, false, Vec::new(), Vec::new(), None, None, None);
    style.apply(Edit::AddImage(Arc::new(grass.unwrap()))).unwrap();
    style
}

/// The style of [`LAYERS`] without images, whose patterns draw nothing.
fn style_without_images() -> Style {
    let json = format!(
        r#"{{"version": 8, "sources": {{"s": {{"type": "vector", "tiles": ["local://{{z}}-{{x}}-{{y}}.mvt"]}}}},
        "layers": {LAYERS}}}"#
    );
    Style::from_json(builtin_registry().style(), &json, None).unwrap()
}

/// A polygon, a line and a point, each with properties that the layers read.
fn data() -> TileData {
    let feature = |id, geometry, class: Option<&str>| {
        let mut properties = PropertyMap::from_iter([("w".to_owned(), Value::Double(3.0))]);
        if let Some(class) = class {
            properties.insert("class".to_owned(), Value::String(class.to_owned()));
        }
        Feature { geometry: Some(geometry), properties: Arc::new(properties), id: Identifier::Uint(id) }
    };
    let square = LineString::from(vec![(100, 100), (2000, 100), (2000, 2000), (100, 2000), (100, 100)]);
    TileData::Features(Arc::new(vec![
        feature(1, Geometry::Polygon(Polygon::new(square, vec![])), Some("ocean")),
        feature(2, Geometry::LineString(LineString::from(vec![(100, 3000), (4000, 3000)])), None),
        feature(3, Geometry::Point(Point::new(3000, 1000)), Some("lake")),
    ]))
}

/// Every layer of [`LAYERS`] lays out, and each of this crate's layer types
/// that takes geometry tiles, but `symbol`, whose layouts need glyphs
/// (tests/symbols.rs), has a layer there. A layout of some layer types lays
/// out only their layers.
#[test]
fn every_geometry_layer_type_lays_out() {
    let (style, data) = (style(), data());
    for layer_type in builtin_registry().style().layer_types() {
        let type_ = layer_type.info().name;
        let needs_layer = layer_type.info().source == SourceKind::Geometry && type_ != "symbol";
        let has_layer = style.get_layers().any(|layer| layer.info().name == type_);
        assert_eq!(needs_layer, has_layer, "{type_} layers need a layer in LAYERS");
    }
    let layout = TileLayout::new(&builtin_registry(), &style, "s", |_| true, (0, 0, 0), &data, |_| None).unwrap();
    assert_eq!(layout.run(data.clone()).unwrap().buckets(), style.get_layers().len());
    let fills = TileLayout::new(&builtin_registry(), &style, "s", |type_| type_ == "fill", (0, 0, 0), &data, |_| None);
    let fill_layers = style.get_layers().filter(|layer| layer.info().name == "fill").count();
    assert_eq!(fills.unwrap().run(data).unwrap().buckets(), fill_layers);
}

/// The buckets of a job behind the codec, the second time too, when the
/// worker has the style's values, are those of the job run directly, with
/// the patterns of the style's image.
#[test]
fn layouts_cross_as_they_were_laid_out() {
    let (style, data) = (style(), data());
    let layout = TileLayout::new(&builtin_registry(), &style, "s", |_| true, (0, 0, 0), &data, |_| None).unwrap();
    let direct = layout.run(data.clone()).unwrap();
    let without_images = style_without_images();
    let without_patterns =
        TileLayout::new(&builtin_registry(), &without_images, "s", |_| true, (0, 0, 0), &data, |_| None).unwrap();
    assert_ne!(
        without_patterns.run(data.clone()).unwrap().encoded().unwrap(),
        direct.encoded().unwrap(),
        "the layout draws the patterns of the style's image",
    );
    let mut codec = Codec::new(builtin_registry());
    for _ in 0..2 {
        let reply = codec.send(&layout, data.clone()).unwrap().run();
        assert_eq!(codec.reply(&reply).unwrap().encoded().unwrap(), direct.encoded().unwrap());
    }
}
