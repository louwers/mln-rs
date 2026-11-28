// Polygon fixup runs on i_overlay, where C++ uses wagyu. Ring classification
// and the conversions to and from longitude/latitude are in
// `mln_style::feature`, where expressions reach them.

use std::f64::consts::PI;

use geo_types::{Coord, Geometry, Polygon};
use geometry::Point;
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay::{Overlay, ShapeType};
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::i_float::int::point::IntPoint;
use mln_style::constants::EXTENT;
use mln_style::feature::{
    Feature, GeometryCollection, GeometryCoordinates, GeometryTileFeature, convert_geometry, convert_geometry_to_world,
    signed_area,
};
use mln_style::tile_id::CanonicalTileID;

use crate::geo::world::WorldMapping;
use crate::tile::TileError;

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

/// Converts a feature of `tile_id` whose geometry is `geometries` to a
/// `Feature` in longitude/latitude, with its properties and id: through
/// `world`, the mapping of the map whose tile it is, from that map's world
/// square. Fails as [`convert_geometry`] does.
pub(crate) fn convert_feature(
    feature: &dyn GeometryTileFeature,
    geometries: &GeometryCollection,
    tile_id: &CanonicalTileID,
    world: &WorldMapping,
) -> Result<Feature<f64>, TileError> {
    let geometry = match world {
        WorldMapping::Mercator => convert_geometry(feature.get_type(), geometries, tile_id)?,
        WorldMapping::Crs(_) => {
            let mut geometry = convert_geometry_to_world(feature.get_type(), geometries, tile_id)?;
            lng_lats_from_world(world, &mut geometry);
            geometry
        }
    };
    let mut converted = Feature::new(geometry);
    converted.properties = feature.get_properties().into_owned();
    converted.id = feature.get_id();
    Ok(converted)
}

/// Replaces each point of `geometry`, a place in the world square of
/// `world`, by its longitude and latitude, in one batch.
fn lng_lats_from_world(world: &WorldMapping, geometry: &mut Geometry<f64>) {
    let mut places = Vec::new();
    for_each_coord_mut(geometry, &mut |coord| places.push([coord.x, coord.y]));
    let mut lng_lats = vec![[0.0; 2]; places.len()];
    world.lng_lats_from_world(&places, &mut lng_lats);
    let mut lng_lats = lng_lats.into_iter();
    for_each_coord_mut(geometry, &mut |coord| {
        if let Some([lng, lat]) = lng_lats.next() {
            *coord = Coord { x: lng, y: lat };
        }
    });
}

/// Calls `f` with every point of `geometry`, of the kinds that
/// [`convert_geometry`] makes, in order.
fn for_each_coord_mut(geometry: &mut Geometry<f64>, f: &mut impl FnMut(&mut Coord<f64>)) {
    match geometry {
        Geometry::Point(point) => f(&mut point.0),
        Geometry::MultiPoint(points) => points.iter_mut().for_each(|point| f(&mut point.0)),
        Geometry::LineString(line) => line.0.iter_mut().for_each(f),
        Geometry::MultiLineString(lines) => lines.iter_mut().for_each(|line| line.0.iter_mut().for_each(&mut *f)),
        Geometry::Polygon(polygon) => polygon_coords_mut(polygon, f),
        Geometry::MultiPolygon(polygons) => polygons.iter_mut().for_each(|polygon| polygon_coords_mut(polygon, f)),
        // The conversion makes none of these.
        Geometry::Line(_) | Geometry::GeometryCollection(_) | Geometry::Rect(_) | Geometry::Triangle(_) => {}
    }
}

/// Calls `f` with every point of `polygon`'s exterior, then of its
/// interiors.
fn polygon_coords_mut(polygon: &mut Polygon<f64>, f: &mut impl FnMut(&mut Coord<f64>)) {
    polygon.exterior_mut(|exterior| exterior.0.iter_mut().for_each(&mut *f));
    polygon.interiors_mut(|interiors| interiors.iter_mut().for_each(|ring| ring.0.iter_mut().for_each(&mut *f)));
}

/// Whether the edge from `a` to `b` runs along a line the tile was clipped
/// at. Clipping cuts on the buffer around the tile, so such an edge lies
/// outside it, and belongs to the cut rather than to the feature.
#[must_use]
pub fn is_boundary_edge(a: Point<i16>, b: Point<i16>) -> bool {
    let outside = |v: i16| v < 0 || i32::from(v) > EXTENT;
    (a.x == b.x && outside(a.x)) || (a.y == b.y && outside(a.y))
}

/// Whether all of `ring` lies beyond one edge of the tile, in its buffer.
#[must_use]
pub fn is_entirely_outside(ring: &[Point<i16>]) -> bool {
    let extent = |v: i16| i32::from(v);
    ring.iter().all(|p| p.x < 0)
        || ring.iter().all(|p| extent(p.x) > EXTENT)
        || ring.iter().all(|p| p.y < 0)
        || ring.iter().all(|p| extent(p.y) > EXTENT)
}

