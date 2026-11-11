// Ports src/mln/util/tile_range.hpp.

use geometry::Point;
use mln_math::wrap;
use mln_util::constants;
use mln_util::geo::{LatLng, LatLngBounds};
use mln_util::projection::Projection;
use mln_util::range::Range;

use crate::tile_id::CanonicalTileID;

/// `util::TileRange`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileRange {
    pub range: Range<Point<u32>>,
    pub zoom_range: Range<u8>,
}

/// `std::max(a, b)`.
fn std_max(a: f64, b: f64) -> f64 {
    if a < b { b } else { a }
}

/// `util::clamp(value, min, max)`: `std::max(min, std::min(max, value))`.
fn clamp(value: f64, min: f64, max: f64) -> f64 {
    let lower = if value < max { value } else { max };
    if min < lower { lower } else { min }
}

impl TileRange {
    /// Compute the range of tiles covered by the bounds at maxZoom.
    pub fn from_lat_lng_bounds_range(bounds: &LatLngBounds, mut min_zoom: u8, mut max_zoom: u8) -> TileRange {
        if min_zoom > max_zoom {
            std::mem::swap(&mut min_zoom, &mut max_zoom);
        }

        // Wrap the west edge into the world and keep the bounds' width, so
        // unwrapped longitudes name the same columns as wrapped ones. `%` (fmod)
        // is exact, so it goes first, before any rounding: a huge longitude
        // still lands on its own meridian. The width of bounds with huge
        // longitudes can overflow to infinity, which covers the world. An
        // inverted interval counts as its west edge.
        let west = wrap(bounds.west() % constants::DEGREES_MAX, -constants::LONGITUDE_MAX, constants::LONGITUDE_MAX);
        let width = std_max(bounds.east() - bounds.west(), 0.0);
        let covers_world = !(width < constants::DEGREES_MAX);
        let max_tile = 2.0f64.powf(f64::from(max_zoom));
        // C++ `LatLng(lat, lon)` throws for a NaN or infinite edge, which
        // these finite, in-range edges can't be.
        let (Ok(sw), Ok(ne)) = (
            LatLng::new(bounds.south(), west),
            LatLng::new(bounds.north(), if covers_world { west } else { west + width }),
        ) else {
            debug_assert!(false, "LatLng from LatLngBounds coordinates");
            let max = (max_tile - 1.0) as u32;
            return TileRange {
                range: Range::new(Point::new(0, 0), Point::new(max, max)),
                zoom_range: Range::new(min_zoom, max_zoom),
            };
        };
        let sw_proj = Projection::project_zoom(&sw, i32::from(max_zoom));
        let ne_proj = Projection::project_zoom(&ne, i32::from(max_zoom));

        // Port-only, ahead of ffi patch 0076 (maplibre-native-ffi#1759): an
        // east edge at 180° ends in the last column. Uncapped, it named the
        // column past the last, which wrapped to column 0. A west edge just
        // under 180° can round to 180° when projected, so it's capped too.
        let west_column = sw_proj.x.floor().min(max_tile - 1.0);
        let east_column = if west + width <= constants::LONGITUDE_MAX {
            ne_proj.x.floor().min(max_tile - 1.0)
        } else {
            ne_proj.x.floor()
        };
        let mut min_x = 0u32;
        let mut max_x = (max_tile - 1.0) as u32;
        // Bounds that reach every column cover the world. Otherwise, bounds
        // that cross the antimeridian end east of the last column, and wrap.
        if !covers_world && east_column - west_column + 1.0 < max_tile {
            min_x = west_column as u32;
            max_x = (if east_column < max_tile { east_column } else { east_column - max_tile }) as u32;
        }
        let min_y = clamp(ne_proj.y.floor(), 0.0, max_tile) as u32;
        let max_y = clamp(sw_proj.y.floor(), 0.0, max_tile) as u32;

        TileRange {
            range: Range::new(Point::new(min_x, min_y), Point::new(max_x, max_y)),
            zoom_range: Range::new(min_zoom, max_zoom),
        }
    }

    /// Compute the range of tiles covered by the bounds.
    pub fn from_lat_lng_bounds(bounds: &LatLngBounds, z: u8) -> TileRange {
        Self::from_lat_lng_bounds_range(bounds, z, z)
    }

    pub fn contains(&self, tile_id: &CanonicalTileID) -> bool {
        if tile_id.z <= self.zoom_range.max && tile_id.z >= self.zoom_range.min {
            if tile_id.z == 0 {
                return true;
            }
            let dz = self.zoom_range.max - tile_id.z;
            let x0 = self.range.min.x.wrapping_shr(u32::from(dz));
            let x1 = self.range.max.x.wrapping_shr(u32::from(dz));
            let y0 = self.range.min.y.wrapping_shr(u32::from(dz));
            let y1 = self.range.max.y.wrapping_shr(u32::from(dz));
            return (if self.range.min.x > self.range.max.x {
                // For wrapped bounds
                tile_id.x >= x0 || tile_id.x <= x1
            } else {
                tile_id.x <= x1 && tile_id.x >= x0
            }) && tile_id.y <= y1
                && tile_id.y >= y0;
        }
        false
    }
}
