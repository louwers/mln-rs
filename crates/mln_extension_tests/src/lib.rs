//! Layer types written against the `mln_rs` facade alone, as an author
//! outside the engine writes them: each is the smallest example of what it
//! shows, and the extension tests render with them.
//!
//! - [`probe`]: a layer without a source that draws with the backend's own
//!   commands.

pub mod probe;
