//! The Metal backend: `MetalGpu`, the backend's `mln_gfx::gpu::Gpu`, on the
//! `objc2-metal` bindings, with its shaders and the targets the renderer
//! draws frames into.
//!
//! The C++ backend calls Metal through metal-cpp; this port makes the same
//! calls through objc2. `NS::SharedPtr`s become `Retained` objects, and the
//! `Context&` members of resources become `Rc<Context>`.
//!
//! The MSL comes from the C++ headers, with the port's changes
//! (`shaders::msl_sources`).
//!
//! A `RendererBackend` renders on a `Device`, into an `OffscreenTarget`
//! or a `SurfaceTarget` built on it.
//!
//! Metal exists on Apple platforms only; elsewhere this crate is empty.

#![cfg(target_vendor = "apple")]
#![expect(unsafe_code, reason = "Metal through objc2 is the FFI here")]

pub mod borrowed_target;
pub mod context;
pub mod custom_layer;
pub mod device;
pub mod exported_target;
pub mod gpu;
pub mod mtl_fwd;
mod readback;
pub mod renderer_backend;
pub mod shaders;
pub mod surface_target;

pub use borrowed_target::{BorrowedTarget, BorrowedTextures, PendingReads};
pub use custom_layer::{AttachmentFormats, CustomLayerEncoder, CustomLayerPass};
pub use device::Device;
pub use exported_target::ExportedTarget;
pub use gpu::MetalGpu;
pub use objc2;
pub use objc2_metal;
/// Frames drawn into textures of the library's and read back.
pub type OffscreenTarget = mln_gfx::offscreen_target::OffscreenTarget<RendererBackend>;
pub use renderer_backend::RendererBackend;
pub use surface_target::{Layer, SurfaceTarget};

// MTLCreateSystemDefaultDevice returns nil in a process that doesn't link
// CoreGraphics, such as a command-line tool.
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {}
