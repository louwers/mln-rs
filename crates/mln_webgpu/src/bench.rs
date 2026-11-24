//! What the GPU ran and how long it took, for the benchmarks (`tools/bench`)
//! to measure, summed over every render pass: its vertex and fragment shader
//! invocations and the primitives that its clipper passed on, and the time
//! from each pass's beginning to its end. Measured once a process calls
//! [`measure_render_passes`], on the devices made after: their pipeline
//! statistics where the device has those queries (Vulkan, D3D12), and their
//! time where it has timestamp queries (Vulkan, D3D12, Metal), which the
//! library's device then asks for, unless the process leaves them untimed
//! ([`leave_render_passes_untimed`]). A build with the feature in which no one
//! calls it, such as a workspace's tests, measures nothing. Built only with
//! the `bench` feature, and hidden from the documentation: it isn't API.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;

/// What the GPU's pipeline ran.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PipelineStatistics {
    pub vertex_shader_invocations: u64,
    /// The primitives the clipper passed on to rasterization.
    pub primitives: u64,
    pub fragment_shader_invocations: u64,
}

/// The device features that measuring asks for, where the adapter has them.
pub(crate) fn features() -> wgpu::Features {
    let mut features = wgpu::Features::empty();
    if COUNTING.load(Ordering::Relaxed) {
        features |= wgpu::Features::PIPELINE_STATISTICS_QUERY;
    }
    if TIMING.load(Ordering::Relaxed) {
        features |= wgpu::Features::TIMESTAMP_QUERY;
    }
    features
}

/// What each statistics query counts, in the order it writes them: by their
/// bits.
const TYPES: wgpu::PipelineStatisticsTypes = wgpu::PipelineStatisticsTypes::VERTEX_SHADER_INVOCATIONS
    .union(wgpu::PipelineStatisticsTypes::CLIPPER_PRIMITIVES_OUT)
    .union(wgpu::PipelineStatisticsTypes::FRAGMENT_SHADER_INVOCATIONS);
/// The values of one statistics query, of 8 bytes each.
const VALUES: u64 = 3;
/// The passes one submission measures at most; a pass past them goes
/// unmeasured.
const QUERIES: u32 = 1024;

/// Whether [`measure_render_passes`] was called.
static COUNTING: AtomicBool = AtomicBool::new(false);
/// Whether the devices made now time their render passes.
static TIMING: AtomicBool = AtomicBool::new(false);

/// Measures the render passes of the devices made from now on, for
/// [`take_pipeline_statistics`] and [`take_gpu_time`] to take after each
/// frame, which the caller must, since what isn't taken stays.
pub fn measure_render_passes() {
    COUNTING.store(true, Ordering::Relaxed);
    TIMING.store(true, Ordering::Relaxed);
}

/// Has the devices made from now on count their render passes' pipeline
/// statistics without timing them, where no one would read the time and the
/// timestamps would only cost work.
pub fn leave_render_passes_untimed() {
    TIMING.store(false, Ordering::Relaxed);
}

/// Whether the devices made now measure their render passes.
pub(crate) fn counting() -> bool {
    COUNTING.load(Ordering::Relaxed)
}

/// The measurements of submissions that the GPU hasn't finished, and the
/// sums of those it has that nobody has taken yet.
static COUNTED: Mutex<Counted> =
    Mutex::new(Counted { pending: Vec::new(), unresolved: Vec::new(), statistics: None, time: None, unknown: false });

struct Counted {
    pending: Vec<Pending>,
    /// The timestamps of submissions that the GPU hasn't finished, or
    /// that haven't been resolved since it did.
    unresolved: Vec<Unresolved>,
    /// `None` until a device has pipeline statistics queries.
    statistics: Option<PipelineStatistics>,
    /// `None` until a device has timestamp queries.
    time: Option<Duration>,
    /// Whether a pass's timestamps came back unreadable, which leaves the
    /// time unknown from then on.
    unknown: bool,
}

impl Counted {
    /// Adds up the submissions whose measurements have come back.
    fn collect(&mut self) {
        let (statistics, time, unknown) = (&mut self.statistics, &mut self.time, &mut self.unknown);
        self.pending.retain(|pending| {
            if !pending.mapped.load(Ordering::Acquire) {
                return true;
            }
            if let Ok(data) = pending.buffer.slice(..).get_mapped_range() {
                let values: Vec<u64> = data.as_chunks::<8>().0.iter().map(|bytes| u64::from_le_bytes(*bytes)).collect();
                if pending.statistics
                    && let Some(statistics) = statistics
                {
                    let queries = &values[..pending.queries as usize * VALUES as usize];
                    for query in queries.as_chunks::<{ VALUES as usize }>().0 {
                        statistics.vertex_shader_invocations += query[0];
                        statistics.primitives += query[1];
                        statistics.fragment_shader_invocations += query[2];
                    }
                }
                if let Some((offset, period)) = pending.timestamps
                    && let Some(time) = time
                {
                    let start = (offset / 8) as usize;
                    match pass_ticks(&values[start..start + 2 * pending.queries as usize]) {
                        Some(ticks) => *time += Duration::from_nanos((ticks as f64 * f64::from(period)).round() as u64),
                        None => *unknown = true,
                    }
                }
            }
            pending.buffer.unmap();
            false
        });
    }

