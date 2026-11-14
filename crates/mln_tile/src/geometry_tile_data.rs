// Polygon fixup runs on i_overlay, where C++ uses wagyu. The feature
// interfaces (`GeometryTileFeature` and the rest) are in
// `mln_tile_id::geometry_tile_data`.

use std::f64::consts::PI;

use feature::Feature;
use geo_types::{Coord, CoordNum, Geometry, LineString, MultiLineString, MultiPolygon, Polygon};
use geometry::Point;
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay::{Overlay, ShapeType};
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::i_float::int::point::IntPoint;
use mln_tile_id::{CanonicalTileID, GeometryCollection, GeometryCoordinates, GeometryTileFeature};
use mln_util::constants::EXTENT;
use mln_util::geometry::FeatureType;
use mln_util::math::{angle_between, cross_product, dist_f64, normal, perp};

use crate::Error;

/// Twice the signed area of `ring`. The products are C++ `int` arithmetic,
/// which can overflow for coordinates near the int16 limits; x86-64 wraps,
/// and so does this.
#[must_use]
pub fn signed_area(ring: &GeometryCoordinates) -> f64 {
    let mut sum = 0.0;

    let len = ring.len();
    let mut j = len.wrapping_sub(1);
    for i in 0..len {
        let p1 = ring[i];
        let p2 = ring[j];
        let dx = i32::from(p2.x) - i32::from(p1.x);
        let sy = i32::from(p1.y) + i32::from(p2.y);
        sum += f64::from(dx.wrapping_mul(sy));
        j = i;
    }

    sum
}

/// Repairs the rings of a polygon from a version 1 vector tile, whose spec
/// doesn't require them to be wound or simple: unions them with an even-odd
/// fill. Gives each polygon's outer ring, wound clockwise with y down as
/// version 2 wants, followed by its holes, wound the other way; every ring
/// is closed.
#[must_use]
pub(crate) fn fixup_polygons(rings: &GeometryCollection) -> GeometryCollection {
    let mut overlay = Overlay::new(rings.iter().map(Vec::len).sum());
    for ring in rings.iter() {
        overlay.add_path_iter(ring.iter().map(|p| IntPoint::new(i32::from(p.x), i32::from(p.y))), ShapeType::Subject);
    }

    let mut result = GeometryCollection::new();
    for shape in overlay.overlay(OverlayRule::Subject, FillRule::EvenOdd) {
        for contour in shape {
            // The union's vertices lie within the input's bounding box, so
            // they fit an i16.
            let mut ring: GeometryCoordinates = contour.iter().map(|p| Point::new(p.x as i16, p.y as i16)).collect();
            if let Some(&first) = ring.first() {
                ring.push(first);
            }
            result.push(ring);
        }
    }
    result
}

/// Classifies an array of rings into polygons with outer rings and holes.
#[must_use]
pub fn classify_rings(rings: &GeometryCollection) -> Vec<GeometryCollection> {
    let mut polygons: Vec<GeometryCollection> = Vec::new();

    let len = rings.len();

    if len <= 1 {
        polygons.push(rings.clone());
        return polygons;
    }

    let mut polygon = GeometryCollection::new();
    let mut ccw: i8 = 0;

    for ring in rings.iter() {
        let area = signed_area(ring);
        if area == 0.0 {
            continue;
        }

        if ccw == 0 {
            ccw = if area < 0.0 { -1 } else { 1 };
        }

        if ccw == (if area < 0.0 { -1 } else { 1 }) && !polygon.is_empty() {
            polygons.push(std::mem::take(&mut polygon));
        }

        polygon.push(ring.clone());
    }

    if !polygon.is_empty() {
        polygons.push(polygon);
    }

    polygons
}

/// Truncates `polygon` to its `max_holes` largest inner rings by absolute
/// area. The order of the kept rings is unspecified, and so is which of
/// several rings tied at the cut is kept.
pub fn limit_holes(polygon: &mut GeometryCollection, max_holes: u32) {
    let max_holes = max_holes as usize;
    if polygon.len() > 1 + max_holes {
        polygon[1..].select_nth_unstable_by(max_holes, |a, b| signed_area(b).abs().total_cmp(&signed_area(a).abs()));
        polygon.truncate(1 + max_holes);
    }
}

