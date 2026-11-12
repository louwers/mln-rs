//! Ports the C++ Vulkan backend (`src/mln/vulkan`, `include/mln/vulkan`,
//! `include/mln/shaders/vulkan`, `src/mln/shaders/vulkan`, the headless
//! backend from platform/default and the window backend from platform/glfw)
//! onto the `ash` bindings.
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
// Vulkan through ash is the FFI here.
#![allow(unsafe_code)]

pub mod borrowed_target;
pub mod buffer_resource;
pub mod command_encoder;
pub mod context;
pub mod descriptor_set;
pub mod device;
pub mod draw_target;
pub mod drawable;
pub mod drawable_builder;
pub mod exported_target;
pub mod layer_group;
pub mod offscreen_target;
pub mod offscreen_texture;
pub mod pipeline;
pub mod render_pass;
pub mod renderer_backend;
pub mod shaders;
pub mod surface_resource;
pub mod surface_target;
pub mod texture2d;
pub mod tile_layer_group;
pub mod uniform_buffer;
pub mod upload_pass;
pub mod vertex_buffer_resource;
#[cfg(windows)]
mod win32;
mod window;

pub use ash;
pub use borrowed_target::{BorrowedImage, BorrowedImages, BorrowedTarget, ImageOwner, PendingReads};
pub use context::Context;
pub use device::{AdapterId, Device, Features, HostDevice, QueueLock, RawDevice};
pub use exported_target::ExportedTarget;
pub use offscreen_target::OffscreenTarget;
pub use renderer_backend::RendererBackend;
pub use surface_target::{HostSurface, SurfaceTarget};
