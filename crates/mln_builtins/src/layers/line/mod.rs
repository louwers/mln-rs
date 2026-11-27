//! The lines' programs, the blocks they read, and their text in each
//! language.

pub(crate) mod bucket;
pub(crate) mod dashes;
pub(crate) mod layer;
pub(crate) mod mesh;
#[rustfmt::skip]
mod glsl;
#[rustfmt::skip]
mod glsl_es;
#[rustfmt::skip]
mod msl;
pub mod programs;
pub mod ubo;
mod wgsl;
