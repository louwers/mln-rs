//! What the benchmarks run, for every program that runs them: `mln-bench`
//! (tools/bench) and the desktop example's benchmark mode.
//!
//! - [`manifest`]: the fixture snapshot that `tools/bench/fixtures.json`
//!   names, and where it's installed, once per user.
//! - [`Snapshot`]: the installed snapshot, as a map reads it: the Liberty
//!   style over one of its tile sets, whose `asset://` URLs a file source
//!   with the snapshot's directory as its asset path resolves.
//! - [`paths`]: the camera paths of the Frame scenarios.

pub mod manifest;
pub mod paths;
mod snapshot;

pub use snapshot::{EARTHQUAKES, Snapshot, Tiles};
