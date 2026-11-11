//! What a case measures besides wall time: the process's CPU time, every
//! thread's, so work moved to the tile workers or a busy wait still counts;
//! and the heap allocations of the whole process, through
//! [`CountingAllocator`], which the `mln-bench` binary installs.
//!
//! A [`Meter`] times the timed part of each iteration of a case and leaves
//! its setup out.

// The allocator and the process CPU clock are the FFI here.
#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::Metrics;
use crate::stats::median;

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED: AtomicU64 = AtomicU64::new(0);
static LIVE: AtomicI64 = AtomicI64::new(0);
static PEAK: AtomicI64 = AtomicI64::new(0);

/// The system allocator, counting allocations, the bytes they ask for, and
/// the most bytes live at once since [`reset_peak`].
pub struct CountingAllocator;

// SAFETY: every call forwards to `System` with the caller's arguments.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size() as i64);
        // SAFETY: the caller's contract is `System::alloc`'s.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(layout.size() as i64);
        // SAFETY: as in `alloc`.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as i64, Ordering::Relaxed);
        // SAFETY: as in `alloc`.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        LIVE.fetch_sub(layout.size() as i64, Ordering::Relaxed);
        record(new_size as i64);
        // SAFETY: as in `alloc`.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

fn record(size: i64) {
    ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
    ALLOCATED.fetch_add(size as u64, Ordering::Relaxed);
    let live = LIVE.fetch_add(size, Ordering::Relaxed) + size;
    PEAK.fetch_max(live, Ordering::Relaxed);
}

/// Starts the peak over again from the bytes live now.
pub fn reset_peak() {
    PEAK.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
}

/// The most heap bytes live at once since [`reset_peak`], in MiB.
pub fn peak_heap_mib() -> f64 {
    PEAK.load(Ordering::Relaxed) as f64 / (1024.0 * 1024.0)
}

/// The heap bytes live now, in MiB.
pub fn live_heap_mib() -> f64 {
    LIVE.load(Ordering::Relaxed) as f64 / (1024.0 * 1024.0)
}

/// The CPU time of every thread of the process so far.
#[cfg(unix)]
pub fn process_cpu() -> Duration {
    let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `time` is a valid timespec for the call to fill.
    let status = unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut time) };
    assert_eq!(status, 0, "clock_gettime(CLOCK_PROCESS_CPUTIME_ID)");
    Duration::new(time.tv_sec as u64, time.tv_nsec as u32)
}

/// The CPU time of every thread of the process so far. Windows adds to it a
/// clock tick at a time (15.625 ms by default), so an iteration shorter than
/// a few ticks reads as zero or a whole tick.
#[cfg(windows)]
pub fn process_cpu() -> Duration {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut creation, mut exit, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: the current process's pseudo handle needs no closing, and each
    // FILETIME is valid for the call to fill.
    let status = unsafe { GetProcessTimes(GetCurrentProcess(), &mut creation, &mut exit, &mut kernel, &mut user) };
    assert_ne!(status, 0, "GetProcessTimes");
    // FILETIME counts 100 ns intervals.
    let hundred_ns = |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    Duration::from_nanos((hundred_ns(kernel) + hundred_ns(user)) * 100)
}

#[derive(Clone, Copy)]
struct Reading {
    wall: Instant,
    cpu: Duration,
    allocations: u64,
    allocated: u64,
}

impl Reading {
    fn now() -> Reading {
        Reading {
            wall: Instant::now(),
            cpu: process_cpu(),
            allocations: ALLOCATIONS.load(Ordering::Relaxed),
            allocated: ALLOCATED.load(Ordering::Relaxed),
        }
    }
}

/// The timed parts of a case's iterations. `time_ms`, `cpu_ms`, `allocs`
/// and `alloc_kib` are each iteration's median; `peak_heap_mib` is the
/// most heap live at once from the meter's creation on, setup between
/// iterations included.
pub struct Meter {
    started: Option<Reading>,
    wall: Vec<f64>,
    cpu: Vec<f64>,
    allocations: Vec<f64>,
    allocated: Vec<f64>,
}

impl Meter {
    /// A meter whose peak heap starts from what is live now, after the
    /// case's setup.
    pub fn new() -> Meter {
        reset_peak();
        Meter { started: None, wall: Vec::new(), cpu: Vec::new(), allocations: Vec::new(), allocated: Vec::new() }
    }

    /// Times `work` as one iteration.
    pub fn time<R>(&mut self, work: impl FnOnce() -> R) -> R {
        self.start();
        let result = work();
        self.stop();
        result
    }

    /// Starts an iteration, for one that [`Meter::time`] can't wrap.
    pub fn start(&mut self) {
        assert!(self.started.is_none(), "an iteration is already running");
        self.started = Some(Reading::now());
    }

    /// Ends the iteration [`Meter::start`] started.
    pub fn stop(&mut self) {
        let end = Reading::now();
        let start = self.started.take().expect("no iteration is running");
        self.wall.push(ms(end.wall - start.wall));
        self.cpu.push(ms(end.cpu - start.cpu));
        self.allocations.push((end.allocations - start.allocations) as f64);
        self.allocated.push((end.allocated - start.allocated) as f64 / 1024.0);
    }

    /// Each iteration's wall time so far, in ms.
    pub fn times(&self) -> &[f64] {
        &self.wall
    }

    /// The iterations' metrics; all but `alloc_kib` lead.
    pub fn finish(self) -> Metrics {
        let mut metrics = Metrics::default();
        metrics.lead("time_ms", median(&self.wall));
        metrics.lead("cpu_ms", median(&self.cpu));
        metrics.lead("allocs", median(&self.allocations));
        metrics.other("alloc_kib", median(&self.allocated));
        metrics.lead("peak_heap_mib", peak_heap_mib());
        metrics
    }
}

impl Default for Meter {
    fn default() -> Self {
        Meter::new()
    }
}

pub fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}
