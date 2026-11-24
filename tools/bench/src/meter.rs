//! The process-wide readings a measurement takes: the heap through
//! [`CountingAllocator`], which `main` installs; the instructions every
//! thread of the process retired, where the system counts them; and the
//! process's CPU time.

#![expect(unsafe_code, reason = "the allocator, the process CPU clock and its counters are FFI")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED: AtomicU64 = AtomicU64::new(0);
static LIVE: AtomicI64 = AtomicI64::new(0);
static PEAK: AtomicI64 = AtomicI64::new(0);

/// The system allocator, counting the allocations of every thread, the bytes
/// they ask for, and the most bytes live at once since [`reset_peak`]. A
/// reallocation counts as an allocation of its new size.
pub(crate) struct CountingAllocator;

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

/// What the heap has seen so far.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Heap {
    pub(crate) allocations: u64,
    pub(crate) allocated: u64,
    pub(crate) live: i64,
}

pub(crate) fn heap() -> Heap {
    Heap {
        allocations: ALLOCATIONS.load(Ordering::Relaxed),
        allocated: ALLOCATED.load(Ordering::Relaxed),
        live: LIVE.load(Ordering::Relaxed),
    }
}

/// Starts the peak over from the bytes live now.
pub(crate) fn reset_peak() {
    PEAK.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
}

/// The most heap bytes live at once since [`reset_peak`].
pub(crate) fn peak() -> i64 {
    PEAK.load(Ordering::Relaxed)
}

/// The CPU time of every thread of the process so far.
#[cfg(unix)]
pub(crate) fn process_cpu() -> Duration {
    let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `time` is a valid timespec for the call to fill.
    let status = unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &raw mut time) };
    assert_eq!(status, 0, "clock_gettime(CLOCK_PROCESS_CPUTIME_ID)");
    Duration::new(time.tv_sec as u64, time.tv_nsec as u32)
}

/// The CPU time of every thread of the process so far. Windows adds to it a
/// clock tick at a time (15.625 ms by default).
#[cfg(windows)]
pub(crate) fn process_cpu() -> Duration {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut creation, mut exit, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: the current process's pseudo handle needs no closing, and each
    // FILETIME is valid for the call to fill.
    let status = unsafe {
        GetProcessTimes(GetCurrentProcess(), &raw mut creation, &raw mut exit, &raw mut kernel, &raw mut user)
    };
    assert_ne!(status, 0, "GetProcessTimes");
    // FILETIME counts 100 ns intervals.
    let hundred_ns = |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    Duration::from_nanos((hundred_ns(kernel) + hundred_ns(user)) * 100)
}

/// Has the devices made from now on measure their render passes: the
/// pipeline statistics that `support::map` counts, and the time that
/// [`gpu_time`] reads.
pub(crate) fn measure_render_passes() {
    mln_webgpu::bench::measure_render_passes();
    #[cfg(all(mln_bench_vulkan, not(any(target_vendor = "apple", target_family = "wasm"))))]
    mln_vulkan::bench::measure_render_passes();
    #[cfg(mln_bench_gl)]
    mln_gl::bench::measure_render_passes();
    #[cfg(all(mln_bench_d3d12, windows))]
    mln_d3d12::bench::measure_render_passes();
    #[cfg(all(mln_bench_mtl, target_vendor = "apple"))]
    mln_mtl::bench::measure_render_passes();
}

/// Leaves the render passes of the devices made from now on untimed, for a
/// scenario that no GPU time would describe, so that it doesn't pay for the
/// timestamps. Their pipeline statistics stay counted.
pub(crate) fn leave_gpu_untimed() {
    mln_webgpu::bench::leave_render_passes_untimed();
    #[cfg(all(mln_bench_vulkan, not(any(target_vendor = "apple", target_family = "wasm"))))]
    mln_vulkan::bench::leave_render_passes_untimed();
    #[cfg(mln_bench_gl)]
    mln_gl::bench::leave_render_passes_untimed();
    #[cfg(all(mln_bench_d3d12, windows))]
    mln_d3d12::bench::leave_render_passes_untimed();
    #[cfg(all(mln_bench_mtl, target_vendor = "apple"))]
    mln_mtl::bench::leave_render_passes_untimed();
}

