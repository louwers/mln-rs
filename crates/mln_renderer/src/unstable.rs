//! Unstable: the engine internals that the layer types of `mln_layers` use,
//! which aren't part of the [layer type contract](crate::layer) or the
//! [draw services](crate::draw). Anything here can change or go in any
//! release.
//!
//! - The frame's shared atlases of dash and pattern images ([`Atlases`],
//!   [`LineAtlas`], [`DashAtlas`], [`PatternAtlas`]), which the engine keeps
//!   and uploads: a layer type can't yet declare state that the frame's
//!   layers share.
//! - The line mesh ([`add_line`]) and line subdivision
//!   ([`subdivide_vertex_line`]), which the engine's tile debug overlay
//!   draws its outlines with too.
//! - The globe's grids of a tile ([`globe_mesh_data`],
//!   [`raw_globe_tile_mesh`]), which the engine's clipping masks and globe
//!   depth draw with too.
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
pub use crate::globe_tile_mesh::{GlobeMeshData, RawGlobeTileMesh, globe_mesh_data, raw_globe_tile_mesh};
pub use crate::line_atlas::{DashAtlas, DashEntry, DashPatternTexture, LineAtlas, LinePatternCap, LinePatternPos};
pub use crate::pattern_atlas::PatternAtlas;
pub use crate::render_layer::Atlases;
pub use crate::util::subdivision::subdivide_vertex_line;
