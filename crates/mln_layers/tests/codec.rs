//! The layer types' layouts behind the worker codec, as the browser build's
//! tile workers run them: a tile's job crosses to a worker instance, its
//! buckets come back, and they are the buckets of a job run directly.

use std::sync::Arc;

use feature::{Feature, Identifier, PropertyMap, Value};
use geo_types::{Geometry, LineString, Point, Polygon};
use mln_renderer::bench::{Codec, TileLayout};
use mln_rs::builtin_registry;
use mln_style::layer_type::SourceKind;
use mln_style::{Edit, Style};
use mln_style_types::image::Impl as ImageImpl;
use mln_tile::TileData;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

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
    let json = format!(
        r#"{{"version": 8, "sources": {{"s": {{"type": "vector", "tiles": ["local://{{z}}-{{x}}-{{y}}.mvt"]}}}},
        "layers": {LAYERS}}}"#
    );
    let mut style = Style::from_json(builtin_registry().style(), &json, None).unwrap();
    let mut pixels = PremultipliedImage::new(Size::new(4, 4));
    pixels.data.fill(255);
    let grass = ImageImpl::new("grass".to_owned(), pixels, 1.0, false, Vec::new(), Vec::new(), None, None, None);
    style.apply(Edit::AddImage(Arc::new(grass.unwrap()))).unwrap();
    style
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
/// that takes geometry tiles has a layer there.
#[test]
fn every_geometry_layer_type_lays_out() {
    let (style, data) = (style(), data());
    for type_ in ["background", "circle", "fill", "fill-extrusion", "heatmap", "line"] {
        let layer_type = builtin_registry().style().layer_type(type_).unwrap();
        let takes_geometry_tiles = layer_type.info().source == SourceKind::Geometry;
        let has_layer = style.get_layers().any(|layer| layer.info().name == type_);
        assert_eq!(takes_geometry_tiles, has_layer, "{type_} layers need a layer in LAYERS");
    }
    let layout = TileLayout::new(&builtin_registry(), &style, "s", |_| true, (0, 0, 0), &data, |_| None).unwrap();
    assert_eq!(layout.run(data).unwrap().buckets(), style.get_layers().len());
}

/// The buckets of a job behind the codec, the second time too, when the
/// worker has the style's values, are those of the job run directly.
#[test]
fn layouts_cross_as_they_were_laid_out() {
    let (style, data) = (style(), data());
    let layout = TileLayout::new(&builtin_registry(), &style, "s", |_| true, (0, 0, 0), &data, |_| None).unwrap();
    let direct = layout.run(data.clone()).unwrap();
    let mut codec = Codec::new(builtin_registry());
    for _ in 0..2 {
        let reply = codec.send(&layout, data.clone()).unwrap().run();
        assert_eq!(codec.reply(&reply).unwrap().encoded().unwrap(), direct.encoded().unwrap());
    }
}
