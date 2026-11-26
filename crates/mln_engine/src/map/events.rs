//! What a map reports to its owner: camera changes, loading, frames, and
//! the activity of its style, glyphs and tiles. The map queues the events,
//! and its owner drains them with [`Map::drain_events`](crate::map::Map::drain_events).

use std::sync::Arc;

use mln_style::StyleLoadError;
use mln_style::error::Error;
use mln_style::sprite::Sprite;

use crate::{FontStack, GlyphRange, OverscaledTileID, RenderMode, RenderingStats, TileOperation};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum CameraChangeMode {
    #[default]
    Immediate,
    Animated,
}

/// What moved the camera, as the C API's
/// `mln_camera_change_source` reports it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum CameraChangeSource {
    /// A camera command outside a gesture, or the map itself, as when a
    /// resize keeps the camera within its bounds.
    #[default]
    Command,
    /// A camera command with a [`GesturePhase`](crate::camera::GesturePhase) other
    /// than `None`.
    Gesture,
    /// The momentum, and the snaps, that a gesture's end started.
    Momentum,
}

/// A camera change: whether it jumps or animates, and what made it.
///
/// [`MapEvent::CameraWillChange`] reports the command that starts. Each
/// [`MapEvent::CameraDidChange`] ends one command and reports that command's
/// source, so a frame that ends a command started by a gesture reports
/// `Gesture` even while a momentum keeps moving the camera.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct CameraChange {
    pub mode: CameraChangeMode,
    pub source: CameraChangeSource,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderFrameStatus {
    pub mode: RenderMode,
    /// In continuous mode, the map changes after this frame without new
    /// commands: a transition runs, or the symbols wait for a placement
    /// ([`Map::repaint_at`](crate::map::Map::repaint_at)).
    pub needs_repaint: bool,
    pub placement_changed: bool,
    /// The render context's counters after this frame: per-frame ones such
    /// as `num_draw_calls` count this frame, running ones such as
    /// `total_draw_calls` and `num_frames` count every frame so far.
    pub rendering_stats: Arc<RenderingStats>,
}

/// An event of a map. The variants match the C API's
/// `mln_runtime_event_type` values for maps.
///
/// The render events come in continuous mode only. A static map reports
/// its still images through the callback of
/// [`Map::render_still`](crate::map::Map::render_still).
#[derive(Clone, Debug)]
pub enum MapEvent {
    /// A camera command starts.
    CameraWillChange(CameraChange),
    /// An animated camera command moved the camera.
    CameraIsChanging,
    /// A camera command ended.
    CameraDidChange(CameraChange),

    /// The style loaded and the map applied its default camera, unless a
    /// camera command came first.
    StyleLoaded,
    /// The map started loading a new style.
    LoadingStarted,
    /// The map rendered everything that the style it started loading needs.
    LoadingFinished,
    /// The style failed to load.
    LoadingFailed(StyleLoadError),
    /// A complete frame of the map's last update drew: everything has
    /// loaded, no transition runs, and no frame follows until something
    /// changes the map.
    Idle,
    /// A frame failed on the GPU, and lacks what failed. The still image it
    /// was drawn for fails with the error. A continuous map reports a run of
    /// failed frames once, and soon draws again, in case the failure passes.
    RenderError(Error),
    RenderFrameStarted,
    RenderFrameFinished(RenderFrameStatus),
    /// The renderer is about to draw the first frame of the map.
    RenderMapStarted,
    /// The renderer drew a frame with everything loaded, the first since the
    /// map started loading its style.
    RenderMapFinished(RenderMode),
    /// A layer uses an image that the style lacks. Adding it to the style
    /// lays out the layer again.
    StyleImageMissing {
        id: String,
    },
    /// A tile of the source `source_id` went through `op`.
    TileAction {
        op: TileOperation,
        id: OverscaledTileID,
        source_id: String,
    },
    /// A style source changed its description, as when its TileJSON
    /// arrives or its GeoJSON data is replaced. The map's style already
    /// holds the change.
    SourceChanged {
        source_id: String,
    },
    GlyphsRequested {
        font_stack: FontStack,
        range: GlyphRange,
    },
    GlyphsLoaded {
        font_stack: FontStack,
        range: GlyphRange,
    },
    /// A glyph range of a font stack failed to load or parse.
    GlyphsError {
        font_stack: FontStack,
        range: GlyphRange,
        error: Error,
    },
    /// The style requested a sprite. `None` stands for a style without one.
    SpriteRequested(Option<Sprite>),
    /// A sprite's image and JSON loaded and parsed. `None` stands for a
    /// style without a sprite.
    SpriteLoaded(Option<Sprite>),
    /// A sprite's image or JSON failed to load, or the sprite failed to
    /// parse. `None` stands for a sprite URL that no file source serves.
    SpriteError(Option<Sprite>, Error),
}
