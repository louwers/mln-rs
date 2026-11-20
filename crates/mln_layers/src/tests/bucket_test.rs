use std::borrow::Cow;
use std::sync::Arc;

use feature::{Identifier, PropertyMap, Value};
use geometry::Point;
use mln_gfx::shaders::segment::SegmentBase;
use mln_map_types::MapMode;
use mln_renderer::draw::{BucketGeometry, Context, DrawIndexes, bucket_indexes, bucket_vertices, paint_vertices};
use mln_renderer::layer::{Bucket, EvaluatedLayer, GeometryUpload};
use mln_renderer::unstable::{LineLayoutVertex, Tessellator};
use mln_style::PropertyEvaluationParameters;
use mln_style::conversion::{Convertible, Error as ConversionError, convert_layer};
use mln_style::evaluated::Evaluated;
use mln_style::layer_type::LayerStyle;
use mln_style_types::image::ImagePositions;
use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::{GeometryCollection, GeometryTileFeature};
use mln_util::geometry::FeatureType;
use mln_util::logging::{Event, EventSeverity, Log, Observer};
use mln_vec_map::VecMap;
use parking_lot::Mutex;

use super::with_context;
use crate::circle::bucket::CircleBucket;
use crate::fill::bucket::FillBucket;
use crate::fill::{FillLayoutProperties, FillPaintProperties};
use crate::fill_extrusion::FillExtrusionLayoutProperties;
use crate::fill_extrusion::bucket::{FillExtrusionBucket, tile_units_per_meter};
use crate::line::LineLayoutProperties;
use crate::line::bucket::LineBucket;
use crate::pattern_layout::{PatternBucket, PatternLayerMap};
use crate::{Fill, Line};

struct StubGeometryTileFeature {
    properties: PropertyMap,
    id: Identifier,
    type_: FeatureType,
    geometry: GeometryCollection,
}

impl GeometryTileFeature for StubGeometryTileFeature {
    fn get_type(&self) -> FeatureType {
        self.type_
    }
    fn get_value(&self, key: &str) -> Option<Value> {
        self.properties.get(key).cloned()
    }
    fn get_id(&self) -> Identifier {
        self.id.clone()
    }
    fn get_geometries(&self) -> Result<Cow<'_, GeometryCollection>, mln_util::error::Error> {
        Ok(Cow::Borrowed(&self.geometry))
    }
}

fn feature(type_: FeatureType, geometry: &GeometryCollection) -> StubGeometryTileFeature {
    StubGeometryTileFeature { properties: PropertyMap::new(), id: Identifier::Null, type_, geometry: geometry.clone() }
}

fn collection(rings: &[&[(i16, i16)]]) -> GeometryCollection {
    GeometryCollection::from(
        rings.iter().map(|ring| ring.iter().map(|(x, y)| Point::new(*x, *y)).collect::<Vec<_>>()).collect::<Vec<_>>(),
    )
}

/// Uploads `bucket`, keeping its geometry in `geometry`, through `context`.
fn upload(bucket: &mut dyn Bucket, geometry: &mut BucketGeometry, context: &mut Context) {
    bucket.upload(&mut GeometryUpload::new(geometry, &[], context));
}

fn needs_upload(bucket: &dyn Bucket) -> bool {
    bucket.needs_upload()
}

// gtest: Buckets.CircleBucket
#[test]
fn circle_bucket() {
    let mut bucket = CircleBucket::new(&VecMap::new(), MapMode::Static, 1.0);
    assert!(!bucket.has_data());
    assert!(!needs_upload(&bucket));

    bucket.segments.push(SegmentBase::new(0, 0));
    assert!(bucket.has_data());
    assert!(needs_upload(&bucket));

    with_context(|_, context| upload(&mut bucket, &mut BucketGeometry::default(), context));
    assert!(bucket.has_data());
    assert!(!needs_upload(&bucket));
}

