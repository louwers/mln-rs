use geometry::Point;
use mln_style::constants;
use mln_style::geo::{LatLng, LatLngBounds};
use mln_style::math::{clamp_f64, wrap};
use mln_style::range::Range;
use mln_style::tile_id::CanonicalTileID;

use crate::geo::projection::Projection;
use crate::geo::world::WorldBox;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileRange {
    pub(crate) range: Range<Point<u32>>,
    pub(crate) zoom_range: Range<u8>,
}

impl TileRange {
    /// Compute the range of tiles covered by the bounds at maxZoom.
    #[must_use]
    pub(crate) fn from_lat_lng_bounds_range(bounds: &LatLngBounds, mut min_zoom: u8, mut max_zoom: u8) -> TileRange {
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
        let width = f64::max(bounds.east() - bounds.west(), 0.0);
        let covers_world = width.is_nan() || width >= constants::DEGREES_MAX;
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
        let min_y = clamp_f64(ne_proj.y.floor(), 0.0, max_tile) as u32;
        let max_y = clamp_f64(sw_proj.y.floor(), 0.0, max_tile) as u32;

        TileRange {
            range: Range::new(Point::new(min_x, min_y), Point::new(max_x, max_y)),
            zoom_range: Range::new(min_zoom, max_zoom),
        }
    }

    /// Compute the range of tiles covered by the bounds.
    #[must_use]
    pub fn from_lat_lng_bounds(bounds: &LatLngBounds, z: u8) -> TileRange {
        Self::from_lat_lng_bounds_range(bounds, z, z)
    }

    /// The tiles from `min_zoom` to `max_zoom` that a rectangle in the world
    /// square covers part of, as GL JS's `TileBounds` counts them: a tile
    /// that the rectangle only touches at its edge doesn't count. A
    /// rectangle as thin as a line covers the tiles it lies in, east and
    /// south of a tile edge it lies on, and one outside the square covers
    /// none.
    #[must_use]
    pub fn from_world_box(world_box: &WorldBox, mut min_zoom: u8, mut max_zoom: u8) -> TileRange {
        if min_zoom > max_zoom {
            std::mem::swap(&mut min_zoom, &mut max_zoom);
        }
        let WorldBox { min, max } = *world_box;
        if !(max.x > 0.0 && max.y > 0.0 && min.x < 1.0 && min.y < 1.0) {
            // A zoom range that no tile is in.
            return TileRange { range: Range::new(Point::new(0, 0), Point::new(0, 0)), zoom_range: Range::new(1, 0) };
        }
        let max_tile = 2.0f64.powf(f64::from(max_zoom));
        let first = |world: f64| clamp_f64((world * max_tile).floor(), 0.0, max_tile - 1.0);
        // The last tile before the edge, and never one before the first,
        // which `contains` would take for a range across the antimeridian.
        let last = |world: f64, first: f64| clamp_f64((world * max_tile).ceil() - 1.0, first, max_tile - 1.0);
        let (min_x, min_y) = (first(min.x), first(min.y));
        TileRange {
            range: Range::new(
                Point::new(min_x as u32, min_y as u32),
                Point::new(last(max.x, min_x) as u32, last(max.y, min_y) as u32),
            ),
            zoom_range: Range::new(min_zoom, max_zoom),
        }
    }

    #[must_use]
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
