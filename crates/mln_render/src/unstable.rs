//! Unstable: the engine internals that the layer types of `mln_layers` use,
//! which aren't part of the [layer type contract](crate::layer) or the
//! [draw services](crate::draw), here and in the hidden
//! [`placement`](crate::placement) module. Anything in either can change or
//! go in any release.
//!
//! - Line subdivision ([`subdivide_vertex_line`]), which the line, fill
//!   and fill-extrusion layer types split their geometry with on the globe.
//! - The globe's grids of a tile ([`globe_mesh_data`],
//!   [`raw_globe_tile_mesh`]), which the engine's clipping masks and globe
//!   depth draw with too.
//!
//! The worker codec is unstable: a layer type's bucket and its style half's
//! property sets implement `mln_wire::Wire`, whose encoding can change in
//! any release.
//!
//! The [placement](crate::placement) capability is unstable too, with the
//! hidden items of the contract that serve it: `LayerType::PLACEMENT`,
//! `UpdateContext::bucket_key` and `Renderer::placement`/`placement_mut`.
//!
//! The context's store handles name key types of the engine's, which are
//! public for the handles to be, but unnameable outside it.
//!
//! Two more hidden modules serve tools, each behind a feature of its own:
//! `testing` (`testing`), what the tests of a crate that adds layer or
//! source types drive the engine's parts with, and `bench` (`bench`), the
//! benchmarks' entry points.

pub use crate::globe_tile_mesh::{GlobeMeshData, RawGlobeTileMesh, globe_mesh_data, raw_globe_tile_mesh};
pub use crate::util::subdivision::subdivide_vertex_line;
