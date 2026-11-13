//! The map's latest update for the renderer that draws it.
//!
//! The map publishes an [`UpdateParameters`] after each change that a frame
//! shows, and a renderer, on the map's thread or another, draws the latest
//! one it finds. An update replaces the one before it, whether or not a
//! renderer drew that one, and the slot keeps the latest while no renderer
//! reads it.
//!
//! A command group holds the updates back: while one is open, what the map
//! publishes waits, and the latest of it shows when the outermost group ends.
//! A hold, which the map's owner sets apart from the groups, does the same.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use mln_util::error::Error;
use mln_util::event_queue::Waker;
use parking_lot::Mutex;

use crate::update_parameters::UpdateParameters;

/// An update that a slot published.
#[derive(Clone)]
pub struct Update {
    /// Counts the updates that the slot published, this one included.
    pub generation: u64,
    pub parameters: Arc<UpdateParameters>,
}

#[derive(Default)]
struct State {
    generation: u64,
    latest: Option<Arc<UpdateParameters>>,
    /// The latest update published while a group or the hold was open.
    held: Option<Arc<UpdateParameters>>,
    group_depth: u32,
    hold: bool,
    closed: bool,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    /// The readers' wakers, by reader.
    wakers: Mutex<Vec<(u64, Waker)>>,
    next_reader: AtomicU64,
}

impl Shared {
    fn reader(self: &Arc<Self>) -> UpdateReader {
        UpdateReader { shared: self.clone(), id: self.next_reader.fetch_add(1, Ordering::Relaxed) }
    }

    /// Runs the wakers, with nothing locked.
    fn wake(&self) {
        let wakers: Vec<Waker> = self.wakers.lock().iter().map(|(_, waker)| waker.clone()).collect();
        for waker in wakers {
            waker();
        }
    }
}

/// The writing side, which the map owns. Dropping it closes the slot: its
/// readers see no update from then on.
#[derive(Default)]
pub struct UpdateSlot {
    shared: Arc<Shared>,
}

// The map and a renderer on another thread share the slot.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<UpdateSlot>();
    assert_send_sync::<UpdateReader>();
};

impl UpdateSlot {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A reader of this slot's updates.
    #[must_use]
    pub fn reader(&self) -> UpdateReader {
        self.shared.reader()
    }

    /// Makes `parameters` the latest update, or, while a group or the hold
    /// is open, the update that shows when the last of them ends.
    pub fn publish(&self, parameters: Arc<UpdateParameters>) {
        {
            let mut state = self.shared.state.lock();
            if state.group_depth > 0 || state.hold {
                state.held = Some(parameters);
                return;
            }
            Self::show(&mut state, parameters);
        }
        self.shared.wake();
    }

    fn show(state: &mut State, parameters: Arc<UpdateParameters>) {
        state.generation += 1;
        state.latest = Some(parameters);
    }

    /// Counts the updates that readers could see.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.shared.state.lock().generation
    }

    /// The generation of the update that shows what was published last:
    /// the current one, or while a group holds an update back, the one its
    /// end shows.
    #[must_use]
    pub fn pending_generation(&self) -> u64 {
        let state = self.shared.state.lock();
        state.generation + u64::from(state.held.is_some())
    }

    /// Opens a command group. Groups nest.
    pub fn begin_group(&self) {
        self.shared.state.lock().group_depth += 1;
    }

    /// Closes the innermost command group. When it was the outermost, the
    /// latest update published inside it shows. Fails with
    /// [`ErrorKind::InvalidState`](mln_util::ErrorKind) when no group is
    /// open.
    pub fn end_group(&self) -> Result<(), Error> {
        let shown = {
            let mut state = self.shared.state.lock();
            if state.group_depth == 0 {
                return Err(Error::invalid_state("the map has no open command group"));
            }
            state.group_depth -= 1;
            Self::show_held(&mut state)
        };
        if shown {
            self.shared.wake();
        }
        Ok(())
    }

    /// Shows the held update once neither a group nor the hold keeps it.
    fn show_held(state: &mut State) -> bool {
        if state.group_depth > 0 || state.hold {
            return false;
        }
        let Some(parameters) = state.held.take() else { return false };
        Self::show(state, parameters);
        true
    }

    /// Holds the updates back, as a group does, until
    /// [`release`](Self::release). It doesn't nest, and it doesn't count
    /// as a group.
    pub fn hold(&self) {
        self.shared.state.lock().hold = true;
    }

    /// Ends the hold: the latest update published during it shows, unless
    /// a group still holds it.
    pub fn release(&self) {
        let shown = {
            let mut state = self.shared.state.lock();
            state.hold = false;
            Self::show_held(&mut state)
        };
        if shown {
            self.shared.wake();
        }
    }

    /// How many command groups are open.
    #[must_use]
    pub fn group_depth(&self) -> u32 {
        self.shared.state.lock().group_depth
    }
}

impl Drop for UpdateSlot {
    fn drop(&mut self) {
        {
            let mut state = self.shared.state.lock();
            state.closed = true;
            state.latest = None;
            state.held = None;
        }
        self.shared.wake();
    }
}

/// The reading side of an [`UpdateSlot`], for the renderer that draws the
/// map. Clones read the same slot, each with a waker of its own.
pub struct UpdateReader {
    shared: Arc<Shared>,
    id: u64,
}

impl Clone for UpdateReader {
    /// Another reader of the same slot, without a waker.
    fn clone(&self) -> Self {
        self.shared.reader()
    }
}

impl Drop for UpdateReader {
    fn drop(&mut self) {
        self.set_waker(None);
    }
}

impl UpdateReader {
    /// The latest update, if the map published one and is still there.
    #[must_use]
    pub fn latest(&self) -> Option<Update> {
        let state = self.shared.state.lock();
        let parameters = state.latest.clone()?;
        Some(Update { generation: state.generation, parameters })
    }

    /// The generation of the latest update, or 0 before the first.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.shared.state.lock().generation
    }

    /// Whether the map that published to the slot is gone.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.shared.state.lock().closed
    }

    /// Sets the waker that runs when an update shows or the slot closes,
    /// until the reader drops. It runs on the map's thread, with the slot
    /// unlocked, and schedules the reader's work rather than drawing there.
    pub fn set_waker(&self, waker: Option<Waker>) {
        let mut wakers = self.shared.wakers.lock();
        wakers.retain(|(id, _)| *id != self.id);
        if let Some(waker) = waker {
            wakers.push((self.id, waker));
        }
    }

    /// Whether both read the same slot.
    #[must_use]
    pub fn ptr_eq(&self, other: &UpdateReader) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }
}
