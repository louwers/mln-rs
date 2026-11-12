// Ports vendor/maplibre-native-base/deps/geojson-vt-cpp/include/mapbox/geojsonvt/types.hpp.

use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use feature::{Identifier, PropertyMap};
use geometry::Point;

/// `vt_point`: a `point<double>` plus the simplification tolerance `z`.
/// `==` is `point<double>`'s (x and y only), as in C++.
#[derive(Clone, Copy, Debug, Default)]
pub struct VtPoint {
    pub x: f64,
    pub y: f64,
    /// simplification tolerance
    pub z: f64,
}

impl VtPoint {
    #[must_use]
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        VtPoint { x, y, z }
    }

    /// `vt_point(x, y)`
    #[must_use]
    pub const fn xy(x: f64, y: f64) -> Self {
        VtPoint { x, y, z: 0.0 }
    }
}

impl PartialEq for VtPoint {
    fn eq(&self, other: &Self) -> bool {
        self.x == other.x && self.y == other.y
    }
}

/// `get<I>(p)`: the coordinate on axis `I` (0 = x, 1 = y).
#[inline]
#[must_use]
pub fn get<const I: u8>(p: &VtPoint) -> f64 {
    if I == 0 { p.x } else { p.y }
}

/// `get<I>(const point<double>&)`
#[inline]
#[must_use]
pub fn get_point<const I: u8>(p: &Point<f64>) -> f64 {
    if I == 0 { p.x } else { p.y }
}

/// `calc_progress<I>(a, b, k)`
#[inline]
#[must_use]
pub fn calc_progress<const I: u8>(a: &VtPoint, b: &VtPoint, k: f64) -> f64 {
    if I == 0 { (k - a.x) / (b.x - a.x) } else { (k - a.y) / (b.y - a.y) }
}

/// `intersect<I>(a, b, k, t)`
#[inline]
#[must_use]
pub fn intersect<const I: u8>(a: &VtPoint, b: &VtPoint, k: f64, t: f64) -> VtPoint {
    if I == 0 {
        let y = (b.y - a.y) * t + a.y;
        VtPoint::new(k, y, 1.0)
    } else {
        let x = (b.x - a.x) * t + a.x;
        VtPoint::new(x, k, 1.0)
    }
}

pub type VtMultiPoint = Vec<VtPoint>;

/// `vt_line_string`; `==` compares the points only (`std::vector`'s).
#[derive(Clone, Debug, Default)]
pub struct VtLineString {
    pub points: Vec<VtPoint>,
    /// line length
    pub dist: f64,
    /// distance along a line in tile units, when lineMetrics = true
    pub seg_start: f64,
    pub seg_end: f64,
}

impl VtLineString {
    #[must_use]
    pub fn new(points: Vec<VtPoint>) -> Self {
        VtLineString { points, ..Default::default() }
    }
}

impl PartialEq for VtLineString {
    fn eq(&self, other: &Self) -> bool {
        self.points == other.points
    }
}

impl Deref for VtLineString {
    type Target = Vec<VtPoint>;
    fn deref(&self) -> &Vec<VtPoint> {
        &self.points
    }
}

impl DerefMut for VtLineString {
    fn deref_mut(&mut self) -> &mut Vec<VtPoint> {
        &mut self.points
    }
}

/// `vt_linear_ring`; `==` compares the points only.
#[derive(Clone, Debug, Default)]
pub struct VtLinearRing {
    pub points: Vec<VtPoint>,
    /// polygon ring area
    pub area: f64,
}

impl VtLinearRing {
    #[must_use]
    pub fn new(points: Vec<VtPoint>) -> Self {
        VtLinearRing { points, area: 0.0 }
    }
}

impl PartialEq for VtLinearRing {
    fn eq(&self, other: &Self) -> bool {
        self.points == other.points
    }
}

impl Deref for VtLinearRing {
    type Target = Vec<VtPoint>;
    fn deref(&self) -> &Vec<VtPoint> {
        &self.points
    }
}

impl DerefMut for VtLinearRing {
    fn deref_mut(&mut self) -> &mut Vec<VtPoint> {
        &mut self.points
    }
}

pub type VtMultiLineString = Vec<VtLineString>;
pub type VtPolygon = Vec<VtLinearRing>;
pub type VtMultiPolygon = Vec<VtPolygon>;
pub type VtGeometryCollection = Vec<VtGeometry>;

