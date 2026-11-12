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
pub use local::LocalTarget;
pub use query::RenderedQueryGeometry;
pub(crate) use session::{Abandoner, SessionId, SessionShared, attach};
pub use session::{
    AcquiredFrame, FrameView, RenderSession, RenderSessionCapabilities, RenderSessionSnapshot, RenderSessionState,
};
pub use target::{
    Adapter, AttachOptions, BackendType, BorrowedRing, ConsumerSync, DepthStencilFormat, DepthStencilFormats, Device,
    DeviceOptions, DmaBufFormat, DmaBufPlane, Driver, ExportKind, ExportKinds, ExportedImage, ExportedRing, FrameDepth,
    FrameTexture, FrameTextureKind, GlApi, GpuSync, NativeTexture, RenderTarget, Surface, SurfaceType, SyncKind,
    SyncKinds, Wake,
};
pub(crate) use worker::{Worker, post as worker_post};
