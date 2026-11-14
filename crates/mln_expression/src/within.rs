// Integer arithmetic on tile coordinates wraps where the C++ int64_t
// arithmetic could overflow (UB in C++; it wraps on the targets we build
// for). Rings and lines with fewer than two points walk no edges (C++ reads
// out of bounds for an empty one).

use std::f64::consts::PI;

use feature::{Value as MbglValue, ValueObject};
use geo_types::{Coord, Geometry, Polygon};
use geojson::GeoJson;
use geometry::Point;
use mln_conversion::{Convertible, Error};
use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::{GeometryCoordinates, GeometryTileFeature};
use mln_util::constants::{DEGREES_MAX, EXTENT, LONGITUDE_MAX};
use mln_util::geometry::{FeatureType, to_feature_type};
use mln_util::logging::{Event, Log};

use crate::expression::{Dependency, EvaluationContext, EvaluationResult, Expression, ExpressionBase, Kind, downcast};
use crate::parsing_context::{ParseResult, ParsingContext};
use crate::type_::Type;
use crate::value::Value;

// --- util/geometry_util ---

/// A bounding box: min x, min y, max x, max y.
type WithinBBox = [i64; 4];
const DEFAULT_WITHIN_BBOX: WithinBBox = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];

type TilePolygon = Vec<Vec<Point<i64>>>;

fn update_bbox(bbox: &mut WithinBBox, p: &Point<i64>) {
    bbox[0] = bbox[0].min(p.x);
    bbox[1] = bbox[1].min(p.y);
    bbox[2] = bbox[2].max(p.x);
    bbox[3] = bbox[3].max(p.y);
}

/// Check if bbox1 is within bbox2.
fn box_within_box(bbox1: &WithinBBox, bbox2: &WithinBBox) -> bool {
    if bbox1[0] <= bbox2[0] {
        return false;
    }
    if bbox1[2] >= bbox2[2] {
        return false;
    }
    if bbox1[1] <= bbox2[1] {
        return false;
    }
    if bbox1[3] >= bbox2[3] {
        return false;
    }
    true
}

fn ray_intersect(p: &Point<i64>, p1: &Point<i64>, p2: &Point<i64>) -> bool {
    ((p1.y > p.y) != (p2.y > p.y))
        && (p.x
            < (p2.x.wrapping_sub(p1.x))
                .wrapping_mul(p.y.wrapping_sub(p1.y))
                .wrapping_div(p2.y.wrapping_sub(p1.y))
                .wrapping_add(p1.x))
}

/// Check if point p is on line segment with end points p1 and p2.
fn point_on_boundary(p: &Point<i64>, p1: &Point<i64>, p2: &Point<i64>) -> bool {
    // requirements of point p on line segment:
    // 1. colinear: cross product of vector p->p1(x1, y1) and vector
    //    p->p2(x2, y2) equals to 0
    // 2. p is between p1 and p2
    let x1 = p.x.wrapping_sub(p1.x);
    let y1 = p.y.wrapping_sub(p1.y);
    let x2 = p.x.wrapping_sub(p2.x);
    let y2 = p.y.wrapping_sub(p2.y);
    (x1.wrapping_mul(y2).wrapping_sub(x2.wrapping_mul(y1)) == 0)
        && (x1.wrapping_mul(x2) <= 0)
        && (y1.wrapping_mul(y2) <= 0)
}