/// Replaces each corner of the rings of `polygon` with an arc whose ends
/// lie `corner_distance` tile units from the corner along its edges, or 20%
/// of the shorter edge if that is less, in steps of at most 30°.
///
/// Corners that turn by less than 5° or nearly reverse stay sharp, and so
/// do corners on a line the tile was clipped at: the neighbouring tile cuts
/// the feature elsewhere, and the two halves have to meet. The vertices are
/// rounded to whole tile units, dropping any that then repeat the one
/// before, since arcs finer than a unit would triangulate into spikes. A
/// ring with fewer than three distinct vertices, before or after rounding,
/// is kept as it is. Closed rings stay closed.
#[must_use]
pub fn round_polygon_corners(polygon: &GeometryCollection, corner_distance: f64) -> GeometryCollection {
    GeometryCollection::from(polygon.iter().map(|ring| round_ring(ring, corner_distance)).collect::<Vec<_>>())
}

fn round_ring(ring: &GeometryCoordinates, corner_distance: f64) -> GeometryCoordinates {
    let closed = ring.len() > 1 && ring.first() == ring.last();
    let vertex_count = if closed { ring.len() - 1 } else { ring.len() };
    if vertex_count < 3 {
        return ring.clone();
    }

    let mut rounded = Vec::with_capacity(vertex_count * 4);
    let mut previous_is_boundary = is_boundary_edge(ring[vertex_count - 1], ring[0]);
    for i in 0..vertex_count {
        let previous = ring[(i + vertex_count - 1) % vertex_count];
        let corner = ring[i];
        let next = ring[(i + 1) % vertex_count];
        let next_is_boundary = is_boundary_edge(corner, next);
        if previous_is_boundary || next_is_boundary {
            rounded.push(Point::new(f64::from(corner.x), f64::from(corner.y)));
        } else {
            append_round_corner(&mut rounded, previous, corner, next, corner_distance);
        }
        previous_is_boundary = next_is_boundary;
    }

    // Halves round up, as in GL JS. The arcs lie within the corners they
    // replace, so they fit an i16.
    let snap = |v: f64| (v + 0.5).floor() as i16;
    let mut snapped = GeometryCoordinates::with_capacity(rounded.len() + 1);
    for p in rounded {
        let p = Point::new(snap(p.x), snap(p.y));
        if snapped.last() != Some(&p) {
            snapped.push(p);
        }
    }
    while snapped.len() > 1 && snapped.first() == snapped.last() {
        snapped.pop();
    }
    if snapped.len() < 3 {
        return ring.clone();
    }
    if closed {
        snapped.push(snapped[0]);
    }
    snapped
}

/// Appends to `ring` the arc that replaces `corner`, between `previous` and
/// `next`, or the corner itself when it is too shallow or too sharp to
/// round.
fn append_round_corner(
    ring: &mut Vec<Point<f64>>,
    previous: Point<i16>,
    corner: Point<i16>,
    next: Point<i16>,
    corner_distance: f64,
) {
    const MAX_EDGE_FRACTION: f64 = 0.2;

    let c = Point::new(f64::from(corner.x), f64::from(corner.y));
    let towards = |p: Point<i16>| Point::new(f64::from(p.x) - c.x, f64::from(p.y) - c.y);
    let (a, b) = (towards(previous), towards(next));
    let (len_a, len_b) = (a.x.hypot(a.y), b.x.hypot(b.y));
    if len_a == 0.0 || len_b == 0.0 {
        ring.push(c);
        return;
    }
    let ua = Point::new(a.x / len_a, a.y / len_a);
    let ub = Point::new(b.x / len_b, b.y / len_b);

    let dot = ua.x * ub.x + ua.y * ub.y;
    if dot.abs() > mln_style::math::deg2rad(5.0).cos() {
        ring.push(c);
        return;
    }

    let r = corner_distance.min(len_a * MAX_EDGE_FRACTION).min(len_b * MAX_EDGE_FRACTION);
    let tangent_a = Point::new(c.x + ua.x * r, c.y + ua.y * r);
    let tangent_b = Point::new(c.x + ub.x * r, c.y + ub.y * r);

    // The arc's center lies on the bisector, r / cos(θ/2) from the corner.
    let bisector = Point::new(ua.x + ub.x, ua.y + ub.y);
    let to_center = r / f64::midpoint(1.0, dot).sqrt() / bisector.x.hypot(bisector.y);
    let center = Point::new(c.x + bisector.x * to_center, c.y + bisector.y * to_center);

    // Rotates tangent A around the center onto tangent B, the short way.
    let from = Point::new(tangent_a.x - center.x, tangent_a.y - center.y);
    let to = Point::new(tangent_b.x - center.x, tangent_b.y - center.y);
    let sweep = (from.x * to.y - from.y * to.x).atan2(from.x * to.x + from.y * to.y);
    // The epsilon keeps rounding error from adding a step at a multiple of 30°.
    let steps = ((sweep.abs() / (PI / 6.0) - 1e-6).ceil() as i32).max(2);
    for step in 0..=steps {
        let (sin, cos) = (sweep * f64::from(step) / f64::from(steps)).sin_cos();
        ring.push(Point::new(center.x + cos * from.x - sin * from.y, center.y + sin * from.x + cos * from.y));
    }
}

