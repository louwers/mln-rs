//! Render sessions: the renderers that draw maps into their targets.

mod frame;
mod query;
mod session;
mod target;
mod worker;

pub use frame::{DemandMode, FrameDemand, FrameDisposition, FrameResult, STILL_IMAGE_TOKEN};
pub use query::RenderedQueryGeometry;
pub use session::{RenderSession, RenderSessionCapabilities, RenderSessionSnapshot, RenderSessionState};
pub(crate) use session::{SessionId, attach};
pub use target::{AttachOptions, Driver, OffscreenDevice, OffscreenDeviceOptions, RenderTarget, Wake};
pub(crate) use worker::{Worker, post as worker_post};
