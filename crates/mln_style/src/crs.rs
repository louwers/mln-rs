//! Planar coordinate reference systems that a map can draw in: an app
//! registers each with its registry, and a style's `projection.type` names
//! one. A [`CrsMapping`] maps lng/lat to and from the world square of one.

mod mapping;

use std::fmt;
use std::sync::Arc;

pub use mapping::{BoxSamples, CrsMapping, WorldBox};

use crate::projection::IDENTITY_PROJECTION_NAME;

/// Converts positions between lng/lat and a coordinate reference system's
/// coordinates, a batch at a time. A position that has no answer comes out
/// NaN. The map calls it from any thread, several at once.
pub trait CrsConverter: Send + Sync {
    /// Each lng/lat in degrees of `lng_lats` to the CRS's coordinates, into
    /// `positions`, which is as long.
    fn forward(&self, lng_lats: &[[f64; 2]], positions: &mut [[f64; 2]]);

    /// Each position of the CRS in `positions` to lng/lat in degrees, into
    /// `lng_lats`, which is as long.
    fn inverse(&self, positions: &[[f64; 2]], lng_lats: &mut [[f64; 2]]);
}

/// The square, power-of-two quad tile grid over a coordinate reference
/// system's plane, what the OGC Two Dimensional Tile Matrix Set standard
/// calls a quad tree tile matrix set: tile 0/0/0 is the square of side
/// `extent_at_zoom0` whose top-left corner is `origin`, and each zoom level
/// splits every tile into four.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileMatrixSet {
    /// The CRS coordinates of the top-left corner of tile 0/0/0, its least x
    /// and greatest y, in the order that the converter's `forward` gives
    /// them.
    pub origin: [f64; 2],
    /// The side of tile 0/0/0, in the CRS's units.
    pub extent_at_zoom0: f64,
}

/// A planar coordinate reference system and the tile grid over it, which a
/// map draws in when its style's `projection.type` names it. Every tile
/// source of the map is taken to be cut in that grid, and its tiles draw as
/// they are. The CRS's units count as meters wherever the map converts
/// meters.
pub struct Crs {
    /// The name that `projection.type` gives, such as `EPSG:2193`. An app's
    /// projections have a colon in their names: names without one are kept
    /// for built-in projections.
    pub name: String,
    pub converter: Box<dyn CrsConverter>,
    pub tile_matrix_set: TileMatrixSet,
}

impl Crs {
    /// The built-in `identity` projection, for maps of a plane, such as a
    /// floor plan or a scanned image: its coordinates are lng/lat degrees
    /// unchanged, and tile 0/0/0 spans -90 to 90 on both axes.
    #[must_use]
    pub fn identity() -> Crs {
        Crs {
            name: IDENTITY_PROJECTION_NAME.to_owned(),
            converter: Box::new(Identity),
            tile_matrix_set: TileMatrixSet { origin: [-90.0, 90.0], extent_at_zoom0: 180.0 },
        }
    }
}

/// Whether `a` and `b` are the same registered projection's mapping, or
/// both `None`, as for the built-in projections.
#[must_use]
pub fn same_mapping(a: Option<&Arc<CrsMapping>>, b: Option<&Arc<CrsMapping>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        _ => false,
    }
}

impl fmt::Debug for Crs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Crs")
            .field("name", &self.name)
            .field("tile_matrix_set", &self.tile_matrix_set)
            .finish_non_exhaustive()
    }
}

/// The converter of [`Crs::identity`], whose coordinates are lng/lat.
struct Identity;

impl CrsConverter for Identity {
    fn forward(&self, lng_lats: &[[f64; 2]], positions: &mut [[f64; 2]]) {
        for (position, lng_lat) in positions.iter_mut().zip(lng_lats) {
            *position = *lng_lat;
        }
    }

    fn inverse(&self, positions: &[[f64; 2]], lng_lats: &mut [[f64; 2]]) {
        self.forward(positions, lng_lats);
    }
}