/// The time the GPU has spent in the render passes of the process's frames
/// so far, from timestamp queries, or `None` until a device that has them
/// is made: WebGPU's, and those of the native Vulkan, OpenGL, D3D12 and Metal backends
/// where the libraries time them (build.rs). A frame counts once the GPU has
/// finished it and the backend has waited for it, which waiting for a frame
/// does: a frame's time comes in the reading after that wait. `None` too
/// once a backend that timed its passes has lost track of their time.
pub(crate) fn gpu_time() -> Option<Duration> {
    /// The time so far, which backends have timed passes, and whether one
    /// of them lost track of the time.
    static TOTAL: Mutex<(Option<Duration>, Vec<bool>, bool)> = Mutex::new((None, Vec::new(), false));
    let (total, timed, unknown) = &mut *TOTAL.lock();
    let taken = [
        mln_webgpu::bench::take_gpu_time(),
        #[cfg(all(mln_bench_vulkan, not(any(target_vendor = "apple", target_family = "wasm"))))]
        mln_vulkan::bench::take_gpu_time(),
        #[cfg(mln_bench_gl)]
        mln_gl::bench::take_gpu_time(),
        #[cfg(all(mln_bench_d3d12, windows))]
        mln_d3d12::bench::take_gpu_time(),
        #[cfg(all(mln_bench_mtl, target_vendor = "apple"))]
        mln_mtl::bench::take_gpu_time(),
    ];
    timed.resize(taken.len(), false);
    for (time, timed) in taken.into_iter().zip(timed) {
        match time {
            Some(time) => {
                *timed = true;
                *total = Some(total.unwrap_or_default() + time);
            }
            // A backend that timed its passes gives no time once it lost
            // track of it.
            None => *unknown |= *timed,
        }
    }
    if *unknown { None } else { *total }
}

/// How the process counts instructions, or `None` where it can't: the
/// system has no counters for it, or doesn't let it read them.
pub(crate) fn instruction_counter() -> Option<&'static str> {
    #[cfg(target_os = "macos")]
    return instructions().map(|_| "proc_pid_rusage");
    #[cfg(any(target_os = "linux", target_os = "android"))]
    return instructions().map(|_| "perf_event_open");
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android")))]
    return None;
}

/// Opens the counters where they count only the threads started after them,
/// as on Linux, so `main` calls it before any thread starts.
pub(crate) fn start_counters() {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    linux::counters();
}

/// The instructions every thread of the process has retired so far, user
/// and kernel space.
#[cfg(target_os = "macos")]
pub(crate) fn instructions() -> Option<u64> {
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
    (status == 0 && info.ri_instructions > 0).then_some(info.ri_instructions)
}

/// The user-space instructions that the threads of the process have retired
/// since [`start_counters`]: the calling thread's and those of every thread
/// started after it. `None` where perf events aren't open to the process
/// (`perf_event_paranoid` above 2, or a container that blocks the call), or
/// where the hardware or a virtual machine leaves the counter at zero.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) fn instructions() -> Option<u64> {
    // Where the hardware or a virtual machine has no working PMU, the counter
    // opens but reads zero; a running process has always retired instructions.
    linux::counters().map(linux::Pmus::read).filter(|&count| count > 0)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android")))]
pub(crate) fn instructions() -> Option<u64> {
    None
}

/// Instructions through `perf_event_open(2)`.
#[cfg(any(target_os = "linux", target_os = "android"))]
mod linux {
    use std::fs::File;
    use std::io::Read as _;
    use std::os::fd::FromRawFd as _;
    use std::sync::OnceLock;

    /// `struct perf_event_attr` as of its first version
    /// (`PERF_ATTR_SIZE_VER0`), which every kernel with perf events takes.
    #[repr(C)]
    struct Attr {
        kind: u32,
        size: u32,
        config: u64,
        sample_period: u64,
        sample_type: u64,
        read_format: u64,
        flags: u64,
        wakeup_events: u32,
        bp_type: u32,
        config1: u64,
    }

