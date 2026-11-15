//! The D3D12 backend, so far its shaders: [`ShaderCompiler`] compiles
//! `mln_glsl`'s GLSL to DXIL, through naga's HLSL back end and DXC, which
//! loads from `dxcompiler.dll` at run time.
//!
//! The crate is empty except on Windows.

#![cfg(windows)]
#![expect(unsafe_code, reason = "DXC through the windows crate is the FFI here")]

mod shaders;

pub use shaders::{CompiledProgram, ShaderCompiler};

#[cfg(test)]
mod tests;
