//! The players of video sources, which the map opens and steers.

use std::sync::Arc;

use mln_storage::FileSource;

use crate::slot::FrameSink;

/// What a map asks of a player when a style adds a video source.
pub struct OpenRequest {
    /// The video's URLs, in the order the style lists them: the player plays
    /// the first one it can, as the style spec says.
    pub urls: Vec<String>,
    /// Where the player reads the video's bytes (through a
    /// [`ByteSource`](crate::ByteSource)), so that the map's resource
    /// options, transforms and cache apply to them. `None` for a map without
    /// one.
    pub file_source: Option<Arc<dyn FileSource>>,
    /// Where the player sends the frames, and any failure.
    pub frames: FrameSink,
}

impl std::fmt::Debug for OpenRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenRequest").field("urls", &self.urls).finish_non_exhaustive()
    }
}

/// Opens players: the platform's ([`platform_players`](crate::platform_players)),
/// or an app's or a test's own.
pub trait VideoPlayers: Send + Sync + std::fmt::Debug {
    /// A player of `request`'s video that starts playing at once, from the
    /// start, muted, and loops, as GL JS plays video sources. It sends
    /// frames to `request.frames` from any thread until it drops. When none
    /// of the URLs plays, it reports that through `request.frames`, or fails
    /// here.
    ///
    /// # Errors
    ///
    /// Why the player couldn't start, such as a platform without one.
    fn open(&self, request: OpenRequest) -> Result<Box<dyn VideoPlayer>, String>;
}

/// A playing video. Dropping it stops the video and lets go of it.
pub trait VideoPlayer {
    fn play(&mut self);
    fn pause(&mut self);
    /// Moves to `seconds` from the video's start, and sends the frame there,
    /// even while paused.
    fn seek(&mut self, seconds: f64);
}

/// The players of a platform without any: each video fails to open.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoPlayers;

impl VideoPlayers for NoPlayers {
    fn open(&self, _request: OpenRequest) -> Result<Box<dyn VideoPlayer>, String> {
        Err("this platform has no video player".to_owned())
    }
}
