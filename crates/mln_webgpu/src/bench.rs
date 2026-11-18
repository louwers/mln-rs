//! What the GPU ran and how long it took, for the benchmarks (`tools/bench`)
//! to measure, summed over every render pass: its vertex and fragment shader
//! invocations and the primitives that its clipper passed on, and the time
//! from each pass's beginning to its end. Measured once a process calls
//! [`measure_render_passes`], on the devices made after: their pipeline
//! statistics where the device has those queries (Vulkan, D3D12), and their
//! time where it has timestamp queries (Vulkan, D3D12, Metal), which the
//! library's device then asks for. A build with the feature in which no one
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
pub(crate) const FEATURES: wgpu::Features =
    wgpu::Features::PIPELINE_STATISTICS_QUERY.union(wgpu::Features::TIMESTAMP_QUERY);

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

/// Measures the render passes of the devices made from now on, for
/// [`take_pipeline_statistics`] and [`take_gpu_time`] to take after each
/// frame, which the caller must, since what isn't taken stays.
pub fn measure_render_passes() {
    COUNTING.store(true, Ordering::Relaxed);
}

/// Whether the devices made now measure their render passes.
pub(crate) fn counting() -> bool {
    COUNTING.load(Ordering::Relaxed)
}

/// The measurements of submissions that the GPU hasn't finished, and the
/// sums of those it has that nobody has taken yet.
static COUNTED: Mutex<Counted> = Mutex::new(Counted { pending: Vec::new(), statistics: None, time: None });

struct Counted {
    pending: Vec<Pending>,
    /// `None` until a device has pipeline statistics queries.
    statistics: Option<PipelineStatistics>,
    /// `None` until a device has timestamp queries.
    time: Option<Duration>,
}

impl Counted {
    /// Adds up the submissions whose measurements have come back.
    fn collect(&mut self) {
        let (statistics, time) = (&mut self.statistics, &mut self.time);
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
                    let passes = &values[start..start + 2 * pending.queries as usize];
                    let ticks: u64 =
                        passes.as_chunks::<2>().0.iter().map(|[begin, end]| end.saturating_sub(*begin)).sum();
                    *time += Duration::from_nanos((ticks as f64 * f64::from(period)).round() as u64);
                }
            }
            pending.buffer.unmap();
            false
        });
    }
}

/// A submission's measurements, on their way back from the GPU.
pub(crate) struct Pending {
    buffer: wgpu::Buffer,
    /// The passes measured.
    queries: u32,
    /// Whether the buffer starts with each pass's pipeline statistics.
    statistics: bool,
    /// Where in the buffer each pass's timestamps begin, and the
    /// nanoseconds of one of their ticks, if the passes were timed.
    timestamps: Option<(u64, f32)>,
    mapped: Arc<AtomicBool>,
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
/// device has timestamp queries.
#[must_use]
pub fn take_gpu_time() -> Option<Duration> {
    let mut counted = COUNTED.lock();
    counted.collect();
    counted.time.as_mut().map(std::mem::take)
}

/// A backend's queries: one of each kind the device has for each render
/// pass of a submission.
pub(crate) struct Queries {
    statistics: Option<wgpu::QuerySet>,
    /// The beginning and end of each pass, and the nanoseconds of one tick.
    timestamps: Option<(wgpu::QuerySet, f32)>,
    /// The passes the open submission has begun.
    used: u32,
}

impl Queries {
    /// The pipeline statistics queries of `device`, if the process measures
    /// and the device has them: a host's own device may have them without
    /// the process measuring. [`timed`](Self::timed) adds timestamps.
    pub(crate) fn new(device: &wgpu::Device) -> Option<Queries> {
        if !counting() || !device.features().intersects(FEATURES) {
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

    /// Times each pass too, where the device has timestamp queries, with
    /// the writes that [`timestamp_writes`](Self::timestamp_writes) gives
    /// the pass.
    pub(crate) fn timed(mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> Queries {
        if device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            COUNTED.lock().time.get_or_insert_default();
            let set = device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("pass timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: 2 * QUERIES,
            });
            self.timestamps = Some((set, queue.get_timestamp_period()));
        }
        self
    }

    /// The timestamps that the pass [`begin`](Self::begin) counts next
    /// writes at its beginning and end, if it is timed.
    pub(crate) fn timestamp_writes(&self) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        let (set, _) = self.timestamps.as_ref().filter(|_| self.used < QUERIES)?;
        Some(wgpu::RenderPassTimestampWrites {
            query_set: set,
            beginning_of_pass_write_index: Some(2 * self.used),
            end_of_pass_write_index: Some(2 * self.used + 1),
        })
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

    /// Records the copy of the submission's measurements into a buffer that
    /// [`read_back`](Self::read_back) maps once the GPU has finished.
    pub(crate) fn resolve(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder) -> Option<Pending> {
        let queries = std::mem::take(&mut self.used);
        if queries == 0 {
            return None;
        }
        let statistics_size = if self.statistics.is_some() { u64::from(queries) * VALUES * 8 } else { 0 };
        let timestamps_offset = statistics_size.next_multiple_of(wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT);
        let timestamps_size = if self.timestamps.is_some() { u64::from(queries) * 2 * 8 } else { 0 };
        let size = timestamps_offset + timestamps_size;
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
        if let Some(set) = &self.statistics {
            encoder.resolve_query_set(set, 0..queries, &resolved, 0);
        }
        if let Some((set, _)) = &self.timestamps {
            encoder.resolve_query_set(set, 0..2 * queries, &resolved, timestamps_offset);
        }
        encoder.copy_buffer_to_buffer(&resolved, 0, &buffer, 0, size);
        Some(Pending {
            buffer,
            queries,
            statistics: self.statistics.is_some(),
            timestamps: self.timestamps.as_ref().map(|&(_, period)| (timestamps_offset, period)),
            mapped: Arc::default(),
        })
    }

    /// Maps `pending`'s buffer once the GPU has finished its submission,
    /// for [`take_pipeline_statistics`] and [`take_gpu_time`] to read.
    pub(crate) fn read_back(pending: Pending) {
        let mapped = Arc::clone(&pending.mapped);
        pending.buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            if result.is_ok() {
                mapped.store(true, Ordering::Release);
            }
        });
        COUNTED.lock().pending.push(pending);
    }
}