fn segment_intersect_segment(a: &Point<i64>, b: &Point<i64>, c: &Point<i64>, d: &Point<i64>) -> bool {
    // a, b are end points for line segment1, c and d are end points for
    // line segment2
    let perp = |v1: &Point<i64>, v2: &Point<i64>| v1.x.wrapping_mul(v2.y).wrapping_sub(v1.y.wrapping_mul(v2.x));

    // check if two segments are parallel or not
    // precondition is end point a, b is inside polygon, if line a->b is
    // parallel to polygon edge c->d, then a->b won't intersect with c->d
    let vector_p = Point::new(b.x.wrapping_sub(a.x), b.y.wrapping_sub(a.y));
    let vector_q = Point::new(d.x.wrapping_sub(c.x), d.y.wrapping_sub(c.y));
    if perp(&vector_q, &vector_p) == 0 {
        return false;
    }

    // check if p1 and p2 are in different sides of line segment q1->q2
    let two_sided = |p1: &Point<i64>, p2: &Point<i64>, q1: &Point<i64>, q2: &Point<i64>| {
        // q1->p1 (x1, y1), q1->p2 (x2, y2), q1->q2 (x3, y3)
        let x1 = p1.x.wrapping_sub(q1.x);
        let y1 = p1.y.wrapping_sub(q1.y);
        let x2 = p2.x.wrapping_sub(q1.x);
        let y2 = p2.y.wrapping_sub(q1.y);
        let x3 = q2.x.wrapping_sub(q1.x);
        let y3 = q2.y.wrapping_sub(q1.y);
        let ret1 = x1.wrapping_mul(y3).wrapping_sub(x3.wrapping_mul(y1));
        let ret2 = x2.wrapping_mul(y3).wrapping_sub(x3.wrapping_mul(y2));
        (ret1 > 0 && ret2 < 0) || (ret1 < 0 && ret2 > 0)
    };

    // If lines are intersecting with each other, the relative location
    // should be: a and b lie in different sides of segment c->d, c and d lie
    // in different sides of segment a->b
    two_sided(a, b, c, d) && two_sided(c, d, a, b)
}

fn line_intersect_polygon(p1: &Point<i64>, p2: &Point<i64>, polygon: &TilePolygon) -> bool {
    // loop through every edge of every ring
    polygon.iter().any(|ring| ring.windows(2).any(|edge| segment_intersect_segment(p1, p2, &edge[0], &edge[1])))
}

/// Ray casting algorithm for detecting if point is in polygon.
fn point_within_polygon(point: &Point<i64>, polygon: &TilePolygon, true_on_boundary: bool) -> bool {
    let mut within = false;
    for ring in polygon {
        // loop through every edge of the ring
        for edge in ring.windows(2) {
            if point_on_boundary(point, &edge[0], &edge[1]) {
                return true_on_boundary;
            }
            if ray_intersect(point, &edge[0], &edge[1]) {
                within = !within;
            }
        }
    }
    within
}

fn point_within_polygons(point: &Point<i64>, polygons: &[TilePolygon], true_on_boundary: bool) -> bool {
    polygons.iter().any(|polygon| point_within_polygon(point, polygon, true_on_boundary))
}

fn line_string_within_polygon(line: &[Point<i64>], polygon: &TilePolygon) -> bool {
    // First, check if geometry points of line segments are all inside polygon
    if !line.iter().all(|point| point_within_polygon(point, polygon, false)) {
        return false;
    }

    // Second, check if there is line segment intersecting polygon edge
    !line.windows(2).any(|segment| line_intersect_polygon(&segment[0], &segment[1], polygon))
}

fn line_string_within_polygons(line: &[Point<i64>], polygons: &[TilePolygon]) -> bool {
    polygons.iter().any(|polygon| line_string_within_polygon(line, polygon))
}

// --- within.cpp ---

fn lat_lon_to_tile_coordinates(point: &Coord<f64>, canonical: &CanonicalTileID) -> Point<i64> {
    let size = f64::from(EXTENT) * 2f64.powi(i32::from(canonical.z));

    let x = (point.x + LONGITUDE_MAX) * size / DEGREES_MAX;
    let y =
        (LONGITUDE_MAX - mln_math::rad2deg((point.y * PI / DEGREES_MAX + PI / 4.0).tan().ln())) * size / DEGREES_MAX;

    // `util::clamp<int64_t>(static_cast<int64_t>(x), min, max)`; `as`
    // saturates where the C++ cast is UB.
    Point::new(x as i64, y as i64)
}

fn get_tile_polygon(polygon: &Polygon<f64>, canonical: &CanonicalTileID, bbox: &mut WithinBBox) -> TilePolygon {
    let mut result = Vec::with_capacity(1 + polygon.interiors().len());
    for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
        let mut temp = Vec::with_capacity(ring.0.len());
        for p in &ring.0 {
            let coord = lat_lon_to_tile_coordinates(p, canonical);
            temp.push(coord);
            update_bbox(bbox, &coord);
        }
        result.push(temp);
    }
    result
}

