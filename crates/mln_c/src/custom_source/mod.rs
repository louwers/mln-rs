//! The C API's custom sources, `custom-geometry` and `custom-vector`:
//! source types of this crate's own, whose tiles' data the host gives on
//! demand. A source holds the host's tile functions, which its origin's
//! provider calls on a pool thread with a [`CustomSourceHandle`] that the
//! host answers through. The runtimes that the C API makes register both
//! types (`crate::registry`).

use std::sync::Arc;

use mln_style::crs::CrsMapping;
use mln_style::error::Error;
use mln_style::geo::{LatLng, LatLngBounds};
use mln_style::tile_id::CanonicalTileID;

mod geometry;
mod origin;
mod provider;
mod vector;

pub(crate) use geometry::{CustomGeometrySourceOptions, CustomGeometrySourceType, TileOptions};
pub(crate) use vector::{CustomVectorSourceOptions, CustomVectorSourceType, CustomVectorTileData, TileDataFormat};

/// The coordinates that the tile `tile_id` spans in the grid of the
/// registered projection `crs`, or of Mercator with `None`. In a registered
/// projection, they are the bounds of the tile's four corners, as MapLibre GL
/// JS takes a tile's coordinates, which can miss part of the tile, such as a
/// pole inside it; `None` when no corner has a coordinate.
pub(crate) fn tile_bounds(tile_id: &CanonicalTileID, crs: Option<&CrsMapping>) -> Option<LatLngBounds> {
    let Some(crs) = crs else {
        return Some(LatLngBounds::from_canonical_tile(tile_id.z, tile_id.x, tile_id.y));
    };
    let tiles = 2f64.powi(i32::from(tile_id.z));
    let (left, top) = (f64::from(tile_id.x) / tiles, f64::from(tile_id.y) / tiles);
    let (right, bottom) = ((f64::from(tile_id.x) + 1.0) / tiles, (f64::from(tile_id.y) + 1.0) / tiles);
    let mut lng_lats = [[0.0; 2]; 4];
    crs.lng_lats_from_world(&[[left, top], [right, top], [right, bottom], [left, bottom]], &mut lng_lats);
    let mut corners = lng_lats.into_iter().filter_map(|[longitude, latitude]| LatLng::new(latitude, longitude).ok());
    let mut bounds = LatLngBounds::singleton(corners.next()?);
    for corner in corners {
        bounds.extend(&corner);
    }
    Some(bounds)
}

/// Called with the canonical ID of a tile that the map needs, or no longer
/// needs, and the handle that the host answers through. It runs on a pool
/// thread, one call at a time, in the order the tiles asked, so a host may
/// answer from inside it: the answer takes effect after the call.
pub(crate) type TileFunction<D> = Arc<dyn Fn(&CanonicalTileID, &CustomSourceHandle<D>) + Send + Sync>;

/// A custom source's tile functions, which take answers of type `D`.
pub(crate) struct TileFunctions<D> {
    /// Asks the host for a tile's data, which it gives with
    /// [`CustomSourceHandle::set_tile_data`].
    pub(crate) fetch_tile: Option<TileFunction<D>>,
    /// Tells the host that no tile needs a tile's data any more.
    pub(crate) cancel_tile: Option<TileFunction<D>>,
}

impl<D> std::fmt::Debug for TileFunctions<D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TileFunctions")
            .field("fetch_tile", &self.fetch_tile.is_some())
            .field("cancel_tile", &self.cancel_tile.is_some())
            .finish()
    }
}

/// What a custom source's handle reaches: the provider of the source's
/// tiles.
pub(crate) trait CustomTileResponder<D>: Send + Sync {
    /// Hands over the tile's data. It is dropped unless a tile needs it.
    fn set_tile_data(&self, tile_id: CanonicalTileID, data: D);
    /// Reports that the host failed to load the tile.
    fn set_tile_error(&self, tile_id: CanonicalTileID, error: Error);
    /// Drops the tile's data, and asks the host for it again if a tile needs
    /// it.
    fn invalidate_tile(&self, tile_id: CanonicalTileID);
    /// Invalidates every tile that `bounds` intersect, at its own zoom.
    fn invalidate_region(&self, bounds: LatLngBounds);
    /// Whether the provider is gone, so that the calls above do nothing.
    fn is_closed(&self) -> bool;
}

/// A `Send` handle to the provider of a custom source's tiles,
/// which the provider hands to the host's tile functions. The host answers
/// through it, from any thread; it does nothing once the provider is gone.
/// Clones reach the same provider.
pub(crate) struct CustomSourceHandle<D>(Arc<dyn CustomTileResponder<D>>);

impl<D> Clone for CustomSourceHandle<D> {
    fn clone(&self) -> Self {
        CustomSourceHandle(Arc::clone(&self.0))
    }
}

impl<D> std::fmt::Debug for CustomSourceHandle<D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomSourceHandle").finish_non_exhaustive()
    }
}

impl<D> CustomSourceHandle<D> {
    /// The handle of the provider behind `responder`.
    #[must_use]
    pub(crate) fn new(responder: Arc<dyn CustomTileResponder<D>>) -> Self {
        CustomSourceHandle(responder)
    }

