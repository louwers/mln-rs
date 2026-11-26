//! The one channel through which what arrives for the tiles reaches the
//! render thread: file responses, the data of GeoJSON and custom sources,
//! the workers' replies and lost lanes, glyph responses, and the device fonts
//! chosen for clusters. Senders on any thread push events and wake the
//! renderer; the tile store takes them in when the renderer updates
//! (`TileStore::receive`).

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use mln_style::error::Error;

use crate::storage::Response;
use crate::text::font_stack::FontStack;
use crate::text::glyph::{FontFace, GlyphRange};
use crate::text::glyph_store::GlyphResult;
use crate::tile::TileData;
use crate::worker::job::{JobId, JobTimes, TileKey};
use crate::worker::tile_workers::{Lane, ReplyBody};
use crate::worker::worker_codec::RemoteEvent;

/// Asks the renderer for an update, from any thread.
pub(crate) type Waker = Arc<dyn Fn() + Send + Sync>;

/// Something that arrived for the tiles.
#[cfg_attr(
    target_pointer_width = "64",
    expect(clippy::large_enum_variant, reason = "a reply moves its layout once, from the worker to the tile")
)]
pub(crate) enum TileEvent {
    /// A response to the file request that tile load `load` made. It applies
    /// only while the tile still waits for that load.
    Response { tile: TileKey, load: u64, response: Response },
    /// What a GeoJSON or custom source gave the tile for its load `load`.
    Data { tile: TileKey, load: u64, data: SourceData },
    /// The reply to job `id`, which ran on `lane`.
    Reply { lane: Lane, id: JobId, body: ReplyBody, times: JobTimes },
    /// The worker of `lane` is gone, with every job it had.
    LaneLost(Lane),
    /// What a remote worker instance sent, for the store's workers to read.
    Remote(RemoteEvent),
    /// The answer to the glyph store's request `id`, for a glyph range. It
    /// applies only while the range still waits for that request.
    Glyphs { font_stack: FontStack, range: GlyphRange, id: u64, result: GlyphResult },
    /// The device's fonts for clusters of `font_stack` that the glyph store
    /// asked its chooser about. Each applies only while its cluster still
    /// waits for a choice.
    DeviceFonts { font_stack: FontStack, faces: Vec<(Vec<char>, Option<FontFace>)> },
}

/// What a GeoJSON or custom source gives a tile.
pub(crate) enum SourceData {
    /// The tile's data; `None` for a tile without any.
    Data(Option<TileData>),
    /// The source couldn't load the tile.
    Error(Error),
    /// The data the tile has is stale: it loads again when it is next
    /// needed.
    Stale,
}

/// The receiving end, which the tile store owns.
pub(crate) struct Inbox {
    sender: InboxSender,
    receiver: Receiver<TileEvent>,
}

impl Inbox {
    /// An inbox whose senders call `wake` after each event.
    pub(crate) fn new(wake: Waker) -> Self {
        let (sender, receiver) = channel();
        Inbox { sender: InboxSender { sender, wake }, receiver }
    }

    /// Where to send events from.
    pub(crate) fn sender(&self) -> &InboxSender {
        &self.sender
    }

    /// The oldest event not taken yet.
    pub(crate) fn next(&self) -> Option<TileEvent> {
        self.receiver.try_recv().ok()
    }
}

/// Sends events to an [`Inbox`], from any thread.
#[derive(Clone)]
pub(crate) struct InboxSender {
    sender: Sender<TileEvent>,
    wake: Waker,
}

impl InboxSender {
    /// Sends `event` and wakes the renderer, unless the inbox is gone.
    pub(crate) fn send(&self, event: TileEvent) {
        if self.sender.send(event).is_ok() {
            (self.wake)();
        }
    }
}
