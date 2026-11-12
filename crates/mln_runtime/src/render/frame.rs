//! Frame demands and the results that they finish with.

use mln_map::{MapView, RenderMode};

/// When a demand draws a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DemandMode {
    /// Draws a frame when the map changed since the last one, and otherwise
    /// finishes with [`FrameDisposition::NoUpdate`].
    #[default]
    IfChanged,
    /// Draws a frame even when nothing changed.
    Force,
    /// Draws a frame when the map changed since the last one, and otherwise
    /// waits for the map's next update.
    WaitForUpdate,
}

/// A demand for one frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FrameDemand {
    pub mode: DemandMode,
    /// A value of the host's choice that the demand's result carries.
    pub token: u64,
}

impl FrameDemand {
    #[must_use]
    pub fn new(mode: DemandMode, token: u64) -> Self {
        FrameDemand { mode, token }
    }
}

/// How a frame demand finished.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FrameDisposition {
    Rendered,
    /// The map had nothing new to draw, or a barrier ended the demand.
    NoUpdate,
    /// The session couldn't take the frame, as after a detach or an abandon.
    TargetNotReady,
    /// A later demand with the same mode replaced this one.
    Superseded,
    /// The session failed. Its snapshot says why.
    Failed,
}

/// How one frame demand finished.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameResult {
    pub disposition: FrameDisposition,
    /// The token of the demand that the result finishes, or `None` for a
    /// frame that the session rendered for
    /// [`Map::request_still_image`](crate::Map::request_still_image).
    pub token: Option<u64>,
    /// The map update that a rendered frame shows, or the latest one for a
    /// demand that rendered nothing. The frame includes a command's change
    /// when this is at least the command's
    /// [`render_update_generation`](crate::Applied::render_update_generation).
    pub map_update_generation: u64,
    /// The session's generation when it rendered the frame: it grows when the
    /// session resizes. Zero for a demand that rendered nothing.
    pub generation: u64,
    /// Counts the session's rendered frames, this one included. Zero for a
    /// demand that rendered nothing.
    pub frame_generation: u64,
    /// Whether the map has transitions still running after this frame.
    pub needs_repaint: bool,
    /// The camera and extent that a rendered frame shows.
    pub view: Option<MapView>,
    /// Whether everything that a rendered frame shows had loaded.
    pub mode: RenderMode,
}

impl FrameResult {
    /// The result of a demand that rendered nothing.
    pub(crate) fn unrendered(token: Option<u64>, disposition: FrameDisposition, map_update_generation: u64) -> Self {
        FrameResult {
            disposition,
            token,
            map_update_generation,
            generation: 0,
            frame_generation: 0,
            needs_repaint: false,
            view: None,
            mode: RenderMode::Partial,
        }
    }
}