fn get_tile_polygons(
    polygon_geo_set: &Geometry<f64>,
    canonical: &CanonicalTileID,
    bbox: &mut WithinBBox,
) -> Vec<TilePolygon> {
    match polygon_geo_set {
        Geometry::MultiPolygon(polygons) => polygons.iter().map(|pg| get_tile_polygon(pg, canonical, bbox)).collect(),
        Geometry::Polygon(polygon) => vec![get_tile_polygon(polygon, canonical, bbox)],
        Geometry::Rect(rect) => vec![get_tile_polygon(&rect.to_polygon(), canonical, bbox)],
        Geometry::Triangle(triangle) => vec![get_tile_polygon(&triangle.to_polygon(), canonical, bbox)],
        _ => Vec::new(),
    }
}

fn update_point(p: &mut Point<i64>, bbox: &mut WithinBBox, poly_bbox: &WithinBBox, world_size: i64) {
    if p.x < poly_bbox[0] || p.x > poly_bbox[2] {
        let get_shift = |point: &Point<i64>, polygon_side: i64, size: i64| -> i64 {
            if point.x.wrapping_sub(polygon_side) > size / 2 {
                return -size;
            }
            if polygon_side.wrapping_sub(point.x) > size / 2 {
                return size;
            }
            0
        };

        let mut shift = get_shift(p, poly_bbox[0], world_size);
        if shift == 0 {
            shift = get_shift(p, poly_bbox[2], world_size);
        }
        p.x = p.x.wrapping_add(shift);
    }
    update_bbox(bbox, p);
}

/// `EXTENT * coordinate`, wrapping as in C++: an int32 times a uint32 is a
/// uint32 product there, widened to `int64_t` afterwards.
fn tile_shift(coordinate: u32) -> i64 {
    i64::from((EXTENT as u32).wrapping_mul(coordinate))
}

fn world_size(canonical: &CanonicalTileID) -> i64 {
    (f64::from(EXTENT) * 2f64.powi(i32::from(canonical.z))) as i64
}

fn get_tile_points(
    points: &GeometryCoordinates,
    canonical: &CanonicalTileID,
    bbox: &mut WithinBBox,
    poly_bbox: &WithinBBox,
) -> Vec<Point<i64>> {
    let x_shift = tile_shift(canonical.x);
    let y_shift = tile_shift(canonical.y);
    let world_size = world_size(canonical);
    let mut results = Vec::with_capacity(points.len());
    for p in points {
        let mut point = Point::new(i64::from(p.x) + x_shift, i64::from(p.y) + y_shift);
        update_point(&mut point, bbox, poly_bbox, world_size);
        results.push(point);
    }
    results
}

fn get_tile_lines(
    lines: &[GeometryCoordinates],
    canonical: &CanonicalTileID,
    bbox: &mut WithinBBox,
    poly_bbox: &WithinBBox,
) -> Vec<Vec<Point<i64>>> {
    let x_shift = tile_shift(canonical.x);
    let y_shift = tile_shift(canonical.y);
    let mut results = Vec::with_capacity(lines.len());
    for line in lines {
        let mut line_string = Vec::with_capacity(line.len());
        for p in line {
            let point = Point::new(i64::from(p.x) + x_shift, i64::from(p.y) + y_shift);
            update_bbox(bbox, &point);
            line_string.push(point);
        }
        results.push(line_string);
    }

    let world_size = world_size(canonical);
    if bbox[2].wrapping_sub(bbox[0]) <= world_size / 2 {
        *bbox = DEFAULT_WITHIN_BBOX;
        for line in &mut results {
            for p in line.iter_mut() {
                update_point(p, bbox, poly_bbox, world_size);
            }
        }
    }
    results
}

