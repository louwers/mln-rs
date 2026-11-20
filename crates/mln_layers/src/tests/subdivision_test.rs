use std::borrow::Cow;

use feature::{Identifier, PropertyMap, Value};
use geometry::Point;
use mln_gfx::shaders::segment::MAX_SEGMENT_VERTICES;
use mln_renderer::unstable::{Tessellator, subdivide_polygon};
use mln_style_types::image::ImagePositions;
use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::{
    GeometryCollection, GeometryCoordinate, GeometryCoordinates, GeometryTileFeature,
};
use mln_util::constants::EXTENT;
use mln_util::geometry::FeatureType;
use mln_util::subdivision_granularity::{SubdivisionGranularityExpression, SubdivisionGranularitySetting};
use mln_vec_map::VecMap;

use crate::fill_extrusion::FillExtrusionLayoutProperties;
use crate::fill_extrusion::bucket::FillExtrusionBucket;
use crate::pattern_layout::{PatternBucket, PatternLayerMap};

/// `EXTENT` as a tile coordinate.
const EXTENT_I16: i16 = EXTENT as i16;

fn p(x: i16, y: i16) -> GeometryCoordinate {
    Point::new(x, y)
}

fn square(x0: i16, y0: i16, x1: i16, y1: i16) -> GeometryCollection {
    GeometryCollection::from(vec![vec![p(x0, y0), p(x1, y0), p(x1, y1), p(x0, y1), p(x0, y0)]])
}

struct StubGeometryTileFeature {
    properties: PropertyMap,
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
        Identifier::Null
    }
    fn get_geometries(&self) -> Result<Cow<'_, GeometryCollection>, mln_util::error::Error> {
        Ok(Cow::Borrowed(&self.geometry))
    }
}

// Extrudes a polygon on a globe tile and checks that every segment has
// something to draw, stays within its 16-bit indices and keeps to its own
// vertices.
fn extrude_on_the_globe(polygon: &GeometryCollection, tile: &CanonicalTileID) -> FillExtrusionBucket {
    let mut bucket =
        FillExtrusionBucket::new(FillExtrusionLayoutProperties::PossiblyEvaluated::default(), &VecMap::new(), 0.0, 1);
    bucket.set_subdivision_granularity(&SubdivisionGranularitySetting::globe());
    let feature = StubGeometryTileFeature {
        properties: PropertyMap::new(),
        type_: FeatureType::Polygon,
        geometry: polygon.clone(),
    };
    bucket.add_feature(
        &feature,
        polygon,
        &ImagePositions::default(),
        &PatternLayerMap::default(),
        0,
        tile,
        &mut Tessellator::default(),
    );
    let mut segments_end = 0;
    let mut indices_outside_their_segment = 0;
    let triangles = bucket.triangles.data();
    for segment in &bucket.triangle_segments {
        assert!(segment.vertex_length <= MAX_SEGMENT_VERTICES);
        assert!(segment.index_length > 0);
        assert!(segment.vertex_offset >= segments_end);
        for i in segment.index_offset..segment.index_offset + segment.index_length {
            indices_outside_their_segment += usize::from(usize::from(triangles[i]) >= segment.vertex_length);
        }
        segments_end = segment.vertex_offset + segment.vertex_length;
    }
    drop(triangles);
    assert_eq!(0, indices_outside_their_segment);
    assert!(segments_end <= bucket.vertices.len());
    bucket
}

// A ring with a fine zigzag along two of its sides: about 9,600 vertices on
// a tile, whatever the granularity.
fn fine_zigzag() -> GeometryCollection {
    let mut ring = GeometryCoordinates::new();
    for x in 0..EXTENT_I16 {
        ring.push(p(x, if x % 2 == 0 { 0 } else { 2 }));
    }
    for y in (0..EXTENT_I16).step_by(6) {
        ring.push(p(if y % 12 == 0 { EXTENT_I16 } else { EXTENT_I16 - 2 }, y));
    }
    ring.push(p(EXTENT_I16, EXTENT_I16));
    ring.push(p(0, EXTENT_I16));
    ring.push(p(0, 0));
    GeometryCollection::from(vec![ring])
}

