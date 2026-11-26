use std::sync::Arc;

use feature::{Feature, Identifier, PropertyMap, Value};
use geo_types::{Geometry, LineString, Point, Polygon};
use mln_style::bitmap::PremultipliedImage;
use mln_style::conversion::{Convertible, Error as ConversionError};
use mln_style::expression::UnavailableImages;
use mln_style::image::Impl as ImageImpl;
use mln_style::layer::{LayerStyle, convert_layer};
use mln_style::layers::{Circle, Fill, Line, Symbol};
use mln_style::size::Size;
use mln_style::subdivision_granularity::SubdivisionGranularitySetting;
use mln_vec_map::VecSet;
use serde_json::json;

use crate::Registry;
use crate::buckets::circle_bucket::CircleBucket;
use crate::buckets::fill_bucket::FillBucket;
use crate::buckets::line_bucket::LineBucket;
use crate::image::image_store::ImageDependencies;
use crate::layer::bucket::ErasedBucket;
use crate::layer::bucket_parameters::BucketParameters;
use crate::layer::filter_groups::FilterGroups;
use crate::layer::layer_type::{ErasedEvaluatedLayer, EvaluatedLayer};
use crate::layer::layout::{LayoutParameters, SourceLayer};
use crate::layer::tile_layout::{Built, Resolved};
use crate::map::MapMode;
use crate::query::feature_index::FeatureIndexBuilder;
use crate::symbol::bucket::SymbolBucket;
use crate::text::atlas::{ImagePosition, ImagePositions, RectU16};
use crate::text::glyph::GlyphDependencies;
use crate::tile::OverscaledTileID;
use crate::tile_store::contents::LayoutBuckets;

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

/// The properties of a layer of the given style JSON, with `sort_key_name`
/// set to the sort key of `type_`.
fn layer(mut json: serde_json::Value, sort_key_name: &str, type_: SortKeyType) -> Arc<dyn ErasedEvaluatedLayer> {
    if let Some(sort_key) = sort_key(type_) {
        json["layout"][sort_key_name] = sort_key;
    }
    let mut error = ConversionError::default();
    let layer = convert_layer(Registry::with_builtins().style(), &Convertible::from(&json), &mut error)
        .unwrap_or_else(|| panic!("{}", error.message));
    let style = layer.style();
    match json["type"].as_str().unwrap() {
        "fill" => Arc::new(EvaluatedLayer::<Fill>::new(LayerStyle::recover(style).unwrap())),
        "line" => Arc::new(EvaluatedLayer::<Line>::new(LayerStyle::recover(style).unwrap())),
        "circle" => Arc::new(EvaluatedLayer::<Circle>::new(LayerStyle::recover(style).unwrap())),
        "symbol" => Arc::new(EvaluatedLayer::<Symbol>::new(LayerStyle::recover(style).unwrap())),
        other => panic!("no properties for {other}"),
    }
}