// gtest: Buckets.FillBucket
#[test]
fn fill_bucket() {
    let layout = FillLayoutProperties::PossiblyEvaluated::default();
    let mut bucket = FillBucket::new(&layout, &VecMap::new(), 5.0, 1);
    assert!(!bucket.has_data());
    assert!(!needs_upload(&bucket));

    let polygon = collection(&[&[(0, 0), (0, 1), (1, 1)]]);
    bucket.add_feature(
        &feature(FeatureType::Polygon, &polygon),
        &polygon,
        &ImagePositions::default(),
        &PatternLayerMap::default(),
        0,
        &CanonicalTileID::new(0, 0, 0),
        &mut Tessellator::default(),
    );
    assert!(bucket.has_data());
    assert!(needs_upload(&bucket));

    with_context(|_, context| upload(&mut bucket, &mut BucketGeometry::default(), context));
    assert!(!needs_upload(&bucket));
}

/// The vertices of a fill-extrusion bucket of `tile` holding `polygon`
/// extruded with `fill-extrusion-rounded-corner-distance` set to `meters`.
fn fill_extrusion_vertex_count(polygon: &GeometryCollection, meters: f32, tile: CanonicalTileID) -> usize {
    let layout = FillExtrusionLayoutProperties::PossiblyEvaluated {
        fill_extrusion_rounded_corner_distance: Evaluated::Constant(meters),
    };
    let mut bucket = FillExtrusionBucket::new(layout, &VecMap::new(), f32::from(tile.z), 1);
    bucket.add_feature(
        &feature(FeatureType::Polygon, polygon),
        polygon,
        &ImagePositions::default(),
        &PatternLayerMap::default(),
        0,
        &tile,
        &mut Tessellator::default(),
    );
    assert!(bucket.has_data());
    bucket.vertices.len()
}

#[test]
fn fill_extrusion_bucket_rounds_corners_in_meters() {
    let square = collection(&[&[(0, 0), (1000, 0), (1000, 1000), (0, 1000), (0, 0)]]);
    let z16 = CanonicalTileID::new(16, 32768, 32768);

    // A ring of n points has n vertices, each the roof's and a wall's.
    assert_eq!(fill_extrusion_vertex_count(&square, 0.0, z16), 5);
    // Rounding replaces each of the 4 corners with 4 points.
    assert_eq!(fill_extrusion_vertex_count(&square, 2.0, z16), 17);
    // At z0 a tile unit is about 5 km, and 2 m rounds to nothing.
    assert_eq!(fill_extrusion_vertex_count(&square, 2.0, CanonicalTileID::new(0, 0, 0)), 5);
}

#[test]
fn fill_extrusion_walls_on_clip_lines_leave_the_pattern_where_it_was() {
    // The edge from (-64, 100) to (-64, 0) is a clip line, and has no wall
    // to wrap the pattern around: the wall after it starts where the one
    // before it ended.
    let square = collection(&[&[(100, 100), (-64, 100), (-64, 0), (100, 0), (100, 100)]]);
    let layout = FillExtrusionLayoutProperties::PossiblyEvaluated {
        fill_extrusion_rounded_corner_distance: Evaluated::Constant(0.0),
    };
    let mut bucket = FillExtrusionBucket::new(layout, &VecMap::new(), 16.0, 1);
    bucket.add_feature(
        &feature(FeatureType::Polygon, &square),
        &square,
        &ImagePositions::default(),
        &PatternLayerMap::default(),
        0,
        &CanonicalTileID::new(16, 32768, 32768),
        &mut Tessellator::default(),
    );
    let edge_distances: Vec<u16> = bucket.vertices.get_mut().iter().map(|v| v.a2[1]).collect();
    assert_eq!(edge_distances, [0, 164, 164, 328, 428]);
}