/// Converts the geometry of a feature of `tile_id` from tile coordinates to
/// longitude/latitude. Fails with `Error::OutOfRange` for a point feature
/// without rings, where C++ throws from `geometries.at(0)`. A feature of
/// `FeatureType::Unknown` fails a debug assertion and otherwise converts to
/// `Point(NaN, NaN)`, as in C++.
pub fn convert_geometry(
    geometry_tile_feature: &dyn GeometryTileFeature,
    tile_id: &CanonicalTileID,
) -> Result<Geometry<f64>, Error> {
    let size = f64::from(EXTENT) * 2f64.powi(i32::from(tile_id.z));
    let x0 = f64::from(EXTENT) * f64::from(tile_id.x);
    let y0 = f64::from(EXTENT) * f64::from(tile_id.y);

    let tile_coordinates_to_lat_lng = |p: &Point<i16>| {
        let y2 = 180.0 - (f64::from(p.y) + y0) * 360.0 / size;
        Coord { x: (f64::from(p.x) + x0) * 360.0 / size - 180.0, y: (y2 * PI / 180.0).exp().atan() * 360.0 / PI - 90.0 }
    };
    let line_string = |points: &[Point<i16>]| LineString(points.iter().map(tile_coordinates_to_lat_lng).collect());

    let geometries = geometry_tile_feature.get_geometries();

    match geometry_tile_feature.get_type() {
        FeatureType::Unknown => {
            debug_assert!(false, "convertGeometry of a feature of unknown type");
            Ok(Geometry::Point(geo_types::Point::new(f64::NAN, f64::NAN)))
        }

        FeatureType::Point => {
            let first = geometries.first().ok_or_else(|| Error::vector_range_check(0, geometries.len()))?;
            if let [p] = first.as_slice() {
                Ok(Geometry::Point(tile_coordinates_to_lat_lng(p).into()))
            } else {
                Ok(Geometry::MultiPoint(
                    first.iter().map(|p| geo_types::Point(tile_coordinates_to_lat_lng(p))).collect(),
                ))
            }
        }

        FeatureType::LineString => {
            if let [g] = geometries.as_slice() {
                Ok(Geometry::LineString(line_string(g)))
            } else {
                Ok(Geometry::MultiLineString(MultiLineString(geometries.iter().map(|g| line_string(g)).collect())))
            }
        }

        FeatureType::Polygon => {
            let mut multi_polygon: Vec<Polygon<f64>> = classify_rings(geometries)
                .iter()
                .map(|pg| {
                    let mut rings = pg.iter().map(|r| line_string(r));
                    let exterior = rings.next().unwrap_or_else(|| LineString(Vec::new()));
                    Polygon::new(exterior, rings.collect())
                })
                .collect();
            if multi_polygon.len() == 1 {
                Ok(Geometry::Polygon(multi_polygon.swap_remove(0)))
            } else {
                Ok(Geometry::MultiPolygon(MultiPolygon(multi_polygon)))
            }
        }
    }
}

/// Narrows `v` as clang emits `static_cast<int16_t>(double)` on x86-64:
/// cvttsd2si to int32 (0x80000000 for NaN and out-of-range values;
/// undefined behaviour in C++), then the low 16 bits.
fn int16_cast_x86_64(v: f64) -> i16 {
    let i = if v.is_nan() || v >= 2_147_483_648.0 || v <= -2_147_483_649.0 { i32::MIN } else { v as i32 };
    i as i16
}

/// Converts `geometry` from longitude/latitude to the coordinates of
/// `tile_id`. The C++ clamps *after* the narrowing cast, so the clamp has no
/// effect; the cast is reproduced as x86-64 executes it (see
/// `int16_cast_x86_64`).
#[must_use]
pub fn convert_geometry_to_tile(geometry: &Geometry<f64>, tile_id: &CanonicalTileID) -> GeometryCollection {
    let size = f64::from(EXTENT) * 2f64.powi(i32::from(tile_id.z));
    let x0 = f64::from(EXTENT) * f64::from(tile_id.x);
    let y0 = f64::from(EXTENT) * f64::from(tile_id.y);

    let lat_lon_to_tile_coodinates = |c: Coord<f64>| {
        let x = (c.x + 180.0) * size / 360.0 - x0;
        let px = int16_cast_x86_64(x);

        let y = (180.0 - (((c.y + 90.0) * PI / 360.0).tan().ln() * 180.0 / PI)) * size / 360.0 - y0;
        let py = int16_cast_x86_64(y);

        Point::new(px, py)
    };

    GeometryCollection::from(geometry_rings(geometry, lat_lon_to_tile_coodinates))
}

