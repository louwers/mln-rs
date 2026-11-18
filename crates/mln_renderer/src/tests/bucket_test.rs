use std::borrow::Cow;
use std::rc::Rc;
use std::sync::Arc;

use feature::{Identifier, PropertyMap, Value};
use geometry::Point;
use mln_gfx::shaders::segment::SegmentBase;
use mln_map_types::MapMode;
use mln_style::PropertyEvaluationParameters;
use mln_style::conversion::{Convertible, Error as ConversionError, convert_layer};
use mln_style::evaluated::Evaluated;
use mln_style::layer_type::LayerStyle;
use mln_style::layers::fill_extrusion_layer_properties::FillExtrusionLayoutProperties;
use mln_style::layers::fill_layer_properties::{FillLayoutProperties, FillPaintProperties};
use mln_style::layers::line_layer_properties::LineLayoutProperties;
use mln_style::layers::symbol_layer_properties::SymbolLayoutProperties;
use mln_style::layers::{Fill, Line};
use mln_style::property_value::PropertyValue;
use mln_style_types::image::ImagePositions;
use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::{GeometryCollection, GeometryTileFeature};
use mln_util::geometry::FeatureType;
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, EventSeverity, Log, Observer};
use mln_util::size::Size;
use mln_vec_map::VecMap;
use parking_lot::Mutex;

use super::with_context;
use crate::Registry;
use crate::bucket::Bucket;
use crate::bucket_geometry::{BucketGeometry, GeometryUpload, bucket_indexes, bucket_vertices, paint_vertices};
use crate::buckets::circle_bucket::CircleBucket;
use crate::buckets::fill_bucket::FillBucket;
use crate::buckets::fill_extrusion_bucket::FillExtrusionBucket;
use crate::buckets::line_bucket::{LineBucket, LineLayoutVertex};
use crate::buckets::raster_bucket::{RasterBucket, RasterLayoutVertex};
use crate::buckets::symbol_bucket::SymbolBucket;
use crate::gfx::context::Context;
use crate::gfx::indexes::DrawIndexes;
use crate::layer_type::EvaluatedLayer;
use crate::layout::pattern_layout::PatternLayerMap;
use crate::layout::symbol_instance::SymbolInstancePools;
use crate::tile_mask::TileMask;

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
    let needs = crate::layers::upload_needs(context);
    bucket.upload(&mut GeometryUpload::new(geometry, context), needs);
}

// gtest: Buckets.CircleBucket
#[test]
fn circle_bucket() {
    let mut bucket = CircleBucket::new(&VecMap::new(), MapMode::Static, 1.0);
    assert!(!bucket.has_data());
    assert!(!bucket.needs_upload());

    // CircleBucket::addFeature() is a no-op.
    let point = collection(&[&[(0, 0)]]);
    bucket
        .add_feature(
            &feature(FeatureType::Point, &point),
            &point,
            &ImagePositions::default(),
            &PatternLayerMap::default(),
            0,
            &CanonicalTileID::new(0, 0, 0),
        )
        .unwrap();
    assert!(!bucket.has_data());
    assert!(!bucket.needs_upload());

    bucket.segments.push(SegmentBase::new(0, 0));
    assert!(bucket.has_data());
    assert!(bucket.needs_upload());

    with_context(|context| upload(&mut bucket, &mut BucketGeometry::default(), context));
    assert!(bucket.has_data());
    assert!(!bucket.needs_upload());
}

// gtest: Buckets.FillBucket
#[test]
fn fill_bucket() {
    let layout = FillLayoutProperties::PossiblyEvaluated::default();
    let mut bucket = FillBucket::new(&layout, &VecMap::new(), 5.0, 1);
    assert!(!bucket.has_data());
    assert!(!bucket.needs_upload());

    let polygon = collection(&[&[(0, 0), (0, 1), (1, 1)]]);
    bucket
        .add_feature(
            &feature(FeatureType::Polygon, &polygon),
            &polygon,
            &ImagePositions::default(),
            &PatternLayerMap::default(),
            0,
            &CanonicalTileID::new(0, 0, 0),
        )
        .unwrap();
    assert!(bucket.has_data());
    assert!(bucket.needs_upload());

    with_context(|context| upload(&mut bucket, &mut BucketGeometry::default(), context));
    assert!(!bucket.needs_upload());
}