#[test]
fn fill_extrusion_bucket_skips_polygons_rounded_off_the_tile() {
    // Only the corner at (0, 100) is on the tile, and rounding cuts it off.
    let triangle = collection(&[&[(0, 100), (-10, 90), (-10, 110), (0, 100)]]);
    let layout = FillExtrusionLayoutProperties::PossiblyEvaluated {
        fill_extrusion_rounded_corner_distance: Evaluated::Constant(100.0),
    };
    let mut bucket = FillExtrusionBucket::new(layout, &VecMap::new(), 16.0, 1);
    bucket.add_feature(
        &feature(FeatureType::Polygon, &triangle),
        &triangle,
        &ImagePositions::default(),
        &PatternLayerMap::default(),
        0,
        &CanonicalTileID::new(16, 32768, 32768),
        &mut Tessellator::default(),
    );
    assert!(!bucket.has_data());
}

#[test]
fn tile_units_per_meter_grows_with_zoom_and_latitude() {
    // A z16 tile at the equator spans about 611 m.
    let equator = tile_units_per_meter(&CanonicalTileID::new(16, 32768, 32768));
    assert!((equator - 8192.0 / 611.5).abs() < 0.01, "{equator}");
    assert!((tile_units_per_meter(&CanonicalTileID::new(17, 65536, 65536)) / equator - 2.0).abs() < 1e-3);
    // Near 60° N a meter is twice as many units.
    let north = tile_units_per_meter(&CanonicalTileID::new(16, 32768, 19031));
    assert!((north / equator - 2.0).abs() < 0.01, "{north}");
}

/// A fill bucket whose "fill-color" is the given data-driven expression, with
/// one polygon.
fn data_driven_fill_bucket(fill_color: &serde_json::Value) -> FillBucket {
    let json = serde_json::json!({"type": "fill", "id": "fill", "source": "s", "paint": {"fill-color": fill_color}});
    let mut error = ConversionError::default();
    let layer = convert_layer(super::registry().style(), &Convertible::from(&json), &mut error)
        .unwrap_or_else(|| panic!("{}", error.message));
    let evaluated = FillPaintProperties::PossiblyEvaluated {
        fill_color: Evaluated::PerFeature {
            expression: LayerStyle::<Fill>::recover_ref(&**layer.style())
                .expect("fill layer")
                .paint
                .fill_color
                .value
                .as_expression()
                .unwrap()
                .clone(),
            default: FillPaintProperties::Defaults::fill_color(),
        },
        ..FillPaintProperties::PossiblyEvaluated::default()
    };
    let properties =
        Arc::new(EvaluatedLayer::<Fill>::with_paint(LayerStyle::recover(layer.style()).unwrap(), evaluated));
    let mut bucket = FillBucket::new(
        &FillLayoutProperties::PossiblyEvaluated::default(),
        &VecMap::from_iter([("fill".to_owned(), properties)]),
        5.0,
        1,
    );

    let polygon = collection(&[&[(0, 0), (0, 1), (1, 1)]]);
    let mut feature = feature(FeatureType::Polygon, &polygon);
    feature.properties.insert("c".to_owned(), Value::String("red".to_owned()));
    bucket.add_feature(
        &feature,
        &polygon,
        &ImagePositions::default(),
        &PatternLayerMap::default(),
        0,
        &CanonicalTileID::new(0, 0, 0),
        &mut Tessellator::default(),
    );
    bucket
}

