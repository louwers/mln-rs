// Port-only: locking as C++ `std::mutex` does.
//
// C++ mutexes have no poisoning: after an exception unwinds out of a locked
// section, the next lock succeeds. A Rust lock is poisoned when a panic
// unwinds while it is held, and `lock().unwrap()` then panics on every later
// lock, on every thread. These recover the guard from a poisoned lock, as
// C++ would lock it.

use std::sync::{
    Condvar, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard, WaitTimeoutResult,
};
use std::time::Duration;

/// `std::mutex::lock` on a [`Mutex`].
pub trait MutexExt<T: ?Sized> {
    fn lock_unpoisoned(&self) -> MutexGuard<'_, T>;
    fn get_mut_unpoisoned(&mut self) -> &mut T;
}

impl<T: ?Sized> MutexExt<T> for Mutex<T> {
    fn lock_unpoisoned(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn get_mut_unpoisoned(&mut self) -> &mut T {
        self.get_mut().unwrap_or_else(PoisonError::into_inner)
    }
}

/// `std::shared_mutex::lock_shared` / `lock` on an [`RwLock`].
pub trait RwLockExt<T: ?Sized> {
    fn read_unpoisoned(&self) -> RwLockReadGuard<'_, T>;
    fn write_unpoisoned(&self) -> RwLockWriteGuard<'_, T>;
    fn get_mut_unpoisoned(&mut self) -> &mut T;
}

impl<T: ?Sized> RwLockExt<T> for RwLock<T> {
    fn read_unpoisoned(&self) -> RwLockReadGuard<'_, T> {
        self.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write_unpoisoned(&self) -> RwLockWriteGuard<'_, T> {
        self.write().unwrap_or_else(PoisonError::into_inner)
    }

    fn get_mut_unpoisoned(&mut self) -> &mut T {
        self.get_mut().unwrap_or_else(PoisonError::into_inner)
    }
}

/// `std::condition_variable::wait` / `wait_for` on a [`Condvar`].
pub trait CondvarExt {
    fn wait_unpoisoned<'a, T>(&self, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T>;
    fn wait_timeout_unpoisoned<'a, T>(
        &self,
        guard: MutexGuard<'a, T>,
        timeout: Duration,
    ) -> (MutexGuard<'a, T>, WaitTimeoutResult);
}

impl CondvarExt for Condvar {
    fn wait_unpoisoned<'a, T>(&self, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
        self.wait(guard).unwrap_or_else(PoisonError::into_inner)
    }

    fn wait_timeout_unpoisoned<'a, T>(
        &self,
        guard: MutexGuard<'a, T>,
        timeout: Duration,
    ) -> (MutexGuard<'a, T>, WaitTimeoutResult) {
        self.wait_timeout(guard, timeout).unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Condvar, Mutex, RwLock};
    use std::time::Duration;

    use super::*;

    #[test]
    fn locks_after_a_panic_while_held() {
        let mutex = Arc::new(Mutex::new(1));
        let rwlock = Arc::new(RwLock::new(2));
        {
            let (mutex, rwlock) = (mutex.clone(), rwlock.clone());
            let _ = std::thread::spawn(move || {
                let _m = mutex.lock().unwrap();
                let _w = rwlock.write().unwrap();
                panic!("poison");
            })
            .join();
        }
        assert!(mutex.is_poisoned() && rwlock.is_poisoned());
        assert_eq!(*mutex.lock_unpoisoned(), 1);
        assert_eq!(*rwlock.read_unpoisoned(), 2);
        *rwlock.write_unpoisoned() = 3;
        let guard = Condvar::new().wait_timeout_unpoisoned(mutex.lock_unpoisoned(), Duration::from_millis(1)).0;
        assert_eq!(*guard, 1);
    }
}