/// The vertices of a fill-extrusion bucket holding `polygon` extruded with
/// `fill-extrusion-rounded-corner-distance` set to `distance`.
fn fill_extrusion_vertex_count(polygon: &GeometryCollection, distance: f32) -> usize {
    let layout = FillExtrusionLayoutProperties::PossiblyEvaluated {
        fill_extrusion_rounded_corner_distance: Evaluated::Constant(distance),
    };
    let mut bucket = FillExtrusionBucket::new(layout, &VecMap::new(), 16.0, 1);
    bucket
        .add_feature(
            &feature(FeatureType::Polygon, polygon),
            polygon,
            &ImagePositions::default(),
            &PatternLayerMap::default(),
            0,
            &CanonicalTileID::new(0, 0, 0),
        )
        .unwrap();
    assert!(bucket.has_data());
    bucket.vertices.len()
}

#[test]
fn fill_extrusion_bucket_rounds_corners() {
    let square = collection(&[&[(0, 0), (100, 0), (100, 100), (0, 100), (0, 0)]]);

    // A ring of n points has n vertices, each the roof's and a wall's.
    assert_eq!(fill_extrusion_vertex_count(&square, 0.0), 5);
    // Rounding replaces each of the 4 corners with 5 points.
    assert_eq!(fill_extrusion_vertex_count(&square, 10.0), 21);
}

/// A fill bucket whose "fill-color" is the given data-driven expression, with
/// one polygon.
fn data_driven_fill_bucket(fill_color: &serde_json::Value) -> FillBucket {
    let json = serde_json::json!({"type": "fill", "id": "fill", "source": "s", "paint": {"fill-color": fill_color}});
    let mut error = ConversionError::default();
    let layer = convert_layer(Registry::with_builtins().style(), &Convertible::from(&json), &mut error)
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
    bucket
        .add_feature(
            &feature,
            &polygon,
            &ImagePositions::default(),
            &PatternLayerMap::default(),
            0,
            &CanonicalTileID::new(0, 0, 0),
        )
        .unwrap();
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

    with_context(|context| {
        // The bucket's upload moves its data to the render thread's
        // vectors, which hold it until they are uploaded.
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
        let buffer = context.upload_vertices(&vertices).expect("uploads").expect("a buffer");
        let mut triangles = DrawIndexes::from(bucket_indexes(&geometry, &bucket.triangles).expect("moved"));
        triangles.upload(context).expect("uploads");

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
        let reused = context.upload_vertices(&again).expect("uploads").expect("a buffer");
        assert!(Rc::ptr_eq(&buffer, &reused));

        // Data-driven paint values are freed too when nothing can rewrite them.
        let binders = bucket.paint_property_binders.get("fill").unwrap();
        assert!(!binders.depends_on_feature_state());
        let buffer = &binders.interleaved_vertex_buffer;
        let paint = paint_vertices(context, &geometry, buffer, false).expect("moved");
        assert!(paint.count() > 0);
        assert!(context.upload_vertices(&paint).expect("uploads").is_some());
        assert!(context.vertices(&paint).is_some_and(|stored| !stored.needs_upload()));
        assert!(buffer.shared_vertex_vector.data().is_empty());
    });
}

// gtest: Buckets.FillBucketKeepsPaintDataThatFeatureStateRewrites
#[test]
fn fill_bucket_keeps_paint_data_that_feature_state_rewrites() {
    let mut bucket = data_driven_fill_bucket(&serde_json::json!([
        "to-color",
        ["case", ["boolean", ["feature-state", "hover"], false], "red", "blue"]
    ]));

    with_context(|context| {
        let mut geometry = BucketGeometry::default();
        upload(&mut bucket, &mut geometry, context);
        let binders = bucket.paint_property_binders.get("fill").unwrap();
        assert!(binders.depends_on_feature_state());
        let buffer = &binders.interleaved_vertex_buffer;
        let paint = paint_vertices(context, &geometry, buffer, binders.depends_on_feature_state()).expect("a copy");
        assert!(paint.count() > 0);
        assert!(context.upload_vertices(&paint).expect("uploads").is_some());
        assert!(!buffer.shared_vertex_vector.is_moved());
        assert_eq!(buffer.shared_vertex_vector.data().len(), paint.count());
    });
}