// gtest: Buckets.FillBucketFreesCpuGeometryAfterUpload
#[test]
fn fill_bucket_frees_cpu_geometry_after_upload() {
    let mut bucket = data_driven_fill_bucket(&serde_json::json!(["to-color", ["get", "c"]]));
    let vertex_count = bucket.vertices.len();
    let index_count = bucket.triangles.len();
    assert!(vertex_count > 0);
    assert!(index_count > 0);

    with_context(|gpu, context| {
        // The bucket's upload moves its data to the context's stores, which
        // hold it until it is uploaded.
        let mut geometry = BucketGeometry::default();
        upload(&mut bucket, &mut geometry, context);
        let vertices = bucket_vertices(&geometry, &bucket.vertices).expect("moved");
        assert!(bucket.vertices.data().is_empty());
        assert_eq!(bucket.vertices.len(), vertex_count);
        let stored = context.vertices(&vertices).expect("stored");
        assert_eq!(stored.bytes().len(), vertex_count * stored.vertex_size());

        // A draw's upload writes them to buffers.
        let buffer = context.upload_vertices(gpu, &vertices).expect("uploads").expect("a buffer");
        let mut triangles = DrawIndexes::from(bucket_indexes(&geometry, &bucket.triangles).expect("moved"));
        triangles.upload(gpu, context).expect("uploads");

        // The CPU copies are gone, the counts that draws are built from
        // are not.
        assert!(context.vertices(&vertices).is_some_and(|stored| !stored.needs_upload() && stored.bytes().is_empty()));
        assert_eq!(vertices.count(), vertex_count);
        assert!(!triangles.has_data(context));
        assert_eq!(triangles.len(context), index_count);
        assert_eq!(bucket.triangles.len(), index_count);

        // A second draw over the same bucket shares the vertices and the
        // buffer without reading the data.
        let again = bucket_vertices(&geometry, &bucket.vertices).expect("moved");
        assert!(vertices == again);
        let indexes = DrawIndexes::from(bucket_indexes(&geometry, &bucket.triangles).expect("moved"));
        assert!(indexes.buffer(context).is_some_and(|buffer| triangles.buffer(context) == Some(buffer)));
        let reused = context.upload_vertices(gpu, &again).expect("uploads").expect("a buffer");
        assert_eq!(buffer, reused);

        // Data-driven paint values are freed too when nothing can rewrite them.
        let binders = bucket.paint_property_binders.get("fill").unwrap();
        assert!(!binders.depends_on_feature_state());
        let buffer = &binders.interleaved_vertex_buffer;
        let paint = paint_vertices(context, &geometry, buffer, false).expect("moved");
        assert!(paint.count() > 0);
        assert!(context.upload_vertices(gpu, &paint).expect("uploads").is_some());
        assert!(context.vertices(&paint).is_some_and(|stored| !stored.needs_upload()));
        assert!(buffer.vertices().data().is_empty());
    });
}

// The buckets of a tile's world copy are copies of the tile's, and the
// upload of either shares what the other moved into the context's stores,
// freeing its own data.
#[test]
fn world_copies_share_their_buckets_stored_geometry() {
    let mut bucket = data_driven_fill_bucket(&serde_json::json!(["to-color", ["get", "c"]]));
    let mut copy = bucket.clone();
    with_context(|_, context| {
        let mut copy_geometry = BucketGeometry::default();
        upload(&mut copy, &mut copy_geometry, context);
        let mut geometry = BucketGeometry::default();
        bucket.upload(&mut GeometryUpload::new(&mut geometry, &[&copy_geometry], context));

        assert!(bucket.vertices.is_moved() && bucket.vertices.data().is_empty());
        assert!(bucket.triangles.is_moved() && bucket.triangles.data().is_empty());
        let vertices = bucket_vertices(&geometry, &bucket.vertices);
        assert!(vertices.is_some() && vertices == bucket_vertices(&copy_geometry, &copy.vertices));
        let indexes = bucket_indexes(&geometry, &bucket.triangles);
        assert!(indexes.is_some() && indexes == bucket_indexes(&copy_geometry, &copy.triangles));
        let mut paint = |bucket: &FillBucket, geometry: &BucketGeometry| {
            let buffer = &bucket.paint_property_binders.get("fill").unwrap().interleaved_vertex_buffer;
            paint_vertices(context, geometry, buffer, false)
        };
        assert!(paint(&bucket, &geometry) == paint(&copy, &copy_geometry));
    });
}

// gtest: Buckets.FillBucketKeepsPaintDataThatFeatureStateRewrites
#[test]
fn fill_bucket_keeps_paint_data_that_feature_state_rewrites() {
    let mut bucket = data_driven_fill_bucket(&serde_json::json!([
        "to-color",
        ["case", ["boolean", ["feature-state", "hover"], false], "red", "blue"]
    ]));

    with_context(|gpu, context| {
        let mut geometry = BucketGeometry::default();
        upload(&mut bucket, &mut geometry, context);
        let binders = bucket.paint_property_binders.get("fill").unwrap();
        assert!(binders.depends_on_feature_state());
        let buffer = &binders.interleaved_vertex_buffer;
        let paint = paint_vertices(context, &geometry, buffer, binders.depends_on_feature_state()).expect("a copy");
        assert!(paint.count() > 0);
        assert!(context.upload_vertices(gpu, &paint).expect("uploads").is_some());
        assert!(!buffer.vertices().is_moved());
        assert_eq!(buffer.vertices().data().len(), paint.count());
    });
}

