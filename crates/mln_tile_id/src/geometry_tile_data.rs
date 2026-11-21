// Ring classification and the conversions between tile coordinates and
// longitude/latitude are here, where expressions reach them without the tile
// decoders; the algorithms that only tiles and buckets need (polygon fixup,
// hole limits, corner rounding, ...) are in `mln_tile`.

use std::borrow::Cow;
use std::f64::consts::PI;
use std::ops::{Deref, DerefMut, Range};
use std::sync::Arc;

use feature::{Identifier, PropertyMap, Value};
use geo_types::{Coord, CoordNum, Geometry, LineString, MultiLineString, MultiPolygon, Polygon};
use geometry::Point;
use mln_util::constants::EXTENT;
use mln_util::error::Error;
use mln_util::geometry::FeatureType;

use crate::CanonicalTileID;

/// Normalized vector tile coordinates. Each geometry coordinate represents a
/// point in a bidimensional space, varying from -V...0...+V, where V is the
/// maximum extent applicable.
pub type GeometryCoordinate = Point<i16>;

pub type GeometryCoordinates = Vec<GeometryCoordinate>;

/// The rings or lines of a feature's geometry, with the triangles of a
/// pre-tessellated polygon.
///
/// `Clone` keeps the triangles, as the C++ `clone()` does (C++ makes the
/// copy constructor private). `==` compares only the rings, as the
/// `std::vector` base does in C++.
#[derive(Clone, Debug, Default)]
pub struct GeometryCollection {
    rings: Vec<GeometryCoordinates>,
    /// The pre-tessellated triangles of an MLT polygon feature: C++
    /// `std::shared_ptr<const mlt::MapLibreTile> triangleOwner` plus the
    /// `std::span<const std::uint32_t> triangles` into its index buffer; here
    /// the shared buffer and the span's range in it.
    triangles: Option<(Arc<[u32]>, Range<usize>)>,
}

impl GeometryCollection {
    #[must_use]
    pub const fn new() -> Self {
        GeometryCollection { rings: Vec::new(), triangles: None }
    }

    /// The triangles of a pre-tessellated polygon: three indices per
    /// triangle into the vertices of the rings, numbered from 0 across the
    /// rings in order. Empty unless the tile carried triangles. Nothing
    /// checks that the indices are in range.
    #[must_use]
    pub fn get_triangles(&self) -> &[u32] {
        match &self.triangles {
            Some((owner, range)) => &owner[range.clone()],
            None => &[],
        }
    }

    /// Sets the triangles to `owner[range]`. Panics if `range` is out of
    /// bounds.
    pub fn set_triangles(&mut self, owner: Arc<[u32]>, range: Range<usize>) {
        assert!(range.start <= range.end && range.end <= owner.len());
        self.triangles = Some((owner, range));
    }
}

impl PartialEq for GeometryCollection {
    fn eq(&self, other: &Self) -> bool {
        self.rings == other.rings
    }
}

impl From<Vec<GeometryCoordinates>> for GeometryCollection {
    fn from(rings: Vec<GeometryCoordinates>) -> Self {
        GeometryCollection { rings, triangles: None }
    }
}

impl Deref for GeometryCollection {
    type Target = Vec<GeometryCoordinates>;
    fn deref(&self) -> &Self::Target {
        &self.rings
    }
}

impl DerefMut for GeometryCollection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.rings
    }
}

static EMPTY_PROPERTIES: PropertyMap = PropertyMap::new();
static EMPTY_GEOMETRIES: GeometryCollection = GeometryCollection::new();

/// A feature as expressions and buckets read it: a view of a tile's
/// feature (`mln_tile::Feature`), or a feature made for evaluating an
/// expression. Reading a value never fails: a reader whose data can be
/// malformed checks it when it makes the view.
pub trait GeometryTileFeature {
    fn get_type(&self) -> FeatureType;

    /// The value of the property `key`; `None` for none or a null value.
    fn get_value(&self, key: &str) -> Option<Value>;

    /// The feature's properties; empty by default. A view that decodes its
    /// properties on demand returns them owned.
    fn get_properties(&self) -> Cow<'_, PropertyMap> {
        Cow::Borrowed(&EMPTY_PROPERTIES)
    }

    /// The feature's id; [`Identifier::Null`] by default.
    fn get_id(&self) -> Identifier {
        Identifier::Null
    }

    /// The feature's geometries; empty by default. A view that decodes its
    /// geometry on demand returns it owned, and fails for a geometry it
    /// can't decode.
    fn get_geometries(&self) -> Result<Cow<'_, GeometryCollection>, Error> {
        Ok(Cow::Borrowed(&EMPTY_GEOMETRIES))
    }
}

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
    let line_string = |points: &[Point<i16>]| LineString(points.iter().map(tile_coordinates_to_lat_lng).collect());

    match feature_type {
        FeatureType::Unknown => {
            debug_assert!(false, "convertGeometry of a feature of unknown type");
            Ok(Geometry::Point(geo_types::Point::new(f64::NAN, f64::NAN)))
        }

        FeatureType::Point => {
            let first = geometries.first().ok_or(OutOfRange { index: 0, len: geometries.len() })?;
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

/// The lines or rings of a tile feature's geometry; a null geometry has
/// none.
#[must_use]
pub fn to_geometry_collection(geom: Option<&Geometry<i16>>) -> GeometryCollection {
    geom.map_or_else(GeometryCollection::new, |geom| GeometryCollection::from(geometry_rings(geom, Point::from)))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Feature;

    impl GeometryTileFeature for Feature {
        fn get_type(&self) -> FeatureType {
            FeatureType::Point
        }
        fn get_value(&self, _key: &str) -> Option<Value> {
            None
        }
    }

    #[test]
    fn defaults() {
        let f = Feature;
        assert!(f.get_properties().is_empty());
        assert_eq!(f.get_id(), Identifier::Null);
        assert!(f.get_geometries().unwrap().is_empty());
        let mut c = GeometryCollection::new();
        c.push(vec![Point::new(1, 2)]);
        assert_eq!(c.clone()[0][0], Point::new(1, 2));
    }

    #[test]
    fn triangles() {
        let mut c = GeometryCollection::from(vec![vec![Point::new(0, 0), Point::new(1, 0), Point::new(0, 1)]]);
        assert_eq!(c.get_triangles(), <&[u32]>::default());
        let owner: Arc<[u32]> = Arc::from(vec![9, 9, 0, 1, 2, 9]);
        c.set_triangles(owner, 2..5);
        assert_eq!(c.get_triangles(), &[0, 1, 2]);
        // The copy keeps them; == ignores them, as the C++ vector base does.
        assert_eq!(c.clone().get_triangles(), &[0, 1, 2]);
        assert_eq!(c, GeometryCollection::from(c.to_vec()));
    }

    fn collection(rings: &[&[(i16, i16)]]) -> GeometryCollection {
        GeometryCollection::from(
            rings
                .iter()
                .map(|r| r.iter().map(|&(x, y)| Point::new(x, y)).collect())
                .collect::<Vec<GeometryCoordinates>>(),
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
