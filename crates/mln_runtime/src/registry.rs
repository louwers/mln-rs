//! The registrations that a runtime's maps, their renderers and their tile
//! workers share: the expression functions, the layer types, the source
//! types and the projections.
//!
//! An app that registers more than the built-ins builds its registry with a
//! [`RegistryBuilder`], [`register_builtins`] first, and gives it to
//! [`Runtime::with_registry`](crate::Runtime::with_registry). In the browser,
//! each tile worker's Web Worker makes the same calls for its own instance.

pub use mln_builtins::register_builtins;
pub use mln_engine::{Registry, RegistryBuilder};
pub use mln_style::crs::{Crs, CrsConverter, TileMatrixSet};
pub use mln_style::expression::Functions;
pub use mln_style::expression::functions::RegisterError;
