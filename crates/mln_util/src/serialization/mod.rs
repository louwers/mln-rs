//! Port-only: serde helpers for the types that cross between wasm instances
//! (mln_renderer's tile/worker_codec), which have no C++ counterpart.
//!
//! - `shared`: a value behind an `Arc` that several messages refer to (a
//!   layer, an expression, an image) travels once per pair of sides and then
//!   by key.
//!
//! It only changes how a value is written while a codec has a session open
//! for the message; otherwise the value is written in full, in the message.

pub mod shared;