fn feature_within_polygons(
    feature: &dyn GeometryTileFeature,
    canonical: &CanonicalTileID,
    polygon_geo_set: &Geometry<f64>,
) -> bool {
    let mut poly_bbox = DEFAULT_WITHIN_BBOX;
    let polygons = get_tile_polygons(polygon_geo_set, canonical, &mut poly_bbox);
    // An empty multi-polygon contains nothing.
    if polygons.is_empty() {
        return false;
    }
    let geometries = feature.get_geometries();
    match feature.get_type() {
        FeatureType::Point => {
            // C++ asserts a geometry and reads `geometries.at(0)` (which
            // throws when there is none).
            let Some(first) = geometries.first() else {
                return false;
            };
            let mut point_bbox = DEFAULT_WITHIN_BBOX;
            let points = get_tile_points(first, canonical, &mut point_bbox, &poly_bbox);
            if !box_within_box(&point_bbox, &poly_bbox) {
                return false;
            }

            points.iter().all(|p| point_within_polygons(p, &polygons, false))
        }
        FeatureType::LineString => {
            let mut line_bbox = DEFAULT_WITHIN_BBOX;
            let multi_line_string = get_tile_lines(geometries, canonical, &mut line_bbox, &poly_bbox);
            if !box_within_box(&line_bbox, &poly_bbox) {
                return false;
            }

            multi_line_string.iter().all(|line| line_string_within_polygons(line, &polygons))
        }
        _ => false,
    }
}

const POLYGON_REQUIRED: &str = "'within' expression requires valid geojson source that contains polygon geometry type.";

fn parse_value(value: &Convertible<'_>, ctx: &mut ParsingContext) -> Option<GeoJson> {
    if value.is_object() {
        let mut error = Error::default();
        let geojson = value.to_geojson(&mut error);
        if let Some(geojson) = geojson
            && error.message.is_empty()
        {
            return Some(geojson);
        }
        ctx.error(error.message);
    }

    ctx.error(POLYGON_REQUIRED.to_owned());
    None
}

fn get_polygon_info(geometry: Option<&geojson::Geometry>, ctx: &mut ParsingContext) -> Option<Geometry<f64>> {
    match geometry.map(|g| feature::geojson::geometry(g.clone())) {
        Some(Ok(geometry)) if to_feature_type(Some(&geometry)) == FeatureType::Polygon => return Some(geometry),
        Some(Err(e)) => {
            ctx.error(e.to_string());
            return None;
        }
        _ => {}
    }
    ctx.error(POLYGON_REQUIRED.to_owned());
    None
}

/// The geometry of each feature of `geojson`; a bare geometry is one.
pub(crate) fn geojson_geometries(geojson: &GeoJson) -> Vec<Option<&geojson::Geometry>> {
    match geojson {
        GeoJson::Geometry(geometry) => vec![Some(geometry)],
        GeoJson::Feature(feature) => vec![feature.geometry.as_ref()],
        GeoJson::FeatureCollection(collection) => collection.features.iter().map(|f| f.geometry.as_ref()).collect(),
    }
}

pub(crate) struct Within {
    base: ExpressionBase,
    geojson_source: GeoJson,
    geometries: Geometry<f64>,
}

impl Within {
    #[must_use]
    pub(crate) fn new(geojson: GeoJson, geometries: Geometry<f64>) -> Self {
        Within {
            base: ExpressionBase::new(Kind::Within, Type::Boolean, Dependency::FEATURE),
            geojson_source: geojson,
            geometries,
        }
    }

    pub(crate) fn parse(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
        if value.is_array() {
            // object value, quoted with ["within", value]
            if value.array_length() != 2 {
                ctx.error(format!(
                    "'within' expression requires exactly one argument, but found {} instead.",
                    value.array_length().wrapping_sub(1)
                ));
                return None;
            }

            let parsed_value = parse_value(&value.array_member(1), ctx)?;

            let geometries = geojson_geometries(&parsed_value);
            // The first polygon wins; every non-polygon before it reports an
            // error.
            for geometry in &geometries {
                if let Some(ret) = get_polygon_info(*geometry, ctx) {
                    return Some(Box::new(Within::new(parsed_value.clone(), ret)));
                }
            }
            // C++ reports nothing for an empty collection, and its parser
            // then asserts that a failed parse reported an error.
            if geometries.is_empty() {
                ctx.error(POLYGON_REQUIRED.to_owned());
            }
            return None;
        }
        ctx.error("'within' expression needs to be an array with exactly one argument.".to_owned());
        None
    }
}

