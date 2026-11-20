use std::sync::Arc;

use feature::{Feature, Identifier, PropertyMap, Value};
use geo_types::{Geometry, LineString, Point, Polygon};
use mln_expression::image_expression::UnavailableImages;
use mln_map_types::MapMode;
use mln_renderer::layer::{
    BucketParameters, BuildParams, EvaluatedLayer, FeatureIndexBuilder, FilterGroups, LayerType, Layout,
    LayoutParameters, Resolved, SourceLayer,
};
use mln_style::conversion::{Convertible, Error as ConversionError, convert_layer};
use mln_style::layer_type::LayerStyle;
use mln_style_types::image::{ImageDependencies, ImagePosition, ImagePositions, Impl as ImageImpl, RectU16};
use mln_text::glyph::GlyphDependencies;
use mln_tile_id::OverscaledTileID;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_vec_map::VecSet;
use serde_json::json;

use crate::{Circle, Fill, Line};

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

/// The values of a `T` layer of the given style JSON, with `sort_key_name`
/// set to the sort key of `type_`.
fn layer<T: LayerType>(mut json: serde_json::Value, sort_key_name: &str, type_: SortKeyType) -> Arc<EvaluatedLayer<T>> {
    if let Some(sort_key) = sort_key(type_) {
        json["layout"][sort_key_name] = sort_key;
    }
    let mut error = ConversionError::default();
    let layer = convert_layer(super::registry().style(), &Convertible::from(&json), &mut error)
        .unwrap_or_else(|| panic!("{}", error.message));
    Arc::new(EvaluatedLayer::new(LayerStyle::recover(layer.style()).unwrap()))
}

/// Lays out three features with ranks 2, 0, 1 at x = 100, 200, 300 and
/// returns the layer's bucket.
fn create_bucket<T: LayerType>(layer: &Arc<EvaluatedLayer<T>>, shape: Shape) -> T::Bucket {
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
    let data = mln_tile::TileData::Features(Arc::new(features));
    let bucket_parameters = BucketParameters {
        tile_id: OverscaledTileID::from_zxy(0, 0, 0),
        mode: MapMode::Continuous,
        pixel_ratio: 1.0,
        global_state: None,
        subdivision_granularity: SubdivisionGranularitySetting::none(),
    };
    let available_images = VecSet::from_iter(["marker".to_owned()]);
    let mut layout = T::create_layout(
        std::slice::from_ref(layer),
        &mut LayoutParameters {
            bucket_parameters: &bucket_parameters,
            font_faces: None,
            glyph_pbfs: true,
            glyph_dependencies: &mut GlyphDependencies::default(),
            image_dependencies: &mut ImageDependencies::default(),
            available_images: &available_images,
            unavailable_images: &UnavailableImages::new(),
            filter_groups: &mut FilterGroups::default(),
        },
        SourceLayer::new(data.clone(), ""),
    )
    .unwrap()
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
    let mut resolved = Resolved::default();
    resolved.patterns = ImagePositions::from_iter([(
        "marker".to_owned(),
        ImagePosition::new(RectU16 { x: 0, y: 0, w: 18, h: 18 }, &marker),
    )]);
    let build =
        BuildParams { first_load: true, show_collision_boxes: false, canonical: bucket_parameters.tile_id.canonical };
    layout.create_bucket(Some(&resolved), &mut FeatureIndexBuilder::new(Some(data)), &build).unwrap().unwrap()
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
        let properties =
            layer::<Fill>(json!({ "id": "fill", "type": "fill", "source": "source" }), "fill-sort-key", type_);
        let bucket = create_bucket(&properties, Shape::Polygon);
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
        let properties =
            layer::<Line>(json!({ "id": "line", "type": "line", "source": "source" }), "line-sort-key", type_);
        let bucket = create_bucket(&properties, Shape::LineString);
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
            layer::<Circle>(json!({ "id": "circle", "type": "circle", "source": "source" }), "circle-sort-key", type_);
        let bucket = create_bucket(&properties, Shape::Point);
        assert_eq!(
            expected_order(type_),
            feature_order(bucket.vertices.data().iter().map(|v| v.a1[0]), 2),
            "{type_:?}"
        );
    }
}