// gtest: Buckets.LineBucket
#[test]
fn line_bucket() {
    let layout = LineLayoutProperties::PossiblyEvaluated::default();
    let mut bucket = LineBucket::new(layout, &VecMap::new(), 10.0, 1);
    assert!(!bucket.has_data());
    assert!(!bucket.needs_upload());

    // Ignore invalid feature type.
    let point = collection(&[&[(0, 0)]]);
    bucket
        .add_feature(
            &feature(FeatureType::Point, &point),
            &point,
            &ImagePositions::default(),
            &PatternLayerMap::default(),
            0,
            &CanonicalTileID::new(0, 0, 0),
        )
        .unwrap();
    assert!(!bucket.has_data());

    let line = collection(&[&[(0, 0), (1, 1)]]);
    bucket
        .add_feature(
            &feature(FeatureType::LineString, &line),
            &line,
            &ImagePositions::default(),
            &PatternLayerMap::default(),
            1,
            &CanonicalTileID::new(0, 0, 0),
        )
        .unwrap();
    assert!(bucket.has_data());
    assert!(bucket.needs_upload());

    with_context(|context| upload(&mut bucket, &mut BucketGeometry::default(), context));
    assert!(!bucket.needs_upload());
}

// gtest: Buckets.LineBucketDrawablesBuildFromFreedIndexes
#[test]
fn line_bucket_drawables_build_from_freed_indexes() {
    let mut bucket = LineBucket::new(LineLayoutProperties::PossiblyEvaluated::default(), &VecMap::new(), 10.0, 1);
    let line = collection(&[&[(0, 0), (100, 100), (200, 0)]]);
    bucket
        .add_feature(
            &feature(FeatureType::LineString, &line),
            &line,
            &ImagePositions::default(),
            &PatternLayerMap::default(),
            0,
            &CanonicalTileID::new(0, 0, 0),
        )
        .unwrap();
    assert!(bucket.has_data());

    with_context(|context| {
        let mut geometry = BucketGeometry::default();
        let index_count = bucket.triangles.len();
        upload(&mut bucket, &mut geometry, context);

        // An earlier draw put the indexes in a buffer, which frees their CPU
        // copy.
        let mut triangles = DrawIndexes::from(bucket_indexes(&geometry, &bucket.triangles).expect("moved"));
        triangles.upload(context).expect("uploads");
        assert!(!triangles.has_data(context));

        // A layer that rebuilds its draws of the tile builds the segments
        // from the counts.
        let indexes = DrawIndexes::from(bucket_indexes(&geometry, &bucket.triangles).expect("moved"));
        assert_eq!(indexes.len(context), index_count);
        assert_eq!(triangles.len(context), index_count);
    });
}

// gtest: Buckets.SymbolBucket
#[test]
fn symbol_bucket() {
    let layout = Arc::new(SymbolLayoutProperties::PossiblyEvaluated::default());
    let icons_need_linear = false;
    let sort_features_by_y = false;
    let bucket_leader_id = "test".to_owned();
    let symbol_instances = Vec::new();
    let symbol_ranges = Vec::new();

    let mut bucket = SymbolBucket::new(
        layout,
        &VecMap::new(),
        &PropertyValue::Constant(16.0),
        &PropertyValue::Constant(1.0),
        0.0,
        icons_need_linear,
        sort_features_by_y,
        bucket_leader_id,
        symbol_instances,
        SymbolInstancePools::default(),
        symbol_ranges,
        1.0,
        false,
        Vec::new(),
        false, /*iconsInText*/
    );
    assert!(!bucket.has_icon_data());
    assert!(!bucket.has_sdf_icon_data());
    assert!(!bucket.has_text_data());
    assert!(!bucket.has_icon_collision_box_data());
    assert!(!bucket.has_text_collision_box_data());
    assert!(!bucket.has_icon_collision_circle_data());
    assert!(!bucket.has_text_collision_circle_data());
    assert!(!bucket.has_data());
    assert!(!bucket.needs_upload());

    // SymbolBucket::addFeature() is a no-op.
    let point = collection(&[&[(0, 0)]]);
    bucket
        .add_feature(
            &feature(FeatureType::Point, &point),
            &point,
            &ImagePositions::default(),
            &PatternLayerMap::default(),
            0,
            &CanonicalTileID::new(0, 0, 0),
        )
        .unwrap();
    assert!(!bucket.has_data());
    assert!(!bucket.needs_upload());

    bucket.state.text.segments.push(SegmentBase::new(0, 0));
    assert!(bucket.has_text_data());
    assert!(bucket.has_data());
    assert!(bucket.needs_upload());

    with_context(|context| upload(&mut bucket, &mut BucketGeometry::default(), context));
    assert!(!bucket.needs_upload());
}

