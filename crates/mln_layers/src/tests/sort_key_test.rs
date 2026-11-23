use std::sync::Arc;

use feature::{Feature, Identifier, PropertyMap, Value};
use geo_types::{Geometry, Point};
use mln_render::bench::{PlacementFrame, TileLayout};
use mln_style::{Edit, Style};
use mln_style_types::image::Impl as ImageImpl;
use mln_tile::TileData;
use mln_util::geo::LatLng;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
use serde_json::json;

use crate::symbol::SymbolLayer;
use crate::symbol::bucket::{SortKeyRange, SymbolBucket};

#[derive(Clone, Copy, Debug, PartialEq)]
enum SortKeyType {
    Undefined,
    Constant,
    Camera,
    DataDriven,
}

const SORT_KEY_TYPES: [SortKeyType; 4] =
    [SortKeyType::Undefined, SortKeyType::Constant, SortKeyType::Camera, SortKeyType::DataDriven];

fn sort_key(type_: SortKeyType) -> Option<serde_json::Value> {
    match type_ {
        SortKeyType::Undefined => None,
        SortKeyType::Constant => Some(json!(5)),
        SortKeyType::Camera => Some(json!(["step", ["zoom"], 5, 10, 10])),
        SortKeyType::DataDriven => Some(json!(["number", ["get", "rank"]])),
    }
}

fn expected_order(type_: SortKeyType) -> Vec<usize> {
    match type_ {
        SortKeyType::Undefined => vec![0, 1, 2],
        // Preserve the existing tie order of explicitly specified sort keys.
        SortKeyType::Constant | SortKeyType::Camera => vec![2, 1, 0],
        SortKeyType::DataDriven => vec![1, 2, 0],
    }
}

/// Runs `check` with the bucket of a symbol layer of icons `marker` with
/// `layout`, and `symbol-sort-key` the sort key of `type_`, over three
/// points with ranks 2, 0 and 1 at x = 100, 200 and 300, and y 300, 200 and
/// 100.
fn with_bucket(mut layout: serde_json::Value, type_: SortKeyType, check: impl FnOnce(&SymbolBucket)) {
    layout["icon-image"] = json!("marker");
    layout["icon-allow-overlap"] = json!(true);
    if let Some(sort_key) = sort_key(type_) {
        layout["symbol-sort-key"] = sort_key;
    }
    let json = json!({
        "version": 8,
        "sources": {"s": {"type": "vector", "tiles": ["local://{z}-{x}-{y}.mvt"]}},
        "layers": [{"id": "symbol", "type": "symbol", "source": "s", "source-layer": "", "layout": layout}],
    });
    let mut style = Style::from_json(super::registry().style(), &json.to_string(), None).unwrap();
    let marker = ImageImpl::new(
        "marker".to_owned(),
        PremultipliedImage::new(Size::new(16, 16)),
        1.0,
        false,
        Vec::new(),
        Vec::new(),
        None,
        None,
        None,
    );
    style.apply(Edit::AddImage(Arc::new(marker.unwrap()))).unwrap();

    let features = [2.0, 0.0, 1.0]
        .into_iter()
        .enumerate()
        .map(|(i, rank)| Feature {
            geometry: Some(Geometry::Point(Point::new(((i + 1) * 100) as i16, 300 - (i as i16) * 100))),
            properties: Arc::new(PropertyMap::from_iter([("rank".to_owned(), Value::Double(rank))])),
            id: Identifier::Null,
        })
        .collect();
    let data = TileData::Features(Arc::new(features));
    let layout = TileLayout::new(super::registry(), &style, "s", |_| true, (0, 0, 0), &data, |_| None).unwrap();
    let laid_out = layout.run(data).unwrap();
    let center = LatLng::new(0.0, 0.0).unwrap();
    let mut frame = PlacementFrame::<SymbolLayer>::new(&style, "s", &[&laid_out], center, 0.0, (512, 512)).unwrap();
    frame.with_context(|cx| check(cx.buckets.get::<SymbolBucket>(&cx.layers[0].tiles[0].bucket).unwrap()));
}

#[test]
fn symbol_order() {
    for type_ in SORT_KEY_TYPES {
        with_bucket(json!({}), type_, |bucket| {
            let order: Vec<usize> =
                bucket.state.symbol_instances.iter().map(|symbol| symbol.data_feature_index).collect();
            assert_eq!(expected_order(type_), order, "{type_:?}");
            // An explicit constant key still disables viewport-Y sorting for symbol-z-order:auto.
            assert_eq!(type_ == SortKeyType::Undefined, bucket.flags.sort_features_by_y, "{type_:?}");
            match type_ {
                SortKeyType::Undefined => assert_eq!(bucket.sort_key_ranges, Vec::<SortKeyRange>::new()),
                SortKeyType::Constant | SortKeyType::Camera => {
                    assert_eq!(bucket.sort_key_ranges.len(), 1, "{type_:?}");
                    let range = bucket.sort_key_ranges[0];
                    assert_eq!((range.sort_key, range.start, range.end), (5.0, 0, 3), "{type_:?}");
                }
                SortKeyType::DataDriven => assert_eq!(bucket.sort_key_ranges.len(), 3),
            }
        });
    }
}

#[test]
fn symbol_viewport_y() {
    for type_ in SORT_KEY_TYPES {
        with_bucket(json!({"symbol-z-order": "viewport-y"}), type_, |bucket| {
            assert!(bucket.flags.sort_features_by_y, "{type_:?}");
            assert!(bucket.sort_key_ranges.is_empty(), "{type_:?}");
            let order: Vec<usize> =
                bucket.state.symbol_instances.iter().map(|symbol| symbol.data_feature_index).collect();
            assert_eq!(vec![0, 1, 2], order, "{type_:?}");
        });
    }
}
