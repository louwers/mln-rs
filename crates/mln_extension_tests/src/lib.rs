//! Layer types written against the `mln_rs` facade alone, as an author
//! outside the engine writes them: each is the smallest example of what it
//! shows, and the extension tests render with them.
//!
//! - [`dot`]: a layer of a geometry source that draws its points as discs.
//! - [`probe`]: a layer without a source that draws with the backend's own
//!   commands.

pub mod dot;
pub mod probe;
