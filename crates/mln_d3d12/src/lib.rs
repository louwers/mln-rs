//! The D3D12 backend: `D3d12Gpu`, the backend's `mln_gfx::gpu::Gpu`, on the
//! `windows` crate's bindings, with its shaders and offscreen frames.
//!
//! Its shaders are `mln_glsl`'s GLSL, which naga writes as HLSL and DXC,
//! loaded from `dxcompiler.dll` at run time, compiles to DXIL
//! (`ShaderCompiler`).
//!
//! A `RendererBackend` renders on a `Device`, which backends on any thread
//! share, into an `OffscreenTarget` or a `SurfaceTarget` built on it.
//!
//! The crate is empty except on Windows.

#![cfg(windows)]
#![expect(unsafe_code, reason = "D3D12 and DXC through the windows crate are the FFI here")]

mod conv;
mod custom_layer;
mod device;
mod gpu;
mod heaps;
mod memory;
mod renderer_backend;
mod root_signature;
mod shaders;
mod surface_target;
mod upload;

pub use custom_layer::{CustomLayerEncoder, CustomLayerPass};
pub use device::Device;
pub use gpu::D3d12Gpu;
/// Frames drawn into textures of the library's and read back.
pub type OffscreenTarget = mln_gfx::offscreen_target::OffscreenTarget<RendererBackend>;
pub use mln_glsl::Stage as ShaderStage;
pub use renderer_backend::RendererBackend;
pub use shaders::{CompiledProgram, ShaderCompiler};
pub use surface_target::{Composition, Overlay, OverlayTarget, SurfaceTarget};
pub use windows;

#[cfg(test)]
mod tests;