/// The points of `geometry` as lines or rings: one for a point, its line
/// for a line string, one per line, and a polygon's exterior then its
/// interiors. A geometry collection has none.
fn geometry_rings<T: CoordNum, U>(geometry: &Geometry<T>, point: impl Fn(Coord<T>) -> U) -> Vec<Vec<U>> {
    let line = |coords: &[Coord<T>]| coords.iter().map(|&c| point(c)).collect::<Vec<_>>();
    let polygon_rings = |polygon: &Polygon<T>| {
        // A polygon of no rings has an empty exterior.
        let exterior =
            (!polygon.exterior().0.is_empty() || !polygon.interiors().is_empty()).then(|| polygon.exterior());
        exterior.into_iter().chain(polygon.interiors()).map(|ring| line(&ring.0)).collect::<Vec<_>>()
    };
    match geometry {
        Geometry::Point(p) => vec![vec![point(p.0)]],
        Geometry::MultiPoint(points) => vec![points.iter().map(|p| point(p.0)).collect()],
        Geometry::Line(l) => vec![vec![point(l.start), point(l.end)]],
        Geometry::LineString(l) => vec![line(&l.0)],
        Geometry::MultiLineString(lines) => lines.iter().map(|l| line(&l.0)).collect(),
        Geometry::Polygon(p) => polygon_rings(p),
        Geometry::MultiPolygon(polygons) => polygons.iter().flat_map(polygon_rings).collect(),
        Geometry::Rect(r) => polygon_rings(&r.to_polygon()),
        Geometry::Triangle(t) => polygon_rings(&t.to_polygon()),
        Geometry::GeometryCollection(_) => Vec::new(),
    }
}

/// Converts a tile feature to a `Feature` in longitude/latitude, with its
/// properties and id. Fails as [`convert_geometry`] does.
pub fn convert_feature(
    geometry_tile_feature: &dyn GeometryTileFeature,
    tile_id: &CanonicalTileID,
) -> Result<Feature<f64>, Error> {
    let mut feature = Feature::new(convert_geometry(geometry_tile_feature, tile_id)?);
    feature.properties = geometry_tile_feature.get_properties().clone();
    feature.id = geometry_tile_feature.get_id();
    Ok(feature)
}

/// The lines or rings of a tile feature's geometry; a null geometry has
/// none.
#[must_use]
pub fn to_geometry_collection(geom: Option<&Geometry<i16>>) -> GeometryCollection {
    geom.map_or_else(GeometryCollection::new, |geom| GeometryCollection::from(geometry_rings(geom, Point::from)))
}

pub(crate) type GeometryCoordinateFloat = Point<f32>;
pub(crate) type GeometryCoordinatesFloat = Vec<GeometryCoordinateFloat>;
pub(crate) type GeometryCollectionFloat = Vec<GeometryCoordinatesFloat>;

