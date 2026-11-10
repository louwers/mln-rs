// Port-only: a raster or raster-dem tile worker's replies, in the order of
// the parses that make them.
//
// C++ `RasterTileWorker::parse` and `RasterDEMTileWorker::parse` decode
// their image synchronously, so a tile gets its worker's replies in the
// order it sent its data. Here the image decodes through
// `mln_platform_default_image::decoder`, whose decoder may answer later
// (the browser's), so two of a worker's images may finish in either order.
// Each parse takes a `ReplySlot` as it starts, and a reply waits for those
// of the slots taken before it. With a decoder that answers before
// returning, every reply goes out as soon as it's sent, as in C++.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, Weak};

use mln_util::sync::MutexExt;

use crate::tile::tile_worker_transport::TileEndpoint;

/// A worker's tile, which gets the replies of the worker's `ReplySlot`s in
/// the order they were taken.
pub struct OrderedReplies<M> {
    state: Arc<Mutex<State<M>>>,
}

struct State<M> {
    parent: Box<dyn TileEndpoint<M>>,
    /// The slots taken and not yet sent, oldest first.
    slots: VecDeque<Slot<M>>,
    /// The number of the oldest slot in `slots`.
    first: u64,
}

enum Slot<M> {
    Waiting,
    Ready(M),
    /// Dropped without a reply.
    Empty,
}

/// The place of one reply among its worker's.
pub struct ReplySlot<M> {
    /// Gone once the worker is: its replies go nowhere, as a closed
    /// mailbox's.
    state: Weak<Mutex<State<M>>>,
    number: u64,
    filled: bool,
}

impl<M> OrderedReplies<M> {
    pub fn new(parent: Box<dyn TileEndpoint<M>>) -> Self {
        OrderedReplies { state: Arc::new(Mutex::new(State { parent, slots: VecDeque::new(), first: 0 })) }
    }

    /// The slot of the next reply.
    pub fn slot(&self) -> ReplySlot<M> {
        let mut state = self.state.lock_unpoisoned();
        let number = state.first + state.slots.len() as u64;
        state.slots.push_back(Slot::Waiting);
        ReplySlot { state: Arc::downgrade(&self.state), number, filled: false }
    }
}

impl<M> ReplySlot<M> {
    /// Sends `message` to the tile once the replies before it have gone.
    pub fn send(mut self, message: M) {
        self.fill(Slot::Ready(message));
    }

    fn fill(&mut self, slot: Slot<M>) {
        self.filled = true;
        let Some(state) = self.state.upgrade() else {
            return;
        };
        let mut state = state.lock_unpoisoned();
        let index = (self.number - state.first) as usize;
        state.slots[index] = slot;
        while !matches!(state.slots.front(), None | Some(Slot::Waiting)) {
            state.first += 1;
            if let Some(Slot::Ready(message)) = state.slots.pop_front() {
                state.parent.send(message);
            }
        }
    }
}

impl<M> Drop for ReplySlot<M> {
    fn drop(&mut self) {
        if !self.filled {
            self.fill(Slot::Empty);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Recorder(Arc<Mutex<Vec<u32>>>);

    impl TileEndpoint<u32> for Recorder {
        fn send(&self, message: u32) {
            self.0.lock_unpoisoned().push(message);
        }
    }

    #[test]
    fn replies_keep_the_order_of_their_slots() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let replies = OrderedReplies::new(Box::new(Recorder(sent.clone())));
        replies.slot().send(1);
        assert_eq!(*sent.lock_unpoisoned(), [1]);

        let (a, b, c, d) = (replies.slot(), replies.slot(), replies.slot(), replies.slot());
        c.send(4);
        b.send(3);
        assert_eq!(*sent.lock_unpoisoned(), [1]);
        a.send(2);
        assert_eq!(*sent.lock_unpoisoned(), [1, 2, 3, 4]);
        drop(d);
        replies.slot().send(5);
        assert_eq!(*sent.lock_unpoisoned(), [1, 2, 3, 4, 5]);
    }

    #[test]
    fn replies_after_the_worker_go_nowhere() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let replies = OrderedReplies::new(Box::new(Recorder(sent.clone())));
        let slot = replies.slot();
        drop(replies);
        slot.send(1);
        assert!(sent.lock_unpoisoned().is_empty());
    }
}
