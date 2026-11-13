//! The library's thread for parsing and evaluating on behalf of a caller that
//! waits for the result, such as a standalone style expression.
//!
//! JSON and the expressions built from it recurse as deep as the input
//! nests, and a host's thread may have little stack: musl's default is 128
//! KiB. The work runs here instead, on a stack of a known size, one job at a
//! time and in the order the callers send them. The browser build runs it on
//! the caller's thread, its only one.

#[cfg(not(target_family = "wasm"))]
use std::sync::mpsc;

#[cfg(not(target_family = "wasm"))]
use parking_lot::Mutex;

use crate::error::Error;

#[cfg(not(target_family = "wasm"))]
type Job = Box<dyn FnOnce() + Send>;

/// The thread's queue, once it has started.
#[cfg(not(target_family = "wasm"))]
static JOBS: Mutex<Option<mpsc::Sender<Job>>> = Mutex::new(None);

#[cfg(not(target_family = "wasm"))]
fn jobs() -> Result<mpsc::Sender<Job>, Error> {
    let mut jobs = JOBS.lock();
    if let Some(jobs) = &*jobs {
        return Ok(jobs.clone());
    }
    let (sender, receiver) = mpsc::channel::<Job>();
    mln_util::thread::builder("mln-parse")
        .spawn(move || {
            for job in receiver {
                job();
            }
        })
        .map_err(|error| Error::native(format!("the parse thread didn't start: {error}")))?;
    *jobs = Some(sender.clone());
    Ok(sender)
}

/// Runs `work` on this thread: the browser's instance has no other, and
/// links with a stack that the parse limits fit in (build.rs).
#[cfg(target_family = "wasm")]
#[expect(clippy::unnecessary_wraps, reason = "the other builds' thread can fail to start")]
pub(crate) fn run<R: Send + 'static>(work: impl FnOnce() -> R + Send + 'static) -> Result<R, Error> {
    Ok(work())
}

/// Runs `work` on the parse thread and waits for its result.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn run<R: Send + 'static>(work: impl FnOnce() -> R + Send + 'static) -> Result<R, Error> {
    let (reply, result) = mpsc::sync_channel(1);
    let job: Job = Box::new(move || {
        // The caller waits for the reply, so it is there to receive it.
        let _ = reply.send(work());
    });
    if jobs()?.send(job).is_err() {
        // The thread ended, which only a panic in a job does: a bug, which
        // ends the process as it would have on the caller's thread.
        std::process::abort();
    }
    match result.recv() {
        Ok(value) => Ok(value),
        // As above: the job panicked before it replied.
        Err(mpsc::RecvError) => std::process::abort(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_runs_on_one_thread_in_order() {
        let names: Vec<_> = (0..3)
            .map(|index| run(move || (index, std::thread::current().name().map(str::to_owned))).unwrap())
            .collect();
        assert_eq!(names.iter().map(|(index, _)| *index).collect::<Vec<_>>(), [0, 1, 2]);
        assert!(names.iter().all(|(_, name)| name.as_deref() == Some("mln-parse")));
    }

    #[test]
    fn the_deepest_json_parses() {
        // serde_json's recursion limit.
        let depth = 127;
        let json = format!("{}1{}", "[".repeat(depth), "]".repeat(depth));
        let parsed = run(move || serde_json::from_str::<serde_json::Value>(&json).is_ok()).unwrap();
        assert!(parsed);
    }
}