fn value_converter(v: &serde_json::Value) -> MbglValue {
    match v {
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                MbglValue::Int(i)
            } else if let Some(u) = n.as_u64() {
                MbglValue::Uint(u)
            } else {
                MbglValue::Double(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::Bool(b) => MbglValue::Bool(*b),
        serde_json::Value::String(s) => MbglValue::String(s.clone()),
        serde_json::Value::Array(a) => MbglValue::Array(a.iter().map(value_converter).collect()),
        serde_json::Value::Object(o) => {
            MbglValue::Object(o.iter().map(|(k, v)| (k.clone(), value_converter(v))).collect())
        }
        // Ignore other types as valid geojson only contains above types.
        serde_json::Value::Null => MbglValue::Null,
    }
}

impl Expression for Within {
    fn base(&self) -> &ExpressionBase {
        &self.base
    }

    fn evaluate(&self, params: &EvaluationContext<'_>) -> EvaluationResult {
        let (Some(feature), Some(canonical)) = (params.feature, params.canonical) else {
            return Ok(Value::Bool(false));
        };
        let geometry_type = feature.get_type();
        // Currently only support Point and LineString types in Polygon/Polygons
        if geometry_type == FeatureType::Point || geometry_type == FeatureType::LineString {
            return Ok(Value::Bool(feature_within_polygons(feature, canonical, &self.geometries)));
        }
        Log::warning(Event::General, "within expression currently only support Point/LineString geometry type.");

        Ok(Value::Bool(false))
    }

    fn each_child<'a>(&'a self, _visit: &mut dyn FnMut(&'a dyn Expression)) {}

    fn equals(&self, e: &dyn Expression) -> bool {
        if e.get_kind() == Kind::Within
            && let Some(rhs) = downcast::<Within>(e)
        {
            return self.geojson_source == rhs.geojson_source && self.geometries == rhs.geometries;
        }
        false
    }

    fn possible_outputs(&self) -> Vec<Option<Value>> {
        vec![Some(Value::Bool(true)), Some(Value::Bool(false))]
    }

    fn serialize(&self) -> MbglValue {
        let value = serde_json::to_value(&self.geojson_source).unwrap_or_default();
        let serialized: ValueObject = if let serde_json::Value::Object(o) = &value {
            o.iter().map(|(k, v)| (k.clone(), value_converter(v))).collect()
        } else {
            Log::error(Event::General, "Failed to serialize 'within' expression, converted rapidJSON is not an object");
            ValueObject::new()
        };
        MbglValue::Array(vec![MbglValue::String(self.get_operator()), MbglValue::Object(serialized)])
    }

    fn get_operator(&self) -> String {
        "within".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_feature_collection_is_a_parse_error() {
        let document = serde_json::json!(["within", {"type": "FeatureCollection", "features": []}]);
        let mut ctx = ParsingContext::new();
        assert!(ctx.parse_expression(&Convertible::from(&document), None).is_none());
        assert_eq!(ctx.get_errors().len(), 1);
        assert_eq!(ctx.get_errors()[0].message, POLYGON_REQUIRED);
    }

    struct PointFeature(mln_tile_id::geometry_tile_data::GeometryCollection);

    impl GeometryTileFeature for PointFeature {
        fn get_type(&self) -> FeatureType {
            FeatureType::Point
        }

        fn get_value(&self, _key: &str) -> Option<MbglValue> {
            None
        }

        fn get_geometries(&self) -> &mln_tile_id::geometry_tile_data::GeometryCollection {
            &self.0
        }
    }

    /// An empty multi-polygon parses, and contains nothing.
    #[test]
    fn nothing_is_within_an_empty_multi_polygon() {
        let document = serde_json::json!(["within", {"type": "MultiPolygon", "coordinates": []}]);
        let mut ctx = ParsingContext::new();
        let within = ctx.parse_expression(&Convertible::from(&document), None).expect("parses");
        let feature = PointFeature(vec![vec![Point::new(10, 10)]].into());
        let canonical = CanonicalTileID::new(3, 3, 3);
        let params = EvaluationContext::from_feature(Some(&feature)).with_canonical_tile_id(Some(&canonical));
        assert_eq!(within.evaluate(&params), Ok(Value::Bool(false)));
    }
}