/// Replaces each corner of the closed rings of `polygon` with an arc that
/// starts and ends `desired_corner_distance` tile units from the corner, or
/// 20% of the shorter adjacent edge if that is less. Corners next to a
/// zero-length edge, and corners turning by less than 5° (or nearly
/// reversing), stay sharp. Each output ring is closed.
///
/// A ring needs its closing point: the last point is dropped before rounding.
/// A ring of fewer than two points comes out empty.
#[must_use]
pub fn round_polygon_corners(polygon: &GeometryCollection, desired_corner_distance: f64) -> GeometryCollectionFloat {
    const ARC_POINTS: i32 = 3;
    const MAX_EDGE_LEN_PERCENT: f64 = 0.2;
    let sin_parallel_threshold = mln_math::deg2rad(5.0).sin();

    let to_f64 = |p: Point<i16>| Point::new(f64::from(p.x), f64::from(p.y));
    let to_f32 = |p: Point<f64>| Point::new(p.x as f32, p.y as f32);
    let sub = |a: Point<f64>, b: Point<f64>| Point::new(a.x - b.x, a.y - b.y);
    let add_scaled = |a: Point<f64>, v: Point<f64>, s: f64| Point::new(a.x + v.x * s, a.y + v.y * s);

    let mut rounded_corner_polygon = GeometryCollectionFloat::with_capacity(polygon.len());

    for ring in polygon.iter() {
        let mut rounded_corner_ring = GeometryCoordinatesFloat::new();

        let n_vertices = ring.len().saturating_sub(1);
        for i in 0..n_vertices {
            let prev_point = to_f64(ring[(i + n_vertices - 1) % n_vertices]);
            let corner_point = to_f64(ring[i]);
            let next_point = to_f64(ring[(i + 1) % n_vertices]);

            let edge1_len = dist_f64(corner_point, prev_point);
            let edge2_len = dist_f64(corner_point, next_point);
            if edge1_len == 0.0 || edge2_len == 0.0 {
                // Duplicate vertex: no direction to round, keep it as-is.
                rounded_corner_ring.push(to_f32(corner_point));
                continue;
            }

            // Compute the start and end points of the rounded corner
            let edge1_vector = normal(prev_point, corner_point);
            let edge2_vector = normal(corner_point, next_point);

            let edge1_max_corner_distance = edge1_len * MAX_EDGE_LEN_PERCENT;
            let edge2_max_corner_distance = edge2_len * MAX_EDGE_LEN_PERCENT;
            let corner_distance = desired_corner_distance.min(edge1_max_corner_distance).min(edge2_max_corner_distance);

            let start_point = add_scaled(corner_point, edge1_vector, -corner_distance);
            let end_point = add_scaled(corner_point, edge2_vector, corner_distance);

            // Perpendicular directions
            let mut perp1_vector = perp(edge1_vector);
            let mut perp2_vector = perp(edge2_vector);

            // Ensure perpendiculars point toward same side
            if cross_product(edge1_vector, edge2_vector) < 0.0 {
                perp1_vector = Point::new(-perp1_vector.x, -perp1_vector.y);
                perp2_vector = Point::new(-perp2_vector.x, -perp2_vector.y);
            }

            // Center = intersection of perpendiculars
            let perp_cross_product = cross_product(perp1_vector, perp2_vector);
            if perp_cross_product.abs() < sin_parallel_threshold {
                rounded_corner_ring.push(to_f32(corner_point));
                continue;
            }
            let t = cross_product(sub(end_point, start_point), perp2_vector) / perp_cross_product;
            let center_point = add_scaled(start_point, perp1_vector, t);

            // Add the start point of the rounded corner
            rounded_corner_ring.push(to_f32(start_point));

            // Generate points along the arc
            let radius = dist_f64(start_point, center_point);
            let start_angle = (start_point.y - center_point.y).atan2(start_point.x - center_point.x);
            let arc_angle = angle_between(sub(start_point, center_point), sub(end_point, center_point));
            for k in 1..=ARC_POINTS {
                let angle = start_angle + arc_angle * f64::from(k) / f64::from(ARC_POINTS + 1);
                let arc_point =
                    Point::new(center_point.x + angle.cos() * radius, center_point.y + angle.sin() * radius);
                rounded_corner_ring.push(to_f32(arc_point));
            }

            // Add the end point of the rounded corner
            rounded_corner_ring.push(to_f32(end_point));
        }
        // Close the ring by repeating the first point at the end
        if let Some(&first) = rounded_corner_ring.first() {
            rounded_corner_ring.push(first);
        }

        // Add the ring to the rounded corner polygon
        rounded_corner_polygon.push(rounded_corner_ring);
    }
    rounded_corner_polygon
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(points: &[(i16, i16)]) -> GeometryCoordinates {
        points.iter().map(|&(x, y)| Point::new(x, y)).collect()
    }

    fn collection(rings: &[&[(i16, i16)]]) -> GeometryCollection {
        GeometryCollection::from(rings.iter().map(|r| ring(r)).collect::<Vec<_>>())
    }

    fn test_signed_area(ring: &GeometryCoordinates) -> f64 {
        signed_area(ring)
    }

    // gtest: GeometryTileData.classifyRings1
    #[test]
    fn classify_rings1() {
        let polygons = classify_rings(&collection(&[&[(0, 0), (0, 40), (40, 40), (40, 0), (0, 0)]]));

        // output: 1 polygon
        assert_eq!(polygons.len(), 1);
        // output: polygon 1 has 1 exterior
        assert_eq!(polygons[0].len(), 1);
    }

    // gtest: GeometryTileData.classifyRings2
    #[test]
    fn classify_rings2() {
        let polygons = classify_rings(&collection(&[
            &[(0, 0), (0, 40), (40, 40), (40, 0), (0, 0)],
            &[(10, 10), (20, 10), (20, 20), (10, 10)],
        ]));

        // output: 1 polygon
        assert_eq!(polygons.len(), 1);
        // output: polygon 1 has 1 exterior, 1 interior
        assert_eq!(polygons[0].len(), 2);
    }

    /// The (outer ring, hole count) of each polygon, with every ring
    /// closed and wound as `fixup_polygons` promises.
    fn fixed_polygons(rings: &[&[(i16, i16)]]) -> Vec<(GeometryCoordinates, usize)> {
        let fixed = fixup_polygons(&collection(rings));
        let polygons = classify_rings(&fixed);
        for polygon in &polygons {
            for (i, ring) in polygon.iter().enumerate() {
                assert_eq!(ring.first(), ring.last(), "{ring:?}");
                assert_eq!(signed_area(ring) > 0.0, i == 0, "{ring:?}");
            }
        }
        polygons.iter().map(|polygon| (polygon[0].clone(), polygon.len() - 1)).collect()
    }

    #[test]
    fn fixup_polygons_rewinds_rings() {
        // Two outer rings wound opposite ways, and a hole wound as its
        // outer ring is.
        let polygons = fixed_polygons(&[
            &[(0, 0), (0, 40), (40, 40), (40, 0), (0, 0)],
            &[(10, 10), (10, 20), (20, 20), (20, 10), (10, 10)],
            &[(50, 0), (90, 0), (90, 40), (50, 40), (50, 0)],
        ]);
        assert_eq!(polygons.len(), 2);
        let mut holes: Vec<usize> = polygons.iter().map(|(_, holes)| *holes).collect();
        holes.sort_unstable();
        assert_eq!(holes, [0, 1]);
    }

    #[test]
    fn fixup_polygons_splits_a_self_intersecting_ring() {
        let polygons = fixed_polygons(&[&[(0, 0), (20, 20), (20, 0), (0, 20), (0, 0)]]);
        assert_eq!(polygons.len(), 2);
        for (outer, holes) in &polygons {
            assert_eq!(*holes, 0);
            assert_eq!(outer.len(), 4, "a closed triangle: {outer:?}");
            // Twice the triangle's area.
            assert_eq!(signed_area(outer), 200.0);
        }
    }

    // gtest: GeometryTileData.limitHoles1
    #[test]
    fn limit_holes1() {
        let mut polygon = collection(&[
            &[(0, 0), (0, 40), (40, 40), (40, 0), (0, 0)],
            &[(30, 30), (32, 30), (32, 32), (30, 30)],
            &[(10, 10), (20, 10), (20, 20), (10, 10)],
        ]);

        limit_holes(&mut polygon, 1);

        // output: polygon 1 has 1 exterior, 1 interior
        assert_eq!(polygon.len(), 2);

        // ensure we've kept the right rings (ones with largest areas)
        assert_eq!(polygon[0][0].x, 0);
        assert_eq!(polygon[1][0].x, 10);
    }

    // gtest: GeometryTileData.limitHoles2
    #[test]
    fn limit_holes2() {
        let mut polygon = collection(&[
            &[(0, 0), (0, 40), (40, 40), (40, 0), (0, 0)],
            &[(10, 10), (20, 10), (20, 20), (10, 10)],
            &[(30, 30), (32, 30), (32, 32), (30, 30)],
        ]);

        limit_holes(&mut polygon, 1);

        // output: polygon 1 has 1 exterior, 1 interior
        assert_eq!(polygon.len(), 2);

        // ensure we've kept the right rings (ones with largest areas)
        assert_eq!(polygon[0][0].x, 0);
        assert_eq!(polygon[1][0].x, 10);
    }

    // gtest: GeometryTileData.limitHoles3
    #[test]
    fn limit_holes3() {
        // real world polygon with interior rings with negative areas
        // that need to be sorted in `limitHoles` by comparing absolute
        // area not signed
        let mut polygon = collection(&[
            &[
                (7336, -248),
                (7304, -248),
                (7272, -168),
                (7176, -200),
                (7080, -136),
                (7048, -56),
                (7128, -8),
                (7176, -56),
                (7288, -56),
                (7316, 0),
                (6918, 0),
                (6904, -40),
                (6984, -72),
                (6952, -88),
                (6952, -168),
                (6888, -88),
                (6856, -88),
                (6856, -8),
                (6872, 0),
                (6170, 0),
                (6184, -40),
                (6136, -72),
                (6104, -56),
                (6132, 0),
                (6028, 0),
                (6104, -152),
                (6184, -200),
                (6206, -256),
                (6272, -256),
                (6264, -248),
                (6248, -120),
                (6280, -136),
                (6280, -232),
                (6288, -256),
                (6790, -256),
                (6792, -248),
                (6800, -256),
                (7058, -256),
                (7064, -248),
                (7096, -256),
                (7338, -256),
                (7336, -248),
            ],
            &[(6344, -104), (6264, -8), (6392, -72), (6360, -200), (6344, -104)],
            &[(6744, -24), (6760, -72), (6728, -104), (6744, -24)],
            &[(6616, -104), (6648, -88), (6632, -72), (6664, -56), (6664, -120), (6616, -104)],
        ]);

        // make a copy for later testing
        let original = polygon.clone();

        assert_eq!(polygon.len(), 4);
        assert_eq!(test_signed_area(&polygon[0]), 515_360.0); // exterior
        assert_eq!(test_signed_area(&polygon[1]), -12288.0); // biggest interior ring
        assert_eq!(test_signed_area(&polygon[2]), -2048.0); // smallest interior ring
        assert_eq!(test_signed_area(&polygon[3]), -3072.0); // second largest interior ring

        limit_holes(&mut polygon, 2);

        // output: polygon 1 has 1 exterior, 2 interior
        assert_eq!(polygon.len(), 3);

        // ensure we've kept the exterior ring
        assert_eq!(original[0], polygon[0]);

        // ensure we've kept the two largest interior rings, in either order
        assert!(polygon[1..].contains(&original[1]));
        assert!(polygon[1..].contains(&original[3]));
    }
    fn assert_ring_near(actual: &GeometryCoordinatesFloat, expected: &[(f32, f32)]) {
        assert_eq!(actual.len(), expected.len(), "{actual:?}");
        for (a, &(x, y)) in actual.iter().zip(expected) {
            assert!((a.x - x).abs() < 1e-3 && (a.y - y).abs() < 1e-3, "{actual:?}");
        }
    }

    // The arc of a right angle at `corner`, from `start` to `end`, around
    // the center `center`.
    fn right_angle_arc(start: (f32, f32), center: (f32, f32), end: (f32, f32)) -> Vec<(f32, f32)> {
        let radius = ((start.0 - center.0).powi(2) + (start.1 - center.1).powi(2)).sqrt();
        let start_angle = (start.1 - center.1).atan2(start.0 - center.0);
        let end_angle = (end.1 - center.1).atan2(end.0 - center.0);
        let mut arc_angle = end_angle - start_angle;
        if arc_angle > std::f32::consts::PI {
            arc_angle -= 2.0 * std::f32::consts::PI;
        } else if arc_angle < -std::f32::consts::PI {
            arc_angle += 2.0 * std::f32::consts::PI;
        }
        let mut points = vec![start];
        for k in 1..=3 {
            let angle = start_angle + arc_angle * k as f32 / 4.0;
            points.push((center.0 + angle.cos() * radius, center.1 + angle.sin() * radius));
        }
        points.push(end);
        points
    }

    #[test]
    fn round_polygon_corners_square() {
        let polygon = collection(&[&[(0, 0), (100, 0), (100, 100), (0, 100), (0, 0)]]);
        let rounded = round_polygon_corners(&polygon, 10.0);

        assert_eq!(rounded.len(), 1);
        let mut expected = Vec::new();
        expected.extend(right_angle_arc((0.0, 10.0), (10.0, 10.0), (10.0, 0.0)));
        expected.extend(right_angle_arc((90.0, 0.0), (90.0, 10.0), (100.0, 10.0)));
        expected.extend(right_angle_arc((100.0, 90.0), (90.0, 90.0), (90.0, 100.0)));
        expected.extend(right_angle_arc((10.0, 100.0), (10.0, 90.0), (0.0, 90.0)));
        expected.push((0.0, 10.0));
        assert_ring_near(&rounded[0], &expected);
        // The middle of the first arc is 10 units from the corner's center.
        let middle = rounded[0][2];
        assert!((middle.x - (10.0 - 10.0 / 2f32.sqrt())).abs() < 1e-3);
    }

    #[test]
    fn round_polygon_corners_clockwise_ring() {
        let polygon = collection(&[&[(0, 0), (0, 100), (100, 100), (100, 0), (0, 0)]]);
        let rounded = round_polygon_corners(&polygon, 10.0);

        let mut expected = Vec::new();
        expected.extend(right_angle_arc((10.0, 0.0), (10.0, 10.0), (0.0, 10.0)));
        expected.extend(right_angle_arc((0.0, 90.0), (10.0, 90.0), (10.0, 100.0)));
        expected.extend(right_angle_arc((90.0, 100.0), (90.0, 90.0), (100.0, 90.0)));
        expected.extend(right_angle_arc((100.0, 10.0), (90.0, 10.0), (90.0, 0.0)));
        expected.push((10.0, 0.0));
        assert_ring_near(&rounded[0], &expected);
    }

    #[test]
    fn round_polygon_corners_clamps_to_a_fifth_of_the_shorter_edge() {
        let polygon = collection(&[&[(0, 0), (100, 0), (100, 50), (0, 50), (0, 0)]]);
        let rounded = round_polygon_corners(&polygon, 1000.0);

        // The shorter edges are 50 long: every arc starts and ends 10 away.
        assert_eq!(rounded[0].len(), 21);
        assert_ring_near(&rounded[0][..1].to_vec(), &[(0.0, 10.0)]);
        assert_ring_near(&rounded[0][4..6].to_vec(), &[(10.0, 0.0), (90.0, 0.0)]);
    }

    #[test]
    fn round_polygon_corners_rounds_every_ring() {
        let polygon = collection(&[
            &[(0, 0), (100, 0), (100, 100), (0, 100), (0, 0)],
            &[(25, 25), (25, 75), (75, 75), (75, 25), (25, 25)],
        ]);
        let rounded = round_polygon_corners(&polygon, 5.0);

        assert_eq!(rounded.len(), 2);
        assert_eq!(rounded[0].len(), 21);
        assert_eq!(rounded[1].len(), 21);
        assert_eq!(rounded[1].first(), rounded[1].last());
        assert_ring_near(&rounded[1][..1].to_vec(), &[(30.0, 25.0)]);
    }

    #[test]
    fn round_polygon_corners_keeps_straight_and_duplicate_vertices() {
        // (50, 0) lies on a straight edge, and (100, 100) is repeated.
        let polygon = collection(&[&[(0, 0), (50, 0), (100, 0), (100, 100), (100, 100), (0, 100), (0, 0)]]);
        let rounded = round_polygon_corners(&polygon, 10.0);

        // 3 rounded corners, the straight vertex, and both copies of the
        // repeated one, which sit next to a zero-length edge.
        assert_eq!(rounded[0].len(), 3 * 5 + 1 + 2 + 1);
        assert_ring_near(&rounded[0][5..6].to_vec(), &[(50.0, 0.0)]);
        assert_ring_near(&rounded[0][11..13].to_vec(), &[(100.0, 100.0), (100.0, 100.0)]);
    }

    #[test]
    fn round_polygon_corners_keeps_a_spike_sharp() {
        // At (100, 1) the ring turns back by almost 180°.
        let polygon = collection(&[&[(0, 0), (100, 1), (0, 2), (0, 0)]]);
        let rounded = round_polygon_corners(&polygon, 10.0);

        assert_ring_near(&rounded[0][5..6].to_vec(), &[(100.0, 1.0)]);
    }

    #[test]
    fn round_polygon_corners_of_degenerate_rings() {
        let polygon = collection(&[&[], &[(1, 1)], &[(1, 1), (1, 1)]]);
        let rounded = round_polygon_corners(&polygon, 10.0);

        assert_eq!(rounded, vec![vec![], vec![], vec![Point::new(1.0, 1.0), Point::new(1.0, 1.0)]]);
    }
}