// The granularity the roof was subdivided at, found from the triangles drawn
// last; 0 when there is none.
fn roof_granularity(bucket: &FillExtrusionBucket, polygon: &GeometryCollection, tile: &CanonicalTileID) -> u32 {
    let mut drawn: Vec<[i16; 6]> = Vec::new();
    let (vertices, triangles) = (bucket.vertices.data(), bucket.triangles.data());
    for segment in &bucket.triangle_segments {
        let mut i = segment.index_offset;
        while i + 3 <= segment.index_offset + segment.index_length {
            let mut triangle = [0i16; 6];
            for k in 0..3 {
                let position = vertices[segment.vertex_offset + usize::from(triangles[i + k])].a1;
                triangle[k * 2] = position[0];
                triangle[k * 2 + 1] = position[1];
            }
            drawn.push(triangle);
            i += 3;
        }
    }
    let mut granularity = SubdivisionGranularitySetting::globe().fill.granularity_for_zoom_level(tile.z);
    while granularity >= 1 {
        let roof = subdivide_polygon(polygon, tile, granularity, false, &mut Tessellator::default());
        let count = roof.triangles.len();
        if count <= drawn.len() {
            let matches = roof.triangles.iter().zip(&drawn[drawn.len() - count..]).all(|(triangle, drawn)| {
                (0..3).all(|k| {
                    let vertex = roof.vertices[triangle[k] as usize];
                    drawn[k * 2] == vertex.x && drawn[k * 2 + 1] == vertex.y
                })
            });
            if matches {
                return granularity;
            }
        }
        granularity /= 2;
    }
    0
}

// gtest: Subdivision.Granularity
#[test]
fn granularity() {
    let fill = SubdivisionGranularityExpression::new(128, 2);
    assert_eq!(128, fill.granularity_for_zoom_level(0));
    assert_eq!(64, fill.granularity_for_zoom_level(1));
    assert_eq!(2, fill.granularity_for_zoom_level(6));
    assert_eq!(2, fill.granularity_for_zoom_level(12));
    assert_eq!(1, SubdivisionGranularityExpression::new(0, 0).granularity_for_zoom_level(3));
}

// gtest: Subdivision.FillExtrusionRoofCountsItsVerticesOnce
#[test]
fn fill_extrusion_roof_counts_its_vertices_once() {
    // A tile-sized roof at zoom 0 has 16,899 vertices: too many to count five
    // times over, few enough to fit once.
    let tile = CanonicalTileID::new(0, 0, 0);
    let polygon = square(0, 0, EXTENT_I16, EXTENT_I16);
    assert_eq!(128, roof_granularity(&extrude_on_the_globe(&polygon, &tile), &polygon, &tile));
}

// gtest: Subdivision.FillExtrusionRoofBesideLongWallsTakesItsOwnSegment
#[test]
fn fill_extrusion_roof_beside_long_walls_takes_its_own_segment() {
    // The zigzag makes a ring of 9,644 vertices, which only the walls draw,
    // and a roof of 59,160 at the zoom's granularity, few enough for a
    // segment of their own.
    let polygon = fine_zigzag();
    let tile = CanonicalTileID::new(2, 1, 1);
    let bucket = extrude_on_the_globe(&polygon, &tile);
    let roof = subdivide_polygon(&polygon, &tile, 32, false, &mut Tessellator::default());
    let last = bucket.triangle_segments.last().expect("segment");
    assert_eq!(roof.vertices.len(), last.vertex_length);
    assert_eq!(roof.triangles.len() * 3, last.index_length);
    assert_eq!(32, roof_granularity(&bucket, &polygon, &tile));
}

// gtest: Subdivision.FillExtrusionRoofBeyondTheIndexLimitKeepsItsGranularity
#[test]
fn fill_extrusion_roof_beyond_the_index_limit_keeps_its_granularity() {
    // At zoom 1 the zigzag's roof needs more vertices than a 16-bit index
    // space holds, so it is spread over several segments, as GL JS does,
    // rather than coarsened.
    let polygon = fine_zigzag();
    let tile = CanonicalTileID::new(1, 0, 0);
    assert!(
        subdivide_polygon(&polygon, &tile, 64, false, &mut Tessellator::default()).vertices.len()
            > MAX_SEGMENT_VERTICES
    );
    let bucket = extrude_on_the_globe(&polygon, &tile);
    assert!(bucket.triangle_segments.len() >= 2);
    assert_eq!(64, roof_granularity(&bucket, &polygon, &tile));
}
