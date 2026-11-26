//! What a renderer reports to the map it renders: frames, resource errors,
//! missing images, glyph and tile activity. The renderer pushes the events
//! into the map's [`RendererEvents`] queue, and the map handles them when it
//! drains the queue.

use std::sync::Arc;

pub use mln_gfx::rendering_stats::RenderingStats;
use mln_style::error::Error;
use mln_style::time::Instant;
pub use mln_text::glyph::GlyphRange;
use mln_util::event_queue::EventQueue;
pub use mln_util::font_stack::FontStack;

use crate::camera::Elevation;
use crate::tile::OverscaledTileID;
pub use crate::tile_store::TileOperation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum RenderMode {
    Partial,
    Full,
}

/// An event of a renderer.
#[derive(Clone, Debug)]
pub enum RendererEvent {
    /// The renderer needs a new frame, as when a tile arrived.
    Invalidate,
    /// A resource failed to load or parse.
    ResourceError(Error),
    /// The renderer is about to draw its first frame.
    RenderMapStarted,
    /// The renderer is about to draw a frame.
    RenderFrameStarted,
    /// The renderer drew a frame.
    RenderFrameFinished {
        /// `Full` when everything the frame shows has loaded and its camera
        /// stood as it would on the terrain's current ground
        /// ([`UpdateParameters::elevation`]), though some of it may have
        /// failed to draw (`error`).
        ///
        /// [`UpdateParameters::elevation`]: crate::UpdateParameters::elevation
        mode: RenderMode,
        /// The frame has transitions still running.
        needs_repaint: bool,
        /// When, without `needs_repaint`, the map changes next though
        /// nothing asks for a frame before: a frame then places the symbols
        /// again.
        repaint_at: Option<Instant>,
        placement_changed: bool,
        /// The still image request of the update that the frame drew, or 0
        /// when the update requested none. Only a frame of the request's own
        /// updates completes it, not one of an earlier update that a renderer
        /// on another thread drew late.
        still_image_request_id: u64,
        /// The [`update_id`] of the update that the frame drew.
        ///
        /// [`update_id`]: crate::UpdateParameters::update_id
        update_id: u64,
        /// The context's counters after the frame.
        stats: Arc<RenderingStats>,
        /// Why the frame failed on the GPU, if it did
        /// ([`DrawnFrame::error`](crate::DrawnFrame::error)): its image
        /// lacks what failed. Boxed, so that the events don't grow for it.
        error: Option<Box<Error>>,
    },
    /// The ground of the terrain changed: the tiles that draw it, or the DEM
    /// data they read; or it comes again for an update that carried a
    /// ground that differs from it at the view's center. `None` once terrain
    /// stops drawing, and for an update that carries a ground while terrain
    /// doesn't draw. It comes before the frame of its update finishes. The
    /// map's camera stands on it, and the map answers it with an update that
    /// carries it ([`UpdateParameters::elevation`]), which a still image
    /// waits for.
    ///
    /// [`UpdateParameters::elevation`]: crate::UpdateParameters::elevation
    TerrainElevation(Option<Arc<dyn Elevation>>),
    /// The renderer drew its first frame with everything loaded.
    RenderMapFinished,
    /// A layer uses an image that the style lacks. The renderer lays out
    /// without it until an update adds it.
    StyleImageMissing(String),
    /// These images were requested before, and no layer uses them anymore.
    UnusedStyleImages(Vec<String>),
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
    /// A tile of the source `source_id` went through `op`: a request, a
    /// response, the start or end of its parse, an error or a cancel.
    TileAction {
        op: TileOperation,
        id: OverscaledTileID,
        source_id: String,
    },
}

/// The queue a renderer pushes its events to.
pub type RendererEvents = EventQueue<RendererEvent>;

// A renderer on another thread than its map sends it the events.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<RendererEvent>();
};