#[cfg(test)]
mod tests {
    use mln_style::feature::classify_rings;

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
    fn points(ring: &GeometryCoordinates) -> Vec<(i16, i16)> {
        ring.iter().map(|p| (p.x, p.y)).collect()
    }

    #[test]
    fn round_polygon_corners_square() {
        let polygon = collection(&[&[(0, 0), (100, 0), (100, 100), (0, 100), (0, 0)]]);
        let rounded = round_polygon_corners(&polygon, 10.0);

        // Each right angle turns in three 30° steps around a center 10 units
        // in from the corner, and the vertices snap to whole units.
        assert_eq!(
            points(&rounded[0]),
            [
                (0, 10),
                (1, 5),
                (5, 1),
                (10, 0),
                (90, 0),
                (95, 1),
                (99, 5),
                (100, 10),
                (100, 90),
                (99, 95),
                (95, 99),
                (90, 100),
                (10, 100),
                (5, 99),
                (1, 95),
                (0, 90),
                (0, 10),
            ]
        );
    }

    #[test]
    fn round_polygon_corners_clamps_to_a_fifth_of_the_shorter_edge() {
        let polygon = collection(&[&[(0, 0), (100, 0), (100, 50), (0, 50), (0, 0)]]);
        let rounded = round_polygon_corners(&polygon, 1000.0);

        // The shorter edges are 50 long: every arc starts and ends 10 away.
        assert_eq!(points(&rounded[0])[..5], [(0, 10), (1, 5), (5, 1), (10, 0), (90, 0)]);
    }

    #[test]
    fn round_polygon_corners_keeps_corners_on_clip_lines_sharp() {
        // The edge at x = -64 is where the tile's buffer clipped the feature.
        let polygon = collection(&[&[(-64, 0), (100, 0), (100, 100), (-64, 100), (-64, 0)]]);
        let rounded = round_polygon_corners(&polygon, 10.0);

        let rounded = points(&rounded[0]);
        assert_eq!(rounded.len(), 2 * 4 + 2 + 1);
        assert_eq!(rounded[0], (-64, 0));
        assert!(rounded.contains(&(-64, 100)));
        assert!(!rounded.contains(&(100, 0)));
    }

    #[test]
    fn round_polygon_corners_drops_vertices_that_snap_together() {
        // Arcs under half a unit snap back onto their corners.
        let polygon = collection(&[&[(0, 0), (100, 0), (100, 100), (0, 100), (0, 0)]]);
        assert_eq!(round_polygon_corners(&polygon, 0.4), polygon);
    }

    #[test]
    fn round_polygon_corners_rounds_every_ring() {
        let polygon = collection(&[
            &[(0, 0), (100, 0), (100, 100), (0, 100), (0, 0)],
            &[(25, 25), (25, 75), (75, 75), (75, 25), (25, 25)],
        ]);
        let rounded = round_polygon_corners(&polygon, 5.0);

        assert_eq!(rounded.len(), 2);
        assert_eq!(rounded[0].len(), 17);
        assert_eq!(rounded[1].len(), 17);
        assert_eq!(rounded[1].first(), rounded[1].last());
        assert_eq!(points(&rounded[1])[0], (30, 25));
    }

    #[test]
    fn round_polygon_corners_keeps_straight_and_reversing_vertices() {
        // (50, 0) lies on a straight edge; at (200, 1) the ring turns back.
        let polygon = collection(&[&[(0, 0), (50, 0), (200, 1), (0, 2), (0, 0)]]);
        let rounded = points(&round_polygon_corners(&polygon, 10.0)[0]);

        assert!(rounded.contains(&(50, 0)));
        assert!(rounded.contains(&(200, 1)));
    }

    #[test]
    fn is_entirely_outside_needs_every_vertex_past_one_edge() {
        assert!(is_entirely_outside(&ring(&[(8200, 0), (9000, 100), (8193, 9000)])));
        assert!(is_entirely_outside(&ring(&[(0, -5), (100, -1), (50, -2000)])));
        // Each vertex is outside, but past different edges.
        assert!(!is_entirely_outside(&ring(&[(-10, 100), (100, -10), (-10, -10)])));
        assert!(!is_entirely_outside(&ring(&[(8192, 0), (9000, 100), (9000, 9000)])));
    }

    #[test]
    fn round_polygon_corners_keeps_degenerate_rings() {
        let polygon = collection(&[&[], &[(1, 1)], &[(1, 1), (1, 1)], &[(0, 0), (9, 0), (0, 0)]]);
        assert_eq!(round_polygon_corners(&polygon, 10.0), polygon);
    }
}