    // linux/perf_event.h.
    const PERF_TYPE_HARDWARE: u32 = 0;
    const PERF_COUNT_HW_INSTRUCTIONS: u64 = 1;
    /// A hardware event's `config` names the PMU's type above these bits.
    const PERF_PMU_TYPE_SHIFT: u64 = 32;
    const FLAG_INHERIT: u64 = 1 << 1;
    const FLAG_EXCLUDE_KERNEL: u64 = 1 << 5;
    const FLAG_EXCLUDE_HV: u64 = 1 << 6;
    const PERF_FLAG_FD_CLOEXEC: libc::c_ulong = 1 << 3;

    /// A counter of the calling thread and of the threads it, and they,
    /// start from now on, in user space.
    fn open(config: u64) -> Option<File> {
        let attr = Attr {
            kind: PERF_TYPE_HARDWARE,
            size: size_of::<Attr>() as u32,
            config,
            sample_period: 0,
            sample_type: 0,
            read_format: 0,
            flags: FLAG_INHERIT | FLAG_EXCLUDE_KERNEL | FLAG_EXCLUDE_HV,
            wakeup_events: 0,
            bp_type: 0,
            config1: 0,
        };
        // SAFETY: `attr` is a perf_event_attr of the size it states, which
        // the kernel only reads; pid 0 and cpu -1 are the calling thread on
        // any CPU, and group -1 is none.
        let fd = unsafe { libc::syscall(libc::SYS_perf_event_open, &raw const attr, 0, -1, -1, PERF_FLAG_FD_CLOEXEC) };
        // SAFETY: a non-negative result is a new file descriptor that
        // nothing else owns.
        (fd >= 0).then(|| unsafe { File::from_raw_fd(fd as libc::c_int) })
    }

    /// The instruction counter of each kind of core.
    pub(super) struct Pmus(Vec<File>);

    impl Pmus {
        /// What the counters counted while they had the hardware, not
        /// scaled up for the time the kernel shared it with other events.
        pub(super) fn read(&self) -> u64 {
            self.0
                .iter()
                .map(|mut counter| {
                    let mut bytes = [0u8; 8];
                    counter.read_exact(&mut bytes).expect("a perf event's value");
                    u64::from_ne_bytes(bytes)
                })
                .sum()
        }
    }

    /// The counters, opened by the first call.
    pub(super) fn counters() -> Option<&'static Pmus> {
        static COUNTERS: OnceLock<Option<Pmus>> = OnceLock::new();
        COUNTERS
            .get_or_init(|| {
                // A CPU with two kinds of cores (Intel's P and E cores, Arm's
                // big.LITTLE) has a PMU for each, and a hardware event counts
                // on one PMU's cores only: count on each and add them up.
                let hybrid: Option<Vec<File>> = core_pmus()
                    .into_iter()
                    .map(|pmu| open((pmu << PERF_PMU_TYPE_SHIFT) | PERF_COUNT_HW_INSTRUCTIONS))
                    .collect();
                match hybrid {
                    Some(pmus) if !pmus.is_empty() => Some(Pmus(pmus)),
                    _ => open(PERF_COUNT_HW_INSTRUCTIONS).map(|counter| Pmus(vec![counter])),
                }
            })
            .as_ref()
    }

    /// The type of each PMU that counts some of the cores, where the kinds
    /// of cores have PMUs of their own; sysfs lists their CPUs.
    fn core_pmus() -> Vec<u64> {
        let Ok(devices) = std::fs::read_dir("/sys/bus/event_source/devices") else {
            return Vec::new();
        };
        devices
            .flatten()
            .filter(|device| device.path().join("cpus").exists())
            .filter_map(|device| std::fs::read_to_string(device.path().join("type")).ok()?.trim().parse().ok())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_allocations() {
        let before = heap();
        let data = std::hint::black_box(vec![0u8; 1000]);
        let after = heap();
        assert!(after.allocations > before.allocations);
        assert!(after.allocated - before.allocated >= 1000);
        drop(data);
    }
}
