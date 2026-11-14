//! The Vulkan backend: [`VulkanGpu`], the backend's `mln_gfx::gpu::Gpu`, on
//! the `ash` bindings, with its shaders and the targets the renderer draws
//! frames into.
//!
//! It allocates memory with gpu-allocator and compiles its GLSL
//! (`shaders::glsl_sources`) to SPIR-V with naga.
//!
//! A `RendererBackend` renders on a `Device`, which backends on any thread
//! share, into an `OffscreenTarget`, a `SurfaceTarget`, a `BorrowedTarget` or
//! an `ExportedTarget` built on it.
//!
//! The crate is empty on Apple platforms and in the browser build.

#![cfg(not(any(target_vendor = "apple", target_family = "wasm")))]
#![expect(unsafe_code, reason = "Vulkan through ash is the FFI here")]

mod allocation;
#[cfg(target_os = "android")]
mod android_hardware_buffer;
mod borrowed_target;
mod context;
mod custom_layer;
mod descriptor_set;
pub mod device;
pub mod exported_target;
mod gpu;
mod renderer_backend;
mod shaders;
mod surface_resource;
mod surface_target;
mod vk_state;
#[cfg(windows)]
mod win32;
mod window;

pub use ash;
pub use borrowed_target::{BorrowedDepth, BorrowedImage, BorrowedImages, BorrowedTarget, ImageOwner, PendingReads};
pub use custom_layer::{CustomLayerEncoder, CustomLayerPass};
pub use device::{AdapterId, Device, HostDevice, QueueLease, QueueLock, RawDevice};
pub use exported_target::ExportedTarget;
pub use gpu::{ImportedImage, VulkanGpu};
/// Frames drawn into textures of the library's and read back.
pub type OffscreenTarget = mln_gfx::offscreen_target::OffscreenTarget<RendererBackend>;
pub use renderer_backend::RendererBackend;
pub use shaders::compiler::{Stage as ShaderStage, compile as compile_glsl};
pub use surface_target::{HostSurface, Overlay, OverlayTarget, SurfaceTarget};

#[cfg(test)]
mod tests;