/// `vt_geometry`, alternatives in the C++ variant order.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum VtGeometry {
    #[default]
    Empty,
    Point(VtPoint),
    LineString(VtLineString),
    Polygon(VtPolygon),
    MultiPoint(VtMultiPoint),
    MultiLineString(VtMultiLineString),
    MultiPolygon(VtMultiPolygon),
    GeometryCollection(VtGeometryCollection),
}

impl VtGeometry {
    /// `mapbox::geometry::for_each_point(geometry, f)`
    pub fn for_each_point(&self, f: &mut impl FnMut(&VtPoint)) {
        match self {
            VtGeometry::Empty => {}
            VtGeometry::Point(p) => f(p),
            VtGeometry::LineString(l) => l.iter().for_each(&mut *f),
            VtGeometry::Polygon(rings) => rings.iter().flat_map(|r| r.iter()).for_each(&mut *f),
            VtGeometry::MultiPoint(points) => points.iter().for_each(&mut *f),
            VtGeometry::MultiLineString(lines) => lines.iter().flat_map(|l| l.iter()).for_each(&mut *f),
            VtGeometry::MultiPolygon(polygons) => {
                polygons.iter().flat_map(|p| p.iter()).flat_map(|r| r.iter()).for_each(&mut *f);
            }
            VtGeometry::GeometryCollection(geometries) => {
                for g in geometries {
                    g.for_each_point(f);
                }
            }
        }
    }

    /// `mapbox::geometry::for_each_point` with a mutable point.
    pub fn for_each_point_mut(&mut self, f: &mut impl FnMut(&mut VtPoint)) {
        match self {
            VtGeometry::Empty => {}
            VtGeometry::Point(p) => f(p),
            VtGeometry::LineString(l) => l.iter_mut().for_each(&mut *f),
            VtGeometry::Polygon(rings) => rings.iter_mut().flat_map(|r| r.iter_mut()).for_each(&mut *f),
            VtGeometry::MultiPoint(points) => points.iter_mut().for_each(&mut *f),
            VtGeometry::MultiLineString(lines) => lines.iter_mut().flat_map(|l| l.iter_mut()).for_each(&mut *f),
            VtGeometry::MultiPolygon(polygons) => {
                polygons.iter_mut().flat_map(|p| p.iter_mut()).flat_map(|r| r.iter_mut()).for_each(&mut *f);
            }
            VtGeometry::GeometryCollection(geometries) => {
                for g in geometries {
                    g.for_each_point_mut(f);
                }
            }
        }
    }
}

/// `mapbox::geometry::box<double>`
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoxD {
    pub min: Point<f64>,
    pub max: Point<f64>,
}

impl BoxD {
    /// `{ { 2, 1 }, { -1, 0 } }`, the initial (inverted) feature and tile
    /// bounding box.
    pub const INITIAL: BoxD = BoxD { min: Point::new(2.0, 1.0), max: Point::new(-1.0, 0.0) };
}

/// `vt_feature`
#[derive(Clone, Debug)]
pub struct VtFeature {
    pub geometry: VtGeometry,
    pub properties: Arc<PropertyMap>,
    pub id: Identifier,

    pub bbox: BoxD,
    pub num_points: u32,
}

impl VtFeature {
    /// `vt_feature(geom, shared_ptr<const property_map>, id)`
    #[must_use]
    pub fn new(geometry: VtGeometry, properties: Arc<PropertyMap>, id: Identifier) -> Self {
        let mut feature = VtFeature { geometry, properties, id, bbox: BoxD::INITIAL, num_points: 0 };
        feature.process_geometry();
        feature
    }

    /// `vt_feature(geom, const property_map&, id)`
    #[must_use]
    pub fn with_properties(geometry: VtGeometry, properties: &PropertyMap, id: Identifier) -> Self {
        Self::new(geometry, Arc::new(properties.clone()), id)
    }

    fn process_geometry(&mut self) {
        let bbox = &mut self.bbox;
        let num_points = &mut self.num_points;
        self.geometry.for_each_point(&mut |p| {
            bbox.min.x = f64::min(p.x, bbox.min.x);
            bbox.min.y = f64::min(p.y, bbox.min.y);
            bbox.max.x = f64::max(p.x, bbox.max.x);
            bbox.max.y = f64::max(p.y, bbox.max.y);
            *num_points = num_points.wrapping_add(1);
        });
    }
}

pub type VtFeatures = Vec<VtFeature>;
