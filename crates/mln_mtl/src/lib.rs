//! The Metal backend: `MetalGpu`, the backend's `mln_gfx::gpu::Gpu`, on the
//! `objc2-metal` bindings, with its MSL shaders and the targets the renderer
//! draws frames into.
//!
//! A `RendererBackend` renders on a `Device`, into an `OffscreenTarget`, a
//! `SurfaceTarget`, a `BorrowedTarget` or an `ExportedTarget` built on it.
//!
//! Metal exists on Apple platforms only; elsewhere this crate is empty.

#![cfg(target_vendor = "apple")]
#![expect(unsafe_code, reason = "Metal through objc2 is the FFI here")]

mod borrowed_target;
mod custom_layer;
mod device;
pub mod exported_target;
mod gpu;
mod mtl_fwd;
mod readback;
mod renderer_backend;
mod shaders;
pub mod surface_target;

pub use borrowed_target::{BorrowedTarget, BorrowedTextures, PendingReads};
pub use custom_layer::{AttachmentFormats, CustomLayerEncoder, CustomLayerPass};
pub use device::Device;
pub use exported_target::ExportedTarget;
pub use gpu::MetalGpu;
pub use objc2;
pub use objc2_metal;
pub use renderer_backend::RendererBackend;
pub use surface_target::{Layer, SurfaceTarget};

/// Frames drawn into textures of the library's and read back.
pub type OffscreenTarget = mln_gfx::offscreen_target::OffscreenTarget<RendererBackend>;

// MTLCreateSystemDefaultDevice returns nil in a process that doesn't link
// CoreGraphics, such as a command-line tool.
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {}