    /// Takes the timestamps of the submissions that the GPU has finished,
    /// of `timer`'s device if given, else of every device.
    fn take_finished(&mut self, timer: Option<&Arc<Timer>>) -> Vec<Unresolved> {
        let (finished, waiting) = std::mem::take(&mut self.unresolved).into_iter().partition(|timed| {
            timed.done.load(Ordering::Acquire) && timer.is_none_or(|timer| Arc::ptr_eq(&timed.timer, timer))
        });
        self.unresolved = waiting;
        finished
    }

    /// Resolves the timestamps of the submissions that the GPU has
    /// finished, in a submission of each device's own, and waits for them
    /// to come back.
    fn resolve_finished(&mut self) {
        let mut finished = self.take_finished(None);
        while let Some(first) = finished.first() {
            let timer = Arc::clone(&first.timer);
            let (same, others): (Vec<_>, Vec<_>) =
                finished.into_iter().partition(|timed| Arc::ptr_eq(&timed.timer, &timer));
            finished = others;
            let mut encoder =
                timer.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("pass timestamps") });
            let resolved: Vec<Pending> = same.into_iter().map(|timed| timed.resolve(&mut encoder)).collect();
            let submission = timer.queue.submit([encoder.finish()]);
            self.pending.extend(resolved.into_iter().map(Pending::map));
            // Should the wait fail, the buffers that map later add up in a
            // later call.
            let _ = timer.device.poll(wgpu::PollType::Wait { submission_index: Some(submission), timeout: None });
        }
    }
}

/// The ticks that passes took, from each pass's timestamps at its
/// beginning and its end, in pairs: `None` if a pass ended before it
/// began, which leaves their time unknown. A pass of which the GPU took no
/// sample, as Metal takes none of a pass without draws, adds nothing: its
/// timestamps resolve to 0, or to `u64::MAX` (`MTLCounterErrorValue`).
fn pass_ticks(timestamps: &[u64]) -> Option<u64> {
    let sampled = |timestamp: &u64| *timestamp != 0 && *timestamp != u64::MAX;
    timestamps
        .as_chunks::<2>()
        .0
        .iter()
        .filter(|pass| pass.iter().all(sampled))
        .try_fold(0u64, |ticks, [begin, end]| Some(ticks + end.checked_sub(*begin)?))
}

/// A buffer for queries to resolve into, and one to read it back with.
fn resolve_buffers(device: &wgpu::Device, size: u64) -> (wgpu::Buffer, wgpu::Buffer) {
    let resolved = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("pass measurements"),
        size,
        usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("pass measurements read back"),
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    (resolved, buffer)
}

/// Measurements on their way back from the GPU.
struct Pending {
    buffer: wgpu::Buffer,
    /// The passes measured.
    queries: u32,
    /// Whether the buffer starts with each pass's pipeline statistics.
    statistics: bool,
    /// Where in the buffer each pass's timestamps begin, and the
    /// nanoseconds of one of their ticks, if the buffer has them.
    timestamps: Option<(u64, f32)>,
    /// The timestamp set that was resolved into the buffer, which goes
    /// back to its timer once the GPU has finished the resolve.
    set: Option<(Arc<Timer>, wgpu::QuerySet)>,
    mapped: Arc<AtomicBool>,
}

impl Pending {
    /// Maps the buffer once the GPU has finished the submission that
    /// copies into it.
    fn map(mut self) -> Pending {
        let mapped = Arc::clone(&self.mapped);
        let set = self.set.take();
        self.buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            if let Some((timer, set)) = set {
                timer.free.lock().push(set);
            }
            if result.is_ok() {
                mapped.store(true, Ordering::Release);
            }
        });
        self
    }
}

/// A submission's timestamps, which wait for the GPU to finish the
/// submission before they are resolved: a resolve in the submission itself
/// can read a pass's end before the GPU has written it, as Metal's does on
/// Apple GPUs after the M1.
struct Unresolved {
    timer: Arc<Timer>,
    set: wgpu::QuerySet,
    /// The passes timed.
    queries: u32,
    /// Whether the GPU has finished the submission.
    done: Arc<AtomicBool>,
}

