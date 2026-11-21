//! Geometry for drawing on the globe, which the engine's own tile meshes
//! use too.
//!
//! On the globe, the vertex shaders' projection
//! ([`ProjectionUBO`](mln_gfx::shaders::layer_ubo::ProjectionUBO)) bends
//! each tile onto the sphere one vertex at a time, so what a tile draws
//! needs vertices often enough to follow the curve. A line or a polygon is
//! split wherever it crosses the boundary of a cell of a grid over the tile
//! ([`subdivide_line`], [`subdivide_ring`], [`subdivide_polygon`]), with
//! the cells per axis, the granularity, that the layout's
//! [`subdivision_granularity`](crate::layer::BucketParameters::subdivision_granularity)
//! gives for the tile's zoom: it splits nothing where the map isn't a
//! globe, and less than 2 splits nothing. A surface that covers the tile,
//! such as a background or a raster, draws with the tile's grid
//! ([`tile_grid`], or [`masked_tile_grids`] for the parts its mask keeps),
//! whose cells the tile's zoom sets.
//!
//! A tile of the top or bottom row reaches its pole: a vertex at
//! [`NORTH_POLE_Y`] or [`SOUTH_POLE_Y`] draws at the pole, and the tile's
//! grid and polygons get triangles out to it.
//!
//! Tiles of neighboring zoom levels split their shared edge at different
//! points. The engine's clipping masks reach a little past each tile to
//! cover the slivers between them, and a fill reaches them with
//! [`add_tile_border_strips`].

mod grid;
mod line;
mod polygon;

pub use self::grid::{TileGrid, masked_tile_grids, tile_grid};
pub(crate) use self::grid::{grid, surface_granularity};
pub use self::line::{subdivide_line, subdivide_ring};
pub use self::polygon::{SubdividedPolygon, add_tile_border_strips, subdivide_polygon};

/// The Y coordinate of a tile vertex that the globe's vertex shaders draw
/// at the north pole.
pub const NORTH_POLE_Y: i16 = -32768;
/// The Y coordinate of a tile vertex that the globe's vertex shaders draw
/// at the south pole.
pub const SOUTH_POLE_Y: i16 = 32767;
