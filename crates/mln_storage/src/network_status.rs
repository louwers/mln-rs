// Observers are `util::AsyncTask*` in C++; here they are the tasks'
// `Send + Sync` senders, keyed by an id the subscriber keeps for
// `unsubscribe`.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use mln_actor::AsyncTaskSender;
use mln_vec_map::VecMap;
use parking_lot::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Status {
    Online,
    Offline,
}

static ONLINE: AtomicBool = AtomicBool::new(true);
static OBSERVERS: Mutex<VecMap<u64, AsyncTaskSender>> = Mutex::new(VecMap::new());
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub struct NetworkStatus;

impl NetworkStatus {
    pub fn get() -> Status {
        if ONLINE.load(Ordering::SeqCst) { Status::Online } else { Status::Offline }
    }

    /// `Set(Status)`
    pub fn set(status: Status) {
        if status == Status::Offline {
            ONLINE.store(false, Ordering::SeqCst);
        } else if !ONLINE.load(Ordering::SeqCst) {
            ONLINE.store(true, Ordering::SeqCst);
            Self::reachable();
        }
    }

    /// Sends to `sender` when the network becomes reachable. Returns the id
    /// for `unsubscribe`.
    pub fn subscribe(sender: AsyncTaskSender) -> u64 {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        OBSERVERS.lock().insert(id, sender);
        id
    }

    pub fn unsubscribe(id: u64) {
        OBSERVERS.lock().remove(&id);
    }

    pub fn reachable() {
        if !ONLINE.load(Ordering::SeqCst) {
            return;
        }
        for sender in OBSERVERS.lock().values() {
            sender.send();
        }
    }
}
