//! The registrations that a runtime's maps, their renderers and their tile
//! workers share: the expression functions and the layer types.
//!
//! An app builds its registry with a [`RegistryBuilder`] and gives it to
//! [`Runtime::new`](crate::Runtime::new). In the browser, each tile
//! worker's Web Worker makes the same calls for its own instance.

pub use mln_renderer::{Registry, RegistryBuilder};
pub use mln_style::expression::Functions;
pub use mln_style::expression::functions::RegisterError;