// gtest: Buckets.RasterBucket
#[test]
fn a_raster_buckets_upload_moves_its_quads_into_the_stores_once_per_mask() {
    let rgba = PremultipliedImage::new(Size::new(1, 1));
    let mut bucket = RasterBucket::new(rgba);
    assert!(bucket.has_data());
    bucket.set_mask(mask(&[(1, 0, 0), (1, 1, 1)]));

    with_context(|context| {
        bucket.upload(context);
        let geometry = bucket.geometry();
        let quads = geometry.quads().expect("the mask's quads");
        assert_eq!(context.vertices(&quads.vertices).expect("stored").count(), 8);
        assert!(geometry.vertices().is_empty(), "the upload moves the vertices");
        assert_eq!(quads.vertices.count(), 8);

        bucket.set_mask(mask(&[(1, 0, 0)]));
        assert!(!bucket.base.uploaded);
        bucket.upload(context);
        let quads = bucket.geometry().quads().expect("the new mask's quads");
        assert_eq!(context.vertices(&quads.vertices).expect("stored").count(), 4);

        bucket.set_mask(mask(&[(0, 0, 0)]));
        bucket.upload(context);
        assert!(bucket.geometry().quads().is_none(), "the whole tile draws the tile's quad");
    });
}

/// A segment's offsets and lengths, which the tests compare.
fn segment_tuples(segments: &[SegmentBase]) -> Vec<(usize, usize, usize, usize)> {
    segments.iter().map(|s| (s.vertex_offset, s.index_offset, s.vertex_length, s.index_length)).collect()
}

fn mask(ids: &[(u8, u32, u32)]) -> TileMask {
    let mut mask = TileMask::new();
    for &(z, x, y) in ids {
        mask.insert(CanonicalTileID::new(z, x, y));
    }
    mask
}

fn raster_vertex(p: (i16, i16), t: (u16, u16)) -> RasterLayoutVertex {
    RasterLayoutVertex::new(Point::new(p.0, p.1), Point::new(t.0, t.1))
}

// gtest: Buckets.RasterBucketMaskEmpty
#[test]
fn raster_bucket_mask_empty() {
    let mut bucket = RasterBucket::with_shared_image(None);
    bucket.set_mask(mask(&[]));
    let geometry = bucket.geometry();
    assert_eq!(Vec::<RasterLayoutVertex>::new(), geometry.vertices());
    assert_eq!(Vec::<u16>::new(), geometry.indices());
    assert_eq!(vec![(0, 0, 0, 0)], segment_tuples(geometry.segments()));
}

// gtest: Buckets.RasterBucketMaskNoChildren
#[test]
fn raster_bucket_mask_no_children() {
    let mut bucket = RasterBucket::with_shared_image(None);
    bucket.set_mask(mask(&[(0, 0, 0)]));

    // A mask of 0/0/0 doesn't produce buffers since we're instead using the
    // global shared buffers.
    let geometry = bucket.geometry();
    assert_eq!(Vec::<RasterLayoutVertex>::new(), geometry.vertices());
    assert_eq!(Vec::<u16>::new(), geometry.indices());
    assert_eq!(Vec::<(usize, usize, usize, usize)>::new(), segment_tuples(geometry.segments()));
}

// gtest: Buckets.RasterBucketMaskTwoChildren
#[test]
fn raster_bucket_mask_two_children() {
    let mut bucket = RasterBucket::with_shared_image(None);
    bucket.set_mask(mask(&[(1, 0, 0), (1, 1, 1)]));

    let geometry = bucket.geometry();
    assert_eq!(
        vec![
            // 1/0/1
            raster_vertex((0, 0), (0, 0)),
            raster_vertex((4096, 0), (4096, 0)),
            raster_vertex((0, 4096), (0, 4096)),
            raster_vertex((4096, 4096), (4096, 4096)),
            // 1/1/1
            raster_vertex((4096, 4096), (4096, 4096)),
            raster_vertex((8192, 4096), (8192, 4096)),
            raster_vertex((4096, 8192), (4096, 8192)),
            raster_vertex((8192, 8192), (8192, 8192)),
        ],
        geometry.vertices()
    );

    assert_eq!(
        vec![
            // 1/0/1
            0u16, 1, 2, 1, 2, 3, //
            // 1/1/1
            4, 5, 6, 5, 6, 7,
        ],
        geometry.indices()
    );

    assert_eq!(vec![(0, 0, 8, 12)], segment_tuples(geometry.segments()));
}