/// Lays out three features with ranks 2, 0, 1 at x = 100, 200, 300 and
/// returns the layer's bucket.
fn create_bucket(properties: &Arc<dyn ErasedEvaluatedLayer>, shape: Shape) -> Box<dyn ErasedBucket> {
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
    let data = crate::tile::TileData::Features(Arc::new(features));
    let bucket_parameters = BucketParameters {
        tile_id: OverscaledTileID::from_zxy(0, 0, 0),
        mode: MapMode::Continuous,
        pixel_ratio: 1.0,
        global_state: None,
        subdivision_granularity: SubdivisionGranularitySetting::none(),
    };
    let mut glyph_dependencies = GlyphDependencies::default();
    let mut image_dependencies = ImageDependencies::default();
    let available_images = VecSet::from_iter(["marker".to_owned()]);
    let layer_id = properties.style().base().id.clone();
    let layout = properties
        .entry()
        .create_layout(
            std::slice::from_ref(properties),
            LayoutParameters {
                bucket_parameters: &bucket_parameters,
                font_faces: None,
                glyph_pbfs: true,
                glyph_dependencies: &mut glyph_dependencies,
                image_dependencies: &mut image_dependencies,
                available_images: &available_images,
                unavailable_images: &UnavailableImages::new(),
                filter_groups: &mut FilterGroups::default(),
            },
            SourceLayer::new(data.clone(), ""),
        )
        .unwrap();

    let marker = Arc::new(
        ImageImpl::new(
            "marker".to_owned(),
            PremultipliedImage::new(Size::new(16, 16)),
            1.0,
            false,
            Vec::new(),
            Vec::new(),
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let image_positions = ImagePositions::from_iter([(
        "marker".to_owned(),
        ImagePosition::new(RectU16 { x: 0, y: 0, w: 18, h: 18 }, &marker),
    )]);
    let resolved = Resolved { icons: image_positions.clone(), patterns: image_positions, ..Resolved::default() };
    let mut built = Built { buckets: LayoutBuckets::default(), feature_index: FeatureIndexBuilder::new(Some(data)) };
    layout.build(Some(&resolved), &mut built, &bucket_parameters, true).unwrap();
    built.buckets.into_bucket(&layer_id).unwrap()
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
        let properties = layer(json!({ "id": "fill", "type": "fill", "source": "source" }), "fill-sort-key", type_);
        let bucket = create_bucket(&properties, Shape::Polygon);
        let bucket = bucket.downcast_ref::<FillBucket>().unwrap();
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
        let properties = layer(json!({ "id": "line", "type": "line", "source": "source" }), "line-sort-key", type_);
        let bucket = create_bucket(&properties, Shape::LineString);
        let bucket = bucket.downcast_ref::<LineBucket>().unwrap();
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
        let properties =
            layer(json!({ "id": "circle", "type": "circle", "source": "source" }), "circle-sort-key", type_);
        let bucket = create_bucket(&properties, Shape::Point);
        let bucket = bucket.downcast_ref::<CircleBucket>().unwrap();
        assert_eq!(
            expected_order(type_),
            feature_order(bucket.vertices.data().iter().map(|v| v.a1[0]), 2),
            "{type_:?}"
        );
    }
}

fn symbol_layer(type_: SortKeyType, z_order: Option<&str>) -> Arc<dyn ErasedEvaluatedLayer> {
    let mut json = json!({
        "id": "symbol", "type": "symbol", "source": "source",
        "layout": { "icon-image": "marker", "icon-allow-overlap": true }
    });
    if let Some(z_order) = z_order {
        json["layout"]["symbol-z-order"] = json!(z_order);
    }
    layer(json, "symbol-sort-key", type_)
}

#[test]
fn symbol_order() {
    for type_ in SORT_KEY_TYPES {
        let bucket = create_bucket(&symbol_layer(type_, None), Shape::Point);
        let bucket = bucket.downcast_ref::<SymbolBucket>().unwrap();
        let order: Vec<usize> = bucket.state.symbol_instances.iter().map(|symbol| symbol.data_feature_index).collect();
        assert_eq!(expected_order(type_), order, "{type_:?}");
        // An explicit constant key still disables viewport-Y sorting for symbol-z-order:auto.
        assert_eq!(type_ == SortKeyType::Undefined, bucket.sort_features_by_y, "{type_:?}");
        match type_ {
            SortKeyType::Undefined => {
                assert_eq!(bucket.sort_key_ranges, Vec::<crate::symbol::bucket::SortKeyRange>::new());
            }
            SortKeyType::Constant | SortKeyType::Camera => {
                assert_eq!(bucket.sort_key_ranges.len(), 1, "{type_:?}");
                let range = bucket.sort_key_ranges[0];
                assert_eq!((range.sort_key, range.start, range.end), (5.0, 0, 3), "{type_:?}");
            }
            SortKeyType::DataDriven => assert_eq!(bucket.sort_key_ranges.len(), 3),
        }
    }
}

#[test]
fn symbol_viewport_y() {
    for type_ in SORT_KEY_TYPES {
        let bucket = create_bucket(&symbol_layer(type_, Some("viewport-y")), Shape::Point);
        let bucket = bucket.downcast_ref::<SymbolBucket>().unwrap();
        assert!(bucket.sort_features_by_y, "{type_:?}");
        assert!(bucket.sort_key_ranges.is_empty(), "{type_:?}");
        let order: Vec<usize> = bucket.state.symbol_instances.iter().map(|symbol| symbol.data_feature_index).collect();
        assert_eq!(vec![0, 1, 2], order, "{type_:?}");
    }
}

#[test]
fn symbol_height_offset_expression_raises_content_elevation() {
    let json = json!({
        "id": "symbol", "type": "symbol", "source": "source",
        "layout": { "icon-image": "marker", "icon-allow-overlap": true, "symbol-height-offset": ["get", "rank"] }
    });
    let bucket = create_bucket(&layer(json, "symbol-sort-key", SortKeyType::Undefined), Shape::Point);
    assert_eq!(bucket.max_elevation(), 2.0);
}
