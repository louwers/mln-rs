//! Ports `vendor/wagyu` (mapbox::geometry::wagyu 0.5.0, commit 9c87e553),
//! the polygon clipper the C++ core uses in `fixupPolygons`
//! (tile/geometry_tile_data.cpp), for the one instantiation the core uses,
//! `wagyu<int32_t>`. One module per header:
//!
//! - `config.hpp` → [`config`] ([`ClipType`], [`PolygonType`], [`FillType`])
//! - `almost_equal.hpp`, `util.hpp`, `point.hpp`, `edge.hpp`, `bound.hpp`,
//!   `ring.hpp`, `local_minimum.hpp`, `scanbeam.hpp`, `build_edges.hpp`,
//!   `local_minimum_util.hpp` (+ `build_local_minima_list.hpp`),
//!   `active_bound_list.hpp`, `ring_util.hpp`, `intersect_util.hpp`
//!   (+ `intersect.hpp`, `bubble_sort.hpp`), `process_horizontal.hpp`,
//!   `process_maxima.hpp`, `snap_rounding.hpp`, `vatti.hpp`,
//!   `topology_correction.hpp`, `build_result.hpp`, `wagyu.hpp`.
//!
//! The pointer-linked C++ structures (points, rings, bounds) live in arenas
//! and are referenced by index. `std::stable_sort` is Rust's stable sort,
//! except for the ULP-tolerant intersection order, which is not a strict
//! weak ordering. The `std::unordered_multimap` of the topology correction
//! keeps libstdc++'s order of equal keys. Exceptions (`std::runtime_error`)
//! are `Err(message)`. `interrupt.hpp` (USE_WAGYU_INTERRUPT) and
//! `quick_clip.hpp` are not used by the core and not ported.

mod active_bound_list;
mod almost_equal;
mod bound;
mod build_edges;
mod build_result;
pub mod config;
mod edge;
mod intersect_util;
mod local_minimum;
mod local_minimum_util;
mod point;
mod process_horizontal;
mod process_maxima;
mod ring;
mod ring_util;
mod scanbeam;
mod snap_rounding;
mod topology_correction;
mod util;
mod vatti;
mod wagyu;

pub use build_result::OutputCoordinate;
pub use config::{ClipType, FillType, PolygonType};
pub use wagyu::Wagyu;

#[cfg(test)]
mod wagyu_test;