// gtest: Buckets.RasterBucketMaskComplex
#[test]
fn raster_bucket_mask_complex() {
    let mut bucket = RasterBucket::with_shared_image(None);
    bucket.set_mask(mask(&[(1, 0, 1), (1, 1, 0), (2, 2, 3), (2, 3, 2), (3, 6, 7), (3, 7, 6)]));

    let geometry = bucket.geometry();
    assert_eq!(
        vec![
            // 1/0/1
            raster_vertex((0, 4096), (0, 4096)),
            raster_vertex((4096, 4096), (4096, 4096)),
            raster_vertex((0, 8192), (0, 8192)),
            raster_vertex((4096, 8192), (4096, 8192)),
            // 1/1/0
            raster_vertex((4096, 0), (4096, 0)),
            raster_vertex((8192, 0), (8192, 0)),
            raster_vertex((4096, 4096), (4096, 4096)),
            raster_vertex((8192, 4096), (8192, 4096)),
            // 2/2/3
            raster_vertex((4096, 6144), (4096, 6144)),
            raster_vertex((6144, 6144), (6144, 6144)),
            raster_vertex((4096, 8192), (4096, 8192)),
            raster_vertex((6144, 8192), (6144, 8192)),
            // 2/3/2
            raster_vertex((6144, 4096), (6144, 4096)),
            raster_vertex((8192, 4096), (8192, 4096)),
            raster_vertex((6144, 6144), (6144, 6144)),
            raster_vertex((8192, 6144), (8192, 6144)),
            // 3/6/7
            raster_vertex((6144, 7168), (6144, 7168)),
            raster_vertex((7168, 7168), (7168, 7168)),
            raster_vertex((6144, 8192), (6144, 8192)),
            raster_vertex((7168, 8192), (7168, 8192)),
            // 3/7/6
            raster_vertex((7168, 6144), (7168, 6144)),
            raster_vertex((8192, 6144), (8192, 6144)),
            raster_vertex((7168, 7168), (7168, 7168)),
            raster_vertex((8192, 7168), (8192, 7168)),
        ],
        geometry.vertices()
    );

    assert_eq!(
        vec![
            // 1/0/1
            0u16, 1, 2, 1, 2, 3, //
            // 1/1/0
            4, 5, 6, 5, 6, 7, //
            // 2/2/3
            8, 9, 10, 9, 10, 11, //
            // 2/3/2
            12, 13, 14, 13, 14, 15, //
            // 3/6/7
            16, 17, 18, 17, 18, 19, //
            // 3/7/6
            20, 21, 22, 21, 22, 23,
        ],
        geometry.indices()
    );

    assert_eq!(vec![(0, 0, 24, 36)], segment_tuples(geometry.segments()));
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
    let layer = convert_layer(Registry::with_builtins().style(), &Convertible::from(&json), &mut error)
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
        bucket
            .add_feature(
                &feature,
                &line,
                &ImagePositions::default(),
                &PatternLayerMap::default(),
                index,
                &CanonicalTileID::new(0, 0, 0),
            )
            .unwrap();
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

// Stored vertices free their CPU copy once they are in a buffer.
#[test]
fn stored_vertices_free_their_data_once_they_have_a_buffer() {
    with_context(|context| {
        let vertices = context.add_vertices(vec![[1i16, 2], [3, 4]]).into_shared();
        assert!(context.vertices(&vertices).is_some_and(|stored| stored.needs_upload() && stored.bytes().len() == 8));

        context.upload_vertices(&vertices).expect("uploads").expect("a buffer");
        assert!(context.vertices(&vertices).is_some_and(|stored| !stored.needs_upload() && stored.bytes().is_empty()));
        assert_eq!(vertices.count(), 2);
    });
}

// The holder of typed vertices replaces them with as many vertices of the
// type, which the next upload writes over their buffer, counted as a buffer
// update, and which the draws that bind them bind again.
#[test]
fn typed_vertices_take_updates_of_as_many_vertices() {
    with_context(|context| {
        let typed = context.add_vertices(vec![[1i16, 2], [3, 4]]);
        let vertices = typed.shared().clone();
        let buffer = context.upload_vertices(&vertices).expect("uploads").expect("a buffer");
        assert!(context.vertices(&vertices).is_some_and(|stored| !stored.needs_upload()));

        assert!(!context.update_vertices(&typed, &[[5, 6]]), "another count");
        assert!(context.update_vertices(&typed, &[[5, 6], [7, 8]]));
        assert!(context.vertices(&vertices).is_some_and(|stored| stored.bytes()
            == bytemuck::cast_slice::<i16, u8>(&[5, 6, 7, 8])
            && stored.needs_upload()));
        let before = context.rendering_stats().clone();
        let rewritten = context.upload_vertices(&vertices).expect("uploads").expect("a buffer");
        assert!(Rc::ptr_eq(&buffer, &rewritten));
        let after = context.rendering_stats().clone();
        assert_eq!(after.vertex_update_bytes, before.vertex_update_bytes + 8);
        assert_eq!(after.buffer_updates, before.buffer_updates + 1);
        assert_eq!(after.num_vertex_buffers, before.num_vertex_buffers);
        assert!(context.vertices(&vertices).is_some_and(|stored| !stored.needs_upload()));

        // An upload without an update writes nothing.
        context.upload_vertices(&vertices).expect("uploads");
        assert_eq!(context.rendering_stats().buffer_updates, after.buffer_updates);
        assert_eq!(vertices.count(), 2);
    });
}

// A draw that binds vertices writes their update in its next upload, however
// soon after its last upload the update comes.
#[test]
fn a_draw_writes_an_update_of_the_vertices_it_binds() {
    use mln_gfx::shader_registry::ShaderRegistry;
    use mln_gfx::shaders::shader_defines::{ID_RASTER_POS_VERTEX_ATTRIBUTE, ID_RASTER_TEXTURE_POS_VERTEX_ATTRIBUTE};
    use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode};

    use crate::draw::{DrawGeometry, DrawObjects};
    use crate::gfx::vertex_attribute::VertexAttributeArray;

    with_context(|context| {
        let registry = ShaderRegistry::new(&mln_gfx::shaders::program_parameters::ProgramParameters::new(1.0, false));
        let shader = registry
            .shader("RasterShader", mln_gfx::projection_variant::ProjectionVariant::Mercator)
            .expect("the raster shader");
        let vertex = |x: i16| RasterLayoutVertex { a1: [x, x], a2: [0, 0] };
        let typed = context.add_vertices(vec![vertex(0), vertex(1), vertex(2)]);
        let stride = size_of::<RasterLayoutVertex>() as u32;
        let mut attrs = VertexAttributeArray::new();
        for (id, offset) in [(ID_RASTER_POS_VERTEX_ATTRIBUTE, 0), (ID_RASTER_TEXTURE_POS_VERTEX_ATTRIBUTE, 4)] {
            attrs.share(id, typed.shared().clone(), offset, 0, stride, AttributeDataType::Short2);
        }
        let geometry = DrawGeometry {
            vertices: attrs,
            vertex_count: 3,
            mode: DrawMode::Triangles(),
            indexes: DrawIndexes::new(vec![0, 1, 2]),
            segments: vec![SegmentBase::new(0, 0)],
            instances: None,
        };
        let mut objects = DrawObjects::new(
            shader,
            ColorMode::alpha_blended(),
            CullFaceMode::disabled(),
            geometry,
            Default::default(),
        );
        objects.upload(context);
        let uploaded = context.rendering_stats().buffer_updates;

        for x in 3..6 {
            assert!(context.update_vertices(&typed, &[vertex(x), vertex(x), vertex(x)]));
            objects.upload(context);
            assert!(context.vertices(typed.shared()).is_some_and(|stored| !stored.needs_upload()), "{x}");
            assert_eq!(context.rendering_stats().buffer_updates, uploaded + x as usize - 2, "{x}");
        }
        // Without an update, an upload writes nothing.
        objects.upload(context);
        assert_eq!(context.rendering_stats().buffer_updates, uploaded + 3);
    });
}

// What draws share stays in the context's stores while a handle to it lasts,
// and goes once the last one drops and the context destroys what the
// renderer released.
#[test]
fn store_entries_last_as_long_as_their_handles() {
    with_context(|context| {
        let vertices = context.add_vertices(vec![[1i16, 2]]).into_shared();
        let held = vertices.clone();
        let texture = context.add_texture();
        let draw_texture = texture.clone();
        drop(vertices);
        drop(texture);
        context.destroy_released();
        assert!(context.vertices(&held).is_some());
        assert!(context.texture(&draw_texture).is_some());

        let key = held.key();
        drop(held);
        drop(draw_texture);
        context.destroy_released();
        assert!(!context.has_vertices(key));
    });
}
