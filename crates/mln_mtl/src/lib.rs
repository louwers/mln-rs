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

#[cfg(feature = "bench")]
#[doc(hidden)]
pub mod bench;
mod borrowed_target;
mod custom_layer;
mod device;
pub mod exported_target;
mod gpu;
mod readback;
mod renderer_backend;
mod shaders;
pub mod surface_target;

pub use borrowed_target::{BorrowedTarget, BorrowedTextures, InFlight, MetalRing};
pub use custom_layer::{AttachmentFormats, CustomLayerEncoder, CustomLayerPass};
pub use device::Device;
pub use exported_target::{ExportedTarget, MetalExporter};
pub use gpu::MetalGpu;
pub use objc2;
pub use objc2_metal;
pub use renderer_backend::RendererBackend;
pub use surface_target::{Layer, SurfaceTarget};

/// An `id<P>` that this side retains: a Metal object of the protocol `P`,
/// such as `Id<dyn MTLTexture>`.
pub(crate) type Id<P> = objc2::rc::Retained<objc2::runtime::ProtocolObject<P>>;

/// Frames drawn into textures of the library's and read back.
pub type OffscreenTarget = mln_gfx::offscreen_target::OffscreenTarget<MetalGpu>;

// MTLCreateSystemDefaultDevice returns nil in a process that doesn't link
// CoreGraphics, such as a command-line tool.
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {}
