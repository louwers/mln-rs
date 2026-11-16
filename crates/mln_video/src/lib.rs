//! The players of a style's `video` sources.
//!
//! The map never decodes video. When a style adds a video source, the map
//! opens a [`VideoPlayer`] of its [`VideoPlayers`], which reads the video's
//! bytes through the map's storage ([`ByteSource`]) and sends each frame it
//! decodes to the source's [`FrameSlot`], in memory or on the GPU. The
//! renderer draws the newest frame of each slot.
//!
//! [`platform_players`] are the platform's players. An app or a test can
//! bring its own.

mod bytes;
mod player;
mod slot;

use std::sync::Arc;

pub use bytes::ByteSource;
pub use mln_gfx::video::{
    CpuPlane, FramePixels, GpuFrame, ImportedFrame, PlaneLayout, VideoFrame, YuvColor, YuvMatrix, YuvRange,
};
pub use player::{NoPlayers, OpenRequest, VideoPlayer, VideoPlayers};
pub use slot::{FrameSink, FrameSlot, Newest};

/// The players of the platform the library runs on. A platform without one
/// fails to open each video, saying so.
#[must_use]
pub fn platform_players() -> Arc<dyn VideoPlayers> {
    Arc::new(NoPlayers)
}
