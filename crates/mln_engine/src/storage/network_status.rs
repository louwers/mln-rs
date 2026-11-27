//! Whether the network is reachable, as the host reports it.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use mln_style::collections::VecMap;
use parking_lot::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Status {
    Online,
    Offline,
}

/// What an observer runs when the network becomes reachable again.
type Observer = Arc<dyn Fn() + Send + Sync>;

/// The network's reachability, which the host sets and the online file
/// sources that share it follow: while it's offline, their requests fail
/// with a connection error without a transfer, and when it comes back
/// online, the requests that failed for want of a connection retry at once.
///
/// It starts online. Two statuses are equal only when they are the same
/// one.
pub struct NetworkStatus {
    online: AtomicBool,
    observers: Mutex<VecMap<u64, Observer>>,
    next_id: AtomicU64,
}

impl Default for NetworkStatus {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for NetworkStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetworkStatus").field("status", &self.get()).finish_non_exhaustive()
    }
}

impl PartialEq for NetworkStatus {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

impl Eq for NetworkStatus {}

impl NetworkStatus {
    /// A status that is online.
    #[must_use]
    pub const fn new() -> Self {
        NetworkStatus {
            online: AtomicBool::new(true),
            observers: Mutex::new(VecMap::new()),
            next_id: AtomicU64::new(1),
        }
    }

    #[must_use]
    pub fn get(&self) -> Status {
        if self.online.load(Ordering::SeqCst) { Status::Online } else { Status::Offline }
    }

    /// Sets the status. Going from offline to online tells the observers
    /// that the network is reachable again.
    pub fn set(&self, status: Status) {
        if status == Status::Offline {
            self.online.store(false, Ordering::SeqCst);
        } else if !self.online.swap(true, Ordering::SeqCst) {
            self.reachable();
        }
    }

    /// Runs `observer` each time the network becomes reachable, on the thread
    /// that reports it, until the returned subscription drops.
    pub(crate) fn subscribe(self: &Arc<Self>, observer: impl Fn() + Send + Sync + 'static) -> Subscription {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.observers.lock().insert(id, Arc::new(observer));
        Subscription { status: Arc::downgrade(self), id }
    }

    /// Tells the observers that the network is reachable, unless the status
    /// is offline.
    pub(crate) fn reachable(&self) {
        if !self.online.load(Ordering::SeqCst) {
            return;
        }
        // Not under the lock, so that an observer may subscribe or drop a
        // subscription.
        let observers: Vec<Observer> = self.observers.lock().values().cloned().collect();
        for observer in observers {
            observer();
        }
    }
}

/// An observer of a [`NetworkStatus`], which stops observing when this
/// drops.
pub(crate) struct Subscription {
    status: Weak<NetworkStatus>,
    id: u64,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(status) = self.status.upgrade() {
            status.observers.lock().remove(&self.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::*;

    fn counting(status: &Arc<NetworkStatus>) -> (Subscription, Arc<AtomicUsize>) {
        let count = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&count);
        let subscription = status.subscribe(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        (subscription, count)
    }

    #[test]
    fn going_back_online_tells_the_observers() {
        let status = Arc::new(NetworkStatus::new());
        let (_subscription, count) = counting(&status);
        assert_eq!(Status::Online, status.get());
        status.set(Status::Online);
        assert_eq!(0, count.load(Ordering::SeqCst));
        status.set(Status::Offline);
        assert_eq!(Status::Offline, status.get());
        assert_eq!(0, count.load(Ordering::SeqCst));
        status.set(Status::Online);
        assert_eq!(1, count.load(Ordering::SeqCst));
    }

    #[test]
    fn an_offline_status_reports_nothing_reachable() {
        let status = Arc::new(NetworkStatus::new());
        let (_subscription, count) = counting(&status);
        status.reachable();
        status.set(Status::Offline);
        status.reachable();
        assert_eq!(1, count.load(Ordering::SeqCst));
    }

    #[test]
    fn a_dropped_subscription_hears_nothing() {
        let status = Arc::new(NetworkStatus::new());
        let (subscription, count) = counting(&status);
        drop(subscription);
        status.reachable();
        assert_eq!(0, count.load(Ordering::SeqCst));
    }

    #[test]
    fn statuses_are_equal_only_to_themselves() {
        let (a, b) = (Arc::new(NetworkStatus::new()), Arc::new(NetworkStatus::new()));
        assert_eq!(a, Arc::clone(&a));
        assert_ne!(a, b);
    }
}
