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
#[cfg(all(unix, not(target_vendor = "apple"), not(target_os = "android"), not(target_family = "wasm")))]
mod gstreamer;
mod player;
mod slot;

use std::sync::Arc;

pub use bytes::ByteSource;
pub use mln_gfx::video::{
    CpuPlane, FramePixels, GpuFrame, ImportedFrame, PlaneLayout, VideoFrame, YuvColor, YuvMatrix, YuvRange,
};
pub use player::{NoPlayers, OpenRequest, VideoPlayer, VideoPlayers};
pub use slot::{FrameSink, FrameSlot, Newest};

/// The players of the platform the library runs on: GStreamer's on Linux and
/// the BSDs, which fail to open each video, saying so, where GStreamer 1.x
/// isn't installed. A platform without players fails to open each video
/// too.
#[must_use]
pub fn platform_players() -> Arc<dyn VideoPlayers> {
    #[cfg(all(unix, not(target_vendor = "apple"), not(target_os = "android"), not(target_family = "wasm")))]
    return Arc::new(gstreamer::GstreamerPlayers);
    #[cfg(not(all(unix, not(target_vendor = "apple"), not(target_os = "android"), not(target_family = "wasm"))))]
    Arc::new(NoPlayers)
}
