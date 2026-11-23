//! Symbol layouts with the glyphs and images they need, behind the worker
//! codec too, and their placement in a view, through the benchmarks' entry
//! points (`mln_render::bench`, `mln_layers::bench`).

use std::cell::RefCell;
use std::sync::Arc;

use feature::{Feature, Identifier, PropertyMap, Value};
use geo_types::{Geometry, LineString, Point};
use mln_layers::bench::{SymbolIndex, SymbolView};
use mln_render::bench::{Codec, TileLayout};
use mln_style::{Edit, Style};
use mln_style_types::image::Impl as ImageImpl;
use mln_tile::TileData;
use mln_util::geo::LatLng;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

mod support;
use support::builtin_registry;

/// A style of a layer of labels, one of labels and icons whose text color
/// a format section overrides, and the icon `marker`, over source `s`.
fn style() -> Style {
    let mut style = style_without_images();
    let mut pixels = PremultipliedImage::new(Size::new(4, 4));
    pixels.data.fill(255);
    let marker = ImageImpl::new("marker".to_owned(), pixels, 1.0, false, Vec::new(), Vec::new(), None, None, None);
    style.apply(Edit::AddImage(Arc::new(marker.unwrap()))).unwrap();
    style
}

/// The layers of [`style`] without the icon, which they draw none of.
fn style_without_images() -> Style {
    let json = r#"{"version": 8, "glyphs": "local://glyphs/{fontstack}/{range}.pbf",
        "sources": {"s": {"type": "vector", "tiles": ["local://{z}-{x}-{y}.mvt"]}},
        "layers": [
            {"id": "names", "type": "symbol", "source": "s", "source-layer": "l",
             "layout": {"text-field": ["get", "name"], "text-font": ["Test"]}},
            {"id": "markers", "type": "symbol", "source": "s", "source-layer": "l",
             "layout": {"text-field": ["format", ["get", "name"], {"text-color": "red"}], "text-font": ["Test"],
                "icon-image": "marker", "text-allow-overlap": true, "icon-allow-overlap": true},
             "paint": {"text-color": "blue"}}
        ]}"#;
    Style::from_json(builtin_registry().style(), json, None).unwrap()
}

fn feature(id: u64, geometry: Geometry<i16>) -> Feature<i16, Arc<PropertyMap>> {
    let mut properties = PropertyMap::new();
    properties.insert("name".to_owned(), Value::String("ab".to_owned()));
    Feature { geometry: Some(geometry), properties: Arc::new(properties), id: Identifier::Uint(id) }
}

/// A line and a point, both named.
fn data() -> TileData {
    TileData::Features(Arc::new(vec![
        feature(1, Geometry::LineString(LineString::from(vec![(100, 100), (4000, 4000)]))),
        feature(2, Geometry::Point(Point::new(2000, 1000))),
    ]))
}

fn glyphs() -> Vec<u8> {
    let path = std::path::Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("testdata/test/fixtures/resources/glyphs.pbf");
    std::fs::read(path).unwrap()
}

#[test]
fn lays_out_with_the_glyphs_it_loads() {
    let (style, data, glyphs) = (style(), data(), glyphs());
    let requested = RefCell::new(Vec::new());
    let load = |url: &str| {
        requested.borrow_mut().push(url.to_owned());
        Some(glyphs.clone())
    };
    let layout = TileLayout::new(&builtin_registry(), &style, "s", |_| true, (0, 0, 0), &data, load).unwrap();
    assert_eq!(*requested.borrow(), ["local://glyphs/Test/0-255.pbf"]);
    assert_eq!(layout.run(data).unwrap().buckets(), 2);
}

/// The buckets of a job behind the codec, the second time too, when the
/// worker has the style's values, are those of the job run directly: with
/// the text and the icon of the glyphs and the image they were laid out
/// with, and the text color that
/// the format section overrides, which the layer's values and the buckets'
/// binders carry.
#[test]
fn layouts_cross_with_their_glyphs_and_images() {
    let (style, data, glyphs) = (style(), data(), glyphs());
    let layout =
        TileLayout::new(&builtin_registry(), &style, "s", |_| true, (0, 0, 0), &data, |_| Some(glyphs.clone()))
            .unwrap();
    let direct = layout.run(data.clone()).unwrap();
    let without_glyphs =
        TileLayout::new(&builtin_registry(), &style, "s", |_| true, (0, 0, 0), &data, |_| None).unwrap();
    assert_ne!(
        without_glyphs.run(data.clone()).unwrap().encoded().unwrap(),
        direct.encoded().unwrap(),
        "the layout draws the text of the glyphs it loads",
    );
    let without_images = style_without_images();
    let without_icons = TileLayout::new(
        &builtin_registry(),
        &without_images,
        "s",
        |_| true,
        (0, 0, 0),
        &data,
        |_| Some(glyphs.clone()),
    )
    .unwrap();
    assert_ne!(
        without_icons.run(data.clone()).unwrap().encoded().unwrap(),
        direct.encoded().unwrap(),
        "the layout draws the icon of the style's image",
    );
    let mut codec = Codec::new(builtin_registry());
    for _ in 0..2 {
        let reply = codec.send(&layout, data.clone()).unwrap().run();
        assert_eq!(codec.reply(&reply).unwrap().encoded().unwrap(), direct.encoded().unwrap());
    }
}

#[test]
fn places_the_symbols_it_indexes() {
    let (style, data, glyphs) = (style(), data(), glyphs());
    let names = |zoom| {
        let layout = TileLayout::overscaled(
            &builtin_registry(),
            &style,
            "s",
            |type_| type_ == "symbol",
            (0, 0, 0),
            zoom,
            &data,
            |_| Some(glyphs.clone()),
        )
        .unwrap();
        layout.run(data.clone()).unwrap()
    };
    let (before, after) = (names(0), names(1));
    let center = LatLng::new(0.0, 0.0).unwrap();
    let mut zoomed_out = SymbolView::new(&style, "s", &[&before], center, 0.0, (512, 512)).unwrap();
    let mut zoomed_in = SymbolView::new(&style, "s", &[&after], center, 1.0, (1024, 1024)).unwrap();

    // Unindexed symbols show nowhere.
    assert_eq!(zoomed_in.place(), 0);
    let mut index = SymbolIndex::default();
    assert_eq!(zoomed_out.index(&mut index), 2);
    assert_eq!(zoomed_in.index(&mut index), 2);
    // Indexing a view again adds nothing.
    assert_eq!(zoomed_in.index(&mut index), 0);
    assert_eq!(zoomed_in.place(), 2);
}
