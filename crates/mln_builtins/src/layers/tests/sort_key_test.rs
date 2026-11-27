//! The fill, line and circle parts of `test/layout/sort_key.test.cpp`: a
//! layer's sort key orders its bucket's features.

use std::sync::Arc;

use geo_types::{Geometry, LineString, Point, Polygon};
use mln_engine::bench::{LaidOut, TileLayout};
use mln_engine::tile::TileData;
use mln_style::Style;
use mln_style::feature::{Feature, Identifier, PropertyMap, Value};
use serde_json::json;

use crate::layers::circle::bucket::CircleBucket;
use crate::layers::fill::bucket::FillBucket;
use crate::layers::line::bucket::LineBucket;

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

#[derive(Clone, Copy, PartialEq)]
enum Shape {
    Polygon,
    LineString,
    Point,
}

/// Lays out three features of `shape` with ranks 2, 0, 1 at x = 100, 200,
/// 300 with the layer `layer`, of type `type_`, whose `sort_key_name` is set
/// to the sort key of `sort`.
fn lay_out(mut layer: serde_json::Value, sort_key_name: &str, sort: SortKeyType, shape: Shape) -> LaidOut {
    if let Some(sort_key) = sort_key(sort) {
        layer["layout"][sort_key_name] = sort_key;
    }
    let style = json!({
        "version": 8,
        "sources": {"source": {"type": "vector", "tiles": ["tiles"]}},
        "layers": [layer],
    });
    let registry = crate::registry();
    let style = Style::from_json(registry.style(), &style.to_string()).expect("the style");
    let ranks = [2.0, 0.0, 1.0];
    let features = ranks
        .iter()
        .enumerate()
        .map(|(i, &rank)| {
            let x = ((i + 1) * 100) as i16;
            let geometry = match shape {
                Shape::Polygon => Geometry::Polygon(Polygon::new(
                    LineString::from(vec![(x, 10), (x, 20), (x + 10, 20), (x, 10)]),
                    vec![],
                )),
                Shape::LineString => Geometry::LineString(LineString::from(vec![(x, 10), (x, 20)])),
                Shape::Point => Geometry::Point(Point::new(x, 300 - (i as i16) * 100)),
            };
            Feature {
                geometry: Some(geometry),
                properties: Arc::new(PropertyMap::from_iter([("rank".to_owned(), Value::Double(rank))])),
                id: Identifier::Null,
            }
        })
        .collect();
    let data = TileData::Features(Arc::new(features));
    let layout = TileLayout::new(&registry, &style, "source", |_| true, (0, 0, 0), &data, |_| None).expect("a layout");
    layout.run(data).expect("the layout's buckets")
}

/// The features in the order their vertices are, from the x coordinate the
/// vertices store `coordinate_scale` times.
fn feature_order(xs: impl Iterator<Item = i16>, coordinate_scale: i16) -> Vec<usize> {
    let mut order: Vec<usize> = Vec::new();
    for x in xs {
        let index = (x / coordinate_scale / 100 - 1) as usize;
        if order.last() != Some(&index) {
            order.push(index);
        }
    }
    order
}

#[test]
fn fill_order() {
    for type_ in SORT_KEY_TYPES {
        let layer = json!({ "id": "fill", "type": "fill", "source": "source" });
        let laid_out = lay_out(layer, "fill-sort-key", type_, Shape::Polygon);
        let bucket = laid_out.bucket::<FillBucket>("fill").unwrap();
        assert_eq!(
            expected_order(type_),
            feature_order(bucket.vertices.data().iter().map(|v| v.a1[0]), 1),
            "{type_:?}"
        );
    }
}

#[test]
fn line_order() {
    for type_ in SORT_KEY_TYPES {
        let layer = json!({ "id": "line", "type": "line", "source": "source" });
        let laid_out = lay_out(layer, "line-sort-key", type_, Shape::LineString);
        let bucket = laid_out.bucket::<LineBucket>("line").unwrap();
        assert_eq!(
            expected_order(type_),
            feature_order(bucket.vertices.data().iter().map(|v| v.a1[0]), 2),
            "{type_:?}"
        );
    }
}

#[test]
fn circle_order() {
    for type_ in SORT_KEY_TYPES {
        let layer = json!({ "id": "circle", "type": "circle", "source": "source" });
        let laid_out = lay_out(layer, "circle-sort-key", type_, Shape::Point);
        let bucket = laid_out.bucket::<CircleBucket>("circle").unwrap();
        assert_eq!(
            expected_order(type_),
            feature_order(bucket.vertices.data().iter().map(|v| v.a1[0]), 2),
            "{type_:?}"
        );
    }
}
