//! The code of a program in each shading language a backend compiles, which
//! a [`Program`](crate::shaders::program::Program) carries in its
//! [`Sources`](crate::shaders::program::Sources): [`wgsl`] for
//! `mln_webgpu`, [`msl`] for `mln_mtl`, [`glsl_es`] for `mln_gl`, and
//! [`glsl`] for the backends that compile through `mln_glsl`, `mln_vulkan`
//! and `mln_d3d12`. Each backend composes a program's code with a prelude
//! of its own, which declares the engine's blocks and the helpers the
//! shaders share.

pub mod glsl;
pub mod glsl_es;
pub mod msl;
pub mod wgsl;
