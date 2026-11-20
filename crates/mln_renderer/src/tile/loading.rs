//! What the engine keeps of a tile's loading, beside its origin's
//! [`TileLoad`]. Each load takes a value of the source's counter, and its
//! replies arrive in the store's inbox naming it; the store hands one to the
//! tile only while that load is the tile's latest.

use mln_storage::RequestOrder;
use mln_util::chrono::Timestamp;

use crate::source::TileLoad;
use crate::tile::tile::{TileLoadOrder, TileNecessity, TileUpdateParameters};

/// When a tile's data was modified and when it expires, as its loads know,
/// and whether it came from the network rather than only the cache.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Freshness {
    pub modified: Option<Timestamp>,
    pub expires: Option<Timestamp>,
    pub from_network: bool,
}

impl Freshness {
    /// Whether the data came from the network and hasn't expired at `now`.
    /// Data without an expiry stays fresh.
    #[must_use]
    pub fn is_fresh(&self, now: Timestamp) -> bool {
        self.from_network && self.expires.is_none_or(|expires| expires > now)
    }
}

/// A tile's loading: what the cover asks of it, its latest load, what its
/// loads know of its data, and its origin's load.
pub(crate) struct Load {
    /// How much the cover last needed the tile.
    pub(crate) necessity: TileNecessity,
    pub(crate) update_parameters: TileUpdateParameters,
    /// When the tile's requests start among those waiting, shared with
    /// them.
    pub(crate) order: RequestOrder,
    /// The load whose replies apply.
    pub(crate) current: Option<u64>,
    /// The cache answered the tile.
    pub(crate) cache_answered: bool,
    pub(crate) freshness: Freshness,
    pub(crate) state: Box<dyn TileLoad>,
}

impl Load {
    /// A new tile's loading, whose origin's load is `state`.
    pub(crate) fn new(state: Box<dyn TileLoad>, order: RequestOrder) -> Self {
        Load {
            necessity: TileNecessity::Optional,
            update_parameters: TileUpdateParameters::default(),
            order,
            current: None,
            cache_answered: false,
            freshness: Freshness::default(),
            state,
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

/// The load of a tile whose origin's load is out for one of its hooks.
pub(crate) struct Detached;

impl TileLoad for Detached {}