impl Unresolved {
    /// Records the copy of the timestamps into a buffer, on `encoder` of
    /// their device, once the GPU has finished their submission.
    fn resolve(self, encoder: &mut wgpu::CommandEncoder) -> Pending {
        let size = u64::from(self.queries) * 2 * 8;
        let (resolved, buffer) = resolve_buffers(&self.timer.device, size);
        encoder.resolve_query_set(&self.set, 0..2 * self.queries, &resolved, 0);
        encoder.copy_buffer_to_buffer(&resolved, 0, &buffer, 0, size);
        Pending {
            buffer,
            queries: self.queries,
            statistics: false,
            timestamps: Some((0, self.timer.period)),
            set: Some((self.timer, self.set)),
            mapped: Arc::default(),
        }
    }
}

/// A device's timestamp query sets, and what resolving them takes.
struct Timer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// The nanoseconds of one tick.
    period: f32,
    /// The sets that no submission holds.
    free: Mutex<Vec<wgpu::QuerySet>>,
}

impl Timer {
    /// A set for a submission to write the timestamps of its passes into.
    fn query_set(&self) -> wgpu::QuerySet {
        self.free.lock().pop().unwrap_or_else(|| {
            self.device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("pass timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: 2 * QUERIES,
            })
        })
    }
}

/// The measurements a submission records, which
/// [`read_back`](Queries::read_back) takes once it's submitted: the copies
/// of its pipeline statistics and of the timestamps of earlier submissions
/// that the GPU has finished, and its own timestamps.
pub(crate) struct Measured {
    copies: Vec<Pending>,
    timestamps: Option<Unresolved>,
}

/// The pipeline statistics of everything the GPU has finished since the
/// last call, once the device has been polled for it, or `None` where no
/// device has the queries.
#[must_use]
pub fn take_pipeline_statistics() -> Option<PipelineStatistics> {
    let mut counted = COUNTED.lock();
    counted.collect();
    counted.statistics.as_mut().map(std::mem::take)
}

/// The time the GPU spent in the render passes it has finished since the
/// last call, once the device has been polled for them, or `None` where no
/// device times them, and from the first pass whose timestamps came back
/// unreadable on, which leaves the time unknown. The call waits for the
/// timestamps of the passes the GPU has finished to come back.
#[must_use]
pub fn take_gpu_time() -> Option<Duration> {
    let mut counted = COUNTED.lock();
    counted.resolve_finished();
    counted.collect();
    if counted.unknown {
        return None;
    }
    counted.time.as_mut().map(std::mem::take)
}

/// A backend's queries: one of each kind the device has for each render
/// pass of a submission.
pub(crate) struct Queries {
    statistics: Option<wgpu::QuerySet>,
    /// The device's timestamp sets, and the one that the open submission
    /// writes the beginning and end of each pass into.
    timestamps: Option<(Arc<Timer>, wgpu::QuerySet)>,
    /// The passes the open submission has begun.
    used: u32,
}

