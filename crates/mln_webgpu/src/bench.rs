//! How much work the GPU's pipeline ran, for the benchmarks (`tools/bench`)
//! to count: its vertex and fragment shader invocations and the primitives
//! that its clipper passed on, summed over every render pass. Counted once a
//! process calls [`count_pipeline_statistics`], on the devices made after,
//! where the device has pipeline statistics queries (Vulkan, D3D12), which
//! the library's device then asks for. A build with the feature in which no
//! one calls it, such as a workspace's tests, counts nothing. Built only
//! with the `bench` feature, and hidden from the documentation: it isn't
//! API.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;

/// What the GPU's pipeline ran.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PipelineStatistics {
    pub vertex_shader_invocations: u64,
    /// The primitives the clipper passed on to rasterization.
    pub primitives: u64,
    pub fragment_shader_invocations: u64,
}

/// What each query counts, in the order it writes them: by their bits.
const TYPES: wgpu::PipelineStatisticsTypes = wgpu::PipelineStatisticsTypes::VERTEX_SHADER_INVOCATIONS
    .union(wgpu::PipelineStatisticsTypes::CLIPPER_PRIMITIVES_OUT)
    .union(wgpu::PipelineStatisticsTypes::FRAGMENT_SHADER_INVOCATIONS);
/// The values of one query, of 8 bytes each.
const VALUES: u64 = 3;
/// The passes one submission counts at most; a pass past them goes
/// uncounted.
const QUERIES: u32 = 1024;

/// Whether [`count_pipeline_statistics`] was called.
static COUNTING: AtomicBool = AtomicBool::new(false);

/// Counts the pipeline statistics of the devices made from now on, for
/// [`take_pipeline_statistics`] to take after each frame, which the caller
/// must, since what isn't taken stays.
pub fn count_pipeline_statistics() {
    COUNTING.store(true, Ordering::Relaxed);
}

/// Whether the devices made now count their pipeline statistics.
pub(crate) fn counting() -> bool {
    COUNTING.load(Ordering::Relaxed)
}

/// The statistics of submissions that the GPU hasn't finished, and the sum
/// of those it has and nobody has taken yet. `None` until a device has the
/// queries.
static COUNTED: Mutex<Option<Counted>> = Mutex::new(None);

#[derive(Default)]
struct Counted {
    pending: Vec<Pending>,
    finished: PipelineStatistics,
}

/// A submission's statistics, on their way back from the GPU.
pub(crate) struct Pending {
    buffer: wgpu::Buffer,
    queries: u32,
    mapped: Arc<AtomicBool>,
}

/// The pipeline statistics of everything the GPU has finished since the
/// last call, once the device has been polled for it, or `None` where no
/// device has the queries.
#[must_use]
pub fn take_pipeline_statistics() -> Option<PipelineStatistics> {
    let mut counted = COUNTED.lock();
    let counted = counted.as_mut()?;
    counted.pending.retain(|pending| {
        if !pending.mapped.load(Ordering::Acquire) {
            return true;
        }
        if let Ok(data) = pending.buffer.slice(..).get_mapped_range() {
            let values: Vec<u64> = data.as_chunks::<8>().0.iter().map(|bytes| u64::from_le_bytes(*bytes)).collect();
            for query in values.as_chunks::<{ VALUES as usize }>().0.iter().take(pending.queries as usize) {
                counted.finished.vertex_shader_invocations += query[0];
                counted.finished.primitives += query[1];
                counted.finished.fragment_shader_invocations += query[2];
            }
        }
        pending.buffer.unmap();
        false
    });
    Some(std::mem::take(&mut counted.finished))
}

/// A backend's queries: one for each render pass of a submission.
pub(crate) struct Queries {
    set: wgpu::QuerySet,
    /// The queries the open submission has begun.
    used: u32,
}

impl Queries {
    /// The queries of `device`, if it has pipeline statistics.
    pub(crate) fn new(device: &wgpu::Device) -> Option<Queries> {
        if !device.features().contains(wgpu::Features::PIPELINE_STATISTICS_QUERY) {
            return None;
        }
        COUNTED.lock().get_or_insert_with(Counted::default);
        let set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("pipeline statistics"),
            ty: wgpu::QueryType::PipelineStatistics(TYPES),
            count: QUERIES,
        });
        Some(Queries { set, used: 0 })
    }

    /// Starts counting the work of `pass`, which [`end`](Self::end) stops.
    /// Returns whether it does: not past [`QUERIES`] passes.
    pub(crate) fn begin(&mut self, pass: &mut wgpu::RenderPass<'_>) -> bool {
        if self.used == QUERIES {
            return false;
        }
        pass.begin_pipeline_statistics_query(&self.set, self.used);
        self.used += 1;
        true
    }

    pub(crate) fn end(pass: &mut wgpu::RenderPass<'_>) {
        pass.end_pipeline_statistics_query();
    }

    /// Records the copy of the submission's statistics into a buffer that
    /// [`read_back`](Self::read_back) maps once the GPU has finished.
    pub(crate) fn resolve(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder) -> Option<Pending> {
        let queries = std::mem::take(&mut self.used);
        if queries == 0 {
            return None;
        }
        let size = u64::from(queries) * VALUES * 8;
        let resolved = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pipeline statistics"),
            size,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pipeline statistics read back"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        encoder.resolve_query_set(&self.set, 0..queries, &resolved, 0);
        encoder.copy_buffer_to_buffer(&resolved, 0, &buffer, 0, size);
        Some(Pending { buffer, queries, mapped: Arc::default() })
    }

    /// Maps `pending`'s buffer once the GPU has finished its submission,
    /// for [`take_pipeline_statistics`] to read.
    pub(crate) fn read_back(pending: Pending) {
        let mapped = Arc::clone(&pending.mapped);
        pending.buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            if result.is_ok() {
                mapped.store(true, Ordering::Release);
            }
        });
        if let Some(counted) = COUNTED.lock().as_mut() {
            counted.pending.push(pending);
        }
    }
}
