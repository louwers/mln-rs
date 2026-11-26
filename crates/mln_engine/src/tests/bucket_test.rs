use std::borrow::Cow;
use std::sync::Arc;

use feature::{Identifier, PropertyMap, Value};
use geometry::Point;
use mln_style::PropertyEvaluationParameters;
use mln_style::conversion::{Convertible, Error as ConversionError};
use mln_style::feature::{GeometryCollection, GeometryTileFeature};
use mln_style::geometry::FeatureType;
use mln_style::layer::{LayerStyle, convert_layer};
use mln_style::layers::fill_extrusion_layer_properties::FillExtrusionLayoutProperties;
use mln_style::layers::fill_layer_properties::FillPaintProperties;
use mln_style::layers::line_layer_properties::LineLayoutProperties;
use mln_style::layers::symbol_layer_properties::SymbolLayoutProperties;
use mln_style::layers::{Fill, Line};
use mln_style::property::evaluated::Evaluated;
use mln_style::property::value::PropertyValue;
use mln_style::subdivision_granularity::SubdivisionGranularitySetting;
use mln_style::tile_id::CanonicalTileID;
use mln_vec_map::VecMap;

use super::with_context;
use crate::Registry;
use crate::buckets::circle_bucket::CircleBucket;
use crate::buckets::fill_bucket::FillBucket;
use crate::buckets::fill_extrusion_bucket::{FillExtrusionBucket, tile_units_per_meter};
use crate::buckets::line_bucket::LineBucket;
use crate::buckets::raster_bucket::{MaskQuads, RasterLayoutVertex, mask_quads};
use crate::draw::context::Context;
use crate::draw::identity::SimpleIdentity;
use crate::draw::indexes::DrawIndexes;
use crate::draw::segment::SegmentBase;
use crate::gfx::line_mesh::LineLayoutVertex;
use crate::layer::bucket::{Bucket, ErasedBucket, Stored, Upload};
use crate::layer::bucket_geometry::{BucketGeometry, bucket_indexes, bucket_vertices, paint_vertices};
use crate::layer::layer_type::EvaluatedLayer;
use crate::layer::paint_property_binder::{LayerBinders, PaintBinders as _};
use crate::layout::pattern_layout::PatternLayerMap;
use crate::symbol::bucket::{SymbolBucket, SymbolBucketStyle, SymbolLayoutFlags};
use crate::symbol::instance::SymbolInstancePools;
use crate::tests::fixture_log::FixtureLog;
use crate::text::atlas::ImagePositions;
use crate::tile::OverscaledTileID;
use crate::tile_store::raster::Masking;
use crate::tile_store::tile_mask::TileMask;
use crate::util::tessellator::Tessellator;

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
    fn get_geometries(&self) -> Result<Cow<'_, GeometryCollection>, mln_style::error::Error> {
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
fn upload(bucket: &mut impl Bucket, geometry: &mut BucketGeometry, context: &mut Context) {
    bucket.upload(&mut Upload::geometry(geometry, &[], context));
}

/// What a bucket's layout does with each of its features: adds its geometry
/// and writes its paint values.
trait AddFeature {
    #[expect(clippy::too_many_arguments, reason = "the feature and what the layout lays it out with")]
    fn add_feature(
        &mut self,
        feature: &dyn GeometryTileFeature,
        geometry: &GeometryCollection,
        positions: &ImagePositions,
        patterns: &PatternLayerMap,
        index: usize,
        canonical: &CanonicalTileID,
        tessellator: &mut Tessellator,
    );
}

impl AddFeature for FillBucket {
    fn add_feature(
        &mut self,
        feature: &dyn GeometryTileFeature,
        geometry: &GeometryCollection,
        positions: &ImagePositions,
        patterns: &PatternLayerMap,
        index: usize,
        canonical: &CanonicalTileID,
        tessellator: &mut Tessellator,
    ) {
        self.add_geometry(geometry, canonical, tessellator);
        let length = self.vertices.len();
        self.paint_property_binders.populate(
            feature,
            length,
            index,
            positions,
            Some(patterns),
            canonical,
            &mln_style::expression::Value::Null,
        );
    }
}

