//! Unstable: the engine internals that the layer types of `mln_layers` use,
//! which aren't part of the [layer type contract](crate::layer) or the
//! [draw services](crate::draw). Anything here can change or go in any
//! release.
//!
//! - The frame's shared atlases ([`Atlases`], [`LineAtlas`],
//!   [`DashAtlas`], [`PatternAtlas`]), which line, fill and background add
//!   to: a layer type can't yet declare state that the frame's layers share.
//! - The line mesh ([`add_line`]), which the engine's tile debug overlay
//!   builds its lines with too.
//! - The quad and index builders ([`Indexed`], [`add_point_quads`]), which
//!   the engine's raster tiles share with the layer types' buckets.
//! - Subdivision and triangulation ([`subdivide_polygon`],
//!   [`Tessellator`]), geometry builders that the engine's tile debug
//!   overlay and tests use too.
//! - The globe's tile meshes ([`GlobeTileMeshCache`]), which the engine's
//!   clipping masks and globe depth draw with too.
//!
//! The engine types that the contract's hooks name are public too, for the
//! contract to be, but outside the engine they're unnameable or
//! `#[doc(hidden)]`.
//!
//! The [placement](crate::placement) capability is unstable too.
//!
//! Two more hidden modules serve tools, each behind a feature of its own:
//! `testing` (`testing`), drivers for the tests of a crate
//! that adds source types, and `bench` (`bench`), the benchmarks' entry
//! points.

pub use crate::gfx::line_mesh::{LineLayoutVertex, LineOptions, LineScratch, add_line};
pub use crate::gfx::mesh::{Indexed, add_point_quads};
pub use crate::globe_tile_mesh::{GlobeTileMesh, GlobeTileMeshCache, raw_globe_tile_mesh};
pub use crate::line_atlas::{DashAtlas, DashEntry, DashPatternTexture, LineAtlas, LinePatternCap, LinePatternPos};
pub use crate::pattern_atlas::PatternAtlas;
pub use crate::render_layer::Atlases;
pub use crate::util::subdivision::{
    SubdividedPolygon, TILE_BORDER_STRIP_WIDTH, add_tile_border_strips, subdivide_polygon, subdivide_vertex_line,
};
pub use crate::util::tessellator::Tessellator;
