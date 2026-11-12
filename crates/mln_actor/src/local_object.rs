// Thread-affine objects behind `Send` handles.
//
// C++ actors and reply closures reach their target through a raw pointer
// (`ActorRef<O>` holds `O*`, `scheduleAndReplyValue` captures `this`) that
// other threads pass around but only the owning thread dereferences, inside
// a task its scheduler runs. Safe Rust can't send such a pointer, so the
// owning thread keeps the object in a thread-local registry and other
// threads carry its key. Resolving a key on any other thread finds nothing.
//
// Not a C++ type; it is the mechanism behind `Mailbox` (actor/mailbox.cpp)
// and `scheduleAndReplyValue` (actor/scheduler.hpp).

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::{self, ThreadId};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static OBJECTS: RefCell<HashMap<u64, Rc<dyn Any>>> = RefCell::new(HashMap::new());
}

pub(crate) fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Registers `object` on the current thread under `id`.
pub(crate) fn insert(id: u64, object: Rc<dyn Any>) {
    let _ = OBJECTS.try_with(|objects| objects.borrow_mut().insert(id, object));
}

/// The object registered under `id` on the current thread.
pub(crate) fn get(id: u64) -> Option<Rc<dyn Any>> {
    OBJECTS.try_with(|objects| objects.borrow().get(&id).cloned()).ok().flatten()
}

/// Unregisters `id` on the current thread and returns its object.
pub(crate) fn remove(id: u64) -> Option<Rc<dyn Any>> {
    OBJECTS.try_with(|objects| objects.borrow_mut().remove(&id)).ok().flatten()
}

/// A `Send + Sync` handle to a value that lives on the thread that created
/// it. Only that thread can reach the value. Dropping the handle on that
/// thread drops the value (unless it was taken); dropped elsewhere, the
/// value lives until its thread exits.
pub struct LocalObject<T: 'static> {
    id: u64,
    thread: ThreadId,
    _type: PhantomData<fn() -> T>,
}

impl<T: 'static> LocalObject<T> {
    pub fn new(value: T) -> Self {
        Self::from_rc(Rc::new(value))
    }

    pub fn from_rc(value: Rc<T>) -> Self {
        let id = next_id();
        insert(id, value);
        LocalObject { id, thread: thread::current().id(), _type: PhantomData }
    }

    /// Whether the calling thread is the one that owns the value.
    #[must_use]
    pub fn is_home_thread(&self) -> bool {
        thread::current().id() == self.thread
    }

    /// The value, on its own thread.
    #[must_use]
    pub fn get(&self) -> Option<Rc<T>> {
        if !self.is_home_thread() {
            return None;
        }
        get(self.id).and_then(|object| object.downcast::<T>().ok())
    }

    /// Removes the value from its thread's registry and returns it.
    #[must_use]
    pub fn take(&self) -> Option<Rc<T>> {
        if !self.is_home_thread() {
            return None;
        }
        remove(self.id).and_then(|object| object.downcast::<T>().ok())
    }
}

impl<T: 'static> Drop for LocalObject<T> {
    fn drop(&mut self) {
        if self.is_home_thread() {
            drop(remove(self.id));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[test]
    fn only_the_home_thread_sees_the_value() {
        let object = LocalObject::new(Cell::new(7));
        assert_eq!(object.get().unwrap().get(), 7);
        let object = thread::spawn(move || {
            assert!(object.get().is_none());
            object
        })
        .join()
        .unwrap();
        assert_eq!(object.take().unwrap().get(), 7);
        assert!(object.get().is_none());
    }
}
