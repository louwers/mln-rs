//! The Vulkan backend: the `mln_gfx` traits implemented on the `ash`
//! bindings, with its shaders and its offscreen, window and borrowed render
//! targets.
//!
//! The C++ backend calls Vulkan through vulkan-hpp, allocates with
//! `VulkanMemoryAllocator` and compiles its GLSL with glslang; this port makes
//! the same calls through ash, allocates with gpu-allocator and compiles
//! with naga. `vk::Unique*` handles become owners that destroy their object
//! when dropped, and the `Context&` members of resources become
//! `Rc<Context>` or `Weak<Context>`.
//!
//! The GLSL comes from the C++ headers, with the port's changes
//! (`shaders::glsl_sources`).
//!
//! A `RendererBackend` renders on a `Device`, which backends on any thread
//! share, into an `OffscreenTarget`, a `SurfaceTarget` or a `BorrowedTarget`
//! built on it.
//!
//! The crate is empty on Apple platforms and in the browser build.

#![cfg(not(any(target_vendor = "apple", target_family = "wasm")))]
#![expect(unsafe_code, reason = "Vulkan through ash is the FFI here")]

#[cfg(target_os = "android")]
mod android_hardware_buffer;
mod borrowed_target;
mod buffer_resource;
mod command_encoder;
mod context;
mod custom_layer;
mod descriptor_set;
pub mod device;
mod draw_target;
mod drawable;
mod drawable_builder;
pub mod exported_target;
mod layer_group;
mod offscreen_target;
mod offscreen_texture;
mod pipeline;
mod render_pass;
mod renderer_backend;
mod shaders;
mod surface_resource;
mod surface_target;
mod texture2d;
mod tile_layer_group;
mod uniform_buffer;
mod upload_pass;
mod vertex_buffer_resource;
#[cfg(windows)]
mod win32;
mod window;

pub use ash;
pub use borrowed_target::{BorrowedDepth, BorrowedImage, BorrowedImages, BorrowedTarget, ImageOwner, PendingReads};
pub use context::Context;
pub use custom_layer::{CustomLayerEncoder, CustomLayerPass};
pub use device::{AdapterId, Device, HostDevice, QueueLease, QueueLock, RawDevice};
pub use exported_target::ExportedTarget;
pub use offscreen_target::OffscreenTarget;
pub use renderer_backend::RendererBackend;
pub use shaders::compiler::{Stage as ShaderStage, compile as compile_glsl};
pub use surface_target::{HostSurface, Overlay, OverlayTarget, SurfaceTarget};
pub use texture2d::Texture2D;

#[cfg(test)]
mod tests;
