//! The players of a style's `video` sources.
//!
//! The map never decodes video. When a style adds a video source, the map
//! opens a [`VideoPlayer`] of its [`VideoPlayers`], which reads the video's
//! bytes through the map's storage ([`ByteSource`]) and sends each frame it
//! decodes to the source's [`FrameSlot`], in memory or on the GPU. The
//! renderer draws the newest frame of each slot.
//!
//! [`platform_players`] are the platform's players. An app or a test can
//! bring its own. In the browser, the host brings them, as `WebPlayers`.

#[cfg(target_os = "android")]
mod android;
#[cfg(target_vendor = "apple")]
mod apple;
mod bytes;
#[cfg(all(
    unix,
    not(target_vendor = "apple"),
    not(target_os = "android"),
    not(target_env = "ohos"),
    not(target_family = "wasm")
))]
mod gstreamer;
#[cfg(target_env = "ohos")]
mod ohos;
mod player;
mod slot;
#[cfg(any(target_family = "wasm", test))]
mod urls;
#[cfg(target_family = "wasm")]
mod web;
#[cfg(windows)]
mod windows;

use std::sync::Arc;

pub use bytes::ByteSource;
pub use mln_gfx::video::{
    CpuPlane, FramePixels, GpuFrame, ImportedFrame, PlaneLayout, VideoFrame, YuvColor, YuvMatrix, YuvRange,
};
pub use player::{NoPlayers, OpenRequest, VideoPlayer, VideoPlayers};
pub use slot::{FrameSink, FrameSlot, Newest};
#[cfg(target_family = "wasm")]
pub use web::{VideoHost, WebPlayers};

/// The players of the platform the library runs on: GStreamer's on Linux and
/// the BSDs, which fail to open each video, saying so, where GStreamer 1.x
/// isn't installed, the NDK's media API on Android, `OpenHarmony`'s `AVCodec`
/// on `OpenHarmony`, Media Foundation on Windows, and `AVFoundation` on
/// Apple's platforms. A platform without players
/// fails to open each video too.
///
/// `AVFoundation` steps its players on the main thread: in a process whose
/// main thread doesn't run its run loop, such as a command-line tool, each
/// video fails after a few seconds.
#[cfg(not(target_family = "wasm"))]
#[must_use]
pub fn platform_players() -> Arc<dyn VideoPlayers> {
    #[cfg(all(unix, not(target_vendor = "apple"), not(target_os = "android"), not(target_env = "ohos")))]
    let players: Arc<dyn VideoPlayers> = Arc::new(gstreamer::GstreamerPlayers);
    #[cfg(target_os = "android")]
    let players: Arc<dyn VideoPlayers> = Arc::new(android::AndroidPlayers);
    #[cfg(target_env = "ohos")]
    let players: Arc<dyn VideoPlayers> = Arc::new(ohos::OhosPlayers);
    #[cfg(windows)]
    let players: Arc<dyn VideoPlayers> = Arc::new(windows::MediaFoundationPlayers);
    #[cfg(target_vendor = "apple")]
    let players: Arc<dyn VideoPlayers> = Arc::new(apple::AvFoundationPlayers);
    #[cfg(not(any(unix, windows)))]
    let players: Arc<dyn VideoPlayers> = Arc::new(NoPlayers);
    players
}

/// The players of the platform the library runs on. In the browser, where
/// the map may run in a Web Worker without `<video>` elements, the host
/// supplies them ([`WebPlayers`]), so these fail to open each video, saying
/// so.
#[cfg(target_family = "wasm")]
#[must_use]
pub fn platform_players() -> Arc<dyn VideoPlayers> {
    web::platform_players()
}
