//! The Vulkan backend: [`VulkanGpu`], the backend's `mln_gfx::gpu::Gpu`, on
//! the `ash` bindings, with its shaders and the targets the renderer draws
//! frames into.
//!
//! It allocates memory with gpu-allocator and compiles `mln_glsl`'s GLSL to
//! SPIR-V with naga.
//!
//! A `RendererBackend` renders on a `Device`, which backends on any thread
//! share, into an `OffscreenTarget`, a `SurfaceTarget`, a `BorrowedTarget` or
//! an `ExportedTarget` built on it.
//!
//! The crate is empty on Apple platforms and in the browser build.

#![cfg_attr(
    any(target_vendor = "apple", target_family = "wasm"),
    expect(rustdoc::broken_intra_doc_links, reason = "the crate docs link to items that these targets compile out")
)]
#![cfg(not(any(target_vendor = "apple", target_family = "wasm")))]
#![expect(unsafe_code, reason = "Vulkan through ash is the FFI here")]

mod allocation;
#[cfg(target_os = "android")]
mod android_hardware_buffer;
#[cfg(feature = "bench")]
#[doc(hidden)]
pub mod bench;
mod borrowed_target;
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

/// The `log` targets of this crate's records (`mln_gfx`'s crate docs).
mod log_target {
    pub(crate) const RENDER: &str = "mln_vulkan::render";
}

pub use ash;
pub use borrowed_target::{
    BorrowedDepth, BorrowedImage, BorrowedImages, BorrowedTarget, ConsumerWait, ImageOwner, VulkanRing,
};
pub use custom_layer::{CustomLayerEncoder, CustomLayerPass};
pub use device::{AdapterId, Device, HostDevice, QueueLease, QueueLock, RawDevice};
pub use exported_target::{ExportedImages, ExportedTarget, VulkanExporter};
pub use gpu::{ImportedImage, VulkanGpu};
/// Frames drawn into textures of the library's and read back.
pub type OffscreenTarget = mln_gfx::offscreen_target::OffscreenTarget<VulkanGpu>;
pub use mln_glsl::Stage as ShaderStage;
pub use renderer_backend::RendererBackend;
pub use shaders::compile as compile_glsl;
pub use surface_target::{HostSurface, Overlay, OverlayTarget, SurfaceTarget};

#[cfg(test)]
mod tests;
