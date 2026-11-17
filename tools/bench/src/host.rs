//! What the sessions measuring on one machine share: a lock that keeps timed
//! rounds apart from every other measurement, and the machine's load.
//!
//! Counts hold up while other processes run, so runs that judge only counts
//! share the lock and overlap freely. Times don't: timed rounds take the
//! lock alone, and wait for the measurements under way to finish. The lock
//! covers only measurements of `mln-bench`. Builds and tests of other
//! sessions still load the machine, which the load average shows.

#![cfg_attr(unix, expect(unsafe_code, reason = "getloadavg is FFI"))]

use std::fs::File;

use crate::fixtures::user_cache;

/// The machine-wide measuring lock, held until it drops, or until the
/// process ends however it ends.
pub(crate) struct Measuring {
    _lock: File,
}

/// Takes the lock that every measuring `mln-bench` of the user holds: alone
/// if `timed`, shared with other untimed measurements otherwise. Waits, and
/// says so, while a measurement that excludes this one runs.
pub(crate) fn measuring(timed: bool) -> Result<Measuring, String> {
    let dir = user_cache()?;
    std::fs::create_dir_all(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let path = dir.join("bench.lock");
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let taken = if timed { file.try_lock() } else { file.try_lock_shared() };
    if taken.is_err() {
        if timed {
            eprintln!("mln-bench: waiting for the machine's other benchmark runs to finish, to time alone");
            file.lock()
        } else {
            eprintln!("mln-bench: waiting for another session's timed rounds to finish");
            file.lock_shared()
        }
        .map_err(|error| format!("{}: {error}", path.display()))?;
    }
    Ok(Measuring { _lock: file })
}

/// The machine's load average over the last minute, where the system keeps
/// one: the processes running or waiting to run, this one's among them.
pub(crate) fn load_average() -> Option<f64> {
    #[cfg(unix)]
    {
        let mut load = [0.0f64; 1];
        // SAFETY: `load` holds the one sample the call is asked for.
        let samples = unsafe { libc::getloadavg(load.as_mut_ptr(), 1) };
        (samples == 1).then_some(load[0])
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// The load average above which times aren't tested, unless `--max-load`
/// names another: a quarter of the machine's cores busy with other work, and
/// at least one and a half.
pub(crate) fn default_max_load() -> f64 {
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    (cores as f64 / 4.0).max(1.5)
}
