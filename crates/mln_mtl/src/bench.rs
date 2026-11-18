//! How long the GPU took, for the benchmarks (`tools/bench`) to measure:
//! the time from each render pass's vertex stage starting to its fragment
//! stage ending, summed over every pass, from timestamps that a counter
//! sample buffer takes at the passes' stage boundaries. Measured once a
//! process calls [`measure_render_passes`], on the backends made after,
//! where the device samples timestamps there, as Apple GPUs do. A build with
//! the feature in which no one calls it, such as a workspace's tests,
//! measures nothing. Built only with the `bench` feature, and hidden from
//! the documentation: it isn't API.

use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use objc2::Message;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSRange;
use objc2_metal::{
    MTLCommonCounterSetTimestamp, MTLCounterSampleBuffer, MTLCounterSampleBufferDescriptor, MTLCounterSamplingPoint,
    MTLCounterSet, MTLDevice, MTLRenderPassDescriptor, MTLStorageMode, MTLTimestamp,
};
use parking_lot::Mutex;

/// The passes one command buffer times at most; a pass past them goes
/// untimed.
const PASSES: usize = 1024;

/// What a sample that the GPU didn't take resolves to.
const ERROR_VALUE: u64 = u64::MAX;

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
/// where no one would read the time and the samples would only cost work.
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

type SampleBuffer = Retained<ProtocolObject<dyn MTLCounterSampleBuffer>>;

/// A backend's sample buffers: one for each command buffer in flight, with
/// the beginning and end of each of its render passes.
pub(crate) struct Timer {
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    timestamps: Retained<ProtocolObject<dyn MTLCounterSet>>,
    /// The CPU's and the GPU's timestamps when the timer was made, from
    /// which a GPU tick's nanoseconds follow.
    calibration: (MTLTimestamp, MTLTimestamp),
    /// The sample buffer of the command buffer being encoded, and the
    /// passes it has begun.
    recording: Option<(SampleBuffer, usize)>,
    /// The sample buffers of committed command buffers, by number, and how
    /// many passes each timed.
    in_flight: Vec<(u64, SampleBuffer, usize)>,
    free: Vec<SampleBuffer>,
}

impl Timer {
    /// The timer of a backend on `device`, if the process measures and the
    /// device samples timestamps at the stages' boundaries.
    pub(crate) fn new(device: &ProtocolObject<dyn MTLDevice>) -> Option<Timer> {
        if !COUNTING.load(Ordering::Relaxed)
            || !device.supportsCounterSampling(MTLCounterSamplingPoint::AtStageBoundary)
        {
            return None;
        }
        // SAFETY: a static that Metal defines.
        let name = unsafe { MTLCommonCounterSetTimestamp };
        let timestamps = device.counterSets()?.iter().find(|set| &*set.name() == name)?;
        TIME.lock().get_or_insert_default();
        Some(Timer {
            device: device.retain(),
            timestamps,
            calibration: sample_timestamps(device),
            recording: None,
            in_flight: Vec::new(),
            free: Vec::new(),
        })
    }

    /// Has the pass of `descriptor` sample its beginning and end, into the
    /// sample buffer of the command buffer being encoded.
    pub(crate) fn time(&mut self, descriptor: &MTLRenderPassDescriptor) {
        if self.recording.is_none() {
            let buffer = self.free.pop().or_else(|| {
                let buffer_descriptor = MTLCounterSampleBufferDescriptor::new();
                buffer_descriptor.setCounterSet(Some(&self.timestamps));
                buffer_descriptor.setStorageMode(MTLStorageMode::Shared);
                // SAFETY: two samples a pass, of 8 bytes each, fit in the
                // 32 KiB that Metal allows a sample buffer.
                unsafe { buffer_descriptor.setSampleCount(2 * PASSES) };
                self.device.newCounterSampleBufferWithDescriptor_error(&buffer_descriptor).ok()
            });
            let Some(buffer) = buffer else { return };
            self.recording = Some((buffer, 0));
        }
        let Some((buffer, passes)) = self.recording.as_mut().filter(|(_, passes)| *passes < PASSES) else { return };
        // SAFETY: index 0 is in the attachment array's bounds, and the
        // sample indexes in the buffer's; each is written once in the
        // command buffer. The other stage boundaries keep their default,
        // `MTLCounterDontSample`.
        unsafe {
            let attachment = descriptor.sampleBufferAttachments().objectAtIndexedSubscript(0);
            attachment.setSampleBuffer(Some(buffer));
            attachment.setStartOfVertexSampleIndex(2 * *passes);
            attachment.setEndOfFragmentSampleIndex(2 * *passes + 1);
        }
        *passes += 1;
    }

    /// Notes that the command buffer being encoded was committed as number
    /// `number`.
    pub(crate) fn committed(&mut self, number: u64) {
        if let Some((buffer, passes)) = self.recording.take() {
            self.in_flight.push((number, buffer, passes));
        }
    }

    /// Adds up the passes of the command buffers up to number `finished`,
    /// which have completed.
    pub(crate) fn finished(&mut self, finished: u64) {
        let done = self.in_flight.iter().take_while(|(number, ..)| *number <= finished).count();
        if done == 0 {
            return;
        }
        let mut ticks = 0;
        for (_, buffer, passes) in self.in_flight.drain(..done) {
            // SAFETY: the range is in the buffer's bounds, and the command
            // buffer that sampled into it has completed.
            let data = unsafe { buffer.resolveCounterRange(NSRange::new(0, 2 * passes)) };
            if let Some(data) = data {
                // SAFETY: Metal resolves each timestamp sample to a u64.
                let bytes = unsafe { data.as_bytes_unchecked() };
                for pass in bytes.as_chunks::<16>().0 {
                    let (start, end) = pass.split_at(8);
                    let start = u64::from_ne_bytes(start.try_into().unwrap_or_default());
                    let end = u64::from_ne_bytes(end.try_into().unwrap_or_default());
                    // A pass without draws has no vertex stage to sample.
                    if start != ERROR_VALUE && end != ERROR_VALUE && end > start && start != 0 {
                        ticks += end - start;
                    }
                }
            }
            self.free.push(buffer);
        }
        let (cpu_then, gpu_then) = self.calibration;
        let (cpu, gpu) = sample_timestamps(&self.device);
        // The CPU's timestamps are in nanoseconds, and the GPU's in ticks
        // of its own.
        let nanoseconds_per_tick = if gpu > gpu_then { (cpu - cpu_then) as f64 / (gpu - gpu_then) as f64 } else { 1.0 };
        let time = Duration::from_nanos((ticks as f64 * nanoseconds_per_tick) as u64);
        if let Some(total) = TIME.lock().as_mut() {
            *total += time;
        }
    }
}

/// The CPU's and the GPU's timestamps, sampled as close together as Metal
/// can.
fn sample_timestamps(device: &ProtocolObject<dyn MTLDevice>) -> (MTLTimestamp, MTLTimestamp) {
    let (mut cpu, mut gpu) = (0, 0);
    // SAFETY: both point to timestamps.
    unsafe { device.sampleTimestamps_gpuTimestamp(NonNull::from(&mut cpu), NonNull::from(&mut gpu)) };
    (cpu, gpu)
}