impl Queries {
    /// The pipeline statistics queries of `device`, if the process measures
    /// and the device has them: a host's own device may have them without
    /// the process measuring. [`timed`](Self::timed) adds timestamps.
    pub(crate) fn new(device: &wgpu::Device) -> Option<Queries> {
        if !counting() || !device.features().intersects(features()) {
            return None;
        }
        let statistics = device.features().contains(wgpu::Features::PIPELINE_STATISTICS_QUERY).then(|| {
            COUNTED.lock().statistics.get_or_insert_default();
            device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("pipeline statistics"),
                ty: wgpu::QueryType::PipelineStatistics(TYPES),
                count: QUERIES,
            })
        });
        Some(Queries { statistics, timestamps: None, used: 0 })
    }

    /// Times each pass too, where the device has timestamp queries and the
    /// process times the passes of the devices it makes now, with the writes
    /// that [`timestamp_writes`](Self::timestamp_writes) gives the pass.
    pub(crate) fn timed(mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> Queries {
        if TIMING.load(Ordering::Relaxed) && device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            COUNTED.lock().time.get_or_insert_default();
            let timer = Arc::new(Timer {
                device: device.clone(),
                queue: queue.clone(),
                period: queue.get_timestamp_period(),
                free: Mutex::new(Vec::new()),
            });
            let set = timer.query_set();
            self.timestamps = Some((timer, set));
        }
        self
    }

    /// The timestamps that the pass [`begin`](Self::begin) counts next
    /// writes at its beginning and end, if it is timed.
    pub(crate) fn timestamp_writes(&self) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        let (_, set) = self.timestamps.as_ref().filter(|_| self.used < QUERIES)?;
        Some(wgpu::RenderPassTimestampWrites {
            query_set: set,
            beginning_of_pass_write_index: Some(2 * self.used),
            end_of_pass_write_index: Some(2 * self.used + 1),
        })
    }

    /// [`timestamp_writes`](Self::timestamp_writes) for a compute pass,
    /// which [`begin_compute`](Self::begin_compute) counts.
    pub(crate) fn compute_timestamp_writes(&self) -> Option<wgpu::ComputePassTimestampWrites<'_>> {
        self.timestamp_writes().map(|writes| wgpu::ComputePassTimestampWrites {
            query_set: writes.query_set,
            beginning_of_pass_write_index: writes.beginning_of_pass_write_index,
            end_of_pass_write_index: writes.end_of_pass_write_index,
        })
    }

    /// [`begin`](Self::begin) for a compute pass, whose statistics count no
    /// vertex, primitive or fragment; [`end_compute`](Self::end_compute)
    /// stops them.
    pub(crate) fn begin_compute(&mut self, pass: &mut wgpu::ComputePass<'_>) -> bool {
        if self.used == QUERIES {
            return false;
        }
        if let Some(set) = &self.statistics {
            pass.begin_pipeline_statistics_query(set, self.used);
        }
        self.used += 1;
        self.statistics.is_some()
    }

    pub(crate) fn end_compute(pass: &mut wgpu::ComputePass<'_>) {
        pass.end_pipeline_statistics_query();
    }

    /// Counts `pass`, begun with [`timestamp_writes`](Self::timestamp_writes),
    /// and starts counting its pipeline statistics, which
    /// [`end`](Self::end) stops. Returns whether it does: where the device
    /// has the queries, and not past [`QUERIES`] passes.
    pub(crate) fn begin(&mut self, pass: &mut wgpu::RenderPass<'_>) -> bool {
        if self.used == QUERIES {
            return false;
        }
        if let Some(set) = &self.statistics {
            pass.begin_pipeline_statistics_query(set, self.used);
        }
        self.used += 1;
        self.statistics.is_some()
    }

    pub(crate) fn end(pass: &mut wgpu::RenderPass<'_>) {
        pass.end_pipeline_statistics_query();
    }

    /// Records, on the submission's `encoder`, the copies of its pipeline
    /// statistics and of the timestamps of the earlier submissions that the
    /// GPU has finished, and hands its own timestamps on, for
    /// [`read_back`](Self::read_back) to take once it's submitted.
    pub(crate) fn resolve(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder) -> Option<Measured> {
        let mut copies = Vec::new();
        if let Some((timer, _)) = &self.timestamps {
            let finished = COUNTED.lock().take_finished(Some(timer));
            copies.extend(finished.into_iter().map(|timed| timed.resolve(encoder)));
        }
        let queries = std::mem::take(&mut self.used);
        if let Some(set) = self.statistics.as_ref().filter(|_| queries > 0) {
            let size = u64::from(queries) * VALUES * 8;
            let (resolved, buffer) = resolve_buffers(device, size);
            encoder.resolve_query_set(set, 0..queries, &resolved, 0);
            encoder.copy_buffer_to_buffer(&resolved, 0, &buffer, 0, size);
            copies.push(Pending {
                buffer,
                queries,
                statistics: true,
                timestamps: None,
                set: None,
                mapped: Arc::default(),
            });
        }
        let timestamps = self.timestamps.as_mut().filter(|_| queries > 0).map(|(timer, set)| Unresolved {
            set: std::mem::replace(set, timer.query_set()),
            timer: Arc::clone(timer),
            queries,
            done: Arc::default(),
        });
        (!copies.is_empty() || timestamps.is_some()).then_some(Measured { copies, timestamps })
    }

    /// Takes the measurements of the submission made last, for
    /// [`take_pipeline_statistics`] and [`take_gpu_time`] to read once the
    /// GPU has finished it.
    pub(crate) fn read_back(measured: Measured) {
        let mut counted = COUNTED.lock();
        counted.pending.extend(measured.copies.into_iter().map(Pending::map));
        if let Some(timed) = measured.timestamps {
            let done = Arc::clone(&timed.done);
            timed.timer.queue.on_submitted_work_done(move || done.store(true, Ordering::Release));
            counted.unresolved.push(timed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::pass_ticks;

    #[test]
    fn passes_add_up_from_their_beginning_to_their_end() {
        assert_eq!(pass_ticks(&[100, 150, 200, 230]), Some(80));
        assert_eq!(pass_ticks(&[]), Some(0));
    }

    #[test]
    fn a_pass_the_gpu_took_no_sample_of_adds_nothing() {
        assert_eq!(pass_ticks(&[0, 0, 100, 150]), Some(50));
        assert_eq!(pass_ticks(&[u64::MAX, u64::MAX, 100, 150]), Some(50));
        assert_eq!(pass_ticks(&[u64::MAX, 120, 100, 150]), Some(50));
    }

    #[test]
    fn a_pass_that_ends_before_it_begins_leaves_the_time_unknown() {
        assert_eq!(pass_ticks(&[100, 150, 300, 200]), None);
    }
}
