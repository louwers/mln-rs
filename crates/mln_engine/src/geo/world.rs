//! The world square a map draws in, which tile 0/0/0 covers and the quad tree
//! of tiles subdivides, and how lng/lat maps to and from it: Web Mercator, or
//! the coordinate reference system of a registered projection.

use std::f64::consts::PI;
use std::sync::Arc;

use geometry::Point;
use mln_style::constants::TILE_SIZE_D;
pub use mln_style::crs::WorldBox;
use mln_style::crs::{CrsMapping, TileMatrixSet};
use mln_style::error::Error;
use mln_style::geo::{LatLng, LatLngBounds, WrapMode};

use crate::geo::constants::EARTH_RADIUS_M;
use crate::geo::projection::Projection;

/// A position Mercator doesn't answer for.
const NO_ANSWER: [f64; 2] = [f64::NAN, f64::NAN];

fn is_answer(position: &[f64; 2]) -> bool {
    position[0].is_finite() && position[1].is_finite()
}

/// How a map's lng/lat maps to and from its world square, where tile 0/0/0
/// spans 0 to 1 on both axes and y grows down.
///
/// Mercator keeps the formulas of [`Projection`], so its answers are bit for
/// bit those of the code that calls them. Two mappings are equal when they
/// are the same one: Mercator, or the same registered projection's.
#[derive(Clone, Debug, Default)]
pub enum WorldMapping {
    /// Web Mercator, whose world repeats east and west.
    #[default]
    Mercator,
    /// A registered projection's coordinate reference system: a bounded
    /// plane, with no world copies, no wrapping and no latitude limit. A
    /// style's registry holds the mapping, which every map drawn in it
    /// shares.
    Crs(Arc<CrsMapping>),
}

