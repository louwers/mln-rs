//! Process exit with live runtimes.
//!
//! A process may exit while its runtimes and maps are live and the library's
//! threads are at work. Nothing at exit stops or joins those threads, so they
//! keep running until the operating system ends the process. Three rules keep
//! that safe:
//!
//! - Nothing that a library thread reads is destroyed at exit: Rust runs no
//!   destructors for statics.
//! - Nothing at exit waits for a library thread.
//! - Once exit begins, the library starts no host callback: each call into a
//!   host function checks [`exiting`] first and skips the call. A callback
//!   whose check passed just before exit began may still start.
//!
//! Graphics drivers are host state that the library can't keep alive: some
//! tear themselves down in exit handlers of their own. So a host ends every
//! render session's graphics calls, with an abandon, before it exits.

use std::sync::atomic::{AtomicBool, Ordering};

static EXITING: AtomicBool = AtomicBool::new(false);

/// Registers the exit handler that marks the start of process exit, once per
/// process. Called before the library first hands host callbacks to its
/// threads: when it creates a runtime, and when it installs the log callback.
/// The C runtime runs exit handlers in reverse order of registration, so this
/// one runs before every exit handler that the host registered earlier.
///
/// On Windows, a DLL's exit handlers run when it unloads at process exit, once
/// the operating system has ended every other thread, so no library thread is
/// left to start a callback by then.
pub(crate) fn watch() {
    #[cfg(not(target_family = "wasm"))]
    {
        extern "C" fn mark_exiting() {
            EXITING.store(true, Ordering::Release);
        }
        static WATCH: std::sync::Once = std::sync::Once::new();
        // If the registration fails, callbacks keep running through exit.
        // SAFETY: registers a function that only stores to an atomic.
        WATCH.call_once(|| unsafe {
            libc::atexit(mark_exiting);
        });
    }
}

/// Whether the process has begun to exit, so that the library starts no host
/// callback.
pub(crate) fn exiting() -> bool {
    EXITING.load(Ordering::Acquire)
}