// gtest: Buckets.LineBucket
#[test]
fn line_bucket() {
    let layout = LineLayoutProperties::PossiblyEvaluated::default();
    let mut bucket = LineBucket::new(layout, &VecMap::new(), 10.0, 1);
    assert!(!bucket.has_data());
    assert!(!needs_upload(&bucket));

    // Ignore invalid feature type.
    let point = collection(&[&[(0, 0)]]);
    bucket.add_feature(
        &feature(FeatureType::Point, &point),
        &point,
        &ImagePositions::default(),
        &PatternLayerMap::default(),
        0,
        &CanonicalTileID::new(0, 0, 0),
        &mut Tessellator::default(),
    );
    assert!(!bucket.has_data());

    let line = collection(&[&[(0, 0), (1, 1)]]);
    bucket.add_feature(
        &feature(FeatureType::LineString, &line),
        &line,
        &ImagePositions::default(),
        &PatternLayerMap::default(),
        1,
        &CanonicalTileID::new(0, 0, 0),
        &mut Tessellator::default(),
    );
    assert!(bucket.has_data());
    assert!(needs_upload(&bucket));

    with_context(|_, context| upload(&mut bucket, &mut BucketGeometry::default(), context));
    assert!(!needs_upload(&bucket));
}

// gtest: Buckets.LineBucketDrawablesBuildFromFreedIndexes
#[test]
fn line_bucket_drawables_build_from_freed_indexes() {
    let mut bucket = LineBucket::new(LineLayoutProperties::PossiblyEvaluated::default(), &VecMap::new(), 10.0, 1);
    let line = collection(&[&[(0, 0), (100, 100), (200, 0)]]);
    bucket.add_feature(
        &feature(FeatureType::LineString, &line),
        &line,
        &ImagePositions::default(),
        &PatternLayerMap::default(),
        0,
        &CanonicalTileID::new(0, 0, 0),
        &mut Tessellator::default(),
    );
    assert!(bucket.has_data());

    with_context(|gpu, context| {
        let mut geometry = BucketGeometry::default();
        let index_count = bucket.triangles.len();
        upload(&mut bucket, &mut geometry, context);

        // An earlier draw put the indexes in a buffer, which frees their CPU
        // copy.
        let mut triangles = DrawIndexes::from(bucket_indexes(&geometry, &bucket.triangles).expect("moved"));
        triangles.upload(gpu, context).expect("uploads");
        assert!(!triangles.has_data(context));

        // A layer that rebuilds its draws of the tile builds the segments
        // from the counts.
        let indexes = DrawIndexes::from(bucket_indexes(&geometry, &bucket.triangles).expect("moved"));
        assert_eq!(indexes.len(context), index_count);
        assert_eq!(triangles.len(context), index_count);
    });
}

type Records = Arc<Mutex<Vec<(EventSeverity, Event, i64, String)>>>;

struct Recorder(Records);

impl Observer for Recorder {
    fn on_record(&self, severity: EventSeverity, event: Event, code: i64, msg: &str) -> bool {
        self.0.lock().push((severity, event, code, msg.to_owned()));
        true
    }
}

/// Captures the calling thread's log records.
struct FixtureLog(Records);

impl FixtureLog {
    fn new() -> Self {
        let records: Records = Arc::new(Mutex::new(Vec::new()));
        let _ = Log::set_thread_observer(Some(Box::new(Recorder(Arc::clone(&records)))));
        FixtureLog(records)
    }
    fn count(&self, severity: EventSeverity, event: Event, msg: &str) -> usize {
        self.0.lock().iter().filter(|m| m.0 == severity && m.1 == event && m.2 == -1 && m.3 == msg).count()
    }
    fn is_empty(&self) -> bool {
        self.0.lock().is_empty()
    }
}

