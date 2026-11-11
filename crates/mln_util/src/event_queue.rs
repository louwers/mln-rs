//! A queue of events that producers push and one owner drains.
//!
//! Producers on any thread push events. The owner takes them all at once, in
//! the order they were pushed, when it's ready to handle them. An optional
//! waker tells the owner that the queue has events again.

use std::sync::{Arc, Mutex};

use crate::sync::MutexExt;

/// Tells the owner of a queue that the queue has events to take. It runs on
/// the thread that pushed the event, so it must not take the events itself
/// there: it schedules the owner to do it, as by sending an `AsyncTask`.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

struct Shared<T> {
    events: Mutex<Vec<T>>,
    waker: Mutex<Option<Waker>>,
}

/// A queue of events. Clones push to and take from the same queue.
pub struct EventQueue<T> {
    shared: Arc<Shared<T>>,
}

impl<T> Clone for EventQueue<T> {
    fn clone(&self) -> Self {
        EventQueue { shared: self.shared.clone() }
    }
}

impl<T> Default for EventQueue<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> EventQueue<T> {
    pub fn new() -> Self {
        EventQueue { shared: Arc::new(Shared { events: Mutex::new(Vec::new()), waker: Mutex::new(None) }) }
    }

    /// Adds `event` at the end of the queue. The waker runs when the queue
    /// was empty, after the queue is unlocked.
    pub fn push(&self, event: T) {
        let was_empty = {
            let mut events = self.shared.events.lock_unpoisoned();
            events.push(event);
            events.len() == 1
        };
        if was_empty {
            let waker = self.shared.waker.lock_unpoisoned().clone();
            if let Some(waker) = waker {
                waker();
            }
        }
    }

    /// Takes every event in the queue, oldest first.
    pub fn take(&self) -> Vec<T> {
        std::mem::take(&mut *self.shared.events.lock_unpoisoned())
    }

    pub fn is_empty(&self) -> bool {
        self.shared.events.lock_unpoisoned().is_empty()
    }

    /// Sets the waker that the queue runs when it gets an event while empty.
    /// A queue that already has events runs the new waker at once.
    pub fn set_waker(&self, waker: Option<Waker>) {
        *self.shared.waker.lock_unpoisoned() = waker.clone();
        if let Some(waker) = waker
            && !self.is_empty()
        {
            waker();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn takes_events_in_order() {
        let queue = EventQueue::new();
        queue.push(1);
        queue.clone().push(2);
        assert_eq!(queue.take(), vec![1, 2]);
        assert!(queue.is_empty());
    }

    #[test]
    fn wakes_once_per_batch() {
        let queue = EventQueue::new();
        let wakes = Arc::new(AtomicUsize::new(0));
        let counter = wakes.clone();
        queue.set_waker(Some(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })));
        queue.push(1);
        queue.push(2);
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        queue.take();
        queue.push(3);
        assert_eq!(wakes.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_new_waker_runs_for_queued_events() {
        let queue = EventQueue::new();
        queue.push(1);
        let wakes = Arc::new(AtomicUsize::new(0));
        let counter = wakes.clone();
        queue.set_waker(Some(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })));
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
    }
}