    /// Hands over the tile's data. The source ignores data for a tile it
    /// hasn't asked for, or has since cancelled.
    pub(crate) fn set_tile_data(&self, tile_id: CanonicalTileID, data: D) {
        self.0.set_tile_data(tile_id, data);
    }

    /// Reports that the host failed to load the tile. Invalidating the tile,
    /// or needing it again, asks for it again.
    pub(crate) fn set_tile_error(&self, tile_id: CanonicalTileID, error: Error) {
        self.0.set_tile_error(tile_id, error);
    }

    /// Drops the tile's data, and asks the host for it again if a tile needs
    /// it.
    pub(crate) fn invalidate_tile(&self, tile_id: CanonicalTileID) {
        self.0.invalidate_tile(tile_id);
    }

    /// Invalidates every tile that `bounds` intersect.
    pub(crate) fn invalidate_region(&self, bounds: LatLngBounds) {
        self.0.invalidate_region(bounds);
    }

    /// Whether the provider is gone, so that the handle does nothing.
    #[must_use]
    pub(crate) fn is_closed(&self) -> bool {
        self.0.is_closed()
    }

    /// Whether both handles reach the same provider.
    #[must_use]
    pub(crate) fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::PI;

    use mln_style::crs::{Crs, CrsConverter, TileMatrixSet};

    use super::*;

    /// GL JS's test polar stereographic projection: EPSG:3413's true scale
    /// at 70 N and central meridian at 45 W, on a sphere, with EPSG:3413's
    /// tile matrix set.
    struct PolarStereographic;

    const DEGREES: f64 = 180.0 / PI;
    const CENTRAL_LONGITUDE: f64 = -45.0;

    fn polar_scale() -> f64 {
        6_378_137.0 * (1.0 + (70.0 / DEGREES).sin())
    }

    impl CrsConverter for PolarStereographic {
        fn forward(&self, lng_lats: &[[f64; 2]], positions: &mut [[f64; 2]]) {
            for (position, &[lng, lat]) in positions.iter_mut().zip(lng_lats) {
                let rho = polar_scale() * (PI / 4.0 - lat / DEGREES / 2.0).tan();
                let angle = (lng - CENTRAL_LONGITUDE) / DEGREES;
                *position = [rho * angle.sin(), -rho * angle.cos()];
            }
        }

        fn inverse(&self, positions: &[[f64; 2]], lng_lats: &mut [[f64; 2]]) {
            for (lng_lat, &[x, y]) in lng_lats.iter_mut().zip(positions) {
                *lng_lat = [
                    CENTRAL_LONGITUDE + x.atan2(-y) * DEGREES,
                    90.0 - 2.0 * (x.hypot(y) / polar_scale()).atan() * DEGREES,
                ];
            }
        }
    }

    /// The bounds as west, south, east and north.
    fn sides(bounds: &LatLngBounds) -> [f64; 4] {
        [bounds.west(), bounds.south(), bounds.east(), bounds.north()]
    }

    // Native: CustomGeometrySource.TileBoundsInARegisteredProjection, whose
    // bounds are GL JS's `tileIdToLngLatBounds` for the same tiles.
    #[test]
    fn a_tile_spans_the_bounds_of_its_corners_in_a_registered_projection_and_its_own_in_mercator() {
        let polar = CrsMapping::new(Arc::new(Crs {
            name: "test:polar".to_owned(),
            converter: Box::new(PolarStereographic),
            tile_matrix_set: TileMatrixSet { origin: [-4_194_304.0, 4_194_304.0], extent_at_zoom0: 8_388_608.0 },
        }));
        // The pole is inside tile 0/0/0, so the box of its corners misses it,
        // and it is the corner of each tile at zoom 1 and of 2/1/1.
        let tiles = [
            ((0, 0, 0), [-180.0, 38.768_726_773_326_68, 90.0, 38.768_726_773_326_68]),
            ((1, 0, 0), [-180.0, 38.768_726_773_326_68, 135.0, 90.0]),
            ((1, 1, 1), [-45.0, 38.768_726_773_326_68, 135.0, 90.0]),
            ((2, 1, 1), [-180.0, 63.038_043_248_050_77, 135.0, 90.0]),
            ((3, 4, 5), [-45.0, 68.537_007_824_457_65, 0.0, 80.310_774_471_895_98]),
            (
                (5, 17, 20),
                [-33.690_067_525_979_785, 76.980_615_692_833_65, -18.434_948_822_922_01, 80.014_060_002_872_81],
            ),
            (
                (8, 130, 140),
                [-36.253_837_737_444_79, 85.952_329_668_048_26, -30.963_756_532_073_52, 86.308_894_767_393_38],
            ),
        ];
        for ((z, x, y), expected) in tiles {
            let tile = CanonicalTileID::new(z, x, y);
            let bounds = sides(&tile_bounds(&tile, Some(&polar)).unwrap());
            for (side, expected) in bounds.iter().zip(expected) {
                assert!((side - expected).abs() < 1e-12, "{tile:?}: {bounds:?}");
            }
            let mercator = tile_bounds(&tile, None).unwrap();
            assert_eq!(sides(&mercator), sides(&LatLngBounds::from_canonical_tile(z, x, y)), "{tile:?}");
        }
    }
}
