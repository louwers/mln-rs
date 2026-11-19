//! How long the GPU took, for the benchmarks (`tools/bench`) to measure:
//! the time from each render pass's beginning to its end, summed over every
//! pass, from timestamp queries. Measured once a process calls
//! [`measure_render_passes`], on the backends made after, where the
//! device's queue writes timestamps. A build with the feature in which no
//! one calls it, such as a workspace's tests, measures nothing. Built only
//! with the `bench` feature, and hidden from the documentation: it isn't
//! API.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ash::vk;
use parking_lot::Mutex;

use crate::device::Device;

/// The passes one submission times at most; a pass past them goes untimed.
const PASSES: u32 = 1024;

/// Whether the backends made now time their render passes.
static COUNTING: AtomicBool = AtomicBool::new(false);

/// The time of the passes the GPU has finished that nobody has taken yet;
/// `None` until a backend times its passes.
static TIME: Mutex<Option<Duration>> = Mutex::new(None);

/// Times the render passes of the backends made from now on, for
/// [`take_gpu_time`] to take after each frame.
pub fn measure_render_passes() {
    COUNTING.store(true, Ordering::Relaxed);
}

/// Leaves the render passes of the backends made from now on untimed,
/// where no one would read the time and the queries would only cost work.
pub fn leave_render_passes_untimed() {
    COUNTING.store(false, Ordering::Relaxed);
}

/// The time the GPU spent in the render passes it has finished since the
/// last call, once the backend has waited for them, or `None` where no
/// backend times its passes.
#[must_use]
pub fn take_gpu_time() -> Option<Duration> {
    TIME.lock().as_mut().map(std::mem::take)
}

/// A backend's timestamp queries: a pool for each submission in flight,
/// with the beginning and end of each of its render passes.
pub(crate) struct Timer {
    /// The nanoseconds of one tick.
    period: f64,
    /// The bits of a timestamp that count.
    mask: u64,
    /// The pool of the batch being recorded, and the passes it has begun.
    recording: Option<(vk::QueryPool, u32)>,
    /// The pools of submissions, by serial, and how many passes each timed.
    in_flight: Vec<(u64, vk::QueryPool, u32)>,
    free: Vec<vk::QueryPool>,
    /// The timestamps read back last, whose memory the next read reuses.
    values: Vec<u64>,
}

impl Timer {
    /// The timer of a backend on `device`, if the process measures and the
    /// device's queue writes timestamps.
    pub(crate) fn new(device: &Device) -> Option<Timer> {
        if !COUNTING.load(Ordering::Relaxed) {
            return None;
        }
        let shared = &device.shared;
        // SAFETY: a plain query of the GPU.
        let families =
            unsafe { shared.instance().get_physical_device_queue_family_properties(shared.physical_device()) };
        let bits = families.get(shared.queue_family_index() as usize)?.timestamp_valid_bits;
        let period = shared.properties().limits.timestamp_period;
        if bits == 0 || period <= 0.0 {
            return None;
        }
        TIME.lock().get_or_insert_default();
        let mask = if bits >= 64 { u64::MAX } else { (1 << bits) - 1 };
        Some(Timer {
            period: f64::from(period),
            mask,
            recording: None,
            in_flight: Vec::new(),
            free: Vec::new(),
            values: Vec::new(),
        })
    }

    /// Readies the pool of the batch being recorded in `command_buffer`,
    /// which is outside a pass, taking one if the batch has none.
    pub(crate) fn prepare(&mut self, device: &ash::Device, command_buffer: vk::CommandBuffer) {
        if self.recording.is_some() {
            return;
        }
        let pool = if let Some(pool) = self.free.pop() {
            pool
        } else {
            let info = vk::QueryPoolCreateInfo::default().query_type(vk::QueryType::TIMESTAMP).query_count(2 * PASSES);
            // SAFETY: the create info is valid for the device.
            match unsafe { device.create_query_pool(&info, None) } {
                Ok(pool) => pool,
                Err(_) => return,
            }
        };
        // SAFETY: the pool's last submission has finished, and the command
        // buffer is recording outside a render pass.
        unsafe { device.cmd_reset_query_pool(command_buffer, pool, 0, 2 * PASSES) };
        self.recording = Some((pool, 0));
    }

    /// Writes the beginning of the pass that `command_buffer` began last,
    /// after [`prepare`](Self::prepare) while it was outside the pass.
    pub(crate) fn begin(&mut self, device: &ash::Device, command_buffer: vk::CommandBuffer) {
        if let Some((pool, passes)) = self.recording.filter(|&(_, passes)| passes < PASSES) {
            // SAFETY: the query was reset when the pool was taken, and is
            // written once in the batch.
            unsafe {
                device.cmd_write_timestamp(command_buffer, vk::PipelineStageFlags::TOP_OF_PIPE, pool, 2 * passes);
            }
        }
    }

    /// Writes the end of the pass that `command_buffer` ended last.
    pub(crate) fn end(&mut self, device: &ash::Device, command_buffer: vk::CommandBuffer) {
        if let Some((pool, passes)) = &mut self.recording
            && *passes < PASSES
        {
            // SAFETY: as in `begin`.
            unsafe {
                device.cmd_write_timestamp(
                    command_buffer,
                    vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                    *pool,
                    2 * *passes + 1,
                );
            }
            *passes += 1;
        }
    }

    /// Takes the batch's pool, which the submit of `serial` holds.
    pub(crate) fn submitted(&mut self, serial: u64) {
        if let Some((pool, passes)) = self.recording.take() {
            self.in_flight.push((serial, pool, passes));
        }
    }

    /// Adds up the passes of the submits up to `serial`, which have
    /// finished.
    pub(crate) fn finished(&mut self, device: &ash::Device, serial: u64) {
        let mut time = Duration::ZERO;
        let values = &mut self.values;
        self.in_flight.retain(|&(submitted, pool, passes)| {
            if submitted > serial {
                return true;
            }
            values.resize(2 * passes as usize, 0u64);
            // SAFETY: the pool's submission has finished, which wrote both
            // timestamps of each pass it counts.
            let read = unsafe { device.get_query_pool_results(pool, 0, values, vk::QueryResultFlags::TYPE_64) };
            if read.is_ok() {
                let ticks: u64 =
                    values.as_chunks::<2>().0.iter().map(|[begin, end]| end.wrapping_sub(*begin) & self.mask).sum();
                time += Duration::from_nanos((ticks as f64 * self.period).round() as u64);
            }
            self.free.push(pool);
            false
        });
        if let Some(total) = TIME.lock().as_mut() {
            *total += time;
        }
    }

    /// Destroys the pools, once the device has finished with them.
    pub(crate) fn destroy(&mut self, device: &ash::Device) {
        let pools = self.recording.take().map(|(pool, _)| pool).into_iter();
        for pool in pools.chain(self.in_flight.drain(..).map(|(_, pool, _)| pool)).chain(self.free.drain(..)) {
            // SAFETY: the device has finished the pool's submissions.
            unsafe { device.destroy_query_pool(pool, None) };
        }
    }
}
