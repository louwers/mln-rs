//! What the engine keeps of a tile's loading, beside its origin's state of
//! it. Each load takes a value of the source's counter, and its replies
//! arrive in the store's inbox naming it; the store hands one to the tile
//! only while that load is the tile's latest.

use std::any::Any;

use mln_storage::RequestOrder;
use mln_util::chrono::Timestamp;

use crate::tile::tile::{TileLoadOrder, TileNecessity};

/// When a tile's data was modified and when it expires, as its origin says:
/// what the tile debug overlay shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TileTimes {
    pub modified: Option<Timestamp>,
    pub expires: Option<Timestamp>,
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
