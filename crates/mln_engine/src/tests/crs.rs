//! Coordinate reference systems for the tests of registered projections,
//! MapLibre Native's (`test/include/mln/test/crs.hpp`) and GL JS's
//! (`src/util/test/util.ts`).

use std::f64::consts::PI;
use std::sync::Arc;

use mln_style::crs::{Crs, CrsConverter, CrsMapping, TileMatrixSet};

use crate::geo::world::WorldMapping;

/// A converter that takes each position on its own through `forward` and
/// `inverse`.
struct EachPosition<F, I> {
    forward: F,
    inverse: I,
}

impl<F, I> CrsConverter for EachPosition<F, I>
where
    F: Fn([f64; 2]) -> [f64; 2] + Send + Sync,
    I: Fn([f64; 2]) -> [f64; 2] + Send + Sync,
{
    fn forward(&self, lng_lats: &[[f64; 2]], positions: &mut [[f64; 2]]) {
        for (position, lng_lat) in positions.iter_mut().zip(lng_lats) {
            *position = (self.forward)(*lng_lat);
        }
    }

    fn inverse(&self, positions: &[[f64; 2]], lng_lats: &mut [[f64; 2]]) {
        for (lng_lat, position) in lng_lats.iter_mut().zip(positions) {
            *lng_lat = (self.inverse)(*position);
        }
    }
}

/// The CRS `name` that converts each position through `forward` and
/// `inverse`, over the tile matrix set at `origin` with `extent` at zoom 0.
pub(crate) fn crs(
    name: &str,
    forward: impl Fn([f64; 2]) -> [f64; 2] + Send + Sync + 'static,
    inverse: impl Fn([f64; 2]) -> [f64; 2] + Send + Sync + 'static,
    origin: [f64; 2],
    extent: f64,
) -> Crs {
    Crs {
        name: name.to_owned(),
        converter: Box::new(EachPosition { forward, inverse }),
        tile_matrix_set: TileMatrixSet { origin, extent_at_zoom0: extent },
    }
}

/// The mapping of `crs`, as a registry holds it.
pub(crate) fn mapping(crs: Crs) -> WorldMapping {
    WorldMapping::Crs(Arc::new(CrsMapping::new(Arc::new(crs))))
}

/// The built-in identity projection's mapping.
pub(crate) fn identity() -> WorldMapping {
    mapping(Crs::identity())
}

/// A north polar stereographic projection of a sphere, with a tile matrix set
/// the size of EPSG:3413's.
pub(crate) fn north_polar_stereographic(name: &str) -> Crs {
    const RADIUS: f64 = 6_378_137.0;
    crs(
        name,
        |[lng, lat]| {
            let lambda = lng * PI / 180.0;
            let rho = 2.0 * RADIUS * (PI / 4.0 - lat * PI / 360.0).tan();
            [rho * lambda.sin(), -rho * lambda.cos()]
        },
        |[x, y]| {
            let rho = x.hypot(y);
            let latitude = 90.0 - (rho / (2.0 * RADIUS)).atan() * 360.0 / PI;
            [x.atan2(-y) * 180.0 / PI, latitude]
        },
        [-4_194_304.0, 4_194_304.0],
        8_388_608.0,
    )
}

/// GL JS's test polar stereographic projection: EPSG:3413's true scale at
/// 70 N and central meridian at 45 W, on a sphere, with EPSG:3413's tile
/// matrix set.
pub(crate) fn polar_stereographic(name: &str) -> Crs {
    const DEGREES: f64 = 180.0 / PI;
    const CENTRAL_LONGITUDE: f64 = -45.0;
    let scale = 6_378_137.0 * (1.0 + (70.0 / DEGREES).sin());
    crs(
        name,
        move |[lng, lat]| {
            let rho = scale * (PI / 4.0 - lat / DEGREES / 2.0).tan();
            let angle = (lng - CENTRAL_LONGITUDE) / DEGREES;
            [rho * angle.sin(), -rho * angle.cos()]
        },
        move |[x, y]| [CENTRAL_LONGITUDE + x.atan2(-y) * DEGREES, 90.0 - 2.0 * (x.hypot(y) / scale).atan() * DEGREES],
        [-4_194_304.0, 4_194_304.0],
        8_388_608.0,
    )
}

/// The identity, with an inverse that has no answer outside the disc
/// inscribed in its tile matrix set.
pub(crate) fn disc(name: &str) -> Crs {
    crs(
        name,
        |position| position,
        |position| if position[0].hypot(position[1]) > 90.0 { [f64::NAN; 2] } else { position },
        [-90.0, 90.0],
        180.0,
    )
}
