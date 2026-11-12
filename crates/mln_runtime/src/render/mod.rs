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
pub use session::{RenderSession, RenderSessionCapabilities, RenderSessionSnapshot, RenderSessionState};
pub use target::{
    Adapter, AttachOptions, BackendType, Device, DeviceOptions, Driver, RenderTarget, Surface, SurfaceType, Wake,
};
pub(crate) use worker::{Worker, post as worker_post};
