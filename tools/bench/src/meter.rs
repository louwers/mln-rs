//! What a case measures besides wall time: the process's CPU time, every
//! thread's, so work moved to the tile workers or a busy wait still counts;
//! on macOS, the instructions and cycles of every thread, which other load
//! on the machine hardly changes; and the heap allocations of the whole
//! process, through [`CountingAllocator`], which the `mln-bench` binary
//! installs.
//!
//! A `Meter` times the timed part of each iteration of a case and leaves
//! its setup out.

// The allocator, the process CPU clock and its counters are the FFI here.
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
/// the most bytes live at once since `reset_peak`.
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
pub(crate) fn reset_peak() {
    PEAK.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
}

/// The most heap bytes live at once since [`reset_peak`], in MiB.
pub(crate) fn peak_heap_mib() -> f64 {
    PEAK.load(Ordering::Relaxed) as f64 / (1024.0 * 1024.0)
}

/// The heap bytes live now, in MiB.
pub(crate) fn live_heap_mib() -> f64 {
    LIVE.load(Ordering::Relaxed) as f64 / (1024.0 * 1024.0)
}

/// The CPU time of every thread of the process so far.
#[cfg(unix)]
#[must_use]
pub(crate) fn process_cpu() -> Duration {
    let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `time` is a valid timespec for the call to fill.
    let status = unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &raw mut time) };
    assert_eq!(status, 0, "clock_gettime(CLOCK_PROCESS_CPUTIME_ID)");
    Duration::new(time.tv_sec as u64, time.tv_nsec as u32)
}

/// The CPU time of every thread of the process so far. Windows adds to it a
/// clock tick at a time (15.625 ms by default), so an iteration shorter than
/// a few ticks reads as zero or a whole tick.
#[cfg(windows)]
pub(crate) fn process_cpu() -> Duration {
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

/// The instructions and cycles every thread of the process has retired so
/// far, or `None` where the system doesn't count them.
#[cfg(target_os = "macos")]
#[must_use]
pub(crate) fn process_counters() -> Option<Counters> {
    // SAFETY: rusage_info_v4 is plain integers, for which zero is valid.
    let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    // SAFETY: the flavor names rusage_info_v4, which `info` is; the call
    // takes it as a `rusage_info_t *`.
    let status = unsafe {
        libc::proc_pid_rusage(
            std::process::id() as libc::c_int,
            libc::RUSAGE_INFO_V4,
            (&raw mut info).cast::<libc::rusage_info_t>(),
        )
    };
    // Where the hardware or a virtual machine has no counters, they read
    // zero; a running process has always retired instructions.
    (status == 0 && info.ri_instructions > 0)
        .then_some(Counters { instructions: info.ri_instructions, cycles: info.ri_cycles })
}

/// The instructions and cycles every thread of the process has retired so
/// far, or `None` where the system doesn't count them.
#[cfg(not(target_os = "macos"))]
#[must_use]
pub(crate) fn process_counters() -> Option<Counters> {
    None
}

/// What [`process_counters`] reads.
#[derive(Clone, Copy)]
pub(crate) struct Counters {
    pub(crate) instructions: u64,
    pub(crate) cycles: u64,
}

#[derive(Clone, Copy)]
struct Reading {
    wall: Instant,
    cpu: Duration,
    counters: Option<Counters>,
    allocations: u64,
    allocated: u64,
}

impl Reading {
    fn now() -> Reading {
        Reading {
            wall: Instant::now(),
            cpu: process_cpu(),
            counters: process_counters(),
            allocations: ALLOCATIONS.load(Ordering::Relaxed),
            allocated: ALLOCATED.load(Ordering::Relaxed),
        }
    }
}

/// The timed parts of a case's iterations. `time_ms`, `cpu_ms`, `minstr`,
/// `mcycles`, `allocs` and `alloc_kib` are each iteration's median;
/// `peak_heap_mib` is the most heap live at once from the meter's creation
/// on, setup between iterations included. `minstr` and `mcycles` are left
/// out where [`process_counters`] has none.
pub(crate) struct Meter {
    started: Option<Reading>,
    wall: Vec<f64>,
    cpu: Vec<f64>,
    instructions: Vec<f64>,
    cycles: Vec<f64>,
    allocations: Vec<f64>,
    allocated: Vec<f64>,
}

impl Meter {
    /// A meter whose peak heap starts from what is live now, after the
    /// case's setup.
    #[must_use]
    pub(crate) fn new() -> Meter {
        reset_peak();
        Meter {
            started: None,
            wall: Vec::new(),
            cpu: Vec::new(),
            instructions: Vec::new(),
            cycles: Vec::new(),
            allocations: Vec::new(),
            allocated: Vec::new(),
        }
    }

    /// Times `work` as one iteration.
    pub(crate) fn time<R>(&mut self, work: impl FnOnce() -> R) -> R {
        self.start();
        let result = work();
        self.stop();
        result
    }

    /// Starts an iteration, for one that [`Meter::time`] can't wrap.
    pub(crate) fn start(&mut self) {
        assert!(self.started.is_none(), "an iteration is already running");
        self.started = Some(Reading::now());
    }

    /// Ends the iteration [`Meter::start`] started.
    pub(crate) fn stop(&mut self) {
        let end = Reading::now();
        let start = self.started.take().expect("no iteration is running");
        self.wall.push(ms(end.wall - start.wall));
        self.cpu.push(ms(end.cpu.checked_sub(start.cpu).unwrap()));
        if let (Some(start), Some(end)) = (start.counters, end.counters) {
            self.instructions.push((end.instructions - start.instructions) as f64 / 1e6);
            self.cycles.push((end.cycles - start.cycles) as f64 / 1e6);
        }
        self.allocations.push((end.allocations - start.allocations) as f64);
        self.allocated.push((end.allocated - start.allocated) as f64 / 1024.0);
    }

    /// Each iteration's wall time so far, in ms.
    #[must_use]
    pub(crate) fn times(&self) -> &[f64] {
        &self.wall
    }

    /// The iterations' metrics; all but `mcycles` and `alloc_kib` lead.
    #[must_use]
    pub(crate) fn finish(self) -> Metrics {
        let mut metrics = Metrics::default();
        metrics.lead("time_ms", median(&self.wall));
        metrics.lead("cpu_ms", median(&self.cpu));
        // Only when every iteration was counted, so the median is of them all.
        if !self.instructions.is_empty() && self.instructions.len() == self.wall.len() {
            metrics.lead("minstr", median(&self.instructions));
            metrics.other("mcycles", median(&self.cycles));
        }
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

#[must_use]
pub(crate) fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_instructions_where_the_system_does() {
        let mut meter = Meter::new();
        // At least one instruction per step, so at least 10 million.
        meter.time(|| (0..10_000_000u64).fold(0u64, |sum, i| std::hint::black_box(sum.wrapping_add(i))));
        let metrics = meter.finish();
        match process_counters() {
            Some(_) => {
                let instructions = metrics.get("minstr").expect("minstr").value;
                assert!(instructions >= 10.0, "{instructions} million instructions");
                assert!(metrics.get("mcycles").expect("mcycles").value > 0.0);
            }
            None => assert!(metrics.get("minstr").is_none() && metrics.get("mcycles").is_none()),
        }
    }
}