impl AddFeature for FillExtrusionBucket {
    fn add_feature(
        &mut self,
        feature: &dyn GeometryTileFeature,
        geometry: &GeometryCollection,
        positions: &ImagePositions,
        patterns: &PatternLayerMap,
        index: usize,
        canonical: &CanonicalTileID,
        tessellator: &mut Tessellator,
    ) {
        self.add_geometry(geometry, canonical, tessellator);
        let length = self.vertices.len();
        self.paint_property_binders.populate(
            feature,
            length,
            index,
            positions,
            Some(patterns),
            canonical,
            &mln_style::expression::Value::Null,
        );
    }
}

impl AddFeature for LineBucket {
    fn add_feature(
        &mut self,
        feature: &dyn GeometryTileFeature,
        geometry: &GeometryCollection,
        positions: &ImagePositions,
        patterns: &PatternLayerMap,
        index: usize,
        canonical: &CanonicalTileID,
        tessellator: &mut Tessellator,
    ) {
        for line in geometry.iter() {
            self.add_geometry(line, feature, canonical, tessellator);
        }
        let length = self.vertices.len();
        self.paint_property_binders.populate(
            feature,
            length,
            index,
            positions,
            Some(patterns),
            canonical,
            &mln_style::expression::Value::Null,
        );
    }
}

/// A line bucket of a tile at zoom 10 for `layout`, of no layers.
fn empty_line_bucket(layout: LineLayoutProperties::PossiblyEvaluated) -> LineBucket {
    let tile = OverscaledTileID::new(10, 0, CanonicalTileID::new(10, 0, 0));
    LineBucket::new(layout, LayerBinders::default(), tile, SubdivisionGranularitySetting::none())
}

// gtest: Buckets.CircleBucket
#[test]
fn circle_bucket() {
    // A bucket uploads once, until it changes.
    let mut bucket = Stored::new(CircleBucket::default());

    with_context(|_, context| {
        bucket.upload(&mut BucketGeometry::default(), &[], context);
    });
}

// gtest: Buckets.FillBucket
#[test]
fn fill_bucket() {
    let mut bucket = FillBucket::new(LayerBinders::default(), SubdivisionGranularitySetting::none());
    assert!(!bucket.has_data());

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

    with_context(|_, context| {
        upload(&mut bucket, &mut BucketGeometry::default(), context);
    });
}

/// The vertices of a fill-extrusion bucket of `tile` holding `polygon`
/// extruded with `fill-extrusion-rounded-corner-distance` set to `meters`.
fn fill_extrusion_vertex_count(polygon: &GeometryCollection, meters: f32, tile: CanonicalTileID) -> usize {
    let layout = FillExtrusionLayoutProperties::PossiblyEvaluated {
        fill_extrusion_rounded_corner_distance: Evaluated::Constant(meters),
    };
    let mut bucket = FillExtrusionBucket::new(layout, LayerBinders::default(), SubdivisionGranularitySetting::none());
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
    let mut bucket = FillExtrusionBucket::new(layout, LayerBinders::default(), SubdivisionGranularitySetting::none());
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
    let mut bucket = FillExtrusionBucket::new(layout, LayerBinders::default(), SubdivisionGranularitySetting::none());
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
    let binders = LayerBinders::new(&[properties], 5.0);
    let mut bucket = FillBucket::new(binders, SubdivisionGranularitySetting::none());

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
        assert!(buffer.shared_vertex_vector.data().is_empty());
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
        bucket.upload(&mut Upload::geometry(&mut geometry, &[&copy_geometry], context));

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
        assert!(!buffer.shared_vertex_vector.is_moved());
        assert_eq!(buffer.shared_vertex_vector.data().len(), paint.count());
    });
}

