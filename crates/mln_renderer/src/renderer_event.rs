//! What a renderer reports to the map it renders: frames, resource errors,
//! missing images, glyph and tile activity. The renderer pushes the events
//! into the map's [`RendererEvents`] queue, and the map handles them when it
//! drains the queue.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

pub use mln_gfx::rendering_stats::{
    FeatureInfo, FrameRenderedFeaturesMap, LayerFeaturesMap, NDCBound, RenderingStats, SourceLayerID,
};
pub use mln_text::glyph::GlyphRange;
pub use mln_tile_id::OverscaledTileID;
use mln_util::error::Error;
use mln_util::event_queue::EventQueue;
pub use mln_util::font_stack::FontStack;

pub use crate::tile::TileOperation;

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
        /// `Full` when everything the frame shows has loaded.
        mode: RenderMode,
        /// The frame has transitions still running.
        needs_repaint: bool,
        placement_changed: bool,
        /// The still image request of the update that the frame drew, or 0
        /// when the update requested none. Only a frame of the request's own
        /// updates completes it, not one of an earlier update that a renderer
        /// on another thread drew late.
        still_image_request_id: u64,
        /// The context's counters after the frame.
        stats: Arc<RenderingStats>,
    },
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
    /// A layer or a drawable failed to render.
    RenderError(Error),
}

/// The queue a renderer pushes its events to.
pub type RendererEvents = EventQueue<RendererEvent>;

// A renderer on another thread than its map sends it the events.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<RendererEvent>();
};

/// The queue that the renderer's parts push to, once the renderer has one.
#[derive(Clone, Default)]
pub(crate) struct EventSink(Rc<RefCell<Option<RendererEvents>>>);

impl EventSink {
    pub(crate) fn set(&self, events: Option<RendererEvents>) {
        *self.0.borrow_mut() = events;
    }

    pub(crate) fn emit(&self, event: RendererEvent) {
        let events = self.0.borrow().clone();
        if let Some(events) = events {
            events.push(event);
        }
    }
}
