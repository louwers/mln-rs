//! Render sessions: the renderers that draw maps into their targets.

mod driver;
mod frame;
mod local;
mod query;
mod session;
mod target;
mod worker;

pub use driver::DriverWork;
pub use frame::{DemandMode, FrameDemand, FrameDisposition, FrameResult};
pub use local::{BackendRef, LocalTarget};
use mln_style::error::{Error, ErrorKind};
use mln_style::size::Size;
pub use query::RenderedQueryGeometry;
pub(crate) use session::{Abandoner, SessionId, SessionShared, attach};
pub use session::{
    AcquiredFrame, FrameView, RenderSession, RenderSessionCapabilities, RenderSessionSnapshot, RenderSessionState,
};
pub use target::{
    Adapter, AttachOptions, BackendType, BorrowedRing, CanvasFormat, ConsumerSync, DepthStencilFormat,
    DepthStencilFormats, Device, DeviceOptions, DmaBufFormat, DmaBufPlane, Driver, ExportKind, ExportKinds,
    ExportedImage, ExportedRing, FrameDepth, FrameTexture, FrameTextureKind, GlApi, GpuSync, NativeTexture,
    RenderTarget, Surface, SurfaceOverlay, SurfaceType, SyncKind, SyncKinds, Wake,
};
pub(crate) use worker::{SessionQueue, Worker, post as worker_post};

/// A graphics backend's `error` as the runtime's.
pub(crate) fn gfx_error(error: mln_gfx::Error) -> Error {
    let kind = match error.kind {
        mln_gfx::ErrorKind::InvalidArgument => ErrorKind::InvalidArgument,
        mln_gfx::ErrorKind::InvalidState => ErrorKind::InvalidState,
        mln_gfx::ErrorKind::Unsupported => ErrorKind::Unsupported,
        mln_gfx::ErrorKind::Native => ErrorKind::Native,
        mln_gfx::ErrorKind::NotFound => ErrorKind::NotFound,
    };
    Error::new(kind, error.message)
}

/// `size` as the graphics backends take it.
pub(crate) fn gfx_size(size: Size) -> mln_gfx::Size {
    mln_gfx::Size::new(size.width, size.height)
}
