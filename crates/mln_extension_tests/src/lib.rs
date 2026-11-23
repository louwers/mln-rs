//! Layer and source types written against the `mln_rs` facade alone, as an
//! author outside the engine writes them: each is the smallest example of
//! what it shows, and the extension tests render with them.
//!
//! - [`probe`]: a layer without a source that draws with the backend's own
//!   commands.
//! - [`stamp`]: a source type whose tiles each hold a value of its own type,
//!   which its origin makes, and two layer types that view that value.

pub mod probe;
pub mod stamp;
