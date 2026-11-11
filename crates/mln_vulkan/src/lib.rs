//! Ports the C++ Vulkan backend (`src/mln/vulkan`, `include/mln/vulkan`,
//! `include/mln/shaders/vulkan`, `src/mln/shaders/vulkan`, the headless
//! backend from platform/default and the window backend from platform/glfw)
//! onto the `ash` bindings.
//!
//! The C++ backend calls Vulkan through vulkan-hpp, allocates with
//! VulkanMemoryAllocator and compiles its GLSL with glslang; this port makes
//! the same calls through ash, allocates with gpu-allocator and compiles
//! with naga. `vk::Unique*` handles become owners that destroy their object
//! when dropped, and the `Context&` members of resources become
//! `Rc<Context>` or `Weak<Context>`.
//!
//! The GLSL comes from the C++ headers, with the port's changes
//! (`shaders::glsl_sources`).
//!
//! The crate is empty on Apple platforms and in the browser build.

#![cfg(not(any(target_vendor = "apple", target_family = "wasm")))]
// Vulkan through ash is the FFI here.
#![allow(unsafe_code)]

pub mod buffer_resource;
pub mod command_encoder;
pub mod context;
pub mod descriptor_set;
pub mod drawable;
pub mod drawable_builder;
pub mod headless_backend;
pub mod layer_group;
pub mod offscreen_texture;
pub mod pipeline;
pub mod render_pass;
pub mod renderable_resource;
pub mod renderer_backend;
pub mod shaders;
pub mod surface_backend;
pub mod texture2d;
pub mod tile_layer_group;
pub mod uniform_buffer;
pub mod upload_pass;
pub mod vertex_buffer_resource;

pub use context::Context;
pub use headless_backend::HeadlessBackend;
pub use renderer_backend::RendererBackend;
pub use surface_backend::SurfaceBackend;
