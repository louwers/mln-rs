//! What the engine keeps of a tile's loading, beside its origin's state of
//! it. Each load takes a value of the source's counter, and its replies
//! arrive in the store's inbox naming it; the store hands one to the tile
//! only while that load is the tile's latest.

use std::any::Any;

use crate::storage::chrono::Timestamp;
use crate::storage::resource::RequestOrder;
use crate::tile_store::tile::{TileLoadOrder, TileNecessity};

/// What a tile's origin says of the times of its data: when it was
/// modified, when it expires, and until when it may be shown at all, which
/// the cache keeps to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TileTimes {
    /// When the data was last modified, if known.
    pub modified: Option<Timestamp>,
    /// When the data stops being fresh; `None` if it never does, or isn't
    /// known.
    pub expires: Option<Timestamp>,
    /// When the data may no longer be shown until its origin loads it
    /// again; `None` if it always may. A tile in the cache whose time has
    /// come goes as the view needs it again, rather than show its data,
    /// and a new tile loads in its place.
    pub usable_until: Option<Timestamp>,
}

impl TileTimes {
    /// Whether the data may still be shown at `now`.
    pub(crate) fn is_usable(&self, now: Timestamp) -> bool {
        self.usable_until.is_none_or(|until| until > now)
    }
}

/// A tile's loading: what the cover asks of it, its latest load, and its
/// origin's state of it.
pub(crate) struct Load {
    /// How much the cover last needed the tile.
    pub(crate) necessity: TileNecessity,
    /// When the tile's requests start among those waiting, shared with
    /// them.
    pub(crate) order: RequestOrder,
    /// The load whose replies apply.
    pub(crate) current: Option<u64>,
    /// The tile has done what it can while optional.
    pub(crate) tried_optional: bool,
    /// The origin's last word on the tile was that its load failed and
    /// waits for a retry
    /// ([`LoadContext::fail_retrying`](crate::source::LoadContext::fail_retrying)),
    /// and nothing has begun or brought anything since. Such a tile isn't
    /// loading: the cache lets go of it rather than wait for it.
    pub(crate) failed: bool,
    pub(crate) times: TileTimes,
    /// The origin's state of the tile: `None` until the origin starts the
    /// tile, and while one of its hooks has it.
    pub(crate) state: Option<Box<dyn Any>>,
}

impl Load {
    /// A new tile's loading, whose requests wait behind every other tile's
    /// until a cover retains it.
    pub(crate) fn new() -> Self {
        let order = RequestOrder::default();
        order.set(TileLoadOrder::UNWANTED.value());
        Load {
            necessity: TileNecessity::Optional,
            order,
            current: None,
            tried_optional: false,
            failed: false,
            times: TileTimes::default(),
            state: None,
        }
    }

    /// Whether the replies of load `load` apply.
    pub(crate) fn awaits(&self, load: u64) -> bool {
        self.current == Some(load)
    }

    /// Sets when the tile's network request starts among those waiting,
    /// including one that waits now.
    pub(crate) fn set_order(&self, order: TileLoadOrder) {
        self.order.set(order.value());
    }
}
