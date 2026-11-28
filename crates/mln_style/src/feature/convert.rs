// Ring classification and the conversions between tile coordinates and
// longitude/latitude, which expressions and the tile decoders share.

use std::f64::consts::PI;

use geo_types::{Coord, CoordNum, Geometry, LineString, MultiLineString, MultiPolygon, Polygon};
use geometry::Point;

use super::GeometryCollection;
use crate::constants::EXTENT;
use crate::geometry::FeatureType;
use crate::tile_id::CanonicalTileID;

/// An index past the end of a vector, with the message libstdc++'s
/// `std::vector::at` throws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("vector::_M_range_check: __n (which is {index}) >= this->size() (which is {len})")]
pub struct OutOfRange {
    pub index: usize,
    pub len: usize,
}

/// Twice the signed area of `ring`, positive for a ring that is clockwise
/// with y down.
#[must_use]
pub fn signed_area(ring: &[Point<i16>]) -> f64 {
    let Some(&last) = ring.last() else {
        return 0.0;
    };
    let mut prev = last;
    let mut sum = 0i64;
    for &p in ring {
        sum += (i64::from(prev.x) - i64::from(p.x)) * (i64::from(p.y) + i64::from(prev.y));
        prev = p;
    }
    sum as f64
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

/// Converts `geometries`, the geometry of a feature of type `feature_type`
/// in `tile_id`, from tile coordinates to longitude/latitude. Fails with
/// [`OutOfRange`] for a point feature without rings. A feature of
/// `FeatureType::Unknown` fails a debug assertion and otherwise converts to
/// `Point(NaN, NaN)`.
pub fn convert_geometry(
    feature_type: FeatureType,
    geometries: &GeometryCollection,
    tile_id: &CanonicalTileID,
) -> Result<Geometry<f64>, OutOfRange> {
    let size = f64::from(EXTENT) * 2f64.powi(i32::from(tile_id.z));
    let x0 = f64::from(EXTENT) * f64::from(tile_id.x);
    let y0 = f64::from(EXTENT) * f64::from(tile_id.y);

    let tile_coordinates_to_lat_lng = |p: &Point<i16>| {
        let y2 = 180.0 - (f64::from(p.y) + y0) * 360.0 / size;
        Coord { x: (f64::from(p.x) + x0) * 360.0 / size - 180.0, y: (y2 * PI / 180.0).exp().atan() * 360.0 / PI - 90.0 }
    };
    tile_geometry(feature_type, geometries, tile_coordinates_to_lat_lng)
}

/// Converts `geometries`, the geometry of a feature of type `feature_type`
/// in `tile_id`, from tile coordinates to the world square, which tile
/// 0/0/0 covers from 0 to 1 on both axes, y down: the place of a map in a
/// registered projection, whose mapping takes it to longitude/latitude.
/// Fails as [`convert_geometry`] does.
pub fn convert_geometry_to_world(
    feature_type: FeatureType,
    geometries: &GeometryCollection,
    tile_id: &CanonicalTileID,
) -> Result<Geometry<f64>, OutOfRange> {
    let size = f64::from(EXTENT) * 2f64.powi(i32::from(tile_id.z));
    let x0 = f64::from(EXTENT) * f64::from(tile_id.x);
    let y0 = f64::from(EXTENT) * f64::from(tile_id.y);
    tile_geometry(feature_type, geometries, |p: &Point<i16>| Coord {
        x: (f64::from(p.x) + x0) / size,
        y: (f64::from(p.y) + y0) / size,
    })
}

/// `geometries`, the geometry of a feature of type `feature_type`, as a
/// geometry of `point`'s coordinates of its tile coordinates. Fails as
/// [`convert_geometry`] does.
fn tile_geometry(
    feature_type: FeatureType,
    geometries: &GeometryCollection,
    point: impl Fn(&Point<i16>) -> Coord<f64>,
) -> Result<Geometry<f64>, OutOfRange> {
    let line_string = |points: &[Point<i16>]| LineString(points.iter().map(&point).collect());

    match feature_type {
        FeatureType::Unknown => {
            debug_assert!(false, "convertGeometry of a feature of unknown type");
            Ok(Geometry::Point(geo_types::Point::new(f64::NAN, f64::NAN)))
        }

        FeatureType::Point => {
            let first = geometries.first().ok_or(OutOfRange { index: 0, len: geometries.len() })?;
            if let [p] = first.as_slice() {
                Ok(Geometry::Point(point(p).into()))
            } else {
                Ok(Geometry::MultiPoint(first.iter().map(|p| geo_types::Point(point(p))).collect()))
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

/// The lines or rings of a tile feature's geometry; a null geometry has
/// none.
#[must_use]
pub fn to_geometry_collection(geom: Option<&Geometry<i16>>) -> GeometryCollection {
    geom.map_or_else(GeometryCollection::new, |geom| GeometryCollection::from(geometry_rings(geom, Point::from)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collection(rings: &[&[(i16, i16)]]) -> GeometryCollection {
        GeometryCollection::from(
            rings.iter().map(|r| r.iter().map(|&(x, y)| Point::new(x, y)).collect()).collect::<Vec<_>>(),
        )
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
}
