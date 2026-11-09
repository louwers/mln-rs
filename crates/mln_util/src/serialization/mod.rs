//! Port-only: serde helpers for the types that cross between wasm instances
//! (mln_renderer's tile/worker_codec), which have no C++ counterpart.
//!
//! - `payload`: large byte arrays (pixels, vertices, indices) travel beside
//!   a message rather than in it, so that a host can transfer them.
//! - `shared`: a value behind an `Arc` that several messages refer to (a
//!   layer, an expression, an image) travels once per pair of sides and then
//!   by key.
//! - `sorted`: hash maps and sets in key order, so that a value always
//!   encodes to the same bytes.
//!
//! The first two only change how a value is written while a codec has a
//! session open for the message; otherwise the value is written in full, in
//! the message.

pub mod payload;
pub mod shared;
pub mod sorted;
