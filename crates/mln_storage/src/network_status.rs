// Ports include/mln/storage/network_status.hpp and
// src/mln/storage/network_status.cpp.
//
// Observers are `util::AsyncTask*` in C++; here they are the tasks'
// `Send + Sync` senders, keyed by an id the subscriber keeps for
// `unsubscribe`.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use mln_actor::AsyncTaskSender;
use mln_util::sync::MutexExt;

/// `NetworkStatus::Status`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Status {
    Online,
    Offline,
}

static ONLINE: AtomicBool = AtomicBool::new(true);
static OBSERVERS: Mutex<BTreeMap<u64, AsyncTaskSender>> = Mutex::new(BTreeMap::new());
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// `NetworkStatus`
pub struct NetworkStatus;

impl NetworkStatus {
    /// `Get()`
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

    /// `Subscribe(util::AsyncTask*)`: returns the id for `unsubscribe`.
    pub fn subscribe(sender: AsyncTaskSender) -> u64 {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        OBSERVERS.lock_unpoisoned().insert(id, sender);
        id
    }

    /// `Unsubscribe(util::AsyncTask*)`
    pub fn unsubscribe(id: u64) {
        OBSERVERS.lock_unpoisoned().remove(&id);
    }

    /// `Reachable()`
    pub fn reachable() {
        if !ONLINE.load(Ordering::SeqCst) {
            return;
        }
        for sender in OBSERVERS.lock_unpoisoned().values() {
            sender.send();
        }
    }
}
