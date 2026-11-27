//! The features that tiles are cut from: GeoJSON features projected onto
//! Web Mercator's unit square, and the pieces that cutting leaves of them.

use std::sync::Arc;

use geo_types::Coord;
use mln_style::feature::{Identifier, PropertyMap};
use smallvec::SmallVec;

/// The x axis, for the functions generic over an axis.
pub(super) const X: usize = 0;
/// The y axis.
pub(super) const Y: usize = 1;

/// A projected point, and how much it matters to its line's shape.
#[derive(Clone, Copy, Debug)]
pub(super) struct Vertex {
    pub(super) x: f64,
    pub(super) y: f64,
    /// The squared distance that simplifying the line through this point
    /// would move it by: a tile keeps the points that matter more than its
    /// squared tolerance. Ends of lines and points that cutting adds have 1,
    /// more than any tolerance.
    pub(super) importance: f64,
}

impl Vertex {
    pub(super) fn new(x: f64, y: f64, importance: f64) -> Self {
        Vertex { x, y, importance }
    }

    /// The coordinate on `AXIS`.
    pub(super) fn get<const AXIS: usize>(self) -> f64 {
        if AXIS == X { self.x } else { self.y }
    }

    pub(super) fn coord(self) -> Coord {
        Coord { x: self.x, y: self.y }
    }

    pub(super) fn same_position(self, other: Vertex) -> bool {
        self.x == other.x && self.y == other.y
    }
}

/// A line, or a piece of one that cutting left.
#[derive(Clone, Debug, Default)]
pub(super) struct Line {
    pub(super) points: Vec<Vertex>,
    /// The length of the whole line.
    pub(super) length: f64,
    /// How far along the whole line this piece starts and ends. Cutting
    /// tracks them only with line metrics.
    pub(super) start: f64,
    pub(super) end: f64,
}

/// A polygon's ring, or what cutting left of one.
#[derive(Clone, Debug, Default)]
pub(super) struct Ring {
    pub(super) points: Vec<Vertex>,
    /// The area of the whole ring.
    pub(super) area: f64,
}

/// A geometry's parts, held inline when there's one, as most often.
pub(super) type Parts<T> = SmallVec<[T; 1]>;

/// A feature's geometry: one or more points, lines or polygons. A polygon is
/// its rings, the outer one first.
#[derive(Clone, Debug)]
pub(super) enum Geometry {
    Points(Parts<Vertex>),
    Lines(Parts<Line>),
    Polygons(Parts<Vec<Ring>>),
}

impl Geometry {
    pub(super) fn vertices(&self) -> impl Iterator<Item = &Vertex> {
        let (points, lines, polygons) = match self {
            Geometry::Points(points) => (Some(points), None, None),
            Geometry::Lines(lines) => (None, Some(lines), None),
            Geometry::Polygons(polygons) => (None, None, Some(polygons)),
        };
        let lines = lines.into_iter().flatten().flat_map(|line| &line.points);
        let rings = polygons.into_iter().flatten().flatten().flat_map(|ring| &ring.points);
        points.into_iter().flatten().chain(lines).chain(rings)
    }

    pub(super) fn vertices_mut(&mut self) -> impl Iterator<Item = &mut Vertex> {
        let (points, lines, polygons) = match self {
            Geometry::Points(points) => (Some(points), None, None),
            Geometry::Lines(lines) => (None, Some(lines), None),
            Geometry::Polygons(polygons) => (None, None, Some(polygons)),
        };
        let lines = lines.into_iter().flatten().flat_map(|line| &mut line.points);
        let rings = polygons.into_iter().flatten().flatten().flat_map(|ring| &mut ring.points);
        points.into_iter().flatten().chain(lines).chain(rings)
    }
}

/// A box that bounds features. The empty box bounds nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Bbox {
    pub(super) min: Coord,
    pub(super) max: Coord,
}

impl Bbox {
    pub(super) const EMPTY: Bbox = Bbox {
        min: Coord { x: f64::INFINITY, y: f64::INFINITY },
        max: Coord { x: f64::NEG_INFINITY, y: f64::NEG_INFINITY },
    };

    pub(super) fn extend(&mut self, other: Bbox) {
        self.min.x = self.min.x.min(other.min.x);
        self.min.y = self.min.y.min(other.min.y);
        self.max.x = self.max.x.max(other.max.x);
        self.max.y = self.max.y.max(other.max.y);
    }

    /// The range on `AXIS`.
    pub(super) fn range<const AXIS: usize>(self) -> (f64, f64) {
        if AXIS == X { (self.min.x, self.max.x) } else { (self.min.y, self.max.y) }
    }
}

/// A feature with at least one point.
#[derive(Clone, Debug)]
pub(super) struct Feature {
    pub(super) geometry: Geometry,
    pub(super) properties: Arc<PropertyMap>,
    pub(super) id: Identifier,
    pub(super) bbox: Bbox,
    pub(super) num_points: u32,
}

impl Feature {
    /// The feature of `geometry`, or `None` if it has no points.
    pub(super) fn new(geometry: Geometry, properties: Arc<PropertyMap>, id: Identifier) -> Option<Self> {
        let mut bbox = Bbox::EMPTY;
        let mut num_points = 0u32;
        for v in geometry.vertices() {
            bbox.extend(Bbox { min: v.coord(), max: v.coord() });
            num_points += 1;
        }
        (num_points > 0).then_some(Feature { geometry, properties, id, bbox, num_points })
    }

    /// Moves the feature by `dx` along the x axis.
    pub(super) fn shift_x(&mut self, dx: f64) {
        for v in self.geometry.vertices_mut() {
            v.x += dx;
        }
        self.bbox.min.x += dx;
        self.bbox.max.x += dx;
    }
}