impl Drop for FixtureLog {
    fn drop(&mut self) {
        let _ = Log::set_thread_observer(None);
    }
}

// FillBucket.TracksSDFPatternsPerLayer
// gtest: FillBucket.TracksSDFPatternsPerLayer
#[test]
fn tracks_sdf_patterns_per_layer() {
    let log = FixtureLog::new();
    let layout = FillLayoutProperties::PossiblyEvaluated::default();
    let mut bucket = FillBucket::new(&layout, &VecMap::new(), 5.0, 1);

    bucket.record_sdf_pattern("sdf-layer", true);
    bucket.record_sdf_pattern("rgba-layer", false);

    assert!(bucket.is_sdf_pattern("sdf-layer"));
    assert!(!bucket.is_sdf_pattern("rgba-layer"));
    assert!(!bucket.is_sdf_pattern("missing-layer"));
    assert!(log.is_empty());
}

// FillBucket.WarnsOnceForMixedPatternTypesPerBucket
// gtest: FillBucket.WarnsOnceForMixedPatternTypesPerBucket
#[test]
fn warns_once_for_mixed_pattern_types_per_bucket() {
    let log = FixtureLog::new();
    let layout = FillLayoutProperties::PossiblyEvaluated::default();
    let mut first_bucket = FillBucket::new(&layout, &VecMap::new(), 5.0, 1);
    let mut second_bucket = FillBucket::new(&layout, &VecMap::new(), 5.0, 1);

    first_bucket.record_sdf_pattern("mixed-layer", true);
    first_bucket.record_sdf_pattern("mixed-layer", false);
    first_bucket.record_sdf_pattern("mixed-layer", false);
    second_bucket.record_sdf_pattern("mixed-layer", true);
    second_bucket.record_sdf_pattern("mixed-layer", false);

    assert_eq!(
        2,
        log.count(
            EventSeverity::Warning,
            Event::Style,
            "Style sheet warning: Cannot mix SDF and non-SDF fill patterns in layer \"mixed-layer\""
        )
    );
}

/// The vertices of a line bucket for a line layer with `layout`, with one
/// feature per `(properties, line)`.
fn line_bucket_vertices(
    layout: &serde_json::Value,
    features: &[(serde_json::Value, &[(i16, i16)])],
) -> Vec<LineLayoutVertex> {
    let json = serde_json::json!({"type": "line", "id": "line", "source": "s", "layout": layout});
    let mut error = ConversionError::default();
    let layer = convert_layer(super::registry().style(), &Convertible::from(&json), &mut error)
        .unwrap_or_else(|| panic!("{}", error.message));
    let layout = LayerStyle::<Line>::recover_ref(&**layer.style())
        .expect("line layer")
        .layout
        .evaluate(&PropertyEvaluationParameters::new(10.0));
    let mut bucket = LineBucket::new(layout, &VecMap::new(), 10.0, 1);
    for (index, (properties, line)) in features.iter().enumerate() {
        let line = collection(&[line]);
        let mut feature = feature(FeatureType::LineString, &line);
        for (key, value) in properties.as_object().expect("properties") {
            let value = match value {
                serde_json::Value::String(s) => Value::String(s.clone()),
                v => Value::Double(v.as_f64().expect("a string or number")),
            };
            feature.properties.insert(key.clone(), value);
        }
        bucket.add_feature(
            &feature,
            &line,
            &ImagePositions::default(),
            &PatternLayerMap::default(),
            index,
            &CanonicalTileID::new(0, 0, 0),
            &mut Tessellator::default(),
        );
    }
    bucket.vertices.data().into_owned()
}