impl PartialEq for WorldMapping {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (WorldMapping::Mercator, WorldMapping::Mercator) => true,
            (WorldMapping::Crs(a), WorldMapping::Crs(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

impl WorldMapping {
    /// `mercator`, or the registered projection's name.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            WorldMapping::Mercator => "mercator",
            WorldMapping::Crs(mapping) => &mapping.crs().name,
        }
    }

    /// Whether the world repeats east and west: world copies, longitude
    /// wrapping and Mercator's latitude limit.
    #[must_use]
    pub fn wraps(&self) -> bool {
        matches!(self, WorldMapping::Mercator)
    }

    /// `lat_lng` in the world copy that longitudes from -180 to 180 cover,
    /// where the world wraps; unchanged where it doesn't.
    #[must_use]
    pub fn wrap(&self, lat_lng: LatLng) -> LatLng {
        if self.wraps() { lat_lng.wrapped() } else { lat_lng }
    }

    /// The tile grid whose tile 0/0/0 is the world square: EPSG:3857 meters
    /// on Mercator.
    #[must_use]
    pub fn tile_matrix_set(&self) -> TileMatrixSet {
        match self {
            WorldMapping::Mercator => TileMatrixSet {
                origin: [-PI * EARTH_RADIUS_M, PI * EARTH_RADIUS_M],
                extent_at_zoom0: 2.0 * PI * EARTH_RADIUS_M,
            },
            WorldMapping::Crs(mapping) => mapping.crs().tile_matrix_set,
        }
    }

    /// Each lng/lat in degrees of `lng_lats` to its place in the world
    /// square, into `world`, which is as long: NaN where the projection has
    /// none.
    pub fn world_from_lng_lats(&self, lng_lats: &[[f64; 2]], world: &mut [[f64; 2]]) {
        match self {
            WorldMapping::Mercator => {
                for (point, lng_lat) in world.iter_mut().zip(lng_lats) {
                    *point = if is_answer(lng_lat) {
                        let lat_lng = LatLng::saturating(lng_lat[1], lng_lat[0], WrapMode::Unwrapped);
                        let projected = Projection::project(&lat_lng, 1.0 / TILE_SIZE_D);
                        [projected.x, projected.y]
                    } else {
                        NO_ANSWER
                    };
                }
            }
            WorldMapping::Crs(mapping) => mapping.world_from_lng_lats(lng_lats, world),
        }
    }

    /// Each place in the world square of `world` to lng/lat in degrees, into
    /// `lng_lats`, which is as long, with the latitude within 90 degrees of
    /// the equator. A registered projection's place that has no answer of its
    /// own takes the nearest one as [`CrsMapping::lng_lats_from_world`]
    /// finds it; NaN where there is none.
    pub fn lng_lats_from_world(&self, world: &[[f64; 2]], lng_lats: &mut [[f64; 2]]) {
        match self {
            WorldMapping::Mercator => {
                for (lng_lat, point) in lng_lats.iter_mut().zip(world) {
                    *lng_lat = match mercator_lat_lng(&Point::new(point[0], point[1])) {
                        Ok(lat_lng) => [lat_lng.longitude(), lat_lng.latitude()],
                        Err(_) => NO_ANSWER,
                    };
                }
            }
            WorldMapping::Crs(mapping) => mapping.lng_lats_from_world(world, lng_lats),
        }
    }

    /// The place of `lat_lng` in the world square, NaN where the projection
    /// has none.
    #[must_use]
    pub fn world_from_lat_lng(&self, lat_lng: &LatLng) -> Point<f64> {
        let mut world = [[0.0; 2]];
        self.world_from_lng_lats(&[[lat_lng.longitude(), lat_lng.latitude()]], &mut world);
        Point::new(world[0][0], world[0][1])
    }

    /// The location of a place in the world square, as
    /// [`lng_lats_from_world`](Self::lng_lats_from_world) finds it. Fails
    /// with [`ErrorKind::InvalidArgument`](mln_style::error::ErrorKind) where
    /// it finds none.
    pub fn lat_lng_from_world(&self, world: &Point<f64>) -> Result<LatLng, Error> {
        match self {
            WorldMapping::Mercator => mercator_lat_lng(world),
            WorldMapping::Crs(mapping) => {
                let mut lng_lat = [[0.0; 2]];
                mapping.lng_lats_from_world(&[[world.x, world.y]], &mut lng_lat);
                if !is_answer(&lng_lat[0]) {
                    return Err(Error::invalid_argument(format!(
                        "the projection \"{}\" has no location at the world position {}, {}",
                        mapping.crs().name,
                        world.x,
                        world.y
                    )));
                }
                LatLng::with_mode(lng_lat[0][1], lng_lat[0][0], WrapMode::Unwrapped)
            }
        }
    }

    /// The point of `lat_lng` in a world `scale` times the tile size wide:
    /// [`Projection::project`] on Mercator.
    #[must_use]
    pub fn project(&self, lat_lng: &LatLng, scale: f64) -> Point<f64> {
        match self {
            WorldMapping::Mercator => Projection::project(lat_lng, scale),
            WorldMapping::Crs(_) => {
                let world = self.world_from_lat_lng(lat_lng);
                let world_size = Projection::world_size(scale);
                Point::new(world.x * world_size, world.y * world_size)
            }
        }
    }

    /// The location of a point in a world `scale` times the tile size wide:
    /// [`Projection::unproject`] on Mercator. A registered projection's world
    /// doesn't wrap, and ignores `wrap_mode`. Fails as
    /// [`lat_lng_from_world`](Self::lat_lng_from_world) does, and on Mercator
    /// as `Projection::unproject` does.
    pub fn unproject(&self, point: &Point<f64>, scale: f64, wrap_mode: WrapMode) -> Result<LatLng, Error> {
        match self {
            WorldMapping::Mercator => Projection::unproject(point, scale, wrap_mode),
            WorldMapping::Crs(_) => {
                let world_size = Projection::world_size(scale);
                self.lat_lng_from_world(&Point::new(point.x / world_size, point.y / world_size))
            }
        }
    }

    /// Whether a place in the world square has a location of its own,
    /// rather than the nearest one that
    /// [`lng_lats_from_world`](Self::lng_lats_from_world) falls back on:
    /// always on Mercator.
    #[must_use]
    pub fn has_lat_lng(&self, world: &Point<f64>) -> bool {
        match self {
            WorldMapping::Mercator => true,
            WorldMapping::Crs(mapping) => mapping.has_lat_lng([world.x, world.y]),
        }
    }

    /// How many meters a world unit spans at a place in the world square:
    /// on Mercator, the zoom 0 world at its latitude; for a registered
    /// projection, the extent of its tile matrix set, its units taken as
    /// meters, anywhere.
    #[must_use]
    pub fn meters_per_world_unit(&self, world: &Point<f64>) -> f64 {
        match self {
            WorldMapping::Mercator => {
                let latitude = mercator_lat_lng(world).map_or(f64::NAN, |lat_lng| lat_lng.latitude());
                Projection::meters_per_pixel_at_latitude(latitude, 0.0) * TILE_SIZE_D
            }
            WorldMapping::Crs(mapping) => mapping.crs().tile_matrix_set.extent_at_zoom0,
        }
    }

    /// The rectangle in the world square around a lng/lat box: in a
    /// registered projection, the box sampled on a grid as
    /// [`CrsMapping::lng_lat_box_to_world_box`] samples it; on Mercator,
    /// whose edges are straight, the rectangle of its corners.
    #[must_use]
    pub fn lng_lat_box_to_world_box(&self, bounds: &LatLngBounds) -> WorldBox {
        match self {
            WorldMapping::Mercator => {
                let corners = [
                    [bounds.west(), bounds.north()],
                    [bounds.east(), bounds.north()],
                    [bounds.west(), bounds.south()],
                    [bounds.east(), bounds.south()],
                ];
                let mut places = [[0.0; 2]; 4];
                self.world_from_lng_lats(&corners, &mut places);
                let mut min = Point::new(f64::INFINITY, f64::INFINITY);
                let mut max = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
                for place in places {
                    min = Point::new(min.x.min(place[0]), min.y.min(place[1]));
                    max = Point::new(max.x.max(place[0]), max.y.max(place[1]));
                }
                WorldBox { min, max }
            }
            WorldMapping::Crs(mapping) => mapping.lng_lat_box_to_world_box(bounds),
        }
    }
}

/// Mercator's location of a place in the world square.
fn mercator_lat_lng(world: &Point<f64>) -> Result<LatLng, Error> {
    Projection::unproject(world, 1.0 / TILE_SIZE_D, WrapMode::Unwrapped)
}

#[cfg(test)]
#[path = "world_test.rs"]
mod world_test;