// gtest: Buckets.LineBucket
#[test]
fn line_bucket() {
    let layout = LineLayoutProperties::PossiblyEvaluated::default();
    let mut bucket = empty_line_bucket(layout);
    assert!(!bucket.has_data());

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

    with_context(|_, context| {
        upload(&mut bucket, &mut BucketGeometry::default(), context);
    });
}

// gtest: Buckets.LineBucketDrawablesBuildFromFreedIndexes
#[test]
fn line_bucket_drawables_build_from_freed_indexes() {
    let mut bucket = empty_line_bucket(LineLayoutProperties::PossiblyEvaluated::default());
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

// gtest: Buckets.SymbolBucket
#[test]
fn symbol_bucket() {
    let layout = Arc::new(SymbolLayoutProperties::PossiblyEvaluated::default());
    let (instances, pools) = (Vec::new(), SymbolInstancePools::default());
    let mut bucket = SymbolBucket::new(
        SymbolBucketStyle {
            layout,
            paint_properties: &VecMap::new(),
            text_size: &PropertyValue::Constant(16.0),
            icon_size: &PropertyValue::Constant(1.0),
            zoom: 0.0,
            tile_pixel_ratio: 1.0,
            leader_id: Arc::from("test"),
        },
        SymbolLayoutFlags::default(),
        instances,
        pools,
        Vec::new(),
    );
    assert!(!bucket.has_icon_data());
    assert!(!bucket.has_sdf_icon_data());
    assert!(!bucket.has_text_data());
    assert!(!bucket.has_data());

    bucket.state.text.segments.push(SegmentBase::new(0, 0));
    assert!(bucket.has_text_data());
    assert!(bucket.has_data());

    with_context(|_, context| {
        upload(&mut bucket, &mut BucketGeometry::default(), context);
    });
}

// gtest: Buckets.RasterBucket
#[test]
fn a_layers_mask_quads_go_into_the_stores_once_per_mask() {
    let mut masking = Masking::default();
    masking.set(mask(&[(1, 0, 0), (1, 1, 1)]));
    let id = SimpleIdentity::new();
    let mut quads = MaskQuads::default();

    with_context(|_, context| {
        let first = quads.get(context, id, &masking).expect("the mask's quads");
        assert_eq!(context.vertices(&first.vertices).expect("stored").count(), 8);
        let first = first.vertices.clone();
        let again = quads.get(context, id, &masking).expect("the mask's quads");
        assert!(again.vertices == first, "made once per mask");

        masking.set(mask(&[(1, 0, 0)]));
        let quads_of_new_mask = quads.get(context, id, &masking).expect("the new mask's quads");
        assert_eq!(context.vertices(&quads_of_new_mask.vertices).expect("stored").count(), 4);

        masking.set(mask(&[(0, 0, 0)]));
        assert!(quads.get(context, id, &masking).is_none(), "the whole tile draws the tile's quad");
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
    let (vertices, indices, segments) = mask_quads(&mask(&[]));
    assert_eq!(Vec::<RasterLayoutVertex>::new(), vertices);
    assert_eq!(Vec::<u16>::new(), indices);
    assert_eq!(Vec::<(usize, usize, usize, usize)>::new(), segment_tuples(&segments));
}

// gtest: Buckets.RasterBucketMaskNoChildren
#[test]
fn raster_bucket_mask_no_children() {
    // A mask of 0/0/0 doesn't produce buffers since we're instead using the
    // global shared buffers.
    let (vertices, indices, segments) = mask_quads(&mask(&[(0, 0, 0)]));
    assert_eq!(Vec::<RasterLayoutVertex>::new(), vertices);
    assert_eq!(Vec::<u16>::new(), indices);
    assert_eq!(Vec::<(usize, usize, usize, usize)>::new(), segment_tuples(&segments));
}

// gtest: Buckets.RasterBucketMaskTwoChildren
#[test]
fn raster_bucket_mask_two_children() {
    let (vertices, indices, segments) = mask_quads(&mask(&[(1, 0, 0), (1, 1, 1)]));
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
        vertices
    );

    assert_eq!(
        vec![
            // 1/0/1
            0u16, 1, 2, 1, 2, 3, //
            // 1/1/1
            4, 5, 6, 5, 6, 7,
        ],
        indices
    );

    assert_eq!(vec![(0, 0, 8, 12)], segment_tuples(&segments));
}

// gtest: Buckets.RasterBucketMaskComplex
#[test]
fn raster_bucket_mask_complex() {
    let (vertices, indices, segments) =
        mask_quads(&mask(&[(1, 0, 1), (1, 1, 0), (2, 2, 3), (2, 3, 2), (3, 6, 7), (3, 7, 6)]));
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
        vertices
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
        indices
    );

    assert_eq!(vec![(0, 0, 24, 36)], segment_tuples(&segments));
}

// FillBucket.TracksSDFPatternsPerLayer
// gtest: FillBucket.TracksSDFPatternsPerLayer
#[test]
fn tracks_sdf_patterns_per_layer() {
    let log = FixtureLog::new();
    let mut bucket = FillBucket::new(LayerBinders::default(), SubdivisionGranularitySetting::none());

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
    let mut first_bucket = FillBucket::new(LayerBinders::default(), SubdivisionGranularitySetting::none());
    let mut second_bucket = FillBucket::new(LayerBinders::default(), SubdivisionGranularitySetting::none());

    first_bucket.record_sdf_pattern("mixed-layer", true);
    first_bucket.record_sdf_pattern("mixed-layer", false);
    first_bucket.record_sdf_pattern("mixed-layer", false);
    second_bucket.record_sdf_pattern("mixed-layer", true);
    second_bucket.record_sdf_pattern("mixed-layer", false);

    assert_eq!(
        2,
        log.count(
            log::Level::Warn,
            crate::log_target::STYLE,
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
    let mut bucket = empty_line_bucket(layout);
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

// Stored vertices free their CPU copy once they are in a buffer.
#[test]
fn stored_vertices_free_their_data_once_they_have_a_buffer() {
    with_context(|gpu, context| {
        let vertices = context.add_vertices(vec![[1i16, 2], [3, 4]]);
        assert!(context.vertices(&vertices).is_some_and(|stored| stored.needs_upload() && stored.bytes().len() == 8));

        context.upload_vertices(gpu, &vertices).expect("uploads").expect("a buffer");
        assert!(context.vertices(&vertices).is_some_and(|stored| !stored.needs_upload() && stored.bytes().is_empty()));
        assert_eq!(vertices.count(), 2);
    });
}

// Dynamic vertices' updates fill the storage of the bytes they were made of,
// which their uploads keep.
#[test]
fn dynamic_vertices_keep_their_storage_across_uploads() {
    with_context(|gpu, context| {
        let typed = context.add_dynamic_vertices(&[[1i16, 2], [3, 4]]);
        let storage = context.vertices(typed.shared()).map(|stored| stored.bytes().as_ptr());
        for x in 5..8 {
            context.upload_vertices(gpu, typed.shared()).expect("uploads").expect("a buffer");
            assert!(context.update_vertices(&typed, &[[x, x], [x, x]]));
            assert_eq!(context.vertices(typed.shared()).map(|stored| stored.bytes().as_ptr()), storage, "{x}");
        }
    });
}

// The holder of typed vertices replaces them with as many vertices of the
// type, which the next upload writes over their buffer, counted as a buffer
// update, and which the draws that bind them bind again.
#[test]
fn typed_vertices_take_updates_of_as_many_vertices() {
    with_context(|gpu, context| {
        let typed = context.add_dynamic_vertices(&[[1i16, 2], [3, 4]]);
        let vertices = typed.shared().clone();
        let buffer = context.upload_vertices(gpu, &vertices).expect("uploads").expect("a buffer");
        assert!(context.vertices(&vertices).is_some_and(|stored| !stored.needs_upload()));

        assert!(!context.update_vertices(&typed, &[[5, 6]]), "another count");
        assert!(context.update_vertices(&typed, &[[5, 6], [7, 8]]));
        assert!(context.vertices(&vertices).is_some_and(|stored| stored.bytes()
            == bytemuck::cast_slice::<i16, u8>(&[5, 6, 7, 8])
            && stored.needs_upload()));
        let before = context.rendering_stats().clone();
        let rewritten = context.upload_vertices(gpu, &vertices).expect("uploads").expect("a buffer");
        assert_eq!(buffer, rewritten);
        let after = context.rendering_stats().clone();
        assert_eq!(after.vertex_update_bytes, before.vertex_update_bytes + 8);
        assert_eq!(after.buffer_updates, before.buffer_updates + 1);
        assert_eq!(after.num_vertex_buffers, before.num_vertex_buffers);
        assert!(context.vertices(&vertices).is_some_and(|stored| !stored.needs_upload()));

        // An upload without an update writes nothing.
        context.upload_vertices(gpu, &vertices).expect("uploads");
        assert_eq!(context.rendering_stats().buffer_updates, after.buffer_updates);
        assert_eq!(vertices.count(), 2);
    });
}

// A draw that binds vertices writes their update in its next upload, however
// soon after its last upload the update comes.
#[test]
fn a_draw_writes_an_update_of_the_vertices_it_binds() {
    use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode};

    use crate::draw::objects::{DrawGeometry, DrawObjects};
    use crate::draw::vertex_attribute::{SharedAttribute, Uniforms, VertexAttributeArray};

    with_context(|gpu, context| {
        let variant = mln_gfx::projection_variant::ProjectionVariant::Mercator;
        let shader = context.shader_key(&crate::layers::raster::programs::RASTER, Uniforms::default(), variant);
        let vertex = |x: i16| RasterLayoutVertex { a1: [x, x], a2: [0, 0] };
        let typed = context.add_dynamic_vertices(&[vertex(0), vertex(1), vertex(2)]);
        let stride = size_of::<RasterLayoutVertex>() as u32;
        let mut attrs = VertexAttributeArray::new();
        for (name, offset) in [("pos", 0), ("texture_pos", 4)] {
            attrs.set_shared(
                name,
                SharedAttribute::new(typed.shared().clone(), AttributeDataType::Short2, offset, stride),
            );
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
        objects.upload(gpu, context);
        let uploaded = context.rendering_stats().buffer_updates;

        for x in 3..6 {
            assert!(context.update_vertices(&typed, &[vertex(x), vertex(x), vertex(x)]));
            objects.upload(gpu, context);
            assert!(context.vertices(typed.shared()).is_some_and(|stored| !stored.needs_upload()), "{x}");
            assert_eq!(context.rendering_stats().buffer_updates, uploaded + x as usize - 2, "{x}");
        }
        // Without an update, an upload writes nothing.
        objects.upload(gpu, context);
        assert_eq!(context.rendering_stats().buffer_updates, uploaded + 3);
    });
}

// A draw binds the buffer of the vertices it reads as a storage buffer as a
// buffer of its own, which its uploads write the vertices' updates to.
#[test]
fn a_draw_binds_the_vertices_it_reads_as_a_storage_buffer() {
    const ID: usize = crate::layers::fill_extrusion::programs::FILL_EXTRUSION_PULLED.buffer_id("wallVertices");
    use mln_gfx::{ColorMode, CullFaceMode, DrawMode};

    use crate::draw::objects::{DrawGeometry, DrawObjects};
    use crate::draw::vertex_attribute::{Uniforms, VertexAttributeArray};

    with_context(|gpu, context| {
        let variant = mln_gfx::projection_variant::ProjectionVariant::Mercator;
        let shader = context.shader_key(
            &crate::layers::fill_extrusion::programs::FILL_EXTRUSION_PULLED,
            Uniforms::default(),
            variant,
        );
        let typed = context.add_dynamic_vertices(&[[0u32; 2], [1; 2]]);
        let geometry = DrawGeometry {
            vertices: VertexAttributeArray::new(),
            vertex_count: 4,
            mode: DrawMode::Triangles(),
            indexes: DrawIndexes::new(vec![0, 2, 1, 1, 2, 3]),
            segments: vec![SegmentBase::new(0, 0)],
            instances: None,
        };
        let mut objects = DrawObjects::new(
            shader,
            ColorMode::alpha_blended(),
            CullFaceMode::back_ccw(),
            geometry,
            Default::default(),
        );
        objects.read_as_storage(ID, Some(typed.shared().clone()));
        assert!(objects.storage_buffer(ID).is_none(), "bound before the upload");

        objects.upload(gpu, context);
        let buffer = context.upload_vertices(gpu, typed.shared()).expect("uploads");
        assert!(buffer.is_some() && objects.storage_buffer(ID) == buffer);
        let uploaded = context.rendering_stats().buffer_updates;

        // An update goes to the same buffer.
        assert!(context.update_vertices(&typed, &[[2; 2], [3; 2]]));
        objects.upload(gpu, context);
        assert_eq!(context.rendering_stats().buffer_updates, uploaded + 1);
        assert_eq!(objects.storage_buffer(ID), buffer);

        objects.read_as_storage(ID, None);
        objects.upload(gpu, context);
        assert!(objects.storage_buffer(ID).is_none());
    });
}

// The pulled walls' shader finds a vertex's paint values where the paint
// attributes read them.
#[test]
fn the_paint_layout_of_pulled_walls_is_the_paint_attributes() {
    use mln_gfx::AttributeDataType::{Float, Float4};

    use crate::draw::vertex_attribute::{SharedAttribute, VertexAttributeArray};
    use crate::layers::render_fill_extrusion_layer::paint_layout;

    with_context(|_, context| {
        let mut attrs = VertexAttributeArray::new();
        assert_eq!(paint_layout(&attrs), 0, "every property a uniform");

        // A vertex's base, its color's four floats, then its height.
        let paint = context.add_vertices(vec![[0f32; 6]]);
        for (name, data_type, offset) in [("base", Float, 0), ("color", Float4, 4), ("height", Float, 20)] {
            attrs.set_shared(name, SharedAttribute::new(paint.clone(), data_type, offset, 24));
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

    use crate::draw::vertex_attribute::{SharedAttribute, VertexAttributeArray};
    use crate::layers::render_fill_layer::outline_paint_layout;

    with_context(|_, context| {
        let mut attrs = VertexAttributeArray::new();
        assert_eq!(outline_paint_layout(&attrs), 0, "every property a uniform");

        // A vertex's fill color, its opacity, then its outline color.
        let paint = context.add_vertices(vec![[0f32; 5]]);
        for (name, data_type, offset) in [("color", Float2, 0), ("opacity", Float, 8), ("outline_color", Float2, 12)] {
            attrs.set_shared(name, SharedAttribute::new(paint.clone(), data_type, offset, 20));
        }
        // Five words a vertex: the outline color at 3, the opacity at 2.
        assert_eq!(outline_paint_layout(&attrs), 0x0002_0305);

        // An interpolated outline color alone.
        let mut attrs = VertexAttributeArray::new();
        let paint = context.add_vertices(vec![[0f32; 4]]);
        attrs.set_shared("outline_color", SharedAttribute::new(paint, Float4, 0, 16));
        assert_eq!(outline_paint_layout(&attrs), 4);
    });
}

// What draws share stays in the context's stores while a handle to it lasts,
// and goes once the last one drops and the context destroys what the
// renderer released.
#[test]
fn store_entries_last_as_long_as_their_handles() {
    with_context(|gpu, context| {
        let vertices = context.add_vertices(vec![[1i16, 2]]);
        let held = vertices.clone();
        let texture = context.add_texture();
        let draw_texture = texture.clone();
        drop(vertices);
        drop(texture);
        context.destroy_released(gpu);
        assert!(context.vertices(&held).is_some());
        assert!(context.texture(&draw_texture).is_some());

        let key = held.key();
        drop(held);
        drop(draw_texture);
        context.destroy_released(gpu);
        assert!(!context.has_vertices(key));
    });
}