#[test]
fn line_bucket_evaluates_cap_and_limits_per_feature() {
    // A sharp corner: its miter length is about 3.2, so a miter limit of 2
    // bevels it and 10 doesn't, and a round limit of 1 rounds it and 10 doesn't.
    const CORNER: &[(i16, i16)] = &[(0, 0), (100, 400), (200, 0)];
    for (property, values) in [
        ("line-cap", [serde_json::json!("butt"), serde_json::json!("round")]),
        ("line-miter-limit", [serde_json::json!(2), serde_json::json!(10)]),
        ("line-round-limit", [serde_json::json!(1), serde_json::json!(10)]),
    ] {
        let join = if property == "line-round-limit" { "round" } else { "miter" };
        let constant = |value: &serde_json::Value| {
            line_bucket_vertices(
                &serde_json::json!({"line-join": join, property: value}),
                &[(serde_json::json!({}), CORNER)],
            )
        };
        let first = constant(&values[0]);
        let second = constant(&values[1]);
        assert_ne!(first, second, "{property}: the two values make the same line");

        let data_driven = line_bucket_vertices(
            &serde_json::json!({"line-join": join, property: ["get", "v"]}),
            &[(serde_json::json!({"v": values[0]}), CORNER), (serde_json::json!({"v": values[1]}), CORNER)],
        );
        assert_eq!(data_driven, [first, second].concat(), "{property}");
    }
}

// The pulled walls' shader finds a vertex's paint values where the paint
// attributes read them.
#[test]
fn the_paint_layout_of_pulled_walls_is_the_paint_attributes() {
    use mln_gfx::AttributeDataType::{Float, Float4};
    use mln_gfx::shaders::shader_defines::{
        ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE,
        ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE,
    };
    use mln_renderer::draw::{SharedAttribute, VertexAttributeArray};

    use crate::fill_extrusion::render_layer::paint_layout;

    with_context(|_, context| {
        let mut attrs = VertexAttributeArray::new();
        assert_eq!(paint_layout(&attrs), 0, "every property a uniform");

        // A vertex's base, its color's four floats, then its height.
        let paint = context.add_vertices(vec![[0f32; 6]]);
        for (id, data_type, offset) in [
            (ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, Float, 0),
            (ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE, Float4, 4),
            (ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE, Float, 20),
        ] {
            attrs.set_shared(id, SharedAttribute::new(paint.clone(), data_type, offset, 24));
        }
        // Six words a vertex: the base at 0, the height at 5, the color at 1.
        assert_eq!(paint_layout(&attrs), 0x0105_0006);
    });
}

// The pulled outlines' shader finds a vertex's paint values where the paint
// attributes read them.
#[test]
fn the_paint_layout_of_pulled_outlines_is_the_paint_attributes() {
    use mln_gfx::AttributeDataType::{Float, Float2, Float4};
    use mln_gfx::shaders::shader_defines::{
        ID_FILL_COLOR_VERTEX_ATTRIBUTE, ID_FILL_OPACITY_VERTEX_ATTRIBUTE, ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE,
    };
    use mln_renderer::draw::{SharedAttribute, VertexAttributeArray};

    use crate::fill::render_layer::outline_paint_layout;

    with_context(|_, context| {
        let mut attrs = VertexAttributeArray::new();
        assert_eq!(outline_paint_layout(&attrs), 0, "every property a uniform");

        // A vertex's fill color, its opacity, then its outline color.
        let paint = context.add_vertices(vec![[0f32; 5]]);
        for (id, data_type, offset) in [
            (ID_FILL_COLOR_VERTEX_ATTRIBUTE, Float2, 0),
            (ID_FILL_OPACITY_VERTEX_ATTRIBUTE, Float, 8),
            (ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE, Float2, 12),
        ] {
            attrs.set_shared(id, SharedAttribute::new(paint.clone(), data_type, offset, 20));
        }
        // Five words a vertex: the outline color at 3, the opacity at 2.
        assert_eq!(outline_paint_layout(&attrs), 0x0002_0305);

        // An interpolated outline color alone.
        let mut attrs = VertexAttributeArray::new();
        let paint = context.add_vertices(vec![[0f32; 4]]);
        attrs.set_shared(ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE, SharedAttribute::new(paint, Float4, 0, 16));
        assert_eq!(outline_paint_layout(&attrs), 4);
    });
}
